#!/usr/bin/env bash
# k2 app resources + the disk guard (prd-app-resources-disk-guard-v1, 0.46.3,
# §17.2 test 21): a sandbox daemon (temp HOME, K2_APPS=1 + K2_APPS_PROCESS=1,
# air-gapped) and this checkout's cli/k2 on loopback.
#
# The disk can't be faked in a separate daemon, so the test moves the LINES
# instead: a critical-free line of 900 TiB puts any real disk at critical,
# deterministically, without writing a byte.
#
# Covers: --json shape; inspect --resources; settings (show, change, a bad
# line exits 2); a refused deploy and build exit 3 with disk_low; the
# owner's override at critical is held (exit 2 with the request); restart
# is never blocked; mode warn goes through with a warning on stderr; mode
# off is today's behaviour.
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
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2res-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    for rf in "$SANDBOX"/home/.k2/apps/*/run/*.json; do
        [ -f "$rf" ] || continue
        pid="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$rf" 2>/dev/null || true)"
        [ -n "$pid" ] && kill -9 -- "-$pid" 2>/dev/null || true
    done
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    chmod -R u+w "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME" "$SANDBOX/tmp"
echo "== sandbox daemon =="
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

echo "== the default mode =="
out="$(k2 app resources settings --json)"
want_mode=on
[ "$(uname -s)" = Darwin ] && want_mode=warn
expect_eq "default mode ($(uname -s))" "$(printf '%s' "$out" | python3 -c 'import json,sys; g=json.load(sys.stdin)["guard"]; print(g["mode"], g["modeSource"])')" "$want_mode default"
# Setup runs with the guard off, so the real disk can't decide it.
out="$(k2 app resources settings --mode off)"
expect_has "settings saved" "$out" "disk guard saved"
expect_has "mode off printed" "$out" "mode      off (setting)"

echo "== workspace and App =="
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
mkapp() { # dir version tag
    mkdir -p "$1"
    cat > "$1/manifest.json" <<EOF
{"schema":1,"kind":"app","id":"usage","name":"Usage","version":"$2",
 "host":{"mode":"cmd","start":["python3","server.py"],"health":{"path":"/healthz"},"drainSecs":2}}
EOF
    printf '%s' "$3" > "$1/tag.txt"
    cat > "$1/server.py" <<'EOF'
import http.server, os
TAG = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "tag.txt")).read().strip()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = (TAG + "\n").encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
http.server.ThreadingHTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
EOF
}
mkapp "$WS/v1" 1.0.0 v1
mkapp "$WS/v2" 2.0.0 v2
out="$(k2 app install usage --memory none --engine process)"
expect_has "installed" "$out" "App usage installed"
out="$(k2 app deploy usage "$WS/v1" 2>"$SANDBOX/err")"
expect_has "deploy is live" "$out" "live on port"
expect_not "mode off: no disk warning" "$(cat "$SANDBOX/err")" "disk warning"

echo "== the view =="
out="$(k2 app resources --json)"
shape="$(printf '%s' "$out" | python3 -c '
import json, sys
v = json.load(sys.stdin)
row = [c for c in v["consumers"] if c["kind"] == "app" and c["id"] == "usage"]
print(v["ok"], v["guard"]["mode"], len(v["disks"]) > 0, "level" in v["disks"][0], len(row), row[0]["diskBytes"] if row else "x",
      "lines" in v["guard"], isinstance(v["recent"], list), "box" in v and "scan" in v)')"
expect_eq "--json shape" "$shape" "True off True True 1 None True True True"
out="$(k2 app resources)"
expect_has "human view: disk block" "$out" "DISK"
expect_has "human view: the App row" "$out" "usage"
out="$(k2 app inspect usage --resources)"
expect_has "inspect --resources prints the App's row" "$out" "usage"
set +e
k2 app inspect nope --resources > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "inspect --resources of an unknown App exits 3" "$rc" "3"
expect_has "  app_not_found" "$(cat "$SANDBOX/out")" "app_not_found"

echo "== settings =="
set +e
k2 app resources settings --warn 97 > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "a bad line exits 2" "$rc" "2"
expect_has "  it names the rule" "$(cat "$SANDBOX/out")" "warn < refuse < critical"
# Any real disk is now critical (900 TiB free is the critical line).
out="$(k2 app resources settings --mode on --critical-free 900T --reserve 900T --warn-free 900T)"
expect_has "lines saved" "$out" "critical  98% used or under 921600 GiB free"

echo "== refusals =="
set +e
k2 app deploy usage "$WS/v2" > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "deploy past the lines exits 3" "$rc" "3"
expect_has "  disk_low" "$(cat "$SANDBOX/out")" "disk_low"
expect_has "  the hint says how to override" "$(cat "$SANDBOX/out")" "--disk-override"
set +e
k2 app build "$WS/v2" > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "build past the lines exits 3" "$rc" "3"
expect_has "  disk_low" "$(cat "$SANDBOX/out")" "disk_low"
set +e
k2 app deploy usage "$WS/v2" --disk-override "cli test" > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "the owner's override at critical is held: exit 2" "$rc" "2"
expect_has "  it prints the request" "$(cat "$SANDBOX/out")" "held for the owner"
expect_not "  never an empty error" "$(cat "$SANDBOX/out")" '"hint": "None"'
out="$(k2 app restart usage 2>&1)" && rc=0 || rc=$?
expect_eq "restart is never blocked" "$rc" "0"

echo "== mode warn and off =="
k2 app resources settings --mode warn >/dev/null
out="$(k2 app deploy usage "$WS/v2" 2>"$SANDBOX/err")"
expect_has "mode warn: it goes through" "$out" "live on port"
expect_has "  with the warning on stderr" "$(cat "$SANDBOX/err")" "disk warning:"
expect_has "  saying what on would do" "$(cat "$SANDBOX/err")" "would have been refused"
k2 app resources settings --mode off >/dev/null
out="$(k2 app deploy usage "$WS/v1" --json 2>"$SANDBOX/err")"
expect_eq "mode off: no new fields" "$(printf '%s' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(any(k in v for k in ("diskWarning","memoryWarning","diskOverride")))')" "False"

echo "== rescan =="
out="$(k2 app resources --rescan 2>&1 >/dev/null)"
expect_has "rescan answers" "$out" "box usage scan"

echo "== clean (0.46.4, Slice B) =="
# A rotated log beside the App's log, and App data: clean takes the first,
# never the second.
mkdir -p "$DHOME/.k2/apps/usage/logs" "$DHOME/.k2/apps/usage/data"
printf 'old\n' > "$DHOME/.k2/apps/usage/logs/d-old.log.1"
printf 'keep\n' > "$DHOME/.k2/apps/usage/data/keep.db"
out="$(k2 app resources clean --dry-run --json)"
shape="$(printf '%s' "$out" | python3 -c '
import json, sys
v = json.load(sys.stdin)
ids = [i["id"] for i in v["items"]]
print(v["ok"], v["dryRun"], v["scope"], "apps/usage/logs/d-old.log.1" in ids, any("data" in i for i in ids),
      all(not i["removed"] for i in v["items"]), "App data" in v["never"])')"
expect_eq "clean --dry-run --json shape" "$shape" "True True box True False True True"
[ -f "$DHOME/.k2/apps/usage/logs/d-old.log.1" ] && ok "a dry run removes nothing" || bad "a dry run removed the log"
out="$(k2 app resources clean --dry-run)"
expect_has "human dry run" "$out" "Would free (this box)"
expect_has "  the item" "$out" "rotated log"
expect_has "  what it never touches" "$out" "Never touched: App data"
out="$(k2 app resources clean)"
expect_has "clean frees" "$out" "Cleaned (this box)"
[ ! -e "$DHOME/.k2/apps/usage/logs/d-old.log.1" ] && ok "the rotated log is gone" || bad "the rotated log stayed"
[ -f "$DHOME/.k2/apps/usage/data/keep.db" ] && ok "App data stays" || bad "clean touched App data"
st="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/app/resources/clean?token=$OWNER_TOKEN")"
expect_eq "GET on clean is 405" "$st" "405"

echo "== trends (0.46.4, Slice B) =="
out="$(k2 app resources trends --window 1h --json)"
shape="$(printf '%s' "$out" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["ok"], v["window"], v["resSecs"], isinstance(v["box"]["points"], list), v["budgets"]["enforced"], v["retention"]["minutesSecs"])')"
expect_eq "trends --json shape" "$shape" "True 1h 60 True False 86400"
out="$(k2 app resources trends)"
expect_has "human trends" "$out" "CPU and memory, last 24h"
set +e
k2 app resources trends --window 2h > "$SANDBOX/out" 2>&1; rc=$?
set -e
expect_eq "a bad window exits 2" "$rc" "2"

expect_has "uninstall" "$(k2 app uninstall usage)" "App usage uninstalled"

echo
echo "app_resources: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
