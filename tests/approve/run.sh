#!/usr/bin/env bash
# run.sh — every signed-approval test that needs Rust: k2-approve's own tests
# (with and without the verify feature) and clippy, with warnings as errors;
# then the shell tests: the CLI with no daemon (tests/cli/approve_verify.sh)
# and the openssl recipe (tests/approve/openssl-recipe.sh).
# Runs on a compute node, not the M5:
#   k2 compute run --sha <commit> --needs rust@stable,cmake@3 -- bash tests/approve/run.sh
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
export RUSTFLAGS="-D warnings"
inst="$(mktemp -d "${TMPDIR:-/tmp}/k2-approve-inst.XXXXXX")"
trap 'rm -rf "$inst"' EXIT
rc=0
cargo test -p k2-approve --locked 2>&1 | tee "$inst/test.log" | grep -E "^test result|FAILED|panicked" || true
grep -q "FAILED\|panicked\|^error" "$inst/test.log" && rc=1
cargo test -p k2-approve --locked --no-default-features --lib 2>&1 | tee "$inst/lib.log" | grep -E "^test result|FAILED|panicked" || true
grep -q "FAILED\|panicked\|^error" "$inst/lib.log" && rc=1
if cargo clippy -p k2-approve --locked --all-targets -- -D warnings > "$inst/clippy.log" 2>&1; then
    echo "clippy: clean"
else
    tail -30 "$inst/clippy.log"; rc=1
fi
cargo install --locked --debug --quiet --path "$REPO/crates/k2-approve" --root "$inst"
K2_APPROVE_BIN="$inst/bin/k2-approve" bash "$REPO/tests/cli/approve_verify.sh" || rc=1
bash "$REPO/tests/approve/openssl-recipe.sh" || rc=1
echo "run.sh: exit $rc"
exit "$rc"
