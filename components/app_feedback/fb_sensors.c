// SPDX-License-Identifier: Apache-2.0
// Board sensors on the `feedback` task: the AI key, BMI270 motion and the
// BQ27220 fuel gauge. All state here is owned by that task; the AI key's
// callbacks run on the button component's timer and only notify it.
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#include "bq27220.h"
#include "bsp/esp_mosaico.h"
#include "esp_log.h"
#include "fb_internal.h"

static const char *TAG = FB_TAG;

/* ------------------------------------------------------------- AI key */

/* As the factory firmware's AI key (main/platform/mosaic_button_platform.c). */
#define AI_LONG_PRESS_MS 600

static struct {
    button_handle_t handle;
    bool armed;      /* false while a press from before the start is still held */
    bool long_fired; /* the current press already reported AiLong */
} s_ai;

/* The feedback task; set before the callbacks are registered. */
static TaskHandle_t s_button_task;

/* Button timer task: forward the edge (user_data is its notify bit). */
static void ai_notify(void *button, void *user_data)
{
    (void)button;
    (void)xTaskNotify(s_button_task, (uint32_t)(uintptr_t)user_data, eSetBits);
}

void fb_button_init(TaskHandle_t task)
{
    button_handle_t buttons[BSP_BUTTON_NUM] = {0};
    int count = 0;
    s_button_task = task;
    esp_err_t err = bsp_iot_button_create(buttons, &count, BSP_BUTTON_NUM);
    button_handle_t ai = err == ESP_OK ? buttons[BSP_BUTTON_AI] : NULL;
    if (ai) {
        button_event_args_t long_press = {.long_press.press_time = AI_LONG_PRESS_MS};
        err = iot_button_register_cb(ai, BUTTON_PRESS_DOWN, NULL, ai_notify, (void *)FB_NOTIFY_AI_DOWN);
        if (err == ESP_OK) {
            err = iot_button_register_cb(ai, BUTTON_LONG_PRESS_START, &long_press, ai_notify,
                                         (void *)FB_NOTIFY_AI_LONG);
        }
        if (err == ESP_OK) {
            err = iot_button_register_cb(ai, BUTTON_PRESS_UP, NULL, ai_notify, (void *)FB_NOTIFY_AI_UP);
        }
        if (err != ESP_OK) {
            (void)iot_button_delete(ai);
            ai = NULL;
        }
    }
    if (!ai) {
        ESP_LOGW(TAG, "AI key unavailable (%s)", esp_err_to_name(err));
        return;
    }
    /* A key held through boot (the Vibe Mode gesture) is ignored until released. */
    s_ai.armed = iot_button_get_key_level(ai) == 0;
    s_ai.handle = ai;
}

void fb_button_service(uint32_t notified)
{
    if (!s_ai.handle) {
        return;
    }
    if (!s_ai.armed) {
        s_ai.armed = (notified & FB_NOTIFY_AI_UP) != 0;
        return;
    }
    if (notified & FB_NOTIFY_AI_DOWN) {
        s_ai.long_fired = false;
    }
    if ((notified & FB_NOTIFY_AI_LONG) && !s_ai.long_fired) {
        s_ai.long_fired = true;
        ESP_LOGI(TAG, "AI key long");
        fb_emit(APP_FEEDBACK_EVENT_BUTTON, APP_FEEDBACK_BUTTON_AI_LONG);
    }
    if ((notified & FB_NOTIFY_AI_UP) && !s_ai.long_fired) {
        ESP_LOGI(TAG, "AI key click");
        fb_emit(APP_FEEDBACK_EVENT_BUTTON, APP_FEEDBACK_BUTTON_AI_CLICK);
    }
}

/* ------------------------------------------------------------- IMU */

#define IMU_POLL_MS          50
#define IMU_MAX_FAILURES     40    /* consecutive read errors before giving up */
#define REST_TOLERANCE_G     0.06f /* |a| within this of 1 g counts as still */
#define REST_SETTLE_MS       1500  /* still this long before a lift can fire */
#define LIFT_DEVIATION_G     0.15f /* |a| off 1 g by this from rest: moved */
#define LIFT_TILT_COS        0.978f /* cos 12 deg: tilted this far from rest */
#define FACE_DOWN_Z          -0.85f /* z share of gravity: screen facing down */
#define FACE_DOWN_HOLD_MS    800
#define FACE_UP_Z            -0.50f /* z share above this leaves face-down */
#define FACE_UP_HOLD_MS      300

static struct {
    bool ok;
    int failures;
    int64_t next_poll;
    float rest[3]; /* unit gravity while at rest */
    bool rest_valid;
    int64_t still_since;
    bool face_down;
    int64_t flip_since; /* the face-down/up candidate began; 0 = none */
    float z;            /* last z share of gravity, for the status line */
} s_imu;

void fb_imu_init(void)
{
    esp_err_t err = bsp_imu_init();
    if (err == ESP_OK) {
        const bsp_imu_config_t config = {
            .accel_odr = BMI2_ACC_ODR_50HZ,
            .accel_range = BMI2_ACC_RANGE_2G,
            .gyro_odr = BMI2_GYR_ODR_25HZ,
            .gyro_range = BMI2_GYR_RANGE_2000,
        };
        err = bsp_imu_start(&config);
        if (err != ESP_OK) {
            (void)bsp_imu_deinit();
        }
    }
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "IMU unavailable (%s); no lift or face-down", esp_err_to_name(err));
        return;
    }
    s_imu.ok = true;
    s_imu.z = 1.0f;
}

static void imu_sample(int64_t now, float ax, float ay, float az)
{
    const float magnitude = sqrtf(ax * ax + ay * ay + az * az);
    if (magnitude < 0.2f) {
        return; /* free fall or a bad sample: no direction */
    }
    const float unit[3] = {ax / magnitude, ay / magnitude, az / magnitude};
    s_imu.z = unit[2];

    /* Lift: a settled board that moves or tilts. */
    const bool still = fabsf(magnitude - 1.0f) < REST_TOLERANCE_G;
    if (s_imu.rest_valid && now - s_imu.still_since >= REST_SETTLE_MS) {
        const float dot = unit[0] * s_imu.rest[0] + unit[1] * s_imu.rest[1] + unit[2] * s_imu.rest[2];
        if (fabsf(magnitude - 1.0f) > LIFT_DEVIATION_G || dot < LIFT_TILT_COS) {
            ESP_LOGI(TAG, "motion lifted");
            fb_emit(APP_FEEDBACK_EVENT_MOTION, APP_FEEDBACK_MOTION_LIFTED);
            s_imu.rest_valid = false;
        }
    }
    if (!still) {
        s_imu.rest_valid = false;
    } else if (!s_imu.rest_valid) {
        s_imu.rest_valid = true;
        s_imu.still_since = now;
        for (int i = 0; i < 3; ++i) {
            s_imu.rest[i] = unit[i];
        }
    }

    /* Face down / up, each held for a while. */
    const bool toward = s_imu.face_down ? unit[2] > FACE_UP_Z : (unit[2] < FACE_DOWN_Z && still);
    if (!toward) {
        s_imu.flip_since = 0;
        return;
    }
    if (s_imu.flip_since == 0) {
        s_imu.flip_since = now;
    }
    if (now - s_imu.flip_since >= (s_imu.face_down ? FACE_UP_HOLD_MS : FACE_DOWN_HOLD_MS)) {
        s_imu.face_down = !s_imu.face_down;
        s_imu.flip_since = 0;
        ESP_LOGI(TAG, "motion face %s", s_imu.face_down ? "down" : "up");
        fb_emit(APP_FEEDBACK_EVENT_MOTION,
                s_imu.face_down ? APP_FEEDBACK_MOTION_FACE_DOWN : APP_FEEDBACK_MOTION_FACE_UP);
    }
}

int64_t fb_imu_service(int64_t now)
{
    if (!s_imu.ok) {
        return FB_NEVER;
    }
    if (now < s_imu.next_poll) {
        return s_imu.next_poll;
    }
    s_imu.next_poll = now + IMU_POLL_MS;
    float ax;
    float ay;
    float az;
    const esp_err_t err = bsp_imu_get_accel(&ax, &ay, &az);
    if (err != ESP_OK) {
        if (++s_imu.failures >= IMU_MAX_FAILURES) {
            s_imu.ok = false;
            ESP_LOGW(TAG, "IMU reads keep failing (%s); motion off", esp_err_to_name(err));
            return FB_NEVER;
        }
        return s_imu.next_poll;
    }
    s_imu.failures = 0;
    imu_sample(now, ax, ay, az);
    return s_imu.next_poll;
}

void fb_imu_describe(char *out, size_t size)
{
    if (!s_imu.ok) {
        snprintf(out, size, "off");
        return;
    }
    snprintf(out, size, "z=%.2f%s%s", (double)s_imu.z, s_imu.face_down ? " face-down" : "",
             s_imu.rest_valid ? " still" : "");
}

/* ------------------------------------------------------------- fuel gauge */

/* Sampling and charge detection follow the factory firmware
 * (main/platform/mosaic_battery_platform.c). */
#define BATTERY_SAMPLE_MS        5000
#define BATTERY_REPORT_MAX_MS    30000 /* report at least this often */
#define BATTERY_RETRY_INIT_MS    30000
#define BATTERY_MIN_VALID_MV     2500
#define BATTERY_MAX_VALID_MV     5000
#define CHARGE_ON_MA             8
#define CHARGE_CONFIRM_SAMPLES   3
#define MINUTES_REPORT_STEP      2     /* smaller estimate changes wait for the periodic report */
#define GAUGE_NO_ESTIMATE        0xFFFF

static struct {
    bool ok;
    int64_t next_sample;
    int64_t next_report;
    bool charging;
    bool charge_candidate;
    int charge_samples;
    bool first_sample;
    bool reported_valid;
    app_feedback_power_t reported;
} s_battery;

void fb_battery_init(void)
{
    const esp_err_t err = bsp_battery_init();
    s_battery.ok = err == ESP_OK;
    s_battery.first_sample = true;
    if (!s_battery.ok) {
        ESP_LOGW(TAG, "fuel gauge unavailable (%s); battery hidden", esp_err_to_name(err));
    }
}

/* DSG is the direction; positive current can enter charging early, but a
 * small negative current under load never ends it on its own. */
static void update_charging(const bsp_battery_status_t *status)
{
    const battery_status_t flags = {.full = status->status_flags};
    const bool positive = status->current_ma >= CHARGE_ON_MA || status->average_current_ma >= CHARGE_ON_MA;
    const bool charge_in = !flags.DSG || positive;
    const bool charge_out = flags.DSG && !positive;
    if (charge_in == charge_out) {
        s_battery.charge_samples = 0;
        return;
    }
    if (s_battery.first_sample) {
        s_battery.charging = charge_in;
        return;
    }
    if (charge_in != s_battery.charge_candidate) {
        s_battery.charge_candidate = charge_in;
        s_battery.charge_samples = 1;
    } else if (s_battery.charge_samples < CHARGE_CONFIRM_SAMPLES) {
        ++s_battery.charge_samples;
    }
    if (s_battery.charge_samples >= CHARGE_CONFIRM_SAMPLES) {
        s_battery.charging = s_battery.charge_candidate;
    }
}

static bool read_power(app_feedback_power_t *power)
{
    bsp_battery_status_t status = {0};
    if (bsp_battery_read(&status) != ESP_OK || status.voltage_mv < BATTERY_MIN_VALID_MV ||
            status.voltage_mv > BATTERY_MAX_VALID_MV || status.state_of_charge > 100) {
        return false;
    }
    update_charging(&status);
    s_battery.first_sample = false;
    const uint16_t minutes = s_battery.charging ? status.time_to_full_min : status.time_to_empty_min;
    *power = (app_feedback_power_t) {
        .percent = status.state_of_charge,
        .charging = s_battery.charging,
        .known = true,
        .minutes = minutes == GAUGE_NO_ESTIMATE ? APP_FEEDBACK_MINUTES_UNKNOWN : minutes,
    };
    return true;
}

static bool minutes_moved(uint16_t a, uint16_t b)
{
    if (a == APP_FEEDBACK_MINUTES_UNKNOWN || b == APP_FEEDBACK_MINUTES_UNKNOWN) {
        return a != b;
    }
    return abs((int)a - (int)b) >= MINUTES_REPORT_STEP;
}

int64_t fb_battery_service(int64_t now)
{
    if (now < s_battery.next_sample) {
        return s_battery.next_sample;
    }
    s_battery.next_sample = now + BATTERY_SAMPLE_MS;
    if (!s_battery.ok && now >= s_battery.next_report) {
        /* Retry the gauge now and then; it reports unknown until it answers. */
        const esp_err_t err = bsp_battery_init();
        s_battery.ok = err == ESP_OK;
        if (s_battery.ok) {
            ESP_LOGI(TAG, "fuel gauge online");
        }
    }
    app_feedback_power_t power = {.minutes = APP_FEEDBACK_MINUTES_UNKNOWN};
    if (s_battery.ok && !read_power(&power)) {
        power = (app_feedback_power_t) {
            .minutes = APP_FEEDBACK_MINUTES_UNKNOWN
        };
    }
    const app_feedback_power_t *last = &s_battery.reported;
    const bool headline = !s_battery.reported_valid || power.known != last->known ||
                          power.percent != last->percent || power.charging != last->charging;
    if (headline || minutes_moved(power.minutes, last->minutes) || now >= s_battery.next_report) {
        if (headline) {
            if (power.known) {
                ESP_LOGI(TAG, "power %u%%%s", power.percent, power.charging ? " charging" : "");
            } else {
                ESP_LOGW(TAG, "power unknown (fuel gauge silent)");
            }
        }
        s_battery.reported = power;
        s_battery.reported_valid = true;
        s_battery.next_report = now + (s_battery.ok ? BATTERY_REPORT_MAX_MS : BATTERY_RETRY_INIT_MS);
        fb_emit_power(&power);
    }
    return s_battery.next_sample;
}

void fb_battery_describe(char *out, size_t size)
{
    const app_feedback_power_t *p = &s_battery.reported;
    if (!s_battery.reported_valid || !p->known) {
        snprintf(out, size, "battery unknown");
    } else if (p->minutes == APP_FEEDBACK_MINUTES_UNKNOWN) {
        snprintf(out, size, "battery %u%%%s", p->percent, p->charging ? " charging" : "");
    } else {
        snprintf(out, size, "battery %u%%%s %u min", p->percent, p->charging ? " charging" : "",
                 p->minutes);
    }
}
