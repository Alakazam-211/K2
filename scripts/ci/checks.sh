#!/usr/bin/env bash
# checks.sh — the steps of .github/workflows/checks.yml as one script, so
# the K2 Compute CI gate (scripts/ci/run-checks.sh) runs exactly what
# GitHub runs (plan-ci-release-on-k2-compute-v1 §3).
#
#   scripts/ci/checks.sh web|rust-check|rust-test|audit|all|brand-xy
#
#   web         packaging tests, bun install, gates, typecheck, vitest,
#               SDK typecheck, zen widget CLI, vite build   (checks.yml web)
#   rust-check  cargo check -D warnings, all targets + the airgap feature,
#               and clippy's deny-level lints on k2-core and k2-daemon
#   rust-test   the test subsets of checks.yml's rust job, run with
#               cargo-nextest (profile ci, .config/nextest.toml; the node
#               needs cargo-nextest: run-checks.sh asks for it)
#   audit       cargo audit + osv-scanner over Cargo.lock and bun.lock,
#               pinned tools, the allow-list rules and a canary (checks.yml
#               audit; prd-security-sweep-0464-v1 SS14), and cargo deny:
#               the root deny.toml against every shipped crate
#   brand-xy    the CI brand (brand/xy): the licence gate's report (never
#               a refusal here), cargo check with K2_BRAND_DIR,
#               the k5 binary built for it, and the output judges on what
#               it prints (scripts/brand/xy-build.sh: in a folder of its
#               own under $TMPDIR, after a free-disk check, removed on
#               every way out)
#   all         the five, in that order
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
    step "third-party notices ship in every output (no build)"; bash scripts/check-third-party-notices.sh
    step "third-party notices check on broken copies"; bash tests/packaging/third-party-notices.sh
    step "release source archives (stub curl)";    bash tests/packaging/release-source-archives.sh
    step "Arch PKGBUILD sources, sums, syntax";    bash scripts/check-arch-pkgbuild.sh
    step "Arch PKGBUILD check on broken fixtures"; bash tests/packaging/check-arch-pkgbuild.sh
    step "Published Arch package never changes (stubbed)"; bash tests/packaging/arch-upload-pkg.sh
    step "Omarchy k2-bin recipe";                  bash scripts/check-omarchy-recipe.sh
    step "Omarchy recipe check on broken fixtures"; bash tests/packaging/check-omarchy-recipe.sh
    step "Linux SHA256SUMS (stubbed)";             bash tests/packaging/linux-sums.sh
    step "Step 10.6, failed Arch build (stubbed)"; bash tests/packaging/linux-sums-step.sh
    step "Release lock (stub k2-node)";            bash tests/packaging/release-lock.sh
    step "Release step timings (temp dirs)";       bash tests/packaging/release-timer.sh
    step "Release k2-ptyd checks (dry)";           bash tests/packaging/release-ptyd-checks.sh
    step "Linux builds on K2 Compute (stubbed)";   bash tests/packaging/compute-release.sh
    step "Update signing on the signing host (stubbed)";     bash tests/packaging/updater-signing.sh
    step "CI gate scripts (stubbed)";              bash tests/packaging/ci-gate.sh
    step "k2-daemon test groups (no cargo)";       bash scripts/check-daemon-test-targets.sh
    step "Test-group check on broken layouts";     bash tests/packaging/daemon-test-targets.sh
    step "k2-node Mac wall test list (no cargo)";  bash scripts/ci/k2-node-mac-tests.sh check
    step "Mac wall test list check on broken lists"; bash tests/garden/k2-node-mac-tests.sh
    step "k2-daemon Mac login-shell test list (no cargo)"; bash scripts/ci/k2-daemon-mac-tests.sh check
    step "Mac login-shell test list check on broken lists"; bash tests/garden/k2-daemon-mac-tests.sh
    step "macOS test run script (stubbed)";        bash tests/garden/mac-tests.sh
    step "Embedded Python compiles (cli/k2, tests, scripts)"; bash tests/check-embedded-python.sh
    step "Rust pin (rust-toolchain.toml)";         bash tests/packaging/rust-pin.sh
    step "cargo-deny policy and git pins (no cargo)"; bash tests/packaging/cargo-deny-policy.sh
    step "release.sh compute mode (stubbed)";      bash tests/packaging/release-compute-mode.sh
    step "Linux build gate on K2 Compute (stubbed)"; bash tests/packaging/linux-gate.sh
    step "Release gates on K2 Compute (stubbed)";  bash tests/packaging/release-gates.sh
    step "release.sh waits + Step 0.5 env (stubbed)"; bash tests/packaging/release-wait-and-preflight.sh
    step "macOS sidecars build once (stubbed)";    bash tests/packaging/build-once.sh
    step "Release path maps and build-path gate";  bash tests/packaging/build-paths.sh
    step "bun install";                            bun install --frozen-lockfile
    step "third-party notices' npm licence copies"; bash scripts/check-third-party-notices.sh --npm
    step "k2-home gate";                           bash scripts/k2-home-gate.sh
    step "k2-home gate's .k2 half on broken trees"; bash tests/garden/k2-home-dot-gate.sh
    step "brand inventory script";                 bash tests/garden/brand-inventory.sh
    step "brand gate (no new product or command name, or k2.dev)"; bash scripts/brand-gate.sh
    step "zone gate on broken trees";              bash tests/garden/zone-gate.sh
    step "brand gate on broken trees";             bash tests/garden/brand-gate.sh
    step "brand output judges on small corpora";   bash tests/garden/brand-output-judges.sh
    step "ceiling-zero gate on scratch repos";     bash tests/garden/ceiling-zero-gate.sh
    step "licence gate on fixture lockfiles";      bash tests/garden/licence-gate.sh
    step "the brand mark is static data (only its reader reads it)"; bash scripts/brand/mark-static-gate.sh
    step "release brand-mark check on made-up binaries"; bash tests/garden/brand-mark.sh
    step "no-bash check on made-up files";         bash tests/garden/no-bash-check.sh
    step "brand name policy (brand.sh lint), offline"; bash tests/garden/brand-lint.sh
    step "k2-literal gate on broken trees";        bash tests/garden/k2-literal-gate.sh
    step "distributor pipeline template, offline"; bash tests/garden/distributor-template.sh
    step "interop wire comparer on fixtures";     python3 -B tests/interop/test_wire_compare.py
    step "interop Postal test plane, no sockets";  python3 -B tests/interop/test_postal_plane.py
    step "private-docs gate";                      bash scripts/private-docs-gate.sh
    step "typecheck:web";                          bun run typecheck:web
    step "vitest";                                 bunx vitest run --reporter=dot
    step "App server SDK typecheck";               bunx tsc -p sdk/server/tsconfig.json
    # Every generated file this part can check: the App server SDK dist,
    # the CLI explorer catalog, the styles and the CLI goldens. No cargo
    # here, so contract-gen's outputs are checked in rust-test ("verb
    # catalog": k2-core's contract:: freshness test).
    step "generated files are fresh (regen.sh)";   bash scripts/regen.sh --check --no-cargo
    step "CLI tests take their k2 from _cli_under_test.sh"; bash tests/cli/cli_under_test_gate.sh
    step "every urlopen in cli/src carries the hand-over headers"; bash tests/cli/k5_urlopen_gate.sh
    step "k2 study prints cli/text/study byte for byte"; bash tests/cli/study_text.sh
    step "k2 help prints cli/text/help byte for byte"; bash tests/cli/help_text.sh
    step "the help k5 answers itself is a plain print in bash"; bash tests/cli/help_native.sh
    step "the CLI test helper picks the program";  bash tests/cli/cli_under_test_helper.sh
    step "CLI help names no machine of ours";      bash tests/cli/help_node_names.sh
    step "k2 answers the same under every name";   bash tests/cli/called_as.sh
    step "k2 finds the files shipped beside it";   bash tests/cli/cli_script_dir.sh
    step "k5-cli ships, off by default";           bash tests/packaging/k5_cli_shipped_off_by_default.sh
    step "k5-cli is signed and checked as itself";  bash tests/packaging/k5_cli_signing_pins.sh
    step "k2 picks a workspace's dot-dir as the daemon does"; bash tests/cli/ws_dot_dir.sh
    step "urlencode never sends a value raw"; bash tests/cli/urlencode_bytes.sh
    step "no daemon call through a proxy or with a query token (lint)"; bash tests/cli/no_proxy_lint.sh
    step "a POST that tried the cell socket is sent once (lint)"; bash tests/cli/post_no_resend_lint.sh
    step "no daemon answer reaches python3 in argv (lint)"; bash tests/cli/python_argv_answer_lint.sh
    step "no shell value is pasted into a python3 or node program (lint)"; bash tests/cli/python_code_values_lint.sh
    step "no K2_CJ_ field or password in a child's argv or environment (lint)"; bash tests/cli/secret_argv_env_lint.sh
    step "a test guest's password stays off every child's argv and environment (stub daemon)"; bash tests/cli/app_test_guest_password_off_argv.sh
    step "the typed-back recovery words stay off every child's argv and environment (stub vault)"; bash tests/cli/vault_recovery_words_off_argv.sh
    step "k2 hostchat open keeps the owner link off every child's argv and environment (stub daemon and vault)"; bash tests/cli/hostchat_open_link_off_argv.sh
    step "k2 ci signer add takes the pairing code on stdin (stub daemon and vault)"; bash tests/cli/ci_signer_code_stdin.sh
    step "a value flag as the last word is a usage error on the most-used verbs (stub daemon)"; bash tests/cli/need_value.sh
    step "k2 compute local enable takes the enroll code and ticket on stdin (stub daemon)"; bash tests/cli/compute_local_enable_stdin.sh
    step "remote tokens have a stdin form and stay off every child's argv and environment (stub server)"; bash tests/cli/remote_token_stdin.sh
    step "k2 connect login says live only when the tunnel started (stub server)"; bash tests/cli/connect_login_live.sh
    step "k2 compute fetch gets every file of a job with many (stub daemon)"; bash tests/cli/compute_fetch_many_artifacts.sh
    step "k2 never sends a daemon request through a proxy (stub daemon)"; bash tests/cli/no_proxy_to_daemon.sh
    step "one proxy rule in Rust, the CLI's curl and its Python (parity table)"; bash tests/cli/proxy_rule_parity.sh
    step "k2 names a body file the Windows way in Git Bash"; bash tests/cli/windows_curl_body_path.sh
    step "k2 heartbeat wake sends the session passport"; bash tests/cli/heartbeat_wake_passport.sh
    step "k2 heartbeat wake says why when it cannot run (stub daemon)"; bash tests/cli/heartbeat_wake_errors.sh
    step "k2 heartbeat log says why when it cannot show the log (stub daemon)"; bash tests/cli/heartbeat_log_errors.sh
    step "k2 fed never sends the owner token from a session"; bash tests/cli/fed_passport.sh
    step "a socket-first sender's TCP fallback never carries the owner token in a session (stub daemon)"; bash tests/cli/socket_first_no_owner.sh
    step "a socket-first sender's TCP fallback never carries the owner token in a session (lint)"; bash tests/cli/socket_first_no_owner_lint.sh
    step "the CLI name rewrite, on a copy";        bash tests/garden/cli-name-flip.sh
    step "the dispatcher arm reader";              bash tests/garden/dispatcher-arms.sh
    step "k2 zen widget CLI (no daemon)";          bash tests/cli/zen_widget_offline.sh
    step "Zen library licence banners (the rule)"; node scripts/zen-lib/check-banners.mjs --self-test
    step "Zen library licence banners (each file)"; node scripts/zen-lib/check-banners.mjs
    step "k2 on a virtual desktop never acts as the owner"; bash tests/cli/virtual_desktop_owner_guard.sh
    step "k2 desktop on a Linux server (stub daemon)"; bash tests/cli/desktop_virtual_cli.sh
    step "k2 desktop ask and the Mac's permission steps (stub daemon)"; bash tests/cli/desktop_ask_cli.sh
    step "vite build hosted web";                  bun run vite:build:web
    step "the web build carries the third-party notices"
    bash scripts/check-third-party-notices.sh --web-out "out/web/app/$(node -p 'require("./package.json").version')"
}

part_rust_check() {
    step "cargo check -D warnings, all targets"
    RUSTFLAGS="-D warnings" cargo check -p k2-core -p k2-client -p k2-daemon -p k2-node-proto -p k2-node \
        -p k2-root-proto -p k2-root-helper -p k2-app-engine -p k2-ptyd -p k2-release-keys -p k2-export \
        -p k2-deskd-proto -p k2-deskd -p k2-deckrender -p k2-scenerender --all-targets
    step "cargo check airgap feature"
    RUSTFLAGS="-D warnings" cargo check -p k2-core -p k2-daemon --features airgap --all-targets
    # Clippy's deny-level lints only: clippy exits non-zero on an error, not
    # on a warning. The same RUSTFLAGS as above (so nothing rebuilds); the
    # trailing -W warnings puts clippy's own warnings back to warnings.
    step "clippy deny-level lints, k2-core"
    RUSTFLAGS="-D warnings" cargo clippy --locked -p k2-core --all-targets --no-deps -- -W warnings
    step "clippy deny-level lints, k2-daemon"
    RUSTFLAGS="-D warnings" cargo clippy --locked -p k2-daemon --all-targets --no-deps -- -W warnings
}

# Advisories K2 lets through are listed with a reason and a review-by date
# (ignoreUntil, at most 90 days out) in osv-scanner.toml; .cargo/audit.toml
# repeats the RustSec IDs for cargo audit. audit-ignores.py enforces the
# rules, and an expired entry fails the part. Its header says how to renew
# or drop an entry.
# cargo audit fails on vulnerabilities, unmaintained and unsound crates, not
# on yanked versions: a yanked version has no advisory ID to allow-list. It
# still prints them (2026-10-24: clipboard-win 5.4.1 via arboard on
# Windows); bump those with the dependency PRD.
#
# cargo deny: the app-wide policy (the root deny.toml: the Kessel licence
# allowlist, RustSec, git sources) against every crate K2 ships, each with
# its default features, on Cargo.lock, and against every other K2-owned
# Cargo.lock (the app template, third_party), each on a copy outside the
# repo. scripts/ci/cargo-deny.sh holds both lists, names the locks it
# doesn't check and why, and says how it is scoped.
part_audit() {
    step "advisory allow-list rules";              python3 scripts/ci/audit-ignores.py check
    step "install cargo-audit (pinned)";           cargo install cargo-audit --locked --version =0.22.2 --root target/audit-tools
    step "install osv-scanner (pinned, sha256)";   bash scripts/ci/install-osv-scanner.sh target/audit-tools
    step "install cargo-deny (pinned)";            bash scripts/ci/cargo-deny.sh install target/audit-tools
    step "scanners catch a known advisory";        bash tests/packaging/audit-canary.sh target/audit-tools
    step "cargo audit";                            target/audit-tools/bin/cargo-audit audit --deny unmaintained --deny unsound --file Cargo.lock
    step "osv-scanner";                            target/audit-tools/osv-scanner scan source --lockfile bun.lock --lockfile Cargo.lock --config osv-scanner.toml
    step "cargo deny (deny.toml, shipped crates and side locks)"; bash scripts/ci/cargo-deny.sh check target/audit-tools/bin/cargo-deny
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
    cargo nextest run --profile ci -p k2-client -p k2-brand
    step "cargo nextest k2-ptyd (Linux)"
    cargo build -p k2-ptyd --bin k2-ptyd
    cargo nextest run --profile ci -p k2-ptyd
    # The remote desktop helper and its wire (prd-remote-desktop-tab-v1 D3/D4);
    # the Xvfb integration tests need a display: scripts/test/deskd-checks.sh xvfb.
    step "cargo nextest k2-deskd (Linux)"
    cargo nextest run --profile ci -p k2-deskd-proto -p k2-deskd
    # The export worker's limits against the real binary (hostile exporters).
    step "cargo nextest k2-export (Linux)"
    cargo nextest run --profile ci -p k2-export --features test-hostile
    # The .pptx slide renderer's limits against the real binary (test-hostile
    # worker), its renders, and the licence listing of what it ships.
    step "cargo nextest k2-deckrender (Linux)"
    cargo nextest run --profile ci -p k2-deckrender --features test-hostile
    # The 3D stills renderer: its limits against the real binary (test-hostile
    # worker), its renders, the determinism digests and the licence listing.
    step "cargo nextest k2-scenerender (Linux)"
    cargo nextest run --profile ci -p k2-scenerender --features test-hostile
    cargo nextest run --profile ci -p k2-core --test ptyd_backend

    step "cargo nextest root helper"
    cargo nextest run --profile ci -p k2-root-proto --features test-signer
    cargo nextest run --profile ci -p k2-release-keys
    cargo nextest run --profile ci -p k2-root-helper
    cargo check -p k2-root-helper --features test-keys --examples
    cargo nextest run --profile ci -p k2-daemon --lib root_helper:: root_helper_routes::update_version_tests:: mail::helper:: update_routes::tests::daemon_update cell_server:: front443:: mail::front_listener::
    bash tests/cli/install_root_helper.sh
    bash tests/cli/root_helper_cli.sh
    bash tests/cli/front_cli.sh
    bash tests/cli/provision_mail_helper.sh
    bash tests/cli/provision_sew.sh
    bash tests/packaging/install-daemon-root-helper.sh
    bash tests/cli/daemon_unit_oompolicy.sh
    bash tests/cli/daemon_unit_runtime_dir.sh
    bash tests/cli/daemon_install_systemd_unit.sh
    bash tests/cli/daemon_status_ptyd.sh

    step "cargo nextest verb catalog + Zen"
    cargo nextest run --profile ci -p k2-core --lib contract:: zen:: app_surface::
    cargo nextest run --profile ci -p k2-daemon --test it_contract -E 'test(/^(contract_routes|app_api_integration)::/)'

    step "cargo nextest compute nodes"
    cargo nextest run --profile ci -p k2-node-proto -p k2-node -p k2-redact
    cargo nextest run --profile ci -p k2-core --lib compute::
    cargo nextest run --profile ci -p k2-daemon --lib --test it_compute -E '(kind(lib) & (test(compute_) | test(route_policy))) | (kind(test) & test(/^(compute_integration|compute_results_integration|compute_secret_integration|compute_place_integration)::/))'
    cargo nextest run --profile ci -p k2-vault --test compute_grant --test redact_pin

    step "cargo nextest App runtime"
    cargo nextest run --profile ci -p k2-core --lib deploy:: app_audit skin_link app_door app_grace::requests
    cargo nextest run --profile ci -p k2-daemon --lib --test it_apps -E '(kind(lib) & test(apps::)) | (kind(test) & test(/^(apps_runtime_integration|apps_door_integration|apps_gateway_link_integration)::/))'
    cargo nextest run --profile ci -p k2-app-engine

    step "cargo nextest Sew (grant hook, chat pass)"
    cargo nextest run --profile ci -p k2-core --lib tool_hook:: sew_revoke:: skin::sew_tests connect_users::
    cargo nextest run --profile ci -p k2-daemon --lib --test it_chat --test grant_hook_integration -E '(kind(lib) & (test(chat_pass::) | test(tool_hook_worker::) | test(sew_mgmt::))) | binary(grant_hook_integration) | (kind(test) & test(/^(chat_pass_integration|chat_pass_mint_integration)::/))'
    unset RUSTFLAGS
}

part_brand_xy() {
    # Report only: what a partner pipeline's licence gate says on this tree.
    step "the xy brand: licence gate (report only)"
    bash scripts/brand/licence-gate.sh --brand xy --report
    step "the xy brand: name policy, check, build, brand mark, output judges"
    bash scripts/brand/xy-build.sh
}

case "${1:-}" in
    web) part_web ;;
    rust-check) part_rust_check ;;
    rust-test) part_rust_test ;;
    audit) part_audit ;;
    all) part_web; part_rust_check; part_rust_test; part_audit; part_brand_xy ;;
    brand-xy) part_brand_xy ;;
    *) echo "usage: scripts/ci/checks.sh web|rust-check|rust-test|audit|all|brand-xy" >&2; exit 2 ;;
esac
echo ""
echo "checks: ${1} passed"
