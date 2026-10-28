#!/usr/bin/env bash
# checks.sh — the steps of .github/workflows/checks.yml as one script, so
# the K2 Compute CI gate (scripts/ci/run-checks.sh) runs exactly what
# GitHub runs (plan-ci-release-on-k2-compute-v1 §3).
#
#   scripts/ci/checks.sh web|rust-check|rust-test|audit|all
#
#   web         packaging tests, bun install, gates, typecheck, vitest,
#               SDK typecheck, zen widget CLI, vite build   (checks.yml web)
#   rust-check  cargo check -D warnings, all targets + the airgap feature
#   rust-test   the test subsets of checks.yml's rust job, run with
#               cargo-nextest (profile ci, .config/nextest.toml; the node
#               needs cargo-nextest: run-checks.sh asks for it)
#   audit       cargo audit + osv-scanner over Cargo.lock and bun.lock,
#               pinned tools, the allow-list rules and a canary (checks.yml
#               audit; prd-security-sweep-0464-v1 SS14)
#   all         the four, in that order
#
# Network: web (bun install) and audit (crates.io for cargo-audit, the
# osv-scanner release download, the RustSec database and osv.dev) need
# the internet. A compute job gets it on a Linux node under the default
# share network (`no-lan`: internet yes, LAN no). On a macOS node a team
# tenant's `no-lan` becomes `none` (no internet at all), so these two
# parts stay on Linux nodes.
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
    step "Release k2-ptyd checks (dry)";           bash tests/packaging/release-ptyd-checks.sh
    step "Linux builds on K2 Compute (stubbed)";   bash tests/packaging/compute-release.sh
    step "CI gate scripts (stubbed)";              bash tests/packaging/ci-gate.sh
    step "release.sh waits + Step 0.5 env (stubbed)"; bash tests/packaging/release-wait-and-preflight.sh
    step "macOS sidecars build once (stubbed)";    bash tests/packaging/build-once.sh
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

# Advisories K2 lets through are listed with a reason and a review-by date
# (ignoreUntil, at most 90 days out) in osv-scanner.toml; .cargo/audit.toml
# repeats the RustSec IDs for cargo audit. audit-ignores.py enforces the
# rules, and an expired entry fails the part. Its header says how to renew
# or drop an entry.
# cargo audit fails on vulnerabilities, unmaintained and unsound crates, not
# on yanked versions: a yanked version has no advisory ID to allow-list. It
# still prints them (2026-10-24: chacha20 0.10.0 via ngrok, clipboard-win
# 5.4.1 via arboard on Windows); bump those with the dependency PRD.
part_audit() {
    step "advisory allow-list rules";              python3 scripts/ci/audit-ignores.py check
    step "install cargo-audit (pinned)";           cargo install cargo-audit --locked --version =0.22.2 --root target/audit-tools
    step "install osv-scanner (pinned, sha256)";   bash scripts/ci/install-osv-scanner.sh target/audit-tools
    step "scanners catch a known advisory";        bash tests/packaging/audit-canary.sh target/audit-tools
    step "cargo audit";                            target/audit-tools/bin/cargo-audit audit --deny unmaintained --deny unsound --file Cargo.lock
    step "osv-scanner";                            target/audit-tools/osv-scanner scan source --lockfile bun.lock --lockfile Cargo.lock --config osv-scanner.toml
}

part_rust_test() {
    # cargo-nextest, profile `ci` (.config/nextest.toml): every test in its
    # own process, no retries, JUnit at target/nextest/ci/junit.xml. One
    # RUSTFLAGS for the whole part: a RUSTFLAGS change rebuilds every crate,
    # and the k2-client / k2-ptyd steps used to run without it, so k2-core
    # and its tree compiled twice per run. rust-check already holds every
    # crate here to -D warnings, so the flag changes no outcome.
    export RUSTFLAGS="-D warnings"
    step "cargo nextest k2-client"
    cargo nextest run --profile ci -p k2-client
    step "cargo nextest k2-ptyd (Linux)"
    cargo build -p k2-ptyd --bin k2-ptyd
    cargo nextest run --profile ci -p k2-ptyd
    cargo nextest run --profile ci -p k2-core --test ptyd_backend

    step "cargo nextest root helper"
    cargo nextest run --profile ci -p k2-root-proto --features test-signer
    cargo nextest run --profile ci -p k2-root-helper
    cargo check -p k2-root-helper --features test-keys --examples
    cargo nextest run --profile ci -p k2-daemon --lib root_helper:: mail::helper:: update_routes::tests::daemon_update cell_server:: front443:: mail::front_listener::
    bash tests/cli/install_root_helper.sh
    bash tests/cli/root_helper_cli.sh
    bash tests/cli/front_cli.sh
    bash tests/cli/provision_mail_helper.sh
    bash tests/cli/daemon_unit_oompolicy.sh
    bash tests/cli/daemon_unit_runtime_dir.sh
    bash tests/cli/daemon_install_systemd_unit.sh
    bash tests/cli/daemon_status_ptyd.sh

    step "cargo nextest verb catalog + Zen"
    cargo nextest run --profile ci -p k2-core --lib contract:: zen:: app_surface::
    cargo nextest run --profile ci -p k2-daemon --test contract_routes --test app_api_integration

    step "cargo nextest compute nodes"
    cargo nextest run --profile ci -p k2-node-proto -p k2-node -p k2-redact
    cargo nextest run --profile ci -p k2-core --lib compute::
    cargo nextest run --profile ci -p k2-daemon --lib --test compute_integration --test compute_results_integration --test compute_secret_integration -E 'kind(test) | test(compute_) | test(route_policy)'
    cargo nextest run --profile ci -p k2-vault --test compute_grant --test redact_pin

    step "cargo nextest App runtime"
    cargo nextest run --profile ci -p k2-core --lib deploy:: app_audit skin_link app_door app_grace::requests
    cargo nextest run --profile ci -p k2-daemon --lib --test apps_runtime_integration --test apps_door_integration --test apps_gateway_link_integration -E 'kind(test) | test(apps::)'
    cargo nextest run --profile ci -p k2-app-engine

    step "cargo nextest Sew (grant hook, chat pass)"
    cargo nextest run --profile ci -p k2-core --lib tool_hook:: sew_revoke:: skin::sew_tests connect_users::
    cargo nextest run --profile ci -p k2-daemon --lib --test chat_pass_integration --test chat_pass_mint_integration --test grant_hook_integration -E 'kind(test) | test(chat_pass::) | test(tool_hook_worker::) | test(sew_mgmt::)'
    unset RUSTFLAGS
}

case "${1:-}" in
    web) part_web ;;
    rust-check) part_rust_check ;;
    rust-test) part_rust_test ;;
    audit) part_audit ;;
    all) part_web; part_rust_check; part_rust_test; part_audit ;;
    *) echo "usage: scripts/ci/checks.sh web|rust-check|rust-test|audit|all" >&2; exit 2 ;;
esac
echo ""
echo "checks: ${1} passed"
