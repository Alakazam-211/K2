#!/usr/bin/env bash
# Garden access (0.46.3): the one-time hint after the upgrade. The first
# `k2 zen …` in a K2 workspace (K2_PROJECT_PATH with its .k2 folder) prints
#   Gardens changed in 0.46.3: see `k2 study garden-changes` (update before 0.47.0)
# once, on stderr only, and writes the workspace's own
# .k2/.garden-changes-hint. Never with --json, never on stdout, never in
# ~/.k2, never without a K2 workspace (a plain shell, ~, a test sandbox).
#
# No daemon: `k2 zen guide` is static (K2_PORT=1 + a dummy token). HOME is
# a throwaway dir; nothing touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
[ -x "$K2" ] || { echo "FAIL: $K2 not found/executable" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2gch-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/ws/.k2" "$SANDBOX/ws2/.k2" "$SANDBOX/plain"
cd "$SANDBOX/plain"

HINT='Gardens changed in 0.46.3: see `k2 study garden-changes` (update before 0.47.0)'

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "  ok: $1"; }
bad() { fail=$((fail + 1)); echo "  FAIL: $1" >&2; }

# zen <project-path or -> <args…>: stdout in $SOUT, stderr in $SERR; exit 0.
zen() {
    local proj="$1" rc
    shift
    set +e
    if [ "$proj" = "-" ]; then
        SOUT="$(K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2" zen "$@" </dev/null 2>"$SANDBOX/err")"
    else
        SOUT="$(K2_PORT=1 K2_HOOK_TOKEN=test-token K2_PROJECT_PATH="$proj" "$K2" zen "$@" </dev/null 2>"$SANDBOX/err")"
    fi
    rc=$?
    set -e
    SERR="$(cat "$SANDBOX/err")"
    [ "$rc" = 0 ] || bad "k2 zen $* exited $rc: $SERR"
}

echo "== first k2 zen in a workspace: the hint, once, on stderr =="
zen "$SANDBOX/ws" guide
if [ "$SERR" = "$HINT" ]; then ok "the hint is the only stderr line"; else bad "stderr: $SERR"; fi
case "$SOUT" in *"garden-changes"*) bad "the hint reached stdout" ;; *) ok "stdout has no hint" ;; esac
if [ -f "$SANDBOX/ws/.k2/.garden-changes-hint" ]; then ok "marker in the workspace's .k2"; else bad "no marker in ws/.k2"; fi
if [ -e "$HOME/.k2/.garden-changes-hint" ]; then bad "marker written in ~/.k2"; else ok "nothing in ~/.k2"; fi
zen "$SANDBOX/ws" guide
if [ -z "$SERR" ]; then ok "second run: no hint"; else bad "second run stderr: $SERR"; fi

echo "== --json: never the hint, no marker =="
zen "$SANDBOX/ws2" guide --json
if [ -z "$SERR" ]; then ok "--json: nothing on stderr"; else bad "--json stderr: $SERR"; fi
if printf '%s' "$SOUT" | python3 -c 'import json,sys; json.load(sys.stdin)'; then ok "--json: stdout is JSON only"; else bad "--json stdout: $SOUT"; fi
if [ -e "$SANDBOX/ws2/.k2/.garden-changes-hint" ]; then bad "--json wrote the marker"; else ok "--json: no marker"; fi
zen "$SANDBOX/ws2" guide
if [ "$SERR" = "$HINT" ]; then ok "the next text-mode run in ws2 gets it"; else bad "ws2 text stderr: $SERR"; fi

echo "== no K2 workspace: no hint =="
zen - guide
if [ -z "$SERR" ]; then ok "no K2_PROJECT_PATH: no hint"; else bad "no project stderr: $SERR"; fi
zen "$SANDBOX/plain" guide
if [ -z "$SERR" ]; then ok "a folder without .k2: no hint"; else bad "plain stderr: $SERR"; fi
if [ -e "$SANDBOX/plain/.k2" ]; then bad "made a .k2 in a plain folder"; else ok "no .k2 made"; fi
zen "$HOME" guide
if [ -z "$SERR" ]; then ok "K2_PROJECT_PATH=~: no hint"; else bad "home stderr: $SERR"; fi
if [ -e "$HOME/.k2/.garden-changes-hint" ]; then bad "marker written in ~/.k2"; else ok "still nothing in ~/.k2"; fi

echo "garden_changes_hint: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
