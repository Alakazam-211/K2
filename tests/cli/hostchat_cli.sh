#!/usr/bin/env bash
# k2 hostchat (Sew S29 K14) — the CLI over /cli/sew/*, against a stub daemon.
#
# Covers:
#   1. help without a daemon; the tool catalog (hostchat = id + locked, so a
#      K2 terminal sends its passport, never the owner token).
#   2. Request shapes: enable --identity standalone --voice --voice-url sends
#      {identity, voice:{publicUrl,...}} and waits on /cli/sew/status;
#      media install/url/probe/renew/uninstall; config --public-url (setup
#      links follow it; --setup-url is refused before any request);
#      users owner/invite/reset/revoke/reactivate/role/list.
#   3. A setup link prints to stdout only (never stderr); --json prints raw.
#   4. Exit codes: 400 → 2, 403/409 → 3, usage errors → 2 before any request.
#   5. Person-only steps refuse inside a K2 terminal before any request.
#   6. A release without a Sew pin: enable/preflight/upgrade/media install
#      exit 3 with sew_not_released; status says so plainly, exit 0.
#   7. upgrade sends tarballPath + mediaTarballPath, waits on /cli/sew/status
#      until the result, exits 0 only on "done"; a refusal exits 3 with
#      upgradeProgress.lastError (the reason) on stderr.
#   9. S30: config lists approvals.by and `config set approvals.by` goes to
#      the approvals policy (chat_cli.sh covers people/approvals/on-behalf).
#   8. Voice flags (--voice, --voice-url, --node-ip) outside enable, and
#      --media-tarball outside enable/upgrade/rollback, exit 2 before any
#      request (they used to be dropped silently).
#   9. media uninstall (0.46.3) asks on a terminal ([y/N]; n sends nothing),
#      refuses with confirm_required naming --yes when there is no
#      terminal, and --yes / -y send {"confirm": true}.
# Fake values only (example.test, made-up ids). Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-hostchat-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + catalog (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
set +e
out="$(env HOME="$HOME1" K2_PORT=1 "$K2_CLI" hostchat --help 2>&1)"; rc=$?
set -e
assert_eq "hostchat --help exit 0" "$rc" "0"
assert_contains "help: identity flag" "$out" "--identity k2|standalone"
assert_contains "help: identity is fixed" "$out" "changing it later means a fresh install"
assert_contains "help: voice url" "$out" "k2 hostchat media url <wss://HOST>"
assert_contains "help: users invite" "$out" "k2 hostchat users invite <name|email>"
assert_contains "help: manage sew" "$out" "chat_manage on"
assert_contains "help: people" "$out" "k2 hostchat people list"
assert_contains "help: approvals" "$out" "k2 hostchat approvals approve <out_…>"
assert_contains "help: approvals.by" "$out" "config set approvals.by people|people+agents"
assert_contains "help: on-behalf" "$out" "on-behalf approve <act_…|msg_…>"
assert_contains "help: restart" "$out" "k2 hostchat restart [--json]"
assert_contains "help: reconfigure" "$out" "k2 hostchat reconfigure [--json]"
assert_contains "help: agents alias" "$out" "alias of k2 chat access"
assert_contains "help: home client is the owner's" "$out" "never an admin or an agent, Manage Sew or not"
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "catalog: hostchat tool id" "$(_cli_tool_id_for_verb hostchat)" "hostchat"
assert_eq "catalog: hostchat is an id tool" "$(_cli_tool_default_mode hostchat)" "id"
if _cli_tool_is_locked hostchat; then ok "catalog: hostchat is locked"; else bad "catalog: hostchat is locked"; fi
schema="$(env HOME="$HOME1" "$K2_CLI" --schema)"
# Over stdin, never argv: the whole schema as one argument is past Linux's
# per-argument limit ("Argument list too long"). python3 reads it all before
# any assert, so printf never takes SIGPIPE under pipefail.
printf '%s' "$schema" | python3 -c '
import json, sys
names = {c["name"] for c in json.load(sys.stdin)["commands"]}
for n in ("hostchat", "hostchat enable", "hostchat media", "hostchat users", "hostchat config",
          "hostchat people", "hostchat approvals", "hostchat agents", "hostchat on-behalf", "hostchat log"):
    assert n in names, n
' && ok "schema lists the hostchat verbs" || bad "schema lists the hostchat verbs"

# ── 2. stub daemon ───────────────────────────────────────────────────
echo "== stub daemon =="
OWNER="owner-disk-token-hostchat"
SCOPED="sess42.scoped-secret-hostchat"
LINK="https://hearth.box.example.test/setup#token=sewpw_ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" "$LINK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped, link = sys.argv[1], sys.argv[2], sys.argv[3]
state = {"polls": 0}

def user(u, role="member"):
    return {"ok": True, "memberId": "48831765147549703", "username": u, "installRole": role, "deactivated": False,
            "created": True, "changed": True, "setupLink": {"url": link, "expiresAt": "2026-10-17T12:00:00Z"}}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        token = (q.get("token") or [""])[0]
        rec = {"method": method, "path": parsed.path, "token": token,
               "query": {k: v[0] for k, v in q.items()}, "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 200, {"ok": True}
        unreleased = os.path.exists(os.path.join(work, "unreleased"))
        msg = "Chat / Voice Hosting isn't available in this K2 release yet. It will be turned on in a later K2 update."
        if unreleased and p == "/cli/sew/status":
            out = {"ok": True, "supported": True, "state": "not-installed", "released": False, "notice": msg}
        elif unreleased and p in ("/cli/sew/server/enable", "/cli/sew/server/upgrade", "/cli/sew/preflight",
                                  "/cli/sew/media/install", "/cli/sew/media/preflight"):
            status, out = 409, {"ok": False, "error": {"code": "sew_not_released", "hint": msg}}
        elif p == "/cli/sew/status" and os.path.exists(os.path.join(work, "upgrade")):
            mode = open(os.path.join(work, "upgrade")).read().strip()
            state["upolls"] = state.get("upolls", 0) + 1
            busy = state["upolls"] < 2
            why = None if mode == "done" else "upgrade to 0.2.0 refused by Sew (exit 3): the tarball must hold exactly one regular file named sew"
            # Key-sorted like the daemon's JSON, several steps in one second.
            if busy:
                steps = {"stage": {"at": 1, "seq": 1}}
            elif mode == "done":
                steps = {"health": {"at": 2, "seq": 3}, "media": {"at": 2, "seq": 4}, "stage": {"at": 2, "seq": 1}, "upgrade": {"at": 2, "seq": 2}}
            else:
                # An older daemon: no seq, ties on whole seconds.
                steps = {"health": {"at": 2}, "media": {"at": 2}, "stage": {"at": 1}, "upgrade": {"at": 2}}
            out = {"ok": True, "supported": True, "state": "upgrading" if busy else "running", "lastError": why,
                   "upgradeProgress": {"from": "0.1.2", "to": "0.2.0", "current": "upgrade" if busy else None, "steps": steps,
                                       "result": None if busy else mode, "lastError": None if busy else why}}
        elif p == "/cli/sew/server/rollback":
            if b.get("mediaTarballPath"):
                out = {"ok": True, "state": "running", "version": "0.1.2", "media": {"result": "done", "to": "0.1.2"},
                       "doctor": {"ok": True, "mediaVersion": {"id": "media.version", "status": "ok", "detail": "sew-media 0.1.2"}}}
            else:
                out = {"ok": False, "state": "running", "version": "0.1.2", "media": {"result": "failed", "to": "0.1.2"},
                       "error": "Sew is back on 0.1.2, but its voice engine was not put back: not kept"}
        elif p == "/cli/sew/server/upgrade":
            state["upolls"] = 0
            out = {"ok": True, "state": "upgrading", "hint": "poll GET /cli/sew/status (upgradeProgress)"}
        elif p == "/cli/sew/status" and os.path.exists(os.path.join(work, "download")):
            # 0.46.7: enable with no --tarball, first poll mid-download.
            state["dpolls"] = state.get("dpolls", 0) + 1
            busy = state["dpolls"] < 2
            out = {"ok": True, "supported": True, "state": "installing" if busy else "running", "identityProvider": "k2",
                   "installedVersion": None if busy else "0.5.0", "alarms": [], "warnings": [],
                   "enableProgress": {"current": "stage" if busy else None,
                                      "download": {"file": "sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz", "label": "Sew 0.5.0",
                                                   "bytes": 12300000 if busy else 24667386, "total": 24667386, "source": "download"}}}
        elif p == "/cli/sew/status":
            state["polls"] += 1
            done = state["polls"] >= 2
            out = {"ok": True, "supported": True, "state": "running" if done else "installing", "identityProvider": "standalone",
                   "installedVersion": "0.1.0", "installId": "48831765147549701", "alarms": [], "warnings": [],
                   "enableProgress": {"current": None if done else "install"},
                   "voice": {"state": "running" if done else "installing", "publicUrl": "wss://hearth.box.example.test",
                             "turnHost": "turn-box.k2.dev", "nodeIp": "203.0.113.7", "cert": {"daysLeft": 89},
                             "turnTls": {"served": "ok"}, "progress": {"result": "done" if done else "running", "step": None if done else "turn-cert"}}}
        elif p == "/cli/sew/server/enable":
            if b.get("identity") not in (None, "standalone", "k2"):
                status, out = 400, {"ok": False, "error": {"code": "bad_request", "hint": "identity is k2 or standalone"}}
            else:
                out = {"ok": True, "state": "installing", "voice": "voice" in b}
        elif p == "/cli/sew/media/install":
            out = {"ok": True, "voice": "installing"}
        elif p == "/cli/sew/media/url":
            if not b.get("publicUrl", "").startswith("wss://"):
                status, out = 400, {"ok": False, "error": {"code": "bad_request", "hint": "voice URL must be wss://<host>"}}
            elif "evil" in b["publicUrl"]:
                status, out = 400, {"ok": False, "error": {"code": "url_not_on_this_box", "hint": "not on this box"}}
            else:
                out = {"ok": True, "publicUrl": b["publicUrl"], "applied": True, "changed": True}
        elif p == "/cli/sew/server/config":
            out = {"ok": True, "set": {k: v for k, v in b.items()}, "restarted": True}
        elif p.startswith("/cli/sew/users/"):
            verb = p.rsplit("/", 1)[1]
            if token == scoped and b.get("user") == "locked":
                status, out = 403, {"ok": False, "error": {"code": "chat_manage_off", "hint": "\"Manage Sew\" is off"}}
            elif verb == "revoke":
                out = {"ok": True, "username": b["user"], "sessionsRevoked": 2}
            elif verb == "invite" and b.get("user") == "taken":
                status, out = 409, {"ok": False, "error": {"code": "username_taken", "hint": "that username is taken"}}
            else:
                out = user(b.get("user"), b.get("role") or ("owner" if verb == "owner" else "member"))
        elif p == "/cli/sew/approvals/policy":
            out = {"ok": True, "approvalsBy": "people", "canChange": True, "allowed": ["people", "people+agents"]}
        elif p == "/cli/sew/approvals/policy/set":
            out = {"ok": True, "approvalsBy": b.get("approvalsBy")}
        elif p == "/cli/sew/users":
            out = {"ok": True, "users": [{"username": "rosson", "installRole": "owner", "kind": "person", "displayName": "Rosson"},
                                         {"username": "bob", "installRole": "member", "kind": "person", "deactivated": True}]}
        elif p.startswith("/cli/sew/media/") or p.startswith("/cli/sew/server/"):
            out = {"ok": True, "echo": b}
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

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
# Requests to /cli/sew/* (a K2 terminal's CLI also asks /cli/whoami).
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then grep -c '"path": "/cli/sew/' "$WORK/reqs.jsonl" || true; else echo 0; fi; }
req_where() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = [r for r in rows if r["path"] == sys.argv[2]]
r = rows[-1]
print(r[sys.argv[3]] if sys.argv[3] != "body" else json.dumps(json.loads(r["body"] or "{}"), sort_keys=True))
' "$WORK/reqs.jsonl" "$1" "$2"
}
run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
mkdir -p "$WORK/proj"
cd "$WORK"
touch sew.tar.gz sew-media.tar.gz

echo "== enable standalone + voice =="
run owner hostchat enable --tarball sew.tar.gz --identity standalone --voice-url wss://hearth.box.example.test --media-tarball sew-media.tar.gz
assert_eq "enable exit 0" "$rc" "0"
assert_eq "enable is a POST with the owner token" "$(req_where /cli/sew/server/enable method)/$(req_where /cli/sew/server/enable token)" "POST/$OWNER"
body="$(req_where /cli/sew/server/enable body)"
assert_contains "enable: identity standalone" "$body" '"identity": "standalone"'
assert_contains "enable: voice url" "$body" '"publicUrl": "wss://hearth.box.example.test"'
assert_contains "enable: media tarball absolute" "$body" "\"mediaTarballPath\": \"$WORK/sew-media.tar.gz\""
assert_contains "enable: tarball absolute" "$body" "\"tarballPath\": \"$WORK/sew.tar.gz\""
assert_contains "enable: identity is fixed (stderr)" "$err" "fixed for this install"
assert_contains "enable waited for status" "$out" "running"
assert_contains "enable: voice summary" "$out" "wss://hearth.box.example.test"
assert_contains "enable: turn line" "$out" "turn-box.k2.dev:5349"

run owner hostchat enable --identity ldap
assert_eq "bad identity → exit 2" "$rc" "2"

echo "== enable with no flags downloads (0.46.7) =="
touch "$WORK/download"
run owner hostchat enable
rm -f "$WORK/download"
assert_eq "enable (no flags) exit 0" "$rc" "0"
body="$(req_where /cli/sew/server/enable body)"
assert_eq "enable (no flags): no path sent" "$body" '{}'
assert_contains "enable (no flags): download progress on stderr" "$err" "installing/stage (downloading Sew 0.5.0: 12.3 of 24.7 MB)"

echo "== help says where Sew comes from =="
out="$(env HOME="$HOME1" K2_PORT=1 "$K2_CLI" hostchat --help 2>&1)"
assert_contains "help: download source" "$out" "github.com/Alakazam-211/sew-releases"
assert_contains "help: tarball is for offline boxes" "$out" "are for a box without"
if printf '%s' "$out" | grep -q "SW1\|has not published"; then bad "help: no stale SW1 words"; else ok "help: no stale SW1 words"; fi

# 0.46.1: voice flags outside enable used to be dropped silently.
before="$(nreq)"
# upgrade and rollback take --media-tarball (Sew's upgrade contract);
# everywhere else it, like the other voice flags, is refused.
run owner hostchat doctor --media-tarball sew-media.tar.gz
assert_eq "doctor --media-tarball → exit 2" "$rc" "2"
assert_contains "doctor --media-tarball names the real verb" "$err" "k2 hostchat media install --tarball FILE"
assert_contains "doctor --media-tarball names the verbs that take it" "$err" "enable, upgrade and rollback"
run owner hostchat upgrade --tarball sew.tar.gz --voice-url wss://hearth.box.example.test
assert_eq "upgrade --voice-url → exit 2" "$rc" "2"
assert_contains "upgrade --voice-url says voice flags are enable's" "$err" "only apply to k2 hostchat enable"
run owner hostchat upgrade --tarball sew.tar.gz --node-ip 203.0.113.7
assert_eq "upgrade --node-ip → exit 2" "$rc" "2"
assert_eq "voice-flag refusals send nothing" "$(nreq)" "$before"

echo "== voice =="
run owner hostchat media install --url wss://hearth.box.example.test --node-ip 203.0.113.7 --tarball sew-media.tar.gz --no-wait
assert_eq "media install exit 0" "$rc" "0"
assert_eq "media install body" "$(req_where /cli/sew/media/install body)" \
    "{\"mediaTarballPath\": \"$WORK/sew-media.tar.gz\", \"nodeIp\": \"203.0.113.7\", \"publicUrl\": \"wss://hearth.box.example.test\"}"
run owner hostchat media url wss://hearth.box.example.test
assert_eq "media url exit 0" "$rc" "0"
assert_eq "media url body" "$(req_where /cli/sew/media/url body)" '{"publicUrl": "wss://hearth.box.example.test"}'
run owner hostchat media url wss://evil.example
assert_eq "foreign voice url → exit 2" "$rc" "2"
assert_contains "foreign voice url says why" "$err" "url_not_on_this_box"
before="$(nreq)"
run owner hostchat media probe --udp ok --tcp ok
assert_eq "probe without --turn-tls → exit 2" "$rc" "2"
assert_eq "probe usage sends nothing" "$(nreq)" "$before"
run owner hostchat media probe --udp ok --tcp fail --turn-tls ok
assert_eq "probe exit 0" "$rc" "0"
assert_eq "probe body" "$(req_where /cli/sew/media/probe body)" '{"tcp": "fail", "turnTls": "ok", "udp": "ok"}'
run owner hostchat media renew
assert_eq "renew is a POST" "$(req_where /cli/sew/media/renew method)" "POST"
# 0.46.3: media uninstall confirms (Sew: "Live calls end."). No terminal and
# no --yes refuses before any request; --yes / -y send confirm: true.
before="$(nreq)"
run owner hostchat media uninstall </dev/null
assert_eq "media uninstall, no tty, no --yes → exit 2" "$rc" "2"
assert_contains "no-tty refusal says confirm_required" "$err" '"error":"confirm_required"'
assert_contains "no-tty refusal names --yes" "$err" "pass --yes"
assert_contains "no-tty refusal says what it costs" "$err" "Live calls end."
run owner hostchat media uninstall --json </dev/null
assert_eq "media uninstall --json, no tty → exit 2" "$rc" "2"
assert_eq "no-tty refusals send nothing" "$(nreq)" "$before"
run owner hostchat media status --yes </dev/null
assert_eq "--yes on another media verb → exit 2" "$rc" "2"
assert_contains "--yes elsewhere names the verb" "$err" "only applies to k2 hostchat media uninstall"
assert_eq "--yes elsewhere sends nothing" "$(nreq)" "$before"
run owner hostchat media uninstall --yes --json </dev/null
assert_eq "media uninstall --yes exit 0" "$rc" "0"
assert_eq "media uninstall is a POST" "$(req_where /cli/sew/media/uninstall method)" "POST"
assert_eq "media uninstall --yes body" "$(req_where /cli/sew/media/uninstall body)" '{"confirm": true}'
before="$(nreq)"
run owner hostchat media uninstall -y </dev/null
assert_eq "media uninstall -y exit 0" "$rc" "0"
assert_eq "media uninstall -y sends one request" "$(nreq)" "$((before + 1))"
assert_eq "media uninstall -y body" "$(req_where /cli/sew/media/uninstall body)" '{"confirm": true}'
# On a terminal it asks; y sends the confirm, anything else sends nothing.
tty_run() {
    # tty_run <answer> <k2 args...>: run the CLI on a pseudo-terminal, type
    # <answer> + Enter at the prompt; rc + everything it printed.
    python3 - "$@" <<'PYEOF'
import os, pty, sys, select, time
answer, argv = sys.argv[1], sys.argv[2:]
pid, fd = pty.fork()
if pid == 0:
    os.execvp(argv[0], argv)
buf, sent, deadline = b"", False, time.time() + 30
while time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            break
        if not chunk:
            break
        buf += chunk
    if not sent and b"[y/N]" in buf:
        os.write(fd, answer.encode() + b"\n")
        sent = True
_, status = os.waitpid(pid, 0)
sys.stdout.write(buf.decode("utf-8", "replace"))
sys.stdout.write("\nRC=%d SENT=%s\n" % (os.waitstatus_to_exitcode(status), sent))
PYEOF
}
before="$(nreq)"
tout="$(tty_run n env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" hostchat media uninstall)"
assert_contains "tty: asks Sew's question" "$tout" "Uninstall voice? Live calls end. [y/N]"
assert_contains "tty: n → exit 1, prompt answered" "$tout" "RC=1 SENT=True"
assert_contains "tty: n says nothing was removed" "$tout" "Voice not removed."
assert_eq "tty: n sends nothing" "$(nreq)" "$before"
tout="$(tty_run y env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" hostchat media uninstall)"
assert_contains "tty: y → exit 0" "$tout" "RC=0 SENT=True"
assert_eq "tty: y sends one request" "$(nreq)" "$((before + 1))"
assert_eq "tty: y body" "$(req_where /cli/sew/media/uninstall body)" '{"confirm": true}'

echo "== config =="
run owner hostchat config --public-url https://hearth.box.example.test
assert_eq "config exit 0" "$rc" "0"
assert_eq "config body" "$(req_where /cli/sew/server/config body)" '{"publicUrl": "https://hearth.box.example.test"}'
before="$(nreq)"
run owner hostchat config --setup-url https://hearth.box.example.test/setup
assert_eq "--setup-url → exit 2" "$rc" "2"
assert_contains "--setup-url says links follow the public URL" "$err" "<public-url>/setup"
assert_eq "--setup-url sends nothing" "$(nreq)" "$before"
run owner hostchat config bogus
assert_eq "config with a stray word → exit 2" "$rc" "2"
run owner hostchat config
assert_eq "config (no args) lists the keys" "$rc/$(req_where /cli/sew/approvals/policy method)" "0/GET"
assert_contains "config shows approvals.by" "$out" "approvals.by         people"
run owner hostchat config set approvals.by people+agents
assert_eq "config set approvals.by → the approvals policy" "$(req_where /cli/sew/approvals/policy/set body)" '{"approvalsBy": "people+agents"}'
run owner hostchat config set default_space hearth
assert_eq "config set default_space → the approvals policy" "$rc/$(req_where /cli/sew/approvals/policy/set body)" '0/{"defaultSpace": "hearth"}'
run owner hostchat config set default_space
assert_eq "config set default_space without a value → exit 2" "$rc" "2"
run owner hostchat config
assert_contains "config shows default_space" "$out" "default_space"
run owner hostchat config set server.public_url https://hearth.box.example.test
assert_eq "config set server.public_url = --public-url" "$(req_where /cli/sew/server/config body)" '{"publicUrl": "https://hearth.box.example.test"}'
run owner hostchat config set link_previews off
assert_eq "config set: unknown key → exit 2" "$rc" "2"

echo "== users =="
run owner hostchat users owner rosson --display-name "Rosson Long"
assert_eq "users owner exit 0" "$rc" "0"
assert_eq "owner body" "$(req_where /cli/sew/users/owner body)" '{"displayName": "Rosson Long", "user": "rosson"}'
assert_contains "the setup link prints to stdout" "$out" "$LINK"
assert_absent "the setup link never reaches stderr" "$err" "sewpw_"
run owner hostchat users invite ana@acme.example.test --admin
assert_eq "invite body" "$(req_where /cli/sew/users/invite body)" '{"role": "admin", "user": "ana@acme.example.test"}'
assert_contains "invite: link on stdout" "$out" "$LINK"
run owner hostchat users invite bob --role guest --email bob@acme.example.test --json
assert_eq "invite --json exit 0" "$rc" "0"
assert_eq "invite --json is the raw body" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["setupLink"]["url"])')" "$LINK"
assert_eq "invite with role + email" "$(req_where /cli/sew/users/invite body)" '{"email": "bob@acme.example.test", "role": "guest", "user": "bob"}'
run owner hostchat users invite taken
assert_eq "username_taken → exit 3" "$rc" "3"
assert_contains "username_taken says so" "$err" "username_taken"
run owner hostchat users reset bob
assert_contains "reset prints a new link" "$out" "$LINK"
run owner hostchat users revoke bob
assert_contains "revoke reports ended sessions" "$out" "sessions ended:  2"
run owner hostchat users reactivate bob
assert_eq "reactivate body" "$(req_where /cli/sew/users/reactivate body)" '{"user": "bob"}'
run owner hostchat users role bob admin
assert_eq "role body" "$(req_where /cli/sew/users/role body)" '{"role": "admin", "user": "bob"}'
before="$(nreq)"
run owner hostchat users role bob
assert_eq "role without a role → exit 2" "$rc" "2"
run owner hostchat users invite
assert_eq "invite without a name → exit 2" "$rc" "2"
assert_eq "usage errors send nothing" "$(nreq)" "$before"
run owner hostchat users list
assert_contains "list shows users" "$out" "rosson"
assert_contains "list marks deactivated" "$out" "(deactivated)"

echo "== K2 terminal (agent) =="
run cell hostchat users invite locked
assert_eq "agent: refusal → exit 3" "$rc" "3"
assert_eq "agent sends its passport, never the owner token" "$(req_where /cli/sew/users/invite token)" "$SCOPED"
assert_contains "agent: Manage Sew hint" "$err" "chat_manage_off"
run cell hostchat media url wss://hearth.box.example.test
assert_eq "agent media url uses the passport" "$(req_where /cli/sew/media/url token)" "$SCOPED"
before="$(nreq)"
run cell hostchat recovery-key
assert_eq "agent: recovery key refused (exit 3)" "$rc" "3"
assert_contains "agent: person only" "$err" "person_only"
run cell hostchat purge --confirm-hostname box
assert_eq "agent: purge refused (exit 3)" "$rc" "3"
run cell hostchat rotate-master --drop-prev
assert_eq "agent: drop-prev refused (exit 3)" "$rc" "3"
assert_eq "person-only refusals send nothing" "$(nreq)" "$before"
# Sew 0.6.0: the governance switch. Usage errors send nothing; inside K2 it
# asks no confirm and sends the passport (the daemon answers person_only).
run cell hostchat app-space governance jobs 5101
assert_eq "governance without app|sew → exit 2" "$rc/$(nreq)" "2/$before"
run cell hostchat app-space governance jobs 5101 maybe
assert_eq "governance maybe → exit 2" "$rc/$(nreq)" "2/$before"
run cell hostchat app-space governance jobs 5101 app
assert_eq "governance from K2 sends the passport" "$(req_where /cli/sew/app-spaces/governance token)" "$SCOPED"
assert_eq "governance body (no confirm inside K2)" "$(req_where /cli/sew/app-spaces/governance body)" '{"app": "jobs", "governance": "app", "spaceId": "5101"}'

echo "== release without a Sew pin =="
touch "$WORK/unreleased"
NR="Chat / Voice Hosting isn't available in this K2 release yet. It will be turned on in a later K2 update."
run owner hostchat status
assert_eq "unreleased: status exit 0" "$rc" "0"
assert_eq "unreleased: status says so plainly" "$out" "Chat / Voice Hosting: $NR"
for verb in "enable --tarball sew.tar.gz" "preflight" "upgrade --tarball sew.tar.gz" "media install --tarball sew-media.tar.gz --no-wait" "media preflight"; do
    # shellcheck disable=SC2086
    run owner hostchat $verb
    assert_eq "unreleased: $verb → exit 3" "$rc" "3"
    assert_contains "unreleased: $verb says sew_not_released" "$err" '"error": "sew_not_released"'
    assert_contains "unreleased: $verb gives the message" "$err" "$NR"
done
rm -f "$WORK/unreleased"

echo "== upgrade waits for the result and carries the reason =="
echo refused > "$WORK/upgrade"
run owner hostchat upgrade --tarball sew.tar.gz --media-tarball sew-media.tar.gz
assert_eq "upgrade refused → exit 3" "$rc" "3"
body="$(req_where /cli/sew/server/upgrade body)"
assert_contains "upgrade: tarball absolute" "$body" "\"tarballPath\": \"$WORK/sew.tar.gz\""
assert_contains "upgrade: media tarball absolute" "$body" "\"mediaTarballPath\": \"$WORK/sew-media.tar.gz\""
assert_contains "upgrade refused: result on stderr" "$err" '"error": "upgrade_refused"'
assert_contains "upgrade refused: Sew's reason on stderr" "$err" "the tarball must hold exactly one regular file named sew"
assert_contains "upgrade: completed steps are shown" "$err" "… upgrade: done"
assert_contains "upgrade: the media step is shown" "$err" "… media: done"
assert_eq "upgrade (no seq, ties): steps in execution order" "$(printf '%s\n' "$err" | grep -o '^… [a-z]*: done' | tr '\n' ' ')" "… stage: done … upgrade: done … health: done … media: done "
echo done > "$WORK/upgrade"
run owner hostchat upgrade --tarball sew.tar.gz
assert_eq "upgrade done → exit 0" "$rc" "0"
assert_contains "upgrade done: from → to" "$out" "upgraded: 0.1.2 -> 0.2.0"
assert_eq "upgrade (seq): steps in execution order" "$(printf '%s\n' "$err" | grep -o '^… [a-z]*: done' | tr '\n' ' ')" "… stage: done … upgrade: done … health: done … media: done "
assert_absent "upgrade done: no media path sent" "$(req_where /cli/sew/server/upgrade body)" "mediaTarballPath"
rm -f "$WORK/upgrade"

echo "== rollback carries the previous engine and fails loudly =="
run owner hostchat rollback --media-tarball sew-media.tar.gz
assert_eq "rollback ok → exit 0" "$rc" "0"
assert_contains "rollback: media tarball absolute" "$(req_where /cli/sew/server/rollback body)" "\"mediaTarballPath\": \"$WORK/sew-media.tar.gz\""
assert_contains "rollback: doctor shown" "$out" "sew-media 0.1.2"
run owner hostchat rollback
assert_eq "rollback not ok → exit 3" "$rc" "3"
assert_contains "rollback not ok: the reason on stderr" "$err" "voice engine was not put back"

echo "== upgrade --accept-binary (0.46.5 item 12): checked here, a person's =="
before="$(nreq)"
run owner hostchat upgrade --tarball sew.tar.gz --accept-binary 7ed8
assert_eq "accept-binary: short sha → exit 2" "$rc" "2"
assert_contains "accept-binary: short sha says why" "$err" "64 hex characters"
run owner hostchat upgrade --tarball sew.tar.gz --accept-binary zz8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385
assert_eq "accept-binary: not hex → exit 2" "$rc" "2"
run cell hostchat upgrade --tarball sew.tar.gz --accept-binary 7ed8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385
assert_eq "accept-binary from a K2 terminal → exit 3" "$rc" "3"
assert_contains "accept-binary from a K2 terminal: person only" "$err" '"error":"person_only"'
assert_eq "accept-binary refusals sent nothing to /cli/sew" "$(nreq)" "$before"

echo
echo "hostchat CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
