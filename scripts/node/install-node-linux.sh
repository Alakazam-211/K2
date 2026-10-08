#!/usr/bin/env bash
# Turn this Linux machine into a K2 compute node (prd-k2-compute-nodes-v1 §8.3).
#
#   sudo scripts/node/install-node-linux.sh \
#     --binary ./k2-node --controller https://rosson.k2.dev \
#     --enroll ABCDE-FGHJK.0123456789abcdef --name z13flow [--human z3thon] [--label rack=home]
#
# Root is needed once: it creates the system user `k2node` (no login, no
# sudo), the group `k2nodectl` (the humans who may pause the node and edit
# its policy), the home /var/lib/k2node, the config dir /etc/k2-node, and
# the systemd unit `k2-node.service` (Delegate=yes for cgroup caps). The
# enrollment runs AS k2node, prints a 6-digit code, and the owner confirms
# that code on the controller (`k2 compute node confirm <name> <code>`).
#
# It never touches the human's files, logins or keys, and refuses when the
# node user's home holds K2 daemon files (someone runs K2 as that user).
set -euo pipefail

NODE_USER="k2node"
HOME_DIR="/var/lib/k2node"
CONFIG_DIR="/etc/k2-node"
LIBEXEC="/usr/local/libexec"
UNIT="/etc/systemd/system/k2-node.service"
BINARY=""
CONTROLLER=""
ENROLL=""
NAME=""
HUMAN="${SUDO_USER:-}"
REENROLL=0
LABELS=()

die() { echo "install-node-linux: $*" >&2; exit 1; }
usage() { sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --binary) BINARY="${2:-}"; shift 2 ;;
        --controller) CONTROLLER="${2:-}"; shift 2 ;;
        --enroll) ENROLL="${2:-}"; shift 2 ;;
        --name) NAME="${2:-}"; shift 2 ;;
        --human) HUMAN="${2:-}"; shift 2 ;;
        --label) LABELS+=("--label" "${2:-}"); shift 2 ;;
        --reenroll) REENROLL=1; shift ;;
        -h|--help) usage ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ "$(uname -s)" = "Linux" ] || die "this is the Linux installer; on a Mac use install-node-macos.sh"
[ "$(id -u)" = "0" ] || die "run as root (sudo); root is needed once to create the node user and service"
[ -n "$BINARY" ] && [ -x "$BINARY" ] || die "--binary must point at an executable k2-node"
"$BINARY" version >/dev/null || die "$BINARY doesn't run on this machine"
command -v systemctl >/dev/null || die "systemd is required"
[ "$HUMAN" != "$NODE_USER" ] || die "--human must be a person's account, not $NODE_USER"
if [ -n "$HUMAN" ]; then
    id -u "$HUMAN" >/dev/null 2>&1 || die "--human $HUMAN is not a user here"
fi

# CN25: never reuse an account that runs K2 itself.
if id -u "$NODE_USER" >/dev/null 2>&1; then
    existing_home="$(getent passwd "$NODE_USER" | cut -d: -f6)"
    [ -n "$existing_home" ] && "$BINARY" check-home "$existing_home"
fi

run_as_node() {
    if command -v runuser >/dev/null; then
        runuser -u "$NODE_USER" -- env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$@"
    else
        sudo -u "$NODE_USER" -H env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$@"
    fi
}

NOLOGIN="$(command -v nologin || echo /usr/sbin/nologin)"

echo "== group k2nodectl and user $NODE_USER"
getent group k2nodectl >/dev/null || groupadd --system k2nodectl
if ! id -u "$NODE_USER" >/dev/null 2>&1; then
    useradd --system --user-group --home-dir "$HOME_DIR" --no-create-home \
        --shell "$NOLOGIN" --comment "K2 compute node" "$NODE_USER"
fi
if id -nG "$NODE_USER" | tr ' ' '\n' | grep -qx k2nodectl; then
    die "$NODE_USER must not be in k2nodectl (it would let jobs edit the owner's policy)"
fi
[ -n "$HUMAN" ] && usermod -aG k2nodectl "$HUMAN"

echo "== files"
install -d -m 0750 -o "$NODE_USER" -g k2nodectl "$HOME_DIR"
install -d -m 0755 -o "$NODE_USER" -g "$NODE_USER" "$HOME_DIR/log"
install -d -m 0755 "$LIBEXEC"
install -m 0755 -o root -g root "$BINARY" "$LIBEXEC/k2-node"
BIN="$LIBEXEC/k2-node"
install -d -m 2775 -o root -g k2nodectl "$CONFIG_DIR"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$BIN" install-files --os linux --binary "$BIN" --home "$HOME_DIR" --config-dir "$CONFIG_DIR" \
    --user "$NODE_USER" --group "$NODE_USER" --out "$tmp" >/dev/null
for f in policy.toml control.toml; do
    [ -f "$CONFIG_DIR/$f" ] || install -m 0664 -o root -g k2nodectl "$tmp/$f" "$CONFIG_DIR/$f"
done
install -m 0644 -o root -g root "$tmp/k2-node.service" "$UNIT"

echo "== enroll"
if [ -n "$ENROLL" ]; then
    [ -n "$CONTROLLER" ] && [ -n "$NAME" ] || die "--enroll needs --controller and --name"
    force=()
    [ "$REENROLL" = "1" ] && force=(--force)
    run_as_node "$BIN" enroll --controller "$CONTROLLER" --enroll "$ENROLL" --name "$NAME" \
        --home "$HOME_DIR" ${LABELS[@]+"${LABELS[@]}"} ${force[@]+"${force[@]}"}
elif [ ! -f "$HOME_DIR/state/controller.json" ]; then
    die "not enrolled yet: pass --controller, --enroll and --name (mint a code with \`k2 compute node add <name>\` on the controller)"
fi

echo "== service"
systemctl daemon-reload
systemctl enable --now k2-node.service
sleep 2
systemctl --no-pager --lines=0 status k2-node.service || true
cat <<EOF

Done. $NAME runs jobs as $NODE_USER once the code above is confirmed on the controller.
  status:  $BIN status --home $HOME_DIR
  pause:   $BIN pause [--now]      (you are in k2nodectl after a new login; or use sudo)
  policy:  $BIN policy show | set cpu_cap_percent=50
  remove:  sudo scripts/node/uninstall-node-linux.sh
Rust for jobs: sudo scripts/node/install-rust-for-node.sh --nextest
EOF
