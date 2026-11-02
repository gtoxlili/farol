// SPDX-License-Identifier: Apache-2.0

#include "app_display.h"

#include "bsp/esp_mosaico.h"
#include "bsp/mosaico_boot_handoff.h"
#include "driver/gpio.h"
#include "driver/spi_master.h"
#include "esp_check.h"
#include "esp_lcd_co5300.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_log.h"
#include "sdkconfig.h"

static const char *TAG = "app_display";

/* Panel IO as the factory firmware configures it
 * (references/esp-mosaico-claw/boards/esp_mosaico/board_devices.yaml:39-48,
 * board_peripherals.yaml:27): one whole RGB565 frame per SPI transaction,
 * two queued transactions, DMA straight from PSRAM. The BSP hard-codes
 * 7,680 B, depth 10 and no direct DMA. */
#define DISPLAY_FRAME_BYTES (BSP_LCD_H_RES * BSP_LCD_V_RES * BSP_LCD_BITS_PER_PIXEL / 8)
#define DISPLAY_TRANS_QUEUE_DEPTH 2

/* Presenter policy as the factory display_service
 * (components/display_service/src/display_service_target.c:109-157,
 * display_service.c:467-475). */
#define DISPLAY_DRAWBUF_LINES 10
#define DISPLAY_TE_COMPOSE_BUFFERS 2
#define DISPLAY_MAX_DAMAGE_AREAS 32

#if !CONFIG_BSP_CO5300_ENABLE_TE
#error "app_display presents in TE_SYNC; enable CONFIG_BSP_CO5300_ENABLE_TE"
#endif

/* Same CO5300 sequences as the BSP (onboard/display.c), TE enabled. */
static const co5300_lcd_init_cmd_t s_vendor_init[] = {
    {0x11, NULL, 0, 600},
    {0xFE, (uint8_t[]){0x20}, 1, 0},
    {0x19, (uint8_t[]){0x10}, 1, 0},
    {0x1C, (uint8_t[]){0xA0}, 1, 0},
    {0xFE, (uint8_t[]){0x00}, 1, 0},
    {0xC4, (uint8_t[]){0x80}, 1, 0},
    {0x3A, (uint8_t[]){0x55}, 1, 0},
    {0x35, (uint8_t[]){0x00}, 1, 0},
    {0x53, (uint8_t[]){0x20}, 1, 0},
    {0x51, (uint8_t[]){0xFF}, 1, 0},
    {0x63, (uint8_t[]){0xFF}, 1, 0},
    {0x2A, (uint8_t[]){0x00, 0x00, 0x01, 0xDF}, 4, 0},
    {0x2B, (uint8_t[]){0x00, 0x00, 0x01, 0xDF}, 4, 0},
    {0x29, NULL, 0, 600},
};

/* The retained bootloader already woke the panel and shows its splash;
 * repeat only the idempotent setup (BSP s_handoff_init). */
static const co5300_lcd_init_cmd_t s_handoff_init[] = {
    {0xFE, (uint8_t[]){0x20}, 1, 0},
    {0x19, (uint8_t[]){0x10}, 1, 0},
    {0x1C, (uint8_t[]){0xA0}, 1, 0},
    {0xFE, (uint8_t[]){0x00}, 1, 0},
    {0xC4, (uint8_t[]){0x80}, 1, 0},
    {0x3A, (uint8_t[]){0x55}, 1, 0},
    {0x35, (uint8_t[]){0x00}, 1, 0},
    {0x53, (uint8_t[]){0x20}, 1, 0},
    {0x63, (uint8_t[]){0xFF}, 1, 0},
    {0x2A, (uint8_t[]){0x00, 0x00, 0x01, 0xDF}, 4, 0},
    {0x2B, (uint8_t[]){0x00, 0x00, 0x01, 0xDF}, 4, 0},
};

/* Created once at boot and kept until reboot (single owner: app_main). */
static struct {
    bool bus_ready;
    esp_lcd_panel_io_handle_t io;
    esp_lcd_panel_handle_t panel;
    esp_display_presenter_t *presenter;
} s_display;

static void release(void)
{
    if (s_display.panel) {
        esp_lcd_panel_del(s_display.panel);
        s_display.panel = NULL;
    }
    if (s_display.io) {
        esp_lcd_panel_io_del(s_display.io);
        s_display.io = NULL;
    }
    if (s_display.bus_ready) {
        spi_bus_free(BSP_LCD_SPI_HOST);
        s_display.bus_ready = false;
    }
}

static esp_err_t create_panel(void)
{
    const bool boot_panel_ready = mosaico_boot_handoff_consume();
    ESP_RETURN_ON_ERROR(bsp_power_set_vcc_3v3(true), TAG, "enable VCC_3V3");
    bsp_board_variant_t variant;
    ESP_RETURN_ON_ERROR(bsp_board_variant_get(&variant), TAG, "board variant");
    const bool v1_0 = variant == BSP_BOARD_VARIANT_V1_0;
    const gpio_num_t scl = v1_0 ? BSP_LCD_SCL_V1_0 : BSP_LCD_SCL_V1_2;
    const gpio_num_t rst = v1_0 ? BSP_LCD_RST_V1_0 : BSP_LCD_RST_V1_2;

    const spi_bus_config_t bus = CO5300_PANEL_BUS_QSPI_CONFIG(
                                     scl, BSP_LCD_DATA0, BSP_LCD_DATA1, BSP_LCD_DATA2, BSP_LCD_DATA3,
                                     DISPLAY_FRAME_BYTES);
    ESP_RETURN_ON_ERROR(spi_bus_initialize(BSP_LCD_SPI_HOST, &bus, SPI_DMA_CH_AUTO),
                        TAG, "QSPI bus");
    s_display.bus_ready = true;
    const gpio_num_t pins[] = {scl, BSP_LCD_DATA0, BSP_LCD_DATA1, BSP_LCD_DATA2, BSP_LCD_DATA3};
    for (size_t i = 0; i < sizeof(pins) / sizeof(pins[0]); ++i) {
        ESP_RETURN_ON_ERROR(gpio_set_drive_capability(
                                pins[i], (gpio_drive_cap_t)CONFIG_BSP_LCD_QSPI_DRIVE_CAP),
                            TAG, "QSPI drive GPIO%d", (int)pins[i]);
    }

    esp_lcd_panel_io_spi_config_t io = CO5300_PANEL_IO_QSPI_CONFIG(BSP_LCD_CS, NULL, NULL);
    io.pclk_hz = BSP_LCD_PIXEL_CLOCK_HZ;
    io.trans_queue_depth = DISPLAY_TRANS_QUEUE_DEPTH;
    io.flags.psram_dma_direct = true;
    ESP_RETURN_ON_ERROR(esp_lcd_new_panel_io_spi((esp_lcd_spi_bus_handle_t)BSP_LCD_SPI_HOST,
                                                 &io, &s_display.io),
                        TAG, "panel IO");

    const co5300_vendor_config_t vendor = {
        .init_cmds = boot_panel_ready ? s_handoff_init : s_vendor_init,
        .init_cmds_size = boot_panel_ready
        ? sizeof(s_handoff_init) / sizeof(s_handoff_init[0])
        : sizeof(s_vendor_init) / sizeof(s_vendor_init[0]),
        .flags.use_qspi_interface = true,
    };
    const esp_lcd_panel_dev_config_t panel = {
        .reset_gpio_num = rst,
        .rgb_ele_order = LCD_RGB_ELEMENT_ORDER_RGB,
        .bits_per_pixel = BSP_LCD_BITS_PER_PIXEL,
        .vendor_config = (void *) &vendor,
    };
    ESP_RETURN_ON_ERROR(esp_lcd_new_panel_co5300(s_display.io, &panel, &s_display.panel),
                        TAG, "CO5300 panel");
    if (!boot_panel_ready) {
        ESP_RETURN_ON_ERROR(esp_lcd_panel_reset(s_display.panel), TAG, "panel reset");
    }
    ESP_RETURN_ON_ERROR(esp_lcd_panel_init(s_display.panel), TAG, "panel init");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_set_gap(s_display.panel, BSP_LCD_X_GAP, BSP_LCD_Y_GAP),
                        TAG, "panel gap");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_swap_xy(s_display.panel, BSP_LCD_BASE_SWAP_XY), TAG, "swap");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_mirror(s_display.panel, BSP_LCD_BASE_MIRROR_X,
                                             BSP_LCD_BASE_MIRROR_Y), TAG, "mirror");
    if (!boot_panel_ready) {
        ESP_RETURN_ON_ERROR(esp_lcd_panel_disp_on_off(s_display.panel, true), TAG, "display on");
    }
    return ESP_OK;
}

static esp_err_t create_presenter(void)
{
    const esp_display_presenter_config_t config = {
        .width = BSP_LCD_H_RES,
        .height = BSP_LCD_V_RES,
        .pixel_format = ESP_DISPLAY_PRESENT_PIXEL_FORMAT_RGB565,
        .max_damage_areas = DISPLAY_MAX_DAMAGE_AREAS,
        .target = {
            .hw = {
                .panel = s_display.panel,
                .io = s_display.io,
                .panel_type = ESP_DISPLAY_PRESENT_PANEL_IO,
                .input_pixel_format = ESP_DISPLAY_PRESENT_PIXEL_FORMAT_RGB565,
                .rotation = ESP_DISPLAY_PRESENT_ROTATE_0,
                .swap_bytes = true,
                .te_enabled = true,
                .te_sync = {
                    .gpio_num = BSP_LCD_TE,
                    .bus_freq_hz = BSP_LCD_PIXEL_CLOCK_HZ,
                    .data_lines = BSP_LCD_DATA_WIDTH,
                },
            },
            .fb = {
                .mode = ESP_DISPLAY_PRESENT_MODE_TE_SYNC,
            },
            .drawbuf = {
                .lines = DISPLAY_DRAWBUF_LINES,
                .in_psram = true,
                .te_compose_buffers = DISPLAY_TE_COMPOSE_BUFFERS,
            },
        },
    };
    return esp_display_presenter_create(&config, &s_display.presenter);
}

/* Touch is optional: without it the UI still renders and Recovery stays usable. */
static esp_lcd_touch_handle_t create_touch(void)
{
    esp_lcd_touch_handle_t touch = NULL;
    const esp_err_t err = bsp_touch_new(BSP_LCD_ROTATION_DEFAULT, &touch);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "touch unavailable (%s); continuing without it", esp_err_to_name(err));
        return NULL;
    }
    return touch;
}

esp_err_t app_display_create(app_display_t *out)
{
    ESP_RETURN_ON_FALSE(out != NULL, ESP_ERR_INVALID_ARG, TAG, "output is null");
    ESP_RETURN_ON_FALSE(s_display.presenter == NULL, ESP_ERR_INVALID_STATE, TAG,
                        "already created");
    esp_err_t err = create_panel();
    if (err == ESP_OK) {
        err = create_presenter();
    }
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "display creation failed: %s", esp_err_to_name(err));
        release();
        return err;
    }
    ESP_LOGI(TAG, "CO5300 %dx%d QSPI %d MHz, transfer %d B, depth %d, PSRAM DMA; "
             "presenter TE_SYNC GPIO%d, %d-line PSRAM drawbuf, %d compose buffers",
             BSP_LCD_H_RES, BSP_LCD_V_RES, BSP_LCD_PIXEL_CLOCK_HZ / 1000000,
             DISPLAY_FRAME_BYTES, DISPLAY_TRANS_QUEUE_DEPTH, BSP_LCD_TE,
             DISPLAY_DRAWBUF_LINES, DISPLAY_TE_COMPOSE_BUFFERS);
    *out = (app_display_t) {
        .presenter = s_display.presenter,
        .touch = create_touch(),
    };
    return ESP_OK;
}

esp_err_t app_display_set_brightness(uint8_t percent)
{
    ESP_RETURN_ON_FALSE(s_display.panel, ESP_ERR_INVALID_STATE, TAG, "display not created");
    ESP_RETURN_ON_FALSE(percent <= 100, ESP_ERR_INVALID_ARG, TAG, "brightness above 100%%");
    return esp_lcd_panel_co5300_set_brightness(s_display.panel, percent);
}

esp_err_t app_display_set_enabled(bool enabled)
{
    ESP_RETURN_ON_FALSE(s_display.panel, ESP_ERR_INVALID_STATE, TAG, "display not created");
    return esp_lcd_panel_disp_on_off(s_display.panel, enabled);
}
