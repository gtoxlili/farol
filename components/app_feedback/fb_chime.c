// SPDX-License-Identifier: Apache-2.0
// The `chime` task: owns the ES8311 codec. It plays the PCM chimes that
// tools/chimes.py synthesizes at build time (16 kHz, 16-bit, mono) and records
// the microphone for dictation, one at a time (a single owner of the codec and
// its I2C registers; the speaker sits next to the microphone anyway).
//
// The speaker opens for a chime and closes after CODEC_IDLE_MS of silence, so
// the amplifier is off (no hiss, no idle current) between chimes. A write
// blocks for the length of the chime, which is why this is its own task.
// Recording reads MIC_READ_BYTES at a time and hands APP_FEEDBACK_AUDIO_CHUNK
// bytes to the audio callback; both directions run at the same 16 kHz, as
// they share one I2S clock.
#include <stdatomic.h>
#include <stdint.h>

#include "bsp/esp_mosaico.h"
#include "esp_codec_dev.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "fb_internal.h"
#include "freertos/queue.h"

static const char *TAG = FB_TAG;

#define CHIME_TASK_PRIORITY 3
#define CHIME_TASK_STACK    (3 * 1024 + 512)
#define SAMPLE_RATE_HZ      16000 /* tools/chimes.py SAMPLE_RATE */
#define CODEC_VOLUME        70    /* percent, esp_codec_dev output volume */
#define CODEC_IDLE_MS       3000
#define WRITE_CHUNK_BYTES   1024
#define MIC_GAIN_DB         30.0f /* the BSP examples' gain */
#define MIC_READ_BYTES      800   /* 25 ms; APP_FEEDBACK_AUDIO_CHUNK is a multiple */
_Static_assert(APP_FEEDBACK_AUDIO_CHUNK % MIC_READ_BYTES == 0, "whole reads per chunk");

/* Task notification bits of the chime task. */
#define NOTIFY_START  BIT0 /* fb_chime_release: BSP bring-up may start */
#define NOTIFY_CHIME  BIT1 /* a chime is waiting in s_queue */
#define NOTIFY_LISTEN BIT2 /* s_listen changed */

extern const uint8_t chime_call_pcm_start[] asm("_binary_chime_call_pcm_start");
extern const uint8_t chime_call_pcm_end[] asm("_binary_chime_call_pcm_end");
extern const uint8_t chime_done_pcm_start[] asm("_binary_chime_done_pcm_start");
extern const uint8_t chime_done_pcm_end[] asm("_binary_chime_done_pcm_end");
extern const uint8_t chime_fail_pcm_start[] asm("_binary_chime_fail_pcm_start");
extern const uint8_t chime_fail_pcm_end[] asm("_binary_chime_fail_pcm_end");

typedef struct {
    const uint8_t *start;
    const uint8_t *end;
    const char *name;
} chime_pcm_t;

static const chime_pcm_t s_chimes[] = {
    [APP_FEEDBACK_CHIME_CALL] = {chime_call_pcm_start, chime_call_pcm_end, "call"},
    [APP_FEEDBACK_CHIME_DONE] = {chime_done_pcm_start, chime_done_pcm_end, "done"},
    [APP_FEEDBACK_CHIME_FAIL] = {chime_fail_pcm_start, chime_fail_pcm_end, "fail"},
};

/* One waiting chime; a newer request overwrites it (any task posts). */
static QueueHandle_t s_queue;

static TaskHandle_t s_task;

/* Chime task state. */
static esp_codec_dev_handle_t s_speaker;
static esp_codec_dev_handle_t s_mic;
static bool s_open;

/* Recording requests, from any task: a recording was asked for, and whether
 * it should still run. Each request gets exactly one end-of-recording call. */
static atomic_bool s_listen_asked;
static atomic_bool s_listen;
static app_feedback_audio_cb_t s_on_audio;
static void *s_audio_ctx;

static bool speaker_init(void)
{
    /* Both directions on I2S0: the speaker out, the microphone in. */
    i2s_std_config_t i2s = {
        .clk_cfg = I2S_STD_CLK_DEFAULT_CONFIG(SAMPLE_RATE_HZ),
        .slot_cfg = I2S_STD_PHILIPS_SLOT_DEFAULT_CONFIG(I2S_DATA_BIT_WIDTH_16BIT, I2S_SLOT_MODE_STEREO),
        .gpio_cfg = {
            .mclk = BSP_AUDIO_I2S_MCLK,
            .bclk = BSP_AUDIO_I2S_SCLK,
            .ws = BSP_AUDIO_I2S_LRCLK,
            .dout = BSP_AUDIO_I2S_SDOUT,
            .din = BSP_AUDIO_I2S_DSIN,
        },
    };
    i2s.clk_cfg.mclk_multiple = I2S_MCLK_MULTIPLE_256;
    const esp_err_t err = bsp_audio_init(&i2s);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "speaker I2S init failed (%s); chimes off", esp_err_to_name(err));
        return false;
    }
    s_speaker = bsp_audio_codec_speaker_init();
    if (!s_speaker) {
        ESP_LOGW(TAG, "ES8311 speaker init failed; chimes off");
        return false;
    }
    return true;
}

static bool codec_open(void)
{
    if (s_open) {
        return true;
    }
    esp_codec_dev_sample_info_t format = {
        .sample_rate = SAMPLE_RATE_HZ,
        .bits_per_sample = 16,
        .channel = 1,
    };
    int ret = esp_codec_dev_open(s_speaker, &format);
    if (ret == ESP_CODEC_DEV_OK) {
        ret = esp_codec_dev_set_out_vol(s_speaker, CODEC_VOLUME);
        if (ret != ESP_CODEC_DEV_OK) {
            (void)esp_codec_dev_close(s_speaker);
        }
    }
    if (ret != ESP_CODEC_DEV_OK) {
        ESP_LOGW(TAG, "speaker open failed (%d)", ret);
        return false;
    }
    s_open = true;
    return true;
}

static void codec_close(void)
{
    if (s_open) {
        (void)esp_codec_dev_close(s_speaker);
        s_open = false;
    }
}

static void play(app_feedback_chime_t chime)
{
    const chime_pcm_t *pcm = &s_chimes[chime];
    if (!codec_open()) {
        return;
    }
    ESP_LOGI(TAG, "chime %s", pcm->name);
    for (const uint8_t *at = pcm->start; at < pcm->end; at += WRITE_CHUNK_BYTES) {
        const int length = pcm->end - at < WRITE_CHUNK_BYTES ? (int)(pcm->end - at) : WRITE_CHUNK_BYTES;
        /* esp_codec_dev_write takes a mutable pointer but only reads it. */
        const int ret = esp_codec_dev_write(s_speaker, (void *)at, length);
        if (ret != ESP_CODEC_DEV_OK) {
            ESP_LOGW(TAG, "chime write failed (%d)", ret);
            codec_close();
            return;
        }
    }
}

static void audio_end(void)
{
    if (s_on_audio) {
        s_on_audio(NULL, 0, s_audio_ctx);
    }
}

/* One recording, until app_feedback_listen(false) or the limit. Chimes asked
 * for meanwhile are dropped: they would be in the recording. */
static void record(void)
{
    codec_close();
    uint8_t *buf = heap_caps_malloc(APP_FEEDBACK_AUDIO_CHUNK, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    esp_codec_dev_sample_info_t format = {
        .sample_rate = SAMPLE_RATE_HZ,
        .bits_per_sample = 16,
        .channel = 1,
    };
    if (!buf || esp_codec_dev_open(s_mic, &format) != ESP_CODEC_DEV_OK) {
        ESP_LOGW(TAG, "microphone open failed%s", buf ? "" : " (no memory)");
        heap_caps_free(buf);
        audio_end();
        return;
    }
    (void)esp_codec_dev_set_in_gain(s_mic, MIC_GAIN_DB);
    ESP_LOGI(TAG, "listening");
    const int64_t until = esp_timer_get_time() / 1000 + APP_FEEDBACK_LISTEN_MAX_MS;
    size_t filled = 0;
    size_t total = 0;
    while (atomic_load(&s_listen) && esp_timer_get_time() / 1000 < until) {
        const int ret = esp_codec_dev_read(s_mic, buf + filled, MIC_READ_BYTES);
        if (ret != ESP_CODEC_DEV_OK) {
            ESP_LOGW(TAG, "microphone read failed (%d)", ret);
            break;
        }
        filled += MIC_READ_BYTES;
        if (filled == APP_FEEDBACK_AUDIO_CHUNK) {
            s_on_audio(buf, filled, s_audio_ctx);
            total += filled;
            filled = 0;
        }
        app_feedback_chime_t dropped;
        while (xQueueReceive(s_queue, &dropped, 0) == pdTRUE) {
        }
    }
    if (filled > 0) {
        s_on_audio(buf, filled, s_audio_ctx);
        total += filled;
    }
    (void)esp_codec_dev_close(s_mic);
    heap_caps_free(buf);
    atomic_store(&s_listen, false);
    ESP_LOGI(TAG, "listened %u ms", (unsigned)(total / (SAMPLE_RATE_HZ * 2 / 1000)));
    audio_end();
}

static void chime_task(void *arg)
{
    (void)arg;
    /* BSP bring-up is not thread-safe: wait until the feedback task is done
     * with its own (fb_chime_release). Chime and listen bits stay pending. */
    uint32_t bits = 0;
    do {
        (void)xTaskNotifyWait(0, NOTIFY_START, &bits, portMAX_DELAY);
    } while (!(bits & NOTIFY_START));
    const bool ready = speaker_init();
    if (ready) {
        s_mic = bsp_audio_codec_microphone_init();
        if (!s_mic) {
            ESP_LOGW(TAG, "ES8311 microphone init failed; dictation off");
        }
    }
    for (;;) {
        if (atomic_exchange(&s_listen_asked, false)) {
            if (s_mic && s_on_audio) {
                record();
            } else {
                audio_end();
            }
            continue;
        }
        app_feedback_chime_t chime;
        if (xQueueReceive(s_queue, &chime, 0) == pdTRUE) {
            if (ready) {
                play(chime);
            }
            continue;
        }
        const TickType_t wait = s_open ? pdMS_TO_TICKS(CODEC_IDLE_MS) : portMAX_DELAY;
        if (xTaskNotifyWait(0, UINT32_MAX, &bits, wait) != pdTRUE) {
            codec_close();
        }
    }
}

esp_err_t fb_chime_start(void)
{
    s_queue = xQueueCreate(1, sizeof(app_feedback_chime_t));
    if (!s_queue) {
        return ESP_ERR_NO_MEM;
    }
    if (xTaskCreate(chime_task, "chime", CHIME_TASK_STACK, NULL, CHIME_TASK_PRIORITY, &s_task) != pdPASS) {
        vQueueDelete(s_queue);
        s_queue = NULL;
        return ESP_ERR_NO_MEM;
    }
    return ESP_OK;
}

esp_err_t fb_chime_post(app_feedback_chime_t chime)
{
    if (!s_queue) {
        return ESP_ERR_INVALID_STATE;
    }
    (void)xQueueOverwrite(s_queue, &chime);
    (void)xTaskNotify(s_task, NOTIFY_CHIME, eSetBits);
    return ESP_OK;
}

void fb_chime_release(void)
{
    if (s_task) {
        (void)xTaskNotify(s_task, NOTIFY_START, eSetBits);
    }
}

esp_err_t fb_mic_on_audio(app_feedback_audio_cb_t on_audio, void *ctx)
{
    if (!on_audio) {
        return ESP_ERR_INVALID_ARG;
    }
    s_audio_ctx = ctx;
    s_on_audio = on_audio;
    return ESP_OK;
}

esp_err_t fb_mic_listen(bool on)
{
    if (!s_task) {
        return ESP_ERR_INVALID_STATE;
    }
    atomic_store(&s_listen, on);
    if (on) {
        atomic_store(&s_listen_asked, true);
    }
    (void)xTaskNotify(s_task, NOTIFY_LISTEN, eSetBits);
    return ESP_OK;
}
