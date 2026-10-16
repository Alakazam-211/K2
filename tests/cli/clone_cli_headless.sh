#!/usr/bin/env bash
# k2 clone — headless end to end (prd-clone-carries-threads-and-vault-v1
# §4.3, §7.6, test plan 16–18): two sandbox daemons (temp HOMEs,
# air-gapped, loopback) + this checkout's cli/k2.
#
# Covers: preview names only; `clone here` (same box); `clone to` (push A→B)
# and `clone from` (pull A→B) over loopback with --token; .env files held
# back and the D4 confirm refusal; the clone gate (Can clone off → 403
# clone_not_allowed on every door, master switch off → pack refused for a
# Member, the owner never blocked); Owner-only settings key and
# set-can-clone; whoami fields; proposals (a person proposes, lists, denies);
# a passport is refused by the CLI's person guard.
#
# Never prompts: no Touch ID or password path is exercised here (those are
# unit-tested with minted confirms).
#
# Build first:  cargo build -p k2-daemon
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }
jget() { python3 -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) else v[k]
print(v if not isinstance(v,(dict,list)) else json.dumps(v))' "$1"; }

SANDBOX="$(mktemp -d /tmp/k2cc-XXXXXX)"
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    sleep 0.5
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon A log tail ---" >&2; tail -30 "$SANDBOX/a/daemon.log" >&2 || true
        echo "--- daemon B log tail ---" >&2; tail -30 "$SANDBOX/b/daemon.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

start_daemon() {
    local name="$1" home="$SANDBOX/$1/home"
    mkdir -p "$home/tmp" "$home/run"
    TMPDIR="$home/tmp" XDG_RUNTIME_DIR="$home/run" K2_RUNTIME_DIR= HOME="$home" K2_AIRGAP=1 \
        "$DAEMON" > "$SANDBOX/$name/daemon.log" 2>&1 &
    PIDS+=("$!")
    for _ in $(seq 1 150); do
        [ -s "$home/.k2/heartbeat.port" ] && [ -s "$home/.k2/heartbeat.token" ] && break
        sleep 0.2
    done
    [ -s "$home/.k2/heartbeat.port" ] || { bad "daemon $name wrote its port"; exit 1; }
    local port; port="$(cat "$home/.k2/heartbeat.port")"
    for _ in $(seq 1 100); do
        curl -sf "http://127.0.0.1:$port/boot-status" | grep -q '"ready"' && break
        sleep 0.2
    done
    ok "daemon $name up on $port"
}

echo "== two sandbox daemons =="
start_daemon a
start_daemon b
AHOME="$SANDBOX/a/home"; BHOME="$SANDBOX/b/home"
PA="$(cat "$AHOME/.k2/heartbeat.port")"; TA="$(cat "$AHOME/.k2/heartbeat.token")"
PB="$(cat "$BHOME/.k2/heartbeat.port")"; TB="$(cat "$BHOME/.k2/heartbeat.token")"
ka() { HOME="$AHOME" "$K2" "$@"; }
kb() { HOME="$BHOME" "$K2" "$@"; }
post() { curl -s -X POST -H 'Content-Type: application/json' -d "$3" "http://127.0.0.1:$1$2"; }
post_status() { curl -s -o "$SANDBOX/body" -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "$3" "http://127.0.0.1:$1$2"; }

echo "== workspace on A =="
WS="$AHOME/agents/salesws"
mkdir -p "$WS"
g() { git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false -c init.defaultBranch=main "$@"; }
g init -q
echo "hello from sales" > "$WS/notes.txt"
echo "STRIPE_KEY=sk_live_cc_planted_value_1234" > "$WS/.env"
g add notes.txt
g commit -q -m one
reg="$(post "$PA" "/cli/projects/add-from-path?token=$TA" "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")")"
expect_has "workspace registered on A" "$reg" "\"path\":\"$WS\""
cd "$WS"

echo "== whoami and defaults =="
who="$(curl -s "http://127.0.0.1:$PA/cli/auth/whoami?token=$TA")"
expect_has "owner canClone" "$who" '"canClone":true'
expect_has "owner clonePersonalComputers" "$who" '"clonePersonalComputers":true'

echo "== preview (names only) =="
pv="$(ka clone preview salesws --json)"
expect_eq "preview resolves the workspace by name" "$(printf '%s' "$pv" | jget workspace.path)" "$WS"
expect_has "the .env file is listed as held back" "$pv" '".env"'
expect_not "no secret value in the preview" "$pv" "sk_live_cc_planted"

echo "== clone here (same box) =="
mkdir -p "$AHOME/copies"
here="$(ka clone here salesws --dest "$AHOME/copies" --yes --json)"
HDEST="$(printf '%s' "$here" | jget destPath)"
expect_eq "here lands under --dest" "$(dirname "$HDEST")" "$AHOME/copies"
expect_eq "workspace file copied" "$(cat "$HDEST/notes.txt")" "hello from sales"
[ ! -e "$HDEST/.env" ] && ok ".env held back by default" || bad ".env travelled without a confirm"
expect_has "the copy is registered on A" "$(curl -s "http://127.0.0.1:$PA/cli/projects/list?token=$TA")" "\"path\":\"$HDEST\""
alias_out="$(ka clone salesws --dest "$AHOME/copies" --yes --json)"
expect_has "bare 'k2 clone <ws> --dest' is the here alias" "$alias_out" '"mode": "here"'

echo "== clone to (push A → B) =="
mkdir -p "$BHOME/landed"
to="$(ka clone to "http://127.0.0.1:$PB" salesws --token "$TB" --dest "$BHOME/landed" --yes --json)"
TDEST="$(printf '%s' "$to" | jget destPath)"
expect_eq "push lands on B under --dest" "$(dirname "$TDEST")" "$BHOME/landed"
expect_eq "pushed file" "$(cat "$TDEST/notes.txt")" "hello from sales"
expect_has "registered on B" "$(curl -s "http://127.0.0.1:$PB/cli/projects/list?token=$TB")" "\"path\":\"$TDEST\""
expect_not "no secret value in the push output" "$to" "sk_live_cc_planted"

echo "== clone from (pull A → B) =="
mkdir -p "$BHOME/pulled"
from="$(kb clone from "http://127.0.0.1:$PA:salesws" --token "$TA" --dest "$BHOME/pulled" --yes --json)"
FDEST="$(printf '%s' "$from" | jget destPath)"
expect_eq "pull lands on B under --dest" "$(dirname "$FDEST")" "$BHOME/pulled"
expect_eq "pulled file" "$(cat "$FDEST/notes.txt")" "hello from sales"
[ ! -e "$FDEST/.env" ] && ok ".env held back on pull" || bad ".env travelled on pull"

echo "== D4: .env files need a confirm =="
st="$(post_status "$PA" "/cli/clone/bundle?token=$TA" "{\"project_path\":\"$WS\",\"carry_secrets\":true}")"
expect_eq "carry_secrets without a confirm is 403" "$st" "403"
expect_has "clone_env_confirm_required" "$(cat "$SANDBOX/body")" "clone_env_confirm_required"

echo "== the clone gate =="
printf 'password1\n' | ka users add mem --password-stdin >/dev/null
login="$(post "$PA" "/cli/auth/login" '{"username":"mem","password":"password1"}')"
MEM="$(printf '%s' "$login" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("token") or v.get("session_token") or "")')"
[ -n "$MEM" ] || { bad "member login on loopback ($login)"; exit 1; }
ok "member logged in"
expect_has "member whoami canClone (default on)" "$(curl -s "http://127.0.0.1:$PA/cli/auth/whoami?token=$MEM")" '"canClone":true'
expect_eq "member previews with Can clone on" "$(post_status "$PA" "/cli/clone/preview?token=$MEM" "{\"project_path\":\"$WS\"}")" "200"
expect_has "member can't copy secrets (D1)" "$(cat "$SANDBOX/body")" '"callerCanCopySecrets":false'
expect_has "set-can-clone off (owner)" "$(ka users set-can-clone mem off)" "Can clone off"
for door in preview bundle pack unpack pack-cleanup; do
    st="$(post_status "$PA" "/cli/clone/$door?token=$MEM" "{\"project_path\":\"$WS\",\"job_id\":\"x\",\"bundle_path\":\"/x\",\"dest_parent\":\"/x\"}")"
    expect_eq "Can clone off: $door refused" "$st" "403"
    expect_has "Can clone off: $door says clone_not_allowed" "$(cat "$SANDBOX/body")" "clone_not_allowed"
done
st="$(curl -s -o "$SANDBOX/body" -w '%{http_code}' "http://127.0.0.1:$PA/cli/clone/pack-status?token=$MEM&job_id=x")"
expect_eq "Can clone off: pack-status refused" "$st" "403"
expect_has "member whoami canClone off" "$(curl -s "http://127.0.0.1:$PA/cli/auth/whoami?token=$MEM")" '"canClone":false'
set +e
kb clone from "http://127.0.0.1:$PA:salesws" --token "$MEM" --dest "$BHOME/pulled" --yes >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "CLI pull by a member without Can clone exits 3" "$rc" "3"
expect_has "CLI prints clone_not_allowed" "$(cat "$SANDBOX/err")" "clone_not_allowed"
ka users set-can-clone mem on >/dev/null

echo "== master switch =="
st="$(post_status "$PA" "/cli/settings/update?token=$MEM" '{"allowClonePersonalComputers":false}')"
expect_eq "a Member can't flip the master switch" "$st" "403"
st="$(post_status "$PA" "/cli/settings/update?token=$TA" '{"allowClonePersonalComputers":false}')"
expect_eq "the owner turns it off" "$st" "200"
st="$(post_status "$PA" "/cli/clone/pack?token=$MEM" "{\"project_path\":\"$WS\"}")"
expect_eq "switch off: member pack refused" "$st" "403"
expect_has "clone_personal_computers_off" "$(cat "$SANDBOX/body")" "clone_personal_computers_off"
expect_eq "switch off: member preview still allowed" "$(post_status "$PA" "/cli/clone/preview?token=$MEM" "{\"project_path\":\"$WS\"}")" "200"
expect_has "member whoami clonePersonalComputers off" "$(curl -s "http://127.0.0.1:$PA/cli/auth/whoami?token=$MEM")" '"clonePersonalComputers":false'
st="$(post_status "$PA" "/cli/clone/pack?token=$TA" "{\"project_path\":\"$WS\"}")"
expect_eq "switch off: the owner is never blocked" "$st" "200"
st="$(post_status "$PA" "/cli/users/set-can-clone?token=$MEM" '{"username":"mem","canClone":true}')"
expect_eq "set-can-clone is owner-only" "$st" "403"
post "$PA" "/cli/settings/update?token=$TA" '{"allowClonePersonalComputers":true}' >/dev/null

echo "== proposals (a person) =="
prop="$(ka clone propose --to here --workspace salesws --why "a spare copy" --skip STRIPE_KEY --json)"
PID_="$(printf '%s' "$prop" | jget proposal.id)"
expect_has "proposal held" "$prop" '"status": "held"'
expect_has "listed" "$(ka clone proposals)" "$PID_"
expect_has "denied" "$(ka clone deny "$PID_" --reason "not now")" "$PID_ denied"
expect_has "listed with --all" "$(ka clone proposals --all --json)" '"status": "denied"'

echo "== agents can't clone =="
set +e
HOME="$AHOME" K2_HOOK_TOKEN="fake.passport" "$K2" clone here salesws --dest "$AHOME/copies" --yes >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a passport is refused by the CLI" "$rc" "3"
expect_has "clone_agents_propose" "$(cat "$SANDBOX/err")" "clone_agents_propose"

echo
echo "clone_cli_headless: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
