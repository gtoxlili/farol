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

The board connects to the Paseo daemon the two ways Paseo's own app offers
when you add a host:

- **Scan QR code**, an encrypted relay connection. The board scans the pairing
  code Paseo shows under Settings › your host › 配对设备 and connects through
  Paseo's relay, end-to-end encrypted to the daemon. Nothing has to be exposed.
- **Direct connection**, on a local network or a VPN. You type the host, port,
  SSL and daemon password on the board.

Either way it signs in with the **daemon password** when one is set, then only
calls the Farol plugin's RPCs (`farol.next`, `farol.focus`, `farol.act`; the
protocol is [farol/1](protocol/README.md)). The plugin opens no port and stores
no password.

> **The board keeps what it connects with in plain text** in its flash (NVS):
> the daemon password, and for the relay the pairing code. Anyone holding the
> board and a USB cable can read them. The password gives full access to your
> daemon, and so does the pairing code when the daemon has no password (Paseo
> asks you to guard it like a password). Put Farol only on a board you keep,
> give the daemon a password, and change it if a board is lost.

A direct connection from another network needs an HTTPS address that forwards
WebSockets to the daemon; [docs/remote-access.md](docs/remote-access.md) has
examples for Caddy, Cloudflare Tunnel and Tailscale Funnel. With SSL the
certificate must be publicly trusted.

## What you need

- An ESP-Mosaico board. The camera module in the left slot scans Wi-Fi and
  Paseo's pairing codes; without it, connect directly (typed on the screen). The
  interaction module in the right slot is optional: with it the screen follows
  your presence, adapts to the room's light and the LEDs show the lamp's state.
- A Paseo daemon, 0.14 or newer, with plugins enabled. For the QR code, its
  relay enabled (Paseo asks the first time it shows the pairing code).
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
   Mode. The camera driver patches ESP-IDF while the project configures, so
   Farol builds against a private copy of the pinned ESP-IDF and stops when
   `$IDF_PATH` lacks the patch, to keep a shared checkout untouched. Set the
   copy up once as in
   [components/app_camera/README.md](components/app_camera/README.md#building-with-the-camera),
   then run the command above with its three exports.

2. Plugin. On the Mac that runs the daemon:

   ```sh
   cd plugin && npm install && npm run check
   paseo plugin install "$PWD" --id farol
   ```

## Connect a board

1. Wi-Fi (2.4 GHz). On the board's "no Wi-Fi" page, scan the network code your
   phone shows under its Wi-Fi settings, or choose a network and type its
   password.
2. Paseo. On the board choose "扫描二维码" and scan the code Paseo shows under
   Settings › your host › 配对设备, or choose "直接连接" and type the host, port
   (6767 by default), SSL and the daemon password. The board shows which Paseo
   it is about to join; tap "连接". When the daemon has a password the code did
   not carry, the board asks for it.

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
node tools/relay_vectors.mjs > protocol/fixtures/relay.json   # after Paseo changes its relay
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
- `components/app_net`, `app_link`: Wi-Fi and the WebSocket to Paseo (direct or
  relay; the relay's encryption is in `rust/logic/src/relay.rs`).
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
