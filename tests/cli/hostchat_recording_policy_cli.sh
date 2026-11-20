#!/usr/bin/env bash
# k2 hostchat recording policy [set KEY VALUE …]: the server's recording
# policies, against a stub daemon and a stub vault.sock. 0.46.16 refused
# every policy line as an unknown recording command; this pins that the
# line reaches the daemon again, on the routes the app's Settings use.
#
# Covers:
#   1. Help without a daemon: the hostchat help and the recording help
#      name the verb and its keys; the recording page, which policy and
#      the Space verbs (app-space recording, recording-lock) print, names
#      them too.
#   2. Help never acts: -h / --help anywhere on a policy line, also before
#      the verb, prints the recording help and makes no request.
#   3. show: GET /cli/sew/recording/policies with the owner token; the
#      flags line, one row per policy, the locks; --json is the daemon's.
#   4. set: POST /cli/sew/recording/policy {changes}, a 403
#      owner_confirm_required, the person's confirm over vault.sock bound
#      to this install (action recording-policy), then the same body with
#      confirmToken. Short keys take the recording. prefix; on/off are
#      booleans, audio_max_days a number (a trailing d is dropped).
#   5. Bad usage: an unknown key, a bad value, a key with no value, no
#      key, a word after policy, an unknown flag: exit 2, nothing sent.
#   6. A K2 terminal (agent passport): the daemon's person_only refusal is
#      exit 3; a confirm the daemon asks of a session is never given here
#      (exit 3, no vault confirm).
# Fake values only. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

# Short path: the stub vault.sock must fit a Unix socket path.
WORK="$(mktemp -d /tmp/k2rp.XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

# ── 1. help, no daemon ───────────────────────────────────────────────
echo "== help (no daemon) =="
HOME1="$WORK/h1"
mkdir -p "$HOME1"
nodaemon() { env HOME="$HOME1" K2_PORT=1 "$K2_CLI" "$@" </dev/null; }
run nodaemon hostchat --help
assert_contains "hostchat help: show" "$out" "k2 hostchat recording policy [--json]"
assert_contains "hostchat help: set" "$out" "k2 hostchat recording policy set KEY VALUE [--json]"
run nodaemon hostchat recording --help
assert_eq "recording --help exit 0" "$rc" "0"
assert_contains "recording help: set" "$out" "k2 hostchat recording policy set KEY VALUE [KEY VALUE …] [--json]"
assert_contains "recording help: dm_enabled" "$out" "recording.dm_enabled on|off        recordings in direct messages"
assert_contains "recording help: who" "$out" "The owner or an admin, in person; recording.required the owner only."
assert_contains "recording help: still the recorder's page" "$out" "k2 hostchat recording enable [--recorder-tarball FILE]"
# The Space verbs' help is the same page, and it names them.
for line in "app-space recording acme 5 --help" "app-space recording-lock 5 on -h" "recording policy --help"; do
    # shellcheck disable=SC2086
    run nodaemon hostchat $line
    assert_eq "hostchat $line exit 0" "$rc" "0"
    assert_contains "hostchat $line: app-space recording" "$out" "k2 hostchat app-space recording <app> <space-id> [--json]"
    assert_contains "hostchat $line: recording-lock" "$out" "k2 hostchat app-space recording-lock <space-id> on|off [--json]"
    assert_contains "hostchat $line: policy" "$out" "k2 hostchat recording policy [--json]"
done

# ── stub daemon + vault.sock ─────────────────────────────────────────
OWNER="owner-disk-token-recpol"
SCOPED="sess77.scoped-secret-recpol"
HOME2="$WORK/h2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"
# How the stub answers a passport's change: person_only (the daemon's
# answer) or owner_confirm_required (a confirm a session must never give).
echo person_only > "$WORK/mode"

python3 - "$WORK" "$SCOPED" "$HOME2/.k2/vault.sock" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped, vsock = sys.argv[1], sys.argv[2], sys.argv[3]
ACTION = "recording-policy"
INSTALL = "inst-recpol-1"
POLICIES = {"ok": True,
            "flags": {"appRecording": True, "voiceRecordCap": True, "recordingRequired": True, "mgmtRecordingReads": False},
            "policies": [
                {"key": "recording.enabled", "want": True, "seen": True, "delivered": True, "setBy": "owner"},
                {"key": "recording.dm_enabled", "want": None, "seen": False},
                {"key": "recording.require_consent_to_join", "want": False, "seen": True, "delivered": False},
                {"key": "recording.required", "want": None, "seen": "off"},
                {"key": "recording.audio_max_days", "want": 30, "seen": 30, "delivered": True, "setBy": "login:ada"}],
            "locks": [],
            "k5SpaceId": "sp-k5"}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(parsed.query).items()}
        auth = self.headers.get("Authorization") or ""
        token = auth[len("Bearer "):] if auth.startswith("Bearer ") else ""
        rec = {"method": method, "path": parsed.path, "token": token, "query": q, "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 200, {"ok": True}
        def err(st, code, hint):
            return st, {"ok": False, "error": {"code": code, "hint": hint}}
        if p == "/cli/vault/confirm/start":
            out = {"methods": ["vault-password"]}
        elif p == "/cli/vault/confirm/finish":
            out = {"token": "tok-" + str(b.get("action")) + "-" + str(b.get("secretId"))}
        elif p == "/cli/sew/status" and method == "GET":
            out = {"ok": True, "installId": INSTALL}
        elif p == "/cli/sew/recording/policies" and method == "GET":
            out = POLICIES
        elif p == "/cli/sew/recording/policy" and method == "POST":
            ch = b.get("changes") or {}
            if token == scoped:
                with open(os.path.join(work, "mode")) as f:
                    mode = f.read().strip()
                if mode == "person_only":
                    status, out = err(403, "person_only", "nothing changed: a person does this, never an agent or a K2 terminal")
                else:
                    status, out = err(403, "owner_confirm_required", "a person confirms this")
            elif not b.get("confirmToken"):
                status, out = err(403, "owner_confirm_required", "a person confirms this, in person")
            elif b["confirmToken"] != "tok-%s-%s" % (ACTION, INSTALL):
                status, out = err(403, "owner_confirm_required", "wrong confirm")
            else:
                rows = [dict(r) for r in POLICIES["policies"]]
                for r in rows:
                    if r["key"] in ch:
                        r.update({"want": ch[r["key"]], "delivered": False, "setBy": "owner"})
                out = {"ok": True, "changed": sorted(ch), "policies": rows}
        else:
            status, out = 404, {"error": "route not found"}
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

class UH(H):
    def address_string(self):
        return "unix"

if os.path.exists(vsock):
    os.unlink(vsock)
us = U(vsock, UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$HOME2/.k2/vault.sock" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] && [ -S "$HOME2/.k2/vault.sock" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"
touch "$WORK/reqs.jsonl"

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@" </dev/null; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@" </dev/null
}
# Requests to the routes under test and the vault (a K2 terminal's CLI may
# make other calls first, such as resolving its session).
nreq() { grep -cE '"path": "/cli/(sew|vault)/' "$WORK/reqs.jsonl" || true; }
req_where() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = [r for r in rows if r["path"] == sys.argv[2]]
r = rows[int(sys.argv[4]) if len(sys.argv) > 4 else -1]
f = sys.argv[3]
if f == "body":
    print(json.dumps(json.loads(r["body"] or "{}"), sort_keys=True))
else:
    print(r[f])
' "$WORK/reqs.jsonl" "$@"
}
count_path() { python3 -c '
import json, sys
print(sum(1 for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]))' "$WORK/reqs.jsonl" "$1"; }
row() { printf '%-36s %-14s %-14s %s' "$@"; }
mkdir -p "$WORK/proj"
cd "$WORK"

# ── 2. help never acts ───────────────────────────────────────────────
echo "== help never acts =="
for line in "recording policy set dm_enabled on -h" "recording policy set dm_enabled on --help" \
            "recording policy -h set dm_enabled on" "recording policy set -h" "recording policy --help" \
            "recording -h policy set dm_enabled on" "recording --help policy" "recording policies set enabled off -h"; do
    before="$(nreq)"
    # shellcheck disable=SC2086
    run owner hostchat $line
    assert_eq "help: hostchat $line → exit 0, no request" "$rc/$(nreq)" "0/$before"
    assert_contains "help: hostchat $line prints the recording help" "$out" "k2 hostchat recording policy set KEY VALUE [KEY VALUE …] [--json]"
done

# ── 3. show ──────────────────────────────────────────────────────────
echo "== show =="
run owner hostchat recording policy
assert_eq "show exit 0" "$rc" "0"
assert_eq "show: GET with the owner token" \
    "$(req_where /cli/sew/recording/policies method)/$(req_where /cli/sew/recording/policies token)" "GET/$OWNER"
assert_eq "show: no body" "$(req_where /cli/sew/recording/policies body)" "{}"
assert_contains "show: the flags" "$out" "Sew flags: record in Apps yes · voice:record cap yes · required yes · server reads no"
assert_contains "show: the header" "$out" "$(row policy "K2 asked" "Sew shows" "")"
assert_contains "show: a delivered want" "$out" "$(row recording.enabled on on "delivered (by owner)")"
assert_contains "show: nothing asked" "$out" "$(row recording.dm_enabled - off "")"
assert_contains "show: not yet delivered" "$out" "$(row recording.require_consent_to_join off on "not yet delivered")"
assert_contains "show: a string value" "$out" "$(row recording.required - off "")"
assert_contains "show: a number" "$out" "$(row recording.audio_max_days 30 30 "delivered (by login:ada)")"
assert_contains "show: no locks" "$out" "locked Spaces: none"
assert_contains "show: the K5 rooms Space" "$out" "K5 rooms Space: sp-k5 (k2 voice settings)"
run owner hostchat recording policies --json
assert_eq "policies --json exit 0" "$rc" "0"
assert_eq "show --json is the daemon's JSON" \
    "$(printf '%s' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(len(v["policies"]), v["flags"]["appRecording"], v["k5SpaceId"])')" "5 True sp-k5"

# ── 4. set, with the person's confirm ────────────────────────────────
echo "== set =="
n0="$(count_path /cli/sew/recording/policy)"
run owner hostchat recording policy set dm_enabled on
assert_eq "set dm_enabled on exit 0" "$rc" "0"
assert_eq "set: two POSTs" "$(( $(count_path /cli/sew/recording/policy) - n0 ))" "2"
assert_eq "set: first POST with the owner token" "$(req_where /cli/sew/recording/policy token "$n0")" "$OWNER"
assert_eq "set: the first body, no token" "$(req_where /cli/sew/recording/policy body "$n0")" \
    '{"changes": {"recording.dm_enabled": true}}'
assert_eq "set: confirm started for this install" "$(req_where /cli/vault/confirm/start body)" \
    '{"action": "recording-policy", "secretId": "inst-recpol-1"}'
assert_eq "set: confirm bound to this install" "$(req_where /cli/vault/confirm/finish body)" \
    '{"action": "recording-policy", "method": "vault-password", "password": "", "secretId": "inst-recpol-1"}'
assert_eq "set: resent with the token" "$(req_where /cli/sew/recording/policy body)" \
    '{"changes": {"recording.dm_enabled": true}, "confirmToken": "tok-recording-policy-inst-recpol-1"}'
assert_contains "set: what was saved" "$out" "saved: recording.dm_enabled"
assert_contains "set: the new row" "$out" "$(row recording.dm_enabled on off "not yet delivered (by owner)")"

n0="$(count_path /cli/sew/recording/policy)"
run owner hostchat recording policy set recording.required multi_person audio_max_days 030d enabled off --json
assert_eq "set three keys --json exit 0" "$rc" "0"
assert_eq "set: three keys, typed" "$(req_where /cli/sew/recording/policy body "$n0")" \
    '{"changes": {"recording.audio_max_days": 30, "recording.enabled": false, "recording.required": "multi_person"}}'
assert_eq "set --json is the daemon's JSON" \
    "$(printf '%s' "$out" | python3 -c 'import json,sys; print(",".join(json.load(sys.stdin)["changed"]))')" \
    "recording.audio_max_days,recording.enabled,recording.required"

n0="$(count_path /cli/sew/recording/policy)"
run owner hostchat recording policy set require_consent_to_join no recording.enabled yes
assert_eq "set no/yes exit 0" "$rc" "0"
assert_eq "set: no and yes are booleans" "$(req_where /cli/sew/recording/policy body "$n0")" \
    '{"changes": {"recording.enabled": true, "recording.require_consent_to_join": false}}'

# ── 5. bad usage: exit 2, nothing sent ───────────────────────────────
echo "== usage =="
before="$(nreq)"
usage_case() {
    local want="$1"; shift
    run owner hostchat "$@"
    assert_eq "usage: hostchat $* → exit 2, nothing sent" "$rc/$(nreq)" "2/$before"
    assert_contains "usage: hostchat $* says why" "$err" "$want"
}
usage_case "unknown policy recording.color: recording.enabled, recording.dm_enabled" recording policy set color on
usage_case "unknown policy recording.dm: recording.enabled" recording policy set recording.dm on
usage_case "recording.dm_enabled is on or off" recording policy set dm_enabled maybe
usage_case "recording.required is off or multi_person" recording policy set required all
usage_case "recording.audio_max_days is a number of days, 1 to 3650" recording policy set audio_max_days 0
usage_case "recording.audio_max_days is a number of days, 1 to 3650" recording policy set audio_max_days 3651
usage_case "recording policy set KEY VALUE [KEY VALUE …]" recording policy set dm_enabled
usage_case "recording policy set KEY VALUE [KEY VALUE …]" recording policy set dm_enabled on enabled
usage_case "recording policy set KEY VALUE [KEY VALUE …]" recording policy set
usage_case "k2 hostchat recording policy [--json] | k2 hostchat recording policy set KEY VALUE" recording policy dm_enabled
usage_case "unknown flag --yes (k2 hostchat recording policy [set KEY VALUE])" recording policy set dm_enabled on --yes
# A bad pair after a good one sends nothing either.
usage_case "unknown policy recording.color" recording policy set dm_enabled on color red

# ── 6. a K2 terminal ─────────────────────────────────────────────────
echo "== agent session =="
fin="$(count_path /cli/vault/confirm/start)"
n0="$(count_path /cli/sew/recording/policy)"
run cell hostchat recording policy set dm_enabled on
assert_eq "agent set → exit 3" "$rc" "3"
assert_contains "agent set: the daemon's person_only" "$err" "person_only"
assert_eq "agent set: one POST, with the passport" \
    "$(( $(count_path /cli/sew/recording/policy) - n0 ))/$(req_where /cli/sew/recording/policy token)" "1/$SCOPED"
echo owner_confirm_required > "$WORK/mode"
n0="$(count_path /cli/sew/recording/policy)"
run cell hostchat recording policy set dm_enabled on
assert_eq "agent set, daemon asks a confirm → exit 3" "$rc" "3"
assert_contains "agent set: the refusal is shown" "$err" "owner_confirm_required"
assert_eq "agent set: not resent" "$(( $(count_path /cli/sew/recording/policy) - n0 ))" "1"
assert_eq "agent: no vault confirm, ever" "$(count_path /cli/vault/confirm/start)" "$fin"

# No request carries a credential in its query string.
in_query="$(python3 -c '
import json, sys
print(sum(1 for l in open(sys.argv[1]) if "token" in json.loads(l)["query"]))' "$WORK/reqs.jsonl")"
assert_eq "no request has token= in its query" "$in_query" "0"

echo
echo "hostchat recording policy CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
