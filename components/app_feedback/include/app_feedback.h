// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Haptic patterns. */
typedef enum {
    APP_FEEDBACK_HAPTIC_TICK = 0, /*!< a page settled: one 10 ms tick */
    APP_FEEDBACK_HAPTIC_CONFIRM,  /*!< an action went out: 25 ms */
    APP_FEEDBACK_HAPTIC_CALL,     /*!< someone is waiting: two soft pulses */
    APP_FEEDBACK_HAPTIC_FAIL,     /*!< a turn failed: long, then short */
} app_feedback_haptic_t;

/** Speaker chimes. */
typedef enum {
    APP_FEEDBACK_CHIME_CALL = 0, /*!< two rising notes */
    APP_FEEDBACK_CHIME_DONE,     /*!< one soft note */
    APP_FEEDBACK_CHIME_FAIL,     /*!< one low note */
} app_feedback_chime_t;

/** What the interaction module's LEDs show. */
typedef enum {
    APP_FEEDBACK_GLOW_OFF = 0,
    APP_FEEDBACK_GLOW_WORK,  /*!< dim cool breathing */
    APP_FEEDBACK_GLOW_CALL,  /*!< amber breathing */
    APP_FEEDBACK_GLOW_FAIL,  /*!< steady red */
    APP_FEEDBACK_GLOW_FLASH, /*!< one white flash over the current glow */
} app_feedback_glow_t;

typedef enum {
    APP_FEEDBACK_EVENT_POWER = 0, /*!< `power` */
    APP_FEEDBACK_EVENT_BUTTON,    /*!< `value`: app_feedback_button_t */
    APP_FEEDBACK_EVENT_MODULE,    /*!< `value`: 1 attached, 0 removed */
    APP_FEEDBACK_EVENT_PRESENCE,  /*!< `value`: 1 someone near (PIR), 0 nobody */
    APP_FEEDBACK_EVENT_AMBIENT,   /*!< `value`: relative light 0..100 */
    APP_FEEDBACK_EVENT_MOTION,    /*!< `value`: app_feedback_motion_t */
} app_feedback_event_kind_t;

typedef enum {
    APP_FEEDBACK_BUTTON_AI_CLICK = 0,
    APP_FEEDBACK_BUTTON_AI_LONG,
    APP_FEEDBACK_BUTTON_PREV, /*!< interaction module touch key */
    APP_FEEDBACK_BUTTON_NEXT, /*!< interaction module touch key */
} app_feedback_button_t;

typedef enum {
    APP_FEEDBACK_MOTION_LIFTED = 0, /*!< picked up or moved from rest */
    APP_FEEDBACK_MOTION_FACE_DOWN,
    APP_FEEDBACK_MOTION_FACE_UP,    /*!< left the face-down position */
} app_feedback_motion_t;

/** `minutes` when the fuel gauge gives no estimate. */
#define APP_FEEDBACK_MINUTES_UNKNOWN UINT16_MAX

typedef struct {
    uint8_t percent;  /*!< state of charge, 0..100 */
    bool charging;
    bool known;       /*!< false: the fuel gauge did not answer; the rest is 0 */
    uint16_t minutes; /*!< to empty, or to full while charging; or ..._UNKNOWN */
} app_feedback_power_t;

typedef struct {
    app_feedback_event_kind_t kind;
    int32_t value;              /*!< see app_feedback_event_kind_t */
    app_feedback_power_t power; /*!< valid for APP_FEEDBACK_EVENT_POWER */
} app_feedback_event_t;

/** Called on the `feedback` task; the event is valid only during the call.
 * Keep it short and never block. */
typedef void (*app_feedback_event_cb_t)(const app_feedback_event_t *event, void *ctx);

/**
 * Start the `feedback` task (motor, AI key, IMU, fuel gauge, interaction
 * module, display power) and the `chime` task (speaker). Call once.
 *
 * Each part initializes on its task and is optional: a part that fails is
 * logged and stays off, the rest keeps working. `on_event` and `ctx` must
 * stay valid until reboot.
 *
 * @return ESP_OK; ESP_ERR_INVALID_ARG without a callback; ESP_ERR_INVALID_STATE
 *         when already started; ESP_ERR_NO_MEM, with everything released.
 */
esp_err_t app_feedback_start(app_feedback_event_cb_t on_event, void *ctx);

/* The requests below copy their argument and return at once, from any task
 * (not from an ISR). Before app_feedback_start they return
 * ESP_ERR_INVALID_STATE; ESP_ERR_INVALID_ARG for an unknown value. */

/** Play a pattern; it replaces one still playing. */
esp_err_t app_feedback_haptic(app_feedback_haptic_t haptic);

/** Play a chime after the one playing; a newer request replaces a waiting one. */
esp_err_t app_feedback_chime(app_feedback_chime_t chime);

/** Show a glow on the interaction module (ignored while it is absent; a newly
 * attached module starts dark). FLASH plays once over the current glow. */
esp_err_t app_feedback_glow(app_feedback_glow_t glow);

/** LED brightness cap, 0..100 % (default 60), applied to every glow. */
esp_err_t app_feedback_led_brightness(uint8_t percent);

/** Panel on at `percent` brightness (0..100), or off. The latest request wins;
 * the panel is driven through app_display. */
esp_err_t app_feedback_display(bool on, uint8_t percent);

#ifdef __cplusplus
}
#endif
