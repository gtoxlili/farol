# Let the board reach your Paseo daemon

The board opens `wss://<host>/ws` and signs in with the daemon password, like
Paseo's web app. It needs:

- an HTTPS address with a **publicly trusted certificate** (a private CA or a
  self-signed certificate is rejected by the board);
- something at that address that forwards WebSockets to the daemon's listen
  address (`daemon.listen` in the daemon's `config.json`, `127.0.0.1:6767` by
  default);
- the host name in the daemon's `daemon.hostnames`. The daemon answers other
  names with HTTP 403, which the board reports as "daemon 不认这个域名（hostnames）".

Use the same address in the plugin's settings page (Settings › Plugins › Farol);
the code the page shows carries it to the board. Plain `ws://` works on a
trusted local network, but the daemon password then travels unencrypted.

The examples use `paseo.example.net` and the default port; replace both.

## Caddy

Caddy obtains a public certificate and proxies WebSockets without extra
settings:

```caddyfile
paseo.example.net {
	reverse_proxy 127.0.0.1:6767
}
```

## Cloudflare Tunnel

```yaml
# ~/.cloudflared/config.yml
tunnel: <tunnel id>
credentials-file: /path/to/<tunnel id>.json
ingress:
  - hostname: paseo.example.net
    service: http://127.0.0.1:6767
  - service: http_status:404
```

Route the host name to the tunnel (`cloudflared tunnel route dns <tunnel> paseo.example.net`)
and run it (`cloudflared tunnel run <tunnel>`). WebSockets pass through.

## Tailscale Funnel

```sh
tailscale funnel --bg 6767
```

The address is `https://<machine>.<tailnet>.ts.net`; add that host name to
`daemon.hostnames`. Funnel makes it reachable from the internet, so keep the
daemon password strong.

## Add the host name to the daemon

In the daemon's `config.json` (in `PASEO_HOME`), add the name to the list and
restart the daemon:

```json
{
  "daemon": {
    "hostnames": ["paseo.example.net"]
  }
}
```

Then check from another network: Paseo's apps should connect to
`https://paseo.example.net` with the daemon password (or its web app should
load there, if enabled). On the board, the Paseo page in the control centre
names the reason when it cannot connect.
