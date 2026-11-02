// SPDX-License-Identifier: Apache-2.0
// Scan sessions: the camera module in the left slot, a live preview pushed to
// a GSP Canvas, and grayscale frames for the QR decoder (cam_qr.c).
// Contract: README.md.
#include "app_camera.h"

#include <inttypes.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <string.h>

#include "cam_qr.h"
#include "driver/ppa.h"
#include "esp_check.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "linux/videodev2.h"
#include "mosaico_module_camera.h"

static const char *TAG = "app_camera";

/* The session task: core 1, below GSP's render task (4), as the plan's
 * threading table places the camera. PPA calls block, so it mostly waits. */
#define CAM_TASK_STACK_BYTES 6144
#define CAM_TASK_PRIORITY 3
#define CAM_TASK_CORE 1

#define PREVIEW_SIDE APP_CAMERA_PREVIEW_SIDE
#define PREVIEW_STRIDE (PREVIEW_SIDE * 2U)                      /* RGB565 */
#define PREVIEW_BYTES ((size_t)PREVIEW_STRIDE * PREVIEW_SIDE)    /* 259,200 */
/* GSP keeps one frame live while the next is converted. */
#define PREVIEW_SLOTS 2
#define PREVIEW_ALIGN 64                                         /* PPA/cache line */
/* The PPA scales in 1/16 steps; crops are chosen so the preview is exact. */
#define PPA_SCALE_STEPS 16U

#define CAMERA_BUFFERS 2
/* Largest gray frame handed to quirc; a larger crop is box-averaged down. */
#define QR_SIDE_MAX 480U
/* At most 10 searches a second, as the plan asks (5-10). */
#define QR_INTERVAL_US 100000
#define STATS_PERIOD_US 5000000
/* Teardown: how long to wait for GSP to hand the last frames back. */
#define CANVAS_RELEASE_TIMEOUT_MS 1000

/* One-instance service: app_main initializes it, the logic task starts and
 * stops sessions, and one session task at a time owns the camera. `lock`
 * guards the request fields; the session task owns everything else. */
static struct {
    esp_gsp_handle_t ui;
    SemaphoreHandle_t lock;
    bool want;               /* the logic asked for a session */
    bool running;            /* a session task exists */
    uint16_t bind;
    app_camera_code_cb_t on_code;
    void *ctx;
} s_cam;

typedef struct {
    uint8_t *pixels;
    atomic_bool live;        /* GSP holds it; its release callback clears this */
} preview_slot_t;

/* Geometry of one stream: a centred square crop, shown at PREVIEW_SIDE and
 * decoded at qr_side (crop / qr_step). */
typedef struct {
    uint32_t width, height, stride, crop, crop_x, crop_y;
    float scale;
    uint32_t qr_step, qr_side;
} geometry_t;

typedef struct {
    uint16_t bind;
    mosaico_camera_handle_t camera;
    bool streaming;
    ppa_client_handle_t ppa;
    cam_qr_t *qr;
    preview_slot_t slots[PREVIEW_SLOTS];
    geometry_t geo;
    int64_t started_us;
    int64_t next_qr_us;
    int64_t next_stats_us;
    uint32_t frames, shown, skipped, short_frames, restarts;
    uint32_t total_frames, total_shown;
    uint32_t total_codes;
} session_t;

static bool session_wanted(void)
{
    xSemaphoreTake(s_cam.lock, portMAX_DELAY);
    const bool want = s_cam.want;
    xSemaphoreGive(s_cam.lock);
    return want;
}

/* ------------------------------------------------------------- geometry */

/* The largest even square crop that fits and scales to PREVIEW_SIDE exactly
 * in the PPA's 1/16 steps (VGA: 480 at 12/16, 720p: 720 at 8/16). */
static bool geometry_init(const mosaico_camera_info_t *info, geometry_t *geo)
{
    const uint32_t shorter = info->width < info->height ? info->width : info->height;
    memset(geo, 0, sizeof(*geo));
    for (uint32_t k = 1; k <= PPA_SCALE_STEPS; ++k) {
        const uint32_t numerator = PREVIEW_SIDE * PPA_SCALE_STEPS;
        if (numerator % k != 0) {
            continue;
        }
        const uint32_t crop = numerator / k;
        if (crop <= shorter && crop % 2 == 0) {
            geo->crop = crop;
            geo->scale = (float)k / (float)PPA_SCALE_STEPS;
            break;
        }
    }
    if (geo->crop == 0 || info->pixel_format != V4L2_PIX_FMT_UYVY) {
        return false;
    }
    geo->width = info->width;
    geo->height = info->height;
    geo->stride = info->bytes_per_line >= info->width * 2 ? info->bytes_per_line : info->width * 2;
    geo->crop_x = ((info->width - geo->crop) / 2) & ~1U;   /* UYVY pairs */
    geo->crop_y = (info->height - geo->crop) / 2;
    geo->qr_step = (geo->crop + QR_SIDE_MAX - 1) / QR_SIDE_MAX;
    geo->qr_side = geo->crop / geo->qr_step;
    return true;
}

/* ------------------------------------------------------------- preview */

static void preview_released(void *ctx)
{
    atomic_store(&((preview_slot_t *)ctx)->live, false);
}

static esp_err_t preview_convert(session_t *s, const mosaico_camera_frame_t *frame, uint8_t *out)
{
    const geometry_t *g = &s->geo;
    /* Rotation as the factory camera app (mosaic_camera.c): the module sits
     * sideways in the left slot. */
    const ppa_srm_oper_config_t op = {
        .in = {
            .buffer = frame->data,
            .pic_w = g->stride / 2,
            .pic_h = g->height,
            .block_w = g->crop,
            .block_h = g->crop,
            .block_offset_x = g->crop_x,
            .block_offset_y = g->crop_y,
            .srm_cm = PPA_SRM_COLOR_MODE_YUV422_UYVY,
            .yuv_range = PPA_COLOR_RANGE_LIMIT,
            .yuv_std = PPA_COLOR_CONV_STD_RGB_YUV_BT601,
        },
        .out = {
            .buffer = out,
            .buffer_size = PREVIEW_BYTES,
            .pic_w = PREVIEW_SIDE,
            .pic_h = PREVIEW_SIDE,
            .srm_cm = PPA_SRM_COLOR_MODE_RGB565,
        },
        .rotation_angle = PPA_SRM_ROTATION_ANGLE_90,
        .scale_x = g->scale,
        .scale_y = g->scale,
        .mode = PPA_TRANS_MODE_BLOCKING,
    };
    return ppa_do_scale_rotate_mirror(s->ppa, &op);
}

static void preview_show(session_t *s, const mosaico_camera_frame_t *frame)
{
    preview_slot_t *slot = NULL;
    for (int i = 0; i < PREVIEW_SLOTS; ++i) {
        if (!atomic_load(&s->slots[i].live)) {
            slot = &s->slots[i];
            break;
        }
    }
    if (slot == NULL) {
        s->skipped++;   /* GSP still holds both: it renders slower than the camera */
        return;
    }
    const esp_err_t err = preview_convert(s, frame, slot->pixels);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "preview conversion failed: %s", esp_err_to_name(err));
        s->skipped++;
        return;
    }
    esp_gsp_canvas_frame_t canvas = ESP_GSP_CANVAS_FRAME_INIT();
    canvas.pixels = slot->pixels;
    canvas.size_bytes = PREVIEW_BYTES;
    canvas.stride_bytes = PREVIEW_STRIDE;
    canvas.on_release = preview_released;
    canvas.release_ctx = slot;
    atomic_store(&slot->live, true);
    if (esp_gsp_canvas_try_submit_frame(s_cam.ui, s->bind, &canvas) != ESP_GSP_OK) {
        atomic_store(&slot->live, false);   /* not accepted: still ours */
        s->skipped++;
        return;
    }
    s->shown++;
}

/* ------------------------------------------------------------- QR feed */

/* Luma of the crop, box-averaged by qr_step, into quirc's buffer. UYVY keeps
 * Y in every odd byte. */
static void qr_fill(const session_t *s, const uint8_t *frame, uint8_t *gray)
{
    const geometry_t *g = &s->geo;
    const uint32_t step = g->qr_step;
    const uint32_t area = step * step;
    for (uint32_t gy = 0; gy < g->qr_side; ++gy) {
        const uint8_t *row = frame + (size_t)(g->crop_y + gy * step) * g->stride + (size_t)g->crop_x * 2 + 1;
        uint8_t *out = gray + (size_t)gy * g->qr_side;
        if (step == 1) {
            for (uint32_t gx = 0; gx < g->qr_side; ++gx) {
                out[gx] = row[gx * 2];
            }
            continue;
        }
        for (uint32_t gx = 0; gx < g->qr_side; ++gx) {
            uint32_t sum = 0;
            for (uint32_t dy = 0; dy < step; ++dy) {
                const uint8_t *p = row + (size_t)dy * g->stride + (size_t)gx * step * 2;
                for (uint32_t dx = 0; dx < step; ++dx) {
                    sum += p[dx * 2];
                }
            }
            out[gx] = (uint8_t)(sum / area);
        }
    }
}

static void qr_feed(session_t *s, const mosaico_camera_frame_t *frame, int64_t now)
{
    if (now < s->next_qr_us) {
        return;
    }
    uint8_t *gray = cam_qr_acquire(s->qr);
    if (gray == NULL) {
        return;   /* still decoding the previous frame */
    }
    qr_fill(s, frame->data, gray);
    cam_qr_submit(s->qr);
    s->next_qr_us = now + QR_INTERVAL_US;
}

/* count per period_us as tenths per second (no float formatting in logs) */
static uint32_t per_s10(uint32_t count, int64_t period_us)
{
    return period_us > 0 ? (uint32_t)((uint64_t)count * 10000000ULL / (uint64_t)period_us) : 0;
}

/* Adds the period's counters to the session totals and clears them. */
static void stats_fold(session_t *s, const cam_qr_stats_t *qr)
{
    s->total_frames += s->frames;
    s->total_shown += s->shown;
    s->total_codes += qr->reported;
    s->frames = s->shown = s->skipped = s->short_frames = s->restarts = 0;
}

static void stats_log(session_t *s, int64_t now)
{
    if (now < s->next_stats_us) {
        return;
    }
    const int64_t period = now - s->next_stats_us + STATS_PERIOD_US;
    cam_qr_stats_t qr;
    cam_qr_take_stats(s->qr, &qr);
    const uint32_t cam = per_s10(s->frames, period);
    const uint32_t shown = per_s10(s->shown, period);
    const uint32_t searched = per_s10(qr.decodes, period);
    ESP_LOGI(TAG, "camera %" PRIu32 ".%" PRIu32 " fps, preview %" PRIu32 ".%" PRIu32 " fps (skipped %" PRIu32
             "), QR %" PRIu32 ".%" PRIu32 "/s at %" PRIu32 " ms, %" PRIu32 " decoded (%" PRIu32 " new)",
             cam / 10, cam % 10, shown / 10, shown % 10, s->skipped, searched / 10, searched % 10,
             qr.decodes ? (uint32_t)(qr.busy_us / qr.decodes / 1000) : 0, qr.codes, qr.reported);
    if (s->short_frames != 0 || s->restarts != 0) {
        ESP_LOGW(TAG, "camera dropped %" PRIu32 " incomplete frames, %" PRIu32 " stream restarts",
                 s->short_frames, s->restarts);
    }
    stats_fold(s, &qr);
    s->next_stats_us = now + STATS_PERIOD_US;
}

/* ------------------------------------------------------------- session */

static esp_err_t session_open(session_t *s)
{
    mosaico_camera_config_t config = MOSAICO_CAMERA_DEFAULT_CONFIG();
    /* The left slot, claimed with its control GPIO (the camera's D4) for the
     * session; mosaico_camera_del() hands it back to the module manager. */
    config.slot = MOSAICO_MODULE_MGR_SLOT_LEFT;
    config.buffer_count = CAMERA_BUFFERS;
    ESP_RETURN_ON_ERROR(mosaico_camera_new(&config, &s->camera), TAG, "no camera module in the left slot");
    ESP_RETURN_ON_ERROR(mosaico_camera_open(s->camera), TAG, "camera open failed");
    mosaico_camera_info_t info = {0};
    ESP_RETURN_ON_ERROR(mosaico_camera_get_info(s->camera, &info), TAG, "camera info failed");
    ESP_RETURN_ON_FALSE(geometry_init(&info, &s->geo), ESP_ERR_NOT_SUPPORTED, TAG,
                        "unsupported camera stream %" PRIu32 "x%" PRIu32 " format 0x%08" PRIx32,
                        info.width, info.height, info.pixel_format);

    for (int i = 0; i < PREVIEW_SLOTS; ++i) {
        s->slots[i].pixels = heap_caps_aligned_calloc(PREVIEW_ALIGN, 1, PREVIEW_BYTES,
                                                      MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
        ESP_RETURN_ON_FALSE(s->slots[i].pixels, ESP_ERR_NO_MEM, TAG, "no memory for the preview");
        atomic_init(&s->slots[i].live, false);
    }
    const ppa_client_config_t ppa = {.oper_type = PPA_OPERATION_SRM, .max_pending_trans_num = 1};
    ESP_RETURN_ON_ERROR(ppa_register_client(&ppa, &s->ppa), TAG, "PPA unavailable");

    xSemaphoreTake(s_cam.lock, portMAX_DELAY);
    app_camera_code_cb_t on_code = s_cam.on_code;
    void *ctx = s_cam.ctx;
    xSemaphoreGive(s_cam.lock);
    ESP_RETURN_ON_ERROR(cam_qr_open((int)s->geo.qr_side, on_code, ctx, &s->qr), TAG, "QR decoder unavailable");

    ESP_RETURN_ON_ERROR(mosaico_camera_start_stream(s->camera), TAG, "camera stream failed");
    s->streaming = true;
    ESP_LOGI(TAG, "camera on in %lld ms: %" PRIu32 "x%" PRIu32 " @ %" PRIu32 " fps, crop %" PRIu32
             ", preview %d, QR %" PRIu32 "x%" PRIu32,
             (long long)((esp_timer_get_time() - s->started_us) / 1000), info.width, info.height,
             info.frame_rate, s->geo.crop, PREVIEW_SIDE, s->geo.qr_side, s->geo.qr_side);
    return ESP_OK;
}

/* Runs until the logic stops the session or the camera fails. */
static void session_stream(session_t *s)
{
    s->next_stats_us = esp_timer_get_time() + STATS_PERIOD_US;
    while (session_wanted()) {
        mosaico_camera_frame_t frame = {0};
        const esp_err_t err = mosaico_camera_get_frame(s->camera, &frame);
        if (err == ESP_ERR_INVALID_SIZE) {
            s->short_frames++;   /* an incomplete frame; the next one is fine */
            continue;
        }
        if (err == ESP_ERR_TIMEOUT) {
            s->restarts++;
            ESP_LOGW(TAG, "camera frame timeout; restarting the stream");
            if (mosaico_camera_restart(s->camera) != ESP_OK) {
                ESP_LOGE(TAG, "camera restart failed");
                return;
            }
            continue;
        }
        if (err != ESP_OK) {
            ESP_LOGE(TAG, "camera capture failed: %s", esp_err_to_name(err));
            return;
        }
        s->frames++;
        const int64_t now = esp_timer_get_time();
        if (frame.size >= (size_t)s->geo.stride * s->geo.height) {
            preview_show(s, &frame);
            qr_feed(s, &frame, now);
        } else {
            s->short_frames++;
        }
        if (mosaico_camera_return_frame(s->camera, &frame) != ESP_OK) {
            ESP_LOGE(TAG, "camera frame return failed");
            return;
        }
        stats_log(s, now);
    }
}

static void session_close(session_t *s)
{
    if (s->qr != NULL) {
        cam_qr_close(s->qr);
    }
    /* Back to the placeholder; GSP returns the live frame on its render task. */
    (void)esp_gsp_canvas_stop(s_cam.ui, s->bind);
    (void)esp_gsp_flush(s_cam.ui, CANVAS_RELEASE_TIMEOUT_MS);
    for (int i = 0; i < PREVIEW_SLOTS; ++i) {
        for (int waited = 0; s->slots[i].pixels != NULL && atomic_load(&s->slots[i].live) &&
                waited < CANVAS_RELEASE_TIMEOUT_MS; waited += 10) {
            vTaskDelay(pdMS_TO_TICKS(10));
        }
        if (s->slots[i].pixels != NULL && atomic_load(&s->slots[i].live)) {
            /* GSP may still read it: leaking is the safe choice. */
            ESP_LOGE(TAG, "preview frame %d not released; leaking %u bytes", i, (unsigned)PREVIEW_BYTES);
            continue;
        }
        heap_caps_free(s->slots[i].pixels);
    }
    if (s->ppa != NULL) {
        (void)ppa_unregister_client(s->ppa);
    }
    if (s->camera != NULL) {
        if (s->streaming) {
            (void)mosaico_camera_stop_stream(s->camera);
        }
        (void)mosaico_camera_close(s->camera);
        if (mosaico_camera_del(s->camera) != ESP_OK) {
            ESP_LOGE(TAG, "camera release failed; the left slot stays claimed");
        }
    }
}

static void camera_task(void *arg)
{
    (void)arg;
    for (;;) {
        session_t session = {0};
        session_t *s = &session;
        s->started_us = esp_timer_get_time();
        xSemaphoreTake(s_cam.lock, portMAX_DELAY);
        s->bind = s_cam.bind;
        xSemaphoreGive(s_cam.lock);

        const bool opened = session_open(s) == ESP_OK;
        if (opened) {
            session_stream(s);
        }
        const int64_t lasted = esp_timer_get_time() - s->started_us;
        if (s->qr != NULL) {
            cam_qr_stats_t qr;
            cam_qr_take_stats(s->qr, &qr);
            stats_fold(s, &qr);   /* the last partial period */
        }
        session_close(s);
        const uint32_t shown = per_s10(s->total_shown, lasted);
        ESP_LOGI(TAG, "camera off after %lld ms: %" PRIu32 " frames, preview %" PRIu32 ".%" PRIu32
                 " fps, %" PRIu32 " codes reported", (long long)(lasted / 1000), s->total_frames,
                 shown / 10, shown % 10, s->total_codes);

        /* Another session if the logic asked again while this one closed;
         * after a failure, wait for the next start instead of retrying. */
        xSemaphoreTake(s_cam.lock, portMAX_DELAY);
        if (!opened || !s_cam.want) {
            s_cam.running = false;
            xSemaphoreGive(s_cam.lock);
            vTaskDelete(NULL);
        }
        xSemaphoreGive(s_cam.lock);
    }
}

/* ------------------------------------------------------------- API */

esp_err_t app_camera_init(esp_gsp_handle_t ui)
{
    ESP_RETURN_ON_FALSE(ui != NULL, ESP_ERR_INVALID_ARG, TAG, "no UI");
    ESP_RETURN_ON_FALSE(s_cam.lock == NULL, ESP_ERR_INVALID_STATE, TAG, "already initialized");
    s_cam.lock = xSemaphoreCreateMutex();
    ESP_RETURN_ON_FALSE(s_cam.lock != NULL, ESP_ERR_NO_MEM, TAG, "no memory");
    s_cam.ui = ui;
    return ESP_OK;
}

esp_err_t app_camera_subscribe(app_camera_code_cb_t on_code, void *ctx)
{
    ESP_RETURN_ON_FALSE(s_cam.lock != NULL, ESP_ERR_INVALID_STATE, TAG, "not initialized");
    xSemaphoreTake(s_cam.lock, portMAX_DELAY);
    s_cam.on_code = on_code;
    s_cam.ctx = ctx;
    xSemaphoreGive(s_cam.lock);
    return ESP_OK;
}

esp_err_t app_camera_start(uint16_t preview_bind)
{
    ESP_RETURN_ON_FALSE(s_cam.lock != NULL, ESP_ERR_INVALID_STATE, TAG, "not initialized");
    esp_err_t err = ESP_OK;
    xSemaphoreTake(s_cam.lock, portMAX_DELAY);
    s_cam.want = true;
    s_cam.bind = preview_bind;
    if (!s_cam.running) {
        if (xTaskCreatePinnedToCore(camera_task, "app_camera", CAM_TASK_STACK_BYTES, NULL,
                                    CAM_TASK_PRIORITY, NULL, CAM_TASK_CORE) == pdPASS) {
            s_cam.running = true;
        } else {
            s_cam.want = false;
            err = ESP_ERR_NO_MEM;
        }
    }
    xSemaphoreGive(s_cam.lock);
    return err;
}

esp_err_t app_camera_stop(void)
{
    ESP_RETURN_ON_FALSE(s_cam.lock != NULL, ESP_ERR_INVALID_STATE, TAG, "not initialized");
    xSemaphoreTake(s_cam.lock, portMAX_DELAY);
    s_cam.want = false;
    xSemaphoreGive(s_cam.lock);
    return ESP_OK;
}
