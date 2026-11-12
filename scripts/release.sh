#!/bin/bash
# K2 Release Script (runs on z3mbpZ or, at Rosson's choice, z3mbp: it signs with that Mac's own key; a release build is heavy on the M5, so pause other builders)
# Builds, signs, notarizes, and releases K2 with both DMG and update bundle.
# Also publishes the hosted-web SPA bundle to Cloudflare R2 (phase 3).
#
# RUN LOCATION (critical):
#   Run this on the build Mac (z3mbpZ) from a LOCAL GUI terminal session
#   (iTerm / Terminal.app under the logged-in desktop user) — NOT plain SSH.
#   SSH-only sessions hit codesign "errSecInternalComponent" (login keychain
#   "User interaction is not allowed"), often first on the frpc sidecar.
#   Durable how-to (incl. AppleScript→iTerm launcher from SSH):
#     .k2/wiki/Release - Unattended release.sh.md
#     .k2/wiki/Release - Build Sign Notarize.md
#
# RELEASE_REPO selects the GitHub repo for `gh release create` + all
# manifest/download URLs. Defaults to the new K2 home; override with
# K2_RELEASE_REPO=Alakazam-211/K2SO for an old-repo (bridge) release.
#
# Prerequisites:
#   - The update keys U1 (the retired key, bridge signatures for installs
#     older than 0.46.6) and this Mac's primary key (U2 on z3mbpZ, U2b on
#     z3mbp: one key per signing Mac, never copied; installs trust both, and
#     U3, the offline backup; docs/release/updater-key-ceremony.md)
#     live with this Mac's `_k2sign` macOS user (prd-signing-user-v1; set up by
#     scripts/k2sign/install.sh). This user can't read them: each update
#     file goes to `sudo -n -u _k2sign …/k2sign sign` on stdin, and the
#     signer signs only inside a session Rosson approved ONCE (Touch ID or his
#     Mac login password)
#     at Step 1.55 (after the pre-flight gates, before any build), bound to
#     the version, the base commit and the asset list. Every update file is signed with BOTH keys.
#     K2_SIGNER=keychain is the rollback to the stopgap (key files in
#     ~/.k2-updater, passwords in the login keychain, readable by every
#     process of this user); it is removed after two green k2sign releases.
#     TAURI_SIGNING_PRIVATE_KEY(_PASSWORD) must NOT be set (refused).
#   - ~/Applications/K2 Release Approve.app, enrolled (scripts/k2sign/approve/).
#   - Apple signing identity in keychain (GUI login session — see above)
#   - gh CLI authenticated (or GH_TOKEN / GITHUB_TOKEN)
#
# Web-bundle publish (R2) — best-effort by default so desktop release is not
# blocked by R2 TLS warmup / missing laptop creds (Ops caveat on brand-new R2):
#   - Creds: ~/.config/cloudflare/r2.env or env R2_ACCESS_KEY_ID /
#     R2_SECRET_ACCESS_KEY / R2_ACCOUNT_ID (see scripts/publish-web-bundles.sh)
#   - Without creds: WARN + continue (publish uses --skip-if-no-creds)
#   - With creds but upload fails (TLS etc.): WARN + continue unless
#     K2_REQUIRE_WEB_PUBLISH=1, in which case the release FAILS
#   - Optional prune after publish: K2_WEB_BUNDLE_PRUNE=1
#   - GitHub Actions secrets (same names): R2_ACCOUNT_ID, R2_ACCESS_KEY_ID,
#     R2_SECRET_ACCESS_KEY (optional R2_ENDPOINT, R2_BUCKET)
#
# Windows NSIS (Step 9.5), Alpha — built on K2-WIN-AX41 (ssh alias
#   k2-win-ax41; code goes there as a git bundle of the tag), copied back
#   here and minisigned here for latest.json. Never signed on the box.
#   OFF by default for now: K2_SKIP_WINDOWS_NSIS defaults to 1; run with
#   K2_SKIP_WINDOWS_NSIS=0 to include Windows. When on, a failed build
#   soft-fails unless K2_REQUIRE_WINDOWS_NSIS=1.
#   See scripts/windows-nsis-build.sh + wiki "Ops - Windows Build Box".
#
# Arch/Omarchy pacman — not built on this Mac and not waited on before the
#   tag. The tag push starts arch-package.yml on the k2-arch runner, alongside
#   app-linux.yml and daemon-binaries.yml; it builds
#   k2-<version>-x86_64.pkg.tar.zst and uploads it. Step 9 attaches the file
#   only if one is already in $DIST_DIR. No .sig. Do not add a Linux GUI key
#   to latest.json or daemon-latest.json. This script doesn't publish to the
#   AUR or pkgs.omarchy.org (Omarchy's bot picks the file up from the release:
#   packaging/omarchy/k2-bin). A build break shows up after the release is live; Step 10.6 says
#   so and how to re-run it (scripts/lib/linux-sums-step.sh).
#   k2-arch (40.160.54.134) is shared with Sew: send Sew `HOLD k2-arch` before
#   the tag (this script does not message Sew) and `k2-arch clear` after the
#   arch-package run ends.
#
# Linux SHA256SUMS (Step 10.6) — scripts/linux-sums.sh check. After the
#   tag, app-linux.yml and arch-package.yml each merge their packages'
#   lines into the release's SHA256SUMS-linux-x86_64.txt. Step 10.6 waits
#   (K2_LINUX_SUMS_WAIT_MIN, default 120) until all four Linux packages and
#   their lines are up and match GitHub's asset digests, else the release
#   ends with exit 1 and a re-run line. A failed Arch build is reported as
#   soon as it fails and not waited for; the rest is still checked, and the
#   script still ends with exit 1. Escape: K2_SKIP_LINUX_SUMS_CHECK=1.
#
# Linux builds on K2 Compute (plan-ci-release-on-k2-compute-v1), switch
#   K2_LINUX_BUILDS:
#     compute (default) the compute-built Linux files ARE the release, and no
#                       release step waits on GitHub
#                       (prd-ci-release-on-compute-stage1-v1 §7):
#                       - 1.6c: submit, beside the Mac build, --tier ci:
#                         L1 (x86_64 binaries), V1 (the same again on
#                         another node), L3 (deb/rpm/AppImage), L2 (aarch64
#                         binaries, cross-compiled with zig on an x86_64
#                         node) and V2 (L2 again on another node). The
#                         binaries jobs get the OAuth values as vault
#                         secrets and run --exclusive.
#                       - 7.9c: wait, fetch, verify. V1 must reproduce L1's
#                         daemon, root helper, k2-node and engine byte for
#                         byte on another node (node_fp), V2 the same for L2.
#                       - 8.5: stage the verified files in daemon-dist.
#                       - 8.9: the release commit's tree must be the tree
#                         compute built; then the repo variable LINUX_BUILDS
#                         is set to what this run did (compute-all: GitHub
#                         builds no Linux binaries; compute: GitHub builds
#                         aarch64), right before the tag push.
#                       - 9: every staged file but the two daemons in the
#                         one gh release create.
#                       - 10.5: scripts/sign-daemon.sh <v> --from-dir
#                         <daemon-dist> --compute-verdict <audit dir> signs
#                         both daemons from the stage (the signing user
#                         checks the node-signed receipts itself, S3b) and
#                         uploads them; the sign-* scripts sign the root
#                         helper, node and engine, already on the release.
#                       - 10.6: deb/rpm/AppImage vs SHA256SUMS, checked once.
#                         The Arch package (arch-package.yml on k2-arch)
#                         follows on its own and is not waited for.
#                       FALLBACK (K2_COMPUTE_FALLBACK=1, the default): when
#                       compute can't do it (not ready at the pre-flight, a
#                       job that fails or doesn't verify, the tree check),
#                       always before the tag, release.sh says so in a
#                       banner and in the final summary and GitHub builds
#                       instead: all of Linux for an x86_64 problem, only
#                       aarch64 for an aarch64 one (Steps 10.5/10.6 then
#                       wait for GitHub, as in github mode).
#                       K2_COMPUTE_FALLBACK=0 stops the release instead.
#                       Under the signing user (K2_SIGNER=k2sign) the
#                       pre-flight needs the controller and every release
#                       node pinned (k2sign-admin pin-compute-*;
#                       k2_compute_signer_check) and the policy's aarch64
#                       lines (scripts/k2sign/install.sh from this commit).
#                       The repo variable is set by release.sh, which needs
#                       repo admin on this gh.
#     github            the GitHub workflows build Linux after the tag
#                       (the old path; the switch to use it on purpose).
#     shadow            also builds the Linux set on K2 Compute BEFORE the
#                       tag (Step 1.6c), fetches it and checks each job's
#                       signed receipt (Step 7.9c), then compares the files
#                       with the GitHub-built assets (Step 10.7, the shadow
#                       report). Publishes and signs nothing from compute; a
#                       compute problem only warns. Step 7.9c waits
#                       K2_COMPUTE_WAIT_MIN (default 60); jobs still running
#                       then are collected at Step 10.7 (up to
#                       K2_COMPUTE_LATE_WAIT_MIN more, default 60).
#   K2_COMPUTE_ARM=github keeps aarch64 on GitHub in compute mode.
#   Settings and node names: scripts/lib/compute-release.sh. Needs this
#   Mac's K2 as a server on the nodes with a grant (k2 compute nodes).
#   Audit: ~/k2-release-audit/<v>/compute/.
#
# Pre-flight gates (Step 0.5 + the Linux gate of 0.6), switch K2_RELEASE_GATES:
#   compute (default)  every gate is its own K2 Compute job at HEAD (plus
#                      uncommitted changes), all at once: cargo check -D
#                      warnings on Linux, full vitest on k2n1-m4,
#                      typecheck:web and vite:build:web on Linux nodes; the
#                      macOS cargo check runs here beside them. One pass/fail
#                      line per gate with its job id; a failed gate stops the
#                      release with the job's --failures summary. Compute
#                      unreachable: the old local path, and it says so. A
#                      node problem on one gate: that gate runs here.
#   local              the old path: the gates one after another on this
#                      Mac, then Step 0.6's own Linux job.
#   Settings: scripts/lib/release-gates.sh.
#
# Usage:
#   ./scripts/release.sh <version>
#   Example: ./scripts/release.sh 0.25.0
#   ./scripts/release.sh --gates-only [<commit>]   only the pre-flight gates,
#       at <commit> (default HEAD): no lock, no signing, no version, nothing
#       published. For trying the gates without a release.

set -euo pipefail

if [ "${1:-}" = "--gates-only" ]; then
    _k2_gates_dir="$(cd "$(dirname "$0")" && pwd)"
    cd "$_k2_gates_dir/.."
    # shellcheck source=scripts/lib/rust-pin.sh
    source "$_k2_gates_dir/lib/rust-pin.sh"
    # shellcheck source=scripts/lib/compute-tier.sh
    source "$_k2_gates_dir/lib/compute-tier.sh"
    # shellcheck source=scripts/lib/linux-gate-spec.sh
    source "$_k2_gates_dir/lib/linux-gate-spec.sh"
    # shellcheck source=scripts/lib/release-gates.sh
    source "$_k2_gates_dir/lib/release-gates.sh"
    k2rg_gates_only "${2:-HEAD}"
    exit $?
fi

# Staple with retry: after `notarytool --wait` reports Accepted, the ticket
# can take a minute to propagate to CloudKit — a premature staple fails with
# "could not find ticket" (Error 65). Retry instead of dying (killed the
# 0.40.23 release on first attempt).
staple_with_retry() {
    local target="$1"
    local attempt
    for attempt in 1 2 3 4 5; do
        if xcrun stapler staple "$target"; then
            return 0
        fi
        echo "  staple attempt ${attempt} failed (ticket not propagated yet?) — retrying in 30s..."
        sleep 30
    done
    echo "FATAL: stapling $target failed after 5 attempts" >&2
    return 1
}

VERSION="${1:-}"
NOTES_FILE="${2:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: ./scripts/release.sh <version> [notes-file]" >&2
    echo "Example: ./scripts/release.sh 0.25.0 release-notes.md" >&2
    echo "" >&2
    echo "If notes-file is provided, its contents are used as GitHub release notes." >&2
    echo "Otherwise, a placeholder is used (edit on GitHub after release)." >&2
    exit 1
fi

# ── Release lock (z3mbpZ is also a K2 compute node) ──
# Its node policy has foreign_locks = ["/var/tmp/k2-release.lock"]: while
# the file exists the node takes no new jobs. Write it first, wait for a
# job already running to finish (never killed), remove it on every exit
# (only while it is still ours). scripts/release-lock.sh.
# shellcheck source=release-lock.sh
source "$(cd "$(dirname "$0")" && pwd)/release-lock.sh"
if [ "$(uname -s)" = "Darwin" ]; then
    k2_release_lock_acquire "$VERSION"
    # The later EXIT trap (below) replaces this one and also releases.
    trap 'k2_release_lock_release' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    k2_release_wait_node_idle
fi

TAG="v${VERSION}"
SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
KEYCHAIN_PROFILE="K2SO-notarize"   # machine-local notarytool profile name — NOT renamed
RELEASE_REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Notarization auth (0.39.45): prefer DIRECT App Store Connect API-key
# auth when the env provides it — the keychain profile lives in the
# data-protection keychain, which headless/agent sessions can't read
# (notarytool reports "No Keychain password item found" even though the
# profile exists/validates). Set in .env:
#   ASC_API_KEY_P8=/path/to/AuthKey_XXXX.p8
#   ASC_API_KEY_ID=XXXXXXXXXX
#   ASC_API_ISSUER=<issuer-uuid>
# Falls back to --keychain-profile K2SO-notarize when unset.
notary_auth_args() {
    if [ -n "${ASC_API_KEY_P8:-}" ] && [ -n "${ASC_API_KEY_ID:-}" ] && [ -n "${ASC_API_ISSUER:-}" ]; then
        printf '%s\n' --key "$ASC_API_KEY_P8" --key-id "$ASC_API_KEY_ID" --issuer "$ASC_API_ISSUER"
    else
        printf '%s\n' --keychain-profile "$KEYCHAIN_PROFILE"
    fi
}

# Overlapped notarization (k2-node beside the app; Steps 4 and 8.55).
# shellcheck source=scripts/lib/notary.sh
source "$PROJECT_DIR/scripts/lib/notary.sh"

# rustup installs cargo at ~/.cargo/bin, which interactive shells source
# via .zshrc / .bashrc. `bun run tauri build` spawns a non-interactive
# subshell that does NOT source those, so cargo appears missing. Prepend
# explicitly to survive that spawn path.
if [ -d "$HOME/.cargo/bin" ] && ! command -v cargo >/dev/null 2>&1; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found on PATH. Install rustup or export PATH manually." >&2
    exit 1
fi

echo "═══════════════════════════════════════════════════"
echo "  K2 Release: ${TAG}  →  ${RELEASE_REPO}"
echo "═══════════════════════════════════════════════════"

# Load .env file if present (K2_GMAIL_CLIENT_ID / K2_GMAIL_CLIENT_SECRET
# for option_env! into k2-daemon mail OAuth defaults — required so Linux CI
# secrets match). It must NOT hold the signing key or its password: `set -a`
# exports everything to every build below (k2u_env_guard refuses).
if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
    set +a
    echo "Loaded .env"
fi
# shellcheck source=scripts/require-mail-oauth-build-env.sh
source "$PROJECT_DIR/scripts/require-mail-oauth-build-env.sh"
# Shared build step (also sources macos-daemon-helper-app.sh).
# shellcheck source=scripts/lib/build-binaries.sh
source "$PROJECT_DIR/scripts/lib/build-binaries.sh"
# Linux builds on K2 Compute (K2_LINUX_BUILDS; see "Linux builds on K2
# Compute" below).
# shellcheck source=scripts/lib/compute-release.sh
source "$PROJECT_DIR/scripts/lib/compute-release.sh"
# The Rust pin (rust-toolchain.toml; Step 0.4 and the compute jobs' needs).
# shellcheck source=scripts/lib/rust-pin.sh
source "$PROJECT_DIR/scripts/lib/rust-pin.sh"
# The pre-flight gates on K2 Compute (K2_RELEASE_GATES; Step 0.5).
# shellcheck source=scripts/lib/compute-tier.sh
source "$PROJECT_DIR/scripts/lib/compute-tier.sh"
# shellcheck source=scripts/lib/linux-gate-spec.sh
source "$PROJECT_DIR/scripts/lib/linux-gate-spec.sh"
# shellcheck source=scripts/lib/release-gates.sh
source "$PROJECT_DIR/scripts/lib/release-gates.sh"
# Wait loops: one line, then dots, a status line every few minutes.
# shellcheck source=scripts/lib/wait-progress.sh
source "$PROJECT_DIR/scripts/lib/wait-progress.sh"
# Step 10.6: the Linux SHA256SUMS check, and saying so when Arch fails.
# shellcheck source=scripts/lib/linux-sums-step.sh
source "$PROJECT_DIR/scripts/lib/linux-sums-step.sh"
# Gmail is required OOTB (a customer box hit invalid_client). MS is optional unless set above.
require_mail_oauth_build_env

# The update-signing key is read only inside scripts/lib/sign-updater.sh,
# at each signing step (prd-security-sweep-0464-v1 SS2/A7). Refuse to run
# when it was exported (an old .env line or shell profile): every build
# script, test and package in this run would inherit it.
# shellcheck source=scripts/lib/sign-updater.sh
source "$PROJECT_DIR/scripts/lib/sign-updater.sh"
k2u_env_guard
# Test seams only. A release verifies every signature against THIS
# checkout's public keys and takes the Linux release commit from THIS
# checkout's tag; a leftover override (a test shell, .env) would move either
# somewhere else without a word.
for _k2_seam in K2_UPDATER_SRC_ROOT K2_SIGN_SOURCE_DIR K2SIGN_TEST_ROOT K2_SIGN_APPROVE_CMD K2SA_HELPER K2SA_OPEN K2SA_NOTIFY; do
    if [ -n "${!_k2_seam:-}" ]; then
        echo "ERROR: $_k2_seam is set (${!_k2_seam}). It is a test seam: a release checks signatures and the release tag against this checkout only. Unset it and run again." >&2
        exit 1
    fi
done
unset _k2_seam
K2_SIGNER="${K2_SIGNER:-k2sign}"
case "$K2_SIGNER" in
    k2sign|keychain) export K2_SIGNER ;;
    *) echo "ERROR: K2_SIGNER must be k2sign or keychain (got $K2_SIGNER)" >&2; exit 1 ;;
esac
# shellcheck source=scripts/lib/sign-approve.sh
source "$PROJECT_DIR/scripts/lib/sign-approve.sh"

cd "$PROJECT_DIR"

# Resume after a successful build+notarize when a later hard gate flakes
# (0.40.109: Step 7.5 scp to the Tart guest). Does NOT re-bump or rebuild.
#   K2_RELEASE_RESUME=7.5  — skip 0.5–7; run pairing smoke + publish
#   K2_RELEASE_RESUME=8    — skip 0.5–7.5; publish only
# Either way Step 1.55 (the signing approval) still runs: it reuses the
# session the first run opened (same base commit and assets, within 12 h).
SKIP_BUILD=0
SKIP_SMOKE=0
case "${K2_RELEASE_RESUME:-}" in
  "") ;;
  7.5) SKIP_BUILD=1 ;;
  8)   SKIP_BUILD=1; SKIP_SMOKE=1 ;;
  *)
    echo "ERROR: K2_RELEASE_RESUME must be 7.5 or 8 (got ${K2_RELEASE_RESUME})" >&2
    exit 1
    ;;
esac
if [ "$SKIP_BUILD" = 1 ]; then
    DMG_RESUME="target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
    if [ ! -f "$DMG_RESUME" ]; then
        echo "ERROR: K2_RELEASE_RESUME=${K2_RELEASE_RESUME} but missing $DMG_RESUME" >&2
        exit 1
    fi
    echo "K2_RELEASE_RESUME=${K2_RELEASE_RESUME} — skipping build/sign/notarize; using $DMG_RESUME"
fi

# The files Step 8.9 commits as the release commit. The shadow compute builds
# use HEAD plus these same files, so they build what will be tagged.
RELEASE_COMMIT_FILES=(
    package.json src-tauri/tauri.conf.json cli/k2 Cargo.lock
    src-tauri/Cargo.toml crates/k2-core/Cargo.toml crates/k2-daemon/Cargo.toml
    crates/k2-menubar/Cargo.toml
    WHATS_NEW.md
)

# A commit object for the release commit without touching HEAD or the
# real index: HEAD's tree plus the working-tree copies of
# RELEASE_COMMIT_FILES. The shadow compute builds use it.
k2_release_preview_commit() {
    local tmpd tree
    tmpd="$(mktemp -d -t k2-release-preview)"
    GIT_INDEX_FILE="$tmpd/index" git read-tree HEAD
    GIT_INDEX_FILE="$tmpd/index" git add -- "${RELEASE_COMMIT_FILES[@]}"
    tree="$(GIT_INDEX_FILE="$tmpd/index" git write-tree)"
    rm -rf "$tmpd"
    git commit-tree "$tree" -p HEAD -m "release preview ${TAG} (pre-tag builds only; never pushed to main)"
}

# ── Linux builds on K2 Compute (see the header; K2_LINUX_BUILDS) ──
K2_LINUX_BUILDS="${K2_LINUX_BUILDS:-compute}"
export K2_LINUX_BUILDS
case "$K2_LINUX_BUILDS" in
    github|shadow|compute) ;;
    *)
        echo "ERROR: K2_LINUX_BUILDS must be compute, github or shadow (got ${K2_LINUX_BUILDS})" >&2
        exit 1
        ;;
esac
K2_COMPUTE_ARM="${K2_COMPUTE_ARM:-compute}"
export K2_COMPUTE_ARM
case "$K2_COMPUTE_ARM" in
    compute|github) ;;
    *)
        echo "ERROR: K2_COMPUTE_ARM must be compute or github (got ${K2_COMPUTE_ARM})" >&2
        exit 1
        ;;
esac
case "${K2_COMPUTE_FALLBACK:-1}" in
    0|1) ;;
    *) echo "ERROR: K2_COMPUTE_FALLBACK must be 1 or 0 (got ${K2_COMPUTE_FALLBACK})" >&2; exit 1 ;;
esac
K2_SHADOW_STATE=""
K2_SHADOW_NOTE=""
K2_SHADOW_LATE=0
# What fell back to GitHub and why ("" = nothing did). Said loudly when it
# happens and again in the final summary.
K2_COMPUTE_FELL_BACK=""

# Compute mode under the signing user (K2_SIGNER=k2sign, prd-signing-user-v1
# S3b): at Step 10.5 the signer takes the node-signed receipts (L1 and V1,
# plus L2 and V2 for aarch64) and checks them against the node keys and
# controller IT pinned. Those nodes and the controller must be pinned there,
# and each verify job must run on its own node: the signer requires the dual
# build. Read-only (k2sign compute-pins). <arch>: x86_64 or aarch64.
k2_compute_signer_check() {
    local arch="${1:-x86_64}" l1 v1 lab vlab pins
    if [ "$arch" = aarch64 ]; then
        l1="$(k2cr_arm_node)" v1="$(k2cr_arm_verify_node)" lab=L2 vlab=V2
    else
        l1="$(k2cr_x86_node)" v1="$(k2cr_verify_node)" lab=L1 vlab=V1
    fi
    if [ "${K2_COMPUTE_OAUTH:-secret}" != secret ]; then
        echo "ERROR: K2_COMPUTE_OAUTH=${K2_COMPUTE_OAUTH}, but the signing user signs a compute-built daemon only from" >&2
        echo "  secret jobs (the OAuth values from the vault): a job with them in --env may share the node's caches." >&2
        return 1
    fi
    if [ -z "$v1" ]; then
        echo "ERROR: no $vlab node, but the signing user (K2_SIGNER=k2sign) signs a compute-built" >&2
        echo "  daemon only with the dual build on a second node. Name a verify node, or use K2_LINUX_BUILDS=shadow." >&2
        return 1
    fi
    if ! pins="$(k2u_compute_pins)"; then
        echo "ERROR: the signing user can't list its K2 Compute pins (k2sign compute-pins). Re-run" >&2
        echo "  scripts/k2sign/install.sh from this commit (S3b), then pin the nodes (docs/release/updater-key-ceremony.md)." >&2
        return 1
    fi
    python3 -I - "$pins" "$l1" "$v1" "$lab" "$vlab" <<'PY'
import json, os, sys
p, l1, v1, lab, vlab = json.loads(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
names = {n["name"] for n in p.get("nodes") or []}
bad = []
if not p.get("controller"):
    bad.append("no controller pinned: sudo /usr/local/libexec/k2sign/k2sign-admin pin-compute-controller <controller_fp read on a node> /Users/%s/.k2/tunnel-key.pem" % os.environ.get("USER", "<you>"))
if l1 == v1:
    bad.append("%s and %s are both %s: the dual build needs two nodes" % (lab, vlab, l1))
for role, n in ((lab, l1), (vlab, v1)):
    if n not in names:
        bad.append("%s's node %s is not pinned: sudo /usr/local/libexec/k2sign/k2sign-admin pin-compute-node %s /abs/path/%s.pem <its node_fp, read on the node>" % (role, n, n, n))
if bad:
    print("ERROR: K2_LINUX_BUILDS=compute: the signing user would refuse the compute-built daemon at Step 10.5:", file=sys.stderr)
    for b in bad:
        print("  - " + b, file=sys.stderr)
    sys.exit(1)
print("  Signing user: compute builds from %s %s and %s %s for controller %s… (prd-signing-user-v1 S3b)." % (lab, l1, vlab, v1, p["controller"][:16]))
PY
}

# The repo variable LINUX_BUILDS tells the tag-push workflows who builds
# Linux: "compute-all" = daemon-binaries.yml builds nothing and app-linux.yml
# skips (this run built x86_64 AND aarch64 on compute); "compute" = only
# aarch64 builds on GitHub (x86_64 on compute); anything else or unset =
# GitHub builds both. release.sh sets it itself, right before the tag push
# (k2_linux_builds_set_var), to what this run really did, so a fallback can't
# leave GitHub building nothing.
k2_linux_builds_want_var() {
    if [ "$K2_LINUX_BUILDS" = compute ]; then
        if [ "$K2_COMPUTE_ARM" = compute ]; then printf 'compute-all'; else printf 'compute'; fi
    else
        printf 'github'
    fi
}
k2_linux_builds_get_var() { gh variable get LINUX_BUILDS --repo "$RELEASE_REPO" 2>/dev/null || true; }
# 0 = the variable already says <want> (unset counts as github).
k2_linux_builds_var_is() {
    local cur
    cur="$(k2_linux_builds_get_var)"
    [ "$cur" = "$1" ] || { [ -z "$cur" ] && [ "$1" = github ]; }
}
# 0 = this gh may set repo variables (repo admin).
k2_linux_builds_can_set_var() {
    [ "$(gh api "repos/$RELEASE_REPO" --jq .permissions.admin 2>/dev/null || true)" = "true" ]
}

# Every check compute mode needs before any build, for one arch. Non-zero,
# with the reasons on stderr, when that arch can't come from compute.
k2_compute_ready() {
    local arch="$1"
    if [ "$arch" = x86_64 ]; then
        case "${K2_COMPUTE_OAUTH:-secret}" in
            secret|env) ;;
            *)
                echo "ERROR: K2_LINUX_BUILDS=compute needs K2_COMPUTE_OAUTH=secret (got ${K2_COMPUTE_OAUTH}):" >&2
                echo "  a daemon built without the Gmail OAuth values must never ship." >&2
                return 1
                ;;
        esac
        # V1 on L1's node is no second build, whoever signs (Step 1.6c
        # refuses it too, in every mode).
        local l1 v1
        l1="$(k2cr_x86_node)" v1="$(k2cr_verify_node)"
        if [ -n "$v1" ] && [ "$l1" = "$v1" ]; then
            echo "  ✗ L1 and V1 are both on $l1: the dual build needs two nodes. Set K2_COMPUTE_VERIFY_NODE to another node." >&2
            return 1
        fi
        if [ "${K2_COMPUTE_EXCLUSIVE:-1}" != 1 ]; then
            echo "ERROR: K2_LINUX_BUILDS=compute needs the release builds --exclusive (K2_COMPUTE_EXCLUSIVE=${K2_COMPUTE_EXCLUSIVE}):" >&2
            echo "  the signing user refuses a build that other jobs could have run beside." >&2
            return 1
        fi
    else
        k2cr_check_arm_nodes || return 1
    fi
    # The signing user signs a compute-built daemon only on the node-signed
    # receipts it checks itself (prd-signing-user-v1 S3b). Find out now, not
    # at Step 10.5 after the release is live.
    if [ "$K2_SIGNER" = "k2sign" ]; then
        k2_compute_signer_check "$arch" || return 1
    fi
}

# Say it plainly, once: what goes to GitHub instead and why. <scope>: all
# (x86_64 and aarch64) or arm (aarch64 only). Only ever before the tag.
# K2_COMPUTE_FALLBACK=0: stop the release instead (nothing pushed yet).
k2_compute_fallback() {
    local scope="$1"
    shift
    if [ "${K2_COMPUTE_FALLBACK:-1}" != 1 ]; then
        k2_compute_fatal "$*"
    fi
    local what="Linux x86_64 AND aarch64"
    [ "$scope" = arm ] && what="Linux aarch64"
    {
        echo ""
        echo "  ════════════════════════════════════════════════════════════════════"
        echo "  ⚠⚠ K2 COMPUTE FAILED: ${what} FALL BACK TO GITHUB FOR THIS RELEASE ⚠⚠"
        echo "  why: $*"
        [ -n "$K2_SHADOW_STATE" ] && echo "  audit: ${K2_SHADOW_STATE}/summary.txt"
        echo "  GitHub's workflows build ${what} after the tag, and Steps 10.5/10.6 wait"
        echo "  for them (about 60-120 min more). Nothing was pushed or tagged yet."
        echo "  To stop instead next time: K2_COMPUTE_FALLBACK=0."
        echo "  ════════════════════════════════════════════════════════════════════"
        echo ""
    } >&2
    K2_COMPUTE_FELL_BACK="${K2_COMPUTE_FELL_BACK:+$K2_COMPUTE_FELL_BACK; }${what}: $*"
    if [ "$scope" = arm ]; then
        k2cr_cancel_arm || true
        K2_COMPUTE_ARM=github
    else
        k2cr_cancel_open || true
        K2_LINUX_BUILDS=github
        K2_COMPUTE_ARM=github
        k2_compute_unstage
    fi
}

# Pre-flight (before any build): can this run build Linux on compute, and
# can it tell GitHub so? Compute mode falls back to GitHub (loudly) when it
# can't; K2_COMPUTE_FALLBACK=0 stops instead.
k2_linux_builds_preflight() {
    local err="${TMPDIR:-/tmp}/k2-compute-ready.$$"
    if [ "$K2_LINUX_BUILDS" = "compute" ]; then
        if ! k2_compute_ready x86_64 2>"$err"; then
            cat "$err" >&2
            k2_compute_fallback all "not ready for compute builds: $(grep -m1 -E 'ERROR|✗|- ' "$err" | sed 's/^ *//')"
        elif [ "$K2_COMPUTE_ARM" = compute ] && k2cr_arm_on && ! k2_compute_ready aarch64 2>"$err"; then
            cat "$err" >&2
            k2_compute_fallback arm "not ready for the aarch64 compute builds: $(grep -m1 -E 'ERROR|✗|- ' "$err" | sed 's/^ *//')"
        elif ! k2cr_arm_on; then
            K2_COMPUTE_ARM=github
        fi
        rm -f "$err"
    fi
    if [ "$K2_LINUX_BUILDS" = "compute" ] && [ -z "$(k2cr_verify_node)" ]; then
        echo "  ⚠⚠ K2_COMPUTE_VERIFY_NODE=none: NO dual build. The daemon, the root helper, k2-node and the App engine" >&2
        echo "  ⚠⚠ ship from ONE node's word (plan §6). Use it only when a release node is down." >&2
    fi
    # The repo variable is set right before the tag push. Make sure this gh
    # can set it, or that it already says what this run will need.
    local want
    want="$(k2_linux_builds_want_var)"
    if ! k2_linux_builds_var_is "$want" && ! k2_linux_builds_can_set_var; then
        if [ "$K2_LINUX_BUILDS" = compute ] && k2_linux_builds_var_is github; then
            k2_compute_fallback all "this gh can't set the repo variable LINUX_BUILDS to ${want} (needs repo admin): gh variable set LINUX_BUILDS -b ${want} --repo $RELEASE_REPO"
        else
            echo "ERROR: the repo variable LINUX_BUILDS is '$(k2_linux_builds_get_var)', this run needs '${want}', and this gh can't set it" >&2
            echo "  (needs repo admin). Set it, then run again: gh variable set LINUX_BUILDS -b ${want} --repo $RELEASE_REPO" >&2
            exit 1
        fi
    fi
    case "$K2_LINUX_BUILDS" in
        compute)
            if [ "$K2_COMPUTE_ARM" = compute ]; then
                echo "  Linux x86_64 and aarch64: K2 Compute ($(k2cr_x86_node)+$(k2cr_verify_node), $(k2cr_arm_node)+$(k2cr_arm_verify_node); L3 ${K2_COMPUTE_APP_NODE:-${K2_COMPUTE_NODE:-k2n8-vin}}). Arch: GitHub (k2-arch), not waited for."
            else
                echo "  Linux x86_64: K2 Compute; aarch64 and Arch: GitHub (K2_COMPUTE_ARM=github)."
            fi
            ;;
        shadow) echo "  Linux: GitHub (shadow builds on K2 Compute, compared at Step 10.7)." ;;
        github) echo "  Linux: GitHub (K2_LINUX_BUILDS=github)." ;;
    esac
}
k2_linux_builds_preflight

# Right before the tag push (from k2_shadow_tree_check): the repo variable
# says what this run really did, so GitHub builds exactly the rest.
k2_linux_builds_set_var() {
    local want got
    want="$(k2_linux_builds_want_var)"
    if k2_linux_builds_var_is "$want"; then
        echo "  repo variable LINUX_BUILDS: ${want} (GitHub builds $(k2_linux_builds_github_part "$want"))."
        return 0
    fi
    if ! gh variable set LINUX_BUILDS -b "$want" --repo "$RELEASE_REPO" >/dev/null 2>&1; then
        echo "FATAL: could not set the repo variable LINUX_BUILDS to ${want}; nothing was pushed or tagged." >&2
        echo "  Set it by hand, then re-run: gh variable set LINUX_BUILDS -b ${want} --repo $RELEASE_REPO" >&2
        exit 1
    fi
    got="$(k2_linux_builds_get_var)"
    if [ "$got" != "$want" ]; then
        echo "FATAL: the repo variable LINUX_BUILDS reads '${got}' after setting it to ${want}; nothing was pushed or tagged." >&2
        exit 1
    fi
    echo "  repo variable LINUX_BUILDS set to ${want} (GitHub builds $(k2_linux_builds_github_part "$want"))."
}
k2_linux_builds_github_part() {
    case "$1" in
        compute-all) printf 'no Linux binaries; Arch on k2-arch' ;;
        compute) printf 'Linux aarch64 and Arch' ;;
        *) printf 'every Linux file' ;;
    esac
}

k2_shadow_warn() {
    echo "  ⚠ shadow (K2 Compute): $*" >&2
    K2_SHADOW_NOTE="${K2_SHADOW_NOTE:+$K2_SHADOW_NOTE; }$*"
}

# The end of the release (K2_COMPUTE_FALLBACK=0 only): always before the
# tag, so nothing committed, tagged or published.
k2_compute_fatal() {
    echo "" >&2
    echo "  FATAL (K2 Compute, stage 1): $*" >&2
    [ -n "$K2_SHADOW_STATE" ] && echo "  FATAL: audit: ${K2_SHADOW_STATE}/summary.txt" >&2
    echo "  FATAL: nothing was pushed, tagged or published (K2_COMPUTE_FALLBACK=0). Fix it and re-run, or let GitHub build Linux:" >&2
    if [ -f "$PROJECT_DIR/target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" ]; then
        # The Mac build and notarization are done: reuse them.
        echo "  FATAL:   K2_LINUX_BUILDS=github K2_RELEASE_RESUME=8 ./scripts/release.sh ${VERSION}   (8 = publish only; keeps this Mac build)" >&2
    else
        echo "  FATAL:   K2_LINUX_BUILDS=github ./scripts/release.sh ${VERSION}" >&2
    fi
    exit 1
}

# A K2 Compute problem with the x86_64 builds: GitHub builds all of Linux
# instead (compute mode; k2_compute_fallback), a warning in shadow mode.
k2_compute_problem() {
    if [ "$K2_LINUX_BUILDS" = "compute" ]; then
        k2_compute_fallback all "$*"
        return 0
    fi
    k2_shadow_warn "$*"
}

k2_compute_on() { [ "$K2_LINUX_BUILDS" = "shadow" ] || [ "$K2_LINUX_BUILDS" = "compute" ]; }

# Step 1.6c. The release commit's tree must be final now, so the lock
# file gets the bumped workspace versions here (Step 2 would write the
# same lines). Shadow: never fatal. Compute: a problem sends Linux to
# GitHub (k2_compute_problem), before the Mac build.
k2_shadow_start() {
    k2_compute_on || return 0
    echo ""
    if [ "$K2_LINUX_BUILDS" = "compute" ]; then
        echo "Step 1.6c: Linux builds on K2 Compute (COMPUTE: these files are the release)..."
    else
        echo "Step 1.6c: Linux builds on K2 Compute (SHADOW: GitHub stays the source of truth)..."
    fi
    if ! cargo update --workspace --offline >/dev/null 2>"/tmp/k2-shadow-lock-${VERSION}.err"; then
        k2_compute_problem "cargo update --workspace --offline failed ($(head -c 200 "/tmp/k2-shadow-lock-${VERSION}.err")); the tagged tree may differ"
        k2_compute_on || return 0
    fi
    local commit
    if ! commit="$(k2_release_preview_commit)"; then
        k2_compute_problem "could not make the release preview commit; no compute builds"
        return 0
    fi
    K2_SHADOW_STATE="${K2_RELEASE_AUDIT_DIR:-$HOME/k2-release-audit}/${VERSION}/compute"
    if ! k2cr_init "$VERSION" "$commit" "$K2_SHADOW_STATE" "$PROJECT_DIR/target/compute-dist/${VERSION}"; then
        local failed="$K2_SHADOW_STATE"
        K2_SHADOW_STATE=""
        k2_compute_problem "could not set up $failed"
        return 0
    fi
    echo "  Release commit ${commit:0:12} (tree ${K2CR_TREE:0:12}); audit ${K2_SHADOW_STATE}"
    if ! k2cr_submit_release_set; then
        k2_compute_problem "not every compute job was submitted"
        k2_compute_on || return 0
    fi
    if [ "$K2_LINUX_BUILDS" = compute ] && [ "$K2_COMPUTE_ARM" = compute ] && [ "${K2CR_ARM_SUBMIT_OK:-}" != 1 ]; then
        k2_compute_fallback arm "the aarch64 jobs (L2, V2) were not submitted"
    fi
}

# Step 7.9c: wait, fetch, verify. Shadow: never fatal. Compute: an x86_64
# problem sends all of Linux to GitHub, an aarch64 one only aarch64; a
# K2_RELEASE_RESUME run (no jobs: the first run's exit cancelled them)
# submits them now and waits.
k2_shadow_collect() {
    k2_compute_on || return 0
    echo ""
    echo "Step 7.9c: K2 Compute Linux builds: wait, fetch, verify (${K2_LINUX_BUILDS})..."
    if [ -z "$K2_SHADOW_STATE" ] || [ ! -s "$K2_SHADOW_STATE/jobs.tsv" ]; then
        if [ "$K2_LINUX_BUILDS" = "compute" ]; then
            echo "  No compute jobs from this run (a K2_RELEASE_RESUME run): submitting them now."
            k2_shadow_start
            k2_compute_on || return 0
        else
            k2_shadow_warn "no compute jobs to collect (not submitted, or a K2_RELEASE_RESUME run)"
            return 0
        fi
    fi
    local t0
    t0=$(date +%s)
    if [ "$K2_LINUX_BUILDS" = "shadow" ]; then
        if ! k2cr_wait "${K2_COMPUTE_WAIT_MIN:-60}"; then
            # Shadow never holds the release up: collect at Step 10.7 instead,
            # after GitHub's builds.
            echo "  compute jobs still running after ${K2_COMPUTE_WAIT_MIN:-60} min; collected at Step 10.7 instead."
            K2_SHADOW_LATE=1
            return 0
        fi
        k2cr_collect || k2_shadow_warn "not every compute build verified (see ${K2_SHADOW_STATE}/summary.txt)"
    else
        # Compute: these files are the release, so a problem sends them to
        # GitHub here, before the tag.
        if ! k2cr_wait "${K2_COMPUTE_WAIT_MIN:-60}"; then
            k2_compute_problem "jobs not done in ${K2_COMPUTE_WAIT_MIN:-60} min"
            return 0
        fi
        k2cr_collect || true
        if [ "${K2CR_X86_OK:-}" != 1 ]; then
            k2_compute_problem "the x86_64 builds did not all verify (see ${K2_SHADOW_STATE}/summary.txt)"
            return 0
        fi
        if [ ! -s "$K2_SHADOW_STATE/V1.verdict.json" ] && [ -n "$(k2cr_verify_node)" ]; then
            k2_compute_problem "no V1 verdict: the dual build didn't run"
            return 0
        fi
        if [ "$K2_COMPUTE_ARM" = compute ] && [ "${K2CR_ARM_OK:-}" != 1 ]; then
            k2_compute_fallback arm "the aarch64 builds (L2, V2) did not all verify (see ${K2_SHADOW_STATE}/summary.txt)"
        fi
    fi
    echo "  waited $(( ($(date +%s) - t0) / 60 )) min after the Mac build."
}

# Before Step 8.9 pushes: the repo variable says what this run did, and the
# commit about to be tagged must have the tree compute built. Compute: a
# tree mismatch sends Linux to GitHub (nothing pushed yet). Shadow: a warning.
k2_shadow_tree_check() {
    if k2_compute_on && [ -n "$K2_SHADOW_STATE" ]; then
        local tagged
        tagged="$(git rev-parse 'HEAD^{tree}')"
        if [ "$tagged" = "$K2CR_TREE" ]; then
            echo "  ✓ the release commit's tree is the tree K2 Compute built (${tagged:0:12})." | tee -a "$K2_SHADOW_STATE/summary.txt"
        else
            echo "  ✗ release tree ${tagged:0:12} != compute-built tree ${K2CR_TREE:0:12}" | tee -a "$K2_SHADOW_STATE/summary.txt"
            k2_compute_problem "the release commit's tree differs from the one compute built"
        fi
    fi
    k2_linux_builds_set_var
}

# Files an earlier compute run staged in DIST_DIR go (only those, never
# outside DIST_DIR), so an old build never rides along with a later release.
k2_compute_unstage() {
    # DIST_DIR is set at Step 8.5; before it nothing was staged this run.
    [ -n "${DIST_DIR:-}" ] || return 0
    local marker="$DIST_DIR/.compute-staged" f
    [ -f "$marker" ] || return 0
    while IFS= read -r f; do
        case "$f" in ""|*/*) continue ;; esac
        rm -f "$DIST_DIR/$f" "$DIST_DIR/$f.sig"
    done < "$marker"
    rm -f "$marker"
}

# Step 8.5 (compute): the verified Linux files go into DIST_DIR, where the
# manifest, signing and Step 9 pick them up (x86_64, and aarch64 unless it
# fell back). Files a previous compute run staged are removed first in every
# mode.
k2_compute_stage() {
    k2_compute_unstage
    [ "$K2_LINUX_BUILDS" = "compute" ] || return 0
    if ! k2cr_stage "$DIST_DIR"; then
        k2_compute_problem "staging the compute-built files failed"
        return 0
    fi
    cp "$K2_SHADOW_STATE/staged.txt" "$DIST_DIR/.compute-staged"
}

# Step 10.7: compare with the GitHub-built assets (they exist after 10.5/10.6).
k2_shadow_report() {
    if [ "$K2_LINUX_BUILDS" = "compute" ]; then
        echo ""
        if [ "$K2_COMPUTE_ARM" = compute ]; then
            echo "  Linux x86_64 and aarch64 came from K2 Compute; audit: ${K2_SHADOW_STATE} (summary.txt, verdicts, receipts)."
        else
            echo "  Linux x86_64 came from K2 Compute, aarch64 from GitHub; audit: ${K2_SHADOW_STATE} (summary.txt, verdicts, receipts)."
        fi
        [ -n "${K2CR_STATE:-}" ] && k2cr_job_times
        return 0
    fi
    [ "$K2_LINUX_BUILDS" = "shadow" ] || return 0
    echo ""
    echo "Step 10.7: Shadow report, K2 Compute vs GitHub Linux assets..."
    if [ "$K2_SHADOW_LATE" = "1" ] && [ -n "$K2_SHADOW_STATE" ]; then
        echo "  collecting the compute jobs Step 7.9c left running (up to ${K2_COMPUTE_LATE_WAIT_MIN:-60} min more)..."
        k2cr_wait "${K2_COMPUTE_LATE_WAIT_MIN:-60}" || k2_shadow_warn "jobs still not done after ${K2_COMPUTE_LATE_WAIT_MIN:-60} more min"
        k2cr_collect || k2_shadow_warn "not every compute build verified (see ${K2_SHADOW_STATE}/summary.txt)"
    fi
    if [ -z "$K2_SHADOW_STATE" ] || [ "${K2CR_COLLECTED:-0}" != "1" ]; then
        k2_shadow_warn "nothing collected; no report"
    else
        k2cr_compare_github "$TAG" || k2_shadow_warn "shadow compare failed"
        echo "  Audit: ${K2_SHADOW_STATE} (summary.txt, shadow-report.json)"
    fi
    [ -n "$K2_SHADOW_NOTE" ] && echo "  shadow notes: $K2_SHADOW_NOTE"
    return 0
}

# EXIT trap: stop shadow jobs nobody will read, a background standalone
# daemon build nobody will collect (Step 3.6), and drop the release lock.
# (The name is left over from the pre-tag Arch gate.)
k2_arch_gate_on_exit() {
    local rc=$?
    k2_standalone_macos_daemon_kill || true
    if k2_compute_on; then
        k2cr_cancel_open || true
    fi
    k2rg_cancel_open || true
    k2_release_lock_release
    exit "$rc"
}
trap k2_arch_gate_on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# ── Step 0.4: Rust pin (rust-toolchain.toml) ──
# Every build of this release (here, the K2 Compute nodes, GitHub) uses the
# same rustc. scripts/lib/rust-pin.sh. Escape: K2_SKIP_RUST_PIN_CHECK=1.
echo ""
echo "Step 0.4: Rust toolchain pin..."
k2_rust_pin_check

# ── Step 0.45: Update-signing preflight (SS1/SS2) ──
# Before any build, and on resumed runs too (Step 8.5 and 10.5 still sign):
# the key file (0600 in a 0700 dir), its keychain password, minisign, a
# self-test signature that verifies with the source tree's public key, and
# no TAURI_SIGNING_* secrets left on GitHub.
echo ""
echo "Step 0.45: Update-signing preflight..."
# The update keys must agree in every verifier copy (k2-release-keys
# `copies` test: lib.rs, cli/k2, install-daemon.sh, tauri.conf.json). A
# dropped copy would ship installs that can't verify this release.
cargo test -p k2-release-keys
bash "$PROJECT_DIR/scripts/sign-daemon.sh" --preflight "$VERSION"

# Windows is decided here (not at Step 9.5) because the approval (Step 1.55)
# names it.
# Same rule as before: off unless K2_SKIP_WINDOWS_NSIS=0 or
# K2_REQUIRE_WINDOWS_NSIS=1.
if [ "${K2_REQUIRE_WINDOWS_NSIS:-0}" = "1" ]; then
    export K2_SKIP_WINDOWS_NSIS="${K2_SKIP_WINDOWS_NSIS:-0}"
else
    export K2_SKIP_WINDOWS_NSIS="${K2_SKIP_WINDOWS_NSIS:-1}"
fi

# ── Step 0.5: Pre-flight quality gates ──
# Added at 0.40.31 when the tree first reached warning-zero / typecheck-zero.
# Fail the release BEFORE any version bump or build if regressions slipped in.
# NOTE: bare `tsc --noEmit` is vacuous here (root tsconfig has files:[] +
# references) — typecheck:web (tsconfig.web.json) is the real renderer check.
# typecheck:web runs tsc-rs (pinned, TS 7 rules; docs/typecheck.md).
# vite:build:web is the hosted-SPA production build (vite.config.web.ts) —
# typecheck alone does not prove the Vite graph / CSS / assets still bundle.
# R2 publish is Step 8.6 and stays best-effort; this gate is build-only.
if [ "$SKIP_BUILD" = 0 ]; then
echo ""
echo "Step 0.5: Pre-flight gates (cargo -D warnings, typecheck:web (tsc-rs), vitest, vite:build:web)..."
# The local commands (k2rg_local in scripts/lib/release-gates.sh) use the
# same mac build env as the release build (scripts/lib/build-binaries.sh):
# SDKROOT from xcrun, the cmake policy floor, and no half-configured cmake
# dirs. Without it the check build's vendored cmake (llama-cpp-sys) picked
# the Command Line Tools' default SDK, which on macOS 27 the Xcode 26.5 ld
# rejects ("tapi: unknown architecture"): 0.46.3 attempt 1 died here.
K2_GATES_WHERE="$(k2rg_mode)"
if [ "$K2_GATES_WHERE" = compute ] && ! k2rg_reachable; then
    echo "  K2 Compute is unreachable ($K2RG_WHY):"
    echo "  running the gates here, one after another (the old path, K2_RELEASE_GATES=local)."
    K2_GATES_WHERE=local
fi
if [ "$K2_GATES_WHERE" = local ]; then
    k2rg_local_all
else
    k2rg_begin "$(git rev-parse HEAD)" release
    if ! k2rg_submit_all; then
        k2rg_cancel_open
        echo "  K2 Compute refused a gate job (above): running the gates here, one after another (the old path)."
        K2_GATES_WHERE=local
        k2rg_local_all
    fi
fi
if [ "$K2_GATES_WHERE" = compute ]; then
    if [ "$(_k2rg_mac_where)" = local ]; then
        echo "  cargo-macos: here, beside the jobs (cargo check -D warnings)..."
        k2rg_local cargo-macos
        echo "  ✓ cargo-macos      here"
    fi
    echo "  waiting for the gate jobs (each prints a line as it ends)..."
    K2_GATES_RC=0
    k2rg_wait_all || K2_GATES_RC=$?
    if [ "$K2_GATES_RC" != 0 ]; then
        k2rg_summary
        echo "FATAL: a pre-flight gate failed (above). Nothing was bumped or built." >&2
        exit 1
    fi
    for _k2_gate in $K2RG_INFRA; do
        echo "  $_k2_gate: a node problem on K2 Compute, so it runs here instead..."
        k2rg_local "$_k2_gate"
        if [ "$_k2_gate" = cargo-linux ]; then K2RG_LINUX_OK=1; K2RG_LINUX_JOB=here; fi
        echo "  ✓ $_k2_gate here"
    done
    k2rg_summary
    echo "  Pre-flight gates passed."
fi

# ── Step 0.55: Root-helper signing preflight (prd-k2-root-helper-v1 §9.1) ──
# The helper is signed after CI builds it (Step 10.5) with the SEPARATE
# root-helper key that lives only on this Mac. Fail now, not after a
# 30-minute build, if the key / keychain password / RELEASE_KEYS are not
# ready. Escape (no root-helper signature this release; boxes keep their
# current helper): K2_SKIP_ROOT_HELPER_SIGN=1 (loud).
echo ""
if [ "${K2_SKIP_ROOT_HELPER_SIGN:-0}" = "1" ]; then
    echo "  WARNING: K2_SKIP_ROOT_HELPER_SIGN=1 — this release's k2-root-helper will NOT be signed;" >&2
    echo "  WARNING: auto-heal and install-root-helper.sh --version refuse unsigned assets." >&2
else
    echo "Step 0.55: Root-helper signing preflight..."
    bash "$PROJECT_DIR/scripts/sign-root-helper.sh" --preflight
fi
# k2-node (prd-k2-node-admin-v1 §8.1) is signed with the SAME root-helper key
# at Step 10.5: nodes self-update only to builds signed by it. Escape (no
# k2-node signature this release; nodes and `k2 compute local enable`
# downloads refuse the unsigned assets, nothing breaks): K2_SKIP_K2_NODE_SIGN=1.
if [ "${K2_SKIP_K2_NODE_SIGN:-0}" = "1" ]; then
    echo "  WARNING: K2_SKIP_K2_NODE_SIGN=1 — this release's k2-node assets will NOT be signed;" >&2
    echo "  WARNING: compute nodes can't update to it and Linux daemons can't download it." >&2
else
    echo "Step 0.55: k2-node signing preflight (root-helper key)..."
    bash "$PROJECT_DIR/scripts/sign-k2-node.sh" --preflight
fi
# k2-app-engine (prd-k2-app-runtime-v1 §10.6, S11) is signed with the SAME
# root-helper key at Step 10.5: the root helper installs only builds signed
# by it. Escape (no engine signature this release; `apps install-engine`
# and the daemon's download refuse the unsigned assets, guarded Apps stay
# on the installed engine): K2_SKIP_APP_ENGINE_SIGN=1.
if [ "${K2_SKIP_APP_ENGINE_SIGN:-0}" = "1" ]; then
    echo "  WARNING: K2_SKIP_APP_ENGINE_SIGN=1 — this release's k2-app-engine assets will NOT be signed;" >&2
    echo "  WARNING: boxes can't install or update the App engine from it." >&2
else
    echo "Step 0.55: k2-app-engine signing preflight (root-helper key)..."
    bash "$PROJECT_DIR/scripts/sign-app-engine.sh" --preflight
fi

# ── Step 0.6: Linux build gate (a K2 Compute job; K2_LINUX_GATE=ssh = the old box) ──
# Shared with scripts/build-app.sh — full rationale and settings live in
# scripts/linux-build-gate.sh: `cargo check --workspace --all-targets`
# (the k2 crate too) at HEAD on a Linux node, --tier ci, the pinned Rust.
# No GitHub runner is involved. Escape: K2_SKIP_LINUX_GATE=1 (loud).
echo ""
echo "Step 0.6: Linux build gate..."
if [ "${K2RG_LINUX_OK:-0}" = 1 ]; then
    echo "  passed in Step 0.5 (cargo-linux, job ${K2RG_LINUX_JOB})."
else
"$PROJECT_DIR/scripts/linux-build-gate.sh"
fi

# ── Step 1: Bump version ──
echo ""
echo "Step 1: Bumping version to ${VERSION}..."
sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"${VERSION}\"/" package.json src-tauri/tauri.conf.json
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" \
    src-tauri/Cargo.toml \
    crates/k2-core/Cargo.toml \
    crates/k2-daemon/Cargo.toml \
    crates/k2-menubar/Cargo.toml
sed -i '' "s/K2_CLI_VERSION=\"[^\"]*\"/K2_CLI_VERSION=\"${VERSION}\"/" cli/k2
echo "  Done."

# ── Step 1.5: Verify WHATS_NEW.md has an entry for this version ──
#
# 0.38.7 release-script contract: every released version MUST have a
# matching `## <version>` header in WHATS_NEW.md (the user-facing
# changelog the Tauri popup displays on first launch after update).
#
# This is intentionally a hard gate — if the entry is missing, the
# release halts BEFORE any build/sign/notarize work so the developer
# has to write the user-facing notes first. WHATS_NEW.md is curated
# user-friendly language (separate audience from docs/changelog/release-notes-X.Y.Z.md
# which goes to the GitHub release body).
echo ""
echo "Step 1.5: Verifying WHATS_NEW.md has an entry for ${VERSION}..."
if [ ! -f "WHATS_NEW.md" ]; then
    echo "  ERROR: WHATS_NEW.md not found at repo root." >&2
    echo "  Create it before releasing — see existing entries for format." >&2
    exit 1
fi
if ! grep -qE "^## ${VERSION//./\\.} " WHATS_NEW.md; then
    echo "  ERROR: WHATS_NEW.md has no '## ${VERSION} — ...' section." >&2
    echo "" >&2
    echo "  Every release needs a user-facing changelog entry. Add a" >&2
    echo "  section like:" >&2
    echo "" >&2
    echo "    ## ${VERSION} — Short headline" >&2
    echo "" >&2
    echo "    Friendly 1–3 paragraph description of what changed for" >&2
    echo "    end users. Lead with what they'll notice, not implementation" >&2
    echo "    detail. The Tauri app shows this in a popup on first launch" >&2
    echo "    after this version installs." >&2
    echo "" >&2
    echo "  Then re-run: ./scripts/release.sh ${VERSION} ${NOTES_FILE:-<notes-file>}" >&2
    exit 1
fi
echo "  Found '## ${VERSION}' entry. OK to proceed."
fi # SKIP_BUILD (the checks before the approval)

# ── Step 1.55: Signing approval, once: Touch ID or the login password (prd-signing-user-v1 S2) ──
# After every check that builds nothing (Steps 0.4–1.5: the pin, the
# signing preflights, the pre-flight gates, the Linux build gate, the version
# bump in the working tree, WHATS_NEW) and before anything is built, signed
# or published (Step 1.6c on). A gate that fails (a flaky vitest) then costs
# no approval: fix it, commit, re-run, and that run asks once for its own
# base commit. (0.46.11: the approval came first, a gate failed, and the
# re-run with the fix commit was refused for the different base.)
# A missed prompt times out (K2_SIGN_APPROVE_TIMEOUT_MIN, default 30) and
# costs nothing. The session binds this version, the base commit (HEAD now;
# the bump is committed at Step 8.9) and the asset list, for 12 h, which
# covers K2_RELEASE_RESUME (resumed runs skip 0.5–7 but still come here and
# reuse the session) and by-hand sign-daemon.sh re-runs; after that it asks
# to RE-APPROVE. K2_SIGNER=keychain skips it.
echo ""
echo "Step 1.55: Signing approval (Touch ID or your Mac login password)..."
if [ "$K2_SIGNER" = k2sign ]; then
    K2_SIGN_BASE="$(git rev-parse HEAD)"
    # A resumed run after Step 8.9 sits on the release commit: its parent
    # is the base the session was approved for.
    if [ "$(git log -1 --format=%s HEAD)" = "chore(release): ${TAG}" ]; then
        K2_SIGN_BASE="$(git rev-parse HEAD^)"
    fi
    K2_SIGN_ASSETS=(K2.app.tar.gz k2-daemon-macos-aarch64 k2-daemon-linux-x86_64 k2-daemon-linux-aarch64)
    if [ "${K2_SKIP_WINDOWS_NSIS}" != "1" ]; then
        K2_SIGN_ASSETS+=(--windows setup.exe "K2_${VERSION}_x64-setup.exe")
    fi
    k2sa_open_session "$VERSION" "$K2_SIGN_BASE" "${K2_SIGN_ASSETS[@]}" || exit 1
else
    echo "  K2_SIGNER=keychain (rollback): no approval session; the stopgap signs as this user."
fi

if [ "$SKIP_BUILD" = 0 ]; then
# ── Step 1.6c: Linux builds on K2 Compute (shadow or compute mode, background) ──
k2_shadow_start

# ── Step 2: Build K2.app + every sidecar binary ──
#
# One shared build step for dev-build.sh, build-app.sh and release.sh:
# scripts/lib/build-binaries.sh. It runs `tauri build` (frontend + the `k2`
# Tauri crate + bundle, in its own target dir target/tauri) AT THE SAME
# TIME as the build of k2-daemon + k2-power-helper, k2-ptyd, k2-export,
# k2-node, k2-menubar and k2-deskd in release mode, in ONE cargo call
# (K2_MAC_SIDECAR_CARGO_ARGS: one feature resolution, so the shared crates
# compile once; Step 8.5 reuses them), and copies them in: `tauri build`
# copies only its own primary bin into the bundle. The sidecars are the
# long pole (17–21 min); the tauri build (4–7.5) runs inside that.
# K2_MAC_BUILD_PARALLEL=0 runs them one after the other as before.
#
# k2-daemon is a peer binary to the main Tauri app that owns the
# persistent-agent runtime (launched by launchd, outlives the Tauri
# process). Since 0.43.2 it ships in a nested background-only helper
# app, `Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon`, so
# Activity Monitor shows the K2 icon + "K2 Daemon". The app finds it
# via `k2_core::daemon_lifecycle::bundled_daemon_path` (plist install
# + heal); sidecars stay in `Contents/MacOS/`. k2-ptyd (PRD
# prd-k2-ptyd-v1 §7.1), the PTY host the daemon launches so agents can
# outlive it, is nested beside k2-daemon (the daemon looks next to its
# own exe), signed inside k2_daemon_helper_sign (hardened runtime, no
# entitlements), notarized with the app below. Own version line:
# release.sh never bumps crates/k2-ptyd. k2-node rides inside K2.app so
# Settings → Compute installs it; k2-menubar is staged to ~/.k2/bin on
# first GUI open. Hardened-runtime signing in Step 3 covers them all.
# OAuth client id/secret already required + exported (require_mail_oauth_build_env).
echo ""
echo "Step 2: Building release (K2.app + sidecars)..."
export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
export APPLE_TEAM_ID="36B8R93HXV"
k2_build_mac_binaries "target/release/bundle/macos/K2.app"
echo "  Build complete."

# ── Step 3: Sign with hardened runtime ──
echo ""
echo "Step 3: Signing with hardened runtime..."
# 0.37.9: pass --entitlements so codesign attaches our requested
# capabilities (audio-input for Apple Dictation, JIT/library
# validation relaxations for wry/WKWebView). Without this, the
# hardened runtime denies audio access and Fn-Fn silently fails.
ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements.plist"
if [ ! -f "$ENTITLEMENTS" ]; then
    echo "  FATAL: entitlements file not found at $ENTITLEMENTS" >&2
    exit 1
fi
# Inner binaries first (Apple requires sub-binaries signed before the
# outer bundle, otherwise codesign rejects with 'resource fork … not
# allowed').
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2"
# Nested K2 Daemon.app: its own, smaller entitlements (K2 vault KV6/VS14).
DAEMON_ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements-daemon.plist"
[ -f "$DAEMON_ENTITLEMENTS" ] || { echo "  FATAL: daemon entitlements not found at $DAEMON_ENTITLEMENTS" >&2; exit 1; }
k2_daemon_helper_sign "target/release/bundle/macos/K2.app" "$SIGNING_IDENTITY" "$DAEMON_ENTITLEMENTS"
# Nested K2 Screen.app (remote desktop): hardened runtime, no entitlements.
k2_screen_helper_sign "target/release/bundle/macos/K2.app" "$SIGNING_IDENTITY"
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-menubar"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-power-helper"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-node"
# frpc tunnel sidecar (Tauri externalBin → Contents/MacOS/frpc). Re-sign
# with hardened runtime so the binary the app stages to ~/.k2/bin/frpc
# is notarization-covered and runs without a Gatekeeper quarantine block.
FRPC_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/frpc"
if [ -x "$FRPC_BIN" ]; then
    codesign --force --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" \
        "$FRPC_BIN"
    echo "  Signed frpc sidecar."
else
    echo "  WARNING: frpc sidecar not found at $FRPC_BIN — K2 Connect host" >&2
    echo "  setup will require a manual frpc install. Did fetch-frpc.sh run?" >&2
fi
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app"
echo "  Signed (main + K2 Daemon.app + k2-ptyd + k2-export + K2 Screen.app + frpc + bundle) with entitlements."
k2_daemon_helper_verify "target/release/bundle/macos/K2.app"
k2_daemon_helper_verify_signed "target/release/bundle/macos/K2.app"
# K2 Screen: identifier, runtime, team, no forbidden entitlement, and the
# AMFI exec check (it matters most once a provisioning profile rides in it).
k2_screen_helper_verify "target/release/bundle/macos/K2.app"
k2_screen_helper_verify_signed "target/release/bundle/macos/K2.app"

# ── Step 3.5: Launch smoke-test — catch AMFI exec rejections (0.40.6 regression) ──
# A signed Developer-ID app carrying a RESTRICTED entitlement (e.g.
# keychain-access-groups) with NO embedded provisioning profile is SIGKILL'd by
# AMFI at exec — it notarizes fine but users get the Finder "K2 can't be opened"
# dialog. `tauri dev` never enforces this, so it slips past dev testing. Exec the
# freshly-signed binary here and FAIL the release if AMFI kills it (rc 137),
# BEFORE wasting a notarization round-trip or publishing an un-launchable build.
echo ""
echo "Step 3.5: Launch smoke-test (AMFI exec check)..."
# `k2 --version` prints and exits before the app starts (0.46.6), so this
# check never runs the daemon version check from an uninstalled bundle
# (that used to kickstart the running daemon). Exit 0 = launchable.
SMOKE_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/k2"
"$SMOKE_BIN" --version >/tmp/k2-smoke.out 2>&1 &
SMOKE_PID=$!
sleep 2
if kill -0 "$SMOKE_PID" 2>/dev/null; then
    # Alive past the AMFI exec check = launchable. A GUI app ignores SIGTERM and
    # never exits on its own, so SIGKILL it + its children and do NOT `wait` —
    # waiting on a SIGTERM-ignoring GUI hangs the release until the harness
    # timeout (the 0.40.6 re-release failure). kill -9 is uncatchable.
    # disown first so bash does not print "<pid> Killed: 9" for our own kill.
    disown "$SMOKE_PID" 2>/dev/null || true
    pkill -9 -P "$SMOKE_PID" 2>/dev/null || true
    kill -9 "$SMOKE_PID" 2>/dev/null || true
    echo "  ✓ App survived exec (not AMFI-killed) — launchable. Smoke check stopped (expected)."
else
    SMOKE_RC=0; wait "$SMOKE_PID" 2>/dev/null || SMOKE_RC=$?
    if [ "$SMOKE_RC" -eq 137 ]; then
        echo "  FATAL: signed app was SIGKILL'd at launch (137 = AMFI) — almost certainly a" >&2
        echo "  restricted entitlement without a provisioning profile. NOT releasing." >&2
        echo "  Check src-tauri/entitlements.plist. smoke output:" >&2
        head -c 400 /tmp/k2-smoke.out >&2; echo "" >&2
        exit 1
    fi
    echo "  ✓ App exec exited rc=$SMOKE_RC (not SIGKILL) — launchable past AMFI."
fi

# ── Step 3.6: Start what can run during the notarization waits (0.46.13) ──
# Steps 4–7.5 are mostly waiting on Apple (three notarizations, 3–5 min
# each) with the CPU idle, so:
#  - Step 8.5's standalone daemon (k2-daemon recompiled with the k2-ptyd and
#    k2-export Step 3 signed embedded, then its fat-LTO link: 15–18 min)
#    builds in the background from here, from copies of those signed files.
#    Step 8.5 waits for it and checks the copies are byte for byte the ones
#    in the shipped (stapled) app. Polled after Steps 4 and 7, so a failed
#    build stops the release early. K2_STANDALONE_DAEMON_EARLY=0 builds it
#    at Step 8.5 in the foreground as before.
#  - k2-node's own notarization (Step 8.55) is submitted at Step 4 beside
#    the app's (below) and collected at Step 8.55.
# Nothing here signs or publishes anything; the order of every signing,
# notarizing and approval step is unchanged.
STANDALONE_STAGE="$PROJECT_DIR/target/release/standalone-daemon"
if [ "${K2_STANDALONE_DAEMON_EARLY:-1}" = "1" ]; then
    echo ""
    echo "Step 3.6: Standalone daemon build in the background..."
    k2_standalone_macos_daemon_start "$PROJECT_DIR/target/release/bundle/macos/K2.app" "$STANDALONE_STAGE" "$PROJECT_DIR" || exit 1
fi

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
# k2-node (Step 8.55's standalone asset) goes in first, without waiting: it
# is the binary Step 3 signed inside K2.app, checked the way Step 8.55
# checks it, from a copy Step 8.55 compares byte for byte before it uses
# the result. K2_SKIP_NODE_NOTARIZE=1 skips it, as before.
NODE_MAC_ASSET="k2-node-macos-aarch64"
NODE_NOTARY_ID=""
NODE_NOTARY_COPY="$PROJECT_DIR/target/release/node-notary/$NODE_MAC_ASSET"
if [ "${K2_SKIP_NODE_NOTARIZE:-0}" != "1" ]; then
    rm -rf "$(dirname "$NODE_NOTARY_COPY")"
    mkdir -p "$(dirname "$NODE_NOTARY_COPY")"
    cp "$PROJECT_DIR/target/release/bundle/macos/K2.app/Contents/MacOS/k2-node" "$NODE_NOTARY_COPY"
    chmod 0755 "$NODE_NOTARY_COPY"
    codesign --verify --strict \
        -R '=anchor apple generic and certificate leaf[subject.OU] = "36B8R93HXV"' \
        "$NODE_NOTARY_COPY" \
        || { echo "  FATAL: k2-node in K2.app is not signed with K2's Developer ID" >&2; exit 1; }
    bash "$PROJECT_DIR/scripts/sign-k2-node.sh" --check "$NODE_NOTARY_COPY" macos aarch64 "$VERSION" >/dev/null
    NODE_ZIP="/tmp/k2-node-macos-aarch64_${VERSION}.zip"
    rm -f "$NODE_ZIP"
    ditto -c -k "$NODE_NOTARY_COPY" "$NODE_ZIP"
    NODE_NOTARY_ID="$(k2_notary_submit_nowait "$NODE_ZIP")" || exit 1
    rm -f "$NODE_ZIP"
    echo "  k2-node submitted for notarization (${NODE_NOTARY_ID}); collected at Step 8.55."
fi
cd target/release/bundle/macos
ditto -c -k --keepParent "K2.app" "/tmp/K2_${VERSION}.zip"
NOTARY_AUTH=()
while IFS= read -r arg; do NOTARY_AUTH+=("$arg"); done < <(notary_auth_args)
xcrun notarytool submit "/tmp/K2_${VERSION}.zip" \
    "${NOTARY_AUTH[@]}" --wait
staple_with_retry "K2.app"
echo "  App notarized and stapled."
# k2-ptyd (P2): the notarized app still ships the signed nested PTY host.
k2_daemon_helper_verify_shipped_ptyd "K2.app"
# The export worker (vs-live §3.1): signed and nested the same way.
k2_daemon_helper_verify_shipped_export "K2.app"
# K2 Screen (remote desktop): signed, nested and notarized with the app.
k2_screen_helper_verify_shipped "K2.app"
# The background standalone daemon build (Step 3.6): stop now if it failed.
k2_standalone_macos_daemon_poll "$STANDALONE_STAGE" || exit 1

# ── Step 5: Create update bundle (tar.gz) from notarized app + sign it ──
echo ""
echo "Step 5: Creating and signing update bundle..."
cd "$PROJECT_DIR"
COPYFILE_DISABLE=1 tar -czf "target/release/bundle/macos/K2.app.tar.gz" \
    -C "target/release/bundle/macos" "K2.app"
# k2-ptyd (P59): the desktop update installs this bundle; it must carry the
# PTY host, or agents would stop surviving after the update.
k2_daemon_helper_verify_tarball_ptyd "target/release/bundle/macos/K2.app.tar.gz"
k2_daemon_helper_verify_tarball_export "target/release/bundle/macos/K2.app.tar.gz"
k2_screen_helper_verify_tarball "target/release/bundle/macos/K2.app.tar.gz"

# Sign the update bundle with the update key. The Tauri app updater reads
# the signature base64-wrapped (b64), exactly as the tauri signer wrote it.
k2u_sign u1 "target/release/bundle/macos/K2.app.tar.gz" K2.app.tar.gz "$VERSION" \
    "target/release/bundle/macos/K2.app.tar.gz.sig" b64
# U2 (SS5): the only signature 0.46.6+ apps accept; goes into latest.json
# `k2_signatures` at Step 8.
k2u_sign u2 "target/release/bundle/macos/K2.app.tar.gz" K2.app.tar.gz "$VERSION" \
    "target/release/bundle/macos/K2.app.tar.gz.sig2" raw
echo "  Update bundle signed (U1 + U2)."

# ── Step 6: Create DMG from notarized app ──
echo ""
echo "Step 6: Creating DMG..."
rm -f "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
# Stage the notarized app NEXT TO an /Applications symlink so the mounted
# DMG shows the standard "drag K2 → Applications" drop target. A bare
# `-srcfolder K2.app` packs only the app, so users see no Applications
# folder to drag into (the symlink tauri's own DMG ships gets dropped when
# we recreate the DMG here from the stapled app). `ditto` preserves the
# app's signature + stapled notarization ticket.
DMG_STAGE="$(mktemp -d)"
ditto "target/release/bundle/macos/K2.app" "$DMG_STAGE/K2.app"
# Styled DMG (background art + icon layout from tauri.conf.json) via tauri's
# own vendored create-dmg script, which `tauri build` leaves behind. Plain
# hdiutil ships a bare white window — the styling lives in the volume's
# .DS_Store, which only this script writes. Values mirror tauri.conf.json
# bundle.macOS.dmg (window 660x400, app 180,170, drop-link 480,170).
# Fallback: bare hdiutil + symlink (script missing / headless session where
# the Finder AppleScript pass can't run).
BUNDLE_DMG="target/release/bundle/dmg/bundle_dmg.sh"
DMG_OUT="target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
if [ -x "$BUNDLE_DMG" ] && bash "$BUNDLE_DMG" \
    --volname "K2" \
    --background "src-tauri/icons/dmg-background.png" \
    --window-size 660 400 \
    --icon-size 100 \
    --icon "K2.app" 180 170 \
    --app-drop-link 480 170 \
    --hide-extension "K2.app" \
    "$DMG_OUT" "$DMG_STAGE"; then
    echo "  Styled DMG created (background + layout)."
else
    echo "  WARN: styled DMG failed or bundle_dmg.sh missing — falling back to plain hdiutil."
    ln -s /Applications "$DMG_STAGE/Applications"
    hdiutil create -volname "K2" \
        -srcfolder "$DMG_STAGE" \
        -ov -format UDZO \
        "$DMG_OUT"
fi
rm -rf "$DMG_STAGE"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"

# ── Step 7: Notarize DMG ──
echo ""
echo "Step 7: Notarizing DMG..."
xcrun notarytool submit "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" \
    "${NOTARY_AUTH[@]}" --wait
staple_with_retry "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."
k2_standalone_macos_daemon_poll "$STANDALONE_STAGE" || exit 1
fi # SKIP_BUILD

# ── Step 7.5: Clean-VM new-user pairing smoke (0.40.33/34 regression class) ──
# Fresh Sequoia guest, empty ~/.k2*, install this DMG, start bundled daemon,
# assert ~/.k2so → ~/.k2 symlink + /boot-status via the thin-client path.
# Hard gate before latest.json / GitHub publish. Escape: K2_SKIP_VM_PAIRING_SMOKE=1
# (loud; do not skip on the machine that cuts real releases).
if [ "$SKIP_SMOKE" = 0 ]; then
echo ""
echo "Step 7.5: Clean-VM new-user pairing smoke..."
# The VM takes up to 8 GB and the standalone daemon's build (Step 3.6) can
# take 8 GB on its own (k2-daemon's compile and fat-LTO link), on a 16 GB
# Mac: let the build finish first rather than swap under the smoke's
# timeouts. By now (after three notarizations' worth of waiting) little of
# it is usually left. K2_STANDALONE_OVERLAP_VM=1 runs them together.
if [ "${K2_STANDALONE_OVERLAP_VM:-0}" != "1" ] && [ -n "$K2_STANDALONE_PID" ]; then
    echo "  letting the background standalone daemon build finish before the VM starts..."
    k2_standalone_macos_daemon_wait "$STANDALONE_STAGE" || exit 1
fi
"$PROJECT_DIR/scripts/vm-newuser-pair-smoke.sh" \
  "$PROJECT_DIR/target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" \
  "$VERSION"
fi # SKIP_SMOKE

# ── Step 7.9c: Collect the shadow compute builds ──
k2_shadow_collect

# ── Step 8: Generate latest.json ──
echo ""
echo "Step 8: Generating latest.json..."
# Dual-signing is fail closed both ways: without the U1 (bridge) signature
# every install on 0.46.5 or older is stranded, so a missing or bad one is
# fatal here, never an empty "signature" field.
SIG_FILE="target/release/bundle/macos/K2.app.tar.gz.sig"
[ -s "$SIG_FILE" ] || { echo "FATAL: $SIG_FILE missing or empty (Step 5 signs the bundle with U1)" >&2; exit 1; }
k2u_verify_u1 "target/release/bundle/macos/K2.app.tar.gz" "$SIG_FILE" K2.app.tar.gz \
    || { echo "FATAL: the bundle's U1 signature does not verify; installs on 0.46.5 or older could not update" >&2; exit 1; }
SIG_CONTENT=$(cat "$SIG_FILE")

# U2 (SS4/SS5): `k2_signatures` = {"<U2 key id>": "<base64 of the minisig>"},
# the only signature 0.46.6+ apps read. Checked here the way they check it.
APP_SIG2_FILE="target/release/bundle/macos/K2.app.tar.gz.sig2"
[ -f "$APP_SIG2_FILE" ] || { echo "FATAL: $APP_SIG2_FILE missing (Step 5 signs the bundle with U2)" >&2; exit 1; }
k2u_verify_u2 "target/release/bundle/macos/K2.app.tar.gz" "$APP_SIG2_FILE" K2.app.tar.gz "$VERSION" \
    || { echo "FATAL: the bundle's U2 signature does not verify; no 0.46.6+ app could update" >&2; exit 1; }
U2_KEY_ID="$(k2u_slot_key_id u2)"
APP_SIG2_B64="$(k2u_b64 "$APP_SIG2_FILE")"

PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
# App updater manifest. Initially macOS only — Windows `windows-x86_64` is
# merged in Step 9.5 after NSIS build + minisign (see windows-nsis-build /
# patch). Publishing darwin-only is fine for Mac; Windows "Check for
# updates" fails with "None of the fallback platforms windows-x86_64 were
# found" until that merge (0.40.95 incident).
cat > "/tmp/latest.json" <<MANIFEST
{
  "version": "${VERSION}",
  "notes": "K2 ${TAG}",
  "pub_date": "${PUB_DATE}",
  "platforms": {
    "darwin-aarch64": {
      "signature": "${SIG_CONTENT}",
      "url": "https://github.com/${RELEASE_REPO}/releases/download/${TAG}/K2.app.tar.gz",
      "k2_signatures": { "${U2_KEY_ID}": "${APP_SIG2_B64}" }
    }
  }
}
MANIFEST
echo "  latest.json generated (darwin-aarch64; windows-x86_64 added after NSIS if present)."

# ── Step 8.5: Standalone per-OS daemon binary + signature + manifest ──
#
# Remote-update P1: publish a STANDALONE k2-daemon binary (no .app
# wrapper) so headless servers and the daemon self-update path (P3) can
# fetch + verify + install the daemon on its own.
#
# This step produces the NATIVE macos-aarch64 standalone daemon, signs it
# with the SAME minisign key the Tauri updater uses (so P3 verifies it
# against `plugins.updater.pubkey` in tauri.conf.json), computes sha256,
# and emits `daemon-latest.json` (schema below).
#
# Linux binaries are NOT cross-built here — clang/ggml cross-compilation on
# macOS is the wrong tool. `.github/workflows/daemon-binaries.yml` builds
# the linux-x86_64 / linux-aarch64 daemons UNSIGNED after the tag, and
# Step 10.5 (scripts/sign-daemon.sh) signs them here and merges them into
# daemon-latest.json. Until then the manifest is macos-only (P2/P3 treat a
# missing artifact key as "no build for that platform yet"). A
# `--features airgap` daemon is custom/enterprise only (no GitHub update
# ping) and is never a GitHub Release asset.
#
# daemon-latest.json schema (P2/P3 consume this):
#   {
#     "version":  "<x.y.z>",
#     "pub_date": "<iso8601 UTC>",
#     "artifacts": {
#       "<platform-key>": {
#         "url":    "https://github.com/.../releases/download/v<ver>/<asset>",
#         "sig":    "<URL to the .sig asset>",   (U1, for installs < 0.46.6)
#         "sha256": "<hex>",
#         "sig2":   "<URL to the .sig2 asset>"   (U2; 0.46.6+ read only this)
#       }, ...
#     },
#     "notes": "<this version's WHATS_NEW.md section, markdown>"   (0.46.3)
#   }
# platform-key ∈ { macos-aarch64, linux-x86_64, linux-aarch64 }.
echo ""
echo "Step 8.5: Building + signing standalone daemon + daemon-latest.json..."

DIST_DIR="$PROJECT_DIR/target/release/daemon-dist"
mkdir -p "$DIST_DIR"

# The native standalone daemon: the Step 2 daemon rebuilt with the k2-ptyd
# that Step 3 signed inside K2.app embedded (prd-k2-ptyd-v1 P84, as the
# Linux daemons do), because the standalone install and the daemon
# self-update fetch only `k2-daemon`. The app's own nested daemon was
# already copied and signed and is NOT changed by this rebuild.
# It is not a relink: build.rs reruns on the embed, the whole k2-daemon
# crate recompiles and fat LTO re-optimizes the program (15–18 min on
# z3mbpZ). Since 0.46.13 it builds in the background from Step 3.6, during
# the notarization waits; here it is collected and checked against the
# shipped app. A resumed run (K2_RELEASE_RESUME, no Step 3.6) or
# K2_STANDALONE_DAEMON_EARLY=0 builds it here in the foreground.
MAC_ASSET="k2-daemon-macos-aarch64"
if [ -n "$K2_STANDALONE_PID" ] || [ -n "$K2_STANDALONE_RC" ]; then
k2_standalone_macos_daemon_finish "$PROJECT_DIR/target/release/bundle/macos/K2.app" "$STANDALONE_STAGE" "$DIST_DIR/$MAC_ASSET" || exit 1
else
k2_build_standalone_macos_daemon "$PROJECT_DIR/target/release/bundle/macos/K2.app" "$DIST_DIR/$MAC_ASSET" "$PROJECT_DIR"
fi

# Sign with the update key. The daemon self-updater and plain minisign
# read the RAW minisig text (Shape B e2e finding, 2026-07-06; the app
# bundle's .sig stays base64-wrapped for the Tauri updater).
k2u_sign u1 "$DIST_DIR/$MAC_ASSET" "$MAC_ASSET" "$VERSION" "$DIST_DIR/$MAC_ASSET.sig" raw
# U2 (SS5): `.sig2`, the only signature 0.46.6+ daemons and installers accept.
k2u_sign u2 "$DIST_DIR/$MAC_ASSET" "$MAC_ASSET" "$VERSION" "$DIST_DIR/$MAC_ASSET.sig2" raw
MAC_SHA256=$(shasum -a 256 "$DIST_DIR/$MAC_ASSET" | awk '{print $1}')
echo "  macos-aarch64 daemon built, signed, hashed."

DL_BASE="https://github.com/${RELEASE_REPO}/releases/download/${TAG}"

# Compute mode: the verified x86 files (Step 7.9c) join the release here.
# The x86 daemon is signed at Step 10.5 by sign-daemon.sh --from-dir, which
# checks it against the compute verdicts first (SS1: no provenance, no
# signature).
k2_compute_stage

# Build the artifacts object incrementally. macos-aarch64 is always present.
ARTIFACTS_JSON=$(cat <<JSON
    "macos-aarch64": {
      "url": "${DL_BASE}/${MAC_ASSET}",
      "sig": "${DL_BASE}/${MAC_ASSET}.sig",
      "sha256": "${MAC_SHA256}",
      "sig2": "${DL_BASE}/${MAC_ASSET}.sig2"
    }
JSON
)

# The Linux entries are added at Step 10.5, by sign-daemon.sh after it has
# signed them (SS1); a Linux daemon staged in $DIST_DIR is not merged here.

# 0.46.3: the manifest also carries `notes` (this version's WHATS_NEW.md
# section), so a phone or remote host can show what's new before it
# updates. scripts/daemon-manifest.sh, dry-tested by
# tests/packaging/daemon-manifest.sh.
# shellcheck source=daemon-manifest.sh
source "$PROJECT_DIR/scripts/daemon-manifest.sh"
k2_daemon_manifest "${VERSION}" "${PUB_DATE}" "${ARTIFACTS_JSON}" "$PROJECT_DIR/WHATS_NEW.md" \
    > "$DIST_DIR/daemon-latest.json"
grep -q '"notes"' "$DIST_DIR/daemon-latest.json" \
    || { echo "ERROR: daemon-latest.json has no notes (WHATS_NEW.md '## ${VERSION}' section?)" >&2; exit 1; }
echo "  daemon-latest.json generated at $DIST_DIR/daemon-latest.json (with notes)"

# ── Step 8.55: Standalone k2-node for macOS (prd-k2-node-admin-v1 §8.2) ──
# A Mac compute node runs /Library/Application Support/K2/node/k2-node,
# outside K2.app, and self-updates from the release asset
# k2-node-macos-aarch64 (the bare Mach-O; the controller downloads it,
# checks it and sends the bytes to the node). It is the SAME binary that
# Step 3 codesigned inside K2.app (Developer ID + hardened runtime, the
# Team ID the node's executor requires). Step 4 already notarized it as
# part of the app; it is ALSO submitted on its own (a zip holding the
# bare binary; a bare Mach-O can't be stapled, so Gatekeeper finds the
# ticket online by cdhash): at Step 4 beside the app since 0.46.13, and
# collected here (must end Accepted); a resumed run submits it here and
# waits, as before. The minisign .sig is added at Step 10.5 by
# sign-k2-node.sh with the root-helper key. Escape for a resumed or
# offline run: K2_SKIP_NODE_NOTARIZE=1 (the app's notarization still
# covers the same cdhash).
echo ""
echo "Step 8.55: Standalone macOS k2-node asset..."
NODE_MAC_ASSET="k2-node-macos-aarch64"
NODE_MAC_SRC="$PROJECT_DIR/target/release/bundle/macos/K2.app/Contents/MacOS/k2-node"
[ -f "$NODE_MAC_SRC" ] || { echo "  FATAL: $NODE_MAC_SRC missing (Step 2.5 bundles it)" >&2; exit 1; }
cp "$NODE_MAC_SRC" "$DIST_DIR/$NODE_MAC_ASSET"
chmod 0755 "$DIST_DIR/$NODE_MAC_ASSET"
codesign --verify --strict \
    -R '=anchor apple generic and certificate leaf[subject.OU] = "36B8R93HXV"' \
    "$DIST_DIR/$NODE_MAC_ASSET" \
    || { echo "  FATAL: $NODE_MAC_ASSET is not signed with K2's Developer ID" >&2; exit 1; }
# Same byte checks sign-k2-node.sh runs at Step 10.5 (marker = this
# release, not a test-keys build), so a bad build stops here, not after
# the tag is out.
bash "$PROJECT_DIR/scripts/sign-k2-node.sh" --check "$DIST_DIR/$NODE_MAC_ASSET" macos aarch64 "$VERSION" >/dev/null
if [ "${K2_SKIP_NODE_NOTARIZE:-0}" = "1" ]; then
    echo "  WARNING: K2_SKIP_NODE_NOTARIZE=1 — standalone k2-node not submitted on its own (the app's ticket covers it)." >&2
elif [ -n "${NODE_NOTARY_ID:-}" ]; then
    # Submitted at Step 4 (0.46.13): the bytes it notarized must be these.
    cmp -s "$NODE_NOTARY_COPY" "$DIST_DIR/$NODE_MAC_ASSET" \
        || { echo "  FATAL: the k2-node submitted at Step 4 is not the k2-node staged here" >&2; exit 1; }
    k2_notary_wait_accepted "$NODE_NOTARY_ID" "k2-node" || exit 1
    echo "  k2-node notarized (bare Mach-O: no staple; ticket is online)."
else
    NODE_ZIP="/tmp/k2-node-macos-aarch64_${VERSION}.zip"
    rm -f "$NODE_ZIP"
    ditto -c -k "$DIST_DIR/$NODE_MAC_ASSET" "$NODE_ZIP"
    NODE_NOTARY_AUTH=()
    while IFS= read -r arg; do NODE_NOTARY_AUTH+=("$arg"); done < <(notary_auth_args)
    xcrun notarytool submit "$NODE_ZIP" "${NODE_NOTARY_AUTH[@]}" --wait
    rm -f "$NODE_ZIP"
    echo "  k2-node notarized (bare Mach-O: no staple; ticket is online)."
fi
( cd "$DIST_DIR" && shasum -a 256 "$NODE_MAC_ASSET" > "$NODE_MAC_ASSET.sha256" )
echo "  $NODE_MAC_ASSET staged (+ .sha256); minisign .sig comes at Step 10.5."

# ── Step 8.6: Publish hosted-web SPA to Cloudflare R2 ──
#
# Additive, versioned layout: app/<ver>/index.html + content-hashed assets.
# Separate artifact from the DMG — desktop release must not die on R2 TLS
# warmup (brand-new endpoint) or missing laptop creds. Fail only when
# K2_REQUIRE_WEB_PUBLISH=1. See scripts/publish-web-bundles.sh header.
echo ""
echo "Step 8.6: Publishing web SPA bundle to R2 (app/${VERSION}/)..."
WEB_PUBLISH_ARGS=("$VERSION" --skip-if-no-creds)
if [ "${K2_WEB_BUNDLE_PRUNE:-0}" = "1" ]; then
    WEB_PUBLISH_ARGS+=(--prune)
fi
set +e
"$PROJECT_DIR/scripts/publish-web-bundles.sh" "${WEB_PUBLISH_ARGS[@]}"
WEB_PUBLISH_RC=$?
set -e
if [ "$WEB_PUBLISH_RC" -eq 0 ]; then
    echo "  Web-bundle publish step finished (see messages above)."
else
    if [ "${K2_REQUIRE_WEB_PUBLISH:-0}" = "1" ]; then
        echo "  FATAL: web-bundle publish failed (rc=$WEB_PUBLISH_RC) and K2_REQUIRE_WEB_PUBLISH=1." >&2
        exit 1
    fi
    echo "  WARNING: web-bundle publish failed (rc=$WEB_PUBLISH_RC) — continuing desktop release." >&2
    echo "  WARNING: re-run: bash scripts/publish-web-bundles.sh ${VERSION}" >&2
    echo "  WARNING: set K2_REQUIRE_WEB_PUBLISH=1 to make this a hard gate." >&2
fi

# ── Step 9: Create GitHub Release ──
echo ""
echo "Step 9: Creating GitHub release ${TAG}..."
ASSETS=(
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
    "target/release/bundle/macos/K2.app.tar.gz"
)
[ -f "$SIG_FILE" ] && ASSETS+=("$SIG_FILE")
ASSETS+=("/tmp/latest.json")

# Standalone daemon assets (remote-update P1): the native macos-aarch64
# binary + its .sig, plus the daemon manifest. The Linux daemons come from
# daemon-binaries.yml (unsigned) and get their .sig at Step 10.5.
ASSETS+=(
    "$DIST_DIR/${MAC_ASSET}"
    "$DIST_DIR/${MAC_ASSET}.sig"
    "$DIST_DIR/${MAC_ASSET}.sig2"
    "$DIST_DIR/daemon-latest.json"
)
# Compute mode: every staged file (node, root helper and engine with their
# .sha256, x86_64 and, unless it fell back, aarch64; deb/rpm/AppImage,
# SHA256SUMS-linux-x86_64.txt) but the daemons: sign-daemon.sh uploads
# each daemon at Step 10.5, after it has checked and signed it. The root-helper-key .sig files come at Step 10.5 as
# for GitHub's uploads; the Arch package's SUMS line is merged in by
# arch-package.yml after the tag.
if [ "$K2_LINUX_BUILDS" = "compute" ]; then
    while IFS= read -r f; do
        case "$f" in
            ""|*/*|k2-daemon-linux-x86_64|k2-daemon-linux-x86_64.sha256|k2-daemon-linux-aarch64|k2-daemon-linux-aarch64.sha256) continue ;;
        esac
        [ -s "$DIST_DIR/$f" ] || { echo "ERROR: staged compute file $f is missing" >&2; exit 1; }
        ASSETS+=("$DIST_DIR/$f")
    done < "$DIST_DIR/.compute-staged"
    echo "  Assets include the K2 Compute x86 set ($(grep -c . "$DIST_DIR/.compute-staged") files; the daemon goes up at Step 10.5)."
fi
# Standalone macOS k2-node (Step 8.55): bare binary + .sha256. Its .sig
# (root-helper key) is uploaded at Step 10.5.
ASSETS+=(
    "$DIST_DIR/${NODE_MAC_ASSET}"
    "$DIST_DIR/${NODE_MAC_ASSET}.sha256"
)

# Arch/Omarchy pacman package. The k2-arch runner builds this after the
# tag and uploads it (arch-package.yml). If a file is already in DIST_DIR,
# attach it here. A missing file does not stop the release. No .sig.
# Its SHA256SUMS-linux-x86_64.txt line comes from arch-package.yml's `sums`
# job either way (a hand-attached file here gets none); Step 10.6 checks it.
ARCH_PKG_NAME="k2-${VERSION}-x86_64.pkg.tar.zst"
if [ -s "$DIST_DIR/$ARCH_PKG_NAME" ]; then
    ASSETS+=("$DIST_DIR/$ARCH_PKG_NAME")
    echo "  Asset: ${ARCH_PKG_NAME}"
else
    echo "  WARNING: ${ARCH_PKG_NAME} not in ${DIST_DIR}." >&2
    echo "  WARNING: arch-package.yml on the k2-arch runner uploads it after the tag." >&2
fi

if [ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ]; then
    NOTES_SRC="$NOTES_FILE"
else
    # No explicit notes file → auto-extract this version's WHATS_NEW.md
    # section (the same `## <version>` block Step 1.5 already verified
    # exists) and use it as the GitHub release body. The public
    # "What's New" site mirrors the GH release body, so leaving a
    # placeholder here silently empties the site — this makes the notes
    # impossible to forget.
    NOTES_SRC="$(mktemp -t k2-relnotes)"
    awk -v ver="$VERSION" '
        $0 ~ "^## " ver " " { inblock = 1; print; next }
        inblock && /^## / { exit }
        inblock { print }
    ' WHATS_NEW.md > "$NOTES_SRC"
    echo "  Using WHATS_NEW.md '## ${VERSION}' section as the release body."
fi
# ── Step 8.9: Commit + push the version bump, then create + push the tag ──
# Historically release.sh bumped the version files only to BUILD the installers
# and never committed them, so `main` lagged at the prior version after every
# release and `gh release create` below tagged origin's stale HEAD (the tag
# pointed at the wrong commit). Commit the bump, push main, and create + push
# the annotated tag HERE — so the GitHub release attaches to the REAL release
# commit and the repo's committed version always matches the published release.
# Runs only after a fully built + notarized release, so a failed build never
# leaves a stray commit/tag. A push failure (e.g. diverged origin) aborts
# BEFORE publishing — safer than the old after-the-fact state.
echo ""
echo "Step 8.9: Committing version bump + pushing tag ${TAG}..."
git add -- "${RELEASE_COMMIT_FILES[@]}"
if git diff --cached --quiet; then
    echo "  (version files already committed — nothing new to commit)"
else
    git commit -m "chore(release): ${TAG}"
    echo "  Committed version bump."
fi
# HEAD:main, not bare HEAD — release worktrees run DETACHED (worktrees
# can't check out a branch already held by the main checkout), and a
# bare `git push origin HEAD` fails there ("not a full refname") AFTER
# the build/sign/notarize already succeeded (v0.40.39 hit this; the
# tail had to be run by hand).
k2_shadow_tree_check
git push origin HEAD:main
git tag -fa "$TAG" -m "K2 ${TAG}"
git push origin "refs/tags/${TAG}" --force
echo "  Pushed main + tag ${TAG} at $(git rev-parse --short HEAD)."

# ── Step 9: Create the GitHub release (attaches to the tag pushed above) ──
gh release create "$TAG" "${ASSETS[@]}" \
    --repo "$RELEASE_REPO" \
    --title "$TAG" \
    --notes-file "$NOTES_SRC"

# ── Step 9.5: Windows NSIS (Alpha) on K2-WIN-AX41 ──
#
# scripts/windows-nsis-build.sh sends the tag to the box as a git bundle,
# builds there, and copies the unsigned installer back to dist-windows/.
# It is minisigned HERE for the updater (windows-patch-latest-json.sh);
# no signing key ever goes to the box.
#
# OFF by default until Windows is switched on for releases: K2_SKIP_WINDOWS_NSIS
# defaults to 1 (set K2_SKIP_WINDOWS_NSIS=0 to build it). When on, a failed
# build soft-fails unless K2_REQUIRE_WINDOWS_NSIS=1 (which also turns it on).
if [ "${K2_REQUIRE_WINDOWS_NSIS:-0}" = "1" ]; then
    export K2_SKIP_WINDOWS_NSIS="${K2_SKIP_WINDOWS_NSIS:-0}"
else
    export K2_SKIP_WINDOWS_NSIS="${K2_SKIP_WINDOWS_NSIS:-1}"
fi
echo ""
echo "Step 9.5: Windows NSIS installer, Alpha (${K2_WINDOWS_SSH_HOST:-k2-win-ax41})..."
WIN_PATCH_FAILED=""
set +e
"$PROJECT_DIR/scripts/windows-nsis-build.sh" "$VERSION"
WIN_NSIS_RC=$?
set -e
if [ "$WIN_NSIS_RC" -eq 0 ] && [ "${K2_SKIP_WINDOWS_NSIS}" != "1" ]; then
    WIN_SETUP="$PROJECT_DIR/dist-windows/K2_${VERSION}_x64-setup.exe"
    WIN_SUMS="$PROJECT_DIR/dist-windows/SHA256SUMS-windows-x86_64.txt"
    if [ -f "$WIN_SETUP" ]; then
        # `file#label`: the release page shows the label, the download
        # keeps the file name.
        gh release upload "$TAG" "${WIN_SETUP}#Windows installer (Alpha, x64)" \
            ${WIN_SUMS:+"$WIN_SUMS"} \
            --repo "$RELEASE_REPO" --clobber
        echo "  Uploaded Windows NSIS (Alpha) to ${TAG}."
        # Say it in the notes too: Windows is Alpha and the installer is
        # not Authenticode-signed, so SmartScreen will warn.
        WIN_NOTES="$(mktemp -t k2-relnotes-win)"
        if gh release view "$TAG" --repo "$RELEASE_REPO" --json body --jq .body > "$WIN_NOTES"; then
            if ! grep -q "Windows (Alpha)" "$WIN_NOTES"; then
                printf '\n\n### Windows (Alpha)\n\n`K2_%s_x64-setup.exe` is an Alpha build for Windows 10/11 (x64). It is not code-signed yet, so Windows SmartScreen will ask before it runs.\n' "$VERSION" >> "$WIN_NOTES"
                gh release edit "$TAG" --repo "$RELEASE_REPO" --notes-file "$WIN_NOTES" \
                    || echo "  WARNING: could not add the Windows (Alpha) note to ${TAG}'s release notes." >&2
            fi
        else
            echo "  WARNING: could not read ${TAG}'s release notes to add the Windows (Alpha) note." >&2
        fi
        rm -f "$WIN_NOTES"
        # Sign setup for Tauri auto-updater and merge windows-x86_64 into
        # latest.json (otherwise Windows Check for Updates fails: no
        # platforms.windows-x86_64 — 0.40.95). The script signs through
        # scripts/lib/sign-updater.sh, like every other signing step.
        if bash "$PROJECT_DIR/scripts/windows-patch-latest-json.sh" "$VERSION" "$WIN_SETUP"; then
            echo "  Patched + re-uploaded latest.json with windows-x86_64."
        else
            # Loud, and the release ends with exit 1 (Steps 10-10.5 still run:
            # the Linux daemons must still be signed). Windows installs can't
            # update until this is fixed.
            WIN_PATCH_FAILED=1
            printf '\033[1;31m  ✗ FATAL: the setup is on %s but latest.json has no signed windows-x86_64 entry: Windows installs cannot update.\033[0m\n' "$TAG" >&2
            printf '\033[1;31m  ✗ Re-run: bash scripts/windows-patch-latest-json.sh %s %s\033[0m\n' "$VERSION" "$WIN_SETUP" >&2
        fi
    else
        echo "  WARNING: windows-nsis-build reported OK but ${WIN_SETUP} missing." >&2
        WIN_NSIS_RC=1
    fi
fi
if [ "$WIN_NSIS_RC" -ne 0 ] && [ "${K2_SKIP_WINDOWS_NSIS}" != "1" ]; then
    if [ "${K2_REQUIRE_WINDOWS_NSIS:-0}" = "1" ]; then
        echo "  FATAL: Windows NSIS failed (rc=$WIN_NSIS_RC) and K2_REQUIRE_WINDOWS_NSIS=1." >&2
        exit 1
    fi
    echo "  WARNING: Windows NSIS failed or skipped upload (rc=$WIN_NSIS_RC) — macOS/Linux release still live." >&2
    echo "  WARNING: re-run: bash scripts/windows-nsis-build.sh ${VERSION} && gh release upload ${TAG} dist-windows/* --clobber" >&2
    echo "  WARNING: set K2_REQUIRE_WINDOWS_NSIS=1 to make this a hard gate." >&2
fi

# ── Step 10: Verify the updater can actually FETCH valid JSON ──
# We GENERATE latest.json + daemon-latest.json above, but a release is only
# good if the updater's real endpoints serve valid JSON. The app updater +
# the daemon self-update both fetch via GitHub's
# `/releases/latest/download/<name>` alias — which can lag for a few seconds
# after publish, and the asset CDN occasionally 504s. So retry, then FAIL
# LOUDLY (the release is already live) rather than let a broken updater pass
# silently. Validates HTTP body actually contains `"version": "<VERSION>"`
# (a 504 HTML page or a stale manifest both fail this).
echo ""
echo "Step 10: Verifying updater endpoints serve valid v${VERSION} JSON..."
VERIFY_BASE="https://github.com/${RELEASE_REPO}/releases/latest/download"
VERIFY_FAILED=""
verify_manifest() {
    local label="$1" url="$2" ok="" body=""
    for attempt in $(seq 1 8); do
        body="$(curl -sL --max-time 20 "$url" 2>/dev/null)"
        if printf '%s' "$body" | grep -q "\"version\"[[:space:]]*:[[:space:]]*\"${VERSION}\""; then
            ok=1; break
        fi
        k2_wait_tick "    ${label}: not ready yet, retrying every 15s (up to 8 tries)"
        sleep 15
    done
    k2_wait_done
    if [ -n "$ok" ]; then
        echo "  ✓ ${label} → valid JSON, version ${VERSION}"
    else
        echo "  ✗ ${label} did NOT serve valid v${VERSION} JSON: ${url}" >&2
        echo "     (release IS published, but the in-app/daemon updater will fail until this clears —" >&2
        echo "      usually a transient GitHub 504 / latest-alias lag; re-run the check or wait a minute)" >&2
        VERIFY_FAILED=1
    fi
}
verify_manifest "app latest.json"    "${VERIFY_BASE}/latest.json"
verify_manifest "daemon-latest.json" "${VERIFY_BASE}/daemon-latest.json"

# SS4/SS5: the published manifests carry U2 signatures that verify with the
# SOURCE tree's UPDATER_KEYS, checked the way a 0.46.6+ install checks them.
# Without them no 0.46.6+ install can update, so this is fatal (the
# release is live: fix by re-uploading). Linux daemons: sign-daemon.sh
# checks its own at Step 10.5. Windows: windows-patch-latest-json.sh.
echo ""
echo "Step 10: U2 + U1 signatures in the published manifests (what 0.46.6+ and older installs read)..."
U2_VERIFY_FAILED=""
k2_verify_published_u2() {
    local tagbase="https://github.com/${RELEASE_REPO}/releases/download/${TAG}" d attempt
    d="$(mktemp -d -t k2-u2-verify)"
    for attempt in 1 2 3 4 5 6; do
        if curl -fsSL --max-time 30 "$tagbase/latest.json" -o "$d/latest.json" \
            && curl -fsSL --max-time 30 "$tagbase/daemon-latest.json" -o "$d/daemon-latest.json"; then
            break
        fi
        [ "$attempt" = 6 ] && { echo "  ✗ could not fetch the published manifests" >&2; return 1; }
        sleep 15
    done
    python3 -I - "$d/latest.json" "$tagbase/K2.app.tar.gz" "$U2_KEY_ID" "$VERSION" > "$d/app.sig2" <<'PY' \
        || { echo "  ✗ latest.json darwin-aarch64 has no k2_signatures for U2 ($U2_KEY_ID)" >&2; return 1; }
import base64, json, sys
m = json.load(open(sys.argv[1]))
e = m["platforms"]["darwin-aarch64"]
assert m["version"] == sys.argv[4] and e["url"] == sys.argv[2]
sys.stdout.write(base64.b64decode(e["k2_signatures"][sys.argv[3]]).decode())
PY
    k2u_verify_u2 "target/release/bundle/macos/K2.app.tar.gz" "$d/app.sig2" K2.app.tar.gz "$VERSION" \
        || { echo "  ✗ the published app k2_signatures does not verify" >&2; return 1; }
    python3 -I - "$d/daemon-latest.json" "$tagbase/${MAC_ASSET}.sig2" "$MAC_SHA256" <<'PY' \
        || { echo "  ✗ daemon-latest.json macos-aarch64 has no (or a wrong) sig2" >&2; return 1; }
import json, sys
e = json.load(open(sys.argv[1]))["artifacts"]["macos-aarch64"]
sys.exit(0 if e.get("sig2") == sys.argv[2] and e.get("sha256") == sys.argv[3] else 1)
PY
    curl -fsSL --max-time 30 "$tagbase/${MAC_ASSET}.sig2" -o "$d/mac.sig2" \
        || { echo "  ✗ could not fetch ${MAC_ASSET}.sig2" >&2; return 1; }
    k2u_verify_u2 "$DIST_DIR/$MAC_ASSET" "$d/mac.sig2" "$MAC_ASSET" "$VERSION" \
        || { echo "  ✗ the published ${MAC_ASSET}.sig2 does not verify" >&2; return 1; }
    # The bridge half (installs on 0.46.5 or older read only these).
    python3 -I - "$d/latest.json" > "$d/app.sig" <<'PY' \
        || { echo "  ✗ latest.json darwin-aarch64 has no U1 signature" >&2; return 1; }
import json, sys
s = json.load(open(sys.argv[1]))["platforms"]["darwin-aarch64"]["signature"]
assert s.strip()
sys.stdout.write(s)
PY
    k2u_verify_u1 "target/release/bundle/macos/K2.app.tar.gz" "$d/app.sig" K2.app.tar.gz \
        || { echo "  ✗ the published app signature (U1) does not verify" >&2; return 1; }
    curl -fsSL --max-time 30 "$tagbase/${MAC_ASSET}.sig" -o "$d/mac.sig" \
        || { echo "  ✗ could not fetch ${MAC_ASSET}.sig" >&2; return 1; }
    k2u_verify_u1 "$DIST_DIR/$MAC_ASSET" "$d/mac.sig" "$MAC_ASSET" \
        || { echo "  ✗ the published ${MAC_ASSET}.sig (U1) does not verify" >&2; return 1; }
    rm -rf "$d"
    echo "  ✓ latest.json k2_signatures and ${MAC_ASSET}.sig2 verify with UPDATER_KEYS; the U1 bridge signatures verify too"
}
k2_verify_published_u2 || U2_VERIFY_FAILED=1

# ── Step 10.5: Sign the Linux daemons (prd-security-sweep-0464-v1 SS1) ──
# daemon-binaries.yml uploads k2-daemon-linux-<arch> + .sha256 UNSIGNED;
# this Mac checks and signs them with the update key, uploads the .sig and
# then merges them into daemon-latest.json. Until it does, Linux daemons
# see no update for their platform (harmless). Re-run by hand:
#   bash scripts/sign-daemon.sh ${VERSION}
# Compute mode: the x86 daemon comes from Step 8.5's staged copy, checked
# against the compute verdicts (--from-dir/--compute-verdict), and aarch64
# too unless it fell back to GitHub (then it waits for daemon-binaries.yml).
# Escape (Linux daemons stay on the previous version): K2_SKIP_DAEMON_SIGN=1.
DAEMON_SIGN_FAILED=""
DAEMON_SIGN_ARGS=("${VERSION}")
if [ "$K2_LINUX_BUILDS" = "compute" ]; then
    DAEMON_SIGN_ARGS+=(--from-dir "$DIST_DIR" --compute-verdict "$K2_SHADOW_STATE")
fi
if [ "${K2_SKIP_DAEMON_SIGN:-0}" = "1" ]; then
    echo ""
    echo "  ⚠⚠ LINUX DAEMON SIGNING SKIPPED (K2_SKIP_DAEMON_SIGN=1) ⚠⚠" >&2
    echo "  Linux daemons can't update to ${VERSION} until: bash scripts/sign-daemon.sh ${DAEMON_SIGN_ARGS[*]}" >&2
else
    echo ""
    if [ "$K2_LINUX_BUILDS" = compute ] && [ "$K2_COMPUTE_ARM" = compute ]; then
        echo "Step 10.5: Signing the Linux k2-daemon builds (from K2 Compute; no GitHub wait)..."
    else
        echo "Step 10.5: Signing the Linux k2-daemon builds (waits for daemon-binaries.yml)..."
    fi
    if ! bash "$PROJECT_DIR/scripts/sign-daemon.sh" "${DAEMON_SIGN_ARGS[@]}"; then
        DAEMON_SIGN_FAILED=1
        echo "  ✗ Linux daemons NOT signed — re-run: bash scripts/sign-daemon.sh ${DAEMON_SIGN_ARGS[*]}" >&2
    fi
fi

# ── Step 10.5: Sign the root helper (prd-k2-root-helper-v1 §9.1) ──
# daemon-binaries.yml uploads k2-root-helper-linux-<arch> + .sha256
# UNSIGNED; this Mac signs them with the root-helper key (never in CI,
# never the updater key) and uploads the .sig. Until it does, daemons'
# auto-heal sees a 404 and backs off — harmless, but the release is not
# complete without it. Re-run by hand: bash scripts/sign-root-helper.sh ${VERSION}
ROOT_HELPER_SIGN_FAILED=""
if [ "${K2_SKIP_ROOT_HELPER_SIGN:-0}" != "1" ]; then
    echo ""
    echo "Step 10.5: Signing k2-root-helper assets (waits for daemon-binaries.yml)..."
    if ! bash "$PROJECT_DIR/scripts/sign-root-helper.sh" "${VERSION}"; then
        ROOT_HELPER_SIGN_FAILED=1
        echo "  ✗ root helper NOT signed — re-run: bash scripts/sign-root-helper.sh ${VERSION}" >&2
    fi
fi
# k2-node (prd-k2-node-admin-v1 §8.1): same key, after the helper. Waits
# for daemon-binaries.yml's two Linux assets; the macOS one went up at Step 9.
K2_NODE_SIGN_FAILED=""
if [ "${K2_SKIP_K2_NODE_SIGN:-0}" != "1" ]; then
    echo ""
    echo "Step 10.5: Signing k2-node assets with the root-helper key..."
    if ! bash "$PROJECT_DIR/scripts/sign-k2-node.sh" "${VERSION}"; then
        K2_NODE_SIGN_FAILED=1
        echo "  ✗ k2-node NOT signed — re-run: bash scripts/sign-k2-node.sh ${VERSION}" >&2
    fi
fi
# k2-app-engine (prd-k2-app-runtime-v1 S11): same key, after k2-node. Waits
# for daemon-binaries.yml's two Linux assets.
APP_ENGINE_SIGN_FAILED=""
if [ "${K2_SKIP_APP_ENGINE_SIGN:-0}" != "1" ]; then
    echo ""
    echo "Step 10.5: Signing k2-app-engine assets with the root-helper key..."
    if ! bash "$PROJECT_DIR/scripts/sign-app-engine.sh" "${VERSION}"; then
        APP_ENGINE_SIGN_FAILED=1
        echo "  ✗ k2-app-engine NOT signed — re-run: bash scripts/sign-app-engine.sh ${VERSION}" >&2
    fi
fi

# ── Step 10.6: Every Linux package has a SHA256SUMS line ──
# app-linux.yml (deb/rpm/AppImage) and arch-package.yml (pkg.tar.zst) each
# merge their lines into SHA256SUMS-linux-x86_64.txt after the tag. The
# desktop updater refuses a Linux package with no line (v0.46.0 shipped
# without the Arch line). Waits for both workflows, then checks each line
# against GitHub's own asset digest. The release is already live here. If
# arch-package.yml fails, this says so at once and stops waiting for that
# package; deb, rpm and AppImage are still checked. Re-run by hand:
#   bash scripts/linux-sums.sh check ${VERSION} --wait-min 60
LINUX_SUMS_FAILED=""
ARCH_PKG_FAILED=""
ARCH_PKG_RUN_URL=""
if [ "${K2_SKIP_LINUX_SUMS_CHECK:-0}" = "1" ]; then
    echo ""
    echo "  ⚠⚠ LINUX SHA256SUMS CHECK SKIPPED (K2_SKIP_LINUX_SUMS_CHECK=1) ⚠⚠"
else
    echo ""
    if [ "$K2_LINUX_BUILDS" = compute ]; then
        # deb/rpm/AppImage and their lines went up at Step 9: one check,
        # no wait. The Arch package follows on k2-arch on its own.
        echo "Step 10.6: Linux packages vs SHA256SUMS-linux-x86_64.txt (compute: checked once; the Arch package is not waited for)..."
        k2_linux_sums_step "${VERSION}" 0 arch-later
    else
        echo "Step 10.6: Linux packages vs SHA256SUMS-linux-x86_64.txt (waits for app-linux + arch-package; a failed Arch build is reported, not waited for)..."
        k2_linux_sums_step "${VERSION}" "${K2_LINUX_SUMS_WAIT_MIN:-120}"
    fi
    if [ -n "$LINUX_SUMS_FAILED" ]; then
        echo "  ✗ Linux SHA256SUMS incomplete or wrong — re-run: bash scripts/linux-sums.sh check ${VERSION} --wait-min 60" >&2
    fi
fi

k2_shadow_report

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Release ${TAG} complete!"
echo "  https://github.com/${RELEASE_REPO}/releases/tag/${TAG}"
[ -n "$VERIFY_FAILED" ] && echo "  ⚠  UPDATER MANIFEST CHECK FAILED — see Step 10 above (updater may 'Could not fetch a valid release JSON' until it clears)"
[ -n "$WIN_PATCH_FAILED" ] && echo "  ⚠  WINDOWS latest.json NOT PATCHED — bash scripts/windows-patch-latest-json.sh ${VERSION} dist-windows/K2_${VERSION}_x64-setup.exe"
[ -n "$U2_VERIFY_FAILED" ] && echo "  ⚠  U2 OR U1 SIGNATURES MISSING OR BAD IN THE PUBLISHED MANIFESTS — some installs can't update (Step 10)"
[ -n "$DAEMON_SIGN_FAILED" ] && echo "  ⚠  LINUX DAEMONS NOT SIGNED — bash scripts/sign-daemon.sh ${DAEMON_SIGN_ARGS[*]}"
[ -n "$ROOT_HELPER_SIGN_FAILED" ] && echo "  ⚠  ROOT HELPER NOT SIGNED — bash scripts/sign-root-helper.sh ${VERSION}"
[ -n "$K2_NODE_SIGN_FAILED" ] && echo "  ⚠  K2-NODE NOT SIGNED — bash scripts/sign-k2-node.sh ${VERSION}"
[ -n "$APP_ENGINE_SIGN_FAILED" ] && echo "  ⚠  K2-APP-ENGINE NOT SIGNED — bash scripts/sign-app-engine.sh ${VERSION}"
[ -n "$LINUX_SUMS_FAILED" ] && echo "  ⚠  LINUX SHA256SUMS INCOMPLETE — bash scripts/linux-sums.sh check ${VERSION} --wait-min 60"
[ -n "$ARCH_PKG_FAILED" ] && echo "  ⚠  ARCH PACKAGE NOT PUBLISHED, THE ARCH BUILD FAILED (the release itself is live; Step 10.6 above has the re-run) — ${ARCH_PKG_RUN_URL}"
[ -n "${ARCH_PKG_PENDING:-}" ] && echo "  …  Arch package still building on k2-arch (not waited for): bash scripts/linux-sums.sh check ${VERSION} --wait-min 60"
[ -n "$K2_COMPUTE_FELL_BACK" ] && echo "  ⚠  K2 COMPUTE FELL BACK TO GITHUB — ${K2_COMPUTE_FELL_BACK}"
echo "═══════════════════════════════════════════════════"
[ -z "$WIN_PATCH_FAILED" ] && [ -z "$U2_VERIFY_FAILED" ] && [ -z "$DAEMON_SIGN_FAILED" ] && [ -z "$ROOT_HELPER_SIGN_FAILED" ] && [ -z "$K2_NODE_SIGN_FAILED" ] && [ -z "$APP_ENGINE_SIGN_FAILED" ] && [ -z "$LINUX_SUMS_FAILED" ] && [ -z "$ARCH_PKG_FAILED" ] || exit 1
