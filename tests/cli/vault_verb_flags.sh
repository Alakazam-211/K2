#!/usr/bin/env bash
# The general k2 vault verbs (cmd_vault in cli/src/migrate.sh) share one
# flag parser. Each verb takes only the flags it reads (K2, 2026-11-16,
# like the send family): any other is refused on stderr, named, exit 2, and
# no request goes out. Never parsed and then dropped.
#
# A tiny stub daemon (python3 http.server) on loopback and a stub vault.sock
# record every request and answer it. No real daemon, no ~/.k2, no keychain:
# HOME is a throwaway dir.
#
# Covers, per verb: the flags it reads still reach the request (body or
# query), and a flag it doesn't read is refused with nothing sent.
# k2 publish secret add|remove and k2 publish env go through these verbs
# too: their flags still reach the request. So do vault placement and
# placements (job.sh): placements --scope picks which secret NAME is, and
# without NAME it is refused (it would be dropped).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

# Short path: the stub vault.sock must fit a Unix socket path.
SANDBOX="$(mktemp -d /tmp/k2vv-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, os, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG, PORTFILE, SOCK = sys.argv[1], sys.argv[2], sys.argv[3]

# One answer for every route: enough for each verb to finish or to have
# sent what this test reads.
ANSWER = {"ok": True, "open": True, "secret": {"id": "sec_1", "name": "K", "latestVersion": 2},
          "grant": {"id": "gr_1", "expiresAt": "2026-11-17T00:00:00Z"}, "methods": ["vault-password"],
          "token": "tok_1", "path": "/tmp/k2-use/K", "envName": "K", "placeholder": "k2v_ph_x",
          "baseUrlEnv": "K_BASE_URL", "baseUrl": "http://127.0.0.1:1/p/x", "recipient": "age1stub",
          "vaultId": "v_1", "count": 0, "targetFingerprint": "fp", "secrets": [], "carryB64": "",
          "dryRun": True, "sourceVaultId": "v_0", "sourceKeyFingerprint": "fp0", "findings": [],
          "audit": [], "requests": [], "presets": [], "placements": [], "moved": [], "envK2": ".env.k2",
          "publicKey": "ssh-ed25519 AAAA K", "agentPolicy": {"burst": 1, "perMinute": 1}, "version": 2}

class H(BaseHTTPRequestHandler):
    channel = "tcp"
    def log_message(self, *a):
        pass
    def answer(self, body):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        q.pop("token", None)
        with open(LOG, "a") as f:
            f.write(json.dumps({"via": self.channel, "method": self.command, "path": u.path, "query": q, "body": body}, sort_keys=True) + "\n")
        b = json.dumps(dict(ANSWER, stub=u.path)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        self.answer(None)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.answer(json.loads(self.rfile.read(n) or b"{}"))

class UH(H):
    channel = "unix"
    def address_string(self):
        return "unix"

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

if os.path.exists(SOCK):
    os.unlink(SOCK)
us = U(SOCK, UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(PORTFILE, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
export HOME="$SANDBOX/h"
mkdir -p "$HOME/.k2"
SOCK="$HOME/.k2/vault.sock"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$SOCK" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && [ -S "$SOCK" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
[ -S "$SOCK" ] || { echo "FAIL: the stub vault.sock did not start" >&2; exit 1; }
printf '%s' "$(cat "$SANDBOX/port")" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"
mkdir -p "$SANDBOX/bk" && : > "$SANDBOX/bk/vault.db"
printf 'X=1\n' > "$SANDBOX/carry.age"
printf 'STRIPE_KEY=sk_live_x\n' > "$SANDBOX/.env"

# reaches <path> <python check on v (the request)> <k2 args…>: run k2 with a
# value on stdin; the last request to <path> must pass the check.
reaches() {
    local path="$1" check="$2"; shift 2
    local before r
    before="$(wc -l < "$LOG")"
    printf 'v4lue' | "$K2" "$@" >/dev/null 2>"$SANDBOX/err" || true
    r="$(tail -n +"$((before + 1))" "$LOG" | grep "\"path\": \"$path\"" | tail -1 || true)"
    if [ -z "$r" ]; then
        bad "k2 $*: no request to $path ($(head -c 300 "$SANDBOX/err"))"
        return 0
    fi
    if python3 -c 'import json,sys; v=json.loads(sys.argv[1]); b=v["body"] or {}; q=v["query"]; sys.exit(0 if eval(sys.argv[2]) else 1)' "$r" "$check"; then
        ok "k2 $*"
    else
        bad "k2 $*: $check is false for $r"
    fi
}
# refused <flag> <k2 args…>: want exit 2, the flag named on stderr, nothing sent.
refused() {
    local flag="$1"; shift
    local before rc
    before="$(wc -l < "$LOG")"
    set +e
    printf 'v4lue' | "$K2" "$@" >/dev/null 2>"$SANDBOX/err"
    rc=$?
    set -e
    if [ "$rc" = 2 ] && grep -Fq -- "$flag" "$SANDBOX/err" && [ "$(wc -l < "$LOG")" = "$before" ]; then
        ok "k2 $* refused ($flag)"
    else
        bad "k2 $*: exit $rc, $(( $(wc -l < "$LOG") - before )) requests, stderr $(printf %q "$(head -c 300 "$SANDBOX/err")"); want exit 2 naming $flag and nothing sent"
    fi
}

echo "== the flags each verb reads still reach the request =="
reaches /cli/vault/status 'True' vault status --json
reaches /cli/vault/list 'q == {"scope": "workspace", "workspace": "acme", "system": "1"}' vault list --scope ws:acme --system --json
reaches /cli/vault/list 'q.get("scope") == "server"' vault ls --scope server --json
reaches /cli/vault/show 'q["name"] == "K" and q["scope"] == "server"' vault show K --scope server --json
reaches /cli/vault/set 'b["name"] == "K" and b["scope"] == "server" and b["kind"] == "generic" and b["hosts"] == ["a.example.com"] and b["delivery"] == "file" and b["ceiling"] == "env" and b["description"] == "d" and b["rotateEveryDays"] == 30 and b["ttl"] == "2h" and b["inject"] == {"dst": "header:X-Key", "fmt": "Key {}"} and b["valueB64"] == "djRsdWU="' \
    vault set K --scope server --kind generic --for a.example.com --delivery file --ceiling env --describe d --rotate-every 30 --ttl 2h --inject-header X-Key --inject-format 'Key {}' --json
reaches /cli/vault/set 'b["preset"] == "stripe" and b["inject"]["dst"] == "query:key" and "system" not in b' vault set K --preset stripe --inject-dst query:key --json
reaches /cli/vault/set 'b["inject"]["sign"] == {"scheme": "hmac-sha256-ts", "header": "X-Sig", "encoding": "base64", "tsHeader": "X-Ts", "template": "{ts}.{body}", "valueFmt": "{sig}"}' \
    vault set K --sign hmac-sha256-ts --sign-header X-Sig --sign-encoding base64 --sign-ts-header X-Ts --sign-template '{ts}.{body}' --sign-value '{sig}' --json
reaches /cli/vault/set 'b["inject"]["sign"] == {"scheme": "oauth-refresh", "tokenUrl": "https://auth.example.com/t", "clientAuth": "basic"}' \
    vault set K --sign oauth-refresh --oauth-token-url https://auth.example.com/t --oauth-client-auth basic --json
reaches /cli/vault/rotate 'b["name"] == "K" and b["scope"] == "server" and b["valueB64"] == "djRsdWU="' vault rotate K --scope server --json
reaches /cli/vault/policy 'b["addHosts"] == ["a.example.com"] and b["removeHosts"] == ["b.example.com"] and b["scope"] == "server"' vault hosts K --add a.example.com --remove b.example.com --scope server --json
reaches /cli/vault/policy 'b["sign"] == {"scheme": "hmac-sha256", "header": "X-Sig", "encoding": "hex", "prefix": "sha256="} and b["scope"] == "server"' \
    vault sign K hmac-sha256 --sign-header X-Sig --sign-encoding hex --sign-prefix 'sha256=' --scope server --json
reaches /cli/vault/policy 'b["sign"] == {"scheme": "oauth-refresh", "tokenUrl": "https://auth.example.com/t", "clientAuth": "post"}' \
    vault sign K oauth-refresh --oauth-token-url https://auth.example.com/t --oauth-client-auth post --json
reaches /cli/vault/public-key 'q["name"] == "K" and q["scope"] == "server"' vault public-key K --scope server --json
reaches /cli/vault/agent-policy 'b["burst"] == 5 and b["perMinute"] == 9 and b["allowForwarded"] is True and b["scope"] == "server"' vault agent-limit K --burst 5 --per-minute 9 --allow-forwarded --scope server --json
reaches /cli/vault/agent-policy 'b["allowForwarded"] is False' vault agent-limit K --no-forwarded --json
reaches /cli/vault/policy 'b["delivery"] == "file" and b["ceiling"] == "env" and b["scope"] == "server"' vault delivery K file --ceiling env --scope server --json
reaches /cli/vault/grant 'b["to"] == "ws:acme" and b["as"] == "env" and b["envName"] == "ACME_KEY" and b["fileName"] == "f" and b["ttl"] == "2h" and b["scope"] == "server"' \
    vault grant K --to ws:acme --as env --env-name ACME_KEY --file-name f --ttl 2h --scope server --json
reaches /cli/vault/revoke 'b["from"] == "ws:acme" and b["scope"] == "server"' vault revoke K --from ws:acme --scope server --json
reaches /cli/vault/revoke 'b["grantId"] == "gr_1"' vault revoke --grant gr_1 --json
reaches /cli/vault/open 'True' vault open --json
reaches /cli/vault/disable 'b["scope"] == "server"' vault disable K --scope server --json
reaches /cli/vault/enable 'b["scope"] == "server"' vault enable K --scope server --json
reaches /cli/vault/destroy 'b["yes"] is True and b["scope"] == "server"' vault destroy K --yes --scope server --json
reaches /cli/vault/get 'b["name"] == "K" and b["scope"] == "server"' vault get K --scope server --json
reaches /cli/vault/audit 'q["name"] == "K" and q["since"] == "2026-11-01T00:00:00Z" and q["scope"] == "server"' vault audit K --since 2026-11-01T00:00:00Z --scope server --json
reaches /cli/vault/requests 'True' vault requests --json
reaches /cli/vault/presets 'True' vault presets --json
reaches /cli/vault/doctor 'q.get("transcripts") == "1"' vault doctor --transcripts --json
reaches /cli/vault/env-file 'b["secrets"] == ["K", "L"] and b["ttl"] == "90s" and b["wait"] == "10m" and b["scope"] == "server"' vault env-file K --secret L --ttl 90s --wait 10m --scope server --json
reaches /cli/vault/env-file 'b["done"] == "/tmp/k2-env/x"' vault env-file --done /tmp/k2-env/x --json
reaches /cli/vault/file 'b["secret"] == "K" and b["ttl"] == "90s" and b["wait"] == "10m" and b["scope"] == "server"' vault file --secret K --ttl 90s --wait 10m --scope server --json
reaches /cli/vault/file 'b["done"] == "/tmp/k2-use/x"' vault file --done /tmp/k2-use/x --json
reaches /cli/vault/proxy-url 'b["secret"] == "K" and b["ttl"] == "2h" and b["scope"] == "session"' vault proxy-url K --ttl 2h --scope session --json
reaches /cli/vault/proxy-url 'b["secret"] == "K"' vault proxy-url --secret K
reaches /cli/vault/import 'b["path"].endswith("/.env") and b["delivery"] == "file" and b["removeOriginal"] is True and b["scope"] == "server"' vault import .env --scope server --delivery file --remove-original --json
reaches /cli/vault/service 'b == {"service": "api", "mode": "clean", "envFile": ".env.k2"}' vault service api clean --env-file .env.k2 --json
reaches /cli/vault/show 'v["via"] == "unix" and q["name"] == "K" and q["scope"] == "server"' vault reveal K --scope server
reaches /cli/vault/rollback 'v["via"] == "unix" and b["version"] == 1 and b["acceptAgentValue"] is True and b["scope"] == "server"' vault rollback K --to 1 --scope server --accept-agent-value
reaches /cli/vault/recipient 'True' vault recipient --json
reaches /cli/vault/carry/export 'b == {"targetRecipient": "age1x", "targetVaultId": "v_2", "ownBoxMove": True, "workspaces": ["acme"], "server": ["S"], "targetWorkspaceId": "ws_2"}' \
    vault carry export --recipient age1x --target v_2 --workspace acme --target-workspace ws_2 --server S --own-box-move --out "$SANDBOX/out-$RANDOM.age"
reaches /cli/vault/carry/import 'b["apply"] is True and b["workspace"] == "ws_2"' vault carry import carry.age --workspace ws_2 --apply --json
reaches /cli/vault/restore 'v["via"] == "unix" and b["holder"] == "backup" and b["dryRun"] is True and b["replace"] is True and b["identity"] == "v4lue"' vault restore --from bk --holder backup --dry-run --replace

echo "== k2 publish secret and k2 publish env go through grant, revoke and service =="
reaches /cli/vault/grant 'b["to"] == "service:api" and b["as"] == "file" and b["envName"] == "API_KEY" and b["scope"] == "server"' publish secret add api K --as file --env-name API_KEY --scope server
reaches /cli/vault/revoke 'b["from"] == "service:api" and b["scope"] == "server"' publish secret remove api K --scope server
reaches /cli/vault/service 'b == {"service": "api", "mode": "clean", "envFile": ".env.k2"}' publish env api clean --env-file .env.k2

echo "== a flag a verb doesn't read is refused, nothing sent =="
refused --scope vault status --scope server
refused --to vault open --to x
refused --yes vault list --yes
refused --to vault show K --to x
refused --yes vault set K --yes
refused --kind vault rotate K --kind generic
refused --to vault hosts K --to x
refused --to vault sign K none --to x
refused --yes vault public-key K --yes
refused --ttl vault agent-limit K --ttl 2h
refused --as vault delivery K file --as env
refused --yes vault grant K --to ws:acme --yes
refused --to vault revoke K --from ws:acme --to x
refused --ttl vault disable K --ttl 2h
refused --yes vault enable K --yes
refused --to vault destroy K --yes --to x
refused --yes vault get K --yes
refused --to vault audit K --to x
refused --scope vault requests --scope server
refused --scope vault presets --scope server
refused --scope vault doctor --scope server
refused --yes vault env-file K --yes
refused --system vault file K --system
refused --wait vault proxy-url K --wait 10m
refused --kind vault import .env --kind generic
refused --scope vault service api --scope server
refused --json vault reveal K --json
refused --json vault recovery status --json
refused --yes vault confirm-password set --yes
refused --json vault kek rotate --json
refused --scope vault rekey --scope server
refused --json vault rollback K --to 1 --json
refused --scope vault recipient --scope server
refused --apply vault carry export --recipient age1x --target v_2 --workspace acme --out x.age --apply
refused --out vault carry import carry.age --out x
refused --json vault backup bk --json
refused --json vault restore --from bk --json
refused --as publish secret remove api K --as file

echo "== placement and placements (job.sh) =="
reaches /cli/vault/placements 'q == {"name": "K", "scope": "workspace", "workspace": "acme"}' vault placements K --scope ws:acme --json
reaches /cli/vault/placements 'q == {"name": "K", "scope": "server"}' vault placement list K --scope server
reaches /cli/vault/placements 'q == {}' vault placements --json
reaches /cli/vault/placement/add 'b == {"name": "K", "where": "box:/etc/k", "note": "n", "scope": "server"}' vault placement add K --where box:/etc/k --note n --scope server --json
reaches /cli/vault/placement/done 'b == {"id": "pl_1"}' vault placement done pl_1 --json
reaches /cli/vault/placement/remove 'b == {"id": "pl_1"}' vault placement rm pl_1 --json
refused --scope vault placements --scope server
refused --where vault placements K --where box:/x
refused --note vault placement done pl_1 --note n
refused --scope vault placement rm pl_1 --scope server
refused --where vault placement list --where box:/x
"$K2" --schema | python3 -c '
import json, sys
c = {x["name"]: x for x in json.load(sys.stdin)["commands"]}
sys.exit(0 if "--scope" in {f["name"] for f in c["vault placements"]["flags"]} else 1)' && ok "schema: vault placements --scope" || bad "schema: vault placements lacks --scope"

echo "== only k2 vault list takes --system (K2, 2026-11-16) =="
# K2's own secrets are the daemon's: no verb but list names them, GET or
# POST alike. Exit 2, the one line, nothing sent.
refused_system() {
    local before rc
    before="$(wc -l < "$LOG")"
    set +e
    printf 'v4lue' | "$K2" "$@" >/dev/null 2>"$SANDBOX/err"
    rc=$?
    set -e
    if [ "$rc" = 2 ] && grep -Fxq "usage: only k2 vault list takes --system" "$SANDBOX/err" && [ "$(wc -l < "$LOG")" = "$before" ]; then
        ok "k2 $* refused: only list takes --system"
    else
        bad "k2 $*: exit $rc, $(( $(wc -l < "$LOG") - before )) requests, stderr $(printf %q "$(head -c 300 "$SANDBOX/err")"); want exit 2, 'usage: only k2 vault list takes --system', nothing sent"
    fi
}
for args in "show K" "set K" "rotate K" "hosts K --add a.example.com" "sign K none" "public-key K" "agent-limit K" \
    "delivery K file" "grant K --to ws:acme" "revoke K --from ws:acme" "disable K" "enable K" "destroy K --yes" "get K" \
    "audit K" "reveal K" "rollback K --to 1" "status" "file K" "env-file K" "proxy-url K" "import .env" "doctor" \
    "send K --to bob" "incoming" "ask bob K" "unsend hd_1" "fetch --secret K https://a.example.com/" "sync" \
    "placements K" "placement add K --where box:/k" "systemd-cred K --unit k2-watch.service --dry-run"; do
    # shellcheck disable=SC2086 # the words are separate arguments on purpose
    refused_system vault $args --system
done
refused_system vault run --system --secret K -- true
refused_system publish secret add api K --system
reaches /cli/vault/list 'q.get("system") == "1"' vault list --system --json

echo
echo "vault_verb_flags: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
