#!/usr/bin/env bash
# cross-pins.sh — the toolchain the Linux aarch64 release build uses when it
# is cross-compiled on an x86_64 node (scripts/ci/build-linux-bins.sh).
# Sourced; sets only these names. Every builder must use exactly these
# versions and bytes: the dual build (two nodes, byte-identical files) needs
# the same toolchain on both, and the signing user's policy names the same
# hashes (compute_aarch64_zig_sha256, compute_aarch64_zigbuild_sha256;
# tests/release/k2sign-s3b.sh keeps the two equal).
#
# zig is the C compiler and linker for the target; it ships its own libc
# headers and glibc stubs, so nothing comes from the node's distro and no
# root install is needed. A build tool that produces something we sign is
# hash-pinned (Steward, 11-11): scripts/ci/cross-toolchain.sh downloads
# zig's release tarball and cargo-zigbuild's crates.io source, refuses any
# other bytes, builds cargo-zigbuild with --locked, and build-info.json
# records both hashes.
#
# K2_CROSS_GLIBC is the oldest glibc the aarch64 binaries run on
# (2.28: Debian 10, Ubuntu 20.04, RHEL 8 and newer).
#
# The zig hashes are ziglang.org's (download/index.json) and were checked
# on 2026-11-11 against the tarballs' minisign signatures with zig's key
# RWSGOq2NVecA2UPNdBUZykf1CCb147pkmdtYxgb3Ti+JO/wCYvhbAb/U. The crate hash
# is crates.io's checksum for the version (the same one cargo checks).
#
# Moving to a new zig or cargo-zigbuild: change the version AND its hash
# here (check zig's signature first), change scripts/k2sign/policy to
# match, then reinstall the signer on the release Mac
# (sudo bash scripts/k2sign/install.sh).
K2_CROSS_ZIG="0.15.2"
K2_CROSS_ZIG_SHA256_X86_64="02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239"
K2_CROSS_ZIG_SHA256_AARCH64="958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f"
K2_CROSS_ZIGBUILD="0.23.4"
K2_CROSS_ZIGBUILD_CRATE_SHA256="0b0c2fe7d5f1a782047272e3fc1cd44214c450776bbde564bf92b9776dc2d781"
K2_CROSS_GLIBC="2.28"
