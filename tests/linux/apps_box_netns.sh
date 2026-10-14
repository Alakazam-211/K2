#!/usr/bin/env bash
# Sourced by apps_box_in_container.sh and apps_box_daemon_e2e.sh (root,
# inside the throwaway container).
#
# The runner starts these containers with --network=none in their own user
# namespace, so the OCI runtime makes the network namespace INSIDE that
# user namespace: container root may write its net sysctls and load nft
# there, while host-wide sysctls (owned by the host's root, "nobody" here)
# stay refused by the kernel. Rootful Podman's default networking would
# make the netns in the host's user namespace instead, where neither works.
#
# `inns` is kept as the one place that says "this runs in the test's netns"
# (today: the container's own). dummy0 carries 203.0.113.1/32, a non-
# private address standing in for "this host's public address" in the nft
# checks.

K2IT_NS=""
export K2IT_NS

k2it_netns_up() {
	ip link set lo up
	ip link add dummy0 type dummy
	ip addr add 203.0.113.1/32 dev dummy0
	ip link set dummy0 up
	echo "  netns: the container's own (container-owned); host address stand-in 203.0.113.1"
}

inns() { "$@"; }
