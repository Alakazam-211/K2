#!/bin/bash
# run-suites-isolated.sh: run the k2sign S2, S3 and S3b suites
# (tests/release/k2sign-s2.sh, k2sign-s3.sh, k2sign-s3b.sh) on the signing
# Mac, at one commit, without touching the installed signer, the keychain
# or any release state.
#
#   bash scripts/k2sign/run-suites-isolated.sh <commit> [--keep]
#   bash scripts/k2sign/run-suites-isolated.sh --probe-only [<commit>]
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
#   4. Runs it all inside sandbox-exec, from W/tree: no file may be
#      written outside W (except xcrun's cache file, below),
#      sudo, security, launchctl, dscl, defaults, osascript, open and the
#      installed signers (/usr/local/libexec/k2sign, k2-signer) cannot run,
#      and the keychain services cannot be reached. A probe proves the
#      sandbox bites before any suite runs, and runs one real signer
#      command (generate-u2) in a throwaway test install under W
#      (--probe-only runs just the probe, at <commit> or HEAD).
#      Only one case runs without its own sandbox: when this script is
#      already inside one (macOS refuses a nested sandbox, as on a compute
#      node) AND no signer is installed on this Mac (no _k2sign user, no
#      /usr/local/libexec/k2sign or k2-signer, no /usr/local/etc/k2sign,
#      /var/k2sign or sudoers.d/k2sign*). On the signing Mac the sandbox is
#      always required.
#   5. Takes a listing of the real state before and after, and fails if
#      any of it changed: names, modes, sizes and mtimes of what a suite
#      could plausibly touch (the keychain files except the live login
#      keychain, LaunchAgents, ~/Applications,
#      ~/.k2sign, ~/.k2-updater, ~/.k2-release-approve, ~/k2-release-audit,
#      the gh and git config, the installed signer's folders, sudoers.d,
#      LaunchDaemons and this checkout's .git), and only the top-level
#      NAMES of the home folder, ~/.k2, ~/.k5 and the keychain folders
#      macOS writes all the time (~/Library/Keychains/<UUID>/, the
#      data-protection keychain-2.db and its -wal and -shm). A running K2
#      daemon writes
#      ~/.k2 all the time (its database and WAL, logs, sockets, ports), so
#      mtimes there say nothing about the suites; the suites never see the
#      real home (HOME is inside W) and the sandbox refuses any write to it,
#      and a file appearing or going away there is still caught.
#      The live login keychain is watched by its ITEMS, not its mtime (the
#      K2 app writes it): `security dump-keychain` without -d (attributes
#      only, never secrets) gives each item's label and class, its own
#      modification date (mdat), and for keys and certificates, which have
#      no mdat, the public key hash. Any change, or a different count,
#      fails the run, and the log names the item by label only
#      (scripts/k2sign/kc-items.awk, tested by tests/release/k2sign-kc-items.sh).
#   5b. The suites get SDKROOT, found once with `xcrun --show-sdk-path`
#      before the sandbox and printed in the log; read-only inside the
#      sandbox. Only this harness sets it; the shipped wrappers' env lists
#      are unchanged.
#   6. Prints each suite's result line and removes W (--keep leaves it, for
#      reading the logs).
# Exit 0 only when all three suites pass and nothing watched changed.
#
# Temp files on macOS, and why the suites run from W/tree:
# - /bin/bash (3.2) never puts a here document's temp file in TMPDIR: it
#   takes /var/tmp, else /tmp, else the current folder. In the sandbox the
#   first two are not writable, so it uses the current folder, which is
#   W/tree (inside W) for every suite.
# - mktemp with no template, or with -t, uses the per-user temp folder and
#   ignores TMPDIR. The suites always pass "${TMPDIR:-/tmp}/name.XXXXXX".
# - The xcrun shims (/usr/bin/python3, git, cc) write one cache file,
#   xcrun_db, in the per-user temp folder. The sandbox allows that one name
#   there and nothing else.
# - The signer's wrappers (scripts/k2sign/k2sign, k2sign-admin) run its
#   python under env -i from /, so TMPDIR never reaches it, and the admin
#   verbs' scratch is tempfile.mkdtemp(). In the sandbox python finds no
#   temp folder it may write. This script does not open /tmp for it and
#   does not change the signer; the signer probe below stops the run with
#   that reason until the signer is given a temp folder inside its test
#   root.
# The probe checks each of these.
#
# The sandbox mode runs only on a Mac that is not itself in a sandbox: a
# compute job is, so there this script runs in the fallback mode.
#
# minisign: the suites need minisign >= 0.11. If one is on the caller's
# PATH it is copied into W; otherwise the zip scripts/k2sign/install.sh pins
# is downloaded (checked by its sha256) before the sandbox starts.
set -euo pipefail

SUITES="k2sign-s2.sh k2sign-s3.sh k2sign-s3b.sh"
SIGNER_USER=_k2sign

usage() {
    sed -n '2,77p' "$0" | sed 's/^# \{0,1\}//'
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
if [ "$PROBE_ONLY" = 1 ]; then COMMIT="${COMMIT:-HEAD}"; else [ -n "$COMMIT" ] || usage; fi

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
SHA="$(/usr/bin/git -C "$SRC" rev-parse --verify --quiet "${COMMIT}^{commit}")" || die "no commit $COMMIT in $SRC"
for s in $SUITES k2sign_fixture.sh; do
    /usr/bin/git -C "$SRC" cat-file -e "$SHA:tests/release/$s" 2>/dev/null || die "$SHA has no tests/release/$s"
done
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
# Full listings (names, modes, sizes, mtimes): what a suite could plausibly
# touch.
WATCH="$REAL_HOME/Library/LaunchAgents
$REAL_HOME/Applications
$REAL_HOME/.k2sign
$REAL_HOME/.k2-updater
$REAL_HOME/.k2-release-approve
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
# Names only (a file appearing or going away): the home folders, and the
# K2 and K5 homes a running daemon and CLI write all the time.
WATCH_NAMES="$REAL_HOME
$CALLER_HOME
$REAL_HOME/.k2
$REAL_HOME/.k5"
snapshot() {
    {
        printf '%s\n' "$WATCH_NAMES" | while IFS= read -r h; do
            if [ -d "$h" ]; then
                /bin/ls -A "$h" 2>/dev/null | sed "s|^|$h/ |" || echo "$h (unreadable)"
            else
                echo "$h (absent)"
            fi
        done
        # The keychains: every file in the folder in full (login and the
        # other *.keychain-db), the names in each subfolder (macOS writes
        # the <UUID>/ data-protection keychain all the time).
        # The live login keychain is the one exception: the K2 app writes
        # it while the suites run, so its mtime does not count here; its
        # items do (kc_items, compared after the suites).
        kc="$REAL_HOME/Library/Keychains"
        if [ -d "$kc" ]; then
            /bin/ls -A "$kc" | sed "s|^|$kc/ |"
            /usr/bin/find "$kc" -maxdepth 1 -type f ! -name login.keychain-db -exec /usr/bin/stat -f '%N %p %z %m' {} + 2>/dev/null || echo "$kc (unreadable)"
            if [ -f "$kc/login.keychain-db" ]; then
                /usr/bin/stat -f '%N %p %z (mtime not counted: its items are)' "$kc/login.keychain-db"
            fi
            /usr/bin/find "$kc" -mindepth 1 -maxdepth 1 -type d | while IFS= read -r d; do
                /bin/ls -A "$d" 2>/dev/null | sed "s|^|$d/ |" || echo "$d (unreadable)"
            done
        else
            echo "$kc (absent)"
        fi
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

# The live login keychain's items, from `security dump-keychain` WITHOUT
# -d, so attributes only, never a secret: label, class and each item's own
# modification date (mdat); for keys and certificates, which have no mdat,
# their label plus the key's or certificate's public key hash. Sorted, so
# the count is compared too. Written inside W and removed with it.
LOGIN_KC="$REAL_HOME/Library/Keychains/login.keychain-db"
kc_items() {
    [ -f "$LOGIN_KC" ] || { echo "(no login keychain)"; return 0; }
    local dump
    dump="$(/usr/bin/security dump-keychain "$LOGIN_KC")" || die "security dump-keychain (attributes only) failed on the login keychain"
    printf '%s\n' "$dump" | /usr/bin/awk -f "$SRC/scripts/k2sign/kc-items.awk" | LC_ALL=C sort
}
kc_items > "$W/logs/kc.before"

# ── 4. the sandbox ─────────────────────────────────────────────────────
USER_TMP="$(cd "$(/usr/bin/getconf DARWIN_USER_TEMP_DIR)" && pwd -P)" || die "cannot read the per-user temp folder"
case "$USER_TMP" in /private/var/folders/*) ;; *) die "unexpected per-user temp folder $USER_TMP" ;; esac
cat > "$W/sandbox.sb" <<SB
(version 1)
(allow default)
(deny file-write*
    (require-not (require-any
        (subpath "$W")
        (regex #"^$USER_TMP/xcrun_db(-[A-Za-z0-9]+)?\$")
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

# The macOS SDK, found here, before the sandbox and env -i. Without SDKROOT,
# clang under env -i takes the Command Line Tools SDK, which may not match
# the linker (S3 links C code). The release build on the signing Mac finds
# its SDK the same way. No SDK: stop, never guess.
SDK="$(/usr/bin/xcrun --show-sdk-path 2>/dev/null)" || SDK=""
if [ -z "$SDK" ] || [ ! -d "$SDK" ]; then
    echo "run-suites-isolated: cannot find the macOS SDK (xcrun --sdk macosx --show-sdk-path)" >&2
    exit 1
fi
case "$SDK" in *'
'*) echo "run-suites-isolated: the SDK path holds a newline: $SDK" >&2; exit 1 ;; esac
# Read-only inside the sandbox: its profile refuses every write outside W.
echo "SDK: $SDK (SDKROOT for the suites only; the shipped wrappers' env lists are unchanged)"

# The one environment every suite gets: nothing from the caller but USER,
# LOGNAME and a locale, plus the SDK found above.
ENVV="HOME=$W/home
SDKROOT=$SDK
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
    # From W/tree: bash puts here-document temp files in the current folder.
    if [ "$SANDBOX" = 1 ]; then
        (cd "$W/tree" && /usr/bin/env -i $ENVV /usr/bin/sandbox-exec -f "$W/sandbox.sb" "$@")
    else
        (cd "$W/tree" && /usr/bin/env -i $ENVV "$@")
    fi
}

# The probe, before any suite runs. In both modes: the stand-ins refuse;
# a here document and a here string work; mktemp and mktemp -d with a
# "$TMPDIR/name.XXXXXX" template and python's tempfile make their files
# inside W; the xcrun shim python3 runs with nothing on stderr. In sandbox
# mode also: writes to both home folders, to /tmp (spelled both ways), to
# /private/var/tmp and to the per-user temp folder are refused, and sudo,
# security and launchctl cannot run.
probe="$REAL_HOME/.k2sign-isolated-probe.$$"
probe2="/private/tmp/.k2sign-isolated-probe.$$"
probe3="/tmp/.k2sign-isolated-probe3.$$"
probe4="/private/var/tmp/.k2sign-isolated-probe.$$"
probe5="$CALLER_HOME/.k2sign-isolated-probe5.$$"
probe6="$USER_TMP/.k2sign-isolated-probe.$$"
in_sandbox /bin/bash -c '
    W="$1" full="$2" bad=0
    shift 2
    if security -h >/dev/null 2>&1; then echo "security on PATH ran"; bad=1; fi
    if sudo -n -V >/dev/null 2>&1; then echo "sudo on PATH ran"; bad=1; fi
    ( : > "$HOME/ok" ) || { echo "a write to the temp home failed"; bad=1; }
    hd="$(cat <<EOF
heredoc-ok
EOF
)" || true
    [ "$hd" = heredoc-ok ] || { echo "a here document failed: [$hd]"; bad=1; }
    hs="$(cat <<< "herestring-ok")" || true
    [ "$hs" = herestring-ok ] || { echo "a here string failed: [$hs]"; bad=1; }
    f="$(mktemp "${TMPDIR:-/tmp}/probe.XXXXXX")" || { echo "mktemp with a template failed"; bad=1; }
    case "$f" in "$W"/*) ;; *) echo "mktemp made [$f], outside $W"; bad=1 ;; esac
    d="$(mktemp -d "${TMPDIR:-/tmp}/probe.XXXXXX")" || { echo "mktemp -d with a template failed"; bad=1; }
    case "$d" in "$W"/*) ;; *) echo "mktemp -d made [$d], outside $W"; bad=1 ;; esac
    t="$(python3 -I -c "import tempfile; print(tempfile.mkdtemp())" 2>"$W/logs/probe-py.err")" || { echo "python tempfile failed"; bad=1; }
    case "$t" in "$W"/*) ;; *) echo "python tempfile made [$t], outside $W"; bad=1 ;; esac
    [ ! -s "$W/logs/probe-py.err" ] || { echo "python3 wrote to stderr: $(head -c 300 "$W/logs/probe-py.err")"; bad=1; }
    [ "$full" = 1 ] || exit "$bad"
    for p in "$@"; do
        if ( : > "$p" ) 2>/dev/null; then echo "a write to $p went through"; bad=1; fi
    done
    if /usr/bin/security -h >/dev/null 2>&1; then echo "/usr/bin/security ran"; bad=1; fi
    if /usr/bin/sudo -n -V >/dev/null 2>&1; then echo "/usr/bin/sudo ran"; bad=1; fi
    if /bin/launchctl version >/dev/null 2>&1; then echo "/bin/launchctl ran"; bad=1; fi
    exit "$bad"
' _ "$W" "$SANDBOX" "$probe" "$probe2" "$probe3" "$probe4" "$probe5" "$probe6" > "$W/logs/probe.out" 2>&1 || {
    /bin/rm -f "$probe" "$probe2" "$probe3" "$probe4" "$probe5" "$probe6" 2>/dev/null || true
    cat "$W/logs/probe.out" >&2
    die "the sandbox does not hold (lines above); no suite was run"
}
for p in "$probe" "$probe2" "$probe3" "$probe4" "$probe5" "$probe6"; do
    [ ! -e "$p" ] || { /bin/rm -f "$probe" "$probe2" "$probe3" "$probe4" "$probe5" "$probe6"; die "a probe file was written outside $W: $p"; }
done
# One real signer command in a throwaway test install under W, the way the
# suites' fixture makes one: install.sh --test, then k2sign-admin
# generate-u2 (an admin verb: its scratch is python's temp folder).
in_sandbox /bin/bash -c '
    W="$1" r="$1/probe-root"
    mkdir -p "$r" || exit 1
    K2SIGN_TEST_ROOT="$r" /bin/bash scripts/k2sign/install.sh --test --minisign "$W/bin/minisign" \
        --gh tests/release/fixtures/gh-stub.sh > "$W/logs/probe-install.out" 2>&1 \
        || { echo "install.sh --test into $r failed:"; tail -5 "$W/logs/probe-install.out"; exit 1; }
    if ! K2SIGN_TEST_ROOT="$r" "$r/usr/local/libexec/k2sign/k2sign-admin" generate-u2 > "$W/logs/probe-admin.out" 2>&1; then
        echo "k2sign-admin generate-u2 failed:"; tail -5 "$W/logs/probe-admin.out"
        if grep -q "No usable temporary directory" "$W/logs/probe-admin.out"; then
            echo "(the signer runs python under env -i from /, so it has no temp folder inside the sandbox)"
        fi
        exit 1
    fi
    grep -q "U2 generated inside $r/" "$W/logs/probe-admin.out" \
        || { echo "k2sign-admin generate-u2 did not say where:"; tail -5 "$W/logs/probe-admin.out"; exit 1; }
' _ "$W" > "$W/logs/probe-signer.out" 2>&1 || {
    cat "$W/logs/probe-signer.out" >&2
    die "the signer does not run in the sandbox (lines above); no suite was run"
}
: > "$W/logs/refused.calls"
say "signer probe: install.sh --test and k2sign-admin generate-u2 ran in a throwaway store under $W"
if [ "$SANDBOX" = 1 ]; then
    say "sandbox probe: writes outside $W, sudo, security and launchctl are all refused"
    say "  here documents, here strings, mktemp, mktemp -d and python's tempfile work, inside $W"
    say "  (writes to both homes, /tmp, /private/var/tmp and the per-user temp folder are refused)"
else
    say "already inside a sandbox (macOS refuses a nested one) and no signer is installed on this Mac:"
    say "  running under that sandbox, with the stand-ins on PATH (probed) and the state check"
    say "  here documents, here strings, mktemp, mktemp -d and python's tempfile work, inside $W"
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
        if [ "$SANDBOX" = 1 ]; then
            # What the sandbox refused lately (the system log; read only).
            echo "  sandbox denials in the last 10 minutes (if any):"
            /usr/bin/log show --last 10m --style compact \
                --predicate 'sender == "Sandbox" AND eventMessage CONTAINS "deny"' 2>/dev/null \
                | grep -F 'deny' | tail -20 | sed 's/^/    /' || true
        fi
    fi
done
if [ -s "$W/logs/refused.calls" ]; then
    rc=1
    echo "FAIL: a suite called a refused command (it was stopped, nothing ran):"
    sed 's/^/  /' "$W/logs/refused.calls"
fi

# ── 5b. the real state, after ──────────────────────────────────────────
snapshot > "$W/logs/state.after"
kc_items > "$W/logs/kc.after"
if ! /usr/bin/cmp -s "$W/logs/kc.before" "$W/logs/kc.after"; then
    rc=1
    echo "FAIL: the login keychain's items changed while the suites ran ($(wc -l < "$W/logs/kc.before" | tr -d ' ') before, $(wc -l < "$W/logs/kc.after" | tr -d ' ') after). By label:"
    # Labels only: an item added, removed, or whose mdat or identity changed.
    /usr/bin/diff "$W/logs/kc.before" "$W/logs/kc.after" | sed -n 's/^\([<>]\) \([^	]*\)	.*/  \1 \2/p' | LC_ALL=C sort -u
elif [ "$(cat "$W/logs/kc.before")" = "(no login keychain)" ]; then
    echo "no login keychain for this user: nothing to compare there"
else
    echo "unchanged: $(wc -l < "$W/logs/kc.before" | tr -d ' ') login keychain items (mdat, or label and hash)"
fi
if ! /usr/bin/diff "$W/logs/state.before" "$W/logs/state.after" > "$W/logs/state.diff"; then
    rc=1
    echo "FAIL: real state changed while the suites ran (check whether something else on this Mac changed it):"
    sed 's/^/  /' "$W/logs/state.diff" | head -40
else
    echo "unchanged: $(grep -vc '(absent)' "$W/logs/state.before") watched paths outside $W"
fi
exit "$rc"
