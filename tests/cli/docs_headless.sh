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

# Tier 0's size cap, lowered so the cap check stays small (the default is
# 32 MiB); 4 MiB still moves a 3 MiB file, past any argv limit.
T0_CAP=4
export K2_DOCS_BLOB_MAX_MIB=$T0_CAP
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
# socket. Again after a daemon restart: a fresh spawn rewrites claude.env;
# a session kept alive across the restart (k2-ptyd) keeps its passport, and
# the new daemon serves its cell socket again.
get_passport() {
    local resp
    resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" \
        -H 'Content-Type: application/json' --data-raw "$(json project "$B")" || true)"
    case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
    for _ in $(seq 1 400); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
    [ -s "$SB/claude.env" ] || fail "the pinned chat's harness never started (no claude.env after 20 s): $resp"
    PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
    SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
    [ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
    for _ in $(seq 1 200); do [ -S "$SOCK" ] && break; sleep 0.05; done
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
owner GET /cli/docs/status "" "workspace=$A&path=notes/a.txt"; want 200 "txt"
jok "$BODY" 'j["tier"] == 0 and j["current"]["reason"] == "import"' "any other file is a tier-0 doc"
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
cli agent "$B" doc blame home.md
want_rc 0 "blame"
has "$OUT" "(agent)" "blame names the agent's lines"
cli agent "$B" doc blame --json -L 1,1 home.md
want_rc 0 "blame -L"
jok "$OUT" 'len(j["lines"]) == 1 and j["lines"][0]["line"] == 1 and j["lines"][0]["author"]' "blame -L 1,1: one line with its author"
cli agent "$B" doc revert 999999 home.md
want_rc 4 "no such change"
ok "revert of an unknown change: exit 4"

echo "== k2 doc: the pack's own verb, and mv =="
printf '# T\n\n## A\nold a\n\n## B\nb\n' >"$B/sec.md"
CLI_IN=$'new a\n'
cli agent "$B" doc replace-section --heading "## A" sec.md
want_rc 0 "replace-section"
for _ in $(seq 1 80); do grep -q 'new a' "$B/sec.md" && break; sleep 0.1; done
grep -q 'new a' "$B/sec.md" && ! grep -q 'old a' "$B/sec.md" || fail "replace-section never reached the file: $(cat "$B/sec.md")"
ok "replace-section (the Markdown pack's verb) is one atomic change"
CLI_IN=$'x\n'
cli agent "$B" doc replace-section --heading "## Nope" sec.md
want_rc 1 "replace-section not found"
ok "replace-section of a missing heading: exit 1, nothing applied"
cli agent "$B" doc mv sec.md moved.md
want_rc 0 "mv"
[ -f "$B/moved.md" ] && [ ! -e "$B/sec.md" ] || fail "mv: files are $(ls "$B")"
cli agent "$B" doc log moved.md
want_rc 0 "log after mv"
has "$OUT" "edit" "the moved doc keeps its history"
ok "mv renames the file; the doc keeps its history"
cli agent "$B" doc mv moved.md ../escape.md
want_rc 2 "mv out of the workspace"
ok "mv can't leave the workspace"

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

echo "== tier 0: any file (presence, a lease, versions, restore) =="
# mk <file> <seed> <n>: a PNG header, a NUL, then n seeded random bytes.
mk() { python3 -c 'import random,sys; r=random.Random(int(sys.argv[2])); open(sys.argv[1],"wb").write(b"\x89PNG\r\n\x1a\n\x00" + r.randbytes(int(sys.argv[3])))' "$@"; }
# put <src> <dst>: an app's save (written aside, renamed over).
put() { cp "$1" "$(dirname "$2")/.k2-test-save.tmp" && mv -f "$(dirname "$2")/.k2-test-save.tmp" "$2"; }
# same_bytes <file> <expected> <what>: wait up to 8 s for the bytes.
same_bytes() {
    for _ in $(seq 1 80); do cmp -s "$1" "$2" && return 0; sleep 0.1; done
    fail "$3: $1 does not hold the bytes of $2"
}
# t0_wait <ws> <rel> <python condition over j> <what>: poll the owner's status.
t0_wait() {
    for _ in $(seq 1 80); do
        owner GET /cli/docs/status "" "workspace=$1&path=$2"
        [ "$CODE" = 200 ] && jcheck "$BODY" "$3" && return 0
        sleep 0.1
    done
    fail "$4: $3 never held: $BODY"
}
# wbody <ws> <rel> <base> <file> [onBehalfOf]: a write's JSON body.
wbody() {
    python3 -c '
import base64, json, sys
d = {"workspace": sys.argv[1], "path": sys.argv[2], "base": sys.argv[3], "bytesB64": base64.b64encode(open(sys.argv[4], "rb").read()).decode()}
if len(sys.argv) > 5:
    d["onBehalfOf"] = sys.argv[5]
print(json.dumps(d))' "$@"
}
mkdir -p "$B/img" "$B/.k2/wiki/assets" "$B/.git/hooks"
mk "$B/img/logo.png" 1 4096
cp "$B/img/logo.png" "$WORK/logo.v0"
mk "$WORK/other.bin" 2 1000
mk "$WORK/logo.big" 3 $((3 * 1024 * 1024))
mk "$WORK/outside.png" 4 2048
mk "$WORK/app-save.png" 5 2048

owner GET /cli/docs/status "" "workspace=$B&path=img/logo.png"
want 200 "tier-0 status"
jok "$BODY" 'j["tier"] == 0 and j["lease"] is None and j["current"]["reason"] == "import" and j["current"]["size"] == 4105 and j["owner"] is True and j["conflictCopies"] == [] and j["disk"]["state"] == "clean"' "owner: a PNG is a tier-0 doc (imported, no lease, disk clean)"
V0="$(jq_ "$BODY" 'j["version"]')"
[ -d "$SB/.k2/docs-blobs" ] || fail "no ~/.k2/docs-blobs in the sandbox home"
mode="$(python3 -c 'import os,stat,sys; print(oct(stat.S_IMODE(os.stat(sys.argv[1]).st_mode)))' "$SB/.k2/docs-blobs")"
[ "$mode" = "0o700" ] || fail "~/.k2/docs-blobs is $mode, want 0o700"
ok "blobs live in ~/.k2/docs-blobs (owner-only)"
owner GET /cli/docs/read "" "workspace=$B&path=img/logo.png"
want 200 "tier-0 read"
python3 -c 'import base64,json,sys; j=json.loads(sys.argv[1]); sys.exit(0 if base64.b64decode(j["bytesB64"]) == open(sys.argv[2],"rb").read() and j["size"] == 4105 else 1)' "$BODY" "$B/img/logo.png" \
    || fail "read: the bytes are not the file's"
ok "read: the current bytes, exact (base64)"
owner POST /cli/docs/tool '{"path":"img/logo.png","argv":["grep","x"]}' "workspace=$B"
want 400 "a text tool on tier 0"; code_is not_a_text_doc "a text tool on tier 0"
jok "$BODY" 'j["error"]["exit"] == 2 and "lease" in j["error"]["hint"] and j["error"]["verb"] == "grep"' "a text verb on any other file: not_a_text_doc (exit 2), naming what it has"
cli agent "$B" doc sed -i 's/a/b/' img/logo.png
want_rc 2 "sed -i on tier 0"; has "$ERR" "not_a_text_doc" "sed -i on tier 0"
cli agent "$B" doc undo img/logo.png
want_rc 2 "undo on tier 0"
cli agent "$B" doc diff --version "$V0" img/logo.png
want_rc 2 "diff --version on tier 0"
cli agent "$B" doc claim --lines 1,2 img/logo.png
want_rc 2 "claim on tier 0"
ok "agent: text verbs (and claims) exit 2 on a tier-0 doc"
owner POST /cli/docs/tool '{"path":"img/logo.png","argv":["cat"]}' "workspace=$B"
want 200 "cat on tier 0"
python3 -c 'import base64,json,sys; j=json.loads(sys.argv[1]); sys.exit(0 if j["exit"] == 0 and base64.b64decode(j["stdoutB64"]) == open(sys.argv[2],"rb").read() else 1)' "$BODY" "$B/img/logo.png" \
    || fail "cat on tier 0: not the file's bytes: $BODY"
owner POST /cli/docs/tool '{"path":"img/logo.png","argv":["cat","-n"]}' "workspace=$B"
jok "$BODY" 'j["exit"] == 2' "cat on any file takes no flags (exit 2)"
ok "cat on any file: its exact bytes"

cli agent "$B" doc lease --ttl 10m img/logo.png
want_rc 0 "agent lease"; has "$OUT" "leased img/logo.png" "agent lease"
owner GET /cli/docs/status "" "workspace=$B&path=img/logo.png"
jok "$BODY" 'j["lease"]["pid"].startswith("agent:") and not j["lease"]["mine"] and j["lease"]["expiresMs"] > 0' "status shows the agent's lease"
cli agent "$B" doc who img/logo.png
want_rc 0 "who"; has "$OUT" "holds the lease" "who shows the holder"
ok "agent: k2 doc lease (status and who show the holder)"
owner POST /cli/docs/write "$(wbody "$B" img/logo.png "$V0" "$WORK/other.bin")"
want 409 "write under the agent's lease"; code_is leased "write under a lease"
jok "$BODY" 'j["error"]["exit"] == 5 and j["error"]["holder"].startswith("agent:") and j["error"]["retryAfterMs"] > 0' "someone else's lease: 409 leased, exit 5, with holder and expiry"
cli owner "$WORK" doc write --base "$V0" --file "$WORK/other.bin" -w "$B" img/logo.png
want_rc 5 "k2 doc write under someone else's lease"; has "$ERR" "leased by" "exit 5 names the holder"
cmp -s "$B/img/logo.png" "$WORK/logo.v0" || fail "a refused write changed the file"
ok "owner: a write under the agent's lease exits 5; nothing is written"
cli agent "$B" doc write --base "$V0" --file "$WORK/logo.big" img/logo.png
want_rc 0 "agent write --base (3 MiB)"
same_bytes "$B/img/logo.png" "$WORK/logo.big" "write-back of a 3 MiB write"
owner GET /cli/docs/status "" "workspace=$B&path=img/logo.png"
V1="$(jq_ "$BODY" 'j["version"]')"
jok "$BODY" "j['version'] != $V0 and j['current']['reason'] == 'write' and j['current']['author'].startswith('agent:') and j['current']['size'] == 3 * 1024 * 1024 + 9" "the write is a new version by the agent"
ok "agent: write --base V of a 3 MiB file (through temp files), written back"
cli agent "$B" doc write --base "$V0" --file "$WORK/other.bin" img/logo.png
want_rc 1 "stale base"; has "$ERR" "stale" "stale base"
cli agent "$B" doc write --base live --file "$WORK/other.bin" img/logo.png
want_rc 2 "--base live"
ok "a stale base exits 1 and --base live exits 2: never last-writer-wins"
cli agent "$B" doc lease --renew img/logo.png; want_rc 0 "renew"
cli agent "$B" doc unlock img/logo.png; want_rc 0 "unlock"; has "$OUT" "released" "unlock"
cli agent "$B" doc lease --renew img/logo.png; want_rc 2 "renew with no lease"; has "$ERR" "no_lease" "renew with no lease"
ok "renew, unlock; renewing with no lease exits 2"
cli agent "$B" doc lock img/logo.png; want_rc 0 "lock"
cli agent "$B" doc lease --break img/logo.png; want_rc 3 "an agent breaks"; has "$ERR" "not_the_owner" "an agent breaks"
cli owner "$WORK" doc lease --break -w "$B" img/logo.png; want_rc 0 "owner breaks"; has "$OUT" "broke" "owner breaks"
owner GET /cli/docs/status "" "workspace=$B&path=img/logo.png"
jok "$BODY" 'j["lease"] is None' "the broken lease is gone"
ok "only the owner breaks someone's lease"

cli agent "$B" doc versions --json img/logo.png
want_rc 0 "versions"
jok "$OUT" '[v["reason"] for v in j["versions"]] == ["import", "write"] and j["versions"][-1]["current"] and all("blob" not in v for v in j["versions"])' "versions: import, then the write; metadata only"
cli agent "$B" doc show --version "$V0" --out "$WORK/shown.png" img/logo.png
want_rc 0 "show --out"
cmp -s "$WORK/shown.png" "$WORK/logo.v0" || fail "show --version $V0 --out: not the imported bytes"
ok "show --version V --out FILE: that version's exact bytes"
cli agent "$B" doc restore --version "$V0" img/logo.png
want_rc 0 "restore"
same_bytes "$B/img/logo.png" "$WORK/logo.v0" "restore write-back"
owner GET /cli/docs/status "" "workspace=$B&path=img/logo.png"
jok "$BODY" "j['current']['reason'] == 'restore' and j['current']['restoredFrom'] == $V0 and j['current']['author'].startswith('agent:')" "restore is a new version, attributed, pointing at V0"
ok "restore --version V: forward, attributed, written back"
cli agent "$B" doc version --name baseline img/logo.png; want_rc 0 "version --name"
cli agent "$B" doc versions --json img/logo.png
jok "$OUT" 'any(v["name"] == "baseline" and v["current"] for v in j["versions"]) and len(j["versions"]) == 3' "the current version is named"
cli agent "$B" doc log img/logo.png; want_rc 0 "log"
has "$OUT" "restore" "log lists the restore"; has "$OUT" " by " "log names who"
cli agent "$B" doc blame img/logo.png; want_rc 0 "blame"
has "$OUT" "set by" "blame"; has "$OUT" "3 versions" "blame counts the history"
cli agent "$B" doc blame -L 1,2 img/logo.png; want_rc 2 "blame -L on tier 0"
ok "version --name, log and blame on any file"
printf 'plain text\n' >"$B/notes.txt"
cli agent "$B" doc status --json notes.txt; want_rc 0 "text file status"
TV0="$(jq_ "$OUT" 'j["version"]')"
jok "$OUT" 'j["tier"] == 0' "a .txt file is tier 0"
cli agent "$B" doc show --version "$TV0" notes.txt; want_rc 0 "show to stdout"
[ "$OUT" = "plain text" ] || fail "show --version to stdout: '$OUT'"
ok "show --version prints text bytes to a pipe"

put "$WORK/outside.png" "$B/img/logo.png"
t0_wait "$B" img/logo.png 'j["current"]["reason"] == "external" and j["current"]["author"] == "kit:external" and not j["conflictCopies"]' "an outside save"
ok "an outside save without a lease is a new version by kit:external (no conflict copy)"
cli agent "$B" doc lease img/logo.png; want_rc 0 "lease before the app's save"
put "$WORK/app-save.png" "$B/img/logo.png"
t0_wait "$B" img/logo.png 'len(j["conflictCopies"]) == 1 and j["current"]["reason"] == "external" and j["current"]["conflictCopy"]' "a save under the lease"
cmp -s "$B/img/logo.png" "$WORK/app-save.png" || fail "the app's save didn't stay current"
copy="$B/$(jq_ "$BODY" 'j["conflictCopies"][0]["path"]')"
case "$copy" in *"/.k2/conflicts/logo (conflict "*").png") ;; *) fail "conflict copy name: $copy" ;; esac
[ "$(cat "$B/.k2/conflicts/.gitignore")" = "*" ] || fail "no * .gitignore in .k2/conflicts"
cmp -s "$copy" "$WORK/outside.png" || fail "the conflict copy doesn't hold the replaced content"
ok "a save under someone's lease keeps both: the save is current, the replaced content a version and a conflict copy"
note=""
for _ in $(seq 1 80); do
    note="$(grep -rl "Conflicting save: img/logo.png" "$B/.k2/inbox" 2>/dev/null | head -1 || true)"
    [ -n "$note" ] && break
    sleep 0.1
done
[ -n "$note" ] || fail "no inbox note for the conflicting save under $B/.k2/inbox"
ok "the conflicting save leaves an inbox note"
cli agent "$B" doc status img/logo.png; want_rc 0 "status lists the copy"; has "$OUT" "conflict copy: .k2/conflicts/logo (conflict" "status lists the copy"
cli agent "$B" doc unlock img/logo.png; want_rc 0 "unlock after the conflict"

python3 -c 'import sys; open(sys.argv[1],"wb").write(b"\0" * (int(sys.argv[2]) * 1048576 + 1))' "$B/big.bin" "$T0_CAP"
owner GET /cli/docs/status "" "workspace=$B&path=big.bin"
want 422 "over the cap"; code_is too_large "over the cap"
jok "$BODY" '"4.0 MiB" in j["error"]["hint"]' "the refusal names the cap"
cli agent "$B" doc status big.bin; want_rc 4 "over the cap (CLI)"
printf '#!/bin/sh\n' >"$B/.git/hooks/pre-commit"
owner GET /cli/docs/status "" "workspace=$B&path=.git/hooks/pre-commit"; want 422 "git hook"; code_is protected "git hook"
printf '{}\n' >"$B/.k2/agent-thing.json"
owner GET /cli/docs/status "" "workspace=$B&path=.k2/agent-thing.json"; want 422 "k2 config"; code_is protected "k2 config"
printf 'x\n' >"$B/deploy.pem"
owner GET /cli/docs/status "" "workspace=$B&path=deploy.pem"; want 422 "pem"; code_is secret "pem"
mk "$B/.k2/wiki/assets/map.png" 6 64
owner GET /cli/docs/status "" "workspace=$B&path=.k2/wiki/assets/map.png"; want 200 "wiki attachment"
ok "declined: over the cap (exit 4), git's folder, K2's .k2 config, secrets; wiki attachments are docs"

cli owner "$WORK" doc lease -w "$B" img/logo.png; want_rc 0 "owner lease before mv"
cli agent "$B" doc mv img/logo.png img/brand.png; want_rc 5 "mv under someone else's lease"
cli owner "$WORK" doc unlock -w "$B" img/logo.png; want_rc 0 "owner unlock"
cli agent "$B" doc mv img/logo.png img/brand.png; want_rc 0 "mv"
[ -f "$B/img/brand.png" ] && [ ! -e "$B/img/logo.png" ] || fail "mv: $(ls "$B/img")"
cli agent "$B" doc versions --json img/brand.png; want_rc 0 "versions after mv"
jok "$OUT" 'len(j["versions"]) >= 5' "the moved doc keeps its versions"
cli agent "$B" doc mv img/brand.png img/brand.md; want_rc 2 "mv to Markdown"; has "$ERR" "tier_change" "mv to Markdown"
ok "mv keeps an any-file doc's history (not under someone else's lease, never into Markdown)"
cli agent "$B" doc status --json img/brand.png; want_rc 0 "status before the on-behalf write"
BV="$(jq_ "$OUT" 'j["version"]')"
agent POST /cli/docs/write "$(wbody "$B" img/brand.png "$BV" "$WORK/other.bin" owner)"
want 200 "on-behalf blob write"
jok "$BODY" 'j["onBehalfOf"] == "person:owner" and j["approval"]["state"] == "pending"' "an agent's write for a person applies at once and waits for their approval"
OBV="$(jq_ "$BODY" 'j["result"]["version"]')"
same_bytes "$B/img/brand.png" "$WORK/other.bin" "the on-behalf write reaches the file"
cli owner "$WORK" doc log --pending --json -w "$B" img/brand.png; want_rc 0 "log --pending"
jok "$OUT" "[x['id'] for x in j['log']] == [$OBV] and j['log'][0]['approval'] == 'pending'" "log --pending lists it"
cli agent "$B" doc approve "$OBV" img/brand.png; want_rc 3 "the agent approves its own"; has "$ERR" "not_the_approver" "the agent approves its own"
cli owner "$WORK" doc decline "$OBV" -w "$B" img/brand.png; want_rc 0 "the person declines"
has "$ERR" "restore --version $BV" "a decline names the version to restore"
cmp -s "$B/img/brand.png" "$WORK/other.bin" || fail "a decline restored by itself"
ok "on-behalf blob writes: pending, only the person settles them, a decline says how to undo it"
cli agent "$B" doc lease home.md; want_rc 2 "lease on Markdown"; has "$ERR" "no_lease_on_text" "lease on Markdown"
ok "Markdown has no lease"
cli agent "$B" doc status --json home.md; want_rc 0 "Markdown status"
jok "$OUT" 'j["tier"] == 1' "a Markdown doc says tier 1"
HV="$(jq_ "$OUT" 'j["version"]')"
CLI_IN="$(cat "$B/home.md")"$'\nrewritten through bytesB64\n'
cli agent "$B" doc write --base "$HV" home.md; want_rc 0 "Markdown write (bytesB64)"
for _ in $(seq 1 80); do grep -q 'rewritten through bytesB64' "$B/home.md" && break; sleep 0.1; done
grep -q 'rewritten through bytesB64' "$B/home.md" || fail "the Markdown write never reached the file: $(cat "$B/home.md")"
ok "k2 doc write on Markdown still merges from --base V (sent as bytes)"

echo "== agent presence: claims, the bounce, free now, --if-version =="
printf '# Shared\n\nalpha\nbeta\ngamma\ndelta\n' >"$B/shared.md"
cli agent "$B" doc cat shared.md; want_rc 0 "agent reads shared.md"
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":"shared.md","edits":[{"lines":"3,3","text":"ALPHA"}]}))')" "workspace=$B"
want 200 "the owner edits after the agent's read"
cli agent "$B" doc sed -i 's/beta/BETA/' shared.md
want_rc 1 "an edit from a stale read"; has "$ERR" "doc changed since" "the stale edit names both versions"
grep -q '^beta$' "$B/shared.md" || fail "the stale edit applied: $(cat "$B/shared.md")"
cli agent "$B" doc cat shared.md; want_rc 0 "agent reads again"
cli agent "$B" doc sed -i 's/beta/BETA/' shared.md; want_rc 0 "the edit after re-reading"
has "$ERR" "checked against v" "the guard is shown"
ok "an agent's edit is checked against the version it last read (exit 1 when the doc changed)"
cli agent "$B" doc replace --old gamma --new GAMMA shared.md; want_rc 0 "replace --old/--new"
printf 'GAMMA' >"$WORK/old.txt"; printf 'Gamma' >"$WORK/new.txt"
cli agent "$B" doc replace --old-file "$WORK/old.txt" --new-file "$WORK/new.txt" shared.md; want_rc 0 "replace --old-file/--new-file"
cli agent "$B" doc replace --old nowhere --new x shared.md; want_rc 1 "replace with no match"
for _ in $(seq 1 80); do grep -q '^Gamma$' "$B/shared.md" && break; sleep 0.1; done
grep -q '^Gamma$' "$B/shared.md" || fail "exact replace never reached the file: $(cat "$B/shared.md")"
ok "replace --old/--new and --old-file/--new-file: exact text; no match is exit 1"
owner POST /cli/docs/claim '{"path":"shared.md","lines":"5,6","note":"reworking the end","ttlMs":60000}' "workspace=$B"
want 200 "the owner claims lines 5-6"
CLAIM="$(jq_ "$BODY" 'j["claim"]["id"]')"
sleep 5.2  # heads-up notes come at most one batch per 5 s per reader
cli agent "$B" doc cat shared.md; want_rc 0 "agent reads under the claim"
has "$ERR" "note:" "a heads-up note names the claim"
cli agent "$B" doc sed -i 's/delta/DELTA/' shared.md
want_rc 5 "an agent's edit into a claim bounces"; has "$ERR" "claimed by" "the bounce names who"; has "$ERR" "in line" "the bounce says it waits"
grep -q '^delta$' "$B/shared.md" || fail "the bounced edit applied: $(cat "$B/shared.md")"
cli agent "$B" doc sed -i 's/ALPHA/Alpha/' shared.md; want_rc 0 "an edit outside the claim applies"
cli agent "$B" doc who shared.md; has "$OUT" "claimed 5-6" "who shows the claim"
owner POST /cli/docs/claim "$(json path shared.md action release id "$CLAIM")" "workspace=$B"
want 200 "the owner releases"
free=""
for _ in $(seq 1 80); do
    owner GET /cli/chatterlog ""
    case "$BODY" in *"are free now"*) free=1; break ;; esac
    sleep 0.1
done
[ -n "$free" ] || fail "no free-now message in Chatter: $BODY"
case "$BODY" in *"waited for a claim"*|*"claimed by"*) ;; *) fail "the bounce never reached Chatter: $BODY" ;; esac
ok "the bounce and the free-now message land in Chatter"
cli agent "$B" doc cat shared.md; want_rc 0 "re-read after free now"
cli agent "$B" doc sed -i 's/delta/DELTA/' shared.md; want_rc 0 "the edit after free now"
cli agent "$B" doc claim --lines 1,2 --note "title" shared.md; want_rc 0 "the agent claims"; has "$OUT" "claimed lines 1-2" "agent claim"
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":"shared.md","edits":[{"lines":"1,1","text":"# Shared doc"}]}))')" "workspace=$B"
want 200 "a person edits inside an agent's claim"
cli agent "$B" doc claim --release shared.md; want_rc 0 "the agent releases"
cli agent "$B" doc claim --release shared.md; want_rc 1 "release with no claim"
cli agent "$B" doc activity shared.md; want_rc 0 "activity"; has "$OUT" "claim" "activity lists claims"
ok "claims: take, release (exit 1 with none), activity; people are never bounced"

mk "$A/notes/diagram.bin" 7 512
cli agent "$B" doc status -w "$A" notes/diagram.bin; want_rc 3 "unshared tier-0"; has "$ERR" "doc_not_shared" "unshared tier-0"
owner POST /cli/docs/share "$(json workspace "$A" path notes/diagram.bin with "$B")"; want 200 "share a tier-0 file"
cli agent "$B" doc status --json -w "$A" notes/diagram.bin; want_rc 0 "shared tier-0"
jok "$OUT" 'j["rights"] == {"read": True, "edit": False, "history": False} and j["conflictCopies"] is None and j["owner"] is False' "a read-only share: read, no edit, no history"
cli agent "$B" doc lease -w "$A" notes/diagram.bin; want_rc 3 "lease on a read-only share"; has "$ERR" "doc_forbidden" "lease read-only"
cli agent "$B" doc versions -w "$A" notes/diagram.bin; want_rc 3 "versions without history"; has "$ERR" "history_forbidden" "versions without history"
owner POST /cli/docs/unshare "$(json workspace "$A" path notes/diagram.bin with "$B")"; want 200 "unshare a tier-0 file"
ok "agent, another workspace: the same shares and rights on any file"

grep -q '"doc-lease-broken"' "$audit" || fail "no doc-lease-broken line in $audit"
grep -q '"doc-conflict"' "$audit" || fail "no doc-conflict line in $audit"
if grep -q 'plain text' "$audit"; then fail "doc content leaked into the audit: $audit"; fi
ok "a broken lease and a conflicting save are audited, metadata only"
cli owner "$WORK" doc status; want_rc 0 "pool"
has "$OUT" "any-file docs:" "pool shows any-file docs"
ok "k2 doc status (pool) shows any-file docs"

echo "== tier 0: a removed workspace's blob key goes with it =="
C="$WORK/gamma"
mkdir -p "$C"
git -c init.defaultBranch=main -C "$C" init -q || fail "git init $C"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$C")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *'"error"'*) fail "register $C: $reg" ;; *"$C"*) ;; *) fail "register $C: $reg" ;; esac
mk "$C/photo.png" 8 256
owner GET /cli/docs/status "" "workspace=$C&path=photo.png"; want 200 "gamma's tier-0 doc"
CID="$(jq_ "$BODY" 'j["doc"]["workspace"]')"
owner POST /cli/projects/delete "$(json id "$CID")"
case "$CODE" in 200) ;; *) fail "remove gamma: HTTP $CODE $BODY" ;; esac
ok "gamma: a tier-0 doc, then the workspace removed"

echo "== restart (docs written back and released on stop, reconciled on open) =="
owner POST /cli/docs/lease '{"path":"notes.txt"}' "workspace=$B"
want 200 "owner lease before the restart"
jok "$BODY" 'j["lease"]["mine"]' "the owner holds notes.txt's lease"
owner POST /cli/docs/edit "$(python3 -c 'import json; print(json.dumps({"path":".k2/wiki/Home.md","edits":[{"after":3,"text":"last words"}]}))')" "workspace=$A"
want 200 "edit just before stop"
stop_daemon
file_is "$A/.k2/wiki/Home.md" '"# Wiki\n\nedited outside\nlast words\n"' "release flushes"
ok "a normal stop writes back what was unwritten"
printf '# Wiki\n\nedited outside\nlast words\nwhile K2 was down\n' >"$A/.k2/wiki/Home.md"
printf 'edited while K2 was down\n' >"$B/notes.txt"
start_daemon daemon2.log
doc_is "$A" .k2/wiki/Home.md '"# Wiki\n\nedited outside\nlast words\nwhile K2 was down\n"' "reconcile"
ok "the store reopens and reconciles edits made while K2 was down"
owner POST /cli/docs/lease '{"path":"notes.txt","action":"renew"}' "workspace=$B"
want 409 "renew after a restart"; code_is no_lease "renew after a restart"
jok "$BODY" 'j["error"]["exit"] == 2' "leases live in memory: after a restart, renew exits 2 (K-T0-7)"
t0_wait "$B" notes.txt 'j["current"]["reason"] == "external" and j["current"]["size"] == len("edited while K2 was down\n")' "tier-0 reconcile"
owner GET /cli/docs/versions "" "workspace=$B&path=notes.txt"; want 200 "versions after the restart"
jok "$BODY" '[v["reason"] for v in j["versions"]] == ["import", "external"]' "a tier-0 doc keeps its versions and takes edits made while K2 was down"
for _ in $(seq 1 120); do grep -q "\"doc-space-key-purged\"" "$audit" && break; sleep 0.25; done
grep "\"doc-space-key-purged\"" "$audit" | grep -q "space=$CID key_destroyed=true" || fail "no doc-space-key-purged line for $CID in $audit"
ok "the removed workspace's docs and blob key are gone (audited)"
echo "PASS ($pass checks)"
