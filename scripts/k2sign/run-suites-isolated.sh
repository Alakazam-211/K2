#!/bin/bash
# run-suites-isolated.sh: run the k2sign S2, S3 and S3b suites
# (tests/release/k2sign-s2.sh, k2sign-s3.sh, k2sign-s3b.sh) on the signing
# Mac, at one commit, without touching the installed signer, the keychain
# or any release state.
#
#   bash scripts/k2sign/run-suites-isolated.sh <commit> [--keep]
#   bash scripts/k2sign/run-suites-isolated.sh --probe-only
#
# Run it as the release user, from any checkout that holds <commit> (it
# never changes that checkout). What it does:
#   1. Refuses as root, as the signing user (_k2sign), under sudo, off
#      macOS, or when sandbox-exec does not work.
#   2. Makes one temp folder W and exports <commit> into W/tree with
#      `git archive` (no .git, so nothing can write to this checkout).
#   3. Runs each suite from W/tree under `env -i` with HOME, TMPDIR, the
#      XDG folders, the git and gh config, and every K2 and K5 home pointed
#      into W, no K2_*, K5_*, K2SIGN_* or K2SA_* from the caller, and
#      stand-ins first on PATH that refuse sudo, security, launchctl, dscl,
#      defaults, osascript, open, gh, k2 and k5.
#   4. Runs it all inside sandbox-exec: no file may be written outside W,
#      sudo, security, launchctl, dscl, defaults, osascript, open and the
#      installed signers (/usr/local/libexec/k2sign, k2-signer) cannot run,
#      and the keychain services cannot be reached. A probe proves the
#      sandbox bites before any suite runs (--probe-only runs just that).
#      Only one case runs without its own sandbox: when this script is
#      already inside one (macOS refuses a nested sandbox, as on a compute
#      node) AND no signer is installed on this Mac (no _k2sign user, no
#      /usr/local/libexec/k2sign or k2-signer, no /usr/local/etc/k2sign,
#      /var/k2sign or sudoers.d/k2sign*). On the signing Mac the sandbox is
#      always required.
#   5. Takes a listing (names, modes, sizes, mtimes) of the real state the
#      suites could touch before and after (the names in HOME, the keychains,
#      LaunchAgents, ~/.k2*, ~/k2-release-audit, the gh and git config, the
#      installed signer's folders, sudoers.d and this checkout's .git), and
#      fails if any of it changed.
#   6. Prints each suite's result line and removes W (--keep leaves it, for
#      reading the logs).
# Exit 0 only when all three suites pass and nothing watched changed.
#
# minisign: the suites need minisign >= 0.11. If one is on the caller's
# PATH it is copied into W; otherwise the zip scripts/k2sign/install.sh pins
# is downloaded (checked by its sha256) before the sandbox starts.
set -euo pipefail

SUITES="k2sign-s2.sh k2sign-s3.sh k2sign-s3b.sh"
SIGNER_USER=_k2sign

usage() {
    sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}
die() { printf 'run-suites-isolated: REFUSED: %s\n' "$*" >&2; exit 1; }
say() { printf 'run-suites-isolated: %s\n' "$*"; }

COMMIT="" KEEP=0 PROBE_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --keep) KEEP=1; shift ;;
        --probe-only) PROBE_ONLY=1; shift ;;
        -h|--help) usage ;;
        -*) echo "run-suites-isolated: unknown option $1" >&2; usage ;;
        *) [ -z "$COMMIT" ] || usage; COMMIT="$1"; shift ;;
    esac
done
if [ "$PROBE_ONLY" = 1 ]; then [ -z "$COMMIT" ] || usage; else [ -n "$COMMIT" ] || usage; fi

# ── 1. who and where ───────────────────────────────────────────────────
[ "$(uname -s)" = Darwin ] || die "the k2sign suites run on macOS only"
[ "$(/usr/bin/id -u)" != 0 ] || die "refused as root"
ME="$(/usr/bin/id -un)"
[ "$ME" != "$SIGNER_USER" ] || die "refused as the signing user $SIGNER_USER"
[ -z "${SUDO_USER:-}${SUDO_UID:-}" ] || die "refused under sudo (run it as the release user, without sudo)"
[ -x /usr/bin/sandbox-exec ] || die "no /usr/bin/sandbox-exec: the suites run only inside the sandbox"
# The account's own home (from the user database, not $HOME), and $HOME
# when that is somewhere else: both are watched and both are off limits.
REAL_HOME="$(/usr/bin/python3 -I -c 'import os, pwd; print(os.path.realpath(pwd.getpwuid(os.getuid()).pw_dir))')" \
    || die "cannot read this account's home folder"
CALLER_HOME="$(cd "${HOME:-$REAL_HOME}" && pwd -P)" || die "cannot read \$HOME"

signer_installed() {
    /usr/bin/id "$SIGNER_USER" >/dev/null 2>&1 && return 0
    local p
    for p in /usr/local/libexec/k2sign /usr/local/libexec/k2-signer /usr/local/etc/k2sign /usr/local/etc/k2-signer \
             /var/k2sign /private/etc/sudoers.d/k2sign /private/etc/sudoers.d/k2-signer; do
        [ ! -e "$p" ] || return 0
    done
    return 1
}
# Can this script apply its own sandbox? macOS refuses a nested one.
SANDBOX=1
sb_err="$(/usr/bin/sandbox-exec -p '(version 1) (allow default)' /usr/bin/true 2>&1)" || {
    case "$sb_err" in
        *"sandbox_apply: Operation not permitted"*)
            ! signer_installed || die "already inside a sandbox, so this script can't apply its own, and a signer is installed on this Mac"
            SANDBOX=0 ;;
        *) die "sandbox-exec does not work here: $sb_err" ;;
    esac
}

SRC="$(cd "$(dirname "$0")/../.." && pwd -P)"
/usr/bin/git -C "$SRC" rev-parse --git-dir >/dev/null 2>&1 || die "$SRC is not a git checkout"
if [ "$PROBE_ONLY" = 0 ]; then
    SHA="$(/usr/bin/git -C "$SRC" rev-parse --verify --quiet "${COMMIT}^{commit}")" || die "no commit $COMMIT in $SRC"
    for s in $SUITES k2sign_fixture.sh; do
        /usr/bin/git -C "$SRC" cat-file -e "$SHA:tests/release/$s" 2>/dev/null || die "$SHA has no tests/release/$s"
    done
fi
GIT_DIR_REAL="$(cd "$SRC" && cd "$(/usr/bin/git rev-parse --git-common-dir)" && pwd -P)"

# ── 2. the temp folder and the export ──────────────────────────────────
base="$(cd "${TMPDIR:-/tmp}" && pwd -P)"
W="$(/usr/bin/mktemp -d "$base/k2sign-isolated.XXXXXX")"
W="$(cd "$W" && pwd -P)"
cleanup() {
    if [ "$KEEP" = 1 ]; then say "kept $W"; else /bin/chmod -R u+w "$W" 2>/dev/null || true; /bin/rm -rf "$W"; fi
}
trap cleanup EXIT
case "$W" in
    "$REAL_HOME"|"$REAL_HOME"/*|"$CALLER_HOME"|"$CALLER_HOME"/*) die "the temp folder $W is inside a home folder; set TMPDIR elsewhere" ;;
    "$SRC"|"$SRC"/*) die "the temp folder $W is inside the checkout" ;;
esac
case "$SRC" in "$W"/*) die "the checkout is inside the temp folder" ;; esac
mkdir -p "$W/tree" "$W/home" "$W/tmp" "$W/bin" "$W/logs" "$W/xdg/config" "$W/xdg/cache" "$W/xdg/data" "$W/xdg/state" \
    "$W/k2home" "$W/k5home" "$W/gh"
if [ "$PROBE_ONLY" = 0 ]; then
    /usr/bin/git -C "$SRC" archive --format=tar "$SHA" | /usr/bin/tar -x -C "$W/tree" \
        || die "git archive of $SHA failed"
    say "commit $SHA exported to $W/tree"

    # ── minisign, before the sandbox (the download needs the network) ─────
    if m="$(command -v minisign 2>/dev/null)" && [ -x "$m" ]; then
        /bin/cp "$m" "$W/bin/minisign"
        say "minisign: copied from $m"
    else
        url="$(sed -n 's/^MINISIGN_ZIP_URL="\(.*\)"$/\1/p' "$W/tree/scripts/k2sign/install.sh")"
        sum="$(sed -n 's/^MINISIGN_ZIP_SHA256="\(.*\)"$/\1/p' "$W/tree/scripts/k2sign/install.sh")"
        [ -n "$url" ] && [ -n "$sum" ] || die "no minisign pin in scripts/k2sign/install.sh"
        /usr/bin/curl -fsSL --proto '=https' --tlsv1.2 -o "$W/tmp/m.zip" "$url" || die "download of $url failed"
        [ "$(/usr/bin/shasum -a 256 "$W/tmp/m.zip" | cut -d' ' -f1)" = "$sum" ] || die "the minisign zip is not the pinned $sum"
        /usr/bin/ditto -x -k "$W/tmp/m.zip" "$W/tmp/mx"
        /bin/cp "$W/tmp/mx/minisign" "$W/bin/minisign"
        /bin/rm -rf "$W/tmp/m.zip" "$W/tmp/mx"
        say "minisign: the pinned release ($sum)"
    fi
    /bin/chmod 0755 "$W/bin/minisign"
fi

# Stand-ins first on PATH: each records the call and fails.
for t in sudo security launchctl dscl defaults osascript open gh k2 k5 k5-cli; do
    cat > "$W/bin/$t" <<SH
#!/bin/sh
echo "$t \$*" >> "$W/logs/refused.calls"
echo "run-suites-isolated: $t is refused here" >&2
exit 97
SH
    /bin/chmod 0755 "$W/bin/$t"
done

# ── 5a. the real state, before ─────────────────────────────────────────
WATCH="$REAL_HOME/Library/Keychains
$REAL_HOME/Library/LaunchAgents
$REAL_HOME/Applications
$REAL_HOME/.k2
$REAL_HOME/.k2sign
$REAL_HOME/.k2-updater
$REAL_HOME/.k2-release-approve
$REAL_HOME/.k5
$REAL_HOME/k2-release-audit
$REAL_HOME/.config/gh
$REAL_HOME/.gitconfig
$REAL_HOME/.config/git
/usr/local/libexec/k2sign
/usr/local/libexec/k2-signer
/usr/local/etc/k2sign
/usr/local/etc/k2-signer
/var/k2sign
/private/etc/sudoers.d
/Library/LaunchDaemons
$GIT_DIR_REAL/HEAD
$GIT_DIR_REAL/index
$GIT_DIR_REAL/refs
$GIT_DIR_REAL/worktrees"
snapshot() {
    {
        # The homes' top level: the names only (a file appearing or going
        # away), since other programs change mtimes there all the time.
        for h in "$REAL_HOME" "$CALLER_HOME"; do
            /bin/ls -A "$h" 2>/dev/null | sed "s|^|$h/ |" || true
        done
        printf '%s\n' "$WATCH" | while IFS= read -r p; do
            if [ -e "$p" ] || [ -L "$p" ]; then
                /usr/bin/find "$p" -maxdepth 3 -exec /usr/bin/stat -f '%N %p %z %m' {} + 2>/dev/null || echo "$p (unreadable)"
            else
                echo "$p (absent)"
            fi
        done
    } | LC_ALL=C sort
}
snapshot > "$W/logs/state.before"

# ── 4. the sandbox ─────────────────────────────────────────────────────
cat > "$W/sandbox.sb" <<SB
(version 1)
(allow default)
(deny file-write*
    (require-not (require-any
        (subpath "$W")
        (literal "/dev/null") (literal "/dev/zero") (literal "/dev/tty") (literal "/dev/dtracehelper")
        (regex #"^/dev/fd/") (regex #"^/dev/ttys"))))
(deny process-exec
    (literal "/usr/bin/sudo") (literal "/usr/bin/security") (literal "/bin/launchctl")
    (literal "/usr/bin/dscl") (literal "/usr/bin/defaults") (literal "/usr/bin/osascript")
    (literal "/usr/bin/open") (literal "/usr/sbin/installer")
    (subpath "/usr/local/libexec/k2sign") (subpath "/usr/local/libexec/k2-signer"))
(deny mach-lookup
    (global-name "com.apple.SecurityServer") (global-name "com.apple.securityd")
    (global-name "com.apple.securityd.xpc") (global-name "com.apple.secd")
    (global-name "com.apple.security.keychain-circle-notification"))
SB

# The one environment every suite gets: nothing from the caller but USER,
# LOGNAME and a locale.
ENVV="HOME=$W/home
TMPDIR=$W/tmp/
PATH=$W/bin:/usr/bin:/bin:/usr/sbin:/sbin
USER=$ME
LOGNAME=$ME
LANG=en_US.UTF-8
SHELL=/bin/bash
CFFIXED_USER_HOME=$W/home
XDG_CONFIG_HOME=$W/xdg/config
XDG_CACHE_HOME=$W/xdg/cache
XDG_DATA_HOME=$W/xdg/data
XDG_STATE_HOME=$W/xdg/state
GIT_CONFIG_NOSYSTEM=1
GIT_CONFIG_GLOBAL=$W/home/.gitconfig
GH_CONFIG_DIR=$W/gh
K2_HOME=$W/k2home
K2SO_HOME=$W/k2home
K5_HOME=$W/k5home
PYTHONDONTWRITEBYTECODE=1"
in_sandbox() {
    local IFS='
'
    # shellcheck disable=SC2046
    if [ "$SANDBOX" = 1 ]; then
        /usr/bin/env -i $ENVV /usr/bin/sandbox-exec -f "$W/sandbox.sb" "$@"
    else
        /usr/bin/env -i $ENVV "$@"
    fi
}

# The sandbox bites before any suite runs: each of these must fail.
probe="$REAL_HOME/.k2sign-isolated-probe.$$"
probe2="/private/tmp/.k2sign-isolated-probe.$$"
in_sandbox /bin/bash -c '
    p="$1" p2="$2" full="$3" bad=0
    if security -h >/dev/null 2>&1; then echo "security on PATH ran"; bad=1; fi
    if sudo -n -V >/dev/null 2>&1; then echo "sudo on PATH ran"; bad=1; fi
    ( : > "$HOME/ok" ) || { echo "a write to the temp home failed"; bad=1; }
    [ "$full" = 1 ] || exit "$bad"
    if ( : > "$p" ) 2>/dev/null; then echo "a write to the home folder went through"; bad=1; fi
    if ( : > "$p2" ) 2>/dev/null; then echo "a write outside the temp folder went through"; bad=1; fi
    if /usr/bin/security -h >/dev/null 2>&1; then echo "/usr/bin/security ran"; bad=1; fi
    if /usr/bin/sudo -n -V >/dev/null 2>&1; then echo "/usr/bin/sudo ran"; bad=1; fi
    if /bin/launchctl version >/dev/null 2>&1; then echo "/bin/launchctl ran"; bad=1; fi
    exit "$bad"
' _ "$probe" "$probe2" "$SANDBOX" > "$W/logs/probe.out" 2>&1 || {
    /bin/rm -f "$probe" "$probe2" 2>/dev/null || true
    cat "$W/logs/probe.out" >&2
    die "the sandbox does not hold (lines above); no suite was run"
}
[ ! -e "$probe" ] && [ ! -e "$probe2" ] || { /bin/rm -f "$probe" "$probe2"; die "a probe file was written outside $W"; }
: > "$W/logs/refused.calls"
if [ "$SANDBOX" = 1 ]; then
    say "sandbox probe: writes outside $W, sudo, security and launchctl are all refused"
else
    say "already inside a sandbox (macOS refuses a nested one) and no signer is installed on this Mac:"
    say "  running under that sandbox, with the stand-ins on PATH (probed) and the state check"
fi
if [ "$PROBE_ONLY" = 1 ]; then
    [ "$SANDBOX" = 1 ] || die "--probe-only: this script could not apply its own sandbox here"
    exit 0
fi

# ── 3. the suites ──────────────────────────────────────────────────────
rc=0
for s in $SUITES; do
    log="$W/logs/$s.log"
    if in_sandbox /bin/bash "$W/tree/tests/release/$s" > "$log" 2>&1; then
        echo "PASS $(tail -1 "$log")"
    else
        rc=1
        echo "FAIL $s:"
        grep -E "FAIL|Traceback|Error|error|refused|deny" "$log" | head -40 || true
        tail -3 "$log"
    fi
done
if [ -s "$W/logs/refused.calls" ]; then
    rc=1
    echo "FAIL: a suite called a refused command (it was stopped, nothing ran):"
    sed 's/^/  /' "$W/logs/refused.calls"
fi

# ── 5b. the real state, after ──────────────────────────────────────────
snapshot > "$W/logs/state.after"
if ! /usr/bin/diff "$W/logs/state.before" "$W/logs/state.after" > "$W/logs/state.diff"; then
    rc=1
    echo "FAIL: real state changed while the suites ran (check whether something else on this Mac changed it):"
    sed 's/^/  /' "$W/logs/state.diff" | head -40
else
    echo "unchanged: $(grep -vc '(absent)' "$W/logs/state.before") watched paths outside $W"
fi
exit "$rc"
