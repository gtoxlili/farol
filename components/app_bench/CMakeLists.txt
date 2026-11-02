# Frame benchmark harness, built only with CONFIG_FAROL_BENCH (see README.md).
# With the option off the component has no sources and no linker wraps.
# Requirements stay unconditional: ESP-IDF expands them before it reads the
# configuration, and the application requires these components anyway.
set(bench_requires esp-gsp esp_lcd_touch esp_display_present)
set(bench_priv_requires esp_timer heap freertos log)

if(NOT CONFIG_FAROL_BENCH)
    idf_component_register(REQUIRES ${bench_requires} PRIV_REQUIRES ${bench_priv_requires})
    return()
endif()

idf_component_register(SRCS "app_bench.c" "bench_script.c" "bench_probe.c"
    INCLUDE_DIRS "include"
    REQUIRES ${bench_requires}
    PRIV_REQUIRES ${bench_priv_requires})
# Frame probes: wrap the presenter calls GSP makes (render start / commit) and
# the presenter's present-completion step (transfer done, ISR). See README.md.
target_link_options(${COMPONENT_LIB} INTERFACE
    "-Wl,--wrap=esp_display_presenter_begin_next_frame"
    "-Wl,--wrap=esp_display_presenter_commit_frame_with_receipt"
    "-Wl,--wrap=esp_display_present_tracker_complete_present_frame_isr")
