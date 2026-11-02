# Development

Farol has two parts. The board application (`main`, `components/`, `rust/`) runs on the ESP-Mosaico board: GSP draws the screen, and Rust in `rust/logic` decides everything. The Paseo plugin (`plugin/`) turns Paseo's state into a summary for the board and carries out the board's actions. They talk over [farol/1](../protocol/README.md).

Tested with ESP-IDF 7b9cc1ac, ESP-GSP 1.9.0 (gspc 0.10.0, simulator 1.9.0), Rust 1.99 and Paseo 0.14.0.

## Layout

| Path | Contents |
|---|---|
| `main` | Startup order only. |
| `components/app_display` | CO5300 panel, presenter, touch and brightness. |
| `components/app_logic` | The scene (`ui/main.json`, generated), fonts, the Rust build and the simulator backend (`host/`). |
| `components/app_net`, `app_link` | Wi-Fi, and the WebSocket to Paseo. |
| `components/app_feedback` | Haptics, chimes, the microphone, the AI key, the IMU, the fuel gauge and the interaction module. |
| `components/app_camera` | The camera and QR decoding. |
| `components/app_bench` | A frame-timing build, off by default. |
| `rust/logic` | Everything the board decides. Board, simulator and tests share it. |
| `rust/platform`, `rust/host` | The board's side and the simulator's side of it. |
| `plugin/` | The Paseo plugin, its tests and an isolated dev daemon (`plugin/dev/`). |
| `protocol/` | farol/1 and the fixtures both sides test against. |
| `tools/` | Scene and font generators, the simulator driver, media and test-vector generators. |

## ESP-IDF with the camera patch

The camera driver patches ESP-IDF's DVP capture while the project configures. A patched ESP-IDF changes every project built from it, so the root `CMakeLists.txt` stops when `$IDF_PATH` lacks the patch. Set `FAROL_ALLOW_IDF_PATCH=1` to accept the patch, or build from a private copy of the pinned ESP-IDF. [components/app_camera/README.md](../components/app_camera/README.md#building-with-the-camera) has the steps for the copy.

## Build and install

From the workspace root:

```sh
python mosaico.py iris system-update --project projects/farol   # build and install
```

Use `mosaico.py` for every device operation. It keeps the board's Vibe Mode and checks the partition layout.

## Simulator

The simulator runs the same scene and `rust/logic` on built-in sample agents (`rust/logic/src/demo.rs`). From this directory:

```sh
python tools/dev.py sim                  # in the browser
python tools/dev.py sim --welcome        # first boot
python tools/simdrive.py                 # scripted flows with checks and screenshots
python tools/media.py                    # docs/media, in Chinese and English
```

`FAROL_SIM_LANG=en` shows the simulator in English. `tools/media.py` also copies the English overview and clips to `plugin/assets/`, which the plugin's store listing uses (`media` in `plugin/paseo-plugin.json`). The settings-page screenshots (`docs/media/*/plugin-settings.png`) come from the Paseo web app against the isolated dev daemon below. After you edit `tools/ui/gen_scene.py`, run `python tools/dev.py scene`. It regenerates the scene, its headers and the Rust bindings.

## Tests

```sh
cd rust && cargo test -p logic          # logic, protocol, relay encryption, text layout
cd plugin && npm run check              # plugin typecheck and tests
```

`protocol/fixtures/` holds data both sides test against. `relay.json` comes from Paseo's own relay code. Regenerate it with `node tools/relay_vectors.mjs > protocol/fixtures/relay.json` after Paseo changes its relay protocol.

## Against a real Paseo

`plugin/dev/dev-daemon.sh` starts an isolated Paseo daemon. It has its own home, port 47661 and password, no relay, and fake agents from the `scripted-agents` plugin. Nothing reaches your own daemon.

```sh
export FAROL_DEV_DIR=/some/scratch/dir
plugin/dev/dev-daemon.sh up && plugin/dev/dev-daemon.sh install
FAROL_WORKDIR=/empty/dir node plugin/dev/e2e.mjs          # a fake board polls, answers and stops
FAROL_WORKDIR=/empty/dir node plugin/dev/start-e2e.mjs    # language, then a session from a board
plugin/dev/dev-daemon.sh down
```

`start-e2e.mjs` needs an agent profile named `scripted` on the `scripted` provider under `daemon.agentProfiles` in the dev daemon's `config.json`.

## Frame timing

`CONFIG_FAROL_BENCH` builds a firmware that replays scripted gestures and prints frame timings. See [components/app_bench/README.md](../components/app_bench/README.md).
