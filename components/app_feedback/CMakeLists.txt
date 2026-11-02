# Haptics, chimes, the AI key, IMU, fuel gauge, the interaction module and
# display power for Farol. See README.md.
idf_component_register(SRCS "app_feedback.c" "fb_chime.c" "fb_module.c" "fb_sensors.c"
    INCLUDE_DIRS "include"
    PRIV_REQUIRES app_display esp_timer log)

# The chimes are synthesized at build time by tools/chimes.py (standard library
# only, deterministic) and embedded as _binary_chime_<name>_pcm_start/_end.
idf_build_get_property(python PYTHON)
set(chime_script "${CMAKE_CURRENT_LIST_DIR}/../../tools/chimes.py")
set(chime_dir "${CMAKE_CURRENT_BINARY_DIR}/chimes")
set(chime_files "${chime_dir}/chime_call.pcm" "${chime_dir}/chime_done.pcm" "${chime_dir}/chime_fail.pcm")
add_custom_command(OUTPUT ${chime_files}
    COMMAND "${python}" "${chime_script}" --out "${chime_dir}"
    DEPENDS "${chime_script}"
    COMMENT "Synthesizing Farol chimes"
    VERBATIM)
foreach(chime_file ${chime_files})
    target_add_binary_data(${COMPONENT_LIB} "${chime_file}" BINARY)
endforeach()
