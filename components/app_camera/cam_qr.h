// SPDX-License-Identifier: Apache-2.0
// QR decoding for one scan session: a quirc decoder on its own task, fed
// grayscale frames by the camera task. Private to app_camera.
#pragma once

#include <stdint.h>

#include "app_camera.h"
#include "esp_err.h"

typedef struct cam_qr cam_qr_t;

/** Counters since the previous cam_qr_take_stats() call. */
typedef struct {
    uint32_t decodes;   /*!< frames searched */
    uint32_t codes;     /*!< codes decoded, repeats included */
    uint32_t reported;  /*!< distinct codes reported */
    uint64_t busy_us;   /*!< time spent searching and decoding */
} cam_qr_stats_t;

/** Create the decoder for side x side gray frames and start its task. */
esp_err_t cam_qr_open(int side, app_camera_code_cb_t on_code, void *ctx, cam_qr_t **out);

/** The frame buffer to fill (side x side, 8-bit luma), or NULL while the
 *  decoder is still busy with the previous frame. Camera task only. */
uint8_t *cam_qr_acquire(cam_qr_t *qr);

/** Hand the buffer filled after cam_qr_acquire() to the decoder. */
void cam_qr_submit(cam_qr_t *qr);

void cam_qr_take_stats(cam_qr_t *qr, cam_qr_stats_t *out);

/** Stop the task (waits for the frame in progress) and free everything. */
void cam_qr_close(cam_qr_t *qr);
