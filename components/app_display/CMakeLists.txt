idf_component_register(SRCS "app_display.c"
    INCLUDE_DIRS "include"
    REQUIRES esp_display_present esp_lcd_touch
    PRIV_REQUIRES esp-mosaico-bsp esp_lcd esp_lcd_co5300 esp_driver_spi esp_driver_gpio log)
