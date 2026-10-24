#!/usr/bin/env bash
# checks.sh — the steps of .github/workflows/checks.yml as one script, so
# the K2 Compute CI gate (scripts/ci/run-checks.sh) runs exactly what
# GitHub runs (plan-ci-release-on-k2-compute-v1 §3).
#
#   scripts/ci/checks.sh web|rust-check|rust-test|all
#
#   web         packaging tests, bun install, gates, typecheck, vitest,
#               SDK typecheck, zen widget CLI, vite build   (checks.yml web)
#   rust-check  cargo check -D warnings, all targets + the airgap feature
#   rust-test   the cargo test subsets of checks.yml's rust job
#   all         the three, in that order
#
# KEEP IN STEP with checks.yml until checks.yml calls this script (stage 1:
# checks.yml → dispatch-only, these statuses are the gate). A step added
# there and not here makes the compute gate weaker than GitHub's.
#
# Tests fail loudly: `set -e`, the first failing step stops the part and
# its exit code is the part's. Never touches ~/.k2 (every suite uses temp
# homes; a compute job's HOME is its own dir and no K2_* reaches it).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

step() { echo ""; echo "── checks: $* ──"; }

part_web() {
    step "Arch/Omarchy packaging lock";            bash tests/packaging/arch-omarchy-bundle.sh
    step "Arch PKGBUILD sources, sums, syntax";    bash scripts/check-arch-pkgbuild.sh
    step "Arch PKGBUILD check on broken fixtures"; bash tests/packaging/check-arch-pkgbuild.sh
    step "Arch build gate script (stubbed)";       bash tests/packaging/arch-build-gate.sh
    step "Linux SHA256SUMS (stubbed)";             bash tests/packaging/linux-sums.sh
    step "Release lock (stub k2-node)";            bash tests/packaging/release-lock.sh
    step "Linux builds on K2 Compute (stubbed)";   bash tests/packaging/compute-release.sh
    step "CI gate scripts (stubbed)";              bash tests/packaging/ci-gate.sh
    step "bun install";                            bun install --frozen-lockfile
    step "k2-home gate";                           bash scripts/k2-home-gate.sh
    step "private-docs gate";                      bash scripts/private-docs-gate.sh
    step "typecheck:web";                          bun run typecheck:web
    step "vitest";                                 bunx vitest run --reporter=dot
    step "App server SDK typecheck";               bunx tsc -p sdk/server/tsconfig.json
    step "k2 zen widget CLI (no daemon)";          bash tests/cli/zen_widget_offline.sh
    step "vite build hosted web";                  bun run vite:build:web
}

part_rust_check() {
    step "cargo check -D warnings, all targets"
    RUSTFLAGS="-D warnings" cargo check -p k2-core -p k2-client -p k2-daemon -p k2-node-proto -p k2-node \
        -p k2-root-proto -p k2-root-helper -p k2-app-engine -p k2-ptyd --all-targets
    step "cargo check airgap feature"
    RUSTFLAGS="-D warnings" cargo check -p k2-core -p k2-daemon --features airgap --all-targets
}

part_rust_test() {
    step "cargo test k2-client"
    cargo test -p k2-client
    step "cargo test k2-ptyd (Linux)"
    cargo build -p k2-ptyd --bin k2-ptyd
    cargo test -p k2-ptyd
    cargo test -p k2-core --test ptyd_backend

    export RUSTFLAGS="-D warnings"
    step "cargo test root helper"
    cargo test -p k2-root-proto --features test-signer
    cargo test -p k2-root-helper
    cargo check -p k2-root-helper --features test-keys --examples
    cargo test -p k2-daemon --lib -- root_helper:: mail::helper:: update_routes::tests::daemon_update cell_server:: front443:: mail::front_listener::
    bash tests/cli/install_root_helper.sh
    bash tests/cli/root_helper_cli.sh
    bash tests/cli/front_cli.sh
    bash tests/cli/provision_mail_helper.sh
    bash tests/cli/daemon_unit_oompolicy.sh
    bash tests/cli/daemon_unit_runtime_dir.sh

    step "cargo test verb catalog + Zen"
    cargo test -p k2-core --lib -- contract:: zen:: app_surface::
    cargo test -p k2-daemon --test contract_routes
    cargo test -p k2-daemon --test app_api_integration

    step "cargo test compute nodes"
    cargo test -p k2-node-proto -p k2-node
    cargo test -p k2-core --lib -- compute::
    cargo test -p k2-daemon --lib -- compute_ route_policy
    cargo test -p k2-daemon --test compute_integration
    cargo test -p k2-daemon --test compute_results_integration

    step "cargo test App runtime"
    cargo test -p k2-core --lib -- deploy::
    cargo test -p k2-daemon --lib -- apps::
    cargo test -p k2-daemon --test apps_runtime_integration
    cargo test -p k2-core --lib -- app_audit skin_link app_door app_grace::requests
    cargo test -p k2-daemon --test apps_door_integration
    cargo test -p k2-daemon --test apps_gateway_link_integration
    cargo test -p k2-app-engine

    step "cargo test Sew (grant hook, chat pass)"
    cargo test -p k2-core --lib -- tool_hook:: sew_revoke:: skin::sew_tests connect_users::
    cargo test -p k2-daemon --lib -- chat_pass:: tool_hook_worker:: sew_mgmt::
    cargo test -p k2-daemon --test chat_pass_integration --test chat_pass_mint_integration --test grant_hook_integration
    unset RUSTFLAGS
}

case "${1:-}" in
    web) part_web ;;
    rust-check) part_rust_check ;;
    rust-test) part_rust_test ;;
    all) part_web; part_rust_check; part_rust_test ;;
    *) echo "usage: scripts/ci/checks.sh web|rust-check|rust-test|all" >&2; exit 2 ;;
esac
echo ""
echo "checks: ${1} passed"
