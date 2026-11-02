// SPDX-License-Identifier: Apache-2.0
/* Wi-Fi station manager. One task, "app_net", owns every piece of state below:
 * the public calls and the ESP-IDF event handler only copy a message into its
 * queue. The task runs the driver (scan, connect, disconnect), keeps the saved
 * list (net_store.c) and reports status changes to the single consumer.
 * Contract: README.md. */
#include "app_net.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_attr.h"
#include "esp_check.h"
#include "esp_event.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_mac.h"
#include "esp_netif.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "net_store.h"

/* Internal RAM stack: driver calls, NVS, logging and the consumer's callbacks. */
#define TASK_STACK_BYTES 5120
/* Below the UI (render 4, logic 5): network bookkeeping is never urgent. */
#define TASK_PRIORITY 3
#define QUEUE_LENGTH 16
/* The system event task waits at most this long for room in the queue. */
#define EVENT_POST_WAIT_MS 50
/* Background rescan of saved networks while offline. */
#define RESCAN_MS 30000
/* RSSI refresh while online. */
#define RSSI_MS 10000
/* Guard for a join the driver never answers (it normally reports within ~10 s). */
#define CONNECT_TIMEOUT_MS 20000
/* Associated this long without a lease reports NoAddress (DHCP keeps trying). */
#define DHCP_TIMEOUT_MS 15000
/* Driver-level attempts per network before moving on. */
#define TRIES_PER_NETWORK 2
/* Scan records read per scan; the rest of a crowded band is dropped. */
#define SCAN_MAX 32
#define HOSTNAME_PREFIX "farol-"

static const char *TAG = "app_net";

/* rust/platform/src/net.rs mirrors these structs and asserts the same sizes. */
_Static_assert(sizeof(app_net_state_t) == 4, "app_net_state_t is an int");
_Static_assert(sizeof(app_net_status_t) == 352, "update net.rs NetStatus");
_Static_assert(sizeof(app_net_ap_t) == 36, "update net.rs ApRecord");

typedef enum {
    MSG_SUBSCRIBE,
    MSG_JOIN,
    MSG_SCAN,
    MSG_RETRY,
    MSG_FORGET,
    EV_STA_START,
    EV_SCAN_DONE,
    EV_CONNECTED,
    EV_DISCONNECTED,
    EV_GOT_IP,
    EV_LOST_IP,
} msg_kind_t;

typedef struct {
    msg_kind_t kind;
    union {
        struct {
            app_net_status_cb_t on_status;
            app_net_scan_cb_t on_scan;
            void *ctx;
        } subscribe;
        net_cred_t cred;                        /* join; forget uses ssid only */
        struct {
            char ssid[APP_NET_SSID_MAX + 1];
            uint16_t reason;
        } link;                                 /* connected, disconnected */
        uint32_t ip;                            /* got IP, network byte order */
        bool scan_ok;
    };
} msg_t;

/* Where the station is; the reported state is derived from it. */
typedef enum {
    STA_IDLE,          /* no attempt: scanning, waiting for a rescan, or nothing to do */
    STA_CONNECTING,    /* esp_wifi_connect() issued for `target` */
    STA_ASSOCIATED,    /* associated with `target`, no address */
    STA_ONLINE,        /* associated with an IPv4 address */
} sta_phase_t;

typedef enum {
    FAIL_AUTH,
    FAIL_NOT_FOUND,
    FAIL_OTHER,
} fail_kind_t;

typedef struct {
    QueueHandle_t queue;
    TaskHandle_t task;
    esp_netif_t *netif;
    esp_event_handler_instance_t wifi_events;
    esp_event_handler_instance_t ip_events;
    bool wifi_initialized;
    net_store_t store;

    app_net_status_cb_t on_status;
    app_net_scan_cb_t on_scan;
    void *ctx;

    app_net_state_t state;
    char state_ssid[APP_NET_SSID_MAX + 1];
    int8_t rssi;
    uint32_t ip;
    app_net_status_t reported;
    bool reported_once;

    sta_phase_t phase;
    char target[APP_NET_SSID_MAX + 1];
    uint8_t tries;
    char candidates[APP_NET_STATUS_SAVED_MAX][APP_NET_SSID_MAX + 1];
    uint8_t candidate_count;
    uint8_t candidate_at;
    /* Networks that refused the password, skipped by background rescans
     * until a Join, Retry or Forget. */
    char rejected[APP_NET_STATUS_SAVED_MAX][APP_NET_SSID_MAX + 1];
    uint8_t rejected_count;

    bool scan_running;
    bool scan_for_user;
    bool scan_for_select;
    bool scan_user_pending;

    int64_t connect_deadline;   /* ms since boot, 0 = off */
    int64_t dhcp_deadline;
    int64_t rescan_at;
    int64_t rssi_at;
} app_net_ctx_t;

/* Task-only state (no ISR access): kept in PSRAM, ~2 KB of internal RAM saved. */
static EXT_RAM_BSS_ATTR app_net_ctx_t s_net;
/* Published once the task runs; the public calls read it from any task. */
static QueueHandle_t volatile s_queue;

static int64_t now_ms(void)
{
    return esp_timer_get_time() / 1000;
}

/* ---------------------------------------------------------------- reporting */

static const char *state_name(app_net_state_t state)
{
    static const char *const names[] = {
        "Off", "NoSaved", "NotFound", "Connecting", "WrongPassword", "NoAddress", "Online",
    };
    return (unsigned)state < sizeof(names) / sizeof(names[0]) ? names[state] : "?";
}

static void set_state(app_net_state_t state, const char *ssid)
{
    s_net.state = state;
    strlcpy(s_net.state_ssid, ssid, sizeof(s_net.state_ssid));
    if (state != APP_NET_ONLINE) {
        s_net.ip = 0;
        s_net.rssi = 0;
    }
}

/* Reports the status when it differs from the last report. */
static void report(void)
{
    app_net_status_t status;
    memset(&status, 0, sizeof(status)); /* compared bytewise, padding included */
    status.state = s_net.state;
    status.rssi = s_net.rssi;
    strlcpy(status.ssid, s_net.state_ssid, sizeof(status.ssid));
    if (s_net.state == APP_NET_ONLINE) {
        esp_ip4_addr_t ip = {.addr = s_net.ip};
        esp_ip4addr_ntoa(&ip, status.ip, sizeof(status.ip));
    }
    status.saved_count = (uint8_t)net_store_names(&s_net.store, status.saved, APP_NET_STATUS_SAVED_MAX);
    if (s_net.reported_once && memcmp(&status, &s_net.reported, sizeof(status)) == 0) {
        return;
    }
    const bool transition = !s_net.reported_once || status.state != s_net.reported.state ||
                            strcmp(status.ssid, s_net.reported.ssid) != 0;
    if (transition) {
        ESP_LOGI(TAG, "state %s ssid=\"%s\" rssi=%d ip=%s saved=%u stack_free=%u B",
                 state_name(status.state), status.ssid, status.rssi, status.ip[0] ? status.ip : "-",
                 status.saved_count, (unsigned)uxTaskGetStackHighWaterMark(NULL));
    }
    s_net.reported = status;
    s_net.reported_once = true;
    if (s_net.on_status != NULL) {
        s_net.on_status(&status, s_net.ctx);
    }
}

/* ---------------------------------------------------------------- scanning */

static void scan_start(void)
{
    if (s_net.scan_running) {
        return; /* the running scan answers every requester */
    }
    const wifi_scan_config_t config = {.show_hidden = false, .scan_type = WIFI_SCAN_TYPE_ACTIVE};
    const esp_err_t err = esp_wifi_scan_start(&config, false);
    if (err == ESP_OK) {
        s_net.scan_running = true;
        ESP_LOGI(TAG, "scan started (%s%s)", s_net.scan_for_select ? "saved networks " : "",
                 s_net.scan_for_user ? "for the UI" : "");
        return;
    }
    ESP_LOGW(TAG, "scan did not start: %s", esp_err_to_name(err));
    if (s_net.scan_for_user) {
        s_net.scan_for_user = false;
        if (s_net.on_scan != NULL) {
            s_net.on_scan(NULL, 0, s_net.ctx);
        }
    }
    if (s_net.scan_for_select) {
        s_net.scan_for_select = false;
        s_net.rescan_at = now_ms() + RESCAN_MS;
    }
}

/* A user scan waits while a join is in flight (the driver refuses both). */
static void scan_user_resume(void)
{
    if (s_net.scan_user_pending && s_net.phase != STA_CONNECTING) {
        s_net.scan_user_pending = false;
        s_net.scan_for_user = true;
        scan_start();
    }
}

static int compare_rssi(const void *a, const void *b)
{
    return ((const app_net_ap_t *)b)->rssi - ((const app_net_ap_t *)a)->rssi;
}

/* Reads the driver's records into `aps`: named networks only, one per SSID
 * at its strongest BSSID, strongest first. Returns the count. */
static size_t scan_collect(bool ok, app_net_ap_t *aps)
{
    wifi_ap_record_t *records = heap_caps_calloc(SCAN_MAX, sizeof(*records),
                                                 MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    uint16_t count = SCAN_MAX;
    if (!ok || records == NULL || esp_wifi_scan_get_ap_records(&count, records) != ESP_OK) {
        if (ok) {
            ESP_LOGW(TAG, "scan records unavailable");
        }
        (void)esp_wifi_clear_ap_list();
        free(records);
        return 0;
    }
    char saved[APP_NET_STATUS_SAVED_MAX][APP_NET_SSID_MAX + 1];
    const size_t saved_count = net_store_names(&s_net.store, saved, APP_NET_STATUS_SAVED_MAX);
    size_t n = 0;
    for (uint16_t i = 0; i < count; ++i) {
        const char *ssid = (const char *)records[i].ssid;
        if (ssid[0] == '\0') {
            continue;
        }
        app_net_ap_t *ap = NULL;
        for (size_t j = 0; j < n; ++j) {
            if (strcmp(aps[j].ssid, ssid) == 0) {
                ap = &aps[j];
                break;
            }
        }
        if (ap != NULL) {
            if (records[i].rssi > ap->rssi) {
                ap->rssi = records[i].rssi;
                ap->secure = records[i].authmode != WIFI_AUTH_OPEN;
            }
            continue;
        }
        ap = &aps[n++];
        strlcpy(ap->ssid, ssid, sizeof(ap->ssid));
        ap->rssi = records[i].rssi;
        ap->secure = records[i].authmode != WIFI_AUTH_OPEN;
        ap->saved = false;
        for (size_t k = 0; k < saved_count; ++k) {
            ap->saved |= strcmp(saved[k], ssid) == 0;
        }
    }
    free(records);
    qsort(aps, n, sizeof(*aps), compare_rssi);
    return n;
}

/* ---------------------------------------------------------------- joining */

static bool is_rejected(const char *ssid)
{
    for (uint8_t i = 0; i < s_net.rejected_count; ++i) {
        if (strcmp(s_net.rejected[i], ssid) == 0) {
            return true;
        }
    }
    return false;
}

static void reject(const char *ssid)
{
    if (!is_rejected(ssid) && s_net.rejected_count < APP_NET_STATUS_SAVED_MAX) {
        strlcpy(s_net.rejected[s_net.rejected_count++], ssid, APP_NET_SSID_MAX + 1);
    }
}

static void unreject(const char *ssid)
{
    for (uint8_t i = 0; i < s_net.rejected_count; ++i) {
        if (strcmp(s_net.rejected[i], ssid) == 0) {
            s_net.rejected[i][0] = '\0';
            memmove(&s_net.rejected[i], &s_net.rejected[i + 1],
                    (size_t)(s_net.rejected_count - i - 1) * sizeof(s_net.rejected[0]));
            --s_net.rejected_count;
            return;
        }
    }
}

/* Leaves the current network, if any. The driver answers with a
 * DISCONNECTED event of reason ASSOC_LEAVE, which on_disconnected ignores. */
static void leave(void)
{
    if (s_net.phase != STA_IDLE) {
        (void)esp_wifi_disconnect();
    }
    s_net.phase = STA_IDLE;
    s_net.target[0] = '\0';
    s_net.connect_deadline = 0;
    s_net.dhcp_deadline = 0;
    s_net.rssi_at = 0;
}

static void attempt_next(void);

static void attempt_start(const char *ssid)
{
    net_cred_t cred;
    if (!net_store_find(&s_net.store, ssid, &cred)) {
        ESP_LOGW(TAG, "\"%s\" is no longer saved; skipping", ssid);
        attempt_next();
        return;
    }
    if (s_net.scan_running) {
        /* A join aborts the scan; a pending user scan is retried afterwards. */
        s_net.scan_user_pending |= s_net.scan_for_user;
        s_net.scan_for_user = false;
        s_net.scan_for_select = false;
        s_net.scan_running = false;
        (void)esp_wifi_scan_stop();
    }
    leave();
    wifi_config_t config = {0};
    memcpy(config.sta.ssid, cred.ssid, strnlen(cred.ssid, sizeof(config.sta.ssid)));
    memcpy(config.sta.password, cred.password, strnlen(cred.password, sizeof(config.sta.password)));
    config.sta.scan_method = WIFI_ALL_CHANNEL_SCAN;
    config.sta.sort_method = WIFI_CONNECT_AP_BY_SIGNAL;
    /* A saved password never joins an open network of the same name. */
    config.sta.threshold.authmode = cred.password[0] ? WIFI_AUTH_WPA_PSK : WIFI_AUTH_OPEN;
    config.sta.pmf_cfg.capable = true;
    config.sta.sae_pwe_h2e = WPA3_SAE_PWE_BOTH;
    net_store_wipe(&cred);

    strlcpy(s_net.target, ssid, sizeof(s_net.target));
    s_net.phase = STA_CONNECTING;
    s_net.tries = 1;
    s_net.connect_deadline = now_ms() + CONNECT_TIMEOUT_MS;
    set_state(APP_NET_CONNECTING, ssid);
    esp_err_t err = esp_wifi_set_config(WIFI_IF_STA, &config);
    memset(&config, 0, sizeof(config));
    if (err == ESP_OK) {
        err = esp_wifi_connect();
    }
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "joining \"%s\" failed to start: %s", ssid, esp_err_to_name(err));
        s_net.phase = STA_IDLE;
        attempt_next();
        return;
    }
    ESP_LOGI(TAG, "joining \"%s\" (%u/%u)", ssid, (unsigned)s_net.candidate_at + 1,
             (unsigned)s_net.candidate_count);
}

/* The candidate list is exhausted: report why and rescan later. */
static void attempts_exhausted(void)
{
    leave();
    const char *rejected = NULL;
    for (uint8_t i = 0; i < s_net.candidate_count && rejected == NULL; ++i) {
        if (is_rejected(s_net.candidates[i])) {
            rejected = s_net.candidates[i];
        }
    }
    if (rejected != NULL) {
        set_state(APP_NET_WRONG_PASSWORD, rejected);
    } else {
        set_state(APP_NET_NOT_FOUND, "");
    }
    s_net.candidate_count = 0;
    s_net.rescan_at = now_ms() + RESCAN_MS;
    ESP_LOGI(TAG, "no saved network joined; rescanning in %d s", RESCAN_MS / 1000);
    scan_user_resume();
}

static void attempt_next(void)
{
    while (++s_net.candidate_at < s_net.candidate_count) {
        if (!is_rejected(s_net.candidates[s_net.candidate_at])) {
            attempt_start(s_net.candidates[s_net.candidate_at]);
            return;
        }
    }
    attempts_exhausted();
}

/* Joins `candidates` in order, beginning with the first. */
static void attempts_begin(void)
{
    s_net.candidate_at = 0;
    if (s_net.candidate_count == 0) {
        attempts_exhausted();
    } else if (is_rejected(s_net.candidates[0])) {
        attempt_next();
    } else {
        attempt_start(s_net.candidates[0]);
    }
}

/* Scans for the saved networks and joins the strongest one found. */
static void select_start(void)
{
    char saved[APP_NET_STATUS_SAVED_MAX][APP_NET_SSID_MAX + 1];
    const size_t count = net_store_names(&s_net.store, saved, APP_NET_STATUS_SAVED_MAX);
    s_net.rescan_at = 0;
    if (count == 0) {
        leave();
        set_state(APP_NET_NO_SAVED, "");
        ESP_LOGI(TAG, "no saved network; waiting for a Join");
        return;
    }
    if (s_net.state == APP_NET_OFF || s_net.state == APP_NET_NO_SAVED) {
        set_state(APP_NET_CONNECTING, saved[0]);
    }
    s_net.scan_for_select = true;
    scan_start();
}

static void on_scan_done(bool ok)
{
    if (!s_net.scan_running) {
        (void)esp_wifi_clear_ap_list(); /* a scan that was stopped */
        return;
    }
    s_net.scan_running = false;
    app_net_ap_t *aps = heap_caps_calloc(SCAN_MAX, sizeof(*aps), MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    const size_t n = aps != NULL ? scan_collect(ok, aps) : 0;
    if (aps == NULL) {
        (void)esp_wifi_clear_ap_list();
        ESP_LOGE(TAG, "no memory for scan results");
    }
    ESP_LOGI(TAG, "scan done: %u networks%s", (unsigned)n, ok ? "" : " (scan failed)");
    if (s_net.scan_for_user) {
        s_net.scan_for_user = false;
        if (s_net.on_scan != NULL) {
            s_net.on_scan(aps, n, s_net.ctx);
        }
    }
    if (s_net.scan_for_select) {
        s_net.scan_for_select = false;
        s_net.candidate_count = 0;
        for (size_t i = 0; i < n && s_net.candidate_count < APP_NET_STATUS_SAVED_MAX; ++i) {
            if (aps[i].saved) {
                strlcpy(s_net.candidates[s_net.candidate_count++], aps[i].ssid, APP_NET_SSID_MAX + 1);
            }
        }
        if (s_net.phase == STA_IDLE) {
            attempts_begin();
        }
    }
    free(aps);
    scan_user_resume();
}

/* ---------------------------------------------------------------- station events */

static fail_kind_t classify(uint16_t reason)
{
    switch (reason) {
    case WIFI_REASON_MIC_FAILURE:
    case WIFI_REASON_4WAY_HANDSHAKE_TIMEOUT:
    case WIFI_REASON_AUTH_FAIL:
    case WIFI_REASON_HANDSHAKE_TIMEOUT:
        return FAIL_AUTH;
    case WIFI_REASON_NO_AP_FOUND:
    case WIFI_REASON_NO_AP_FOUND_W_COMPATIBLE_SECURITY:
    case WIFI_REASON_NO_AP_FOUND_IN_AUTHMODE_THRESHOLD:
    case WIFI_REASON_NO_AP_FOUND_IN_RSSI_THRESHOLD:
        return FAIL_NOT_FOUND;
    default:
        return FAIL_OTHER;
    }
}

static void attempt_failed(fail_kind_t kind, uint16_t reason)
{
    static const char *const kinds[] = {"password rejected", "not found", "failed"};
    ESP_LOGW(TAG, "joining \"%s\" %s (reason %u, try %u/%u)", s_net.target, kinds[kind],
             reason, s_net.tries, TRIES_PER_NETWORK);
    if (kind != FAIL_NOT_FOUND && s_net.tries < TRIES_PER_NETWORK) {
        ++s_net.tries;
        s_net.connect_deadline = now_ms() + CONNECT_TIMEOUT_MS;
        if (esp_wifi_connect() == ESP_OK) {
            return;
        }
    }
    if (kind == FAIL_AUTH) {
        reject(s_net.target);
    }
    s_net.phase = STA_IDLE;
    attempt_next();
}

static void on_disconnected(const char *ssid, uint16_t reason)
{
    if (reason == WIFI_REASON_ASSOC_LEAVE || s_net.phase == STA_IDLE) {
        return; /* our own leave() */
    }
    if (ssid[0] != '\0' && strcmp(ssid, s_net.target) != 0) {
        ESP_LOGI(TAG, "ignoring a stale disconnect from \"%s\" (reason %u)", ssid, reason);
        return;
    }
    if (s_net.phase == STA_CONNECTING) {
        attempt_failed(classify(reason), reason);
        return;
    }
    /* An established link dropped: look for the best saved network again. */
    ESP_LOGW(TAG, "lost \"%s\" (reason %u); reselecting", s_net.target, reason);
    char lost[APP_NET_SSID_MAX + 1];
    strlcpy(lost, s_net.target, sizeof(lost));
    s_net.phase = STA_IDLE;
    leave();
    set_state(APP_NET_CONNECTING, lost);
    select_start();
}

static void on_connected(const char *ssid)
{
    if (s_net.phase != STA_CONNECTING || strcmp(ssid, s_net.target) != 0) {
        ESP_LOGI(TAG, "ignoring an association with \"%s\"", ssid);
        return;
    }
    s_net.phase = STA_ASSOCIATED;
    s_net.connect_deadline = 0;
    s_net.dhcp_deadline = now_ms() + DHCP_TIMEOUT_MS;
    ESP_LOGI(TAG, "associated with \"%s\"; waiting for DHCP", ssid);
    scan_user_resume();
}

static void read_rssi(void)
{
    int rssi = 0;
    if (esp_wifi_sta_get_rssi(&rssi) == ESP_OK) {
        s_net.rssi = (int8_t)rssi;
    }
    s_net.rssi_at = now_ms() + RSSI_MS;
}

static void on_got_ip(uint32_t ip)
{
    if (s_net.phase != STA_ASSOCIATED && s_net.phase != STA_ONLINE) {
        return;
    }
    s_net.phase = STA_ONLINE;
    s_net.dhcp_deadline = 0;
    set_state(APP_NET_ONLINE, s_net.target);
    s_net.ip = ip;
    read_rssi();
    unreject(s_net.target);
    (void)net_store_touch(&s_net.store, s_net.target);
}

static void on_lost_ip(void)
{
    if (s_net.phase == STA_ONLINE) {
        ESP_LOGW(TAG, "DHCP lease on \"%s\" lost", s_net.target);
        s_net.phase = STA_ASSOCIATED;
        s_net.rssi_at = 0;
        set_state(APP_NET_NO_ADDRESS, s_net.target);
    }
}

/* ---------------------------------------------------------------- commands */

static void on_join(const net_cred_t *cred)
{
    /* An empty password means "with the saved password if there is one":
     * the own entry moves to the front, the shared network stays read-only,
     * and only an unknown network is saved as open. */
    if (cred->password[0] == '\0' && s_net.phase == STA_ONLINE && strcmp(s_net.target, cred->ssid) == 0) {
        ESP_LOGI(TAG, "join \"%s\": already online", cred->ssid);
        return;
    }
    bool known = false;
    if (cred->password[0] == '\0') {
        net_cred_t saved;
        net_store_unhide(&s_net.store, cred->ssid);
        known = net_store_find(&s_net.store, cred->ssid, &saved);
        net_store_wipe(&saved);
    }
    const esp_err_t err = known ? net_store_touch(&s_net.store, cred->ssid)
                          : net_store_remember(&s_net.store, cred->ssid, cred->password);
    if (err == ESP_ERR_INVALID_ARG) {
        return;
    }
    unreject(cred->ssid);
    strlcpy(s_net.candidates[0], cred->ssid, APP_NET_SSID_MAX + 1);
    s_net.candidate_count = 1;
    s_net.rescan_at = 0;
    ESP_LOGI(TAG, "join \"%s\" requested (%s)", cred->ssid,
             cred->password[0] ? "new password" : known ? "saved password" : "open");
    attempts_begin();
}

static void on_forget(const char *ssid)
{
    if (net_store_forget(&s_net.store, ssid) != ESP_OK) {
        ESP_LOGW(TAG, "forget \"%s\": not saved", ssid);
        return;
    }
    ESP_LOGI(TAG, "forgot \"%s\"", ssid);
    unreject(ssid);
    const bool in_use = s_net.phase != STA_IDLE && strcmp(s_net.target, ssid) == 0;
    const bool shown = strcmp(s_net.state_ssid, ssid) == 0;
    if (in_use || shown || s_net.state == APP_NET_NOT_FOUND) {
        leave();
        set_state(APP_NET_CONNECTING, "");
        select_start();
    }
}

static void on_retry(void)
{
    if (s_net.phase == STA_ONLINE || s_net.phase == STA_CONNECTING) {
        ESP_LOGI(TAG, "retry ignored: %s", s_net.phase == STA_ONLINE ? "online" : "already joining");
        return;
    }
    if (s_net.state == APP_NET_OFF) {
        return;
    }
    ESP_LOGI(TAG, "retry requested");
    s_net.rejected_count = 0;
    leave();
    select_start();
}

static void on_scan(void)
{
    if (s_net.scan_running) {
        s_net.scan_for_user = true;
    } else if (s_net.phase == STA_CONNECTING) {
        s_net.scan_user_pending = true;
    } else {
        s_net.scan_for_user = true;
        scan_start();
    }
}

/* ---------------------------------------------------------------- task */

static void handle(msg_t *msg)
{
    switch (msg->kind) {
    case MSG_SUBSCRIBE:
        s_net.on_status = msg->subscribe.on_status;
        s_net.on_scan = msg->subscribe.on_scan;
        s_net.ctx = msg->subscribe.ctx;
        s_net.reported_once = false; /* the new consumer gets the current status */
        break;
    case MSG_JOIN:
        on_join(&msg->cred);
        break;
    case MSG_SCAN:
        on_scan();
        break;
    case MSG_RETRY:
        on_retry();
        break;
    case MSG_FORGET:
        on_forget(msg->cred.ssid);
        break;
    case EV_STA_START:
        ESP_LOGI(TAG, "station started");
        select_start();
        break;
    case EV_SCAN_DONE:
        on_scan_done(msg->scan_ok);
        break;
    case EV_CONNECTED:
        on_connected(msg->link.ssid);
        break;
    case EV_DISCONNECTED:
        on_disconnected(msg->link.ssid, msg->link.reason);
        break;
    case EV_GOT_IP:
        on_got_ip(msg->ip);
        break;
    case EV_LOST_IP:
        on_lost_ip();
        break;
    }
    net_store_wipe(&msg->cred);
}

static void run_timers(void)
{
    const int64_t now = now_ms();
    if (s_net.connect_deadline && now >= s_net.connect_deadline && s_net.phase == STA_CONNECTING) {
        ESP_LOGW(TAG, "joining \"%s\" timed out", s_net.target);
        s_net.connect_deadline = 0;
        (void)esp_wifi_disconnect();
        s_net.tries = TRIES_PER_NETWORK;
        attempt_failed(FAIL_OTHER, 0);
    }
    if (s_net.dhcp_deadline && now >= s_net.dhcp_deadline && s_net.phase == STA_ASSOCIATED) {
        s_net.dhcp_deadline = 0;
        ESP_LOGW(TAG, "no DHCP lease from \"%s\" after %d s", s_net.target, DHCP_TIMEOUT_MS / 1000);
        set_state(APP_NET_NO_ADDRESS, s_net.target);
    }
    if (s_net.rescan_at && now >= s_net.rescan_at && s_net.phase == STA_IDLE) {
        s_net.rescan_at = 0;
        select_start();
    }
    if (s_net.rssi_at && now >= s_net.rssi_at && s_net.phase == STA_ONLINE) {
        read_rssi();
    }
}

static TickType_t wait_ticks(void)
{
    const int64_t deadlines[] = {
        s_net.connect_deadline, s_net.dhcp_deadline, s_net.rescan_at, s_net.rssi_at,
    };
    int64_t next = 0;
    for (size_t i = 0; i < sizeof(deadlines) / sizeof(deadlines[0]); ++i) {
        if (deadlines[i] && (next == 0 || deadlines[i] < next)) {
            next = deadlines[i];
        }
    }
    if (next == 0) {
        return portMAX_DELAY;
    }
    const int64_t wait = next - now_ms();
    return wait <= 0 ? 0 : pdMS_TO_TICKS(wait) + 1;
}

static void net_task(void *arg)
{
    (void)arg;
    msg_t msg;
    for (;;) {
        if (xQueueReceive(s_net.queue, &msg, wait_ticks()) == pdTRUE) {
            handle(&msg);
        }
        run_timers();
        report();
    }
}

/* ---------------------------------------------------------------- driver events */

/* Runs on the system event task (small stack): copy and forward only. */
static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    (void)arg;
    msg_t msg = {0};
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_START) {
        msg.kind = EV_STA_START;
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_SCAN_DONE) {
        msg.kind = EV_SCAN_DONE;
        msg.scan_ok = ((const wifi_event_sta_scan_done_t *)data)->status == 0;
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_CONNECTED) {
        const wifi_event_sta_connected_t *event = data;
        msg.kind = EV_CONNECTED;
        memcpy(msg.link.ssid, event->ssid, event->ssid_len <= APP_NET_SSID_MAX ? event->ssid_len : APP_NET_SSID_MAX);
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
        const wifi_event_sta_disconnected_t *event = data;
        msg.kind = EV_DISCONNECTED;
        memcpy(msg.link.ssid, event->ssid, event->ssid_len <= APP_NET_SSID_MAX ? event->ssid_len : APP_NET_SSID_MAX);
        msg.link.reason = event->reason;
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        const ip_event_got_ip_t *event = data;
        if (event->esp_netif != s_net.netif) {
            return;
        }
        msg.kind = EV_GOT_IP;
        msg.ip = event->ip_info.ip.addr;
    } else if (base == IP_EVENT && id == IP_EVENT_STA_LOST_IP) {
        msg.kind = EV_LOST_IP;
    } else {
        return;
    }
    if (xQueueSend(s_net.queue, &msg, pdMS_TO_TICKS(EVENT_POST_WAIT_MS)) != pdTRUE) {
        ESP_LOGW(TAG, "event %ld dropped: queue full", (long)id);
    }
}

/* ---------------------------------------------------------------- lifecycle */

static void set_hostname(void)
{
    uint8_t mac[6];
    char hostname[sizeof(HOSTNAME_PREFIX) + 4];
    /* Same four hex digits as the logic's device id (rust/platform board.rs). */
    if (esp_efuse_mac_get_default(mac) == ESP_OK) {
        snprintf(hostname, sizeof(hostname), HOSTNAME_PREFIX "%02x%02x", mac[4], mac[5]);
        (void)esp_netif_set_hostname(s_net.netif, hostname);
    }
}

static void rollback(void)
{
    if (s_net.wifi_events != NULL) {
        (void)esp_event_handler_instance_unregister(WIFI_EVENT, ESP_EVENT_ANY_ID, s_net.wifi_events);
        s_net.wifi_events = NULL;
    }
    if (s_net.ip_events != NULL) {
        (void)esp_event_handler_instance_unregister(IP_EVENT, ESP_EVENT_ANY_ID, s_net.ip_events);
        s_net.ip_events = NULL;
    }
    if (s_net.wifi_initialized) {
        (void)esp_wifi_stop();
        (void)esp_wifi_deinit();
        s_net.wifi_initialized = false;
    }
    if (s_net.netif != NULL) {
        esp_netif_destroy_default_wifi(s_net.netif);
        s_net.netif = NULL;
    }
    if (s_net.queue != NULL) {
        vQueueDelete(s_net.queue);
        s_net.queue = NULL;
    }
    /* The default event loop and the esp_netif core belong to the whole
     * firmware (ESP-Iris's TCP transport uses them too) and stay. */
}

esp_err_t app_net_start(const char *nvs_namespace)
{
    ESP_RETURN_ON_FALSE(s_net.queue == NULL, ESP_ERR_INVALID_STATE, TAG, "already started");
    ESP_RETURN_ON_ERROR(net_store_load(&s_net.store, nvs_namespace), TAG, "load saved networks");
    ESP_RETURN_ON_ERROR(esp_netif_init(), TAG, "netif init");
    esp_err_t ret = esp_event_loop_create_default();
    ESP_RETURN_ON_FALSE(ret == ESP_OK || ret == ESP_ERR_INVALID_STATE, ret, TAG, "default event loop");
    ret = ESP_OK;

    s_net.queue = xQueueCreate(QUEUE_LENGTH, sizeof(msg_t));
    ESP_GOTO_ON_FALSE(s_net.queue != NULL, ESP_ERR_NO_MEM, fail, TAG, "queue");
    s_net.netif = esp_netif_create_default_wifi_sta();
    ESP_GOTO_ON_FALSE(s_net.netif != NULL, ESP_ERR_NO_MEM, fail, TAG, "station netif");
    set_hostname();
    const wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_GOTO_ON_ERROR(esp_wifi_init(&init), fail, TAG, "Wi-Fi init");
    s_net.wifi_initialized = true;
    /* The saved list lives in net_store; the driver keeps nothing in flash. */
    ESP_GOTO_ON_ERROR(esp_wifi_set_storage(WIFI_STORAGE_RAM), fail, TAG, "Wi-Fi storage");
    ESP_GOTO_ON_ERROR(esp_wifi_set_mode(WIFI_MODE_STA), fail, TAG, "station mode");
    ESP_GOTO_ON_ERROR(esp_event_handler_instance_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL,
                                                          &s_net.wifi_events), fail, TAG, "Wi-Fi events");
    ESP_GOTO_ON_ERROR(esp_event_handler_instance_register(IP_EVENT, ESP_EVENT_ANY_ID, on_event, NULL,
                                                          &s_net.ip_events), fail, TAG, "IP events");
    /* Events posted before the task runs wait in the queue. */
    ESP_GOTO_ON_ERROR(esp_wifi_start(), fail, TAG, "Wi-Fi start");
    ESP_GOTO_ON_FALSE(xTaskCreatePinnedToCore(net_task, "app_net", TASK_STACK_BYTES, NULL, TASK_PRIORITY,
                                              &s_net.task, tskNO_AFFINITY) == pdPASS,
                      ESP_ERR_NO_MEM, fail, TAG, "task");
    s_queue = s_net.queue;
    ESP_LOGI(TAG, "started: %u own networks saved", (unsigned)s_net.store.own_count);
    return ESP_OK;

fail:
    ESP_LOGE(TAG, "Wi-Fi unavailable (%s)", esp_err_to_name(ret));
    rollback();
    return ret;
}

/* ---------------------------------------------------------------- public calls */

static esp_err_t post(msg_t *msg)
{
    QueueHandle_t queue = s_queue;
    esp_err_t err = ESP_ERR_INVALID_STATE;
    if (queue != NULL) {
        err = xQueueSend(queue, msg, 0) == pdTRUE ? ESP_OK : ESP_ERR_NO_MEM;
    }
    net_store_wipe(&msg->cred);
    return err;
}

esp_err_t app_net_subscribe(app_net_status_cb_t on_status, app_net_scan_cb_t on_scan, void *ctx)
{
    msg_t msg = {.kind = MSG_SUBSCRIBE};
    msg.subscribe.on_status = on_status;
    msg.subscribe.on_scan = on_scan;
    msg.subscribe.ctx = ctx;
    return post(&msg);
}

esp_err_t app_net_join(const char *ssid, const char *password)
{
    if (ssid == NULL || password == NULL) {
        return ESP_ERR_INVALID_ARG;
    }
    const size_t ssid_len = strnlen(ssid, APP_NET_SSID_MAX + 1);
    const size_t password_len = strnlen(password, APP_NET_PASSWORD_MAX + 1);
    /* WPA passphrases are 8..63 characters, or 64 hex digits. */
    if (ssid_len < 1 || ssid_len > APP_NET_SSID_MAX || password_len > APP_NET_PASSWORD_MAX ||
            (password_len > 0 && password_len < 8)) {
        return ESP_ERR_INVALID_ARG;
    }
    msg_t msg = {.kind = MSG_JOIN};
    memcpy(msg.cred.ssid, ssid, ssid_len);
    memcpy(msg.cred.password, password, password_len);
    return post(&msg);
}

esp_err_t app_net_scan(void)
{
    msg_t msg = {.kind = MSG_SCAN};
    return post(&msg);
}

esp_err_t app_net_retry(void)
{
    msg_t msg = {.kind = MSG_RETRY};
    return post(&msg);
}

esp_err_t app_net_forget(const char *ssid)
{
    if (ssid == NULL || strnlen(ssid, APP_NET_SSID_MAX + 1) > APP_NET_SSID_MAX || ssid[0] == '\0') {
        return ESP_ERR_INVALID_ARG;
    }
    msg_t msg = {.kind = MSG_FORGET};
    strlcpy(msg.cred.ssid, ssid, sizeof(msg.cred.ssid));
    return post(&msg);
}
