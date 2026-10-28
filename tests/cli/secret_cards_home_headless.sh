#!/usr/bin/env bash
# 0.46.6 headless smoke: secret cards that say how a secret is sent
# (prd-secret-card-injection-v1) and accepting a secret handoff from Home
# (prd-accept-cards-from-home-v1 S4). Two sandbox daemons under temp HOMEs
# (worktree build), air-gapped; agent CLIs are `exec cat` shims.
#
# Daemon A (secret cards):
#   1. the agent posts a Basic card (--inject-dst basic); stdout is the card
#      id, stderr says what the card says;
#   2. an answer without the `inject` echo (an older client) is 409
#      card_inject_unseen "Update K2 to answer this card"; nothing is saved;
#   3. the owner answers with the echo; the agent's `k2 vault fetch` reaches a
#      local listener with `Authorization: Basic base64(user:pw)` and sees
#      the echo redacted;
#   4. the agent can't change how it is sent (403), and can't put a card
#      secret in a request body.
# Daemon B (Home): a Member's login on B (what a Home remote room and K2's
# conversation widget send) decides a pending handoff card; the same request
# with a widget `origin` (query or body) is 403 handoff_person_only; a
# replay is 409. Values never reach stdout, stderr or a response.
#
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2CLI="$ROOT/cli/k2"
for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

DAEMON=""
for cand in "${CARGO_TARGET_DIR:-$ROOT/target}/debug/k2-daemon" "${CARGO_TARGET_DIR:-$ROOT/target}/release/k2-daemon"; do
    [ -x "$cand" ] && { DAEMON="$cand"; break; }
done
[ -n "$DAEMON" ] || { echo "FAIL: build k2-daemon first" >&2; exit 1; }

SANDBOX="$(mktemp -d /tmp/k2sc-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
PIDS=()
pass=0
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    sleep 0.3
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done
    rm -rf "$SANDBOX"
}
trap cleanup EXIT
fail() {
    echo "FAIL: $*" >&2
    for n in a b; do echo "--- daemon $n log tail" >&2; tail -30 "$SANDBOX/$n/daemon.log" >&2 || true; done
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
has() { grep -Fq -- "$2" <<<"$1" || fail "missing '$2' in: $1"; }
hasnt() { if grep -Fq -- "$2" <<<"$1"; then fail "unexpected '$2' in: $1"; fi; }
run() { set +e; out="$("$@" 2>"$SANDBOX/stderr")"; rc=$?; err="$(cat "$SANDBOX/stderr")"; set -e; }

start_daemon() {
    local name="$1" home="$SANDBOX/$1/home" shim="$SANDBOX/$1/shim"
    mkdir -p "$home/tmp" "$home/run" "$shim"
    cat >"$shim/claude" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac
env >"$shim/claude.env"
exec cat
EOF
    chmod +x "$shim/claude"
    TMPDIR="$home/tmp" XDG_RUNTIME_DIR="$home/run" K2_RUNTIME_DIR= HOME="$home" K2_AIRGAP=1 \
        K2_TEST_AGENT_SHIM_DIR="$shim" K2_HEARTBEAT_NO_SELF_HEAL=1 \
        "$DAEMON" >"$SANDBOX/$name/daemon.log" 2>&1 &
    PIDS+=("$!")
    for _ in $(seq 1 150); do
        [ -s "$home/.k2/heartbeat.port" ] && [ -s "$home/.k2/heartbeat.token" ] && break
        sleep 0.2
    done
    [ -s "$home/.k2/heartbeat.port" ] || fail "daemon $name wrote its port"
    local port; port="$(cat "$home/.k2/heartbeat.port")"
    for _ in $(seq 1 150); do
        curl -sf "http://127.0.0.1:$port/boot-status" | grep -q '"ready"' && break
        sleep 0.2
    done
    ok "daemon $name up on $port"
}

# owner_k2 <daemon> args… / agent_k2 <daemon> <passport> <workspace path> args…
owner_k2() {
    local h="$SANDBOX/$1/home"; shift
    env -i HOME="$h" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$(cat "$h/.k2/heartbeat.port")" \
        ${K2_PROJECT_PATH:+K2_PROJECT_PATH="$K2_PROJECT_PATH"} bash "$K2CLI" "$@"
}
agent_k2() {
    local h="$SANDBOX/$1/home" pp="$2" dir="$3"; shift 3
    env -i HOME="$h" PATH="$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 K2_PORT="$(cat "$h/.k2/heartbeat.port")" \
        K2_HOOK_TOKEN="$pp" K2_HOOK_SOCK="$SANDBOX/not-a-socket.sock" K2_PROJECT_PATH="$dir" bash "$K2CLI" "$@"
}
port_of() { cat "$SANDBOX/$1/home/.k2/heartbeat.port"; }
tok_of() { cat "$SANDBOX/$1/home/.k2/heartbeat.token"; }
post() { curl -s -o "$SANDBOX/body" -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "$3" "http://127.0.0.1:$(port_of "$1")$2"; }

# hire <daemon> <name> → its path; handle_of <daemon> <path> → handle
hire() {
    local d="$1" name="$2" dir="$SANDBOX/$1/work/$2"
    mkdir -p "$SANDBOX/$1/work"
    owner_k2 "$d" agent hire "$dir" --name "$name" --json >"$SANDBOX/hire.json" || fail "hire $name: $(cat "$SANDBOX/hire.json")"
    echo "$dir"
}
handle_of() {
    curl -s "http://127.0.0.1:$(port_of "$1")/cli/projects/list?token=$(tok_of "$1")" |
        python3 -c 'import json,sys
d=json.load(sys.stdin); want=sys.argv[1]
rows=d if isinstance(d,list) else (d.get("projects") or d.get("items") or [])
for p in rows:
    if p.get("path")==want: print(p.get("handle") or p.get("name")); break' "$2"
}
passport_of() { # start the pinned chat of <path> on <daemon>; print its passport
    local d="$1" dir="$2" shim="$SANDBOX/$1/shim"
    rm -f "$shim/claude.env"
    owner_k2 "$d" agent restart "$dir" --json >/dev/null || fail "start the pinned chat of $dir"
    for _ in $(seq 1 100); do [ -s "$shim/claude.env" ] && break; sleep 0.1; done
    sed -n 's/^K2_HOOK_TOKEN=//p' "$shim/claude.env" | head -1
}

echo "== two sandbox daemons =="
start_daemon a
start_daemon b

# ── Daemon A: a Basic card, end to end ──────────────────────────────────
echo "== A: the agent posts a Basic card =="
# A local listener that records the request head and echoes it back.
cat >"$SANDBOX/echo.py" <<'EOF'
import http.server, sys
log = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        head = self.requestline + "\n" + "".join(f"{k}: {v}\n" for k, v in self.headers.items())
        open(log, "a").write(head)
        body = head.encode()
        self.send_response(200); self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(log + ".port", "w").write(str(s.server_address[1]))
s.serve_forever()
EOF
python3 "$SANDBOX/echo.py" "$SANDBOX/echo.log" &
PIDS+=("$!")
for _ in $(seq 1 50); do [ -s "$SANDBOX/echo.log.port" ] && break; sleep 0.1; done
EP="$(cat "$SANDBOX/echo.log.port")"

OPS_A="$(hire a robotops)"
HA="$(handle_of a "$OPS_A")"; [ -n "$HA" ] || fail "no handle for $OPS_A"
PP_A="$(passport_of a "$OPS_A")"; [ -n "$PP_A" ] || fail "A's pinned chat got no passport"
run agent_k2 a "$PP_A" "$OPS_A" thread secret "$HA" --name ROBOT --kind password --for "http://127.0.0.1:$EP" --inject-dst basic --prompt "Robot login, as user:password"
[ "$rc" = 0 ] || fail "post the card exit $rc: $out $err"
CARD="$(head -1 <<<"$out")"
[ -n "$CARD" ] && [ "$(wc -l <<<"$out")" = 1 ] || fail "stdout is the card id only: $out"
has "$err" "card: K2 will send ROBOT as a Basic login (user:password) to 127.0.0.1:$EP"
ok "the card id on stdout, what the card says on stderr"
run agent_k2 a "$PP_A" "$OPS_A" thread secret "$HA" --name LEAKY --for "http://127.0.0.1:$EP" --inject-dst body
[ "$rc" != 0 ] || fail "a body card was posted: $out"
has "$err" "inject_body_not_on_cards"
run agent_k2 a "$PP_A" "$OPS_A" thread secret "$HA" --name ENVY --delivery env --inject-dst basic
[ "$rc" != 0 ] || fail "an env card with a sending was posted: $out"
has "$err" "inject_needs_fetch"
ok "body and env+sending are refused at post"

echo "== A: an older client (no echo) is refused; the owner answers with it =="
PW="k2test-robot-$RANDOM$RANDOM"
st="$(post a "/cli/thread/answer?token=$(tok_of a)" "{\"addr\":\"$HA\",\"id\":\"$CARD\",\"secret\":\"robotuser:$PW\",\"disclosed\":\"1\"}")"
[ "$st" = 409 ] || fail "no echo: $st $(cat "$SANDBOX/body")"
b="$(cat "$SANDBOX/body")"; has "$b" "card_inject_unseen"; has "$b" "Update K2 to answer this card"; hasnt "$b" "$PW"
st="$(post a "/cli/thread/answer?token=$(tok_of a)" "{\"addr\":\"$HA\",\"id\":\"$CARD\",\"secret\":\"$PW\",\"disclosed\":\"1\",\"inject\":{\"dst\":\"basic\",\"fmt\":\"{}\"}}")"
[ "$st" = 400 ] || fail "Basic without user: $st $(cat "$SANDBOX/body")"
has "$(cat "$SANDBOX/body")" "basic_needs_user"; hasnt "$(cat "$SANDBOX/body")" "$PW"
st="$(post a "/cli/thread/answer?token=$(tok_of a)" "{\"addr\":\"$HA\",\"id\":\"$CARD\",\"secret\":\"robotuser:$PW\",\"disclosed\":\"1\",\"inject\":{\"dst\":\"basic\",\"fmt\":\"{}\"}}")"
[ "$st" = 200 ] || fail "answer: $st $(cat "$SANDBOX/body")"
hasnt "$(cat "$SANDBOX/body")" "$PW"
ok "409 card_inject_unseen without the echo, 400 basic_needs_user, saved with it"

echo "== A: the agent's fetch sends a Basic login, and sees it redacted =="
run agent_k2 a "$PP_A" "$OPS_A" vault fetch --secret ROBOT "http://127.0.0.1:$EP/server"
[ "$rc" = 0 ] || fail "fetch exit $rc: $out $err"
WIRE="Basic $(printf 'robotuser:%s' "$PW" | base64)"
has "$(tr "[:upper:]" "[:lower:]" < "$SANDBOX/echo.log")" "authorization: $(tr "[:upper:]" "[:lower:]" <<<"$WIRE")"
hasnt "$out$err" "$PW"; hasnt "$out$err" "$WIRE"
has "$out" "[k2v:ROBOT]"
ok "the wire carries Authorization: Basic …; the agent sees [k2v:ROBOT]"
# The policy route the CLI's set/hosts/delivery verbs use, as the agent.
st="$(post a "/cli/vault/policy?token=$PP_A" "{\"name\":\"ROBOT\",\"inject\":{\"dst\":\"header:X-Leak\"}}")"
[ "$st" = 403 ] || fail "agent changing how it is sent: $st $(cat "$SANDBOX/body")"
has "$(cat "$SANDBOX/body")" "vault_owner_confirm_required"
ok "the agent can't change how it is sent (403 vault_owner_confirm_required)"

# ── Daemon B: decide a handoff card from Home ───────────────────────────
echo "== B: a pending handoff for ops =="
SALES="$(hire b sales)"; OPS="$(hire b ops)"
HS="$(handle_of b "$SALES")"; HO="$(handle_of b "$OPS")"
[ -n "$HS" ] && [ -n "$HO" ] || fail "handles on B: $HS $HO"
K2_PROJECT_PATH="$SALES" owner_k2 b connections add "$HO" >/dev/null || fail "connect sales → ops"
PP_S="$(passport_of b "$SALES")"; [ -n "$PP_S" ] || fail "sales got no passport"
HKEY="k2test-handoff-$RANDOM$RANDOM"
printf '%s' "$HKEY" | agent_k2 b "$PP_S" "$SALES" vault set HOME_KEY --for api.stripe.com >/dev/null || fail "sales vault set"
run agent_k2 b "$PP_S" "$SALES" vault send HOME_KEY --to "$HO" --json
DID="$(jq -r '.deliveryId // empty' <<<"$out")"
[ -n "$DID" ] || fail "send: rc=$rc $out $err"
[ "$(jq -r .status <<<"$out")" = pending ] || fail "pending: $out"
ok "sales sent HOME_KEY to ops; it waits for a person"

echo "== B: a Member's login decides (Home and K2's conversation widget send this) =="
printf 'password123\n' | owner_k2 b users add mia --role member --password-stdin >/dev/null || fail "users add mia"
st="$(post b "/cli/auth/login" '{"username":"mia","password":"password123"}')"
[ "$st" = 200 ] || fail "login: $st $(cat "$SANDBOX/body")"
MIA="$(jq -r '.token // .session_token // empty' "$SANDBOX/body")"; [ -n "$MIA" ] || fail "no session token"
# A widget's origin (query, then body only): refused, nothing lands.
st="$(post b "/cli/vault/incoming/decide?token=$MIA&origin=%7B%22widget%22%3A%22w%22%7D" "{\"id\":\"$DID\",\"action\":\"accept\"}")"
[ "$st" = 403 ] || fail "origin query: $st $(cat "$SANDBOX/body")"
has "$(cat "$SANDBOX/body")" "handoff_person_only"; has "$(cat "$SANDBOX/body")" "answer it in the agent's Thread"
st="$(post b "/cli/vault/incoming/decide?token=$MIA" "{\"id\":\"$DID\",\"action\":\"accept\",\"origin\":{\"widget\":\"w\",\"garden\":\"g\"}}")"
[ "$st" = 403 ] || fail "origin body: $st $(cat "$SANDBOX/body")"
# The agent itself: refused.
st="$(post b "/cli/vault/incoming/decide?token=$PP_S" "{\"id\":\"$DID\",\"action\":\"accept\"}")"
[ "$st" = 403 ] || fail "passport decide: $st $(cat "$SANDBOX/body")"
ok "a widget origin and an agent passport are refused (403)"
st="$(post b "/cli/vault/incoming/decide?token=$MIA" "{\"id\":\"$DID\",\"action\":\"accept\"}")"
[ "$st" = 200 ] || fail "member decide: $st $(cat "$SANDBOX/body")"
[ "$(jq -r .delivery.status "$SANDBOX/body")" = landed ] || fail "landed: $(cat "$SANDBOX/body")"
[ "$(jq -r .delivery.decidedBy "$SANDBOX/body")" = "connect:mia" ] || fail "decidedBy: $(cat "$SANDBOX/body")"
hasnt "$(cat "$SANDBOX/body")" "$HKEY"
st="$(post b "/cli/vault/incoming/decide?token=$MIA" "{\"id\":\"$DID\",\"action\":\"accept\"}")"
[ "$st" = 409 ] || fail "replay: $st $(cat "$SANDBOX/body")"
has "$(cat "$SANDBOX/body")" "not_pending"
ok "the Member's login accepts it (landed, by connect:mia); a replay is 409"

for n in a b; do hasnt "$(cat "$SANDBOX/$n/daemon.log")" "$PW"; hasnt "$(cat "$SANDBOX/$n/daemon.log")" "$HKEY"; done
ok "no value in either daemon's log"
echo "secret_cards_home_headless: $pass passed"
