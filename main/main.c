// SPDX-License-Identifier: Apache-2.0
#include "sdkconfig.h"
#if CONFIG_FAROL_BENCH
#include "app_bench.h"
#endif
#include "app_camera.h"
#include "app_display.h"
#include "app_link.h"
#include "app_logic.h"
#include "app_net.h"
#include "esp_gsp_esp_lcd.h"
#include "esp_iris.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "iris_ota_support.h"
#include "iris_screen_mirror.h"
#include "nvs_flash.h"
#include "ui_bundle.h"

static const char *TAG = "farol";
/* NVS namespace (default partition) shared with the Rust logic's settings. */
#define FAROL_NVS_NAMESPACE "farol"

#if CONFIG_FAROL_BENCH
/* Every bench run starts from the same demo moment: 14 s into the script the
 * question is up and "runtime" is mid-turn, so the fourth card is a running
 * agent with its shimmer bar, and the hold stops it. */
#define BENCH_DEMO_AT_MS 14000

static bool bench_reset_scene(void)
{
    return app_logic_restart_demo(BENCH_DEMO_AT_MS);
}
#endif

/* Panel commands stop GSP's frames first (app_display_guard_t); app_feedback's
 * task is the only caller, so the pause token needs no lock. */
#define PANEL_PAUSE_TIMEOUT_MS 500
#define PANEL_RESUME_ATTEMPTS 3
static esp_gsp_esp_lcd_pause_t *s_frames_paused;
static esp_gsp_handle_t *s_ui;

static void frames_resume(void *ctx)
{
    (void)ctx;
    for (int attempt = 0; s_frames_paused && attempt < PANEL_RESUME_ATTEMPTS; ++attempt) {
        esp_gsp_handle_t ui = NULL;
        const esp_err_t err = esp_gsp_esp_lcd_resume_paused(s_frames_paused, &ui);
        if (err == ESP_OK) {
            s_frames_paused = NULL;
            if (ui != *s_ui) {
                ESP_LOGW(TAG, "UI resumed with another handle");
            }
            return;
        }
        ESP_LOGW(TAG, "UI resume failed (%s)", esp_err_to_name(err));
        vTaskDelay(pdMS_TO_TICKS(50));
    }
}

static bool frames_pause(void *ctx)
{
    (void)ctx;
    const esp_err_t err = esp_gsp_esp_lcd_pause(*s_ui, PANEL_PAUSE_TIMEOUT_MS, &s_frames_paused);
    if (err == ESP_OK) {
        return true;
    }
    if (err == ESP_ERR_NOT_FINISHED) {
        frames_resume(NULL); /* the token is valid but frames may still run */
    }
    ESP_LOGW(TAG, "UI pause failed (%s); panel command skipped", esp_err_to_name(err));
    return false;
}

/* The composition root retains owned handles; all live until reboot. */
static struct {
    app_display_t display;
    esp_lcd_touch_handle_t touch;   /* what GSP reads: the panel, or the bench script */
    esp_gsp_handle_t ui;
} application;

void app_main(void)
{
    esp_err_t err;
    ESP_ERROR_CHECK_WITHOUT_ABORT(esp_iris_boot_probe());
    ESP_ERROR_CHECK(nvs_flash_init());
    /* Recovery must remain reachable even when the UI bundle cannot be loaded. */
    iris_ota_support_start();
    /* Networking before the UI, so ESP-Iris's TCP transport (which listens
     * once a netif exists) is up even when the UI fails. Neither failure is
     * fatal: the logic then reports Wi-Fi off or the link down, and Recovery
     * stays reachable over USB. The link stays idle until the session opens it. */
    err = app_net_start(FAROL_NVS_NAMESPACE);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Wi-Fi unavailable (0x%x); continuing without network", err);
    }
    err = app_link_start();
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Paseo link unavailable (0x%x)", err);
    }

    esp_gsp_config_t config;
    err = ui_bundle_open(&config);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "GSP UI unavailable (0x%x); Recovery RPC remains active", err);
        return;
    }
    ESP_ERROR_CHECK(app_logic_font_apply(&config));
    err = app_display_create(&application.display);
    application.touch = application.display.touch;
#if CONFIG_FAROL_BENCH
    if (err == ESP_OK) {
        err = app_bench_touch_create(&application.touch);
    }
#endif
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Display unavailable (0x%x); Recovery RPC remains active", err);
        return;
    }

    esp_gsp_esp_lcd_config_t display = ESP_GSP_ESP_LCD_CONFIG_INIT();
    display.presenter = application.display.presenter;
    display.touch = application.touch;
#if CONFIG_FAROL_BENCH
    /* The scripted touch has no interrupt line; GSP polls it. */
    display.touch_input_mode = ESP_GSP_TOUCH_INPUT_POLLING;
#else
    /* AUTO: interrupt on the touch INT line when it registers (as the factory
     * firmware), otherwise GSP logs a warning and polls. */
    display.touch_input_mode = ESP_GSP_TOUCH_INPUT_AUTO;
#endif
    /* CO5300 QSPI requires the X origin and width in multiples of 4 pixels. */
    display.render_alignment = (esp_display_present_render_alignment_t) {
        .x_pixels = 4,
        .width_pixels = 4,
    };
    ESP_ERROR_CHECK(iris_screen_mirror_init());
    ESP_ERROR_CHECK(esp_gsp_esp_lcd_start(&config, &display, &application.ui));
    ESP_ERROR_CHECK(iris_screen_mirror_attach(application.ui));
    s_ui = &application.ui;
    app_display_set_guard(&(app_display_guard_t) {
        .pause = frames_pause,
        .resume = frames_resume,
    });
    /* The scan page's camera preview pushes frames to this UI. Without it the
     * logic's camera requests fail and are logged; the rest runs on. */
    err = app_camera_init(application.ui);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "camera unavailable (0x%x)", err);
    }

    const int32_t logic_err = app_logic_start(application.ui);
    if (logic_err != 0) {
        ESP_LOGE(TAG, "Rust logic failed to start (%ld); Recovery RPC remains active",
                 (long)logic_err);
        return;
    }
#if CONFIG_FAROL_BENCH
    static const app_bench_hooks_t bench_hooks = {.reset_scene = bench_reset_scene};
    ESP_ERROR_CHECK(app_bench_start(application.ui, application.display.presenter, &bench_hooks));
#endif
    /* CONFIG_ESP_MAIN_TASK_STACK_SIZE is sized from this line. */
    ESP_LOGI(TAG, "ready (main stack unused %u B, internal free %u B, min %u B)",
             (unsigned)uxTaskGetStackHighWaterMark(NULL),
             (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
             (unsigned)heap_caps_get_minimum_free_size(MALLOC_CAP_INTERNAL));
}
