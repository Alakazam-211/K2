#!/usr/bin/env python3
"""The release's no-bash check: a build of a brand with `cli.no_bash` true
carries no copy of the shell CLI (`cli/k2`), in any of its files.

    python3 scripts/brand/check-no-bash.py --cli <cli/k2> <file or folder>...

Beside scripts/brand/check-brand-mark.py, and like it, it reads bytes and
runs nothing. A folder is read file by file (links are not followed). The
shell CLI could be there three ways, and each is looked for:

  - plain: the daemon and the root helper compiled it in as text
    (include_str!). Found by markers taken from <cli/k2> itself: its first
    two lines (the shebang and its title), its K2_CLI_VERSION line and its
    update-key line (`_DAEMON_UPDATE_PUBKEYS="...`). No other file of the
    build holds them: the help pages and the --schema text k5 carries are
    not the script's head, and k5's version mark is K5_CLI_VERSION=.
  - gzipped: k5 packs it with gzip (crates/k5-cli/build.rs). Every gzip
    member in a file (the 1f 8b 08 magic) is inflated, and it counts when
    it is <cli/k2> byte for byte, when it is any bash script that carries a
    K2_CLI_VERSION line in its first 40 lines (another release's copy), or
    when what it inflates to holds a plain marker (a .tar.gz with the
    script in it).
  - a file of its own: a file that is a bash script with a K2_CLI_VERSION
    line, or the alias shim beside <cli/k2> (`k2so`) byte for byte, under
    any name (a package or a folder that ships them).

Each finding names the file, the offset and what was found. Exit 0 when
there is none, 1 when there is one, 2 for a usage error.
"""
import os
import re
import sys
import zlib

GZIP_MAGIC = b"\x1f\x8b\x08"
# What one gzip member may inflate to here: more than any script, and a
# bound on a false magic that inflates forever.
INFLATE_MAX = 256 * 1024 * 1024
VERSION_LINE = re.compile(rb'^K2_CLI_VERSION="[0-9A-Za-z.+-]+"$')
HEAD_LINES = 40
# The alias shims shipped beside the script (src-tauri/tauri.conf.json).
SHIMS = ("k2so",)


def markers(cli):
    """The plain markers of the shell CLI, from <cli/k2>'s own bytes."""
    lines = cli.split(b"\n")
    if len(lines) < 2 or lines[0] != b"#!/bin/bash":
        raise ValueError("does not start with #!/bin/bash")
    head = b"\n".join(lines[:2]) + b"\n"
    version = next((l for l in lines[:HEAD_LINES] if VERSION_LINE.match(l)), None)
    if version is None:
        raise ValueError(f"has no K2_CLI_VERSION=\"x.y.z\" line in its first {HEAD_LINES} lines")
    keys = next((l for l in lines if l.startswith(b'_DAEMON_UPDATE_PUBKEYS="')), None)
    if keys is None:
        raise ValueError('has no _DAEMON_UPDATE_PUBKEYS="..." line')
    return [("first two lines", head), ("version line", version + b"\n"), ("update-key line", keys + b"\n")]


def is_shell_cli(data):
    """A bash script with a K2_CLI_VERSION line near its top."""
    if not data.startswith(b"#!/bin/bash\n"):
        return False
    return any(VERSION_LINE.match(l) for l in data[: 64 * 1024].split(b"\n")[:HEAD_LINES])


def plain_findings(data, marks):
    """(offset, which marker) for each marker in `data`."""
    out = []
    for what, m in marks:
        at = data.find(m)
        if at >= 0:
            out.append((at, what))
    return out


def inflate(data, at):
    """What the gzip member at `at` inflates to, or None when it is not one."""
    d = zlib.decompressobj(16 + zlib.MAX_WBITS)
    try:
        out = d.decompress(memoryview(data)[at:], INFLATE_MAX)
    except zlib.error:
        return None
    if not d.eof and len(out) < 2:
        return None
    return out


def gzip_findings(data, cli, marks, shims):
    out = []
    at = data.find(GZIP_MAGIC)
    while at >= 0:
        inner = inflate(data, at)
        if inner:
            if inner == cli:
                out.append((at, "a gzip member that inflates to the shell CLI, byte for byte"))
            elif inner in shims:
                out.append((at, "a gzip member that inflates to the shell CLI's alias shim"))
            elif is_shell_cli(inner):
                out.append((at, "a gzip member that inflates to a shell CLI script (another copy)"))
            else:
                out += [(at, f"a gzip member that holds the shell CLI's {what}") for _, what in plain_findings(inner, marks)]
        at = data.find(GZIP_MAGIC, at + 1)
    return out


def check_file(path, cli, marks, shims):
    with open(path, "rb") as f:
        data = f.read()
    found = []
    if is_shell_cli(data):
        found.append((0, "the file is a shell CLI script"))
    if data in shims:
        found.append((0, "the file is the shell CLI's alias shim"))
    found += [(at, f"the shell CLI's {what}, as text") for at, what in plain_findings(data, marks)]
    found += gzip_findings(data, cli, marks, shims)
    return sorted(set(found))


def files(paths):
    for p in paths:
        if os.path.isdir(p) and not os.path.islink(p):
            for root, dirs, names in os.walk(p):
                dirs.sort()
                for n in sorted(names):
                    full = os.path.join(root, n)
                    if os.path.isfile(full) and not os.path.islink(full):
                        yield full
        else:
            yield p


def main(argv):
    if len(argv) < 3 or argv[0] != "--cli":
        print("usage: check-no-bash.py --cli <cli/k2> <file or folder>...", file=sys.stderr)
        return 2
    script, paths = argv[1], argv[2:]
    try:
        with open(script, "rb") as f:
            cli = f.read()
        marks = markers(cli)
    except OSError as e:
        print(f"check-no-bash: can't read {script} ({e.strerror})", file=sys.stderr)
        return 2
    except ValueError as e:
        print(f"check-no-bash: {script} {e}: not the shell CLI", file=sys.stderr)
        return 2
    # The alias shims beside the script (cli/k2so): bash too, with no
    # version line, so they are matched byte for byte.
    here = os.path.dirname(os.path.abspath(script))
    shims = set()
    for name in SHIMS:
        p = os.path.join(here, name)
        if os.path.isfile(p):
            with open(p, "rb") as f:
                shims.add(f.read())
    bad, n = [], 0
    for path in files(paths):
        n += 1
        try:
            found = check_file(path, cli, marks, shims)
        except OSError as e:
            bad.append(f"{path}: can't read it ({e.strerror})")
            continue
        bad += [f"{path}: at byte {at}: {why}" for at, why in found]
    if n == 0:
        print("check-no-bash: no files to read", file=sys.stderr)
        return 2
    if bad:
        for b in bad:
            print(f"check-no-bash: {b}", file=sys.stderr)
        print("check-no-bash: REFUSED: the build carries the shell CLI", file=sys.stderr)
        return 1
    print(f"check-no-bash: {n} files, no copy of the shell CLI")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
