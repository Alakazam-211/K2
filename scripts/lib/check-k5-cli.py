#!/usr/bin/env python3
"""Byte checks on a built k5 command program before it ships.

Usage: check-k5-cli.py <file> <cli/k2> <macos|linux-x86_64|linux-aarch64>

Used by scripts/lib/build-binaries.sh (K2.app's Contents/MacOS/k5-cli),
scripts/ci/build-linux-bins.sh, scripts/package-daemon-deb.sh and
.github/workflows/daemon-binaries.yml. Read, never run. Exit 0 and one
summary line when every check passes; exit 1 and one line per problem
otherwise. Checks:

- its version is this release's: every `K5_CLI_VERSION=<v>` mark followed
  by a NUL (crates/k5-cli/build.rs writes it from cli/k2) is the
  `K2_CLI_VERSION="<v>"` of <cli/k2>, and there is at least one. That is the
  mark k2-client's cli_install::check_binary reads before the owner's switch
  may point a link at it; no version is ever written by hand;
- macos: a universal Mach-O with exactly an arm64 and an x86_64 slice, each
  carrying the mark;
- linux-<arch>: a 64-bit little-endian ELF for <arch>.
"""

import re
import struct
import sys

MARK = re.compile(rb"K5_CLI_VERSION=([0-9A-Za-z.+-]+)\x00")
ELF_MACHINES = {"x86_64": 62, "aarch64": 183}
# Mach-O cputype values (CPU_ARCH_ABI64 | CPU_TYPE_*).
MACHO_X86_64 = 0x01000007
MACHO_ARM64 = 0x0100000C


def cli_version(path):
    with open(path, "rb") as f:
        head = f.read(16 * 1024).decode("utf-8", "replace").splitlines()[:40]
    for line in head:
        m = re.fullmatch(r'K2_CLI_VERSION="([0-9A-Za-z.+-]+)"', line)
        if m:
            return m.group(1)
    return None


def marks(data):
    return [m.decode() for m in MARK.findall(data)]


def fat_slices(data):
    """[(cputype, bytes)] of a universal Mach-O, or None when it isn't one."""
    if len(data) < 8 or data[:4] != b"\xca\xfe\xba\xbe":
        return None
    (n,) = struct.unpack(">I", data[4:8])
    if n == 0 or n > 8 or len(data) < 8 + 20 * n:
        return None
    out = []
    for i in range(n):
        cputype, _sub, offset, size, _align = struct.unpack(">IIIII", data[8 + 20 * i:28 + 20 * i])
        if offset + size > len(data):
            return None
        out.append((cputype, data[offset:offset + size]))
    return out


def main(argv):
    if len(argv) != 4 or argv[3] not in ("macos", "linux-x86_64", "linux-aarch64"):
        print("usage: check-k5-cli.py <file> <cli/k2> <macos|linux-x86_64|linux-aarch64>", file=sys.stderr)
        return 2
    path, script, kind = argv[1], argv[2], argv[3]
    want = cli_version(script)
    if not want:
        print(f"FATAL: {script} has no K2_CLI_VERSION=\"x.y.z\" line", file=sys.stderr)
        return 1
    with open(path, "rb") as f:
        data = f.read()
    problems = []
    found = marks(data)
    if not found:
        problems.append(f"{path}: carries no K5_CLI_VERSION mark")
    elif set(found) != {want}:
        problems.append(f"{path}: carries version {', '.join(sorted(set(found)))}, the script {want}")
    if kind == "macos":
        slices = fat_slices(data)
        if slices is None:
            problems.append(f"{path}: not a universal Mach-O")
        else:
            types = sorted(t for t, _ in slices)
            if types != sorted([MACHO_X86_64, MACHO_ARM64]):
                problems.append(f"{path}: slices {[hex(t) for t in types]}, want exactly arm64 and x86_64")
            for t, b in slices:
                if want not in marks(b):
                    problems.append(f"{path}: the {hex(t)} slice carries no K5_CLI_VERSION={want} mark")
    else:
        arch = kind.split("-", 1)[1]
        ok = len(data) >= 20 and data[:4] == b"\x7fELF" and data[4] == 2 and data[5] == 1
        if not ok or struct.unpack("<H", data[18:20])[0] != ELF_MACHINES[arch]:
            problems.append(f"{path}: not a 64-bit little-endian {arch} ELF")
    if problems:
        for p in problems:
            print(f"FATAL: {p}", file=sys.stderr)
        return 1
    print(f"  k5-cli ok: {kind}, version {want} (the script's), {len(found)} mark(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
