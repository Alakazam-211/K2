#!/usr/bin/env bash
# k2 zen widget against a REAL headless daemon (prd-zen-user-widgets-v2
# UW41, TUW5.1). Needs a daemon with the widget routes (`zen-widgets-v1`
# in /boot-status); it fails loudly on an older one.
#
# Boots the worktree's k2-daemon under a temp HOME (never the real ~/.k2,
# never a production daemon), then:
#   - before Zen is set up every widget verb exits 3 with the top-bar sentence;
#   - after setup: widget new w --from hello prints the folder; list --json
#     parses with w in it; validate/history --widget w work; new w again
#     exits 1; widget grant / revoke only say there are no permissions
#     (Rosson 2026-10-08) and reach no route;
#   - nothing the CLI does writes a grant.
# The parts that need no daemon are tests/cli/zen_widget_offline.sh.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d -t k2-zen-widget-XXXXXX)"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

mkdir -p "$SANDBOX/.k2" "$SANDBOX/shim"
env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2_PORT -u K2SO_PORT \
    HOME="$SANDBOX" K2_TEST_AGENT_SHIM_DIR="$SANDBOX/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 200); do
    [ -s "$SANDBOX/.k2/daemon.port" ] && [ -s "$SANDBOX/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SANDBOX/.k2/daemon.port" ] || { echo "FAIL: daemon never wrote daemon.port" >&2; tail -30 "$SANDBOX/daemon.log" >&2; exit 1; }
PORT="$(cat "$SANDBOX/.k2/daemon.port")"
TOKEN="$(cat "$SANDBOX/.k2/daemon.token")"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$PORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }
features="$(curl -s "http://127.0.0.1:$PORT/boot-status" | python3 -c 'import json,sys; print(" ".join(json.load(sys.stdin).get("features") or []))')"
case " $features " in
    *" zen-widgets-v1 "*) ;;
    *) echo "FAIL: this daemon has no zen-widgets-v1 (features: $features)" >&2; exit 1 ;;
esac

ZEN="$SANDBOX/.k2/zen"

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2_PROJECT_PATH -u K2_CELL \
        HOME="$SANDBOX" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$TOKEN" \
        "$K2_CLI" "$@"
}

capture() {
    set +e
    out="$(run_k2 "$@" 2>&1)"
    rc=$?
    set -e
}

echo "== not set up =="
NOT_SET_UP="Zen isn't set up on this computer. Turn it on with the Zen toggle in the K2 app's top bar."
for verb in "widget list" "widget new w" "validate --widget w" "history --widget w"; do
    # shellcheck disable=SC2086
    capture zen $verb
    assert_eq "zen $verb before setup exits 3" "$rc" "3"
    assert_contains "zen $verb before setup says so" "$out" "$NOT_SET_UP"
done
[ ! -e "$ZEN" ] && ok "no widget verb creates ~/.k2/zen" || bad "a widget verb created ~/.k2/zen"

echo "== set up (as the app does) =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
assert_contains "setup created the folder" "$resp" '"createdFolder":true'

echo "== widget new / list / validate / history =="
capture zen widget new w --from hello
assert_eq "widget new exits 0" "$rc" "0"
assert_contains "widget new prints the folder" "$out" "$ZEN/widgets/w"
[ -f "$ZEN/widgets/w/manifest.json" ] && ok "the example's manifest is on disk" || bad "no manifest.json in widgets/w"
capture zen widget new w --from hello
assert_eq "widget new on an existing folder exits 1" "$rc" "1"
capture zen widget list --json
assert_eq "widget list --json exits 0" "$rc" "0"
names="$(printf '%s' "$out" | python3 -c 'import json,sys; print(" ".join(w["name"] for w in json.load(sys.stdin)["widgets"]))')"
assert_contains "list --json has w" " $names " " w "
capture zen widget list
assert_eq "widget list exits 0" "$rc" "0"
assert_contains "widget list says w isn't placed" "$out" "not placed in any Garden"
capture zen validate --widget w
assert_eq "validate --widget w exits 0" "$rc" "0"
capture zen history --widget w
assert_eq "history --widget w exits 0" "$rc" "0"
capture zen validate --widget nope
[ "$rc" -ne 0 ] && ok "validate --widget on a missing folder fails" || bad "validate --widget nope exited 0"

echo "== no permissions (Rosson 2026-10-08) =="
for verb in "widget grant" "widget revoke --widget w" "widget allow w"; do
    # shellcheck disable=SC2086
    capture zen $verb
    assert_eq "zen $verb exits 0" "$rc" "0"
    assert_eq "zen $verb says there are no permissions" "$out" "Widgets in your own Gardens need no permissions: they can talk to your agents right away. To stop one, take its [[widget]] out of the Garden file."
done
for route in widget/grant widget/revoke widget/sending; do
    code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/cli/zen/$route?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
    [ "$code" != "200" ] && ok "POST $route is gone ($code)" || bad "POST $route still answers 200"
done
capture zen widget list
assert_eq "widget list exits 0" "$rc" "0"
case "$out" in *grant*|*review*) bad "widget list still talks about grants: $out" ;; *) ok "widget list says nothing about grants" ;; esac
[ ! -e "$ZEN/grants.json" ] && ok "nothing wrote grants.json" || bad "grants.json appeared"

echo
echo "zen_widget_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
