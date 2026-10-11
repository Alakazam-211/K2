#!/usr/bin/env bash
# Turn this Linux machine into a K2 compute node (prd-k2-compute-nodes-v1 §8.3).
#
#   sudo k2-node setup [--private-home | --no-private-home] [--human USER]
#        [--allow-packages "libssl-dev pkg-config"] [--tools "node@22 rust@stable"]
#        [--controller https://you.k2.dev --enroll ABCDE-FGHJK.0123456789abcdef --name NAME]
#   (or: sudo scripts/node/install-node-linux.sh --binary ./k2-node …)
#
# Root is needed once: it creates the system user `k2node` (no login, no
# sudo), the group `k2nodectl` (the
# humans who may pause the node and edit its policy), the home
# /var/lib/k2node, the config dir /etc/k2-node, and the systemd unit
# `k2-node.service` (Delegate=yes for cgroup caps).
#
# Enrolling is optional here: without --enroll the node starts unenrolled
# and the machine's K2 (`k2 compute local enable`, Settings → Compute)
# or `k2-node request` enrolls it later, as the node user.
#
# Remote installs (SSH): one sudo call with the password on stdin, never
# in argv:  ssh host 'sudo -S -p "" /path/k2-node setup --no-private-home …' < pwfile
#
# It never touches the human's files, logins or keys (the private-home
# step is shown and asked, or set with a flag), and refuses when the node
# user's home holds K2 daemon files (someone runs K2 as that user).
set -euo pipefail
# Every step runs from / : started inside a home this script makes
# private, child processes running as k2node couldn't read their cwd.
cd /

NODE_USER="k2node"
HOME_DIR="/var/lib/k2node"
CONFIG_DIR="/etc/k2-node"
LIBEXEC="/usr/local/libexec"
UNIT="/etc/systemd/system/k2-node.service"
PKG_PATH_UNIT="/etc/systemd/system/k2-node-packages.path"
PKG_SERVICE_UNIT="/etc/systemd/system/k2-node-packages.service"
BINARY=""
CONTROLLER=""
ENROLL=""
NAME=""
HUMAN="${SUDO_USER:-}"
REENROLL=0
PRIVATE_HOME=""
ALLOW_PACKAGES=""
PACKAGE_RULE=1
TOOLS=""
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
        --private-home) PRIVATE_HOME=1; shift ;;
        --no-private-home) PRIVATE_HOME=0; shift ;;
        --allow-packages) ALLOW_PACKAGES="${2:-}"; shift 2 ;;
        --no-package-installs) PACKAGE_RULE=0; shift ;;
        --tools) TOOLS="${2:-}"; shift 2 ;;
        -h|--help) usage ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ "$(uname -s)" = "Linux" ] || die "this is the Linux installer; on a Mac use install-node-macos.sh"
[ "$(id -u)" = "0" ] || die "run as root (sudo); root is needed once to create the node user and service.
  Over SSH: ssh host 'sudo -S -p \"\" $0 …' < password-file (the password never goes in argv)"
[ -n "$BINARY" ] && [ -x "$BINARY" ] || die "--binary must point at an executable k2-node"
"$BINARY" version >/dev/null || die "$BINARY doesn't run on this machine (wrong architecture? this is $(uname -m))"
command -v systemctl >/dev/null || die "systemd is required (no systemctl found)"
[ "$HUMAN" != "$NODE_USER" ] || die "--human must be a person's account, not $NODE_USER"
if [ -n "$HUMAN" ]; then
    id -u "$HUMAN" >/dev/null 2>&1 || die "--human $HUMAN is not a user here"
fi
if [ -n "$ENROLL" ] || [ -n "$CONTROLLER" ]; then
    [ -n "$ENROLL" ] && [ -n "$CONTROLLER" ] && [ -n "$NAME" ] || die "--enroll needs --controller and --name (mint a code with \`k2 compute node add <name>\` on the controller)"
fi

# CN25: never reuse an account that runs K2 itself.
if id -u "$NODE_USER" >/dev/null 2>&1; then
    existing_home="$(getent passwd "$NODE_USER" | cut -d: -f6)"
    [ -n "$existing_home" ] && "$BINARY" check-home "$existing_home"
fi

# ── the human's home: shown and asked, never silent ──────────────────
HOME_STATUS="not checked (no --human)"
if [ -n "$HUMAN" ]; then
    human_home="$(getent passwd "$HUMAN" | cut -d: -f6)"
    if [ -d "$human_home" ]; then
        mode="$(stat -c %a "$human_home")"
        other=$(( 8#$mode & 8#005 ))
        if [ "$other" = "0" ]; then
            HOME_STATUS="$human_home is already private (mode $mode)"
        else
            echo "== $human_home is readable by other users (mode $mode)"
            echo "   Jobs run as $NODE_USER and could read anything there that is readable by everyone, e.g.:"
            for spot in .ssh .k2 .env .config .aws DevProjects projects src code; do
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
fi

run_as_node() {
    if command -v runuser >/dev/null; then
        (cd / && runuser -u "$NODE_USER" -- env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$@")
    else
        (cd / && sudo -u "$NODE_USER" -H env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$@")
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
if [ "$(readlink -f "$BINARY")" != "$LIBEXEC/k2-node" ]; then
    install -m 0755 -o root -g root "$BINARY" "$LIBEXEC/k2-node.new"
    mv -f "$LIBEXEC/k2-node.new" "$LIBEXEC/k2-node"
fi
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

# The human's K2 must reach the node now, not after a new login (group
# membership only applies to new logins): per-user ACLs on the config dir
# (pause, policy, enroll requests) and on the node home (status).
ACCESS_STATUS="no --human: only root can pause the node or edit its policy"
if [ -n "$HUMAN" ]; then
    if command -v setfacl >/dev/null && setfacl -m "u:$HUMAN:rwx" "$CONFIG_DIR" 2>/dev/null; then
        setfacl -m "u:$HUMAN:x" "$HOME_DIR"
        for f in policy.toml control.toml; do setfacl -m "u:$HUMAN:rw" "$CONFIG_DIR/$f"; done
        ACCESS_STATUS="$HUMAN can pause, edit policy and enroll from K2 now (ACLs; also in k2nodectl)"
    else
        ACCESS_STATUS="$HUMAN is in k2nodectl; K2 on this machine can manage the node after $HUMAN logs in again (no setfacl here)"
    fi
fi

# ── system packages: the owner's approved list + a root path unit ─────
# The node runs under NoNewPrivileges (no sudo, no setuid). A job that
# needs `sys:<name>` makes k2-node drop a request in
# $HOME_DIR/run/pkg-requests/; the root unit below installs only names on
# the root-owned /etc/k2-node-packages.toml and answers in
# /run/k2-node-packages/. k2node has no sudo at all.
PKG_STATUS="jobs can't install system packages (--no-package-installs)"
if [ "$PACKAGE_RULE" = "1" ]; then
    if [ -n "$ALLOW_PACKAGES" ]; then
        # shellcheck disable=SC2086
        "$BIN" packages allow $ALLOW_PACKAGES >/dev/null
    fi
    install -d -m 0755 -o "$NODE_USER" -g "$NODE_USER" "$HOME_DIR/run"
    install -d -m 0700 -o "$NODE_USER" -g "$NODE_USER" "$HOME_DIR/run/pkg-requests"
    cat > "$PKG_PATH_UNIT" <<EOF
# K2 compute node: install owner-approved system packages for jobs.
[Unit]
Description=K2 compute node package requests

[Path]
DirectoryNotEmpty=$HOME_DIR/run/pkg-requests

[Install]
WantedBy=paths.target
EOF
    cat > "$PKG_SERVICE_UNIT" <<EOF
# K2 compute node: answers package requests from k2node. Installs only
# names on /etc/k2-node-packages.toml (root-owned), never anything else.
[Unit]
Description=K2 compute node package installer (owner-approved list only)

[Service]
Type=oneshot
ExecStart=$BIN packages serve --home $HOME_DIR
RuntimeDirectory=k2-node-packages
RuntimeDirectoryMode=0755
RuntimeDirectoryPreserve=yes
EOF
    chmod 0644 "$PKG_PATH_UNIT" "$PKG_SERVICE_UNIT"
    approved="$("$BIN" packages list | sed -n 's/^approved: //p' | tr '\n' ' ')"
    PKG_STATUS="approved: ${approved:-(none)} — jobs install these on demand (k2-node-packages.path); approve more with: sudo $BIN packages allow <name>"
else
    rm -f "$PKG_PATH_UNIT" "$PKG_SERVICE_UNIT"
fi

echo "== service"
systemctl daemon-reload
systemctl enable k2-node.service >/dev/null
if [ -f "$PKG_PATH_UNIT" ]; then
    systemctl enable --now k2-node-packages.path >/dev/null
else
    systemctl disable --now k2-node-packages.path >/dev/null 2>&1 || true
fi

if [ -n "$ENROLL" ]; then
    echo "== enroll"
    force=()
    [ "$REENROLL" = "1" ] && force=(--force)
    run_as_node "$BIN" enroll --controller "$CONTROLLER" --enroll "$ENROLL" --name "$NAME" \
        --home "$HOME_DIR" ${LABELS[@]+"${LABELS[@]}"} ${force[@]+"${force[@]}"}
fi
systemctl restart k2-node.service
sleep 2
systemctl --no-pager --lines=0 status k2-node.service >/dev/null 2>&1 || die "k2-node.service didn't start: journalctl -u k2-node -n 50"

TOOLS_STATUS="none preinstalled (jobs install what they declare on demand)"
if [ -n "$TOOLS" ]; then
    echo "== toolchains: $TOOLS"
    # shellcheck disable=SC2086
    if run_as_node "$BIN" tools install --home "$HOME_DIR" $TOOLS; then
        TOOLS_STATUS="installed: $TOOLS"
    else
        TOOLS_STATUS="FAILED to install: $TOOLS (see above; retry: sudo -u $NODE_USER $BIN tools install $TOOLS)"
    fi
fi

if [ -f "$HOME_DIR/state/controller.json" ]; then
    ENROLL_STATUS="enrolled (confirm the code above on the controller if you haven't)"
else
    ENROLL_STATUS="not enrolled yet: k2 compute local enable --controller URL --enroll CODE --name NAME (as $HUMAN), or sudo -u $NODE_USER $BIN enroll …"
fi
cat <<EOF

Done.
  service:     k2-node.service (running as $NODE_USER)
  enrollment:  $ENROLL_STATUS
  home folder: $HOME_STATUS
  your access: $ACCESS_STATUS
  packages:    $PKG_STATUS
  toolchains:  $TOOLS_STATUS
  status:  $BIN status --home $HOME_DIR
  pause:   $BIN pause [--now]      (you are in k2nodectl after a new login; or use sudo)
  policy:  $BIN policy show | set cpu_cap_percent=50
  remove:  sudo $BIN uninstall
EOF
