#!/usr/bin/env bash
# `k2 update` / `--app` / `--list` say so when GitHub can't be reached.
#
# Regression: under set -e a failed curl in `latest=$(curl ...)` stopped
# `k2 update` right after "Checking for app updates..." with curl's exit
# code and no word (`--list` the same, after its header). Now each form
# prints one line on stderr, "Error: couldn't reach GitHub (curl exit N:
# <curl's message>)", and exits 1; the request has a time limit, so a dead
# network ends.
#
# How: a curl stub first on PATH logs every call and plays one case per
# STUB_MODE: refused (exit 7), timed out (exit 28), HTTP 500 (exit 22 under
# -f; without -f it prints the error page and exits 0, as curl does), or ok
# (GitHub's two release URLs from fixtures). Like curl it prints its error
# only when -S is given or -s is not. Hermetic: env -i, a temp HOME and
# TMPDIR, no daemon, no network. Loud.
# Run with: bash tests/cli/update_app_github_unreachable.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 required (k2 update --app reads GitHub's JSON with it)" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

WORK="$(mktemp -d -t k2-update-unreachable-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
STUBS="$WORK/stubs"
H="$WORK/home"
T="$WORK/tmp"
LOG="$WORK/curl.log"
mkdir -p "$STUBS" "$H/.k2" "$T"
cat >"$STUBS/curl" <<'EOF'
#!/bin/sh
printf 'curl %s\n' "$*" >>"$STUB_LOG"
# The body goes where -o says (k2 update follows each redirect itself).
prev=""
for a in "$@"; do
    if [ "$prev" = "-o" ]; then exec >"$a"; fi
    prev="$a"
done
f=0; s=0; S=0; url=""
for a in "$@"; do
    case "$a" in
        --*) ;;
        -*) case "$a" in *f*) f=1 ;; esac
            case "$a" in *s*) s=1 ;; esac
            case "$a" in *S*) S=1 ;; esac ;;
        https://*) url="$a" ;;
    esac
done
# say <code> <message>: curl's error line, shown only as curl would.
say() {
    if [ "$S" = 1 ] || [ "$s" = 0 ]; then echo "curl: ($1) $2" >&2; fi
    exit "$1"
}
case "$STUB_MODE" in
    refused) say 7 "Failed to connect to api.github.com port 443 after 3 ms: Couldn't connect to server" ;;
    timeout) say 28 "Connection timed out after 10002 milliseconds" ;;
    http500)
        [ "$f" = 1 ] && say 22 "The requested URL returned error: 500"
        printf '<html><body>500 Internal Server Error</body></html>'
        exit 0 ;;
    ok)
        case "$url" in
            https://api.github.com/repos/Alakazam-211/K2/releases/latest)
                printf '{"tag_name":"v9.9.9","published_at":"2026-01-02T00:00:00Z"}'
                exit 0 ;;
            "https://api.github.com/repos/Alakazam-211/K2/releases?per_page=10")
                printf '[{"tag_name":"v9.9.9","published_at":"2026-01-02T00:00:00Z","prerelease":false}]'
                exit 0 ;;
        esac ;;
esac
echo "stub curl: no answer for mode '$STUB_MODE': $*" >&2
exit 99
EOF
chmod +x "$STUBS/curl"

LATEST_URL="https://api.github.com/repos/Alakazam-211/K2/releases/latest"
LIST_URL="https://api.github.com/repos/Alakazam-211/K2/releases?per_page=10"

# run <mode> <k2 update args...>: sets OUT, ERR, RC; clears the curl log.
# No daemon connection: these forms need none.
run() {
    local mode="$1"; shift
    : >"$LOG"
    set +e
    env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" TMPDIR="$T" STUB_LOG="$LOG" STUB_MODE="$mode" \
        "$K2" update "$@" >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
}

# asked <label> <url>: exactly one request, for <url>, with a time limit.
asked() {
    local n
    n="$(grep -c . "$LOG" || true)"
    [ "$n" = 1 ] && ok "$1: one request" || { bad "$1: $n requests"; show "$(cat "$LOG")"; }
    grep -qF -- " $2" "$LOG" && ok "$1: asked $2" || { bad "$1: did not ask $2"; show "$(cat "$LOG")"; }
    grep -q -- '--connect-timeout ' "$LOG" && grep -q -- '--max-time ' "$LOG" \
        && ok "$1: the request has a time limit" || { bad "$1: no --connect-timeout/--max-time"; show "$(cat "$LOG")"; }
}

no_temp_left() {
    if [ -z "$(ls -A "$T")" ]; then ok "$1: no temp file left"; else bad "$1: left $(ls -A "$T")"; fi
}

echo "== GitHub out of reach: one line on stderr, exit 1 =="
for mode in refused timeout http500; do
    case "$mode" in
        refused) want="Error: couldn't reach GitHub (curl exit 7: Failed to connect to api.github.com port 443 after 3 ms: Couldn't connect to server)" ;;
        timeout) want="Error: couldn't reach GitHub (curl exit 28: Connection timed out after 10002 milliseconds)" ;;
        http500) want="Error: couldn't reach GitHub (curl exit 22: The requested URL returned error: 500)" ;;
    esac
    for form in "" "--app" "--list"; do
        # shellcheck disable=SC2086
        run "$mode" $form
        label="$mode: k2 update${form:+ $form}"
        [ "$RC" -eq 1 ] && ok "$label: exit 1" || bad "$label: exit $RC (want 1)"
        [ "$ERR" = "$want" ] && ok "$label: the one stderr line" || { bad "$label: stderr is not the one line"; show "$ERR"; }
        if [ "$form" = "--list" ]; then
            [ -z "$OUT" ] && ok "$label: nothing on stdout" || { bad "$label: stdout"; show "$OUT"; }
            asked "$label" "$LIST_URL"
        else
            [ "$OUT" = "Checking for app updates..." ] && ok "$label: stdout stops at the check line" \
                || { bad "$label: stdout"; show "$OUT"; }
            asked "$label" "$LATEST_URL"
        fi
        no_temp_left "$label"
    done
done

echo "== GitHub answers: the versions, nothing on stderr =="
run ok --app
[ "$RC" -eq 0 ] && ok "ok: k2 update --app: exit 0" || bad "ok: k2 update --app: exit $RC (want 0)"
[ -z "$ERR" ] && ok "ok: k2 update --app: stderr empty" || { bad "ok: k2 update --app: stderr"; show "$ERR"; }
case "$OUT" in *"  Latest:  v9.9.9"*) ok "ok: k2 update --app: the latest tag" ;; *) bad "ok: k2 update --app: no latest tag"; show "$OUT" ;; esac
asked "ok: k2 update --app" "$LATEST_URL"
no_temp_left "ok: k2 update --app"
run ok --list
[ "$RC" -eq 0 ] && ok "ok: k2 update --list: exit 0" || bad "ok: k2 update --list: exit $RC (want 0)"
[ -z "$ERR" ] && ok "ok: k2 update --list: stderr empty" || { bad "ok: k2 update --list: stderr"; show "$ERR"; }
[ "$OUT" = "$(printf 'Available K2 releases:\n  v9.9.9  2026-01-02')" ] && ok "ok: k2 update --list: the header and the release" \
    || { bad "ok: k2 update --list: stdout"; show "$OUT"; }
asked "ok: k2 update --list" "$LIST_URL"
no_temp_left "ok: k2 update --list"

echo ""
echo "update_app_github_unreachable: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
