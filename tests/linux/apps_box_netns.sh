#!/usr/bin/env bash
# Sourced by apps_box_in_container.sh and apps_box_daemon_e2e.sh (root,
# inside the throwaway container).
#
# Rootful Podman makes a container's network namespace in the HOST's user
# namespace, so container root (a high host uid here) may neither write its
# net sysctls nor load nftables there. Box prep needs both. So the tests
# make a network namespace from inside the container — owned by the
# container's user namespace — and run the (fake or real) daemon unit in
# it with NetworkNamespacePath=. slirp4netns gives it internet (apt) the
# way rootless Podman does. No mount is needed (AppArmor refuses
# `ip netns add` in a container): the namespace is held by a sleeping
# process and named /proc/<pid>/ns/net.
#
# Sets K2IT_NS. `inns <cmd…>` runs a command in it. 203.0.113.1/32 on
# dummy0 is "this host's own address" for the nft checks.

k2it_netns_up() {
	systemd-run --quiet --unit k2it-netns unshare --net sleep infinity
	local pid=0
	for _ in $(seq 1 50); do
		pid="$(systemctl show -p MainPID --value k2it-netns)"
		[ "$pid" != 0 ] && [ -e "/proc/$pid/ns/net" ] && [ "$(readlink "/proc/$pid/ns/net")" != "$(readlink /proc/self/ns/net)" ] && break
		sleep 0.1
	done
	[ "$pid" != 0 ] || { echo "FAIL: no netns holder" >&2; exit 1; }
	K2IT_NS="/proc/$pid/ns/net"
	export K2IT_NS
	systemd-run --quiet --unit k2it-slirp slirp4netns --configure --mtu=65520 --disable-host-loopback "$pid" tap0
	for _ in $(seq 1 100); do
		nsenter --net="$K2IT_NS" ip -4 addr show tap0 2>/dev/null | grep -q 10.0.2.100 && break
		sleep 0.1
	done
	nsenter --net="$K2IT_NS" ip -4 addr show tap0 | grep -q 10.0.2.100 || { echo "FAIL: slirp4netns" >&2; exit 1; }
	nsenter --net="$K2IT_NS" ip link add dummy0 type dummy
	nsenter --net="$K2IT_NS" ip addr add 203.0.113.1/32 dev dummy0
	nsenter --net="$K2IT_NS" ip link set dummy0 up
	echo "  netns $K2IT_NS (container-owned) via slirp4netns; host address stand-in 203.0.113.1"
}

inns() { nsenter --net="$K2IT_NS" "$@"; }
