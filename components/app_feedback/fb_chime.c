// SPDX-License-Identifier: Apache-2.0
// The `chime` task: owns the ES8311 speaker path and plays the PCM chimes
// that tools/chimes.py synthesizes at build time (16 kHz, 16-bit, mono).
//
// The codec opens for a chime and closes after CODEC_IDLE_MS of silence, so
// the amplifier is off (no hiss, no idle current) between chimes. A write
// blocks for the length of the chime, which is why this is its own task.
#include <stdint.h>

#include "bsp/esp_mosaico.h"
#include "esp_codec_dev.h"
#include "esp_log.h"
#include "fb_internal.h"
#include "freertos/queue.h"

static const char *TAG = FB_TAG;

#define CHIME_TASK_PRIORITY 3
#define CHIME_TASK_STACK    (3 * 1024 + 512)
#define SAMPLE_RATE_HZ      16000 /* tools/chimes.py SAMPLE_RATE */
#define CODEC_VOLUME        70    /* percent, esp_codec_dev output volume */
#define CODEC_IDLE_MS       3000
#define WRITE_CHUNK_BYTES   1024

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
static bool s_open;

static bool speaker_init(void)
{
    /* Speaker only: the microphone input pin stays free (Farol has no use for it). */
    i2s_std_config_t i2s = {
        .clk_cfg = I2S_STD_CLK_DEFAULT_CONFIG(SAMPLE_RATE_HZ),
        .slot_cfg = I2S_STD_PHILIPS_SLOT_DEFAULT_CONFIG(I2S_DATA_BIT_WIDTH_16BIT, I2S_SLOT_MODE_STEREO),
        .gpio_cfg = {
            .mclk = BSP_AUDIO_I2S_MCLK,
            .bclk = BSP_AUDIO_I2S_SCLK,
            .ws = BSP_AUDIO_I2S_LRCLK,
            .dout = BSP_AUDIO_I2S_SDOUT,
            .din = GPIO_NUM_NC,
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

static void chime_task(void *arg)
{
    (void)arg;
    /* BSP bring-up is not thread-safe: wait until the feedback task is done
     * with its own (fb_chime_release). */
    (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
    const bool ready = speaker_init();
    for (;;) {
        app_feedback_chime_t chime;
        const TickType_t wait = s_open ? pdMS_TO_TICKS(CODEC_IDLE_MS) : portMAX_DELAY;
        if (xQueueReceive(s_queue, &chime, wait) != pdTRUE) {
            codec_close();
        } else if (ready) {
            play(chime);
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
    return ESP_OK;
}

void fb_chime_release(void)
{
    if (s_task) {
        xTaskNotifyGive(s_task);
    }
}
