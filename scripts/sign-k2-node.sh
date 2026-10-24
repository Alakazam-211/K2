#!/usr/bin/env bash
# sign-k2-node.sh — sign a release's k2-node assets with the ROOT-HELPER key
# (prd-k2-node-admin-v1 §8.1/§8.2, S5). k2-node has a root mode (`k2-node
# admin serve`), so its updates are signed with the root-helper key, never
# the app-updater key. Runs on z3mbpZ only, from release.sh (Step 10.5,
# after sign-root-helper.sh) or by hand. Same key, same keychain password,
# same ceremony as sign-root-helper.sh (see its header).
#
#   bash scripts/sign-k2-node.sh --preflight            # release.sh Step 0.55
#   bash scripts/sign-k2-node.sh <version>              # after the assets are on the release
#   bash scripts/sign-k2-node.sh --check <file> <os> <arch> <version>   # checks only (tests)
#
# Assets (all three must be on the release first, each with a .sha256):
#   k2-node-linux-x86_64, k2-node-linux-aarch64   unsigned, from daemon-binaries.yml
#   k2-node-macos-aarch64                         codesigned + notarized, from release.sh Step 8.55
#
# For each asset it checks the sha256 against the published .sha256, the
# binary format (64-bit ELF / thin Mach-O for that arch), exactly one build
# marker `K2_NODE_BUILD v=<version> proto=<p> os=<os> arch=<arch>;` equal
# to the release, that it is not a test-keys build (no
# `K2_NODE_TEST_KEYS_ENABLED`), and on macOS the Developer ID signature with
# K2's Team ID (the same requirement the node's executor checks before a
# swap). Then it signs with
#   minisign -S -t "file:k2-node-<os>-<arch> version:<version> protocol:<p>"
# verifies the new .sig with the public key, and uploads only the .sig.
# Nodes and the controller verify it with k2_root_proto::node_build.
#
# Environment (shared with sign-root-helper.sh):
#   K2_ROOT_HELPER_KEY      secret key path (default ~/.k2-root-helper/k2-root-helper.key)
#   K2_ROOT_HELPER_PUB      public key path (default ~/.k2-root-helper/k2-root-helper.pub)
#   K2_ROOT_HELPER_KEYCHAIN keychain service for the password (default k2-root-helper-signing)
#   K2_RELEASE_REPO         default Alakazam-211/K2
#   K2_SIGN_WAIT_MIN        minutes to wait for CI assets (default 60)
#   K2_SIGN_POLL_SECS       seconds between polls for them (default 30)
#   K2_RELEASE_KEYS_RS      keys.rs to read RELEASE_KEYS from (default the repo's; tests only)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEY="${K2_ROOT_HELPER_KEY:-$HOME/.k2-root-helper/k2-root-helper.key}"
PUB="${K2_ROOT_HELPER_PUB:-$HOME/.k2-root-helper/k2-root-helper.pub}"
SERVICE="${K2_ROOT_HELPER_KEYCHAIN:-k2-root-helper-signing}"
REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
WAIT_MIN="${K2_SIGN_WAIT_MIN:-60}"
# Seconds between polls for the CI assets (tests set 0).
POLL_SECS="${K2_SIGN_POLL_SECS:-30}"
# One "waiting for …" line, then dots, a status line every few minutes.
# shellcheck source=lib/wait-progress.sh
source "$ROOT/scripts/lib/wait-progress.sh"
KEYS_RS="${K2_RELEASE_KEYS_RS:-$ROOT/crates/k2-root-proto/src/keys.rs}"
# The macOS Developer ID team (release.sh SIGNING_IDENTITY; k2-node
# admin/update.rs APPLE_TEAM_ID).
TEAM_ID="36B8R93HXV"
ASSETS=(k2-node-linux-x86_64 k2-node-linux-aarch64 k2-node-macos-aarch64)

log() { printf '[sign-k2-node] %s\n' "$*"; }
die() { printf '[sign-k2-node] ERROR: %s\n' "$*" >&2; exit 1; }

keys_in_source() {
	sed -n '/^pub const RELEASE_KEYS/,/^];/p' "$KEYS_RS" \
		| grep -o 'public_key_b64: "[^"]*"' | sed 's/public_key_b64: "\(.*\)"/\1/'
}

preflight() {
	command -v minisign >/dev/null 2>&1 || die "minisign is not installed (brew install minisign)"
	command -v gh >/dev/null 2>&1 || die "gh is not installed"
	command -v codesign >/dev/null 2>&1 || die "codesign is missing (the macOS asset's Developer ID check needs it; run on z3mbpZ)"
	[ -n "$(keys_in_source)" ] || die "RELEASE_KEYS in $KEYS_RS is empty — no root-helper key ceremony"
	[ -f "$KEY" ] || die "secret key missing: $KEY (root-helper key; z3mbpZ only)"
	[ -f "$PUB" ] || die "public key missing: $PUB"
	local publine
	publine="$(sed -n 2p "$PUB")"
	keys_in_source | grep -qxF "$publine" || die "$PUB is not one of RELEASE_KEYS — wrong key on this Mac?"
	security find-generic-password -s "$SERVICE" -w >/dev/null 2>&1 \
		|| die "no keychain password for service '$SERVICE'"
	log "preflight ok: root-helper key $(sed -n 1p "$PUB" | sed 's/.*: //') signs k2-node; minisign + gh + codesign + keychain ready"
}

# check_asset <file> <os> <arch> <version> → prints the protocol on success.
check_asset() {
	local f="$1" os="$2" arch="$3" version="$4" name magic cpu ftype markers
	name="k2-node-$os-$arch"
	[ -f "$f" ] || die "$name: no file at $f"
	case "$os/$arch" in
	linux/x86_64 | linux/aarch64 | macos/x86_64 | macos/aarch64) ;;
	*) die "$name: unsupported os/arch" ;;
	esac
	if [ "$os" = linux ]; then
		[ "$(head -c 4 "$f" | od -An -c | tr -d ' \n')" = '177ELF' ] || die "$name is not an ELF"
		# EI_CLASS 2 = 64-bit, EI_DATA 1 = little-endian; e_machine 0x3e x86_64, 0xb7 aarch64.
		[ "$(od -An -tu1 -j4 -N2 "$f" | tr -s ' ' | sed 's/^ //;s/ $//')" = '2 1' ] || die "$name is not a 64-bit little-endian ELF"
		cpu="$(od -An -tu1 -j18 -N2 "$f" | tr -s ' ' | sed 's/^ //;s/ $//')"
		case "$arch" in
		x86_64) [ "$cpu" = '62 0' ] || die "$name is not an x86_64 ELF" ;;
		aarch64) [ "$cpu" = '183 0' ] || die "$name is not an aarch64 ELF" ;;
		esac
	else
		magic="$(od -An -tx1 -N4 "$f" | tr -d ' \n')"
		[ "$magic" = cffaedfe ] || die "$name is not a thin 64-bit Mach-O (universal binaries are refused)"
		cpu="$(od -An -tx1 -j4 -N4 "$f" | tr -d ' \n')"
		case "$arch" in
		x86_64) [ "$cpu" = 07000001 ] || die "$name is not an x86_64 Mach-O" ;;
		aarch64) [ "$cpu" = 0c000001 ] || die "$name is not an arm64 Mach-O" ;;
		esac
		ftype="$(od -An -tx1 -j12 -N4 "$f" | tr -d ' \n')"
		[ "$ftype" = 02000000 ] || die "$name is not a Mach-O executable"
	fi
	if LC_ALL=C grep -a -q K2_NODE_TEST_KEYS_ENABLED "$f"; then
		die "$name is a TEST-KEYS build — refusing to sign"
	fi
	if [ "$(LC_ALL=C grep -a -o 'K2_NODE_BUILD v=' "$f" | grep -c .)" != 1 ]; then
		die "$name: expected exactly one K2_NODE_BUILD marker"
	fi
	markers="$(LC_ALL=C grep -a -o 'K2_NODE_BUILD v=[0-9.]* proto=[0-9]* os=[a-z]* arch=[a-z0-9_]*;' "$f" || true)"
	[ "$(printf '%s\n' "$markers" | grep -c .)" = 1 ] || die "$name: build marker unreadable: $markers"
	[[ "$markers" =~ ^K2_NODE_BUILD\ v=([0-9]+\.[0-9]+\.[0-9]+)\ proto=([0-9]+)\ os=([a-z]+)\ arch=([a-z0-9_]+)\;$ ]] || die "$name: marker unreadable: $markers"
	[ "${BASH_REMATCH[1]}" = "$version" ] || die "$name: built as v${BASH_REMATCH[1]}, release is v$version"
	[ "${BASH_REMATCH[3]}" = "$os" ] || die "$name: built for os ${BASH_REMATCH[3]}"
	[ "${BASH_REMATCH[4]}" = "$arch" ] || die "$name: built for arch ${BASH_REMATCH[4]}"
	printf '%s\n' "${BASH_REMATCH[2]}"
}

check_codesign() {
	local f="$1" name="$2"
	codesign --verify --strict -R "=anchor apple generic and certificate leaf[subject.OU] = \"$TEAM_ID\"" "$f" \
		|| die "$name is not signed by K2's Developer ID ($TEAM_ID); nodes would refuse it"
}

if [ "${1:-}" = "--preflight" ]; then
	preflight
	exit 0
fi
if [ "${1:-}" = "--check" ]; then
	[ $# -eq 5 ] || die "usage: sign-k2-node.sh --check <file> <os> <arch> <version>"
	check_asset "$2" "$3" "$4" "${5#v}"
	exit 0
fi
VERSION="${1:-}"
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: sign-k2-node.sh <x.y.z> | --preflight | --check <file> <os> <arch> <version>"
preflight

TMP="$(mktemp -d -t k2-sign-k2-node.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
TAG="v$VERSION"

wait_for_assets() {
	local deadline=$(( $(date +%s) + WAIT_MIN * 60 )) names a missing
	while :; do
		names="$(gh release view "$TAG" -R "$REPO" --json assets -q '.assets[].name' 2>/dev/null || true)"
		missing=""
		for a in "${ASSETS[@]}"; do
			printf '%s\n' "$names" | grep -qx "$a" || missing="$missing $a"
			printf '%s\n' "$names" | grep -qx "$a.sha256" || missing="$missing $a.sha256"
		done
		if [ -z "$missing" ]; then
			k2_wait_done
			return 0
		fi
		[ "$(date +%s)" -lt "$deadline" ] || { k2_wait_done; die "$TAG is missing within ${WAIT_MIN} min:$missing (linux: daemon-binaries.yml; macos: release.sh Step 8.55)"; }
		k2_wait_tick "[sign-k2-node] waiting for:$missing"
		sleep "$POLL_SECS"
	done
}

wait_for_assets
for A in "${ASSETS[@]}"; do
	rest="${A#k2-node-}"
	OS="${rest%%-*}"
	ARCH="${rest#*-}"
	gh release download "$TAG" -R "$REPO" -p "$A" -p "$A.sha256" -D "$TMP" --clobber
	want="$(awk '{print $1; exit}' "$TMP/$A.sha256")"
	got="$(shasum -a 256 "$TMP/$A" | awk '{print $1}')"
	[ -n "$want" ] && [ "$want" = "$got" ] || die "$A: sha256 differs from the published .sha256 ($got vs $want)"
	PROTO="$(check_asset "$TMP/$A" "$OS" "$ARCH" "$VERSION")"
	[ "$OS" = macos ] && check_codesign "$TMP/$A" "$A"
	COMMENT="file:$A version:$VERSION protocol:$PROTO"
	log "signing $A ($COMMENT)"
	security find-generic-password -s "$SERVICE" -w \
		| minisign -S -s "$KEY" -m "$TMP/$A" -x "$TMP/$A.sig" -t "$COMMENT" >/dev/null
	out="$(minisign -V -m "$TMP/$A" -p "$PUB" -x "$TMP/$A.sig" -Q)" || die "$A: the new signature does not verify"
	[ "$(printf '%s\n' "$out" | tail -n1)" = "$COMMENT" ] || die "$A: trusted comment round-trip mismatch: $out"
	gh release upload "$TAG" -R "$REPO" "$TMP/$A.sig" --clobber
	log "$A.sig uploaded"
done
log "done: $TAG k2-node assets are signed with the root-helper key"
