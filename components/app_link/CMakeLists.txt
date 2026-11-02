# WebSocket link to Paseo (esp_websocket_client, pinned in main/idf_component.yml).
# Contract: README.md.
idf_component_register(SRCS "app_link.c"
    INCLUDE_DIRS "include"
    PRIV_REQUIRES espressif__esp_websocket_client esp-tls mbedtls tcp_transport heap log)
