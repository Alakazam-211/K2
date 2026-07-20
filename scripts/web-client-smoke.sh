#!/usr/bin/env bash
# Local hosted-web smoke — verifies loader + SPA + daemon paths without a GUI.
#
# Requires a running K2 daemon (heartbeat port file). Builds the web SPA if
# missing. Starts a short-lived same-origin reverse proxy when caddy/python/bun
# is available; otherwise curls the daemon for /boot-status and checks static
# files on disk for loader + /app/<ver>/index.html.
#
# Usage:
#   bash scripts/web-client-smoke.sh
#   K2_WEB_SMOKE_USER=you K2_WEB_SMOKE_PASS=secret bash scripts/web-client-smoke.sh
#
# Optional env:
#   K2_DAEMON_PORT          — override heartbeat port file
#   K2_HEARTBEAT_PORT_FILE  — alternate path to heartbeat.port
#   K2_WEB_VERSION          — SPA version under out/web/app/ (default: package.json)
#   K2_WEB_SMOKE_PORT       — fixed proxy listen port (default: ephemeral)
#   K2_WEB_SMOKE_USER/PASS  — optional POST /cli/auth/login (password never printed)
#   K2_WEB_SMOKE_NO_BUILD   — if set, skip vite:build:web even when bundle missing
#
# Exit 0 on all required assertions pass; non-zero otherwise.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PASS=0
FAIL=0
PROXY_PID=""
PROXY_MODE=""
BASE_URL=""
TMPDIR_SMOKE=""
CLEANED=0

cleanup() {
  if [[ "$CLEANED" -eq 1 ]]; then
    return
  fi
  CLEANED=1
  if [[ -n "${PROXY_PID:-}" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
    kill "$PROXY_PID" 2>/dev/null || true
    wait "$PROXY_PID" 2>/dev/null || true
  fi
  if [[ -n "${TMPDIR_SMOKE:-}" && -d "$TMPDIR_SMOKE" ]]; then
    rm -rf "$TMPDIR_SMOKE"
  fi
}
trap cleanup EXIT INT TERM

pass() {
  echo "  PASS: $*"
  PASS=$((PASS + 1))
}

fail() {
  echo "  FAIL: $*" >&2
  FAIL=$((FAIL + 1))
}

# ── Resolve daemon port ──────────────────────────────────────────────────────

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
    echo "FAIL: daemon heartbeat port missing ($PORT_FILE)." >&2
    echo "Start the K2 daemon (or set K2_DAEMON_PORT=…)." >&2
    exit 1
  fi
fi

if [[ ! "$K2_DAEMON_PORT" =~ ^[0-9]+$ ]]; then
  echo "FAIL: invalid daemon port: ${K2_DAEMON_PORT}" >&2
  exit 1
fi

DAEMON_BASE="http://127.0.0.1:${K2_DAEMON_PORT}"

# Quick liveness — heartbeat file alone is not enough if daemon died.
if ! curl -fsS --max-time 3 "${DAEMON_BASE}/boot-status" >/dev/null 2>&1; then
  echo "FAIL: daemon not reachable at ${DAEMON_BASE}/boot-status" >&2
  echo "  (port from ${PORT_FILE:-K2_DAEMON_PORT})" >&2
  exit 1
fi
echo "daemon: ${DAEMON_BASE} (ok)"

# ── Resolve SPA version + ensure bundle ──────────────────────────────────────

if [[ -z "${K2_WEB_VERSION:-}" ]]; then
  if command -v node >/dev/null 2>&1; then
    K2_WEB_VERSION="$(node -p "require('./package.json').version")"
  elif command -v bun >/dev/null 2>&1; then
    K2_WEB_VERSION="$(bun -e "console.log(require('./package.json').version)")"
  else
    echo "FAIL: set K2_WEB_VERSION (or install node/bun to read package.json)." >&2
    exit 1
  fi
fi

APP_DIR="out/web/app/${K2_WEB_VERSION}"
APP_INDEX="${APP_DIR}/index.html"
LOADER_HTML="web/loader/index.html"
LOADER_JS="web/loader/loader.js"

if [[ ! -f "$LOADER_HTML" || ! -f "$LOADER_JS" ]]; then
  echo "FAIL: missing ${LOADER_HTML} or ${LOADER_JS}" >&2
  exit 1
fi

if [[ ! -f "$APP_INDEX" ]]; then
  if [[ -n "${K2_WEB_SMOKE_NO_BUILD:-}" ]]; then
    echo "FAIL: missing ${APP_INDEX} and K2_WEB_SMOKE_NO_BUILD is set." >&2
    exit 1
  fi
  if ! command -v bun >/dev/null 2>&1; then
    echo "FAIL: missing ${APP_INDEX} and bun not on PATH for vite:build:web." >&2
    exit 1
  fi
  echo "bundle missing — running bun run vite:build:web …"
  if [[ ! -x "node_modules/.bin/vite" && ! -f "node_modules/vite/bin/vite.js" ]]; then
    echo "  node_modules missing vite — running bun install …"
    bun install
  fi
  bun run vite:build:web
fi

if [[ ! -f "$APP_INDEX" ]]; then
  echo "FAIL: ${APP_INDEX} still missing after build." >&2
  exit 1
fi
echo "spa:    /app/${K2_WEB_VERSION}/ (ok)"

# ── Free listen port ─────────────────────────────────────────────────────────

pick_port() {
  if [[ -n "${K2_WEB_SMOKE_PORT:-}" ]]; then
    echo "$K2_WEB_SMOKE_PORT"
    return
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
    return
  fi
  # Last resort — likely free; proxy start will fail loudly if not.
  echo "18765"
}

# ── Start short-lived same-origin reverse proxy ──────────────────────────────

start_caddy_proxy() {
  local port="$1"
  export K2_DAEMON_PORT
  export K2_WEB_VERSION
  export K2_WEB_PORT="$port"
  caddy run --config "$ROOT/web/Caddyfile" --adapter caddyfile >/dev/null 2>&1 &
  PROXY_PID=$!
  PROXY_MODE="caddy"
}

start_python_proxy() {
  local port="$1"
  # Inline mini edge: loader @ /, SPA @ /app/*, proxy data plane → daemon.
  python3 - "$port" "$K2_DAEMON_PORT" "$ROOT" <<'PY' &
import http.client
import mimetypes
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

listen_port = int(sys.argv[1])
daemon_port = int(sys.argv[2])
root = Path(sys.argv[3])
loader_root = root / "web" / "loader"
web_root = root / "out" / "web"

PROXY_PREFIXES = ("/boot-status", "/cli/", "/events")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        return

    def _is_proxy(self, path: str) -> bool:
        if path == "/boot-status" or path.startswith("/boot-status?"):
            return True
        if path.startswith("/cli/"):
            return True
        if path == "/events" or path.startswith("/events/") or path.startswith("/events?"):
            return True
        return False

    def _proxy(self):
        parts = urlsplit(self.path)
        path = parts.path or "/"
        qs = parts.query
        target = path + (("?" + qs) if qs else "")
        body = b""
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            body = self.rfile.read(length)
        conn = http.client.HTTPConnection("127.0.0.1", daemon_port, timeout=10)
        try:
            headers = {
                k: v
                for k, v in self.headers.items()
                if k.lower() not in ("host", "connection", "content-length")
            }
            headers["Host"] = f"127.0.0.1:{daemon_port}"
            headers["Connection"] = "close"
            conn.request(self.command, target, body=body or None, headers=headers)
            resp = conn.getresponse()
            data = resp.read()
            self.send_response(resp.status)
            for k, v in resp.getheaders():
                if k.lower() in ("transfer-encoding", "connection", "content-length"):
                    continue
                self.send_header(k, v)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Connection", "close")
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(data)
        except Exception as e:
            msg = f"proxy error: {e}".encode()
            self.send_response(502)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(msg)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(msg)
        finally:
            conn.close()

    def _send_file(self, path: Path, status=200):
        if not path.is_file():
            self.send_error(404, "not found")
            return
        data = path.read_bytes()
        ctype = mimetypes.guess_type(str(path))[0] or "application/octet-stream"
        if path.suffix == ".js":
            ctype = "application/javascript"
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def _serve_static(self):
        path = unquote(urlsplit(self.path).path or "/")
        if path.startswith("/app/"):
            rel = path[len("/app/") :]
            # Prevent path escape outside out/web/app
            candidate = (web_root / "app" / rel).resolve()
            app_root = (web_root / "app").resolve()
            if not str(candidate).startswith(str(app_root) + os.sep) and candidate != app_root:
                self.send_error(403, "forbidden")
                return
            if candidate.is_dir():
                candidate = candidate / "index.html"
            if not candidate.is_file():
                self.send_error(404, "not found")
                return
            self._send_file(candidate)
            return

        # Loader tree at /
        if path == "/" or path == "":
            self._send_file(loader_root / "index.html")
            return
        rel = path.lstrip("/")
        candidate = (loader_root / rel).resolve()
        if not str(candidate).startswith(str(loader_root.resolve()) + os.sep) and candidate != loader_root.resolve():
            self.send_error(403, "forbidden")
            return
        if candidate.is_file():
            self._send_file(candidate)
            return
        # SPA-style fallback for unknown non-API paths → loader index
        self._send_file(loader_root / "index.html")

    def do_GET(self):
        path = urlsplit(self.path).path or "/"
        if self._is_proxy(path):
            self._proxy()
            return
        self._serve_static()

    def do_HEAD(self):
        self.do_GET()

    def do_POST(self):
        path = urlsplit(self.path).path or "/"
        if self._is_proxy(path):
            self._proxy()
            return
        self.send_error(405, "method not allowed")


httpd = ThreadingHTTPServer(("127.0.0.1", listen_port), Handler)
httpd.serve_forever()
PY
  PROXY_PID=$!
  PROXY_MODE="python"
}

start_bun_proxy() {
  local port="$1"
  # Minimal Bun edge: static loader + /app/* + proxy data plane.
  bun -e '
const daemonPort = process.env.K2_DAEMON_PORT;
const listenPort = Number(process.env.K2_WEB_SMOKE_LISTEN);
const root = process.env.K2_SMOKE_ROOT;
const loaderRoot = root + "/web/loader";
const webRoot = root + "/out/web";

function isProxy(path) {
  return path === "/boot-status" || path.startsWith("/boot-status?")
    || path.startsWith("/cli/")
    || path === "/events" || path.startsWith("/events/") || path.startsWith("/events?");
}

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: listenPort,
  async fetch(req) {
    const url = new URL(req.url);
    const path = url.pathname;
    if (isProxy(path)) {
      const target = `http://127.0.0.1:${daemonPort}${path}${url.search}`;
      const headers = new Headers(req.headers);
      headers.delete("host");
      const init = { method: req.method, headers };
      if (req.method !== "GET" && req.method !== "HEAD") {
        init.body = await req.arrayBuffer();
      }
      return fetch(target, init);
    }
    if (path.startsWith("/app/")) {
      const filePath = webRoot + path;
      const f = Bun.file(filePath);
      if (await f.exists()) return new Response(f);
      return new Response("not found", { status: 404 });
    }
    let rel = path === "/" ? "/index.html" : path;
    const f = Bun.file(loaderRoot + rel);
    if (await f.exists()) return new Response(f);
    return new Response(Bun.file(loaderRoot + "/index.html"));
  },
});
// keep alive
await new Promise(() => {});
' &
  PROXY_PID=$!
  PROXY_MODE="bun"
}

wait_for_proxy() {
  local url="$1"
  local i
  for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if curl -fsS --max-time 1 "${url}/" >/dev/null 2>&1 \
      || curl -fsS --max-time 1 -o /dev/null -w '' "${url}/boot-status" 2>/dev/null; then
      # Prefer readiness on either loader or boot-status.
      if curl -fsS --max-time 2 "${url}/boot-status" >/dev/null 2>&1; then
        return 0
      fi
    fi
    if [[ -n "${PROXY_PID:-}" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
      return 1
    fi
    sleep 0.15
  done
  return 1
}

SMOKE_PORT="$(pick_port)"
export K2_DAEMON_PORT
export K2_WEB_VERSION
export K2_SMOKE_ROOT="$ROOT"
export K2_WEB_SMOKE_LISTEN="$SMOKE_PORT"

if command -v caddy >/dev/null 2>&1; then
  start_caddy_proxy "$SMOKE_PORT"
elif command -v python3 >/dev/null 2>&1; then
  start_python_proxy "$SMOKE_PORT"
elif command -v bun >/dev/null 2>&1; then
  start_bun_proxy "$SMOKE_PORT"
else
  PROXY_MODE="direct"
fi

if [[ "$PROXY_MODE" != "direct" ]]; then
  BASE_URL="http://127.0.0.1:${SMOKE_PORT}"
  if wait_for_proxy "$BASE_URL"; then
    echo "proxy:  ${BASE_URL} (${PROXY_MODE})"
  else
    echo "WARN: proxy (${PROXY_MODE}) failed to become ready — falling back to direct checks." >&2
    if [[ -n "${PROXY_PID:-}" ]]; then
      kill "$PROXY_PID" 2>/dev/null || true
      wait "$PROXY_PID" 2>/dev/null || true
      PROXY_PID=""
    fi
    PROXY_MODE="direct"
    BASE_URL=""
  fi
fi

if [[ "$PROXY_MODE" = "direct" ]]; then
  echo "proxy:  none (direct daemon + on-disk static checks)"
fi

# ── Helpers for HTTP assertions ──────────────────────────────────────────────
# curl into files so status is never trapped in a command-substitution subshell
# (set -u would trip on HTTP_CODE otherwise).

HTTP_CODE="000"
HTTP_BODY_FILE=""

http_fetch() {
  # usage: http_fetch URL [method]
  # sets HTTP_CODE and HTTP_BODY_FILE (caller must not delete mid-assert)
  local url="$1"
  local method="${2:-GET}"
  HTTP_BODY_FILE="$(mktemp)"
  if [[ "$method" = "HEAD" ]]; then
    HTTP_CODE="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 8 -I "$url" 2>/dev/null \
      || curl -sS -o /dev/null -w '%{http_code}' --max-time 8 -X HEAD "$url" 2>/dev/null \
      || echo "000")"
    : >"$HTTP_BODY_FILE"
  else
    HTTP_CODE="$(curl -sS -o "$HTTP_BODY_FILE" -w '%{http_code}' --max-time 8 "$url" 2>/dev/null || echo "000")"
  fi
}

echo ""
echo "== hosted web smoke =="

# ── 1. GET /boot-status ──────────────────────────────────────────────────────

if [[ -n "$BASE_URL" ]]; then
  BOOT_URL="${BASE_URL}/boot-status"
else
  BOOT_URL="${DAEMON_BASE}/boot-status"
fi

http_fetch "$BOOT_URL"
if [[ "$HTTP_CODE" = "200" ]]; then
  pass "GET /boot-status → 200"
else
  fail "GET /boot-status → ${HTTP_CODE} (want 200)"
fi

BOOT_BODY="$(cat "$HTTP_BODY_FILE" 2>/dev/null || true)"
rm -f "$HTTP_BODY_FILE"

# version required
if printf '%s' "$BOOT_BODY" | grep -Eq '"version"[[:space:]]*:[[:space:]]*"[^"]+"'; then
  BOOT_VERSION="$(printf '%s' "$BOOT_BODY" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  pass "boot-status has version (${BOOT_VERSION})"
else
  fail "boot-status JSON missing version"
  BOOT_VERSION=""
fi

# webClient.enabled if present
if printf '%s' "$BOOT_BODY" | grep -Eq '"webClient"'; then
  if printf '%s' "$BOOT_BODY" | grep -Eq '"enabled"[[:space:]]*:[[:space:]]*(true|false)'; then
    pass "boot-status webClient.enabled present"
  else
    fail "boot-status has webClient but no enabled field"
  fi
else
  pass "boot-status webClient absent (older daemon; skipped)"
fi

# ── 2. GET / (loader) ────────────────────────────────────────────────────────

if [[ -n "$BASE_URL" ]]; then
  http_fetch "${BASE_URL}/"
  if [[ "$HTTP_CODE" = "200" ]]; then
    pass "GET / → 200"
  else
    fail "GET / → ${HTTP_CODE} (want 200)"
  fi
  LOADER_BODY="$(cat "$HTTP_BODY_FILE" 2>/dev/null || true)"
  rm -f "$HTTP_BODY_FILE"
  if printf '%s' "$LOADER_BODY" | grep -Eq 'loader\.js|/boot-status|boot-status'; then
    pass "loader HTML references loader.js or boot-status"
  else
    fail "loader HTML missing loader.js / boot-status fetch logic"
  fi
else
  # On-disk fallback when no proxy
  LOADER_BODY="$(cat "$LOADER_HTML")"
  if [[ -f "$LOADER_HTML" ]]; then
    pass "loader index.html present on disk (no proxy)"
  else
    fail "loader index.html missing"
  fi
  if printf '%s' "$LOADER_BODY" | grep -Eq 'loader\.js|/boot-status|boot-status'; then
    pass "loader HTML references loader.js or boot-status"
  else
    fail "loader HTML missing loader.js / boot-status fetch logic"
  fi
  if grep -Eq 'boot-status|webClientVersion|/app/' "$LOADER_JS"; then
    pass "loader.js contains boot-status fetch logic"
  else
    fail "loader.js missing boot-status fetch logic"
  fi
fi

# ── 3. HEAD or GET /app/<ver>/index.html ─────────────────────────────────────

APP_PATH="/app/${K2_WEB_VERSION}/index.html"
if [[ -n "$BASE_URL" ]]; then
  http_fetch "${BASE_URL}${APP_PATH}" HEAD
  HEAD_CODE="$HTTP_CODE"
  rm -f "$HTTP_BODY_FILE"
  if [[ "$HEAD_CODE" = "200" ]]; then
    pass "HEAD ${APP_PATH} → 200"
  else
    http_fetch "${BASE_URL}${APP_PATH}"
    rm -f "$HTTP_BODY_FILE"
    if [[ "$HTTP_CODE" = "200" ]]; then
      pass "GET ${APP_PATH} → 200 (HEAD was ${HEAD_CODE})"
    else
      fail "${APP_PATH} → HEAD ${HEAD_CODE} / GET ${HTTP_CODE} (want 200)"
    fi
  fi
else
  if [[ -f "$APP_INDEX" ]]; then
    pass "${APP_PATH} present on disk (no proxy)"
  else
    fail "${APP_PATH} missing on disk"
  fi
fi

# ── 4. Optional login ────────────────────────────────────────────────────────

if [[ -n "${K2_WEB_SMOKE_USER:-}" ]]; then
  if [[ -z "${K2_WEB_SMOKE_PASS:-}" ]]; then
    fail "K2_WEB_SMOKE_USER set but K2_WEB_SMOKE_PASS empty"
  else
    if [[ -n "$BASE_URL" ]]; then
      LOGIN_URL="${BASE_URL}/cli/auth/login"
    else
      LOGIN_URL="${DAEMON_BASE}/cli/auth/login"
    fi
    # Never print password. Build JSON without echoing secrets into process list when possible.
    LOGIN_OUT="$(mktemp)"
    LOGIN_CODE="$(
      curl -sS -o "$LOGIN_OUT" -w '%{http_code}' --max-time 15 \
        -X POST "$LOGIN_URL" \
        -H 'Content-Type: application/json' \
        -H 'X-K2-Client: web' \
        --data-binary @- <<EOF || echo "000"
{"username":$(printf '%s' "$K2_WEB_SMOKE_USER" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'),"password":$(printf '%s' "$K2_WEB_SMOKE_PASS" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'),"web":true}
EOF
    )"
    if [[ "$LOGIN_CODE" = "200" ]]; then
      if grep -Eq '"token"[[:space:]]*:' "$LOGIN_OUT"; then
        pass "POST /cli/auth/login → 200 with token (user=${K2_WEB_SMOKE_USER})"
      else
        fail "POST /cli/auth/login → 200 but no token field"
      fi
    else
      fail "POST /cli/auth/login → ${LOGIN_CODE} (user=${K2_WEB_SMOKE_USER}; password not shown)"
    fi
    rm -f "$LOGIN_OUT"
  fi
else
  echo "  SKIP: login (set K2_WEB_SMOKE_USER / K2_WEB_SMOKE_PASS to exercise)"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "results: ${PASS} passed, ${FAIL} failed (proxy=${PROXY_MODE})"
if [[ "$FAIL" -gt 0 ]]; then
  echo "SMOKE FAIL"
  exit 1
fi
echo "SMOKE PASS"
exit 0
