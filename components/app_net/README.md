# app_net

The board's Wi-Fi: a native 2.4 GHz station that joins saved networks and
reports what it is doing. API: `include/app_net.h`.

## Saved networks

- The board's own list: up to `APP_NET_SAVED_MAX` (8) networks, most recently
  used first, in NVS (default partition, the namespace passed to
  `app_net_start`, key `wifi`; key `wifi_hidden` names a hidden shared network).
  Passwords are stored as entered; NVS is not encrypted on this board.
- Vibe Mode's shared network (`mosaico_shared_get("wifi_ssid" / "wifi_password")`),
  read-only and read again at every selection. Forgetting it hides it from this
  board until a Join of the same name; Vibe Mode's copy is never touched.
- `app_net_join` saves first, then connects; a wrong password stays saved until
  a new Join replaces it or Forget removes it. A Join with an empty password
  uses the saved one when there is one (the shared network stays read-only)
  and otherwise saves the network as open; it never erases a saved password.

## Behaviour

| Situation | What app_net does | Reported state |
|---|---|---|
| Boot, nothing saved | waits for a Join | `NoSaved` |
| Boot or Retry | scans, then joins saved networks in range, strongest first, two tries each | `Connecting` |
| No saved network in range, or all failed | rescans every 30 s | `NotFound` |
| A network refused the password (reasons 14, 15, 202, 204) | skips it in background rescans until Join, Retry or Forget | `WrongPassword` |
| Associated, no DHCP lease after 15 s, or the lease lost | stays associated; DHCP keeps trying | `NoAddress` |
| IPv4 address | refreshes RSSI every 10 s | `Online` |
| An online link drops | scans and joins the best saved network again | `Connecting` |

`app_net_scan` answers through the scan callback: named networks only, one
entry per SSID at its strongest BSSID, strongest first, `saved` flagged. A scan
asked for during a join runs after it. Hidden networks can be joined with an
explicit Join, but background selection only finds networks that broadcast
their name.

## Ownership and threads

`app_net_start` creates the station netif, the Wi-Fi driver (`WIFI_STORAGE_RAM`:
the driver keeps nothing in flash), the event handlers and the `app_net` task
(5 KB internal stack, priority 3). That task owns all state; the public calls
and the event handler copy a message into its queue and return at once. Status
and scan callbacks run on that task and must not block. One consumer subscribes
with `app_net_subscribe` and immediately receives the current status.

On a start failure everything created is released, the state stays `Off`, and
later calls return `ESP_ERR_INVALID_STATE`. The default event loop and the
esp_netif core are shared with ESP-Iris (its TCP transport listens once the
netif exists) and are never torn down. Requires `nvs_flash_init()` first.

## Logs

Tag `app_net`: every state change (state, SSID, RSSI, address, saved count and
the task's unused stack), each scan, each join attempt with the driver's
disconnect reason, and timeouts. Passwords are never logged.
