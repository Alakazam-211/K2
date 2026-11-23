#!/usr/bin/env bash
# Agent Isolation M2, B4 pre-rows (design-agent-isolation-m2-b4-nftables
# rev 3.1, "Kernel rows first"): what systemd, polkit and the kernel do on
# this box before any B4 code relies on it. Each row RECORDS what happens
# (`row <id>: …` lines); the review reads them. The script fails only when
# a row could not be run.
#
# Isolation test boxes only. It starts transient units in a test slice
# (`k2isorow.slice`, removed at the end), turns the K2 user's lingering on
# and back to how it was (P2, P3), and puts a `Delegate=no` drop-in on the
# test daemon unit for P4 (removed at the end). Needs: Linux, cgroup v2,
# systemd, sudo, busctl, curl, tmux (P7, in the kernel test).
#
# usage: scripts/agent-isolation-b4-prerows.sh <artifact-dir>
#   <artifact-dir> as for agent-isolation-m1b-kernel-tests.sh (run its
#   --setup once); the kernel test binary runs rows 4, P2 and P7.
# env: K2_ISO_UNIT (default k2iso-daemon.service), K2_ISO_PERSIST
#      (default ~/iso-persist).
set -uo pipefail
dir="$(cd "${1:?usage: $0 <artifact-dir>}" && pwd)"
unit="${K2_ISO_UNIT:-k2iso-daemon.service}"
persist="${K2_ISO_PERSIST:-$HOME/iso-persist}"
user="$(id -un)"
uid="$(id -u)"
slice=k2isorow.slice
[ "$(uname -s)" = Linux ] || { echo "Linux only" >&2; exit 2; }
for t in busctl systemd-run systemctl loginctl curl tmux; do
    command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 2; }
done
[ -f "/etc/systemd/system/$unit" ] || { echo "no $unit: run agent-isolation-m1b-kernel-tests.sh --setup first" >&2; exit 2; }
echo "box: $(hostname) · $(. /etc/os-release && echo "$PRETTY_NAME") · kernel $(uname -r) · $(systemctl --version | head -1)"

ran=0
row() { echo "row $1: $2"; ran=$((ran + 1)); }
first_line() { tr '\n' ' ' | cut -c1-300; }
linger_was="$(loginctl show-user "$user" -p Linger --value 2>/dev/null || echo no)"
dropin="/etc/systemd/system/$unit.d/b4-prerow-delegate.conf"
cleanup() {
    sudo systemctl stop "$slice" 2>/dev/null || true
    sudo rm -f "$dropin"
    sudo systemctl daemon-reload
    if [ "$linger_was" = yes ]; then sudo loginctl enable-linger "$user"; else sudo loginctl disable-linger "$user"; fi
}
trap cleanup EXIT

# A sibling process in its own root-made scope (another "agent").
sudo systemd-run --quiet --collect --scope --slice="$slice" --unit=k2isorow-sibling --uid="$uid" sleep 600 &
sib_wrap=$!
sib=""
for _ in $(seq 1 50); do
    sib="$(systemctl show -p MainPID --value k2isorow-sibling.scope 2>/dev/null)"
    [ -n "$sib" ] && [ "$sib" != 0 ] && break
    sib="$(pgrep -u "$uid" -f '^sleep 600$' | head -1)"
    [ -n "$sib" ] && break
    sleep 0.1
done
[ -n "$sib" ] || { echo "no sibling process" >&2; exit 1; }

# 1. AttachProcessesToUnit as the K2 user (system bus): a sibling's pid and
#    our own, onto the test daemon's unit.
out="$(busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager AttachProcessesToUnit ssau "$unit" "" 1 "$sib" 2>&1)"
row 1a "attach a sibling's pid onto $unit as $user: rc=$? $(echo "$out" | first_line) · now in $(cut -d: -f3 /proc/"$sib"/cgroup)"
sleep 300 & own=$!
out="$(busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager AttachProcessesToUnit ssau "$unit" "" 1 "$own" 2>&1)"
row 1b "attach own pid onto $unit as $user: rc=$? $(echo "$out" | first_line) · now in $(cut -d: -f3 /proc/"$own"/cgroup)"
kill "$own" 2>/dev/null

# 3. Does systemd remove and remake an empty slice's cgroup?
sudo systemd-run --quiet --collect --slice=k2isorow-empty.slice --unit=k2isorow-e1 true
sleep 0.5
ino1="$(stat -c %i /sys/fs/cgroup/$slice/k2isorow-empty.slice 2>/dev/null || stat -c %i /sys/fs/cgroup/k2isorow.slice/k2isorow-empty.slice 2>/dev/null || echo gone)"
sudo systemctl daemon-reload
sleep 1
ino_after_reload="$(stat -c %i /sys/fs/cgroup/k2isorow.slice/k2isorow-empty.slice 2>/dev/null || echo gone)"
sudo systemd-run --quiet --collect --slice=k2isorow-empty.slice --unit=k2isorow-e2 true
sleep 0.5
ino2="$(stat -c %i /sys/fs/cgroup/k2isorow.slice/k2isorow-empty.slice 2>/dev/null || echo gone)"
row 3 "empty slice cgroup inode: after its unit ended $ino1, after daemon-reload $ino_after_reload, after a new unit $ino2"
sudo systemctl stop k2isorow-empty.slice 2>/dev/null

# PF. Whether this systemd takes PIDFDs= on a transient scope (R3): its
#     version, and a transient scope named with ':' made by root around a
#     sleep (the name kept unchanged as the cgroup folder).
row PF "$(systemctl --version | head -1) · busctl $(busctl --version | head -1)"
sudo systemd-run --quiet --collect --scope --slice="$slice" --unit="k2-agent-row:0123456789abcdef.scope" sleep 5 &
sleep 1
row PF-colon "a scope named k2-agent-row:0123456789abcdef.scope: $(ls -d /sys/fs/cgroup/$slice/k2-agent-row:0123456789abcdef.scope 2>&1 | first_line)"
wait $! 2>/dev/null

# 5. What the daemon's unit sets today.
row 5 "$unit: $(systemctl show "$unit" -p MemoryMax -p MemoryHigh -p TasksMax -p OOMPolicy -p CPUWeight -p IOWeight -p Delegate -p DelegateSubgroup -p KillMode | tr '\n' ' ')"

# P1. StartTransientUnit on the SYSTEM bus as the K2 user, into the test
#     slice (as the agents slice would be).
out="$(systemd-run --system --scope --quiet --collect --slice="$slice" --unit="k2-agent-row:$(head -c8 /dev/urandom | od -An -tx1 | tr -d ' \n').scope" -- id -u 2>&1)"
rc=$?
row P1 "systemd-run --system --scope into $slice as $user: rc=$rc $(echo "$out" | first_line)"
row P1-polkit "rules: $(sudo sh -c 'ls /etc/polkit-1/rules.d /usr/share/polkit-1/rules.d 2>/dev/null' | tr '\n' ' ')"

# P3. Lingering turned on by the user itself (from off).
sudo loginctl disable-linger "$user"
out="$(loginctl enable-linger 2>&1)"
row P3 "loginctl enable-linger as $user (from off): rc=$? $(echo "$out" | first_line) · Linger=$(loginctl show-user "$user" -p Linger --value)"

# 2 + 4 + P2 + P7 need the user manager running: lingering on for them.
sudo loginctl enable-linger "$user"
for _ in $(seq 1 50); do [ -S "/run/user/$uid/bus" ] && break; sleep 0.1; done
[ -S "/run/user/$uid/bus" ] || { echo "no user bus at /run/user/$uid/bus after enable-linger" >&2; exit 1; }
export XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus"

# 2. The user manager asked to move a pid outside its tree (the sibling).
out="$(busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StartTransientUnit 'ssa(sv)a(sa(sv))' k2isorow-move.scope fail 1 PIDs au 1 "$sib" 0 2>&1)"
row 2 "user manager StartTransientUnit PIDs=[sibling]: rc=$? $(echo "$out" | first_line) · sibling now in $(cut -d: -f3 /proc/"$sib"/cgroup)"
systemctl --user stop k2isorow-move.scope 2>/dev/null

# 4, P2, P7: from a fenced session (the kernel test binary).
log="$(mktemp)"
for f in agent_isolation_kernel_integration k2-daemon k2-ptyd k2-ptyd-test-agent k2; do
    [ -x "$dir/$f" ] || { echo "missing or not executable: $dir/$f" >&2; exit 2; }
done
# The owner's tmux server (unfenced, as the K2 user) for P7.
tmux -S "/tmp/tmux-$uid/default" kill-server 2>/dev/null
mkdir -p "/tmp/tmux-$uid" && chmod 700 "/tmp/tmux-$uid"
tmux -S "/tmp/tmux-$uid/default" new-session -d -s owner 'sleep 600'
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
    "$dir/agent_isolation_kernel_integration" b4_prerows_from_a_fenced_session \
    --exact --ignored --nocapture --test-threads=1 > "$log" 2>&1
rc=$?
tmux -S "/tmp/tmux-$uid/default" kill-server 2>/dev/null
grep -E '^row ' "$log"
if [ "$rc" != 0 ] || ! grep -q '^row P7' "$log"; then
    echo "the fenced-session rows did not run (rc=$rc):" >&2
    tail -40 "$log" >&2
    exit 1
fi
ran=$((ran + 3))

# P4. The daemon unit after a Delegate=no drop-in, with a live ptyd.
port_file="$persist/home/.k2/daemon.port"
sudo systemctl start "$unit"
for _ in $(seq 1 120); do
    port="$(cat "$port_file" 2>/dev/null || true)"
    [ -n "$port" ] && curl -s "http://127.0.0.1:$port/boot-status" | grep -q '"ready"' && break
    sleep 1
done
cg="$(systemctl show -p ControlGroup --value "$unit")"
tree() { find "/sys/fs/cgroup$cg" -maxdepth 2 -name cgroup.procs -printf '%h ' 2>/dev/null; echo "· owner $(stat -c %U "/sys/fs/cgroup$cg" 2>/dev/null)"; }
row P4a "before the drop-in: $unit at $cg: $(tree)"
sudo mkdir -p "$(dirname "$dropin")"
printf '[Service]\nDelegate=no\nDelegateSubgroup=\n' | sudo tee "$dropin" >/dev/null
sudo systemctl daemon-reload
sudo systemctl restart "$unit"
sleep 3
cg="$(systemctl show -p ControlGroup --value "$unit")"
ptyd="$(pgrep -u "$uid" -x k2-ptyd | head -1)"
daemon="$(systemctl show -p MainPID --value "$unit")"
row P4b "after the drop-in + restart (ptyd alive): Delegate=$(systemctl show -p Delegate --value "$unit") · $(tree) · daemon in $(cut -d: -f3 /proc/"$daemon"/cgroup 2>/dev/null) · ptyd in $(cut -d: -f3 /proc/"$ptyd"/cgroup 2>/dev/null)"
sleep 300 & own=$!
out="$(busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager AttachProcessesToUnit ssau "$unit" "" 1 "$own" 2>&1)"
row P4c "attach own pid onto $unit after the drop-in: rc=$? $(echo "$out" | first_line) · now in $(cut -d: -f3 /proc/"$own"/cgroup)"
kill "$own" 2>/dev/null
sudo systemctl kill --signal=KILL "$unit"
sudo systemctl stop "$unit"
sudo systemctl start "$unit"
sleep 3
cg="$(systemctl show -p ControlGroup --value "$unit")"
row P4d "after a full stop and start: $(tree) · inode $(stat -c %i "/sys/fs/cgroup$cg" 2>/dev/null)"
sudo systemctl stop "$unit"
sudo systemctl kill --signal=KILL "$unit" 2>/dev/null

kill "$sib_wrap" 2>/dev/null
echo "rows recorded: $ran"
[ "$ran" -ge 12 ] || { echo "fewer rows than expected ran" >&2; exit 1; }
