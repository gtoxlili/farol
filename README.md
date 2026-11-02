# Farol

[中文](README_CN.md)

A desk lamp for your [Paseo](https://paseo.sh) agents, on the ESP-Mosaico board.
The light tells you how your agents are doing at a glance: cool and breathing
while they work, amber when one is waiting for you, red when a turn failed, and
its level follows your tightest usage quota. Behind the lamp is a deck of cards,
one per agent, where you answer a question, a permission or a plan, stop a
running turn, or mark a finished one as read, all without opening the app.

Farol is two parts, both in this repository:

- the board application (ESP32-S31, GSP renders the interface, Rust decides);
- the Paseo plugin (`plugin/`), which turns Paseo's state into a compact summary
  for the board and carries out the board's actions after checking them.

## How the board talks to Paseo

The board connects to the Paseo daemon's WebSocket the way the web app does and
signs in with the **daemon password**. After that it only calls the Farol
plugin's RPCs (`farol.next`, `farol.focus`, `farol.act`; the protocol is
[farol/1](protocol/README.md)). The plugin opens no port and stores no password.

> **The board stores the daemon password in plain text** in its flash (NVS).
> Anyone holding the board and a USB cable can read it, and the password gives
> full access to your daemon. Put Farol only on a board you keep. If a board is
> lost, change the daemon password.

The board has to reach the daemon over the network, usually from outside your
Mac: an HTTPS address that forwards WebSockets to the daemon, listed in the
daemon's `hostnames`. [docs/remote-access.md](docs/remote-access.md) has
examples for Caddy, Cloudflare Tunnel and Tailscale Funnel. Certificates must be
publicly trusted; self-signed endpoints are not supported.

## What you need

- An ESP-Mosaico board. The camera module in the left slot is used to scan the
  setup codes; without it you type the address and password on the screen. The
  interaction module in the right slot is optional: with it the screen follows
  your presence, adapts to the room's light and the LEDs show the lamp's state.
- A Paseo daemon, 0.14 or newer, with plugins enabled.
- An [ESP-Mosaico workspace](https://github.com/esp-mosaico/esp-mosaico-vibe)
  with its submodules, ESP-IDF at the workspace's pinned commit, Python 3.10+,
  and Rust stable with the `riscv32imafc-unknown-none-elf` target
  (`rust/rust-toolchain.toml` installs it through rustup).

Verified with: workspace `esp-mosaico-vibe` 1872fd8, ESP-IDF 7b9cc1ac, ESP-GSP
1.9.0 (gspc 0.10.0, simulator 1.9.0), Paseo 0.14.0.

## Install

1. Firmware. Put this repository at `projects/farol` inside the workspace, then
   from the workspace root:

   ```sh
   python mosaico.py iris system-update --project projects/farol
   ```

   It builds the firmware and installs it next to the board's retained Vibe
   Mode. The camera support needs the build steps in
   [components/app_camera/README.md](components/app_camera/README.md).

2. Plugin. On the Mac that runs the daemon:

   ```sh
   cd plugin && npm install && npm run check
   paseo plugin install "$PWD" --id farol
   ```

3. Address. Make the daemon reachable from the board
   ([docs/remote-access.md](docs/remote-access.md)) and enter that address in
   Paseo under Settings › Plugins › Farol.

## Connect a board

1. Wi-Fi (2.4 GHz). On the board's "no Wi-Fi" page, scan the network code your
   phone shows under its Wi-Fi settings, or choose a network and type its
   password.
2. Paseo. In Paseo choose "添加设备" (Settings › Plugins › Farol, or "Farol：添加设备"
   in the Command Center), type the daemon password once, and scan the code with
   the board. Without a camera, choose "手动输入" and type the address and the
   password. The board shows which Paseo it is about to join; tap "连接".

## Using it

| Where | Gesture | What happens |
|---|---|---|
| anywhere | swipe left / right | usage ← home → cards |
| anywhere | pull down from the top | control centre: Wi-Fi, Paseo, do not disturb, sound, vibration, brightness |
| home | tap the lamp | the most urgent card |
| cards | swipe up / down | the next agent; most urgent first |
| a question | press and hold an option (0.5 s) | answers with that option |
| a running card | press and hold (1.2 s) | stops this turn; letting go early cancels |
| a finished card | tap "已读" | greys it out; it leaves the deck an hour later |
| AI key | click / hold | most urgent card, again for the lamp / do not disturb for an hour |
| the board | turn face down | do not disturb while it lies there |

A tap never acts on an agent. If "Allow the board to act on agents" is off in
the plugin's settings, the board is read-only. Without the interaction module
the screen dims after two minutes untouched and goes dark after ten; with it,
the screen stays on while someone is near and goes dark three minutes after they
leave. A touch on a dark screen only wakes it.

## Develop

From this directory:

```sh
cd rust && cargo test -p logic                 # logic, protocol and text tests
python tools/dev.py sim                        # simulator in the browser, demo fleet
python tools/dev.py check                      # scripted scenario (screenshots)
python tools/simdrive.py                       # long-press, keyboard and page flows
python tools/dev.py scene                      # after editing tools/ui/gen_scene.py
cd plugin && npm run check                     # plugin typecheck and tests
```

The simulator runs the same Rust logic as the board on a built-in demo fleet.
`plugin/dev/` starts an isolated Paseo daemon with scripted agents and drives
the plugin as a board would, without touching your own daemon. Setting
`CONFIG_FAROL_BENCH` builds a firmware that replays scripted gestures and prints
frame timings ([components/app_bench/README.md](components/app_bench/README.md)).

## Layout

- `main`: startup order only.
- `components/app_display`: CO5300 panel, presenter, touch, brightness.
- `components/app_logic`: the scene (`ui/main.json`, generated by
  `tools/ui/gen_scene.py`), fonts, and the Rust build; `host/` is the simulator
  backend.
- `components/app_net`, `app_link`: Wi-Fi and the WebSocket to Paseo.
- `components/app_feedback`, `app_camera`: vibration, sound, the interaction
  module, sensors; the camera and QR decoding.
- `components/app_bench`: the frame benchmark harness (off by default).
- `rust/logic`: everything the board decides, shared by board, simulator and
  tests; `rust/platform`: the board side; `rust/host`: the simulator side.
- `plugin/`: the Paseo plugin; `protocol/`: farol/1 and the fixtures both sides
  test against.
- `tools/`: scene and font generators, simulator drivers.

## Licence

Apache-2.0 ([LICENSE](LICENSE)). The fonts are subsets of Noto Sans SC and Geist
under the SIL Open Font License (`components/app_logic/ui/fonts/OFL-*.txt`).
