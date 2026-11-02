# farol/1

The board talks to the Farol plugin with plugin RPCs on the daemon's own WebSocket. It reaches the daemon the two ways Paseo's app does (see [Connecting](#connecting)). It sends a `hello`, with the daemon password when one is set, and after that only `plugin.rpc.invoke.request` frames with `pluginId: "farol"`, plus Paseo's dictation messages for a new session. The plugin opens no port and stores no password.

The board reads with `rust/logic/src/model.rs` (serde names below). The plugin's schemas are in `plugin/shared/protocol.ts`. Both sides ignore unknown fields and default missing optional ones.

## RPCs

| Method | Caller | Input | Output |
| --- | --- | --- | --- |
| `farol.next` | board | `Next` | `Snapshot` |
| `farol.focus` | board | `Focus` | `{ "ok": true }`, ignored |
| `farol.act` | board | `Act` | `ActResult` |
| `farol.start` | board | `Start` | `ActResult` with `agent` |
| `farol.language` | Paseo app | `{ "locale": "zh-CN" }` | `{ "ok": true }` |

Paseo times plugin RPCs out after 30 s. A plugin error reaches the board as `rpc_error`. "Plugin is not available" means the plugin is not installed or not running.

### `farol.next`, the long poll

```json
{ "v": 1, "cursor": 42, "focus": "agent-id", "device": "3a1f", "screen": "deck", "fw": "1.0.0", "prefs": { … } }
```

- `cursor` is the `rev` of the last snapshot the board received, 0 at start.
- The plugin answers at once when the board is behind (first poll, reconnect, plugin restart). It also answers as soon as that board's snapshot changes, and after 20 s with an idle snapshot. Each held poll shows in the daemon log as a slow request.
- Full snapshots to one board are at least 220 ms apart.
- Each `device` has one waiting poll. A newer poll answers the older one idle.
- `focus` is the card on screen. Only that card carries `recent` and `todo`.
- `screen` is `usage`, `home` or `deck`. When a board turns to `usage`, the plugin refreshes provider usage, at most once a minute.
- `fw` is the board's firmware version, for the plugin's list of boards.
- `prefs` (`BoardPrefs`) are the board's control-centre settings. The board resends them on every poll until one is answered. The plugin shows them with the board and changes nothing else with them.

### `farol.focus`

`{ "focus": "agent-id", "device": "3a1f", "screen": "deck", "prefs": { … } }` sets the focus (absent means none). It answers that board's waiting `farol.next` with the focused card's detail, after up to 1.5 s for the card's history.

### `farol.act`

`{ "agent": "id", "device": "farol-3a1f", "kind": "answer" | "stop" | "seen", "requestId": "…", "choice": 0, "turn": "…" }`

`device` names the board in the line the plugin adds to the agent's timeline.

- `answer` picks option `choice` of the pending `ask` whose `requestId` matches. Paseo applies one response per request. The plugin waits up to 3 s for `agent.permission_resolved` and answers `ok` only when the applied response is the board's, `already_resolved` when another client's won.
- `stop` interrupts the turn named by `turn`. The plugin runs `paseo stop <id>`, because Paseo's SDK has no cancel. That cancels whatever turn is current, so the plugin first re-reads the agent and refuses (`already_resolved`) when another turn has started. A turn that starts in the milliseconds between that check and the cancel is still stopped.
- `seen` marks a finished or failed card as read until a newer turn ends.

### `farol.start`

`{ "device": "farol-3a1f", "text": "…" }` starts an agent in the workspace with the agent profile set in the plugin's settings, `text` as its first prompt. The agent carries the label `farol.device`. The answer carries the new agent's id in `agent`.

### `ActResult`

`{ "ok": true }`, or `{ "ok": false, "reason": …, "message": … }` with `reason`:

| Reason | Meaning |
| --- | --- |
| `already_resolved` | The ask is no longer open, or the turn already ended. |
| `read_only` | Boards may not act (plugin setting). `seen` still works. |
| `not_found` | No such agent, or it is archived. |
| `no_workspace` | `farol.start` without a workspace or agent profile in the settings. |
| `failed` | Anything else, with a message. |

## Snapshot

```json
{
  "v": 1, "rev": 42, "now": 1793598060000, "tz": 480,
  "host": "studio-mac", "readOnly": false, "idle": false, "lang": "zh",
  "compose": { "workspace": "web-console" },
  "settings": { "rev": 7, "sound": true, "ledBrightness": 60, "quietStart": 1320, "quietEnd": 480 },
  "agents": [ … ], "usage": [ … ], "fuel": { "label": "Claude · 5h", "left": 35 }
}
```

| Field | Meaning |
| --- | --- |
| `v` | Always 1. The board reports any other value as version skew. |
| `rev` | Increases when this board's snapshot changes. The next `cursor`. |
| `now` | The plugin's clock, epoch ms. |
| `tz` | The time zone of the board's clock, minutes east of UTC. |
| `host` | The computer's name. |
| `readOnly` | Boards may not act. |
| `idle` | The poll timed out with no change. Only `v`, `rev` and `now` count. |
| `lang` | `zh` or `en`: the plugin setting, else the Paseo app's language. Absent while unknown, and the board keeps its own. |
| `compose` | Where `farol.start` sessions go. Absent without a workspace and profile, or when read-only. |
| `settings` | Board settings made in Paseo. A newer `rev` replaces the board's own sound, LED brightness and quiet hours. `quietStart` and `quietEnd` are minutes after local midnight. They may wrap past midnight, and equal values mean no quiet hours. |
| `agents` | Cards, most urgent first (`call`, `fail`, `done`, `work`, `seen`), then newest `since`. At most 24. Archived and internal agents are left out, and so are read cards older than an hour, except the focused one. |
| `usage` | Quota windows: `label`, `used` percent, `resets` epoch ms. |
| `fuel` | The tightest window, as percent `left`. |

### Agent

| Field | Meaning |
| --- | --- |
| `id`, `title`, `project`, `model` | Paseo agent id, title, project name, short model label. |
| `state` | `call`: a question, permission or plan waits. `fail`: the last turn failed. `done`: finished, not yet read. `work`: a turn runs. `seen`: anything else. |
| `since` | When the running turn or the attention began, epoch ms. |
| `now` | Working cards: the latest action. |
| `recent` | Focused working card: up to three earlier actions, newest first. |
| `todo` | Focused working card: `{ done, total, current }`. |
| `ctx` | Context window used, percent. |
| `cost` | Cost of the last turn, on finished cards. |
| `ask` | `call` cards: `{ kind, requestId, text, options }`, `kind` one of question, permission, plan. `choice` indexes `options`. Permissions and plans without their own actions offer allow/deny and approve/keep planning. |
| `body` | Finished, failed and read cards: the start of the last reply, when reply preview is on. |
| `error` | Failed cards: Paseo's last error. |
| `stoppable` | Working cards, when the plugin can run `paseo stop`. |
| `turn` | Working cards: the running turn's id, which a stop names. |

### BoardPrefs

`{ "sound": bool, "haptic": bool, "brightness": 5..100, "dnd": bool }`, the board's control centre.

## Connecting

The board takes what Paseo's app takes when adding a host (`rust/logic/src/endpoint.rs`).

- Direct connection: host, port (6767 by default), SSL and the daemon password. The board opens `ws[s]://<host>:<port>/ws`.
- Paseo's pairing QR (Settings › host › Pair device): `https://app.paseo.sh/#offer=<base64url JSON>` with the daemon's relay id, its X25519 public key and the relay. The board also reads `relay://` links as `paseo daemon pair` prints them, and `#connect=` links. It opens `wss://<relay>/ws?serverId=<id>&role=client&v=2` and runs Paseo's protected relay encryption (`rust/logic/src/relay.rs`). The handshake is Noise_NK_25519_ChaChaPoly_SHA256 against the daemon's key in `e2ee_hello` and `e2ee_ready`. After it, every frame is one Paseo record in base64 text. A daemon without this mode is refused.

A pairing code carries no password. When the daemon asks for one, the board asks the user. The board requests `hello.rejected` in its hello, because a relay replaces the daemon's close codes. The board also scans phones' Wi-Fi share codes (`rust/logic/src/qr.rs`).

## Dictation

For a new session by voice, the board uses Paseo's dictation on its own session, not the plugin (`rust/logic/src/dictation.rs`). `server_info.capabilities.voice.dictation.enabled` says whether it is available. The board sends `dictation_stream_start`, then 16 kHz mono 16-bit PCM in base64 `dictation_stream_chunk` messages after the daemon's first ack, then `dictation_stream_finish`. The daemon answers with `dictation_stream_partial`, `dictation_stream_final` or `dictation_stream_error`. The board then sends the text with `farol.start`.

## Fixtures

- `fixtures/snapshot.json`: a snapshot the board parses.
- `fixtures/digest-input.json`: Paseo-shaped facts. The plugin's tests digest them into exactly `snapshot.json`, so both sides agree on the same bytes.
- `fixtures/relay.json`: a relay handshake, records both ways and pairing links, made by Paseo's own code (`node tools/relay_vectors.mjs`). The board's tests require them byte for byte.
