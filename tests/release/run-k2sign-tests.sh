#!/usr/bin/env bash
# run-k2sign-tests.sh — run the k2sign tests on a compute Mac (k2n1-m4), never
# on z3mbp or z3mbpZ. Fetches minisign 0.12 (the zip install.sh pins, by its
# sha256) into a temp folder for PATH, then runs each test and prints its
# summary line. No test shows UI, posts a notification or asks for Touch ID.
#   k2 compute run k2n1-m4 --os macos --sha <commit> -- bash tests/release/run-k2sign-tests.sh [test.sh ...]
# With no names: every tests/release/k2sign-*.sh.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
[ "$(uname -s)" = Darwin ] || { echo "FAIL: the k2sign tests run on macOS" >&2; exit 1; }
work="$(mktemp -d "${TMPDIR:-/tmp}/k2sign-run.XXXXXX")"
trap 'rm -rf "$work"' EXIT
if ! command -v minisign >/dev/null 2>&1; then
    url="$(sed -n 's/^MINISIGN_ZIP_URL="\(.*\)"$/\1/p' "$REPO/scripts/k2sign/install.sh")"
    sum="$(sed -n 's/^MINISIGN_ZIP_SHA256="\(.*\)"$/\1/p' "$REPO/scripts/k2sign/install.sh")"
    [ -n "$url" ] && [ -n "$sum" ] || { echo "FAIL: no minisign pin in install.sh" >&2; exit 1; }
    /usr/bin/curl -fsSL --proto '=https' --tlsv1.2 -o "$work/m.zip" "$url" || { echo "FAIL: download $url" >&2; exit 1; }
    [ "$(shasum -a 256 "$work/m.zip" | cut -d' ' -f1)" = "$sum" ] || { echo "FAIL: minisign zip sha256 is not the pinned $sum" >&2; exit 1; }
    /usr/bin/ditto -x -k "$work/m.zip" "$work/x"
    mkdir -p "$work/bin"
    cp "$work/x/minisign" "$work/bin/minisign"
    chmod 0755 "$work/bin/minisign"
    export PATH="$work/bin:$PATH"
fi
echo "minisign: $(minisign -v 2>&1 | head -1)"
tests=("$@")
if [ "${#tests[@]}" = 0 ]; then
    for t in "$REPO"/tests/release/k2sign-*.sh; do tests+=("$(basename "$t")"); done
fi
rc=0
for t in "${tests[@]}"; do
    log="$work/$(basename "$t").log"
    if bash "$REPO/tests/release/$(basename "$t")" > "$log" 2>&1; then
        echo "PASS $(tail -1 "$log")"
    else
        rc=1
        echo "FAIL $(basename "$t"):"
        grep -E "FAIL|Traceback|Error|error" "$log" | head -40
        tail -3 "$log"
    fi
done
exit "$rc"
