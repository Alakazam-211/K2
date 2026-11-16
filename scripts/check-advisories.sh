#!/bin/sh
# Fail if any crate in the repo's lock, or in the fuzz crate's (when the tree has one), has a
# RustSec advisory: a vulnerability, an unmaintained or unsound crate, or a yanked version
# (`deny.toml`, at the repo root). cargo-deny reads each lock at its exact versions and fetches
# the advisory database. Nothing is skipped: without cargo-deny, or when the database can't be
# fetched, the step fails. Licences have their own step (scripts/check-licences.sh).
#
#   scripts/check-advisories.sh       exit 0: no advisory in either lock; 1: an advisory, or the
#                                     check couldn't run
set -eu
cd "$(dirname "$0")/.."
if ! cargo deny --version >/dev/null 2>&1; then
  echo "check-advisories: cargo-deny isn't installed; install it with: cargo install cargo-deny --locked" >&2
  exit 1
fi
echo "check-advisories: $(cargo deny --version)"
# the advisories deny.toml ignores, each by its id and reason, before the check
grep -E '^[[:space:]]*\{ *id = "' deny.toml | sed -e 's/^[[:space:]]*{ *id = "\([^"]*\)", *reason = "\([^"]*\)".*/check-advisories: ignored: \1 (\2)/'
cargo deny --locked --config deny.toml check advisories
locks=1
if [ -f fuzz/Cargo.toml ]; then
  cargo deny --locked --config deny.toml --manifest-path fuzz/Cargo.toml check advisories
  locks=2
fi
echo "check-advisories: ok (no advisory in $locks lock(s))"
