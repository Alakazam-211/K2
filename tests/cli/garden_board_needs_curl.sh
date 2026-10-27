#!/usr/bin/env bash
# Headless curl smoke for prd-garden-board-needs-v1 (0.46.5): a sandbox
# daemon, the owner token, no window, no CLI. Both doors:
#   - POST-only rows answer 405 on GET; bodies over the cap answer 413;
#   - the widget door (/cli/zen/blob/*) refuses a web Origin and answers
#     only for a placed widget;
#   - the agent door round-trips bytes and lists them; a ref the space
#     doesn't hold is blob_missing (409);
#   - history / restore / snapshot (when the daemon has garden-history-v1).
#
# Build first:  cargo build -p k2-daemon   (CARGO_TARGET_DIR honoured)
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
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

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/k2gbn-XXXXXX")"
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
HOME="$DHOME" K2_AIRGAP=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
BASE="http://127.0.0.1:$PORT"

# req METHOD PATH [BODY-FILE] [extra curl args…] → $CODE, body in $SANDBOX/body
req() {
    local method="$1" path="$2" body="${3:-}"
    shift 3 2>/dev/null || shift $#
    local sep='?'
    case "$path" in *\?*) sep='&' ;; esac
    if [ -n "$body" ]; then
        CODE="$(curl -s -o "$SANDBOX/body" -w '%{http_code}' -X "$method" -H 'Content-Type: application/json' \
            --data-binary "@$body" "$@" "$BASE$path${sep}token=$TOKEN")"
    else
        CODE="$(curl -s -o "$SANDBOX/body" -w '%{http_code}' -X "$method" "$@" "$BASE$path${sep}token=$TOKEN")"
    fi
}
jget() { python3 -c 'import functools, json, sys; print(json.dumps(functools.reduce(lambda v, k: v[int(k)] if isinstance(v, list) else v[k], sys.argv[1].split("."), json.load(open(sys.argv[2])))))' "$1" "$SANDBOX/body"; }
json_body() { python3 -c 'import json, sys; open(sys.argv[1], "w").write(sys.argv[2])' "$SANDBOX/req.json" "$1"; }

echo "== method and size gates =="
for p in /cli/zen/blob/put /cli/zen/data/blob/put; do
    req GET "$p"
    expect_eq "GET $p is 405" "$CODE" "405"
done
python3 -c 'import sys; open(sys.argv[1], "w").write("{\"widget\":\"board\",\"base64\":\"" + "A" * (13 * 1024 * 1024) + "\",\"last\":true}")' "$SANDBOX/huge.json"
req POST /cli/zen/data/blob/put "$SANDBOX/huge.json"
expect_eq "a body over 12 MiB is 413" "$CODE" "413"

echo "== the widget door =="
json_body '{"garden":"g","placement":"p","hash":"h","base64":"eA==","last":true}'
req POST /cli/zen/blob/put "$SANDBOX/req.json" -H 'Origin: https://evil.example'
expect_eq "a web Origin is refused (403)" "$CODE" "403"
expect_eq "origin_refused" "$(jget error.code)" '"origin_refused"'
req POST /cli/zen/blob/put "$SANDBOX/req.json" -H 'Origin: tauri://localhost'
expect_eq "no placed widget: 404" "$CODE" "404"
expect_eq "widget_not_found" "$(jget error.code)" '"widget_not_found"'

echo "== the agent door, as the person =="
B64="$(printf 'hello blob' | base64)"
json_body "{\"widget\":\"board\",\"base64\":\"$B64\",\"last\":true,\"name\":\"hello.txt\"}"
req POST /cli/zen/data/blob/put "$SANDBOX/req.json"
expect_eq "put 200" "$CODE" "200"
REF="$(jget ref | tr -d '"')"
expect_eq "the ref is the sha256" "$REF" "k2blob:sha256-$(printf 'hello blob' | python3 -c 'import hashlib, sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())')"
expect_eq "mime sniffed as text" "$(jget mime)" '"text/plain"'
req GET "/cli/zen/data/blob/get?widget=board&ref=$REF"
expect_eq "get 200" "$CODE" "200"
expect_eq "the bytes come back" "$(jget base64 | tr -d '"' | base64 -d 2>/dev/null || jget base64 | tr -d '"' | base64 -D)" "hello blob"
req GET "/cli/zen/data/blob/list?widget=board"
expect_eq "list 200" "$CODE" "200"
expect_eq "one pending blob" "$(jget items.0.pending)" "true"
json_body "{\"widget\":\"board\",\"key\":\"board/b1/note\",\"value\":{\"src\":\"$REF\"}}"
req POST /cli/zen/data/put "$SANDBOX/req.json"
expect_eq "a value naming it is accepted" "$CODE" "200"
req GET "/cli/zen/data/blob/list?widget=board"
expect_eq "now held by one key" "$(jget items.0.refs)" "1"
GHOST="k2blob:sha256-$(printf 'ghost' | python3 -c 'import hashlib, sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())')"
json_body "{\"widget\":\"board\",\"key\":\"board/b1/ghost\",\"value\":[\"$GHOST\"]}"
req POST /cli/zen/data/put "$SANDBOX/req.json"
expect_eq "an unheld ref is 409" "$CODE" "409"
expect_eq "blob_missing names it" "$(jget error.ref)" "\"$GHOST\""

if curl -sf "$BASE/boot-status" | grep -q '"garden-history-v1"'; then
    echo "== history, snapshot, restore =="
    json_body '{"widget":"board","key":"board/b1/obj","value":{"v":1}}'
    req POST /cli/zen/data/put "$SANDBOX/req.json"
    REV1="$(jget rev)"
    json_body '{"widget":"board","key":"board/b1/obj","value":{"v":2}}'
    req POST /cli/zen/data/put "$SANDBOX/req.json"
    req GET "/cli/zen/data/history?widget=board&key=board/b1/obj"
    expect_eq "history 200" "$CODE" "200"
    expect_eq "newest first: v2 over v1" "$(jget items.0.value.v)-$(jget items.0.prev.v)" "2-1"
    json_body '{"widget":"board","prefixes":["board/b1/"],"label":"before"}'
    req POST /cli/zen/data/snapshot "$SANDBOX/req.json"
    expect_eq "snapshot 200" "$CODE" "200"
    json_body "{\"widget\":\"board\",\"toRev\":$REV1,\"prefixes\":[\"board/b1/obj\"]}"
    req POST /cli/zen/data/restore "$SANDBOX/req.json"
    expect_eq "restore 200" "$CODE" "200"
    expect_eq "one key written" "$(jget written)" "1"
    req GET "/cli/zen/data/get?widget=board&key=board/b1/obj"
    expect_eq "restored to v1" "$(jget entry.value.v)" "1"
    req GET "/cli/zen/data/history?widget=board&key=board/b1/obj&limit=1"
    expect_eq "the restore is marked in history" "$(jget items.0.restore | cut -c2-4)" "rs-"
    req GET /cli/zen/data/restore
    expect_eq "GET restore is 405" "$CODE" "405"
fi

echo
echo "garden_board_needs_curl: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
