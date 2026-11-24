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
# 0.46.18: a managing agent's change to another workspace's list
# (`allow_nodes_for_needs_person`) exits 3 and ends with the daemon's "Ask
# with: k2 permission request compute.allow-nodes-for …" line, the body
# naming that workspace and the change; the help and study name the card.
# 0.46.18: `--fixed on|off` sends `fixed` true or false and nothing else
# (any other word is a usage error, exit 2); the list says whether it is
# fixed or follows new nodes, and so does `k2 compute nodes`.

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
    def do_GET(self):
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": None}) + "\n")
        if u.path == "/cli/compute/nodes":
            return self.reply(200, {"nodes": [], "allowNodes": ["k2n13-vin"], "allowNodesWorkspace": "builder", "allowNodesFixed": True})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body}) + "\n")
        if u.path != "/cli/compute/allow-nodes/set":
            return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
        if "fixed" in body:
            return self.reply(200, {"workspace": "builder", "list": True, "change": "fixed", "fixed": body["fixed"],
                                    "nodes": [{"name": "k2n13-vin", "state": "active", "granted": True}]})
        if body.get("workspace") == "managed-ws":
            key = "add" if body.get("add") else ("remove" if body.get("remove") else "clear")
            value = body.get(key) if key != "clear" else "true"
            cmd = 'k2 permission request compute.allow-nodes-for %s=%s target=managed-ws --why "<what you need it for>"' % (key, value)
            return self.reply(403, {"error": {"code": "allow_nodes_for_needs_person", "workspace": "managed-ws", key: value,
                                              "hint": "builder manages managed-ws (it hired managed-ws), but a person decides"},
                                    "permission": {"action": "compute.allow-nodes-for", "params": {"target": "managed-ws", key: value},
                                                   "approvers": "owner or admin", "command": cmd}})
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

echo "== a managing agent: another workspace's list asks on its card =="
for change in "--add k2n13-vin:add=k2n13-vin" "--remove k2n14-vin:remove=k2n14-vin" "--clear:clear=true"; do
    flags="${change%%:*}"
    want="${change#*:}"
    set +e
    # shellcheck disable=SC2086
    err="$("$K2" compute allow-nodes $flags --workspace managed-ws 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
    expect_eq "$flags on another workspace exits 3" "$rc" "3"
    expect_has "$flags: the code" "$err" "allow_nodes_for_needs_person"
    expect_eq "$flags ends with the card for that change" "$(printf '%s\n' "$err" | tail -1)" \
        "Ask with: k2 permission request compute.allow-nodes-for $want target=managed-ws --why \"<what you need it for>\""
done
expect_eq "the body names the workspace and the change" "$(tail -1 "$LOG" | python3 -c 'import json,sys; b=json.load(sys.stdin)["body"]; print(b.get("workspace"), b.get("clear"))')" "managed-ws True"

echo "== --fixed on|off: a person's flag, shown on the list =="
last_body() { tail -1 "$LOG" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["body"], sort_keys=True))'; }
set +e
out="$("$K2" compute allow-nodes --fixed on --workspace builder 2>&1 </dev/null)"; rc=$?
set -e
expect_eq "--fixed on exits 0" "$rc" "0"
expect_eq "--fixed on sends fixed true only" "$(last_body)" '{"fixed": true, "workspace": "builder"}'
expect_has "a fixed list says so" "$out" "fixed: a node newly added to this server does not join it"
set +e
out="$("$K2" compute allow-nodes --fixed off --workspace builder 2>&1 </dev/null)"; rc=$?
set -e
expect_eq "--fixed off exits 0" "$rc" "0"
expect_eq "--fixed off sends fixed false" "$(last_body)" '{"fixed": false, "workspace": "builder"}'
expect_has "a following list says so" "$out" "follows new nodes: a node newly added to this server joins it"
before="$(wc -l < "$LOG")"
set +e
err="$("$K2" compute allow-nodes --fixed maybe --workspace builder 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "--fixed maybe is a usage error" "$rc" "2"
expect_has "--fixed names its words" "$err" "(--fixed takes on or off)"
expect_eq "a usage error sends nothing" "$(wc -l < "$LOG")" "$before"
set +e
out="$("$K2" compute nodes --workspace builder 2>&1 </dev/null)"; rc=$?
set -e
expect_eq "nodes exits 0" "$rc" "0"
expect_has "nodes: the list" "$out" "node list (builder): jobs go only to k2n13-vin"
expect_has "nodes: fixed" "$out" "  fixed: new nodes do not join it"

echo "== help and study name the card =="
help="$("$K2" compute --help)"
expect_has "help: the card" "$help" "k2 permission request compute.allow-nodes nodes=N[,N…]"
expect_has "help: not granted" "$help" "allow_nodes_not_granted"
study="$("$K2" study compute)"
expect_has "study: the card" "$study" 'k2 permission request compute.allow-nodes nodes=N[,N…] --why "…"'
expect_has "study: exactly those" "$study" "Allow adds exactly those nodes"
expect_has "study: the code" "$study" "allow_nodes_not_granted"
expect_has "help: --fixed" "$help" "k2 compute allow-nodes --fixed on|off [--workspace WS]"
expect_has "help: the card for another workspace" "$help" "compute.allow-nodes-for target=WS add=N[,N…]"
expect_has "study: the card for another workspace" "$study" "k2 permission request compute.allow-nodes-for target=WS add=N[,N…]"
expect_has "study: its code" "$study" "allow_nodes_for_needs_person"
expect_has "study: new nodes join" "$study" "a node newly added to this server joins"

echo
echo "compute allow-nodes CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
