#!/usr/bin/env bash
# `k2 doc cores` (prd-multiplayer-cores-settings-v1 §7, S3) end to end
# against a foreground sandbox daemon built from THIS worktree (temp HOME,
# no client attached): list, on/off, --json, an unknown core (exit 2), an
# agent (SCOPED_TOKEN: person_only, exit 3, no call), a Member login (exit
# 3), the turn-off report line, and exit 1 when a write-back failed.
#
#   cargo build -p k2-daemon && bash tests/cli/doc_cores.sh
#
# Every check fails loudly with the output.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d /tmp/k2cores.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
DAEMON_PID=""
DLOG="$SB/daemon.log"
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    chmod -R u+w "$WORK" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -40 "$DLOG" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }

# cli [env K=V ...] -- <k2 args>: OUT, ERR, RC.
cli() {
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT")
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    set +e
    OUT="$(env "${envs[@]}" "$K2_CLI" "$@" 2>"$WORK/cli.err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }
cores_json() { curl -s "$BASE/cli/docs/cores?token=$DTOKEN"; }

echo "== daemon =="
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$DLOG" 2>&1 &
DAEMON_PID=$!
phase=""
for _ in $(seq 1 400); do
    if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
        DPORT="$(cat "$SB/.k2/daemon.port")"
        phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
        [ "$phase" = "ready" ] && break
    fi
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
ok "sandbox daemon ready ($DAEMON_BIN)"

echo "== help =="
set +e
help_out="$(env HOME="$SB" "$K2_CLI" doc cores --help 2>&1)"; help_rc=$?
set -e
[ "$help_rc" = 0 ] || fail "k2 doc cores --help exit $help_rc: $help_out"
has "$help_out" "k2 doc cores on <core>" "cores help"
has "$(env HOME="$SB" "$K2_CLI" doc --help 2>&1)" "Live docs are switched per kind of file on each server: k2 doc cores." "k2 doc --help names k2 doc cores"
ok "help: k2 doc cores --help, and the line in k2 doc --help"

echo "== list =="
cli -- doc cores
want_rc 0 "list"
has "$OUT" "Multiplayer cores on this server (the owner or an Admin turns them on or off):" "list header"
has "$OUT" "  markdown   off   Markdown core (Beta): .md and .markdown files" "markdown row"
has "$OUT" "  text       off   Text and code core (Beta): code, config, plain text, logs, CSV/TSV as text" "text row"
has "$OUT" "  any-file   off   Any other file (Beta): presence, a lease, versions and restore (agents only for now)" "any-file row"
has "$OUT" "While a core is off, k2 doc refuses its files (core_off, exit 3) and people open them the usual way." "list footer"
ok "k2 doc cores lists three cores, all off"
cli -- doc cores --json
want_rc 0 "list --json"
jok "$OUT" 'j["ok"] is True and j["canChange"] is True and [c["id"] for c in j["cores"]] == ["markdown", "text", "any-file"] and j["later"] == ["CSV grid", "Word"]' "--json is the daemon's answer"

echo "== bad usage =="
cli -- doc cores on word
want_rc 2 "unknown core"
has "$ERR" "no such core: word (markdown, text, any-file)" "unknown core message"
cli -- doc cores flip markdown
want_rc 2 "bad action"
cli -- doc cores on
want_rc 2 "on without a core"
ok "an unknown core or a bad action exits 2"

echo "== an agent: person_only, no call =="
# The daemon is out of reach for this call (port 1): person_only must
# stop it before any request is made.
cli K2_PORT=1 K2_HOOK_TOKEN=k2hook_not_real K2_HOOK_SOCK="$WORK/nope.sock" -- doc cores on markdown
want_rc 3 "agent on"
has "$ERR" "Error [person_only]: k2 doc cores on|off is for people" "person_only"
jok "$(cores_json)" 'not any(c["on"] for c in j["cores"])' "an agent's on|off makes no call (every core still off)"
[ ! -e "$SB/.k2/docs.redb" ] || fail "the docs store was opened"

echo "== on =="
cli -- doc cores on markdown
want_rc 0 "on markdown"
[ "$OUT" = "The Markdown core is on. Its files are live docs on this server from now on." ] || fail "on line: $OUT"
ok "on: one line"
cli -- doc cores on markdown
want_rc 0 "on again"
[ "$OUT" = "The Markdown core is already on." ] || fail "already line: $OUT"
ok "on again: already on"
cli -- doc cores on text --json
want_rc 0 "on text --json"
jok "$OUT" 'j["multiplayerCores"] == {"markdown": True, "text": True} and j["multiplayerCoresReport"]["changed"] == [{"core": "text", "on": True}]' "on --json: the switches and the report"
for _ in $(seq 1 100); do
    st="$(curl -s "$BASE/cli/docs/status?token=$DTOKEN")"
    jcheck "$st" 'j["pool"]["running"]' && break
    sleep 0.1
done
jok "$st" 'j["pool"]["running"]' "turning a core on started the docs runtime"

echo "== a Member login: refused =="
printf 'password1\n' | env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" "$K2_CLI" users add mo --password-stdin >/dev/null 2>&1 || true
login="$(curl -s -X POST -H 'Content-Type: application/json' --data-raw '{"username":"mo","password":"password1"}' "$BASE/cli/auth/login")"
MEM="$(printf '%s' "$login" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("token") or v.get("session_token") or "")')"
[ -n "$MEM" ] || fail "member login: $login"
cli -- doc cores off markdown --host "http://127.0.0.1:$DPORT" --token "$MEM"
want_rc 3 "member off"
has "$ERR" 'updating "multiplayerCores" requires the Owner or Admin role' "member refusal"
jok "$(cores_json)" '[c["on"] for c in j["cores"]] == [True, True, False]' "a Member's off changed nothing"
cli -- doc cores --host "http://127.0.0.1:$DPORT" --token "$MEM" --json
want_rc 0 "member list"
jok "$OUT" 'j["canChange"] is False' "a Member may list the cores, never change them"

echo "== off: written back and closed; a failed write-back exits 1 =="
WS="$WORK/ws"
mkdir -p "$WS/ro"
printf '# Notes\n' >"$WS/notes.md"
printf 'one\n' >"$WS/ro/x.txt"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git -c init.defaultBranch=main -C "$WS" init -q || fail "git init"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$WS")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *'"error"'*) fail "register $WS: $reg" ;; *"$WS"*) ;; *) fail "register $WS: $reg" ;; esac
edit() { # <rel> <after> <text>
    local r
    r="$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
        --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "edits": [{"after": int(sys.argv[2]), "text": sys.argv[3]}]}))' "$1" "$2" "$3")" \
        "$BASE/cli/docs/edit?token=$DTOKEN&workspace=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$WS")")"
    [ "$r" = 200 ] || fail "edit $1: HTTP $r"
}
edit notes.md 1 "live line"
cli -- doc cores off markdown
want_rc 0 "off markdown"
[ "$OUT" = "The Markdown core is off. 1 live file was written to disk and closed (notes.md)." ] || fail "off line: $OUT"
grep -q "live line" "$WS/notes.md" || fail "notes.md lacks the live edit after off: $(cat "$WS/notes.md")"
ok "off: the report line, and the edit is on disk"
cli -- doc cat "$WS/notes.md"
want_rc 3 "cat after off"
has "$ERR" "notes.md: the Markdown core is off on this server. The owner or an Admin turns it on in Settings → Cores → Multiplayer (k2 doc cores on markdown, people only). Until then, edit the file with ordinary tools." "core_off refusal"
ok "k2 doc cat on a file whose core is off exits 3 with core_off"
cli -- doc cores off markdown
want_rc 0 "off again"
[ "$OUT" = "The Markdown core is already off." ] || fail "already off: $OUT"
edit ro/x.txt 1 "two"
for _ in $(seq 1 80); do grep -q two "$WS/ro/x.txt" && break; sleep 0.1; done
grep -q two "$WS/ro/x.txt" || fail "ro/x.txt never written back"
chmod 555 "$WS/ro"
edit ro/x.txt 2 "while read-only"
cli -- doc cores off text
want_rc 1 "off text with a failed write-back"
has "$OUT" "The text and code core is off. 1 file couldn't be written: ro/x.txt (" "failed line"
has "$OUT" "its changes are kept in K2's history and are written when the core is turned on again." "failed line tail"
chmod 755 "$WS/ro"
ok "a failed write-back: the flip completes, the line names it, exit 1"
cli -- doc cores --json
jok "$OUT" '[c["on"] for c in j["cores"]] == [False, False, False] and [f["path"] for f in j["cores"][1]["lastChange"]["failed"]] == ["ro/x.txt"]' "every core off; the text row keeps the failed path"
cli -- doc cores on text
want_rc 0 "text on again"
cli -- doc cat "$WS/ro/x.txt"
want_rc 0 "cat after on"
for _ in $(seq 1 80); do grep -q "while read-only" "$WS/ro/x.txt" && break; sleep 0.1; done
grep -q "while read-only" "$WS/ro/x.txt" || fail "the kept change wasn't written after on: $(cat "$WS/ro/x.txt")"
ok "turned on again: the kept change is written"

echo "PASS ($pass checks)"
