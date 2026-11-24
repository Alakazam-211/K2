#!/usr/bin/env bash
# Agent Isolation M2, the Unix-socket block's kernel rows on an isolation
# test box: lingering on (so the K2 user's bus exists) and the owner's tmux
# server for the duration, then each row in its own delegated transient
# unit. Restores lingering and stops the tmux server at the end.
#
# usage: scripts/agent-isolation-sockblock-rows.sh <artifact-dir> [test …]
#   default tests: sockblock_enforced_rows_4_p2_p7_and_section_7,
#   sockblock_record_logs_and_off_is_the_bite, sockblock_inventory_record_only
set -uo pipefail
dir="$(cd "${1:?usage: $0 <artifact-dir> [test ...]}" && pwd)"
shift
tests=("$@")
[ "${#tests[@]}" -gt 0 ] || tests=(sockblock_enforced_rows_4_p2_p7_and_section_7 sockblock_record_logs_and_off_is_the_bite sockblock_inventory_record_only)
user="$(id -un)"
uid="$(id -u)"
linger_was="$(loginctl show-user "$user" -p Linger --value 2>/dev/null || echo no)"
cleanup() {
    tmux -S "/tmp/tmux-$uid/default" kill-server 2>/dev/null
    if [ "$linger_was" = yes ]; then sudo loginctl enable-linger "$user"; else sudo loginctl disable-linger "$user"; fi
}
trap cleanup EXIT
echo "box: $(hostname) · kernel $(uname -r) · $(systemctl --version | head -1) · yama ptrace_scope $(cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null) · Linger=$linger_was · k2-daemon.service=$(systemctl is-active k2-daemon.service)"
sudo loginctl enable-linger "$user"
for _ in $(seq 1 50); do [ -S "/run/user/$uid/bus" ] && break; sleep 0.1; done
[ -S "/run/user/$uid/bus" ] || { echo "no user bus" >&2; exit 1; }
mkdir -p "/tmp/tmux-$uid" && chmod 700 "/tmp/tmux-$uid"
tmux -S "/tmp/tmux-$uid/default" new-session -d -s owner 'sleep 1800'
fail=0
for t in "${tests[@]}"; do
    echo "=== $t"
    sudo systemd-run --uid="$uid" --gid="$(id -g)" \
        -p Delegate=yes -p DelegateSubgroup=daemon -p KillMode=process \
        --wait --pipe --collect --quiet \
        --setenv=HOME="$HOME" --setenv=USER="$user" --setenv=PATH=/usr/local/bin:/usr/bin:/bin \
        --setenv=K2_ISO_DAEMON_BIN="$dir/k2-daemon" \
        --setenv=K2_PTYD_BIN="$dir/k2-ptyd" \
        --setenv=K2_PTYD_TEST_AGENT="$dir/k2-ptyd-test-agent" \
        --setenv=K2_ISO_CLI="$dir/k2" \
        --setenv=K2_ISO_USER_BUS="/run/user/$uid/bus" \
        --setenv=K2_ISO_TMUX_SOCK="/tmp/tmux-$uid/default" \
        --setenv=RUST_BACKTRACE=1 \
        "$dir/agent_isolation_kernel_integration" "$t" --exact --ignored --nocapture --test-threads=1
    rc=$?
    echo "=== $t: rc=$rc"
    [ "$rc" = 0 ] || fail=1
done
exit "$fail"
