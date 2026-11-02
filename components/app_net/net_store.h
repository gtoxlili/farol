// SPDX-License-Identifier: Apache-2.0
/* Saved networks (private to app_net): the board's own list in NVS and Vibe
 * Mode's shared network, read-only. Owned and used by the app_net task only. */
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "app_net.h"
#include "esp_err.h"

/** NVS namespaces are at most 15 characters. */
#define NET_STORE_NAMESPACE_MAX 15

typedef struct {
    char ssid[APP_NET_SSID_MAX + 1];
    char password[APP_NET_PASSWORD_MAX + 1];
} net_cred_t;

typedef struct {
    char ns[NET_STORE_NAMESPACE_MAX + 1];
    net_cred_t own[APP_NET_SAVED_MAX];     /*!< most recent first */
    uint8_t own_count;
    char hidden_shared[APP_NET_SSID_MAX + 1]; /*!< shared SSID this board forgot */
} net_store_t;

/** Reads the saved list; a missing or unreadable record leaves it empty. */
esp_err_t net_store_load(net_store_t *store, const char *ns);

/** Puts `ssid` first with `password` and persists the list. */
esp_err_t net_store_remember(net_store_t *store, const char *ssid, const char *password);

/** Moves a network of the own list to the front; persists only on a change. */
esp_err_t net_store_touch(net_store_t *store, const char *ssid);

/** Removes `ssid` from the own list and hides it when it is the shared one. */
esp_err_t net_store_forget(net_store_t *store, const char *ssid);

/** Shows the shared network again when it is `ssid` and was hidden. */
void net_store_unhide(net_store_t *store, const char *ssid);

/** Credentials for `ssid` from the own list or the shared network. */
bool net_store_find(const net_store_t *store, const char *ssid, net_cred_t *out);

/** Saved names, own list first then the shared network; returns the count. */
size_t net_store_names(const net_store_t *store,
                       char names[][APP_NET_SSID_MAX + 1], size_t max);

/** Overwrites a credential copy (passwords never outlive their use). */
void net_store_wipe(net_cred_t *cred);
