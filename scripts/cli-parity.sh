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
# Two tests/cli tests meet one through the binary, and check the binary's
# side of it exactly, by the difference's name, never by a skip:
# socket_first_no_owner.sh (owner-token-with-passport: `read` when the
# socket says "not served"), and
# socket_first_no_owner_headless.sh (written for the same difference:
# `feedback list` over TCP from a real session).
# (airgap_no_python.sh met native-without-python3 until `daemon
# install` was native: its rows now expect bash's answer with no
# python3 too.)
# Any other is mapped to its difference the same way. (The
# binary's own parity tests meet owner-token-with-passport:
# crates/k5-cli/tests/whoami_parity.rs, done_parity.rs and
# activity_parity.rs; since 0.46.17 the shell's read and setup send
# the passport there too, so read_parity.rs and setup_parity.rs no longer
# meet it. They meet
# native-without-python3 in every case of the parity rig, in
# whoami_parity.rs, pages_without_python3.rs and the study cards tests;
# tests/cli runs with python3 on PATH. They meet bash32-pipe-reader in the
# no-answer cases of inbox_parity.rs, read_parity.rs, activity_parity.rs,
# connections_write_parity.rs, connect_parity.rs, arms_parity.rs (reviews),
# remind_parity.rs, review_skills_whats_new_parity.rs (skills list),
# workspace_verbs_parity.rs, wiki_parity.rs, api_parity.rs,
# api_key_parity.rs, agent_parity.rs, glossary_parity.rs,
# heartbeat_parity.rs (list), msg_parity.rs (the sender lookup),
# postal_parity.rs, talk_parity.rs and tunnel_parity.rs, through the rig's
# Compare::Bash32PipeReader on macOS. They meet bash32-empty-brace in
# msg_signal_parity.rs's empty-payload row, on a bash before 4 (macOS's
# /bin/bash 3.2). They meet python-version-error-text
# wherever the shell's Python words an error otherwise than 3.12: the rig
# takes it for every row (common/parity.rs, as_this_python, and
# older_slice_error for a traceback's dict sliced), and
# compute_render_oracle.rs for a dict sliced. They meet nul-in-answer in
# feedback_parity.rs's "a NUL in a string" rows of list and show, through
# Compare::NulInAnswer. They meet feedback-optin-without-python3 in
# help_feedback_optin_parity.rs: every row's run with no python3 prints
# what the binary prints with one, and its no-python3 test checks the
# rows that answer otherwise there. They meet env-answer-large in
# wiki_parity.rs's wiki_status_with_a_large_answer and
# mail_writes_parity.rs's a_large_body, zen_parity.rs's
# zen_history_with_a_large_answer, agent_parity.rs's
# a_list_above_the_environment_s_limit and publish_parity.rs's ps_large. They meet
# curl-version-proxy-exit where the node's curl exits 7 on a refused
# tunnel: plain_http_daemon_parity.rs's
# whoami_through_a_proxy_that_refuses_the_tunnel checks the binary's side
# alone there, by the difference's name, and wiki_parity.rs's
# wiki_publish_through_the_proxy takes bash's 4 tries.)
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
    "version-offline|13.9 #3|--version (also -v and version, with any words after it but exactly --version --check, which the router's arm never reads) with no port or token: bash refuses (exit 1, \"Error: Cannot connect to K2.\"); the binary prints \"k2 <version>\" (exit 0), as help, --schema and --version never need the daemon"
    "native-without-python3|K2 ruling, out-of-box S1|a native k5 command runs when python3 is missing; that is a named, beneficial difference, and parity stays exact everywhere python3 exists. Hand-offs to bash for anything not native stay, with a plain line naming what is missing (python3 on a fresh Mac, Git Bash on Windows): with no python3 on PATH, study errors --json prints its JSON as with python3 where bash prints none, and a family still handed to the shell (the first of a list the check walks) prints one line naming Python 3 and exits 69 without starting bash"
    "owner-token-with-passport|13.9 #4|an open verb (whoami, done, activity, api off) in a session with a cell socket set and the owner token on disk: bash sends the owner token over TCP (COMPAT-58, _cli_apply_auth_for_verb); the binary sends the passport, never the owner token. Also workspace_swap_canonical.sh's swap lookup: bash looks the workspace up with the owner token; the binary's passport is refused there, so it sends the typed name and the swap gets 400 not registered (K2 ruling; until the daemon's handle_swap_canonical resolves names itself, after r3)"
    "bash32-pipe-reader|R3.3, K2 ruling at the 0.46.15 rebase|a request on the left of a pipe into a python3 reader that gets no answer (the check: an empty reply, one request over TCP, the owner token on disk): \`inbox\`, \`inbox read a1\`, \`inbox folders\`, \`read acme\`, \`activity\`, \`connect status\` (GETs), \`connections add beta\`, \`connections remove beta\`, \`api on\` and \`api-key revoke k1\` (POSTs), \`api-key list\` and \`agent template list\` (GETs), and \`agent template remove x\` (a POST). Both print the no-answer line (\"Error: K2's server ...\"); the binary then dies of SIGPIPE (141) with nothing on stdout, as bash 5 does. macOS's /bin/bash 3.2 runs the reader on the empty input and exits 1 with its output added: inbox \"No items.\" on stdout, inbox read \"Error: no answer from the K2 daemon (...). Try again: k2 inbox read <id>\" on stderr, inbox folders \"No folders yet. Create one by moving an item:\" and \"  k2 inbox move <id> projects\" on stdout, read an empty line on stderr, activity \"Error: the K2 daemon gave no usable answer: ''\" on stderr, connections add and remove \"Error: daemon returned an unparseable response: ''\" on stderr (0.46.17), api on the same line on stdout, api-key list and revoke \"Error: Expecting value: line 1 column 1 (char 0)\" on stderr, connect status nothing, agent template list and remove \"Error: daemon answered: \" on stderr. The pipe mark (_K2_PIPE_MARK) quiets only the heartbeat readers that set it, not these"
    "bash32-empty-brace|K2 ruling, macOS bash 3.2 until the shell's fix|\`msg ws --signal <kind> --payload ''\` with an empty payload (msg_signal_parity.rs, \"presence, an empty payload and kind, --inbox-silent\"): the payload takes cmd_signal's default, \"\${3:-{\\}}\"; macOS's /bin/bash 3.2 keeps the backslash there, so bash sends the text \"{\\}\" where the binary sends \"\" (bash 4 and later read {}, as the binary does). The rest of the request, the output and the exit are the same. The shell's fix (the default quoted) retires it"
    "feedback-optin-without-python3|K2 ruling at the 0.46.17 feedback opt-in|an agent (a passport over TCP) whose server says agents may not report (\"agents\": false, \"senderKind\": \"agent\"), cards off: with python3 both CLIs leave send-feedback's block out of \`help\` and print the one line for \`study send-feedback\`; with no python3 on PATH bash's probe can't read the answer, so bash prints the full help and the full page, while the binary reads the answer itself and still leaves the block out and prints the one line, as it does with python3. Both send the same requests either way (help: the settings read, then the cards; study: the settings read)"
    "env-answer-large|K2 ruling, the K5 lanes' large answers|bash hands a daemon answer to Python in an environment variable, so above the kernel's argument limit bash stops with 'Argument list too long' (exit 126) and k5 prints the answer. The cases, each with its own large-answer row: \`wiki status\` with an index above 1 MiB (wiki_parity.rs, wiki_status_with_a_large_answer; on macOS its /bin/bash 3.2 exits 1, and both send the same two requests), \`mail send --body-file\` with a body above 128 KiB (mail_writes_parity.rs, a_large_body; the body rides M_BODY: on Linux bash sends nothing, the binary sends it once; macOS has one limit for the whole environment, where both send it), \`zen history\` with an answer above 1 MiB (zen_parity.rs, zen_history_with_a_large_answer; macOS's bash fails there too, its wording not checked), \`agent list\` above 1 MiB (agent_parity.rs, a_list_above_the_environment_s_limit; 1 on macOS) and \`publish ps\` (publish_parity.rs, ps_large)"
    "expect-100-continue|K2 ruling, K5 transport|a POST whose body is over 1 MiB (\`store blob put\` of a 1.2 MB file, one 2 MiB chunk, 1.6 MB of JSON): bash's curl adds \"Expect: 100-continue\" to the request's head and waits for the daemon's 100 before the body; the binary never sends it and sends the body at once. Everything else in the two requests is the same (the parity rig's hand-over headers masked), and a body under 1 MiB goes out the same from both. stdout, stderr and the exit code are the same"
    "python-version-error-text|K2 ruling, the macOS range 2 fixes|the binary words a Python error as Python 3.12 does (it has no Python version to copy); the shell's python3 words four of them otherwise, by its version, read from that python3, never guessed: before 3.11 \"string indices must be integers\" has no \", not 'str'\" (macOS's 3.9); before 3.12 a dict sliced raises TypeError (\"unhashable type: 'slice'\") where the binary's raises KeyError (the parity rig's tracebacks, mail forward show's \"events that are a dict\"); before 3.12 datetime.utcfromtimestamp prints no DeprecationWarning, where the binary prints 3.12's one line (\"<string>:<n>: DeprecationWarning: datetime.datetime.utcfromtimestamp() is deprecated ...\", remote-session's times); from 3.13 a JSON trailing comma is \"Illegal trailing comma before end of object\" (or array) at the comma, where the binary says \"Expecting property name enclosed in double quotes\" (or \"Expecting value\") after it (api-key list's \"a trailing comma\"). The rest of the output, the exit and the requests are the same"
    "edge-usage-exit-2|K2 ruling D6|bash's own failure on a value it can't read, which only bash can print: hostchat's value flags with an empty value (\`hostchat enable --tarball ''\`, also --recovery-key-file, --identity, --voice-url, --media-tarball, --node-ip, --accept-binary, --confirm-hostname and --public-url; \`hostchat media install --url ''\`, also --voice-url, --public-url, --node-ip, --tarball, --media-tarball, --udp, --tcp and --turn-tls; \`hostchat users invite ann --role ''\`, also --display-name and --email): bash stops on its own \"<path to cli/k2>: line <n>: 2: --tarball FILE\" (its \${2:?...}) with exit 1, and the binary prints \"Error: --tarball needs a value (try 'k2 hostchat --help')\" and exits 2 (hostchat_parity.rs). Neither sends anything. A value flag as the LAST word is no longer a difference: bash's _need_value and the binary both print \"Error: <flag> takes a value\" and exit 2. Also \`zen data restore w --rev\` (any flag but --json and --dry-run as the last word): until the shell's restore fix reaches this branch, bash's loop there never ends (\`shift 2 || true\` shifts nothing), so only the binary runs it: \"Error: --rev takes a value\", exit 2, nothing sent (crates/k5-cli/tests/zen_parity.rs, the brand rows)"
    "exec-child-env|K2 ruling 1b, vault run (db run's ruling 2a child the same)|the program a command runs in its place (vault run's helper, db run's command, named in families::vault::run and families::db::run): bash's child also gets bash's own environment changes, SHLVL and _K2_PY_PROXY_RULE (the shell CLI's Python proxy rule), and _ (the program's path) where bash's exec sets it, none of which the binary's child gets; and an exec that fails (a folder the daemon named) exits 126 on both, worded \"<path to cli/k2>: line <n>: <helper>: Is a directory\" by bash (\"is a directory\" by macOS bash 3.2) and \"exec: <helper>: Permission denied (os error 13)\" by the binary. The request each sends is the same"
    "curl-config-files|K2 and the Steward, K5 shared transport|the binary never reads curl's config files (~/.curlrc, CURL_HOME): one can carry options that change where the token goes. Bash's curl reads ~/.curlrc on every call it makes, so a header that file adds (the check: header = \"X-From-Curlrc: acme\") is on bash's request to the daemon and not on the binary's. The rest of the request, stdout, stderr and the exit code are the same (\`feedback list\`). The binary does read CURL_CA_BUNDLE and SSL_CERT_FILE, as curl does"
    "macos-system-proxy|K2 ruling, K5 shared transport|on macOS the system proxy (System Settings, Network, Proxies; no environment variable) is read by python3's urllib (urllib.request.getproxies_macosx_sysconf, when no proxy variable is set), so a bash Python block that reaches another machine over https may go through it; the binary reads only the proxy variables, as curl does, and never the system setting (it links no SystemConfiguration framework). Every bash request made with curl ignores the system setting too. Linux has no such setting: not proven there"
    "curl-version-proxy-exit|K2 ruling, K5 shared transport|a proxy that refuses the tunnel (407) to a K2 daemon at K2_HOST=https://<name> (the check: \`whoami\`, https_proxy set): the binary sends one CONNECT and stops, as curl 8.5 does (exit 56, never tried again by _k2_curl). A newer curl (8.23 on one node) fails that CONNECT with exit 7, which _k2_curl tries 4 times in all, 1 s apart, so bash sends 4 CONNECTs and prints what its curl 7 means; the binary's one CONNECT, exit and lines are the same on every node. Where bash's curl exits 56 the two are the same: not proven there"
    "python-tls-error-text|K2 ruling, K5 lane small6 (connect login)|a TLS handshake to the Connect account server that fails (\`connect login\`'s account calls, the shell's Python urllib): bash prints OpenSSL's reason inside Python's URLError (\"<urlopen error [SSL: WRONG_VERSION_NUMBER] wrong version number (_ssl.c:1000)>\"), the binary its own words (\"<urlopen error [SSL] TLS handshake: ...>\"). The exit code (1), every line before that one and that no byte of the HTTP request was sent are the same"
    "getpass-no-tty-warning|K2 ruling, K5 lane small6 (connect login)|\`connect login\`'s password with no terminal (no /dev/tty to open, stdin a pipe), read from stdin: bash's Python getpass warns with its own file and line, \"<path>/getpass.py:<n>: GetPassWarning: Can not control echo on the terminal.\" (and the source line Python shows under it when it has one); the binary prints one line of its own that says the same, \"Warning: Can not control echo on the terminal.\". Both then print Python's \"Warning: Password input may be echoed.\" and the prompt, read the line from stdin and send the same request; every other line and the exit code are the same"
    "picker-env-size|K2 ruling, K5 lane small6 (connect login)|\`connect login\` with a subdomain list above what one environment variable may hold (128 KiB on Linux; about 1 MiB for the whole environment on macOS): bash's picker gets the list (tunnel tokens taken out) in SUBS_JSON and can't start, \"<path>: line <n>: <python3>: Argument list too long\" on stderr, nothing on stdout, exit 1, after the list request and before anything is asked of the daemon. The binary reads the list itself, picks, claims, binds and starts: \"✓ live at <url>\", exit 0"
    "nul-in-answer|K2 ruling at the 0.46.15 rebase|a NUL byte in the daemon's answer to \`feedback list\` or \`feedback show <id>\`: bash's resp=\$(cli_request ...) drops it and, from bash 4.4 on, writes one line on stderr, \"<path to cli/k2>: line <n>: warning: command substitution: ignored null byte in input\"; the binary drops it silently. stdout, the exit code and the requests are the same (bash 3.2 writes no warning, so there the two are exact)"
    "passport-heal-once|K2 ruling, the doc lane's follow-ups|a doc call that sends a whole file or a tool call (_doc_xfer: \`doc cat\`, \`append\`, \`write\`, \`show --version\`, \`export\`) whose passport the daemon's auth gate refuses (403 and exactly {\"error\":\"Invalid or missing auth token\"}, sent before any handler runs, so nothing was done), in a session whose passport was minted again: bash's _doc_xfer never tries the new passport and the command ends on the refusal (exit 3); the binary sends the same call once more with the new passport and answers it (cli_send_json's one heal, which bash's other doc calls already make). An answer a handler gave that holds those words (a document's text, a conflict) is never sent again by either: one request, the same output"
    "doc-mktemp-wording|K2 ruling, the doc lane's follow-ups|a doc verb that needs a temp folder (_doc_tmp) where TMPDIR can't hold one: the binary words mktemp's line as GNU mktemp does in the C locale (\"mktemp: failed to create directory via template '<TMPDIR>/k2doc.XXXXXX': No such file or directory\"; GNU's in a UTF-8 locale quotes the template with curly quotes); macOS's mktemp words it its own way (\"mktemp: mkdtemp failed on <TMPDIR>/k2doc.XXXXXX: No such file or directory\"; a newer macOS names the folder it tried, the six X filled in). The two lines after it (\"Error: can't make a temp folder\" and the tmp error's JSON), the exit (2) and stdout (nothing) are the same, and neither sends anything. The same holds for \`app exec\` with stdin on a pipe (k2-exec-XXXXXX) and \`hostchat open\` (k2-hostchat-open.XXXXXX) where TMPDIR can't hold the folder: on a newer macOS mktemp's line names the folder it tried, which the binary can't know, so the parity rows (Compare::MktempTried) take mktemp's line as the same when only that random name differs: the same folder and stem before it, one or more of [A-Za-z0-9.] in its place, the same reason after it, the same exit, and every other line byte for byte"
    "python-proxy-rule|K2 ruling R4, the clone engine's port|the person's proxy for a clone request to an https server that isn't this computer (the check: \`clone preview ws1 --host https://cp.example.invalid --token-stdin\` with only ALL_PROXY set): the shell's Python urllib reads https_proxy and HTTPS_PROXY only (never ALL_PROXY), its own no_proxy rule, opens a tunnel only on a 200 and writes its own CONNECT head, so it goes straight there; the binary takes the shared CONNECT path with curl's rules (all_proxy and ALL_PROXY too, curl's no_proxy, any 2xx, curl's head) and sends one CONNECT. Either way the token rides inside the tunnel only, never to the proxy"
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

# check_edge_usage_exit_2: a hostchat value flag with an empty value, with
# a port and an owner token on disk but nothing listening (neither side
# may ask). Bash stops on its own ${2:?...} line and exits 1; the binary
# prints its one line and exits 2.
check_edge_usage_exit_2() {
    local d="$WORK/d6" rc out err problems=() args sub flag hint
    mkdir -p "$d/home/.k2" "$d/ws"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    printf '1\n' >"$d/home/.k2/heartbeat.port"
    out="$d/out" err="$d/err"
    for args in "enable|--tarball|--tarball FILE" "media install|--url|--url wss://HOST"; do
        IFS='|' read -r sub flag hint <<<"$args"
        # shellcheck disable=SC2086 # sub is one or two plain words
        rc=0; (cd "$d/ws" && env -i PATH="$PATH" HOME="$d/home" bash "$ROOT/cli/k2" hostchat $sub "$flag" "") >"$out" 2>"$err" </dev/null || rc=$?
        [ "$rc" = 1 ] || problems+=("bash hostchat $sub $flag '' exited $rc, want 1")
        [ ! -s "$out" ] || problems+=("bash hostchat $sub $flag '' printed on stdout: $(head -c 200 "$out")")
        [ "$(wc -l <"$err" | tr -d ' ')" = 1 ] && grep -q "^.*: line [0-9]*: 2: $hint\$" "$err" \
            || problems+=("bash hostchat $sub $flag '' wrote: $(head -c 300 "$err")")
        # shellcheck disable=SC2086
        rc=0; (cd "$d/ws" && env -i PATH="$PATH" HOME="$d/home" "$WORK/bin/k2" hostchat $sub "$flag" "") >"$out" 2>"$err" </dev/null || rc=$?
        [ "$rc" = 2 ] || problems+=("the binary's hostchat $sub $flag '' exited $rc, want 2")
        [ ! -s "$out" ] || problems+=("the binary's hostchat $sub $flag '' printed on stdout: $(head -c 200 "$out")")
        [ "$(cat "$err")" = "Error: $flag needs a value (try 'k2 hostchat --help')" ] \
            || problems+=("the binary's hostchat $sub $flag '' wrote: $(head -c 300 "$err")")
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
# binary's JSON exactly as with python3, and bash's prints none; a family
# the binary still hands to the shell prints the one line naming Python 3
# on stderr and exits 69, and bash never starts (the shell CLI is not
# written out). The family is the first of a list, in the order they are
# planned to go native, that the binary does not answer itself, so the
# check moves on as lanes land; it fails when none is left.
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
    local fam="" try seen=()
    # The last, `setup agent-reports --json`, stays the shell's by K2's ruling
    # (send-feedback's internals); the error line names its noun.
    for try in daemon desktop hooks ci app-update release reserve migrate llm publish "setup agent-reports --json"; do
        # shellcheck disable=SC2086
        rc=0; got="$(run_in "$nopath" "$WORK/bin/k5" $try 2>"$d/err")" || rc=$?
        if [ "$rc" = 69 ]; then fam="$try"; break; fi
        seen+=("$try: exit $rc")
        rm -rf "$d/home/.k2/run/cli"
    done
    if [ -z "$fam" ]; then
        problems+=("no family in the list is handed to the shell any more (${seen[*]}): name one the binary still hands over")
    else
        [ -z "$got" ] || problems+=("the binary's $fam with no python3 printed on stdout: $(printf '%s' "$got" | head -c 200)")
        [ "$(wc -l <"$d/err" | tr -d ' ')" = 1 ] && grep -q "^k5 ${fam%% *}: this command is not built into k5 yet and needs Python 3\\. " "$d/err" \
            || problems+=("the binary's $fam with no python3 wrote: $(head -c 300 "$d/err")")
        [ ! -e "$d/home/.k2/run/cli" ] || problems+=("the binary's $fam with no python3 wrote the shell CLI out")
    fi
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_owner_token_with_passport: both CLIs run `whoami --json`, `done`,
# `activity` and `api off` with a passport, K2_HOOK_SOCK set (no socket
# there, so both go over TCP), K2_PROJECT_PATH set (so `activity` asks no
# /cli/whoami first) and the owner token on disk, against a stub that logs
# each request's credential: its Authorization: Bearer header (neither CLI
# puts one in the query any more; one there is logged as "query:<T>").
# Every request a verb makes must carry the side's credential (a verb may
# make more than one). `feedback list` and `feedback show` left it: since
# the tickets change the shell sends the session's passport there too
# (`_cli_tickets_sends_passport`), as the binary does.
check_owner_token_with_passport() {
    local d="$WORK/owner-token" port="" pid rc side tok want what problems=()
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
    for verb in "whoami --json" done activity "api off"; do
        for side in bash binary; do
            : >"$d/log"
            rc=0
            if [ "$side" = bash ]; then set -- bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
            # shellcheck disable=SC2086
            (cd "$d/ws" && env -i PATH="$PATH" HOME="$d/home" K2_PORT="$port" K2_HOOK_TOKEN="$passport" \
                K2_HOOK_SOCK="$d/no-socket" K2_PROJECT_PATH="$d/ws" K2_AGENT_NAME=parity "$@" $verb) >"$d/out" 2>&1 </dev/null || rc=$?
            tok="$(tr '\n' ' ' <"$d/log")"
            [ "$rc" = 0 ] || problems+=("$side $verb exited $rc: $(head -c 200 "$d/out")")
            if [ "$side" = bash ]; then want="$owner" what="the owner token"; else want="$passport" what="the passport"; fi
            # Each request's credential, one per line: at least one, all the side's.
            [ -s "$d/log" ] && ! grep -qvxF "$want" "$d/log" \
                || problems+=("$side $verb sent tokens '$tok', want $what on every request")
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
# {"role":"canonical"}. Mode envbig: /cli/wiki/serve/status answers
# {"enabled":false}, /cli/wiki/index 60,000 notes (about 1.4 MiB),
# /cli/mail/send a queued send (out_9) and /cli/zen/history 12,000
# files (about 1.2 MiB), anything else {"role":"canonical"}. Mode
# heads: the log holds each
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
ENVBIG = dict(WIKIBIG)
ENVBIG["/cli/mail/send"] = b"{\"ok\":true,\"queued\":true,\"id\":\"out_9\"}"
ENVBIG["/cli/zen/history"] = b"{\"files\":[" + b",".join(b"{\"file\":\"gardens/g%d.toml\",\"snapshots\":[{\"name\":\"snap-%d\",\"at\":\"2026-11-01T10:00:00Z\",\"clean\":true}]}" % (i, i) for i in range(12000)) + b"]}"
TABLE = {"nul": NUL, "optin": OPTIN, "envbig": ENVBIG}.get(mode, {})
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
    def do_CONNECT(self):
        # Mode refuse407: a proxy that refuses every tunnel.
        with open(os.path.join(d, "log"), "a") as f:
            f.write("proxy " + self.requestline + "\n")
        self.close_connection = True
        self.send_response(407)
        self.send_header("Proxy-Authenticate", "Basic realm=\"acme\"")
        self.send_header("Content-Length", "0")
        self.end_headers()
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

# check_bash32_empty_brace: both CLIs run `msg ws --signal "" --payload ""
# --inbox-silent` (an empty kind and payload), the owner token and port on
# disk, the stub answering. Both exit 0 with the same stdout and stderr and
# send one POST /cli/awareness/publish (its id and time masked). On macOS
# with /bin/bash 3.2, bash's default for the empty payload, "${3:-{\}}",
# keeps the backslash: its body's text is "{\}" (JSON "{\\}") where the
# binary's is "" (bash 4 and later read {}); everything else in the
# requests is the same. With any other bash the requests must be the same,
# and the check says the difference itself was not proven here (returns 3).
check_bash32_empty_brace() {
    local d="$WORK/bash32-brace" shell=bash ver rc rr b r side f note="" problems=()
    local from='"text": "{\\}"' to='"text": ""'
    local words=(msg ws --signal "" --payload "" --inbox-silent)
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    [ "$(uname -s)" != Darwin ] || shell=/bin/bash
    ver="$("$shell" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    start_stub "$d" plain || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    rc=0; run_side "$d" "$shell" "${words[@]}" || rc=$?
    cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
    rr=0; run_side "$d" binary "${words[@]}" || rr=$?
    stop_stub
    [ "$rc" = 0 ] || problems+=("$shell ($ver) exited $rc, want 0: $(head -c 300 "$d/err.bash")")
    [ "$rr" = 0 ] || problems+=("the binary exited $rr, want 0: $(head -c 300 "$d/err")")
    cmp -s "$d/out.bash" "$d/out" || problems+=("stdout differs: bash '$(head -c 300 "$d/out.bash")', the binary '$(head -c 300 "$d/out")'")
    cmp -s "$d/err.bash" "$d/err" || problems+=("stderr differs: bash '$(head -c 300 "$d/err.bash")', the binary '$(head -c 300 "$d/err")'")
    for side in bash binary; do
        if [ "$side" = bash ]; then f="$d/log.bash"; else f="$d/log"; fi
        [ "$(wc -l <"$f" | tr -d ' ')" = 1 ] && grep -q '^POST /cli/awareness/publish ' "$f" \
            || problems+=("$side sent, want one POST /cli/awareness/publish: $(head -c 300 "$f")")
    done
    b="$(sed -E 's/"id": "[0-9a-f-]{36}"/"id": "<id>"/; s/"at": "[^"]*"/"at": "<at>"/' "$d/log.bash")"
    r="$(sed -E 's/"id": "[0-9a-f-]{36}"/"id": "<id>"/; s/"at": "[^"]*"/"at": "<at>"/' "$d/log")"
    case "$r" in *'"kind": {"kind": "msg", "data": {"text": ""}}'*) ;; *) problems+=("the binary's body has no empty msg text: $(head -c 400 "$d/log")") ;; esac
    if [ "$ver" = 3.2 ]; then
        case "$b" in *"$from"*) ;; *) problems+=("$shell ($ver) sent no text \"{\\}\": the difference is gone or changed: $(head -c 400 "$d/log.bash")") ;; esac
        b="${b//"$from"/$to}"
        note=", bash 3.2's text \"{\\}\" read as \"\""
    fi
    [ "$b" = "$r" ] || problems+=("the requests differ (id and time masked$note): bash '$(printf '%s' "$b" | head -c 400)', the binary '$(printf '%s' "$r" | head -c 400)'")
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$ver" = 3.2 ] && return 0
    echo "checked only on bash 3.2, macOS's /bin/bash (the macOS parity job runs it); here $shell is bash $ver, which reads the empty payload's default as {} as the binary does, so the difference itself was not proven on this host"
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
# check_python_version_error_text: the python3 on PATH words the four
# cases as its version says it does (above). On a 3.12, which words all
# four as the binary does, the check says the difference itself was not
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
    local warned utc_line
    utc_line="<string>:1: DeprecationWarning: datetime.datetime.utcfromtimestamp() is deprecated and scheduled for removal in a future version. Use timezone-aware objects to represent datetimes in UTC: datetime.datetime.fromtimestamp(timestamp, datetime.UTC)."
    warned="$(python3 -I -c 'import datetime; datetime.datetime.utcfromtimestamp(0)' 2>&1 >/dev/null)" \
        || { echo "python3 failed on utcfromtimestamp: $warned"; return 1; }
    ver="$(printf '%s\n' "$got" | sed -n 1p)"
    local major="${ver%%.*}" minor="${ver#*.}"
    [ "$major" = 3 ] || { echo "python3 is $ver, not 3.x"; return 1; }
    if [ "$minor" -lt 11 ]; then want="string indices must be integers"; else want="string indices must be integers, not 'str'"; fi
    [ "$(printf '%s\n' "$got" | sed -n 2p)" = "$want" ] || problems+=("python3 $ver says '$(printf '%s\n' "$got" | sed -n 2p)', want '$want'")
    if [ "$minor" -lt 12 ]; then want=TypeError; else want=KeyError; fi
    [ "$(printf '%s\n' "$got" | sed -n 3p)" = "$want" ] || problems+=("python3 $ver raises $(printf '%s\n' "$got" | sed -n 3p) for a dict sliced, want $want")
    if [ "$minor" -ge 13 ]; then want="Illegal trailing comma before end of array: line 1 column 3 (char 2)"; else want="Expecting value: line 1 column 4 (char 3)"; fi
    [ "$(printf '%s\n' "$got" | sed -n 4p)" = "$want" ] || problems+=("python3 $ver says '$(printf '%s\n' "$got" | sed -n 4p)' at a trailing comma, want '$want'")
    if [ "$minor" -ge 12 ]; then want="$utc_line"; else want=""; fi
    [ "$warned" = "$want" ] || problems+=("python3 $ver warns '$warned' for utcfromtimestamp, want '$want'")
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$minor" = 12 ] || return 0
    echo "python3 here is 3.12, which words all four as the binary does, so the difference itself was not proven on this host (the macOS job's 3.9 proves the first two and the warning; a python3 of 3.13 or later, the trailing comma)"
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
                # bash 4+ says "Is a directory"; macOS bash 3.2 says "is a directory".
                grep -qiE ": $d/dir-helper: is a directory\$" "$d/err.bash" || problems+=("$mode: bash's stderr: $(head -c 300 "$d/err.bash")")
                grep -qx "exec: $d/dir-helper: Permission denied (os error 13)" "$d/err" || problems+=("$mode: the binary's stderr: $(head -c 300 "$d/err")")
                ;;
        esac
    done
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_env_answer_large: each case of the difference, the owner token
# and port on disk (stub mode envbig).
# - `wiki status`, the index about 1.4 MiB of notes: bash exits 126 (1 on
#   macOS, with its /bin/bash 3.2) with "Argument list too long" on stderr
#   and nothing on stdout; the binary exits 0, prints the status with
#   "  Notes:      60000" and nothing on stderr; both send the same
#   requests, the serve status then the index.
# - `mail send --body-file` of a body above 128 KiB: on Linux bash exits
#   126 with "Argument list too long" and sends nothing; the binary sends
#   it once and prints the queued line, exit 0. On macOS (one limit for
#   the whole environment) both send it and print the same: what was
#   checked there held, and the check says the mail case was not proven.
# - `zen history`, an answer of 12,000 files (about 1.2 MiB): on Linux
#   bash exits 126 with "Argument list too long" and nothing on stdout
#   (on macOS only its failing is checked); the binary exits 0, prints
#   two lines a file and nothing on stderr; both send the same request.
check_env_answer_large() {
    local d="$WORK/env-large" rc rr want=126 mail_proven=1 problems=()
    local queued="queued for approval (out_9) — track with 'k2 mail outbox out_9'"
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    python3 -I -c 'import sys; open(sys.argv[1], "w", encoding="utf-8").write("é and some words of the body " * 7000)' "$d/ws/big.txt"
    start_stub "$d" envbig || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    # wiki status
    rc=0; run_side "$d" bash wiki status || rc=$?
    cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
    rr=0; run_side "$d" binary wiki status || rr=$?
    [ "$(uname -s)" != Darwin ] || want=1
    [ "$rc" = "$want" ] || problems+=("wiki status: bash exited $rc, want $want: $(head -c 300 "$d/err.bash")")
    grep -q 'Argument list too long' "$d/err.bash" || problems+=("wiki status: bash's stderr has no 'Argument list too long': $(head -c 300 "$d/err.bash")")
    [ ! -s "$d/out.bash" ] || problems+=("wiki status: bash printed on stdout: $(head -c 300 "$d/out.bash")")
    [ "$rr" = 0 ] || problems+=("wiki status: the binary exited $rr, want 0: $(head -c 300 "$d/err")")
    grep -qx '  Notes:      60000' "$d/out" || problems+=("wiki status: the binary printed no note count: $(head -c 300 "$d/out")")
    [ ! -s "$d/err" ] || problems+=("wiki status: the binary wrote on stderr: $(head -c 300 "$d/err")")
    [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = 2 ] || problems+=("wiki status: bash sent, want two requests: $(head -c 300 "$d/log.bash")")
    cmp -s "$d/log.bash" "$d/log" || problems+=("wiki status: the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log")'")
    # zen history
    rc=0; run_side "$d" bash zen history || rc=$?
    cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
    rr=0; run_side "$d" binary zen history || rr=$?
    if [ "$(uname -s)" = Linux ]; then
        [ "$rc" = 126 ] || problems+=("zen history: bash exited $rc, want 126: $(head -c 300 "$d/err.bash")")
        grep -q 'Argument list too long' "$d/err.bash" || problems+=("zen history: bash's stderr has no 'Argument list too long': $(head -c 300 "$d/err.bash")")
        [ ! -s "$d/out.bash" ] || problems+=("zen history: bash printed on stdout: $(head -c 300 "$d/out.bash")")
    else
        [ "$rc" != 0 ] || problems+=("zen history: bash exited 0 on an answer above 1 MiB")
    fi
    [ "$rr" = 0 ] || problems+=("zen history: the binary exited $rr, want 0: $(head -c 300 "$d/err")")
    [ "$(wc -l <"$d/out" | tr -d ' ')" = 24000 ] || problems+=("zen history: the binary printed $(wc -l <"$d/out" | tr -d ' ') lines, want 24000: $(head -c 300 "$d/out")")
    [ ! -s "$d/err" ] || problems+=("zen history: the binary wrote on stderr: $(head -c 300 "$d/err")")
    [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = 1 ] || problems+=("zen history: bash sent, want one request: $(head -c 300 "$d/log.bash")")
    cmp -s "$d/log.bash" "$d/log" || problems+=("zen history: the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log")'")
    # mail send --body-file
    local mail=(mail send a@x.example --subject S --body-file big.txt)
    rc=0; run_side "$d" bash "${mail[@]}" || rc=$?
    cp "$d/out" "$d/out.bash"; cp "$d/err" "$d/err.bash"; cp "$d/log" "$d/log.bash"
    rr=0; run_side "$d" binary "${mail[@]}" || rr=$?
    stop_stub
    [ "$rr" = 0 ] || problems+=("mail send: the binary exited $rr, want 0: $(head -c 300 "$d/err")")
    [ "$(cat "$d/out")" = "$queued" ] || problems+=("mail send: the binary printed '$(head -c 300 "$d/out")', want '$queued'")
    [ ! -s "$d/err" ] || problems+=("mail send: the binary wrote on stderr: $(head -c 300 "$d/err")")
    [ "$(wc -l <"$d/log" | tr -d ' ')" = 1 ] && grep -q '^POST /cli/mail/send' "$d/log" \
        || problems+=("mail send: the binary sent, want one POST /cli/mail/send: $(head -c 300 "$d/log")")
    if [ "$(uname -s)" = Darwin ]; then
        mail_proven=0
        [ "$rc" = 0 ] || problems+=("mail send: bash on macOS exited $rc, want 0: $(head -c 300 "$d/err.bash")")
        cmp -s "$d/out.bash" "$d/out" || problems+=("mail send: bash on macOS printed '$(head -c 300 "$d/out.bash")'")
        cmp -s "$d/log.bash" "$d/log" || problems+=("mail send: bash on macOS sent '$(head -c 300 "$d/log.bash")'")
    else
        [ "$rc" = 126 ] || problems+=("mail send: bash exited $rc, want 126: $(head -c 300 "$d/err.bash")")
        grep -q 'Argument list too long' "$d/err.bash" || problems+=("mail send: bash's stderr has no 'Argument list too long': $(head -c 300 "$d/err.bash")")
        [ ! -s "$d/log.bash" ] || problems+=("mail send: bash sent: $(head -c 300 "$d/log.bash")")
    fi
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$mail_proven" = 1 ] && return 0
    echo "mail send: macOS has one limit for the whole environment, so bash sends the body as the binary does; the mail case is proven on Linux"
    return 3
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
# check_curl_config_files: a ~/.curlrc that adds a header, the stub in
# mode heads (each request's head logged), the owner token and port on
# disk, and both CLIs run `feedback list`. Bash's request carries
# "X-From-Curlrc: acme" once; the binary's never does. With that line
# and the hand-over headers masked, the requests are the same, and so are
# stdout, stderr and the exit code.
check_curl_config_files() {
    local d="$WORK/curlrc" rc=0 rr=0 r side tmp problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    printf 'header = "X-From-Curlrc: acme"\n' >"$d/home/.curlrc"
    start_stub "$d" heads || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    for side in bash binary; do
        : >"$d/log"
        tmp="$(mktemp -d "$d/tmp.XXXXXX")"
        if [ "$side" = bash ]; then set -- K5_CLI_VIA=rust bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
        r=0
        (cd "$d/ws" && exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" "$@" feedback list) \
            >"$d/out.$side" 2>"$d/err.$side" </dev/null || r=$?
        cp "$d/log" "$d/log.$side"
        if [ "$side" = bash ]; then rc=$r; else rr=$r; fi
    done
    stop_stub
    [ "$rc" = "$rr" ] || problems+=("bash exited $rc, the binary $rr")
    cmp -s "$d/out.bash" "$d/out.binary" || problems+=("stdout differs: bash '$(head -c 300 "$d/out.bash")', the binary '$(head -c 300 "$d/out.binary")'")
    cmp -s "$d/err.bash" "$d/err.binary" || problems+=("stderr differs: bash '$(head -c 300 "$d/err.bash")', the binary '$(head -c 300 "$d/err.binary")'")
    python3 -I - "$d/log.bash" "$d/log.binary" >"$d/verdict" <<'PY' || problems+=("$(cat "$d/verdict")")
import re, sys
RC = "X-From-Curlrc: acme"
bad = []

def blocks(p):
    return [b.split("\n") for b in open(p).read().split("\n\n") if b.strip()]

def norm(lines, native):
    # curl 8.5 and 8.7 write a query's %XX escapes in lower case.
    out = [re.sub("%[0-9a-fA-F]{2}", lambda m: m.group(0).upper(), lines[0])] if lines else []
    for l in lines[1:]:
        if l.startswith("User-Agent: "):
            out.append("User-Agent: <ua>")
        elif l.startswith("X-K5-Cli-Version: ") or l.startswith("X-K5-Cli-Noun: "):
            continue
        elif l.startswith("X-K5-Cli-Via: "):
            out.append("X-K5-Cli-Via: <via>")
        else:
            out.append(l)
    return out

b, r = blocks(sys.argv[1]), blocks(sys.argv[2])
if not b or len(b) != len(r):
    bad.append("bash sent %d requests, the binary %d" % (len(b), len(r)))
for i, (bl, rl) in enumerate(zip(b, r)):
    if bl.count(RC) != 1:
        bad.append("bash request %d has no %r: ~/.curlrc was not read, the difference is gone: %r" % (i, RC, bl))
    if RC in rl:
        bad.append("the binary's request %d has %r: it read ~/.curlrc: %r" % (i, RC, rl))
    bn = [l for l in norm(bl, False) if l != RC]
    rn = norm(rl, True)
    if bn != rn:
        bad.append("request %d differs beyond %r: bash %r, the binary %r" % (i, RC, bn, rn))
if bad:
    print("\n".join(bad))
    sys.exit(1)
PY
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_macos_system_proxy: on macOS, python3's urllib has the reader of
# the system proxy setting and calls it when no proxy variable is set,
# and the binary links no SystemConfiguration framework (the only place
# that setting is read from). Elsewhere there is no such setting: 3.
check_macos_system_proxy() {
    local libs problems=()
    if [ "$(uname -s)" != Darwin ]; then
        echo "no system proxy setting outside the environment on $(uname -s); checked on macOS only"
        return 3
    fi
    env -i PATH="$PATH" python3 -I -c '
import sys, urllib.request as u
ok = hasattr(u, "getproxies_macosx_sysconf") and u.getproxies is not u.getproxies_environment
sys.exit(0 if ok else 1)
' || problems+=("python3's urllib on this Mac does not read the system proxy setting: the difference is gone")
    libs="$(otool -L "$WORK/bin/k5" 2>&1)" || problems+=("otool -L failed on the binary: $libs")
    printf '%s\n' "$libs" | grep -q 'libSystem' || problems+=("otool -L listed no libSystem, so it read nothing: $(printf '%s' "$libs" | head -c 300)")
    ! printf '%s\n' "$libs" | grep -q 'SystemConfiguration' || problems+=("the binary links SystemConfiguration: it may read the system proxy setting")
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_curl_version_proxy_exit: a stub proxy that answers every CONNECT
# with 407, K2_HOST=https://cp.example.invalid (never resolves: only the
# proxy can reach it) and port 443, the owner token on disk, and both CLIs
# run `whoami`. The binary: exactly one CONNECT, for cp.example.invalid:443,
# a non-zero exit. Bash: 4 CONNECTs (its curl exits 7: the difference
# holds), or one (its curl exits 56: then stdout, stderr and the exit are
# the binary's, and it is not proven on this host: 3).
check_curl_version_proxy_exit() {
    local d="$WORK/proxy407" side r rc=0 rr=0 nb nr problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    start_stub "$d" refuse407 || { echo "the stub proxy didn't start"; return 1; }
    for side in bash binary; do
        : >"$d/log"
        if [ "$side" = bash ]; then set -- K5_CLI_VIA=rust bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
        r=0
        (cd "$d/ws" && exec env -i PATH="$PATH" HOME="$d/home" K2_HOST=https://cp.example.invalid K2_PORT=443 \
            https_proxy="http://127.0.0.1:$STUB_PORT" "$@" whoami) >"$d/out.$side" 2>"$d/err.$side" </dev/null || r=$?
        cp "$d/log" "$d/log.$side"
        if [ "$side" = bash ]; then rc=$r; else rr=$r; fi
    done
    stop_stub
    nb="$(grep -c '^proxy CONNECT cp.example.invalid:443 HTTP/1.1$' "$d/log.bash" || true)"
    nr="$(grep -c '^proxy CONNECT cp.example.invalid:443 HTTP/1.1$' "$d/log.binary" || true)"
    [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = "$nb" ] || problems+=("bash's log has more than its CONNECTs: $(tr '\n' ';' <"$d/log.bash")")
    [ "$(wc -l <"$d/log.binary" | tr -d ' ')" = "$nr" ] || problems+=("the binary's log has more than its CONNECTs: $(tr '\n' ';' <"$d/log.binary")")
    [ "$nr" = 1 ] || problems+=("the binary sent $nr CONNECTs, want 1")
    [ "$rr" != 0 ] || problems+=("the binary exited 0 past a refused tunnel")
    [ "$rc" != 0 ] || problems+=("bash exited 0 past a refused tunnel")
    ! grep -q 'over plain http' "$d/err.bash" "$d/err.binary" || problems+=("a plain-http warning for an https host")
    case "$nb" in
        4) ;;
        1)
            [ "$rc" = "$rr" ] || problems+=("bash's curl exits 56 here, yet bash exited $rc, the binary $rr")
            cmp -s "$d/out.bash" "$d/out.binary" || problems+=("stdout differs: bash '$(head -c 300 "$d/out.bash")', the binary '$(head -c 300 "$d/out.binary")'")
            cmp -s "$d/err.bash" "$d/err.binary" || problems+=("stderr differs: bash '$(head -c 300 "$d/err.bash")', the binary '$(head -c 300 "$d/err.binary")'")
            ;;
        *) problems+=("bash sent $nb CONNECTs, want 4 (a newer curl) or 1 (curl 8.5)") ;;
    esac
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    if [ "$nb" = 1 ]; then
        echo "bash's curl ($(curl --version | head -1 | cut -d' ' -f1-2)) exits 56 on a refused tunnel, as the binary: proven on a node whose curl exits 7"
        return 3
    fi
    return 0
}

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

# check_passport_heal_once: a stub daemon whose /cli/docs/tool answers
# by plan. gate: the first call is the auth gate's refusal (403 and its
# exact body) after it writes the session's new passport to the secrets
# folder, as the daemon mints one; any later call gets a tool answer.
# words: every call is a 200 tool answer whose text holds the refusal's
# words, after the same new passport. Both CLIs run `doc cat a.md` with the
# old passport over TCP (no owner token). gate: bash sends the tool call
# once and exits 3; the binary sends it twice, the old passport then the
# new, and prints the answer (exit 0). words: one call each, the same
# stdout, exit 0.
check_passport_heal_once() {
    local d="$WORK/heal" plan side rc tmp cli n first second problems=()
    mkdir -p "$d/home/.k2" "$d/ws" "$d/secrets"
    rm -f "$d/port" "$d/port.tmp"
    python3 -I -c '
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
d = sys.argv[1]
GATE = b"{\"error\":\"Invalid or missing auth token\"}"
def tool(text):
    return json.dumps({"stdout": text, "exit": 0, "doc": {"path": "/w/a.md"}}).encode()
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        with open(os.path.join(d, "log"), "a") as f:
            f.write("%s %s %s\n" % (self.command, path, self.headers.get("Authorization") or ""))
        status, body = 200, b"{}"
        if path == "/cli/docs/tool":
            plan = open(os.path.join(d, "plan")).read().strip()
            calls = os.path.join(d, "calls")
            k = len(open(calls).read()) if os.path.exists(calls) else 0
            open(calls, "a").write("x")
            if plan == "gate" and k == 0:
                open(os.path.join(d, "secrets", ".k2-passport"), "w").write("sessH.new2\n")
                status, body = 403, GATE
            elif plan == "gate":
                body = tool("healed\n")
            else:
                open(os.path.join(d, "secrets", ".k2-passport"), "w").write("sessH.new2\n")
                body = tool("Invalid or missing auth token\n")
        self.close_connection = True
        self.send_response(status)
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
    STUB_PID=$!
    for _ in $(seq 1 100); do [ -s "$d/port" ] && break; sleep 0.05; done
    STUB_PORT="$(cat "$d/port" 2>/dev/null || true)"
    [ -n "$STUB_PORT" ] || { kill "$STUB_PID" 2>/dev/null || true; echo "the stub daemon didn't start"; return 1; }
    for plan in gate words; do
        printf '%s\n' "$plan" >"$d/plan"
        for side in bash binary; do
            rm -f "$d/calls" "$d/secrets/.k2-passport"
            : >"$d/log"
            tmp="$(mktemp -d "$d/tmp.XXXXXX")"
            if [ "$side" = bash ]; then cli=(bash "$ROOT/cli/k2"); else cli=("$WORK/bin/k5"); fi
            rc=0
            (cd "$d/ws" && exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" K2_PORT="$STUB_PORT" \
                K2_HOOK_TOKEN=sessH.old1 K2_SECRETS_DIR="$d/secrets" "${cli[@]}" doc cat a.md) \
                >"$d/out.$side" 2>"$d/err.$side" </dev/null || rc=$?
            n="$(grep -c '^POST /cli/docs/tool ' "$d/log" || true)"
            first="$(grep '^POST /cli/docs/tool ' "$d/log" | sed -n 1p)"
            second="$(grep '^POST /cli/docs/tool ' "$d/log" | sed -n 2p)"
            [ "$first" = "POST /cli/docs/tool Bearer sessH.old1" ] || problems+=("$plan: $side's first tool call was '$first', want the old passport")
            case "$plan:$side" in
                gate:bash)
                    [ "$rc" = 3 ] || problems+=("gate: bash exited $rc, want 3: $(head -c 300 "$d/err.bash")")
                    [ "$n" = 1 ] || problems+=("gate: bash sent the tool call $n times, want once: the difference is gone")
                    ;;
                gate:binary)
                    [ "$rc" = 0 ] || problems+=("gate: the binary exited $rc, want 0: $(head -c 300 "$d/err.binary")")
                    [ "$n" = 2 ] || problems+=("gate: the binary sent the tool call $n times, want twice")
                    [ "$second" = "POST /cli/docs/tool Bearer sessH.new2" ] || problems+=("gate: the binary's second tool call was '$second', want the new passport")
                    [ "$(cat "$d/out.binary")" = healed ] || problems+=("gate: the binary printed '$(head -c 200 "$d/out.binary")', want the answer")
                    ;;
                words:*)
                    [ "$rc" = 0 ] || problems+=("words: $side exited $rc, want 0: $(head -c 300 "$d/err.$side")")
                    [ "$n" = 1 ] || problems+=("words: $side sent the tool call $n times, want once")
                    ;;
            esac
        done
        if [ "$plan" = words ]; then
            cmp -s "$d/out.bash" "$d/out.binary" || problems+=("words: stdout differs: bash '$(head -c 200 "$d/out.bash")', the binary '$(head -c 200 "$d/out.binary")'")
        fi
    done
    stop_stub
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_doc_mktemp_wording: both CLIs run `doc cat a.md` with TMPDIR
# naming a folder that isn't there, the owner token and a stub's port on
# disk. Both exit 2, print nothing on stdout, send nothing, and end with
# the same two lines; the first line is mktemp's: the binary's in GNU's
# words, bash's in its mktemp's. Where that mktemp is GNU's the two are
# exact, so the check says the difference itself was not proven here and
# returns 3 (the macOS job proves it).
# mktemp_tried_ok <head> <reason> <want exit> <exit> <line>: the exit is the
# one wanted and <line> is "<head><name>: <reason>", <name> one or more of
# [A-Za-z0-9.] (the template's six X, or the random name a newer macOS
# fills in). <head> and <reason> are literal: only the random name may differ.
mktemp_tried_ok() {
    local head=$1 why=$2 line=$5
    [ "$3" = "$4" ] || return 1
    [ "${line#"$head"}" != "$line" ] || return 1
    line=${line#"$head"}
    [ "${line%": $why"}" != "$line" ] || return 1
    line=${line%": $why"}
    [[ $line =~ ^[A-Za-z0-9.]+$ ]]
}

check_doc_mktemp_wording() {
    local d="$WORK/doc-mktemp" side rc cli gnu bsd why rcs=() problems=()
    mkdir -p "$d/home/.k2" "$d/ws"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    start_stub "$d" plain || { echo "the stub daemon didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    gnu="mktemp: failed to create directory via template '$d/no-tmp/k2doc.XXXXXX': No such file or directory"
    bsd="mktemp: mkdtemp failed on $d/no-tmp/k2doc.XXXXXX: No such file or directory"
    # A newer macOS mktemp names the folder it tried: the template's six X filled in.
    bsd_tried="mktemp: mkdtemp failed on $d/no-tmp/k2doc."
    why="No such file or directory"
    # The pattern's own rows: it takes a random name, and nothing else apart.
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried}aB3.x9: $why" \
        || problems+=("mktemp_tried_ok refuses a random name")
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried}XXXXXX: $why" \
        || problems+=("mktemp_tried_ok refuses the template")
    mktemp_tried_ok "$bsd_tried" "$why" 2 1 "${bsd_tried}aB3x9Q: $why" \
        && problems+=("mktemp_tried_ok takes another exit")
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried%k2doc.}k2dox.aB3x9Q: $why" \
        && problems+=("mktemp_tried_ok takes another prefix")
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried/no-tmp/other}aB3x9Q: $why" \
        && problems+=("mktemp_tried_ok takes another folder")
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried}aB/x9Q: $why" \
        && problems+=("mktemp_tried_ok takes a slash in the name")
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried}: $why" \
        && problems+=("mktemp_tried_ok takes no name")
    mktemp_tried_ok "$bsd_tried" "$why" 2 2 "${bsd_tried}aB3x9Q: Permission denied" \
        && problems+=("mktemp_tried_ok takes another reason")
    for side in bash binary; do
        : >"$d/log"
        if [ "$side" = bash ]; then cli=(bash "$ROOT/cli/k2"); else cli=("$WORK/bin/k5"); fi
        rc=0
        (cd "$d/ws" && exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$d/no-tmp" "${cli[@]}" doc cat a.md) \
            >"$d/out.$side" 2>"$d/err.$side" </dev/null || rc=$?
        rcs+=("$rc")
        [ "$rc" = 2 ] || problems+=("$side exited $rc, want 2: $(head -c 300 "$d/err.$side")")
        [ ! -s "$d/out.$side" ] || problems+=("$side printed on stdout: $(head -c 200 "$d/out.$side")")
        [ ! -s "$d/log" ] || problems+=("$side sent: $(head -c 300 "$d/log")")
        [ "$(wc -l <"$d/err.$side" | tr -d ' ')" = 3 ] || problems+=("$side wrote, want three lines: $(head -c 400 "$d/err.$side")")
    done
    stop_stub
    cmp -s <(sed 1d "$d/err.bash") <(sed 1d "$d/err.binary") \
        || problems+=("the lines after mktemp's differ: bash '$(sed 1d "$d/err.bash" | head -c 300)', the binary '$(sed 1d "$d/err.binary" | head -c 300)'")
    [ "$(sed -n 1p "$d/err.binary")" = "$gnu" ] || problems+=("the binary's mktemp line: '$(sed -n 1p "$d/err.binary")', want '$gnu'")
    case "$(sed -n 1p "$d/err.bash")" in
        "$gnu") ;;
        "$bsd") ;;
        *) mktemp_tried_ok "$bsd_tried" "$why" 2 "${rcs[0]}" "$(sed -n 1p "$d/err.bash")" || problems+=("bash's mktemp line is neither GNU's nor macOS's: '$(sed -n 1p "$d/err.bash")'") ;;
    esac
    if [ ${#problems[@]} -gt 0 ]; then
        printf '%s\n' "${problems[@]}"
        return 1
    fi
    [ "$(sed -n 1p "$d/err.bash")" != "$gnu" ] && return 0
    echo "mktemp here is GNU's, which words its line as the binary does: the two are exact here, so the difference itself was not proven on this host (the macOS job proves it)"
    return 3
}

# check_python_tls_error_text: an account server at https:// that speaks
# no TLS (it answers any bytes with a plain HTTP 400 and logs whether an
# HTTP request reached it); both CLIs run `connect login --token-stdin
# --subdomain acme`, the token on stdin. Both exit 1 with nothing on
# stdout and the same lines on stderr up to the last; that line starts
# "Error: could not list subdomains: <urlopen error " on both, then
# OpenSSL's "[SSL" on bash and the binary's "[SSL] "; the server saw no
# HTTP request from either.
check_python_tls_error_text() {
    local d="$WORK/tls" side rc tmp pid n problems=() last_b last_r
    local prefix="Error: could not list subdomains: <urlopen error "
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    : >"$d/tlslog"
    python3 -I -c '
import os, socket, sys
d = sys.argv[1]
s = socket.socket()
s.bind(("127.0.0.1", 0))
s.listen(8)
open(os.path.join(d, "tlsport.tmp"), "w").write(str(s.getsockname()[1]))
os.rename(os.path.join(d, "tlsport.tmp"), os.path.join(d, "tlsport"))
while True:
    c, _ = s.accept()
    c.settimeout(5)
    try:
        data = c.recv(65536)
    except Exception:
        data = b""
    with open(os.path.join(d, "tlslog"), "a") as f:
        f.write("http-request\n" if b"HTTP/1." in data else "no-http\n")
    try:
        c.sendall(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
    except Exception:
        pass
    c.close()
' "$d" &
    pid=$!
    for n in $(seq 1 50); do [ -s "$d/tlsport" ] && break; sleep 0.1; done
    [ -s "$d/tlsport" ] || { kill "$pid" 2>/dev/null; echo "the TLS stand-in didn't start"; return 1; }
    for side in bash binary; do
        tmp="$(mktemp -d "$d/tmp.XXXXXX")"
        rm -f "$d/home/.k2/connect-account.json"
        if [ "$side" = binary ]; then set -- "$WORK/bin/k5"; else set -- bash "$ROOT/cli/k2"; fi
        rc=0
        (cd "$d/ws" && printf 'jwt-tls-parity\n' | exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" \
            K2_CONNECT_DB_URL="https://127.0.0.1:$(cat "$d/tlsport")/" "$@" connect login --token-stdin --subdomain acme) \
            >"$d/out.$side" 2>"$d/err.$side" || rc=$?
        [ "$rc" = 1 ] || problems+=("$side exited $rc, want 1: $(head -c 300 "$d/err.$side")")
        [ ! -s "$d/out.$side" ] || problems+=("$side printed on stdout: $(head -c 200 "$d/out.$side")")
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    last_b="$(tail -n 1 "$d/err.bash")" last_r="$(tail -n 1 "$d/err.binary")"
    cmp -s <(sed '$d' "$d/err.bash") <(sed '$d' "$d/err.binary") \
        || problems+=("the lines before the last differ: bash '$(head -c 300 "$d/err.bash")', the binary '$(head -c 300 "$d/err.binary")'")
    case "$last_b" in "$prefix[SSL"*) ;; *) problems+=("bash's last line is not OpenSSL's: $last_b") ;; esac
    case "$last_r" in "$prefix[SSL] "*) ;; *) problems+=("the binary's last line is not its own TLS line: $last_r") ;; esac
    [ "$last_b" != "$last_r" ] || problems+=("the two last lines are the same: the difference no longer holds")
    [ "$(grep -c '^no-http$' "$d/tlslog")" = 2 ] || problems+=("want two connections with no HTTP request, got: $(cat "$d/tlslog")")
    ! grep -q '^http-request$' "$d/tlslog" || problems+=("an HTTP request reached the server")
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# no_ctty: runs its words in a new session, so there is no controlling
# terminal (/dev/tty can't be opened), whatever this shell has; its exit
# code is theirs.
no_ctty() {
    python3 -I -c '
import os, sys
pid = os.fork()
if pid == 0:
    os.setsid()
    os.execvp(sys.argv[1], sys.argv[1:])
_, st = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(st))
' "$@"
}

# connect_stub <dir> <mode>: a stub that is the Connect account server and
# the daemon in one, on a free loopback port, logging "METHOD PATH" and a
# body after it, one request a line, to <dir>/log. Mode refuse: the
# password grant answers 400 "Invalid login credentials". Mode biglist:
# the subdomain list answers 6000 rows (about 1.6 MB without their
# tunnel tokens), the claim [], the tunnel config and start their
# success. Sets STUB_PID and STUB_PORT; returns 1 when it doesn't start.
connect_stub() {
    local d="$1" mode="$2"
    rm -f "$d/port" "$d/port.tmp"
    : >"$d/log"
    python3 -I -c '
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
d, mode = sys.argv[1], sys.argv[2]
class H(BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        with open(os.path.join(d, "log"), "a") as f:
            f.write("%s %s %s\n" % (self.command, self.path, body))
        path = self.path.split("?")[0]
        status, out = 200, {"ok": True}
        if path == "/auth/v1/token":
            status, out = 400, {"error_description": "Invalid login credentials"}
        elif path == "/rest/v1/subdomains" and mode == "biglist":
            pad = "x" * 220
            out = [{"label": "s%04d" % i, "c": pad, "tunnel_token": "t%04d" % i} for i in range(6000)]
        elif path == "/rest/v1/rpc/claim_subdomain":
            out = []
        elif path == "/cli/tunnel/config":
            out = {"tokenSet": True, "subdomain": "s5999"}
        elif path == "/cli/tunnel/start":
            out = {"running": True, "public_url": "https://s5999.k2.dev"}
        b = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    do_GET = do_POST = _any
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

# check_getpass_no_tty_warning: both CLIs run `connect login --email
# me@acme.example --subdomain acme` in a new session (no terminal),
# "hunter2-5353" on stdin, against a stub whose password grant refuses.
# Both exit 1 with nothing on stdout and send the same one request (the
# grant, the password in its body). Bash's stderr has Python's warning
# once, with getpass.py's file and line (and maybe its source line under
# it), the binary's its own line once and no GetPassWarning; then
# "Warning: Password input may be echoed." on both; with bash's warning
# put as the binary's line, the two are the same.
check_getpass_no_tty_warning() {
    local d="$WORK/getpass" side rc tmp problems=()
    local mine="Warning: Can not control echo on the terminal."
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    connect_stub "$d" refuse || { echo "the stub didn't start"; return 1; }
    for side in bash binary; do
        tmp="$(mktemp -d "$d/tmp.XXXXXX")"
        rm -f "$d/home/.k2/connect-account.json"
        : >"$d/log"
        if [ "$side" = binary ]; then set -- "$WORK/bin/k5"; else set -- bash "$ROOT/cli/k2"; fi
        rc=0
        (cd "$d/ws" && printf 'hunter2-5353\n' | no_ctty env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" \
            K2_CONNECT_DB_URL="http://127.0.0.1:$STUB_PORT/" "$@" connect login --email me@acme.example --subdomain acme) \
            >"$d/out.$side" 2>"$d/err.$side" || rc=$?
        cp "$d/log" "$d/log.$side"
        [ "$rc" = 1 ] || problems+=("$side exited $rc, want 1: $(head -c 300 "$d/err.$side")")
        [ ! -s "$d/out.$side" ] || problems+=("$side printed on stdout: $(head -c 200 "$d/out.$side")")
    done
    stop_stub
    [ "$(grep -c 'getpass\.py:[0-9]*: GetPassWarning: Can not control echo on the terminal\.$' "$d/err.bash")" = 1 ] \
        || problems+=("bash's stderr has no single getpass warning with its file and line: $(head -c 400 "$d/err.bash")")
    [ "$(grep -cxF "$mine" "$d/err.binary")" = 1 ] || problems+=("the binary's stderr has no single line '$mine': $(head -c 400 "$d/err.binary")")
    ! grep -q GetPassWarning "$d/err.binary" || problems+=("the binary printed Python's GetPassWarning")
    for side in bash binary; do
        grep -qxF 'Warning: Password input may be echoed.' "$d/err.$side" \
            || problems+=("$side's stderr has no 'Warning: Password input may be echoed.': $(head -c 400 "$d/err.$side")")
    done
    awk -v mine="$mine" '
        skip && /^  / { skip = 0; next }
        { skip = 0 }
        match($0, /[^ ]*getpass\.py:[0-9]+: GetPassWarning: Can not control echo on the terminal\.$/) { print substr($0, 1, RSTART - 1) mine; skip = 1; next }
        { print }
    ' "$d/err.bash" >"$d/err.bash.mapped"
    cmp -s "$d/err.bash.mapped" "$d/err.binary" \
        || problems+=("the lines differ beyond the warning: bash '$(head -c 400 "$d/err.bash.mapped")', the binary '$(head -c 400 "$d/err.binary")'")
    [ "$(wc -l <"$d/log.bash" | tr -d ' ')" = 1 ] && grep -q '^POST /auth/v1/token?grant_type=password .*hunter2-5353' "$d/log.bash" \
        || problems+=("bash's requests, want the password grant: $(head -c 300 "$d/log.bash")")
    cmp -s "$d/log.bash" "$d/log.binary" \
        || problems+=("the requests differ: bash '$(head -c 300 "$d/log.bash")', the binary '$(head -c 300 "$d/log.binary")'")
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_picker_env_size: the stub's subdomain list is about 1.6 MB
# (mode biglist), the owner token and port on disk, and both CLIs run
# `connect login --token-stdin --subdomain s5999`. Bash exits 1 with
# "Argument list too long" on stderr and nothing on stdout, having sent
# only the list request; the binary exits 0 with "✓ live at
# https://s5999.k2.dev" and sends the list, the claim, the tunnel config
# and the start.
check_picker_env_size() {
    local d="$WORK/picker-env" side rc tmp problems=()
    mkdir -p "$d/home/.k2" "$d/ws/.k2"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    connect_stub "$d" biglist || { echo "the stub didn't start"; return 1; }
    printf '%s\n' "$STUB_PORT" >"$d/home/.k2/heartbeat.port"
    for side in bash binary; do
        tmp="$(mktemp -d "$d/tmp.XXXXXX")"
        rm -f "$d/home/.k2/connect-account.json"
        : >"$d/log"
        if [ "$side" = binary ]; then set -- "$WORK/bin/k5"; else set -- bash "$ROOT/cli/k2"; fi
        rc=0
        (cd "$d/ws" && printf 'jwt-env-parity\n' | exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" \
            K2_CONNECT_DB_URL="http://127.0.0.1:$STUB_PORT/" "$@" connect login --token-stdin --subdomain s5999) \
            >"$d/out.$side" 2>"$d/err.$side" || rc=$?
        echo "$rc" >"$d/rc.$side"
        awk '{ split($2, p, "?"); printf "%s%s %s", (NR > 1 ? "; " : ""), $1, p[1] } END { print "" }' "$d/log" >"$d/paths.$side"
    done
    stop_stub
    [ "$(cat "$d/rc.bash")" = 1 ] || problems+=("bash exited $(cat "$d/rc.bash"), want 1: $(head -c 300 "$d/err.bash")")
    grep -q 'Argument list too long' "$d/err.bash" || problems+=("bash's stderr has no 'Argument list too long': $(head -c 300 "$d/err.bash")")
    [ ! -s "$d/out.bash" ] || problems+=("bash printed on stdout: $(head -c 300 "$d/out.bash")")
    [ "$(cat "$d/paths.bash")" = "GET /rest/v1/subdomains" ] || problems+=("bash sent, want the list only: $(cat "$d/paths.bash")")
    [ "$(cat "$d/rc.binary")" = 0 ] || problems+=("the binary exited $(cat "$d/rc.binary"), want 0: $(head -c 300 "$d/err.binary")")
    [ "$(cat "$d/out.binary")" = "✓ live at https://s5999.k2.dev" ] || problems+=("the binary printed: $(head -c 300 "$d/out.binary")")
    ! grep -q 'Argument list' "$d/err.binary" || problems+=("the binary's stderr: $(head -c 300 "$d/err.binary")")
    [ "$(cat "$d/paths.binary")" = "GET /rest/v1/subdomains; POST /rest/v1/rpc/claim_subdomain; POST /cli/tunnel/config; POST /cli/tunnel/start" ] || problems+=("the binary sent, want the list, the claim, the config and the start: $(cat "$d/paths.binary")")
    [ ${#problems[@]} -eq 0 ] && return 0
    printf '%s\n' "${problems[@]}"
    return 1
}

# check_python_proxy_rule: with only ALL_PROXY set, bash's clone engine
# (Python urllib) sends no CONNECT and the binary sends one, never with the
# token in it.
check_python_proxy_rule() {
    local d="$WORK/pyproxy" side tmp pid port problems=()
    mkdir -p "$d/home/.k2" "$d/ws"
    printf 'owner-tok-parity\n' >"$d/home/.k2/heartbeat.token"
    printf '1\n' >"$d/home/.k2/heartbeat.port"
    : >"$d/heads"
    python3 -I - "$d" <<'PY' &
import os, socket, sys
d = sys.argv[1]
s = socket.socket()
s.bind(("127.0.0.1", 0))
s.listen(8)
with open(os.path.join(d, "port.tmp"), "w") as f:
    f.write(str(s.getsockname()[1]))
os.rename(os.path.join(d, "port.tmp"), os.path.join(d, "port"))
while True:
    c, _ = s.accept()
    c.settimeout(10)
    head = b""
    try:
        while b"\r\n\r\n" not in head:
            more = c.recv(4096)
            if not more:
                break
            head += more
    except OSError:
        pass
    with open(os.path.join(d, "heads"), "ab") as f:
        f.write(head.replace(b"\r\n", b" | ") + b"\n")
    try:
        c.sendall(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    except OSError:
        pass
    c.close()
PY
    pid=$!
    for _ in $(seq 1 50); do [ -s "$d/port" ] && break; sleep 0.1; done
    [ -s "$d/port" ] || { kill "$pid" 2>/dev/null; echo "the recording proxy didn't start"; return 1; }
    port="$(cat "$d/port")"
    for side in bash binary; do
        : >"$d/heads"
        tmp="$(mktemp -d "$d/tmp.XXXXXX")"
        if [ "$side" = bash ]; then set -- bash "$ROOT/cli/k2"; else set -- "$WORK/bin/k5"; fi
        (cd "$d/ws" && printf 'tok-pyproxy-parity\n' | exec env -i PATH="$PATH" HOME="$d/home" TMPDIR="$tmp" \
            ALL_PROXY="http://127.0.0.1:$port" "$@" clone preview ws1 --host https://cp.example.invalid --token-stdin --json) \
            >"$d/out.$side" 2>"$d/err.$side" || true
        sleep 0.2
        cp "$d/heads" "$d/heads.$side"
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    [ ! -s "$d/heads.bash" ] || problems+=("bash's Python used ALL_PROXY: $(head -c 300 "$d/heads.bash")")
    [ "$(wc -l <"$d/heads.binary" | tr -d ' ')" = 1 ] || problems+=("want one CONNECT from the binary, got: $(head -c 300 "$d/heads.binary")")
    grep -q '^CONNECT cp.example.invalid:443 HTTP/1.1 ' "$d/heads.binary" || problems+=("the binary's is not a CONNECT for cp.example.invalid:443: $(head -c 300 "$d/heads.binary")")
    ! grep -q 'tok-pyproxy-parity' "$d/heads.bash" "$d/heads.binary" || problems+=("the token reached the proxy")
    grep -q '"daemon_unreachable"' "$d/err.bash" || problems+=("bash didn't go straight there: $(head -c 300 "$d/err.bash")")
    grep -q 'Tunnel connection failed: 407' "$d/err.binary" || problems+=("the binary's refused tunnel isn't its line: $(head -c 300 "$d/err.binary")")
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
