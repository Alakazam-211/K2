#!/usr/bin/env bash
# A bare `k2 daemon` prints what `k2 daemon status` prints.
#
# Regression (0.46.17): the router's daemon arm takes status as the default
# word, but its status arm shifted two words. With only `daemon` there was
# one, so the shift failed under set -e: exit 1 and nothing printed.
#
# Now `k2 daemon` is `k2 daemon status`: the same stdout, exit 0, nothing
# on stderr, with and without --json after `status`. The other words still
# go where they went: an unknown word prints the daemon usage (exit 1).
#
# How: stand-ins first on PATH for systemctl (Linux) and launchctl (macOS)
# report a running unit with a fixed PID, and curl logs and refuses. HOME
# is a temp dir with no port file, so nothing asks a daemon. Status only
# reads. Never touches the real ~/.k2 or the running daemon. Loud.
# Run with: bash tests/cli/daemon_bare_status.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-daemon-bare-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
STUBS="$WORK/stubs"
H="$WORK/home"
LOG="$WORK/stub.log"
mkdir -p "$STUBS" "$H/.k2"
: >"$LOG"

cat >"$STUBS/curl" <<'EOF'
#!/bin/sh
printf 'curl %s\n' "$*" >>"$STUB_LOG"
exit 7
EOF
cat >"$STUBS/systemctl" <<'EOF'
#!/bin/sh
case "$*" in
    "--user cat k2-daemon.service") exit 0 ;;
    *is-active*) echo active ;;
    *MainPID*) echo 4242 ;;
    *) exit 1 ;;
esac
EOF
cat >"$STUBS/launchctl" <<'EOF'
#!/bin/sh
case "$1" in
    print) printf '\tpid = 4242\n\tlast exit code = 0\n' ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$STUBS/curl" "$STUBS/systemctl" "$STUBS/launchctl"

# run <args...>: sets OUT (stdout), ERR (stderr), RC.
run() {
    set +e
    OUT="$(env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" STUB_LOG="$LOG" K2_PORT=1 K2_HOOK_TOKEN=test-token \
        "$K2" "$@" 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

echo "== k2 daemon status (the reference) =="
run daemon status
REF="$OUT"
[ "$RC" -eq 0 ] && ok "k2 daemon status: exit 0" || bad "k2 daemon status: exit $RC (want 0)"
case "$REF" in
    "K2 Daemon"*) ok "k2 daemon status: starts with K2 Daemon" ;;
    *) bad "k2 daemon status: no header"; printf '%s\n' "$REF$ERR" | sed 's/^/      | /' >&2 ;;
esac
case "$REF" in
    *4242*) ok "k2 daemon status: reads the stand-in's PID" ;;
    *) bad "k2 daemon status: no PID 4242"; printf '%s\n' "$REF" | sed 's/^/      | /' >&2 ;;
esac

echo "== a bare k2 daemon is k2 daemon status =="
run daemon
[ "$RC" -eq 0 ] && ok "k2 daemon: exit 0" || bad "k2 daemon: exit $RC (want 0)"
[ "$OUT" = "$REF" ] && ok "k2 daemon: prints what k2 daemon status prints" \
    || { bad "k2 daemon: output differs from k2 daemon status"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
[ -z "$ERR" ] && ok "k2 daemon: nothing on stderr" \
    || { bad "k2 daemon: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }

echo "== status --json still reads its flag =="
run daemon status --json
[ "$RC" -eq 0 ] && ok "k2 daemon status --json: exit 0" || bad "k2 daemon status --json: exit $RC (want 0)"
if python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["pid"] == "4242", d' "$OUT"; then
    ok "k2 daemon status --json: JSON with the PID"
else
    bad "k2 daemon status --json: not the status JSON"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2
fi

echo "== an unknown word still prints the usage =="
run daemon nope
[ "$RC" -eq 1 ] && ok "k2 daemon nope: exit 1" || bad "k2 daemon nope: exit $RC (want 1)"
case "$ERR" in
    "Usage: k2 daemon <status|"*) ok "k2 daemon nope: the usage on stderr" ;;
    *) bad "k2 daemon nope: no usage"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2 ;;
esac
[ -z "$OUT" ] && ok "k2 daemon nope: nothing on stdout" || bad "k2 daemon nope: stdout: $OUT"

if [ -s "$LOG" ]; then
    bad "a daemon request was sent:"; sed 's/^/      | /' "$LOG" >&2
else
    ok "no daemon request at all"
fi

echo ""
echo "daemon_bare_status: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
