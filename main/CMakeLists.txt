idf_component_register(SRCS "main.c"
    PRIV_REQUIRES app_bench app_camera app_display app_link app_logic app_net esp_iris esp_mosaico_app_recovery
        esp_mosaico_gsp_bundle esp_mosaico_gsp_iris nvs_flash log)
