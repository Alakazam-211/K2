#!/usr/bin/env bash
# `k2 store blob` and `k2 zen data export|import --blobs` (0.46.5,
# prd-garden-board-needs-v1 GB16, GB17) against a headless sandbox daemon,
# as the person (the owner token, a terminal outside K2):
#   - put prints the ref (the daemon's sha256 of the bytes), get writes the
#     exact bytes back (a 5 MiB file crosses several 2 MiB chunks), ls lists;
#   - a value naming a blob this widget's data doesn't hold: blob_missing,
#     exit 4, nothing written; after the upload it is accepted;
#   - one put whose single chunk is a body over 1 MiB goes through a relay
#     that records each request on its way to the daemon: the daemon takes
#     it whole with or without "Expect: 100-continue" (bash's curl sends
#     it; the k5 binary never does: scripts/cli-parity.sh's named
#     difference expect-100-continue), and get gives the bytes back;
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
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
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
RELAY_PID=""
cleanup() {
    [ -n "$RELAY_PID" ] && kill "$RELAY_PID" 2>/dev/null || true
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

echo "== one POST over 1 MiB, through a recording relay =="
# The relay passes each connection's bytes both ways untouched and keeps
# what the CLI sent, one file per connection (each CLI request is its own
# connection). K2_PORT points this one call at it; the token is the owner
# token on disk, as above.
mkdir -p "$SANDBOX/relay"
python3 -I -c '
import os, socket, sys, threading
d, up = sys.argv[1], int(sys.argv[2])
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(8)
open(os.path.join(d, "port.tmp"), "w").write(str(s.getsockname()[1]))
os.rename(os.path.join(d, "port.tmp"), os.path.join(d, "port"))
def pipe(a, b, keep):
    try:
        while True:
            buf = a.recv(65536)
            if not buf:
                break
            if keep:
                keep.write(buf)
                keep.flush()
            b.sendall(buf)
    except OSError:
        pass
    finally:
        for c in (a, b):
            try:
                c.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
def serve(c, n):
    u = socket.create_connection(("127.0.0.1", up))
    keep = open(os.path.join(d, "conn-%03d" % n), "wb")
    back = threading.Thread(target=pipe, args=(u, c, None), daemon=True)
    back.start()
    pipe(c, u, keep)
    back.join()
    keep.close()
    c.close()
    u.close()
n = 0
while True:
    c, _ = s.accept()
    n += 1
    threading.Thread(target=serve, args=(c, n), daemon=True).start()
' "$SANDBOX/relay" "$PORT" &
RELAY_PID=$!
for _ in $(seq 1 100); do [ -s "$SANDBOX/relay/port" ] && break; sleep 0.05; done
[ -s "$SANDBOX/relay/port" ] || { bad "the relay wrote its port"; exit 1; }
RELAY_PORT="$(cat "$SANDBOX/relay/port")"
# 1,200,000 bytes: one 2 MiB chunk, about 1.6 MB of base64 in its JSON.
python3 -c 'import os, sys; open(sys.argv[1], "wb").write(os.urandom(1200000))' "$SANDBOX/mid.bin"
# covers: store --widget
set +e
HOME="$DHOME" K2_PORT="$RELAY_PORT" "$K2" store blob put "$SANDBOX/mid.bin" --widget board >"$SANDBOX/out" 2>"$SANDBOX/err"
rc=$?
set -e
expect_eq "a put of one chunk over 1 MiB, through the relay, exits 0" "$rc" "0"
MID="$(cat "$SANDBOX/out")"
expect_eq "its ref is the sha256 of the bytes" "$MID" "k2blob:sha256-$(sha "$SANDBOX/mid.bin")"
# Each request the relay saw: "<request line>|<Content-Length>|<bytes of
# body seen>|<1 when the head has Expect: 100-continue, else 0>".
python3 -c '
import glob, os, sys
for p in sorted(glob.glob(os.path.join(sys.argv[1], "conn-*"))):
    raw = open(p, "rb").read()
    head, _, body = raw.partition(b"\r\n\r\n")
    lines = head.decode("latin-1").split("\r\n")
    hdr = {}
    for l in lines[1:]:
        k, _, v = l.partition(":")
        hdr.setdefault(k.strip().lower(), []).append(v.strip())
    expect = int(any(v.lower() == "100-continue" for v in hdr.get("expect", [])))
    print("%s|%s|%d|%d" % (lines[0], ",".join(hdr.get("content-length", [])), len(body), expect))
' "$SANDBOX/relay" >"$SANDBOX/relay.seen"
kill "$RELAY_PID" 2>/dev/null || true
wait "$RELAY_PID" 2>/dev/null || true
RELAY_PID=""
expect_eq "the relay saw one request" "$(wc -l <"$SANDBOX/relay.seen" | tr -d ' ')" "1"
IFS='|' read -r SEEN_LINE SEEN_LEN SEEN_BODY SEEN_EXPECT <"$SANDBOX/relay.seen"
expect_eq "it is the blob put" "$SEEN_LINE" "POST /cli/zen/data/blob/put HTTP/1.1"
if [ "$SEEN_LEN" -gt 1048576 ]; then ok "its body is over 1 MiB ($SEEN_LEN bytes)"; else bad "its body is $SEEN_LEN bytes, want over 1 MiB"; fi
expect_eq "the whole body reached the daemon" "$SEEN_BODY" "$SEEN_LEN"
if [ "$CLI_UNDER_TEST" != "$CLI_BASH_SOURCE" ]; then
    # The k5 binary never sends Expect: 100-continue: scripts/cli-parity.sh's
    # named difference expect-100-continue. The daemon takes the body as it
    # comes.
    expect_eq "the k5 binary sent no Expect: 100-continue" "$SEEN_EXPECT" "0"
else
    expect_eq "bash's curl sent Expect: 100-continue on a body over 1 MiB" "$SEEN_EXPECT" "1"
fi
run store blob get "$MID" --widget board --out "$SANDBOX/mid.back"
expect_eq "its get exits 0" "$rc" "0"
expect_eq "its get gives the bytes back exactly" "$(sha "$SANDBOX/mid.back")" "$(sha "$SANDBOX/mid.bin")"

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
