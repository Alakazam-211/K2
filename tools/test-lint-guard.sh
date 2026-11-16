#!/bin/sh
# Self-test of the lint part's guards (tools/tests/full.sh, `lint`), each planted in a scratch
# copy, never in the checkout:
#   - tools/lint-guard.sh: a clean tree passes; a formatted (changed) tracked file fails; an
#     untracked file passes; LINT_DIRTY=1 lets the changed tree through;
#   - the fmt step: the tools crate as committed passes `cargo fmt --check`, and with one
#     unformatted line planted it fails.
# Needs git and cargo (with rustfmt). Exit 0 when every case went as planted.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
fail=0
expect() { # $1: want exit, $2: name, then the command
  want=$1; name=$2; shift 2
  set +e; "$@" > "$T/out" 2>&1; got=$?; set -e
  if [ "$got" = "$want" ]; then echo "ok   $name: exit $got"; else echo "FAIL $name: exit $got, want $want"; sed 's/^/     /' "$T/out"; fail=1; fi
}
g() { git -C "$T/r" -c user.name=t -c user.email=t@example.invalid "$@"; }
mkdir -p "$T/r"; g init -q; printf 'fn main() {}\n' > "$T/r/a.rs"; g add a.rs; g commit -qm a
expect 0 clean-tree sh "$HERE/lint-guard.sh" "$T/r"
printf 'fn  main( ) { }\n' > "$T/r/a.rs"
expect 1 changed-tracked-file sh "$HERE/lint-guard.sh" "$T/r"
expect 0 changed-with-LINT_DIRTY env LINT_DIRTY=1 sh "$HERE/lint-guard.sh" "$T/r"
g checkout -q -- a.rs; printf 'x\n' > "$T/r/untracked.txt"
expect 0 untracked-file sh "$HERE/lint-guard.sh" "$T/r"
expect 2 not-a-folder sh "$HERE/lint-guard.sh" "$T/nope"
# the fmt step on a copy of the tools crate (sources only; its own target dir)
mkdir -p "$T/c"; cp -R "$REPO/tools/mpc-tools/Cargo.toml" "$REPO/tools/mpc-tools/Cargo.lock" "$REPO/tools/mpc-tools/src" "$T/c/"
expect 0 fmt-as-committed cargo fmt --manifest-path "$T/c/Cargo.toml" -- --check
printf '\nfn  planted ( ) -> u8 { 1 }\n' >> "$T/c/src/py.rs"
expect 1 fmt-planted-unformatted-line cargo fmt --manifest-path "$T/c/Cargo.toml" -- --check
[ $fail = 0 ] && echo "lint guards: every plant caught (6 cases)"
exit $fail
