#!/bin/bash
# install.sh — set up the release signing user on z3mbpZ
# (prd-signing-user-v1 rev 2, S1; §4.1 layout, §4.2 sudoers, §7 sitting 1).
#
#   bash scripts/k2sign/install.sh --dry-run        # anyone: checks + the plan, changes nothing
#   sudo bash scripts/k2sign/install.sh             # Rosson, from a REVIEWED commit
#   K2SIGN_TEST_ROOT=<dir> bash scripts/k2sign/install.sh --test --minisign <path>   # tests only
#
# What it does (idempotent; a re-run upgrades the signer and re-pins):
#   1. checks: macOS, root, a root-owned Xcode/CLT python3 (every file of its
#      framework root-owned, nothing group/world-writable), no pam_tid for
#      sudo (A11), sudoers reads /private/etc/sudoers.d;
#   2. the hidden role user `_k2sign`: a free UID = GID in 400–499, its own
#      group (not staff), shell /usr/bin/false, password "*", home
#      /var/k2sign (0700), not in com.apple.access_ssh. An existing _k2sign
#      with other attributes is refused, never changed;
#   3. /usr/local/libexec/k2sign and /usr/local/etc/k2sign (root:wheel 0755)
#      and /var/k2sign/{keys,state,audit,tmp} (_k2sign 0700);
#   4. minisign 0.12 from jedisct1's GitHub release: the zip's sha256 AND the
#      binary's sha256 must equal the values pinned below; then its own
#      minisign signature is checked against jedisct1's key (a consistency
#      check only: the pinned hashes are the real control);
#   5. the signer (k2sign, k2sign-admin with the python path filled in,
#      k2sign.py), the policy, and pubkeys from THIS tree (U1 always; U2/U3
#      once the ceremony's pubkeys commit is in the tree; refused when the
#      tree's U2 differs from the one generate-u2 made);
#   6. /etc/sudoers.d/k2sign, checked with visudo -cf before it is moved in:
#        <release user> ALL=(_k2sign) NOPASSWD: /usr/local/libexec/k2sign/k2sign
#   7. pins.sha256 over all of it, then prints every hash (K2 compares them
#      with the branch).
# Nothing here reads, makes or moves a key: generate-u2 and import u1 are
# separate k2sign-admin steps (sitting 2).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TREE="$(cd "$HERE/../.." && pwd)"

MINISIGN_ZIP_URL="https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-macos.zip"
MINISIGN_ZIP_SHA256="89000b19535765f9cffc65a65d64a820f433ef6db8020667f7570e06bf6aac63"
MINISIGN_BIN_SHA256="d41cde458303d45c95b00473e2455a7f45f95b550931f1f0cc98ef1f61b2a8ff"
JEDISCT1_MINISIGN_PUB="RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
# gh (S3: the signer verifies Linux build attestations with its own copy).
GH_ZIP_URL="https://github.com/cli/cli/releases/download/v2.83.1/gh_2.83.1_macOS_arm64.zip"
GH_ZIP_SHA256="4461a4f07cfdbeb83f96dce8d3789d5115b22185fb1a6fd3ec7f7befb755db0c"
GH_BIN_SHA256="36a67e2862458ccb817544a8bfd22a399f370d5860b4ae4e7f9d99a980fb4375"
# Only the Command Line Tools python qualifies: /Applications is root:admin
# 0775, so any admin user (z3thon is one) can move Xcode.app aside and put
# another python at Xcode's path. Every folder above the CLT python is
# root:wheel 0755.
PYTHON_CANDIDATES=(
    /Library/Developer/CommandLineTools/usr/bin/python3
)

MODE=install DRY=0 RELEASE_USER="" LOCAL_MINISIGN="" LOCAL_GH=""
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY=1; shift ;;
        --test) MODE=test; shift ;;
        --minisign) LOCAL_MINISIGN="${2:?--minisign <path>}"; shift 2 ;;
        --gh) LOCAL_GH="${2:?--gh <path>}"; shift 2 ;;
        --release-user) RELEASE_USER="${2:?--release-user <name>}"; shift 2 ;;
        -h|--help) sed -n '2,32p' "$0"; exit 0 ;;
        *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

die() { printf '\033[1;31minstall.sh: ERROR: %s\033[0m\n' "$*" >&2; exit 1; }
say() { printf 'install.sh: %s\n' "$*"; }
# Every change goes through act: printed in --dry-run, run otherwise.
act() {
    if [ "$DRY" = 1 ]; then printf '  would run: %s\n' "$*"; else "$@"; fi
}

[ "$(uname -s)" = Darwin ] || die "macOS only (this sets up z3mbpZ)"
ROOT=""
if [ "$MODE" = test ]; then
    ROOT="${K2SIGN_TEST_ROOT:-}"
    [ -n "$ROOT" ] && [ -d "$ROOT" ] || die "--test needs K2SIGN_TEST_ROOT=<existing dir>"
    [ "$(id -u)" != 0 ] || die "--test is refused as root"
    [ -n "$LOCAL_MINISIGN" ] || die "--test needs --minisign <path>"
    [ -n "$LOCAL_GH" ] || die "--test needs --gh <path> (a stub)"
else
    [ -z "${K2SIGN_TEST_ROOT:-}" ] || die "K2SIGN_TEST_ROOT is set: that is for --test only"
    [ -z "$LOCAL_MINISIGN$LOCAL_GH" ] || die "--minisign and --gh are for --test only (production downloads and checks the pinned releases)"
    if [ "$DRY" = 0 ] && [ "$(id -u)" != 0 ]; then
        die "run with sudo (or --dry-run to see the plan first)"
    fi
fi
LIBEXEC="$ROOT/usr/local/libexec/k2sign"
ETC="$ROOT/usr/local/etc/k2sign"
VAR="$ROOT/var/k2sign"
SUDOERS_DIR="$ROOT/etc/sudoers.d"
[ "$MODE" = test ] || SUDOERS_DIR="/private/etc/sudoers.d"

# Owner/group flags for install(1): root/_k2sign in production, the caller in tests.
if [ "$MODE" = test ]; then
    OWN_ROOT=() OWN_SIGNER=()
else
    OWN_ROOT=(-o root -g wheel) OWN_SIGNER=(-o _k2sign -g _k2sign)
fi

sha256() { /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'; }

# ── 1. checks ──────────────────────────────────────────────────────────────
say "checking z3mbpZ (read-only)..."
PY=""
for c in "${PYTHON_CANDIDATES[@]}"; do
    [ -x "$c" ] || continue
    real="$("$c" -I -c 'import os,sys; print(os.path.realpath(sys.executable))')" || continue
    PY="$real"; break
done
[ -n "$PY" ] || die "no Command Line Tools python3 at ${PYTHON_CANDIDATES[0]} (install the Command Line Tools: xcode-select --install; Xcode's own python can't be used, /Applications is admin-writable)"
# Every directory from / to the binary, and every file of the framework,
# must be root-owned and not group/world-writable: the signer runs this
# python as _k2sign, so a z3thon-writable file in it would run as _k2sign.
chain_ok() {
    local p="$1" st
    while :; do
        st="$(/usr/bin/stat -f '%u %Lp' "$p")"
        [ "${st%% *}" = 0 ] || return 1
        case "${st##* }" in *[2367]?|*?[2367]) return 1 ;; esac
        [ "$p" = / ] && return 0
        p="$(dirname "$p")"
    done
}
chain_ok "$PY" || die "$PY (or a folder above it) is not root-owned and root-only writable"
FRAMEWORK="${PY%%/Versions/*}"
[ -d "$FRAMEWORK" ] || die "can't find python's framework folder from $PY"
bad="$(/usr/bin/find "$FRAMEWORK" \( ! -user root -o -perm -0020 -o -perm -0002 \) ! -type l -print 2>/dev/null | head -3)"
[ -z "$bad" ] || die "python's framework has files not root-only: $bad"
say "  python: $PY (root-owned framework $FRAMEWORK)"
"$PY" -I -c 'import fcntl, hashlib, json, secrets' || die "$PY lacks a module the signer needs"
if [ "$MODE" != test ]; then
    # The folders the signer's files and state live in must be root-only too.
    for d in /usr/local /private/var /private/etc/sudoers.d; do
        chain_ok "$d" || die "$d (or a folder above it) is not root-owned and root-only writable"
    done
    if [ -d /usr/local/libexec ]; then chain_ok /usr/local/libexec || die "/usr/local/libexec is not root-only writable"; fi
fi

if [ -f /etc/pam.d/sudo_local ] && grep -Eq '^[[:space:]]*auth[[:space:]].*pam_tid' /etc/pam.d/sudo_local; then
    die "Touch ID for sudo is on (/etc/pam.d/sudo_local has pam_tid). Turn it off first: a sudo Touch ID sheet mid-release could pass for the release approval (prd A11)."
fi
say "  no pam_tid for sudo (A11)"
[ -x /usr/bin/openssl ] || die "/usr/bin/openssl missing"
/usr/sbin/visudo -cf /dev/null >/dev/null || die "visudo does not work"

if [ "$MODE" != test ]; then
    if [ -r /etc/sudoers ]; then
        grep -Eq '^[#@]includedir[[:space:]]+/private/etc/sudoers\.d' /etc/sudoers \
            || die "/etc/sudoers does not include /private/etc/sudoers.d"
    else
        say "  (dry run as a normal user: /etc/sudoers isn't readable; checked when run with sudo)"
    fi
    if [ -z "$RELEASE_USER" ]; then RELEASE_USER="${SUDO_USER:-}"; fi
    if [ -z "$RELEASE_USER" ] && [ "$DRY" = 1 ]; then RELEASE_USER="$(id -un)"; fi
    [ -n "$RELEASE_USER" ] && [ "$RELEASE_USER" != root ] || die "run with sudo from the release user's terminal (or pass --release-user <name>)"
    ruid="$(id -u "$RELEASE_USER" 2>/dev/null)" || die "no user $RELEASE_USER"
    [ "$ruid" -ge 501 ] || die "$RELEASE_USER (uid $ruid) is not a normal login user"
    say "  release user: $RELEASE_USER (uid $ruid)"
else
    RELEASE_USER="${RELEASE_USER:-$(id -un)}"
fi

# Pubkeys from THIS tree: U1 always; U2 (primary) and U3 (backup) once the
# ceremony's pubkeys commit is in it.
PUBKEYS_TEXT="$("$PY" -I - "$TREE/crates/k2-release-keys/src/lib.rs" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'pub const LEGACY_UPDATER_KEY_B64: &str = "([^"]+)";', src)
if not m:
    sys.exit("no LEGACY_UPDATER_KEY_B64 in the tree")
print("u1 " + m.group(1))
block = src[src.index("// BEGIN UPDATER_KEYS"):src.index("// END UPDATER_KEYS")]
keys = re.findall(r'role: "([a-z]+)",\s*key_id: "([0-9A-F]{16})",\s*public_key_b64: "([^"]+)"', block)
for role, slot in (("primary", "u2"), ("backup", "u3")):
    k = [x[2] for x in keys if x[0] == role]
    if len(k) > 1:
        sys.exit("UPDATER_KEYS has more than one %s key" % role)
    if k:
        print(slot + " " + k[0])
PY
)" || die "could not read the public keys from $TREE"
TREE_U2="$(printf '%s\n' "$PUBKEYS_TEXT" | /usr/bin/awk '$1=="u2"{print $2}')"
if [ -f "$ETC/u2-generated.pub" ] && [ -n "$TREE_U2" ]; then
    [ "$(tr -d '[:space:]' < "$ETC/u2-generated.pub")" = "$TREE_U2" ] \
        || die "this tree's U2 public key is not the one k2sign-admin generate-u2 made ($ETC/u2-generated.pub). Install from the commit that landed THAT key."
    say "  the tree's U2 matches the key generate-u2 made"
fi
if [ -z "$TREE_U2" ]; then
    say "  NOTE: this tree has no U2/U3 yet (pre-ceremony). Only U1 is pinned; preflight refuses until you"
    say "        re-run this installer from the commit that lands the ceremony's public keys."
fi

# ── 2. the user ────────────────────────────────────────────────────────────
SIGNER_ID=""
if [ "$MODE" != test ]; then
    if /usr/bin/dscl . -read /Users/_k2sign >/dev/null 2>&1; then
        uid="$(/usr/bin/dscl . -read /Users/_k2sign UniqueID | awk '{print $2}')"
        gid="$(/usr/bin/dscl . -read /Users/_k2sign PrimaryGroupID | awk '{print $2}')"
        shell="$(/usr/bin/dscl . -read /Users/_k2sign UserShell | awk '{print $2}')"
        home="$(/usr/bin/dscl . -read /Users/_k2sign NFSHomeDirectory | awk '{print $2}')"
        ggid="$(/usr/bin/dscl . -read /Groups/_k2sign PrimaryGroupID 2>/dev/null | awk '{print $2}')"
        [ "$uid" -ge 400 ] && [ "$uid" -le 499 ] && [ "$gid" = "$uid" ] && [ "$ggid" = "$gid" ] \
            && [ "$shell" = /usr/bin/false ] && [ "$home" = /var/k2sign ] \
            || die "_k2sign exists with other attributes (uid $uid gid $gid group $ggid shell $shell home $home); not changing it. Remove it by hand (dscl . -delete /Users/_k2sign) after checking why."
        SIGNER_ID="$uid"
        say "  _k2sign exists (uid $uid), attributes ok"
    else
        used="$( { /usr/bin/dscl . -list /Users UniqueID; /usr/bin/dscl . -list /Groups PrimaryGroupID; } | awk '{print $2}')"
        for i in $(seq 400 499); do
            printf '%s\n' "$used" | grep -qx "$i" || { SIGNER_ID="$i"; break; }
        done
        [ -n "$SIGNER_ID" ] || die "no free UID/GID in 400-499"
        say "creating hidden role user _k2sign (uid = gid = $SIGNER_ID)..."
        act /usr/bin/dscl . -create /Groups/_k2sign
        act /usr/bin/dscl . -create /Groups/_k2sign PrimaryGroupID "$SIGNER_ID"
        act /usr/bin/dscl . -create /Groups/_k2sign RealName "K2 release signer"
        act /usr/bin/dscl . -create /Groups/_k2sign Password '*'
        act /usr/bin/dscl . -create /Users/_k2sign
        act /usr/bin/dscl . -create /Users/_k2sign UniqueID "$SIGNER_ID"
        act /usr/bin/dscl . -create /Users/_k2sign PrimaryGroupID "$SIGNER_ID"
        act /usr/bin/dscl . -create /Users/_k2sign UserShell /usr/bin/false
        act /usr/bin/dscl . -create /Users/_k2sign NFSHomeDirectory /var/k2sign
        act /usr/bin/dscl . -create /Users/_k2sign RealName "K2 release signer"
        act /usr/bin/dscl . -create /Users/_k2sign Password '*'
        act /usr/bin/dscl . -create /Users/_k2sign IsHidden 1
    fi
    if [ "$DRY" = 0 ]; then
        for g in com.apple.access_ssh staff admin; do
            if /usr/sbin/dseditgroup -o checkmember -m _k2sign "$g" >/dev/null 2>&1; then
                [ "$g" = com.apple.access_ssh ] || die "_k2sign is a member of $g; refusing"
                act /usr/sbin/dseditgroup -o edit -d _k2sign -t user com.apple.access_ssh
            fi
        done
    fi
fi

# ── 3. folders ─────────────────────────────────────────────────────────────
say "folders..."
if [ "$MODE" != test ] && [ ! -d /usr/local/libexec ]; then
    act /usr/bin/install -d -o root -g wheel -m 0755 /usr/local/libexec
fi
act /usr/bin/install -d ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$LIBEXEC"
act /usr/bin/install -d ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$ETC"
for d in "$VAR" "$VAR/keys" "$VAR/gh" "$VAR/state" "$VAR/state/challenges" "$VAR/state/sessions" "$VAR/state/assets" "$VAR/audit" "$VAR/tmp"; do
    act /usr/bin/install -d ${OWN_SIGNER[@]+"${OWN_SIGNER[@]}"} -m 0700 "$d"
done
if [ "$MODE" = test ]; then
    act /bin/mkdir -p "$SUDOERS_DIR"
fi

# ── 4. minisign ────────────────────────────────────────────────────────────
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2sign-install.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
if [ "$MODE" = test ]; then
    cp "$LOCAL_MINISIGN" "$WORK/minisign"
else
    say "minisign 0.12 from jedisct1's release..."
    /usr/bin/curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/m.zip" "$MINISIGN_ZIP_URL" || die "download failed: $MINISIGN_ZIP_URL"
    /usr/bin/curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/m.zip.minisig" "$MINISIGN_ZIP_URL.minisig" || die "download failed: .minisig"
    [ "$(sha256 "$WORK/m.zip")" = "$MINISIGN_ZIP_SHA256" ] || die "minisign zip sha256 is not the pinned $MINISIGN_ZIP_SHA256"
    /usr/bin/ditto -x -k "$WORK/m.zip" "$WORK/x"
    cp "$WORK/x/minisign" "$WORK/minisign"
    [ "$(sha256 "$WORK/minisign")" = "$MINISIGN_BIN_SHA256" ] || die "minisign binary sha256 is not the pinned $MINISIGN_BIN_SHA256"
    chmod 0755 "$WORK/minisign"
    "$WORK/minisign" -Vm "$WORK/m.zip" -x "$WORK/m.zip.minisig" -P "$JEDISCT1_MINISIGN_PUB" -q \
        || die "minisign zip signature does not verify with jedisct1's key"
    say "  zip and binary match the pinned sha256; jedisct1's signature verifies"
fi
"$WORK/minisign" -v 2>&1 | grep -q '^minisign 0\.1[1-9]' || die "minisign does not run or is older than 0.11"
if [ "$MODE" = test ]; then
    cp "$LOCAL_GH" "$WORK/gh"
    cp "$LOCAL_GH" "$WORK/gh.zip"   # any bytes: the test attests them itself
else
    say "gh 2.83.1 from cli/cli's release..."
    /usr/bin/curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/gh.zip" "$GH_ZIP_URL" || die "download failed: $GH_ZIP_URL"
    [ "$(sha256 "$WORK/gh.zip")" = "$GH_ZIP_SHA256" ] || die "gh zip sha256 is not the pinned $GH_ZIP_SHA256"
    /usr/bin/ditto -x -k "$WORK/gh.zip" "$WORK/ghx"
    cp "$WORK/ghx/gh_2.83.1_macOS_arm64/bin/gh" "$WORK/gh"
    [ "$(sha256 "$WORK/gh")" = "$GH_BIN_SHA256" ] || die "gh binary sha256 is not the pinned $GH_BIN_SHA256"
    say "  zip and binary match the pinned sha256"
fi
chmod 0755 "$WORK/gh"
"$WORK/gh" --version 2>&1 | grep -q '^gh version ' || die "gh does not run"

# ── 5. signer, policy, pubkeys ─────────────────────────────────────────────
say "signer files..."
for w in k2sign k2sign-admin; do
    sed "s#@PYTHON@#$PY#" "$HERE/$w" > "$WORK/$w"
    grep -q '@PYTHON@' "$WORK/$w" && die "could not fill in the python path in $w"
done
printf '%s\n' "# Public keys pinned by install.sh from $(cd "$TREE" && git -c safe.directory='*' rev-parse HEAD 2>/dev/null || echo 'an unknown commit')." \
    "$PUBKEYS_TEXT" > "$WORK/pubkeys"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$WORK/k2sign" "$LIBEXEC/k2sign"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$WORK/k2sign-admin" "$LIBEXEC/k2sign-admin"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0644 "$HERE/k2sign.py" "$LIBEXEC/k2sign.py"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$WORK/minisign" "$LIBEXEC/minisign"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0755 "$WORK/gh" "$LIBEXEC/gh"
# The release zip stays too: k2sign gh-selfcheck verifies cli/cli's attestation of it.
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0644 "$WORK/gh.zip" "$LIBEXEC/gh-release.zip"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0644 "$TREE/scripts/lib/check-daemon-asset.py" "$LIBEXEC/check-daemon-asset.py"
if [ -f "$ETC/policy" ] && ! cmp -s "$HERE/policy" "$ETC/policy"; then
    say "  NOTE: replacing a changed $ETC/policy with the reviewed copy (a soft rollback edit is undone)"
fi
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0644 "$HERE/policy" "$ETC/policy"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0644 "$WORK/pubkeys" "$ETC/pubkeys"

# ── 6. sudoers ─────────────────────────────────────────────────────────────
say "sudoers drop-in..."
{
    echo "# K2 release signing (prd-signing-user-v1 §4.2), written by scripts/k2sign/install.sh."
    echo "# $RELEASE_USER may run ONLY the signer as _k2sign, without a password. Any other"
    echo "# command as _k2sign, and anything as root, still needs the admin password."
    echo "$RELEASE_USER ALL=(_k2sign) NOPASSWD: /usr/local/libexec/k2sign/k2sign"
} > "$WORK/sudoers"
/usr/sbin/visudo -cf "$WORK/sudoers" >/dev/null || die "the sudoers drop-in does not parse"
act /usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0440 "$WORK/sudoers" "$SUDOERS_DIR/k2sign"
if [ "$MODE" != test ] && [ "$DRY" = 0 ]; then
    /usr/sbin/visudo -c >/dev/null || die "sudoers as a whole no longer parses: remove $SUDOERS_DIR/k2sign NOW (sudo rm $SUDOERS_DIR/k2sign)"
fi

# ── 7. pins + hashes ───────────────────────────────────────────────────────
if [ "$DRY" = 1 ]; then
    say "dry run: would write $ETC/pins.sha256 and print every hash. Nothing was changed."
    exit 0
fi
{
    for f in k2sign k2sign-admin k2sign.py minisign gh gh-release.zip check-daemon-asset.py; do printf '%s  libexec/%s\n' "$(sha256 "$LIBEXEC/$f")" "$f"; done
    for f in policy pubkeys approver.pem; do
        [ -f "$ETC/$f" ] && printf '%s  etc/%s\n' "$(sha256 "$ETC/$f")" "$f"
    done
} > "$WORK/pins"
/usr/bin/install ${OWN_ROOT[@]+"${OWN_ROOT[@]}"} -m 0644 "$WORK/pins" "$ETC/pins.sha256"
say "installed. Hashes (K2 checks these against the branch):"
sed 's/^/  /' "$ETC/pins.sha256"
printf '  %s  etc/sudoers.d/k2sign\n' "$(sha256 "$SUDOERS_DIR/k2sign")"
printf '  %s  python %s\n' "$(sha256 "$PY")" "$PY"
say "GitHub token (S3 evidence): sudo $LIBEXEC/k2sign-admin set-gh-token  (fine-grained, read-only, this repo)"
say "next: enrol the approver (K2 Release Approve → Enrol), then sudo $LIBEXEC/k2sign-admin set-approver ~/.k2-release-approve/approver.pem"
