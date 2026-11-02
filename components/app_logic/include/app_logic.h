// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_err.h"
#include "esp_gsp.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Start the Rust logic (task "logic") for a running UI; contract: ../README.md.
 * Call once from app_main after esp_gsp_esp_lcd_start(). ui must stay alive
 * until reboot. Returns 0, or a negative code naming the failed start step. */
int32_t app_logic_start(esp_gsp_handle_t ui);

/** Give the UI configuration the dynamic font for runtime text. Call before
 * the UI starts. The font is copied to PSRAM once and kept until reboot; when
 * that allocation fails the configuration borrows it from flash instead. */
esp_err_t app_logic_font_apply(esp_gsp_config_t *config);

/** CONFIG_FAROL_DEMO, for the Rust logic: start on the demo fleet. */
bool app_logic_demo_build(void);

/** Restart the demo fleet offset_ms into its script, on the home page. Any
 * task, never blocks; false before app_logic_start or when the logic's queue
 * is full. Implemented in rust/platform. */
bool app_logic_restart_demo(uint32_t offset_ms);

#ifdef __cplusplus
}
#endif
