#!/usr/bin/env bash
# run.sh — build k2-approve and run the signed-approval shell tests:
# the CLI with no daemon (tests/cli/approve_verify.sh) and the openssl recipe
# (tests/approve/openssl-recipe.sh). Rust's own tests: cargo test -p k2-approve.
#   bash tests/approve/run.sh
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
inst="$(mktemp -d "${TMPDIR:-/tmp}/k2-approve-inst.XXXXXX")"
trap 'rm -rf "$inst"' EXIT
cargo install --locked --debug --quiet --path "$REPO/crates/k2-approve" --root "$inst"
rc=0
K2_APPROVE_BIN="$inst/bin/k2-approve" bash "$REPO/tests/cli/approve_verify.sh" || rc=1
bash "$REPO/tests/approve/openssl-recipe.sh" || rc=1
exit "$rc"
