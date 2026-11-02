// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdint.h>

#include "esp_gsp.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Start the Rust logic (task "logic") for a running UI; contract: ../README.md.
 * Call once from app_main after esp_gsp_esp_lcd_start(). ui must stay alive
 * until reboot. Returns 0, or a negative code naming the failed start step. */
int32_t app_logic_start(esp_gsp_handle_t ui);

#ifdef __cplusplus
}
#endif
