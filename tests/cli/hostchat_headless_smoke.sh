#!/usr/bin/env bash
# k2 hostchat — headless smoke: a sandbox k2-daemon (temp HOME, air-gapped)
# and this checkout's cli/k2, on loopback. Exercises the real /cli/sew/*
# wiring (dispatcher, route policy, POST guards, handler gates) and the CLI
# over it. Sew itself runs on Linux boxes only: on a Mac the lifecycle
# routes answer `unsupported`; on Linux the box has no Sew and no root
# helper here, so they answer not_installed / helper_needed.
#
# Build first:  cargo build -p k2-daemon
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2hc-XXXXXX)"
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
mkdir -p "$DHOME/tmp" "$DHOME/run"
TMPDIR="$DHOME/tmp" XDG_RUNTIME_DIR="$DHOME/run" K2_RUNTIME_DIR= HOME="$DHOME" K2_AIRGAP=1 \
    "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
TOK="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
U="http://127.0.0.1:$PORT"
post() { curl -s -X POST -H 'Content-Type: application/json' -d "$2" "$U$1?token=$TOK"; }
k2() { HOME="$DHOME" "$K2" "$@"; }
LINUX=0
[ "$(uname -s)" = "Linux" ] && LINUX=1

echo "== routes =="
st="$(curl -s "$U/cli/sew/status?token=$TOK")"
if [ "$LINUX" = 1 ]; then
    expect_has "status: not installed" "$st" '"state":"not-installed"'
else
    expect_has "status: not supported on a Mac" "$st" '"supported":false'
fi
expect_eq "GET on a POST-only route → 405" "$(curl -s -o /dev/null -w '%{http_code}' "$U/cli/sew/media/url?token=$TOK")" "405"
expect_eq "GET on users/invite → 405" "$(curl -s -o /dev/null -w '%{http_code}' "$U/cli/sew/users/invite?token=$TOK")" "405"
expect_eq "no token → 403" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -d '{}' "$U/cli/sew/media/url")" "403"
expect_has "bad identity → 400" "$(post /cli/sew/server/enable '{"identity":"ldap"}')" '"code":"bad_request"'
expect_has "voice object shape → 400" "$(post /cli/sew/server/enable '{"voice":"yes"}')" '"code":"bad_request"'
expect_has "media probe needs ok|fail" "$(post /cli/sew/media/probe '{"udp":"maybe","tcp":"ok","turnTls":"ok"}')" '"code":"bad_request"'
expect_has "users needs a user" "$(post /cli/sew/users/invite '{}')" '"code":"bad_request"'
expect_has "media preflight is a read" "$(curl -s "$U/cli/sew/media/preflight?token=$TOK")" '"checks"'
if [ "$LINUX" = 1 ]; then
    expect_has "users without Sew → not_installed" "$(post /cli/sew/users/invite '{"user":"bob"}')" '"code":"not_installed"'
    expect_has "voice URL off this box → 400" "$(post /cli/sew/media/url '{"publicUrl":"wss://evil.example"}')" '"code":"url_not_on_this_box"'
else
    expect_has "users on a Mac → unsupported" "$(post /cli/sew/users/invite '{"user":"bob"}')" '"code":"unsupported"'
    expect_has "voice URL on a Mac → unsupported" "$(post /cli/sew/media/url '{"publicUrl":"wss://hearth.box.k2.dev"}')" '"code":"unsupported"'
    expect_has "media install on a Mac → unsupported" "$(post /cli/sew/media/install '{}')" '"code":"unsupported"'
fi

echo "== k2 hostchat =="
out="$(k2 hostchat status 2>&1)"
expect_has "hostchat status prints" "$out" "Sew"
set +e
out="$(k2 hostchat users invite bob 2>&1)"; rc=$?
set -e
expect_eq "hostchat users invite without Sew → exit 3" "$rc" "3"
set +e
out="$(k2 hostchat enable --identity ldap 2>&1)"; rc=$?
set -e
expect_eq "hostchat enable --identity ldap → exit 2" "$rc" "2"
set +e
out="$(k2 hostchat media probe --udp maybe --tcp ok --turn-tls ok 2>&1)"; rc=$?
set -e
expect_eq "hostchat media probe bad value → exit 2" "$rc" "2"

echo
echo "hostchat headless: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
