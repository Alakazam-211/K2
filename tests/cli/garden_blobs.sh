#!/usr/bin/env bash
# `k2 store blob` and `k2 zen data export|import --blobs` (0.46.5,
# prd-garden-board-needs-v1 GB16, GB17) against a headless sandbox daemon,
# as the person (the owner token, a terminal outside K2):
#   - put prints the ref (the daemon's sha256 of the bytes), get writes the
#     exact bytes back (a 5 MiB file crosses several 2 MiB chunks), ls lists;
#   - a value naming a blob this widget's data doesn't hold: blob_missing,
#     exit 4, nothing written; after the upload it is accepted;
#   - export --blobs copies each named file into <dir>/<hash>; import
#     --blobs into a new widget brings the files with the data, and the same
#     import without them fails whole with blob_missing (exit 4).
# Agent grants on blobs are covered by the daemon's unit tests (a headless
# script has no session passport).
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
field() { python3 -c 'import functools, json, sys; print(json.dumps(functools.reduce(lambda v, k: v[k], sys.argv[2].split("."), json.load(open(sys.argv[1])))))' "$SANDBOX/out" "$1"; }
sha() { python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$1"; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/k2gbl-XXXXXX")"
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

echo "== sandbox daemon =="
HOME="$DHOME" K2_AIRGAP=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
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
expect_has "boot-status names garden-blobs-v1" "$(curl -sf "http://127.0.0.1:$PORT/boot-status")" '"garden-blobs-v1"'

echo "== put, get, ls =="
printf '\x89PNG\r\n\x1a\nnot-really-pixels' > "$SANDBOX/shot.png"
run store blob put "$SANDBOX/shot.png" --widget board
expect_eq "put exit 0" "$rc" "0"
REF="$(cat "$SANDBOX/out")"
expect_eq "the ref is the sha256 of the bytes" "$REF" "k2blob:sha256-$(sha "$SANDBOX/shot.png")"
run store blob put "$SANDBOX/shot.png" --widget board --json
expect_eq "the same bytes again: held already" "$(field new)" "false"
expect_eq "the type is sniffed" "$(field mime)" '"image/png"'
run store blob get "$REF" --widget board --out "$SANDBOX/back.png"
expect_eq "get exit 0" "$rc" "0"
expect_eq "get writes the exact bytes" "$(sha "$SANDBOX/back.png")" "$(sha "$SANDBOX/shot.png")"
python3 -c 'import os, sys; open(sys.argv[1], "wb").write(os.urandom(5 * 1024 * 1024 + 7))' "$SANDBOX/big.bin"
run store blob put "$SANDBOX/big.bin" --widget board
expect_eq "a 5 MiB put (3 chunks) exits 0" "$rc" "0"
BIG="$(cat "$SANDBOX/out")"
run store blob get "$BIG" --widget board --out "$SANDBOX/big.back"
expect_eq "a 5 MiB get comes back exactly" "$(sha "$SANDBOX/big.back")" "$(sha "$SANDBOX/big.bin")"
run store put --widget board board/b1/img --value "{\"src\":\"$REF\"}"
expect_eq "a value naming a held blob is accepted" "$rc" "0"
run store blob ls --widget board
expect_eq "ls exit 0" "$rc" "0"
expect_has "ls shows the held blob with its key count" "$(cat "$SANDBOX/out")" "$REF"
expect_has "ls says 1 key" "$(grep "$REF" "$SANDBOX/out")" "1 key"
expect_has "ls says the big one is pending" "$(grep "$BIG" "$SANDBOX/out")" "pending"

echo "== blob_missing =="
GHOST="k2blob:sha256-$(printf 'nobody uploaded this' | python3 -c 'import hashlib, sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())')"
run store put --widget board board/b1/ghost --value "{\"src\":\"$GHOST\"}"
expect_eq "a value naming an unheld blob exits 4" "$rc" "4"
expect_has "and says blob_missing" "$(cat "$SANDBOX/err")" "blob_missing"
run store get --widget board board/b1/ghost --json
expect_eq "nothing was written" "$(field entry)" "null"
run store blob get "$GHOST" --widget board --out "$SANDBOX/ghost"
expect_eq "getting a blob the data doesn't hold exits 4" "$rc" "4"
[ -e "$SANDBOX/ghost" ] && bad "a refused get left a file" || ok "a refused get leaves no file"
run store blob get "k2blob:sha256-../../etc/passwd" --widget board --out "$SANDBOX/x"
expect_eq "a ref that isn't one is refused (exit 1, bad_request)" "$rc" "1"

echo "== export / import --blobs =="
run zen data export board --blobs "$SANDBOX/files"
expect_eq "export --blobs exit 0" "$rc" "0"
cp "$SANDBOX/out" "$SANDBOX/data.jsonl"
expect_has "the export carries the ref" "$(cat "$SANDBOX/data.jsonl")" "$REF"
expect_eq "the named file is in the folder" "$(sha "$SANDBOX/files/${REF#k2blob:sha256-}")" "$(sha "$SANDBOX/shot.png")"
[ -e "$SANDBOX/files/${BIG#k2blob:sha256-}" ] && bad "an unnamed (pending) blob was exported" || ok "only named blobs are exported"
runin "$SANDBOX/data.jsonl" zen data import board3
expect_eq "import without the files exits 4" "$rc" "4"
expect_has "and says blob_missing" "$(cat "$SANDBOX/err")" "blob_missing"
run store get --widget board3 board/b1/img --json
expect_eq "a refused import writes nothing" "$(field entry)" "null"
runin "$SANDBOX/data.jsonl" zen data import board2 --blobs "$SANDBOX/files"
expect_eq "import --blobs exit 0" "$rc" "0"
run store blob get "$REF" --widget board2 --out "$SANDBOX/b2.png"
expect_eq "the file came with the data" "$(sha "$SANDBOX/b2.png")" "$(sha "$SANDBOX/shot.png")"

echo
echo "garden_blobs: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
