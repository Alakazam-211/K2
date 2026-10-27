#!/usr/bin/env bash
# `k2 store batch --widget` with patch ops (0.46.5, Board via Appa) against a
# headless sandbox daemon, as the person (the owner token, a terminal
# outside K2):
#   - put + patch + delete in one call land at one rev; the patch keeps the
#     fields it doesn't name (null removes one), like `k2 store patch`;
#   - a stale ifRev on one patch writes nothing, exits 5 (conflict) and the
#     error names the op's index and key; --json carries {op, key, rev};
#   - an invalid patch (a partial that isn't an object) writes nothing and
#     names the op;
#   - over 64 ops the CLI uses a transaction (begin/add/commit): a patch in
#     the last part commits with the rest at one rev, and a stale one rolls
#     every part back.
# Agent grants on patch ops are covered by the daemon's unit tests (a
# headless script has no session passport).
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
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/k2gbp-XXXXXX")"
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
# `k2 store get --widget board <key> --json` → "<rev> <value as compact JSON>".
entry() {
    run store get --widget board "$1" --json
    python3 -c 'import json,sys; e=json.load(open(sys.argv[1]))["entry"]; print("null" if e is None else "%s %s" % (e["rev"], json.dumps(e["value"], sort_keys=True, separators=(",", ":"))))' "$SANDBOX/out"
}
# The space's rev now, from the daemon's own changes route.
head_rev() {
    curl -sf "http://127.0.0.1:$PORT/cli/zen/data/changes?widget=board&since=0&token=$OWNER_TOKEN" |
        python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["head"])'
}

echo "== sandbox daemon =="
HOME="$DHOME" K2_AIRGAP=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
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

echo "== seed (the person) =="
run store put --widget board obj/1 --value '{"t":"a","x":1,"col":"todo"}'
expect_eq "put obj/1" "$rc" "0"
run store put --widget board obj/2 --value '{"t":"b"}'
expect_eq "put obj/2" "$rc" "0"
expect_eq "head is rev 2" "$(head_rev)" "2"

echo "== batch: put + patch + delete, one rev =="
cat > "$SANDBOX/ops.json" <<'EOF'
[{"op":"patch","key":"obj/1","value":{"x":5,"t":null},"ifRev":1},
 {"op":"put","key":"edge/1","value":{"from":"obj/1","to":"obj/2"}},
 {"op":"delete","key":"obj/2","ifRev":2}]
EOF
runin "$SANDBOX/ops.json" store batch --widget board
expect_eq "batch exit 0" "$rc" "0"
expect_eq "batch answers one rev" "$(cat "$SANDBOX/out")" "rev 3 (3 ops)"
expect_eq "obj/1 patched, other fields kept" "$(entry obj/1)" '3 {"col":"todo","x":5}'
expect_eq "edge/1 put at the same rev" "$(entry edge/1)" '3 {"from":"obj/1","to":"obj/2"}'
expect_eq "obj/2 deleted" "$(entry obj/2)" "null"
run store changes --widget board --since 2
expect_eq "changes exit 0" "$rc" "0"
expect_eq "one change row per key at rev 3" \
    "$(python3 -c 'import json,sys; rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]; print(" ".join("%s@%s%s" % (c["key"], c["rev"], "-" if c["deleted"] else "") for c in rows))' "$SANDBOX/out")" \
    "edge/1@3 obj/1@3 obj/2@3-"

echo "== batch: a stale ifRev on one patch writes nothing =="
printf '%s\n' '{"op":"put","key":"obj/9","value":9}' '{"op":"patch","key":"obj/1","value":{"x":6},"ifRev":1}' > "$SANDBOX/stale.jsonl"
runin "$SANDBOX/stale.jsonl" store batch --widget board
expect_eq "conflict exits 5" "$rc" "5"
expect_has "the error names the op and key" "$(cat "$SANDBOX/err")" "conflict: op 1 ('obj/1')"
runin "$SANDBOX/stale.jsonl" store batch --widget board --json
expect_eq "--json conflict exits 5" "$rc" "5"
expect_eq "--json carries op, key and the live rev" \
    "$(python3 -c 'import json,sys; e=json.load(open(sys.argv[1]))["error"]; print(e["code"], e["op"], e["key"], e["rev"])' "$SANDBOX/out")" \
    "conflict 1 obj/1 3"
expect_eq "obj/9 not written" "$(entry obj/9)" "null"
expect_eq "obj/1 unchanged" "$(entry obj/1)" '3 {"col":"todo","x":5}'
expect_eq "no new rev" "$(head_rev)" "3"

echo "== batch: an invalid patch writes nothing =="
printf '%s\n' '[{"op":"delete","key":"edge/1"},{"op":"patch","key":"obj/1","value":[1]}]' > "$SANDBOX/bad.json"
runin "$SANDBOX/bad.json" store batch --widget board
expect_eq "bad patch exits 1" "$rc" "1"
expect_has "bad_request names the op" "$(cat "$SANDBOX/err")" "bad_request: op 1 ('obj/1'): "
expect_eq "edge/1 kept" "$(entry edge/1)" '3 {"from":"obj/1","to":"obj/2"}'
expect_eq "still rev 3" "$(head_rev)" "3"

echo "== over 64 ops: a transaction, patch in the last part =="
python3 -c '
import json
ops = [{"op": "put", "key": "bulk/%03d" % i, "value": {"i": i}} for i in range(70)]
ops.append({"op": "patch", "key": "obj/1", "value": {"col": "done"}, "ifRev": 3})
print(json.dumps(ops))' > "$SANDBOX/big.json"
runin "$SANDBOX/big.json" store batch --widget board
expect_eq "transaction exit 0" "$rc" "0"
expect_eq "transaction answers one rev" "$(cat "$SANDBOX/out")" "rev 4 (71 ops)"
expect_eq "obj/1 patched in the commit" "$(entry obj/1)" '4 {"col":"done","x":5}'
expect_eq "bulk/069 at the same rev" "$(entry bulk/069)" '4 {"i":69}'
python3 -c '
import json
ops = [{"op": "put", "key": "more/%03d" % i, "value": i} for i in range(70)]
ops.append({"op": "patch", "key": "obj/1", "value": {"col": "x"}, "ifRev": 3})
print(json.dumps(ops))' > "$SANDBOX/bigstale.json"
runin "$SANDBOX/bigstale.json" store batch --widget board
expect_eq "stale transaction exits 5" "$rc" "5"
expect_has "the error names op 70" "$(cat "$SANDBOX/err")" "op 70 ('obj/1')"
expect_eq "no part was written" "$(entry more/000)" "null"
expect_eq "still rev 4" "$(head_rev)" "4"

echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
