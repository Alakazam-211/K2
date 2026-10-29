#!/usr/bin/env bash
# Compute easy mode, Lane A (prd-compute-easy-mode-0467) gates, one step per
# run, meant for a compute node:
#   k2 compute run <node> --sha <sha> --needs rust@stable,cmake@3,sys:pkg-config,cargo-nextest \
#     --mem 24G -- scripts/test/compute-easy-gates.sh <step>
#
# Steps:
#   check     RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --exclude k2,
#             contract-gen --check
#   smoke     build k2-ptyd, k2-daemon and k2-node; the compute CLI smoke
#   int       the compute route tests and the permission-cards-off golden
#   nextest   build k2-ptyd; cargo nextest, profile ci, k2-core and k2-daemon
#
# Every K2_* variable is dropped so nothing reaches a real ~/.k2; SHELL is
# set (the node's job user has a nologin shell).
set -euo pipefail
export SHELL=/bin/bash
for v in $(compgen -e); do
  case "$v" in K2_* | K2SO_*) unset "$v" ;; esac
done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
cd "$(dirname "$0")/../.."

step="${1:?step: check|smoke|int|nextest}"
case "$step" in
  check)
    RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --exclude k2 --message-format short
    cargo run -q -p k2-core --bin contract-gen -- --check
    ;;
  smoke)
    cargo build -p k2-ptyd --bins
    cargo build -p k2-daemon -p k2-node
    bash tests/cli/compute_cli_smoke.sh
    ;;
  int)
    cargo build -p k2-ptyd --bins
    cargo test -p k2-daemon --test compute_place_integration --test compute_shared_integration --test compute_integration --test permission_off_golden
    ;;
  nextest)
    cargo build -p k2-ptyd --bins
    cargo nextest run --profile ci -p k2-core -p k2-daemon --no-fail-fast
    ;;
  *)
    echo "unknown step $step" >&2
    exit 2
    ;;
esac
echo "GATE $step: OK"
