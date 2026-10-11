#!/usr/bin/env bash
# install-root-helper.sh — install K2's one root door on a Linux box
# (prd-k2-root-helper-v1 §11). Run ONCE per box, AS ROOT, by ops or the
# owner — never inside a K2 terminal. After this the daemon keeps the
# helper current itself through signed self-updates (no more root steps).
#
#   curl -fsSL https://github.com/Alakazam-211/K2/releases/download/v<version>/install-root-helper.sh \
#     | sudo bash -s -- --version <version> [--user k2] [--unit k2-daemon.service] [--scope system]
#
#   sudo bash install-root-helper.sh --version 0.45.5          # the daemon's version
#   sudo bash install-root-helper.sh --file ./k2-root-helper   # a local build
#
# What it does (and nothing else):
#   1. Gets k2-root-helper for this arch from the release of <version>
#      (k2-root-helper-linux-<arch> + .sig + .sha256) and verifies the
#      minisign signature against the ROOT-HELPER keys (never the app
#      updater key), the exact trusted comment
#      `file:k2-root-helper-linux-<arch> version:<v> protocol:<p>`, then
#      the sha256. Or takes --file (a .sig beside it is checked the same
#      way; without one it warns: operator-supplied).
#   2. Checks the binary: ELF, exactly one build marker, marker == comment,
#      protocol >= 3. Refuses a version older than the installed root
#      helper unless --allow-downgrade. Never executes the new binary as
#      root, and never executes anything the daemon user can write.
#   3. Installs /usr/local/libexec/k2-root-helper (root:root 0755,
#      stage + rename).
#   4. Writes /etc/k2-root-helper/daemon.toml (root:root 0644): which
#      user/unit/scope is the daemon (the helper's caller check).
#   5. Writes /etc/sudoers.d/k2-root-helper (0440) with exactly:
#        <user> ALL=(root) NOPASSWD: /usr/local/libexec/k2-root-helper
#        Defaults:<user> !requiretty
#      validated with `visudo -cf` BEFORE it moves into place.
#   6. Once the daemon user can run it (`sudo -n -l` + a real `helper
#      version` as that user): retires the legacy pg helper right away
#      (/usr/local/libexec/k2-pg-helper + its sudoers file; closes the old
#      `su -c` hole) and runs `visudo -c`. The legacy k2-mail-helper stays
#      one release train (a daemon rolled back to 0.45.x still works);
#      --retire-legacy-mail removes it now.
#
# It never touches Stalwart or Postgres, never restarts anything, never
# edits any other sudoers file. Idempotent.
#
# Environment (rarely needed):
#   K2_RUN_USER      daemon user (default k2; same as --user)
#   K2_RELEASE_BASE  release download base (file:// works for mirrors)
#   K2_INSTALL_ROOT  DESTDIR-style prefix for the hermetic tests only
set -euo pipefail

# Root-helper signing public keys (minisign key lines). Identical to
# crates/k2-root-proto/src/keys.rs RELEASE_KEYS (a daemon test checks).
# Empty until the key ceremony: then every download is refused.
K2_ROOT_HELPER_PUBKEYS=()

RELEASE_BASE="${K2_RELEASE_BASE:-https://github.com/Alakazam-211/K2/releases/download}"
RUN_USER="${K2_RUN_USER:-k2}"
DEST_ROOT="${K2_INSTALL_ROOT:-}"
# Hermetic tests only (ignored unless K2_INSTALL_ROOT is set): extra
# trusted keys, e.g. the TEST key.
if [ -n "$DEST_ROOT" ] && [ -n "${K2_ROOT_HELPER_TEST_PUBKEYS:-}" ]; then
	# shellcheck disable=SC2206
	K2_ROOT_HELPER_PUBKEYS+=(${K2_ROOT_HELPER_TEST_PUBKEYS})
fi
HELPER_PATH="/usr/local/libexec/k2-root-helper"
SUDOERS_PATH="/etc/sudoers.d/k2-root-helper"
CONFIG_DIR="/etc/k2-root-helper"
DAEMON_TOML="/etc/k2-root-helper/daemon.toml"
LEGACY_PG_HELPER="/usr/local/libexec/k2-pg-helper"
LEGACY_PG_SUDOERS="/etc/sudoers.d/k2-pg-helper"
LEGACY_MAIL_HELPER="/usr/local/libexec/k2-mail-helper"
LEGACY_MAIL_SUDOERS="/etc/sudoers.d/k2-mail-helper"
HELPER_DEST="${DEST_ROOT}${HELPER_PATH}"
SUDOERS_DEST="${DEST_ROOT}${SUDOERS_PATH}"
CONFIG_DEST="${DEST_ROOT}${CONFIG_DIR}"
TOML_DEST="${DEST_ROOT}${DAEMON_TOML}"
MIN_PROTOCOL=3

VERSION=""
FILE=""
UNIT="k2-daemon.service"
SCOPE="system"
ALLOW_DOWNGRADE=0
RETIRE_LEGACY_MAIL=0

usage() {
	cat <<'EOF'
Usage: install-root-helper.sh (--version <x.y.z> | --file <path>)
                              [--user <name>] [--unit <unit>] [--scope system|user]
                              [--allow-downgrade] [--retire-legacy-mail]

Run as root (an admin SSH session, not a K2 terminal). Installs
/usr/local/libexec/k2-root-helper (root:root 0755), /etc/k2-root-helper/
daemon.toml (0644) and /etc/sudoers.d/k2-root-helper (0440, visudo-checked).

  --version <v>        release to fetch: the daemon's version, plain x.y.z
  --file <path>        install this local binary instead of downloading
  --user <name>        daemon user (default: k2)
  --unit <unit>        the daemon's systemd unit (default: k2-daemon.service)
  --scope system|user  system unit (provisioned boxes) or systemd --user
                       (self-hosted install-daemon.sh). Default: system
  --allow-downgrade    install a version older than the installed helper
  --retire-legacy-mail also remove the legacy k2-mail-helper now
EOF
}

log() { printf '[install-root-helper] %s\n' "$*"; }
die() { printf '[install-root-helper] ERROR: %s\n' "$*" >&2; exit 1; }
need_arg() { [ "$1" -ge 2 ] || die "$2 needs a value"; }

while [ $# -gt 0 ]; do
	case "$1" in
		--version) need_arg $# "$1"; VERSION="$2"; shift 2 ;;
		--version=*) VERSION="${1#--version=}"; shift ;;
		--file) need_arg $# "$1"; FILE="$2"; shift 2 ;;
		--file=*) FILE="${1#--file=}"; shift ;;
		--user) need_arg $# "$1"; RUN_USER="$2"; shift 2 ;;
		--user=*) RUN_USER="${1#--user=}"; shift ;;
		--unit) need_arg $# "$1"; UNIT="$2"; shift 2 ;;
		--unit=*) UNIT="${1#--unit=}"; shift ;;
		--scope) need_arg $# "$1"; SCOPE="$2"; shift 2 ;;
		--scope=*) SCOPE="${1#--scope=}"; shift ;;
		--allow-downgrade) ALLOW_DOWNGRADE=1; shift ;;
		--retire-legacy-mail) RETIRE_LEGACY_MAIL=1; shift ;;
		-h|--help) usage; exit 0 ;;
		*) usage >&2; die "unknown argument: $1" ;;
	esac
done

SEMVER_RE='^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$'
[ -z "$VERSION" ] || [ -z "$FILE" ] || die "pass --version or --file, not both"
[ -n "$VERSION" ] || [ -n "$FILE" ] || die "pass --version <x.y.z> (the daemon's version: 'k2 root-helper status' or the release you installed) or --file <path>. This script never runs the daemon's own binary to find it."
VERSION="${VERSION#v}"
if [ -n "$VERSION" ] && ! [[ "$VERSION" =~ $SEMVER_RE ]]; then
	die "--version must look like 0.45.5 (got '$VERSION')"
fi
# These land in sudoers and daemon.toml: plain names only.
[[ "$RUN_USER" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || die "--user must be a plain user name (got '$RUN_USER')"
case "$UNIT" in *.service) ;; *) UNIT="${UNIT}.service" ;; esac
[[ "$UNIT" =~ ^[A-Za-z0-9@._-]{1,64}\.service$ ]] || die "--unit must be a systemd unit name (got '$UNIT')"
case "$SCOPE" in system|user) ;; *) die "--scope must be system or user (got '$SCOPE')" ;; esac

[ "$(id -u)" = "0" ] || die "run as root (sudo bash install-root-helper.sh ...)"
[ "$(uname -s)" = "Linux" ] || die "the root helper is Linux-only; this is $(uname -s)"
case "$(uname -m)" in
	x86_64|amd64) ARCH="x86_64" ;;
	aarch64|arm64) ARCH="aarch64" ;;
	*) die "unsupported CPU arch: $(uname -m) (expected x86_64 or aarch64)" ;;
esac

command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required (coreutils)"
command -v sudo >/dev/null 2>&1 || die "sudo is not installed — the daemon reaches the helper only through sudo -n. Install it (apt-get install sudo) and re-run."
command -v visudo >/dev/null 2>&1 || die "visudo is not installed — it validates the sudoers file before it goes live. Install sudo and re-run."

TMP="$(mktemp -d "${TMPDIR:-/tmp}/k2-root-helper.XXXXXX")" || die "mktemp failed"
STAGED=()
cleanup() {
	for f in "${STAGED[@]:-}"; do [ -z "$f" ] || rm -f "$f"; done
	rm -rf "$TMP"
}
trap cleanup EXIT

sha_of() { sha256sum "$1" | awk '{print $1}'; }
fetch() { curl -fsSL --retry 3 --retry-delay 2 -o "$2" "$1"; }
is_elf() { [ "$(head -c 4 "$1" | od -An -c | tr -d ' \n')" = '177ELF' ]; }

# All build markers in a binary, one per line: "<version> <protocol> <arch>".
# Never executes the file.
markers_of() {
	LC_ALL=C grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]* proto=[0-9]* arch=[a-z0-9_]*;' "$1" \
		| sed 's/^K2_ROOT_HELPER_BUILD v=\([0-9.]*\) proto=\([0-9]*\) arch=\([a-z0-9_]*\);$/\1 \2 \3/' || true
}

# ver_lt a b: a < b (plain x.y.z).
ver_lt() {
	local a1 a2 a3 b1 b2 b3
	IFS=. read -r a1 a2 a3 <<<"$1"
	IFS=. read -r b1 b2 b3 <<<"$2"
	[ "$a1" -lt "$b1" ] && return 0; [ "$a1" -gt "$b1" ] && return 1
	[ "$a2" -lt "$b2" ] && return 0; [ "$a2" -gt "$b2" ] && return 1
	[ "$a3" -lt "$b3" ]
}

# verify_sig <file> <sig> → prints the trusted comment when one of the
# root-helper keys verifies it (signature AND the global signature over the
# comment); fails otherwise.
verify_sig() {
	local f="$1" sig="$2" pub="$TMP/k.pub" key out
	command -v minisign >/dev/null 2>&1 || die "minisign is required to verify the helper (apt-get install minisign) — refusing to install an unverified root binary"
	[ "${#K2_ROOT_HELPER_PUBKEYS[@]}" -gt 0 ] || die "this installer carries no root-helper signing keys yet (key ceremony pending) — use --file with a build you trust"
	for key in "${K2_ROOT_HELPER_PUBKEYS[@]}"; do
		printf 'untrusted comment: k2 root helper key\n%s\n' "$key" >"$pub"
		if out="$(minisign -V -m "$f" -p "$pub" -x "$sig" -Q 2>/dev/null)"; then
			printf '%s\n' "$out" | tail -n1
			return 0
		fi
	done
	return 1
}

# ── 1. the binary ────────────────────────────────────────────────────
SRC=""
PROVENANCE=""
WANT_COMMENT_VERSION=""
if [ -n "$FILE" ]; then
	[ -f "$FILE" ] || die "--file not found: $FILE"
	# Copy into root's own temp dir FIRST and check/install only that copy:
	# a --file the daemon user can write must not be swapped between the
	# checks and the install (security review #4).
	cp -- "$FILE" "$TMP/candidate"
	SRC="$TMP/candidate"
	if [ -f "$FILE.sig" ]; then
		cp -- "$FILE.sig" "$TMP/candidate.sig"
		COMMENT="$(verify_sig "$SRC" "$TMP/candidate.sig")" || die "MINISIGN VERIFICATION FAILED for $FILE (with $FILE.sig) — refusing to install"
		PROVENANCE="local file $FILE (minisign verified: $COMMENT)"
	else
		COMMENT=""
		PROVENANCE="local file $FILE (operator-supplied, NOT signature-checked)"
		log "WARNING: $FILE has no .sig — installing an operator-supplied build"
	fi
else
	command -v curl >/dev/null 2>&1 || die "curl is required"
	ASSET="k2-root-helper-linux-${ARCH}"
	URL="${RELEASE_BASE}/v${VERSION}/${ASSET}"
	log "downloading ${ASSET} (v${VERSION})"
	fetch "$URL" "$TMP/$ASSET" || die "download failed: $URL — the root helper ships from v0.45.5. Update the daemon (k2 update) or pass --version / --file."
	fetch "$URL.sig" "$TMP/$ASSET.sig" || die "download failed: $URL.sig"
	fetch "$URL.sha256" "$TMP/$ASSET.sha256" || die "download failed: $URL.sha256"
	COMMENT="$(verify_sig "$TMP/$ASSET" "$TMP/$ASSET.sig")" || die "MINISIGN VERIFICATION FAILED for $URL — refusing to install"
	WANT_SHA="$(awk '{print $1; exit}' "$TMP/$ASSET.sha256")"
	GOT_SHA="$(sha_of "$TMP/$ASSET")"
	[ -n "$WANT_SHA" ] && [ "$WANT_SHA" = "$GOT_SHA" ] || die "SHA256 MISMATCH for $URL (expected '$WANT_SHA', got '$GOT_SHA') — refusing to install"
	SRC="$TMP/$ASSET"
	PROVENANCE="release v${VERSION} ${ASSET} (minisign + sha256 verified)"
	WANT_COMMENT_VERSION="$VERSION"
fi
if [ -n "${COMMENT:-}" ]; then
	# Exactly: file:k2-root-helper-linux-<arch> version:<v> protocol:<p>
	[[ "$COMMENT" =~ ^file:k2-root-helper-linux-${ARCH}\ version:([0-9.]+)\ protocol:([0-9]+)$ ]] \
		|| die "the signature's trusted comment is not for k2-root-helper-linux-${ARCH} (got '$COMMENT') — refusing"
	C_VERSION="${BASH_REMATCH[1]}"
	C_PROTOCOL="${BASH_REMATCH[2]}"
	[ -z "$WANT_COMMENT_VERSION" ] || [ "$C_VERSION" = "$WANT_COMMENT_VERSION" ] \
		|| die "the signed helper is v$C_VERSION, not v$WANT_COMMENT_VERSION — refusing"
fi
is_elf "$SRC" || die "$SRC is not an ELF binary — refusing to install"
MARKERS="$(markers_of "$SRC")"
[ "$(printf '%s\n' "$MARKERS" | grep -c . || true)" = "1" ] || die "$SRC does not carry exactly one k2-root-helper build marker — refusing"
read -r NEW_VERSION NEW_PROTOCOL NEW_ARCH <<<"$MARKERS"
[[ "$NEW_VERSION" =~ $SEMVER_RE ]] || die "build marker version '$NEW_VERSION' is not x.y.z — refusing"
[ "$NEW_ARCH" = "$ARCH" ] || die "$SRC is built for $NEW_ARCH, this box is $ARCH — refusing"
[ "$NEW_PROTOCOL" -ge "$MIN_PROTOCOL" ] || die "$SRC speaks protocol $NEW_PROTOCOL (< $MIN_PROTOCOL) — refusing"
if [ -n "${COMMENT:-}" ]; then
	[ "$NEW_VERSION" = "$C_VERSION" ] && [ "$NEW_PROTOCOL" = "$C_PROTOCOL" ] \
		|| die "build marker ($NEW_VERSION/$NEW_PROTOCOL) does not match the signed comment ($C_VERSION/$C_PROTOCOL) — refusing"
fi
[ -z "$VERSION" ] || [ "$NEW_VERSION" = "$VERSION" ] || die "the build is v$NEW_VERSION, --version says v$VERSION — refusing"
if LC_ALL=C grep -a -q 'K2_ROOT_HELPER_TEST_KEYS_ENABLED' "$SRC"; then
	if [ -z "$DEST_ROOT" ] && [ "${K2_ALLOW_TEST_KEYS_BUILD:-0}" != "1" ]; then
		die "$SRC is a TEST-KEYS build (trusts the public test key) — never on a real box"
	fi
	log "WARNING: installing a TEST-KEYS build (test environment)"
fi
NEW_SHA="$(sha_of "$SRC")"

# Anti-downgrade against the installed root helper (read its marker; never run it).
OLD_VERSION=""
if [ -f "$HELPER_DEST" ] && [ ! -L "$HELPER_DEST" ]; then
	read -r OLD_VERSION _ _ <<<"$(markers_of "$HELPER_DEST" | head -n1)"
	if [ -n "$OLD_VERSION" ] && ver_lt "$NEW_VERSION" "$OLD_VERSION" && [ "$ALLOW_DOWNGRADE" != 1 ]; then
		die "the installed root helper is v$OLD_VERSION; v$NEW_VERSION is older. Pass --allow-downgrade to install it anyway."
	fi
fi

# ── 2. candidates, checked before anything is installed ─────────────
CANDIDATE="$TMP/k2-root-helper.sudoers"
printf '%s ALL=(root) NOPASSWD: %s\nDefaults:%s !requiretty\n' \
	"$RUN_USER" "$HELPER_PATH" "$RUN_USER" >"$CANDIDATE"
chmod 0440 "$CANDIDATE"
visudo -cf "$CANDIDATE" >/dev/null || die "visudo rejected the sudoers candidate — nothing was installed or written"
TOML_CANDIDATE="$TMP/daemon.toml"
cat >"$TOML_CANDIDATE" <<EOF
# Written by install-root-helper.sh. Root-owned: only root may change who
# the K2 daemon is (the helper's caller check, prd-k2-root-helper-v1 §6).
[[daemon]]
user = "${RUN_USER}"
unit = "${UNIT}"
scope = "${SCOPE}"
EOF

# ── 3. /usr/local/libexec/k2-root-helper (root:root 0755) ───────────
HELPER_RESULT="unchanged"
if [ -f "$HELPER_DEST" ] && [ ! -L "$HELPER_DEST" ] \
	&& [ "$(sha_of "$HELPER_DEST")" = "$NEW_SHA" ] \
	&& [ "$(stat -c '%U:%G %a' "$HELPER_DEST")" = "root:root 755" ]; then
	:
else
	[ ! -L "$HELPER_DEST" ] || die "$HELPER_PATH is a symlink — refusing (remove it by hand)"
	[ -d "$(dirname "$HELPER_DEST")" ] || install -d -m 0755 "$(dirname "$HELPER_DEST")"
	STAGE="$(dirname "$HELPER_DEST")/.k2-root-helper.install.$$"
	STAGED+=("$STAGE")
	install -m 0755 -o root -g root "$SRC" "$STAGE"
	mv -f "$STAGE" "$HELPER_DEST"
	HELPER_RESULT="installed v${NEW_VERSION}${OLD_VERSION:+ (was v$OLD_VERSION)}"
fi

# ── 4. /etc/k2-root-helper/daemon.toml (root:root 0644) ─────────────
TOML_RESULT="unchanged"
[ -d "$CONFIG_DEST" ] || install -d -m 0755 "$CONFIG_DEST"
[ ! -L "$CONFIG_DEST" ] && [ ! -L "$TOML_DEST" ] || die "$CONFIG_DIR or $DAEMON_TOML is a symlink — refusing"
if ! { [ -f "$TOML_DEST" ] && cmp -s "$TOML_CANDIDATE" "$TOML_DEST"; }; then
	STAGE="$CONFIG_DEST/.daemon.toml.new.$$"
	STAGED+=("$STAGE")
	install -m 0644 -o root -g root "$TOML_CANDIDATE" "$STAGE"
	mv -f "$STAGE" "$TOML_DEST"
	TOML_RESULT="written (user=$RUN_USER unit=$UNIT scope=$SCOPE)"
fi

# ── 5. /etc/sudoers.d/k2-root-helper (0440, visudo-checked) ─────────
SUDOERS_RESULT="unchanged"
if [ -f "$SUDOERS_DEST" ] && [ ! -L "$SUDOERS_DEST" ] \
	&& cmp -s "$CANDIDATE" "$SUDOERS_DEST" \
	&& [ "$(stat -c '%U:%G %a' "$SUDOERS_DEST")" = "root:root 440" ]; then
	:
else
	[ -d "$(dirname "$SUDOERS_DEST")" ] || install -d -m 0750 "$(dirname "$SUDOERS_DEST")"
	# sudo ignores sudoers.d names containing '.', so the stage is inert.
	STAGE="$(dirname "$SUDOERS_DEST")/.k2-root-helper.new.$$"
	STAGED+=("$STAGE")
	install -m 0440 -o root -g root "$CANDIDATE" "$STAGE"
	visudo -cf "$STAGE" >/dev/null || die "visudo rejected the staged sudoers file — removed, nothing written to $SUDOERS_PATH"
	mv -f "$STAGE" "$SUDOERS_DEST"
	SUDOERS_RESULT="written"
fi

# ── 6. check as the daemon user, then retire legacy helpers ─────────
CHECK="skipped"
DOOR_OK=0
if [ -n "$DEST_ROOT" ]; then
	CHECK="skipped (K2_INSTALL_ROOT=$DEST_ROOT)"
	DOOR_OK="${K2_TEST_DOOR_OK:-1}"
elif ! id "$RUN_USER" >/dev/null 2>&1; then
	CHECK="user '$RUN_USER' does not exist yet (the line applies once it does); legacy helpers left in place"
elif ANSWER="$(runuser -u "$RUN_USER" -- sudo -n "$HELPER_PATH" helper version 2>/dev/null)" \
	&& P="$(printf '%s' "$ANSWER" | grep -o '"protocol":[0-9]*' | head -n1 | cut -d: -f2)" \
	&& [ -n "$P" ] && [ "$P" -ge "$MIN_PROTOCOL" ]; then
	CHECK="$RUN_USER may run it: $(printf '%s' "$ANSWER" | grep -o '"version":"[0-9.]*"' | head -n1)"
	DOOR_OK=1
else
	CHECK="WARNING: '$RUN_USER' cannot run 'sudo -n $HELPER_PATH helper version' — check /etc/sudoers (includedir?) — legacy helpers left in place"
fi

retire() {
	# retire <bin> <sudoers>: remove the fixed legacy files (regular files only).
	local removed=""
	for p in "$2" "$1"; do
		local d="${DEST_ROOT}$p"
		if [ -L "$d" ]; then
			log "WARNING: $p is a symlink — left alone"
		elif [ -f "$d" ]; then
			rm -f "$d" && removed="$removed $p"
		fi
	done
	printf '%s' "${removed:- (none)}"
}
PG_RESULT="kept"
MAIL_RESULT="kept one release train (daemon retires it from 0.46.0; --retire-legacy-mail to remove now)"
if [ "$DOOR_OK" = 1 ]; then
	PG_RESULT="retired:$(retire "$LEGACY_PG_HELPER" "$LEGACY_PG_SUDOERS")"
	if [ "$RETIRE_LEGACY_MAIL" = 1 ]; then
		MAIL_RESULT="retired:$(retire "$LEGACY_MAIL_HELPER" "$LEGACY_MAIL_SUDOERS")"
	fi
	if [ -z "$DEST_ROOT" ]; then
		visudo -c >/dev/null 2>&1 || die "visudo -c fails after retiring the legacy helpers — check /etc/sudoers.d"
	else
		visudo -c >/dev/null
	fi
fi

INCLUDE_NOTE=""
if [ -z "$DEST_ROOT" ] && [ -f /etc/sudoers ] \
	&& ! grep -Eq '^[[:space:]]*[#@]includedir[[:space:]]+/etc/sudoers\.d' /etc/sudoers; then
	INCLUDE_NOTE="WARNING: /etc/sudoers has no includedir /etc/sudoers.d — the line above is not read"
fi

echo
echo "k2-root-helper:"
echo "  source      : $PROVENANCE"
echo "  helper      : $HELPER_PATH  $HELPER_RESULT  (root:root 0755, sha256 ${NEW_SHA:0:16}…, protocol $NEW_PROTOCOL)"
echo "  daemon.toml : $DAEMON_TOML  $TOML_RESULT"
echo "  sudoers     : $SUDOERS_PATH  $SUDOERS_RESULT  (0440, visudo ok)"
echo "  check       : $CHECK"
echo "  legacy pg   : $PG_RESULT"
echo "  legacy mail : $MAIL_RESULT"
[ -z "$INCLUDE_NOTE" ] || echo "  $INCLUDE_NOTE"
echo "  next        : as $RUN_USER: k2 root-helper status  →  state: root"
