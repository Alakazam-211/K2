#!/usr/bin/env bash
# Retired verbs answer from one table (cli/k2 BEGIN_RETIRED_VERBS): every
# row prints its one-line pointer and exits 1, `k2 help-deprecated` lists
# every row, and a verb that was never a K2 verb still gets the normal
# "Unknown command" answer. Old hand-written AGENT.md / SKILL.md text
# leans on these pointers, so a row that stops answering fails here.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

OUT="$(mktemp -t k2-retired-out-XXXXXX)"
ERR="$(mktemp -t k2-retired-err-XXXXXX)"
MAP="$(mktemp -t k2-retired-map-XXXXXX)"
trap 'rm -f "$OUT" "$ERR" "$MAP"; hermetic_cli_cleanup' EXIT

fails=0
bad() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

# The table itself, straight from cli/k2.
block="$(sed -n '/^# BEGIN_RETIRED_VERBS$/,/^# END_RETIRED_VERBS$/p' "$K2_CLI")"
[ -n "$block" ] || { echo "FAIL: no BEGIN/END_RETIRED_VERBS block in cli/k2" >&2; exit 1; }
eval "$block"
[ "${#_K2_RETIRED_VERBS[@]}" -gt 0 ] || { echo "FAIL: the retired-verb table is empty" >&2; exit 1; }

"$K2_CLI" help-deprecated >"$MAP" 2>&1 || { echo "FAIL: help-deprecated exited non-zero" >&2; cat "$MAP" >&2; exit 1; }

# Every row: exit 1, nothing on stdout, exactly its one pointer line on
# stderr, and its line in the help-deprecated map.
for row in "${_K2_RETIRED_VERBS[@]}"; do
    key="${row%%|*}"
    new="${row#*|}"
    case "$new" in
        "("*) want="Error: '$key' is retired $new. Run 'k2 help-deprecated' for the full map." ;;
        *)    want="Error: '$key' is retired. Use instead: $new" ;;
    esac
    # shellcheck disable=SC2086  # the key is one or two words on purpose
    set +e; "$K2_CLI" $key some-arg >"$OUT" 2>"$ERR"; rc=$?; set -e
    [ "$rc" = 1 ] || bad "'$key' exited $rc, want 1"
    [ ! -s "$OUT" ] || bad "'$key' printed on stdout: $(head -3 "$OUT")"
    [ "$(cat "$ERR")" = "$want" ] || bad "'$key' said: $(head -3 "$ERR") | want: $want"
    line="$(printf '  %-31s → %s' "$key" "$new")"
    grep -qxF -- "$line" "$MAP" || bad "help-deprecated has no line: $line"
done

# The pointers K2 named for the text this branch moved.
pin() {
    local key="$1" want="$2" row found=""
    for row in "${_K2_RETIRED_VERBS[@]}"; do
        [ "${row%%|*}" = "$key" ] && found="${row#*|}"
    done
    case "$found" in
        "$want"*) ;;
        *) bad "'$key' points at '${found:-nothing}', want '$want...'" ;;
    esac
}
pin "work move"      "k2 agent complete --agent"
pin "work create"    "k2 inbox compose"
pin "work send"      "k2 msg <workspace> --inbox-silent"
pin "agents create"  "k2 skills create <name>"
pin "agents running" "k2 workspace list --running"
pin "agent create"   "k2 skills create <name>"
pin "agent delete"   "k2 agent retire"
pin "delegate"       "your harness's sub-agent feature, with k2 skills profile"
pin "mode"           "k2 settings --mode custom"
pin "who"            "k2 connections list"
pin "whatsnew"       "k2 whats-new"
pin "commit-merge"   "k2 commit --merge"

# A verb that never existed still gets the normal answers.
set +e; "$K2_CLI" frobnicate >"$OUT" 2>"$ERR"; rc=$?; set -e
[ "$rc" = 1 ] || bad "'frobnicate' exited $rc, want 1"
[ "$(head -1 "$ERR")" = "Unknown command: frobnicate" ] || bad "'frobnicate' said: $(head -1 "$ERR")"
set +e; "$K2_CLI" agent frobnicate >"$OUT" 2>"$ERR"; rc=$?; set -e
[ "$rc" = 2 ] || bad "'agent frobnicate' exited $rc, want 2"
[ "$(head -1 "$ERR")" = "Unknown agent subcommand: frobnicate" ] || bad "'agent frobnicate' said: $(head -1 "$ERR")"
set +e; "$K2_CLI" work frobnicate >"$OUT" 2>"$ERR"; rc=$?; set -e
[ "$(head -1 "$ERR")" = "Unknown command: work" ] || bad "'work frobnicate' said: $(head -1 "$ERR")"

if [ "$fails" -gt 0 ]; then
    echo "retired_verbs_table: $fails failure(s)" >&2
    exit 1
fi
echo "OK: ${#_K2_RETIRED_VERBS[@]} retired verbs answer with their pointer; unknown verbs still say Unknown"
