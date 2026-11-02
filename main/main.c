// SPDX-License-Identifier: Apache-2.0
#include "app_display.h"
#include "app_logic.h"
#include "esp_gsp_esp_lcd.h"
#include "esp_iris.h"
#include "esp_log.h"
#include "iris_ota_support.h"
#include "iris_screen_mirror.h"
#include "nvs_flash.h"
#include "ui_bundle.h"

static const char *TAG = "farol";

/* The composition root retains owned handles; all live until reboot. */
static struct {
    app_display_t display;
    esp_gsp_handle_t ui;
} application;

void app_main(void)
{
    ESP_ERROR_CHECK_WITHOUT_ABORT(esp_iris_boot_probe());
    ESP_ERROR_CHECK(nvs_flash_init());
    /* Recovery must remain reachable even when the UI bundle cannot be loaded. */
    iris_ota_support_start();

    esp_gsp_config_t config;
    esp_err_t err = ui_bundle_open(&config);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "GSP UI unavailable (0x%x); Recovery RPC remains active", err);
        return;
    }
    err = app_display_create(&application.display);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Display unavailable (0x%x); Recovery RPC remains active", err);
        return;
    }

    esp_gsp_esp_lcd_config_t display = ESP_GSP_ESP_LCD_CONFIG_INIT();
    display.presenter = application.display.presenter;
    display.touch = application.display.touch;
    /* AUTO: interrupt on the touch INT line when it registers (as the factory
     * firmware), otherwise GSP logs a warning and polls. */
    display.touch_input_mode = ESP_GSP_TOUCH_INPUT_AUTO;
    /* CO5300 QSPI requires the X origin and width in multiples of 4 pixels. */
    display.render_alignment = (esp_display_present_render_alignment_t) {
        .x_pixels = 4,
        .width_pixels = 4,
    };
    ESP_ERROR_CHECK(iris_screen_mirror_init());
    ESP_ERROR_CHECK(esp_gsp_esp_lcd_start(&config, &display, &application.ui));
    ESP_ERROR_CHECK(iris_screen_mirror_attach(application.ui));

    const int32_t logic_err = app_logic_start(application.ui);
    if (logic_err != 0) {
        ESP_LOGE(TAG, "Rust logic failed to start (%ld); Recovery RPC remains active",
                 (long)logic_err);
        return;
    }
    ESP_LOGI(TAG, "ready");
}
