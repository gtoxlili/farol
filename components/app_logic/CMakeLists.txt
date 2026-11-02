# Application logic: the GSP scene (ui/main.json) and the Rust workspace in
# ../../rust, whose `platform` crate is built here by cargo as a no_std
# staticlib for riscv32imafc-unknown-none-elf. host/ is the simulator backend's
# separate CMake project and is not part of the device build.
idf_component_register(SRCS "app_logic.c" INCLUDE_DIRS "include" REQUIRES esp-gsp
    PRIV_REQUIRES heap log)

include("${MOSAICO_UTILS_ROOT}/mosaico-tools/cmake/gsp_bundle.cmake")
mosaico_gsp_add_ui_bundle(${COMPONENT_LIB} "ui/main.json")

# Runtime text (agent titles, questions, replies) is Chinese and Latin of any
# kind, beyond what the bundle's baked glyphs cover: GSP rasterizes the rest
# from this font with FreeType. It is linked into the application image (the
# bundle partition is 1 MB) and copied to PSRAM at boot (app_logic.c).
gsp_enable_freetype()
target_add_binary_data(${COMPONENT_LIB} "ui/fonts/NotoSansSC-Farol.ttf" BINARY)

set(app_logic_workspace "${CMAKE_CURRENT_LIST_DIR}/../../rust")
set(app_logic_target "riscv32imafc-unknown-none-elf")
set(app_logic_cargo_dir "${CMAKE_BINARY_DIR}/cargo")
set(app_logic_staticlib "${app_logic_cargo_dir}/${app_logic_target}/release/libplatform.a")

find_program(APP_LOGIC_CARGO cargo)
find_program(APP_LOGIC_RUSTC rustc)
if(NOT APP_LOGIC_CARGO OR NOT APP_LOGIC_RUSTC)
    message(FATAL_ERROR "cargo not found on PATH; install Rust with rustup")
endif()

# Keep the build host's paths out of the image (panic locations, line tables).
execute_process(COMMAND "${APP_LOGIC_RUSTC}" --print sysroot
    WORKING_DIRECTORY "${app_logic_workspace}"
    OUTPUT_VARIABLE app_logic_sysroot OUTPUT_STRIP_TRAILING_WHITESPACE
    COMMAND_ERROR_IS_FATAL ANY)
if(DEFINED ENV{CARGO_HOME})
    set(app_logic_cargo_home "$ENV{CARGO_HOME}")
else()
    set(app_logic_cargo_home "$ENV{HOME}/.cargo")
endif()
get_filename_component(app_logic_workspace_abs "${app_logic_workspace}" REALPATH)
set(app_logic_remap
    "--remap-path-prefix=${app_logic_sysroot}/lib/rustlib/src/rust=/rustc"
    "--remap-path-prefix=${app_logic_cargo_home}/registry/src=/cargo"
    "--remap-path-prefix=${app_logic_workspace_abs}=${CMAKE_PROJECT_NAME}")
list(TRANSFORM app_logic_remap PREPEND "\"")
list(TRANSFORM app_logic_remap APPEND "\"")
list(JOIN app_logic_remap "," app_logic_remap)

# cargo is incremental: this runs on every build and relinks only when the
# staticlib changes. rustup reads rust-toolchain.toml from the working directory.
add_custom_target(app_logic_cargo
    COMMAND "${CMAKE_COMMAND}" -E env "CARGO_TARGET_DIR=${app_logic_cargo_dir}"
        "${APP_LOGIC_CARGO}" build --release --locked -p platform --target "${app_logic_target}"
        --config "target.${app_logic_target}.rustflags=[${app_logic_remap}]"
    WORKING_DIRECTORY "${app_logic_workspace}"
    BYPRODUCTS "${app_logic_staticlib}"
    COMMENT "cargo build platform (${app_logic_target})"
    USES_TERMINAL
    VERBATIM)

# Components whose C symbols the Rust code calls directly.
add_prebuilt_library(app_logic_staticlib "${app_logic_staticlib}"
    PRIV_REQUIRES espressif__esp-gsp heap log esp_timer freertos nvs_flash esp_hw_support
        app_display app_link app_net)
add_dependencies(app_logic_staticlib app_logic_cargo)
target_link_libraries(${COMPONENT_LIB} INTERFACE app_logic_staticlib)
