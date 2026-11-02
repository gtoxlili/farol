// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Why a connection ended; mirrors logic::paseo::Down (rust/logic). */
typedef enum {
    APP_LINK_DOWN_DNS = 0,      /*!< the host name did not resolve */
    APP_LINK_DOWN_TIMEOUT,      /*!< TCP connect, TLS handshake or upgrade timed out */
    APP_LINK_DOWN_TLS,          /*!< TLS failed: untrusted certificate, wrong host, handshake */
    APP_LINK_DOWN_REFUSED,      /*!< `code` = HTTP status of a refused upgrade, 0 = TCP refused */
    APP_LINK_DOWN_CLOSED,       /*!< after opening; `code` = WebSocket close code (1006 abnormal) */
} app_link_down_t;

/** Called on the link's tasks; each must return quickly and not call app_link. */
typedef void (*app_link_open_cb_t)(void *ctx);
/** One complete text message, reassembled from its frames; `text` is not
 * NUL-terminated and is only valid during the call. */
typedef void (*app_link_text_cb_t)(const char *text, size_t length, void *ctx);
typedef void (*app_link_close_cb_t)(app_link_down_t why, uint16_t code, void *ctx);

/** Create the link task, idle until app_link_open(). Call once from app_main;
 * on failure nothing is left behind. Contract: ../README.md. */
esp_err_t app_link_start(void);

/** Register the single consumer before the first app_link_open(). Any task. */
esp_err_t app_link_subscribe(app_link_open_cb_t on_open, app_link_text_cb_t on_text,
                             app_link_close_cb_t on_close, void *ctx);

/** Connect to a ws:// or wss:// URL (copied), replacing any current
 * connection silently. Reports on_open, then on_close once; or on_close only.
 * Any task; returns at once. */
esp_err_t app_link_open(const char *url);

/** Queue one text frame (copied). Dropped with a log when no connection is
 * open. Any task; returns at once. */
esp_err_t app_link_send(const char *text, size_t length);

/** Close the current connection silently (no on_close). Any task; returns at once. */
esp_err_t app_link_close(void);

#ifdef __cplusplus
}
#endif
