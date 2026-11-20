#!/usr/bin/env bash
# `k2 update --help` prints the update usage and sends nothing.
#
# Regression: cmd_update_v2 skipped -h/--help as an unknown word, so
# `k2 update --help` ran the --app form and asked GitHub for the latest
# release. Now -h/--help anywhere in `k2 update` prints the usage (stdout,
# exit 0, nothing on stderr) and sends no request, with or without a
# daemon connection (no PORT or token needed).
#
# A bare `--` anywhere on `k2 update` is a usage error: exit 2, nothing
# sent. update runs no command, and the words after `--` are never help,
# so without this `k2 update -- -h` would run the update.
#
# How: a curl stub first on PATH logs every call and refuses; the CLI's
# GitHub and daemon requests both go through curl. Every form must leave
# the log empty. Then the plain `k2 update` still asks GitHub (the stub
# sees one call), so the stub is known to catch it.
#
# Hermetic: env -i, a temp HOME, no daemon, no network. Loud.
# Run with: bash tests/cli/update_help_no_request.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-update-help-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
STUBS="$WORK/stubs"
H="$WORK/home"
LOG="$WORK/curl.log"
mkdir -p "$STUBS" "$H/.k2"
: >"$LOG"
cat >"$STUBS/curl" <<'EOF'
#!/bin/sh
printf 'curl %s\n' "$*" >>"$STUB_LOG"
echo "stub curl: refusing $*" >&2
exit 22
EOF
chmod +x "$STUBS/curl"

USAGE_FIRST="Usage: k2 update [--app|--cli [--check]|--list]"

# run <conn|noconn> <args...>: sets OUT (stdout), ERR (stderr), RC.
# conn hands the CLI a dead port and a dummy token; noconn gives neither.
run() {
    local how="$1"; shift
    local conn=()
    [ "$how" = conn ] && conn=(K2_PORT=1 K2_HOOK_TOKEN=test-token)
    set +e
    OUT="$(env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" STUB_LOG="$LOG" ${conn[@]+"${conn[@]}"} \
        "$K2" update "$@" 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

run noconn --help
REF="$OUT"
case "$REF" in
    "$USAGE_FIRST"*) ok "k2 update --help starts with the usage line" ;;
    *) bad "k2 update --help: no usage line"; printf '%s\n' "$REF$ERR" | sed 's/^/      | /' >&2 ;;
esac
case "$REF" in
    *"None of these downloads or installs anything."*) ok "the usage says nothing is downloaded" ;;
    *) bad "the usage does not say nothing is downloaded" ;;
esac

for how in conn noconn; do
    for form in "--help" "-h" "--app --help" "--app -h" "--list --help" "--help --list" \
        "--cli --help" "--cli --check -h" "extra --help"; do
        : >"$LOG"
        # shellcheck disable=SC2086
        run "$how" $form
        label="$how: k2 update $form"
        [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
        [ "$OUT" = "$REF" ] && ok "$label: prints the usage" \
            || { bad "$label: output differs from k2 update --help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
        [ -z "$ERR" ] && ok "$label: nothing on stderr" \
            || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
        if [ -s "$LOG" ]; then
            bad "$label: sent a request:"; sed 's/^/      | /' "$LOG" >&2
        else
            ok "$label: sent nothing"
        fi
    done
done

echo "== a bare -- is a usage error, nothing sent =="
for how in conn noconn; do
    for form in "-- -h" "-- --help" "--" "--app -- -h" "--cli -- --check" "-h --"; do
        : >"$LOG"
        # shellcheck disable=SC2086
        run "$how" $form
        label="$how: k2 update $form"
        [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
        [ -z "$OUT" ] && ok "$label: nothing on stdout" \
            || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
        case "$ERR" in
            *"k2 update takes no '--'"*) ok "$label: says why" ;;
            *) bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2 ;;
        esac
        if [ -s "$LOG" ]; then
            bad "$label: sent a request:"; sed 's/^/      | /' "$LOG" >&2
        else
            ok "$label: sent nothing"
        fi
    done
done

echo "== the stub sees the plain form's request =="
: >"$LOG"
run conn
if grep -q 'api.github.com/repos/Alakazam-211/K2/releases/latest' "$LOG"; then
    ok "k2 update (no --help) still asks GitHub"
else
    bad "k2 update (no --help) sent no GitHub request"; sed 's/^/      | /' "$LOG" >&2
fi
case "$OUT" in
    *"$USAGE_FIRST"*) bad "k2 update (no --help) printed the usage" ;;
    *) ok "k2 update (no --help) is not the usage" ;;
esac

echo ""
echo "update_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
