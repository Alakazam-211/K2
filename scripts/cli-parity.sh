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
# No tests/cli test meets one through the binary today; one that does is
# mapped to its difference here by name, never skipped broadly. (The
# binary's own parity tests meet owner-token-with-passport:
# crates/k5-cli/tests/whoami_parity.rs, done_parity.rs, activity_parity.rs
# and feedback_parity.rs; since 0.46.17 the shell's read and setup send
# the passport there too, so read_parity.rs and setup_parity.rs no longer
# meet it. They meet
# native-without-python3 in every case of the parity rig, in
# whoami_parity.rs, pages_without_python3.rs and the study cards tests;
# tests/cli runs with python3 on PATH. They meet bash32-pipe-reader in the
# no-answer cases of inbox_parity.rs, read_parity.rs, activity_parity.rs,
# connections_write_parity.rs and connect_parity.rs, through the rig's
# Compare::Bash32PipeReader on macOS. They meet nul-in-answer in
# feedback_parity.rs's "a NUL in a string" rows of list and show, through
# Compare::NulInAnswer.)
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
    "native-without-python3|K2 ruling, out-of-box S1|a native k5 command runs when python3 is missing; that is a named, beneficial difference, and parity stays exact everywhere python3 exists. Hand-offs to bash for anything not native stay, with a plain line naming what is missing (python3 on a fresh Mac, Git Bash on Windows): with no python3 on PATH, study errors --json prints its JSON as with python3 where bash prints none, and msg prints one line naming Python 3 and exits 69 without starting bash"
    "owner-token-with-passport|13.9 #4|an open verb (whoami, done, activity, feedback list and show) in a session with a cell socket set and the owner token on disk: bash sends the owner token over TCP (COMPAT-58, _cli_apply_auth_for_verb); the binary sends the passport, never the owner token"
    "bash32-pipe-reader|R3.3, K2 ruling at the 0.46.15 rebase|a request on the left of a pipe into a python3 reader that gets no answer (the check: an empty reply, one request over TCP, the owner token on disk): \`inbox\`, \`inbox read a1\`, \`inbox folders\`, \`read acme\`, \`activity\`, \`connect status\` (GETs), \`connections add beta\` and \`connections remove beta\` (POSTs). Both print the no-answer line (\"Error: K2's server ...\"); the binary then dies of SIGPIPE (141) with nothing on stdout, as bash 5 does. macOS's /bin/bash 3.2 runs the reader on the empty input and exits 1 with its output added: inbox \"No items.\" on stdout, inbox read \"Error: no answer from the K2 daemon (...). Try again: k2 inbox read <id>\" on stderr, inbox folders \"No folders yet. Create one by moving an item:\" and \"  k2 inbox move <id> projects\" on stdout, read an empty line on stderr, activity \"Error: the K2 daemon gave no usable answer: ''\" on stderr, connections add and remove \"Error: daemon returned an unparseable response: ''\" on stderr (0.46.17), connect status nothing. The pipe mark (_K2_PIPE_MARK) quiets only the heartbeat readers that set it, not these"
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

# check_native_without_python3: a PATH with no python3 (each folder that
# has one replaced by a folder of links to its other files). With it,
# `study errors --json` (whose JSON the shell's Python writes) prints the
# binary's JSON exactly as with python3, and bash's prints none; `msg`
# (not built in) prints the one line naming Python 3 on stderr and
# exits 69, and bash never starts (the shell CLI is not written out).
check_native_without_python3() {
    local d="$WORK/no-python3" i=0 dir f nopath="" rc want got problems=()
    local -a dirs
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf '1\n' >"$d/home/.k2/heartbeat.port"
    printf 'tok-parity\n' >"$d/home/.k2/heartbeat.token"
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
    run_in() { (cd "$d/ws" && env -i PATH="$1" HOME="$d/home" "${@:2}") </dev/null; }
    want="$(run_in "$PATH" "$WORK/bin/k5" study errors --json 2>&1)" || problems+=("the binary with python3 failed: $want")
    case "$want" in '{"id":"errors"'*) ;; *) problems+=("the binary with python3 printed no JSON page: $(printf '%s' "$want" | head -c 200)") ;; esac
    rc=0; got="$(run_in "$nopath" "$WORK/bin/k5" study errors --json 2>&1)" || rc=$?
    [ "$rc" = 0 ] && [ "$got" = "$want" ] \
        || problems+=("the binary with no python3 exited $rc and printed: $(printf '%s' "$got" | head -c 200)")
    got="$(run_in "$nopath" bash "$ROOT/cli/k2" study errors --json 2>/dev/null)" || true
    [ "$got" != "$want" ] || problems+=("bash printed the JSON page with no python3: the difference is gone")
    rc=0; got="$(run_in "$nopath" "$WORK/bin/k5" msg acme hello 2>"$d/err")" || rc=$?
    [ "$rc" = 69 ] || problems+=("the binary's msg with no python3 exited $rc, want 69")
    [ -z "$got" ] || problems+=("the binary's msg with no python3 printed on stdout: $(printf '%s' "$got" | head -c 200)")
    [ "$(wc -l <"$d/err" | tr -d ' ')" = 1 ] && grep -q '^k5 msg: this command is not built into k5 yet and needs Python 3\. ' "$d/err" \
        || problems+=("the binary's msg with no python3 wrote: $(head -c 300 "$d/err")")
    [ ! -e "$d/home/.k2/run/cli" ] || problems+=("the binary's msg with no python3 wrote the shell CLI out")
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
    for verb in "whoami --json" done activity "feedback list" "feedback show x"; do
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
# whose title holds a NUL byte, anything else {"role":"canonical"}. Sets
# STUB_PID and STUB_PORT; returns 1 when it doesn't start.
start_stub() {
    local d="$1" mode="$2"
    rm -f "$d/port" "$d/port.tmp"
    python3 -I -c '
import os, re, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
d, mode = sys.argv[1], sys.argv[2]
NUL = {
    "/cli/feedback/list": b"{\"items\":[{\"id\":\"fb_nul1\",\"status\":\"waiting\",\"priority\":2,\"kind\":\"question\",\"title\":\"acme\x00globex\"}]}",
    "/cli/feedback/show": b"{\"id\":\"fb_nul1\",\"status\":\"waiting\",\"priority\":2,\"kind\":\"question\",\"title\":\"acme\x00globex\",\"createdAt\":0}",
}
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        with open(os.path.join(d, "log"), "a") as f:
            n = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
            line = self.path + (" " + body if body else "")
            line = re.sub("%[0-9a-fA-F]{2}", lambda m: m.group(0).upper(), line)
            f.write(self.command + " " + line + "\n")
        self.close_connection = True
        if mode == "close":
            return
        body = NUL.get(self.path.split("?", 1)[0], b"{\"role\":\"canonical\"}")
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
    all_words=("inbox" "inbox read a1" "inbox folders" "read acme" "activity" "connect status" "connections add beta" "connections remove beta")
    reader_out=($'No items.\n' "" $'No folders yet. Create one by moving an item:\n  k2 inbox move <id> projects\n' "" "" "" "" "")
    reader_err=("" $'Error: no answer from the K2 daemon (neither this session\'s cell socket nor the loopback port answered). Is K2 running? Try again: k2 inbox read <id>\n' "" $'\n' $'Error: the K2 daemon gave no usable answer: \'\'\n' "" "$unparseable" "$unparseable")
    has_line=(1 1 1 1 1 1 1 1)
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    [ "$(uname -s)" != Darwin ] || shell=/bin/bash
    ver="$("$shell" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    start_stub "$d" close || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    for i in 0 1 2 3 4 5 6 7; do
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
    echo "checked only on bash 3.2, macOS's /bin/bash (the macOS parity job runs it); here $shell is bash $ver, where bash and the binary answer alike on all eight (SIGPIPE 141, nothing on stdout, the same stderr), so the difference itself was not proven on this host"
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
