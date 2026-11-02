// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Longest SSID (802.11) and WPA passphrase, without the terminating NUL. */
#define APP_NET_SSID_MAX 32
#define APP_NET_PASSWORD_MAX 64
/** Networks the board keeps; a Join beyond this drops the least recent one. */
#define APP_NET_SAVED_MAX 8
/** Saved names a status reports: the board's own list plus Vibe Mode's network. */
#define APP_NET_STATUS_SAVED_MAX (APP_NET_SAVED_MAX + 1)

/** What the station is doing; mirrors logic::sys::NetState (rust/logic). */
typedef enum {
    APP_NET_OFF = 0,            /*!< Wi-Fi not started (or failed to start) */
    APP_NET_NO_SAVED,           /*!< no network saved, nothing to try */
    APP_NET_NOT_FOUND,          /*!< saved networks exist, none in range */
    APP_NET_CONNECTING,         /*!< scanning for or joining `ssid` */
    APP_NET_WRONG_PASSWORD,     /*!< `ssid` rejected the password */
    APP_NET_NO_ADDRESS,         /*!< associated with `ssid`, no DHCP lease */
    APP_NET_ONLINE,             /*!< associated with an IPv4 address */
} app_net_state_t;

/** One status report. Strings are NUL-terminated UTF-8 (SSIDs as broadcast). */
typedef struct {
    app_net_state_t state;
    char ssid[APP_NET_SSID_MAX + 1];      /*!< network in use or being tried, may be empty */
    int8_t rssi;                          /*!< dBm while online, else 0 */
    char ip[16];                          /*!< dotted IPv4 while online, else empty */
    uint8_t saved_count;
    char saved[APP_NET_STATUS_SAVED_MAX][APP_NET_SSID_MAX + 1]; /*!< most recent first */
} app_net_status_t;

/** One network a scan found: hidden SSIDs skipped, BSSIDs of one SSID merged. */
typedef struct {
    char ssid[APP_NET_SSID_MAX + 1];
    int8_t rssi;                          /*!< dBm of the strongest BSSID */
    bool secure;                          /*!< needs a password */
    bool saved;                           /*!< among the saved networks */
} app_net_ap_t;

/** Called on the app_net task whenever the status changes. Must not block. */
typedef void (*app_net_status_cb_t)(const app_net_status_t *status, void *ctx);
/** Called on the app_net task with a scan's result, strongest first. Must not block. */
typedef void (*app_net_scan_cb_t)(const app_net_ap_t *aps, size_t count, void *ctx);

/** Start the station: netif, Wi-Fi driver, event handlers and the app_net
 * task, then connect to the best saved network in range. Call once from
 * app_main after nvs_flash_init(); `nvs_namespace` (copied) holds the saved
 * list in the default NVS partition. On failure everything this call created
 * is released, the state stays APP_NET_OFF and the other calls return
 * ESP_ERR_INVALID_STATE. Contract: ../README.md. */
esp_err_t app_net_start(const char *nvs_namespace);

/** Register the single consumer. The current status is reported at once, then
 * on every change. Any task; returns without waiting. */
esp_err_t app_net_subscribe(app_net_status_cb_t on_status, app_net_scan_cb_t on_scan, void *ctx);

/** Save `ssid` first in the list (replacing its old password) and connect to
 * it. An empty `password` joins with the saved password when the board has
 * one (own list or Vibe Mode's network), else as an open network; it never
 * overwrites a saved password. Any task; returns at once. */
esp_err_t app_net_join(const char *ssid, const char *password);

/** Scan for networks; answered through `on_scan`. Any task; returns at once. */
esp_err_t app_net_scan(void);

/** Try the saved networks again now (ignored while online). Any task. */
esp_err_t app_net_retry(void);

/** Remove a saved network; disconnects when it is in use. Vibe Mode's shared
 * network is hidden from this board instead of erased. Any task. */
esp_err_t app_net_forget(const char *ssid);

#ifdef __cplusplus
}
#endif
