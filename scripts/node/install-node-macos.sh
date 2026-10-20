#!/usr/bin/env bash
# Turn this Mac into a K2 compute node (prd-k2-compute-nodes-v1 §8.3, CN27).
#
#   sudo k2-node setup (--private-home | --no-private-home) [--human rosson] [--tools "rust@stable cargo-nextest"]
#        [--controller https://rosson.k2.dev --enroll ABCDE-FGHJK.0123456789abcdef --name mini-1]
#   (or: sudo scripts/node/install-node-macos.sh --binary ./k2-node …)
#
# Root is needed once. It creates the hidden service account `_k2node`
# (alias `k2node`, UID 200-400, shell /usr/bin/false, home /var/k2node),
# the group `k2nodectl` (humans who may pause the node and edit its
# policy), the config dir /etc/k2-node, copies the binary to
# /Library/Application Support/K2/node/, and loads the LaunchDaemon
# dev.k2.node. The enrollment runs AS _k2node and prints a 6-digit code
# to confirm on the controller.
#
# macOS homes are world-readable by default. Jobs run under a Seatbelt
# profile that closes /Users (0.46.3), but other local users (and older
# nodes) still read them. Making the human's home private (chmod 700) is
# shown and asked in a terminal; without a terminal (SSH with the
# password on stdin) pass --private-home or --no-private-home. Other
# people's open homes are listed, and fixed only when asked.
#
# Remote installs (SSH): one sudo call, the password on stdin, never in
# argv:  ssh mini 'sudo -S -p "" /path/k2-node setup --no-private-home …' < pwfile
#
# Limits on macOS: root (an admin password) is needed once; jobs can't
# install system packages (the owner runs brew); the binary must be the
# signed one from K2.app or have its quarantine flag removed.
set -euo pipefail
# Every step runs from / (the human's home may become 0700 below).
cd /

NODE_USER="_k2node"
NODE_ALIAS="k2node"
HOME_DIR="/var/k2node"
CONFIG_DIR="/etc/k2-node"
BIN_DIR="/Library/Application Support/K2/node"
PLIST="/Library/LaunchDaemons/dev.k2.node.plist"
ADMIN_PLIST="/Library/LaunchDaemons/dev.k2.node-admin.plist"
ADMIN_STATE="/var/db/k2-node-admin"
ADMIN_SOCKET="/var/run/k2-node-admin.sock"
REMOTE_ADMIN=""
BINARY=""
CONTROLLER=""
ENROLL=""
NAME=""
HUMAN="${SUDO_USER:-}"
PRIVATE_HOME=""
REENROLL=0
TOOLS=""
LABELS=()

die() { echo "install-node-macos: $*" >&2; exit 1; }
# bootout returns before launchd lets go of the label; a bootstrap right
# after fails with "5: Input/output error" and leaves the service unloaded.
reload_daemon() {
    local label=$1 plist=$2 i
    launchctl bootout "system/$label" 2>/dev/null || true
    for i in $(seq 1 50); do
        launchctl print "system/$label" >/dev/null 2>&1 || break
        sleep 0.2
    done
    for i in 1 2 3; do
        launchctl bootstrap system "$plist" 2>/dev/null && return 0
        sleep 1
    done
    launchctl bootstrap system "$plist"
}
usage() { sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --binary) BINARY="${2:-}"; shift 2 ;;
        --controller) CONTROLLER="${2:-}"; shift 2 ;;
        --enroll) ENROLL="${2:-}"; shift 2 ;;
        --name) NAME="${2:-}"; shift 2 ;;
        --human) HUMAN="${2:-}"; shift 2 ;;
        --label) LABELS+=("--label" "${2:-}"); shift 2 ;;
        --private-home) PRIVATE_HOME=1; shift ;;
        --no-private-home) PRIVATE_HOME=0; shift ;;
        --reenroll) REENROLL=1; shift ;;
        --tools) TOOLS="${2:-}"; shift 2 ;;
        --remote-admin) REMOTE_ADMIN="${2:-}"; shift 2 ;;
        --allow-packages) echo "note: --allow-packages is Linux-only; on a Mac the owner installs packages with brew" >&2; shift 2 ;;
        -h|--help) usage ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ "$(uname -s)" = "Darwin" ] || die "this is the macOS installer; on Linux use install-node-linux.sh"
[ "$(id -u)" = "0" ] || die "run as root (sudo); root is needed once to create the node user and service.
  Over SSH: ssh mini 'sudo -S -p \"\" $0 …' < password-file (the password never goes in argv)"
[ -n "$BINARY" ] && [ -x "$BINARY" ] || die "--binary must point at an executable k2-node"
"$BINARY" version >/dev/null 2>&1 || die "$BINARY doesn't run on this Mac (wrong architecture, or Gatekeeper blocked an unsigned copy: xattr -d com.apple.quarantine $BINARY)"
[ -n "$HUMAN" ] || HUMAN="$(stat -f %Su /dev/console)"
[ "$HUMAN" != "root" ] && [ "$HUMAN" != "$NODE_USER" ] || die "--human must be a person's account"
id -u "$HUMAN" >/dev/null 2>&1 || die "--human $HUMAN is not a user here"
CLT_STATUS="installed"
if ! xcode-select -p >/dev/null 2>&1; then
    CLT_STATUS="MISSING: jobs that need git or clang fail until the owner runs: xcode-select --install"
    echo "note: the Xcode Command Line Tools are not installed; jobs that need git or clang will fail (xcode-select --install)" >&2
fi
case "$REMOTE_ADMIN" in ""|off|policy|full) ;; *) die "--remote-admin must be off, policy or full" ;; esac
if [ -n "$ENROLL" ] || [ -n "$CONTROLLER" ]; then
    [ -n "$ENROLL" ] && [ -n "$CONTROLLER" ] && [ -n "$NAME" ] || die "--enroll needs --controller and --name (mint a code with \`k2 compute node add <name>\` on the controller)"
fi

# CN25: never reuse an account that runs K2 itself.
if dscl . -read "/Users/$NODE_USER" >/dev/null 2>&1; then
    existing_home="$(dscl . -read "/Users/$NODE_USER" NFSHomeDirectory | awk '{print $2}')"
    [ -n "$existing_home" ] && "$BINARY" check-home "$existing_home"
fi

# A UID/GID in 200-400 free for both users and groups.
free_id() {
    local used id
    used="$( { dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID; } | awk '{print $2}')"
    for id in $(seq 200 400); do
        if ! printf '%s\n' "$used" | grep -qx "$id"; then
            echo "$id"
            return 0
        fi
    done
    return 1
}

echo "== groups and the hidden user $NODE_USER"
if ! dscl . -read "/Groups/$NODE_USER" >/dev/null 2>&1; then
    gid="$(free_id)" || die "no free id in 200-400"
    dscl . -create "/Groups/$NODE_USER"
    dscl . -create "/Groups/$NODE_USER" PrimaryGroupID "$gid"
    dscl . -create "/Groups/$NODE_USER" RealName "K2 compute node"
    dscl . -create "/Groups/$NODE_USER" Password '*'
fi
gid="$(dscl . -read "/Groups/$NODE_USER" PrimaryGroupID | awk '{print $2}')"
if ! dscl . -read "/Users/$NODE_USER" >/dev/null 2>&1; then
    uid="$gid"
    if dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$uid"; then
        uid="$(free_id)" || die "no free id in 200-400"
    fi
    dscl . -create "/Users/$NODE_USER"
    dscl . -create "/Users/$NODE_USER" UniqueID "$uid"
    dscl . -create "/Users/$NODE_USER" PrimaryGroupID "$gid"
    dscl . -create "/Users/$NODE_USER" UserShell /usr/bin/false
    dscl . -create "/Users/$NODE_USER" NFSHomeDirectory "$HOME_DIR"
    dscl . -create "/Users/$NODE_USER" RealName "K2 compute node"
    dscl . -create "/Users/$NODE_USER" IsHidden 1
    dscl . -create "/Users/$NODE_USER" Password '*'
    dscl . -append "/Users/$NODE_USER" RecordName "$NODE_ALIAS"
fi
if ! dscl . -read /Groups/k2nodectl >/dev/null 2>&1; then
    cgid="$(free_id)" || die "no free id in 200-400"
    dscl . -create /Groups/k2nodectl
    dscl . -create /Groups/k2nodectl PrimaryGroupID "$cgid"
    dscl . -create /Groups/k2nodectl RealName "K2 compute node owners"
    dscl . -create /Groups/k2nodectl Password '*'
fi
if dseditgroup -o checkmember -m "$NODE_USER" k2nodectl >/dev/null 2>&1; then
    die "$NODE_USER must not be in k2nodectl (it would let jobs edit the owner's policy)"
fi
dseditgroup -o edit -a "$HUMAN" -t user k2nodectl

echo "== files"
install -d -m 0750 -o "$NODE_USER" -g k2nodectl "$HOME_DIR"
install -d -m 0755 -o "$NODE_USER" -g "$NODE_USER" "$HOME_DIR/log"
install -d -m 0755 -o root -g wheel "$BIN_DIR"
install -m 0755 -o root -g wheel "$BINARY" "$BIN_DIR/k2-node"
xattr -d com.apple.quarantine "$BIN_DIR/k2-node" 2>/dev/null || true
BIN="$BIN_DIR/k2-node"
install -d -m 2775 -o root -g k2nodectl "$CONFIG_DIR"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$BIN" install-files --os macos --binary "$BIN" --home "$HOME_DIR" --config-dir "$CONFIG_DIR" \
    --user "$NODE_USER" --group "$NODE_USER" --out "$tmp" >/dev/null
for f in policy.toml control.toml; do
    [ -f "$CONFIG_DIR/$f" ] || install -m 0664 -o root -g k2nodectl "$tmp/$f" "$CONFIG_DIR/$f"
done
install -m 0644 -o root -g wheel "$tmp/dev.k2.node.plist" "$PLIST"

# The human's K2 must reach the node now, not after a new login: per-user
# ACLs on the config dir (pause, policy, enroll requests) and the node
# home (status). Re-runs replace the entry instead of stacking it.
chmod -a "user:$HUMAN allow list,add_file,search,add_subdirectory,delete_child" "$CONFIG_DIR" 2>/dev/null || true
chmod -a "user:$HUMAN allow search" "$HOME_DIR" 2>/dev/null || true
if chmod +a "user:$HUMAN allow list,add_file,search,add_subdirectory,delete_child" "$CONFIG_DIR" \
    && chmod +a "user:$HUMAN allow search" "$HOME_DIR"; then
    ACCESS_STATUS="$HUMAN can pause, edit policy and enroll from K2 now (ACLs; also in k2nodectl)"
else
    ACCESS_STATUS="$HUMAN is in k2nodectl; K2 can manage the node after $HUMAN logs in again"
fi

# The human's home: shown and asked, never silent.
HOME_STATUS="not checked"
human_home="$(dscl . -read "/Users/$HUMAN" NFSHomeDirectory | awk '{print $2}')"
if [ -d "$human_home" ]; then
    mode="$(stat -f %Lp "$human_home")"
    other=$(( 8#$mode & 8#005 ))
    if [ "$other" = "0" ]; then
        HOME_STATUS="$human_home is already private (mode $mode)"
    else
        echo "== $human_home is readable by other users (mode $mode)"
        echo "   Jobs run as $NODE_USER and could read anything there that is readable by everyone, e.g.:"
        for spot in .ssh .k2 .env .config .aws DevProjects Projects Documents Desktop; do
            [ -e "$human_home/$spot" ] && echo "     $human_home/$spot"
        done
        if [ -z "$PRIVATE_HOME" ]; then
            if [ -t 0 ]; then
                printf "   Make %s private (chmod 700)? [Y/n] " "$human_home"
                read -r answer
                case "$answer" in n|N|no|NO) PRIVATE_HOME=0 ;; *) PRIVATE_HOME=1 ;; esac
            else
                die "$human_home is readable by others: pass --private-home (chmod 700) or --no-private-home (leave it). Not a terminal, so I can't ask."
            fi
        fi
        if [ "$PRIVATE_HOME" = "1" ]; then
            chmod 700 "$human_home"
            HOME_STATUS="$human_home made private (was $mode, now 700)"
        else
            HOME_STATUS="$human_home left readable by others (mode $mode): jobs can read files there that are readable by everyone"
        fi
    fi
fi

# Everyone else's home (V16): shown, and made private only when asked. Jobs
# run under the Seatbelt profile (no /Users), but other local users and
# older nodes still get in.
open_homes="$("$BIN" admin homes --json 2>/dev/null || echo '[]')"
if [ "$open_homes" != "[]" ]; then
    echo "== home folders other users can read"
    "$BIN" admin homes || true
    if [ "$PRIVATE_HOME" != "0" ] && [ -t 0 ]; then
        printf "   Make these private too (chmod 700)? [y/N] "
        read -r answer
        case "$answer" in y|Y|yes) "$BIN" admin homes --fix --yes ;; esac
    fi
fi
open_homes="$("$BIN" admin homes --json 2>/dev/null || echo '[]')"
if [ "$open_homes" = "[]" ]; then
    HOMES_STATUS="every home under /Users is private"
else
    HOMES_STATUS="some homes are readable by others: $BIN admin homes (fix: sudo \"$BIN\" admin homes --fix)"
fi
if [ -x /usr/bin/sandbox-exec ]; then
    WALL_STATUS="on: jobs run under a Seatbelt profile (no node key or state, no /Users or keychains; network = none works)"
else
    WALL_STATUS="OFF: /usr/bin/sandbox-exec is missing; jobs run as $NODE_USER and can read its key"
fi

run_as_node() { (cd / && sudo -u "$NODE_USER" -H env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$@"); }
if [ -n "$ENROLL" ]; then
    echo "== enroll"
    force=()
    [ "$REENROLL" = "1" ] && force=(--force)
    run_as_node "$BIN" enroll --controller "$CONTROLLER" --enroll "$ENROLL" --name "$NAME" \
        --home "$HOME_DIR" ${LABELS[@]+"${LABELS[@]}"} ${force[@]+"${force[@]}"}
fi

# ── remote admin: the root executor (prd-k2-node-admin-v1 §5.2 option A) ──
# A LaunchDaemon with a launchd socket (0660 root:$NODE_USER), started on
# demand. It runs only commands signed by your K2 server, and root ones
# only with your own approval (Touch ID / password) for each.
if [ -z "$REMOTE_ADMIN" ] && [ ! -f "$CONFIG_DIR/admin.toml" ]; then
    if [ -t 0 ]; then
        echo "== remote admin for this Mac"
        echo "   off    = agents can only narrow limits and read health (today's behaviour)"
        echo "   policy = also change limits (widening needs your approval), pause, update k2-node"
        echo "   full   = also root actions, each approved by you (Touch ID / password) on your K2"
        printf "   Remote admin level? [off/policy/full] (off) "
        read -r answer
        case "$answer" in policy|full) REMOTE_ADMIN="$answer" ;; *) REMOTE_ADMIN="off" ;; esac
    else
        REMOTE_ADMIN="off"
    fi
fi
install -d -m 0700 -o root -g wheel "$ADMIN_STATE"
level_args=()
[ -n "$REMOTE_ADMIN" ] && level_args=(--level "$REMOTE_ADMIN")
"$BIN" admin init --config-dir "$CONFIG_DIR" --home "$HOME_DIR" ${level_args[@]+"${level_args[@]}"} >/dev/null
chown root:wheel "$CONFIG_DIR/admin.toml"
chmod 0644 "$CONFIG_DIR/admin.toml"
"$BIN" admin install-files --os macos --binary "$BIN" --home "$HOME_DIR" --config-dir "$CONFIG_DIR" \
    --state-dir "$ADMIN_STATE" --socket "$ADMIN_SOCKET" --group "$gid" --out "$tmp" >/dev/null
install -m 0644 -o root -g wheel "$tmp/dev.k2.node-admin.plist" "$ADMIN_PLIST"
reload_daemon dev.k2.node-admin "$ADMIN_PLIST" || die "dev.k2.node-admin didn't load (System Settings → General → Login Items may block background items)"
if [ -f "$HOME_DIR/state/controller.json" ]; then
    "$BIN" admin pin --config-dir "$CONFIG_DIR" --home "$HOME_DIR" >/dev/null || true
fi

echo "== LaunchDaemon"
reload_daemon dev.k2.node "$PLIST" || die "dev.k2.node didn't load (System Settings → General → Login Items may block background items)"
sleep 2
launchctl print system/dev.k2.node 2>/dev/null | grep -qE "state = running" || die "dev.k2.node didn't start: see $HOME_DIR/log/k2-node.log"

TOOLS_STATUS="none preinstalled (jobs install what they declare on demand)"
if [ -n "$TOOLS" ]; then
    echo "== toolchains: $TOOLS"
    # shellcheck disable=SC2086
    if run_as_node "$BIN" tools install --home "$HOME_DIR" $TOOLS; then
        TOOLS_STATUS="installed: $TOOLS"
    else
        TOOLS_STATUS="FAILED to install: $TOOLS (retry: sudo -u $NODE_USER \"$BIN\" tools install $TOOLS)"
    fi
fi
if [ -f "$HOME_DIR/state/controller.json" ]; then
    ENROLL_STATUS="enrolled (confirm the code above on the controller if you haven't)"
else
    ENROLL_STATUS="not enrolled yet: k2 compute local enable --controller URL --enroll CODE --name NAME (as $HUMAN)"
fi
cat <<EOF

Done.
  service:      dev.k2.node (running as $NODE_USER)
  enrollment:   $ENROLL_STATUS
  home folder:  $HOME_STATUS
  other homes:  $HOMES_STATUS
  job wall:     $WALL_STATUS
  your access:  $ACCESS_STATUS
  Xcode CLT:    $CLT_STATUS
  packages:     jobs can't install system packages on macOS; a job that needs one fails with the brew command to run
  toolchains:   $TOOLS_STATUS
  remote admin: $("$BIN" admin init --config-dir "$CONFIG_DIR" --home "$HOME_DIR" 2>&1) (dev.k2.node-admin)
  status:  "$BIN" status --home $HOME_DIR
  pause:   "$BIN" pause [--now]     (you are in k2nodectl after a new login; or use sudo)
  policy:  "$BIN" policy show | set cpu_cap_percent=50
  log:     $HOME_DIR/log/k2-node.log
  remove:  sudo "$BIN" uninstall
  admin:   sudo "$BIN" admin level off|policy|full   |   sudo "$BIN" admin audit
EOF
