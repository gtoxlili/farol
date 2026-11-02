# Simulator backend (sim_bridge): the scene plus the Rust logic, built for the
# build host. `project sim` builds and loads it; the IDF build ignores host/.
cmake_minimum_required(VERSION 3.20)
project(app_logic_backend LANGUAGES C)

if(NOT ESP_GSP_COMPONENT_DIR AND DEFINED ENV{ESP_GSP_COMPONENT_DIR})
    set(ESP_GSP_COMPONENT_DIR "$ENV{ESP_GSP_COMPONENT_DIR}")
endif()
if(NOT ESP_GSP_COMPONENT_DIR)
    get_filename_component(ESP_GSP_COMPONENT_DIR
        "${CMAKE_CURRENT_LIST_DIR}/../../../managed_components/espressif__esp-gsp" ABSOLUTE)
endif()
if(NOT EXISTS "${ESP_GSP_COMPONENT_DIR}/tools/sim_bridge/CMakeLists.txt")
    message(FATAL_ERROR "Run the IDF build once to install espressif/esp-gsp, or set ESP_GSP_COMPONENT_DIR")
endif()
add_subdirectory("${ESP_GSP_COMPONENT_DIR}/tools/sim_bridge" sim_bridge)

# The Rust `host` crate (logic + backend glue) as a native staticlib.
find_program(APP_LOGIC_CARGO cargo REQUIRED)
get_filename_component(app_logic_workspace "${CMAKE_CURRENT_LIST_DIR}/../../../rust" ABSOLUTE)
set(app_logic_cargo_dir "${CMAKE_BINARY_DIR}/cargo")
set(app_logic_host_lib "${app_logic_cargo_dir}/release/${CMAKE_STATIC_LIBRARY_PREFIX}host${CMAKE_STATIC_LIBRARY_SUFFIX}")
add_custom_target(app_logic_host_cargo
    COMMAND "${CMAKE_COMMAND}" -E env "CARGO_TARGET_DIR=${app_logic_cargo_dir}"
        "${APP_LOGIC_CARGO}" build --release --locked -p host
    WORKING_DIRECTORY "${app_logic_workspace}"
    BYPRODUCTS "${app_logic_host_lib}"
    COMMENT "cargo build host"
    USES_TERMINAL
    VERBATIM)
add_library(app_logic_host STATIC IMPORTED)
set_target_properties(app_logic_host PROPERTIES IMPORTED_LOCATION "${app_logic_host_lib}")
if(NOT APPLE)
    # The Rust library calls the bridge's GSP API; GNU ld needs the bridge after
    # it (Apple's ld resolves in any order), plus the native libraries Rust's std
    # needs on Linux (cargo --print native-static-libs).
    set_property(TARGET app_logic_host APPEND PROPERTY INTERFACE_LINK_LIBRARIES
        esp_gsp::sim_bridge pthread dl m)
endif()

gsp_add_backend(app_logic_backend
    SOURCES platform_pc.c
    INCLUDE_DIRECTORIES .. ../include
    SCENES ../ui/main.json)
foreach(backend app_logic_backend app_logic_backend_module)
    add_dependencies(${backend} app_logic_host_cargo)
    target_link_libraries(${backend} PRIVATE app_logic_host)
endforeach()
