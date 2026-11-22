#!/usr/bin/env bash
# k2 compute allow-nodes and the node-list card (0.46.15): CLI shapes
# against a tiny stub daemon on loopback (python3 http.server). No real
# daemon, no ~/.k2: HOME is a throwaway dir.
#
# Covers: `--add N` and the comma form (and repeated --add) send exactly
# those names in the body's `add`; an agent's add refused
# `allow_nodes_needs_person` exits 3 and ends with the daemon's "Ask with:
# k2 permission request compute.allow-nodes nodes=…" line; a node not
# granted (`allow_nodes_not_granted`) and another workspace's list
# (`forbidden`) exit 3 and end with "It can't be requested."; the help and
# `k2 study compute` name the card and the new code. (The permissions
# skill's section is checked in k2-core: it equals `k2 study permissions`.)
# A hint that names the command and the product comes with `hintTemplate`
# beside it (white-label C5): the CLI prints the template with its own
# names (k2, K2), never the server's (zz, Globex Desk); a daemon that sends
# no template (old-ws) has its hint printed as sent.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2an-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

ASK_ONE='k2 permission request compute.allow-nodes nodes=k2n13-vin --why "<what you need it for>"'
ASK_TWO='k2 permission request compute.allow-nodes nodes=k2n13-vin,k2n14-vin --why "<what you need it for>"'

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG = sys.argv[1]
NOT = {"requestable": False, "where": "a person does this, in Settings or from a terminal outside K2"}

def ask(nodes):
    return {"action": "compute.allow-nodes", "params": {"nodes": nodes}, "approvers": "owner or admin",
            "command": 'k2 permission request compute.allow-nodes nodes=%s --why "<what you need it for>"' % nodes}

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
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body}) + "\n")
        if u.path != "/cli/compute/allow-nodes/set":
            return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
        if body.get("workspace") in ("other-ws", "old-ws"):
            ws = body["workspace"]
            t = "an agent changes only its own workspace's compute node list, never %s's: a person runs {cli} compute allow-nodes --add <node> --workspace %s from a terminal outside {product}" % (ws, ws)
            e = {"code": "forbidden", "workspace": ws, "hint": t.replace("{cli}", "zz").replace("{product}", "Globex Desk")}
            if ws == "other-ws":
                e["hintTemplate"] = t
            return self.reply(403, {"error": e, "permission": NOT})
        add = body.get("add") or ""
        if "k2n9-ungranted" in add:
            return self.reply(403, {"error": {"code": "allow_nodes_not_granted", "workspace": "builder", "notGranted": "k2n9-ungranted",
                                              "hint": "k2n9-ungranted is not granted to workspace builder"},
                                    "permission": NOT})
        return self.reply(403, {"error": {"code": "allow_nodes_needs_person", "workspace": "builder", "nodes": add,
                                          "hint": "adding %s to workspace builder's compute node list lets its jobs reach more machines" % add},
                                "permission": ask(add)})

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
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

last_add() { tail -1 "$LOG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["body"].get("add"))'; }

run() {
    set +e
    err="$("$K2" compute allow-nodes "$@" --workspace builder 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
}

echo "== an agent's add: exit 3, ends with the ask =="
run --add k2n13-vin
expect_eq "add exits 3" "$rc" "3"
expect_has "refusal code" "$err" "allow_nodes_needs_person"
expect_eq "it ends with the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK_ONE"
expect_eq "the body names exactly that node" "$(last_add)" "k2n13-vin"

run --add k2n13-vin,k2n14-vin
expect_eq "comma form exits 3" "$rc" "3"
expect_eq "comma form ends with the ask for both" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK_TWO"
expect_eq "comma form body" "$(last_add)" "k2n13-vin,k2n14-vin"

run --add k2n13-vin --add k2n14-vin
expect_eq "repeated --add body" "$(last_add)" "k2n13-vin,k2n14-vin"
expect_eq "repeated --add ends with the ask for both" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK_TWO"

echo "== not requestable: an ungranted node, another workspace =="
run --add k2n9-ungranted
expect_eq "ungranted exits 3" "$rc" "3"
expect_has "ungranted code" "$err" "allow_nodes_not_granted"
expect_eq "ungranted can't be requested" "$(printf '%s\n' "$err" | tail -1)" "It can't be requested."
set +e
err="$("$K2" compute allow-nodes --add k2n13-vin --workspace other-ws 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "another workspace exits 3" "$rc" "3"
expect_eq "another workspace can't be requested" "$(printf '%s\n' "$err" | tail -1)" "It can't be requested."
expect_has "another workspace: the hint with this CLI's names" "$err" "a person runs k2 compute allow-nodes --add <node> --workspace other-ws from a terminal outside K2"
case "$err" in *"zz compute"*|*"Globex Desk"*) bad "another workspace: the server's names leaked: $err" ;; *) ok "another workspace: no server names" ;; esac
set +e
err="$("$K2" compute allow-nodes --add k2n13-vin --workspace old-ws 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "no template (an older daemon) exits 3" "$rc" "3"
expect_has "no template: the hint as sent" "$err" "a person runs zz compute allow-nodes --add <node> --workspace old-ws from a terminal outside Globex Desk"

echo "== help and study name the card =="
help="$("$K2" compute --help)"
expect_has "help: the card" "$help" "k2 permission request compute.allow-nodes nodes=N[,N…]"
expect_has "help: not granted" "$help" "allow_nodes_not_granted"
study="$("$K2" study compute)"
expect_has "study: the card" "$study" 'k2 permission request compute.allow-nodes nodes=N[,N…] --why "…"'
expect_has "study: exactly those" "$study" "Allow adds exactly those nodes"
expect_has "study: the code" "$study" "allow_nodes_not_granted"

echo
echo "compute allow-nodes CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
