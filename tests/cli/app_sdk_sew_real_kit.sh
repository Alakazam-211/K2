#!/usr/bin/env bash
# `k2 app sdk kit` + `k2 app sdk add --sew` against Sew's REAL App kit,
# headless: a sandbox k2-daemon (temp HOME, air-gapped) built with the
# release pins (a plain `cargo build -p k2-daemon`, never test-signer) and
# this checkout's cli/k2. Then `npm install` in the App folder proves the
# vendored packages and the printed install line resolve.
#
#   bash tests/cli/app_sdk_sew_real_kit.sh <sew-app-kit-<v>.tar.gz> [k2-daemon]
#
# The kit must be the pinned release's (k2_root_proto::sew::SEW_APP_KITS);
# K2 refuses any other bytes. Needs node + npm on PATH, and the npm
# registry for the kit's external dependencies (react, livekit-client, …).
# Never touches the real ~/.k2, the production daemon or the keychain.
set -euo pipefail

KIT="${1:?usage: $0 <sew-app-kit-<v>.tar.gz> [k2-daemon]}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
DAEMON="${2:-${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}/debug/k2-daemon}"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
[ -f "$KIT" ] || { echo "FAIL: no kit at $KIT" >&2; exit 1; }
command -v npm >/dev/null || { echo "FAIL: npm is required" >&2; exit 1; }
KIT_V="$(basename "$KIT" | sed -n 's/^sew-app-kit-\([0-9.]*\)\.tar\.gz$/\1/p')"
[ -n "$KIT_V" ] || { echo "FAIL: $KIT is not named sew-app-kit-<version>.tar.gz" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2kit-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME/tmp" "$DHOME/run" "$DHOME/work/jobs-app"
TMPDIR="$DHOME/tmp" XDG_RUNTIME_DIR="$DHOME/run" K2_RUNTIME_DIR= HOME="$DHOME" K2_AIRGAP=1 \
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

APP="$DHOME/work/jobs-app"
cp "$KIT" "$DHOME/tmp/"
out="$(k2 app sdk kit "$DHOME/tmp/$(basename "$KIT")" --json)"
expect_eq "the box keeps the pinned kit" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("version"))')" "$KIT_V"
out="$(k2 app sdk add "$APP" --sew --json)"
printf '%s\n' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("    vendored:", [r["file"] for r in v["vendored"]]); print("    install:", v.get("install"))'
expect_eq "--sew vendors the kit's version" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("sew"))')" "$KIT_V"
expect_eq "@k2/server + the five Sew packages" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["vendored"]))')" "6"
for p in client voice channel e2ee bff; do
    [ -f "$APP/vendor/sew-$p-$KIT_V.tgz" ] && ok "vendor/sew-$p-$KIT_V.tgz" || bad "vendor/sew-$p-$KIT_V.tgz missing"
done
INSTALL="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("install") or "")')"
st="$(k2 app sdk status "$APP" --json)"
expect_eq "status: every package current" "$(printf '%s' "$st" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(all(r["state"]=="current" for r in v["packages"]))')" "True"

echo "== npm install in the App folder =="
( cd "$APP" && npm install --no-audit --no-fund --loglevel=error >"$SANDBOX/npm.log" 2>&1 ) && ok "npm install (the vendored packages)" || { bad "npm install"; tail -40 "$SANDBOX/npm.log" >&2; }
if [ -n "$INSTALL" ]; then
    # The line K2 printed for the kit's peers (e.g. npm i livekit-client@… react@^19 …).
    ( cd "$APP" && eval "$INSTALL --no-audit --no-fund --loglevel=error" >"$SANDBOX/npm-peers.log" 2>&1 ) && ok "the printed install line: $INSTALL" || { bad "the printed install line: $INSTALL"; tail -40 "$SANDBOX/npm-peers.log" >&2; }
fi
for p in client voice channel e2ee bff; do
    v="$(cd "$APP" && node -p "require('./node_modules/@sew/$p/package.json').version" 2>/dev/null || true)"
    expect_eq "@sew/$p installed at $KIT_V" "$v" "$KIT_V"
done
( cd "$APP" && npm ls --all >"$SANDBOX/npm-ls.log" 2>&1 ) && ok "npm ls: the tree is whole" || { bad "npm ls"; grep -E "ERR|missing|invalid" "$SANDBOX/npm-ls.log" | head -20 >&2; }

echo "app sdk --sew (real kit $KIT_V): $pass passed, $fail failed"
[ "$fail" = 0 ]
