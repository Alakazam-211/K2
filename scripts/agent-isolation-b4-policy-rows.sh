#!/usr/bin/env bash
# Agent Isolation M2, B4: three rows after the bus policy closed rows 1c and 2
# (scientist, 2026-11-24). Each RECORDS what happens (`row <id>: …`); the
# script restores the box (policy file, test units, lingering) at the end.
#   J1  running, not moving: with the policy on, a process of the K2 user in
#       a root-made "agent" scope runs `systemd-run --user <cmd>`: where does
#       the new process land?
#   J2  StartTransientUnit with PIDs, sent to the SYSTEM bus by the K2 user:
#       still refused, and by which layer (polkit or the bus policy)?
#   J3  the policy file survives a reinstall of the same dbus package and a
#       reload; the in-effect probe (what `agents layout` runs) reads
#       "not in effect" when the file is gone.
#
# Isolation test boxes only (it reinstalls the dbus package at the same
# version, turns lingering on and back, and installs the policy file for the
# test user, all restored after). Needs: sudo, busctl, systemd-run, apt-get.
# usage: scripts/agent-isolation-b4-policy-rows.sh <path to agent-isolation-b4-prerows.sh>
set -uo pipefail
rows_script="${1:?usage: $0 <path to agent-isolation-b4-prerows.sh> (the policy text comes from it)}"
user="$(id -un)"
uid="$(id -u)"
slice=k2isoj.slice
policy=/etc/dbus-1/system.d/k2-agents.conf
first_line() { tr '\n' ' ' | cut -c1-300; }
row() { echo "row $1: $2"; }
policy_text() {
    sed -n '/^<!DOCTYPE busconfig/,/^<\/busconfig>/p' "$rows_script" | sed "s/user=\"k2-row-user\"/user=\"$user\"/"
}
reload() { busctl call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null 2>&1; sleep 1; }
probe() {
    LC_ALL=C busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager \
        AttachProcessesToUnit ssau k2-agents-probe.service "" 0 2>&1 | first_line
}
[ -n "$(policy_text)" ] || { echo "no policy text in $rows_script" >&2; exit 2; }
linger_was="$(loginctl show-user "$user" -p Linger --value 2>/dev/null || echo no)"
had_policy=0
[ -e "$policy" ] && had_policy=1
cleanup() {
    systemctl --user stop k2isoj-run.service 2>/dev/null
    sudo systemctl stop "$slice" 2>/dev/null
    if [ "$had_policy" = 0 ]; then sudo rm -f "$policy"; fi
    reload
    if [ "$linger_was" = yes ]; then sudo loginctl enable-linger "$user"; else sudo loginctl disable-linger "$user"; fi
}
trap cleanup EXIT
echo "box state at start: k2-daemon.service=$(systemctl is-active k2-daemon.service) · policy file: $([ "$had_policy" = 1 ] && echo present || echo absent) · Linger=$linger_was · dbus $(dpkg-query -W -f '${Version}' dbus 2>/dev/null)"

policy_text | sudo tee "$policy" >/dev/null
reload
row J0 "policy installed for $user; in-effect probe: $(probe)"

# J1: a K2-user process in a root-made scope (standing in for an open
# agent's session) asks its user manager to run a command.
sudo loginctl enable-linger "$user"
for _ in $(seq 1 50); do [ -S "/run/user/$uid/bus" ] && break; sleep 0.1; done
export XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus"
sudo systemd-run --quiet --collect --scope --slice="$slice" --unit=k2isoj-agent --uid="$uid" \
    --setenv=XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" --setenv=DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" \
    sh -c 'systemd-run --user --quiet --collect --unit=k2isoj-run -- sleep 120; echo "rc=$?"' > /tmp/k2isoj-out 2>&1
sleep 1
run_pid="$(systemctl --user show -p MainPID --value k2isoj-run.service 2>/dev/null)"
row J1 "systemd-run --user from a process in /$slice/k2isoj-agent.scope: $(first_line < /tmp/k2isoj-out) · the new process (pid $run_pid) is in $(cut -d: -f3 "/proc/$run_pid/cgroup" 2>/dev/null) · its user: $(ps -o user= -p "$run_pid" 2>/dev/null)"
rm -f /tmp/k2isoj-out
systemctl --user stop k2isoj-run.service 2>/dev/null

# J2: StartTransientUnit with PIDs on the system bus, as the K2 user.
since="$(date '+%Y-%m-%d %H:%M:%S')"
sleep 300 & own=$!
out="$(LC_ALL=C busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StartTransientUnit 'ssa(sv)a(sa(sv))' k2isoj-own.scope fail 2 Slice s "$slice" PIDs au 1 "$own" 0 2>&1)"
rc=$?
sleep 0.5
bus_rejected="$(sudo journalctl -u dbus --since "$since" --no-pager 2>/dev/null | grep -c 'Rejected send message.*StartTransientUnit')"
polkit_seen="$(sudo journalctl --since "$since" --no-pager 2>/dev/null | grep -ci 'polkit')"
row J2 "StartTransientUnit PIDs=[own] on the system bus as $user: rc=$rc $(echo "$out" | first_line) · own pid now in $(cut -d: -f3 /proc/"$own"/cgroup) · bus policy rejections logged: $bus_rejected · polkit lines: $polkit_seen"
kill "$own" 2>/dev/null

# J3: the policy file through a reinstall of the same dbus package, a
# reload, and the probe with the file gone.
ver="$(dpkg-query -W -f '${Version}' dbus 2>/dev/null)"
out="$(sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall "dbus=$ver" 2>&1 | tail -2 | first_line)"
reload
row J3a "apt-get install --reinstall dbus=$ver: $out · file $(ls "$policy" 2>&1 | first_line) · probe: $(probe)"
reload
row J3b "after another ReloadConfig: probe: $(probe)"
sudo mv "$policy" "$policy.k2iso-away"
reload
row J3c "file removed, reloaded: probe: $(probe)"
sudo mv "$policy.k2iso-away" "$policy"
reload
row J3d "file back, reloaded: probe: $(probe)"
