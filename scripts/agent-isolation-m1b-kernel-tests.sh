#!/usr/bin/env bash
# Agent Isolation M1b: the root-owned agent cgroups, on a real kernel, with
# the root helper placing sessions for the box's registered daemon (a
# persistent test unit), and the bite: the same test with the helper moved
# away, where the forging write works again.
#
# Isolation test boxes only (it installs a unit and the root helper, and
# moves the helper aside for a moment). Needs: Linux, cgroup v2, systemd
# 254+, sudo, curl.
#
# usage:
#   scripts/agent-isolation-m1b-kernel-tests.sh --setup <artifact-dir>
#       once per box: writes the persistent unit and installs the root
#       helper (install-root-helper.sh --file) registered for it
#   scripts/agent-isolation-m1b-kernel-tests.sh <artifact-dir>
#       the run: new binaries into the unit, then the test with the helper
#       (K2_ISO_EXPECT=root) and without it (K2_ISO_EXPECT=user, the bite)
# <artifact-dir> holds agent_isolation_kernel_integration, k2-daemon,
# k2-ptyd, k2-ptyd-test-agent, k2 and (for --setup) k2-root-helper and
# install-root-helper.sh.
# env: K2_ISO_UNIT (default k2iso-daemon.service),
#      K2_ISO_PERSIST (default ~/iso-persist: the unit's binaries and HOME)
set -euo pipefail
setup=0
if [ "${1:-}" = --setup ]; then setup=1; shift; fi
dir="$(cd "${1:?usage: $0 [--setup] <artifact-dir>}" && pwd)"
unit="${K2_ISO_UNIT:-k2iso-daemon.service}"
persist="${K2_ISO_PERSIST:-$HOME/iso-persist}"
helper=/usr/local/libexec/k2-root-helper
user="$(id -un)"
[ "$(uname -s)" = Linux ] || { echo "Linux only" >&2; exit 2; }

if [ "$setup" = 1 ]; then
    for f in k2-root-helper install-root-helper.sh; do
        [ -f "$dir/$f" ] || { echo "missing $dir/$f" >&2; exit 2; }
    done
    mkdir -p "$persist/bin" "$persist/home/.k2" "$persist/home/xdg" "$persist/home/tmp" "$persist/home/DevProjects"
    chmod 700 "$persist/home/xdg"
    sudo tee "/etc/systemd/system/$unit" >/dev/null <<EOF
[Unit]
Description=K5 agent isolation test daemon (isolation test box only)

[Service]
Type=simple
User=$user
Environment=HOME=$persist/home
Environment=XDG_RUNTIME_DIR=$persist/home/xdg
Environment=TMPDIR=$persist/home/tmp
Environment=K2_PTYD=1
Environment=K2_PTYD_BIN=$persist/bin/k2-ptyd
Environment=K2_TUNNEL_LEASE=off
Environment=K2_CONNECT_BASE=http://127.0.0.1:1
Environment=K2_CERT_BROKER_URL=http://127.0.0.1:1
Environment=K2_HANDOFF_DROPIN=off
Environment=K2_HEARTBEAT_NO_SELF_HEAL=1
Environment=K2_LLM_ACCOUNTS_LOOP=0
Environment=K2SO_WATCHDOG_DISABLED=1
ExecStart=$persist/bin/k2-daemon
Delegate=yes
DelegateSubgroup=daemon
KillMode=process
Restart=no
EOF
    sudo systemctl daemon-reload
    sudo bash "$dir/install-root-helper.sh" --file "$dir/k2-root-helper" --user "$user" --unit "$unit" --scope system
    echo "setup: $unit written, the root helper installed for it"
    exit 0
fi

for f in agent_isolation_kernel_integration k2-daemon k2-ptyd k2; do
    [ -x "$dir/$f" ] || { echo "missing or not executable: $dir/$f" >&2; exit 2; }
done
[ -f "/etc/systemd/system/$unit" ] || { echo "no $unit: run with --setup first" >&2; exit 2; }
echo "box: $(hostname) · $(. /etc/os-release && echo "$PRETTY_NAME") · kernel $(uname -r) · $(systemctl --version | head -1)"

restart() {
    sudo systemctl stop "$unit" 2>/dev/null || true
    rm -f "$persist/home/.k2/daemon.port" "$persist/home/.k2/daemon.ready"
    for f in k2-daemon k2-ptyd k2; do
        install -m 0755 "$dir/$f" "$persist/bin/$f.new" && mv -f "$persist/bin/$f.new" "$persist/bin/$f"
    done
    sudo systemctl start "$unit"
    local i port=""
    for i in $(seq 1 120); do
        port="$(cat "$persist/home/.k2/daemon.port" 2>/dev/null || true)"
        if [ -n "$port" ] && curl -s "http://127.0.0.1:$port/boot-status" | grep -q '"ready"'; then
            return 0
        fi
        sleep 1
    done
    echo "the persistent daemon did not get ready" >&2
    sudo journalctl -u "$unit" -n 40 --no-pager >&2 || true
    return 1
}

run_test() { # root|user
    K2_ISO_PERSIST_HOME="$persist/home" K2_ISO_EXPECT="$1" RUST_BACKTRACE=1 \
        "$dir/agent_isolation_kernel_integration" root_owned_agent_cgroups_refuse_a_sibling \
        --exact --ignored --nocapture --test-threads=1
}

# 1. The helper places the sessions.
[ -x "$helper" ] || { echo "no root helper at $helper (run --setup)" >&2; exit 2; }
restart
echo "== with the root helper (K2_ISO_EXPECT=root)"
rc_root=0
run_test root || rc_root=$?

# 2. The bite: the helper away, the daemon places them itself.
put_back() {
    if [ -e "$helper.m1b-off" ]; then
        sudo mv -f "$helper.m1b-off" "$helper"
    fi
}
trap put_back EXIT
sudo mv "$helper" "$helper.m1b-off"
restart
echo "== the bite: no root helper (K2_ISO_EXPECT=user)"
rc_user=0
run_test user || rc_user=$?
put_back
trap - EXIT
restart

echo "m1b root-owned: $([ "$rc_root" = 0 ] && echo ok || echo "FAIL ($rc_root)")"
echo "m1b bite (helper off): $([ "$rc_user" = 0 ] && echo ok || echo "FAIL ($rc_user)")"
[ "$rc_root" = 0 ] && [ "$rc_user" = 0 ]
