#!/usr/bin/env bash
# The air-gap setting is read without python3.
#
# Regression: `k2 daemon install` (and every other air-gap refusal) read
# `airgap` from ~/.k2/settings.json with python3 only. With no python3, or
# a python3 that only stands in for one (macOS /usr/bin/python3 without the
# developer tools), the read failed and counted as off: `"airgap": true`
# was ignored and the install went on to GitHub. Now the shell reads it:
#   - "airgap": true refuses, with the same line and exit code as before;
#   - "airgap": false, a file with no "airgap" key, or no file goes on;
#   - a file that is there but can't be read, or an "airgap" value that is
#     not true or false, refuses too, saying the setting can't be read.
# Each row runs three ways: no python3 on PATH, a python3 stand-in that
# fails, and the normal PATH (python3 there). The refusal reads the same in
# all three.
#
# Hermetic: env -i, a temp HOME, stubs on PATH for uname (Linux x86_64) and
# curl (logs each call and refuses, so nothing is fetched). `--dry-run`, so
# nothing would be installed either way. Loud.
# Run with: bash tests/cli/airgap_no_python.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 required (the normal-PATH rows use it)" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-airgap-nopy-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# ── stubs ──
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
REAL_UNAME="$(command -v uname)"
cat >"$STUBS/uname" <<EOF
#!/bin/sh
case "\${1:-}" in
    -s) echo Linux ;;
    -m) echo x86_64 ;;
    *) exec "$REAL_UNAME" "\$@" ;;
esac
EOF
cat >"$STUBS/curl" <<'EOF'
#!/bin/sh
printf 'curl %s\n' "$*" >>"$STUB_LOG"
echo "stub curl: refusing $*" >&2
exit 22
EOF
chmod +x "$STUBS/uname" "$STUBS/curl"

# A PATH with no python3: every program on the normal PATH (first one of
# each name wins), linked into one folder, without python*.
NOPY="$WORK/nopy"
mkdir -p "$NOPY"
IFS=: read -r -a _dirs <<<"$PATH"
for d in "${_dirs[@]}"; do
    [ -d "$d" ] || continue
    for p in "$d"/*; do
        n="${p##*/}"
        case "$n" in python*) continue ;; esac
        [ -x "$p" ] && [ ! -d "$p" ] || continue
        [ -e "$NOPY/$n" ] || ln -s "$p" "$NOPY/$n"
    done
done
# The stand-in: the same, plus a python3 that only says it isn't there.
STANDIN="$WORK/standin"
mkdir -p "$STANDIN"
cat >"$STANDIN/python3" <<'EOF'
#!/bin/sh
echo "xcode-select: note: No developer tools were found, requesting install." >&2
exit 1
EOF
chmod +x "$STANDIN/python3"

PATH_NOPY="$STUBS:$NOPY"
PATH_STANDIN="$STUBS:$STANDIN:$NOPY"
PATH_NORMAL="$STUBS:$PATH"
if PATH="$PATH_NOPY" command -v python3 >/dev/null 2>&1; then
    echo "FAIL: the no-python PATH still has python3: $(PATH="$PATH_NOPY" command -v python3)" >&2
    exit 1
fi
[ "$(PATH="$PATH_STANDIN" command -v python3)" = "$STANDIN/python3" ] \
    || { echo "FAIL: the stand-in python3 is not first on its PATH" >&2; exit 1; }
PATH="$PATH_NORMAL" python3 -c 'import json' \
    || { echo "FAIL: no working python3 on the normal PATH" >&2; exit 1; }

REFUSE_ON="Error: air-gap is on (K2_AIRGAP=1). This command cannot phone Connect, GitHub, or other hosted services."
FETCH_FAIL="Failed to fetch daemon manifest: https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json"

# setup_home <name> <settings-kind>: a fresh HOME with ~/.k2 and the
# settings file the row wants; sets H, LOG.
setup_home() {
    H="$WORK/home-$1"
    LOG="$WORK/log-$1"
    rm -rf "$H"
    mkdir -p "$H/.k2"
    : >"$LOG"
    local s="$H/.k2/settings.json"
    case "$2" in
        on) printf '{\n  "theme": "dark",\n  "airgap": true,\n  "listenLan": false\n}\n' >"$s" ;;
        on-compact) printf '{"airgap":true}' >"$s" ;;
        off) printf '{\n  "theme": "dark",\n  "airgap": false\n}\n' >"$s" ;;
        nokey) printf '{\n  "theme": "dark"\n}\n' >"$s" ;;
        none) ;;
        garbled) printf '{\n  "airgap": "yes"\n}\n' >"$s" ;;
        truncated) printf '{\n  "theme": "dark",\n  "airgap":' >"$s" ;;
        notafile) mkdir "$s" ;;
        *) echo "FAIL: unknown settings kind $2" >&2; exit 1 ;;
    esac
}

# run_install <path>: `k2 daemon install --dry-run` with only that PATH;
# sets OUT, RC.
run_install() {
    set +e
    OUT="$(env -i PATH="$1" HOME="$H" STUB_LOG="$LOG" "$K2" daemon install --dry-run 2>&1)"
    RC=$?
    set -e
}

# The k5 binary with no python3 on PATH: `daemon install` is not built
# into it, so it hands nothing to bash and runs nothing, by the K2 ruling
# that scripts/cli-parity.sh names native-without-python3: one line naming
# Python 3, exit 69, and nothing fetched, whatever the settings say.
BIN_NOPY=""
NO_PYTHON_LINE="k5 daemon: this command is not built into k5 yet and needs Python 3."
expect_no_python() {
    local label="$1"
    if [ "$RC" -eq 69 ]; then ok "$label: exit 69 (the k5 binary)"; else bad "$label: exit $RC (want 69, the k5 binary)"; fi
    case "$OUT" in
        "$NO_PYTHON_LINE"*) ok "$label: the one line naming Python 3" ;;
        *) bad "$label: not the no-Python line"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2 ;;
    esac
    if [ -s "$LOG" ]; then
        bad "$label: reached the network:"; sed 's/^/      | /' "$LOG" >&2
    else
        ok "$label: nothing fetched"
    fi
}

expect_refused() {
    local label="$1" line="$2"
    [ -z "$BIN_NOPY" ] || { expect_no_python "$label"; return; }
    if [ "$RC" -eq 1 ]; then ok "$label: exit 1"; else bad "$label: exit $RC (want 1)"; fi
    case "$OUT" in
        *"$line"*) ok "$label: says why" ;;
        *) bad "$label: missing $(printf %q "$line")"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2 ;;
    esac
    if [ -s "$LOG" ]; then
        bad "$label: reached the network:"; sed 's/^/      | /' "$LOG" >&2
    else
        ok "$label: nothing fetched"
    fi
}

expect_fetched() {
    local label="$1"
    [ -z "$BIN_NOPY" ] || { expect_no_python "$label"; return; }
    if [ "$RC" -eq 1 ]; then ok "$label: exit 1 (the stub refused the fetch)"; else bad "$label: exit $RC (want 1)"; fi
    case "$OUT" in
        *"air-gap"*) bad "$label: refused as air-gap"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2 ;;
        *"$FETCH_FAIL"*) ok "$label: went on to the manifest fetch" ;;
        *) bad "$label: missing the fetch line"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2 ;;
    esac
    if grep -q 'releases/latest/download/daemon-latest.json' "$LOG"; then
        ok "$label: asked for the manifest"
    else
        bad "$label: the manifest was not asked for"; sed 's/^/      | /' "$LOG" >&2
    fi
}

UNREADABLE_TAIL="(\"airgap\" must be true or false), so k2 treats air-gap as on. This command cannot phone Connect, GitHub, or other hosted services."

for mode in nopy standin normal; do
    case "$mode" in
        nopy) P="$PATH_NOPY" ;;
        standin) P="$PATH_STANDIN" ;;
        normal) P="$PATH_NORMAL" ;;
    esac
    echo "== $mode =="
    BIN_NOPY=""
    [ "$mode" != nopy ] || [ "$CLI_UNDER_TEST" = "$CLI_BASH_SOURCE" ] || BIN_NOPY=1

    setup_home "$mode-on" on
    run_install "$P"
    expect_refused "$mode: \"airgap\": true" "$REFUSE_ON"
    eval "OUT_ON_$mode=\$OUT"

    setup_home "$mode-on-compact" on-compact
    run_install "$P"
    expect_refused "$mode: {\"airgap\":true} on one line" "$REFUSE_ON"

    setup_home "$mode-off" off
    run_install "$P"
    expect_fetched "$mode: \"airgap\": false"

    setup_home "$mode-nokey" nokey
    run_install "$P"
    expect_fetched "$mode: no \"airgap\" key"

    setup_home "$mode-none" none
    run_install "$P"
    expect_fetched "$mode: no settings file"

    setup_home "$mode-garbled" garbled
    run_install "$P"
    expect_refused "$mode: \"airgap\": \"yes\"" "Error: can't read the air-gap setting in $H/.k2/settings.json $UNREADABLE_TAIL"

    setup_home "$mode-truncated" truncated
    run_install "$P"
    expect_refused "$mode: a cut-off \"airgap\":" "Error: can't read the air-gap setting in $H/.k2/settings.json $UNREADABLE_TAIL"

    setup_home "$mode-notafile" notafile
    run_install "$P"
    expect_refused "$mode: settings.json can't be read" "Error: can't read the air-gap setting in $H/.k2/settings.json $UNREADABLE_TAIL"
done

echo "== the refusal reads the same with and without python3 =="
# The HOME path differs per mode; compare with it taken out.
norm() { printf '%s' "$1" | sed "s#$WORK/home-[a-z]*-on#HOME#g"; }
if [ "$CLI_UNDER_TEST" != "$CLI_BASH_SOURCE" ]; then
    # The binary's no-python3 run is its one line (checked above), not bash's.
    case "$OUT_ON_nopy" in
        "$NO_PYTHON_LINE"*) ok "no python3: the k5 binary's one line" ;;
        *) bad "no python3: the k5 binary printed something else"; printf '%s\n' "$OUT_ON_nopy" | sed 's/^/      | /' >&2 ;;
    esac
elif [ "$(norm "$OUT_ON_nopy")" = "$(norm "$OUT_ON_normal")" ]; then
    ok "no python3 = python3"
else
    bad "no python3 differs from python3"
    diff <(norm "$OUT_ON_nopy") <(norm "$OUT_ON_normal") | sed 's/^/      | /' >&2 || true
fi
if [ "$(norm "$OUT_ON_standin")" = "$(norm "$OUT_ON_normal")" ]; then
    ok "stand-in python3 = python3"
else
    bad "stand-in python3 differs from python3"
    diff <(norm "$OUT_ON_standin") <(norm "$OUT_ON_normal") | sed 's/^/      | /' >&2 || true
fi

echo ""
echo "airgap_no_python: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
