#!/usr/bin/env bash
# 0.46.3 r6 smoke fixes, through the CLI (a sandbox daemon: temp HOME,
# K2_APPS=1 + K2_APPS_PROCESS=1, air-gapped; this checkout's cli/k2):
#
#   - k2 app wallet settings <App id> --on stores it under the App's service
#     id (what guest sign-in keys on); an unknown App is refused.
#   - k2 publish client-ip <label> on refuses a label that doesn't exist.
#   - k2 app doctor / k2 app resources / k2 app box prepare --dry-run off
#     Linux: only what applies, honest RAM wording, no K2-internal folders,
#     and the dry run's package section.
#   - On Linux: the dry run lists the exact distro packages it would install.
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
expect_ne() { if [ "$2" != "$3" ]; then ok "$1"; else bad "$1 (both $(printf %q "$2"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2sf-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    chmod -R u+w "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
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
expect_has "App installed" "$(k2 app install wapp --memory none --engine process)" "App wapp installed"
SVC="$(k2 app inspect wapp --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["row"]["serviceId"])')"
expect_ne "the App's service id isn't its name" "$SVC" "wapp"

echo "== wallet settings by App id =="
run app wallet settings wapp --on
expect_eq "by App id: exit 0" "$RC" "0"
expect_has "  names the App and its service id" "$OUT" "wapp ($SVC): wallet on"
got="$(k2 app wallet settings "$SVC" --json | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["settings"]["serviceId"], v["settings"]["enabled"], v["appId"])')"
expect_eq "by service id: the same row, on" "$got" "$SVC True wapp"
run app wallet settings nosuchapp-zz --on
expect_ne "an unknown App: refused" "$RC" "0"
expect_has "  app_not_found" "$OUT" "app_not_found"
expect_has "  names it" "$OUT" "nosuchapp-zz"
run app wallet settings nosuchapp-zz
expect_ne "an unknown App: show refused too" "$RC" "0"

echo "== publish client-ip =="
run publish client-ip nosuch-zz on
expect_eq "an unknown label: exit 2" "$RC" "2"
expect_has "  says so" "$OUT" "no nested service \"nosuch-zz\""
expect_has "  nothing listed" "$(k2 publish client-ip)" "none (all raw)"
run publish client-ip wapp on
expect_eq "an App's label: on" "$RC" "0"
expect_has "  on" "$OUT" "wapp: client IP on"
run publish client-ip wapp off
expect_eq "  and off" "$RC" "0"

echo "== doctor, resources, dry run =="
if [ "$(uname -s)" = Darwin ]; then
    run app doctor
    expect_eq "doctor off Linux: exit 1 (not ready)" "$RC" "1"
    expect_has "  says Apps are Linux-only" "$OUT" "not a Linux box"
    expect_not "  no box-prep fix" "$OUT" "box prepare"
    expect_not "  no root helper row" "$OUT" "root-helper"
    expect_not "  no automatic prep line" "$OUT" "Automatic prep"
    run app box prepare --dry-run
    expect_eq "dry run off Linux: exit 0" "$RC" "0"
    expect_has "  no package manager, said plainly" "$OUT" "Distro packages: none (no supported package manager"
else
    run app box prepare --dry-run --json
    expect_eq "dry run: exit 0" "$RC" "0"
    pk="$(printf '%s' "$OUT" | python3 -c 'import json,sys; p=json.load(sys.stdin)["packages"]; print(p.get("manager") or "", isinstance(p.get("install"), list), bool(p.get("distroPodman") or p.get("error") or p.get("note")))')"
    case "$pk" in
        "apt True True"|"dnf True True"|"pacman True True") ok "dry run JSON lists the packages ($pk)" ;;
        *) bad "dry run JSON lists the packages (got $pk)" ;;
    esac
    run app box prepare --dry-run
    expect_has "  text lists them" "$OUT" "Distro packages"
    expect_not "  never 'untouched'" "$OUT" "Podman is untouched"
fi
k2 app resources --rescan --wait >/dev/null 2>&1 || true
run app resources
expect_eq "resources: exit 0" "$RC" "0"
expect_not "  no _orphan folder" "$OUT" "_orphan"
expect_not "  no _broadcast folder" "$OUT" "_broadcast"
expect_not "  never '— available'" "$OUT" "— available"
if [ "$(uname -s)" = Darwin ]; then
    expect_has "  RAM wording on macOS" "$OUT" "available not measured on this OS"
fi

expect_has "uninstall" "$(k2 app uninstall wapp)" "App wapp uninstalled"
echo
echo "app_smoke_fixes_0463: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
