#!/usr/bin/env bash
# Email Notifications headless check (prd-email-notifications-core-v1 N5).
#
# A daemon-only run: no client, no webview, no mail. It starts a DEBUG
# k2-daemon in a throwaway HOME with K2_TEST_EMAIL_SINK_DIR set, so every
# relay hand-off lands in a local folder instead of any SMTP server (a
# release build ignores that variable and the script refuses to run one).
# Then it drives the owner's flow over curl only:
#
#   vault confirm password → owner address → relay add (confirm) →
#   sender add on the relay (confirm) → relay test (confirm; to the owner
#   only) → system sender set (confirm) → system-sender test (confirm) →
#   the log (metadata only) → sender rm → its address stays reserved →
#   the App door refuses without a pass → GET on a mutation is 405.
#
#   scripts/email-notifications-headless.sh <path/to/debug/k2-daemon>
#   scripts/email-notifications-headless.sh --service <path/to/debug/k2-daemon>
#
# The full run needs a PERSON's terminal (your own SSH login or a terminal
# outside K2): the vault mints confirm tokens only for a person. --service
# is for a run under K2's own service group (a K2 Compute job): it checks
# everything up to the confirms and that the vault refuses to mint one
# (person_required), then stops; it never skips silently.
#
# Linux or macOS. Every K2_* variable is unset first; the real ~/.k2, the
# keychain and any running daemon are never touched. Fails loudly (exit 1)
# on the first wrong answer.

set -euo pipefail

MODE=person
if [ "${1:-}" = "--service" ]; then MODE=service; shift; fi
BIN="${1:-}"
[ -x "$BIN" ] || { echo "usage: $0 [--service] <path/to/debug/k2-daemon>" >&2; exit 2; }
case "$BIN" in
    */release/*) echo "refusing a release build: the sink only exists in debug builds" >&2; exit 2 ;;
esac

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/k2-email-headless-XXXXXX")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME"
SINK="$SANDBOX/sink"
export K2_TEST_EMAIL_SINK_DIR="$SINK"
PID=""
cleanup() {
    if [ -n "$PID" ]; then
        kill "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true
    fi
    if [ "${KEEP:-0}" != 1 ]; then rm -rf "$SANDBOX"; fi
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log (last 40 lines) ---" >&2
    tail -40 "$SANDBOX/daemon.log" >&2 || true
    exit 1
}
pass() { echo "PASS: $*"; }

# On Linux the vault's person check refuses a caller in the daemon's own
# cgroup ("K2's own service group"), so a person run starts the daemon in
# its own transient scope, the way systemd runs it on a box.
LAUNCH=()
if [ "$MODE" = person ] && [ "$(uname -s)" = Linux ] && command -v systemd-run >/dev/null 2>&1; then
    LAUNCH=(systemd-run --scope --quiet --collect --unit "k2-email-headless-$$" --)
fi
# ${LAUNCH[@]+…}: macOS's bash 3.2 calls an empty array unbound under set -u.
${LAUNCH[@]+"${LAUNCH[@]}"} "$BIN" >"$SANDBOX/daemon.log" 2>&1 &
PID=$!

# A fresh HOME: the daemon writes only the canonical ~/.k2.
K2DIR=""
for _ in $(seq 1 150); do
    d="$HOME/.k2"
    if [ -s "$d/daemon.port" ] && { [ -s "$d/daemon.token" ] || [ -s "$d/heartbeat.token" ]; }; then K2DIR="$d"; break; fi
    sleep 0.2
done
[ -n "$K2DIR" ] || fail "the daemon never wrote its port and token"
PORT="$(tr -d '[:space:]' < "$K2DIR/daemon.port")"
TOKEN="$(cat "$K2DIR/daemon.token" 2>/dev/null || cat "$K2DIR/heartbeat.token")"
BASE="http://127.0.0.1:$PORT"
for _ in $(seq 1 150); do
    if curl -s "$BASE/boot-status" | grep -q '"ready"'; then break; fi
    sleep 0.2
done
curl -s "$BASE/boot-status" | grep -q '"ready"' || fail "the daemon never became ready"
curl -s "$BASE/boot-status" | grep -q 'email-notifications-v1' || fail "boot-status lacks email-notifications-v1"
pass "headless daemon ready on $PORT with email-notifications-v1"

# HTTP helpers: STATUS and BODY of the last call.
STATUS=""
BODY=""
call() {
    local method="$1" path="$2" body="${3:-}" out
    if [ "$method" = GET ]; then
        out="$(curl -s -G "$BASE$path" --data-urlencode "token=$TOKEN" ${body:+--data "$body"} -w $'\n%{http_code}')"
    else
        out="$(printf '%s' "$body" | curl -s -X POST "$BASE$path?token=$TOKEN" -H 'Content-Type: application/json' --data-binary @- -w $'\n%{http_code}')"
    fi
    STATUS="${out##*$'\n'}"
    BODY="${out%$'\n'*}"
}
field() { printf '%s' "$BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) else (v or {}).get(k)
print("" if v is None else v)' "$1"; }
expect() { [ "$STATUS" = "$1" ] || fail "$2: HTTP $STATUS (want $1): $BODY"; pass "$2"; }

VSOCK="$K2DIR/vault.sock"
for _ in $(seq 1 50); do [ -S "$VSOCK" ] && break; sleep 0.2; done
[ -S "$VSOCK" ] || fail "no vault socket at $VSOCK"
vault() {
    local route="$1" body="$2" out
    out="$(printf '%s' "$body" | curl -s -X POST --unix-socket "$VSOCK" "http://localhost/cli/vault/$route" \
        -H 'X-K2-Tty: 1' -H 'Content-Type: application/json' --data-binary @- -w $'\n%{http_code}')"
    STATUS="${out##*$'\n'}"
    BODY="${out%$'\n'*}"
}
call GET /cli/notifications/email
expect 200 "status reads"
[ "$(field boxSwitch)" = on ] || fail "the box switch reads on: $BODY"
call GET /cli/notifications/email/sender/add
expect 405 "GET on a mutation is 405"
call POST /cli/notifications/email/settings '{"ownerEmail":"owner@headless.example","logRetentionDays":14}'
expect 200 "settings: the owner's own address"

RELAY_BODY='{"host":"smtp.relay.example","port":587,"tls":"starttls","username":"user","password":"HEADLESS-RELAY-SECRET","fromDomain":"headless.example"}'
call POST /cli/notifications/email/relay/add "$RELAY_BODY"
expect 403 "relay add without a confirm is refused"
[ "$(field error.code)" = vault_confirm_required ] || fail "relay add refusal code: $BODY"
case "$BODY" in *HEADLESS-RELAY-SECRET*) fail "the relay password was echoed in a refusal" ;; esac
call POST /cli/notifications/email/sender/add '{"address":"noreply@headless.example"}'
expect 403 "sender add without a confirm is refused"
[ "$(field error.secretId)" = noreply@headless.example ] || fail "the confirm is bound to the address: $BODY"
out="$(curl -s -X POST "$BASE/cli/skin/email/send" -H 'Content-Type: application/json' -d '{"to":"a@b.example","subject":"s","text":"t"}' -w $'\n%{http_code}')"
[ "${out##*$'\n'}" = 401 ] || fail "the App door without a pass: $out"
pass "the App door refuses without a pass"

VPW="headless-$(date +%s)-pw"
vault confirm-password/set "{\"password\":\"$VPW\"}"
if [ "$MODE" = service ]; then
    # Under K2's service group the vault never mints a confirm: the
    # person-only half can't run here (and must not).
    expect 403 "the vault refuses a non-person (service mode)"
    [ "$(field error.code)" = person_required ] || fail "person_required: $BODY"
    echo "ALL PASS (service mode: the confirm-gated half needs a person's terminal)"
    exit 0
fi
expect 200 "vault confirm password set"
# A fresh confirm token for (action, secretId).
token_for() {
    vault confirm/start "{\"action\":\"$1\"}"
    [ "$STATUS" = 200 ] || fail "confirm start $1: $BODY"
    vault confirm/finish "{\"action\":\"$1\",\"secretId\":\"$2\",\"method\":\"vault-password\",\"password\":\"$VPW\"}"
    [ "$STATUS" = 200 ] || fail "confirm finish $1: $BODY"
    printf '%s' "$BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])'
}

T="$(token_for email-relay smtp.relay.example)"
call POST /cli/notifications/email/relay/add "$(printf '%s' "$RELAY_BODY" | T="$T" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b["confirmToken"]=os.environ["T"]; print(json.dumps(b))')"
expect 200 "relay add"
case "$BODY" in *HEADLESS-RELAY-SECRET*) fail "the relay password was echoed" ;; esac
RELAY="$(field relay.id)"

T="$(token_for email-sender noreply@headless.example)"
call POST /cli/notifications/email/sender/add "{\"address\":\"noreply@headless.example\",\"name\":\"Headless\",\"relay\":\"$RELAY\",\"confirmToken\":\"$T\"}"
expect 200 "sender add on the relay"
SENDER="$(field sender.id)"
[ "$(field sender.state)" = needs_test ] || fail "a relay sender starts needs_test: $BODY"

T="$(token_for email-test noreply@headless.example)"
call POST /cli/notifications/email/relay/test "{\"relay\":\"$RELAY\",\"from\":\"$SENDER\",\"confirmToken\":\"$T\"}"
expect 200 "relay test (to the owner only)"
call GET /cli/notifications/email/senders
[ "$(field senders.0.state)" = ready ] || fail "the sender is ready after the test: $BODY"

T="$(token_for email-system-sender noreply@headless.example)"
call POST /cli/notifications/email/system-sender/set "{\"sender\":\"$SENDER\",\"confirmToken\":\"$T\"}"
expect 200 "system sender set"
T="$(token_for email-test noreply@headless.example)"
call POST /cli/notifications/email/system-sender/test "{\"confirmToken\":\"$T\"}"
expect 200 "system-sender test"

N="$(find "$SINK" -name '*.eml' | wc -l | tr -d ' ')"
[ "$N" = 2 ] || fail "the sink holds $N messages (want 2)"
grep -h '^X-K2-Sink-To:' "$SINK"/*.eml | grep -v 'owner@headless.example' && fail "a test reached someone else"
grep -q 'HEADLESS-RELAY-SECRET' "$SINK"/*.eml && fail "the password reached a message"
pass "both tests reached the sink, addressed to the owner only"

call GET /cli/notifications/email/log
expect 200 "the log reads"
[ "$(printf '%s' "$BODY" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["items"]))')" = 2 ] || fail "two log rows: $BODY"
case "$BODY" in *"This is a test email"*) fail "a body reached the log" ;; esac
pass "the log keeps metadata only"

call POST /cli/notifications/email/sender/rm "{\"sender\":\"$SENDER\"}"
expect 200 "sender rm"
T="$(token_for email-sender noreply@headless.example)"
call POST /cli/notifications/email/sender/add "{\"address\":\"noreply@headless.example\",\"relay\":\"$RELAY\",\"confirmToken\":\"$T\"}"
expect 409 "a removed sender's address stays reserved"
[ "$(field error.code)" = reserved ] || fail "reserved code: $BODY"

out="$(curl -s -X POST "$BASE/cli/skin/email/send" -H 'Content-Type: application/json' -d '{"to":"a@b.example","subject":"s","text":"t"}' -w $'\n%{http_code}')"
[ "${out##*$'\n'}" = 401 ] || fail "the App door without a pass: $out"
pass "the App door refuses without a pass"

echo "ALL PASS (sink: $SINK)"
