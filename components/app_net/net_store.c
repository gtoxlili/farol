// SPDX-License-Identifier: Apache-2.0
#include "net_store.h"

#include <stdlib.h>
#include <string.h>

#include "esp_check.h"
#include "esp_log.h"
#include "mosaico_shared.h"
#include "nvs.h"

/* Record layout of KEY_LIST: version, count, then per network a length-
 * prefixed SSID and a length-prefixed password. Bump the version on change. */
#define KEY_LIST "wifi"
#define KEY_HIDDEN "wifi_hidden"
#define LIST_VERSION 1
#define LIST_MAX_BYTES (2 + APP_NET_SAVED_MAX * (2 + APP_NET_SSID_MAX + APP_NET_PASSWORD_MAX))
/* Vibe Mode > Settings > Shared information (mosaico_shared.h). */
#define SHARED_SSID "wifi_ssid"
#define SHARED_PASSWORD "wifi_password"

static const char *TAG = "app_net";

void net_store_wipe(net_cred_t *cred)
{
    /* volatile keeps the compiler from dropping a store to dead memory */
    volatile char *p = (volatile char *)cred->password;
    for (size_t i = 0; i < sizeof(cred->password); ++i) {
        p[i] = 0;
    }
}

static bool cred_valid(const char *ssid, const char *password)
{
    const size_t ssid_len = strnlen(ssid, APP_NET_SSID_MAX + 1);
    const size_t password_len = strnlen(password, APP_NET_PASSWORD_MAX + 1);
    return ssid_len >= 1 && ssid_len <= APP_NET_SSID_MAX && password_len <= APP_NET_PASSWORD_MAX;
}

static int own_index(const net_store_t *store, const char *ssid)
{
    for (int i = 0; i < store->own_count; ++i) {
        if (strcmp(store->own[i].ssid, ssid) == 0) {
            return i;
        }
    }
    return -1;
}

static esp_err_t persist(const net_store_t *store)
{
    uint8_t *blob = calloc(1, LIST_MAX_BYTES);
    ESP_RETURN_ON_FALSE(blob != NULL, ESP_ERR_NO_MEM, TAG, "no memory for the saved list");
    size_t n = 0;
    blob[n++] = LIST_VERSION;
    blob[n++] = store->own_count;
    for (int i = 0; i < store->own_count; ++i) {
        const size_t ssid_len = strlen(store->own[i].ssid);
        const size_t password_len = strlen(store->own[i].password);
        blob[n++] = (uint8_t)ssid_len;
        memcpy(blob + n, store->own[i].ssid, ssid_len);
        n += ssid_len;
        blob[n++] = (uint8_t)password_len;
        memcpy(blob + n, store->own[i].password, password_len);
        n += password_len;
    }
    nvs_handle_t nvs;
    esp_err_t err = nvs_open(store->ns, NVS_READWRITE, &nvs);
    if (err == ESP_OK) {
        err = nvs_set_blob(nvs, KEY_LIST, blob, n);
        if (err == ESP_OK) {
            err = store->hidden_shared[0] ? nvs_set_str(nvs, KEY_HIDDEN, store->hidden_shared)
                  : nvs_erase_key(nvs, KEY_HIDDEN);
            if (err == ESP_ERR_NVS_NOT_FOUND) {
                err = ESP_OK;
            }
        }
        if (err == ESP_OK) {
            err = nvs_commit(nvs);
        }
        nvs_close(nvs);
    }
    memset(blob, 0, LIST_MAX_BYTES);
    free(blob);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "saving the network list failed: %s", esp_err_to_name(err));
    }
    return err;
}

static bool decode(net_store_t *store, const uint8_t *blob, size_t length)
{
    if (length < 2 || blob[0] != LIST_VERSION || blob[1] > APP_NET_SAVED_MAX) {
        return false;
    }
    size_t n = 2;
    const uint8_t count = blob[1];
    for (uint8_t i = 0; i < count; ++i) {
        net_cred_t *cred = &store->own[i];
        if (n >= length || blob[n] < 1 || blob[n] > APP_NET_SSID_MAX || n + 1 + blob[n] >= length) {
            return false;
        }
        memcpy(cred->ssid, blob + n + 1, blob[n]);
        n += 1 + blob[n];
        if (blob[n] > APP_NET_PASSWORD_MAX || n + 1 + blob[n] > length) {
            return false;
        }
        memcpy(cred->password, blob + n + 1, blob[n]);
        n += 1 + blob[n];
    }
    store->own_count = count;
    return n == length;
}

esp_err_t net_store_load(net_store_t *store, const char *ns)
{
    memset(store, 0, sizeof(*store));
    ESP_RETURN_ON_FALSE(ns != NULL && strlen(ns) >= 1 && strlen(ns) <= NET_STORE_NAMESPACE_MAX,
                        ESP_ERR_INVALID_ARG, TAG, "bad NVS namespace");
    strlcpy(store->ns, ns, sizeof(store->ns));
    nvs_handle_t nvs;
    esp_err_t err = nvs_open(ns, NVS_READONLY, &nvs);
    if (err == ESP_ERR_NVS_NOT_FOUND) {
        return ESP_OK; /* nothing saved yet */
    }
    ESP_RETURN_ON_ERROR(err, TAG, "open NVS namespace");
    uint8_t *blob = calloc(1, LIST_MAX_BYTES);
    if (blob == NULL) {
        nvs_close(nvs);
        return ESP_ERR_NO_MEM;
    }
    size_t length = LIST_MAX_BYTES;
    err = nvs_get_blob(nvs, KEY_LIST, blob, &length);
    if (err == ESP_OK && !decode(store, blob, length)) {
        ESP_LOGW(TAG, "saved network list unreadable; starting empty");
        memset(store->own, 0, sizeof(store->own));
        store->own_count = 0;
    } else if (err != ESP_OK && err != ESP_ERR_NVS_NOT_FOUND) {
        ESP_LOGW(TAG, "reading the network list failed: %s", esp_err_to_name(err));
    }
    size_t hidden_size = sizeof(store->hidden_shared);
    if (nvs_get_str(nvs, KEY_HIDDEN, store->hidden_shared, &hidden_size) != ESP_OK) {
        store->hidden_shared[0] = '\0';
    }
    memset(blob, 0, LIST_MAX_BYTES);
    free(blob);
    nvs_close(nvs);
    return ESP_OK;
}

/* Vibe Mode's network, unless this board hid it or keeps its own copy. */
static bool shared_get(const net_store_t *store, net_cred_t *out)
{
    memset(out, 0, sizeof(*out));
    if (mosaico_shared_get(SHARED_SSID, out->ssid, sizeof(out->ssid)) != ESP_OK || out->ssid[0] == '\0') {
        return false;
    }
    if (mosaico_shared_get(SHARED_PASSWORD, out->password, sizeof(out->password)) != ESP_OK) {
        out->password[0] = '\0';
    }
    if (strcmp(out->ssid, store->hidden_shared) == 0 || own_index(store, out->ssid) >= 0) {
        net_store_wipe(out);
        return false;
    }
    return true;
}

esp_err_t net_store_remember(net_store_t *store, const char *ssid, const char *password)
{
    ESP_RETURN_ON_FALSE(cred_valid(ssid, password), ESP_ERR_INVALID_ARG, TAG, "bad network credentials");
    int at = own_index(store, ssid);
    if (at < 0) {
        at = store->own_count < APP_NET_SAVED_MAX ? store->own_count++ : APP_NET_SAVED_MAX - 1;
    }
    memmove(&store->own[1], &store->own[0], (size_t)at * sizeof(net_cred_t));
    strlcpy(store->own[0].ssid, ssid, sizeof(store->own[0].ssid));
    memset(store->own[0].password, 0, sizeof(store->own[0].password));
    strlcpy(store->own[0].password, password, sizeof(store->own[0].password));
    if (strcmp(store->hidden_shared, ssid) == 0) {
        store->hidden_shared[0] = '\0';
    }
    return persist(store);
}

esp_err_t net_store_touch(net_store_t *store, const char *ssid)
{
    const int at = own_index(store, ssid);
    if (at <= 0) {
        return ESP_OK;
    }
    net_cred_t moved = store->own[at];
    memmove(&store->own[1], &store->own[0], (size_t)at * sizeof(net_cred_t));
    store->own[0] = moved;
    net_store_wipe(&moved);
    return persist(store);
}

esp_err_t net_store_forget(net_store_t *store, const char *ssid)
{
    bool changed = false;
    const int at = own_index(store, ssid);
    if (at >= 0) {
        net_store_wipe(&store->own[at]);
        memmove(&store->own[at], &store->own[at + 1],
                (size_t)(store->own_count - at - 1) * sizeof(net_cred_t));
        --store->own_count;
        memset(&store->own[store->own_count], 0, sizeof(net_cred_t));
        changed = true;
    }
    net_cred_t shared;
    if (shared_get(store, &shared) && strcmp(shared.ssid, ssid) == 0) {
        strlcpy(store->hidden_shared, ssid, sizeof(store->hidden_shared));
        changed = true;
    }
    net_store_wipe(&shared);
    return changed ? persist(store) : ESP_ERR_NOT_FOUND;
}

void net_store_unhide(net_store_t *store, const char *ssid)
{
    if (store->hidden_shared[0] != '\0' && strcmp(store->hidden_shared, ssid) == 0) {
        store->hidden_shared[0] = '\0';
        (void)persist(store);
    }
}

bool net_store_find(const net_store_t *store, const char *ssid, net_cred_t *out)
{
    const int at = own_index(store, ssid);
    if (at >= 0) {
        *out = store->own[at];
        return true;
    }
    if (shared_get(store, out) && strcmp(out->ssid, ssid) == 0) {
        return true;
    }
    net_store_wipe(out);
    return false;
}

size_t net_store_names(const net_store_t *store, char names[][APP_NET_SSID_MAX + 1], size_t max)
{
    size_t n = 0;
    for (int i = 0; i < store->own_count && n < max; ++i) {
        strlcpy(names[n++], store->own[i].ssid, APP_NET_SSID_MAX + 1);
    }
    net_cred_t shared;
    if (n < max && shared_get(store, &shared)) {
        strlcpy(names[n++], shared.ssid, APP_NET_SSID_MAX + 1);
    }
    net_store_wipe(&shared);
    return n;
}
