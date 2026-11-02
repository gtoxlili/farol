// SPDX-License-Identifier: Apache-2.0
/* The logic itself is Rust (../../rust, linked as the `platform` staticlib).
 * This file anchors the component library the GSP bundle attaches to, checks
 * the build configuration the Rust side assumes, and hands GSP the dynamic
 * font. */
#include "app_logic.h"

#include <string.h>

#include "esp_heap_caps.h"
#include "esp_log.h"
#include "sdkconfig.h"

/* rust/platform converts milliseconds to ticks 1:1. */
_Static_assert(CONFIG_FREERTOS_HZ == 1000, "app_logic assumes a 1 kHz FreeRTOS tick");

/* The Rust bindings embed gspc's scene constants; fail the build when the
 * scene changed without regenerating them (cargo xtask gen-gsp). */
#include "app_actions.h"
#include "app_binds.h"
#include "app_objects.h"
#include "gsp_bindings_check.h"

static const char *TAG = "app_logic";

bool app_logic_demo_build(void)
{
#if CONFIG_FAROL_DEMO
    return true;
#else
    return false;
#endif
}

/* target_add_binary_data(... NotoSansSC-Farol.ttf BINARY) in CMakeLists.txt */
extern const uint8_t font_start[] asm("_binary_NotoSansSC_Farol_ttf_start");
extern const uint8_t font_end[] asm("_binary_NotoSansSC_Farol_ttf_end");

esp_err_t app_logic_font_apply(esp_gsp_config_t *config)
{
    if (config == NULL) {
        return ESP_ERR_INVALID_ARG;
    }
    const size_t size = (size_t)(font_end - font_start);
    /* Kept for the rest of the boot: the running UI borrows it. */
    static void *s_font;
    if (s_font == NULL) {
        s_font = heap_caps_malloc(size, MALLOC_CAP_SPIRAM);
        if (s_font != NULL) {
            memcpy(s_font, font_start, size);
        } else {
            ESP_LOGW(TAG, "no PSRAM for the %u B font; reading it from flash", (unsigned)size);
        }
    }
    config->ttf = s_font != NULL ? s_font : font_start;
    config->ttf_size = size;
    return ESP_OK;
}
