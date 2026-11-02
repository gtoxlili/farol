# farol/1

The board talks to the Farol Paseo plugin with plugin RPCs over the daemon's own
WebSocket. The board connects like Paseo's web app: one WebSocket to `<address>`
(the daemon's `/ws`), a `hello` with the daemon password, then only
`plugin.rpc.invoke.request` frames with `pluginId: "farol"`. The plugin opens no
port and stores no password.

The board's reader of record is `rust/logic/src/model.rs` (serde names below);
the plugin's schemas are `plugin/shared/protocol.ts`. Unknown fields are ignored
and missing optional fields default, on both sides.

## RPCs

| Method | Input | Output |
| --- | --- | --- |
| `farol.next` | `Next` | `Snapshot` |
| `farol.focus` | `Focus` | `{ "ok": true }` (the board ignores it) |
| `farol.act` | `Act` | `ActResult` |

Paseo times plugin RPCs out after 30 s. A plugin error reaches the board as
`rpc_error`; "Plugin is not available" means the plugin is not installed or not
running.

### `farol.next`: long poll

```json
{ "v": 1, "cursor": 42, "focus": "agent-id", "device": "3a1f", "screen": "deck", "fw": "1.0.0", "prefs": { … } }
```

- `cursor` is the `rev` of the last snapshot the board received (0 at start).
- The plugin answers at once when the board is behind (its cursor is not the
  last rev sent to that device: first poll, reconnect, plugin restart), as soon
  as that board's snapshot changes, or after 20 s with an idle snapshot (Paseo
  times plugin RPCs out at 30 s; each held poll shows in the daemon log as a
  slow request, which is expected).
- Full snapshots to one board are at least 220 ms apart (at most 5 per second,
  with margin for network jitter).
- One waiting poll per `device`; a newer poll answers the older one idle.
- `focus` (optional) is the card on screen. Only that card carries `recent`
  and `todo`.
- `screen` is the screen in front: `usage`, `home` or `deck`. When a board
  turns to `usage`, the plugin refreshes provider usage at once (at most once a
  minute; otherwise every 5 minutes and after turns end).
- `fw` is the board's firmware version, shown in the plugin's list of boards.
- `prefs` (optional, `BoardPrefs`) are settings changed in the board's control
  centre; the board resends them on every poll until one is answered. They stay
  the board's own (it keeps them in flash); the plugin shows them with the
  board and does not change Paseo's settings with them.
- `screen` and `fw` are absent from boards older than firmware 1.0.0; the plugin
  accepts that.

### `farol.focus`

`{ "focus": "agent-id", "device": "3a1f", "screen": "deck", "prefs": { … } }`.
Sets the focus (absent means none) and answers that board's waiting
`farol.next` with the focused card's detail. The plugin waits up to 1.5 s for
the card's history first. Carries `screen` (as in `farol.next`) and may carry
`prefs`.

### `farol.act`

`{ "agent": "id", "device": "farol-3a1f", "kind": "answer" | "stop" | "seen", "requestId": "…", "choice": 0 }`

`device` is the acting board as Paseo knows it (its client id); the plugin
names it in the row it adds to the agent's timeline ("在 farol-3a1f 上回答：…").
Older boards omit it and the row says "Farol".

- `answer`: option `choice` of the pending `ask` whose `requestId` matches. At
  most once per request.
- `stop`: interrupts the running turn (the plugin runs `paseo stop <id>`, as
  Paseo's public SDK has no cancel).
- `seen`: the user read a finished or failed card. The plugin keeps that
  (agent, `since`) pair, and the card stays `seen` until a newer turn ends.

`ActResult`: `{ "ok": true }` or `{ "ok": false, "reason": …, "message": … }`
with `reason` one of `already_resolved` (the ask is no longer open or the turn
already ended), `read_only` (actions are off in the plugin's settings; `seen` is
still accepted), `not_found` (no such agent, or archived), `failed` (with a
message).

## Snapshot

```json
{
  "v": 1, "rev": 42, "now": 1793598060000, "tz": 480,
  "host": "studio-mac", "readOnly": false, "idle": false,
  "settings": { "rev": 7, "sound": true, "ledBrightness": 60, "quietStart": 1320, "quietEnd": 480 },
  "agents": [ … ], "usage": [ … ], "fuel": { "label": "Claude · 5h", "left": 35 }
}
```

| Field | Meaning |
| --- | --- |
| `v` | Always 1, on every answer (idle too). The board reports any other value as plugin/firmware version skew. |
| `rev` | Increases whenever this board's snapshot changed; the next `cursor`. |
| `now` | The plugin's clock, epoch ms. |
| `tz` | Display time zone, minutes east of UTC (plugin setting, else the Mac's zone). |
| `host` | The Mac's name. |
| `readOnly` | Board actions are off in the plugin's settings. |
| `idle` | The poll timed out with no change; only `v`, `rev`, `now` matter, and `agents`/`usage` are empty. |
| `settings` | Board settings made in Paseo. `rev` increases on every change; boards apply a newer one. `quietStart`/`quietEnd` are minutes after local midnight, may wrap past midnight, equal means no quiet hours. |
| `agents` | Cards, most urgent first: `call`, `fail`, `done`, `work`, `seen`; then newest `since`. At most 24. Archived and internal agents are left out, and so are read cards older than an hour, except the focused one. |
| `usage` | Quota windows: `label` ("Claude · 5h"), `used` percent, `resets` epoch ms. |
| `fuel` | The tightest window (Paseo's summary windows when any are marked), as percent `left`. |

### Agent

| Field | Meaning |
| --- | --- |
| `id`, `title`, `project`, `model` | Paseo agent id, title, project name, short model label ("opus 5.5"). |
| `state` | `call`: a question, permission or plan waits. `fail`: the last turn failed and needs attention. `done`: finished and not yet looked at. `work`: a turn runs. `seen`: anything else. |
| `since` | Epoch ms the running turn started (`work`) or the attention began (others). |
| `now` | Working cards: the latest action ("cargo test -p runtime", "edit  src/lib.rs"). |
| `recent` | Focused working card: up to three earlier actions, newest first. |
| `todo` | Focused working card: `{ done, total, current }`. |
| `ctx` | Context window used, percent. |
| `cost` | Cost of the last turn ("$0.42"), on finished cards. |
| `ask` | `call` cards: `{ kind: question/permission/plan, requestId, text, options }`. `options` are in the order `choice` indexes. Questions show their first question's options; permissions and plans show the request's own actions when it has them, else 允许/拒绝 and 批准计划/继续规划. |
| `body` | Finished, failed and read cards: the start of the last reply (when reply preview is on). |
| `error` | Failed cards: Paseo's last error. |
| `stoppable` | Working cards, when the plugin can run `paseo stop`. |

### BoardPrefs

`{ "sound": bool, "haptic": bool, "brightness": 5..100, "dnd": bool }`: the
board's control centre, shown with the recent boards. Settings made in Paseo
reach every board through `settings` (a newer `rev` replaces the board's own
sound, LED brightness and quiet hours); a later change on a board stays on that
board.

## Connection code

The plugin's settings page shows a QR code with

```
farol://connect?u=<url>&p=<password>&n=<host>
```

each value percent-encoded; `p` and `n` are optional. `u` must be `ws://` or
`wss://` (the daemon's `/ws`). The board also scans phones' Wi-Fi share codes
(`WIFI:T:WPA;S:<ssid>;P:<password>;;`). Parser: `rust/logic/src/qr.rs`.

## Fixtures

- `fixtures/snapshot.json`: a snapshot the board parses (`model.rs` tests).
- `fixtures/digest-input.json`: Paseo-shaped facts (agent snapshots, timeline
  items, usage, settings). The plugin's tests digest them and require exactly
  `snapshot.json`, so both sides agree on the same bytes.
