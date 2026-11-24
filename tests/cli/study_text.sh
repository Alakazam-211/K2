#!/usr/bin/env bash
# The `k2 study` pages live in cli/text/study/<id>.txt (scripts/build-cli.sh
# inlines them into cli/k2). This test proves the CLI prints those files
# byte for byte, and, with --base, that it prints exactly what an older
# cli/k2 printed.
#
#   bash tests/cli/study_text.sh               the CLI prints the text files
#   bash tests/cli/study_text.sh --base REV    and every page, the list, the
#                                              JSON and the usage match
#                                              REV's cli/k2 byte for byte
#
# 1. For every topic `k2 study --json` lists: `k2 study <id>` is
#    cli/text/study/<id>.txt exactly, or the topic is one of the pages still
#    written in bash (BASH_PAGES, each with its reason). Two alias topics
#    print a heads-up line first. The apps page goes through the permission
#    card filter: with cards off (no daemon, no token) a ` #if-cards` line
#    is dropped and a ` #if-no-cards` line loses its tag. `k2 study` is
#    _catalog.txt and `k2 study --help` is _usage.txt. Every text file is
#    a topic's page, one of those two, or the cards-only page (CARDS_ONLY).
# 2. With --base REV: `git show REV:cli/k2` runs beside the CLI under test
#    (CLI_UNDER_TEST). Every topic of either, each page as text and as
#    JSON, the list, the list as JSON and the usage: same bytes, same exit.
# 3. When the CLI under test is the k5 binary (K5_CLI_UNDER_TEST set): the
#    forms it answers itself (the list, the usage, every page as text and
#    as JSON, the aliases) print what this checkout's bash prints, with
#    the binary's hand-over to bash broken: K5_HOME_OVERRIDE set to a
#    relative path, which bash never reads and the binary refuses before
#    it runs bash. So a pass is the binary's own answer. The forms bash
#    keeps (an unknown topic, the bash pages, errors but the cards page's
#    refusal without a passport) must fail that way, never print a page.
# 4. Also with the binary: with a passport, the pages that ask the daemon
#    whether permission cards are on (the list, the usage, apps, skins,
#    permissions), against a stub daemon of this script's own that says
#    on, or off, on a cell socket, and on over TCP. Each one pages.rs
#    answers itself (its switches, mirrored in CARDS_* below) prints what
#    bash prints in the same environment, and both ask exactly once; each
#    one it leaves to bash must go there.
#
# No daemon but that stub: study pages are offline. Each run gets an empty
# HOME and no K2_/K5_ settings (env -i), in a scratch folder. Never
# touches ~/.k2.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
NEW="$CLI_UNDER_TEST"
TEXT="$ROOT/cli/text/study"

BASE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --base) [ $# -ge 2 ] || { echo "study_text: --base needs a revision" >&2; exit 2; }; BASE="$2"; shift ;;
        *) echo "study_text: unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

# Pages still written in bash, and why.
BASH_PAGES="app-api app-errors live"
#   app-api, app-errors, live: contract-gen writes them into their part
#   (fenced `# >>> generated` blocks), so they move with contract-gen.
# A page only shown while permission cards are on. That needs a daemon, so
# this test can't print it; tests/cli/permission_teach.sh does, on a stub.
CARDS_ONLY="permissions"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

SANDBOX="$(mktemp -d -t k2-study-text-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/home" "$SANDBOX/cwd" "$SANDBOX/out"

# run <prog> <out file> <args...>: the program's stdout to the file, its
# exit code as the file's last line in <out file>.code. RUN_ENV holds
# settings to add (NAME=value words).
RUN_ENV=()
run() {
    local prog="$1" out="$2" code=0; shift 2
    (cd "$SANDBOX/cwd" && env -i HOME="$SANDBOX/home" PATH="$PATH" LC_ALL=C ${RUN_ENV[@]+"${RUN_ENV[@]}"} "$prog" "$@") \
        > "$out" 2> "$out.err" || code=$?
    printf '%s\n' "$code" > "$out.code"
}

# topics <prog>: the topic ids, one per line, from `k2 study --json`.
topics() {
    run "$1" "$SANDBOX/out/topics.json" study --json
    [ "$(cat "$SANDBOX/out/topics.json.code")" = 0 ] || { echo "FAIL: $1 study --json exited $(cat "$SANDBOX/out/topics.json.code")" >&2; exit 1; }
    python3 -c '
import json, sys
for t in json.load(open(sys.argv[1]))["topics"]:
    print(t["id"])
' "$SANDBOX/out/topics.json"
}

# same <label> <got file> <want file>: byte for byte, or the first diff lines.
same() {
    if cmp -s "$2" "$3"; then
        ok "$1"
    else
        bad "$1 differs:"
        diff "$3" "$2" | head -8 >&2 || true
    fi
}

echo "== the CLI prints cli/text/study =="
ids="$(topics "$NEW")"
[ "$(printf '%s\n' "$ids" | wc -l | tr -d ' ')" -gt 50 ] || { echo "FAIL: only these topics: $ids" >&2; exit 1; }
seen=" _catalog _usage $CARDS_ONLY "
for id in $ids; do
    got="$SANDBOX/out/page"
    run "$NEW" "$got" study "$id"
    [ "$(cat "$got.code")" = 0 ] || { bad "k2 study $id exited $(cat "$got.code")"; continue; }
    page="$id"
    case "$id" in skins) page=apps ;; llm-accounts) page=llm-tokens ;; esac
    file="$TEXT/$page.txt"
    if [ ! -f "$file" ]; then
        case " $BASH_PAGES " in
            *" $id "*) ok "$id is still written in bash (listed)" ;;
            *) bad "$id has no cli/text/study/$page.txt and is not in BASH_PAGES" ;;
        esac
        continue
    fi
    seen="$seen$page "
    want="$SANDBOX/out/want"
    case "$id" in
        skins) { printf '%s\n\n' "k2 study skins is now k2 study apps — this page still works."; cli_page "$file"; } > "$want" ;;
        llm-accounts) { printf '%s\n\n' "k2 study llm-accounts is now k2 study llm-tokens — this page still works."; cli_page "$file"; } > "$want" ;;
        *) cli_page "$file" > "$want" ;;
    esac
    if [ "$page" = apps ]; then
        # Cards off: drop ` #if-cards` lines, untag ` #if-no-cards` lines.
        grep -q ' #if-cards$' "$want" || bad "the apps text has no #if-cards line (the filter check checks nothing)"
        sed -e '/ #if-cards$/d' -e 's/ #if-no-cards$//' "$want" > "$want.off"
        mv "$want.off" "$want"
    fi
    same "k2 study $id is cli/text/study/$page.txt" "$got" "$want"
done
for id in $BASH_PAGES; do
    grep -qx -- "$id" <<<"$ids" || bad "BASH_PAGES names $id, which is not a topic"
done
run "$NEW" "$SANDBOX/out/list" study
cli_page "$TEXT/_catalog.txt" > "$SANDBOX/out/catalog.want"
same "k2 study is cli/text/study/_catalog.txt" "$SANDBOX/out/list" "$SANDBOX/out/catalog.want"
run "$NEW" "$SANDBOX/out/usage" study --help
cli_page "$TEXT/_usage.txt" > "$SANDBOX/out/usage.want"
same "k2 study --help is cli/text/study/_usage.txt" "$SANDBOX/out/usage" "$SANDBOX/out/usage.want"
for f in "$TEXT"/*.txt; do
    name="$(basename "$f" .txt)"
    case "$seen" in *" $name "*) ;; *) bad "cli/text/study/$name.txt is not a page any topic prints" ;; esac
done
ok "every cli/text/study file is a topic's page, the catalog or the usage"

if [ -n "$BASE" ]; then
    echo "== every study output matches $BASE's cli/k2 =="
    OLD="$SANDBOX/old-k2"
    git -C "$ROOT" show "$BASE:cli/k2" > "$OLD" || { echo "FAIL: git show $BASE:cli/k2" >&2; exit 1; }
    chmod +x "$OLD"
    old_ids="$(topics "$OLD")"
    if [ "$old_ids" = "$ids" ]; then ok "the same topics, in the same order ($(printf '%s\n' "$ids" | wc -l | tr -d ' '))"; else bad "the topic lists differ"; fi
    both() {
        local label="$1"; shift
        run "$OLD" "$SANDBOX/out/old" "$@"
        run "$NEW" "$SANDBOX/out/new" "$@"
        if cmp -s "$SANDBOX/out/old" "$SANDBOX/out/new" && cmp -s "$SANDBOX/out/old.code" "$SANDBOX/out/new.code" \
            && cmp -s "$SANDBOX/out/old.err" "$SANDBOX/out/new.err"; then
            ok "$label"
        else
            bad "$label differs from $BASE"
            diff "$SANDBOX/out/old" "$SANDBOX/out/new" | head -8 >&2 || true
        fi
    }
    both "k2 study" study
    both "k2 study list" study list
    both "k2 study --json" study --json
    both "k2 study --help" study --help
    both "k2 study nope (the refusal)" study nope
    for id in $(printf '%s\n%s\n' "$ids" "$old_ids" | sort -u); do
        both "k2 study $id" study "$id"
        both "k2 study $id --json" study "$id" --json
    done
fi

if [ "$NEW" != "$CLI_BASH_SOURCE" ]; then
    echo "== the binary answers the study pages itself =="
    BROKEN="K5_HOME_OVERRIDE=not/absolute"
    # native <args...>: the binary with its hand-over broken prints what
    # the bash prints.
    native() {
        RUN_ENV=()
        run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" study "$@"
        RUN_ENV=("$BROKEN")
        run "$NEW" "$SANDBOX/out/bin" study "$@"
        RUN_ENV=()
        if cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
            && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code"; then
            ok "the binary answers k2 study $*"
        else
            bad "k2 study $*: the binary's own answer differs from bash"
            diff "$SANDBOX/out/bash" "$SANDBOX/out/bin" | head -6 >&2 || true
            diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
        fi
    }
    # handed <args...>: the binary hands the form to bash (so with the
    # hand-over broken it fails, saying why, and prints nothing on stdout).
    handed() {
        RUN_ENV=("$BROKEN")
        run "$NEW" "$SANDBOX/out/bin" study "$@"
        RUN_ENV=()
        if [ "$(cat "$SANDBOX/out/bin.code")" != 0 ] && [ ! -s "$SANDBOX/out/bin" ] \
            && grep -q K5_HOME_OVERRIDE "$SANDBOX/out/bin.err"; then
            ok "k2 study $* goes to bash"
        else
            bad "k2 study $* did not go to bash (exit $(cat "$SANDBOX/out/bin.code"))"
        fi
    }
    native
    native list
    native --help
    native -h
    native errors --help
    native --json -h
    native "" errors
    for a in routines routine reminders reminder cron; do native "$a"; native "$a" --json; done
    for id in $ids; do
        case " $BASH_PAGES " in *" $id "*) handed "$id"; handed "$id" --json; continue ;; esac
        native "$id"
        native "$id" --json
    done
    native --json
    native list --json
    handed nope
    # The cards-only topic with no passport: cards are off without asking,
    # so it is the refusal and the usage, which the binary prints itself.
    native "$CARDS_ONLY"
    native "$CARDS_ONLY" --json
    handed --bogus
    # A second topic: the refusal and the usage.
    native errors db
    native errors db --json
    # A stray -- (study takes none): the binary's own refusal, the shell's
    # line and exit 2, nothing sent (the front's _k2_takes_dashdash rule).
    native -- errors
    # A passport, and a page that asks nothing: the binary's own page.
    RUN_ENV=("K2_HOOK_TOKEN=sessT.notreal1" "$BROKEN")
    run "$NEW" "$SANDBOX/out/bin" study errors
    RUN_ENV=()
    if [ "$(cat "$SANDBOX/out/bin.code")" = 0 ] && cmp -s "$SANDBOX/out/bin" <(cli_page "$TEXT/errors.txt"); then
        ok "with a passport the binary answers k2 study errors"
    else
        bad "with a passport k2 study errors was not the binary's page"
    fi

    echo "== with a passport, the pages that ask whether permission cards are on =="
    # Which of them the binary answers itself: pages.rs's switches.
    CARDS_LIST=native
    #   k2 study, study list, study --help, study -h (LIST_WITH_PASSPORT)
    #
    #
    #
    CARDS_APPS=native
    #   study apps and study skins, text and --json (APPS_WITH_PASSPORT)
    #
    #
    #
    CARDS_PERMISSIONS=native
    #   study permissions, text and --json (PERMISSIONS_WITH_PASSPORT)
    #
    # The stub: cards off on one cell socket, on on another and over TCP.
    # One line in its log per request.
    STUB_LOG="$SANDBOX/cards.log"
    : > "$STUB_LOG"
    python3 -I - "$SANDBOX" "$STUB_LOG" <<'PY' &
import http.server, json, os, socketserver, sys, threading
box, log = sys.argv[1], sys.argv[2]
lock = threading.Lock()
def handler(name, on):
    class H(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def do_GET(self):
            with lock, open(log, "a") as f:
                f.write("%s %s\n" % (name, self.path.split("?", 1)[0]))
            if self.path.split("?", 1)[0] == "/cli/permission/enabled":
                status, body = 200, json.dumps({"enabled": on}).encode()
            else:
                status, body = 404, b'{"error":"not found"}'
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True
        do_POST = do_GET
        def address_string(self):
            return name
        def log_message(self, *a):
            pass
    return H
class Unix(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
servers = [Unix(os.path.join(box, "cards-off.sock"), handler("off", False)),
           Unix(os.path.join(box, "cards-on.sock"), handler("on", True)),
           http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler("tcp", True))]
for srv in servers:
    threading.Thread(target=srv.serve_forever, daemon=True).start()
with open(os.path.join(box, "stub.port.tmp"), "w") as f:
    f.write(str(servers[2].server_address[1]))
os.rename(os.path.join(box, "stub.port.tmp"), os.path.join(box, "stub.port"))
threading.Event().wait()
PY
    STUB_PID=$!
    trap 'kill "$STUB_PID" 2>/dev/null || true; rm -rf "$SANDBOX"' EXIT
    for _ in $(seq 100); do [ -s "$SANDBOX/stub.port" ] && break; sleep 0.1; done
    [ -s "$SANDBOX/stub.port" ] || { echo "FAIL: the cards stub did not start" >&2; exit 1; }
    STUB_PORT="$(cat "$SANDBOX/stub.port")"
    requests() { wc -l < "$STUB_LOG" | tr -d ' '; }
    # cards_form <switch> <label> <settings...> -- <study args...>
    cards_form() {
        local switch="$1" label="$2" a b n0 n1 n2; shift 2
        local -a set=()
        while [ "$1" != "--" ]; do set+=("$1"); shift; done
        shift
        n0="$(requests)"
        RUN_ENV=("K2_HOOK_TOKEN=sessT.notreal1" ${set[@]+"${set[@]}"})
        run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" study "$@"
        n1="$(requests)"
        RUN_ENV+=("$BROKEN")
        run "$NEW" "$SANDBOX/out/bin" study "$@"
        n2="$(requests)"
        RUN_ENV=()
        a=$((n1 - n0)); b=$((n2 - n1))
        if [ "$switch" = native ]; then
            if cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
                && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code" && [ "$a" = 1 ] && [ "$b" = 1 ]; then
                ok "with a passport ($label) the binary answers k2 study $*, one request each"
            else
                bad "with a passport ($label) k2 study $*: the binary differs from bash (requests: bash $a, binary $b)"
                diff "$SANDBOX/out/bash" "$SANDBOX/out/bin" | head -6 >&2 || true
                diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
            fi
        else
            if [ "$(cat "$SANDBOX/out/bin.code")" != 0 ] && [ ! -s "$SANDBOX/out/bin" ] \
                && grep -q K5_HOME_OVERRIDE "$SANDBOX/out/bin.err" && [ "$b" = 0 ] && [ "$a" = 1 ]; then
                ok "with a passport ($label) k2 study $* goes to bash"
            else
                bad "with a passport ($label) k2 study $* did not go to bash (exit $(cat "$SANDBOX/out/bin.code"), requests: bash $a, binary $b)"
            fi
        fi
    }
    for where in off on tcp; do
        case "$where" in
            tcp) at=("K2_PORT=$STUB_PORT") ;;
            *) at=("K2_HOOK_SOCK=$SANDBOX/cards-$where.sock" "K2_PORT=1") ;;
        esac
        for f in "" list --help -h "errors --help"; do
            # shellcheck disable=SC2086  # f is words
            cards_form "$CARDS_LIST" "$where" "${at[@]}" -- $f
        done
        for f in apps "apps --json" skins "--json skins"; do
            # shellcheck disable=SC2086
            cards_form "$CARDS_APPS" "$where" "${at[@]}" -- $f
        done
        for f in permissions "permissions --json"; do
            # shellcheck disable=SC2086
            cards_form "$CARDS_PERMISSIONS" "$where" "${at[@]}" -- $f
        done
    done
    # The pages differ with cards on: the stub is heard.
    RUN_ENV=("K2_HOOK_TOKEN=sessT.notreal1" "K2_HOOK_SOCK=$SANDBOX/cards-on.sock")
    run "$CLI_BASH_SOURCE" "$SANDBOX/out/on" study list
    RUN_ENV=("K2_HOOK_TOKEN=sessT.notreal1" "K2_HOOK_SOCK=$SANDBOX/cards-off.sock")
    run "$CLI_BASH_SOURCE" "$SANDBOX/out/off" study list
    RUN_ENV=()
    if grep -q '^  permissions ' "$SANDBOX/out/on" && ! grep -q '^  permissions ' "$SANDBOX/out/off"; then
        ok "the stub says on on one socket and off on the other"
    else
        bad "the stub's on and off read the same to bash"
    fi
    # No passport: nothing is asked, and the pages are the binary's.
    n0="$(requests)"
    RUN_ENV=("K2_HOOK_SOCK=$SANDBOX/cards-on.sock" "$BROKEN")
    run "$NEW" "$SANDBOX/out/bin" study list
    RUN_ENV=()
    if [ "$(requests)" = "$n0" ] && cmp -s "$SANDBOX/out/bin" <(cli_page "$TEXT/_catalog.txt"); then
        ok "no passport: the binary's own list, nothing asked"
    else
        bad "no passport: k2 study list asked the stub or was not the plain list"
    fi
fi

echo ""
echo "study_text: $pass passed, $fail failed"
[ "$fail" = 0 ]
