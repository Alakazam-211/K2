#!/usr/bin/env bash
# k2 app (App runtime stage 1, prd-k2-app-runtime-v1 S6–S9) — CLI smoke: a
# sandbox daemon (temp HOME, K2_APPS=1 + K2_APPS_PROCESS=1 for the process engine, air-gapped) + this checkout's
# cli/k2 on loopback. Fixture Apps are python3 HTTP servers.
#
# Covers: validate (ok and failing, exit 1); build prints a Seed; install;
# deploy (exit 0, live), a failing health gate (exit 1, the old version
# keeps serving); rollback (instant, warm); deployment (who/commit/state);
# list/inspect/history (audit chain verifies); logs and errors are framed;
# config --memory 8G / none; stop/start; uninstall; a refusal exits 3.
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

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2ar-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    # Fixture Apps outlive a daemon stop by design (they are re-adopted);
    # kill each one's process group from its run file.
    for rf in "$SANDBOX"/home/.k2/apps/*/run/*.json; do
        [ -f "$rf" ] || continue
        pid="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$rf" 2>/dev/null || true)"
        [ -n "$pid" ] && kill -9 -- "-$pid" 2>/dev/null || true
    done
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    # Seeds are read-only trees: make them removable first.
    chmod -R u+w "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME" "$SANDBOX/tmp"
echo "== sandbox daemon =="
# Its own TMPDIR too: the daemon keeps runtime files (frpc.toml) there,
# and its shutdown reaps strays matching that path.
HOME="$DHOME" TMPDIR="$SANDBOX/tmp" K2_APPS=1 K2_APPS_PROCESS=1 K2_AIRGAP=1 K2_APPS_WATCH_SECS=120 K2_APPS_DEPLOY_TIMEOUT_SECS=10 K2_APPS_DRAIN_SECS=2 K2_APPS_STOP_GRACE_SECS=2 \
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

echo "== workspace =="
WS="$SANDBOX/ws"
mkdir -p "$WS"
# Workspaces are git checkouts (add-from-path registers only those).
git -C "$WS" -c init.defaultBranch=main init -q
git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m one
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
expect_has "workspace registered" "$reg" "\"path\":\"$WS\""
cd "$WS"
export K2_PROJECT_PATH="$WS"

mkapp() { # dir version tag
    mkdir -p "$1"
    cat > "$1/manifest.json" <<EOF
{"schema":1,"kind":"app","id":"smoke","name":"Smoke","version":"$2",
 "host":{"mode":"cmd","start":["python3","server.py"],"health":{"path":"/healthz"},"drainSecs":2}}
EOF
    printf '%s' "$3" > "$1/tag.txt"
    cat > "$1/server.py" <<'EOF'
import http.server, os, sys
TAG = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "tag.txt")).read().strip()
print("smoke %s up; error: none" % TAG, flush=True)
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        code = 500 if (TAG.startswith("sick") and self.path == "/healthz") else 200
        b = (TAG + "\n").encode()
        self.send_response(code); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
http.server.ThreadingHTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
EOF
}
mkapp "$WS/v1" 1.0.0 v1
mkapp "$WS/v2" 2.0.0 v2
mkapp "$WS/sick" 3.0.0 sick

echo "== validate / build =="
out="$(k2 app validate "$WS/v1")"
expect_has "validate ok (manifest, contract and the stage 0 code pass)" "$out" "ok — checks: manifest, contract, code"
mkdir -p "$WS/broken"
echo '{"schema":1,"kind":"app","id":"smoke"' > "$WS/broken/manifest.json"
set +e
k2 app validate "$WS/broken" > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "validate errors exit 1" "$rc" "1"
expect_has "validate names the problem" "$(cat "$SANDBOX/out")" "manifest_invalid"
out="$(k2 app build "$WS/v1")"
expect_has "build prints a Seed" "$out" "Seed "
SEED1="$(printf '%s\n' "$out" | sed -n 's/^Seed \([0-9a-f]\{64\}\)$/\1/p')"
[ -n "$SEED1" ] || bad "Seed hash parsed"
out="$(k2 app validate "${SEED1:0:16}")"
expect_has "one k2 app validate also takes a stored Seed" "$out" "ok — checks: manifest, contract, code"

echo "== install / deploy / rollback =="
# 0.46.3: a new App defaults to a container; this smoke drives the
# unwalled process engine, so it asks for it.
out="$(k2 app install smoke --memory none --engine process)"
expect_has "installed" "$out" "App smoke installed"
FRONT="$(k2 app list --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["apps"][0]["frontPort"])')"
out="$(k2 app deploy smoke "$SEED1")"
expect_has "deploy is live" "$out" "live on port"
expect_eq "front serves v1" "$(curl -s "http://127.0.0.1:$FRONT/")" "v1"
k2 app deploy smoke "$WS/v2" >/dev/null
expect_eq "front serves v2" "$(curl -s "http://127.0.0.1:$FRONT/")" "v2"
set +e
k2 app deploy smoke "$WS/sick" > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "a failed health gate exits 1" "$rc" "1"
expect_has "failed with the reason" "$(cat "$SANDBOX/out")" "health_failed"
expect_eq "the old version keeps serving" "$(curl -s "http://127.0.0.1:$FRONT/")" "v2"
out="$(k2 app rollback smoke)"
expect_has "rollback is instant" "$out" "instant"
expect_eq "front serves v1 again" "$(curl -s "http://127.0.0.1:$FRONT/")" "v1"

echo "== see it =="
out="$(k2 app deployment smoke)"
expect_has "deployment shows who" "$out" "who     owner"
expect_has "deployment shows the state" "$out" "state   live"
out="$(k2 app list)"
expect_has "list shows the App" "$out" "smoke"
expect_has "list shows the label" "$out" "Preview, unwalled (process)"
# 0.46.4 (update continuity R16): the engine column, last on the App's line.
expect_has "list shows the engine column" "$(printf '%s\n' "$out" | grep '^smoke ')" "engine unwalled, K2 "
expect_eq "list --json carries the engine block" \
    "$(k2 app list --json | python3 -c 'import json,sys; e=json.load(sys.stdin)["apps"][0]["engine"]; print(e["fingerprint"]["wall"], e["rehostDue"])')" "unwalled None"
out="$(k2 app inspect smoke)"
expect_has "inspect: audit verifies" "$out" "chain verifies"
out="$(k2 app history smoke)"
expect_has "history has the rollback" "$out" "rollback"
expect_has "history: audit verifies" "$out" "audit chain: verifies"
out="$(k2 app logs smoke)"
expect_has "logs are framed" "$out" "<untrusted-app-output app=\"smoke\" kind=\"logs\">"
expect_has "logs carry the App's output" "$out" "smoke v1 up"
out="$(k2 app errors smoke)"
expect_has "errors list the failed deploy" "$out" "health_failed"

echo "== config / stop / start / uninstall =="
expect_has "memory set" "$(k2 app config smoke --memory 8G)" "memory limit 8192 MiB"
expect_has "memory cleared" "$(k2 app config smoke --memory none)" "memory limit none"

echo "== disk ceilings / exec / shell / process exception (lane D) =="
expect_has "io: tighten writes" "$(k2 app config smoke --io-write 20M --io-write-iops 500)" "write 20 MiB/s"
expect_has "io: shows on inspect" "$(k2 app inspect smoke --json)" '"wiops":500'
expect_has "io: the owner may lift it" "$(k2 app config smoke --io none)" "write none"
expect_has "io: back to the default" "$(k2 app config smoke --io default)" "write 40 MiB/s"
set +e
k2 app config smoke --io-write 40X > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "io: a bad rate exits 2" "$rc" "2"
set +e
k2 app exec smoke -- ls > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "exec into a preview (process-engine) App is refused (exit 3)" "$rc" "3"
expect_has "exec says why" "$(cat "$SANDBOX/out")" "exec_unavailable"
set +e
k2 app exec smoke > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "exec without a command is usage (exit 2)" "$rc" "2"
set +e
k2 app shell smoke < /dev/null > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "shell into a preview App is refused before the upgrade (exit 3)" "$rc" "3"
expect_has "shell says why" "$(cat "$SANDBOX/out")" "exec_unavailable"
expect_has "exception off on a guarded/preview App is a no-op" "$(k2 app exception smoke off)" "(no change)"
resp="$(curl -s -X POST -H 'Content-Type: application/json' --data '{"app":"smoke","on":true}' "http://127.0.0.1:$PORT/cli/app/exception?token=$OWNER_TOKEN")"
# One confirm code (Rosson 2026-10-16): vault_confirm_required on Linux;
# off Linux the process exception isn't available at all.
if [ "$(uname -s)" = Linux ]; then want_code=vault_confirm_required; else want_code=engine_unavailable; fi
case "$resp" in
    *owner_confirm_required*) bad "exception on answers the retired owner_confirm_required: $resp" ;;
    *"\"$want_code\""*) ok "exception on without an owner confirm is refused ($want_code)" ;;
    *) bad "exception on without a confirm: $resp" ;;
esac
expect_has "audit has the io change" "$(k2 app history smoke --json)" '"event":"config"'

expect_has "stop" "$(k2 app stop smoke)" "smoke: stopped"
expect_eq "front answers 503 when stopped" "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$FRONT/")" "503"
expect_has "start" "$(k2 app start smoke)" "live on port"
set +e
k2 app deploy nosuch "$SEED1" > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "a refusal exits 3" "$rc" "3"
expect_has "refusal names the code" "$(cat "$SANDBOX/out")" "app_not_found"

echo "== expose / uninstall keeps access (App-governed Spaces S7) =="
expect_has "expose --local on a local App is a no-op" "$(k2 app expose smoke --local)" "already local only"
set +e
k2 app expose smoke --tunnel > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "expose --tunnel on an air-gapped box is refused (exit 3)" "$rc" "3"
expect_has "and says the URL step can't run" "$(cat "$SANDBOX/out")" "engine_unavailable"
expect_has "still local after the refusal" "$(k2 app expose smoke --local --json)" '"changed":false'
set +e
k2 app expose smoke > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "expose needs --tunnel or --local (exit 2)" "$rc" "2"
expect_eq "GET /cli/app/expose is 405" "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/app/expose?token=$OWNER_TOKEN")" "405"
printf 'smoke-user-pw-1\n' | k2 app user add smokeu --app smoke --password-stdin > "$SANDBOX/out" 2>&1 || bad "app user add: $(cat "$SANDBOX/out")"
set +e
k2 app uninstall smoke > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "uninstall with an App login is refused (exit 3)" "$rc" "3"
expect_has "it names the code" "$(cat "$SANDBOX/out")" "uninstall_drops_access"
expect_has "and the count" "$(cat "$SANDBOX/out")" "1 App login"
expect_has "still installed" "$(k2 app list)" "smoke"
# Release and workspace remove: all or nothing, every App listed.
set +e
k2 app release "$WS" --apps uninstall > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "release --apps uninstall with an App login is refused (exit 3)" "$rc" "3"
expect_has "  nothing was released" "$(cat "$SANDBOX/out")" "nothing was released"
expect_has "  it lists the App and its count" "$(cat "$SANDBOX/out")" "smoke: 1 App login"
set +e
k2 workspace remove "$WS" --apps uninstall > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "workspace remove --apps uninstall is refused too (exit 3)" "$rc" "3"
expect_has "  same refusal" "$(cat "$SANDBOX/out")" "uninstall_drops_access"
expect_has "the App is still installed" "$(k2 app list)" "smoke"
expect_has "the workspace is still registered" "$(curl -s "http://127.0.0.1:$PORT/cli/projects/list?token=$OWNER_TOKEN")" "$WS"
set +e
k2 app release "$WS" --apps park --drop-access > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "--drop-access only goes with --apps uninstall (exit 2)" "$rc" "2"
expect_has "uninstall --drop-access" "$(k2 app uninstall smoke --drop-access)" "App smoke uninstalled"
expect_has "list is empty" "$(k2 app list)" "No Apps on this box"

echo "== box doctor / prepare (S10) =="
# A sandbox daemon never has a protocol-6 root helper (a Mac has none;
# a Linux build box's helper predates protocol 6), so the box is not
# ready and prepare refuses before any root verb. Prepare is only tried
# when the doctor confirms that: this smoke never preps a real box.
set +e
out="$(k2 app doctor --json)"; rc=$?
set -e
expect_eq "doctor: a box without a protocol-6 helper is not ready (exit 1)" "$rc" "1"
expect_has "doctor lists the helper check" "$out" '"id":"helper"'
expect_has "doctor lists the platform check" "$out" '"id":"platform"'
expect_has "doctor names the apps protocol" "$out" '"appsMinProtocol":6'
human="$(k2 app doctor 2>&1 || true)"
expect_has "doctor prints a verdict" "$human" "App box: NOT READY"
proto="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["helperProtocol"])')"
if [ "$proto" -lt 6 ]; then
    set +e
    k2 app box prepare > "$SANDBOX/out" 2>&1; rc=$?
    set -e
    expect_eq "prepare without a protocol-6 helper exits 3" "$rc" "3"
    case "$(cat "$SANDBOX/out")" in
        *linux_only*|*helper_needed*) ok "prepare names why (linux_only / helper_needed)" ;;
        *) bad "prepare names why: $(cat "$SANDBOX/out")" ;;
    esac
else
    bad "the sandbox daemon saw a protocol-$proto helper: refusing to prepare a real box from a smoke"
fi
set +e
k2 app box prepare --runtime runc > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "prepare refuses an unknown runtime (exit 2)" "$rc" "2"

echo "== K2's Podman bundle (0.46.1 lane P) =="
# --dry-run is a read: it answers on any box and runs no mutating root verb.
set +e
out="$(k2 app box prepare --dry-run --json)"; rc=$?
set -e
expect_eq "prepare --dry-run answers (exit 0)" "$rc" "0"
expect_has "dry run says so" "$out" '"dryRun":true'
expect_has "dry run changes nothing" "$out" '"changed":false'
expect_has "dry run reports the Podman decision" "$out" '"podman":{"action":'
expect_has "dry run lists the steps" "$out" '"apps packages install"'
human="$(k2 app box prepare --dry-run 2>&1 || true)"
expect_has "dry run prints its verdict" "$human" "Dry run: nothing changed."
# Uninstall needs a protocol-8 helper on Linux: refused here, exit 3.
set +e
k2 app box podman uninstall > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "podman uninstall without a protocol-8 helper exits 3" "$rc" "3"
case "$(cat "$SANDBOX/out")" in
    *linux_only*|*helper_needed*) ok "podman uninstall names why (linux_only / helper_needed)" ;;
    *) bad "podman uninstall names why: $(cat "$SANDBOX/out")" ;;
esac
set +e
k2 app box podman remove > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "podman takes only uninstall (exit 2)" "$rc" "2"
# F8: undoing box prep needs the helper on Linux too: refused here, exit 3.
set +e
k2 app box unprep --yes > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "unprep without a helper exits 3" "$rc" "3"
case "$(cat "$SANDBOX/out")" in
    *linux_only*|*helper_needed*) ok "unprep names why (linux_only / helper_needed)" ;;
    *) bad "unprep names why: $(cat "$SANDBOX/out")" ;;
esac
set +e
k2 app box auto-prepare again > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "auto-prepare takes only retry (exit 2)" "$rc" "2"

echo "== guarded engine (S11/S12) =="
# Same rule: a sandbox never has a protocol-6 helper, so the engine install
# refuses before any download or root call, and a guarded deploy without an
# engine fails its pipeline (never falls back to the process engine).
if [ "$proto" -lt 6 ]; then
    set +e
    k2 app engine install > "$SANDBOX/out" 2>&1; rc=$?
    set -e
    expect_eq "engine install without a protocol-6 helper exits 3" "$rc" "3"
    case "$(cat "$SANDBOX/out")" in
        *linux_only*|*helper_needed*) ok "engine install names why (linux_only / helper_needed)" ;;
        *) bad "engine install names why: $(cat "$SANDBOX/out")" ;;
    esac
fi
expect_has "a second App for the engine checks" "$(k2 app install walled --memory none)" "App walled installed"
set +e
k2 app deploy walled "$WS/v1" --engine vm > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "deploy refuses an unknown engine (exit 2)" "$rc" "2"
set +e
k2 app deploy walled "$WS/v1" --engine guarded --json > "$SANDBOX/out" 2>&1; rc=$?
set -e
case "$(cat "$SANDBOX/out")" in
    *engine_not_answering*) ok "a guarded deploy without an engine is refused, not run unwalled (rc $rc)" ;;
    *) bad "guarded deploy without an engine: rc $rc $(cat "$SANDBOX/out")" ;;
esac
[ "$rc" != 0 ] && ok "and it does not exit 0" || bad "guarded deploy without an engine exited 0"
if [ "$(uname -s)" = Linux ]; then
    expect_has "the guarded choice is recorded on the App" "$(k2 app list)" "Guarded (container)"
fi
expect_has "uninstall the engine-check App" "$(k2 app uninstall walled)" "App walled uninstalled"

echo
echo "app_runtime_cli_smoke: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
