// SPDX-License-Identifier: Apache-2.0
// The interaction module in the right slot, on the `feedback` task: hot-plug,
// PIR, ambient light, the two touch keys and the six WS2812 LEDs.
//
// Hot-plug: the BSP's module manager probes both slots' EEPROMs every 250 ms
// on its own task (3 matching probes to change state). This file reads the
// right slot's snapshot every HOTPLUG_POLL_MS; an Interaction descriptor there
// opens the driver (which claims the slot), and ABSENT closes it. The manager
// keeps probing a claimed Interaction board, so removal is seen while open.
// The left slot (camera) is never claimed or touched here.
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#include "esp_log.h"
#include "fb_internal.h"
#include "mosaico_module_interact.h"
#include "mosaico_module_mgr.h"

static const char *TAG = FB_TAG;

#define MODULE_SLOT         MOSAICO_MODULE_MGR_SLOT_RIGHT
#define HOTPLUG_POLL_MS     250
#define OPEN_RETRY_MS       5000
#define OPEN_TIMEOUT_MS     200   /* the descriptor is already valid when we claim */
#define INPUT_POLL_MS       50    /* touch-key taps last about 100 ms */
#define PIR_CONFIRM_SAMPLES 2
#define AMBIENT_STEP        4     /* report light changes of at least this much */
#define AMBIENT_MIN_GAP_MS  1000
/* Driver-level brightness for 100 % (of 255): bounds current and glare; the
 * logic's LED percentage scales below it. */
#define LED_HARDWARE_CAP    96
#define FRAME_MS            33

/* Glow look (RGB at full scale, levels 0..1 before the brightness cap). */
typedef struct {
    uint8_t r, g, b;
    float low, high;  /* breathing range; equal = steady */
    uint32_t period_ms;
} glow_look_t;

static const glow_look_t s_looks[] = {
    [APP_FEEDBACK_GLOW_OFF] = {0, 0, 0, 0.0f, 0.0f, 0},
    /* Cool moonlight blue, very dim and slow: work in progress. */
    [APP_FEEDBACK_GLOW_WORK] = {70, 130, 255, 0.06f, 0.28f, 4000},
    /* Lamp-fire amber, slow breathing: someone is waiting. */
    [APP_FEEDBACK_GLOW_CALL] = {255, 130, 20, 0.15f, 1.0f, 2400},
    /* Ember red, steady until read. */
    [APP_FEEDBACK_GLOW_FAIL] = {255, 28, 8, 0.75f, 0.75f, 0},
};

/* The finished-turn flash: rise, hold, fade, over whatever glows below. */
#define FLASH_RISE_MS 60
#define FLASH_HOLD_MS 140
#define FLASH_FADE_MS 400

static const char *const s_glow_names[] = {"off", "work", "call", "fail", "flash"};

static struct {
    bool mgr_ok;
    mosaico_interact_handle_t handle;
    bool closing;   /* removed, but the driver has not released yet */
    int reported;   /* last Module report: -1 none, 0 absent, 1 attached */
    int64_t next_hotplug;
    int64_t retry_at;
    int64_t next_input;
    bool left, right;
    bool near;
    int pir_samples;
    bool pir_reported;
    int ambient;    /* last reported, -1 none */
    int64_t ambient_at;
    app_feedback_glow_t glow;
    int64_t glow_since;
    int64_t flash_at; /* 0 = no flash */
    uint8_t led_percent;
    mosaico_interact_rgb_t shown[MOSAICO_INTERACT_LED_COUNT];
    bool shown_valid;
} s_mod = {.reported = -1, .ambient = -1, .led_percent = 60};

void fb_module_init(void)
{
    const esp_err_t err = mosaico_module_mgr_init(NULL);
    s_mod.mgr_ok = err == ESP_OK;
    if (!s_mod.mgr_ok) {
        ESP_LOGW(TAG, "module slots unavailable (%s); running without the module",
                 esp_err_to_name(err));
    }
}

/* ------------------------------------------------------------- LEDs */

static float glow_level(const glow_look_t *look, int64_t since, int64_t now)
{
    if (look->period_ms == 0) {
        return look->high;
    }
    const float phase = (float)((now - since) % look->period_ms) / (float)look->period_ms;
    /* Start dark, ease in and out; squared so the dim end lingers like a breath. */
    const float wave = 0.5f - 0.5f * cosf(2.0f * (float)M_PI * phase);
    return look->low + (look->high - look->low) * wave * wave;
}

static float flash_level(int64_t now)
{
    if (s_mod.flash_at == 0) {
        return 0.0f;
    }
    const int64_t t = now - s_mod.flash_at;
    if (t < FLASH_RISE_MS) {
        return (float)t / FLASH_RISE_MS;
    }
    if (t < FLASH_RISE_MS + FLASH_HOLD_MS) {
        return 1.0f;
    }
    if (t < FLASH_RISE_MS + FLASH_HOLD_MS + FLASH_FADE_MS) {
        return 1.0f - (float)(t - FLASH_RISE_MS - FLASH_HOLD_MS) / FLASH_FADE_MS;
    }
    s_mod.flash_at = 0;
    return 0.0f;
}

static uint8_t channel(uint8_t base, float level, float flash)
{
    const float value = ((float)base * level * (1.0f - flash) + 255.0f * flash) * (float)s_mod.led_percent / 100.0f;
    return (uint8_t)(value > 255.0f ? 255.0f : value + 0.5f);
}

/* Draws the current frame; returns when the next one is due. */
static int64_t render(int64_t now)
{
    if (!s_mod.handle || s_mod.closing) {
        return FB_NEVER;
    }
    const glow_look_t *look = &s_looks[s_mod.glow];
    const float level = glow_level(look, s_mod.glow_since, now);
    const float flash = flash_level(now);
    const mosaico_interact_rgb_t color = {
        channel(look->r, level, flash), channel(look->g, level, flash), channel(look->b, level, flash),
    };
    mosaico_interact_rgb_t frame[MOSAICO_INTERACT_LED_COUNT];
    bool same = s_mod.shown_valid;
    for (size_t i = 0; i < MOSAICO_INTERACT_LED_COUNT; ++i) {
        frame[i] = color;
        same = same && s_mod.shown[i].r == color.r && s_mod.shown[i].g == color.g && s_mod.shown[i].b == color.b;
    }
    if (!same) {
        const esp_err_t err = mosaico_interact_led_write(s_mod.handle, frame, MOSAICO_INTERACT_LED_COUNT);
        s_mod.shown_valid = err == ESP_OK;
        if (s_mod.shown_valid) {
            for (size_t i = 0; i < MOSAICO_INTERACT_LED_COUNT; ++i) {
                s_mod.shown[i] = frame[i];
            }
        }
    }
    const bool animating = s_mod.flash_at != 0 || look->period_ms != 0;
    return animating ? now + FRAME_MS : FB_NEVER;
}

void fb_module_glow(app_feedback_glow_t glow, int64_t now)
{
    if (glow == APP_FEEDBACK_GLOW_FLASH) {
        if (s_mod.handle && !s_mod.closing) {
            s_mod.flash_at = now;
            ESP_LOGI(TAG, "glow flash");
        }
        return;
    }
    if (glow != s_mod.glow) {
        s_mod.glow = glow;
        s_mod.glow_since = now;
        if (s_mod.handle) {
            ESP_LOGI(TAG, "glow %s", s_glow_names[glow]);
        }
    }
}

void fb_module_led_brightness(uint8_t percent)
{
    if (percent != s_mod.led_percent) {
        s_mod.led_percent = percent;
        ESP_LOGI(TAG, "LED brightness %u%%", percent);
    }
}

/* ------------------------------------------------------------- attach */

static void report_module(bool attached)
{
    if (s_mod.reported != (int)attached) {
        s_mod.reported = attached;
        fb_emit(APP_FEEDBACK_EVENT_MODULE, attached);
    }
}

static void attach(int64_t now)
{
    const mosaico_interact_config_t config = {
        .slot = MODULE_SLOT,
        .discovery_timeout_ms = OPEN_TIMEOUT_MS,
        .button_mode = MOSAICO_INTERACT_BUTTON_MODE_AUTO,
        .led_brightness = LED_HARDWARE_CAP,
    };
    mosaico_interact_handle_t handle = NULL;
    const esp_err_t err = mosaico_interact_open(&config, &handle);
    if (err != ESP_OK) {
        if (handle) {
            /* The driver could not release what it took; finish that first. */
            s_mod.handle = handle;
            s_mod.closing = true;
        }
        s_mod.retry_at = now + OPEN_RETRY_MS;
        ESP_LOGW(TAG, "module present but not opened (%s); retry in %d s", esp_err_to_name(err),
                 OPEN_RETRY_MS / 1000);
        return;
    }
    mosaico_interact_info_t info = {0};
    (void)mosaico_interact_get_info(handle, &info);
    s_mod.handle = handle;
    s_mod.closing = false;
    s_mod.left = s_mod.right = false;
    s_mod.near = false;
    s_mod.pir_samples = 0;
    s_mod.pir_reported = false;
    s_mod.ambient = -1;
    s_mod.glow = APP_FEEDBACK_GLOW_OFF; /* the logic sends the glow it wants */
    s_mod.flash_at = 0;
    s_mod.shown_valid = false;
    s_mod.next_input = now;
    ESP_LOGI(TAG, "module attached (%s keys)",
             info.button_mode == MOSAICO_INTERACT_BUTTON_MODE_TOUCH ? "touch" : "GPIO");
    report_module(true);
}

static void detach(void)
{
    if (!s_mod.closing) {
        s_mod.closing = true;
        ESP_LOGI(TAG, "module removed");
        report_module(false);
    }
    const esp_err_t err = mosaico_interact_close(s_mod.handle);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "module release failed (%s); retrying", esp_err_to_name(err));
        return;
    }
    s_mod.handle = NULL;
    s_mod.closing = false;
}

static void hotplug(int64_t now)
{
    mosaico_module_mgr_info_t info;
    if (mosaico_module_mgr_get_info(MODULE_SLOT, &info) != ESP_OK) {
        return;
    }
    if (s_mod.handle) {
        if (s_mod.closing || info.presence == MOSAICO_MODULE_PRESENCE_ABSENT) {
            detach();
        }
        return;
    }
    if (info.presence == MOSAICO_MODULE_PRESENCE_UNKNOWN ||
            (info.presence == MOSAICO_MODULE_PRESENCE_PRESENT &&
             info.descriptor_state == MOSAICO_MODULE_DESCRIPTOR_UNKNOWN)) {
        return; /* still settling */
    }
    const bool interact = info.presence == MOSAICO_MODULE_PRESENCE_PRESENT &&
                          info.descriptor_state == MOSAICO_MODULE_DESCRIPTOR_VALID &&
                          info.eeprom.board_type == MOSAICO_BOARD_TYPE_INTERACT;
    if (!interact) {
        if (s_mod.reported != 0 && info.presence == MOSAICO_MODULE_PRESENCE_PRESENT) {
            ESP_LOGI(TAG, "right slot holds a %s board; running without the module",
                     mosaico_module_mgr_type_to_name(info.eeprom.board_type));
        }
        s_mod.retry_at = 0;
        report_module(false);
        return;
    }
    if (now >= s_mod.retry_at) {
        attach(now);
    }
    if (!s_mod.handle) {
        report_module(false);
    }
}

/* ------------------------------------------------------------- inputs */

static void read_inputs(int64_t now)
{
    mosaico_interact_inputs_t in;
    if (mosaico_interact_read_inputs(s_mod.handle, &in) != ESP_OK) {
        return; /* a removed board fails here until the manager reports ABSENT */
    }
    /* Keys report on press. */
    if (in.left_pressed && !s_mod.left) {
        ESP_LOGI(TAG, "key prev");
        fb_emit(APP_FEEDBACK_EVENT_BUTTON, APP_FEEDBACK_BUTTON_PREV);
    }
    if (in.right_pressed && !s_mod.right) {
        ESP_LOGI(TAG, "key next");
        fb_emit(APP_FEEDBACK_EVENT_BUTTON, APP_FEEDBACK_BUTTON_NEXT);
    }
    s_mod.left = in.left_pressed;
    s_mod.right = in.right_pressed;

    /* PIR: a change holds for PIR_CONFIRM_SAMPLES; the first state reports at once. */
    if (in.motion_detected != s_mod.near || !s_mod.pir_reported) {
        if (++s_mod.pir_samples >= PIR_CONFIRM_SAMPLES || !s_mod.pir_reported) {
            s_mod.near = in.motion_detected;
            s_mod.pir_reported = true;
            s_mod.pir_samples = 0;
            ESP_LOGI(TAG, "presence %s", s_mod.near ? "near" : "away");
            fb_emit(APP_FEEDBACK_EVENT_PRESENCE, s_mod.near);
        }
    } else {
        s_mod.pir_samples = 0;
    }

    /* Light: the driver filters it; report steps, at most once a second. */
    const int level = in.light_level > 100 ? 100 : in.light_level;
    if (s_mod.ambient < 0 ||
            (abs(level - s_mod.ambient) >= AMBIENT_STEP && now - s_mod.ambient_at >= AMBIENT_MIN_GAP_MS)) {
        s_mod.ambient = level;
        s_mod.ambient_at = now;
        ESP_LOGD(TAG, "ambient %d", level);
        fb_emit(APP_FEEDBACK_EVENT_AMBIENT, level);
    }
}

int64_t fb_module_service(int64_t now)
{
    if (!s_mod.mgr_ok) {
        return FB_NEVER;
    }
    if (now >= s_mod.next_hotplug) {
        s_mod.next_hotplug = now + HOTPLUG_POLL_MS;
        hotplug(now);
    }
    int64_t next = s_mod.next_hotplug;
    if (s_mod.handle && !s_mod.closing) {
        if (now >= s_mod.next_input) {
            s_mod.next_input = now + INPUT_POLL_MS;
            read_inputs(now);
        }
        next = s_mod.next_input < next ? s_mod.next_input : next;
        const int64_t frame = render(now);
        next = frame < next ? frame : next;
    }
    return next;
}

void fb_module_describe(char *out, size_t size)
{
    if (!s_mod.mgr_ok) {
        snprintf(out, size, "slots off");
    } else if (!s_mod.handle || s_mod.closing) {
        snprintf(out, size, "absent");
    } else {
        snprintf(out, size, "attached presence=%s ambient=%d glow=%s led=%u%%", s_mod.near ? "near" : "away",
                 s_mod.ambient, s_glow_names[s_mod.glow], s_mod.led_percent);
    }
}
