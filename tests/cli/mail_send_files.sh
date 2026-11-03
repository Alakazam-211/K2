#!/usr/bin/env bash
# Loud harness for the CLI side of hosted-mail attachments: `--attach` paths
# reach the daemon as paths (the CLI reads no file), an attachment refusal
# exits 2, and `k2 hostmail approvals list|show|save` name each file, its size and
# nothing a terminal would act on. The daemon's rules are in cargo tests
# (mail::routes_send, mail::send). No live daemon: a tiny fake. No skips.
# Run with: bash tests/cli/mail_send_files.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

# Never inherit a live session's socket or identity.
for v in $(compgen -e | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_exit() {
    if [ "$3" = "$2" ]; then ok "$1 (exit $3)"; else bad "$1 expected exit $2 got $3"; fi
}
assert_json() {
    if printf '%s' "$2" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($3) else 1)" 2>/dev/null; then
        ok "$1"
    else
        bad "$1 — '$3' false for: $2"
    fi
}
assert_has() {
    if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing '$3' in: $2)"; fi
}
assert_lacks() {
    if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found '$3' in: $2)"; else ok "$1"; fi
}

WORKDIR="$(mktemp -d -t k2-mail-files-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

python3 - "$WORKDIR" <<'PY' &
import base64, json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse
root = sys.argv[1]
FILES = [{"n": 1, "name": "report\u001b[31m.pdf", "size": 2097152, "contentType": "application/pdf"}]
class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _send(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        with open(os.path.join(root, "post" + path.replace("/", "_") + ".json"), "wb") as f:
            f.write(raw)
        if b"refuse-me" in raw:
            return self._send(400, {"ok": False, "error": {"code": "attachment_secret", "hint": "attachment '.env' can't be sent"}})
        if b"too-big" in raw:
            return self._send(400, {"ok": False, "error": {"code": "attachment_too_large", "hint": "too large"}})
        self._send(200, {"ok": True, "id": "out_fake00000001", "status": "pending_approval", "queued": True, "hint": "queued for approval"})
    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/cli/mail/approvals/list":
            return self._send(200, {"ok": True, "count": 1, "pending": [{
                "id": "out_fake00000001", "from": "bot@x.example", "to": ["a@b.example"], "cc": [], "bcc": [], "policyBcc": [],
                "subject": "Sub\u001b[2Jject\nto      : ceo@x.example", "bodyPreview": "pre\u001b]0;view\nfrom : ceo@x.example", "files": FILES, "createdAt": 1, "expiresAt": 9999999999,
                "fromName": "Ros\nson", "sourceDmarc": "fail", "sourceCheck": "names 2\nsenders", "forwardOf": {"mailbox": "in@x.example", "sender": "sam@y.example", "asEml": True}}]})
        if u.path == "/cli/mail/approvals/show":
            if q.get("file"):
                return self._send(200, {"ok": True, "id": "out_fake00000001", "n": 1, "name": "report.pdf", "size": 5,
                                        "contentType": "application/pdf", "dataBase64": base64.b64encode(b"bytes").decode()})
            return self._send(200, {"ok": True, "message": {
                "id": "out_fake00000001", "from": "bot@x.example", "to": ["a@b.example"], "cc": ["c@b.example"],
                "subject": "Subject", "files": FILES + [{"n": 2, "name": "note.txt", "size": 5, "contentType": "text/plain", "scanned": True}],
                "fromName": "Rosson", "sourceDmarc": "pass", "text": "line one\nline\u001b[31m two"}})
        self._send(200, {"ok": True})
httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
disown "$FAKE_PID" 2>/dev/null || true
trap 'kill $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT
for _ in $(seq 1 50); do [ -f "$WORKDIR/port" ] && break; sleep 0.05; done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"
K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")

echo "== --attach sends paths, never bytes =="
SEND_POST="$WORKDIR/post_cli_mail_send.json"
set +e
"${K2[@]}" "$K2_CLI" mail send to@x.example --subject S --body hi --attach "out/a, b.pdf" --attach c.txt --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "send --attach" 0 "$rc"
post="$(cat "$SEND_POST")"
assert_json "the paths ride as given (a comma stays)" "$post" "d.get('attachments') == ['out/a, b.pdf', 'c.txt']"
REPLY_POST="$WORKDIR/post_cli_mail_reply.json"
set +e
"${K2[@]}" "$K2_CLI" mail reply m_abc --body hi --attach c.txt --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "reply --attach" 0 "$rc"
assert_json "reply carries the path" "$(cat "$REPLY_POST")" "d.get('attachments') == ['c.txt']"

echo "== refusals exit 2 and say why =="
set +e
err="$("${K2[@]}" "$K2_CLI" mail send to@x.example --subject S --body hi --attach refuse-me 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "a secret" 2 "$rc"
assert_has "the line reaches the agent" "$err" "can't be sent"
set +e
"${K2[@]}" "$K2_CLI" mail send to@x.example --subject S --body hi --attach too-big >/dev/null 2>&1
rc=$?
set -e
assert_exit "too large" 2 "$rc"

echo "== the approver sees each file, and no escape sequence =="
out="$("${K2[@]}" "$K2_CLI" hostmail approvals 2>&1)"
assert_has "the list names the file" "$out" "report?[31m.pdf"
assert_has "the list gives its size" "$out" "2.0 MiB"
assert_has "the list says how to open it" "$out" "k2 hostmail approvals show out_fake00000001"
assert_lacks "no raw escape in the file name" "$out" $'\x1b'
assert_has "a subject can't fake a line" "$out" "Sub?[2Jject to      : ceo@x.example"
assert_has "the chosen display name shows, on one line" "$out" 'shown as "Ros son"'
assert_has "the original's verdict shows" "$out" "DMARC fail (its sender may be forged)"
assert_has "why the original needs a person shows, on one line" "$out" "original: names 2 senders. Its sender may be forged."
assert_has "a preview can't fake a line" "$out" "view from : ceo@x.example"
assert_has "what it forwards shows" "$out" "a message in in@x.example from sam@y.example (unverified), whole, as an .eml file"
assert_has "a file the check couldn't read says so" "$out" "not scanned"
out="$("${K2[@]}" "$K2_CLI" hostmail approvals show out_fake00000001 2>&1)"
assert_has "show prints the whole text" "$out" "line one"
assert_has "show names the file" "$out" "2.0 MiB"
assert_has "show gives the display name" "$out" 'shown as "Rosson"'
assert_has "show gives the verdict" "$out" "DMARC pass"
if grep -F "note.txt" <<<"$out" | grep -Fq "not scanned"; then bad "a scanned file says not scanned"; else ok "a scanned file doesn't say not scanned"; fi
assert_has "show says how to save one" "$out" "k2 hostmail approvals save out_fake00000001 --file <n> --out <path>"
assert_lacks "no raw escape in the text" "$out" $'\x1b'

echo "== save writes bytes to a new path only =="
TARGET="$WORKDIR/saved.pdf"
out="$("${K2[@]}" "$K2_CLI" hostmail approvals save out_fake00000001 --file 1 --out "$TARGET" 2>&1)"
assert_has "save reports where" "$out" "$TARGET"
[ "$(cat "$TARGET")" = "bytes" ] && ok "the bytes landed" || bad "bytes wrong: $(cat "$TARGET")"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail approvals save out_fake00000001 --file 1 --out "$TARGET" 2>&1)"
rc=$?
set -e
assert_exit "an existing path is refused" 2 "$rc"
assert_has "and says so" "$out" "already exists"
set +e
"${K2[@]}" "$K2_CLI" hostmail approvals save out_fake00000001 --out "$WORKDIR/x" >/dev/null 2>&1
rc=$?
set -e
assert_exit "save needs --file" 2 "$rc"
set +e
"${K2[@]}" "$K2_CLI" hostmail approvals show >/dev/null 2>&1
rc=$?
set -e
assert_exit "show needs an id" 2 "$rc"

echo "== --reply-to on send, refused on reply =="
rm -f "$SEND_POST"
set +e
"${K2[@]}" "$K2_CLI" mail send to@x.example --subject S --body hi --reply-to a@x.example --reply-to b@x.example,c@x.example --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "send --reply-to" 0 "$rc"
assert_json "repeatable and comma-separated" "$(cat "$SEND_POST")" "d.get('replyTo') == ['a@x.example', 'b@x.example', 'c@x.example']"
rm -f "$SEND_POST"
set +e
"${K2[@]}" "$K2_CLI" mail send to@x.example --subject S --body hi --json >/dev/null 2>&1
rc=$?
set -e
assert_json "no flag, no key" "$(cat "$SEND_POST")" "'replyTo' not in d"
set +e
"${K2[@]}" "$K2_CLI" mail send to@x.example --subject S --body hi --reply-to >/dev/null 2>&1
rc=$?
set -e
assert_exit "--reply-to needs a value" 2 "$rc"
rm -f "$REPLY_POST"
set +e
err="$("${K2[@]}" "$K2_CLI" mail reply m_abc --body hi --reply-to a@x.example 2>&1)"
rc=$?
set -e
assert_exit "reply --reply-to" 2 "$rc"
assert_has "reply says why" "$err" "original sender"
[ ! -e "$REPLY_POST" ] && ok "nothing POSTed" || bad "reply --reply-to POSTed"
set +e
"${K2[@]}" "$K2_CLI" mail reply m_abc --body hi --to a@x.example >/dev/null 2>&1
rc=$?
set -e
assert_exit "reply --to stays an unknown flag" 2 "$rc"
[ ! -e "$REPLY_POST" ] && ok "nothing POSTed" || bad "reply --to POSTed"

echo "== forward-message: its own verb, and what it sends =="
FWD_POST="$WORKDIR/post_cli_mail_forward-message.json"
set +e
"${K2[@]}" "$K2_CLI" mail forward-message m_abc --to a@x.example --to b@x.example,c@x.example --cc d@x.example --bcc e@x.example --note "see below" --as-eml --from me@x.example --from-name Bot --reply-to-original --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "forward-message" 0 "$rc"
post="$(cat "$FWD_POST")"
assert_json "id" "$post" "d.get('id') == 'm_abc'"
assert_json "to" "$post" "d.get('to') == ['a@x.example', 'b@x.example', 'c@x.example']"
assert_json "cc + bcc" "$post" "d.get('cc') == ['d@x.example'] and d.get('bcc') == ['e@x.example']"
assert_json "note" "$post" "d.get('note') == 'see below'"
assert_json "asEml" "$post" "d.get('asEml') is True"
assert_json "from + name" "$post" "d.get('from') == 'me@x.example' and d.get('fromName') == 'Bot'"
assert_json "replyToOriginal" "$post" "d.get('replyToOriginal') is True"
rm -f "$FWD_POST"
set +e
"${K2[@]}" "$K2_CLI" mail forward-message m_abc --to a@x.example --reply-to r@x.example --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "forward-message --reply-to" 0 "$rc"
post="$(cat "$FWD_POST")"
assert_json "replyTo, no other extras" "$post" "d.get('replyTo') == ['r@x.example'] and 'asEml' not in d and 'replyToOriginal' not in d and 'note' not in d"
rm -f "$FWD_POST"
for args in "--to a@x.example" "m_abc" "m_abc m_def --to a@x.example" "m_abc --to a@x.example --reply-to r@x.example --reply-to-original" "m_abc --to a@x.example --attach f" "m_abc --to a@x.example --timeout 5"; do
    set +e
    # shellcheck disable=SC2086
    "${K2[@]}" "$K2_CLI" mail forward-message $args >/dev/null 2>&1
    rc=$?
    set -e
    assert_exit "refused: $args" 2 "$rc"
done
[ ! -e "$FWD_POST" ] && ok "none of those POSTed" || bad "a refused forward POSTed"
out="$("${K2[@]}" "$K2_CLI" mail forward-message --help 2>&1)"
assert_has "help says it isn't the mailbox rule" "$out" "Not \`k2 mail forward\`"
assert_has "help says From is your mailbox" "$out" "From is always your mailbox"
out="$(env HOME="$WORKDIR" "$K2_CLI" --schema 2>/dev/null)"
assert_has "schema lists it" "$out" "mail forward-message"

echo "== help no longer says linked only =="
out="$("${K2[@]}" "$K2_CLI" mail send --help 2>&1)"
assert_lacks "send help" "$out" "LINKED inboxes only"
assert_has "send help names the limit" "$out" "25 MiB"
out="$("${K2[@]}" "$K2_CLI" mail reply --help 2>&1)"
assert_lacks "reply help" "$out" "LINKED"
out="$(env HOME="$WORKDIR" "$K2_CLI" --schema 2>/dev/null)"
assert_lacks "schema" "$out" "LINKED inboxes only"
assert_lacks "schema descriptions" "$out" "not hosted yet"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
