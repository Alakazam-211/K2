#!/usr/bin/env bash
# install-mail-helper.sh — install the hosted-mail root helper on a K2 Linux
# box. Run ONCE per box, AS ROOT, by ops. Never by the agent.
#
#   curl -fsSL https://github.com/Alakazam-211/K2/releases/download/v<version>/install-mail-helper.sh \
#     | sudo bash -s -- --version <version>
#
#   sudo bash install-mail-helper.sh                         # version of the daemon on this box
#   sudo bash install-mail-helper.sh --version 0.44.1
#   sudo bash install-mail-helper.sh --file ./k2-mail-helper  # locally built binary
#
# What it does (and nothing else):
#   1. Gets k2-mail-helper for this box's arch from the GitHub release of
#      <version> (k2-mail-helper-linux-<arch> + .sig + .sha256), and verifies
#      the minisign signature against the SAME embedded updater pubkey as
#      install-daemon.sh, then the sha256. Or takes --file.
#   2. Installs it to /usr/local/libexec/k2-mail-helper, root:root 0755.
#   3. Writes /etc/sudoers.d/k2-mail-helper (0440) with exactly:
#        k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-mail-helper
#        Defaults:k2 !requiretty
#      validated with `visudo -cf` BEFORE it moves into place. A failed
#      validation leaves nothing behind.
#
# The daemon (user k2) then runs `sudo -n /usr/local/libexec/k2-mail-helper
# <verb>` — an exact-argv allowlist (ensure-user, mkdir, chown, write,
# install-bin, systemctl, remove). The agent never gets a root shell.
# It never touches /etc/sudoers.d/k2-pg-helper, never touches Stalwart, and
# never creates /usr/local/bin/stalwart (`k2 hostmail enable` does that
# through the helper).
#
# Idempotent: re-running with the same version re-verifies and changes
# nothing. Release assets exist from 0.44.1; the helper's argv must match
# the daemon, so install the version the daemon runs.
#
# Environment (rarely needed):
#   K2_RUN_USER      daemon user (default k2; same as --user)
#   K2_RELEASE_BASE  release download base (default the GitHub releases URL;
#                    file:// works for offline mirrors)
set -euo pipefail

# Minisign verify pubkey — literal from plugins.updater.pubkey in
# src-tauri/tauri.conf.json, identical to scripts/install-daemon.sh and
# cli/k2. Rotate all three together.
K2_HELPER_PUBKEY="dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEU5MTExNDQ2RjY1RUJCMDUKUldRRnUxNzJSaFFSNlFCcXptaWoyRTlidERHaERXbXBkSCthaDEvTTRQbXVIUElOVVd2S0xmNm8K"

RELEASE_BASE="${K2_RELEASE_BASE:-https://github.com/Alakazam-211/K2/releases/download}"
RUN_USER="${K2_RUN_USER:-k2}"
# K2_INSTALL_ROOT is a DESTDIR-style prefix for the hermetic test
# (tests/cli/install_mail_helper.sh). Production leaves it empty.
DEST_ROOT="${K2_INSTALL_ROOT:-}"
HELPER_PATH="/usr/local/libexec/k2-mail-helper"
SUDOERS_PATH="/etc/sudoers.d/k2-mail-helper"
HELPER_DEST="${DEST_ROOT}${HELPER_PATH}"
SUDOERS_DEST="${DEST_ROOT}${SUDOERS_PATH}"

VERSION=""
FILE=""

usage() {
	cat <<'EOF'
Usage: install-mail-helper.sh [--version <x.y.z> | --file <path>] [--user <name>]

Run as root. Installs /usr/local/libexec/k2-mail-helper (root:root 0755) and
/etc/sudoers.d/k2-mail-helper (0440, visudo-checked) so the K2 daemon user
can enable hosted mail.

  --version <v>   release to fetch (default: the daemon version on this box,
                  from its /boot-status or `k2-daemon --version`)
  --file <path>   install this local binary instead of downloading
  --user <name>   daemon user for the sudoers line (default: k2)
EOF
}

log() { printf '[install-mail-helper] %s\n' "$*"; }
die() { printf '[install-mail-helper] ERROR: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
	case "$1" in
		--version)
			[ $# -ge 2 ] || die "--version needs a value"
			VERSION="$2"; shift 2 ;;
		--version=*) VERSION="${1#--version=}"; shift ;;
		--file)
			[ $# -ge 2 ] || die "--file needs a path"
			FILE="$2"; shift 2 ;;
		--file=*) FILE="${1#--file=}"; shift ;;
		--user)
			[ $# -ge 2 ] || die "--user needs a name"
			RUN_USER="$2"; shift 2 ;;
		--user=*) RUN_USER="${1#--user=}"; shift ;;
		-h|--help) usage; exit 0 ;;
		*) usage >&2; die "unknown argument: $1" ;;
	esac
done

[ -z "$VERSION" ] || [ -z "$FILE" ] || die "pass --version or --file, not both"
VERSION="${VERSION#v}"
if [ -n "$VERSION" ] && ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.-]*$ ]]; then
	die "--version must look like 0.44.1 (got '$VERSION')"
fi
# The user name lands in sudoers: refuse anything but a plain login name.
[[ "$RUN_USER" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || die "--user must be a plain user name (got '$RUN_USER')"

[ "$(id -u)" = "0" ] || die "run as root (sudo bash install-mail-helper.sh ...)"
[ "$(uname -s)" = "Linux" ] || die "hosted mail is Linux-only; this is $(uname -s)"

case "$(uname -m)" in
	x86_64|amd64) ARCH="x86_64" ;;
	aarch64|arm64) ARCH="aarch64" ;;
	*) die "unsupported CPU arch: $(uname -m) (expected x86_64 or aarch64)" ;;
esac

command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required (coreutils)"
command -v sudo >/dev/null 2>&1 || die "sudo is not installed — the daemon reaches the helper only through sudo -n. Install it (apt-get install sudo) and re-run."
command -v visudo >/dev/null 2>&1 || die "visudo is not installed — it validates the sudoers file before it goes live. Install sudo (apt-get install sudo) and re-run."

TMP="$(mktemp -d "${TMPDIR:-/tmp}/k2-mail-helper.XXXXXX")" || die "mktemp failed"
STAGED_HELPER=""
STAGED_SUDOERS=""
cleanup() {
	[ -z "$STAGED_HELPER" ] || rm -f "$STAGED_HELPER"
	[ -z "$STAGED_SUDOERS" ] || rm -f "$STAGED_SUDOERS"
	rm -rf "$TMP"
}
trap cleanup EXIT

sha_of() { sha256sum "$1" | awk '{print $1}'; }

user_home() {
	local h
	h="$(getent passwd "$RUN_USER" 2>/dev/null | cut -d: -f6 || true)"
	[ -n "$h" ] || h="/home/$RUN_USER"
	printf '%s\n' "$h"
}

# The version the box's daemon runs. Prefer the RUNNING daemon's
# unauthenticated /boot-status (no binary is executed). Fall back to
# `k2-daemon --version` (safe on 0.40.82+, which exits without booting;
# the helper itself needs 0.40.150+), bounded by timeout.
detect_version() {
	local home port body v bin
	home="$(user_home)"
	for pf in "$home/.k2/daemon.port" "$home/.k2so/daemon.port"; do
		[ -f "$pf" ] || continue
		port="$(tr -dc '0-9' <"$pf")"
		[ -n "$port" ] || continue
		body="$(curl -fsS --max-time 3 "http://127.0.0.1:${port}/boot-status" 2>/dev/null || true)"
		v="$(printf '%s' "$body" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
		if [ -n "$v" ]; then
			printf '%s\n' "$v"
			return 0
		fi
	done
	for bin in "$home/.local/bin/k2-daemon" "$(command -v k2-daemon 2>/dev/null || true)" \
		/usr/bin/k2-daemon /usr/local/bin/k2-daemon; do
		[ -n "$bin" ] && [ -x "$bin" ] || continue
		v="$(timeout 10 "$bin" --version 2>/dev/null | sed -n 's/^k2-daemon[[:space:]]\{1,\}\([0-9][^[:space:]]*\).*/\1/p' | head -n1 || true)"
		if [ -n "$v" ]; then
			printf '%s\n' "$v"
			return 0
		fi
	done
	return 1
}

fetch() {
	# fetch <url> <dest>
	curl -fsSL --retry 3 --retry-delay 2 -o "$2" "$1"
}

is_elf() { [ "$(head -c 4 "$1" | od -An -c | tr -d ' \n')" = '177ELF' ]; }

# Same scheme as install-daemon.sh: tauri pubkey = base64 of the two-line
# minisign key file; a .sig may be raw minisig text or base64 of it.
verify_minisign() {
	# verify_minisign <file> <sig>
	local pub="$TMP/minisign.pub"
	if ! head -c 17 "$2" | grep -q "untrusted comment"; then
		base64 -d "$2" >"$2.dec" 2>/dev/null || die "signature is neither minisig text nor base64 of it"
		mv "$2.dec" "$2"
	fi
	printf '%s' "$K2_HELPER_PUBKEY" | base64 -d >"$pub" 2>/dev/null || die "embedded pubkey failed to decode"
	minisign -Vm "$1" -p "$pub" -x "$2" >/dev/null 2>&1
}

# ── 1. the binary ────────────────────────────────────────────────────
SRC=""
PROVENANCE=""
if [ -n "$FILE" ]; then
	[ -f "$FILE" ] || die "--file not found: $FILE"
	SRC="$FILE"
	if [ -f "$FILE.sig" ] && command -v minisign >/dev/null 2>&1; then
		cp "$FILE.sig" "$TMP/local.sig"
		verify_minisign "$FILE" "$TMP/local.sig" || die "MINISIGN VERIFICATION FAILED for $FILE (with $FILE.sig) — refusing to install"
		PROVENANCE="local file $FILE (minisign verified)"
	else
		PROVENANCE="local file $FILE (operator-supplied, not signature-checked)"
	fi
else
	if [ -z "$VERSION" ]; then
		VERSION="$(detect_version)" || die "could not find the daemon version on this box (no running daemon for user '$RUN_USER', no k2-daemon binary). Pass --version <x.y.z> — the same version as the daemon."
		log "daemon version on this box: $VERSION"
	fi
	command -v minisign >/dev/null 2>&1 || die "minisign is required to verify the helper (apt-get install minisign) — refusing to install an unverified root binary"
	command -v curl >/dev/null 2>&1 || die "curl is required"
	ASSET="k2-mail-helper-linux-${ARCH}"
	URL="${RELEASE_BASE}/v${VERSION}/${ASSET}"
	log "downloading ${ASSET} (v${VERSION})"
	fetch "$URL" "$TMP/$ASSET" || die "download failed: $URL — release assets for the helper exist from v0.44.1. Update the daemon (k2 update) or pass --version / --file."
	fetch "$URL.sig" "$TMP/$ASSET.sig" || die "download failed: $URL.sig"
	fetch "$URL.sha256" "$TMP/$ASSET.sha256" || die "download failed: $URL.sha256"
	verify_minisign "$TMP/$ASSET" "$TMP/$ASSET.sig" || die "MINISIGN VERIFICATION FAILED for $URL — refusing to install"
	WANT_SHA="$(awk '{print $1; exit}' "$TMP/$ASSET.sha256")"
	GOT_SHA="$(sha_of "$TMP/$ASSET")"
	[ -n "$WANT_SHA" ] && [ "$WANT_SHA" = "$GOT_SHA" ] || die "SHA256 MISMATCH for $URL (expected '$WANT_SHA', got '$GOT_SHA') — refusing to install"
	SRC="$TMP/$ASSET"
	PROVENANCE="release v${VERSION} ${ASSET} (minisign + sha256 verified)"
fi
is_elf "$SRC" || die "$SRC is not an ELF binary — refusing to install"
NEW_SHA="$(sha_of "$SRC")"

# The sudoers candidate is built and visudo-checked BEFORE anything is
# installed, so a rejection leaves the box exactly as it was.
CANDIDATE="$TMP/k2-mail-helper.sudoers"
printf '%s ALL=(root) NOPASSWD: %s\nDefaults:%s !requiretty\n' \
	"$RUN_USER" "$HELPER_PATH" "$RUN_USER" >"$CANDIDATE"
chmod 0440 "$CANDIDATE"
visudo -cf "$CANDIDATE" >/dev/null || die "visudo rejected the sudoers candidate — nothing was installed or written"

# ── 2. /usr/local/libexec/k2-mail-helper (root:root 0755) ───────────
HELPER_RESULT="unchanged"
if [ -f "$HELPER_DEST" ] && [ ! -L "$HELPER_DEST" ] \
	&& [ "$(sha_of "$HELPER_DEST")" = "$NEW_SHA" ] \
	&& [ "$(stat -c '%U:%G %a' "$HELPER_DEST")" = "root:root 755" ]; then
	:
else
	# Create only when absent — never re-mode an existing directory.
	[ -d "$(dirname "$HELPER_DEST")" ] || install -d -m 0755 "$(dirname "$HELPER_DEST")"
	STAGED_HELPER="$(dirname "$HELPER_DEST")/.k2-mail-helper.new.$$"
	install -m 0755 -o root -g root "$SRC" "$STAGED_HELPER"
	# Rename, not copy: atomic, and never "Text file busy" on a running helper.
	mv -f "$STAGED_HELPER" "$HELPER_DEST"
	STAGED_HELPER=""
	HELPER_RESULT="installed"
fi

# ── 3. /etc/sudoers.d/k2-mail-helper (0440, visudo-checked) ─────────
# Exactly the two candidate lines. Never edits /etc/sudoers.d/k2-pg-helper.
SUDOERS_RESULT="unchanged"
if [ -f "$SUDOERS_DEST" ] && [ ! -L "$SUDOERS_DEST" ] \
	&& cmp -s "$CANDIDATE" "$SUDOERS_DEST" \
	&& [ "$(stat -c '%U:%G %a' "$SUDOERS_DEST")" = "root:root 440" ]; then
	:
else
	[ -d "$(dirname "$SUDOERS_DEST")" ] || install -d -m 0750 "$(dirname "$SUDOERS_DEST")"
	# sudo ignores sudoers.d names containing '.', so the staged file is
	# inert until the rename.
	STAGED_SUDOERS="$(dirname "$SUDOERS_DEST")/.k2-mail-helper.new.$$"
	install -m 0440 -o root -g root "$CANDIDATE" "$STAGED_SUDOERS"
	if ! visudo -cf "$STAGED_SUDOERS" >/dev/null; then
		die "visudo rejected the staged sudoers file — removed, nothing written to $SUDOERS_PATH"
	fi
	mv -f "$STAGED_SUDOERS" "$SUDOERS_DEST"
	STAGED_SUDOERS=""
	SUDOERS_RESULT="written"
fi

# ── 4. check + summary ──────────────────────────────────────────────
INCLUDE_NOTE=""
if [ -z "$DEST_ROOT" ] && [ -f /etc/sudoers ] \
	&& ! grep -Eq '^[[:space:]]*[#@]includedir[[:space:]]+/etc/sudoers\.d' /etc/sudoers; then
	INCLUDE_NOTE="WARNING: /etc/sudoers has no includedir /etc/sudoers.d — the line above is not read"
fi
if ! id "$RUN_USER" >/dev/null 2>&1; then
	SUDO_CHECK="user '$RUN_USER' does not exist yet (the line applies once it does)"
elif [ -z "$DEST_ROOT" ] && sudo -l -U "$RUN_USER" "$HELPER_PATH" >/dev/null 2>&1; then
	SUDO_CHECK="$RUN_USER may run the helper without a password"
elif [ -n "$DEST_ROOT" ]; then
	SUDO_CHECK="skipped (K2_INSTALL_ROOT=$DEST_ROOT)"
else
	SUDO_CHECK="WARNING: sudo -l -U $RUN_USER does not list the helper — check /etc/sudoers"
fi

echo
echo "k2-mail-helper:"
echo "  source  : $PROVENANCE"
echo "  helper  : $HELPER_PATH  $HELPER_RESULT  (root:root 0755, sha256 ${NEW_SHA:0:16}…)"
echo "  sudoers : $SUDOERS_PATH  $SUDOERS_RESULT  (0440, visudo ok)"
echo "  sudo    : $SUDO_CHECK"
[ -z "$INCLUDE_NOTE" ] || echo "  $INCLUDE_NOTE"
echo "  next    : as $RUN_USER (or its agent): k2 hostmail status  →  helper: installed"
