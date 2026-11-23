#!/usr/bin/env bash
# A value flag given an empty value ('') is a usage error that exits 1, on
# every bash, macOS's /bin/bash 3.2 included.
#
# The voice and hostchat option loops read those values with `${2:?<hint>}`.
# Bash 5 prints "<cli path>: line <n>: 2: <hint>" and exits 1. macOS's
# /bin/bash 3.2, under set -e with cli/k2's EXIT trap set
# (_permission_note_flush), printed the same line and exited 0: a false
# success. The loops now call _need_nonempty_value, which prints the same
# line and exits 1 itself. Rows, one per value flag:
# - k2 voice: --room, --after, --limit, --recording, --call, --notice,
#   --require-recording, --start-level;
# - k2 hostchat enable (and upgrade, purge, config): --tarball,
#   --recovery-key-file, --identity, --voice-url, --media-tarball, --node-ip,
#   --accept-binary, --confirm-hostname, --public-url;
# - k2 hostchat media install and probe: --url, --voice-url, --public-url,
#   --node-ip, --tarball, --media-tarball, --udp, --tcp, --turn-tls;
# - k2 hostchat users invite: --display-name, --role, --email.
#
# Each row wants exit 1, nothing on stdout, and exactly the one line on
# stderr, whose line number points at the flag's line in cli/k2.
#
# BASH_UNDER_TEST picks the shell (default /bin/bash). WANT_BASH_VERSION
# (for example 3.2), when set, must be that shell's major.minor, so a job
# proves which bash it ran. No daemon: HOME is a temp dir with port 1 (a
# closed port, checked first) and a dummy owner token, the environment is
# emptied, and the working folder is a temp dir. Fail loud; no skips.
# Run with: BASH_UNDER_TEST=/bin/bash bash tests/cli/bash32_guard_exit.sh
# bash-only: runs cli/k2 as source under a chosen bash; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_BASH_SOURCE"

SH="${BASH_UNDER_TEST:-/bin/bash}"
[ -x "$SH" ] || { echo "FAIL: BASH_UNDER_TEST is not an executable file: $SH" >&2; exit 1; }
ver="$("$SH" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
if [ -n "${WANT_BASH_VERSION:-}" ] && [ "$ver" != "$WANT_BASH_VERSION" ]; then
    echo "FAIL: $SH is bash $ver, want $WANT_BASH_VERSION" >&2
    exit 1
fi
echo "== cli/k2 under $SH ($ver)"

WORK="$(mktemp -d -t k2-bash32-guard-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/home/.k2" "$WORK/ws"
printf '1\n' >"$WORK/home/.k2/heartbeat.port"
printf 'owner-tok-test\n' >"$WORK/home/.k2/heartbeat.token"

# Nothing may answer on port 1, or a row could reach a daemon.
if (exec 3<>/dev/tcp/127.0.0.1/1) 2>/dev/null; then
    echo "FAIL: something listens on 127.0.0.1:1; this test needs it closed" >&2
    exit 1
fi

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

# Rows: "<words before the flag>;<flag>;<hint>". The flag gets ''.
ROWS=(
    "voice recordings;--room;--room <room>"
    "voice transcript r1;--after;--after <id>"
    "voice transcript r1;--limit;--limit N"
    "voice transcripts reads;--recording;--recording <id>"
    "voice leave;--call;--call <callId>"
    "voice consent r1 yes;--notice;--notice <version>"
    "voice settings;--require-recording;--require-recording on|off|inherit"
    "voice settings;--start-level;--start-level none|moderator|member|inherit"
    "hostchat enable;--tarball;--tarball FILE"
    "hostchat enable;--recovery-key-file;--recovery-key-file FILE"
    "hostchat enable;--identity;--identity k2|standalone"
    "hostchat enable;--voice-url;--voice-url wss://HOST"
    "hostchat enable;--media-tarball;--media-tarball FILE"
    "hostchat enable;--node-ip;--node-ip IP"
    "hostchat upgrade;--accept-binary;--accept-binary SHA256"
    "hostchat purge;--confirm-hostname;--confirm-hostname HOST"
    "hostchat config;--public-url;--public-url https://HOST"
    "hostchat media install;--url;--url wss://HOST"
    "hostchat media install;--voice-url;--url wss://HOST"
    "hostchat media install;--public-url;--url wss://HOST"
    "hostchat media install;--node-ip;--node-ip IP"
    "hostchat media install;--tarball;--tarball FILE"
    "hostchat media install;--media-tarball;--tarball FILE"
    "hostchat media probe;--udp;--udp ok|fail"
    "hostchat media probe;--tcp;--tcp ok|fail"
    "hostchat media probe;--turn-tls;--turn-tls ok|fail"
    "hostchat users invite ann;--display-name;--display-name NAME"
    "hostchat users invite ann;--role;--role member|admin|guest"
    "hostchat users invite ann;--email;--email EMAIL"
)

for row in "${ROWS[@]}"; do
    IFS=';' read -r words flag hint <<<"$row"
    name="k2 $words $flag ''"
    rc=0
    # shellcheck disable=SC2086 # words are plain words
    (cd "$WORK/ws" && env -i PATH="$PATH" HOME="$WORK/home" K2_HOST=127.0.0.1 \
        "$SH" "$K2" $words "$flag" "" >"$WORK/out" 2>"$WORK/err" </dev/null) || rc=$?
    if [ "$rc" != 1 ]; then
        bad "$name: exit $rc, want 1 (stderr: $(head -c 300 "$WORK/err"))"
        continue
    fi
    if [ -s "$WORK/out" ]; then
        bad "$name: printed on stdout: $(head -c 200 "$WORK/out")"
        continue
    fi
    lines="$(wc -l <"$WORK/err" | tr -d ' ')"
    err="$(cat "$WORK/err")"
    if [ "$lines" != 1 ]; then
        bad "$name: want one line on stderr, got $lines: $(head -c 300 "$WORK/err")"
        continue
    fi
    n="${err#"$K2: line "}"
    n="${n%%": 2: "*}"
    case "$n" in
        ''|*[!0-9]*) bad "$name: stderr is not '$K2: line <n>: 2: $hint': $err"; continue ;;
    esac
    if [ "$err" != "$K2: line $n: 2: $hint" ]; then
        bad "$name: stderr is '$err', want '$K2: line $n: 2: $hint'"
        continue
    fi
    src="$(sed -n "${n}p" "$K2")"
    case "$src" in
        *"$flag"*"$hint"*) ok ;;
        *) bad "$name: line $n of cli/k2 is not the flag's line: $src" ;;
    esac
done

echo "bash32_guard_exit ($ver): $pass passed, $fail failed (of ${#ROWS[@]})"
[ "$fail" = 0 ] && [ "$pass" = "${#ROWS[@]}" ]
