// SPDX-License-Identifier: Apache-2.0
/* Farol's scripted input and the fake esp_lcd_touch device that replays it.
 * GSP polls the device on its render task; every read samples the script on a
 * 10 ms grid of esp_timer time since the run started. Coordinates are screen
 * pixels of the 480x480 panel (36 px status bar at the top). */

#include <stdlib.h>
#include <string.h>

#include "app_bench.h"
#include "bench_internal.h"
#include "esp_check.h"
#include "esp_heap_caps.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"

#define SAMPLE_PERIOD_MS 10
#define SCREEN_MAX_PX 479

#define WARMUP_MS 5000
#define IDLE_HOME_MS 5000
#define DRAG_MS 300
#define SETTLE_MS 600
#define DECK_IDLE_MS 3000
#define RUNNING_IDLE_MS 3000
#define STOP_HOLD_MS 1700   /* stop ring: appears after 250 ms, animates 1.2 s */
#define USAGE_IDLE_MS 2000
#define IDLE_END_MS 5000

/* A press, a linear move and a release one sample after the end point, so
 * the last pressed sample sits exactly on the end coordinate. */
#define RELEASE_AFTER(move_ms) ((move_ms) + SAMPLE_PERIOD_MS)
/* One drag and the settle after its release. */
#define DRAG_STEP_MS (RELEASE_AFTER(DRAG_MS) + SETTLE_MS)

#define T_IDLE_HOME WARMUP_MS
#define T_TO_DECK (T_IDLE_HOME + IDLE_HOME_MS)
#define T_DECK_IDLE (T_TO_DECK + DRAG_STEP_MS)
#define T_FLICK_1 (T_DECK_IDLE + DECK_IDLE_MS)
#define T_FLICK_2 (T_FLICK_1 + DRAG_STEP_MS)
#define T_FLICK_3 (T_FLICK_2 + DRAG_STEP_MS)
#define T_RUNNING_IDLE (T_FLICK_3 + DRAG_STEP_MS)
#define T_HOLD_STOP (T_RUNNING_IDLE + RUNNING_IDLE_MS)
#define T_TO_HOME (T_HOLD_STOP + STOP_HOLD_MS + SETTLE_MS)
#define T_DRAWER_DOWN (T_TO_HOME + DRAG_STEP_MS)
#define T_DRAWER_UP (T_DRAWER_DOWN + DRAG_STEP_MS)
#define T_TO_USAGE (T_DRAWER_UP + DRAG_STEP_MS)
#define T_USAGE_BACK (T_TO_USAGE + DRAG_STEP_MS + USAGE_IDLE_MS)
#define T_IDLE_END (T_USAGE_BACK + DRAG_STEP_MS)
#define T_RUN_END (T_IDLE_END + IDLE_END_MS)

static const bench_phase_window_t s_phases[BENCH_PHASE_COUNT] = {
    [BENCH_PHASE_WARMUP] = {"warmup", 0, T_IDLE_HOME},
    [BENCH_PHASE_IDLE_HOME] = {"idle_home", T_IDLE_HOME, T_TO_DECK},
    [BENCH_PHASE_TO_DECK] = {"to_deck", T_TO_DECK, T_DECK_IDLE},
    [BENCH_PHASE_DECK_IDLE] = {"deck_idle", T_DECK_IDLE, T_FLICK_1},
    [BENCH_PHASE_DECK_FLICK] = {"deck_flick", T_FLICK_1, T_RUNNING_IDLE},
    [BENCH_PHASE_RUNNING_IDLE] = {"running_idle", T_RUNNING_IDLE, T_HOLD_STOP},
    [BENCH_PHASE_HOLD_STOP] = {"hold_stop", T_HOLD_STOP, T_TO_HOME},
    [BENCH_PHASE_TO_HOME] = {"to_home", T_TO_HOME, T_DRAWER_DOWN},
    [BENCH_PHASE_CONTROL] = {"control", T_DRAWER_DOWN, T_TO_USAGE},
    [BENCH_PHASE_TO_USAGE] = {"to_usage", T_TO_USAGE, T_IDLE_END},
    [BENCH_PHASE_IDLE_END] = {"idle_end", T_IDLE_END, T_RUN_END},
};

typedef struct {
    uint32_t start_ms;
    uint32_t move_ms;   /* 0 for a press in place */
    uint32_t hold_ms;   /* pressed for [start, start + hold) */
    int16_t x0, y0, x1, y1;
} gesture_t;

#define DRAG(t, x0, y0, x1, y1) {(t), DRAG_MS, RELEASE_AFTER(DRAG_MS), (x0), (y0), (x1), (y1)}

static const gesture_t s_gestures[] = {
    DRAG(T_TO_DECK, 400, 300, 80, 300),                       /* screens: home -> deck */
    DRAG(T_FLICK_1, 240, 380, 240, 100),                      /* cards: next */
    DRAG(T_FLICK_2, 240, 380, 240, 100),                      /* cards: next */
    DRAG(T_FLICK_3, 240, 380, 240, 100),                      /* cards: the running agent */
    {T_HOLD_STOP, 0, STOP_HOLD_MS, 240, 420, 240, 420},       /* hold to stop */
    DRAG(T_TO_HOME, 80, 300, 400, 300),                       /* screens: deck -> home */
    DRAG(T_DRAWER_DOWN, 240, 6, 240, 380),                    /* control drawer down */
    DRAG(T_DRAWER_UP, 240, 370, 240, 20),                     /* control drawer up */
    DRAG(T_TO_USAGE, 80, 300, 400, 300),                      /* screens: home -> usage */
    DRAG(T_USAGE_BACK, 400, 300, 80, 300),                    /* screens: usage -> home */
};

const bench_phase_window_t *bench_script_phases(void)
{
    return s_phases;
}

uint32_t bench_script_run_ms(void)
{
    return T_RUN_END;
}

/* Run clock: written by the bench task, read by the render task's touch poll. */
static portMUX_TYPE s_clock_lock = portMUX_INITIALIZER_UNLOCKED;
static int64_t s_run_t0_us;
static bool s_run_active;

void bench_clock_start_run(int64_t t0_us)
{
    portENTER_CRITICAL(&s_clock_lock);
    s_run_t0_us = t0_us;
    s_run_active = true;
    portEXIT_CRITICAL(&s_clock_lock);
}

void bench_clock_stop(void)
{
    portENTER_CRITICAL(&s_clock_lock);
    s_run_active = false;
    portEXIT_CRITICAL(&s_clock_lock);
}

static bool clock_read(int64_t *out_t0_us)
{
    portENTER_CRITICAL(&s_clock_lock);
    const bool active = s_run_active;
    *out_t0_us = s_run_t0_us;
    portEXIT_CRITICAL(&s_clock_lock);
    return active;
}

typedef struct {
    bool pressed;
    uint16_t x, y;
} contact_t;

static contact_t script_at(uint32_t t_ms)
{
    for (size_t i = 0; i < sizeof(s_gestures) / sizeof(s_gestures[0]); ++i) {
        const gesture_t *g = &s_gestures[i];
        if (t_ms < g->start_ms || t_ms >= g->start_ms + g->hold_ms) {
            continue;
        }
        const uint32_t dt = t_ms - g->start_ms;
        const int32_t num = (int32_t)(dt < g->move_ms ? dt : g->move_ms);
        const int32_t den = g->move_ms ? (int32_t)g->move_ms : 1;
        return (contact_t) {
            .pressed = true,
            .x = (uint16_t)(g->x0 + (g->x1 - g->x0) * num / den),
            .y = (uint16_t)(g->y0 + (g->y1 - g->y0) * num / den),
        };
    }
    return (contact_t) {
        0
    };
}

/* The device embeds the esp_lcd_touch base; GSP owns the poll timing. */
typedef struct {
    esp_lcd_touch_t base;
    contact_t current;
    int64_t last_grid_us;   /* absolute time of the last delivered grid sample */
    bool last_pressed;
} script_touch_t;

static esp_err_t script_read_data(esp_lcd_touch_handle_t tp)
{
    script_touch_t *touch = __containerof(tp, script_touch_t, base);
    int64_t t0_us;
    if (!clock_read(&t0_us)) {
        touch->current = (contact_t) {
            0
        };
        touch->last_pressed = false;
        return ESP_OK;
    }
    const int64_t now_us = esp_timer_get_time();
    const int64_t elapsed_ms = now_us > t0_us ? (now_us - t0_us) / 1000 : 0;
    const int64_t grid_ms = elapsed_ms - elapsed_ms % SAMPLE_PERIOD_MS;
    const int64_t grid_us = t0_us + grid_ms * 1000;
    touch->current = script_at((uint32_t)grid_ms);
    /* A new grid sample is delivered now. Presses, moves and the release
     * sample count for latency; idle polls do not. */
    if (grid_us != touch->last_grid_us) {
        if (touch->current.pressed || touch->last_pressed) {
            bench_probe_touch_sample(grid_us);
        }
        touch->last_grid_us = grid_us;
        touch->last_pressed = touch->current.pressed;
    }
    return ESP_OK;
}

static bool script_get_xy(esp_lcd_touch_handle_t tp, uint16_t *x, uint16_t *y,
                          uint16_t *strength, uint8_t *point_num, uint8_t max_point_num)
{
    script_touch_t *touch = __containerof(tp, script_touch_t, base);
    if (!touch->current.pressed || max_point_num == 0) {
        *point_num = 0;
        return false;
    }
    x[0] = touch->current.x;
    y[0] = touch->current.y;
    if (strength) {
        strength[0] = 1;
    }
    *point_num = 1;
    return true;
}

static esp_err_t script_del(esp_lcd_touch_handle_t tp)
{
    free(__containerof(tp, script_touch_t, base));
    return ESP_OK;
}

esp_err_t app_bench_touch_create(esp_lcd_touch_handle_t *out_touch)
{
    ESP_RETURN_ON_FALSE(out_touch != NULL, ESP_ERR_INVALID_ARG, "bench", "output is null");
    script_touch_t *touch = heap_caps_calloc(1, sizeof(*touch), MALLOC_CAP_INTERNAL);
    ESP_RETURN_ON_FALSE(touch != NULL, ESP_ERR_NO_MEM, "bench", "touch device");
    touch->base.read_data = script_read_data;
    touch->base.get_xy = script_get_xy;
    touch->base.del = script_del;
    touch->base.config = (esp_lcd_touch_config_t) {
        .x_max = SCREEN_MAX_PX,
        .y_max = SCREEN_MAX_PX,
        .rst_gpio_num = GPIO_NUM_NC,
        .int_gpio_num = GPIO_NUM_NC,
    };
    *out_touch = &touch->base;
    return ESP_OK;
}
