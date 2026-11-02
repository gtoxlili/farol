# main: device composition root

Initializes Iris and NVS, starts the Recovery RPC, starts Wi-Fi (`app_net`)
and the idle Paseo link (`app_link`), opens the UI bundle in
`ui_apps`, creates the display (panel, presenter, touch), starts GSP on that
presenter, attaches the Iris screen mirror and starts the Rust logic, in that
order. It exports no headers and holds only owned handles, all alive until
reboot.

Networking starts before the UI so ESP-Iris's TCP transport can listen even
when the UI fails; a Wi-Fi or link failure only logs, and the logic then shows
Wi-Fi off or Paseo down. A bundle, display or logic failure logs and returns
with Recovery reachable;
other failures use ESP-IDF's error-check policy. Touch uses
`ESP_GSP_TOUCH_INPUT_AUTO`: GSP registers the touch INT line (GPIO 6) as the
factory firmware does, and logs `Touch interrupt is unavailable; using polling`
when it cannot.
