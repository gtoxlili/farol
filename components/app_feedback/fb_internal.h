// SPDX-License-Identifier: Apache-2.0
// Shared between the parts of app_feedback. Everything declared here runs on
// the `feedback` task unless it says otherwise; each part's state is private
// to its file and owned by that task.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "app_feedback.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#define FB_TAG "feedback"

/* Task notification bits of the `feedback` task. */
#define FB_NOTIFY_COMMAND BIT0 /* the request mailbox changed */
#define FB_NOTIFY_AI_DOWN BIT1 /* AI key pressed (button timer task) */
#define FB_NOTIFY_AI_LONG BIT2 /* AI key held past the long-press time */
#define FB_NOTIFY_AI_UP   BIT3 /* AI key released */
#define FB_NOTIFY_START   BIT4 /* app_feedback_start finished */

/** No deadline: the part has nothing scheduled. */
#define FB_NEVER INT64_MAX

int64_t fb_now_ms(void);

/** Report an event through the callback given to app_feedback_start (from the
 * `feedback` task; LOUDNESS from the `chime` task, which the callback allows). */
void fb_emit(app_feedback_event_kind_t kind, int32_t value);
void fb_emit_power(const app_feedback_power_t *power);

/* AI key (fb_sensors.c). Init registers callbacks that only notify `task`. */
void fb_button_init(TaskHandle_t task);
void fb_button_service(uint32_t notified);

/* BMI270 motion (fb_sensors.c). Services return their next deadline. */
void fb_imu_init(void);
int64_t fb_imu_service(int64_t now);

/* BQ27220 fuel gauge (fb_sensors.c). */
void fb_battery_init(void);
int64_t fb_battery_service(int64_t now);
/** One short phrase for the periodic status line. */
void fb_battery_describe(char *out, size_t size);
void fb_imu_describe(char *out, size_t size);

/* Interaction module: hot-plug, PIR, light, touch keys, LEDs (fb_module.c). */
void fb_module_init(void);
int64_t fb_module_service(int64_t now);
void fb_module_glow(app_feedback_glow_t glow, int64_t now);
void fb_module_led_brightness(uint8_t percent);
void fb_module_describe(char *out, size_t size);

/* Speaker (fb_chime.c): its own `chime` task; post from any task. */
esp_err_t fb_chime_start(void);
esp_err_t fb_chime_post(app_feedback_chime_t chime);
/** Let the chime task bring up the speaker (after the feedback task's BSP init). */
void fb_chime_release(void);
/* Microphone (fb_chime.c, on the chime task, which owns the codec). */
esp_err_t fb_mic_listen(bool on);
esp_err_t fb_mic_on_audio(app_feedback_audio_cb_t on_audio, void *ctx);
