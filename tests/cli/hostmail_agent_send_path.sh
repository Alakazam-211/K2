#!/usr/bin/env bash
# Loud harness for the 0.45.0 mail-credential rule in the CLI (the daemon
# decides; these check the CLI carries it):
# - a refusal (403 agent_send_path_only for an agent without mail-manage,
#   403 owner_only for an IT agent on its own sends) exits 3 with the hint;
# - `k2 hostmail create` prints "password : withheld" when the daemon
#   withholds it and the once password otherwise; `--person` sends
#   person:true; `k2 hostmail person <addr> on|off` POSTs the flag;
# - `k2 hostmail password keep` / `app-password keep` POST the right body
#   to /cli/mail/credentials/keep (incl. bulk `--all-existing`).
# cargo does not cover cmd_hostmail_* / _mail_py. Fail-loud: no skips.
# Run with: bash tests/cli/hostmail_agent_send_path.sh  (fake daemon, no live one).

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

assert_json() {
    local label="$1" hay="$2" expr="$3"
    if printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($expr) else 1)" 2>/dev/null; then
        ok "$label"
    else
        bad "$label — '$expr' false for: $hay"
    fi
}

WORKDIR="$(mktemp -d -t k2-agent-send-path-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

# Fake daemon. Records each POST body at post<path-with-slashes-as-_>.json.
# MODE file switches /cli/mail/address/create between agent and owner mints.
python3 - "$WORKDIR" <<'PY' &
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer

root = sys.argv[1]
REFUSAL = {"ok": False, "error": {"code": "agent_send_path_only", "hint":
    "Agents send mail with `k2 mail send` (and `k2 mail reply`), where the owner's rules "
    "apply. Only the owner can create an app password for this mailbox: `k2 hostmail "
    "app-password add <addr>`, run by the owner from a terminal outside a K2 agent session."}}

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
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
        if path in ("/cli/mail/app-password", "/cli/mail/address/password",
                    "/cli/mail/approvals/approve", "/cli/mail/approvals/deny"):
            self._send(403, REFUSAL)
        elif path == "/cli/mail/config/set":
            b = json.loads(raw or b"{}")
            if "agentSend" in b or "alwaysBcc" in b:
                # An IT agent on its OWN workspace (self-elevation).
                self._send(403, {"ok": False, "error": {"code": "owner_only",
                    "hint": "An agent can't loosen mail rules on its own sends: changing "
                            "agentSend or always-BCC for this agent's own workspace needs "
                            "the owner. Ask your human."}})
            else:
                self._send(200, {"ok": True, "applied": {}})
        elif path == "/cli/mail/address/person":
            b = json.loads(raw or b"{}")
            if b.get("all"):
                self._send(200, {"ok": True, "all": True, "workspace": "ws-uuid",
                                 "person": b.get("person"),
                                 "changed": ["staff1@shop.example", "staff2@shop.example"],
                                 "excludedSendIdentities": ["desk@shop.example"]})
            else:
                self._send(200, {"ok": True, "address": b.get("address"), "person": b.get("person")})
        elif path == "/cli/mail/address/create":
            mode = open(os.path.join(root, "MODE")).read().strip()
            b = json.loads(raw or b"{}")
            d = {"ok": True, "id": "row-1", "address": "bot@shop.example", "existing": False,
                 "cap": {"used": 1, "cap": 5}, "username": "bot@shop.example",
                 "submission": {"host": "mail.shop.example", "port": 465, "tls": True}}
            if mode == "agent" and not b.get("person"):
                d["passwordWithheld"] = True
                d["passwordHint"] = "password withheld: agents send mail with `k2 mail send`"
            else:
                d["password"] = "OWNER-ONCE-PASSWORD"
            self._send(200, d)
        elif path == "/cli/mail/credentials/keep" and json.loads(raw or b"{}").get("allExisting"):
            b = json.loads(raw)
            mbox = b.get("kind") == "mailbox"
            self._send(200, {"ok": True, "allExisting": True, "cutoff": 1700000000,
                             "kept": {"mailboxPasswords": 4 if mbox else 0,
                                      "appPasswords": 0 if mbox else 2},
                             "leftFlagged": {"createdAfterCutoff": 1, "noCreationTime": 0},
                             "listErrors": []})
        elif path == "/cli/mail/credentials/keep":
            b = json.loads(raw or b"{}")
            self._send(200, {"ok": True, "address": b.get("address"), "kept": True,
                             "kind": "app_password" if b.get("appPasswordId") else "mailbox",
                             "appPasswordId": b.get("appPasswordId")})
        else:
            self._send(404, {"ok": False, "error": {"code": "not_found", "hint": path}})
    def do_GET(self):
        self._send(200, {"ok": True})

httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
disown "$FAKE_PID" 2>/dev/null || true
trap 'kill $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")

echo "== agent: hostmail app-password add is refused (exit 3) =="
set +e
err="$("${K2[@]}" "$K2_CLI" hostmail app-password add bot@shop.example --label phone 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "app-password add as agent" 3 "$rc"
assert_json "stderr code agent_send_path_only" "$err" "d['error']['code'] == 'agent_send_path_only'"
assert_json "stderr hint names k2 mail send" "$err" "'k2 mail send' in d['error']['hint']"
post="$(cat "$WORKDIR/post_cli_mail_app-password.json" 2>/dev/null || echo '{}')"
assert_json "add POST body" "$post" "d == {'address': 'bot@shop.example', 'label': 'phone'}"

echo "== agent: hostmail password rotate is refused (exit 3) =="
set +e
err="$("${K2[@]}" "$K2_CLI" hostmail password rotate bot@shop.example 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "password rotate as agent" 3 "$rc"
assert_json "rotate stderr code" "$err" "d['error']['code'] == 'agent_send_path_only'"

echo "== agent mint: password withheld =="
echo agent > "$WORKDIR/MODE"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail create bot 2>&1)"
rc=$?
set -e
assert_exit "mail create (agent)" 0 "$rc"
printf '%s' "$out" | grep -q "password : withheld" && ok "prints withheld" || bad "withheld line missing in: $out"
printf '%s' "$out" | grep -q "k2 mail send" && ok "withheld hint names k2 mail send" || bad "hint missing in: $out"

echo "== owner mint: password shown once =="
echo owner > "$WORKDIR/MODE"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail create bot 2>&1)"
rc=$?
set -e
assert_exit "mail create (owner)" 0 "$rc"
printf '%s' "$out" | grep -q "password : OWNER-ONCE-PASSWORD" && ok "owner sees the once password" || bad "owner password missing in: $out"
if printf '%s' "$out" | grep -q "withheld"; then bad "owner mint must not say withheld: $out"; else ok "owner mint not withheld"; fi

echo "== hostmail password keep =="
KEEP_POST="$WORKDIR/post_cli_mail_credentials_keep.json"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail password keep staff@shop.example 2>&1)"
rc=$?
set -e
assert_exit "password keep" 0 "$rc"
post="$(cat "$KEEP_POST" 2>/dev/null || echo '{}')"
assert_json "password keep POST body (no appPasswordId)" "$post" "d == {'address': 'staff@shop.example'}"
printf '%s' "$out" | grep -q "kept the mailbox password of staff@shop.example" && ok "password keep output" || bad "keep output: $out"

echo "== hostmail app-password keep =="
rm -f "$KEEP_POST"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail app-password keep staff@shop.example ap-7 --json 2>&1)"
rc=$?
set -e
assert_exit "app-password keep" 0 "$rc"
post="$(cat "$KEEP_POST" 2>/dev/null || echo '{}')"
assert_json "app-password keep POST body" "$post" "d == {'address': 'staff@shop.example', 'appPasswordId': 'ap-7'}"
assert_json "app-password keep --json output" "$out" "d['kept'] is True and d['appPasswordId'] == 'ap-7'"

echo "== bulk review: --all-existing =="
rm -f "$KEEP_POST"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail password keep --all-existing 2>&1)"
rc=$?
set -e
assert_exit "password keep --all-existing" 0 "$rc"
post="$(cat "$KEEP_POST" 2>/dev/null || echo '{}')"
assert_json "password bulk POST body" "$post" "d == {'allExisting': True, 'kind': 'mailbox'}"
printf '%s' "$out" | grep -q "kept 4 mailbox password(s) that existed before this update" && ok "bulk mailbox output" || bad "bulk output: $out"
printf '%s' "$out" | grep -q "still flagged: 1 created after the update" && ok "bulk names what stays flagged" || bad "bulk leftover line: $out"
rm -f "$KEEP_POST"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail app-password keep --all-existing --json 2>&1)"
rc=$?
set -e
assert_exit "app-password keep --all-existing" 0 "$rc"
post="$(cat "$KEEP_POST" 2>/dev/null || echo '{}')"
assert_json "app-password bulk POST body" "$post" "d == {'allExisting': True, 'kind': 'app_password'}"
assert_json "app-password bulk --json" "$out" "d['kept']['appPasswords'] == 2"
rm -f "$KEEP_POST"
set +e
"${K2[@]}" "$K2_CLI" hostmail password keep staff@shop.example --all-existing >/dev/null 2>&1
rc=$?
set -e
assert_exit "--all-existing with an address" 2 "$rc"
[ ! -f "$KEEP_POST" ] && ok "no POST for --all-existing + address" || bad "POST went out for --all-existing + address"

echo "== agent refusals on approvals and own policy exit 3 =="
set +e
err="$("${K2[@]}" "$K2_CLI" hostmail approvals approve out_abc0000000001 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "approvals approve as agent" 3 "$rc"
assert_json "approve stderr code" "$err" "d['error']['code'] == 'agent_send_path_only'"
set +e
"${K2[@]}" "$K2_CLI" hostmail approvals deny out_abc0000000001 --note no >/dev/null 2>&1
rc=$?
set -e
assert_exit "approvals deny as agent" 3 "$rc"
set +e
err="$("${K2[@]}" "$K2_CLI" hostmail config --workspace shop-cs --agent-send on 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "config --agent-send as IT agent on its own workspace" 3 "$rc"
assert_json "config stderr code" "$err" "d['error']['code'] == 'owner_only'"
assert_json "config stderr names self-elevation" "$err" "\"can't loosen mail rules on its own sends\" in d['error']['hint']"

echo "== person mailboxes =="
CREATE_POST="$WORKDIR/post_cli_mail_address_create.json"
echo agent > "$WORKDIR/MODE"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail create staff --person 2>&1)"
rc=$?
set -e
assert_exit "create --person" 0 "$rc"
post="$(cat "$CREATE_POST" 2>/dev/null || echo '{}')"
assert_json "create --person POST body" "$post" "d.get('person') is True and d.get('localPart') == 'staff'"
printf '%s' "$out" | grep -q "password : OWNER-ONCE-PASSWORD" && ok "person mint shows the password" || bad "person mint output: $out"
set +e
"${K2[@]}" "$K2_CLI" hostmail create bot >/dev/null 2>&1
set -e
post="$(cat "$CREATE_POST" 2>/dev/null || echo '{}')"
assert_json "create without --person sends no person key" "$post" "'person' not in d"
PERSON_POST="$WORKDIR/post_cli_mail_address_person.json"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail person staff@shop.example on 2>&1)"
rc=$?
set -e
assert_exit "person on" 0 "$rc"
assert_json "person on POST body" "$(cat "$PERSON_POST")" "d == {'address': 'staff@shop.example', 'person': True}"
printf '%s' "$out" | grep -q "is now a person's mailbox" && ok "person on output" || bad "person on output: $out"
set +e
"${K2[@]}" "$K2_CLI" hostmail person staff@shop.example off --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "person off" 0 "$rc"
assert_json "person off POST body" "$(cat "$PERSON_POST")" "d == {'address': 'staff@shop.example', 'person': False}"
rm -f "$PERSON_POST"
set +e
"${K2[@]}" "$K2_CLI" hostmail person staff@shop.example maybe >/dev/null 2>&1
rc=$?
set -e
assert_exit "person with a bad state" 2 "$rc"
[ ! -f "$PERSON_POST" ] && ok "no POST on a bad person state" || bad "a person POST went out on a usage error"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail person --all --workspace it-email on 2>&1)"
rc=$?
set -e
assert_exit "person --all --workspace on" 0 "$rc"
assert_json "person --all POST body" "$(cat "$PERSON_POST")" "d == {'all': True, 'workspace': 'it-email', 'person': True}"
printf '%s' "$out" | grep -q "marked 2 mailbox(es) in it-email as people's mailboxes" && ok "person --all output" || bad "person --all output: $out"
printf '%s' "$out" | grep -q "left as the agent's own (its send identities): desk@shop.example" && ok "person --all names the excluded send identity" || bad "excluded line: $out"
rm -f "$PERSON_POST"
set +e
"${K2[@]}" "$K2_CLI" hostmail person --all on >/dev/null 2>&1
rc=$?
set -e
assert_exit "person --all without --workspace" 2 "$rc"
set +e
"${K2[@]}" "$K2_CLI" hostmail person staff@shop.example --all --workspace it-email on >/dev/null 2>&1
rc=$?
set -e
assert_exit "person --all with an address" 2 "$rc"
[ ! -f "$PERSON_POST" ] && ok "no POST on --all usage errors" || bad "a person POST went out on a --all usage error"
h="$("${K2[@]}" "$K2_CLI" hostmail person --help)"
printf '%s' "$h" | grep -q "k2 hostmail person --all --workspace <ws> on|off" && ok "person --all help" || bad "person --all help: $h"
printf '%s' "$h" | grep -q "k2 hostmail person <addr> on|off" && ok "person help" || bad "person help: $h"

echo "== keep usage errors (no POST) =="
rm -f "$KEEP_POST"
set +e
"${K2[@]}" "$K2_CLI" hostmail app-password keep staff@shop.example >/dev/null 2>&1
rc=$?
set -e
assert_exit "app-password keep without id" 2 "$rc"
set +e
"${K2[@]}" "$K2_CLI" hostmail password keep >/dev/null 2>&1
rc=$?
set -e
assert_exit "password keep without addr" 2 "$rc"
[ ! -f "$KEEP_POST" ] && ok "nothing POSTed on usage errors" || bad "a keep POST went out on a usage error"

echo "== help =="
h="$("${K2[@]}" "$K2_CLI" hostmail app-password keep --help)"
printf '%s' "$h" | grep -q "k2 hostmail app-password keep <addr> <id>" && ok "app-password keep leaf help" || bad "leaf help: $h"
h="$("${K2[@]}" "$K2_CLI" hostmail password keep --help)"
printf '%s' "$h" | grep -q "k2 hostmail password keep <addr>" && ok "password keep leaf help" || bad "leaf help: $h"
h="$("${K2[@]}" "$K2_CLI" hostmail app-password add --help)"
printf '%s' "$h" | grep -q "loosen mail rules on its own sends" && ok "add help names the refusal" || bad "add help: $h"
h="$("${K2[@]}" "$K2_CLI" hostmail app-password --help)"
printf '%s' "$h" | grep -q "app-password keep" && ok "group help lists keep" || bad "group help: $h"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
