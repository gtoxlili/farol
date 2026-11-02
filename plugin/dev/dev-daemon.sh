#!/usr/bin/env bash
# An isolated Paseo daemon for Farol development.
#
# Everything lives under $FAROL_DEV_DIR: the daemon home, a stand-in user HOME
# (so provider discovery and hook installers never see the real one), logs.
# It listens on 127.0.0.1:$FAROL_DEV_PORT only, has no relay, no MCP, every
# built-in model provider disabled, and its own password. Fake agents come from
# the scripted-agents plugin next to this script.
#
#   FAROL_DEV_DIR=/some/scratch/dir dev/dev-daemon.sh up
#   dev/dev-daemon.sh install      # installs farol + scripted-agents into it
#   dev/dev-daemon.sh cli agent ls # any paseo command, aimed at this daemon
#   dev/dev-daemon.sh down
#
# Every paseo call made here carries --home (and the process env) for the dev
# home, with the caller's environment scrubbed, so nothing reaches the
# default daemon.
set -euo pipefail

DIR="${FAROL_DEV_DIR:?set FAROL_DEV_DIR to a scratch directory for the dev daemon}"
PORT="${FAROL_DEV_PORT:-47661}"
PASSWORD="${FAROL_DEV_PASSWORD:-farol-dev}"
DEV_HOME="$DIR/paseo-home"
USER_HOME="$DIR/user-home"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_DIR="$(cd "$HERE/.." && pwd)"
SCRIPTED_DIR="$HERE/scripted-agents"
PASEO_BIN="${PASEO_BIN:-$(command -v paseo || true)}"
[ -n "$PASEO_BIN" ] || { echo "paseo CLI not found on PATH" >&2; exit 1; }

mkdir -p "$DEV_HOME" "$USER_HOME"
DEV_HOME="$(cd "$DEV_HOME" && pwd -P)"

# Refuse to run against anything that looks like a real daemon home.
guard() {
  local real
  for real in "${PASEO_HOME:-}" "$HOME/.paseo"; do
    [ -n "$real" ] || continue
    if [ -d "$real" ] && [ "$(cd "$real" && pwd -P)" = "$DEV_HOME" ]; then
      echo "refusing: $DEV_HOME is a real Paseo home" >&2
      exit 1
    fi
  done
  if [ -f "$DEV_HOME/config.json" ] && [ ! -f "$DEV_HOME/.farol-dev" ]; then
    echo "refusing: $DEV_HOME was not created by this script" >&2
    exit 1
  fi
}

# Runs a command with a scrubbed environment pointed at the dev home.
in_dev() {
  env -i PATH="$PATH" LANG="${LANG:-en_US.UTF-8}" TERM="${TERM:-dumb}" \
    HOME="$USER_HOME" PASEO_HOME="$DEV_HOME" NPM_CONFIG_CACHE="$DIR/npm-cache" \
    "$@"
}

# paseo <command...> against the dev daemon.
dev_paseo() {
  in_dev "$PASEO_BIN" "$@" --home "$DEV_HOME"
}

write_config() {
  touch "$DEV_HOME/.farol-dev"
  in_dev node --input-type=module -e '
    import { createRequire } from "node:module";
    import { writeFileSync, existsSync, readFileSync } from "node:fs";
    const require = createRequire(process.argv[1] + "/");
    const bcrypt = require("bcryptjs");
    const [, , home, port, password] = process.argv;
    const file = home + "/config.json";
    const prior = existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : {};
    const off = { enabled: false };
    const config = {
      ...prior,
      version: 1,
      daemon: {
        ...(prior.daemon ?? {}),
        listen: "127.0.0.1:" + port,
        relay: { enabled: false },
        mcp: { enabled: false, injectIntoAgents: false },
        enableTerminalAgentHooks: false,
        auth: { password: bcrypt.hashSync(password, 10) },
      },
      pluginsEnabled: true,
      agents: {
        ...(prior.agents ?? {}),
        providers: { claude: off, codex: off, copilot: off, opencode: off, pi: off, omp: off },
      },
      features: { dictation: off, voiceMode: off },
    };
    writeFileSync(file, JSON.stringify(config, null, 2) + "\n");
  ' "$PLUGIN_DIR" "$DEV_HOME" "$PORT" "$PASSWORD"
}

case "${1:-}" in
  up)
    guard
    if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
      echo "port $PORT is already in use" >&2
      exit 1
    fi
    write_config
    dev_paseo daemon start --timeout 120
    dev_paseo daemon status
    echo "ws://127.0.0.1:$PORT/ws  password: $PASSWORD"
    ;;
  down)
    guard
    dev_paseo daemon stop || true
    ;;
  status)
    guard
    dev_paseo daemon status
    ;;
  install)
    guard
    dev_paseo plugin install "$SCRIPTED_DIR" --id scripted-agents
    dev_paseo plugin install "$PLUGIN_DIR" --id farol
    dev_paseo plugin ls
    ;;
  reload)
    guard
    dev_paseo plugin reload "${2:-farol}"
    ;;
  logs)
    guard
    dev_paseo plugin logs "${2:-farol}"
    ;;
  cli)
    guard
    shift
    dev_paseo "$@"
    ;;
  env)
    # For scripts: the URL and password a board would use.
    echo "FAROL_URL=ws://127.0.0.1:$PORT/ws"
    echo "FAROL_PASSWORD=$PASSWORD"
    echo "FAROL_DEV_HOME=$DEV_HOME"
    ;;
  *)
    echo "usage: FAROL_DEV_DIR=<dir> $0 up|down|status|install|reload [id]|logs [id]|cli <paseo args>|env" >&2
    exit 2
    ;;
esac
