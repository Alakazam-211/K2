#!/usr/bin/env bash
# 0.46.4 App-move polish (Agent NSI's list), through the CLI against a
# sandbox daemon (temp HOME, K2_APPS=1 + K2_APPS_PROCESS=1, air-gapped;
# this checkout's cli/k2):
#
#   - k2 app move-plan <service> [--json] [--request]: every step, marked;
#     one held request for the owner's steps (exit 2).
#   - k2 app warnings: the App's own env (K2_PASS_FILE) reads "expected".
#   - k2 app build: the Seed size line, the heaviest paths, .k2appignore.
#   - k2 app domain add|rm / k2 app domains.
#   - k2 app audit --oldest-first.
#   - k2 app user add <app> <name> --test, the list's test flag,
#     k2 app test-guests, k2 app user remove <name> --test.
#   - a bare App id on an owner verb (k2 app people).
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
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2mp-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    pkill -f "http.server $SVC_PORT" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    chmod -R u+w "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
SVC_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME" "$SANDBOX/tmp"
HOME="$DHOME" TMPDIR="$SANDBOX/tmp" K2_APPS=1 K2_APPS_PROCESS=1 K2_AIRGAP=1 \
    "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
k2() { HOME="$DHOME" "$K2" "$@"; }
run() { set +e; OUT="$(k2 "$@" 2>&1)"; RC=$?; set -e; }

WS="$SANDBOX/ws"
mkdir -p "$WS"
git -C "$WS" -c init.defaultBranch=main init -q
git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m one
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
expect_has "workspace registered" "$reg" "\"path\":\"$WS\""
cd "$WS"
export K2_PROJECT_PATH="$WS"

# The legacy --cmd service (App-mode code already reads its pass file).
printf 'const pass = readFileSync(process.env.K2_PASS_FILE);\nconst data = process.env.K2_APP_DATA;\n' > "$WS/server.mjs"
run publish run agents --cmd "python3 -m http.server $SVC_PORT" --port "$SVC_PORT" --no-tunnel
expect_eq "legacy service started" "$RC" "0"
expect_has "the App installed" "$(k2 app install mvapp --memory none --engine process)" "App mvapp installed"

echo "== move-plan =="
run app move-plan agents --app mvapp
expect_eq "move-plan: exit 0" "$RC" "0"
expect_has "  names the move" "$OUT" "into App mvapp"
expect_has "  install is done" "$OUT" "[x] agent  Install the App"
expect_has "  a process-engine App has no box to prepare" "$OUT" "[-] owner  Prepare the box for Apps"
expect_has "  the Build switch is the owner's" "$OUT" "[ ] owner  Turn on \"Build and run Apps\""
expect_has "  no public label: n/a" "$OUT" "[-] agent  Move the service's own hostname"
expect_has "  stopping the old service" "$OUT" "k2 publish stop agents"
expect_has "  one approval for the owner's steps" "$OUT" "k2 app move-plan agents --app mvapp --request"
run app move-plan agents --app mvapp --json
ids="$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(",".join(s["id"] for s in json.load(sys.stdin)["steps"]))')"
expect_eq "  --json: every step in order" "$ids" "box_prep,engine,build_switch,install,deploy,copy_host,guest_rooms,data_put,label_take,stop_old"
run app move-plan agents --app mvapp --request
expect_eq "--request: held (exit 2)" "$RC" "2"
expect_has "  says so" "$OUT" "Held for the owner: request w_"
run app approvals
expect_has "  one card in approvals" "$OUT" "move agents into App mvapp"
run app move-plan mvapp
expect_eq "an App isn't a --cmd service: refused" "$RC" "3"
expect_has "  not_a_cmd_service" "$OUT" "not_a_cmd_service"

echo "== warnings: the App's own env is expected =="
run app warnings agents
expect_has "K2_PASS_FILE is expected" "$OUT" "expected"
expect_has "  named" "$OUT" "K2_PASS_FILE"
expect_not "  never a break" "$(printf '%s' "$OUT" | grep BREAKS || true)" "K2_PASS_FILE"

echo "== build: Seed size and .k2appignore =="
F="$WS/bff"
mkdir -p "$F/var" "$F/node_modules/.cache"
printf '{"schema":1,"kind":"app","id":"mvapp","name":"mvapp","version":"1.0.0","host":{"mode":"cmd","start":["python3","-m","http.server"],"health":{"path":"/"}}}' > "$F/manifest.json"
python3 -c 'import sys; open(sys.argv[1], "w").write("x" * 300000)' "$F/var/state.json"
python3 -c 'import sys; open(sys.argv[1], "w").write("c" * 200000)' "$F/node_modules/.cache/big"
run app build "$F"
expect_eq "build: exit 0" "$RC" "0"
expect_has "  the size line" "$OUT" "Seed: "
expect_has "  the heaviest path" "$OUT" "largest: var/state.json"
printf 'var/\nnode_modules/.cache\n' > "$F/.k2appignore"
run app build "$F"
expect_has "  .k2appignore applied" "$OUT" ".k2appignore applied"
expect_not "  var/ left out" "$OUT" "var/state.json"

echo "== domains =="
run app domains mvapp
expect_eq "domains: exit 0" "$RC" "0"
expect_has "  its own label" "$OUT" "(its own label)"
run app domain add mvapp extra1
expect_eq "domain add on an App that isn't live: refused" "$RC" "3"
expect_has "  app_not_healthy" "$OUT" "app_not_healthy"
run app domain add mvapp agents
expect_has "the service's own label is label take" "$OUT" "use_label_take"
run app domain rm mvapp extra1
expect_has "rm of nothing attached" "$OUT" "domain_not_attached"

echo "== audit order =="
run app audit mvapp --oldest-first
expect_eq "audit --oldest-first: exit 0" "$RC" "0"

echo "== test guests =="
run app user add mvapp qa1 --test --expires 2h
expect_eq "test guest added" "$RC" "0"
expect_has "  says it" "$OUT" "Test guest qa1 of App mvapp"
expect_has "  the password, once" "$OUT" "password (shown once): "
run app user list
expect_has "the roster flags it" "$OUT" "[test: App mvapp"
run app test-guests mvapp
expect_has "test-guests lists it" "$OUT" "mvapp: 1 test guest"
run app user add mvapp qa2 --test --expires 8d
expect_eq "past 7 days: refused (exit 2)" "$RC" "2"
run app user remove qa1 --test
expect_has "removed" "$OUT" "Test guest qa1 removed."
run app test-guests mvapp
expect_has "  gone" "$OUT" "mvapp: 0 test guests"

echo "== a bare App id on an owner verb =="
run app people mvapp
expect_eq "k2 app people <bare App id>: exit 0" "$RC" "0"

k2 publish stop agents >/dev/null 2>&1 || true
expect_has "uninstall" "$(k2 app uninstall mvapp)" "App mvapp uninstalled"
echo
echo "app_move_polish: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
