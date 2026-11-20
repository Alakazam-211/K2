#!/usr/bin/env bash
# The name the CLI was called by changes nothing it does: the same output,
# on both streams, and the same exit code under every name (k2, k5, the
# old k2so shim's name, a copy under another name) and every value of
# K5_CLI_CALLED_AS, which the Rust front may still set. An alias's notice
# or tombstone is the front's job, from the brand file; the script reads
# neither $0's name nor that variable.
# bash-only: reads cli/k2 as source; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
unset K5_CLI_CALLED_AS

T="$(mktemp -d -t k2-called-as-XXXXXX)"
trap 'rm -rf "$T"; hermetic_cli_cleanup' EXIT
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

ln -s "$K2_CLI" "$T/k2"
ln -s "$K2_CLI" "$T/k5"
ln -s "$K2_CLI" "$T/k2so"
cp "$K2_CLI" "$T/some-copy"

for args in "--version" "study what" "nosuchverb"; do
    # The reference: the script under its own path, the variable unset.
    # shellcheck disable=SC2086
    want="$(bash "$K2_CLI" $args 2>&1)" && wcode=0 || wcode=$?
    [ -n "$want" ] || die "'$args': no output at all"
    for name in k2 k5 k2so some-copy; do
        for called in unset k2 k5 k2so "" K5 /usr/local/bin/k5; do
            if [ "$called" = unset ]; then
                # shellcheck disable=SC2086
                got="$(bash "$T/$name" $args 2>&1)" && code=0 || code=$?
            else
                # shellcheck disable=SC2086
                got="$(K5_CLI_CALLED_AS="$called" bash "$T/$name" $args 2>&1)" && code=0 || code=$?
            fi
            [ "$code" = "$wcode" ] || die "'$args' as $name, K5_CLI_CALLED_AS=$called: exit $code, wanted $wcode"
            [ "$got" = "$want" ] || die "'$args' as $name, K5_CLI_CALLED_AS=$called: the output differs"
            pass=$((pass + 1))
        done
    done
done

echo "called_as: $pass passed, 0 failed"
