// SPDX-License-Identifier: Apache-2.0
#include "cam_qr.h"

#include <stdatomic.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/idf_additions.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "quirc.h"

static const char *TAG = "app_camera";

/* quirc_decode() keeps a ~9 KB datastream on the stack. The stack lives in
 * PSRAM: the task only computes, and it exists only during a scan session. */
#define QR_TASK_STACK_BYTES (16 * 1024)
/* Below the camera task (3), so decoding takes what the preview leaves. */
#define QR_TASK_PRIORITY 2
#define QR_TASK_CORE 1
/* Distinct payloads remembered per session; beyond this the oldest is
 * forgotten and could be reported again. */
#define QR_SEEN_MAX 8

struct cam_qr {
    struct quirc *decoder;
    /* Large result structs (~13 KB) kept off the task stack. */
    struct quirc_code *code;
    struct quirc_data *data;
    app_camera_code_cb_t on_code;
    void *ctx;
    TaskHandle_t task;
    SemaphoreHandle_t parked;   /* given once the task left its loop */
    atomic_bool busy;           /* the decoder owns the frame buffer */
    atomic_bool quit;
    uint32_t seen[QR_SEEN_MAX]; /* FNV-1a hashes of reported payloads */
    uint8_t seen_count;
    uint8_t seen_next;
    portMUX_TYPE stats_lock;
    cam_qr_stats_t stats;
};

static uint32_t fnv1a(const uint8_t *bytes, size_t length)
{
    uint32_t hash = 2166136261u;
    for (size_t i = 0; i < length; ++i) {
        hash = (hash ^ bytes[i]) * 16777619u;
    }
    return hash;
}

/* True the first time a payload is seen this session. */
static bool remember(cam_qr_t *qr, const uint8_t *payload, size_t length)
{
    const uint32_t hash = fnv1a(payload, length);
    for (uint8_t i = 0; i < qr->seen_count; ++i) {
        if (qr->seen[i] == hash) {
            return false;
        }
    }
    qr->seen[qr->seen_next] = hash;
    qr->seen_next = (uint8_t)((qr->seen_next + 1) % QR_SEEN_MAX);
    if (qr->seen_count < QR_SEEN_MAX) {
        qr->seen_count++;
    }
    return true;
}

/* What a payload is, for the log; the payload itself (a password) is never logged. */
static const char *kind(const uint8_t *payload, size_t length)
{
    static const char wifi[] = "WIFI:";
    static const char farol[] = "farol://";
    if (length >= sizeof(wifi) - 1 && memcmp(payload, wifi, sizeof(wifi) - 1) == 0) {
        return "Wi-Fi";
    }
    if (length >= sizeof(farol) - 1 && memcmp(payload, farol, sizeof(farol) - 1) == 0) {
        return "Farol";
    }
    return "other";
}

static bool decode_one(cam_qr_t *qr, int index)
{
    quirc_extract(qr->decoder, index, qr->code);
    quirc_decode_error_t err = quirc_decode(qr->code, qr->data);
    if (err == QUIRC_ERROR_DATA_ECC) {
        /* A mirrored code (front camera, or a reflection) decodes after a flip. */
        quirc_flip(qr->code);
        err = quirc_decode(qr->code, qr->data);
    }
    return err == QUIRC_SUCCESS && qr->data->payload_len > 0;
}

static void search(cam_qr_t *qr)
{
    const int64_t started = esp_timer_get_time();
    quirc_end(qr->decoder);
    const int count = quirc_count(qr->decoder);
    uint32_t codes = 0;
    uint32_t reported = 0;
    for (int i = 0; i < count; ++i) {
        if (!decode_one(qr, i)) {
            continue;
        }
        codes++;
        const uint8_t *payload = qr->data->payload;
        const size_t length = (size_t)qr->data->payload_len;
        if (!remember(qr, payload, length)) {
            continue;
        }
        reported++;
        ESP_LOGI(TAG, "QR decoded: %s code, %u bytes, %lld ms after the frame", kind(payload, length),
                 (unsigned)length, (long long)((esp_timer_get_time() - started) / 1000));
        if (qr->on_code != NULL) {
            qr->on_code((const char *)payload, length, qr->ctx);
        }
    }
    portENTER_CRITICAL(&qr->stats_lock);
    qr->stats.decodes++;
    qr->stats.codes += codes;
    qr->stats.reported += reported;
    qr->stats.busy_us += (uint64_t)(esp_timer_get_time() - started);
    portEXIT_CRITICAL(&qr->stats_lock);
}

static void qr_task(void *arg)
{
    cam_qr_t *qr = arg;
    for (;;) {
        ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        if (atomic_load(&qr->quit)) {
            break;
        }
        if (atomic_load(&qr->busy)) {
            search(qr);
            atomic_store(&qr->busy, false);
        }
    }
    xSemaphoreGive(qr->parked);
    /* Deleted by cam_qr_close() from the camera task (vTaskDeleteWithCaps). */
    vTaskSuspend(NULL);
}

esp_err_t cam_qr_open(int side, app_camera_code_cb_t on_code, void *ctx, cam_qr_t **out)
{
    *out = NULL;
    cam_qr_t *qr = heap_caps_calloc(1, sizeof(*qr), MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT);
    if (qr == NULL) {
        return ESP_ERR_NO_MEM;
    }
    qr->on_code = on_code;
    qr->ctx = ctx;
    qr->stats_lock = (portMUX_TYPE)portMUX_INITIALIZER_UNLOCKED;
    atomic_init(&qr->busy, false);
    atomic_init(&qr->quit, false);
    /* quirc's image (one byte per pixel, labelled in place) and flood-fill
     * stack go to PSRAM through malloc, being above
     * CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL. */
    qr->decoder = quirc_new();
    qr->code = heap_caps_malloc(sizeof(*qr->code), MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    qr->data = heap_caps_malloc(sizeof(*qr->data), MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    qr->parked = xSemaphoreCreateBinary();
    esp_err_t err = ESP_OK;
    if (qr->decoder == NULL || qr->code == NULL || qr->data == NULL || qr->parked == NULL ||
            quirc_resize(qr->decoder, side, side) != 0) {
        err = ESP_ERR_NO_MEM;
    } else if (xTaskCreatePinnedToCoreWithCaps(qr_task, "app_cam_qr", QR_TASK_STACK_BYTES, qr,
                                               QR_TASK_PRIORITY, &qr->task, QR_TASK_CORE,
                                               MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT) != pdPASS) {
        qr->task = NULL;
        err = ESP_ERR_NO_MEM;
    }
    if (err != ESP_OK) {
        cam_qr_close(qr);
        return err;
    }
    *out = qr;
    return ESP_OK;
}

uint8_t *cam_qr_acquire(cam_qr_t *qr)
{
    if (atomic_load(&qr->busy)) {
        return NULL;
    }
    int w = 0;
    int h = 0;
    return quirc_begin(qr->decoder, &w, &h);
}

void cam_qr_submit(cam_qr_t *qr)
{
    atomic_store(&qr->busy, true);
    xTaskNotifyGive(qr->task);
}

void cam_qr_take_stats(cam_qr_t *qr, cam_qr_stats_t *out)
{
    portENTER_CRITICAL(&qr->stats_lock);
    *out = qr->stats;
    memset(&qr->stats, 0, sizeof(qr->stats));
    portEXIT_CRITICAL(&qr->stats_lock);
}

void cam_qr_close(cam_qr_t *qr)
{
    if (qr == NULL) {
        return;
    }
    if (qr->task != NULL) {
        atomic_store(&qr->quit, true);
        xTaskNotifyGive(qr->task);
        xSemaphoreTake(qr->parked, portMAX_DELAY);
        vTaskDeleteWithCaps(qr->task);
    }
    if (qr->parked != NULL) {
        vSemaphoreDelete(qr->parked);
    }
    if (qr->decoder != NULL) {
        quirc_destroy(qr->decoder);
    }
    heap_caps_free(qr->code);
    heap_caps_free(qr->data);
    heap_caps_free(qr);
}
