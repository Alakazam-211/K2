#!/usr/bin/env bash
# k2 app data ls|get|rm|mv (0.46.4): a sandbox daemon (temp HOME, K2_APPS=1
# + K2_APPS_PROCESS=1, air-gapped) and this checkout's cli/k2 on loopback, as
# the owner. An unwalled (process engine) App's data folder is the daemon
# user's, so the test plays the App's pages by writing into it directly.
#
# Covers: ls (folder, one file, a link, --json); get (default ./<name>,
# --to a folder, --to a file, - to stdout, --json, a file past one 512 KiB
# piece byte for byte, no part file left); rm (a folder needs --recursive,
# a link goes as a link); mv (no overwrite without --force, --force, never
# into a folder); refusals: .. and other absolute paths exit 2, a symlink on
# the way exits 3 data_symlink, a missing file exits 3 data_not_found.
#
# Build first:  cargo build -p k2-daemon
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
# APP_DATA_BUILD=1 builds it first (one command on a compute node).
if [ "${APP_DATA_BUILD:-0}" = 1 ]; then (cd "$PROJECT_ROOT" && cargo build -p k2-daemon); fi
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2data-XXXXXX)"
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
# Own TMPDIR / XDG_RUNTIME_DIR: the daemon's private runtime dir never
# resolves to the real one (/dev/shm/k2-<uid>).
HOME="$DHOME" TMPDIR="$SANDBOX/tmp" XDG_RUNTIME_DIR="$SANDBOX/run" K2_RUNTIME_DIR= K2_APPS=1 K2_APPS_PROCESS=1 K2_AIRGAP=1 K2_DISK_GUARD=off \
    "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
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
k2() { HOME="$DHOME" "$K2" "$@"; }
run() { # out-file cmd… → rc (never exits the script)
    local out="$1"; shift
    set +e
    k2 "$@" > "$out" 2>&1
    local rc=$?
    set -e
    echo "$rc"
}

echo "== workspace and App =="
WS="$SANDBOX/ws"
mkdir -p "$WS"
git -C "$WS" -c init.defaultBranch=main init -q
git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m one
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
expect_has "workspace registered" "$reg" "\"path\":\"$WS\""
cd "$WS"
export K2_PROJECT_PATH="$WS"
out="$(k2 app install pins --memory none --engine process)"
expect_has "installed" "$out" "App pins installed"

# What the App's PIN pages wrote.
DATA="$DHOME/.k2/apps/pins/data"
mkdir -p "$DATA/var/files/sub"
printf '{"pin":"ok","q1":"yes"}' > "$DATA/var/answers.json"
python3 -c 'import sys; open(sys.argv[1], "wb").write(bytes((i * 7) % 251 for i in range(1200000)))' "$DATA/var/files/scan.bin"
printf 'n' > "$DATA/var/files/sub/note.txt"
printf 'outside' > "$SANDBOX/outside.txt"
ln -s "$SANDBOX/outside.txt" "$DATA/var/leak"
ln -s "$SANDBOX" "$DATA/up"

echo "== ls =="
out="$(k2 app data ls pins)"
expect_has "root listing" "$out" "var/"
out="$(k2 app data ls pins /data/var)"
expect_has "  answers.json with its size" "$out" "23 B"
expect_has "  a link says it's never followed" "$out" "leak (link: never followed)"
out="$(k2 app data ls pins var/files --json)"
expect_eq "--json entries" "$(printf '%s' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["kind"], [(e["name"], e["type"], e["size"]) for e in v["entries"]])')" "dir [('scan.bin', 'file', 1200000), ('sub', 'dir', None)]"

echo "== get =="
mkdir -p "$SANDBOX/dl"
cd "$SANDBOX/dl"
out="$(k2 app data get pins var/answers.json)"
expect_has "default ./<name>" "$out" "saved var/answers.json (23 bytes) from App pins to ./answers.json"
expect_eq "  the bytes" "$(cat "$SANDBOX/dl/answers.json")" '{"pin":"ok","q1":"yes"}'
cd "$WS"
k2 app data get pins var/files/scan.bin --to "$SANDBOX/dl" >/dev/null
expect_eq "a file past one piece, byte for byte (--to a folder)" \
    "$(python3 -c 'import sys; print(open(sys.argv[1],"rb").read() == open(sys.argv[2],"rb").read())' "$DATA/var/files/scan.bin" "$SANDBOX/dl/scan.bin")" "True"
expect_eq "  no part file left" "$(ls -A "$SANDBOX/dl" | grep -c k2-part || true)" "0"
out="$(k2 app data get pins /data/var/answers.json --to "$SANDBOX/dl/renamed.json" --json)"
expect_eq "--to a file --json" "$(printf '%s' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["bytes"], v["to"].endswith("renamed.json"))')" "23 True"
expect_eq "- to stdout" "$(k2 app data get pins var/answers.json -)" '{"pin":"ok","q1":"yes"}'

echo "== refusals =="
for p in "../../outside.txt" "/etc/passwd" "/data/../x"; do
    rc="$(run "$SANDBOX/out" app data get pins "$p" --to "$SANDBOX/dl/x")"
    expect_eq "get $p exits 2" "$rc" "2"
    expect_has "  bad_request" "$(cat "$SANDBOX/out")" "bad_request"
done
for p in var/leak up/outside.txt; do
    rc="$(run "$SANDBOX/out" app data get pins "$p" --to "$SANDBOX/dl/x")"
    expect_eq "get $p (a symlink) exits 3" "$rc" "3"
    expect_has "  data_symlink" "$(cat "$SANDBOX/out")" "data_symlink"
done
expect_eq "  nothing saved and no part file" "$(ls -A "$SANDBOX/dl" | grep -c -e '^x$' -e k2-part || true)" "0"
rc="$(run "$SANDBOX/out" app data get pins var/nope.json --to "$SANDBOX/dl/x")"
expect_eq "a missing file exits 3" "$rc" "3"
expect_has "  data_not_found" "$(cat "$SANDBOX/out")" "data_not_found"

echo "== mv =="
out="$(k2 app data mv pins var/answers.json done/answers.json)"
expect_has "moved" "$out" "moved var/answers.json to done/answers.json in App pins"
[ -f "$DATA/done/answers.json" ] && ok "  it landed" || bad "  it landed"
printf 'old' > "$DATA/done/b.json"
rc="$(run "$SANDBOX/out" app data mv pins done/answers.json done/b.json)"
expect_eq "no overwrite without --force: exit 3" "$rc" "3"
expect_has "  data_exists" "$(cat "$SANDBOX/out")" "data_exists"
out="$(k2 app data mv pins done/answers.json done/b.json --force)"
expect_has "--force replaces" "$out" "replaced what was there"
rc="$(run "$SANDBOX/out" app data mv pins done/b.json var --force)"
expect_eq "never into a folder: exit 3" "$rc" "3"
rc="$(run "$SANDBOX/out" app data mv pins done/b.json up/stolen.json)"
expect_eq "through a symlink: exit 3" "$rc" "3"
[ ! -e "$SANDBOX/stolen.json" ] && ok "  nothing outside" || bad "  nothing outside"

echo "== rm =="
rc="$(run "$SANDBOX/out" app data rm pins var/files)"
expect_eq "a folder without --recursive: exit 3" "$rc" "3"
expect_has "  data_is_folder" "$(cat "$SANDBOX/out")" "data_is_folder"
out="$(k2 app data rm pins var/files --recursive)"
expect_has "--recursive" "$out" "removed folder"
[ ! -e "$DATA/var/files" ] && ok "  gone" || bad "  gone"
out="$(k2 app data rm pins var/leak --json)"
expect_eq "a link goes as a link" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["removed"])')" "link"
expect_eq "  its target stays" "$(cat "$SANDBOX/outside.txt")" "outside"

echo "== help =="
out="$(k2 app data --help)"
expect_has "help lists get" "$out" "k2 app data get <app-id> <path> [--to <local path> | -] [--json]"

expect_has "uninstall" "$(k2 app uninstall pins)" "App pins uninstalled"

echo
echo "app_data: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
