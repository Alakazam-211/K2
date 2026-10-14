#!/usr/bin/env bash
# Garden data export, history and import (contract-garden-data-v1 §2.3,
# §4.5, §8; B3) against a headless sandbox daemon, as the person (the owner
# token, a terminal outside K2):
#   - k2 zen data export / k2 store export --widget: JSONL, header first;
#   - --at <today> after the dev-build snapshot hook equals the live keys;
#     the snapshot is 0600 in a 0700 folder; --at before any snapshot exits
#     4 (no_snapshot); a bad date exits 2;
#   - k2 zen data import: merge (one rev), --replace (deletes what the file
#     doesn't name), a pre-op snapshot before each; GET on import is 405.
# Agent filtering is covered by the daemon's unit tests (a headless script
# has no session passport).
#
# Build first:  cargo build -p k2-daemon   (CARGO_TARGET_DIR honoured)
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/k2gd-XXXXXX")"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.3
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2
        tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME"
k2() { HOME="$DHOME" "$K2" "$@"; }
run() {
    set +e
    k2 "$@" >"$SANDBOX/out" 2>"$SANDBOX/err"
    rc=$?
    set -e
}
runin() { # stdin-file args…
    local f="$1"
    shift
    set +e
    k2 "$@" <"$f" >"$SANDBOX/out" 2>"$SANDBOX/err"
    rc=$?
    set -e
}
keys_of() { tail -n +2 "$1" | python3 -c 'import json,sys; print(" ".join(json.loads(l)["key"] for l in sys.stdin if l.strip()))'; }

echo "== sandbox daemon (dev build, K2_GARDEN_DATA_SNAPSHOT_NOW=1) =="
HOME="$DHOME" K2_AIRGAP=1 K2_GARDEN_DATA_SNAPSHOT_NOW=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"

echo "== some data (the person) =="
run store put --widget board obj/1 --value '{"t":"one"}'
expect_eq "put obj/1" "$rc" "0"
run store put --widget board obj/2 --value '"two"'
expect_eq "put obj/2" "$rc" "0"
run store put --widget board project/7/obj/9 --value '[1,2]'
expect_eq "put project/7/obj/9" "$rc" "0"

echo "== export (live) =="
run zen data export board
expect_eq "zen data export exit 0" "$rc" "0"
cp "$SANDBOX/out" "$SANDBOX/live.jsonl"
head -1 "$SANDBOX/live.jsonl" > "$SANDBOX/head"
expect_eq "header names the space and rev" \
    "$(python3 -c 'import json,sys; h=json.load(open(sys.argv[1])); print(h["space"], h["rev"], h["snapshot"])' "$SANDBOX/head")" \
    "data:board 3 None"
expect_eq "rows in key order" "$(keys_of "$SANDBOX/live.jsonl")" "obj/1 obj/2 project/7/obj/9"
expect_eq "a row's value and by" \
    "$(sed -n 2p "$SANDBOX/live.jsonl" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(json.dumps(r["value"]), r["by"])')" \
    '{"t": "one"} you'
run store export --widget board
expect_eq "store export --widget exit 0" "$rc" "0"
expect_eq "store export --widget = zen data export (rows)" "$(keys_of "$SANDBOX/out")" "obj/1 obj/2 project/7/obj/9"
run zen data export nothing-here
expect_eq "a widget with no data exports a header only" "$(wc -l < "$SANDBOX/out" | tr -d ' ')" "1"

echo "== history (--at) =="
TODAY="$(date +%Y-%m-%d)"
run store export --widget board --at "$TODAY"
expect_eq "--at today exit 0" "$rc" "0"
expect_eq "--at today = live keys" "$(keys_of "$SANDBOX/out")" "obj/1 obj/2 project/7/obj/9"
expect_eq "--at header names the snapshot" \
    "$(head -1 "$SANDBOX/out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["snapshot"])')" "$TODAY"
SNAPS="$DHOME/.k2/stores/.snapshots"
[ -f "$SNAPS/gardens-$TODAY.sqlite" ] && ok "today's snapshot file" || bad "today's snapshot file"
expect_eq "snapshot is 0600" "$(stat -f %Lp "$SNAPS/gardens-$TODAY.sqlite" 2>/dev/null || stat -c %a "$SNAPS/gardens-$TODAY.sqlite")" "600"
expect_eq "snapshot folder is 0700" "$(stat -f %Lp "$SNAPS" 2>/dev/null || stat -c %a "$SNAPS")" "700"
run store export --widget board --at 2000-01-01
expect_eq "--at before any snapshot exits 4" "$rc" "4"
expect_has "no_snapshot" "$(cat "$SANDBOX/err")" "no_snapshot"
run zen data export board --at yesterday
expect_eq "a bad date exits 2" "$rc" "2"

echo "== import =="
python3 - "$SANDBOX/live.jsonl" > "$SANDBOX/edited.jsonl" <<'EOF'
import json, sys
lines = open(sys.argv[1]).read().splitlines()
print(lines[0])
for l in lines[1:]:
    r = json.loads(l)
    if r["key"] == "obj/1":
        r["value"] = {"t": "edited"}
    print(json.dumps(r))
print(json.dumps({"key": "obj/3", "value": 3}))
EOF
runin "$SANDBOX/edited.jsonl" zen data import board
expect_eq "merge import exit 0" "$rc" "0"
expect_has "merge wrote 4 at one rev" "$(cat "$SANDBOX/out")" "Imported 4 key(s) into board at rev 4; 0 deleted."
run store get --widget board obj/1 --json
expect_eq "obj/1 edited at rev 4" "$(python3 -c 'import json,sys; e=json.load(open(sys.argv[1]))["entry"]; print(e["rev"], e["value"]["t"])' "$SANDBOX/out")" "4 edited"
printf '%s\n' '[{"key":"obj/2","value":"kept"}]' > "$SANDBOX/only2.json"
runin "$SANDBOX/only2.json" zen data import board --replace --json
expect_eq "replace import exit 0" "$rc" "0"
expect_eq "replace: 1 written, 3 deleted, rev 5" \
    "$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(v["written"], v["deleted"], v["rev"])' "$SANDBOX/out")" "1 3 5"
run zen data export board
expect_eq "only obj/2 is left" "$(keys_of "$SANDBOX/out")" "obj/2"
expect_eq "two pre-op import snapshots" "$(ls "$SNAPS" | grep -c -- '-import\.sqlite$')" "2"
printf '%s\n' '{"key":"/bad","value":1}' > "$SANDBOX/bad.jsonl"
runin "$SANDBOX/bad.jsonl" zen data import board
expect_eq "a bad key exits 1" "$rc" "1"
expect_has "bad_key" "$(cat "$SANDBOX/err")" "bad_key"
expect_eq "GET on import is 405" \
    "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/zen/data/import?token=$OWNER_TOKEN")" "405"

echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
