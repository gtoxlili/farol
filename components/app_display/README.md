# app_display

Creates the CO5300 panel the way the factory firmware does, instead of through
`bsp_display_new` (which hard-codes a 7,680 B transfer), the presenter GSP draws
into, and the touch controller. It owns display and touch hardware policy only.

| Setting | Value | Factory reference (`references/esp-mosaico-claw`) |
|---|---|---|
| Bus | QSPI 40 MHz x 4, `max_transfer_sz` 460,800 B (one frame) | `boards/esp_mosaico/board_peripherals.yaml:27` |
| Panel IO | `trans_queue_depth` 2, `psram_dma_direct` | `board_devices.yaml:39-48` |
| Init sequence | BSP `s_vendor_init` / `s_handoff_init` (TE on) | BSP `onboard/display.c` |
| Presenter | `TE_SYNC`, TE GPIO 43, byte swap, 10-line PSRAM drawbuf, 2 TE compose buffers, 32 damage areas | `display_service_target.c:109-157`, `display_service.c:467-475` |
| Touch | CST9217 through `bsp_touch_new`, INT GPIO 6 | `board_devices.yaml:76-101` |

```c
app_display_t display;
ESP_ERROR_CHECK(app_display_create(&display));
/* esp_gsp_esp_lcd_config_t: .presenter = display.presenter, .touch = display.touch,
 * render_alignment 4/4 */
```

Call once from `app_main` before GSP starts, not from an ISR or concurrently. On a
display failure everything created so far is released, the error is returned and
the call may be retried. A touch failure is logged and leaves `touch` NULL with
`ESP_OK`: the UI renders without input and Recovery stays usable. The presenter,
panel, IO and touch stay alive until reboot; GSP borrows the presenter and touch,
and the caller must not free them. Pins and the board variant come from the BSP;
power is enabled through `bsp_power_set_vcc_3v3`. The retained bootloader's splash
handoff is honoured as in the BSP (no panel reset when it is present). Requires
`CONFIG_BSP_CO5300_ENABLE_TE`.

Measured on this path (`.agents/analysis/bench-results.md`): a full 480x480 frame
takes 23.7 ms on the wire, more than one 16.7 ms TE period, so whole-screen motion
runs at about 27–30 fps; small damaged regions are cheap. Iris screenshots capture
the pixel buffer before the panel transfer and cannot show panel-side defects.

After creation, `app_display_set_brightness(percent)` writes the CO5300
brightness register (0..100 %) and `app_display_set_enabled(on)` switches panel
scanning, keeping brightness and GSP state. Like the factory display service,
both send their commands to the panel directly while the presenter runs; each
returns `ESP_ERR_INVALID_STATE` before `app_display_create` succeeded.
In Farol both are called only from `app_feedback`'s task, which carries out
the logic's display requests.
