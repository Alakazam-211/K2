#!/usr/bin/env bash
# The `vault.share-service` card, headless and end to end: a real daemon under
# a temp HOME (worktree build, air-gapped), the real `k2` CLI as an agent, and
# curl for what a person's client sends. Agent CLIs are `exec cat` shims.
#
#   1. a person gives the agent a secret on a Thread secret card (file);
#   2. cards off: `k2 publish secret add` is refused, as today, no Ask line;
#   3. cards on: `k2 publish secret add` and `k2 vault grant --to service:` end
#      with the exact Ask-with line;
#   4. the agent's request for another workspace's service or secret is refused;
#   5. the agent can't answer, a Member can't, Allow without a confirm can't;
#   6. an Admin login confirms with her password and allows; the service still doesn't have it until
#      the agent restarts it, then reads the file in $K2_SECRETS_DIR;
#   7. a second answer is 409; the value is in no response, log or audit.
#
# Build first: cargo build -p k2-ptyd -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2CLI="$ROOT/cli/k2"
for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

DAEMON=""
for cand in "${CARGO_TARGET_DIR:-$ROOT/target}/debug/k2-daemon" "${CARGO_TARGET_DIR:-$ROOT/target}/release/k2-daemon"; do
    [ -x "$cand" ] && { DAEMON="$cand"; break; }
done
[ -n "$DAEMON" ] || { echo "FAIL: build k2-daemon first" >&2; exit 1; }

SANDBOX="$(mktemp -d /tmp/k2ss-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
PIDS=()
pass=0
HOME_A="$SANDBOX/a/home"
SHIM="$SANDBOX/a/shim"
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    sleep 0.3
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done
    rm -rf "$SANDBOX"
}
trap cleanup EXIT
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail" >&2; tail -30 "$SANDBOX/a/daemon.log" >&2 || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
has() { grep -Fq -- "$2" <<<"$1" || fail "missing '$2' in: $1"; }
hasnt() { if grep -Fq -- "$2" <<<"$1"; then fail "unexpected '$2' in: $1"; fi; }
run() { set +e; out="$("$@" 2>"$SANDBOX/stderr")"; rc=$?; err="$(cat "$SANDBOX/stderr")"; set -e; }

mkdir -p "$HOME_A/tmp" "$HOME_A/run" "$SHIM"
cat >"$SHIM/claude" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac
env >"$SHIM/claude.env"
exec cat
EOF
chmod +x "$SHIM/claude"
TMPDIR="$HOME_A/tmp" XDG_RUNTIME_DIR="$HOME_A/run" K2_RUNTIME_DIR= HOME="$HOME_A" K2_AIRGAP=1 \
    K2_TEST_AGENT_SHIM_DIR="$SHIM" K2_HEARTBEAT_NO_SELF_HEAL=1 \
    "$DAEMON" >"$SANDBOX/a/daemon.log" 2>&1 &
PIDS+=("$!")
for _ in $(seq 1 150); do
    [ -s "$HOME_A/.k2/heartbeat.port" ] && [ -s "$HOME_A/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$HOME_A/.k2/heartbeat.port" ] || fail "the daemon wrote its port"
PORT="$(cat "$HOME_A/.k2/heartbeat.port")"
OWNER="$(cat "$HOME_A/.k2/heartbeat.token")"
for _ in $(seq 1 150); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"

owner_k2() {
    env -i HOME="$HOME_A" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$PORT" \
        ${K2_PROJECT_PATH:+K2_PROJECT_PATH="$K2_PROJECT_PATH"} bash "$K2CLI" "$@"
}
agent_k2() {
    local pp="$1" dir="$2"; shift 2
    env -i HOME="$HOME_A" PATH="$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 K2_PORT="$PORT" \
        K2_HOOK_TOKEN="$pp" K2_HOOK_SOCK="$SANDBOX/not-a-socket.sock" K2_PROJECT_PATH="$dir" bash "$K2CLI" "$@"
}
post() { curl -s -o "$SANDBOX/body" -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "$2" "http://127.0.0.1:$PORT$1"; }
body() { cat "$SANDBOX/body"; }
hire() {
    local dir="$SANDBOX/a/work/$1"
    mkdir -p "$SANDBOX/a/work"
    owner_k2 agent hire "$dir" --name "$1" --json >"$SANDBOX/hire.json" || fail "hire $1: $(cat "$SANDBOX/hire.json")"
    echo "$dir"
}
handle_of() {
    curl -s "http://127.0.0.1:$PORT/cli/projects/list?token=$OWNER" |
        python3 -c 'import json,sys
d=json.load(sys.stdin); want=sys.argv[1]
rows=d if isinstance(d,list) else (d.get("projects") or d.get("items") or [])
for p in rows:
    if p.get("path")==want: print(p.get("handle") or p.get("name")); break' "$1"
}
passport_of() {
    rm -f "$SHIM/claude.env"
    owner_k2 agent restart "$1" --json >/dev/null || fail "start the pinned chat of $1"
    for _ in $(seq 1 100); do [ -s "$SHIM/claude.env" ] && break; sleep 0.1; done
    sed -n 's/^K2_HOOK_TOKEN=//p' "$SHIM/claude.env" | head -1
}
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
# A service that reports what it finds in $K2_SECRETS_DIR.
service_at() {
    cat >"$1/serve.py" <<'EOF'
import http.server, os
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        d = os.environ.get("K2_SECRETS_DIR", "")
        p = os.path.join(d, self.path.lstrip("/")) if d else ""
        code, body = (200, open(p, "rb").read()) if p and os.path.isfile(p) else (404, b"absent")
        self.send_response(code); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
http.server.HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
EOF
}
wait_http() {
    for _ in $(seq 1 100); do curl -s -o /dev/null "http://127.0.0.1:$1/" && return 0; sleep 0.1; done
    fail "nothing on port $1"
}

VALUE="k2test-share-$RANDOM$RANDOM$RANDOM"

echo "== a person gives the agent a secret on a Thread card; it has a service =="
AGENT="$(hire shareagent)"
OTHER="$(hire shareother)"
HA="$(handle_of "$AGENT")"; [ -n "$HA" ] || fail "no handle for $AGENT"
HO="$(handle_of "$OTHER")"; [ -n "$HO" ] || fail "no handle for $OTHER"
PP="$(passport_of "$AGENT")"; [ -n "$PP" ] || fail "the agent got no passport"
run agent_k2 "$PP" "$AGENT" thread secret "$HA" --name PREVIEW_PIN --delivery file --prompt "The preview PIN"
[ "$rc" = 0 ] || fail "post the secret card exit $rc: $out $err"
CARD="$(head -1 <<<"$out")"
st="$(post "/cli/thread/answer?token=$OWNER" "{\"addr\":\"$HA\",\"id\":\"$CARD\",\"secret\":\"$VALUE\",\"disclosed\":\"1\"}")"
[ "$st" = 200 ] || fail "the person's answer: $st $(body)"
hasnt "$(body)" "$VALUE"

SVC_PORT="$(free_port)"
service_at "$AGENT"
K2_PROJECT_PATH="$AGENT" owner_k2 publish run sharesvc --cmd 'python3 serve.py' --port "$SVC_PORT" --no-tunnel >"$SANDBOX/run.out" 2>&1 || fail "publish run: $(cat "$SANDBOX/run.out")"
wait_http "$SVC_PORT"
service_at "$OTHER"
K2_PROJECT_PATH="$OTHER" owner_k2 publish run theirsvc --cmd 'python3 serve.py' --port "$(free_port)" --no-tunnel >"$SANDBOX/run2.out" 2>&1 || fail "publish run: $(cat "$SANDBOX/run2.out")"
ok "a person-given secret, and a service in each workspace"

echo "== cards off: today's refusal =="
run agent_k2 "$PP" "$AGENT" publish secret add sharesvc PREVIEW_PIN --scope "ws:$HA" --as file
[ "$rc" = 3 ] || fail "exit $rc: $out $err"
has "$err" "vault_owner_confirm_required: a person gave this secret, so an agent can't widen where it goes or who can read it. Settings → Vault can't share with a service, so ask the owner or an Admin to run this in their own terminal"
hasnt "$err" "Ask with:"
hasnt "$err" "permission card"
[ "$(printf '%s\n' "$err" | wc -l | tr -d ' ')" = 1 ] || fail "cards off is one line: $err"
ok "no Ask line, the text as it was"

echo "== cards on: the Ask-with line =="
st="$(post "/cli/settings/update?token=$OWNER" '{"permissionCards":true}')"; [ "$st" = 200 ] || fail "server switch: $st $(body)"
st="$(post "/cli/agent-access/set?token=$OWNER" "{\"toggle\":\"permission_cards\",\"workspace\":\"$HA\",\"value\":true}")"; [ "$st" = 200 ] || fail "agent switch: $st $(body)"
ASK='Ask with: k2 permission request vault.share-service as=file secret=PREVIEW_PIN service=sharesvc --why "<what you need it for>"'
run agent_k2 "$PP" "$AGENT" publish secret add sharesvc PREVIEW_PIN --scope "ws:$HA" --as file
[ "$rc" = 3 ] || fail "publish secret add exit $rc: $out $err"
[ "$(printf '%s\n' "$err" | tail -1)" = "$ASK" ] || fail "publish secret add closing line: $err"
has "$err" "allow it on a permission card"
run agent_k2 "$PP" "$AGENT" vault grant PREVIEW_PIN --to service:sharesvc --as file
[ "$rc" = 3 ] || fail "vault grant exit $rc: $out $err"
[ "$(printf '%s\n' "$err" | tail -1)" = "$ASK" ] || fail "vault grant closing line: $err"
ok "both doors end with the exact line"

echo "== who may ask =="
ask_body() { printf '{"action":"vault.share-service","params":{"secret":"%s","service":"%s","as":"file"},"why":"The service needs it to start"}' "$1" "$2"; }
st="$(post "/cli/permission/request?token=$PP" "$(ask_body PREVIEW_PIN theirsvc)")"
[ "$st" = 400 ] || fail "another workspace's service: $st $(body)"; has "$(body)" "has no service or App named theirsvc"
PPO="$(passport_of "$OTHER")"; [ -n "$PPO" ] || fail "the other agent got no passport"
st="$(post "/cli/agent-access/set?token=$OWNER" "{\"toggle\":\"permission_cards\",\"workspace\":\"$HO\",\"value\":true}")"; [ "$st" = 200 ] || fail "other's switch: $st $(body)"
st="$(post "/cli/permission/request?token=$PPO" "$(ask_body PREVIEW_PIN theirsvc)")"
[ "$st" = 400 ] || fail "another workspace's secret: $st $(body)"; has "$(body)" "has no secret named PREVIEW_PIN"
ok "another workspace's service or secret is refused when asked"

echo "== the card =="
# Exercise the CLI's own dry run, then ask through the same route.
run agent_k2 "$PP" "$AGENT" permission request vault.share-service as=file secret=PREVIEW_PIN service=sharesvc --why "The preview needs it to start" --dry-run
[ "$rc" = 0 ] || fail "dry run exit $rc: $out $err"
has "$out" "Let $HA give its service sharesvc the secret PREVIEW_PIN as a file"
st="$(post "/cli/permission/request?token=$PP" "$(ask_body PREVIEW_PIN sharesvc)")"
[ "$st" = 200 ] || fail "ask: $st $(body)"
REQ="$(jq -r .id "$SANDBOX/body")"; CARD2="$(jq -r .card "$SANDBOX/body")"
[ -n "$REQ" ] && [ "$REQ" != null ] && [ -n "$CARD2" ] && [ "$CARD2" != null ] || fail "no request id or card: $(body)"
hasnt "$(body)" "$VALUE"
answer() { printf '{"addr":"%s","id":"%s","answer":"Allow"%s}' "$HA" "$CARD2" "$1"; }

st="$(post "/cli/thread/answer?token=$PP" "$(answer '')")"
[ "$st" = 403 ] || fail "the agent answered: $st $(body)"; has "$(body)" "action_card_person_only"
ok "the agent can't answer its own card"

st="$(post "/cli/thread/answer?token=$OWNER" "$(answer '')")"
[ "$st" = 403 ] || fail "no confirm: $st $(body)"; has "$(body)" "vault_confirm_required"
st="$(post "/cli/thread/answer?token=$OWNER" "$(answer ',"confirmToken":"k2vc_made_up"')")"
[ "$st" = 403 ] || fail "a made-up confirm: $st $(body)"; has "$(body)" "vault_confirm_required"
[ "$(curl -s "http://127.0.0.1:$SVC_PORT/PREVIEW_PIN" -o /dev/null -w '%{http_code}')" = 404 ] || fail "the service had it before Allow"
ok "Allow takes a real confirm; nothing was shared"

echo "== Allow =="
# People sign in over Connect (setting the vault password takes a person at
# the machine, not a token): a Member, and an Admin who confirms with her
# own password.
printf 'k2test-Member-Horse-71\n' | owner_k2 users add mem --role member --password-stdin >/dev/null || fail "users add mem"
printf 'k2test-Admin-Horse-71\n' | owner_k2 users add ada --role admin --password-stdin >/dev/null || fail "users add ada"
login() {
    st="$(post "/cli/auth/login" "{\"username\":\"$1\",\"password\":\"$2\"}")"
    [ "$st" = 200 ] || fail "login $1: $st $(body)"
    jq -r '.token // .session_token // empty' "$SANDBOX/body"
}
MEM="$(login mem k2test-Member-Horse-71)"; ADA="$(login ada k2test-Admin-Horse-71)"
[ -n "$MEM" ] && [ -n "$ADA" ] || fail "no session tokens"
st="$(post "/cli/thread/answer?token=$MEM" "$(answer '')")"
[ "$st" = 403 ] || fail "a Member answered: $st $(body)"
finish() { post "/cli/vault/confirm/finish?token=$1" "{\"action\":\"vault-share-service\",\"secretId\":\"$REQ\",\"method\":\"$2\",\"password\":\"$3\"}"; }
st="$(finish "$PP" vault-password x)"
[ "$st" = 403 ] || fail "an agent got a confirm: $st $(body)"
st="$(finish "$ADA" connect-password k2test-Admin-Horse-71)"
[ "$st" = 200 ] || fail "confirm finish: $st $(body)"
TOKEN="$(jq -r .token "$SANDBOX/body")"; [ -n "$TOKEN" ] && [ "$TOKEN" != null ] || fail "no token: $(body)"
st="$(post "/cli/thread/answer?token=$OWNER" "$(answer ",\"confirmToken\":\"$TOKEN\"")")"
[ "$st" = 403 ] || fail "someone else's confirm: $st $(body)"; has "$(body)" "vault_confirm_required"
st="$(finish "$ADA" connect-password k2test-Admin-Horse-71)"
[ "$st" = 200 ] || fail "confirm finish again: $st $(body)"
TOKEN="$(jq -r .token "$SANDBOX/body")"
st="$(post "/cli/thread/answer?token=$ADA" "$(answer ",\"confirmToken\":\"$TOKEN\"")")"
[ "$st" = 200 ] || fail "Allow: $st $(body)"
[ "$(jq -r .action.result.outcome "$SANDBOX/body")" = approved ] || fail "not approved: $(body)"
hasnt "$(body)" "$VALUE"
ok "a Member can't answer; an Admin confirmed with her password and allowed it"

[ "$(curl -s "http://127.0.0.1:$SVC_PORT/PREVIEW_PIN" -o /dev/null -w '%{http_code}')" = 404 ] || fail "a running process got it without a restart"
run agent_k2 "$PP" "$AGENT" publish stop sharesvc
[ "$rc" = 0 ] || fail "stop: $out $err"
run agent_k2 "$PP" "$AGENT" publish start sharesvc
[ "$rc" = 0 ] || fail "start: $out $err"
wait_http "$SVC_PORT"
[ "$(curl -s "http://127.0.0.1:$SVC_PORT/PREVIEW_PIN")" = "$VALUE" ] || fail "the service can't read the file after the restart"
ok "after the agent's restart the service reads \$K2_SECRETS_DIR/PREVIEW_PIN"

echo "== once, and no value anywhere =="
st="$(post "/cli/thread/answer?token=$ADA" "$(answer ",\"confirmToken\":\"$TOKEN\"")")"
[ "$st" = 409 ] || fail "a second answer: $st $(body)"
run agent_k2 "$PP" "$AGENT" permission status "$REQ"
[ "$rc" = 0 ] || fail "status: $out $err"; has "$out" "approved"; hasnt "$out" "$VALUE"
curl -s "http://127.0.0.1:$PORT/cli/vault/audit?token=$OWNER&limit=200" >"$SANDBOX/audit.json"
has "$(cat "$SANDBOX/audit.json")" '"grant"'; hasnt "$(cat "$SANDBOX/audit.json")" "$VALUE"
curl -s "http://127.0.0.1:$PORT/cli/thread/read?token=$OWNER&addr=$HA" >"$SANDBOX/thread.json" || true
hasnt "$(cat "$SANDBOX/thread.json")" "$VALUE"
hasnt "$(cat "$SANDBOX/a/daemon.log")" "$VALUE"
if grep -rIl --exclude-dir=run --exclude-dir=tmp -F -- "$VALUE" "$HOME_A/.k2" >"$SANDBOX/leaks" 2>/dev/null; then
    fail "the value is on disk in the clear: $(cat "$SANDBOX/leaks")"
fi
ok "a second answer is 409; the value is in no answer, audit, log or plain file"

echo "PASS: $pass checks"
