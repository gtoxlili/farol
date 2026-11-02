# Builds utils' esp_mosaico_gsp_iris sources unchanged; only the manifest
# beside this file differs (see ../README.md).
set(upstream "${MOSAICO_UTILS_ROOT}/mosaico-tools/components/esp_mosaico_gsp_iris")
idf_component_register(SRCS "${upstream}/iris_screen_mirror.c" INCLUDE_DIRS "${upstream}/include"
    REQUIRES esp-gsp esp_iris
    PRIV_REQUIRES esp-mosaico-bsp esp_display_present esp_lcd freertos)
target_link_options(${COMPONENT_LIB} INTERFACE
    "-Wl,--wrap=esp_display_presenter_submit_buffer")
