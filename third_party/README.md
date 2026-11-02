# third_party

Utils' `esp_mosaico_gsp_bundle` (opens the deployable GSP bundle in `ui_apps`)
and `esp_mosaico_gsp_iris` (Iris screen mirror) for an ESP-GSP 1.9.0 application.
Each `CMakeLists.txt` compiles the sources and public headers in
`submodule/esp-mosaico-utils/mosaico-tools/components/<name>` unchanged; only
`idf_component.yml` differs: utils pins `espressif/esp-gsp ==1.5.1`, which the
component manager cannot solve next to this application's `==1.9.0`.

The root `CMakeLists.txt` adds this directory to `EXTRA_COMPONENT_DIRS` in place
of utils' copies. Component names, APIs and contracts are utils'; see their
headers. When utils moves its pin to the application's ESP-GSP version, delete
this directory and list utils' component directories again, as the Hello World
template does. A new ESP-GSP release changes both manifests here together with
`main/idf_component.yml`.
