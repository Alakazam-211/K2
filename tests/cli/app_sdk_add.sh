#!/usr/bin/env bash
# k2 app sdk add (0.46.6), end to end: a sandbox daemon (temp HOME,
# K2_APPS=1, air-gapped) and this checkout's cli/k2 as the owner. A tiny
# App that does `import { fromEnv } from '@k2/server'` resolves after
# `npm install --offline` against the vendored tarball, with an empty npm
# cache (so nothing came from the registry). Also: the rerun is
# idempotent, `status` says current, `--sew` on a build whose Sew pin has no
# App kit is refused clearly, and nothing is written on a refusal.
#
# Needs node 22+ and npm (fails loudly without them).
# Build first:  cargo build -p k2-daemon   (or APP_SDK_BUILD=1)
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
if [ "${APP_SDK_BUILD:-0}" = 1 ]; then (cd "$PROJECT_ROOT" && cargo build -p k2-daemon); fi
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v node >/dev/null || { echo "FAIL: node 22+ is required" >&2; exit 1; }
command -v npm >/dev/null || { echo "FAIL: npm is required" >&2; exit 1; }
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]')"
[ "$NODE_MAJOR" -ge 22 ] || { echo "FAIL: node 22+ is required (have $(node --version))" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2sdk-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    chmod -R u+w "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME" "$SANDBOX/tmp" "$SANDBOX/run"
chmod 700 "$SANDBOX/run"
echo "== sandbox daemon =="
HOME="$DHOME" TMPDIR="$SANDBOX/tmp" XDG_RUNTIME_DIR="$SANDBOX/run" K2_RUNTIME_DIR= K2_APPS=1 K2_AIRGAP=1 K2_DISK_GUARD=off \
    "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
k2() { HOME="$DHOME" "$K2" "$@"; }
run() { # out-file cmd… → rc (never exits the script)
    local out="$1"; shift
    set +e
    k2 "$@" > "$out" 2>&1
    local rc=$?
    set -e
    echo "$rc"
}

APP="$SANDBOX/ws/app"
mkdir -p "$APP"
cat > "$APP/server.mjs" <<'JS'
import { fromEnv, K2App, APP_VERBS } from '@k2/server'
const k2 = fromEnv({ K2_GATEWAY_URL: 'http://127.0.0.1:9', K2_APP_ID: 'demo' })
console.log(typeof fromEnv, k2 instanceof K2App, Object.keys(APP_VERBS).length > 0)
JS

echo "== add (no package.json yet) =="
rc="$(run "$SANDBOX/add1.txt" app sdk add "$APP")"
out="$(cat "$SANDBOX/add1.txt")"
expect_eq "add exits 0" "$rc" "0"
expect_has "says what it vendored" "$out" "@k2/server"
expect_has "says what to run next" "$out" "npm install"
VER="$(k2 app sdk status "$APP" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["box"]["k2Server"])')"
[ -f "$APP/vendor/k2-server-$VER.tgz" ] && ok "vendor/k2-server-$VER.tgz" || bad "vendor/k2-server-$VER.tgz is missing"
dep="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["dependencies"]["@k2/server"])' "$APP/package.json")"
expect_eq "package.json points at it" "$dep" "file:vendor/k2-server-$VER.tgz"
pkgver="$(tar -xzOf "$APP/vendor/k2-server-$VER.tgz" package/package.json | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["name"], v["version"], "scripts" in v)')"
expect_eq "the tarball is @k2/server at K2's version, no scripts" "$pkgver" "@k2/server $VER False"

echo "== npm install --offline, empty cache =="
mkdir -p "$SANDBOX/npm-home"
set +e
(cd "$APP" && HOME="$SANDBOX/npm-home" npm_config_cache="$SANDBOX/npm-home/cache" npm install --offline --no-audit --no-fund) > "$SANDBOX/npm.txt" 2>&1
rc=$?
set -e
expect_eq "npm install --offline exits 0" "$rc" "0"
[ "$rc" = 0 ] || cat "$SANDBOX/npm.txt" >&2
set +e
got="$(cd "$APP" && node server.mjs 2>&1)"
set -e
expect_eq "import { fromEnv } from '@k2/server' resolves and runs" "$got" "function true true"

echo "== rerun is idempotent =="
cp "$APP/package.json" "$SANDBOX/pj.before"
out="$(k2 app sdk add "$APP" --json)"
expect_eq "package.json unchanged" "$(printf '%s' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["packageJson"], v["vendored"][0]["written"], v["removed"])')" "unchanged False []"
cmp -s "$APP/package.json" "$SANDBOX/pj.before" && ok "package.json byte for byte" || bad "package.json changed on a rerun"
expect_eq "one tarball in vendor/" "$(ls "$APP/vendor" | tr '\n' ' ')" "k2-server-$VER.tgz "

echo "== an older vendored SDK is replaced =="
cp "$APP/vendor/k2-server-$VER.tgz" "$APP/vendor/k2-server-0.40.1.tgz"
out="$(k2 app sdk add "$APP" --json)"
expect_has "removed the older one" "$out" "vendor/k2-server-0.40.1.tgz"
expect_eq "still one tarball" "$(ls "$APP/vendor" | tr '\n' ' ')" "k2-server-$VER.tgz "

echo "== status =="
out="$(k2 app sdk status "$APP")"
expect_has "current" "$out" "current"

echo "== --sew without a kit is refused, nothing written =="
APP2="$SANDBOX/ws/app2"
mkdir -p "$APP2"
rc="$(run "$SANDBOX/sew.txt" app sdk add "$APP2" --sew)"
out="$(cat "$SANDBOX/sew.txt")"
expect_eq "--sew exits 3" "$rc" "3"
expect_has "  says why" "$out" "sew_kit_"
expect_has "  names Sew 0.5.0+" "$out" "0.5.0"
[ ! -e "$APP2/vendor" ] && [ ! -e "$APP2/package.json" ] && ok "  nothing written" || bad "  wrote into $APP2"

echo "== usage =="
rc="$(run "$SANDBOX/u.txt" app sdk add "$SANDBOX/nope")"
expect_eq "a missing folder exits 2" "$rc" "2"
rc="$(run "$SANDBOX/u2.txt" app sdk add)"
expect_eq "no folder exits 2" "$rc" "2"
out="$(k2 app sdk --help)"
expect_has "help names add" "$out" "k2 app sdk add <app-folder> [--sew]"

echo
echo "app_sdk_add: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
