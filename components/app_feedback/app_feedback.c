// SPDX-License-Identifier: Apache-2.0
// The `feedback` task: request mailbox, haptic patterns, display power, and
// the scheduling of the sensor parts (fb_sensors.c, fb_module.c).
//
// Ownership: requests from other tasks only write the mailbox under a spinlock
// and notify the task; the task alone drives the motor, the panel power, the
// sensors and the LEDs, and calls the event callback. The speaker has its own
// task (fb_chime.c) because a chime write blocks for its whole length.
#include "app_feedback.h"

#include <inttypes.h>
#include <stdio.h>

#include "app_display.h"
#include "bsp/esp_mosaico.h"
#include "esp_check.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "fb_internal.h"

static const char *TAG = FB_TAG;

/* Above the logic task (5): a pattern step is due within a tick. Every
 * service on this task is short (I2C reads, an LED refresh). */
#define FEEDBACK_TASK_PRIORITY 6
#define FEEDBACK_TASK_STACK    (5 * 1024)
#define STATUS_PERIOD_MS       60000
#define DEFAULT_LED_PERCENT    60

_Static_assert(sizeof(app_feedback_event_t) == 16, "app_feedback_event_t layout is mirrored in Rust");

typedef struct {
    uint8_t strength; /* motor PWM, percent; 0 = pause */
    uint8_t ms;
} haptic_step_t;

typedef struct {
    const haptic_step_t *steps;
    uint8_t count;
} haptic_pattern_t;

/* An ERM motor needs a few ms to spin up: short steps run at full strength. */
static const haptic_step_t s_tick[] = {{100, 10}};
static const haptic_step_t s_confirm[] = {{100, 25}};
static const haptic_step_t s_call[] = {{55, 45}, {0, 110}, {55, 45}};
static const haptic_step_t s_fail[] = {{90, 170}, {0, 90}, {90, 50}};

static const haptic_pattern_t s_patterns[] = {
    [APP_FEEDBACK_HAPTIC_TICK] = {s_tick, sizeof(s_tick) / sizeof(s_tick[0])},
    [APP_FEEDBACK_HAPTIC_CONFIRM] = {s_confirm, sizeof(s_confirm) / sizeof(s_confirm[0])},
    [APP_FEEDBACK_HAPTIC_CALL] = {s_call, sizeof(s_call) / sizeof(s_call[0])},
    [APP_FEEDBACK_HAPTIC_FAIL] = {s_fail, sizeof(s_fail) / sizeof(s_fail[0])},
};

static const char *const s_haptic_names[] = {"tick", "confirm", "call", "fail"};
static const char *const s_glow_names[] = {"off", "work", "call", "fail", "flash"};

/* Requests waiting for the task (any task writes, under s_lock). */
typedef struct {
    bool haptic_pending;
    app_feedback_haptic_t haptic;
    bool display_pending;
    bool display_on;
    uint8_t display_percent;
    bool glow_pending;
    app_feedback_glow_t glow;
    bool flash_pending;
    bool led_pending;
    uint8_t led_percent;
} mailbox_t;

static portMUX_TYPE s_lock = portMUX_INITIALIZER_UNLOCKED;
static mailbox_t s_mailbox;
static TaskHandle_t s_task;
static app_feedback_event_cb_t s_on_event;
static void *s_ctx;

/* Feedback task state. */
static struct {
    bool motor_ok;
    const haptic_pattern_t *pattern;
    uint8_t step;
    int64_t step_end;
    bool display_known; /* the panel state below was applied */
    bool display_on;
    uint8_t display_percent;
    int64_t next_status;
} s_state;

int64_t fb_now_ms(void)
{
    return esp_timer_get_time() / 1000;
}

void fb_emit(app_feedback_event_kind_t kind, int32_t value)
{
    const app_feedback_event_t event = {.kind = kind, .value = value};
    s_on_event(&event, s_ctx);
}

void fb_emit_power(const app_feedback_power_t *power)
{
    const app_feedback_event_t event = {.kind = APP_FEEDBACK_EVENT_POWER, .power = *power};
    s_on_event(&event, s_ctx);
}

/* ------------------------------------------------------------- haptics */

static void motor_set(uint8_t strength)
{
    if (!s_state.motor_ok) {
        return;
    }
    const esp_err_t err = bsp_motor_set_strength(strength);
    if (err != ESP_OK) {
        s_state.motor_ok = false;
        ESP_LOGW(TAG, "motor write failed (%s); haptics off", esp_err_to_name(err));
    }
}

static void haptic_start(app_feedback_haptic_t haptic, int64_t now)
{
    if (!s_state.motor_ok) {
        return;
    }
    s_state.pattern = &s_patterns[haptic];
    s_state.step = 0;
    s_state.step_end = now + s_state.pattern->steps[0].ms;
    motor_set(s_state.pattern->steps[0].strength);
    ESP_LOGI(TAG, "haptic %s", s_haptic_names[haptic]);
}

static int64_t haptic_service(int64_t now)
{
    while (s_state.pattern && now >= s_state.step_end) {
        if (++s_state.step >= s_state.pattern->count) {
            s_state.pattern = NULL;
            motor_set(0);
            break;
        }
        const haptic_step_t *step = &s_state.pattern->steps[s_state.step];
        /* Steps run back to back from the planned end, not from a late wake. */
        s_state.step_end += step->ms;
        motor_set(step->strength);
    }
    return s_state.pattern ? s_state.step_end : FB_NEVER;
}

/* ------------------------------------------------------------- display */

static void display_apply(bool on, uint8_t percent)
{
    const bool was_on = s_state.display_known && s_state.display_on;
    esp_err_t err = ESP_OK;
    if (on) {
        if (!s_state.display_known || percent != s_state.display_percent) {
            err = app_display_set_brightness(percent);
        }
        if (err == ESP_OK && !was_on) {
            err = app_display_set_enabled(true);
        }
    } else if (!s_state.display_known || was_on) {
        err = app_display_set_enabled(false);
    }
    if (err != ESP_OK) {
        /* Unknown afterwards: the next request applies everything again. */
        s_state.display_known = false;
        ESP_LOGW(TAG, "display %s failed (%s)", on ? "on" : "off", esp_err_to_name(err));
        return;
    }
    if (on != was_on || !s_state.display_known) {
        if (on) {
            ESP_LOGI(TAG, "display on %u%%", percent);
        } else {
            ESP_LOGI(TAG, "display off");
        }
    } else if (on && percent != s_state.display_percent) {
        ESP_LOGD(TAG, "display brightness %u%%", percent);
    }
    s_state.display_known = true;
    s_state.display_on = on;
    s_state.display_percent = on ? percent : s_state.display_percent;
}

/* ------------------------------------------------------------- task */

static void take_requests(int64_t now)
{
    portENTER_CRITICAL(&s_lock);
    const mailbox_t box = s_mailbox;
    s_mailbox.haptic_pending = false;
    s_mailbox.display_pending = false;
    s_mailbox.glow_pending = false;
    s_mailbox.flash_pending = false;
    s_mailbox.led_pending = false;
    portEXIT_CRITICAL(&s_lock);

    if (box.display_pending) {
        display_apply(box.display_on, box.display_percent);
    }
    if (box.haptic_pending) {
        haptic_start(box.haptic, now);
    }
    if (box.led_pending) {
        fb_module_led_brightness(box.led_percent);
    }
    if (box.glow_pending) {
        fb_module_glow(box.glow, now);
    }
    if (box.flash_pending) {
        fb_module_glow(APP_FEEDBACK_GLOW_FLASH, now);
    }
}

static void log_status(void)
{
    char battery[48];
    char imu[32];
    char module[64];
    fb_battery_describe(battery, sizeof(battery));
    fb_imu_describe(imu, sizeof(imu));
    fb_module_describe(module, sizeof(module));
    ESP_LOGI(TAG, "status: %s | imu %s | module %s", battery, imu, module);
}

static int64_t min64(int64_t a, int64_t b)
{
    return a < b ? a : b;
}

static void feedback_task(void *arg)
{
    (void)arg;
    /* Wait for app_feedback_start to finish; keep what else arrived meanwhile. */
    uint32_t notified = FB_NOTIFY_COMMAND; /* requests made before the gate opened */
    for (uint32_t bits = 0; !(bits & FB_NOTIFY_START);) {
        (void)xTaskNotifyWait(0, UINT32_MAX, &bits, portMAX_DELAY);
        notified |= bits & ~FB_NOTIFY_START;
    }
    const esp_err_t motor = bsp_motor_init();
    s_state.motor_ok = motor == ESP_OK;
    if (s_state.motor_ok) {
        motor_set(0);
    } else {
        ESP_LOGW(TAG, "motor unavailable (%s); haptics off", esp_err_to_name(motor));
    }
    fb_button_init(xTaskGetCurrentTaskHandle());
    fb_battery_init();
    fb_imu_init();
    fb_module_init();
    fb_chime_release();
    s_state.next_status = fb_now_ms() + STATUS_PERIOD_MS;
    ESP_LOGI(TAG, "ready: motor %s", s_state.motor_ok ? "on" : "off");

    for (;;) {
        int64_t now = fb_now_ms();
        if (notified & FB_NOTIFY_COMMAND) {
            take_requests(now);
        }
        fb_button_service(notified);

        int64_t next = haptic_service(now);
        next = min64(next, fb_module_service(now));
        next = min64(next, fb_imu_service(now));
        next = min64(next, fb_battery_service(now));
        if (now >= s_state.next_status) {
            s_state.next_status = now + STATUS_PERIOD_MS;
            log_status();
        }
        next = min64(next, s_state.next_status);

        now = fb_now_ms();
        /* The status line bounds every wait; the clamp only guards the cast. */
        const int64_t wait_ms = next <= now ? 0 : (next - now > STATUS_PERIOD_MS ? STATUS_PERIOD_MS : next - now);
        notified = 0;
        (void)xTaskNotifyWait(0, UINT32_MAX, &notified, pdMS_TO_TICKS((uint32_t)wait_ms));
    }
}

/* ------------------------------------------------------------- API */

static esp_err_t post(void (*write)(mailbox_t *box, int a, int b), int a, int b)
{
    if (!s_task) {
        return ESP_ERR_INVALID_STATE;
    }
    portENTER_CRITICAL(&s_lock);
    write(&s_mailbox, a, b);
    portEXIT_CRITICAL(&s_lock);
    (void)xTaskNotify(s_task, FB_NOTIFY_COMMAND, eSetBits);
    return ESP_OK;
}

static void write_haptic(mailbox_t *box, int haptic, int unused)
{
    (void)unused;
    box->haptic_pending = true;
    box->haptic = (app_feedback_haptic_t)haptic;
}

static void write_display(mailbox_t *box, int on, int percent)
{
    box->display_pending = true;
    box->display_on = on;
    box->display_percent = (uint8_t)percent;
}

static void write_glow(mailbox_t *box, int glow, int unused)
{
    (void)unused;
    if (glow == APP_FEEDBACK_GLOW_FLASH) {
        box->flash_pending = true;
    } else {
        box->glow_pending = true;
        box->glow = (app_feedback_glow_t)glow;
    }
}

static void write_led(mailbox_t *box, int percent, int unused)
{
    (void)unused;
    box->led_pending = true;
    box->led_percent = (uint8_t)percent;
}

esp_err_t app_feedback_haptic(app_feedback_haptic_t haptic)
{
    if ((unsigned)haptic > APP_FEEDBACK_HAPTIC_FAIL) {
        return ESP_ERR_INVALID_ARG;
    }
    return post(write_haptic, haptic, 0);
}

esp_err_t app_feedback_chime(app_feedback_chime_t chime)
{
    if ((unsigned)chime > APP_FEEDBACK_CHIME_FAIL) {
        return ESP_ERR_INVALID_ARG;
    }
    if (!s_task) {
        return ESP_ERR_INVALID_STATE;
    }
    return fb_chime_post(chime);
}

esp_err_t app_feedback_glow(app_feedback_glow_t glow)
{
    if ((unsigned)glow > APP_FEEDBACK_GLOW_FLASH) {
        return ESP_ERR_INVALID_ARG;
    }
    ESP_LOGD(TAG, "glow %s requested", s_glow_names[glow]);
    return post(write_glow, glow, 0);
}

esp_err_t app_feedback_led_brightness(uint8_t percent)
{
    if (percent > 100) {
        return ESP_ERR_INVALID_ARG;
    }
    return post(write_led, percent, 0);
}

esp_err_t app_feedback_display(bool on, uint8_t percent)
{
    if (percent > 100) {
        return ESP_ERR_INVALID_ARG;
    }
    return post(write_display, on, percent);
}

esp_err_t app_feedback_start(app_feedback_event_cb_t on_event, void *ctx)
{
    ESP_RETURN_ON_FALSE(on_event, ESP_ERR_INVALID_ARG, TAG, "no event callback");
    ESP_RETURN_ON_FALSE(!s_task, ESP_ERR_INVALID_STATE, TAG, "already started");
    s_on_event = on_event;
    s_ctx = ctx;
    s_mailbox.led_pending = true;
    s_mailbox.led_percent = DEFAULT_LED_PERCENT;

    /* The feedback task waits at its gate until the chime task exists, so a
     * failure here can still delete it before it touched any hardware. */
    TaskHandle_t task = NULL;
    if (xTaskCreate(feedback_task, "feedback", FEEDBACK_TASK_STACK, NULL, FEEDBACK_TASK_PRIORITY,
                    &task) != pdPASS) {
        ESP_LOGE(TAG, "feedback task not created");
        return ESP_ERR_NO_MEM;
    }
    const esp_err_t err = fb_chime_start();
    if (err == ESP_ERR_NO_MEM) {
        vTaskDelete(task);
        return err;
    }
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "speaker unavailable (%s); chimes off", esp_err_to_name(err));
    }
    portENTER_CRITICAL(&s_lock);
    s_task = task;
    portEXIT_CRITICAL(&s_lock);
    (void)xTaskNotify(task, FB_NOTIFY_START, eSetBits);
    return ESP_OK;
}
