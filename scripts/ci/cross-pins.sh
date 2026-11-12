#!/usr/bin/env bash
# cross-pins.sh — the toolchain the Linux aarch64 release build uses when it
# is cross-compiled on an x86_64 node (scripts/ci/build-linux-bins.sh).
# Sourced; sets only these names. Every builder must use exactly these
# versions: the dual build (two nodes, byte-identical files) needs the same
# toolchain on both, and the signing user's policy names the same versions
# in compute_needs_aarch64 (tests/packaging/compute-release.sh keeps the
# two equal).
#
# zig is the C compiler and linker for the target; it ships its own libc
# headers and glibc stubs, so nothing comes from the node's distro and no
# root install is needed. K2 Compute installs both from --needs
# (zig@<K2_CROSS_ZIG>, cargo:cargo-zigbuild@<K2_CROSS_ZIGBUILD>).
#
# K2_CROSS_GLIBC is the oldest glibc the aarch64 binaries run on
# (2.28: Debian 10, Ubuntu 20.04, RHEL 8 and newer).
#
# Moving to a new zig or cargo-zigbuild: change it here and in
# scripts/k2sign/policy (compute_needs_aarch64), then reinstall the signer
# on the release Mac (sudo bash scripts/k2sign/install.sh).
K2_CROSS_ZIG="0.15.2"
K2_CROSS_ZIGBUILD="0.23.4"
K2_CROSS_GLIBC="2.28"
