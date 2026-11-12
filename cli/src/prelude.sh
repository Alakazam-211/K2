#!/bin/bash
# K2 CLI — Agent interface to K2 workspace orchestration
# This script communicates with the K2 daemon via HTTP.
# Works inside K2 terminals (auto-configured) and external terminals
# (auto-discovers connection from ~/.k2/heartbeat.{port,token}).
#
# NOTE (0.40.4+): each workspace's dot-dir was renamed `.k2so/` -> `.k2/`.
# Inbox/skills/prds/agent now live under `<workspace>/.k2/`. This CLI resolves
# either layout (`.k2/` preferred, `.k2so/` fallback) via `_ws_dot_dir()`, so
# agents should read/write `<workspace>/.k2/...` going forward.

K2_CLI_VERSION="0.46.11"

set -euo pipefail

# Windows (Git Bash): `python3` is usually the Microsoft Store stub, and
# Python there writes CRLF to pipes. Use whichever real Python 3 answers
# (`python3`, `py -3`, `python`) and drop the CRs from its stdout, so every
# `$(python3 …)` below reads what it reads on macOS and Linux. Unix shells
# never enter this branch.
_K2_WINDOWS=""
case "$(uname -s 2>/dev/null)" in
    MINGW*|MSYS*|CYGWIN*)
        _K2_WINDOWS=1
        # Python on Windows writes pipes and opens files in the ANSI code
        # page (cp1252): "✓" crashed it and "—" came out as a non-UTF-8
        # byte. UTF-8 mode makes it read and write UTF-8, as on macOS/Linux.
        export PYTHONUTF8=1
        _k2_py=""
        for _k2_c in "command python3" "py -3" "python"; do
            if $_k2_c -c 'import sys; sys.exit(0 if sys.version_info[0] == 3 else 1)' >/dev/null 2>&1; then
                _k2_py="$_k2_c"
                break
            fi
        done
        if [ -n "$_k2_py" ]; then
            eval "python3() { $_k2_py \"\$@\" | tr -d '\\r'; }"
        else
            # No real Python 3: say so where a command needs it, instead of
            # the Store stub's "Python was not found".
            python3() {
                echo "K2's command-line tools need Python 3 on Windows. Install it from https://www.python.org/downloads/windows/ (tick \"Add python.exe to PATH\"), then open a new terminal." >&2
                return 127
            }
        fi
        unset _k2_c
        ;;
esac

# Connection details (set by K2 when creating terminal sessions)
# Falls back to ~/.k2/heartbeat.{port,token} for external terminals
PORT="${K2_PORT:-${K2SO_PORT:-}}"
# LAN / remote daemon: override loopback. Default matches today's bind.
HOST="${K2_HOST:-127.0.0.1}"
TOKEN="${K2_HOOK_TOKEN:-${K2SO_HOOK_TOKEN:-}}"

# #58 Phase-1 close — capture the per-cell SCOPED token NOW, before any
# Open-tool disk-owner inversion (see _cli_apply_auth_for_verb) can clobber
# $TOKEN with the daemon owner token. The scoped token is what the per-cell
# UDS arm authenticates with (Bearer). For ID/locked tools we MUST keep this
# passport and never re-source ~/.k2/heartbeat.token (owner) — that COMPAT-58
# inversion is the agent elevation hole for ID verbs. Empty outside a scoped
# cell (the UDS arm is then skipped — see cli_send).
SCOPED_TOKEN="${K2_HOOK_TOKEN:-${K2SO_HOOK_TOKEN:-}}"

# BEGIN_CLI_HEALED_PASSPORT
# A session's env can't change, so when a daemon restart finds the passport
# store lost it mints a new passport for this same session into the session's
# own secrets dir ($K2_SECRETS_DIR/.k2-passport, 0600, written only by the
# daemon's adoption). Used when it names this session and differs from the
# env's; never for another session, never for an owner token (no `.`).
_K2_PASSPORT_REFUSAL="Invalid or missing auth token"
_K2_PASSPORT_RETRIED=0
_cli_healed_passport() {
    local cur="${SCOPED_TOKEN:-}" f="${K2_SECRETS_DIR:-}/.k2-passport" t=""
    [ -n "${K2_SECRETS_DIR:-}" ] && [[ "$cur" == *.* ]] || return 1
    [ -f "$f" ] && [ ! -L "$f" ] || return 1
    IFS= read -r t <"$f" 2>/dev/null || [ -n "$t" ] || return 1
    [[ "$t" =~ ^[A-Za-z0-9-]+\.[A-Za-z0-9]+$ ]] || return 1
    [ "$t" != "$cur" ] && [ "${t%%.*}" = "${cur%%.*}" ] || return 1
    printf '%s' "$t"
}
# Switch this run (and any k2 it starts) to the re-minted passport. False when
# there is none.
_cli_use_healed_passport() {
    local t
    t="$(_cli_healed_passport)" || return 1
    [ "${TOKEN:-}" = "${SCOPED_TOKEN:-}" ] && TOKEN="$t"
    SCOPED_TOKEN="$t"
    export K2_HOOK_TOKEN="$t"
    if [ -n "${K2SO_HOOK_TOKEN:-}" ]; then export K2SO_HOOK_TOKEN="$t"; fi
    return 0
}
# A call refused its passport, started before the new one was written (the
# daemon was restarting): true once per shell when there is a new passport to
# try. A 403 ran no handler, so sending the request again is safe.
_cli_retry_with_healed_passport() {
    [ "$_K2_PASSPORT_RETRIED" = 0 ] || return 1
    [[ "${1:-}" == *"$_K2_PASSPORT_REFUSAL"* ]] || return 1
    _cli_use_healed_passport || return 1
    _K2_PASSPORT_RETRIED=1
}
_cli_use_healed_passport || true
# END_CLI_HEALED_PASSPORT

# Disk owner credential (if present). Used only for Open-tool TCP ergonomics
# and external (non-cell) terminals — never for ID/locked tools when a
# scoped session marker ($K2_HOOK_SOCK) is present.
DISK_OWNER_TOKEN=""

# Daemon home: ~/.k2 post-0.40 (the migration leaves ~/.k2so as a compat
# symlink, so either resolves to the same files — prefer the new name,
# fall back for a not-yet-migrated machine).
K2_HOME="${HOME}/.k2"
[ -d "$K2_HOME" ] || K2_HOME="${HOME}/.k2so"

# Air-gap: same polarity as k2_core::airgap (env wins; garbage → on;
# unset → persisted `airgap` in ~/.k2/settings.json; default off).
_k2_airgap_enabled() {
    if [ "${K2_AIRGAP+x}" = "x" ]; then
        case "$(printf '%s' "$K2_AIRGAP" | tr '[:upper:]' '[:lower:]')" in
            1|true|on|yes) return 0 ;;
            0|false|off|no) return 1 ;;
            *) return 0 ;;
        esac
    fi
    python3 -c '
import json, sys
try:
    with open(sys.argv[1]) as fh:
        v = json.load(fh).get("airgap")
except Exception:
    sys.exit(1)
sys.exit(0 if v is True else 1)
' "${K2_HOME}/settings.json" 2>/dev/null
}

_k2_refuse_if_airgap() {
    if _k2_airgap_enabled; then
        echo "Error: air-gap is on (K2_AIRGAP=1). This command cannot phone Connect, GitHub, or other hosted services." >&2
        exit 1
    fi
}

# Workspace dot-dir for a project root: .k2/ preferred when present,
# .k2so/ is the 0.x default (Q2 dual-compat; full flip lands pre-1.0.0).
_ws_dot_dir() {
    if [ -d "$1/.k2" ]; then echo "$1/.k2"; else echo "$1/.k2so"; fi
}

# One `note:` line about an address, on stderr (exit code unaffected).
# `_k2_addr_note old new [renamed]`: "is now" with `renamed` (a local
# rename; prd-thread-survives-tab-rename S5), "is" without (a synonym;
# prd-cli-addressing A7). Same name and shape as the CLI-addressing
# builder's helper (TR17), so the two branches print one way.
_k2_addr_note() {
    local old="${1:-}" new="${2:-}" how="${3:-}"
    if [ -z "$old" ] || [ -z "$new" ] || [ "$old" = "$new" ]; then
        return 0
    fi
    if [ "$how" = "renamed" ]; then
        printf 'note: %s is now %s\n' "$old" "$new" >&2
    else
        printf 'note: %s is %s\n' "$old" "$new" >&2
    fi
    return 0
}

# The rename note when a daemon response carries `movedFrom` (+ `addr`).
_k2_moved_from_note() {
    local pair
    pair=$(printf '%s' "$1" | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    sys.exit(0)
if isinstance(d,dict) and d.get("movedFrom") and d.get("addr"):
    print("%s %s" % (d["movedFrom"], d["addr"]))' 2>/dev/null)
    [ -n "$pair" ] || return 0
    _k2_addr_note "${pair%% *}" "${pair#* }" renamed
}

# After a successful overlay write (not --json): first post, then every
# 10th. Agents forget Thread is markdown; help is not re-read. stderr
# so stdout stays the id/seq line. Best-effort — missing .k2 is a no-op.
_thread_md_nudge() {
    local root="${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-${PROJECT:-$PWD}}}"
    local dot
    dot=$(_ws_dot_dir "$root")
    [ -d "$dot" ] || return 0
    local file="$dot/.thread-md-nudge"
    local n=0
    if [ -f "$file" ]; then
        n=$(tr -d '[:space:]' < "$file" 2>/dev/null || true)
    fi
    case "$n" in
        ''|*[!0-9]*) n=0 ;;
    esac
    n=$((n + 1))
    printf '%s\n' "$n" > "$file" 2>/dev/null || return 0
    if [ "$n" -eq 1 ] || [ $((n % 10)) -eq 0 ]; then
        echo "hint: Thread renders markdown — **bold**, lists, \`code\`, fences, links." >&2
    fi
}

# Same policy as k2_core::db::resolve_home_db_path.
# Stage B (0.46.6): the daemon renames k2so.db to k2.db at boot and leaves
# a k2so.db -> k2.db symlink, so this normally returns k2.db.
# Before that (or when the rename was deferred / refused): both real files
# → the one with more live workspaces (a stub k2.db cannot hide k2so.db);
# else k2.db if it exists, else k2so.db if it exists, else k2.db. Never
# creates either file.
#
# prd-k2-db-rename-v1 DB-A9: K2's database is only ever opened through
# _k2_db_uri (`file:…?mode=rw`): it opens an existing file and never
# creates a missing one (a plain `sqlite3 <missing>` mints a 0-byte stub,
# which is how the z3mbp stub k2.db appeared). Not `-readonly`/`mode=ro`:
# the sqlite3 CLI refuses those on a WAL database once K2 has stopped and
# removed its -shm. Callers only read.
_k2_db_uri() {
    local p="$1"
    p="${p//%/%25}"; p="${p//\?/%3f}"; p="${p//#/%23}"
    printf 'file:%s?mode=rw' "$p"
}
_k2_home_db() {
    local new="$K2_HOME/k2.db"
    local old="$K2_HOME/k2so.db"
    if [ -f "$new" ] && [ ! -L "$new" ] && [ -f "$old" ] && [ ! -L "$old" ]; then
        local cn=0 co=0
        if command -v sqlite3 >/dev/null 2>&1; then
            cn=$(sqlite3 "$(_k2_db_uri "$new")" "SELECT count(*) FROM projects WHERE id NOT IN ('_orphan', '_broadcast');" 2>/dev/null || true)
            co=$(sqlite3 "$(_k2_db_uri "$old")" "SELECT count(*) FROM projects WHERE id NOT IN ('_orphan', '_broadcast');" 2>/dev/null || true)
        fi
        cn=$(( ${cn:-0} + 0 ))
        co=$(( ${co:-0} + 0 ))
        if [ "$co" -gt "$cn" ]; then
            echo "$old"
            return
        fi
        if [ "$cn" -gt "$co" ]; then
            echo "$new"
            return
        fi
        local ln=0 lo=0
        ln=$(wc -c < "$new" 2>/dev/null | tr -d '[:space:]')
        lo=$(wc -c < "$old" 2>/dev/null | tr -d '[:space:]')
        ln=$(( ${ln:-0} + 0 ))
        lo=$(( ${lo:-0} + 0 ))
        if [ "$lo" -gt $(( ln * 2 )) ] && [ "$lo" -gt 1000000 ]; then
            echo "$old"
            return
        fi
        if [ "$ln" -gt $(( lo * 2 )) ] && [ "$ln" -gt 1000000 ]; then
            echo "$new"
            return
        fi
        echo "$old"
        return
    fi
    if [ -e "$new" ] || [ ! -e "$old" ]; then
        echo "$new"
    else
        echo "$old"
    fi
}

# Can the CLI run `git` without side effects? On a fresh Mac `/usr/bin/git`
# is Apple's shim: until the Command Line Tools are installed every call
# fails AND opens Apple's install dialog, so `k2 <anything>` from a
# non-K2 folder used to pop it (0.46.4). Run git only when the git on PATH
# isn't that shim, or `xcode-select -p` names a developer folder with a
# real git in it (`xcode-select -p` never opens the dialog). Cached.
_K2_GIT_USABLE=""
_k2_git_usable() {
    if [ -z "$_K2_GIT_USABLE" ]; then
        _K2_GIT_USABLE=no
        local g dev
        g="$(command -v git 2>/dev/null || true)"
        if [ -n "$g" ]; then
            if [ "$(uname -s 2>/dev/null)" = Darwin ] && [ "$g" = /usr/bin/git ]; then
                dev="$(/usr/bin/xcode-select -p 2>/dev/null || true)"
                [ -n "$dev" ] && [ -x "$dev/usr/bin/git" ] && _K2_GIT_USABLE=yes
            else
                _K2_GIT_USABLE=yes
            fi
        fi
    fi
    [ "$_K2_GIT_USABLE" = yes ]
}

# Resolve project root: if K2_PROJECT_PATH is set, use it directly.
# Otherwise walk up from CWD to find a .k2/ or .k2so/ directory, skipping $HOME.
# Also handles git worktrees: if CWD is inside a worktree, resolve back to
# the main repo root where the workspace dot-dir lives.
PROJECT="${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-$(pwd)}}"
if [ -z "${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-}}" ]; then
    _dir="$(pwd)"
    _found=""
    while [ "$_dir" != "/" ]; do
        if { [ -d "$_dir/.k2so" ] || [ -d "$_dir/.k2" ]; } && [ "$_dir" != "$HOME" ]; then
            _found="$_dir"
            break
        fi
        _dir="$(dirname "$_dir")"
    done
    if [ -z "$_found" ]; then
        # May be inside a git worktree — resolve to main repo root.
        # `|| true` is REQUIRED: from a non-git CWD, `git rev-parse` exits 128,
        # and under `set -euo pipefail` that failing command substitution would
        # abort the entire script before it ever reads the daemon port — a
        # silent exit 128 (GH #16). Swallow it so a non-git CWD just leaves
        # _main_root empty and we fall back to $PWD as PROJECT.
        _main_root=""
        if _k2_git_usable; then
            _main_root=$(git -C "$(pwd)" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's|/.git$||' || true)
        fi
        if [ -n "$_main_root" ] && { [ -d "$_main_root/.k2so" ] || [ -d "$_main_root/.k2" ]; }; then
            _found="$_main_root"
        fi
    fi
    PROJECT="${_found:-$PROJECT}"
    unset _dir _found _main_root
fi

# 0.40.4 cutover heads-up — git-derived + self-clearing. While the
# `.k2so/` -> `.k2/` rename is uncommitted, `git ls-files .k2so` still lists
# the (now-deleted-on-disk) tracked files; once the user commits the rename it
# returns nothing and this goes silent. `|| true` guards the git call from
# aborting the script in a non-git CWD (GH#16). stderr only — never pollutes
# command stdout / JSON.
if [ -d "$PROJECT/.k2" ] && [ ! -d "$PROJECT/.k2so" ] && _k2_git_usable; then
    _k2so_pending=$(git -C "$PROJECT" ls-files .k2so 2>/dev/null | head -1 || true)
    if [ -n "$_k2so_pending" ]; then
        echo "i  K2 0.40.4 renamed this workspace's .k2so/ -> .k2/ (harness symlinks + .gitignore updated). Commit the rename when ready — git tracks it as renames." >&2
    fi
    unset _k2so_pending
fi

# Auto-discover connection from the daemon home if not in a K2 terminal.
# Always capture DISK_OWNER_TOKEN when readable so Open tools can still do
# the COMPAT-58 TCP inversion; never *apply* that inversion here — ID/locked
# tools must not inherit the owner credential just because $K2_HOOK_SOCK is set.
if [ -f "${K2_HOME}/heartbeat.port" ]; then
    PORT="${PORT:-$(cat "${K2_HOME}/heartbeat.port" 2>/dev/null)}"
fi
# A K2 virtual desktop (prd-linux-virtual-desktop-v1 LD-10) runs every
# program with K2_NO_OWNER_FALLBACK=1: a terminal opened there has no
# session passport, and must never act as this server's owner by reading
# the owner token files. The connection gate below says why.
_k2_no_owner_fallback=""
if [ "${K2_NO_OWNER_FALLBACK:-}" = "1" ]; then
    _k2_no_owner_fallback=1
fi
if [ -z "$_k2_no_owner_fallback" ] && [ -r "${K2_HOME}/heartbeat.token" ]; then
    DISK_OWNER_TOKEN="$(cat "${K2_HOME}/heartbeat.token" 2>/dev/null)"
fi
# Fresh headless boxes always get daemon.token; heartbeat.token is the
# same secret written alongside. Prefer heartbeat (CLI convention) but
# accept daemon.token so external owner ops work when only one is present.
if [ -z "$_k2_no_owner_fallback" ] && [ -z "$DISK_OWNER_TOKEN" ] && [ -r "${K2_HOME}/daemon.token" ]; then
    DISK_OWNER_TOKEN="$(cat "${K2_HOME}/daemon.token" 2>/dev/null)"
fi
if [ -z "$TOKEN" ] && [ -n "$DISK_OWNER_TOKEN" ]; then
    TOKEN="$DISK_OWNER_TOKEN"
fi

