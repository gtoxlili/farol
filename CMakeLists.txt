cmake_minimum_required(VERSION 3.16)
set(MOSAICO_UTILS_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../submodule/esp-mosaico-utils")
set(MOSAICO_BSP_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../submodule/esp-mosaico-bsp")
if(NOT SDKCONFIG)
    set(SDKCONFIG "${CMAKE_BINARY_DIR}/sdkconfig")
endif()
if(NOT SDKCONFIG_DEFAULTS)
    set(SDKCONFIG_DEFAULTS "sdkconfig.defaults;sdkconfig.application.defaults")
endif()

# GSP compiler pin for this application. The workspace pin in utils'
# gsp-toolchain.json serves older applications; this file overrides it here,
# and ESP-GSP's own resolver and the simulator runner read the same file.
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${CMAKE_CURRENT_LIST_DIR}/.gspc_version")
file(STRINGS "${CMAKE_CURRENT_LIST_DIR}/.gspc_version" MOSAICO_GSPC_VERSION LIMIT_COUNT 1)
if(NOT MOSAICO_GSPC_VERSION STREQUAL APP_GSPC_RESOLVED_VERSION)
    unset(GSPC_EXECUTABLE CACHE)
endif()
include("${MOSAICO_UTILS_ROOT}/mosaico-tools/cmake/gsp_compiler.cmake")
set(APP_GSPC_RESOLVED_VERSION "${MOSAICO_GSPC_VERSION}" CACHE INTERNAL "GSPC version GSPC_EXECUTABLE was resolved for")

list(APPEND EXTRA_COMPONENT_DIRS
    "${MOSAICO_UTILS_ROOT}/esp-mosaico-vibe-mode/components/esp_mosaico_app_recovery"
    "${CMAKE_CURRENT_LIST_DIR}/third_party")
include("${MOSAICO_UTILS_ROOT}/esp-mosaico-vibe-mode/cmake/mosaico_idf_project.cmake")
project(farol VERSION 1.0.0)
include("${MOSAICO_UTILS_ROOT}/mosaico-tools/cmake/system_update.cmake")
