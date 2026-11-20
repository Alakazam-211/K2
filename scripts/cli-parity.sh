#!/usr/bin/env bash
# cli-parity.sh: the R1 gate, first form. Every CLI test passes on the
# k5-cli binary as it does on the bash CLI.
#
#   bash scripts/cli-parity.sh <path to a built k5-cli> [--shard I/N] [-- test.sh ...]
#
# Makes a temp folder with `k2` and `k5` links to the binary (so its
# argv[0] is a real name) and runs each tests/cli/*.sh twice: plain (the
# bash CLI, cli/k2) and with K5_CLI_UNDER_TEST=<temp>/k2 (the binary;
# tests/cli/_cli_under_test.sh hands it to the test). Skipped: helpers
# (a name starting with `_`) and tests marked `# bash-only` (they read
# cli/k2 as source, which a binary can't stand in for).
#
# Each run gets a fresh temp HOME, no K2_*, K5_* or K2SO_* variable but
# K5_CLI_UNDER_TEST, stdin from /dev/null, and at most
# CLI_PARITY_TIMEOUT seconds (default 900) when `timeout` is there.
#
# A test that passes on bash and fails on the binary fails the gate: its
# name and the tail of its output are printed. One that fails on both is
# listed as "fails on both" and does not fail the gate; nor does one that
# fails on bash only (listed too: it points at a flaky test).
#
# Named expected differences: where the binary answers otherwise than the
# bash on purpose, by a PRD ruling. They are not skips: before any test
# runs, each is checked to hold exactly as written (both sides), and the
# gate fails, saying which one no longer holds, when either side changes.
# One that holds only on some hosts proves itself where it holds and,
# elsewhere, checks what it can and prints "NOT PROVEN on this host" with
# where it is proven (bash32-pipe-reader: the macOS job, on /bin/bash 3.2).
# Three tests/cli tests meet one through the binary, and check the binary's
# side of it exactly, by the difference's name, never by a skip:
# airgap_no_python.sh (native-without-python3: no python3 on PATH, the
# one line and exit 69) and socket_first_no_owner.sh
# (owner-token-with-passport: `read` when the socket says "not served"),
# and socket_first_no_owner_headless.sh (the same difference: `feedback
# list` over TCP from a real session, refused for the passport).
# Any other is mapped to its difference the same way. (The
# binary's own parity tests meet owner-token-with-passport:
# crates/k5-cli/tests/whoami_parity.rs, done_parity.rs, activity_parity.rs
# and feedback_parity.rs; since 0.46.17 the shell's read and setup send
# the passport there too, so read_parity.rs and setup_parity.rs no longer
# meet it. They meet
# native-without-python3 in every case of the parity rig, in
# whoami_parity.rs, pages_without_python3.rs and the study cards tests;
# tests/cli runs with python3 on PATH. They meet bash32-pipe-reader in the
# no-answer cases of inbox_parity.rs, read_parity.rs, activity_parity.rs,
# connections_write_parity.rs, connect_parity.rs, arms_parity.rs (reviews),
# remind_parity.rs, review_skills_whats_new_parity.rs (skills list),
# workspace_verbs_parity.rs and wiki_parity.rs, through the rig's
# Compare::Bash32PipeReader on macOS. They meet python-version-error-text
# wherever the shell's Python words an error otherwise than 3.12: the rig
# takes it for every row (common/parity.rs, as_this_python), and
# compute_render_oracle.rs for a dict sliced. They meet nul-in-answer in
# feedback_parity.rs's "a NUL in a string" rows of list and show, through
# Compare::NulInAnswer. They meet feedback-optin-without-python3 in
# help_feedback_optin_parity.rs: every row's run with no python3 prints
# what the binary prints with one, and its no-python3 test checks the
# rows that answer otherwise there. They meet wiki-status-large-answer in
# wiki_parity.rs's wiki_status_with_a_large_answer.)
# garden_blobs.sh meets expect-100-continue against a real daemon: its
# one-chunk put of a body over 1 MiB goes through a relay that records each
# request, and the test checks each side's head by the difference's name.
#
# --shard I/N runs every Nth test starting at the Ith (1-based), so the
# list can be split across jobs. Names after `--` run only those tests.
#
# Exit 0 when nothing fails on the binary only, 1 when something does or
# a named difference no longer holds, 2 on a usage or setup error.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
usage() { echo "usage: bash scripts/cli-parity.sh <k5-cli> [--shard I/N] [-- test.sh ...]" >&2; exit 2; }

[ $# -ge 1 ] || usage
BIN="$1"; shift
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
[ -f "$BIN" ] && [ -x "$BIN" ] || { echo "cli-parity: not an executable file: $BIN" >&2; exit 2; }

SHARD=1 SHARDS=1
ONLY=()
while [ $# -gt 0 ]; do
    case "$1" in
        --shard)
            [ $# -ge 2 ] || usage
            case "$2" in
                [0-9]*/[0-9]*) SHARD="${2%/*}"; SHARDS="${2#*/}" ;;
                *) usage ;;
            esac
            [ "$SHARDS" -ge 1 ] && [ "$SHARD" -ge 1 ] && [ "$SHARD" -le "$SHARDS" ] || usage
            shift 2
            ;;
        --) shift; ONLY=("$@"); break ;;
        *) usage ;;
    esac
done

TIMEOUT_SECS="${CLI_PARITY_TIMEOUT:-900}"
TIMEOUT=()
if command -v timeout >/dev/null 2>&1; then TIMEOUT=(timeout "$TIMEOUT_SECS"); fi

WORK="$(mktemp -d -t k5-cli-parity-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/out"
ln -s "$BIN" "$WORK/bin/k2"
ln -s "$BIN" "$WORK/bin/k5"

# The tests, in a stable order.
TESTS=()
skipped_bash_only=0
if [ ${#ONLY[@]} -gt 0 ]; then
    for t in "${ONLY[@]}"; do
        t="${t##*/}"
        [ -f "$ROOT/tests/cli/$t" ] || { echo "cli-parity: no test tests/cli/$t" >&2; exit 2; }
        TESTS+=("$t")
    done
else
    i=0
    while IFS= read -r path; do
        t="${path##*/}"
        case "$t" in _*) continue ;; esac
        if grep -q '^# bash-only' "$path"; then
            skipped_bash_only=$((skipped_bash_only + 1))
            continue
        fi
        i=$((i + 1))
        [ $(( (i - 1) % SHARDS + 1 )) -eq "$SHARD" ] || continue
        TESTS+=("$t")
    done < <(find "$ROOT/tests/cli" -maxdepth 1 -name '*.sh' -type f | LC_ALL=C sort)
fi
[ ${#TESTS[@]} -gt 0 ] || { echo "cli-parity: no tests selected" >&2; exit 2; }

# run_one <test> <bash|binary>: the test's exit code; its output goes to
# $WORK/out/<test>.<mode>.
run_one() {
    local t="$1" mode="$2" home rc=0
    home="$(mktemp -d "$WORK/home.XXXXXX")"
    (
        while IFS= read -r v; do
            case "$v" in K2_*|K5_*|K2SO_*) unset "$v" ;; esac
        done < <(compgen -e)
        export HOME="$home"
        if [ "$mode" = binary ]; then export K5_CLI_UNDER_TEST="$WORK/bin/k2"; fi
        cd "$ROOT"
        exec ${TIMEOUT[@]+"${TIMEOUT[@]}"} bash "tests/cli/$t"
    ) >"$WORK/out/$t.$mode" 2>&1 </dev/null || rc=$?
    rm -rf "$home"
    return "$rc"
}

# The links answer as the bash does before any test runs. The bash wants a
# port and a token even for --version (a dead port and a dummy, as
# tests/cli/_hermetic_cli.sh hands it).
hermetic=(env -i PATH="$PATH" HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=test-token)
want_version="$("${hermetic[@]}" bash "$ROOT/cli/k2" --version)"
for name in k2 k5; do
    got="$("${hermetic[@]}" "$WORK/bin/$name" --version)" \
        || { echo "cli-parity: $name --version failed" >&2; exit 2; }
    [ "$got" = "$want_version" ] || { echo "cli-parity: $name --version printed '$got', bash cli/k2 '$want_version'" >&2; exit 2; }
done

# The named expected differences: "name|PRD reference|what differs".
EXPECTED_DIFFS=(
    "version-offline|13.9 #3|--version with no port or token: bash refuses (exit 1, \"Error: Cannot connect to K2.\"); the binary prints \"k2 <version>\" (exit 0), as help, --schema and --version never need the daemon"
    "native-without-python3|K2 ruling, out-of-box S1|a native k5 command runs when python3 is missing; that is a named, beneficial difference, and parity stays exact everywhere python3 exists. Hand-offs to bash for anything not native stay, with a plain line naming what is missing (python3 on a fresh Mac, Git Bash on Windows): with no python3 on PATH, study errors --json prints its JSON as with python3 where bash prints none, and hostmail (not built in yet) prints one line naming Python 3 and exits 69 without starting bash"
    "owner-token-with-passport|13.9 #4|an open verb (whoami, done, activity, feedback list and show, api) in a session with a cell socket set and the owner token on disk: bash sends the owner token over TCP (COMPAT-58, _cli_apply_auth_for_verb); the binary sends the passport, never the owner token. Also workspace_swap_canonical.sh's swap lookup: bash looks the workspace up with the owner token; the binary's passport is refused there, so it sends the typed name and the swap gets 400 not registered (K2 ruling; until the daemon's handle_swap_canonical resolves names itself, after r3)"
    "bash32-pipe-reader|R3.3, K2 ruling at the 0.46.15 rebase|a request on the left of a pipe into a python3 reader that gets no answer (the check: an empty reply, one request over TCP, the owner token on disk): \`inbox\`, \`inbox read a1\`, \`inbox folders\`, \`read acme\`, \`activity\`, \`connect status\` (GETs), \`connections add beta\`, \`connections remove beta\`, \`api on\` and \`api-key revoke k1\` (POSTs), and \`api-key list\` (a GET). Both print the no-answer line (\"Error: K2's server ...\"); the binary then dies of SIGPIPE (141) with nothing on stdout, as bash 5 does. macOS's /bin/bash 3.2 runs the reader on the empty input and exits 1 with its output added: inbox \"No items.\" on stdout, inbox read \"Error: no answer from the K2 daemon (...). Try again: k2 inbox read <id>\" on stderr, inbox folders \"No folders yet. Create one by moving an item:\" and \"  k2 inbox move <id> projects\" on stdout, read an empty line on stderr, activity \"Error: the K2 daemon gave no usable answer: ''\" on stderr, connections add and remove \"Error: daemon returned an unparseable response: ''\" on stderr (0.46.17), api on the same line on stdout, api-key list and revoke \"Error: Expecting value: line 1 column 1 (char 0)\" on stderr, connect status nothing. The pipe mark (_K2_PIPE_MARK) quiets only the heartbeat readers that set it, not these"
    "feedback-optin-without-python3|K2 ruling at the 0.46.17 feedback opt-in|an agent (a passport over TCP) whose server says agents may not report (\"agents\": false, \"senderKind\": \"agent\"), cards off: with python3 both CLIs leave send-feedback's block out of \`help\` and print the one line for \`study send-feedback\`; with no python3 on PATH bash's probe can't read the answer, so bash prints the full help and the full page, while the binary reads the answer itself and still leaves the block out and prints the one line, as it does with python3. Both send the same requests either way (help: the settings read, then the cards; study: the settings read)"
    "wiki-status-large-answer|K5 lane r8c, for K2 to rule on|\`wiki status\` hands both of the daemon's answers (the serve status and the index) to its Python in environment variables; an index above what one variable may hold (128 KiB on Linux, about 1 MiB in all on macOS) stops bash there: \"<path>: line <n>: <python3>: Argument list too long\" on stderr, nothing on stdout, exit 126 (exit 1 on macOS, with its /bin/bash 3.2). The binary reads the answers itself and prints the status, exit 0. Both send the same two requests"
    "expect-100-continue|K2 ruling, K5 transport|a POST whose body is over 1 MiB (\`store blob put\` of a 1.2 MB file, one 2 MiB chunk, 1.6 MB of JSON): bash's curl adds \"Expect: 100-continue\" to the request's head and waits for the daemon's 100 before the body; the binary never sends it and sends the body at once. Everything else in the two requests is the same (the parity rig's hand-over headers masked), and a body under 1 MiB goes out the same from both. stdout, stderr and the exit code are the same"
    "python-version-error-text|K2 ruling, the macOS range 2 fixes|the binary words a Python error as Python 3.12 does (it has no Python version to copy); the shell's python3 words three of them otherwise, by its version, read from that python3, never guessed: before 3.11 \"string indices must be integers\" has no \", not 'str'\" (macOS's 3.9); before 3.12 a dict sliced raises TypeError (\"unhashable type: 'slice'\") where the binary's raises KeyError; from 3.13 a JSON trailing comma is \"Illegal trailing comma before end of object\" (or array) at the comma, where the binary says \"Expecting property name enclosed in double quotes\" (or \"Expecting value\") after it (api-key list's \"a trailing comma\"). The rest of the output, the exit and the requests are the same"
    "edge-usage-exit-2|K2 ruling D6|bash's own failure on a command line it can't read, which only bash can print: a value flag as the last word (\`msg ws hi --from\`, also --command, --wake-timeout and --via; \`thread bob --from\`, also --limit and --before-seq, and \`thread unread --kind\`; \`talk ws hi --lines\`, also --from, --command, --agent, --session, --token, --answer, --respond, --answers and --wake-timeout) stops bash on \"<path to cli/k2>: line <n>: \$2: unbound variable\" with exit 1; the binary prints one plain line, \"Error: --from needs a value (try 'k2 msg --help')\", and exits 2. Neither sends anything"
    "exec-child-env|K2 ruling 1b, vault run (db run's ruling 2a child the same)|the program a command runs in its place (vault run's helper, db run's command, named in families::vault::run and families::db::run): bash's child also gets bash's own environment changes, SHLVL and _K2_PY_PROXY_RULE (the shell CLI's Python proxy rule), and _ (the program's path) where bash's exec sets it, none of which the binary's child gets; and an exec that fails (a folder the daemon named) exits 126 on both, worded \"<path to cli/k2>: line <n>: <helper>: Is a directory\" by bash and \"exec: <helper>: Permission denied (os error 13)\" by the binary. The request each sends is the same"
    "nul-in-answer|K2 ruling at the 0.46.15 rebase|a NUL byte in the daemon's answer to \`feedback list\` or \`feedback show <id>\`: bash's resp=\$(cli_request ...) drops it and, from bash 4.4 on, writes one line on stderr, \"<path to cli/k2>: line <n>: warning: command substitution: ignored null byte in input\"; the binary drops it silently. stdout, the exit code and the requests are the same (bash 3.2 writes no warning, so there the two are exact)"
)

# check_version_offline: both CLIs run `--version` with a temp HOME and no
# K2_*/K5_* variable at all (no port, no token). Prints what doesn't hold
# and returns 1 when the difference is not exactly as named.
check_version_offline() {
    local cli_version clean out err rc name problems=()
    cli_version="$(grep -m1 '^K2_CLI_VERSION=' "$ROOT/cli/k2" | cut -d'"' -f2)"
    [ -n "$cli_version" ] || { echo "no K2_CLI_VERSION line in cli/k2"; return 1; }
    mkdir -p "$WORK/offline-home"
    clean=(env -i PATH="$PATH" HOME="$WORK/offline-home")
    out="$WORK/offline.out" err="$WORK/offline.err"
    rc=0; "${clean[@]}" bash "$ROOT/cli/k2" --version >"$out" 2>"$err" </dev/null || rc=$?
    [ "$rc" = 1 ] || problems+=("bash exited $rc, want 1")
    [ ! -s "$out" ] || problems+=("bash printed on stdout: $(head -c 200 "$out")")
    grep -qxF 'Error: Cannot connect to K2.' "$err" \
        || problems+=("bash's stderr has no line 'Error: Cannot connect to K2.': $(head -c 200 "$err")")
    for name in k2 k5; do
        rc=0; "${clean[@]}" "$WORK/bin/$name" --version >"$out" 2>"$err" </dev/null || rc=$?
        [ "$rc" = 0 ] || problems+=("the binary as $name exited $rc, want 0")
        [ "$(cat "$out")" = "k2 $cli_version" ] && [ "$(wc -l <"$out" | tr -d ' ')" = 1 ] \
            || problems+=("the binary as $name printed '$(head -c 200 "$out")', want exactly 'k2 $cli_version'")
        [ ! -s "$err" ] || problems+=("the binary as $name wrote on stderr: $(head -c 200 "$err")")
    done
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_edge_usage_exit_2: a value flag as the last word, with a port and
# an owner token on disk but nothing listening (neither side may ask).
# Bash stops on its own unbound-variable line and exits 1; the binary
# prints its one line and exits 2.
check_edge_usage_exit_2() {
    local d="$WORK/d6" flag rc out err problems=()
    mkdir -p "$d/home/.k2" "$d/ws"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    printf '1\n' >"$d/home/.k2/heartbeat.port"
    out="$d/out" err="$d/err"
    local verb verb_flag
    for verb_flag in msg:--from msg:--command msg:--wake-timeout msg:--via \
        talk:--from talk:--command talk:--lines talk:--agent talk:--session talk:--token \
        talk:--answer talk:--respond talk:--answers talk:--wake-timeout; do
        verb="${verb_flag%%:*}" flag="${verb_flag#*:}"
        rc=0; (cd "$d/ws" && env -i PATH="$PATH" HOME="$d/home" bash "$ROOT/cli/k2" "$verb" ws hi "$flag") >"$out" 2>"$err" </dev/null || rc=$?
        [ "$rc" = 1 ] || problems+=("bash $verb ws hi $flag exited $rc, want 1")
        [ ! -s "$out" ] || problems+=("bash $verb ws hi $flag printed on stdout: $(head -c 200 "$out")")
        [ "$(wc -l <"$err" | tr -d ' ')" = 1 ] && grep -q '^.*: line [0-9]*: \$2: unbound variable$' "$err" \
            || problems+=("bash $verb ws hi $flag wrote: $(head -c 300 "$err")")
        rc=0; (cd "$d/ws" && env -i PATH="$PATH" HOME="$d/home" "$WORK/bin/k2" "$verb" ws hi "$flag") >"$out" 2>"$err" </dev/null || rc=$?
        [ "$rc" = 2 ] || problems+=("the binary's $verb ws hi $flag exited $rc, want 2")
        [ ! -s "$out" ] || problems+=("the binary's $verb ws hi $flag printed on stdout: $(head -c 200 "$out")")
        [ "$(cat "$err")" = "Error: $flag needs a value (try 'k2 $verb --help')" ] \
            || problems+=("the binary's $verb ws hi $flag wrote: $(head -c 300 "$err")")
    done
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# path_without_python3 <dir>: prints this PATH with no python3 on it, each
# folder that has one replaced by a folder of links to its other files
# (made under <dir>); returns 1, saying so, when python3 is still found.
path_without_python3() {
    local d="$1" i=0 dir f nopath=""
    local -a dirs
    IFS=: read -r -a dirs <<<"$PATH"
    for dir in "${dirs[@]}"; do
        i=$((i + 1))
        if [ -e "$dir/python3" ] || [ -L "$dir/python3" ]; then
            mkdir -p "$d/path-$i"
            while IFS= read -r f; do
                [ "$f" = python3 ] || ln -s "$dir/$f" "$d/path-$i/$f"
            done < <(ls -A "$dir")
            dir="$d/path-$i"
        fi
        nopath="${nopath:+$nopath:}$dir"
    done
    if (PATH="$nopath"; command -v python3 >/dev/null 2>&1); then
        echo "python3 is still on the PATH made without it: $nopath"
        return 1
    fi
    printf '%s\n' "$nopath"
}

# check_native_without_python3: a PATH with no python3 (each folder that
# has one replaced by a folder of links to its other files). With it,
# `study errors --json` (whose JSON the shell's Python writes) prints the
# binary's JSON exactly as with python3, and bash's prints none; `hostmail`
# (not built in) prints the one line naming Python 3 on stderr and
# exits 69, and bash never starts (the shell CLI is not written out).
check_native_without_python3() {
    local d="$WORK/no-python3" nopath rc want got problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf '1\n' >"$d/home/.k2/heartbeat.port"
    printf 'tok-parity\n' >"$d/home/.k2/heartbeat.token"
    nopath="$(path_without_python3 "$d")" || { echo "$nopath"; return 1; }
    run_in() { (cd "$d/ws" && env -i PATH="$1" HOME="$d/home" "${@:2}") </dev/null; }
    want="$(run_in "$PATH" "$WORK/bin/k5" study errors --json 2>&1)" || problems+=("the binary with python3 failed: $want")
    case "$want" in '{"id":"errors"'*) ;; *) problems+=("the binary with python3 printed no JSON page: $(printf '%s' "$want" | head -c 200)") ;; esac
    rc=0; got="$(run_in "$nopath" "$WORK/bin/k5" study errors --json 2>&1)" || rc=$?
    [ "$rc" = 0 ] && [ "$got" = "$want" ] \
        || problems+=("the binary with no python3 exited $rc and printed: $(printf '%s' "$got" | head -c 200)")
    got="$(run_in "$nopath" bash "$ROOT/cli/k2" study errors --json 2>/dev/null)" || true
    [ "$got" != "$want" ] || problems+=("bash printed the JSON page with no python3: the difference is gone")
    rc=0; got="$(run_in "$nopath" "$WORK/bin/k5" hostmail 2>"$d/err")" || rc=$?
    [ "$rc" = 69 ] || problems+=("the binary's hostmail with no python3 exited $rc, want 69")
    [ -z "$got" ] || problems+=("the binary's hostmail with no python3 printed on stdout: $(printf '%s' "$got" | head -c 200)")
    [ "$(wc -l <"$d/err" | tr -d ' ')" = 1 ] && grep -q '^k5 hostmail: this command is not built into k5 yet and needs Python 3\. ' "$d/err" \
        || problems+=("the binary's hostmail with no python3 wrote: $(head -c 300 "$d/err")")
    [ ! -e "$d/home/.k2/run/cli" ] || problems+=("the binary's hostmail with no python3 wrote the shell CLI out")
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_owner_token_with_passport: both CLIs run `whoami --json`, `done`,
# `read acme`, `activity`, `feedback list` and `feedback show x` with a
# passport, K2_HOOK_SOCK set (no socket there, so both go over TCP),
# K2_PROJECT_PATH set (so `activity` and `feedback list` ask no
# /cli/whoami first) and the owner token on disk, against a stub that logs
# each request's credential: its Authorization: Bearer header (neither CLI
# puts one in the query any more; one there is logged as "query:<T>").
check_owner_token_with_passport() {
    local d="$WORK/owner-token" port="" pid rc side tok problems=()
    local owner="owner-tok-parity" passport="sessP.parity1"
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf '%s\n' "$owner" >"$d/home/.k2/heartbeat.token"
    python3 -I -c '
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
d = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
        auth = self.headers.get("Authorization") or ""
        tok = auth[len("Bearer "):] if auth.startswith("Bearer ") else ""
        if q.get("token"):
            tok = "query:" + q["token"][0]
        with open(os.path.join(d, "log"), "a") as f:
            f.write(tok + "\n")
        body = json.dumps({"role": "canonical"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_POST = do_GET
    def log_message(self, *a):
        pass
s = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(d, "port.tmp"), "w").write(str(s.server_port))
os.rename(os.path.join(d, "port.tmp"), os.path.join(d, "port"))
s.serve_forever()
' "$d" &
    pid=$!
    for _ in $(seq 1 100); do [ -s "$d/port" ] && break; sleep 0.05; done
    port="$(cat "$d/port" 2>/dev/null || true)"
    if [ -z "$port" ]; then
        kill "$pid" 2>/dev/null || true
        echo "the stub daemon didn't start"
        return 1
    fi
    # (read left it in 0.46.17: its cli_send arm sends the passport on
    # TCP when the session's socket failed, as the binary does.)
    for verb in "whoami --json" done activity "feedback list" "feedback show x" "api off"; do
        for side in bash binary; do
            : >"$d/log"
            rc=0
            if [ "$side" = bash ]; then set -- bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
            # shellcheck disable=SC2086
            (cd "$d/ws" && env -i PATH="$PATH" HOME="$d/home" K2_PORT="$port" K2_HOOK_TOKEN="$passport" \
                K2_HOOK_SOCK="$d/no-socket" K2_PROJECT_PATH="$d/ws" K2_AGENT_NAME=parity "$@" $verb) >"$d/out" 2>&1 </dev/null || rc=$?
            tok="$(cat "$d/log")"
            [ "$rc" = 0 ] || problems+=("$side $verb exited $rc: $(head -c 200 "$d/out")")
            if [ "$side" = bash ]; then
                [ "$tok" = "$owner" ] || problems+=("bash $verb sent token '$tok', want the owner token")
            else
                [ "$tok" = "$passport" ] || problems+=("the binary's $verb sent token '$tok', want the passport")
            fi
        done
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# start_stub <dir> <mode>: a stub daemon on a free loopback port that
# logs each request ("GET /path?query", a POST's body after it) to
# <dir>/log, each %XX escape in
# upper case (curl writes -G's in lower case, the binary in upper: K2's
# ruling, as the parity rig's normaliser says). Mode close: no
# reply at all, the connection just closes (an empty reply, curl's 52).
# Mode nul: /cli/feedback/list and /cli/feedback/show answer a ticket
# whose title holds a NUL byte, anything else {"role":"canonical"}. Mode
# optin: /cli/product-feedback/settings says agents may not report (to an
# agent), /cli/permission/enabled says cards are off, anything else
# {"role":"canonical"}. Mode wikibig: /cli/wiki/serve/status answers
# {"enabled":false} and /cli/wiki/index 60,000 notes (about 1.4 MiB),
# anything else {"role":"canonical"}. Mode heads: the log holds each
# request's whole head (the request line and every header, in order, as
# sent), then "body <bytes> <sha256>" and an empty line; the stub speaks
# HTTP/1.1, so it answers "Expect: 100-continue" with a 100 as a daemon
# does, and /cli/zen/data/blob/put answers the ref of the chunk's bytes.
# Sets STUB_PID and STUB_PORT; returns 1 when it doesn't start.
start_stub() {
    local d="$1" mode="$2"
    rm -f "$d/port" "$d/port.tmp"
    python3 -I -c '
import base64, hashlib, json, os, re, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
d, mode = sys.argv[1], sys.argv[2]
NUL = {
    "/cli/feedback/list": b"{\"items\":[{\"id\":\"fb_nul1\",\"status\":\"waiting\",\"priority\":2,\"kind\":\"question\",\"title\":\"acme\x00globex\"}]}",
    "/cli/feedback/show": b"{\"id\":\"fb_nul1\",\"status\":\"waiting\",\"priority\":2,\"kind\":\"question\",\"title\":\"acme\x00globex\",\"createdAt\":0}",
}
OPTIN = {
    "/cli/product-feedback/settings": b"{\"enabled\":true,\"agents\":false,\"canSend\":false,\"why\":\"agents_off\",\"senderKind\":\"agent\",\"ok\":true}",
    "/cli/permission/enabled": b"{\"enabled\":false}",
}
WIKIBIG = {
    "/cli/wiki/serve/status": b"{\"enabled\":false}",
    "/cli/wiki/index": b"{\"nodes\":[" + b",".join(b"{\"id\":\"note-%05d.md\"}" % i for i in range(60000)) + b"]}",
}
TABLE = {"nul": NUL, "optin": OPTIN, "wikibig": WIKIBIG}.get(mode, {})
HELPERS = {"vaultrun": "helper", "vaultrun-dir": "dir-helper"}
if mode in HELPERS:
    TABLE = {"/cli/vault/run-helper": json.dumps({"helper": os.path.join(d, HELPERS[mode]), "tokenFd": True}).encode()}
class H(BaseHTTPRequestHandler):
    if mode == "heads":
        protocol_version = "HTTP/1.1"
    def do_GET(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        with open(os.path.join(d, "log"), "a") as f:
            if mode == "heads":
                f.write(self.requestline + "\n")
                for k, v in self.headers.items():
                    f.write(k + ": " + v + "\n")
                f.write("body %d %s\n\n" % (len(raw), hashlib.sha256(raw).hexdigest()))
            else:
                body = raw.decode("utf-8", "replace")
                line = self.path + (" " + body if body else "")
                line = re.sub("%[0-9a-fA-F]{2}", lambda m: m.group(0).upper(), line)
                f.write(self.command + " " + line + "\n")
        self.close_connection = True
        if mode == "close":
            return
        body = TABLE.get(self.path.split("?", 1)[0], b"{\"role\":\"canonical\"}")
        if mode == "heads" and self.path == "/cli/zen/data/blob/put":
            chunk = base64.b64decode(json.loads(raw)["base64"])
            body = json.dumps({"ref": "k2blob:sha256-" + hashlib.sha256(chunk).hexdigest()}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_POST = do_GET
    def log_message(self, *a):
        pass
s = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(d, "port.tmp"), "w").write(str(s.server_port))
os.rename(os.path.join(d, "port.tmp"), os.path.join(d, "port"))
s.serve_forever()
' "$d" "$mode" &
    STUB_PID=$!
    for _ in $(seq 1 100); do [ -s "$d/port" ] && break; sleep 0.05; done
    STUB_PORT="$(cat "$d/port" 2>/dev/null || true)"
    if [ -z "$STUB_PORT" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        return 1
    fi
}

stop_stub() {
    kill "$STUB_PID" 2>/dev/null || true
    wait "$STUB_PID" 2>/dev/null || true
}

# run_side <dir> <shell|binary> <words...>: one CLI's run, in <dir>/ws
# with HOME <dir>/home (the owner token and the stub's port on disk), a
# fresh TMPDIR, no K2_*/K5_* variable and stdin from /dev/null. <shell>
# is the bash to run cli/k2 with. Its stdout goes to <dir>/out, stderr to
# <dir>/err, the requests to <dir>/log; returns its exit code.
run_side() {
    local d="$1" side="$2" tmp rc=0
    shift 2
    : >"$d/log"
    tmp="$(mktemp -d "$d/tmp.XXXXXX")"
    if [ "$side" = binary ]; then set -- "$WORK/bin/k5" "$@"; else set -- "$side" "$ROOT/cli/k2" "$@"; fi
    (cd "$d/ws" && exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" "$@") \
        >"$d/out" 2>"$d/err" </dev/null || rc=$?
    return "$rc"
}

# The no-answer line's seconds, masked.
mask_seconds() { sed -E 's/([ (])[0-9]+ s([;),])/\1N s\2/g' "$1"; }

# check_bash32_pipe_reader: the stub never answers (an empty reply) and
# both CLIs run `inbox`, `inbox read a1`, `inbox folders`, `read acme`,
# `activity`, `connect status`, `connections add beta` and `connections
# remove beta`, the owner token and port on disk. The binary prints the
# no-answer line and dies of SIGPIPE (141) with nothing on stdout. (The
# workspace verbs left this list at 0.46.17: no reader is piped there any
# more, so bash on any version and the binary print the line and exit 1
# alike; their parity tests compare them exactly.) On macOS
# with /bin/bash 3.2 bash must exit 1 with no signal, its stdout the
# reader's for an empty input and its stderr the binary's (seconds
# masked) then the reader's. With any other bash, bash must answer as
# the binary does; the check then says plainly that the difference
# itself was not proven here, and returns 3.
check_bash32_pipe_reader() {
    local d="$WORK/bash32-pipe" shell=bash ver i rc rr words problems=()
    local -a all_words reader_out reader_err has_line
    local unparseable=$'Error: daemon returned an unparseable response: \'\'\n'
    local nojson=$'Error: Expecting value: line 1 column 1 (char 0)\n'
    all_words=("inbox" "inbox read a1" "inbox folders" "read acme" "activity" "connect status" "connections add beta" "connections remove beta" "api on" "api-key list" "api-key revoke k1")
    reader_out=($'No items.\n' "" $'No folders yet. Create one by moving an item:\n  k2 inbox move <id> projects\n' "" "" "" "" "" "$unparseable" "" "")
    reader_err=("" $'Error: no answer from the K2 daemon (neither this session\'s cell socket nor the loopback port answered). Is K2 running? Try again: k2 inbox read <id>\n' "" $'\n' $'Error: the K2 daemon gave no usable answer: \'\'\n' "" "$unparseable" "$unparseable" "" "$nojson" "$nojson")
    has_line=(1 1 1 1 1 1 1 1 1 1 1)
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    [ "$(uname -s)" != Darwin ] || shell=/bin/bash
    ver="$("$shell" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    start_stub "$d" close || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    for i in 0 1 2 3 4 5 6 7 8 9 10; do
        words="${all_words[$i]}"
        # shellcheck disable=SC2086
        rc=0; run_side "$d" "$shell" $words || rc=$?
        cp "$d/out" "$d/out.bash"; cp "$d/log" "$d/log.bash"
        mask_seconds "$d/err" >"$d/err.bash"
        # shellcheck disable=SC2086
        rr=0; run_side "$d" binary $words || rr=$?
        [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = 1 ] || problems+=("$shell $words sent $(wc -l <"$d/log.bash" | tr -d ' ') requests, want 1: $(head -c 300 "$d/log.bash")")
        cmp -s "$d/log.bash" "$d/log" || problems+=("$words: the requests differ: $shell '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log")'")
        [ "$rr" = 141 ] || problems+=("the binary's $words exited $rr, want 141 (SIGPIPE): $(head -c 300 "$d/err")")
        [ ! -s "$d/out" ] || problems+=("the binary's $words printed on stdout: $(head -c 200 "$d/out")")
        if [ "${has_line[$i]}" = 1 ]; then
            grep -q "^Error: K2's server " "$d/err" || problems+=("the binary's $words wrote no no-answer line: $(head -c 300 "$d/err")")
        else
            [ ! -s "$d/err" ] || problems+=("the binary's $words wrote on stderr, want nothing (the lookup's line goes to /dev/null): $(head -c 300 "$d/err")")
        fi
        if [ "$ver" = 3.2 ]; then
            [ "$rc" = 1 ] || problems+=("$shell ($ver) $words exited $rc, want 1 (the reader ran): the difference is gone or changed")
            printf '%s' "${reader_out[$i]}" >"$d/want.out"
            cmp -s "$d/want.out" "$d/out.bash" || problems+=("$shell ($ver) $words printed '$(head -c 300 "$d/out.bash")' on stdout, want the reader's '${reader_out[$i]}'")
            { mask_seconds "$d/err"; printf '%s' "${reader_err[$i]}"; } >"$d/want.err"
        else
            [ "$rc" = 141 ] || problems+=("$shell ($ver) $words exited $rc, want 141 as the binary: the difference is no longer only bash 3.2's")
            [ ! -s "$d/out.bash" ] || problems+=("$shell ($ver) $words printed on stdout: $(head -c 300 "$d/out.bash")")
            mask_seconds "$d/err" >"$d/want.err"
        fi
        cmp -s "$d/want.err" "$d/err.bash" || problems+=("$shell ($ver) $words wrote '$(head -c 400 "$d/err.bash")' on stderr (seconds masked), want '$(head -c 400 "$d/want.err")'")
    done
    stop_stub
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$ver" = 3.2 ] && return 0
    echo "checked only on bash 3.2, macOS's /bin/bash (the macOS parity job runs it); here $shell is bash $ver, where bash and the binary answer alike on all eleven (SIGPIPE 141, nothing on stdout, the same stderr), so the difference itself was not proven on this host"
    return 3
}

# check_nul_in_answer: the stub answers `feedback list` and `feedback show
# fb_nul1` with a NUL inside the title ("acme<NUL>globex"), the owner token
# and port on disk. Both CLIs exit 0, send the same requests and print the
# same stdout, which has the title as "acmeglobex"; the binary writes
# nothing about the NUL; bash (the one on PATH, as the tests run it)
# writes exactly one line, "<path to cli/k2>: line <n>: warning: command
# substitution: ignored null byte in input", from 4.4 on, none before;
# stderr is otherwise the same. On a bash before 4.4 the check says so
# and returns 3: the two are exact there.
# check_python_version_error_text: the python3 on PATH words the three
# errors as its version says it does (above). On a 3.12, which words all
# three as the binary does, the check says the difference itself was not
# proven here and returns 3.
check_python_version_error_text() {
    local got ver want problems=()
    got="$(python3 -I -c '
import json, sys
print("%d.%d" % sys.version_info[:2])
try:
    "a"[str("b")]
except TypeError as e:
    print(e)
try:
    {}[:3]
except Exception as e:
    print(type(e).__name__)
try:
    json.loads("[1,]")
except ValueError as e:
    print(e)
')" || { echo "python3 failed: $got"; return 1; }
    ver="$(printf '%s\n' "$got" | sed -n 1p)"
    local major="${ver%%.*}" minor="${ver#*.}"
    [ "$major" = 3 ] || { echo "python3 is $ver, not 3.x"; return 1; }
    if [ "$minor" -lt 11 ]; then want="string indices must be integers"; else want="string indices must be integers, not 'str'"; fi
    [ "$(printf '%s\n' "$got" | sed -n 2p)" = "$want" ] || problems+=("python3 $ver says '$(printf '%s\n' "$got" | sed -n 2p)', want '$want'")
    if [ "$minor" -lt 12 ]; then want=TypeError; else want=KeyError; fi
    [ "$(printf '%s\n' "$got" | sed -n 3p)" = "$want" ] || problems+=("python3 $ver raises $(printf '%s\n' "$got" | sed -n 3p) for a dict sliced, want $want")
    if [ "$minor" -ge 13 ]; then want="Illegal trailing comma before end of array: line 1 column 3 (char 2)"; else want="Expecting value: line 1 column 4 (char 3)"; fi
    [ "$(printf '%s\n' "$got" | sed -n 4p)" = "$want" ] || problems+=("python3 $ver says '$(printf '%s\n' "$got" | sed -n 4p)' at a trailing comma, want '$want'")
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$minor" = 12 ] || return 0
    echo "python3 here is 3.12, which words all three as the binary does, so the difference itself was not proven on this host (the macOS job's 3.9 proves the first two; a python3 of 3.13 or later, the third)"
    return 3
}

check_nul_in_answer() {
    local d="$WORK/nul" ver major minor warns=0 rc rr n line words route problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    ver="$(bash -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    major="${ver%%.*}" minor="${ver#*.}"
    if [ "$major" -gt 4 ] || { [ "$major" -eq 4 ] && [ "$minor" -ge 4 ]; }; then warns=1; fi
    start_stub "$d" nul || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    for words in "feedback list" "feedback show fb_nul1"; do
        # shellcheck disable=SC2086
        rc=0; run_side "$d" bash $words || rc=$?
        cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
        # shellcheck disable=SC2086
        rr=0; run_side "$d" binary $words || rr=$?
        [ "$rc" = 0 ] || problems+=("bash $words exited $rc: $(head -c 300 "$d/err.bash")")
        [ "$rr" = 0 ] || problems+=("the binary's $words exited $rr: $(head -c 300 "$d/err")")
        route="${words#feedback }"; route="${route%% *}"
        grep -q "^GET /cli/feedback/$route" "$d/log.bash" \
            || problems+=("bash $words never asked for the ticket: $(head -c 300 "$d/log.bash")")
        cmp -s "$d/log.bash" "$d/log" || problems+=("$words: the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log")'")
        cmp -s "$d/out.bash" "$d/out" || problems+=("$words: stdout differs: bash '$(head -c 300 "$d/out.bash")', the binary '$(head -c 300 "$d/out")'")
        grep -q 'acmeglobex' "$d/out" || problems+=("the binary's $words printed no title 'acmeglobex' (the NUL dropped): $(head -c 300 "$d/out")")
        n=0; : >"$d/err.rest"
        while IFS= read -r line || [ -n "$line" ]; do
            case "$line" in
                "$ROOT/cli/k2: line "*": warning: command substitution: ignored null byte in input") n=$((n + 1)) ;;
                *) printf '%s\n' "$line" >>"$d/err.rest" ;;
            esac
        done <"$d/err.bash"
        [ "$n" = "$warns" ] || problems+=("bash $ver $words wrote $n NUL warning lines, want $warns (bash warns from 4.4 on): $(head -c 300 "$d/err.bash")")
        cmp -s "$d/err.rest" "$d/err" || problems+=("$words: stderr but bash's NUL warning differs: bash '$(head -c 300 "$d/err.rest")', the binary '$(head -c 300 "$d/err")'")
        ! grep -q 'null byte' "$d/err" || problems+=("the binary's $words wrote about the NUL: $(head -c 300 "$d/err")")
    done
    stop_stub
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$warns" = 1 ] && return 0
    echo "bash on PATH is $ver, which drops a NUL silently as the binary does (the warning came in bash 4.4): the two are exact here, so the difference itself was not proven on this host"
    return 3
}

# check_exec_child_env: the stub names a run helper; both CLIs run `vault
# run --secret A -- true` with the owner token on disk (no passport, fd 3
# empty). The helper (Python, which adds nothing) prints which of SHLVL, _
# and _K2_PY_PROXY_RULE it was given: SHLVL and _K2_PY_PROXY_RULE (and _
# where bash's exec sets it) for bash, none for the binary. Then a folder
# (126 on both, worded each its way). A helper with no #! line is no
# difference: execvp hands it to /bin/sh as bash does. Each pair sends
# the same one request.
check_exec_child_env() {
    local d="$WORK/exec-child" mode rc rr problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2" "$d/dir-helper"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    printf '%s\n' '#!/usr/bin/env python3' 'import os' \
        'for n in ("SHLVL", "_", "_K2_PY_PROXY_RULE"): print(n, "set" if n in os.environ else "unset")' >"$d/helper"
    chmod +x "$d/helper" "$d/dir-helper"
    for mode in vaultrun vaultrun-dir; do
        start_stub "$d" "$mode" || { echo "the stub daemon didn't start ($mode)"; return 1; }
        printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
        rc=0; run_side "$d" bash vault run --secret A -- true || rc=$?
        cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
        rr=0; run_side "$d" binary vault run --secret A -- true || rr=$?
        stop_stub
        [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = 1 ] || problems+=("$mode: bash sent, want one request: $(head -c 300 "$d/log.bash")")
        cmp -s "$d/log.bash" "$d/log" || problems+=("$mode: the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log")'")
        case "$mode" in
            vaultrun)
                [ "$rc" = 0 ] && [ "$rr" = 0 ] || problems+=("$mode: exits bash $rc, binary $rr, want 0 and 0: $(head -c 300 "$d/err.bash") / $(head -c 300 "$d/err")")
                grep -qx 'SHLVL set' "$d/out.bash" && grep -qx '_K2_PY_PROXY_RULE set' "$d/out.bash" || problems+=("$mode: bash's helper got: $(head -c 300 "$d/out.bash")")
                [ "$(cat "$d/out")" = "$(printf 'SHLVL unset\n_ unset\n_K2_PY_PROXY_RULE unset')" ] || problems+=("$mode: the binary's helper got: $(head -c 300 "$d/out")")
                ;;
            vaultrun-dir)
                [ "$rc" = 126 ] && [ "$rr" = 126 ] || problems+=("$mode: exits bash $rc, binary $rr, want 126 and 126")
                grep -q ": $d/dir-helper: Is a directory\$" "$d/err.bash" || problems+=("$mode: bash's stderr: $(head -c 300 "$d/err.bash")")
                grep -qx "exec: $d/dir-helper: Permission denied (os error 13)" "$d/err" || problems+=("$mode: the binary's stderr: $(head -c 300 "$d/err")")
                ;;
        esac
    done
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_wiki_status_large_answer: the stub answers the wiki index with
# about 1.4 MiB of notes (mode wikibig), the owner token and port on disk,
# and both CLIs run `wiki status`. Bash exits 126 (1 on macOS, with its
# /bin/bash 3.2) with "Argument list too long" on stderr and nothing on
# stdout; the binary exits 0, prints the
# status with "  Notes:      60000" and nothing on stderr; both send the
# same requests, the serve status then the index.
check_wiki_status_large_answer() {
    local d="$WORK/wiki-large" rc rr want=126 problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    start_stub "$d" wikibig || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    rc=0; run_side "$d" bash wiki status || rc=$?
    cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
    rr=0; run_side "$d" binary wiki status || rr=$?
    stop_stub
    [ "$(uname -s)" != Darwin ] || want=1
    [ "$rc" = "$want" ] || problems+=("bash exited $rc, want $want: $(head -c 300 "$d/err.bash")")
    grep -q 'Argument list too long' "$d/err.bash" || problems+=("bash's stderr has no 'Argument list too long': $(head -c 300 "$d/err.bash")")
    [ ! -s "$d/out.bash" ] || problems+=("bash printed on stdout: $(head -c 300 "$d/out.bash")")
    [ "$rr" = 0 ] || problems+=("the binary exited $rr, want 0: $(head -c 300 "$d/err")")
    grep -qx '  Notes:      60000' "$d/out" || problems+=("the binary printed no note count: $(head -c 300 "$d/out")")
    [ ! -s "$d/err" ] || problems+=("the binary wrote on stderr: $(head -c 300 "$d/err")")
    [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = 2 ] || problems+=("bash sent, want two requests: $(head -c 300 "$d/log.bash")")
    cmp -s "$d/log.bash" "$d/log" || problems+=("the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log")'")
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_feedback_optin_without_python3: the stub says agents may not
# report (mode optin) and both CLIs run `help` and `study send-feedback`
# as an agent (a passport, the stub's port, no socket), once with this
# PATH and once with no python3 on it. With python3 both leave the block
# out of help (no "  send-feedback " line) and print the one line for the
# study page, the same bytes. With no python3 bash prints the block and the
# full page, the binary exactly what it printed with python3. Each run
# sends what bash sends: the help the settings read then the cards, the
# study page the settings read.
check_feedback_optin_without_python3() {
    local d="$WORK/feedback-optin" nopath words side path rc tag n problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    nopath="$(path_without_python3 "$d")" || { echo "$nopath"; return 1; }
    start_stub "$d" optin || { echo "the stub daemon didn't start"; return 1; }
    for words in "help" "study send-feedback"; do
        tag="${words// /-}"
        for path in "$PATH" "$nopath"; do
            for side in bash binary; do
                : >"$d/log"
                rc=0
                if [ "$side" = bash ]; then set -- bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
                # shellcheck disable=SC2086
                (cd "$d/ws" && env -i PATH="$path" HOME="$d/home" TMPDIR="$d" K2_PORT="$STUB_PORT" \
                    K2_HOOK_TOKEN=sessP.optin1 "$@" $words) >"$d/out.$side" 2>"$d/err.$side" </dev/null || rc=$?
                cp "$d/log" "$d/log.$side"
                [ "$rc" = 0 ] || problems+=("$side $words (python3 $([ "$path" = "$PATH" ] && echo on || echo off) PATH) exited $rc: $(head -c 300 "$d/err.$side")")
                [ ! -s "$d/err.$side" ] || problems+=("$side $words wrote on stderr: $(head -c 300 "$d/err.$side")")
            done
            if [ "$words" = help ]; then n=2; grep -q "^GET /cli/permission/enabled" "$d/log.bash" || problems+=("bash help asked no cards: $(head -c 300 "$d/log.bash")"); else n=1; fi
            [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = "$n" ] && head -1 "$d/log.bash" | grep -q "^GET /cli/product-feedback/settings$" \
                || problems+=("bash $words sent, want the settings read first and $n requests: $(head -c 300 "$d/log.bash")")
            cmp -s "$d/log.bash" "$d/log.binary" || problems+=("$words: the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log.binary")'")
            if [ "$path" = "$PATH" ]; then
                cmp -s "$d/out.bash" "$d/out.binary" || problems+=("$words with python3: stdout differs: bash '$(head -c 300 "$d/out.bash")', the binary '$(head -c 300 "$d/out.binary")'")
                cp "$d/out.binary" "$d/want.$tag"
                if [ "$words" = help ]; then
                    ! grep -q '^  send-feedback ' "$d/out.bash" || problems+=("bash help with python3 lists send-feedback: the hide is gone")
                    grep -q '^  feedback list ' "$d/out.bash" || problems+=("bash help with python3 printed no daily tier: $(head -c 300 "$d/out.bash")")
                else
                    [ "$(wc -l <"$d/out.bash" | tr -d ' ')" = 1 ] && grep -q "^Agents don't report " "$d/out.bash" \
                        || problems+=("bash study send-feedback with python3 is not the one line: $(head -c 300 "$d/out.bash")")
                fi
            else
                cmp -s "$d/want.$tag" "$d/out.binary" || problems+=("the binary's $words with no python3 printed otherwise than with python3: $(head -c 300 "$d/out.binary")")
                if [ "$words" = help ]; then
                    grep -q '^  send-feedback ' "$d/out.bash" || problems+=("bash help with no python3 leaves send-feedback out: the difference is gone")
                else
                    [ "$(wc -l <"$d/out.bash" | tr -d ' ')" -gt 20 ] || problems+=("bash study send-feedback with no python3 is not the full page: the difference is gone: $(head -c 300 "$d/out.bash")")
                fi
            fi
        done
    done
    stop_stub
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_expect_100_continue: the stub logs each request's whole head (mode
# heads), the owner token and port on disk, and both CLIs run `store blob
# put <file> --widget board`: once with a 600,000-byte file (800,000 bytes
# of base64, a body under 1 MiB) and once with a 1,200,000-byte file (a
# body of about 1.6 MB, still one 2 MiB chunk). Bash runs with
# K5_CLI_VIA=rust, as the parity rig runs it, so both sides send the
# hand-over headers. With the rig's masks (each side's User-Agent and
# X-K5-Cli-Via masked, the binary's X-K5-Cli-Version and bash's
# X-K5-Cli-Noun dropped), the small put's requests are the same line for
# line and body for body; the big put's differ in one line only, bash's
# "Expect: 100-continue". Both print the chunk's ref, write nothing on
# stderr and exit 0.
check_expect_100_continue() {
    local d="$WORK/expect100" size side rc tmp want problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    start_stub "$d" heads || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    for size in 600000 1200000; do
        python3 -I -c 'import os, sys; open(sys.argv[1], "wb").write(os.urandom(int(sys.argv[2])))' "$d/f$size.bin" "$size"
        want="k2blob:sha256-$(python3 -I -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$d/f$size.bin")"
        for side in bash binary; do
            : >"$d/log"
            rc=0
            tmp="$(mktemp -d "$d/tmp.XXXXXX")"
            if [ "$side" = bash ]; then set -- K5_CLI_VIA=rust bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
            (cd "$d/ws" && exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" "$@" store blob put "$d/f$size.bin" --widget board) \
                >"$d/out.$side" 2>"$d/err.$side" </dev/null || rc=$?
            cp "$d/log" "$d/log.$side"
            [ "$rc" = 0 ] || problems+=("$side put of $size bytes exited $rc: $(head -c 300 "$d/err.$side")")
            [ ! -s "$d/err.$side" ] || problems+=("$side put of $size bytes wrote on stderr: $(head -c 300 "$d/err.$side")")
            [ "$(cat "$d/out.$side")" = "$want" ] || problems+=("$side put of $size bytes printed '$(head -c 200 "$d/out.$side")', want the chunk's ref $want")
        done
        python3 -I - "$d/log.bash" "$d/log.binary" "$size" >"$d/verdict" <<'PY' || problems+=("$(cat "$d/verdict")")
import sys
EXPECT = "Expect: 100-continue"
bash_log, bin_log, size = sys.argv[1], sys.argv[2], int(sys.argv[3])
bad = []

def blocks(p):
    return [b.split("\n") for b in open(p).read().split("\n\n") if b.strip()]

def norm(lines, native, where):
    out, seen = [], {"ua": 0, "via": 0, "ver": 0, "noun": 0}
    for l in lines:
        if l.startswith("User-Agent: " + ("k5/" if native else "curl/")):
            seen["ua"] += 1
            out.append("User-Agent: <ua>")
        elif native and l.startswith("X-K5-Cli-Version: "):
            seen["ver"] += 1
        elif l == "X-K5-Cli-Via: " + ("native" if native else "fallthrough"):
            seen["via"] += 1
            out.append("X-K5-Cli-Via: <via>")
        elif not native and l.startswith("X-K5-Cli-Noun: "):
            seen["noun"] += 1
        else:
            out.append(l)
    want = {"ua": 1, "via": 1, "ver": int(native), "noun": int(not native)}
    if seen != want:
        bad.append("%s: the masked lines %r, want %r: %r" % (where, seen, want, lines))
    return out

b, r = blocks(bash_log), blocks(bin_log)
if len(b) != 1 or len(r) != 1:
    bad.append("a put of %d bytes is one chunk, one request: bash sent %d, the binary %d" % (size, len(b), len(r)))
for i, (bl, rl) in enumerate(zip(b, r)):
    bn = norm(bl, False, "bash request %d" % i)
    rn = norm(rl, True, "the binary's request %d" % i)
    if bn[0] != "POST /cli/zen/data/blob/put HTTP/1.1":
        bad.append("bash request %d is not the blob put: %r" % (i, bn[0]))
    cl = [int(l.split(": ", 1)[1]) for l in bn if l.startswith("Content-Length: ")]
    if len(cl) != 1:
        bad.append("bash request %d has %d Content-Length lines" % (i, len(cl)))
        continue
    big = cl[0] > 1048576
    if big != (size > 700000):
        bad.append("the put of %d bytes sent a body of %d bytes: the case no longer straddles 1 MiB" % (size, cl[0]))
    if EXPECT in rn:
        bad.append("the binary sent %r (body %d bytes): the difference is gone" % (EXPECT, cl[0]))
    if big:
        if bn.count(EXPECT) != 1:
            bad.append("bash sent no %r on a body of %d bytes: the difference is gone or changed" % (EXPECT, cl[0]))
        bn = [l for l in bn if l != EXPECT]
    elif EXPECT in bn:
        bad.append("bash sent %r on a body of %d bytes, under 1 MiB" % (EXPECT, cl[0]))
    if bn != rn:
        bad.append("the put of %d bytes: the requests differ beyond %r: bash %r, the binary %r" % (size, EXPECT, bn, rn))
if bad:
    print("\n".join(bad))
    sys.exit(1)
PY
    done
    stop_stub
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

for entry in "${EXPECTED_DIFFS[@]}"; do
    IFS='|' read -r d_name d_prd d_what <<<"$entry"
    d_rc=0; why="$("check_${d_name//-/_}")" || d_rc=$?
    case "$d_rc" in
        0) echo "cli-parity: named difference $d_name (PRD $d_prd) holds: $d_what" ;;
        3)
            # It applies elsewhere only; what was checked here held.
            echo "cli-parity: named difference $d_name (PRD $d_prd) NOT PROVEN on this host: $d_what"
            printf '%s\n' "$why" | sed 's/^/    /'
            ;;
        *)
            echo "cli-parity: FAIL: the named difference $d_name (PRD $d_prd) no longer holds: $d_what" >&2
            printf '%s\n' "$why" | sed 's/^/    /' >&2
            exit 1
            ;;
    esac
done

echo "cli-parity: ${#TESTS[@]} tests (shard $SHARD/$SHARDS), binary $BIN ($want_version)"
BOTH=() BINARY_ONLY=() FAIL_BOTH=() BASH_ONLY=()
for t in "${TESTS[@]}"; do
    start=$SECONDS
    rb=0; run_one "$t" bash || rb=$?
    rr=0; run_one "$t" binary || rr=$?
    took=$((SECONDS - start))
    if [ "$rb" = 0 ] && [ "$rr" = 0 ]; then
        BOTH+=("$t"); echo "  ok          $t (${took}s)"
    elif [ "$rb" = 0 ]; then
        BINARY_ONLY+=("$t"); echo "  FAIL binary $t (bash 0, binary $rr, ${took}s)"
        echo "    ── tail of its output on the binary ──"
        tail -25 "$WORK/out/$t.binary" | sed 's/^/    | /'
    elif [ "$rr" = 0 ]; then
        BASH_ONLY+=("$t"); echo "  fails bash  $t (bash $rb, binary 0, ${took}s)"
    else
        FAIL_BOTH+=("$t"); echo "  fails both  $t (bash $rb, binary $rr, ${took}s)"
        tail -3 "$WORK/out/$t.bash" | sed 's/^/    bash | /'
    fi
done

echo ""
[ ${#BINARY_ONLY[@]} -eq 0 ] || echo "fail on the binary only: ${BINARY_ONLY[*]}"
[ ${#FAIL_BOTH[@]} -eq 0 ] || echo "fail on both: ${FAIL_BOTH[*]}"
[ ${#BASH_ONLY[@]} -eq 0 ] || echo "fail on bash only: ${BASH_ONLY[*]}"
echo "cli-parity: ${#BOTH[@]} passed on both, ${#BINARY_ONLY[@]} fail on the binary only, ${#FAIL_BOTH[@]} fail on both, ${#BASH_ONLY[@]} fail on bash only ($skipped_bash_only bash-only skipped; shard $SHARD/$SHARDS)"
[ ${#BINARY_ONLY[@]} -eq 0 ]
