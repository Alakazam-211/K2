#!/usr/bin/env bash
# `k2 changes` (Changes tab C4) end to end against a foreground sandbox
# daemon built from THIS worktree (temp HOME, no client attached), as the
# owner and as an agent holding only its session passport (cell socket).
# Contract: .k2/prds/prd-changes-tab-v1.md (routes) + the CLI parity table.
#
#   cargo build -p k2-daemon && bash tests/cli/changes_cli_headless.sh
#   bash tests/cli/changes_cli_headless.sh --build   (builds first)
#
# Discards go to $K2_GIT_TRASH_DIR (a temp dir), never the OS Trash.
# Every check fails loudly with the command output.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

# Git under test: no user/system config (hooks, signing, templates).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
GITC=(git -c user.name=K2-Test -c user.email=k2-test@example.com -c init.defaultBranch=main -c commit.gpgsign=false)

# Short root: the cell socket (~/.k2/run/cells/<uuid>.sock) must fit SUN_LEN.
WORK="$(mktemp -d /tmp/k2c4.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
TRASH="$WORK/trash"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run" "$TRASH"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    pkill -9 -f "frpc -c $SB/" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -30 "$SB/daemon.log" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
# run <who> <args…> → OUT (stdout), ERR (stderr), RC. <who>: owner|agent;
# runs in $CWD (default: the repo). Never a TTY on stdin.
CWD=""
run() {
    local who="$1"; shift
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_PROJECT_PATH="$REPO")
    if [ "$who" = "agent" ]; then
        envs+=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$SOCK")
    fi
    set +e
    OUT="$(cd "${CWD:-$REPO}" && env "${envs[@]}" "$K2_CLI" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1
stderr: $ERR"; }
lacks() { [[ "$1" != *"$2"* ]] || fail "$3: unexpected $(printf %q "$2") in:
$1"; }
# jcheck <json> <python-expr over j>
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }
errcode() { # the code in the last stderr JSON line
    python3 -c '
import json, sys
code = ""
for line in sys.stdin.read().splitlines():
    try:
        v = json.loads(line)
    except Exception:
        continue
    if isinstance(v, dict) and isinstance(v.get("error"), dict):
        code = v["error"].get("code", "")
print(code)' <<<"$ERR"
}

echo "== CLI without a daemon =="
set +e
help_out="$(env HOME="$SB" "$K2_CLI" changes --help 2>&1)"; help_rc=$?
set -e
[ "$help_rc" = 0 ] || fail "k2 changes --help exit $help_rc: $help_out"
has "$help_out" "k2 changes discard <path>... [--staged] [--yes]" "help"
ok "k2 changes --help needs no daemon"
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
[ "$(_cli_tool_id_for_verb changes)" = "changes" ] || fail "changes → tool id"
[ "$(_cli_tool_default_mode changes)" = "id" ] || fail "changes mode id"
_cli_tool_is_locked changes || fail "changes locked"
ok "changes is an id + locked tool (passport inside K2, never the owner token)"

echo "== daemon =="
# `--build`: build this worktree's daemon first (one compute job runs both).
if [ "${1:-}" = "--build" ]; then
    (cd "$PROJECT_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build -p k2-daemon) || fail "cargo build -p k2-daemon"
fi
DAEMON_BIN=""
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
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny K2_GIT_TRASH_DIR="$TRASH" \
    "$DAEMON_BIN" >"$SB/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 200); do
    [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SB/.k2/daemon.port" ] || fail "daemon never wrote daemon.port"
DPORT="$(cat "$SB/.k2/daemon.port")"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "$BASE/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
ok "sandbox daemon ready ($DAEMON_BIN)"

echo "== repo + workspace + agent passport =="
REPO="$WORK/repo"
OTHER="$WORK/other"
PLAIN="$WORK/plain"
mkdir -p "$REPO" "$OTHER" "$PLAIN"
for r in "$REPO" "$OTHER"; do
    "${GITC[@]}" -C "$r" init -q
    printf 'one\ntwo\n' >"$r/a.txt"
    printf 'keep\n' >"$r/b.txt"
    "${GITC[@]}" -C "$r" add -A
    "${GITC[@]}" -C "$r" commit -q -m init
    git -C "$r" config user.name K2-Test
    git -C "$r" config user.email k2-test@example.com
    git -C "$r" config commit.gpgsign false
    # K2's own per-workspace state is not part of the change set under test.
    printf '.k2/\n.claude/\nAGENTS.md\nCLAUDE.md\n.worktrees/\n' >>"$r/.git/info/exclude"
done
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$REPO")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *"$REPO"*) ok "workspace registered" ;; *) fail "register: $reg" ;; esac
resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" \
    -H 'Content-Type: application/json' --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"project": sys.argv[1]}))' "$REPO")")"
case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
for _ in $(seq 1 100); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
[ -S "$SOCK" ] || fail "the pinned chat's cell socket is missing: '$SOCK'"
ok "agent passport + cell socket"

echo "== owner: list / diff =="
run owner changes --json
want_rc 0 "clean list"
jok "$OUT" 'j["isRepo"] and j["counts"]["total"] == 0' "clean snapshot"
run owner changes
want_rc 0 "clean human list"
has "$OUT" "No changes." "clean human list"
ok "clean human list"

printf 'one\nTWO\nthree\n' >"$REPO/a.txt"
printf 'new\n' >"$REPO/new.txt"
rm "$REPO/b.txt"
run owner changes
want_rc 0 "dirty list"
for needle in "On main" "Unstaged (2)" "Untracked (1)" "M  a.txt" "D  b.txt" "U  new.txt" "+2 -1" "3 changes"; do
    has "$OUT" "$needle" "dirty list"
done
ok "dirty human list"
run owner changes --json
jok "$OUT" 'j["counts"]["unstaged"] == 2 and j["counts"]["untracked"] == 1 and j["workspacePath"].endswith("/repo")' "dirty --json is the snapshot"

run owner changes diff a.txt
want_rc 0 "diff a.txt"
for needle in "diff --git a/a.txt b/a.txt" "--- a/a.txt" "+++ b/a.txt" "@@ -1,2 +1,3 @@" "-two" "+TWO" "+three"; do
    has "$OUT" "$needle" "diff a.txt"
done
printf '%s\n' "$OUT" >"$WORK/a.patch"
git -C "$REPO" apply --check -R "$WORK/a.patch" || fail "diff a.txt does not reverse-apply with git apply: $OUT"
ok "diff a.txt is a unified diff git apply accepts"

run owner changes diff
want_rc 0 "diff (all)"
has "$OUT" "+++ b/new.txt" "diff (all) new file"
has "$OUT" "--- /dev/null" "diff (all) new file"
has "$OUT" "+++ /dev/null" "diff (all) deleted file"
has "$OUT" "-keep" "diff (all) deleted file"
printf '%s\n' "$OUT" >"$WORK/all.patch"
git -C "$REPO" apply --check -R "$WORK/all.patch" || fail "diff (all) does not reverse-apply: $OUT"
ok "diff (all): modified + new + deleted, reverse-applies"

run owner changes diff --json
want_rc 0 "diff --json"
jok "$OUT" 'sorted(f["path"] for f in j["files"]) == ["a.txt", "b.txt", "new.txt"] and all(f["patch"] for f in j["files"]) and j["mode"] == "worktree"' "diff --json"

run owner changes diff --staged
want_rc 0 "diff --staged (nothing staged)"
[ -z "$OUT" ] || fail "diff --staged with nothing staged printed: $OUT"
ok "diff --staged empty when nothing is staged"

echo "== owner: stage / unstage / commit =="
run owner changes stage a.txt
want_rc 0 "stage a.txt"
has "$OUT" "Staged a.txt" "stage a.txt"
run owner changes --json
jok "$OUT" 'any(f["path"] == "a.txt" and f["group"] == "staged" for f in j["files"])' "a.txt staged"
run owner changes diff --staged
want_rc 0 "diff --staged"
has "$OUT" "+TWO" "diff --staged"
lacks "$OUT" "new.txt" "diff --staged"
ok "diff --staged shows only the staged file"
run owner changes unstage a.txt
want_rc 0 "unstage a.txt"
run owner changes --json
jok "$OUT" 'j["counts"]["staged"] == 0' "a.txt unstaged"

mkdir -p "$REPO/sub"
printf 'c\n' >"$REPO/sub/c.txt"
CWD="$REPO/sub" run owner changes stage c.txt
want_rc 0 "stage from a sub-folder"
run owner changes --json
jok "$OUT" 'any(f["path"] == "sub/c.txt" and f["group"] == "staged" for f in j["files"])' "a relative path is taken from where you stand"

run owner changes stage ../outside.txt
want_rc 3 "stage outside the repo"
[ "$(errcode)" = "bad_path" ] || fail "stage ../outside.txt code: $ERR"
ok "stage ../outside.txt → exit 3 bad_path"

run owner changes commit -m "owner commit"
want_rc 0 "commit"
has "$OUT" "Committed " "commit"
[ "$(git -C "$REPO" log -1 --format=%s)" = "owner commit" ] || fail "HEAD subject after commit"
[ "$(git -C "$REPO" log -1 --format=%an)" = "K2-Test" ] || fail "commit author"
[ -z "$(git -C "$REPO" status --porcelain -- sub/c.txt)" ] || fail "sub/c.txt not committed"
ok "commit made the commit (author = the repo's git user)"

run owner changes commit -m "nothing here"
want_rc 3 "commit with nothing staged"
[ "$(errcode)" = "nothing_staged" ] || fail "nothing_staged code: $ERR"
[ "$(git -C "$REPO" log -1 --format=%s)" = "owner commit" ] || fail "an empty commit was made"
ok "commit with nothing staged → exit 3 nothing_staged, no empty commit"

run owner changes commit
want_rc 2 "commit without -m"
ok "commit without -m → exit 2"

echo "== owner: discard =="
run owner changes discard a.txt new.txt
want_rc 2 "discard without --yes off a terminal"
[ "$(errcode)" = "confirm_required" ] || fail "confirm_required code: $ERR"
[ -e "$REPO/new.txt" ] || fail "new.txt touched without --yes"
ok "discard without --yes (no TTY) → exit 2 confirm_required, nothing touched"

run owner changes discard a.txt new.txt --yes
want_rc 0 "discard --yes"
has "$OUT" "restored  a.txt" "discard"
has "$OUT" "trashed   new.txt" "discard"
[ "$(cat "$REPO/a.txt")" = "$(printf 'one\ntwo')" ] || fail "a.txt not restored: $(cat "$REPO/a.txt")"
[ ! -e "$REPO/new.txt" ] || fail "new.txt still in the repo"
[ -n "$(find "$TRASH" -name 'new.txt*' -print -quit)" ] || fail "new.txt not in the trash dir: $(ls -R "$TRASH")"
ok "discard: a.txt restored, new.txt (and a.txt's old copy) in the Trash"

run owner changes discard a.txt ../../etc/hosts --yes --json
want_rc 3 "discard bad path"
[ "$(errcode)" = "bad_path" ] || fail "discard bad_path code: $ERR"
ok "discard with a path outside → exit 3 bad_path"

echo "== owner: refusals =="
run owner changes --base no-such-ref
want_rc 3 "bad base"
[ "$(errcode)" = "bad_ref" ] || fail "bad_ref code: $ERR"
ok "--base no-such-ref → exit 3 bad_ref"
run owner changes --workspace "$PLAIN"
want_rc 3 "not a repo"
[ "$(errcode)" = "not_a_repo" ] || fail "not_a_repo code: $ERR"
run owner changes --workspace "$PLAIN" --json
want_rc 0 "not a repo --json"
jok "$OUT" 'j["isRepo"] is False' "not a repo: exit 3 (human), the snapshot as is (--json)"
run owner changes --workspace "$OTHER" --json
want_rc 0 "owner --workspace other"
jok "$OUT" 'j["isRepo"] and j["workspacePath"].endswith("/other")' "owner may name any workspace"
run owner changes frobnicate
want_rc 2 "unknown sub"
ok "unknown subcommand → exit 2"

echo "== agent (passport on the cell socket) =="
printf 'one\ntwo\nagent\n' >"$REPO/a.txt"
printf 'scratch\n' >"$REPO/scratch.txt"
run agent changes --json
want_rc 0 "agent list"
jok "$OUT" 'j["isRepo"] and j["workspacePath"].endswith("/repo") and j["counts"]["unstaged"] == 2 and j["counts"]["untracked"] == 1' "agent: its own workspace's snapshot (a.txt edited, b.txt still deleted, scratch.txt new)"
run agent changes diff a.txt
want_rc 0 "agent diff"
has "$OUT" "+agent" "agent diff"
ok "agent: diff"
run agent changes stage a.txt
want_rc 0 "agent stage"
run agent changes commit -m "agent commit"
want_rc 0 "agent commit"
[ "$(git -C "$REPO" log -1 --format=%s)" = "agent commit" ] || fail "agent commit not on HEAD"
ok "agent: stage + commit"
run agent changes discard scratch.txt --yes
want_rc 0 "agent discard"
[ ! -e "$REPO/scratch.txt" ] || fail "scratch.txt still there"
[ -n "$(find "$TRASH" -name 'scratch.txt*' -print -quit)" ] || fail "scratch.txt not in the trash dir"
ok "agent: discard → Trash"
run agent changes --workspace "$OTHER"
want_rc 3 "agent on another workspace"
[ "$(errcode)" = "forbidden" ] || fail "agent other workspace code: $ERR"
ok "agent: another workspace → exit 3 forbidden (passport, never the owner token)"
run agent changes stage a.txt --workspace "$OTHER"
want_rc 3 "agent stage in another workspace"
[ "$(errcode)" = "forbidden" ] || fail "agent stage other code: $ERR"
ok "agent: stage in another workspace → forbidden"

"${GITC[@]}" -C "$REPO" worktree add -q "$REPO/.worktrees/feat" -b feat
printf 'wt\n' >"$REPO/.worktrees/feat/wt.txt"
CWD="$REPO/.worktrees/feat" run agent changes --json
want_rc 0 "agent in a worktree"
jok "$OUT" 'j["workspacePath"].endswith("/.worktrees/feat") and any(f["path"] == "wt.txt" for f in j["files"])' "agent: the checkout it stands in (a worktree inside its workspace)"
CWD="$REPO/.worktrees/feat" run agent changes stage wt.txt
want_rc 0 "agent stage in the worktree"
CWD="$REPO/.worktrees/feat" run agent changes commit -m "wt commit"
want_rc 0 "agent commit in the worktree"
[ "$(git -C "$REPO/.worktrees/feat" log -1 --format=%s)" = "wt commit" ] || fail "worktree commit"
[ "$(git -C "$REPO" log -1 --format=%s)" = "agent commit" ] || fail "main checkout moved"
ok "agent: stage + commit in its worktree; main checkout untouched"
CWD="$REPO/.worktrees/feat" run agent changes diff --base main
want_rc 0 "agent diff --base main"
has "$OUT" "+++ b/wt.txt" "agent diff --base main"
has "$OUT" "+wt" "agent diff --base main"
ok "agent: diff --base main shows the branch's committed work"
CWD="$REPO/.worktrees/feat" run agent changes --base main
want_rc 0 "agent list --base main"
has "$OUT" "Compared with main" "list --base"
has "$OUT" "Changed vs base (1)" "list --base"
ok "agent: list --base main (branch review)"

echo "== C5: untracked folder stages recursively (gitignore respected) =="
mkdir -p "$REPO/newdir/deep"
printf 'x\n' >"$REPO/newdir/x.txt"
printf 'y\n' >"$REPO/newdir/deep/y.txt"
printf 'noise\n' >"$REPO/newdir/deep/noise.log"
printf '*.log\n' >>"$REPO/.git/info/exclude"
run owner changes --json
jok "$OUT" 'any(f["path"] == "newdir/" and f["dir"] for f in j["files"])' "the folder is one untracked row"
run owner changes stage newdir/
want_rc 0 "stage newdir/"
run owner changes --json
jok "$OUT" 'sorted(f["path"] for f in j["files"] if f["group"] == "staged") == ["newdir/deep/y.txt", "newdir/x.txt"]' "stage newdir/ adds its files, not the ignored one"
run owner changes commit -m "folder"
want_rc 0 "commit the folder"

echo "== C5: review notes (owner writes, agent reads + resolves) =="
run owner changes notes add a.txt --line 2 -m "Why two?" -m "Say why."
want_rc 0 "owner notes add"
has "$OUT" "Added note n_" "notes add"
NOTE_ID="$(sed -n 's/^Added note \(n_[0-9a-f]*\) .*/\1/p' <<<"$OUT")"
[ -n "$NOTE_ID" ] || fail "no note id in: $OUT"
ok "owner: notes add → $NOTE_ID"
run owner changes notes add a.txt --line 40 -m "past the end"
want_rc 3 "notes add past the end"
[ "$(errcode)" = "bad_request" ] || fail "past-the-end code: $ERR"
ok "notes add past the end → exit 3 bad_request"
run owner changes notes add a.txt -m "no line"
want_rc 2 "notes add without --line"
run owner changes notes --json
want_rc 0 "owner notes list"
jok "$OUT" 'j["counts"]["open"] == 1 and j["counts"]["unsent"] == 1 and j["notes"][0]["body"] == "Why two?\n\nSay why." and j["notes"][0]["anchorText"] == "two" and j["notes"][0]["outdated"] is False' "owner: notes list (--json)"

run agent changes notes
want_rc 0 "agent notes list"
has "$OUT" "$NOTE_ID  a.txt:2" "agent notes list"
has "$OUT" "Why two?" "agent notes list"
ok "agent: reads the notes left on its workspace"
run agent changes notes add a.txt --line 1 -m "agent note"
want_rc 3 "agent notes add"
[ "$(errcode)" = "forbidden" ] || fail "agent add code: $ERR"
run agent changes notes send
want_rc 3 "agent notes send"
[ "$(errcode)" = "forbidden" ] || fail "agent send code: $ERR"
ok "agent: add / send → exit 3 forbidden"
run agent changes notes --workspace "$OTHER"
want_rc 3 "agent notes on another workspace"
[ "$(errcode)" = "forbidden" ] || fail "agent other notes code: $ERR"
ok "agent: notes on another workspace → forbidden"

run owner changes notes send
want_rc 0 "owner notes send"
has "$OUT" "Sent 1 note to the agent of $REPO" "notes send"
run owner changes notes --status sent --json
jok "$OUT" 'len(j["notes"]) == 1 and j["notes"][0]["sentAt"] and (j["notes"][0]["deliveryId"] or "").startswith("m_")' "owner: send delivered once through the msg path (delivery id) and marked the note sent"
run owner changes notes send
want_rc 3 "send again"
[ "$(errcode)" = "nothing_to_send" ] || fail "send again code: $ERR"
ok "a second send has nothing to send (no double delivery)"

run agent changes notes resolve "$NOTE_ID"
want_rc 0 "agent resolve"
has "$OUT" "Resolved $NOTE_ID" "agent resolve"
run owner changes notes --status resolved --json
jok "$OUT" 'j["notes"][0]["resolvedBy"].startswith("agent:")' "agent: resolve (resolvedBy = the agent)"
run agent changes notes reopen "$NOTE_ID"
want_rc 3 "agent reopen"
[ "$(errcode)" = "forbidden" ] || fail "agent reopen code: $ERR"
run owner changes notes reopen "$NOTE_ID"
want_rc 0 "owner reopen"
run owner changes notes edit "$NOTE_ID" -m "Explain the two."
want_rc 0 "owner edit"
run owner changes notes --json
jok "$OUT" 'j["counts"]["unsent"] == 1 and j["notes"][0]["body"] == "Explain the two."' "owner: reopen (goes out again) + edit"
printf 'zero\none\ntwo\nagent\n' >"$REPO/a.txt"
run owner changes notes --json
jok "$OUT" 'j["notes"][0]["currentStartLine"] == 3 and not j["notes"][0]["outdated"]' "a moved line is followed"
printf 'one\nTWO\nagent\n' >"$REPO/a.txt"
# The index still has "two" (a staged row's new side); stage the rewrite too.
run owner changes stage a.txt
want_rc 0 "stage the rewrite"
run owner changes notes --json
jok "$OUT" 'j["notes"][0]["outdated"] is True' "a rewritten line is outdated"
run owner changes notes delete "$NOTE_ID"
want_rc 0 "owner delete"
run owner changes notes --status all --json
jok "$OUT" 'j["notes"] == []' "owner: delete"
code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/git/notes/send?token=$DTOKEN")"
[ "$code" = "405" ] || fail "GET notes/send → $code"
ok "GET /cli/git/notes/send → 405"

echo "== C5: open + init =="
run owner changes open
want_rc 0 "owner open"
has "$OUT" "Asked K2 to open the Changes tab for $REPO" "owner open"
run agent changes open --json
want_rc 0 "agent open"
jok "$OUT" 'j["workspacePath"].endswith("/repo")' "agent: changes open on its own workspace"
code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/git/open-changes?token=$DTOKEN")"
[ "$code" = "405" ] || fail "GET open-changes → $code"
ok "GET /cli/git/open-changes → 405"

info="$(curl -s "$BASE/cli/git/info?token=$DTOKEN&path=$PLAIN")"
jok "$info" 'j["isRepo"] is False and j["gitAvailable"] is True and isinstance(j["gitCli"], bool)' "git/info reports git available (the + menu offers Changes)"
run owner changes init --workspace "$PLAIN"
want_rc 3 "init a folder that is no workspace"
[ "$(errcode)" = "not_found" ] || fail "init unregistered code: $ERR"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$PLAIN")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *"$PLAIN"*) ;; *) fail "register plain: $reg" ;; esac
mkdir -p "$PLAIN/sub"
run owner changes init --workspace "$PLAIN/sub"
want_rc 3 "init below a workspace root"
[ "$(errcode)" = "bad_path" ] || fail "init sub code: $ERR"
[ ! -e "$PLAIN/sub/.git" ] || fail "sub got a .git"
run owner changes init --workspace "$PLAIN"
want_rc 0 "owner init"
has "$OUT" "Initialized a git repository in" "init"
[ -d "$PLAIN/.git" ] || fail "no .git after init"
run owner changes --workspace "$PLAIN" --json
jok "$OUT" 'j["isRepo"] and j["unborn"]' "init: the workspace is now a repo (no commits yet)"
git -C "$PLAIN" config user.name K2-Test
git -C "$PLAIN" config user.email k2-test@example.com
printf '.k2/\n.claude/\nAGENTS.md\nCLAUDE.md\n' >>"$PLAIN/.git/info/exclude"
printf 'hello\n' >"$PLAIN/hello.txt"
run owner changes stage hello.txt --workspace "$PLAIN"
want_rc 0 "stage in the new repo"
run owner changes commit -m "first" --workspace "$PLAIN"
want_rc 0 "first commit"
[ "$(git -C "$PLAIN" log -1 --format=%s)" = "first" ] || fail "first commit missing"
[ "$(git -C "$PLAIN" rev-list --count HEAD)" = "1" ] || fail "first commit count"
ok "the first commit in a fresh repo works (no HEAD yet)"
run owner changes init --workspace "$PLAIN"
want_rc 3 "init twice"
[ "$(errcode)" = "already_a_repo" ] || fail "init twice code: $ERR"
run agent changes init
want_rc 3 "agent init in a repo"
[ "$(errcode)" = "already_a_repo" ] || fail "agent init code: $ERR"
run agent changes init --workspace "$PLAIN"
want_rc 3 "agent init elsewhere"
[ "$(errcode)" = "forbidden" ] || fail "agent init other code: $ERR"
ok "init: root only, once; agents only their own workspace"

echo
echo "PASS: $pass checks"
