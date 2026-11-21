#!/usr/bin/env bash
# Agent Isolation M1: run the real-kernel tests
# (crates/k2-daemon/tests/agent_isolation_kernel_integration.rs) on THIS
# Linux box, inside a delegated systemd system unit (Delegate=yes,
# DelegateSubgroup=daemon), the shape box prep gives the daemon.
#
# Needs: Linux, cgroup v2, systemd 254+ (DelegateSubgroup), Landlock, sudo
# (only to start the transient unit; the tests run as you), curl, git, bash.
#
# usage: scripts/agent-isolation-kernel-tests.sh <artifact-dir> [test-filter]
#   <artifact-dir> holds: agent_isolation_kernel_integration (the test
#   binary), k2-daemon, k2-ptyd, k2-ptyd-test-agent, k2 (cli/k2), all built
#   for this box (debug profile: the daemon reads K2_PTYD_BIN only there).
# env: K2_ISO_TOOLCHAIN_HOME=<a home with .cargo and .rustup> turns on the
#   "cargo build inside the fence" step (default: $HOME when it has them).
set -euo pipefail
dir="$(cd "${1:?usage: $0 <artifact-dir> [filter]}" && pwd)"
filter="${2:-}"
for f in agent_isolation_kernel_integration k2-daemon k2-ptyd k2-ptyd-test-agent k2; do
    [ -x "$dir/$f" ] || { echo "missing or not executable: $dir/$f" >&2; exit 2; }
done
[ "$(uname -s)" = Linux ] || { echo "Linux only" >&2; exit 2; }
grep -q landlock /sys/kernel/security/lsm || { echo "Landlock is not on in this kernel (lsm=)" >&2; exit 2; }
tc="${K2_ISO_TOOLCHAIN_HOME:-}"
if [ -z "$tc" ] && [ -d "$HOME/.cargo" ] && [ -d "$HOME/.rustup" ]; then tc="$HOME"; fi
path="/usr/local/bin:/usr/bin:/bin"
[ -n "$tc" ] && path="$tc/.cargo/bin:$path"
echo "box: $(hostname) · $(. /etc/os-release && echo "$PRETTY_NAME") · kernel $(uname -r) · $(systemctl --version | head -1)"
echo "LSMs: $(cat /sys/kernel/security/lsm)"
args=(--ignored --test-threads=1 --nocapture)
[ -n "$filter" ] && args=("$filter" "${args[@]}")
exec sudo systemd-run --uid="$(id -u)" --gid="$(id -g)" \
    -p Delegate=yes -p DelegateSubgroup=daemon -p KillMode=process \
    --wait --pipe --collect --quiet \
    --setenv=HOME="$HOME" --setenv=USER="$(id -un)" --setenv=PATH="$path" \
    --setenv=K2_ISO_DAEMON_BIN="$dir/k2-daemon" \
    --setenv=K2_PTYD_BIN="$dir/k2-ptyd" \
    --setenv=K2_PTYD_TEST_AGENT="$dir/k2-ptyd-test-agent" \
    --setenv=K2_ISO_CLI="$dir/k2" \
    --setenv=K2_ISO_TOOLCHAIN_HOME="$tc" \
    --setenv=RUST_BACKTRACE=1 \
    "$dir/agent_isolation_kernel_integration" "${args[@]}"
