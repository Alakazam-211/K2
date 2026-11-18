#!/usr/bin/env bash
# `k2 done` escapes the agent name, so the daemon gets one agent value.
#
# Regression: cmd_done built "agent=$agent" raw. `k2 done --agent 'a#b'`
# cut the URL at '#' (curl drops the fragment), so the daemon marked agent
# "a" done and lost --blocked; `--agent 'x&blocked=y'` added a blocked
# parameter of its own. Now the name is percent-encoded like every other
# value. The stub logs each request's raw query (token and project left
# out) and its body, and the test checks the exact bytes and the parsed
# form.
#
# Hermetic: temp HOME, a stub daemon on loopback, no real ~/.k2. Loud.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORKDIR="$(mktemp -d -t k2-done-escape-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORKDIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlsplit

root = sys.argv[1]
log = os.path.join(root, "requests.jsonl")

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def answer(self):
        u = urlsplit(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("latin-1") if n else ""
        # The raw query in its order, minus the token and project pairs.
        raw = "&".join(p for p in u.query.split("&")
                       if p.split("=", 1)[0] not in ("token", "project"))
        form = parse_qs(u.query, keep_blank_values=True)
        for k in ("token", "project"):
            form.pop(k, None)
        with open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path, "raw": raw,
                                "body": body, "form": form}, sort_keys=True) + "\n")
        out = b'{"success": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    do_GET = answer
    do_POST = answer

httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do
    [ -s "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -s "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
export K2_PORT="$(cat "$WORKDIR/stub.port")"
export K2_AGENT_NAME=tester

# run <env...> -- <k2 args...>: runs the CLI, sets RC, ERR and REQS (each
# request as "<METHOD> <path>?<raw query> body=<body> form=<JSON>").
run() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : > "$WORKDIR/requests.jsonl"
    RC=0
    env ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err" || RC=$?
    ERR="$(cat "$WORKDIR/err")"
    REQS="$(python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.strip():
        r = json.loads(l)
        print("%s %s?%s body=%s form=%s" % (r["method"], r["path"], r["raw"], r["body"],
                                            json.dumps(r["form"], sort_keys=True)))
' "$WORKDIR/requests.jsonl")"
}

# sends <label> <want> <env...> -- <k2 args...>
sends() {
    local label="$1" want="$2"; shift 2
    run "$@"
    if [ "$RC" = 0 ] && [ "$REQS" = "$want" ]; then
        ok "$label"
    else
        bad "$label: rc=$RC"
        echo "    got:  [$REQS]" >&2
        echo "    want: [$want]" >&2
        echo "    stderr: [$ERR]" >&2
    fi
}

echo "== --agent with # and & is one agent value =="
sends "--agent 'a#b' --blocked" \
    'POST /cli/done?agent=a%23b&blocked=waiting%20on%20review body= form={"agent": ["a#b"], "blocked": ["waiting on review"]}' \
    -- done --agent 'a#b' --blocked "waiting on review"
sends "--agent 'a#b'" \
    'POST /cli/done?agent=a%23b body= form={"agent": ["a#b"]}' \
    -- done --agent 'a#b'
sends "--agent 'x&blocked=y'" \
    'POST /cli/done?agent=x%26blocked%3Dy body= form={"agent": ["x&blocked=y"]}' \
    -- done --agent 'x&blocked=y'
sends "--agent 'x&blocked=y' --blocked" \
    'POST /cli/done?agent=x%26blocked%3Dy&blocked=real body= form={"agent": ["x&blocked=y"], "blocked": ["real"]}' \
    -- done --agent 'x&blocked=y' --blocked real
sends "--agent 'a b+c'" \
    'POST /cli/done?agent=a%20b%2Bc body= form={"agent": ["a b+c"]}' \
    -- done --agent 'a b+c'

echo "== a name with &, =, a space and % is one agent value =="
# Raw, '%41' would reach the daemon as 'A' and '&' would start a parameter.
sends "--agent 'a&b=c d%41' --blocked" \
    'POST /cli/done?agent=a%26b%3Dc%20d%2541&blocked=x body= form={"agent": ["a&b=c d%41"], "blocked": ["x"]}' \
    -- done --agent 'a&b=c d%41' --blocked x
sends "--agent '50%'" \
    'POST /cli/done?agent=50%25 body= form={"agent": ["50%"]}' \
    -- done --agent '50%'

echo "== the name from K2_AGENT_NAME is escaped the same way =="
sends "K2_AGENT_NAME='a#b'" \
    'POST /cli/done?agent=a%23b&blocked=x body= form={"agent": ["a#b"], "blocked": ["x"]}' \
    K2_AGENT_NAME='a#b' -- done --blocked x
sends "K2_AGENT_NAME='a&b=c d%41'" \
    'POST /cli/done?agent=a%26b%3Dc%20d%2541 body= form={"agent": ["a&b=c d%41"]}' \
    K2_AGENT_NAME='a&b=c d%41' -- done

echo "== a plain name is unchanged =="
sends "--agent other" \
    'POST /cli/done?agent=other body= form={"agent": ["other"]}' \
    -- done --agent other

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
