#!/usr/bin/env bash
# Turn this Mac into a K2 compute node (prd-k2-compute-nodes-v1 §8.3, CN27).
#
#   sudo scripts/node/install-node-macos.sh \
#     --binary ./k2-node --controller https://rosson.k2.dev \
#     --enroll ABCDE-FGHJK.0123456789abcdef --name mini-1 [--human rosson] [--no-private-home]
#
# Root is needed once. It creates the hidden service account `_k2node`
# (alias `k2node`, UID 200-400, shell /usr/bin/false, home /var/k2node),
# the group `k2nodectl` (humans who may pause the node and edit its
# policy), the config dir /etc/k2-node, copies the binary to
# /Library/Application Support/K2/node/, and loads the LaunchDaemon
# dev.k2.node. The enrollment runs AS _k2node and prints a 6-digit code
# to confirm on the controller.
#
# macOS homes are world-readable by default, so jobs (as _k2node) could
# read the human's files. By default this makes the human's home private
# (chmod 700); --no-private-home skips that.
set -euo pipefail

NODE_USER="_k2node"
NODE_ALIAS="k2node"
HOME_DIR="/var/k2node"
CONFIG_DIR="/etc/k2-node"
BIN_DIR="/Library/Application Support/K2/node"
PLIST="/Library/LaunchDaemons/dev.k2.node.plist"
BINARY=""
CONTROLLER=""
ENROLL=""
NAME=""
HUMAN="${SUDO_USER:-}"
PRIVATE_HOME=1
REENROLL=0
LABELS=()

die() { echo "install-node-macos: $*" >&2; exit 1; }
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
        -h|--help) usage ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ "$(uname -s)" = "Darwin" ] || die "this is the macOS installer; on Linux use install-node-linux.sh"
[ "$(id -u)" = "0" ] || die "run as root (sudo); root is needed once to create the node user and service"
[ -n "$BINARY" ] && [ -x "$BINARY" ] || die "--binary must point at an executable k2-node"
"$BINARY" version >/dev/null || die "$BINARY doesn't run on this Mac"
[ -n "$HUMAN" ] || HUMAN="$(stat -f %Su /dev/console)"
[ "$HUMAN" != "root" ] && [ "$HUMAN" != "$NODE_USER" ] || die "--human must be a person's account"
id -u "$HUMAN" >/dev/null 2>&1 || die "--human $HUMAN is not a user here"
xcode-select -p >/dev/null 2>&1 || echo "note: the Xcode Command Line Tools are not installed; jobs that need git or clang will fail (xcode-select --install)" >&2

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

if [ "$PRIVATE_HOME" = "1" ]; then
    human_home="$(dscl . -read "/Users/$HUMAN" NFSHomeDirectory | awk '{print $2}')"
    if [ -d "$human_home" ]; then
        chmod 700 "$human_home"
        echo "made $human_home private (chmod 700) so jobs can't read it"
    fi
fi

echo "== enroll"
run_as_node() { sudo -u "$NODE_USER" -H env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$@"; }
if [ -n "$ENROLL" ]; then
    [ -n "$CONTROLLER" ] && [ -n "$NAME" ] || die "--enroll needs --controller and --name"
    force=()
    [ "$REENROLL" = "1" ] && force=(--force)
    run_as_node "$BIN" enroll --controller "$CONTROLLER" --enroll "$ENROLL" --name "$NAME" \
        --home "$HOME_DIR" ${LABELS[@]+"${LABELS[@]}"} ${force[@]+"${force[@]}"}
elif [ ! -f "$HOME_DIR/state/controller.json" ]; then
    die "not enrolled yet: pass --controller, --enroll and --name (mint a code with \`k2 compute node add <name>\` on the controller)"
fi

echo "== LaunchDaemon"
launchctl bootout system/dev.k2.node 2>/dev/null || true
launchctl bootstrap system "$PLIST"
sleep 2
launchctl print system/dev.k2.node 2>/dev/null | grep -E "state =|pid =" || true
cat <<EOF

Done. $NAME runs jobs as $NODE_USER once the code above is confirmed on the controller.
  status:  "$BIN" status --home $HOME_DIR
  pause:   "$BIN" pause [--now]     (you are in k2nodectl after a new login; or use sudo)
  policy:  "$BIN" policy show | set cpu_cap_percent=50
  log:     $HOME_DIR/log/k2-node.log
  remove:  sudo scripts/node/uninstall-node-macos.sh
Rust for jobs: sudo scripts/node/install-rust-for-node.sh --nextest
EOF
