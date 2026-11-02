# app_logic

The scene and the application logic. GSP renders the scene and runs every motion
(the page flow's drag and settle, the tap marker's tween); the Rust workspace in
`../../rust` decides everything else.

- `ui/main.json`: scene `app`, 480x480 RGB565. A three-page `page_flow` (`pager`:
  `home`, `clock`, `about`) that the finger drags and GSP settles; on `home` the
  bound label `status`, the button `tap_button` (callback `tap`) and the marker
  `pulse` (runtime `opacity` 0..255); on `clock` the bound label `uptime`.
  Font: `ui/fonts/rhr-medium-subset.ttf` (see the project README).
- `CMakeLists.txt`: packs the scene into the deployable bundle for `ui_apps`, runs
  cargo for `riscv32imafc-unknown-none-elf` and links `libplatform.a`.
- `app_logic.c`: anchors the component, asserts `CONFIG_FREERTOS_HZ == 1000` and
  that the Rust bindings match the generated scene constants (`gsp_bindings_check.h`,
  written by `cargo xtask gen-gsp`).
- `host/`: the simulator backend (a separate CMake project for sim_bridge) that
  links the Rust `host` crate; `host/check.json` is its headless acceptance run.

```c
if (app_logic_start(ui) != 0) { /* logic unavailable; UI and Recovery keep running */ }
```

## Contract

`app_logic_start` runs once on the caller's task after `esp_gsp_esp_lcd_start`;
it registers the GSP event callback and creates the `logic` task (4 KB stack in
internal RAM, priority 5), which owns all logic state. Return values: 0, -1 null
handle or second call, -2 queue allocation, -3 callback registration, -4 task
creation. Nothing is torn down: the UI must live until reboot.

Threading: the GSP callback runs on the render task and only decodes the event
and posts it to a 16-entry queue without blocking; a full queue drops the tap.
The `logic` task waits on that queue until the logic's next deadline, then handles
the event and polls time-driven work. Its GSP setters are asynchronous: GSP queues
them and applies them before its next frame. Rust allocations prefer PSRAM; a
Rust panic logs and aborts into ESP-IDF's panic path (core dump, reboot). The
first setter GSP rejects is logged once (tag `logic`); later ones are counted.

Behaviour owned by Rust (`rust/logic`): each tap sets `status` to "已点击 N 次"
and tweens `pulse` opacity to 255 over 300 ms ease-out (from 0 on the first tap,
from 96 after); `uptime` shows `HH:MM:SS` since start, updated on a one-second
grid that skips missed seconds.

## Simulator

`project sim` builds `host/` into a backend library and loads it into the GSP
simulator; the Rust `host` crate calls the same `logic::App` inline on the
bridge's single thread and polls it from a 50 ms GSP timer. It prints
`app_logic host: started` to stderr once the logic runs, and
`app_logic host: taps=… uptime_s=… rejected=…` when the bridge shuts it down in
order (not when the runner kills the simulator after `--duration`). The backend
build needs `cargo` on PATH and fails while the bindings are stale.
