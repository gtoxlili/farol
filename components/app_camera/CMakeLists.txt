# Scan sessions: the left-slot camera module, the scan page's Canvas preview
# and QR decoding with quirc. Contract: README.md.
idf_component_register(SRCS "app_camera.c" "cam_qr.c"
    INCLUDE_DIRS "include"
    REQUIRES esp-gsp
    PRIV_REQUIRES mosaico_module_camera esp_driver_ppa esp_timer log)
