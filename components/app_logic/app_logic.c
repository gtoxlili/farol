// SPDX-License-Identifier: Apache-2.0
/* The logic itself is Rust (../../rust, linked as the `platform` staticlib).
 * This file anchors the component library the GSP bundle attaches to and
 * checks the build configuration the Rust side assumes. */
#include "app_logic.h"

#include "sdkconfig.h"

/* rust/platform converts milliseconds to ticks 1:1. */
_Static_assert(CONFIG_FREERTOS_HZ == 1000, "app_logic assumes a 1 kHz FreeRTOS tick");

/* The Rust bindings embed gspc's scene constants; fail the build when the
 * scene changed without regenerating them (cargo xtask gen-gsp). */
#include "app_actions.h"
#include "app_binds.h"
#include "app_objects.h"
#include "gsp_bindings_check.h"
