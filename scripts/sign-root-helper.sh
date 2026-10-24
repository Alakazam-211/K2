#!/usr/bin/env bash
# sign-root-helper.sh — sign a release's k2-root-helper assets with the
# ROOT-HELPER key (prd-k2-root-helper-v1 §9.1, RH7). Runs on z3mbpZ only,
# from release.sh (Step 10.5) or by hand. The secret key never leaves that
# Mac and never enters GitHub; it is NOT the app-updater key.
#
#   bash scripts/sign-root-helper.sh --preflight        # release.sh Step 0.6
#   bash scripts/sign-root-helper.sh <version>          # after CI uploaded the assets
#
# For each arch it: waits for daemon-binaries.yml to upload
# k2-root-helper-linux-<arch> + .sha256 (unsigned); downloads them; checks
# the sha256, the ELF, exactly one build marker equal to
# `v=<version> proto=<p> arch=<arch>`, and that it is not a test-keys build;
# signs with `minisign -S -t "file:k2-root-helper-linux-<arch>
# version:<version> protocol:<p>"`; verifies the new .sig with the primary
# public key; uploads the .sig.
#
# ── KEY CEREMONY (Rosson, once, on z3mbpZ in a GUI terminal) ─────────────
#  1. brew install minisign
#  2. Primary key (stays on z3mbpZ, encrypted):
#       mkdir -p ~/.k2-root-helper && chmod 700 ~/.k2-root-helper
#       minisign -G -p ~/.k2-root-helper/k2-root-helper.pub \
#                   -s ~/.k2-root-helper/k2-root-helper.key
#     Pick a long random password. Store it in the login keychain:
#       security add-generic-password -a "$USER" -s k2-root-helper-signing -w
#     (prompts for the password; never put it on the command line).
#  3. Backup key (offline; never on any K2 box, never on z3mbpZ after):
#       minisign -G -p backup.pub -s backup.key   (on a trusted machine)
#     Keep backup.key + its password printed / in your password manager,
#     then delete the files from disk. Keep backup.pub.
#  4. Send K2 the TWO public key lines (the second line of each .pub) and
#     the key ids (`minisign` prints them). K2 puts them in:
#       crates/k2-root-proto/src/keys.rs  RELEASE_KEYS (primary, then backup)
#       scripts/install-root-helper.sh    K2_ROOT_HELPER_PUBKEYS=(primary backup)
#     A daemon test fails if the two copies differ. Public keys only — a
#     secret key never goes in the repo.
#  5. Dry run: bash scripts/sign-root-helper.sh --preflight
#  Rotation: release N signed by A carries [A, B]; N+1 signed by B carries
#  [B, C]. Lost primary: sign with the backup, ship a new pair. Stolen key:
#  emergency release + the fleet reinstall pass with a new key set.
#
# Environment:
#   K2_ROOT_HELPER_KEY      secret key path (default ~/.k2-root-helper/k2-root-helper.key)
#   K2_ROOT_HELPER_PUB      public key path (default ~/.k2-root-helper/k2-root-helper.pub)
#   K2_ROOT_HELPER_KEYCHAIN keychain service for the password (default k2-root-helper-signing)
#   K2_RELEASE_REPO         default Alakazam-211/K2
#   K2_SIGN_WAIT_MIN        minutes to wait for CI assets (default 60)
#   K2_SIGN_POLL_SECS       seconds between polls for them (default 30)
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

log() { printf '[sign-root-helper] %s\n' "$*"; }
die() { printf '[sign-root-helper] ERROR: %s\n' "$*" >&2; exit 1; }

keys_in_source() {
	# The public keys compiled into the helper/daemon (keys.rs RELEASE_KEYS).
	sed -n '/^pub const RELEASE_KEYS/,/^];/p' "$ROOT/crates/k2-root-proto/src/keys.rs" \
		| grep -o 'public_key_b64: "[^"]*"' | sed 's/public_key_b64: "\(.*\)"/\1/'
}

preflight() {
	command -v minisign >/dev/null 2>&1 || die "minisign is not installed (brew install minisign)"
	command -v gh >/dev/null 2>&1 || die "gh is not installed"
	[ -n "$(keys_in_source)" ] || die "RELEASE_KEYS in crates/k2-root-proto/src/keys.rs is empty — the key ceremony (this file's header) has not been done. A release without keys ships a helper that can never self-update."
	[ -f "$KEY" ] || die "secret key missing: $KEY (key ceremony step 2; z3mbpZ only)"
	[ -f "$PUB" ] || die "public key missing: $PUB"
	local publine
	publine="$(sed -n 2p "$PUB")"
	keys_in_source | grep -qxF "$publine" || die "$PUB is not one of RELEASE_KEYS — wrong key on this Mac?"
	security find-generic-password -s "$SERVICE" -w >/dev/null 2>&1 \
		|| die "no keychain password for service '$SERVICE' (key ceremony step 2)"
	log "preflight ok: key $(sed -n 1p "$PUB" | sed 's/.*: //') is in RELEASE_KEYS; minisign + gh + keychain ready"
}

if [ "${1:-}" = "--preflight" ]; then
	preflight
	exit 0
fi
VERSION="${1:-}"
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: sign-root-helper.sh <x.y.z> | --preflight"
preflight

TMP="$(mktemp -d -t k2-sign-root-helper)"
trap 'rm -rf "$TMP"' EXIT
TAG="v$VERSION"

wait_for_assets() {
	local deadline=$(( $(date +%s) + WAIT_MIN * 60 )) names
	while :; do
		names="$(gh release view "$TAG" -R "$REPO" --json assets -q '.assets[].name' 2>/dev/null || true)"
		if printf '%s\n' "$names" | grep -qx k2-root-helper-linux-x86_64 \
			&& printf '%s\n' "$names" | grep -qx k2-root-helper-linux-x86_64.sha256 \
			&& printf '%s\n' "$names" | grep -qx k2-root-helper-linux-aarch64 \
			&& printf '%s\n' "$names" | grep -qx k2-root-helper-linux-aarch64.sha256; then
			k2_wait_done
			return 0
		fi
		[ "$(date +%s)" -lt "$deadline" ] || { k2_wait_done; die "CI did not upload the k2-root-helper assets to $TAG within ${WAIT_MIN} min (daemon-binaries.yml)"; }
		k2_wait_tick "[sign-root-helper] waiting for daemon-binaries.yml to upload the root helper assets…"
		sleep "$POLL_SECS"
	done
}

wait_for_assets
for ARCH in x86_64 aarch64; do
	A="k2-root-helper-linux-$ARCH"
	gh release download "$TAG" -R "$REPO" -p "$A" -p "$A.sha256" -D "$TMP" --clobber
	want="$(awk '{print $1; exit}' "$TMP/$A.sha256")"
	got="$(shasum -a 256 "$TMP/$A" | awk '{print $1}')"
	[ -n "$want" ] && [ "$want" = "$got" ] || die "$A: sha256 differs from CI's .sha256 ($got vs $want)"
	[ "$(head -c 4 "$TMP/$A" | od -An -c | tr -d ' \n')" = '177ELF' ] || die "$A is not an ELF"
	if LC_ALL=C grep -a -q K2_ROOT_HELPER_TEST_KEYS_ENABLED "$TMP/$A"; then
		die "$A is a TEST-KEYS build — refusing to sign"
	fi
	if LC_ALL=C grep -a -q K2_SEW_TEST_PINS_ENABLED "$TMP/$A"; then
		die "$A pins Sew's TEST builds (sew-test-pins) — refusing to sign"
	fi
	markers="$(LC_ALL=C grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]* proto=[0-9]* arch=[a-z0-9_]*;' "$TMP/$A" || true)"
	[ "$(printf '%s\n' "$markers" | grep -c .)" = 1 ] || die "$A: expected exactly one build marker, found: $markers"
	[[ "$markers" =~ ^K2_ROOT_HELPER_BUILD\ v=([0-9.]+)\ proto=([0-9]+)\ arch=([a-z0-9_]+)\;$ ]] || die "$A: marker unreadable"
	[ "${BASH_REMATCH[1]}" = "$VERSION" ] || die "$A: built as v${BASH_REMATCH[1]}, release is v$VERSION"
	[ "${BASH_REMATCH[3]}" = "$ARCH" ] || die "$A: built for ${BASH_REMATCH[3]}"
	PROTO="${BASH_REMATCH[2]}"
	COMMENT="file:$A version:$VERSION protocol:$PROTO"
	log "signing $A ($COMMENT)"
	security find-generic-password -s "$SERVICE" -w \
		| minisign -S -s "$KEY" -m "$TMP/$A" -x "$TMP/$A.sig" -t "$COMMENT" >/dev/null
	out="$(minisign -V -m "$TMP/$A" -p "$PUB" -x "$TMP/$A.sig" -Q)" || die "$A: the new signature does not verify"
	[ "$(printf '%s\n' "$out" | tail -n1)" = "$COMMENT" ] || die "$A: trusted comment round-trip mismatch: $out"
	gh release upload "$TAG" -R "$REPO" "$TMP/$A.sig" --clobber
	log "$A.sig uploaded"
done
log "done: $TAG root helper assets are signed"
