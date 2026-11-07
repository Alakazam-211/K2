#!/bin/bash
# install-k2-signer.sh — install k2-signer beside k2sign on the signing Mac
# (prd-ci-on-compute-for-clients-v1 C4, vs-live V28: the same `_k2sign` role
# user, sharing its keys, sessions and registries).
#
#   bash scripts/k2sign/install-k2-signer.sh --binary <k2-signer> --dry-run   # anyone: checks + the plan
#   sudo bash scripts/k2sign/install-k2-signer.sh --binary <k2-signer>        # from a REVIEWED commit
#   K2_SIGNER_TEST_ROOT=<dir> bash scripts/k2sign/install-k2-signer.sh --test --binary <k2-signer>   # tests
#
# <k2-signer> is `cargo build --release --locked -p k2-signer` of THIS commit
# (target/release/k2-signer), built by the release user. The installer prints
# its sha256 next to the commit; compare it with a second build.
#
# What it does (idempotent; a re-run upgrades the binary and re-pins):
#   1. checks: macOS, root, k2sign installed (the `_k2sign` user and its
#      /var/k2sign, 0700), the binary is an arm64 Mach-O, no pam_tid for sudo;
#   2. /usr/local/libexec/k2-signer and /usr/local/etc/k2-signer{,/projects}
#      (root:wheel 0755), /var/k2sign/k2-signer (_k2sign 0700);
#   3. the binary (k2-signer-bin) and the wrappers k2-signer, k2-signer-admin
#      (root:wheel 0755; env -i, fixed PATH/HOME/umask, no caller paths);
#   4. /etc/sudoers.d/k2-signer, checked with visudo -cf:
#        <release user> ALL=(_k2sign) NOPASSWD: /usr/local/libexec/k2-signer/k2-signer
#   5. pins.sha256 over all of it (k2-signer-admin repin), then every hash.
# It reads, makes and moves no key and no policy: those are k2-signer-admin
# steps a person takes (adopt-k2sign, pair-begin/-accept, trust-owner,
# policy-approve, roster-accept, generate-key / import-key).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TREE="$(cd "$HERE/../.." && pwd)"

MODE=install DRY=0 RELEASE_USER="" BINARY=""
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY=1; shift ;;
        --test) MODE=test; shift ;;
        --binary) BINARY="${2:?--binary <path>}"; shift 2 ;;
        --release-user) RELEASE_USER="${2:?--release-user <name>}"; shift 2 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "install-k2-signer.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

die() { printf '\033[1;31minstall-k2-signer.sh: ERROR: %s\033[0m\n' "$*" >&2; exit 1; }
say() { printf 'install-k2-signer.sh: %s\n' "$*"; }
act() {
    if [ "$DRY" = 1 ]; then printf '  would run: %s\n' "$*"; else "$@"; fi
}
sha256() { /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'; }

[ "$(uname -s)" = Darwin ] || die "macOS only (the signer's v1 platform; Linux is the next slice)"
[ -n "$BINARY" ] && [ -f "$BINARY" ] || die "--binary <target/release/k2-signer built from this commit>"
ROOT=""
if [ "$MODE" = test ]; then
    ROOT="${K2_SIGNER_TEST_ROOT:-}"
    [ -n "$ROOT" ] && [ -d "$ROOT" ] || die "--test needs K2_SIGNER_TEST_ROOT=<existing dir>"
    [ "$(id -u)" != 0 ] || die "--test is refused as root"
else
    [ -z "${K2_SIGNER_TEST_ROOT:-}" ] || die "K2_SIGNER_TEST_ROOT is set: that is for --test only"
    if [ "$DRY" = 0 ] && [ "$(id -u)" != 0 ]; then die "run with sudo (or --dry-run to see the plan first)"; fi
fi
LIBEXEC="$ROOT/usr/local/libexec/k2-signer"
ETC="$ROOT/usr/local/etc/k2-signer"
VAR="$ROOT/var/k2sign"
SUDOERS_DIR="$ROOT/etc/sudoers.d"
[ "$MODE" = test ] || SUDOERS_DIR="/private/etc/sudoers.d"
if [ "$MODE" = test ]; then
    OWN_ROOT=() OWN_SIGNER=()
else
    OWN_ROOT=(-o root -g wheel) OWN_SIGNER=(-o _k2sign -g _k2sign)
fi

# ── 1. checks ──────────────────────────────────────────────────────────────
say "checking (read-only)..."
/usr/bin/file "$BINARY" | grep -q 'Mach-O 64-bit executable arm64' || die "$BINARY is not an arm64 Mach-O executable"
BIN_SHA="$(sha256 "$BINARY")"
COMMIT="$(cd "$TREE" && git -c safe.directory='*' rev-parse HEAD 2>/dev/null || echo unknown)"
DIRTY="$(cd "$TREE" && git -c safe.directory='*' status --porcelain -- crates/k2-signer crates/k2-node-proto Cargo.lock 2>/dev/null | head -1 || true)"
[ -z "$DIRTY" ] || die "the tree has uncommitted changes under crates/k2-signer, crates/k2-node-proto or Cargo.lock: install from a reviewed commit"
say "  binary sha256 $BIN_SHA (commit $COMMIT): compare it with a second build of this commit"
if [ "$MODE" != test ]; then
    /usr/bin/dscl . -read /Users/_k2sign >/dev/null 2>&1 || die "no _k2sign user: install k2sign first (sudo bash scripts/k2sign/install.sh)"
    home="$(/usr/bin/dscl . -read /Users/_k2sign NFSHomeDirectory | awk '{print $2}')"
    shell="$(/usr/bin/dscl . -read /Users/_k2sign UserShell | awk '{print $2}')"
    [ "$home" = /var/k2sign ] && [ "$shell" = /usr/bin/false ] || die "_k2sign has other attributes (home $home, shell $shell): not k2sign's role user"
    if [ -f /etc/pam.d/sudo_local ] && grep -Eq '^[[:space:]]*auth[[:space:]].*pam_tid' /etc/pam.d/sudo_local; then
        die "Touch ID for sudo is on (/etc/pam.d/sudo_local has pam_tid): a sudo sheet could pass for a release approval (prd-signing-user A11)"
    fi
    if [ -z "$RELEASE_USER" ]; then RELEASE_USER="${SUDO_USER:-}"; fi
    if [ -z "$RELEASE_USER" ] && [ "$DRY" = 1 ]; then RELEASE_USER="$(id -un)"; fi
    [ -n "$RELEASE_USER" ] && [ "$RELEASE_USER" != root ] || die "run with sudo from the release user's terminal (or --release-user <name>)"
    ruid="$(id -u "$RELEASE_USER" 2>/dev/null)" || die "no user $RELEASE_USER"
    [ "$ruid" -ge 501 ] || die "$RELEASE_USER (uid $ruid) is not a normal login user"
else
    RELEASE_USER="${RELEASE_USER:-$(id -un)}"
fi
if [ "$DRY" = 0 ]; then
    [ -d "$VAR" ] || die "$VAR is missing: install k2sign first"
    [ "$(/usr/bin/stat -f '%Lp' "$VAR")" = 700 ] || die "$VAR must be mode 700"
fi
/usr/sbin/visudo -cf /dev/null >/dev/null || die "visudo does not work"

# ── 2. folders ─────────────────────────────────────────────────────────────
say "folders..."
act /usr/bin/install -d ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$LIBEXEC"
act /usr/bin/install -d ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$ETC"
act /usr/bin/install -d ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$ETC/projects"
act /usr/bin/install -d ${OWN_SIGNER[@]+"${OWN_SIGNER[@]}"} -m 0700 "$VAR/k2-signer"
[ "$MODE" != test ] || act /bin/mkdir -p "$SUDOERS_DIR"

# ── 3. binary and wrappers ─────────────────────────────────────────────────
say "signer files..."
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$BINARY" "$LIBEXEC/k2-signer-bin"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$HERE/k2-signer" "$LIBEXEC/k2-signer"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$HERE/k2-signer-admin" "$LIBEXEC/k2-signer-admin"

# ── 4. sudoers ─────────────────────────────────────────────────────────────
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-signer-install.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
{
    echo "# k2-signer (prd-ci-on-compute-for-clients-v1 C4), written by scripts/k2sign/install-k2-signer.sh."
    echo "# $RELEASE_USER may run ONLY the signer as _k2sign, without a password. Everything"
    echo "# else as _k2sign, and k2-signer-admin, still needs the admin password."
    echo "$RELEASE_USER ALL=(_k2sign) NOPASSWD: /usr/local/libexec/k2-signer/k2-signer"
} > "$WORK/sudoers"
/usr/sbin/visudo -cf "$WORK/sudoers" >/dev/null || die "the sudoers drop-in does not parse"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0440 "$WORK/sudoers" "$SUDOERS_DIR/k2-signer"
if [ "$MODE" != test ] && [ "$DRY" = 0 ]; then
    /usr/sbin/visudo -c >/dev/null || die "sudoers as a whole no longer parses: remove $SUDOERS_DIR/k2-signer NOW (sudo rm $SUDOERS_DIR/k2-signer)"
fi

# ── 5. pins ────────────────────────────────────────────────────────────────
if [ "$DRY" = 1 ]; then
    say "dry run: would pin every file (k2-signer-admin repin) and print the hashes. Nothing was changed."
    exit 0
fi
"$LIBEXEC/k2-signer-admin" repin >/dev/null || die "k2-signer-admin repin failed"
say "installed. Hashes:"
sed 's/^/  /' "$ETC/pins.sha256"
printf '  %s  etc/sudoers.d/k2-signer\n' "$(sha256 "$SUDOERS_DIR/k2-signer")"
say "next, at this Mac (each asks for the admin password):"
say "  sudo $LIBEXEC/k2-signer-admin adopt-k2sign"
say "  sudo $LIBEXEC/k2-signer-admin pair-begin <label>   (then k2 ci signer add <code> on your K2 server, and pair-accept)"
say "  sudo $LIBEXEC/k2-signer-admin trust-owner <roster key fp read on the owner's screen> <ed25519:key> <label>"
say "  sudo $LIBEXEC/k2-signer-admin policy-approve <project> < policy.toml   (shows the diff and its sha256)"
