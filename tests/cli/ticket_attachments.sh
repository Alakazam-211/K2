#!/usr/bin/env bash
# Ticket files (0.46.6, ticket-attachments-v1; prd-ticket-attachments-v1
# TK12/TK13, TA3/TA21/TA22). Exercises the real `cli/k2` against a stub
# daemon that records POST bodies:
#   1. `k2 tickets ask … --file a --file b` embeds `attachments` as a JSON
#      ARRAY in the create body (never a string) and prints [file] lines.
#   2. `k2 tickets comment <id> --file x` works without text; with text too.
#   3. `k2 tickets resolve <id> --answered "…" --file x` carries the files;
#      `resolve <id> --file x` (no --answered) is a usage error, exit 2,
#      nothing posted.
#   4. More than 10 files → exit 2, nothing posted.
#   5. An old daemon (an answer without `attachments`) → exit 1
#      server_too_old, never a silent drop.
#   6. `k2 tickets show` prints each message's files under it, absolute
#      (projectPath + path), with changed/gone notes.
#   7. `ask --wait`: stdout is the BARE answer; the files on the answering
#      message are one stderr line; `--json` carries them absolute.
#   8. Help pages teach --file.
#
# No daemon build needed. Never touches ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-tkt-files-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/out" "$WORK/docs"
printf 'png' > "$WORK/out/a.png"
printf 'png' > "$WORK/out/b.png"
printf '# plan\n' > "$WORK/docs/plan.md"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
counter = {"n": 0}
TICKET = "fb-stub-0000000000000001"

def mode():
    try:
        return open(os.path.join(work, "mode")).read().strip()
    except OSError:
        return "new"

def views(b):
    out = []
    for a in b.get("attachments") or []:
        p = a.get("path") or a.get("relPath")
        rel = os.path.relpath(p, work) if os.path.isabs(p) else p
        v = {"kind": "file", "path": rel, "name": os.path.basename(rel), "size": 3,
             "mime": "image/png", "modifiedAt": 1, "state": "ok", "canDownload": True}
        for k in ("line", "lineEnd"):
            if a.get(k):
                v[k] = a[k]
        out.append(v)
    return out

class H(BaseHTTPRequestHandler):
    def _send(self, resp):
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        path = self.path.split("?")[0]
        counter["n"] += 1
        b = json.loads(body or b"{}")
        with open(os.path.join(work, "post-%d.json" % counter["n"]), "wb") as f:
            f.write(json.dumps({"path": path, "body": b}).encode())
        old = mode() == "old"
        if path == "/cli/feedback/create":
            resp = {"ok": True, "id": TICKET, "title": b.get("title"), "kind": "question",
                    "priority": 3, "status": "waiting", "warnings": []}
        elif path == "/cli/feedback/comment":
            resp = {"ok": True, "id": TICKET, "commentId": "c-1", "author": b.get("author")}
        elif path == "/cli/feedback/resolve":
            resp = {"ok": True, "id": TICKET, "status": b.get("status", "resolved"), "answer": b.get("answer")}
        else:
            resp = {"error": {"code": "not_found", "hint": "stub: unknown route"}}
        if not old and "error" not in resp and path != "/cli/feedback/resolve" or (not old and b.get("status") == "answered"):
            resp["attachments"] = views(b)
        self._send(resp)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/cli/feedback/show":
            f = lambda rel, state="ok": {"kind": "file", "path": rel, "name": os.path.basename(rel), "size": 2048,
                                         "mime": "image/png", "modifiedAt": 1, "state": state, "canDownload": True}
            self._send({
                "ok": True, "id": TICKET, "status": "answered", "answer": "B",
                "agentName": "scout", "projectPath": work, "title": "Which logo?",
                "comments": [
                    {"id": "c-0", "author": "scout", "body": "Which logo?", "at": 1,
                     "attachments": [f("out/a.png"), f("out/b.png", "changed")]},
                    {"id": "c-1", "author": "rosson", "body": "B", "at": 2,
                     "attachments": [f("docs/shot.png", "gone")]},
                ],
            })
        else:
            self._send({"error": {"code": "not_found", "hint": "stub: unknown route"}})

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"

for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.1
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub daemon never wrote its port" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0
pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
contains() { if grep -Fq -- "$3" <<<"$2"; then pass "$1"; else fail "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

k2() {
    (cd "$WORK" && env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN \
        HOME="$WORK" K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="stub-token" \
        K2_PROJECT_PATH="$WORK" K2_AGENT_NAME="scout" \
        "$K2_CLI" "$@")
}

posts_count() { find "$WORK" -name 'post-*.json' | wc -l | tr -d ' '; }
last_post() { python3 -c 'import json,sys,glob
files = sorted(glob.glob(sys.argv[1] + "/post-*.json"), key=lambda p: int(p.rsplit("-",1)[1].split(".")[0]))
print(open(files[-1]).read())' "$WORK"; }
jq_py() { python3 -c "import json,sys; d=json.loads(sys.argv[1]); print($2)" "$1"; }

echo "== 1. ask --file =="
out="$(k2 tickets ask "Which logo?" --options "A,B" --assign owner --file out/a.png --file out/b.png 2>&1)"
post="$(last_post)"
contains "ask: create posted" "$post" '"/cli/feedback/create"'
[ "$(jq_py "$post" 'type(d["body"]["attachments"]).__name__')" = "list" ] && pass "ask: attachments is a JSON array" || fail "ask: attachments not an array: $post"
[ "$(jq_py "$post" 'd["body"]["attachments"][0]["path"]')" = "$WORK/out/a.png" ] && pass "ask: an existing path is sent absolute" || fail "ask: path: $post"
contains "ask: prints the files" "$out" "[file] out/a.png (3 B)"

echo "== 2. comment --file, text optional =="
before="$(posts_count)"
out="$(k2 tickets comment fb-stub --file docs/plan.md:1 2>&1)"
post="$(last_post)"
[ "$(posts_count)" -gt "$before" ] && pass "comment: posted without text" || fail "comment: nothing posted ($out)"
[ "$(jq_py "$post" 'd["body"]["body"]')" = "" ] && pass "comment: empty body" || fail "comment body: $post"
[ "$(jq_py "$post" 'd["body"]["attachments"][0]["line"]')" = "1" ] && pass "comment: line rides" || fail "comment line: $post"
contains "comment: prints the file" "$out" "[file] docs/plan.md:1"
out="$(k2 tickets comment fb-stub "see" --file out/a.png 2>&1)"
[ "$(jq_py "$(last_post)" 'd["body"]["body"]')" = "see" ] && pass "comment: text and file" || fail "comment text: $(last_post)"
rc=0; out="$(k2 tickets comment fb-stub 2>&1)" || rc=$?
[ "$rc" = 2 ] && pass "comment: no text and no file is exit 2" || fail "comment empty: rc=$rc $out"

echo "== 3. resolve --answered --file =="
k2 tickets resolve fb-stub --answered "Shipped A" --file docs/plan.md >/dev/null 2>&1
post="$(last_post)"
[ "$(jq_py "$post" 'd["body"]["status"] + "|" + d["body"]["attachments"][0]["path"]')" = "answered|$WORK/docs/plan.md" ] \
    && pass "resolve --answered carries the files" || fail "resolve: $post"
before="$(posts_count)"
rc=0; out="$(k2 tickets resolve fb-stub --file docs/plan.md 2>&1)" || rc=$?
[ "$rc" = 2 ] && pass "resolve --file without --answered is exit 2" || fail "resolve --file: rc=$rc $out"
contains "resolve --file: hint" "$out" "files ride a message"
[ "$(posts_count)" = "$before" ] && pass "resolve --file: nothing posted" || fail "resolve --file posted"

echo "== 4. more than 10 =="
args=(); for i in $(seq 1 11); do args+=(--file "f$i.md"); done
before="$(posts_count)"
rc=0; out="$(k2 tickets comment fb-stub "x" "${args[@]}" 2>&1)" || rc=$?
[ "$rc" = 2 ] && pass "11 files: exit 2" || fail "11 files: rc=$rc $out"
contains "11 files: hint" "$out" "at most 10 files per message"
rc=0; out="$(k2 tickets ask "t" "${args[@]}" 2>&1)" || rc=$?
[ "$rc" = 2 ] && pass "ask 11 files: exit 2" || fail "ask 11 files: rc=$rc"
[ "$(posts_count)" = "$before" ] && pass "11 files: nothing posted" || fail "11 files posted"

echo "== 5. an old daemon fails loudly =="
echo old > "$WORK/mode"
for cmd in "tickets comment fb-stub x --file out/a.png" "tickets ask t --assign owner --file out/a.png" "tickets resolve fb-stub --answered done --file out/a.png"; do
    rc=0
    # shellcheck disable=SC2086
    err="$(k2 $cmd 2>&1 >/dev/null)" || rc=$?
    [ "$rc" = 1 ] && pass "old daemon: exit 1 ($cmd)" || fail "old daemon: rc=$rc ($cmd) $err"
    contains "old daemon: server_too_old ($cmd)" "$err" '"code": "server_too_old"'
done
rc=0; k2 tickets comment fb-stub "no files" >/dev/null 2>&1 || rc=$?
[ "$rc" = 0 ] && pass "old daemon: a comment without files still works" || fail "old daemon plain comment rc=$rc"
echo new > "$WORK/mode"

echo "== 6. show prints files =="
out="$(k2 tickets show fb-stub 2>&1)"
contains "show: absolute path + size" "$out" "[file] $WORK/out/a.png (2.0 KB)"
contains "show: changed" "$out" "[file] $WORK/out/b.png (2.0 KB) (changed since sent)"
contains "show: gone" "$out" "[file] $WORK/docs/shot.png (2.0 KB) (gone)"
out="$(k2 tickets show fb-stub --json)"
[ "$(jq_py "$out" 'd["comments"][0]["id"]')" = "c-0" ] && pass "show --json: comment ids pass through" || fail "show --json: $out"

echo "== 7. ask --wait =="
stdout="$(k2 tickets ask "Which?" --assign owner --wait 2>"$WORK/wait.err")"
[ "$stdout" = "B" ] && pass "--wait: stdout is the bare answer" || fail "--wait stdout: $(printf %q "$stdout")"
contains "--wait: stderr note" "$(cat "$WORK/wait.err")" "1 file attached: k2 tickets show fb-stub-"
out="$(k2 tickets ask "Which?" --assign owner --wait --json 2>/dev/null)"
[ "$(jq_py "$out" 'd["answer"] + "|" + d["attachments"][0]["absPath"]')" = "B|$WORK/docs/shot.png" ] \
    && pass "--wait --json: attachments absolute" || fail "--wait --json: $out"

echo "== 8. help =="
contains "ask help" "$(k2 tickets ask --help 2>&1)" "--file <path[:line[-end]]>"
contains "comment help" "$(k2 tickets comment --help 2>&1)" "Optional with --file"
contains "resolve help" "$(k2 tickets resolve --help 2>&1)" "files ride a message"
contains "show help" "$(k2 tickets show --help 2>&1)" "(changed since sent)"
contains "study app-tickets" "$(k2 study app-tickets 2>&1)" "GET  /cli/feedback/attachment"
contains "study feedback-loop" "$(k2 study feedback-loop 2>&1)" "Attached: <abs path>"

echo
echo "ticket_attachments.sh: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
