# Wi-Fi station manager: the saved networks (own list in NVS plus Vibe Mode's
# shared network) and the app_net task that joins them. Contract: README.md.
idf_component_register(SRCS "app_net.c" "net_store.c"
    INCLUDE_DIRS "include"
    PRIV_REQUIRES esp_wifi esp_netif esp_event esp_timer esp_hw_support nvs_flash log
        esp_mosaico_app_recovery)
