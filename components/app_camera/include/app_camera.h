// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"
#include "esp_gsp.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Side of the square preview, px: the scene's Canvas image `scan_preview`
 *  must be this size, RGB565, unscaled (tools/ui/gen_scene.py PREVIEW). */
#define APP_CAMERA_PREVIEW_SIDE 360

/** A decoded QR payload, raw bytes (not NUL-terminated, may hold any byte).
 *  Runs on the app_cam_qr task; copy and return without blocking. */
typedef void (*app_camera_code_cb_t)(const char *payload, size_t length, void *ctx);

/** A scan session could not start: no camera module claimed in the left
 *  slot, the sensor was not detected, or the stream did not start. Called
 *  once per failed start, never on a normal stop; any task, must not block. */
typedef void (*app_camera_failed_cb_t)(void *ctx);

/** Keep the UI the preview is pushed to. Call once, after the UI started and
 *  before the logic can call app_camera_start(); ui must live until reboot. */
esp_err_t app_camera_init(esp_gsp_handle_t ui);

/** Register the consumer of decoded codes and failed starts (one; a later
 *  call replaces it). Each distinct payload is reported once per session. */
esp_err_t app_camera_subscribe(app_camera_code_cb_t on_code, app_camera_failed_cb_t on_failed,
                               void *ctx);

/** Start a scan session that previews into Canvas bind `preview_bind`.
 *  Returns at once; the camera opens on its own task. Idempotent. */
esp_err_t app_camera_start(uint16_t preview_bind);

/** End the scan session. Returns at once; the task stops the camera, restores
 *  the placeholder and frees every buffer. Idempotent. */
esp_err_t app_camera_stop(void);

#ifdef __cplusplus
}
#endif
