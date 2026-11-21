#!/usr/bin/env bash
# Loud harness for the 0.46.17 mail drafting CLI: `k2 mail draft <id> --to`,
# the idempotency key and its retry, `k2 mail draft delete`, `k2 mail link
# relink`, the inbox_busy / inbox_throttled exits (never retried) and the
# link list status words. A fake daemon answers from a script; no live
# daemon, no mail. Fail-loud: no skip-if-missing.
# Run with: bash tests/cli/mail_draft_keys_delete_relink.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_exit() { if [ "$3" = "$2" ]; then ok "$1 (exit $3)"; else bad "$1: expected exit $2, got $3"; fi; }
expect_in() { if printf '%s' "$2" | grep -qF -- "$3"; then ok "$1"; else bad "$1: '$3' not in: $2"; fi; }

WORKDIR="$(mktemp -d -t k2-mail-draft-keys-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

# The fake daemon: each request is appended to requests.jsonl as
# {method, path, body}; the answer for a path is the next line of
# script/<path with / as _> (status<TAB>json), the last line repeating.
python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
root = sys.argv[1]
os.makedirs(os.path.join(root, "script"), exist_ok=True)
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def answer(self, method):
        path = self.path.split("?", 1)[0]
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        with open(os.path.join(root, "requests.jsonl"), "a") as f:
            f.write(json.dumps({"method": method, "path": path, "body": json.loads(raw) if raw else None}) + "\n")
        name = os.path.join(root, "script", path.strip("/").replace("/", "_"))
        lines = open(name).read().splitlines() if os.path.exists(name) else ["200\t{\"ok\":true}"]
        # The answer index lives beside the script, so a reset clears it.
        cpath = name + ".count"
        i = int(open(cpath).read()) if os.path.exists(cpath) else 0
        with open(cpath, "w") as f:
            f.write(str(i + 1))
        status, body = lines[min(i, len(lines) - 1)].split("\t", 1)
        data = body.encode()
        self.send_response(int(status))
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_POST(self):
        self.answer("POST")
    def do_GET(self):
        self.answer("GET")
httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
trap 'kill $FAKE_PID 2>/dev/null || true; wait $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT
for _ in $(seq 1 100); do [ -f "$WORKDIR/port" ] && break; sleep 0.05; done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"
mkdir -p "$WORKDIR/script"

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")
reset() { rm -f "$WORKDIR/requests.jsonl" "$WORKDIR"/script/*; }
script() { printf '%s\n' "${@:2}" > "$WORKDIR/script/$1"; }
count() { grep -c "\"path\": \"$1\"" "$WORKDIR/requests.jsonl" 2>/dev/null || true; }
nth_body() { sed -n "${2}p" "$WORKDIR/requests.jsonl" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['body'].get('$1')))"; }
run() { set +e; out="$("${K2[@]}" "$K2_CLI" "$@" 2>&1)"; rc=$?; set -e; }

echo "== reply draft takes --to; every save carries a key =="
reset
script cli_mail_draft $'200\t{"ok":true,"address":"me@x.example","folder":"Drafts","id":"m_1","draftId":"d_00000000000000000000000000000001","hint":"draft saved"}'
run mail draft m_1 --to boss@example.com --cc cy@example.com --body hi
expect_exit "reply --to" 0 "$rc"
expect_in "reply --to POSTs to" "$(nth_body to 1)" "boss@example.com"
expect_in "a fresh key rides the save" "$(nth_body idempotencyKey 1)" "k2d_"
expect_in "the draft id is printed" "$out" "d_00000000000000000000000000000001"
run mail draft m_1 --to a@example.com --reply-all --body hi
expect_exit "--to with --reply-all" 2 "$rc"
run mail draft m_1 --subject s --body hi
expect_exit "--subject on a reply" 2 "$rc"
expect_in "subject hint" "$out" "not both"

echo "== a slow save is asked again with the SAME key, once it answers =="
reset
script cli_mail_draft \
    $'409\t{"ok":false,"error":{"code":"busy","hint":"still saving"}}' \
    $'200\t{"ok":true,"address":"me@x.example","draftId":"d_00000000000000000000000000000002","replayed":true,"hint":"draft d_00000000000000000000000000000002 was already saved"}'
run mail draft --to a@example.com --subject s --body hi --idempotency-key mykey-0001 --json
expect_exit "busy then replay" 0 "$rc"
[ "$(count /cli/mail/draft)" = "2" ] && ok "asked twice" || bad "asked $(count /cli/mail/draft) times"
expect_in "first key" "$(nth_body idempotencyKey 1)" "mykey-0001"
expect_in "same key again" "$(nth_body idempotencyKey 2)" "mykey-0001"
expect_in "replayed answer" "$out" '"replayed":true'

echo "== inbox_busy / inbox_throttled are final: no retry, clear exits =="
reset
script cli_mail_draft $'429\t{"ok":false,"error":{"code":"inbox_busy","hint":"4 sessions open","retry_after_s":5},"retry_after_s":5}'
run mail draft m_1 --body hi
expect_exit "inbox_busy" 2 "$rc"
expect_in "busy keeps retry_after_s" "$out" '"retry_after_s":5'
[ "$(count /cli/mail/draft)" = "1" ] && ok "busy not retried" || bad "busy retried ($(count /cli/mail/draft))"
reset
script cli_mail_attachments $'503\t{"ok":false,"error":{"code":"inbox_throttled","hint":"the provider is limiting this account","retry_after_s":300}}'
run mail attachments m_1
expect_exit "inbox_throttled" 3 "$rc"
expect_in "throttled keeps retry_after_s" "$out" '"retry_after_s":300'
[ "$(count /cli/mail/attachments)" = "1" ] && ok "throttled not retried" || bad "throttled retried"
reset
script cli_mail_attachments $'502\t{"ok":false,"error":{"code":"engine","hint":"FETCH failed — Safe to retry","retry":"safe"}}'
run mail attachments m_1
expect_exit "a failed fetch" 1 "$rc"
expect_in "says whether to retry" "$out" '"retry":"safe"'

echo "== draft delete =="
reset
script cli_mail_draft_delete $'200\t{"ok":true,"draftId":"d_00000000000000000000000000000003","draftStatus":"discarded","hint":"draft d_00000000000000000000000000000003 deleted from me@x.example (moved to Trash)"}'
run mail draft delete d_00000000000000000000000000000003
expect_exit "delete own" 0 "$rc"
expect_in "delete POSTs the id" "$(nth_body id 1)" "d_00000000000000000000000000000003"
expect_in "delete says where" "$out" "moved to Trash"
reset
script cli_mail_draft_delete $'403\t{"ok":false,"error":{"code":"not_your_draft","hint":"nothing was touched"}}'
run mail draft delete d_00000000000000000000000000000004
expect_exit "delete someone else's" 3 "$rc"
reset
run mail draft delete
expect_exit "delete without an id" 2 "$rc"
run mail draft delete d_1 -h
expect_exit "delete -h" 0 "$rc"
expect_in "delete -h prints help" "$out" "k2 mail draft delete <draft-id>"
[ -f "$WORKDIR/requests.jsonl" ] && bad "help sent a request" || ok "help sends nothing"

echo "== link relink and link list =="
reset
script cli_mail_link_oauth_start $'403\t{"ok":false,"error":{"code":"owner_only","hint":"owner surface"}}'
run mail link relink me@gmail.com
expect_exit "an agent's relink" 3 "$rc"
expect_in "relink asks for inbox + relink" "$(nth_body relink 1)" "true"
expect_in "relink names the inbox" "$(nth_body inbox 1)" "me@gmail.com"
[ "$(nth_body features 1)" = "null" ] && ok "relink sends no features (keeps the link's)" || bad "relink sent features"
run mail link relink me@gmail.com --features calendar
expect_exit "relink --features" 2 "$rc"
run mail link relink
expect_exit "relink without an address" 2 "$rc"
reset
script cli_mail_inboxes $'200\t{"ok":true,"inboxes":[{"address":"a@gmail.com","source":"linked","status":"connected","needsRelink":true,"relinkReason":"credential_refused","host":"imap.gmail.com","tls":"implicit-tls"},{"address":"b@gmail.com","source":"linked","status":"connected","needsRelink":true,"relinkReason":"mail_access_not_granted","host":"imap.gmail.com","tls":"implicit-tls"},{"address":"c@gmail.com","source":"linked","status":"connected","needsRelink":false,"relinkReason":"throttled","host":"imap.gmail.com","tls":"implicit-tls"}]}'
run mail link list
expect_exit "link list" 0 "$rc"
expect_in "needs re-link" "$out" "needs re-link"
expect_in "mail access not granted" "$out" "mail access not granted"
expect_in "throttled" "$out" "throttled (wait)"
expect_in "relink hint" "$out" "k2 mail link relink a@gmail.com"
run mail link relink me@gmail.com -h
expect_exit "relink -h" 0 "$rc"
[ "$(count /cli/mail/link/oauth/start)" = "0" ] && ok "relink -h sends nothing" || bad "relink -h sent a request"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
