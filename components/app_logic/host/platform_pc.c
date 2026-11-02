// SPDX-License-Identifier: Apache-2.0
/* sim_bridge entry points, delegated to the Rust `host` crate (../../../rust/host),
 * which runs the same logic as the board. */
#include "esp_gsp.h"
#include "gsp_sim_bridge.h"

/* The Rust bindings embed gspc's scene constants; fail the backend build when
 * the scene changed without regenerating them (cargo xtask gen-gsp). */
#include "scene0/app_actions.h"
#include "scene0/app_binds.h"
#include "scene0/app_objects.h"
#include "gsp_bindings_check.h"

esp_gsp_err_t app_logic_host_start(esp_gsp_handle_t ui);
void app_logic_host_stop(esp_gsp_handle_t ui);

esp_gsp_err_t gsp_bridge_app_init(esp_gsp_handle_t ui)
{
    return app_logic_host_start(ui);
}

void gsp_bridge_app_deinit(esp_gsp_handle_t ui)
{
    app_logic_host_stop(ui);
}
