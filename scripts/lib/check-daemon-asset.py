#!/usr/bin/env python3
"""Byte checks on a Linux k2-daemon release asset before z3mbpZ signs it.

Usage: check-daemon-asset.py <file> <arch> <version>

Used by scripts/sign-daemon.sh (prd-security-sweep-0464-v1 SS1). Exit 0 and
one summary line when every check passes; exit 1 and one line per problem
otherwise. Checks:

- an ELF64 little-endian file for <arch> (x86_64 = EM_X86_64, aarch64 =
  EM_AARCH64);
- exactly one build marker `K2_DAEMON_BUILD v=<version> os=linux
  arch=<arch>;` (crates/k2-daemon/src/main.rs);
- no test-keys or test-pins flag from any K2 crate;
- k2-ptyd embedded (prd-k2-ptyd-v1 P84): an ELF for the same arch inside the
  daemon whose sha256 hex the daemon also carries (build.rs stores it next
  to the bytes). The embedded file's length is read from its own header
  (section headers sit at the end of a linked ELF).

The OAuth check needs the real client values and stays in shell
(assert_daemon_oauth_not_placeholder).
"""

import hashlib
import re
import struct
import sys

MACHINES = {"x86_64": 62, "aarch64": 183}
TEST_FLAGS = (
    b"K2_ROOT_HELPER_TEST_KEYS_ENABLED",
    b"K2_NODE_TEST_KEYS_ENABLED",
    b"K2_APP_ENGINE_TEST_KEYS_ENABLED",
    b"K2_SEW_TEST_PINS_ENABLED",
)
MARKER = re.compile(rb"K2_DAEMON_BUILD v=([0-9.]+) os=([a-z]+) arch=([a-z0-9_]+);")


def elf_machine(b, at=0):
    if b[at:at + 4] != b"\x7fELF" or len(b) < at + 64:
        return None
    if b[at + 4] != 2 or b[at + 5] != 1:  # ELFCLASS64, little-endian
        return None
    return struct.unpack_from("<H", b, at + 18)[0]


def embedded_elves(b, machine):
    """(offset, size, sha256) of each ELF64 of `machine` found after offset 0."""
    out = []
    i = 1
    while True:
        i = b.find(b"\x7fELF", i)
        if i < 0:
            return out
        if elf_machine(b, i) == machine:
            shoff = struct.unpack_from("<Q", b, i + 0x28)[0]
            shentsize, shnum = struct.unpack_from("<HH", b, i + 0x3A)
            size = shoff + shentsize * shnum
            if 4096 < size <= len(b) - i:
                out.append((i, size, hashlib.sha256(b[i:i + size]).hexdigest()))
        i += 1


def check(path, arch, version):
    problems = []
    if arch not in MACHINES:
        return ["unknown arch %r" % arch], None
    with open(path, "rb") as f:
        b = f.read()
    m = elf_machine(b)
    if m is None:
        problems.append("not an ELF64 little-endian file")
    elif m != MACHINES[arch]:
        problems.append("ELF machine %d, expected %d (%s)" % (m, MACHINES[arch], arch))
    markers = MARKER.findall(b)
    if len(markers) != 1:
        problems.append("expected exactly one K2_DAEMON_BUILD marker, found %d" % len(markers))
    else:
        v, os_, a = (x.decode() for x in markers[0])
        if v != version:
            problems.append("built as v%s, release is v%s" % (v, version))
        if os_ != "linux" or a != arch:
            problems.append("marker says os=%s arch=%s, expected linux %s" % (os_, a, arch))
    for flag in TEST_FLAGS:
        if flag in b:
            problems.append("carries %s (a test build) — refusing to sign" % flag.decode())
    ptyd = [e for e in embedded_elves(b, MACHINES[arch]) if e[2].encode() in b]
    if not ptyd:
        problems.append("does not embed k2-ptyd (no embedded %s ELF whose sha256 the daemon carries; P84)" % arch)
    return problems, (ptyd[0] if ptyd else None)


def main(argv):
    if len(argv) != 4:
        sys.exit(__doc__)
    path, arch, version = argv[1:4]
    problems, ptyd = check(path, arch, version)
    if problems:
        for p in problems:
            print("%s: %s" % (path, p))
        sys.exit(1)
    print("%s: ok (ELF %s, marker v%s, k2-ptyd %s… embedded)" % (path, arch, version, ptyd[2][:12]))


if __name__ == "__main__":
    main(sys.argv)
