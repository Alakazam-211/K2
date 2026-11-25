#!/usr/bin/env bash
# Hermetic test for step 9 / 9b (the first owner and the website-management
# login) of scripts/provision-k2-server.sh, 0.46.18.
#
# Since 0.46.15 users/set-role owner with the owner token answers 403
# vault_presence_required until this computer's own user check (a person
# present) is passed, so the provisioner's `set-role <u> owner` stopped
# every provision. Now the provisioner makes no owner login at all: the
# k2.dev account holder claims the server's first owner from their
# dashboard (POST /cli/users/claim-first-owner). So:
#   - K2_OWNER_USER stops the run before any work, with a line naming the
#     k2.dev claim (and the self-hosted `k2 users add`); --bake ignores it;
#   - step 9/9b never calls users/set-role or the vault socket, and makes
#     K2_OPS_USER a Member (the server stays claimable: no owner login);
#   - the callback says ownerUser "" and firstOwner "claim".
#
# How: the early check and the step 9/9b block are cut out of the script
# and run alone; 9/9b against a stub daemon (python3, TCP) that logs every
# request and answers users/add the daemon's way, and set-role owner with
# the daemon's 403 vault_presence_required. No root, no network, no daemon.
# K2_TEST_PROVISION names another copy of the script (the bite).
# Run with: bash tests/cli/provision_first_owner.sh
# shellcheck disable=SC2016
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROVISION="${K2_TEST_PROVISION:-$ROOT/scripts/provision-k2-server.sh}"
command -v python3 >/dev/null 2>&1 || { echo "FAIL: python3 required" >&2; exit 1; }
[ -f "$PROVISION" ] || { echo "FAIL: missing $PROVISION" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-first-owner-XXXXXX")"
STUB_PID=""
cleanup() {
	if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
	rm -rf "$WORK"
}
trap cleanup EXIT

echo "== static shape =="
bash -n "$PROVISION" || fail "bash -n"
BLOCK="$(sed -n '/^# ── 9\. first owner user/,/^# ── 9c\. Sew/p' "$PROVISION" | sed '$d')"
[ -n "$BLOCK" ] || fail "the step 9 block is not found"
CURL_FNS="$(sed -n '/^_curl_cfg_quote() {$/,/^}$/p; /^k2_curl() {$/,/^}$/p' "$PROVISION")"
[ -n "$CURL_FNS" ] || fail "k2_curl is not found"
EARLY="$(sed -n '/^# Step 9: this run makes no owner/,/^fi$/p' "$PROVISION")"
[ -n "$EARLY" ] || fail "the early K2_OWNER_USER check is not found"
L_EARLY="$(grep -n '^# Step 9: this run makes no owner' "$PROVISION" | cut -d: -f1)"
L_PKGS="$(grep -n '^# ── 1\. packages' "$PROVISION" | cut -d: -f1)"
[ "$L_EARLY" -lt "$L_PKGS" ] || fail "the K2_OWNER_USER check must run before step 1 (no work first)"
ok "the K2_OWNER_USER check runs before any work"
for never in 'users/set-role' 'vault.sock' 'confirm/finish' 'X-K2-Tty' 'K2_FIRST_OWNER'; do
	if grep -q -- "$never" "$PROVISION"; then fail "the provisioner must not use $never (no owner is made by a run)"; fi
done
ok "no set-role, no vault socket, no tty claim, no bridge switch anywhere in the script"
CB="$(sed -n '/^# ── 10\. control-plane callback/,/^# ── 11\./p' "$PROVISION")"
<<<"$CB" grep -q '"ownerUser":"",' || fail "the callback's ownerUser must be empty"
<<<"$CB" grep -q '"firstOwner":"claim",' || fail "the callback must say firstOwner claim"
ok "the callback says ownerUser \"\" and firstOwner claim"

echo "== K2_OWNER_USER stops the run =="
run_early() { # env assignments... → prints the die line or "go"
	env -i PATH="$PATH" "$@" bash -c 'set -euo pipefail; die() { echo "DIE: $*"; exit 1; }; BAKE_ONLY="${BAKE_ONLY:-0}"; '"$EARLY"'; echo go' 2>&1 || true
}
out="$(run_early K2_OWNER_USER=alice K2_OWNER_PASSWORD=x)"
case "$out" in "DIE: K2_OWNER_USER is set, but this script no longer makes an owner login: a server's first owner is claimed from the k2.dev dashboard"*) ;; *) fail "K2_OWNER_USER must stop: $out" ;; esac
<<<"$out" grep -q 'k2 users add <you> --role owner from your own SSH login' || fail "the stop names the self-hosted way: $out"
ok "K2_OWNER_USER stops before any work, naming the k2.dev claim and the self-hosted way"
[ "$(run_early K2_OWNER_USER=alice K2_FIRST_OWNER=terminal)" != go ] || fail "no switch brings back an owner-making run"
[ "$(run_early)" = go ] || fail "no K2_OWNER_USER must go on"
[ "$(run_early K2_OWNER_USER=alice BAKE_ONLY=1)" = go ] || fail "--bake ignores K2_OWNER_USER"
ok "no switch gets past it; no K2_OWNER_USER and --bake go on"

echo "== the stub daemon =="
cat >"$WORK/stub.py" <<'PY'
import http.server, json, os, socketserver, sys, threading
work = sys.argv[1]
lock = threading.Lock()
users = {}

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n).decode() or "{}")
        path, _, query = self.path.partition("?")
        with lock:
            with open(os.path.join(work, "requests.jsonl"), "a") as f:
                f.write(json.dumps({"path": path, "query": query, "body": body}) + "\n")
            code, out = self.route(path, query, body)
            with open(os.path.join(work, "users.json"), "w") as f:
                json.dump(users, f)
        b = json.dumps(out).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def route(self, path, query, body):
        if "token=owner-tok" not in query:
            return 401, {"error": "unauthorized"}
        if path == "/cli/users/add":
            u = body["username"]
            if u in users:
                return 400, {"error": "user '%s' already exists" % u}
            users[u] = {"role": "member", "mustChangePassword": body.get("mustChangePassword")}
            return 200, {"success": True}
        if path == "/cli/users/set-role" and body.get("role") == "owner":
            return 403, {"error": {"code": "vault_presence_required"}}
        return 404, {"error": "not_found"}

class TCP(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True

tcp = TCP(("127.0.0.1", 0), H)
with open(os.path.join(work, "port"), "w") as f:
    f.write(str(tcp.server_address[1]))
tcp.serve_forever()
PY
: >"$WORK/requests.jsonl"
python3 "$WORK/stub.py" "$WORK" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "the stub never started"
PORT="$(cat "$WORK/port")"
ok "stub daemon on $PORT"

# run_block <env assignments...>: step 9/9b alone; prints its log lines.
run_block() {
	env -i PATH="$PATH" BASE="http://127.0.0.1:$PORT" TOKEN=owner-tok K2_RUN_USER=k2 "$@" \
		bash -c 'set -euo pipefail
log() { echo "LOG: $*"; }
die() { echo "DIE: $*"; exit 1; }
runuser() { echo "DIE: runuser called: $*"; exit 1; }
'"$CURL_FNS"'
'"$BLOCK"'
echo "DONE ops_password=${OPS_PASSWORD:+set}"' 2>&1
}
paths() { python3 -c 'import json,sys; print(" ".join(json.loads(l)["path"] for l in open(sys.argv[1])))' "$WORK/requests.jsonl"; }
roles() { python3 -c 'import json,sys; u=json.load(open(sys.argv[1])); print(" ".join("%s=%s" % (k, v["role"]) for k, v in sorted(u.items())))' "$WORK/users.json"; }

echo "== hosted: the ops login is a Member, no owner =="
out="$(run_block K2_OPS_USER=k2cloud-ops)" || fail "the run must succeed: $out"
[ "$(paths)" = "/cli/users/add" ] || fail "only users/add, got: $(paths)"
[ "$(roles)" = "k2cloud-ops=member" ] || fail "want only a Member ops login, got $(roles)"
<<<"$out" grep -q 'LOG: first owner: none made here; the k2.dev account holder claims it from their dashboard' || fail "the owner line: $out"
<<<"$out" grep -q "LOG: creating service user 'k2cloud-ops' (website management; a Member until the owner claims this server)" || fail "the ops line: $out"
<<<"$out" grep -q 'DONE ops_password=set' || fail "the ops password goes to the callback: $out"
ok "one users/add; k2cloud-ops is a Member; zero owners, so the server stays claimable"

echo "== a re-run: the ops login exists =="
: >"$WORK/requests.jsonl"
out="$(run_block K2_OPS_USER=k2cloud-ops)" || fail "the re-run must succeed: $out"
[ "$(paths)" = "/cli/users/add" ] || fail "re-run: only users/add, got: $(paths)"
<<<"$out" grep -q 'LOG: ops user already exists — leaving its password and role unchanged' || fail "re-run line: $out"
<<<"$out" grep -q 'DONE ops_password=$' || fail "no new ops password on a re-run: $out"
ok "the existing ops login is left as it is (password and role)"

echo "== self-hosted: no ops user, no calls =="
: >"$WORK/requests.jsonl"
out="$(run_block)" || fail "the self-hosted run must succeed: $out"
[ -z "$(paths)" ] || fail "no request at all, got: $(paths)"
ok "no K2_OPS_USER: step 9/9b sends nothing"

echo "ALL PASS ($pass)"
