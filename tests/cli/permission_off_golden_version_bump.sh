#!/usr/bin/env bash
# The off-goldens survive a release.sh version bump (0.46.5 patch-up).
#
# On the v0.46.4 tip, 32 golden pages broke only because release.sh bumped
# K2_CLI_VERSION: the normalizer replaced every occurrence of the current
# version, so prose that names a release ("FILES (0.46.4, …)") flipped
# between a literal and <K2_CLI_VERSION> on each bump. The normalizer now
# folds the version only where the CLI prints it as its own version.
#
# This test copies cli/k2 into a throwaway dir, rewrites ONLY its
# K2_CLI_VERSION line (exactly what release.sh does to cli/k2), and runs the
# unchanged golden harness against the copy. Two fake versions:
#   * the next patch after today's (the bump release.sh will really make)
#   * one that no page mentions (9.87.6)
# Any golden that moves fails this test loudly.
#
# Hermetic like the harness itself (throwaway HOME, stub daemon, K2_* unset).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

CUR="$(grep -m1 '^K2_CLI_VERSION=' "$K2" | cut -d'"' -f2)"
[ -n "$CUR" ] || { echo "FAIL: no K2_CLI_VERSION in $K2" >&2; exit 1; }
case "$CUR" in
    *.*.*) ;;
    *) echo "FAIL: K2_CLI_VERSION '$CUR' is not x.y.z" >&2; exit 1 ;;
esac
NEXT_PATCH="${CUR%.*}.$(( ${CUR##*.} + 1 ))"

WORK="$(mktemp -d /tmp/k2pogv-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

fail=0
for fake in "$NEXT_PATCH" "9.87.6"; do
    [ "$fake" != "$CUR" ] || { echo "FAIL: fake version equals the real one ($CUR)" >&2; exit 1; }
    copy="$WORK/k2-$fake"
    # The same edit release.sh makes: the one K2_CLI_VERSION="…" line.
    sed "s/^K2_CLI_VERSION=\"$CUR\"\$/K2_CLI_VERSION=\"$fake\"/" "$K2" > "$copy"
    chmod +x "$copy"
    got="$(grep -m1 '^K2_CLI_VERSION=' "$copy" | cut -d'"' -f2)"
    [ "$got" = "$fake" ] || { echo "FAIL: the copy's version is '$got', want '$fake'" >&2; exit 1; }
    # The copy must report the fake version, or the test proves nothing.
    mkdir -p "$WORK/home" "$WORK/ws"
    reported="$(cd "$WORK/ws" && env -i PATH="$PATH" HOME="$WORK/home" K2_PORT=1 K2_HOOK_TOKEN=test-token "$copy" --version 2>"$WORK/version.err" || true)"
    [ "$reported" = "k2 $fake" ] || {
        echo "FAIL: '$copy --version' printed '$reported', want 'k2 $fake'; stderr:" >&2
        cat "$WORK/version.err" >&2
        exit 1
    }

    echo "== goldens against a CLI that says $fake (real: $CUR) =="
    if PERMISSION_GOLDEN_CLI="$copy" bash "$SCRIPT_DIR/permission_off_golden.sh"; then
        echo "  ok: $fake"
    else
        echo "  FAIL: a version bump to $fake moves the goldens (see the diffs above)" >&2
        fail=1
    fi
done

[ "$fail" -eq 0 ] || exit 1
echo "permission_off_golden_version_bump: ok"
