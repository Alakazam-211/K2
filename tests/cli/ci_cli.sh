#!/usr/bin/env bash
# k2 ci (0.46.10, prd-ci-on-compute-for-clients-v1 C6a): the thin CLI over
# /cli/ci/*. A python stub daemon on loopback records every request and
# answers like the daemon; the daemon side (the engine, refusals, the
# vault token, statuses) is crates/k2-daemon/tests/it_compute/ci_integration.rs.
#
# Covers: help and the study page with no daemon (what a blind agent reads
# first), usage errors (exit 2, nothing sent), each verb's request body
# (init's argv with commas kept, --mem 8G, --poll 2m, --until 3d, the
# workspace this folder is in), the renders agents read, run --wait's exit
# (1 on a failed run), logs through k2 compute logs, and refusal exit codes
# (2 / 3 / 4) with `code: hint` on stderr.
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi; }
eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

WORK="$(mktemp -d -t k2-ci-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

echo "== no daemon: help and study =="
help="$("$K2" ci --help)"
has "help: init" "$help" "k2 ci init [--check NAME]"
has "help: run --wait" "$help" "[--wait [--wait-min 60]]"
has "help: watch on" "$help" "k2 ci watch on [--remote origin]"
has "help: grants" "$help" "--tier ci needs"
has "help: study pointer" "$help" "More: k2 study ci"
eq "k2 ci alone prints help" "$("$K2" ci | head -1)" "$(head -1 <<<"$help")"
study="$("$K2" study ci)"
has "study: the steps" "$study" "SET UP CI FOR THIS REPO"
has "study: step 2 exact" "$study" "k2 ci init --check test --needs rust@stable --os linux --mem 8 -- cargo test --locked"
has "study: commit it" "$study" "git add .k2/ci.toml && git commit"
has "study: run" "$study" "k2 ci run --wait"
has "study: a Node recipe" "$study" "--needs node@22 ... -- sh -c 'npm ci && npm test'"
has "study: mem is GB" "$study" "--mem is GB (8 = 8 GB)"
has "study: watch" "$study" "k2 ci watch on"
has "study: the person's grant" "$study" "k2 compute grant <node> --workspace <workspace> --ci"
has "study: the token line" "$study" "k2 vault set K2_CI_STATUSES_TOKEN --preset github --scope ws:<workspace> < token.txt"
has "study: no --ci fallback" "$study" "k2 ci run --tier standard --wait"
for c in ci_no_config ci_config_invalid ci_config_exists ci_check_not_found ci_commit_not_found ci_not_a_repo ci_remote_unreachable ci_watch_invalid compute_off ci_not_allowed; do
    has "study: $c" "$study" "$c"
done
n="$(printf '%s\n' "$study" | wc -l | tr -d ' ')"
if [ "$n" -le 110 ]; then ok "study ci is bounded ($n lines)"; else bad "study ci is $n lines (max 110)"; fi
has "catalog row" "$("$K2" study)" "  ci                  CI on compute: .k2/ci.toml, k2 ci run/watch, GitHub statuses"
schema="$("$K2" --schema)"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); c=[x for x in v["commands"] if x["name"]=="ci"]; assert len(c)==1 and "k2 study ci" in c[0]["description"], c' "$schema" && ok "schema has ci" || bad "schema has ci"

# ── the stub daemon ──────────────────────────────────────────────────
REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"
python3 - "$WORK" <<'PYEOF' &
import base64, json, sys, time, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
SHA = "1a2b3c4d5e6f" + "0" * 28
def run(state, checks=None):
    return {"id": "ab12cd34" + "0" * 24, "short": "ab12cd34", "sha": SHA, "ref": "main", "trigger": "manual",
            "state": state, "detail": None, "by": "owner", "tier": "ci", "createdAt": int(time.time()) - 30,
            "endedAt": None, "statuses": {"slug": "acme/app", "context": "k2-ci", "tokenSecret": "K2_CI_STATUSES_TOKEN"},
            "checks": checks if checks is not None else [
                {"name": "unit", "job": "jobaaaaa1111", "state": "success" if state != "running" else "running", "description": "job jobaaaaa on k2n7, 2m", "status": "success", "statusError": None},
                {"name": "lint", "job": None, "state": "error", "description": "not submitted: ci_not_allowed: x", "status": None, "statusError": "no secret K2_CI_STATUSES_TOKEN"}]}
polls = {"n": 0}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def record(self, body):
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.log", "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path, "query": u.query, "body": body}) + "\n")
        return u.path, dict(urllib.parse.parse_qsl(u.query))
    def do_GET(self):
        n = int(self.headers.get("Content-Length") or 0)
        extra = self.rfile.read(n).decode() if n else ""
        path, q = self.record(extra)
        q.update(dict(urllib.parse.parse_qsl(extra)))
        if path == "/cli/ci/status":
            r = q.get("run")
            if r == "waitrun" or r == "ab12cd34" + "0" * 24:
                polls["n"] += 1
                return self.reply({"workspace": "proj", "run": run("running" if polls["n"] < 2 else "failed")})
            if r:
                return self.reply({"workspace": "proj", "run": run("failed")})
            return self.reply({"workspace": "proj", "watch": {"on": True, "remote": "origin", "branches": ["main", "integ/*"], "pollSecs": 60, "tier": "ci", "maxInflight": 2, "by": "owner", "at": 1, "lastPollAt": int(time.time()) - 20, "lastError": None},
                               "freeze": None, "inflight": 0, "runs": [run("failed")]})
        if path == "/cli/compute/jobs/get":
            return self.reply({"id": q.get("job"), "result": {}})
        if path == "/cli/compute/jobs/logs":
            if q.get("cursor") == "0":
                return self.reply({"cursor": 1, "chunks": [{"data": base64.b64encode(b"hello from unit\n").decode()}]})
            return self.reply({"cursor": 1, "chunks": []})
        self.reply({"error": {"code": "not_found", "hint": "GET " + path}}, 404)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path, q = self.record(body)
        v = json.loads(body or "{}")
        if path == "/cli/ci/run":
            refusals = {"nocfg": (404, "ci_no_config"), "forbid": (403, "compute_off"), "bad": (400, "bad_request"), "busy": (409, "ci_not_a_repo"), "dark": (409, "ci_preview_off")}
            if v.get("rev") in refusals:
                st, code = refusals[v["rev"]]
                return self.reply({"error": {"code": code, "hint": "the hint for " + code}}, st)
            return self.reply({"workspace": "proj", "run": run("running")})
        if path == "/cli/ci/watch":
            if not v.get("on"):
                return self.reply({"workspace": "proj", "watch": None, "changed": True})
            return self.reply({"workspace": "proj", "watch": {"on": True, "remote": "origin", "branches": v.get("branches") or ["main"], "pollSecs": v.get("pollSecs", 60), "tier": "ci", "maxInflight": 2, "lastPollAt": None, "lastError": None},
                               "tips": [{"branch": "main", "sha": SHA}], "skippedCurrent": bool(v.get("skipCurrent"))})
        if path == "/cli/ci/init":
            return self.reply({"workspace": "proj", "path": work + "/proj/.k2/ci.toml", "config": "version = 1\n", "placeholder": "argv" not in v, "ignored": False})
        if path == "/cli/ci/freeze":
            if v.get("off"):
                return self.reply({"workspace": "proj", "freeze": None})
            return self.reply({"workspace": "proj", "freeze": {"until": v["until"], "why": v.get("why"), "by": "owner", "at": 1}})
        self.reply({"error": {"code": "not_found", "hint": "POST " + path}}, 404)
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# The last request to <path>: its body (POST) or query (GET).
last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
if not rows:
    sys.exit("no request to " + sys.argv[2])
r = rows[-1]
print(r["body"] if r["method"] == "POST" else r["query"] + "&" + r["body"])' "$REQ_LOG" "$1"
}
field() { python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1]).get(sys.argv[2])))' "$1" "$2"; }
count() { grep -c "\"path\": \"$1\"" "$REQ_LOG" || true; }

# rc <label> <want> <args…>: the exit code; stderr in $WORK/err.
rc() {
    local label="$1" want="$2" got
    shift 2
    set +e
    "$K2" "$@" >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "$label exits $want" "$got" "$want"
}

echo "== usage errors send nothing =="
before="$(wc -l <"$REQ_LOG")"
rc "unknown verb" 2 ci frob
has "unknown verb says so" "$(cat "$WORK/err")" "unknown k2 ci verb 'frob'"
rc "bad tier" 2 ci run --tier bulk
rc "freeze without --until" 2 ci freeze
rc "freeze bad --until" 2 ci freeze --until soon
rc "--mem not a number" 2 ci init --mem lots
rc "watch bogus" 2 ci watch maybe
rc "logs without a run" 2 ci logs
rc "unknown flag" 2 ci status --bogus
rc "init -- with no command" 2 ci init --
eq "nothing reached the daemon" "$(wc -l <"$REQ_LOG")" "$before"

echo "== init =="
rc "init" 0 ci init --check unit --needs rust@stable,cmake@3 --os linux --mem 8G --timeout 1h --branches main,integ/* --statuses-token K2_CI_STATUSES_TOKEN -- sh -c 'echo a,b'
b="$(last /cli/ci/init)"
eq "init argv keeps commas" "$(field "$b" argv)" '["sh", "-c", "echo a,b"]'
eq "init check" "$(field "$b" check)" '"unit"'
eq "init needs" "$(field "$b" needs)" '["rust@stable", "cmake@3"]'
eq "init memGb from 8G" "$(field "$b" memGb)" "8"
eq "init timeout" "$(field "$b" timeoutSecs)" "3600"
eq "init branches" "$(field "$b" branches)" '["main", "integ/*"]'
eq "init token" "$(field "$b" statusesToken)" '"K2_CI_STATUSES_TOKEN"'
eq "a session sends no workspace (the passport's own)" "$(field "$b" workspace)" "null"
has "init: wrote" "$(cat "$WORK/out")" "wrote $WORK/proj/.k2/ci.toml"
has "init: next steps" "$(cat "$WORK/out")" "git add .k2/ci.toml && git commit"
rc "init with no command" 0 ci init --force
has "init: placeholder step" "$(cat "$WORK/out")" "the placeholder check fails until you do"
eq "init --force sent" "$(field "$(last /cli/ci/init)" force)" "true"

echo "== run =="
rc "run" 0 ci run main --check unit,lint --tier standard
b="$(last /cli/ci/run)"
eq "run rev" "$(field "$b" rev)" '"main"'
eq "run checks" "$(field "$b" checks)" '["unit", "lint"]'
eq "run tier" "$(field "$b" tier)" '"standard"'
out="$(cat "$WORK/out")"
has "run render: header" "$out" "run ab12cd34 · running · 1a2b3c4d5e6f (main) · manual · CI tier"
has "run render: statuses" "$out" "statuses: github.com/acme/app as k2-ci/<check> (token K2_CI_STATUSES_TOKEN"
has "run render: a check row" "$out" "lint             error    -"
has "run render: status error" "$out" "status not posted: no secret K2_CI_STATUSES_TOKEN"
has "run render: follow" "$out" "follow: k2 ci status ab12cd34"
rc "run --workspace" 0 ci run --workspace other
eq "--workspace is sent" "$(field "$(last /cli/ci/run)" workspace)" '"other"'
rc "run --json" 0 ci run --json
python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); assert v["run"]["short"] == "ab12cd34"' "$WORK/out" && ok "run --json is the daemon JSON" || bad "run --json"
eq "run with no rev sends none" "$(field "$(last /cli/ci/run)" rev)" "null"
K2_CI_WAIT_SECS=0 rc "run --wait on a failed run" 1 ci run --wait
has "run --wait renders the verdict" "$(cat "$WORK/out")" "run ab12cd34 · failed"
has "run --wait polled status" "$(last /cli/ci/status)" "run=ab12cd34"

echo "== refusals: code: hint, exit by status =="
rc "404" 4 ci run nocfg
has "404 says code: hint" "$(cat "$WORK/err")" "ci_no_config: the hint for ci_no_config"
rc "403" 3 ci run forbid
has "403 says code" "$(cat "$WORK/err")" "compute_off:"
rc "400" 2 ci run bad
rc "409" 3 ci run busy
# 0.46.10 ships CI on compute dark: the daemon's preview-off refusal.
rc "ci_preview_off" 3 ci run dark
has "ci_preview_off says code: hint" "$(cat "$WORK/err")" "ci_preview_off: the hint for ci_preview_off"
has "help names the preview switch" "$("$K2" ci --help)" "(preview: k2 settings set ciPreview true)"
has "study names the preview switch" "$("$K2" study ci)" "k2 settings set ciPreview true"

echo "== status =="
rc "status" 0 ci status
out="$(cat "$WORK/out")"
has "status: watch line" "$out" "watch: on · origin main, integ/* · every 60s · CI tier · max 2 in flight"
has "status: run row" "$out" "ab12cd34  failed   1a2b3c4d5e6f main"
has "status: checks" "$out" "unit=success lint=error"
rc "status one run" 0 ci status 1a2b3c4d
has "status run= sent" "$(last /cli/ci/status)" "run=1a2b3c4d"
has "status: logs hint" "$(cat "$WORK/out")" "logs: k2 ci logs ab12cd34"

echo "== watch =="
rc "watch on" 0 ci watch on --branches main,integ/* --poll 2m --skip-current
b="$(last /cli/ci/watch)"
eq "watch on" "$(field "$b" on)" "true"
eq "watch branches" "$(field "$b" branches)" '["main", "integ/*"]'
eq "watch poll from 2m" "$(field "$b" pollSecs)" "120"
eq "watch skip" "$(field "$b" skipCurrent)" "true"
has "watch on: skipped tips" "$(cat "$WORK/out")" "current tips skipped (main @ 1a2b3c4d)"
rc "watch off" 0 ci watch off
eq "watch off body" "$(field "$(last /cli/ci/watch)" on)" "false"
has "watch off says so" "$(cat "$WORK/out")" "watch off"
rc "watch (show)" 0 ci watch
has "watch shows the remote" "$(cat "$WORK/out")" "\"remote\": \"origin\""

echo "== freeze =="
now="$(date +%s)"
rc "freeze --until 3d" 0 ci freeze --until 3d --why "0.9 release"
b="$(last /cli/ci/freeze)"
u="$(field "$b" until)"
if [ "$u" -ge $((now + 259200 - 5)) ] && [ "$u" -le $((now + 259200 + 60)) ]; then ok "until is 3 days ahead"; else bad "until $u (now $now)"; fi
eq "freeze why" "$(field "$b" why)" '"0.9 release"'
has "freeze render" "$(cat "$WORK/out")" "(0.9 release)"
rc "freeze --until a time" 0 ci freeze --until 2099-01-01T00:00:00Z
eq "an RFC 3339 --until" "$(field "$(last /cli/ci/freeze)" until)" "4070908800"
rc "freeze off" 0 ci freeze off
eq "freeze off body" "$(field "$(last /cli/ci/freeze)" off)" "true"
has "freeze off render" "$(cat "$WORK/out")" "no freeze"

echo "== logs =="
rc "logs" 0 ci logs ab12cd34
out="$(cat "$WORK/out")"
has "logs: unit header" "$out" "── unit · job jobaaaaa · success"
has "logs: unit log through k2 compute logs" "$out" "hello from unit"
has "logs: lint never submitted" "$out" "(never submitted: no log)"
has "logs fetched the job" "$(last /cli/compute/jobs/logs)" "job=jobaaaaa1111"
rc "logs --check lint only" 0 ci logs ab12cd34 --check lint
if grep -Fq "hello from unit" "$WORK/out"; then bad "--check lint printed unit's log"; else ok "--check picks one check"; fi
rc "logs --check nope" 4 ci logs ab12cd34 --check nope

echo
echo "ci_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
