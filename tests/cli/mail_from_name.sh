#!/usr/bin/env bash
# Loud harness for `k2 mail send --from-name <name>` (0.46.3) and
# `k2 mail reply --from-name <name>` (0.46.4): the CLI sends the display name
# as `fromName` in the POST /cli/mail/send (or /cli/mail/reply) body, sends no
# fromName key without the flag, and refuses a missing/blank value (exit 2,
# nothing POSTed). The daemon validates the name itself (CR/LF, '@', length;
# cargo tests in mail::send / mail::routes_send). cargo does not cover
# cmd_mail_* / _mail_py. Fail-loud: no skip-if-missing.
# Run with: bash tests/cli/mail_from_name.sh  (no live daemon — a tiny fake).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -f "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found" >&2; exit 1; }
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
bash -n "$K2_CLI" || { echo "FAIL: bash -n cli/k2" >&2; exit 1; }

# Never inherit a live session's socket or identity.
for v in $(compgen -e | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done

pass=0
fail=0

ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

assert_exit() {
    local label="$1" expected="$2" got="$3"
    if [ "$got" = "$expected" ]; then ok "$label (exit $got)"; else bad "$label expected exit $expected got $got"; fi
}

# assert_json <label> <json> <python-expr over d> — expr must be truthy.
assert_json() {
    local label="$1" hay="$2" expr="$3"
    if printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($expr) else 1)" 2>/dev/null; then
        ok "$label"
    else
        bad "$label — '$expr' false for: $hay"
    fi
}

WORKDIR="$(mktemp -d -t k2-mail-from-name-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

# Fake daemon: record each POST body at post<path-with-slashes-as-_>.json.
python3 - "$WORKDIR" <<'PY' &
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer

root = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _ok(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        name = "post" + path.replace("/", "_") + ".json"
        with open(os.path.join(root, name), "wb") as f:
            f.write(raw)
        self._ok({"ok": True, "id": "out_fake00000001", "status": "pending_approval",
                  "queued": True, "hint": "queued for approval"})
    def do_GET(self):
        self._ok({"ok": True})

httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
disown "$FAKE_PID" 2>/dev/null || true   # no "Terminated" noise at exit
trap 'kill $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")
SEND_POST="$WORKDIR/post_cli_mail_send.json"

echo "== mail send --from-name reaches fromName (with --from) =="
set +e
"${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject Quote --body hi \
    --from laura@acme.example --from-name "Laura" --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "send --from-name" 0 "$rc"
post="$(cat "$SEND_POST" 2>/dev/null || echo '{}')"
assert_json "POST fromName" "$post" "d.get('fromName') == 'Laura'"
assert_json "POST from unchanged" "$post" "d.get('from') == 'laura@acme.example'"
assert_json "POST to" "$post" "d.get('to') == ['cust@dest.example']"

echo "== quotes, commas, angle brackets and non-ASCII pass through verbatim =="
rm -f "$SEND_POST"
set +e
"${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject Quote --body hi \
    --from-name 'Smith, "LJ" <Zoë>' --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "send --from-name specials" 0 "$rc"
post="$(cat "$SEND_POST" 2>/dev/null || echo '{}')"
assert_json "POST fromName verbatim" "$post" "d.get('fromName') == 'Smith, \"LJ\" <Zoë>'"
assert_json "POST no from without --from" "$post" "'from' not in d"

echo "== without --from-name no fromName key is sent =="
rm -f "$SEND_POST"
set +e
"${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject Quote --body hi --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "send (no from-name)" 0 "$rc"
post="$(cat "$SEND_POST" 2>/dev/null || echo '{}')"
assert_json "POST has no fromName" "$post" "'fromName' not in d and 'to' in d"

echo "== --from-name needs a value =="
rm -f "$SEND_POST"
set +e
out="$("${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject s --body b --from-name 2>&1)"
rc=$?
set -e
assert_exit "--from-name without value" 2 "$rc"
printf '%s' "$out" | grep -q -- "--from-name requires a display name" && ok "missing-value hint" || bad "missing-value hint missing in: $out"
[ ! -f "$SEND_POST" ] && ok "nothing POSTed (missing value)" || bad "a POST went out without a --from-name value"

echo "== --from-name '' / blank is refused =="
for blank in "" "   "; do
    rm -f "$SEND_POST"
    set +e
    out="$("${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject s --body b --from-name "$blank" 2>&1)"
    rc=$?
    set -e
    assert_exit "--from-name '$blank'" 2 "$rc"
    printf '%s' "$out" | grep -q "non-empty display name" && ok "blank hint ('$blank')" || bad "blank hint missing in: $out"
    [ ! -f "$SEND_POST" ] && ok "nothing POSTed ('$blank')" || bad "a POST went out for a blank --from-name"
done

echo "== help and schema document the flag =="
help="$("${K2[@]}" "$K2_CLI" mail send --help 2>&1)"
printf '%s' "$help" | grep -q -- "--from-name <name>" && ok "help lists --from-name" || bad "help lacks --from-name: $help"
schema="$("${K2[@]}" "$K2_CLI" --schema json 2>/dev/null)"
assert_json "schema lists --from-name" "$schema" \
    "any(f['name'] == '--from-name' for c in (d['commands'] if isinstance(d, dict) else d) if c['name'] == 'mail send' for f in c['flags'])"

REPLY_POST="$WORKDIR/post_cli_mail_reply.json"

echo "== mail reply --from-name reaches fromName =="
rm -f "$REPLY_POST"
set +e
"${K2[@]}" "$K2_CLI" mail reply msg_fake01 --body "thanks" --from-name "Laura" --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "reply --from-name" 0 "$rc"
post="$(cat "$REPLY_POST" 2>/dev/null || echo '{}')"
assert_json "reply POST fromName" "$post" "d.get('fromName') == 'Laura'"
assert_json "reply POST id" "$post" "d.get('id') == 'msg_fake01'"
assert_json "reply POST body" "$post" "d.get('body') == 'thanks'"
assert_json "reply POST has no from (address stays locked)" "$post" "'from' not in d"

echo "== mail reply --from-name passes specials through verbatim =="
rm -f "$REPLY_POST"
set +e
"${K2[@]}" "$K2_CLI" mail reply msg_fake01 --body "thanks" --from-name 'Smith, "LJ" <Zoë>' --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "reply --from-name specials" 0 "$rc"
post="$(cat "$REPLY_POST" 2>/dev/null || echo '{}')"
assert_json "reply POST fromName verbatim" "$post" "d.get('fromName') == 'Smith, \"LJ\" <Zoë>'"

echo "== mail reply without --from-name sends no fromName key =="
rm -f "$REPLY_POST"
set +e
"${K2[@]}" "$K2_CLI" mail reply msg_fake01 --body "thanks" --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "reply (no from-name)" 0 "$rc"
post="$(cat "$REPLY_POST" 2>/dev/null || echo '{}')"
assert_json "reply POST has no fromName" "$post" "'fromName' not in d and d.get('id') == 'msg_fake01'"

echo "== mail reply --from-name needs a value =="
rm -f "$REPLY_POST"
set +e
out="$("${K2[@]}" "$K2_CLI" mail reply msg_fake01 --body b --from-name 2>&1)"
rc=$?
set -e
assert_exit "reply --from-name without value" 2 "$rc"
printf '%s' "$out" | grep -q -- "--from-name requires a display name" && ok "reply missing-value hint" || bad "reply missing-value hint missing in: $out"
[ ! -f "$REPLY_POST" ] && ok "reply: nothing POSTed (missing value)" || bad "a reply POST went out without a --from-name value"

echo "== mail reply --from-name '' / blank is refused =="
for blank in "" "   "; do
    rm -f "$REPLY_POST"
    set +e
    out="$("${K2[@]}" "$K2_CLI" mail reply msg_fake01 --body b --from-name "$blank" 2>&1)"
    rc=$?
    set -e
    assert_exit "reply --from-name '$blank'" 2 "$rc"
    printf '%s' "$out" | grep -q "non-empty display name" && ok "reply blank hint ('$blank')" || bad "reply blank hint missing in: $out"
    [ ! -f "$REPLY_POST" ] && ok "reply: nothing POSTed ('$blank')" || bad "a reply POST went out for a blank --from-name"
done

echo "== reply help and schema document the flag =="
help="$("${K2[@]}" "$K2_CLI" mail reply --help 2>&1)"
printf '%s' "$help" | grep -q -- "--from-name <name>" && ok "reply help lists --from-name" || bad "reply help lacks --from-name: $help"
assert_json "schema lists reply --from-name" "$schema" \
    "any(f['name'] == '--from-name' for c in (d['commands'] if isinstance(d, dict) else d) if c['name'] == 'mail reply' for f in c['flags'])"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
