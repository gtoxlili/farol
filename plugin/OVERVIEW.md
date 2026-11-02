Farol is the companion plugin for the Farol desk lamp, an ESP32 board that shows
your Paseo agents at a glance: cool light while agents work, warm light when one
waits for you. From the board you can answer a question, a permission or a plan,
stop a running turn, and mark finished agents as read.

## How it works

The board connects to this daemon's WebSocket like the web app does, with the
daemon password, and then only calls this plugin. The plugin reads agents,
timelines and provider usage through Paseo's public SDK, sends the board a
compact summary, and performs the board's actions after checking that the
request is still open. It opens no port and stores no password.

## Setup

The board needs an address it can reach: an HTTPS or WSS address that forwards
WebSockets to this daemon (your reverse proxy, a tunnel), listed in the daemon's
`hostnames`. Enter it under Settings › Plugins › Farol, then use "添加设备"
(also in the Command Center as "Farol：添加设备"): type the daemon password once
and scan the code with the board. Requires Paseo 0.14 or newer.

## Settings

- Allow the board to act on agents. Off makes every board read-only.
- Reply preview on finished cards.
- Include agents in background workspaces.
- Sound, LED brightness and quiet hours, applied by every board when you change
  them here. A board's own control centre changes only that board (shown under
  recent boards).
- Time zone for the board's clock; empty follows this Mac.

## Things to know

- The board holds the daemon password, which grants full access to this daemon.
  If a board is lost, change the daemon password.
- Stopping a turn runs the `paseo stop` command on this Mac (the same cancel as
  the app's stop button). Only the turn the board showed is stopped: if a new
  turn started meanwhile, for example from your phone, the board's stop is
  refused. Without the CLI on the daemon's PATH the board offers no stop.
- An answer from a board counts only when Paseo applied it; if you answered the
  same question elsewhere first, the board says so.
- Each answer or stop made on a board adds a line to that agent's timeline
  (display only: it never reaches the model).
- Boards wait for news with a 20-second request, which the daemon log lists as a
  slow request; that is expected.
