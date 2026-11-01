#!/usr/bin/env bash
# The Board Garden end to end (Rosson 2026-11-01) against a foreground
# sandbox daemon built from THIS worktree (temp HOME, no client attached):
# the Board core is off on a fresh box (k2 board and raw k2 store answer
# core_off, exit 3); adding a Board Garden without the confirm is 409, with
# it the core turns on in the same step; k2 board makes a board and runs the
# blind agent's add / connect / delete (the cascade takes the connector);
# turning the core off keeps the data; the export carries no K2 ids.
#
#   cargo build -p k2-daemon && bash tests/cli/board_headless.sh
#
# Every check fails loudly with the output.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d /tmp/k2board.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
DAEMON_PID=""
DLOG="$SB/daemon.log"
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    chmod -R u+w "$WORK" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -40 "$DLOG" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }

# cli [env K=V ...] -- <k2 args>: OUT, ERR, RC.
cli() {
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT")
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    set +e
    OUT="$(env "${envs[@]}" "$K2_CLI" "$@" 2>"$WORK/cli.err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }
cores_json() { curl -s "$BASE/cli/docs/cores?token=$DTOKEN"; }

echo "== daemon =="
# The agent's pinned chat runs this shim, which records its env: the real
# passport (K2_HOOK_TOKEN) and cell socket (K2_HOOK_SOCK) K2 hands an agent.
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$DLOG" 2>&1 &
DAEMON_PID=$!
phase=""
for _ in $(seq 1 400); do
    if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
        DPORT="$(cat "$SB/.k2/daemon.port")"
        phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
        [ "$phase" = "ready" ] && break
    fi
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
ok "sandbox daemon ready ($DAEMON_BIN)"

post() { curl -s -o "$WORK/post.out" -w '%{http_code}' -X POST -H 'Content-Type: application/json' --data-raw "$2" "$BASE$1?token=$DTOKEN"; }
oid() { printf '%s' "$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'; }

echo "== the core is off on a fresh box =="
jok "$(cores_json)" '[c for c in j["cores"] if c["id"] == "board"][0]["on"] is False and [c for c in j["cores"] if c["id"] == "board"][0]["view"] == "garden"' "board core listed, off, view garden"
cli -- board boards
want_rc 3 "k2 board while off"
has "$ERR" "core_off" "the refusal names core_off"
cli -- store list --widget k2:board
want_rc 3 "raw k2 store while off"
templates="$(curl -s "$BASE/cli/zen/templates?token=$DTOKEN")"
jok "$templates" '[t for t in j["templates"] if t["short"] == "board"][0]["coreOn"] is False' "New Garden lists Board with its core off"

echo "== adding a Board Garden is the opt-in =="
[ "$(post /cli/zen/setup '{}')" = 200 ] || fail "zen setup: $(cat "$WORK/post.out")"
code="$(post /cli/zen/garden/new '{"name":"Ideas","template":"board"}')"
[ "$code" = 409 ] || fail "garden/new without the confirm: $code $(cat "$WORK/post.out")"
jok "$(cat "$WORK/post.out")" 'j["error"]["code"] == "core_off" and j["error"]["core"] == "board"' "409 core_off names the core"
code="$(post /cli/zen/garden/new '{"name":"Ideas","template":"board","turnOnCore":"board"}')"
[ "$code" = 200 ] || fail "garden/new with the confirm: $code $(cat "$WORK/post.out")"
ok "the Board Garden is made"
jok "$(cores_json)" '[c for c in j["cores"] if c["id"] == "board"][0]["on"] is True' "the core is on in the same step"

echo "== k2 board: the blind agent's add / connect / delete =="
cli -- board board-new --name Launch
want_rc 0 "board-new"
cli -- board add shape --text Old --board Launch
want_rc 0 "add shape"
SHAPE="$(oid "$OUT")"
cli -- board add sticky --text "Ship the beta" --x 400 --y 300 --color yellow --board Launch
want_rc 0 "add sticky"
jok "$OUT" 'j["text"] == "Ship the beta" and j["x"] == 400 and j["color"] == "yellow" and j["createdBy"] == "person:owner"' "the sticky as written"
STICKY="$(oid "$OUT")"
cli -- board connect "$SHAPE" "$STICKY" --label next --board Launch
want_rc 0 "connect"
jok "$OUT" 'j["endCap"] == "arrow" and j["label"] == "next"' "an arrow labelled next"
CONN="$(oid "$OUT")"
cli -- board delete "$SHAPE" --board Launch
want_rc 0 "delete"
[ "$OUT" = "Deleted $SHAPE, $CONN" ] || fail "the cascade: $OUT"
ok "delete takes the connector"
cli -- board list --board Launch --json
jok "$OUT" '[o["id"] for o in j["objects"]] == ["'"$STICKY"'"]' "only the sticky is left"
LONG="$(python3 -c 'print("x" * 5001)')"
cli -- board update "$STICKY" --text "$LONG" --board Launch
want_rc 2 "too long"
has "$ERR" "too_long" "refused, not cut"

echo "== off keeps the data =="
cli -- doc cores off board
want_rc 0 "core off"
cli -- board list --board Launch
want_rc 3 "list while off"
cli -- doc cores on board
want_rc 0 "core on"
cli -- board list --board Launch --json
jok "$OUT" 'len(j["objects"]) == 1' "the board is back as it was"

echo "== export =="
cli -- board export --board Launch --out "$WORK/launch.json"
want_rc 0 "export"
python3 - "$WORK/launch.json" <<'PY' || fail "export envelope"
import json, sys
t = open(sys.argv[1]).read(); e = json.loads(t)
assert e["format"] == "k2-board" and e["version"] == 1 and e["title"] == "Launch", e
assert len(e["objects"]) == 1 and e["objects"][0]["createdBy"] == "p1", e
for bad in ("person:owner", "k2blob:", '"batch"'):
    assert bad not in t, bad
PY
ok "the export has people tokens and no K2 ids"

echo "board_headless: $pass checks passed"
