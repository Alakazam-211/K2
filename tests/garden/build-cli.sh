#!/usr/bin/env bash
# Tests for scripts/build-cli.sh on a throwaway tree.
# Run: bash tests/garden/build-cli.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
B="$HERE/scripts/build-cli.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(bash "$B" --root "$T" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($*): exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL ($*): output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
S="$T/cli/src"
mkdir -p "$S"
printf '#!/bin/bash\nhead\n' > "$S/prelude.sh"
printf 'mail one\nmail two\n' > "$S/mail.sh"
printf 'no newline at the end' > "$S/router.sh"
printf '# the order the parts are joined in\nprelude.sh\nmail.sh\n\nrouter.sh\n' > "$S/ORDER"
WANT="$(printf '#!/bin/bash\nhead\nmail one\nmail two\nno newline at the end')"

# Nothing built yet: --check says so and writes nothing.
run 1 "cli/k2 is not the join of cli/src" --check
[ ! -e "$T/cli/k2" ] || { echo "FAIL: --check wrote cli/k2" >&2; exit 1; }
# The join is the parts in ORDER, byte for byte, and the file is executable.
run 0 "wrote cli/k2 from 3 parts"
[ "$(cat "$T/cli/k2")" = "$WANT" ] && [ "$(wc -c < "$T/cli/k2" | tr -d ' ')" = "56" ] || { echo "FAIL: the join: $(cat "$T/cli/k2")" >&2; exit 1; }
[ -x "$T/cli/k2" ] || { echo "FAIL: cli/k2 is not executable" >&2; exit 1; }
pass=$((pass + 2))
run 0 "cli/k2 is current (3 parts)" --check
run 0 "already current"
# An edit made in cli/k2 is caught, and a build puts the parts back.
echo "edited here" >> "$T/cli/k2"
run 1 "Edit the part in cli/src (never cli/k2)" --check
run 0 "wrote cli/k2"
run 0 "is current" --check
# An edit to a part shows as stale until the build runs; the mode is kept.
printf 'mail one\nmail 2\n' > "$S/mail.sh"
run 1 "not the join" --check
run 0 "wrote cli/k2"
[ -x "$T/cli/k2" ] && grep -q "mail 2" "$T/cli/k2" || { echo "FAIL: rebuild" >&2; exit 1; }
pass=$((pass + 1))
# A part that ORDER does not list, a listed part that is missing, a part
# listed twice, a path outside cli/src: each is an error, nothing written.
printf 'x\n' > "$S/stray.sh"
run 2 "cli/src/stray.sh is not in ORDER"
rm "$S/stray.sh"
printf 'prelude.sh\nmail.sh\ngone.sh\nrouter.sh\n' > "$S/ORDER"
run 2 "ORDER lists gone.sh, but cli/src/gone.sh does not exist" --check
printf 'prelude.sh\nmail.sh\nmail.sh\nrouter.sh\n' > "$S/ORDER"
run 2 "ORDER lists mail.sh twice"
printf 'prelude.sh\n../k2\n' > "$S/ORDER"
run 2 "ORDER names a part outside cli/src: ../k2"
: > "$S/ORDER"
run 2 "ORDER lists no parts"
rm "$S/ORDER"
run 2 "no $S/ORDER"
run 2 "unknown argument: --chekc" --chekc
grep -q "mail 2" "$T/cli/k2" || { echo "FAIL: an error changed cli/k2" >&2; exit 1; }
pass=$((pass + 1))
echo "build-cli tests: $pass passed"
