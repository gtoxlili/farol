# ESP-Mosaico farol

Created with `python mosaico.py project init my_app` from the workspace's
`gsp_rust` template. GSP 1.9.0 renders the scene and runs every motion; the
application logic is Rust, shared unchanged by the board and the GSP simulator.

From the workspace root:

```sh
python mosaico.py project sim --project projects/farol --interactive
python mosaico.py iris system-update --project projects/farol
```

`project sim` needs one IDF build first (it installs `espressif/esp-gsp`).
Use `system-update` for a new app or changed resources/layouts; `app-update`
only for code changes with an identical full partition table and resources.

`project sim --interactive` serves a browser preview with the Rust logic
running; `--headless --duration 3` runs it without a window. `--dump-ppm`
and `--scene-only` render the scene alone, without the logic. The scripted
check `components/app_logic/host/check.json` taps the button, waits for the
state Rust sets, swipes to the clock page and saves PNG screenshots; it runs
through ESP-GSP's runner with the tool versions `mosaico.py` would select:

```sh
P=projects/farol
export ESP_GSP_COMPONENT_DIR=$PWD/$P/managed_components/espressif__esp-gsp
FETCH=submodule/esp-mosaico-utils/mosaico-tools/tools/gsp-sim/fetch_gspc.py
uv run --no-project python $ESP_GSP_COMPONENT_DIR/tools/sim_bridge/run.py \
  --project $P/components/app_logic/host --build-dir $P/build-sim \
  --gspc "$(uv run --no-project python $FETCH)" --host "$(uv run --no-project python $FETCH --sim)" \
  --scenario $P/components/app_logic/host/check.json --report-dir <new directory>
```

## Layout

- `main`: device startup and dependency composition only.
- `components/app_display`: factory-aligned CO5300 panel IO, TE-synchronized
  presenter and touch.
- `components/app_logic`: the scene (`ui/main.json`), the device build of the
  Rust logic, and `host/`, the simulator backend's separate CMake project.
- `rust/`: the Rust workspace:
  - `logic`: state, event handling and the GSP setters (`Ui`), with its tests;
  - `gsp_sys`: bindings to `esp_gsp.h` and the scene's constants, for the board
    (`bindings_riscv32.rs`) and the build host (`bindings_host.rs`);
  - `platform`: board allocator, panic handler, FreeRTOS and timer FFI, and the
    `logic` task (`app_logic_start`);
  - `host`: the simulator backend's glue (`gsp_bridge_app_init` side);
  - `xtask`: `cargo xtask gen-gsp`.
- `third_party`: utils' GSP bundle and Iris mirror components under an ESP-GSP
  1.9.0 manifest (see its README).

Dependency direction: `main -> app_display -> esp_display_present/esp_lcd_touch`,
`main -> app_logic -> ESP-GSP`; `app_logic` links `rust/platform`, which depends
on `logic` and `gsp_sys`. Neither component depends on `main` or on its sibling.

## Data flow

On the board, GSP calls the Rust event callback on its render task; the callback
decodes the event and posts it to a FreeRTOS queue. The `logic` task owns the
`logic::App`, waits on the queue until the app's next deadline, handles the
event or polls, and calls GSP setters, which GSP applies asynchronously before
its next frame. In the simulator, sim_bridge runs callbacks and timers on one
thread, so the host backend calls the same `App` inline and polls it from a GSP
timer.

## Versions and pins

| What | Where | Value |
|---|---|---|
| ESP-GSP | `main/idf_component.yml`, `third_party/*/idf_component.yml` | `==1.9.0` |
| GSPC (scene compiler) | `.gspc_version` | `0.10.0` |
| GSP simulator | follows the installed ESP-GSP version | `1.9.0` |
| Rust | `rust/rust-toolchain.toml` | stable (verified 1.99.1) |
| Rust crates | `rust/Cargo.lock` | bindgen 0.73.2 |

The root `CMakeLists.txt` reads `.gspc_version` for the IDF build, overriding the
workspace's GSPC pin for this application; ESP-GSP's own resolver and the
simulator runner read the same file or the installed component. Change the three
ESP-GSP pins and `.gspc_version` together, following ESP-GSP's CHANGELOG, then
rebuild and regenerate the bindings.

## Rust bindings

After changing named objects, binds or callbacks in `ui/main.json`, or the
ESP-GSP version: run the IDF build once, then `cargo xtask gen-gsp` in `rust/`
(it reads `managed_components/` and `build/` and needs `$IDF_TOOLS_PATH`), then
build again. Both the IDF build and the simulator backend fail while the
checked-in bindings are stale (`components/app_logic/gsp_bindings_check.h`).

`cargo test` in `rust/` runs the logic tests on the build host.

## Font

`components/app_logic/ui/fonts/rhr-medium-subset.ttf` is Resource Han Rounded CN
Medium (OFL, `OFL.txt` beside it) cut to printable ASCII plus the 50 hanzi and
two CJK punctuation marks the scene uses (26 KB). Static label text is baked automatically; runtime text needs
its glyphs in the label's `font_charset`, and the font file must contain them.
To add characters, subset again from a full copy of the font, for example
`projects/xiao_chuang/assets/fonts/rhr-medium.ttf` (GB2312):

```sh
uv run --no-project --with fonttools pyftsubset <full font> \
  --unicodes=U+0020-007E --text-file=<all scene characters> \
  --layout-features='' --no-hinting --desubroutinize \
  --output-file=components/app_logic/ui/fonts/rhr-medium-subset.ttf
```

The bundle only carries glyphs at the sizes and charsets the scene declares;
a 3,755-character runtime charset at 24 px costs about 2.3 MB of `ui_apps`,
which the 1 MB partition here does not hold.

## Conventions

Keep `main` a composition root. Add cohesive `app_*` components only for
independent responsibilities, declare public/private CMake dependencies
explicitly, and keep the graph acyclic. Avoid cross-module globals; pass handles
and value snapshots. Public API comments stay within three lines; ownership,
lifecycle, threading and timeout contracts live in each component README.
Third-party dependencies keep upstream names; never edit `managed_components/`.

Git, editor and hook entry files stay at the root; format rules, project policy
and setup instructions live in `tools/quality/`. See its README to enable hooks.
