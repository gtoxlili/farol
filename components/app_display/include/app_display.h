// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_display_present.h"
#include "esp_err.h"
#include "esp_lcd_touch.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Display handles for GSP; owned by app_display and alive until reboot. */
typedef struct {
    esp_display_presenter_t *presenter; /*!< TE-synchronized presenter */
    esp_lcd_touch_handle_t touch;       /*!< NULL when touch is unavailable */
} app_display_t;

/** Create the CO5300 panel IO, panel, TE-synchronized presenter and touch.
 * Call once at boot before GSP. Returns ESP_OK with *out filled (touch may be
 * NULL), or an error with the display released. Contract: ../README.md. */
esp_err_t app_display_create(app_display_t *out);

/** Stops and restarts frame production around panel commands. The panel IO
 * is not safe against the presenter's own transfers on another task (both
 * drain the same SPI device's in-flight transactions), so every command runs
 * while frames are stopped. `pause` returns true once no frame is in flight. */
typedef struct {
    bool (*pause)(void *ctx);
    void (*resume)(void *ctx);
    void *ctx;
} app_display_guard_t;

/** Register the guard (copied) once the UI runs, before any command below.
 * Without one the commands go to the panel unguarded. */
void app_display_set_guard(const app_display_guard_t *guard);

/** Set the panel brightness, 0..100 %, with frames stopped around the write.
 * ESP_ERR_INVALID_STATE before app_display_create succeeded, ESP_ERR_TIMEOUT
 * when frames could not be stopped. One task issues all commands. */
esp_err_t app_display_set_brightness(uint8_t percent);

/** Switch the panel off (frames stay stopped while it is dark: nothing renders
 * or transfers) or on again (frames resume). */
esp_err_t app_display_set_enabled(bool enabled);

#ifdef __cplusplus
}
#endif
