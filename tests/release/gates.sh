#!/usr/bin/env bash
# The signing lane's gates in one place, for a compute node:
#   bash tests/release/gates.sh rust     # -D warnings check (workspace, all targets, no k2) + k2-release-keys
#   bash tests/release/gates.sh shell    # tests/packaging/updater-signing.sh + tests/release/k2sign-*.sh
#   bash tests/release/gates.sh k2sign   # tests/release/k2sign-*.sh only (macOS: the signer is Mac tooling)
#   bash tests/release/gates.sh all
# Every step must pass; the first failure ends the run with its exit code.
set -euo pipefail
cd "$(dirname "$0")/../.."
what="${1:-all}"
step() { echo; echo "=== gate: $* ==="; "$@"; }
rust() {
    export RUSTFLAGS='-D warnings'
    step cargo check --workspace --all-targets --exclude k2
    step cargo test -p k2-release-keys
}
k2sign_tests() {
    local t n=0
    for t in tests/release/k2sign-*.sh; do
        [ -e "$t" ] || continue
        n=$((n + 1))
        step bash "$t"
    done
    [ "$n" -gt 0 ] || { echo "no tests/release/k2sign-*.sh found" >&2; exit 1; }
}
shell_tests() {
    step bash tests/packaging/updater-signing.sh
    k2sign_tests
}
case "$what" in
    rust) rust ;;
    shell) shell_tests ;;
    k2sign) k2sign_tests ;;
    all) rust; shell_tests ;;
    *) echo "usage: gates.sh rust|shell|k2sign|all" >&2; exit 2 ;;
esac
echo
echo "=== gates ($what): all passed ==="
