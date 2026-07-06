#!/usr/bin/env bash
# package-dedicated-artifacts.sh — harvest the PROVEN Dedicated-tier sandbox
# stack off the reference box into versioned, sha256-manifested tarballs and
# (optionally) publish them as GitHub release assets.
#
# Runs ON the reference sandbox box (k2-sandbox-01, root@37.27.67.180) as
# root. It is a READ-ONLY harvest of the live system — it never touches the
# running k2-daemon, the systemd units, or any installed file; it only READS
# the artifact paths and WRITES tarballs into its own output directory.
#
# What it packages (PRD prd-k2-cloud-hosted-servers-v1.md §4.1 Dedicated —
# "prebuilt artifacts we host — do NOT compile per-box"):
#   1. k2-libkrun-libs-<ver>-<arch>.tar.zst
#        /usr/local/lib64 libkrun.so* + libkrunfw.so* (+ pkgconfig/libkrun.pc).
#        libkrun.so.2.0.0 carries RPATH=/usr/local/lib64 (patchelf'd per
#        RUNBOOK-v2 §3) — preserved verbatim by the tarball.
#   2. k2-guest-base-<ver>-<arch>.tar.zst
#        The active guest rootfs (/opt/k2/guest-base-v2, build-guest.sh
#        output: Debian bookworm + node + claude-code + k2 CLI +
#        k2-hook-forwarder + k2-guest-init + seeded /root/.claude.json).
#   3. k2-vmm-worker-<ver>-<arch>.tar.zst
#        The setuid worker binary. Packaged mode 0755 (setuid deliberately
#        STRIPPED in transit); the bootstrap re-applies
#        `chown root:root && chmod u+s` on install. PATH CONVENTION: the
#        daemon resolves the worker NEXT TO ITS OWN BINARY
#        (k2-core sandbox.rs resolve_worker_path) — the bootstrap installs it
#        into the daemon's bin dir, NOT a fixed /opt path.
#   4. manifest.json  — provenance: versions, git SHAs, raw-binary sha256s,
#        install conventions.
#   5. SHA256SUMS     — sha256 of every asset; the bootstrap fail-closes on it.
#
# Environment contract:
#   K2_ARTIFACTS_VERSION  asset/tag version label            (default: v1)
#   K2_OUT_DIR            staging dir for the built assets
#                         (default: /root/k2-dedicated-artifacts-<ver>)
#   K2_LIB_DIR            libkrun install dir                (default: /usr/local/lib64)
#   K2_GUEST_BASE         guest rootfs to package            (default: /opt/k2/guest-base-v2)
#   K2_WORKER_BIN         worker binary to package
#                         (default: /opt/k2/target/debug/k2-vmm-worker)
#   K2_GH_REPO            GitHub repo for the release        (default: Alakazam-211/K2)
#   K2_SKIP_UPLOAD        1 = stage + manifest only; print the exact gh
#                         commands instead of running them   (default: 0)
#
# Upload target: release tag `dedicated-artifacts-<ver>` on K2_GH_REPO
# (created if absent, assets --clobber'd if present). Requires an authed
# `gh` CLI when K2_SKIP_UPLOAD=0.
#
# Idempotent: re-running rebuilds the staging dir from scratch and
# re-uploads with --clobber; nothing on the box outside K2_OUT_DIR changes.
set -euo pipefail

K2_ARTIFACTS_VERSION="${K2_ARTIFACTS_VERSION:-v1}"
K2_OUT_DIR="${K2_OUT_DIR:-/root/k2-dedicated-artifacts-${K2_ARTIFACTS_VERSION}}"
K2_LIB_DIR="${K2_LIB_DIR:-/usr/local/lib64}"
K2_GUEST_BASE="${K2_GUEST_BASE:-/opt/k2/guest-base-v2}"
K2_WORKER_BIN="${K2_WORKER_BIN:-/opt/k2/target/debug/k2-vmm-worker}"
K2_GH_REPO="${K2_GH_REPO:-Alakazam-211/K2}"
K2_SKIP_UPLOAD="${K2_SKIP_UPLOAD:-0}"

log() { printf '\033[1;36m[package]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[package] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = "0" ] || die "run as root (needs to read root-owned guest rootfs verbatim)"
command -v zstd >/dev/null 2>&1 || die "zstd missing (apt-get install -y zstd)"
command -v python3 >/dev/null 2>&1 || die "python3 missing"

case "$(uname -m)" in
	x86_64|amd64) ARCH="amd64" ;;
	aarch64|arm64) ARCH="arm64" ;;
	*) die "unsupported arch: $(uname -m)" ;;
esac

VER="$K2_ARTIFACTS_VERSION"
TAG="dedicated-artifacts-${VER}"
LIBS_TAR="k2-libkrun-libs-${VER}-${ARCH}.tar.zst"
GUEST_TAR="k2-guest-base-${VER}-${ARCH}.tar.zst"
WORKER_TAR="k2-vmm-worker-${VER}-${ARCH}.tar.zst"

# ── 0. preflight: every source artifact must exist and look right ────
log "preflight — verifying source artifacts"
LIB_FILES=$(cd "$K2_LIB_DIR" 2>/dev/null && ls libkrun.so* libkrunfw.so* 2>/dev/null || true)
[ -n "$LIB_FILES" ] || die "no libkrun*/libkrunfw* in $K2_LIB_DIR"
LIBKRUN_REAL=$(cd "$K2_LIB_DIR" && ls libkrun.so.*.*.* 2>/dev/null | head -1)
LIBKRUNFW_REAL=$(cd "$K2_LIB_DIR" && ls libkrunfw.so.*.*.* 2>/dev/null | head -1)
[ -n "$LIBKRUN_REAL" ] || die "libkrun real .so not found in $K2_LIB_DIR"
[ -n "$LIBKRUNFW_REAL" ] || die "libkrunfw real .so not found in $K2_LIB_DIR"

[ -d "$K2_GUEST_BASE" ] || die "guest base missing: $K2_GUEST_BASE"
[ -x "$K2_GUEST_BASE/usr/local/bin/k2-guest-init" ] \
	|| die "guest base has no k2-guest-init — wrong/stale rootfs?"
[ -f "$K2_GUEST_BASE/root/.claude.json" ] \
	|| die "guest base missing seeded /root/.claude.json (claude exits rc=2 without it)"
# W7 de-Claudify: codex/gemini/copilot/pi joined the guest recipe
# (build-guest.sh EXTRA_AGENT_PKGS) so non-claude presets can run in cells.
# A pre-W7 guest base FAILS here on purpose — re-bake before packaging.
# (Validated only at the next Linux image bake; no Linux build runs on a Mac.)
for t in k2 k2-hook-forwarder claude codex gemini copilot pi node; do
	[ -e "$K2_GUEST_BASE/usr/local/bin/$t" ] || die "guest base missing /usr/local/bin/$t"
done

[ -f "$K2_WORKER_BIN" ] || die "worker binary missing: $K2_WORKER_BIN"
file "$K2_WORKER_BIN" | grep -q "ELF" || die "worker is not an ELF binary"

# ── 1. stage ──────────────────────────────────────────────────────────
log "staging into $K2_OUT_DIR"
rm -rf "$K2_OUT_DIR"
mkdir -p "$K2_OUT_DIR"

# 1a. libkrun + libkrunfw (real .so files + their symlink chains + the .pc)
log "packing $LIBS_TAR ($(echo "$LIB_FILES" | tr '\n' ' '))"
PC_REL=""
[ -f "$K2_LIB_DIR/pkgconfig/libkrun.pc" ] && PC_REL="pkgconfig/libkrun.pc"
# shellcheck disable=SC2086  # LIB_FILES is a deliberate word-split file list
(cd "$K2_LIB_DIR" && tar --numeric-owner -cf - $LIB_FILES $PC_REL) \
	| zstd -T0 -q -o "$K2_OUT_DIR/$LIBS_TAR"

# 1b. guest base rootfs (packaged as ./ so it extracts INTO the target dir)
log "packing $GUEST_TAR from $K2_GUEST_BASE ($(du -sh "$K2_GUEST_BASE" | cut -f1)) — this takes a minute"
(cd "$K2_GUEST_BASE" && tar --numeric-owner -cf - .) \
	| zstd -T0 -q -o "$K2_OUT_DIR/$GUEST_TAR"

# 1c. worker (normalized name + mode; setuid stripped in transit)
log "packing $WORKER_TAR"
WORKER_STAGE=$(mktemp -d)
install -m 0755 "$K2_WORKER_BIN" "$WORKER_STAGE/k2-vmm-worker"
(cd "$WORKER_STAGE" && tar --numeric-owner -cf - k2-vmm-worker) \
	| zstd -T0 -q -o "$K2_OUT_DIR/$WORKER_TAR"
rm -rf "$WORKER_STAGE"

# ── 2. manifest.json — provenance for humans + the bootstrap ─────────
log "writing manifest.json"
LIBKRUN_GIT=$(git -C /opt/libkrun log -1 --format=%H 2>/dev/null || echo "unknown")
LIBKRUN_TAG=$(git -C /opt/libkrun describe --tags 2>/dev/null || echo "untagged")
LIBKRUNFW_GIT=$(git -C /opt/libkrunfw log -1 --format=%H 2>/dev/null || echo "unknown")
LIBKRUNFW_TAG=$(git -C /opt/libkrunfw describe --tags 2>/dev/null || echo "untagged")
# worker lives at <repo>/target/<profile>/k2-vmm-worker → repo = two dirs up
K2_REPO_SHA=$(git -C "$(dirname "$K2_WORKER_BIN")/../.." log -1 --format=%H 2>/dev/null \
	|| git -C /opt/k2 log -1 --format=%H 2>/dev/null || echo "unknown")
GUEST_META=$(cat /opt/k2-sandbox-versions.txt 2>/dev/null || echo "")

export MANIFEST_VER="$VER" MANIFEST_ARCH="$ARCH" MANIFEST_TAG="$TAG"
export MANIFEST_LIBKRUN_REAL="$LIBKRUN_REAL" MANIFEST_LIBKRUNFW_REAL="$LIBKRUNFW_REAL"
export MANIFEST_LIBKRUN_GIT="$LIBKRUN_GIT" MANIFEST_LIBKRUN_TAG="$LIBKRUN_TAG"
export MANIFEST_LIBKRUNFW_GIT="$LIBKRUNFW_GIT" MANIFEST_LIBKRUNFW_TAG="$LIBKRUNFW_TAG"
export MANIFEST_K2_REPO_SHA="$K2_REPO_SHA" MANIFEST_GUEST_META="$GUEST_META"
export MANIFEST_LIBS_TAR="$LIBS_TAR" MANIFEST_GUEST_TAR="$GUEST_TAR" MANIFEST_WORKER_TAR="$WORKER_TAR"
export MANIFEST_WORKER_SHA="$(sha256sum "$K2_WORKER_BIN" | cut -d' ' -f1)"
export MANIFEST_LIBKRUN_SHA="$(sha256sum "$K2_LIB_DIR/$LIBKRUN_REAL" | cut -d' ' -f1)"
export MANIFEST_LIBKRUNFW_SHA="$(sha256sum "$K2_LIB_DIR/$LIBKRUNFW_REAL" | cut -d' ' -f1)"
export MANIFEST_HOST="$(hostname)" MANIFEST_KERNEL="$(uname -r)"
python3 - "$K2_OUT_DIR/manifest.json" <<'PYEOF'
import json, os, sys, datetime
e = os.environ
m = {
    "schema": 1,
    "version": e["MANIFEST_VER"],
    "tag": e["MANIFEST_TAG"],
    "arch": e["MANIFEST_ARCH"],
    "built_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "built_on": {"host": e["MANIFEST_HOST"], "kernel": e["MANIFEST_KERNEL"]},
    "assets": {
        "libs": e["MANIFEST_LIBS_TAR"],
        "guest_base": e["MANIFEST_GUEST_TAR"],
        "worker": e["MANIFEST_WORKER_TAR"],
    },
    "libkrun": {
        "so": e["MANIFEST_LIBKRUN_REAL"],
        "git": e["MANIFEST_LIBKRUN_GIT"],
        "git_tag": e["MANIFEST_LIBKRUN_TAG"],
        "sha256": e["MANIFEST_LIBKRUN_SHA"],
        "rpath": "/usr/local/lib64",
    },
    "libkrunfw": {
        "so": e["MANIFEST_LIBKRUNFW_REAL"],
        "git": e["MANIFEST_LIBKRUNFW_GIT"],
        "git_tag": e["MANIFEST_LIBKRUNFW_TAG"],
        "sha256": e["MANIFEST_LIBKRUNFW_SHA"],
        "license_note": "libkrunfw bundles a Linux kernel (GPLv2) — installing it is an explicit operator opt-in (K2_SANDBOX_CONSENT=1 in the bootstrap)",
    },
    "worker": {
        "sha256_raw_binary": e["MANIFEST_WORKER_SHA"],
        "k2_repo_sha": e["MANIFEST_K2_REPO_SHA"],
        "install_convention": "install NEXT TO the k2-daemon binary (resolve_worker_path = current_exe sibling); then chown root:root && chmod u+s — re-apply after ANY daemon dir change",
    },
    "guest_base": {
        "install_path": "/opt/k2/guest-base-v2 (K2_SANDBOX_GUEST_BASE)",
        "build_recipe": ".k2/notes/sandbox-passt/build-guest.sh",
        "source_metadata": e["MANIFEST_GUEST_META"],
    },
    "install_notes": {
        "libs": "extract into /usr/local/lib64, add /etc/ld.so.conf.d/k2-libkrun.conf, ldconfig",
        "nft": "setcap cap_net_admin+ei /usr/sbin/nft",
        "host_packages": ["nftables", "passt", "zstd", "e2fsprogs"],
    },
}
with open(sys.argv[1], "w") as f:
    json.dump(m, f, indent=2)
    f.write("\n")
PYEOF

# ── 3. SHA256SUMS over every asset ────────────────────────────────────
log "writing SHA256SUMS"
(cd "$K2_OUT_DIR" && sha256sum "$LIBS_TAR" "$GUEST_TAR" "$WORKER_TAR" manifest.json > SHA256SUMS)
cat "$K2_OUT_DIR/SHA256SUMS"

# ── 4. upload as GitHub release assets ────────────────────────────────
ASSETS=("$K2_OUT_DIR/$LIBS_TAR" "$K2_OUT_DIR/$GUEST_TAR" "$K2_OUT_DIR/$WORKER_TAR"
	"$K2_OUT_DIR/manifest.json" "$K2_OUT_DIR/SHA256SUMS")
if [ "$K2_SKIP_UPLOAD" = "1" ]; then
	log "K2_SKIP_UPLOAD=1 — staged only. To publish, run:"
	echo "  gh release create '$TAG' -R '$K2_GH_REPO' --title 'Dedicated sandbox artifacts $VER' \\"
	echo "      --notes 'Prebuilt Dedicated-tier sandbox stack (libkrun/libkrunfw + guest base + k2-vmm-worker). See manifest.json. libkrunfw bundles a GPLv2 Linux kernel.' \\"
	printf '      %q %q %q %q %q\n' "${ASSETS[@]}"
	echo "  # or, if the tag already exists:"
	printf '  gh release upload %q -R %q --clobber %q %q %q %q %q\n' "$TAG" "$K2_GH_REPO" "${ASSETS[@]}"
else
	command -v gh >/dev/null 2>&1 || die "gh CLI missing (or set K2_SKIP_UPLOAD=1)"
	if gh release view "$TAG" -R "$K2_GH_REPO" >/dev/null 2>&1; then
		log "release $TAG exists — uploading assets (--clobber)"
		gh release upload "$TAG" -R "$K2_GH_REPO" --clobber "${ASSETS[@]}"
	else
		log "creating release $TAG on $K2_GH_REPO"
		gh release create "$TAG" -R "$K2_GH_REPO" \
			--title "Dedicated sandbox artifacts $VER" \
			--notes "Prebuilt Dedicated-tier sandbox stack (libkrun/libkrunfw + guest base + k2-vmm-worker) harvested from the reference box. See manifest.json for provenance. NOTE: libkrunfw bundles a Linux kernel (GPLv2) — the bootstrap installs it only with K2_SANDBOX_CONSENT=1." \
			"${ASSETS[@]}"
	fi
	log "published: https://github.com/$K2_GH_REPO/releases/tag/$TAG"
fi

log "PACKAGE COMPLETE — $K2_OUT_DIR"
ls -lh "$K2_OUT_DIR"
