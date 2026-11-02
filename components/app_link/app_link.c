// SPDX-License-Identifier: Apache-2.0
/* WebSocket link to Paseo over esp_websocket_client. Contract: README.md.
 *
 * Tasks and ownership:
 * - "app_link" owns the client handle: it creates, starts, closes and destroys
 *   clients and sends frames, in the order the public calls queued them.
 * - The client's own task ("app_link_ws") runs the connection and dispatches
 *   its events to on_ws_event(); a send error makes the client dispatch from
 *   "app_link" instead. The client's lock serialises both.
 * - Each connection gets a generation number, its events carry it, and only
 *   the current generation reaches the consumer: a replaced or closed
 *   connection is silent, and each connection reports on_close at most once.
 *   `lock` guards the generation, its open/ended flags and the callbacks.
 * - The reassembly buffer belongs to the running connection's event handler;
 *   "app_link" resets or frees it only while no client task exists. */
#include "app_link.h"

#include <errno.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#include "esp_check.h"
#include "esp_crt_bundle.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_tls_errors.h"
#include "esp_websocket_client.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "freertos/task.h"

#define QUEUE_LENGTH 16
/* Internal RAM stack of "app_link": client lifecycle and sends. */
#define TASK_STACK_BYTES 4096
/* The client task runs DNS, the TLS handshake (certificate bundle) and the
 * consumer's callbacks. Its unused stack is logged when it ends. */
#define WS_TASK_STACK_BYTES 6144
/* Below GSP's render task (4) and the logic (5). */
#define TASK_PRIORITY 3
/* The client's receive and send buffers; longer frames arrive in pieces. */
#define WS_BUFFER_BYTES 1024
/* DNS, TCP connect, TLS handshake and upgrade each give up after this. */
#define NETWORK_TIMEOUT_MS 10000
/* The closing handshake may take this long before the socket is dropped. */
#define CLOSE_WAIT_MS 1000
#define SEND_TIMEOUT_MS 5000
/* WebSocket pings from the client; liveness is the session's own ping. */
#define WS_PING_INTERVAL_S 30
/* Reassembled messages live in PSRAM and may grow to this size. */
#define MESSAGE_INITIAL_BYTES 4096
#define MESSAGE_MAX_BYTES (256 * 1024)
/* RFC 6455 close codes the link reports itself. */
#define CLOSE_NO_STATUS 1005
#define CLOSE_ABNORMAL 1006

static const char *TAG = "app_link";

typedef enum {
    CMD_OPEN,
    CMD_SEND,
    CMD_CLOSE,
    CMD_REAP,       /* a client task ended on its own: destroy its handle */
} cmd_kind_t;

typedef struct {
    cmd_kind_t kind;
    uint32_t gen;   /* CMD_REAP */
    char *data;     /* CMD_OPEN: NUL-terminated URL; CMD_SEND: text. Freed by the task. */
    size_t length;
} cmd_t;

static struct {
    QueueHandle_t queue;
    TaskHandle_t task;
    SemaphoreHandle_t lock;

    /* guarded by lock */
    uint32_t current;   /* generation whose events reach the consumer, 0 = none */
    bool opened;
    bool ended;
    app_link_open_cb_t on_open;
    app_link_text_cb_t on_text;
    app_link_close_cb_t on_close;
    void *ctx;

    /* "app_link" task only */
    esp_websocket_client_handle_t client;
    uint32_t client_gen;
    uint32_t next_gen;

    /* the running connection's event handler only */
    char *message;
    size_t message_length;
    size_t message_capacity;
    bool in_message;
    bool skip_message;
} s_link;

/* ---------------------------------------------------------------- delivery */

static const char *down_name(app_link_down_t why)
{
    static const char *const names[] = {"Dns", "Timeout", "Tls", "Refused", "Closed"};
    return (unsigned)why < sizeof(names) / sizeof(names[0]) ? names[why] : "?";
}

static bool is_current(uint32_t gen)
{
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    const bool current = gen == s_link.current && !s_link.ended;
    xSemaphoreGive(s_link.lock);
    return current;
}

static void deliver_open(uint32_t gen)
{
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    const bool deliver = gen == s_link.current && !s_link.ended && !s_link.opened;
    s_link.opened |= deliver;
    const app_link_open_cb_t on_open = s_link.on_open;
    void *ctx = s_link.ctx;
    xSemaphoreGive(s_link.lock);
    if (deliver) {
        ESP_LOGI(TAG, "#%lu open", (unsigned long)gen);
        if (on_open != NULL) {
            on_open(ctx);
        }
    }
}

static void deliver_close(uint32_t gen, app_link_down_t why, uint16_t code)
{
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    const bool deliver = gen == s_link.current && !s_link.ended;
    s_link.ended |= deliver;
    const app_link_close_cb_t on_close = s_link.on_close;
    void *ctx = s_link.ctx;
    xSemaphoreGive(s_link.lock);
    if (deliver) {
        ESP_LOGW(TAG, "#%lu down: %s(%u)", (unsigned long)gen, down_name(why), code);
        if (on_close != NULL) {
            on_close(why, code, ctx);
        }
    }
}

static void deliver_text(uint32_t gen, const char *text, size_t length)
{
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    const bool deliver = gen == s_link.current && !s_link.ended;
    const app_link_text_cb_t on_text = s_link.on_text;
    void *ctx = s_link.ctx;
    xSemaphoreGive(s_link.lock);
    if (deliver && on_text != NULL) {
        on_text(text, length, ctx);
    }
}

/* ---------------------------------------------------------------- events */

/* Appends a frame piece; false when the message outgrows the limit or memory. */
static bool message_append(const char *data, size_t length)
{
    const size_t needed = s_link.message_length + length;
    if (needed > MESSAGE_MAX_BYTES) {
        ESP_LOGE(TAG, "message over %u B dropped", (unsigned)MESSAGE_MAX_BYTES);
        return false;
    }
    if (needed > s_link.message_capacity) {
        size_t capacity = s_link.message_capacity ? s_link.message_capacity : MESSAGE_INITIAL_BYTES;
        while (capacity < needed) {
            capacity *= 2;
        }
        capacity = capacity < MESSAGE_MAX_BYTES ? capacity : MESSAGE_MAX_BYTES;
        char *grown = heap_caps_realloc(s_link.message, capacity, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
        if (grown == NULL) {
            ESP_LOGE(TAG, "no PSRAM for a %u B message; dropped", (unsigned)needed);
            return false;
        }
        s_link.message = grown;
        s_link.message_capacity = capacity;
    }
    memcpy(s_link.message + s_link.message_length, data, length);
    s_link.message_length = needed;
    return true;
}

static void on_data(uint32_t gen, const esp_websocket_event_data_t *event)
{
    const uint8_t opcode = event->op_code;
    if (opcode >= WS_TRANSPORT_OPCODES_CLOSE) {
        return; /* control frames; the client answers pings itself */
    }
    const bool frame_start = event->payload_offset == 0;
    if (frame_start && opcode != WS_TRANSPORT_OPCODES_CONT) {
        s_link.in_message = true;
        s_link.message_length = 0;
        s_link.skip_message = opcode != WS_TRANSPORT_OPCODES_TEXT;
        if (s_link.skip_message) {
            ESP_LOGW(TAG, "#%lu binary message ignored", (unsigned long)gen);
        }
    }
    if (!s_link.in_message) {
        return; /* a continuation without a start */
    }
    if (!s_link.skip_message && event->data_len > 0 &&
            !message_append(event->data_ptr, (size_t)event->data_len)) {
        s_link.skip_message = true;
    }
    const bool frame_end = event->payload_offset + event->data_len >= event->payload_len;
    if (frame_end && event->fin) {
        s_link.in_message = false;
        if (!s_link.skip_message) {
            deliver_text(gen, s_link.message, s_link.message_length);
        }
        s_link.message_length = 0;
    }
}

/* Reads a failed or lost connection as a Down reason. */
static void classify(const esp_websocket_event_data_t *event, bool opened,
                     app_link_down_t *why, uint16_t *code)
{
    const esp_websocket_error_codes_t *e = &event->error_handle;
    *code = 0;
    if (opened) {
        *why = APP_LINK_DOWN_CLOSED;
        *code = event->close_status_code > 0 ? (uint16_t)event->close_status_code : CLOSE_ABNORMAL;
        return;
    }
    const int status = e->esp_ws_handshake_status_code;
    const int sock = e->esp_transport_sock_errno;
    const esp_err_t tls = e->esp_tls_last_esp_err;
    ESP_LOGW(TAG, "connect failed: http=%d tls_err=0x%x tls_stack=-0x%x verify=0x%x errno=%d",
             status, (unsigned)tls, (unsigned) - e->esp_tls_stack_err,
             (unsigned)e->esp_tls_cert_verify_flags, sock);
    if (status >= 100 && status != 101) {
        *why = APP_LINK_DOWN_REFUSED;
        *code = (uint16_t)status;
    } else if (tls == ESP_ERR_ESP_TLS_CANNOT_RESOLVE_HOSTNAME) {
        *why = APP_LINK_DOWN_DNS;
    } else if (tls == ESP_ERR_ESP_TLS_CONNECTION_TIMEOUT || tls == ESP_ERR_ESP_TLS_SERVER_HANDSHAKE_TIMEOUT) {
        *why = APP_LINK_DOWN_TIMEOUT;
    } else if (sock == ECONNREFUSED || sock == ECONNRESET) {
        *why = APP_LINK_DOWN_REFUSED;
    } else if (e->esp_tls_cert_verify_flags != 0 || e->esp_tls_stack_err != 0 ||
               (tls >= ESP_ERR_MBEDTLS_CERT_PARTLY_OK && tls < ESP_ERR_ESP_TLS_BASE + 0x100)) {
        *why = APP_LINK_DOWN_TLS;
    } else if (tls == ESP_ERR_ESP_TLS_FAILED_CONNECT_TO_HOST || sock == ETIMEDOUT || sock == EAGAIN ||
               sock == EHOSTUNREACH || sock == ENETUNREACH) {
        *why = APP_LINK_DOWN_TIMEOUT;
    } else {
        *why = APP_LINK_DOWN_CLOSED;
        *code = CLOSE_ABNORMAL;
    }
}

static void on_ws_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    (void)base;
    const uint32_t gen = (uint32_t)(uintptr_t)arg;
    const esp_websocket_event_data_t *event = data;
    switch (id) {
    case WEBSOCKET_EVENT_CONNECTED:
        deliver_open(gen);
        break;
    case WEBSOCKET_EVENT_DATA:
        if (is_current(gen)) {
            on_data(gen, event);
        }
        break;
    case WEBSOCKET_EVENT_ERROR:
        if (event->data_ptr != NULL && event->data_len > 0) {
            ESP_LOGW(TAG, "#%lu %.*s", (unsigned long)gen, event->data_len, event->data_ptr);
        }
        break;
    case WEBSOCKET_EVENT_DISCONNECTED: {
        xSemaphoreTake(s_link.lock, portMAX_DELAY);
        const bool opened = s_link.opened && gen == s_link.current;
        xSemaphoreGive(s_link.lock);
        app_link_down_t why;
        uint16_t code;
        classify(event, opened, &why, &code);
        deliver_close(gen, why, code);
        break;
    }
    case WEBSOCKET_EVENT_CLOSED:
        deliver_close(gen, APP_LINK_DOWN_CLOSED,
                      event->close_status_code > 0 ? (uint16_t)event->close_status_code : CLOSE_NO_STATUS);
        break;
    case WEBSOCKET_EVENT_FINISH: {
        ESP_LOGI(TAG, "#%lu client task ends, stack_free=%u B", (unsigned long)gen,
                 (unsigned)uxTaskGetStackHighWaterMark(NULL));
        const cmd_t reap = {.kind = CMD_REAP, .gen = gen};
        (void)xQueueSend(s_link.queue, &reap, 0); /* a full queue: the next open or close reaps */
        break;
    }
    default:
        break;
    }
}

/* ---------------------------------------------------------------- the link task */

static void message_free(void)
{
    free(s_link.message);
    s_link.message = NULL;
    s_link.message_length = 0;
    s_link.message_capacity = 0;
    s_link.in_message = false;
    s_link.skip_message = false;
}

/* Destroys the client; waits for its task, so no handler runs afterwards. */
static void client_destroy(void)
{
    if (s_link.client == NULL) {
        return;
    }
    (void)esp_websocket_client_destroy(s_link.client);
    s_link.client = NULL;
    s_link.client_gen = 0;
    message_free();
}

/* Silences the current connection, then closes it cleanly if it is open. */
static void client_drop(void)
{
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    s_link.current = 0;
    xSemaphoreGive(s_link.lock);
    if (s_link.client != NULL && esp_websocket_client_is_connected(s_link.client)) {
        ESP_LOGI(TAG, "#%lu closing", (unsigned long)s_link.client_gen);
        (void)esp_websocket_client_close(s_link.client, pdMS_TO_TICKS(CLOSE_WAIT_MS));
    }
    client_destroy();
}

static void client_open(const char *url)
{
    if (++s_link.next_gen == 0) {
        s_link.next_gen = 1;
    }
    const uint32_t gen = s_link.next_gen;
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    s_link.current = gen;
    s_link.opened = false;
    s_link.ended = false;
    xSemaphoreGive(s_link.lock);
    message_free();

    const bool tls = strncmp(url, "wss://", 6) == 0;
    if (!tls && strncmp(url, "ws://", 5) != 0) {
        ESP_LOGE(TAG, "#%lu not a ws:// or wss:// URL", (unsigned long)gen);
        deliver_close(gen, APP_LINK_DOWN_CLOSED, CLOSE_ABNORMAL);
        return;
    }
    /* No Origin header and no subprotocol: Paseo refuses both. */
    const esp_websocket_client_config_t config = {
        .uri = url,
        .disable_auto_reconnect = true,   /* reconnecting is the session's call */
        .task_prio = TASK_PRIORITY,
        .task_name = "app_link_ws",
        .task_stack = WS_TASK_STACK_BYTES,
        .buffer_size = WS_BUFFER_BYTES,
        .crt_bundle_attach = tls ? esp_crt_bundle_attach : NULL,
        .network_timeout_ms = NETWORK_TIMEOUT_MS,
        .ping_interval_sec = WS_PING_INTERVAL_S,
        .disable_pingpong_discon = true,
    };
    s_link.client = esp_websocket_client_init(&config);
    esp_err_t err = s_link.client != NULL ? ESP_OK : ESP_ERR_NO_MEM;
    if (err == ESP_OK) {
        s_link.client_gen = gen;
        err = esp_websocket_register_events(s_link.client, WEBSOCKET_EVENT_ANY, on_ws_event,
                                            (void *)(uintptr_t)gen);
    }
    if (err == ESP_OK) {
        err = esp_websocket_client_start(s_link.client);
    }
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "#%lu client did not start: %s", (unsigned long)gen, esp_err_to_name(err));
        client_destroy();
        deliver_close(gen, APP_LINK_DOWN_CLOSED, CLOSE_ABNORMAL);
        return;
    }
    ESP_LOGI(TAG, "#%lu opening %s", (unsigned long)gen, url);
}

static void client_send(const char *text, size_t length)
{
    if (s_link.client == NULL || !esp_websocket_client_is_connected(s_link.client)) {
        ESP_LOGW(TAG, "send of %u B dropped: not open", (unsigned)length);
        return;
    }
    const int sent = esp_websocket_client_send_text(s_link.client, text, (int)length,
                                                    pdMS_TO_TICKS(SEND_TIMEOUT_MS));
    if (sent < 0) {
        /* The client aborts the connection and reports it. */
        ESP_LOGW(TAG, "#%lu send of %u B failed", (unsigned long)s_link.client_gen, (unsigned)length);
    }
}

static void link_task(void *arg)
{
    (void)arg;
    cmd_t cmd;
    for (;;) {
        if (xQueueReceive(s_link.queue, &cmd, portMAX_DELAY) != pdTRUE) {
            continue;
        }
        switch (cmd.kind) {
        case CMD_OPEN:
            client_drop();
            client_open(cmd.data);
            break;
        case CMD_SEND:
            client_send(cmd.data, cmd.length);
            break;
        case CMD_CLOSE:
            client_drop();
            break;
        case CMD_REAP:
            if (cmd.gen == s_link.client_gen) {
                client_destroy();
            }
            break;
        }
        free(cmd.data);
    }
}

/* ---------------------------------------------------------------- public calls */

esp_err_t app_link_start(void)
{
    ESP_RETURN_ON_FALSE(s_link.queue == NULL, ESP_ERR_INVALID_STATE, TAG, "already started");
    s_link.lock = xSemaphoreCreateMutex();
    s_link.queue = xQueueCreate(QUEUE_LENGTH, sizeof(cmd_t));
    if (s_link.lock == NULL || s_link.queue == NULL ||
            xTaskCreatePinnedToCore(link_task, "app_link", TASK_STACK_BYTES, NULL, TASK_PRIORITY,
                                    &s_link.task, tskNO_AFFINITY) != pdPASS) {
        if (s_link.queue != NULL) {
            vQueueDelete(s_link.queue);
            s_link.queue = NULL;
        }
        if (s_link.lock != NULL) {
            vSemaphoreDelete(s_link.lock);
            s_link.lock = NULL;
        }
        ESP_LOGE(TAG, "link unavailable: no memory");
        return ESP_ERR_NO_MEM;
    }
    ESP_LOGI(TAG, "idle until opened");
    return ESP_OK;
}

esp_err_t app_link_subscribe(app_link_open_cb_t on_open, app_link_text_cb_t on_text,
                             app_link_close_cb_t on_close, void *ctx)
{
    if (s_link.lock == NULL) {
        return ESP_ERR_INVALID_STATE;
    }
    xSemaphoreTake(s_link.lock, portMAX_DELAY);
    s_link.on_open = on_open;
    s_link.on_text = on_text;
    s_link.on_close = on_close;
    s_link.ctx = ctx;
    xSemaphoreGive(s_link.lock);
    return ESP_OK;
}

/* Queues a command, taking ownership of `data` (freed on failure too). */
static esp_err_t post(cmd_kind_t kind, char *data, size_t length)
{
    if (s_link.queue == NULL) {
        free(data);
        return ESP_ERR_INVALID_STATE;
    }
    const cmd_t cmd = {.kind = kind, .data = data, .length = length};
    if (xQueueSend(s_link.queue, &cmd, 0) != pdTRUE) {
        free(data);
        ESP_LOGW(TAG, "command dropped: queue full");
        return ESP_ERR_NO_MEM;
    }
    return ESP_OK;
}

static char *copy(const char *data, size_t length)
{
    char *out = heap_caps_malloc(length + 1, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    if (out == NULL) {
        out = malloc(length + 1);
    }
    if (out != NULL) {
        memcpy(out, data, length);
        out[length] = '\0';
    }
    return out;
}

esp_err_t app_link_open(const char *url)
{
    if (url == NULL) {
        return ESP_ERR_INVALID_ARG;
    }
    char *owned = copy(url, strlen(url));
    return owned != NULL ? post(CMD_OPEN, owned, 0) : ESP_ERR_NO_MEM;
}

esp_err_t app_link_send(const char *text, size_t length)
{
    if (text == NULL && length > 0) {
        return ESP_ERR_INVALID_ARG;
    }
    char *owned = copy(text != NULL ? text : "", length);
    return owned != NULL ? post(CMD_SEND, owned, length) : ESP_ERR_NO_MEM;
}

esp_err_t app_link_close(void)
{
    return post(CMD_CLOSE, NULL, 0);
}
