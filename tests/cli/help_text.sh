#!/usr/bin/env bash
# The CLI's static help lives in cli/text/help/ (scripts/build-cli.sh
# inlines each file into cli/k2 at its `%include` line). This test proves
# the CLI prints those files byte for byte, and, with --base, that every
# help entry point prints exactly what an older cli/k2 printed.
#
#   bash tests/cli/help_text.sh               the CLI prints the text files
#   bash tests/cli/help_text.sh --base REV    and every help entry point
#                                             matches REV's cli/k2 byte for
#                                             byte (stdout, stderr, exit)
#
# The file name is the entry point:
#   help/<w1>/.../<wn>.txt   is what `k2 <w1> ... <wn> --help` prints
#                            (help/mail/send.txt: `k2 mail send --help`),
#                            unless ENTRY names another command for it.
#   a name starting with _   is not one command's whole page: a tier of
#                            `k2 help` (help/_advanced.txt), or a piece bash
#                            puts together with other output
#                            (help/chat/_manage.txt). PAGES says which
#                            command prints which pieces, in order.
# A page whose heredoc is sent to stderr (`cat <<'EOF' >&2`) is checked on
# stderr with stdout empty; any other on stdout. A ` | _perm_lines` page
# goes through the permission card filter: with cards off (no daemon) a
# ` #if-cards` line is dropped and a ` #if-no-cards` line loses its tag.
#
# 1. Every file under cli/text/help is printed by the command its name or
#    ENTRY or PAGES gives, exactly.
# 2. With --base REV: `git show REV:cli/k2` runs beside the CLI under test
#    (CLI_UNDER_TEST). Every command above as written, and its words in
#    three forms (`<words> --help`, `<words> -h`, `help <words>`), plus
#    the tiers (`k2`, `k2 help`, `--help`, `-h`, `help --advanced`, ...):
#    the same stdout, the same stderr, the same exit. (`<words> help` is
#    not a help form for most verbs: `k2 domain remove help` would remove
#    a domain named help. It runs only where ENTRY names it.)
# 3. When the CLI under test is the k5 binary (K5_CLI_UNDER_TEST set):
#    every form in crates/k5-cli/src/help_native.txt (the forms it answers
#    itself, tests/cli/help_native.sh) prints what this checkout's bash
#    prints, with the binary's hand-over to bash broken: K5_HOME_OVERRIDE
#    set to a relative path, which bash never reads and the binary refuses
#    before it runs bash. So a pass is the binary's own answer. Forms bash
#    keeps (the tiers made of pieces, the card pages, pieces, a form that
#    is not exactly a row) must fail that way. So must any help with no
#    port or no token (the connection gate), and a family's help with
#    K2_HOOK_SOCK set and a CODEX_ setting (bash looks for a shared Codex
#    server). An agent's terminal (a passport, K2_PORT and K2_HOOK_SOCK a
#    live socket) gets every row from the binary too; with K2_HOOK_SOCK
#    set and no passport, an id tool's help is refused the same way
#    (exit 3) by both. (The dead-passport probe, with the socket gone,
#    needs a daemon: crates/k5-cli/tests/help_session_parity.rs.)
#
# No daemon: help is offline. Each run gets a scratch HOME and no K2_/K5_
# settings from the caller (env -i). The connection gate wants a port and a
# token before most verbs run, even for --help, so the scratch HOME holds a
# dead port (1) and a dummy owner token in .k2/heartbeat.{port,token}. It
# is an owner token on disk, not a session passport (K2_HOOK_TOKEN), on
# purpose: with a passport `k2 --help` and the App pages ask the daemon
# whether permission cards are on, and that call to a dead port waits for
# its retries. With none they know cards are off, as with no daemon.
# curl, launchctl, systemctl, sudo, security, open and k2 itself are stubs
# that log and fail: a help run that calls one fails the test. Never
# touches ~/.k2.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
NEW="$CLI_UNDER_TEST"
TEXT="$ROOT/cli/text/help"
SRC="$ROOT/cli/src"

BASE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --base) [ $# -ge 2 ] || { echo "help_text: --base needs a revision" >&2; exit 2; }; BASE="$2"; shift ;;
        *) echo "help_text: unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

# Pages printed by a command other than the one their name gives:
# <file under cli/text/help>|<argv>
ENTRY='
app.txt|app help
app/box-service.txt|app box-service help
app/doctor-store.txt|app doctor --store --help
app/door.txt|app door help
app/email.txt|app email help
app/grace.txt|app grace help
app/handover.txt|app handover help
app/surfaces.txt|app surfaces help
app/wallet.txt|app wallet help
read.txt|help read
skills.txt|help skills
sources/trigger.txt|sources trigger help
wiki/chat.txt|wiki chat help
wiki/serve.txt|wiki serve help
workspace/api-guest-policy.txt|workspace api-guest-policy help
workspace/api-skip-permissions.txt|workspace api-skip-permissions help
workspace/default-model.txt|workspace default-model help
workspace/force-model-on-resume.txt|workspace force-model-on-resume help
workspace/host-session-cell-cap.txt|workspace host-session-cell-cap help
'
# Commands that print pieces: <argv>|<piece> <piece> ... (in order; a
# trailing " 2>" on the argv: on stderr, stdout empty). "..." stands for
# output bash makes between the pieces (at most one per row):
#   the daily tier's permission line (cards on only) sits between _daily
#     and _daily-end; with cards off nothing does;
#   _retired_verbs_map lists the retired verbs inside the deprecated tier;
#   sources propose|add|edit|try --help print their own first line, then
#     the shared sources/_flags.txt;
#   an unknown `k2 wiki` subcommand prints its error line, then
#     wiki/_unknown.txt.
# chat/_manage.txt is left out only when _CHAT_HELP_MANAGE=0 is set;
# app/_runtime.txt is the App runtime page every
# `k2 app <runtime verb> --help` prints, and `k2 app --help` prints it
# after the App family page (app.txt).
PAGES='
--help|_daily.txt _daily-end.txt
help --advanced|_advanced.txt
help --internal|_internal.txt
help deprecated|_deprecated.txt ... _deprecated-end.txt
chat --help|chat/_head.txt chat/_manage.txt chat/_end.txt
sources propose --help|... sources/_flags.txt
app doctor --help|app/_runtime.txt
app --help 2>|app.txt app/_runtime.txt
wiki nope 2>|... wiki/_unknown.txt
'

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

SANDBOX="$(mktemp -d -t k2-help-text-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/home/.k2" "$SANDBOX/cwd" "$SANDBOX/out" "$SANDBOX/stub"
printf '1\n' > "$SANDBOX/home/.k2/heartbeat.port"
printf 'test-token\n' > "$SANDBOX/home/.k2/heartbeat.token"
for t in curl wget nc launchctl systemctl sudo security open osascript pkill killall ssh k2 k5; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/stub.log"\nexit 97\n' "$t" "$SANDBOX" > "$SANDBOX/stub/$t"
    chmod +x "$SANDBOX/stub/$t"
done

# run <prog> <out file> <args...>: stdout to the file, stderr to <out>.err,
# the exit code to <out>.code. A stub call is a failure.
# RUN_ENV holds settings to add (NAME=value words; a later HOME wins).
RUN_ENV=()
run() {
    local prog="$1" out="$2" code=0; shift 2
    : > "$SANDBOX/stub.log"
    (cd "$SANDBOX/cwd" && env -i HOME="$SANDBOX/home" PATH="$SANDBOX/stub:$PATH" LC_ALL=C \
        ${RUN_ENV[@]+"${RUN_ENV[@]}"} "$prog" "$@") > "$out" 2> "$out.err" < /dev/null || code=$?
    printf '%s\n' "$code" > "$out.code"
    if [ -s "$SANDBOX/stub.log" ]; then
        bad "k2 $* ran $(tr '\n' ';' < "$SANDBOX/stub.log")"
    fi
}

# The part line above `%include cli/text/help/<rel>`: how the page prints.
opener_of() {
    grep -h -B1 -x -F "%include cli/text/help/$1" "$SRC"/*.sh | head -1
}

# want_of <rel> <file>: the bytes the page prints (filtered when piped).
want_of() {
    if [[ "$(opener_of "$1")" == *"| _perm_lines"* ]]; then
        grep -q ' #if-cards$' "$TEXT/$1" || bad "$1 is piped through _perm_lines but has no #if-cards line"
        sed -e '/ #if-cards$/d' -e 's/ #if-no-cards$//' "$TEXT/$1" > "$2"
    else
        cat "$TEXT/$1" > "$2"
    fi
}

# check <label> <want file> <stderr: 1|0> <argv...>
check() {
    local label="$1" want="$2" to_err="$3" got="$SANDBOX/out/got"; shift 3
    run "$NEW" "$got" "$@"
    if [ "$to_err" = 1 ]; then
        if cmp -s "$got.err" "$want" && [ ! -s "$got" ]; then ok; else
            bad "$label: k2 $* did not print it on stderr (stdout empty)"
            diff "$want" "$got.err" | head -6 >&2 || true
        fi
    else
        if cmp -s "$got" "$want"; then ok; else
            bad "$label: k2 $* did not print it on stdout"
            diff "$want" "$got" | head -6 >&2 || true
        fi
    fi
}

echo "== the CLI prints cli/text/help =="
[ -d "$TEXT" ] || { echo "FAIL: no $TEXT" >&2; exit 1; }
files="$(cd "$TEXT" && find . -name '*.txt' | sed 's|^\./||' | sort)"
[ "$(printf '%s\n' "$files" | wc -l | tr -d ' ')" -gt 200 ] || { echo "FAIL: only these help files: $files" >&2; exit 1; }
covered=" "
cmds=""
# Whole pages: the name (or ENTRY) gives the command.
for rel in $files; do
    case "/$rel" in */_*) continue ;; esac
    argv="$(printf '%s\n' "$ENTRY" | awk -F'|' -v f="$rel" '$1 == f { print $2; exit }')"
    if [ -z "$argv" ]; then
        argv="$(printf '%s' "${rel%.txt}" | tr '/' ' ') --help"
    fi
    opener="$(opener_of "$rel")"
    [ -n "$opener" ] || { bad "$rel is not included by any part"; continue; }
    to_err=0
    case "$opener" in *">&2"*) to_err=1 ;; esac
    want_of "$rel" "$SANDBOX/out/want"
    # shellcheck disable=SC2086  # argv is words
    check "$rel" "$SANDBOX/out/want" "$to_err" $argv
    covered="$covered$rel "
    cmds="$cmds$argv
"
done
# Composed pages: the pieces in order.
while IFS='|' read -r argv pieces; do
    [ -n "$argv" ] || continue
    head_f="$SANDBOX/out/head"; tail_f="$SANDBOX/out/tail"
    : > "$head_f"; : > "$tail_f"
    into="$head_f"; gap=0
    for p in $pieces; do
        if [ "$p" = "..." ]; then into="$tail_f"; gap=1; continue; fi
        [ -f "$TEXT/$p" ] || { bad "PAGES names $p, which is not under cli/text/help"; continue; }
        want_of "$p" "$SANDBOX/out/piece"; cat "$SANDBOX/out/piece" >> "$into"
        covered="$covered$p "
    done
    to_err=0
    case "$argv" in *" 2>") to_err=1; argv="${argv% 2>}" ;; esac
    if [ "$gap" = 0 ]; then
        # shellcheck disable=SC2086
        check "$argv" "$head_f" "$to_err" $argv
        continue
    fi
    got="$SANDBOX/out/got"
    # shellcheck disable=SC2086
    run "$NEW" "$got" $argv
    stream="$got"
    if [ "$to_err" = 1 ]; then
        stream="$got.err"
        [ ! -s "$got" ] || bad "k2 $argv printed on stdout too"
    fi
    hb="$(wc -c < "$head_f" | tr -d ' ')"; tb="$(wc -c < "$tail_f" | tr -d ' ')"
    if { [ "$hb" = 0 ] || head -c "$hb" "$stream" | cmp -s - "$head_f"; } \
        && { [ "$tb" = 0 ] || tail -c "$tb" "$stream" | cmp -s - "$tail_f"; } \
        && [ "$(wc -c < "$stream" | tr -d ' ')" -gt "$((hb + tb))" ]; then
        ok
    else
        bad "k2 $argv does not start with its head pieces and end with its tail pieces"
    fi
done <<<"$PAGES"
for rel in $files; do
    case "$covered" in *" $rel "*) ;; *) bad "cli/text/help/$rel is printed by no command this test knows (name it in PAGES)" ;; esac
done
printf '%s\n' "$ENTRY" | while IFS='|' read -r rel _; do
    [ -z "$rel" ] || [ -f "$TEXT/$rel" ] || { echo "  FAIL: ENTRY names $rel, which is not under cli/text/help" >&2; exit 1; }
done || fail=$((fail + 1))
echo "  $pass checks passed"

if [ -n "$BASE" ]; then
    echo "== every help entry point matches $BASE's cli/k2 =="
    OLD="$SANDBOX/old-k2"
    git -C "$ROOT" show "$BASE:cli/k2" > "$OLD" || { echo "FAIL: git show $BASE:cli/k2" >&2; exit 1; }
    chmod +x "$OLD"
    before=$pass
    both() {
        run "$OLD" "$SANDBOX/out/old" "$@"
        run "$NEW" "$SANDBOX/out/new" "$@"
        if cmp -s "$SANDBOX/out/old" "$SANDBOX/out/new" && cmp -s "$SANDBOX/out/old.code" "$SANDBOX/out/new.code" \
            && cmp -s "$SANDBOX/out/old.err" "$SANDBOX/out/new.err"; then
            ok
        else
            bad "k2 $* differs from $BASE"
            diff "$SANDBOX/out/old" "$SANDBOX/out/new" | head -6 >&2 || true
            diff "$SANDBOX/out/old.err" "$SANDBOX/out/new.err" | head -6 >&2 || true
        fi
    }
    both
    both help
    both --help
    both -h
    for t in --advanced advanced -a --internal internal --deprecated deprecated nope; do both help "$t"; done
    both help-advanced
    exact="$(printf '%s%s\n' "$cmds" "$(printf '%s\n' "$PAGES" | cut -d'|' -f1 | sed 's/ 2>$//')" | sed '/^$/d')"
    # A `help <verb>` entry runs as written only: `k2 read --help` is not
    # a help form (it reads the terminal of a workspace named --help).
    words="$(printf '%s\n' "$exact" | grep -v '^help ' | sed -E 's/ (--help|-h|help)$//; s/^--help$//')"
    all="$( {
        printf '%s\n' "$exact"
        printf '%s\n' "$words" | sed '/^$/d' | while IFS= read -r w; do
            printf '%s --help\n%s -h\nhelp %s\n' "$w" "$w" "$w"
        done
    } | sort -u)"
    while IFS= read -r cmd; do
        [ -n "$cmd" ] || continue
        # shellcheck disable=SC2086
        both $cmd
    done <<<"$all"
    echo "  $((pass - before)) entry points match"
fi

if [ "$NEW" != "$CLI_BASH_SOURCE" ]; then
    echo "== the binary answers the help forms itself =="
    LIST="$ROOT/crates/k5-cli/src/help_native.txt"
    BROKEN="K5_HOME_OVERRIDE=not/absolute"
    before=$pass
    # native <settings words> -- <args...>: the binary with its hand-over
    # broken prints what the bash prints, both with the settings.
    native() {
        local -a set=()
        while [ "$1" != "--" ]; do set+=("$1"); shift; done
        shift
        RUN_ENV=(${set[@]+"${set[@]}"})
        run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" "$@"
        RUN_ENV=(${set[@]+"${set[@]}"} "$BROKEN")
        run "$NEW" "$SANDBOX/out/bin" "$@"
        RUN_ENV=()
        if cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
            && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code"; then
            ok
        else
            bad "k2 $* (${set[*]:-}): the binary's own answer differs from bash"
            diff "$SANDBOX/out/bash" "$SANDBOX/out/bin" | head -6 >&2 || true
            diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
        fi
    }
    # handed <settings words> -- <args...>: the binary hands it to bash, so
    # with the hand-over broken it fails, saying why, with stdout empty.
    handed() {
        local -a set=()
        while [ "$1" != "--" ]; do set+=("$1"); shift; done
        shift
        RUN_ENV=(${set[@]+"${set[@]}"} "$BROKEN")
        run "$NEW" "$SANDBOX/out/bin" "$@"
        RUN_ENV=()
        if [ "$(cat "$SANDBOX/out/bin.code")" != 0 ] && [ ! -s "$SANDBOX/out/bin" ] \
            && grep -q K5_HOME_OVERRIDE "$SANDBOX/out/bin.err"; then
            ok
        else
            bad "k2 $* (${set[*]:-}) did not go to bash (exit $(cat "$SANDBOX/out/bin.code"))"
        fi
    }
    rows=0
    while IFS='|' read -r argv _; do
        case "$argv" in ''|'#'*) continue ;; esac
        rows=$((rows + 1))
        # shellcheck disable=SC2086  # argv is words
        native -- $argv
    done < "$LIST"
    [ "$rows" -gt 300 ] || bad "only $rows rows in $LIST"
    for cmd in "" "help" "--help" "-h" "help deprecated" "help nope" "app --help" "chat --help" \
        "mail send x --help" "help mail send" "zen --help" "wiki nope" "mail --help x"; do
        # shellcheck disable=SC2086
        handed -- $cmd
    done
    SOCK="K2_HOOK_SOCK=$SANDBOX/no.sock"
    # No passport: an id tool's help is refused, an open one prints.
    native "$SOCK" -- mail send --help
    native "$SOCK" -- vault --help
    native "$SOCK" -- agent harness --help
    native "$SOCK" -- agent --help
    native "$SOCK" -- help mail
    native "$SOCK" -- help-advanced
    handed "$SOCK" "CODEX_THREAD_ID=t1" -- mail send --help
    native "$SOCK" "CODEX_THREAD_ID=t1" -- agent --help
    # An agent's terminal: a passport and its live socket (a bound socket
    # file; nothing needs to listen, the checks only test that it is one).
    python3 -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$SANDBOX/live.sock"
    [ -S "$SANDBOX/live.sock" ] || bad "no socket at $SANDBOX/live.sock"
    AGENT=("K2_PORT=1" "K2_HOOK_TOKEN=sessT.notreal1" "K2_HOOK_SOCK=$SANDBOX/live.sock")
    agent_rows=0
    while IFS='|' read -r argv _; do
        case "$argv" in ''|'#'*) continue ;; esac
        agent_rows=$((agent_rows + 1))
        # shellcheck disable=SC2086  # argv is words
        native "${AGENT[@]}" -- $argv
    done < "$LIST"
    [ "$agent_rows" = "$rows" ] || bad "$agent_rows rows in an agent's terminal, $rows in all"
    EMPTY="HOME=$SANDBOX/empty-home"
    mkdir -p "$SANDBOX/empty-home"
    handed "$EMPTY" -- help mail
    handed "$EMPTY" -- mail send --help
    handed "$EMPTY" "K2_PORT=1" -- help mail
    handed "$EMPTY" "K2_HOOK_TOKEN=sessT.notreal1" -- help mail
    native "$EMPTY" "K2_PORT=1" "K2_HOOK_TOKEN=sessT.notreal1" -- help mail
    native "$EMPTY" "K2_PORT=1" "K2_HOOK_TOKEN=sessT.notreal1" -- mail send --help
    handed "SHELLOPTS=xtrace" -- mail send --help
    echo "  $((pass - before)) binary checks passed ($rows forms, each also in an agent's terminal)"
fi

echo ""
echo "help_text: $pass passed, $fail failed"
[ "$fail" = 0 ]
