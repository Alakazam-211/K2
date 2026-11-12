#!/usr/bin/env bash
# One command for every generated file, in the order they depend on each
# other.
#
#   scripts/regen.sh                  run every generator
#   scripts/regen.sh --check          change nothing; exit 1 if anything is stale
#   scripts/regen.sh --bless-goldens  also re-capture the CLI goldens (see 5)
#   scripts/regen.sh --no-cargo       skip step 1 (a machine without cargo)
#
# The rule for a merge or rebase conflict in a generated file: take either
# side, finish the merge, run this, commit what it changed. Never merge a
# generated file by hand.
#
# The steps:
#   0. build-cli.sh            cli/src/*.sh -> cli/k2, once cli/k2 is built
#                              from parts (there is a cli/src/ORDER)
#   1. contract-gen            catalog.json -> the fenced blocks in cli/k2,
#                              app_surface.rs, sdk/generated/*, the server
#                              SDK's verbs.ts, the renderer's verb tables
#   2. sdk-server-dist.mjs     sdk/server/src -> sdk/server/dist (reads 1)
#   3. gen-cli-explorer-catalog.mjs   cli/k2 -> cli-catalog.generated.json (reads 1)
#   4. build-styles.mjs        styles -> styles.generated.css / .ts
#   5. the CLI goldens (tests/golden/permission_off): only checked. They
#      pin what the CLI prints with permissions off, so a difference is for
#      a person to read first. After reading it, --bless-goldens re-captures.
#
# --check runs every step even after one fails, then lists what is stale.
#
# For tests: --root DIR runs against another tree.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK=0
BLESS=0
CARGO=1
while [ $# -gt 0 ]; do
    case "$1" in
        --check) CHECK=1 ;;
        --bless-goldens) BLESS=1 ;;
        --no-cargo) CARGO=0 ;;
        --root) ROOT="$2"; shift ;;
        *) echo "regen: unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done
if [ "$CHECK" = 1 ] && [ "$BLESS" = 1 ]; then
    echo "regen: --check changes nothing, so it can't take --bless-goldens" >&2
    exit 2
fi
cd "$ROOT"

stale=""
skipped=""
# run <name> <command...>: in --check a failing step is noted and the rest
# still run; otherwise the first failure stops the script.
run() {
    local name="$1"; shift
    echo "regen: $name"
    if [ "$CHECK" = 1 ]; then
        "$@" || stale="$stale${stale:+, }$name"
    else
        "$@"
    fi
}
flag() { if [ "$CHECK" = 1 ]; then echo "--check"; fi; }

if [ -f cli/src/ORDER ]; then
    # shellcheck disable=SC2046  # the flag is one word or nothing
    run "cli/k2 from its parts" bash scripts/build-cli.sh $(flag)
fi
if [ "$CARGO" = 1 ]; then
    if [ "$CHECK" = 1 ]; then
        run "contract-gen" cargo run --locked -q -p k2-core --bin contract-gen -- --check
    else
        run "contract-gen" cargo run --locked -q -p k2-core --bin contract-gen
    fi
else
    skipped="contract-gen (--no-cargo)"
    echo "regen: contract-gen SKIPPED (--no-cargo): the steps below read what it writes"
fi
# shellcheck disable=SC2046  # the flag is one word or nothing
run "server SDK dist" node scripts/sdk-server-dist.mjs $(flag)
# shellcheck disable=SC2046
run "CLI explorer catalog" node scripts/gen-cli-explorer-catalog.mjs $(flag)
# shellcheck disable=SC2046
run "styles" node scripts/build-styles.mjs $(flag)

if [ "$BLESS" = 1 ]; then
    run "CLI goldens (re-capture)" env PERMISSION_GOLDEN_BLESS=1 bash tests/cli/permission_off_golden.sh
elif [ "$CHECK" = 1 ]; then
    run "CLI goldens" bash tests/cli/permission_off_golden.sh
else
    echo "regen: CLI goldens (check only)"
    if ! bash tests/cli/permission_off_golden.sh; then
        echo "regen: the CLI goldens differ (above). Read the difference; if it is what you meant, run: scripts/regen.sh --bless-goldens" >&2
        exit 1
    fi
fi

if [ -n "$stale" ]; then
    echo "regen: STALE: $stale. Run scripts/regen.sh and commit what it changes." >&2
    exit 1
fi
if [ -n "$skipped" ]; then
    echo "regen: done, but skipped: $skipped."
else
    echo "regen: done; everything generated is current."
fi
