// SPDX-License-Identifier: Apache-2.0
#pragma once

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

#ifdef __cplusplus
}
#endif
