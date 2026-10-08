#!/usr/bin/env bash
# S0(a) spike — prd-k2-compute-nodes-v1 §18/§21.4.
#
# Runs K2's Rust tests as a fresh, hidden, throwaway OS user — the way a
# compute node's jobs will run — while the machine's own K2 keeps running,
# and lists the tests that need a desktop session (Aqua, keychain, Touch
# ID, Trash, launchctl) or the human's home. Nothing is silently skipped:
# every failure is listed with a guessed reason.
#
# Run ON the target machine (a Mac mini or z13flow) as root:
#   scp scripts/node/spikes/s0a-tests-as-hidden-user.sh <user>@<host>:
#   ssh -t <user>@<host> sudo bash s0a-tests-as-hidden-user.sh --sha <commit>
# Options:
#   --sha REV        commit of github.com/Alakazam-211/K2 to test (required)
#   --packages "…"   cargo -p list (default: k2-node-proto k2-node k2-core k2-daemon)
#   --jobs N         cargo build jobs (default: half the cores)
#   --keep           keep the throwaway user and its home afterwards
#   --report PATH    where to write the report (default /var/tmp/k2-s0a-report.txt)
#
# Linux: waits while /home/sew-ci/sew-build/.job.lock exists and holds
# /var/tmp/k2-smoke.lock for the whole run (z13flow is shared with Sew CI).
# Needs network (rustup, crates.io, github.com). Takes 30–90 minutes.

set -euo pipefail

SHA=""
PACKAGES="k2-node-proto k2-node k2-core k2-daemon"
JOBS=""
KEEP=0
REPORT="/var/tmp/k2-s0a-report.txt"
while [ $# -gt 0 ]; do
    case "$1" in
        --sha) SHA="${2:-}"; shift 2 ;;
        --packages) PACKAGES="${2:-}"; shift 2 ;;
        --jobs) JOBS="${2:-}"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --report) REPORT="${2:-}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ "$(id -u)" = "0" ] || { echo "run as root (sudo): this creates and removes a throwaway user" >&2; exit 2; }
[ -n "$SHA" ] || { echo "--sha <commit> is required" >&2; exit 2; }
case "$SHA" in *[!0-9a-fA-F]*) echo "--sha must be a hex commit" >&2; exit 2 ;; esac

OS="$(uname -s)"
SPIKE_USER="k2spike"
LOCK_SEW="/home/sew-ci/sew-build/.job.lock"
LOCK_K2="/var/tmp/k2-smoke.lock"
WROTE_LOCK=0

cleanup() {
    local rc=$?
    if [ "$WROTE_LOCK" = "1" ]; then rm -f "$LOCK_K2"; fi
    if [ "$KEEP" = "0" ]; then
        if [ "$OS" = "Darwin" ]; then
            dscl . -delete "/Users/_${SPIKE_USER}" 2>/dev/null || true
            dscl . -delete "/Groups/_${SPIKE_USER}" 2>/dev/null || true
            rm -rf "/var/${SPIKE_USER}"
        else
            userdel -r "$SPIKE_USER" 2>/dev/null || true
            rm -rf "/var/lib/${SPIKE_USER}"
        fi
    fi
    exit "$rc"
}
trap cleanup EXIT

if [ "$OS" = "Linux" ]; then
    while [ -e "$LOCK_SEW" ]; do
        echo "waiting: Sew CI holds $LOCK_SEW"; sleep 30
    done
    if [ -e "$LOCK_K2" ]; then
        echo "another K2 smoke holds $LOCK_K2:" >&2; cat "$LOCK_K2" >&2; exit 3
    fi
    printf 'k2 compute S0(a) spike\nstarted %s\npid %s\n' "$(date -u +%FT%TZ)" "$$" > "$LOCK_K2"
    WROTE_LOCK=1
fi

# ── the throwaway user (same shape as the node user) ─────────────────
if [ "$OS" = "Darwin" ]; then
    HOME_DIR="/var/${SPIKE_USER}"
    REC="_${SPIKE_USER}"
    if ! dscl . -read "/Users/$REC" >/dev/null 2>&1; then
        uid=""
        for c in $(seq 400 -1 200); do
            if ! dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$c" && \
               ! dscl . -list /Groups PrimaryGroupID | awk '{print $2}' | grep -qx "$c"; then uid="$c"; break; fi
        done
        [ -n "$uid" ] || { echo "no free id in 200–400" >&2; exit 1; }
        dscl . -create "/Groups/$REC" PrimaryGroupID "$uid"
        dscl . -create "/Users/$REC"
        dscl . -create "/Users/$REC" RecordName "$REC" "$SPIKE_USER"
        dscl . -create "/Users/$REC" UniqueID "$uid"
        dscl . -create "/Users/$REC" PrimaryGroupID "$uid"
        dscl . -create "/Users/$REC" UserShell /usr/bin/false
        dscl . -create "/Users/$REC" NFSHomeDirectory "$HOME_DIR"
        dscl . -create "/Users/$REC" IsHidden 1
    fi
    mkdir -p "$HOME_DIR"
    chown "$REC:$REC" "$HOME_DIR"
    chmod 700 "$HOME_DIR"
    RUN_AS=(sudo -u "$REC")
else
    HOME_DIR="/var/lib/${SPIKE_USER}"
    if ! getent passwd "$SPIKE_USER" >/dev/null; then
        useradd --system --create-home --home-dir "$HOME_DIR" --shell /usr/sbin/nologin "$SPIKE_USER"
    fi
    chmod 700 "$HOME_DIR"
    RUN_AS=(sudo -u "$SPIKE_USER")
fi

CORES="$( (nproc 2>/dev/null || sysctl -n hw.ncpu) | head -1)"
[ -n "$JOBS" ] || JOBS=$(( CORES / 2 > 0 ? CORES / 2 : 1 ))

# Everything below runs as the throwaway user with an EMPTY env, like a job.
JOB_PATH="$HOME_DIR/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
as_user() {
    "${RUN_AS[@]}" env -i HOME="$HOME_DIR" PATH="$JOB_PATH" LANG=C.UTF-8 CI=1 TMPDIR="$HOME_DIR/tmp" \
        CARGO_HOME="$HOME_DIR/.cargo" RUSTUP_HOME="$HOME_DIR/.rustup" "$@"
}
as_user mkdir -p "$HOME_DIR/tmp"

echo "== toolchain (as $SPIKE_USER) =="
if ! as_user test -x "$HOME_DIR/.cargo/bin/cargo"; then
    as_user /bin/sh -c 'curl -fsSL https://sh.rustup.rs -o "$HOME/rustup-init.sh" && sh "$HOME/rustup-init.sh" -y --no-modify-path --profile minimal'
fi
if ! as_user /bin/sh -c 'cargo nextest --version' >/dev/null 2>&1; then
    as_user cargo install cargo-nextest --locked
fi

echo "== source $SHA =="
as_user /bin/sh -c "rm -rf \"\$HOME/K2\" && git init -q \"\$HOME/K2\" && cd \"\$HOME/K2\" && git fetch -q --depth 1 https://github.com/Alakazam-211/K2.git $SHA && git checkout -q FETCH_HEAD"

echo "== tests: $PACKAGES =="
PFLAGS=""
for p in $PACKAGES; do PFLAGS="$PFLAGS -p $p"; done
start=$(date +%s)
set +e
as_user /bin/sh -c "cd \"\$HOME/K2\" && CFLAGS=-DHAVE_STRCHRNUL CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo nextest run $PFLAGS --no-fail-fast -j $JOBS --build-jobs $JOBS 2>&1" > "$HOME_DIR/nextest.log"
rc=$?
set -e
took=$(( $(date +%s) - start ))

# ── report ───────────────────────────────────────────────────────────
{
    echo "S0(a) — K2 tests as a hidden throwaway user"
    echo "host: $(hostname)  os: $OS $(uname -r)  arch: $(uname -m)  cores: $CORES"
    echo "commit: $SHA  packages: $PACKAGES  nextest rc: $rc  took: ${took}s"
    echo
    echo "summary:"
    grep -E "Summary|tests run|passed|failed|skipped" "$HOME_DIR/nextest.log" | tail -5
    echo
    echo "failures (with a guessed reason; none are skipped):"
    grep -E "^\s+FAIL \[" "$HOME_DIR/nextest.log" | sed -E 's/^\s+FAIL \[[^]]*\]\s*//' | sort -u | while read -r crate test; do
        name="$crate $test"
        why="other"
        ctx="$(grep -F -A40 "$test" "$HOME_DIR/nextest.log" | head -80)"
        case "$name $ctx" in
            *[Kk]eychain*|*SecItem*|*errSec*|*security\ find*) why="keychain (human session)" ;;
            *LAContext*|*TouchID*|*[Bb]iometr*) why="Touch ID (human session)" ;;
            *[Tt]rash*|*NSWorkspace*|*recycle*) why="Trash / NSWorkspace (Aqua session)" ;;
            *launchctl*|*gui/[0-9]*|*LaunchAgent*) why="launchctl user domain (human session)" ;;
            *osascript*|*AppleEvent*|*TCC*) why="AppleScript / TCC (Aqua session)" ;;
            *DBUS*|*dbus*|*XDG_RUNTIME_DIR*|*systemctl\ --user*) why="user session bus (Linux desktop session)" ;;
            *"Permission denied"*|*EACCES*) why="permissions (node user can't read something)" ;;
            *"No such file"*|*ENOENT*) why="missing tool or file on PATH" ;;
        esac
        printf '  %-70s %s\n' "$name" "$why"
    done
} > "$REPORT"
cat "$REPORT"
echo "(full log: $HOME_DIR/nextest.log$([ "$KEEP" = 0 ] && echo ' — removed with the user; pass --keep to keep it'))"
[ "$KEEP" = "0" ] && cp "$HOME_DIR/nextest.log" "${REPORT%.txt}.log" && echo "(log copied to ${REPORT%.txt}.log)"
exit 0
