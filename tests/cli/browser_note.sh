#!/usr/bin/env bash
# `k2 browser note` / `k2 browser redeliver` / `k2 workspace design-mode`
# (prd-browser-design-mode-v1 D-A13 + the 2026-10-16 Design mode setting).
#
# Drives the REAL cli/k2 against a stub daemon that records each POST body
# and asserts what the CLI sends:
#   1. note: POST /cli/browser/notes with workspace, page {url,title} and one
#      note {comment, intent, payload {selector, html, text}} (html read from
#      --html-file); prints "Sent to …" on delivered:true.
#   2. note without element flags: no payload (a whole-page note).
#   3. delivered:false prints the redeliver hint and still exits 0.
#   4. redeliver: POST /cli/browser/notes/redeliver {workspace, threadItemId}.
#   5. a daemon error exits 1 with the message on stderr.
#   6. usage errors exit 2 (no --url, bad --intent) and never hit the daemon.
#   7. workspace design-mode set off → /cli/workspace/set fields
#      {design_mode_enabled: "0"}; get prints on/off from /cli/settings.
#   8. --schema lists "browser note"; k2 study design exists.
#
# No daemon build; never touches ~/.k2 (env carries port + token).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORK="$(mktemp -d -t k2-dm-browser-note-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

work = sys.argv[1]
n = {"i": 0}

def mode():
    p = os.path.join(work, "mode")
    return open(p).read().strip() if os.path.exists(p) else "ok"

class H(BaseHTTPRequestHandler):
    def reply(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urlparse(self.path).path
        q = parse_qs(urlparse(self.path).query)
        if path == "/cli/workspace/resolve":
            return self.reply({"path": "/ws/" + q.get("q", [""])[0]})
        if path == "/cli/settings":
            return self.reply({"designModeEnabled": mode() != "off"})
        self.reply({"error": "stub: unknown GET " + path}, 404)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        path = urlparse(self.path).path
        n["i"] += 1
        with open(os.path.join(work, "req-%d.json" % n["i"]), "wb") as f:
            f.write(json.dumps({"path": path, "body": json.loads(body or b"{}")}).encode())
        m = mode()
        if path in ("/cli/browser/notes", "/cli/browser/notes/redeliver"):
            if m == "error":
                return self.reply({"error": "remote_instruction_disabled", "message": "no remote instructions here"}, 403)
            return self.reply({"ok": True, "threadItemId": "item-1", "seq": 3,
                               "delivered": m != "undelivered", "targetSessionId": None,
                               "addr": "docs", "notes": 1})
        if path == "/cli/workspace/set":
            return self.reply({"ok": True, "changed": True, "actions": [
                {"field": "design_mode_enabled", "status": "done", "value": "0"}]})
        self.reply({"error": "stub: unknown POST " + path}, 404)

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"
for _ in $(seq 1 50); do [ -f "$WORK/stub.port" ] && break; sleep 0.1; done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
last_req() { ls -1 "$WORK"/req-*.json 2>/dev/null | sort -t- -k2 -n | tail -1; }
req_count() { ls -1 "$WORK"/req-*.json 2>/dev/null | wc -l | tr -d ' '; }
k2() { "$K2_CLI" "$@"; }

# 1. A full note.
printf '<button class="cta">Buy</button>' > "$WORK/el.html"
out="$(k2 browser note --workspace docs --url 'http://localhost:5173/p?x=1' --title Pricing \
    --selector 'button.cta' --html-file "$WORK/el.html" --text Buy --intent question 'why so small?' 2>&1)" \
    || bad "1: exit non-zero: $out"
got="$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
b = r["body"]
n = b["notes"][0]
print(r["path"], b["workspace"], b["page"]["url"], b["page"]["title"], n["intent"], n["comment"], n["payload"]["selector"], n["payload"]["html"], n["payload"]["text"], sep="|")
' "$(last_req)")"
want='/cli/browser/notes|docs|http://localhost:5173/p?x=1|Pricing|question|why so small?|button.cta|<button class="cta">Buy</button>|Buy'
[ "$got" = "$want" ] && ok "1: note body" || bad "1: body: got [$got] want [$want]"
grep -qF 'Sent to docs (Thread item item-1).' <<<"$out" && ok "1: delivered message" || bad "1: output: $out"

# 2. A whole-page note has no payload.
k2 browser note --workspace docs --url http://a.test/ 'the header is too tall' >/dev/null 2>&1 || bad "2: exit non-zero"
has_payload="$(python3 -c 'import json,sys; print("payload" in json.load(open(sys.argv[1]))["body"]["notes"][0])' "$(last_req)")"
[ "$has_payload" = "False" ] && ok "2: no payload without element flags" || bad "2: payload present"

# 3. Not delivered: hint, exit 0.
echo undelivered > "$WORK/mode"
out="$(k2 browser note --workspace docs --url http://a.test/ 'x' 2>&1)" || bad "3: exit non-zero: $out"
grep -qF 'k2 browser redeliver item-1 --workspace docs' <<<"$out" && ok "3: redeliver hint" || bad "3: output: $out"
echo ok > "$WORK/mode"

# 4. Redeliver.
k2 browser redeliver item-1 --workspace docs --json >/dev/null 2>&1 || bad "4: exit non-zero"
got="$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["path"], r["body"]["workspace"], r["body"]["threadItemId"])' "$(last_req)")"
[ "$got" = "/cli/browser/notes/redeliver docs item-1" ] && ok "4: redeliver body" || bad "4: got [$got]"

# 5. A refusal exits 1 with the message.
echo error > "$WORK/mode"
set +e
err="$(k2 browser note --workspace docs --url http://a.test/ 'x' 2>&1 >/dev/null)"; rc=$?
set -e
echo ok > "$WORK/mode"
[ "$rc" -eq 1 ] && grep -qF 'no remote instructions here' <<<"$err" && ok "5: refusal exits 1" || bad "5: rc=$rc err=$err"

# 6. Usage errors exit 2 and never reach the daemon.
before="$(req_count)"
set +e
k2 browser note --workspace docs 'no url' >/dev/null 2>&1; rc1=$?
k2 browser note --workspace docs --url http://a.test/ --intent delete 'x' >/dev/null 2>&1; rc2=$?
k2 browser bogus >/dev/null 2>&1; rc3=$?
set -e
[ "$rc1" -eq 2 ] && [ "$rc2" -eq 2 ] && [ "$rc3" -eq 2 ] && [ "$(req_count)" = "$before" ] \
    && ok "6: usage errors exit 2 offline" || bad "6: rc=$rc1/$rc2/$rc3 reqs $before→$(req_count)"

# 7. Design mode setting.
out="$(k2 workspace design-mode set docs off 2>&1)" || bad "7: set exit non-zero: $out"
got="$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["path"], r["body"]["project"], json.dumps(r["body"]["fields"]))' "$(last_req)")"
[ "$got" = '/cli/workspace/set /ws/docs {"design_mode_enabled": "0"}' ] && ok "7: set body" || bad "7: got [$got]"
grep -qF 'Design mode off.' <<<"$out" && ok "7: set output" || bad "7: output: $out"
echo off > "$WORK/mode"
[ "$(k2 workspace design-mode get docs 2>&1)" = "off" ] && ok "7: get off" || bad "7: get off"
echo ok > "$WORK/mode"
[ "$(k2 workspace design-mode get docs 2>&1)" = "on" ] && ok "7: get on" || bad "7: get on"
set +e
k2 workspace design-mode set docs maybe >/dev/null 2>&1; rc=$?
set -e
[ "$rc" -eq 2 ] && ok "7: bad value exits 2" || bad "7: bad value rc=$rc"

# 8. Discoverable.
schema="$(k2 --schema 2>/dev/null)"
grep -qF '"name": "browser note"' <<<"$schema" && ok "8: --schema lists browser note" || bad "8: schema"
page="$(k2 study design 2>/dev/null)"
grep -qF 'k2 study design' <<<"$page" && ok "8: k2 study design" || bad "8: study page"

echo
echo "browser_note: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
