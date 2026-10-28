#!/usr/bin/env bash
# Live docs (Markdown multiplayer v1, K1 + K2) end to end against a
# foreground sandbox daemon built from THIS worktree (temp HOME, no client
# attached): `/cli/docs/*` with curl as the owner token and as an agent's
# passport on its cell socket. Covers the store (open, release on stop,
# reopen and reconcile), disk sync both ways (write-back, flush, outside
# edits via FsChanged and K2's own .k2/ watch), the jail and deny list,
# the rights matrix (own / connected-only / shared-read /
# shared-write+history / unshared, MI25), history (K-PERM-9), on-behalf
# edits and approvals (K-ID-10/11), POST guards, and the passport allowlist.
#
#   cargo build -p k2-daemon && bash tests/cli/docs_headless.sh
#   bash tests/cli/docs_headless.sh --build   (builds first)
#
# Every check fails loudly with the response.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

# Short root: the cell socket (~/.k2/run/cells/<uuid>.sock) must fit SUN_LEN.
WORK="$(mktemp -d /tmp/k2docs.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
DAEMON_PID=""
DLOG="$SB/daemon1.log"
stop_daemon() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
        DAEMON_PID=""
    fi
}
cleanup() {
    stop_daemon
    pkill -9 -f "frpc -c $SB/" 2>/dev/null || true
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

# jcheck <json> <python expr over j>
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }
jq_() { python3 -c 'import json,sys; j=json.loads(sys.argv[1]); print(eval(sys.argv[2], {"j": j}))' "$1" "$2"; }
json() { python3 -c 'import json,sys; print(json.dumps(dict(zip(sys.argv[1::2], sys.argv[2::2]))))' "$@"; }

# owner <METHOD> <path> [json body] [query] → BODY, CODE
owner() {
    local m="$1" p="$2" b="${3:-}" q="${4:-}"
    local out
    if [ "$m" = "GET" ]; then
        out="$(curl -s -G -w '\n%{http_code}' "$BASE$p" --data-urlencode "token=$DTOKEN" ${q:+--data-raw "$q"} || true)"
    else
        [ -n "$b" ] || b='{}'
        out="$(curl -s -X "$m" -w '\n%{http_code}' -H 'Content-Type: application/json' --data-raw "$b" "$BASE$p?token=$DTOKEN${q:+&$q}" || true)"
    fi
    BODY="${out%$'\n'*}"; CODE="${out##*$'\n'}"
}
# agent <METHOD> <path> [json body] [query]: the agent's passport on its cell socket.
agent() {
    local m="$1" p="$2" b="${3:-}" q="${4:-}"
    local out
    if [ "$m" = "GET" ]; then
        out="$(curl -s -G -w '\n%{http_code}' --unix-socket "$SOCK" -H "Authorization: Bearer $PASSPORT" "http://localhost$p" ${q:+--data-raw "$q"} || true)"
    else
        [ -n "$b" ] || b='{}'
        out="$(curl -s -X "$m" -w '\n%{http_code}' --unix-socket "$SOCK" -H "Authorization: Bearer $PASSPORT" -H 'Content-Type: application/json' --data-raw "$b" "http://localhost$p${q:+?$q}" || true)"
    fi
    BODY="${out%$'\n'*}"; CODE="${out##*$'\n'}"
}
want() { [ "$CODE" = "$1" ] || fail "$2: HTTP $CODE (want $1): $BODY"; }
code_is() { jcheck "$BODY" "j['error']['code'] == '$1'" || fail "$2: error code is not $1: $BODY"; }

DAEMON_BIN=""
start_daemon() { # $1 = log name; rest = extra env
    local log="$SB/$1"; shift
    TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
        HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
        K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny "$@" \
        "$DAEMON_BIN" >"$log" 2>&1 &
    DAEMON_PID=$!
    DLOG="$log"
    local phase=""
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
    # The docs store opens off the boot path; wait for it.
    for _ in $(seq 1 100); do
        owner GET /cli/docs/status
        jcheck "$BODY" 'j.get("pool", {}).get("running")' && return 0
        sleep 0.1
    done
    fail "docs runtime never started: $BODY"
}

echo "== daemon =="
if [ "${1:-}" = "--build" ]; then
    (cd "$PROJECT_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build -p k2-daemon) || fail "cargo build -p k2-daemon"
fi
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
# The pinned chat's harness is a shim that records its env (the passport).
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"

start_daemon daemon1.log
ok "sandbox daemon ready ($DAEMON_BIN)"
[ -f "$SB/.k2/docs.redb" ] || fail "no ~/.k2/docs.redb in the sandbox home"
ok "docs store at ~/.k2/docs.redb"
jok "$BODY" 'j["pool"]["budgetBytes"] == 256 * 1024 * 1024 and j["pool"]["diskSync"] is True' "pool: 256 MiB budget, disk sync on"
features="$(curl -s "$BASE/boot-status?token=$DTOKEN")"
jok "$features" '"docs-md-v1" in j.get("features", [])' "docs-md-v1 advertised"

echo "== workspaces + agent passport =="
A="$WORK/alpha"; B="$WORK/beta"
mkdir -p "$A/notes" "$A/.k2/vault" "$A/.k2/wiki" "$B"
printf '# Plan\n\none\ntwo\nthree\n' >"$A/notes/plan.md"
printf '# Wiki\n' >"$A/.k2/wiki/Home.md"
printf 'secret\n' >"$A/.k2/vault/keys.md"
printf 'not markdown\n' >"$A/notes/a.txt"
printf '# B home\n' >"$B/home.md"
# add-from-path registers git checkouts only; no user/system git config.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
for ws in "$A" "$B"; do
    git -c init.defaultBranch=main -C "$ws" init -q || fail "git init $ws"
    reg="$(curl -s -X POST -H 'Content-Type: application/json' \
        -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$ws")" \
        "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
    case "$reg" in *'"error"'*) fail "register $ws: $reg" ;; *"$ws"*) ;; *) fail "register $ws: $reg" ;; esac
done
ok "workspaces alpha + beta registered"
# beta's pinned chat (the shim records its env): its passport and cell
# socket. Again after a daemon restart (a new session, a new socket).
get_passport() {
    rm -f "$SB/claude.env"
    local resp
    resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" \
        -H 'Content-Type: application/json' --data-raw "$(json project "$B")" || true)"
    case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
    for _ in $(seq 1 100); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
    PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
    SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
    [ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
    [ -S "$SOCK" ] || fail "the pinned chat's cell socket is missing: '$SOCK'"
}
get_passport
ok "beta's agent passport + cell socket"

# file_is <path> <python string literal>: wait up to 8 s for the file to hold it.
file_is() {
    for _ in $(seq 1 80); do
        python3 -c 'import sys; sys.exit(0 if open(sys.argv[1], encoding="utf-8").read() == eval(sys.argv[2]) else 1)' "$1" "$2" 2>/dev/null && return 0
        sleep 0.1
    done
    fail "$3: $1 holds $(python3 -c 'import sys; print(repr(open(sys.argv[1]).read()))' "$1" 2>&1), want $2"
}
# doc_is <workspace> <rel> <python string literal>: wait up to 8 s for the doc to hold it.
doc_is() {
    for _ in $(seq 1 80); do
        owner GET /cli/docs/read "" "workspace=$1&path=$2"
        jcheck "$BODY" "j.get('text') == $3" && return 0
        sleep 0.1
    done
    fail "$4: the doc holds $BODY, want $3"
}

echo "== owner: read, deny list, jail =="
owner GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
want 200 "owner read"
jok "$BODY" 'j["text"] == "# Plan\n\none\ntwo\nthree\n" and j["diskMatches"] and j["rights"] == {"read": True, "edit": True, "history": True}' "owner reads the file as imported"
V1="$(jq_ "$BODY" 'j["version"]')"
owner GET /cli/docs/read "" "path=$A/notes/plan.md"
want 200 "owner read by absolute path"
jok "$BODY" 'j["doc"]["path"] == "notes/plan.md"' "an absolute path normalises to (workspace, rel)"
owner GET /cli/docs/read "" "workspace=$A&path=.k2/vault/keys.md"; want 422 "vault"; code_is protected "vault"; ok "vault files are never docs"
owner GET /cli/docs/read "" "workspace=$A&path=notes/a.txt"; want 422 "txt"; code_is not_markdown "txt"; ok "non-Markdown is declined"
owner GET /cli/docs/read "" "workspace=$A&path=../beta/home.md"; want 400 "escape"; ok "the jail refuses .."
owner GET /cli/docs/read "" "path=notes/plan.md"; want 400 "no workspace"; code_is workspace_required "no workspace"; ok "a person names the workspace"
owner GET /cli/docs/edit "" "workspace=$A&path=notes/plan.md"; want 405 "GET on edit"; ok "GET on a POST route is 405"

echo "== owner: edit, write --base, undo, who, log; disk sync both ways =="
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":"notes/plan.md","edits":[{"lines":"3,3","text":"ONE"}]}))')" "workspace=$A"
want 200 "owner edit"
jok "$BODY" 'j["result"]["linesAdded"] == 1 and j["result"]["linesRemoved"] == 1' "owner replaces a line"
file_is "$A/notes/plan.md" '"# Plan\n\nONE\ntwo\nthree\n"' "write-back"
ok "the edit reaches the file (write-back)"
owner POST /cli/docs/write "$(python3 -c 'import json,sys; print(json.dumps({"path":"notes/plan.md","base":int(sys.argv[1]),"text":"# Plan\n\none\ntwo\nthree\nfour\n"}))' "$V1")" "workspace=$A"
want 200 "write --base V1"
owner GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
jok "$BODY" 'j["text"] == "# Plan\n\nONE\ntwo\nthree\nfour\n"' "write --base merges three-way (keeps ONE, adds four)"
owner POST /cli/docs/flush '{"path":"notes/plan.md"}' "workspace=$A"
want 200 "flush"
jok "$BODY" 'j["disk"]["state"] == "clean" and len(j["hash"]) == 64' "flush: written now, disk clean"
file_is "$A/notes/plan.md" '"# Plan\n\nONE\ntwo\nthree\nfour\n"' "flush"
owner POST /cli/docs/undo '{"path":"notes/plan.md","steps":1}' "workspace=$A"
want 200 "undo"
owner GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
jok "$BODY" 'j["text"] == "# Plan\n\nONE\ntwo\nthree\n"' "undo reverts the owner's last edit only"
file_is "$A/notes/plan.md" '"# Plan\n\nONE\ntwo\nthree\n"' "undo write-back"
# An outside program edits the file (FsChanged → file_changed()).
printf '# Plan\n\nONE\ntwo\nthree\nfrom vim\n' >"$A/notes/plan.md"
doc_is "$A" notes/plan.md '"# Plan\n\nONE\ntwo\nthree\nfrom vim\n"' "outside edit"
ok "an outside edit merges into the doc"
owner GET /cli/docs/who "" "workspace=$A&path=notes/plan.md"
owner POST /cli/docs/cursor '{"path":"notes/plan.md","at":"3:1","to":"3:4"}' "workspace=$A"; want 200 "cursor"
owner GET /cli/docs/who "" "workspace=$A&path=notes/plan.md"
want 200 "who"
jok "$BODY" 'len(j["roster"]) == 1 and j["roster"][0]["pid"] == "person:owner" and j["roster"][0]["kind"] == "person" and j["roster"][0]["cursor"]["head"] == "3:4"' "who: the owner, with a caret"
owner GET /cli/docs/log "" "workspace=$A&path=notes/plan.md"
want 200 "log"
jok "$BODY" 'any(x["type"] == "version" and x["reason"] == "import" for x in j["log"]) and any(x["type"] == "change" and x["kind"] == "external" and x["author"] == "kit:external" for x in j["log"]) and any(x["type"] == "change" and x["kind"] == "undo" for x in j["log"]) and all("text" not in x and "content" not in x for x in j["log"])' "log: changes (edit, undo, external) and versions, metadata only"
owner GET /cli/docs/status "" "workspace=$A&path=notes/plan.md"
want 200 "status"
jok "$BODY" 'j["participants"] >= 1 and j["disk"]["state"] in ("clean", "dirty")' "status carries the disk state"

echo "== .k2/wiki: K2's own watch =="
owner GET /cli/docs/read "" "workspace=$A&path=.k2/wiki/Home.md"
want 200 "wiki read"
printf '# Wiki\n\nedited outside\n' >"$A/.k2/wiki/Home.md"
doc_is "$A" .k2/wiki/Home.md '"# Wiki\n\nedited outside\n"' "wiki outside edit"
ok "an outside edit under .k2/ merges too"

echo "== restart (docs written back and released on stop, reconciled on open) =="
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":".k2/wiki/Home.md","edits":[{"after":3,"text":"last words"}]}))')" "workspace=$A"
want 200 "edit just before stop"
stop_daemon
file_is "$A/.k2/wiki/Home.md" '"# Wiki\n\nedited outside\nlast words\n"' "release flushes"
ok "a normal stop writes back what was unwritten"
printf '# Wiki\n\nedited outside\nlast words\nwhile K2 was down\n' >"$A/.k2/wiki/Home.md"
start_daemon daemon2.log
get_passport
doc_is "$A" .k2/wiki/Home.md '"# Wiki\n\nedited outside\nlast words\nwhile K2 was down\n"' "reconcile"
ok "the store reopens and reconciles edits made while K2 was down"

echo "== agent: own workspace =="
agent GET /cli/docs/read "" "path=home.md"
want 200 "agent reads its own workspace"
jok "$BODY" 'j["text"] == "# B home\n" and j["rights"]["history"]' "agent: own workspace, every right"
agent POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":"home.md","edits":[{"after":1,"text":"agent line"}]}))')"
want 200 "agent edits its own doc"
agent GET /cli/docs/who "" "path=home.md"
jok "$BODY" 'j["roster"][0]["kind"] == "agent" and j["roster"][0]["pid"].startswith("agent:")' "agent: present as an agent, stamped by K2"

echo "== agent: another workspace (MI25) =="
agent GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
want 403 "not connected"; code_is not_connected "not connected"; ok "not connected: 403 not_connected"
conn="$(curl -s -X POST "$BASE/cli/connections?token=$DTOKEN&action=add&project=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$B")&target=alpha")"
case "$conn" in *rror*) fail "connections add: $conn" ;; esac
agent GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
want 403 "connected only"; code_is doc_not_shared "connected only"
jok "$BODY" 'j["error"]["path"] == "notes/plan.md" and j["error"]["workspace"]' "connected but not shared: 403 doc_not_shared names the workspace and path"
owner POST /cli/docs/share "$(json workspace "$A" path notes with "$B")"
want 200 "share folder read-only"
jok "$BODY" 'j["share"]["folder"] and not j["share"]["write"] and not j["share"]["history"]' "share: a folder, read-only by default"
agent GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
want 200 "shared read"
jok "$BODY" 'j["rights"] == {"read": True, "edit": False, "history": False}' "shared read-only: read, no edit, no history"
agent POST /cli/docs/edit "$(python3 -c 'import json,sys; print(json.dumps({"workspace":sys.argv[1],"path":"notes/plan.md","edits":[{"after":0,"text":"x"}]}))' "$A")"
want 403 "read-only edit"; code_is doc_forbidden "read-only edit"; ok "read-only share refuses edits"
agent GET /cli/docs/log "" "workspace=$A&path=notes/plan.md"
want 403 "no history"; code_is history_forbidden "no history"; ok "history needs its own grant"
agent POST /cli/docs/share "$(json workspace "$A" path notes with "$B")"
want 403 "agent shares"; code_is person_only "agent shares"; ok "an agent can't share"
owner POST /cli/docs/share "$(python3 -c 'import json,sys; print(json.dumps({"workspace":sys.argv[1],"path":"notes/plan.md","with":sys.argv[2],"write":True,"history":True}))' "$A" "$B")"
want 200 "share write+history"
agent GET /cli/docs/log "" "workspace=$A&path=notes/plan.md"; want 200 "history granted"; ok "history granted on the share"

echo "== on-behalf edits and approvals (K-ID-10/11) =="
agent POST /cli/docs/edit "$(python3 -c 'import json,sys; print(json.dumps({"workspace":sys.argv[1],"path":"notes/plan.md","onBehalfOf":"owner","edits":[{"after":0,"text":"> for the owner"}]}))' "$A")"
want 200 "on-behalf edit"
jok "$BODY" 'j["onBehalfOf"] == "person:owner" and j["approval"]["state"] == "pending"' "an on-behalf edit applies at once and waits for approval"
CH="$(jq_ "$BODY" 'j["result"]["version"]')"
owner GET /cli/docs/approvals "" "workspace=$A&path=notes/plan.md&pending=1&mine=1"
want 200 "approvals"
jok "$BODY" "any(a['change'] == $CH for a in j['approvals'])" "the owner sees it pending (log --pending --on-behalf-of me)"
agent POST /cli/docs/approve "$(python3 -c 'import json,sys; print(json.dumps({"workspace":sys.argv[1],"path":"notes/plan.md","change":int(sys.argv[2]),"state":"approved"}))' "$A" "$CH")"
want 403 "agent approves"; code_is not_the_approver "agent approves"; ok "the agent can't approve its own edit"
owner POST /cli/docs/approve "$(python3 -c 'import json,sys; print(json.dumps({"workspace":sys.argv[1],"path":"notes/plan.md","change":int(sys.argv[2]),"state":"approved"}))' "$A" "$CH")"
want 200 "owner approves"
jok "$BODY" 'j["approval"]["state"] == "approved" and j["approval"]["by"] == "person:owner"' "the named person approves"
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":"notes/plan.md","onBehalfOf":"owner","edits":[{"after":0,"text":"x"}]}))')" "workspace=$A"
want 400 "person on behalf"; ok "only agents act on someone's behalf"

echo "== unshare =="
owner POST /cli/docs/unshare "$(json workspace "$A" path notes/plan.md with "$B")"; want 200 "unshare file"
owner POST /cli/docs/unshare "$(json workspace "$A" path notes with "$B")"; want 200 "unshare folder"
owner GET /cli/docs/shares "" "workspace=$A"
jok "$BODY" 'j["shares"] == []' "no shares left"
agent GET /cli/docs/read "" "workspace=$A&path=notes/plan.md"
want 403 "after unshare"; code_is doc_not_shared "after unshare"; ok "unshared: refused again"

echo "== audit (metadata only) =="
audit="$SB/.k2/auth-audit.jsonl"
[ -f "$audit" ] || audit="$(ls "$SB"/.k2/*audit*.jsonl 2>/dev/null | head -1)"
[ -n "$audit" ] && [ -f "$audit" ] || fail "no access audit file under $SB/.k2"
grep -q '"doc-share"' "$audit" || fail "no doc-share line in $audit"
grep -q '"doc-approval"' "$audit" || fail "no doc-approval line in $audit"
if grep -q 'for the owner\|agent line\|# Plan' "$audit"; then fail "doc content leaked into the audit: $audit"; fi
ok "share, unshare and approval are audited, with no doc content"

echo "== k2 doc (the CLI, K3) =="
K2_CLI="$PROJECT_ROOT/cli/k2"
# cli <owner|agent> <cwd> <args…> → OUT, ERR, RC. Never a TTY on stdin
# (stdin: $CLI_IN when set).
CLI_IN=""
cli() {
    local who="$1" cwd="$2"; shift 2
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT")
    if [ "$who" = "agent" ]; then
        envs+=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$SOCK" K2_PROJECT_PATH="$B")
    fi
    set +e
    OUT="$(cd "$cwd" && printf '%s' "$CLI_IN" | env "${envs[@]}" "$K2_CLI" "$@" 2>"$WORK/cli.err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
    CLI_IN=""
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }

set +e
help_out="$(env HOME="$SB" "$K2_CLI" doc --help 2>&1)"; help_rc=$?
set -e
[ "$help_rc" = 0 ] || fail "k2 doc --help exit $help_rc: $help_out"
has "$help_out" "k2 doc <verb> [flags] <file.md>" "help"
ok "k2 doc --help needs no daemon"
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
[ "$(_cli_tool_id_for_verb doc)" = "docs" ] || fail "doc -> tool id docs"
[ "$(_cli_tool_default_mode docs)" = "id" ] || fail "docs mode id"
_cli_tool_is_locked docs || fail "docs locked"
ok "doc is an id + locked tool (the passport inside K2, never the owner token)"

cli owner "$WORK" doc flush -w "$A" notes/plan.md
want_rc 0 "owner flush"
has "$OUT" "disk clean" "flush leaves the disk clean"
cli owner "$WORK" doc cat -w "$A" notes/plan.md
want_rc 0 "owner cat"
[ "$OUT" = "$(cat "$A/notes/plan.md")" ] || fail "owner cat: '$OUT' is not the file: '$(cat "$A/notes/plan.md")'"
ok "owner: k2 doc cat -w <ws> <rel> prints the doc"
cli owner "$A/notes" doc grep -n ONE plan.md
want_rc 0 "owner grep (relative to cwd)"
has "$OUT" ":ONE" "owner grep"
lacks_fence="$ERR"
[[ "$lacks_fence" != *"BEGIN"* ]] || fail "no fence outside a K2 terminal by default: $ERR"
ok "owner: a path relative to where you stand; no fence outside K2"

cli agent "$B" doc grep -n home home.md
want_rc 0 "agent grep"
has "$OUT" "home" "agent grep"
has "$ERR" "k2 doc" "agent fence"
has "$ERR" "BEGIN home.md" "agent fence"
ok "agent: read output is fenced on stderr inside a K2 terminal"
cli agent "$B" doc grep -q nothing-like-this home.md
want_rc 1 "grep no match"
ok "grep's 'no' is exit 1"
cli agent "$B" doc sed -i 's/# B home/# B HOME/' home.md
want_rc 0 "agent sed -i"
has "$ERR" "as " "edit summary names the editor"
for _ in $(seq 1 80); do grep -q '# B HOME' "$B/home.md" && break; sleep 0.1; done
grep -q '# B HOME' "$B/home.md" || fail "sed -i never reached the file: $(cat "$B/home.md")"
ok "agent: sed -i edits live and reaches the file"
cli agent "$B" doc sed -i '1w /tmp/k2-doc-escape' home.md
want_rc 2 "sed w refused"
[ ! -e /tmp/k2-doc-escape ] || fail "sed w wrote a file"
ok "sed's w command is refused, nothing applied (exit 2)"
CLI_IN=$'appended by the CLI\n'
cli agent "$B" doc append home.md
want_rc 0 "append from stdin"
cli agent "$B" doc tail -n 1 home.md
want_rc 0 "tail"
[ "$OUT" = "appended by the CLI" ] || fail "tail -n 1: '$OUT'"
ok "append from stdin, tail -n 1"
cli agent "$B" doc undo home.md
want_rc 0 "undo"
cli agent "$B" doc who home.md
want_rc 0 "who"
has "$OUT" "(agent)" "who lists the agent"
ok "undo, who"
echo "== k2 doc history (kit C7a) =="
cli agent "$B" doc append --json --text $'revert me\n' home.md
want_rc 0 "append to revert"
CH="$(jq_ "$OUT" 'j["version"]')"
cli agent "$B" doc revert "$CH" home.md
want_rc 0 "revert"
cli agent "$B" doc grep -c "revert me" home.md
want_rc 1 "reverted text is gone"
ok "revert <change> reverses one change from the log"
cli agent "$B" doc log home.md
want_rc 0 "log"
has "$OUT" "revert" "log lists the revert"
ok "log lists changes with their kind"
cli agent "$B" doc versions --json home.md
want_rc 0 "versions"
V0="$(jq_ "$OUT" 'j["versions"][0]["id"]')"
cli agent "$B" doc show --version "$V0" home.md
want_rc 0 "show"
[ "$OUT" = "# B home" ] || fail "show --version $V0: '$OUT'"
ok "show --version: the imported text"
cli agent "$B" doc diff --version "$V0" home.md
want_rc 1 "diff --version differs"
has "$OUT" "+" "diff text"
ok "diff --version: unified diff, exit 1 when different"
cli agent "$B" doc restore --version "$V0" home.md
want_rc 0 "restore"
cli agent "$B" doc cat home.md
[ "$OUT" = "# B home" ] || fail "after restore: '$OUT'"
cli agent "$B" doc undo home.md
want_rc 0 "undo the restore"
cli agent "$B" doc cat home.md
[ "$OUT" != "# B home" ] || fail "undo of the restore changed nothing"
ok "restore is a new change, and undo reverses it"
cli agent "$B" doc revert 999999 home.md
want_rc 4 "no such change"
ok "revert of an unknown change: exit 4"

cli agent "$B" doc cat -w "$A" notes/plan.md
want_rc 3 "cross-workspace refused"
has "$ERR" "doc_not_shared" "cross-workspace refused"
ok "another workspace without a share: exit 3 doc_not_shared"
cli agent "$B" doc cat no-such.md
want_rc 4 "missing file"
ok "no such file: exit 4"
cli owner "$WORK" doc status
want_rc 0 "pool status"
has "$OUT" "disk sync on" "pool status"
ok "k2 doc status (no path): the pool"

echo "PASS ($pass checks)"
