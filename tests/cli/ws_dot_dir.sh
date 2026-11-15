#!/usr/bin/env bash
# The CLI picks a workspace's dot-dir by the daemon's rule
# (crates/k2-core/src/workspace_dir.rs, workspace_dot_dir): .k2 when it
# is a folder, else .k2so when that is a folder, else .k2. The two used
# to disagree for a workspace with neither folder: the CLI said .k2so.
# No daemon is needed: every case stops before the first request.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-ws-dot-dir-XXXXXX)" && pwd -P)"
trap 'rm -rf "$T"; hermetic_cli_cleanup' EXIT
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }
ok() { pass=$((pass + 1)); }

# ── The rule itself, case by case. The right column is what the daemon's
# workspace_dot_dir answers for the same layout (its own tests pin it).
fn="$(sed -n '/^_ws_dot_dir()/,/^}/p' "$K2_CLI")"
[ -n "$fn" ] || die "no _ws_dot_dir in cli/k2"
[ "$(grep -c '^_ws_dot_dir()' "$K2_CLI")" = 1 ] || die "_ws_dot_dir is defined more than once"
eval "$fn"

layout() { # layout <name> <things to make...>: a fresh root with them
    local root="$T/$1"; shift
    mkdir -p "$root"
    local thing
    for thing in "$@"; do
        case "$thing" in
            dir:*) mkdir -p "$root/${thing#dir:}" ;;
            file:*) : > "$root/${thing#file:}" ;;
            *) die "bad layout item $thing" ;;
        esac
    done
    printf '%s\n' "$root"
}
want() { # want <case> <root> <dot-dir name>
    local got; got="$(_ws_dot_dir "$2")"
    [ "$got" = "$2/$3" ] || die "$1: the CLI answers '${got#"$2"/}', the daemon answers '$3'"
    ok
}

want "neither folder: a new workspace gets .k2" "$(layout neither)" .k2
want "only .k2" "$(layout only-k2 dir:.k2)" .k2
want "only .k2so: an older workspace keeps its name" "$(layout only-k2so dir:.k2so)" .k2so
want "both: .k2 wins" "$(layout both dir:.k2 dir:.k2so)" .k2
want "a file named .k2 in the way" "$(layout file-k2 file:.k2)" .k2so
want "a file named .k2 beside a .k2so folder" "$(layout file-k2-dir-k2so file:.k2 dir:.k2so)" .k2so
want "a file named .k2so does not count" "$(layout file-k2so file:.k2so)" .k2

# ── Each caller, through the real CLI, in a workspace with neither folder.
NEW="$(layout caller-neither)"
OLD="$(layout caller-old dir:.k2so/sessions)"

# awareness.sh, the sessions dir (two callers: archives and compact).
out="$(K2_PROJECT_PATH="$NEW" bash "$K2_CLI" sessions compact s-123 2>&1)" && die "sessions compact with no archive should fail" || true
[ "$out" = "No archive directory for session s-123 at $NEW/.k2/sessions/s-123" ] || die "sessions compact, new workspace: $out"
ok
out="$(K2_PROJECT_PATH="$OLD" bash "$K2_CLI" sessions compact s-123 2>&1)" && die "sessions compact with no archive should fail" || true
[ "$out" = "No archive directory for session s-123 at $OLD/.k2so/sessions/s-123" ] || die "sessions compact, older workspace: $out"
ok
out="$(K2_PROJECT_PATH="$NEW" bash "$K2_CLI" sessions archives --json 2>&1)" || die "sessions archives --json failed: $out"
[ "$out" = "[]" ] || die "sessions archives --json, new workspace: $out"
ok
mkdir -p "$T/caller-k2/.k2/sessions/s-9"
out="$(K2_PROJECT_PATH="$T/caller-k2" bash "$K2_CLI" sessions archives --json 2>&1)" || die "sessions archives --json failed: $out"
grep -q '"s-9"' <<<"$out" || die "sessions archives did not read .k2/sessions: $out"
ok
[ ! -e "$NEW/.k2" ] && [ ! -e "$NEW/.k2so" ] || die "reading made a folder in the new workspace"
ok

# prelude.sh, the Thread markdown nudge: it never makes the folder, and
# writes its counter in .k2 once that exists.
eval "$(sed -n '/^_thread_md_nudge()/,/^}/p' "$K2_CLI")"
[ "$(type -t _thread_md_nudge)" = function ] || die "no _thread_md_nudge in cli/k2"
K2_PROJECT_PATH="$NEW" _thread_md_nudge 2>/dev/null
[ ! -e "$NEW/.k2" ] && [ ! -e "$NEW/.k2so" ] || die "the nudge made a folder in a workspace with neither"
ok
mkdir "$NEW/.k2"
K2_PROJECT_PATH="$NEW" _thread_md_nudge 2>/dev/null
[ -f "$NEW/.k2/.thread-md-nudge" ] || die "the nudge did not write its counter under .k2"
[ ! -e "$NEW/.k2so" ] || die "the nudge made .k2so"
ok
rmdir "$NEW/.k2" 2>/dev/null || rm -rf "$NEW/.k2"

# zen-retired.sh, the garden-changes hint: same.
eval "$(sed -n '/^_zen_garden_changes_hint()/,/^}/p' "$K2_CLI")"
[ "$(type -t _zen_garden_changes_hint)" = function ] || die "no _zen_garden_changes_hint in cli/k2"
K2_HOME="$HOME/.k2"
K2_PROJECT_PATH="$NEW" _zen_garden_changes_hint 2>/dev/null
[ ! -e "$NEW/.k2" ] && [ ! -e "$NEW/.k2so" ] || die "the garden hint made a folder in a workspace with neither"
ok
mkdir "$NEW/.k2"
hint="$(K2_PROJECT_PATH="$NEW" _zen_garden_changes_hint 2>&1)"
[ -f "$NEW/.k2/.garden-changes-hint" ] || die "the garden hint did not write its mark under .k2 ($hint)"
[ ! -e "$NEW/.k2so" ] || die "the garden hint made .k2so"
ok

echo "ws_dot_dir: $pass passed"
