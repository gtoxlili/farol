# app_link

One WebSocket connection to Paseo over `espressif/esp_websocket_client`
(version pinned in `main/idf_component.yml`). API: `include/app_link.h`. The
session in `rust/logic/src/paseo.rs` decides when to open, close and reconnect;
this component only carries frames and reports how a connection ended.

## Contract

- `app_link_open(url)` takes a full `ws://` or `wss://` URL including the path.
  An open while a connection exists replaces it: the old one is closed cleanly
  (1 s for the closing handshake) and reports nothing more.
- Each open reports `on_open` then exactly one `on_close`, or `on_close` alone
  when it never opened. `app_link_close()` is silent: no `on_close`.
- Text frames only. `app_link_send` sends one text message (fragmented on the
  wire above 1 KB); without an open connection it is dropped with a log line.
  Received fragmented or multi-piece messages are reassembled in PSRAM (up to
  256 KB) and delivered once, complete; binary messages are ignored.
- The handshake sends no `Origin` header and no subprotocol. The client answers
  server pings and sends its own every 30 s, but never disconnects for a
  missing pong: liveness is the session's application-level ping. The client's
  automatic reconnect is off.
- TLS verifies the server against the ESP x509 certificate bundle (public CAs).
  Self-signed or private-CA endpoints are out of scope and fail with `Tls`.

| `on_close` | Detected from (`esp_websocket_event_data_t` of the DISCONNECTED or CLOSED event) |
|---|---|
| `Refused(status)` | upgrade answered with an HTTP status other than 101 (`esp_ws_handshake_status_code`), e.g. 403 when the host is not in the daemon's `hostnames` |
| `Refused(0)` | TCP refused or reset (`errno` `ECONNREFUSED`/`ECONNRESET`) |
| `Dns` | `ESP_ERR_ESP_TLS_CANNOT_RESOLVE_HOSTNAME` |
| `Timeout` | `ESP_ERR_ESP_TLS_CONNECTION_TIMEOUT`, TLS handshake timeout, connect failure with `ETIMEDOUT`/`EAGAIN`/unreachable (10 s network timeout) |
| `Tls` | an mbedTLS error, a non-zero TLS stack error or certificate verify flags |
| `Closed(code)` | after opening: the server's close code (1005 without one), or 1006 when the connection dropped; also 1006 for an invalid URL or a client that could not start |

## Ownership and threads

`app_link_start` creates the `app_link` task (4 KB internal stack, priority 3),
its command queue and a mutex; it owns the client handle and performs every
open, send and close in call order. The public calls copy their arguments into
the queue and return at once. Each connection runs in the client's own task
(`app_link_ws`, 6 KB internal stack, created per connection and destroyed when
it ends); callbacks run there, or on `app_link` when a send fails, and must not
block or call app_link. One consumer registers with `app_link_subscribe`
before the first open. Connections carry a generation number, so events from a
replaced or closed client never reach the consumer.

## Logs

Tag `app_link`: `#<n> opening <url>`, `#<n> open`, `#<n> down: <Reason>(<code>)`
with the raw HTTP status, esp-tls error, TLS stack error, verify flags and errno
of a failed connect, dropped sends, and the client task's unused stack when it
ends. Frame contents are never logged.
