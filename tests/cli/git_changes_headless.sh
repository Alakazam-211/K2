#!/usr/bin/env bash
# Changes tab v1 (C1) — headless curl test of the git routes and the
# `git_changed` event against a foreground sandbox daemon (temp HOME, no
# client attached). Contract: .k2/prds/prd-changes-tab-v1.md.
#
#   cargo build -p k2-daemon && bash tests/cli/git_changes_headless.sh
#
# Discards go to $K2_GIT_TRASH_DIR (a temp dir), never the OS Trash.
# Every check fails loudly (exit 1) with the response body.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
source "$SCRIPT_DIR/_sandbox_daemon.sh"

# Git under test: no user/system config (hooks, signing, templates).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
GITC=(git -c user.name=K2-Test -c user.email=k2-test@example.com -c init.defaultBranch=main -c commit.gpgsign=false)

WORK="$(mktemp -d -t k2-gitchg-work-XXXXXX)"
export K2_GIT_TRASH_DIR="$WORK/trash"
mkdir -p "$K2_GIT_TRASH_DIR"

sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$WORK"' EXIT

BASE="http://127.0.0.1:${K2SO_PORT}"
TOK="$K2SO_TOKEN"
REPO="$WORK/repo"
mkdir -p "$REPO"
REPO="$(cd "$REPO" && pwd -P)"
pass=0

fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2 || true; exit 1; }
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
urlq() { python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
# jcheck <json> <python-expr over j>  — the expression must be truthy.
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
# req METHOD route [json-body] → sets STATUS and BODY; echoes the curl.
req() {
    local method="$1" route="$2" data="${3:-}" out
    echo "  \$ curl -X $method '$BASE$route&token=…'${data:+ -d '$data'}"
    if [ -n "$data" ]; then
        out="$(curl -s -w $'\n%{http_code}' -X "$method" -H 'Content-Type: application/json' -d "$data" "$BASE$route&token=$TOK")"
    else
        out="$(curl -s -w $'\n%{http_code}' -X "$method" "$BASE$route&token=$TOK")"
    fi
    STATUS="${out##*$'\n'}"
    BODY="${out%$'\n'*}"
}
expect() { # status python-expr label
    [ "$STATUS" = "$1" ] || fail "$3: status $STATUS (want $1): $BODY"
    jcheck "$BODY" "$2" || fail "$3: $2 is false for: $BODY"
    ok "$3"
}

echo "== repo + workspace =="
"${GITC[@]}" -C "$REPO" init -q
printf 'one\ntwo\n' >"$REPO/a.txt"
"${GITC[@]}" -C "$REPO" add -A
"${GITC[@]}" -C "$REPO" commit -q -m init
"${GITC[@]}" -C "$REPO" config user.name K2-Test
"${GITC[@]}" -C "$REPO" config user.email k2-test@example.com
# K2's own per-workspace state is not part of the change set under test.
printf '.k2/\n.claude/\n' >>"$REPO/.git/info/exclude"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$REPO")" \
    "$BASE/cli/projects/add-from-path?token=$TOK")"
case "$reg" in *"$REPO"*) ok "workspace registered" ;; *) fail "register: $reg" ;; esac
P="$(urlq "$REPO")"

echo "== live event socket =="
WSLOG="$WORK/events.jsonl"
python3 - "$K2SO_PORT" "$TOK" "$WSLOG" <<'PY' &
import base64, os, socket, struct, sys, time
port, tok, out = int(sys.argv[1]), sys.argv[2], sys.argv[3]
s = socket.create_connection(("127.0.0.1", port))
key = base64.b64encode(os.urandom(16)).decode()
s.sendall((f"GET /cli/sessions/events?token={tok} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\n"
           f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
buf = b""
while b"\r\n\r\n" not in buf:
    buf += s.recv(4096)
head, buf = buf.split(b"\r\n\r\n", 1)
assert b" 101 " in head.split(b"\r\n")[0], head
s.settimeout(0.5)
deadline = time.time() + 60
def need(n):
    global buf
    while len(buf) < n:
        if time.time() > deadline:
            raise TimeoutError
        try:
            chunk = s.recv(65536)
        except socket.timeout:
            continue
        if not chunk:
            raise EOFError
        buf += chunk
with open(out, "a") as f:
    try:
        while True:
            need(2)
            op, ln = buf[0] & 0x0F, buf[1] & 0x7F
            off = 2
            if ln == 126:
                need(4); ln = struct.unpack(">H", buf[2:4])[0]; off = 4
            elif ln == 127:
                need(10); ln = struct.unpack(">Q", buf[2:10])[0]; off = 10
            need(off + ln)
            payload, buf = buf[off:off + ln], buf[off + ln:]
            if op == 1:
                f.write(payload.decode() + "\n"); f.flush()
            elif op == 9:  # ping → masked pong
                mask = os.urandom(4)
                s.sendall(bytes([0x8A, 0x80 | len(payload)]) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))
            elif op == 8:
                break
    except (TimeoutError, EOFError):
        pass
PY
WS_PID=$!
sleep 2.5 # watcher picks up the new project root (resync) + socket hello

echo "== GET /cli/git/snapshot =="
req GET "/cli/git/snapshot?path=$P"
expect 200 'j["isRepo"] and j["branch"]=="main" and j["counts"]["total"]==0 and j["limit"]==1000 and ":" in j["rev"]' "clean snapshot"
REV0="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["rev"])' "$BODY")"

printf 'one\nTWO\nthree\n' >"$REPO/a.txt"
printf 'new\n' >"$REPO/new.txt"
req GET "/cli/git/snapshot?path=$P"
expect 200 '[(f["group"],f["path"],f["status"],f["additions"],f["deletions"]) for f in j["files"]]==[("unstaged","a.txt","M",2,1),("untracked","new.txt","U",1,0)]' "dirty snapshot: unstaged M (+2 -1), untracked U"

req GET "/cli/git/snapshot?path=$(urlq "$WORK/not-a-repo")"
expect 200 'j["isRepo"] is False' "non-repo path → isRepo:false"
req GET "/cli/git/snapshot?path=$P&base=no-such-ref"
expect 400 'j["code"]=="bad_ref"' "bad base → 400 bad_ref"
req GET "/cli/git/snapshot?path=$P&base=main"
expect 200 'j["base"]["ref"]=="main" and any(f["group"]=="base" and f["path"]=="a.txt" for f in j["files"])' "snapshot against base=main"

echo "== GET /cli/git/diff-file =="
req GET "/cli/git/diff-file?path=$P&file_path=a.txt&sides=1"
expect 200 'j["mode"]=="worktree" and j["old"]["label"]=="HEAD" and j["old"]["text"]=="one\ntwo\n" and j["new"]["text"]=="one\nTWO\nthree\n" and (j["additions"],j["deletions"])==(2,1) and not j["binary"] and not j["tooLarge"]' "both sides HEAD→worktree"
req GET "/cli/git/diff-file?path=$P&file_path=a.txt"
expect 200 'isinstance(j, list) and len(j)>=1' "without sides=1: unchanged hunks array"
req GET "/cli/git/diff-file?path=$P&file_path=..%2F..%2Fetc%2Fpasswd&sides=1"
expect 400 'j["code"]=="bad_path"' "traversal → 400 bad_path"
head -c 4096 /dev/urandom >"$REPO/blob.bin"; printf '\0' >>"$REPO/blob.bin"
req GET "/cli/git/diff-file?path=$P&file_path=blob.bin&sides=1"
expect 200 'j["binary"] and j["new"]["text"] is None' "binary file → binary:true, no text"
rm -f "$REPO/blob.bin"

echo "== POST stage / unstage =="
req POST "/cli/git/stage?x=1" "{\"path\":\"$REPO\",\"filePath\":\"a.txt\"}"
expect 200 'j["success"]' "stage a.txt"
req GET "/cli/git/diff-file?path=$P&file_path=a.txt&sides=1&staged=1"
expect 200 'j["mode"]=="staged" and j["new"]["label"]=="INDEX" and j["new"]["text"]=="one\nTWO\nthree\n"' "staged diff HEAD→INDEX"
req POST "/cli/git/stage?x=1" "{\"path\":\"$REPO\",\"filePath\":\"../escape\"}"
expect 400 'j["code"]=="bad_path"' "stage traversal → 400 bad_path"
req POST "/cli/git/unstage?x=1" "{\"path\":\"$REPO\",\"filePath\":\"a.txt\"}"
expect 200 'j["success"]' "unstage a.txt"

echo "== POST /cli/git/discard =="
req GET "/cli/git/discard?path=$P"
[ "$STATUS" = 405 ] || fail "GET discard: $STATUS $BODY"; ok "GET discard → 405"
req POST "/cli/git/discard?x=1" "{\"path\":\"$REPO\",\"files\":[\"a.txt\",\"../../etc/hosts\"]}"
expect 400 'j["code"]=="bad_path"' "discard traversal → 400, nothing touched"
grep -q TWO "$REPO/a.txt" || fail "a.txt changed by a refused discard"
req POST "/cli/git/discard?x=1" "{\"path\":\"$REPO\",\"files\":[\"a.txt\",\"new.txt\"]}"
expect 200 'j["restored"]==["a.txt"] and sorted(j["trashed"])==["a.txt","new.txt"] and j["failed"]==[]' "discard: a.txt restored, both copies to Trash"
[ "$(cat "$REPO/a.txt")" = "$(printf 'one\ntwo')" ] || fail "a.txt not restored"
[ ! -e "$REPO/new.txt" ] || fail "new.txt still there"
ls "$K2_GIT_TRASH_DIR" | grep -q '^new.txt-' || fail "new.txt not in the trash dir: $(ls "$K2_GIT_TRASH_DIR")"
grep -rq TWO "$K2_GIT_TRASH_DIR" || fail "the discarded edit is not in the trash dir"
ok "trash dir holds new.txt and the discarded a.txt edit"

echo "== POST commit =="
printf 'c\n' >"$REPO/c.txt"
req POST "/cli/git/stage-all?x=1" "{\"path\":\"$REPO\"}"
expect 200 'j["success"]' "stage-all"
req POST "/cli/git/commit?x=1" "{\"path\":\"$REPO\",\"message\":\"from the tab\"}"
expect 200 'len(j["oid"])==40' "commit"
req GET "/cli/git/snapshot?path=$P"
expect 200 'j["counts"]["total"]==0 and j["rev"]!="'"$REV0"'"' "clean again, rev moved"

echo "== auth =="
out="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/git/snapshot?path=$P&token=k2skn_not_a_login")"
[ "$out" = 403 ] || fail "app-pass token on snapshot: $out"; ok "app pass → 403"
out="$(curl -s -o /dev/null -w '%{http_code}' -X POST -d '{}' "$BASE/cli/git/discard?token=bogus")"
[ "$out" = 403 ] || fail "bogus token on discard: $out"; ok "bad token on discard → 403"

echo "== git_changed from the watcher (no route involved) =="
before="$(grep -c '"git_changed"' "$WSLOG" || true)"
printf 'd\n' >"$REPO/d.txt"
"${GITC[@]}" -C "$REPO" add d.txt
"${GITC[@]}" -C "$REPO" commit -q -m external
sleep 2.5
after="$(grep -c '"git_changed"' "$WSLOG" || true)"
[ "$after" -gt "$before" ] || fail "no git_changed after an external git add/commit (log: $(cat "$WSLOG"))"
grep '"git_changed"' "$WSLOG" | tail -1 | python3 -c '
import json, sys
e = json.loads(sys.stdin.read())
assert e["workspacePath"] == sys.argv[1], e
assert e["repoRoot"] and e["rev"], e
' "$REPO" || fail "git_changed frame shape: $(grep git_changed "$WSLOG" | tail -1)"
ok "git_changed {workspacePath, repoRoot, rev} over /cli/sessions/events ($((after - before)) frame(s) for add+commit)"
total="$(grep -c '"git_changed"' "$WSLOG" || true)"
echo "  (git_changed frames seen in total: $total)"
kill "$WS_PID" 2>/dev/null || true

echo "PASS: $pass checks"
