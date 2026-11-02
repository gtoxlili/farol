# Builds utils' esp_mosaico_gsp_bundle sources unchanged; only the manifest
# beside this file differs (see ../README.md).
set(upstream "${MOSAICO_UTILS_ROOT}/mosaico-tools/components/esp_mosaico_gsp_bundle")
idf_component_register(SRCS "${upstream}/ui_bundle.c" INCLUDE_DIRS "${upstream}/include"
    REQUIRES esp-gsp esp_partition
    PRIV_REQUIRES mbedtls)
