// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdbool.h>

#include "esp_display_present.h"
#include "esp_err.h"
#include "esp_gsp.h"
#include "esp_lcd_touch.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Hooks the harness calls into the application under test. */
typedef struct {
    /** Return the scene to its start state at each run start; any task,
     * non-blocking. NULL: runs start from wherever the previous one ended. */
    bool (*reset_scene)(void);
} app_bench_hooks_t;

/** Create the scripted touch device; pass it to GSP in polling mode.
 * Call once before GSP starts; the handle lives until reboot. */
esp_err_t app_bench_touch_create(esp_lcd_touch_handle_t *out_touch);

/** Start the bench task: three scripted runs, one BENCH line per phase.
 * ui and presenter must be the running GSP instance and its presenter;
 * hooks may be NULL. Call once after GSP and the application logic started.
 * Contracts: ../README.md. */
esp_err_t app_bench_start(esp_gsp_handle_t ui, esp_display_presenter_t *presenter,
                          const app_bench_hooks_t *hooks);

#ifdef __cplusplus
}
#endif
