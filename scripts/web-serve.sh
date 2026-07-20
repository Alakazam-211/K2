#!/usr/bin/env bash
# Start the local Caddy edge for the hosted web client.
# Proxies /boot-status, /cli/*, /events → daemon; serves out/web SPA.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if ! command -v caddy >/dev/null 2>&1; then
  cat >&2 <<'EOF'
caddy not found on PATH.

Install (macOS):  brew install caddy
Then re-run:      bun run web:serve

Without Caddy you can still open the static build, but the browser will
hit a different origin for the daemon and CORS will block /cli + /events.
Prefer Caddy for same-origin local smoke.
EOF
  exit 1
fi

# Prefer post-0.40 ~/.k2; fall back to ~/.k2so (compat symlink / old installs).
if [[ -n "${K2_HEARTBEAT_PORT_FILE:-}" ]]; then
  PORT_FILE="$K2_HEARTBEAT_PORT_FILE"
elif [[ -f "$HOME/.k2/heartbeat.port" ]]; then
  PORT_FILE="$HOME/.k2/heartbeat.port"
else
  PORT_FILE="$HOME/.k2so/heartbeat.port"
fi
if [[ -z "${K2_DAEMON_PORT:-}" ]]; then
  if [[ -f "$PORT_FILE" ]]; then
    K2_DAEMON_PORT="$(tr -d '[:space:]' <"$PORT_FILE")"
  else
    echo "K2_DAEMON_PORT unset and $PORT_FILE missing." >&2
    echo "Start the K2 daemon (or set K2_DAEMON_PORT=…)." >&2
    exit 1
  fi
fi

if [[ -z "${K2_WEB_VERSION:-}" ]]; then
  if command -v node >/dev/null 2>&1; then
    K2_WEB_VERSION="$(node -p "require('./package.json').version")"
  elif command -v bun >/dev/null 2>&1; then
    K2_WEB_VERSION="$(bun -e "console.log(require('./package.json').version)")"
  else
    echo "Set K2_WEB_VERSION (or install node/bun to read package.json)." >&2
    exit 1
  fi
fi

export K2_DAEMON_PORT
export K2_WEB_VERSION
export K2_WEB_PORT="${K2_WEB_PORT:-8080}"

APP_DIR="out/web/app/${K2_WEB_VERSION}"
if [[ ! -d "$APP_DIR" ]]; then
  echo "Missing $APP_DIR — run: bun run vite:build:web" >&2
  exit 1
fi

echo "web-serve: daemon=127.0.0.1:${K2_DAEMON_PORT}  app=/app/${K2_WEB_VERSION}/  listen=:${K2_WEB_PORT}"
echo "open http://127.0.0.1:${K2_WEB_PORT}/"

exec caddy run --config "$ROOT/web/Caddyfile" --adapter caddyfile
