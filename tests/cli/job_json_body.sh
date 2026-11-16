#!/usr/bin/env bash
# k2 job: what each verb sends the daemon, and --json on every verb, against
# a tiny stub daemon on loopback (python3 http.server). No real daemon, no
# ~/.k2, no keychain: HOME is a throwaway dir. The real daemon's side is
# tests/cli/job_headless.sh.
#
# Covers: each verb's route and body or query (--workspace, --every, --cron,
# --timeout, --cwd, --run, --all, the command after --); --json prints the
# daemon's JSON as it came, on every verb; without --json a verb prints its
# own line; a refusal still exits non-zero with --json and prints nothing
# on stdout; a verb that needs NAME and has none is a usage error (exit 2)
# that sends nothing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2job-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
JOB = {"name": "nightly", "workspace": "acme", "schedule": "every 15m", "command": "sh -c 'echo hi'", "enabled": True}

def answer(path, body):
    verb = path.rsplit("/", 1)[-1]
    if body.get("name") == "theirs":
        return 403, {"error": {"code": "job_not_yours", "hint": "stub refusal"}}
    if verb in ("add", "edit", "enable", "disable") or (verb in ("pause", "resume") and not body.get("all")):
        return 200, {"ok": True, "stub": verb, "job": JOB}
    if verb in ("pause", "resume"):
        return 200, {"ok": True, "stub": verb, "paused": verb == "pause"}
    if verb in ("run", "kill"):
        return 200, {"ok": True, "stub": verb, "job": "nightly", "run": "r1"}
    if verb == "remove":
        return 200, {"ok": True, "stub": verb, "removed": "nightly", "grantsRevoked": 2}
    if verb == "list":
        return 200, {"stub": verb, "jobs": [], "paused": False}
    if verb == "show":
        return 200, {"stub": verb, "job": JOB}
    if verb == "logs":
        return 200, {"stub": verb, "job": "nightly", "run": {"id": "r1"}, "log": "hi\n", "truncated": False}
    return 404, {"error": {"code": "not_found", "hint": path}}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def record(self, method, path, sent):
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": method, "path": path, "sent": sent}, sort_keys=True) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items() if k != "token"}
        self.record("GET", u.path, q)
        self.reply(*answer(u.path, q))
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        self.record("POST", u.path, body)
        self.reply(*answer(u.path, body))

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

# The last request the stub saw: METHOD path {sent}.
last() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["method"], v["path"], json.dumps(v["sent"], sort_keys=True))'; }
# `"stub": "<verb>"` from a --json answer: proof the daemon's JSON came out as it was sent.
stub_of() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])["stub"])' "$1"; }

echo "== each verb: its route and what it sends, and --json prints the daemon's JSON =="
out="$("$K2" job add nightly --workspace acme --every 15m --timeout 5m --cwd scripts --json -- sh -c 'echo hi')"
expect_eq "add sends the schedule, timeout, folder and argv" "$(last)" \
    'POST /cli/jobs/add {"argv": ["sh", "-c", "echo hi"], "cwd": "scripts", "every": "15m", "name": "nightly", "timeout": "5m", "workspace": "acme"}'
expect_eq "add --json" "$(stub_of "$out")" "add"

out="$("$K2" job edit nightly --workspace acme --cron '0 9 * * 1' --json)"
expect_eq "edit sends the cron schedule" "$(last)" \
    'POST /cli/jobs/edit {"cron": "0 9 * * 1", "name": "nightly", "workspace": "acme"}'
expect_eq "edit --json" "$(stub_of "$out")" "edit"

out="$("$K2" job edit nightly --workspace acme --cwd . --json)"
expect_eq "edit --cwd . sends the folder (the workspace folder)" "$(last)" \
    'POST /cli/jobs/edit {"cwd": ".", "name": "nightly", "workspace": "acme"}'

for verb in run enable disable kill; do
    out="$("$K2" job "$verb" nightly --workspace acme --json)"
    expect_eq "$verb sends the name and workspace" "$(last)" \
        "POST /cli/jobs/$verb {\"name\": \"nightly\", \"workspace\": \"acme\"}"
    expect_eq "$verb --json" "$(stub_of "$out")" "$verb"
done

out="$("$K2" job run-now nightly --workspace acme --json)"
expect_eq "run-now is run" "$(last)" 'POST /cli/jobs/run {"name": "nightly", "workspace": "acme"}'

out="$("$K2" job rm nightly --workspace acme --json)"
expect_eq "rm posts to remove" "$(last)" 'POST /cli/jobs/remove {"name": "nightly", "workspace": "acme"}'
expect_eq "rm --json" "$(stub_of "$out")" "remove"

for verb in pause resume; do
    out="$("$K2" job "$verb" nightly --workspace acme --json)"
    expect_eq "$verb NAME sends the name" "$(last)" \
        "POST /cli/jobs/$verb {\"name\": \"nightly\", \"workspace\": \"acme\"}"
    expect_eq "$verb NAME --json" "$(stub_of "$out")" "$verb"
    out="$("$K2" job "$verb" --all --json)"
    expect_eq "$verb --all sends all" "$(last)" \
        "POST /cli/jobs/$verb {\"all\": true, \"workspace\": \"$SANDBOX\"}"
    expect_eq "$verb --all --json" "$(stub_of "$out")" "$verb"
done

out="$("$K2" job list --all --json)"
expect_eq "list --all asks for all" "$(last)" "GET /cli/jobs/list {\"all\": \"1\", \"workspace\": \"$SANDBOX\"}"
expect_eq "list --json" "$(stub_of "$out")" "list"

out="$("$K2" job show nightly --workspace acme --json)"
expect_eq "show asks for the name" "$(last)" 'GET /cli/jobs/show {"name": "nightly", "workspace": "acme"}'
expect_eq "show --json" "$(stub_of "$out")" "show"

out="$("$K2" job logs nightly --workspace acme --run r1 --json)"
expect_eq "logs --run asks for that run" "$(last)" 'GET /cli/jobs/logs {"name": "nightly", "run": "r1", "workspace": "acme"}'
expect_eq "logs --json" "$(stub_of "$out")" "logs"

echo "== without --json, a verb prints its own line =="
expect_eq "add" "$("$K2" job add nightly --workspace acme --every 15m -- true | head -1)" \
    "added nightly: every 15m, runs sh -c 'echo hi'"
expect_eq "rm" "$("$K2" job rm nightly --workspace acme)" "removed nightly (2 secret grant(s) removed)"
expect_eq "pause --all" "$("$K2" job pause --all)" "every job is paused"

echo "== a refusal with --json: exit 3, the error on stderr, nothing on stdout =="
set +e
out="$("$K2" job run theirs --workspace acme --json 2>"$SANDBOX/err")"
rc=$?
set -e
expect_eq "exit code" "$rc" "3"
expect_eq "stdout" "$out" ""
expect_eq "stderr" "$(cat "$SANDBOX/err")" "job_not_yours: stub refusal"

echo "== a verb that needs NAME and has none: usage, exit 2, nothing sent =="
before="$(wc -l < "$LOG")"
for verb in show logs run enable disable kill rm; do
    set +e
    "$K2" job "$verb" --workspace acme --json >/dev/null 2>"$SANDBOX/err"
    rc=$?
    set -e
    expect_eq "$verb with no NAME exits 2" "$rc" "2"
done
set +e
"$K2" job pause --json >/dev/null 2>&1
rc=$?
set -e
expect_eq "pause with no NAME and no --all exits 2" "$rc" "2"
expect_eq "nothing reached the daemon" "$(wc -l < "$LOG")" "$before"

echo
echo "job_json_body: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
