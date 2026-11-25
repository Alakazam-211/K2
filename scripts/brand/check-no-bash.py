#!/usr/bin/env python3
"""The release's no-bash check: a build of a brand with `cli.no_bash` true
carries no copy of the shell CLI (`cli/k2`), in any of its files.

    python3 scripts/brand/check-no-bash.py --cli <cli/k2> <file or folder>...
    python3 scripts/brand/check-no-bash.py --cli <cli/k2> --bins <list> <folder>

With --bins, the files are the programs the list names (one per line,
the first word; # starts a comment), each in <folder>: every one must be
there, and a missing one is refused by name with the list it came from,
so a build step that left a program out can't pass by checking less.

Beside scripts/brand/check-brand-mark.py, and like it, it reads bytes and
runs nothing. A folder is read file by file (links are not followed). The
shell CLI could be there these ways, and each is looked for:

  - plain: the daemon compiles it in as text (include_str!). Found by
    markers taken from <cli/k2> itself: its first two lines (the shebang
    and its title), its K2_CLI_VERSION line and its update-key line
    (`_DAEMON_UPDATE_PUBKEYS="...`). No other file of the build holds
    them: the help pages and the --schema text k5 carries are not the
    script's head, and k5's version mark is K5_CLI_VERSION=.
  - its hash: a program that carries the script also carries its hash, to
    check the copy before it runs or writes it: the root helper its
    sha256 (`cliSha256`), k5 its BLAKE3. Either one in hex (lower or upper
    case) is refused on its own, with no copy beside it.
  - packed: every packed stream in a file is unpacked, and it counts when
    it is <cli/k2> byte for byte, when it is any bash script that carries
    a K2_CLI_VERSION line in its first 40 lines (another release's copy),
    when it is the alias shim, or when what it unpacks to holds a plain
    marker or a hash (a .tar.gz with the script in it). The packings:
      gzip   (the 1f 8b 08 magic): k5's embed (crates/k5-cli/build.rs);
      zlib   (78 01, 78 5e, 78 9c or 78 da);
      xz     (fd 37 7a 58 5a 00);
      raw deflate (no header): the root helper's embed
             (crates/k2-root-helper/build.rs). With no magic to find,
             a raw stream is tried at every byte of each long run of
             near-random bytes (RUN_MIN or more, measured in WINDOW-byte
             windows that zlib can't shrink), and from WINDOW bytes
             before it. Any packed copy of the shell CLI (megabytes of
             text) makes such a run; a short run is skipped. A raw stream
             counts when it unpacks to a script (it starts with #!).
  - a file of its own: a file that is a bash script with a K2_CLI_VERSION
    line, or the alias shim beside <cli/k2> (`k2so`) byte for byte, under
    any name (a package or a folder that ships them).

Each finding names the file, the offset and what was found. Exit 0 when
there is none, 1 when there is one, 2 for a usage error.
"""
import hashlib
import lzma
import os
import re
import struct
import sys
import zlib

GZIP_MAGIC = b"\x1f\x8b\x08"
# A zlib header with no preset dictionary: 78 and one of the four levels.
ZLIB_HEADER = re.compile(rb"\x78[\x01\x5e\x9c\xda]")
XZ_MAGIC = b"\xfd7zXZ\x00"
# What one packed stream may unpack to here: more than any script, and a
# bound on a false magic that unpacks forever.
INFLATE_MAX = 256 * 1024 * 1024
VERSION_LINE = re.compile(rb'^K2_CLI_VERSION="[0-9A-Za-z.+-]+"$')
HEAD_LINES = 40
# The alias shims shipped beside the script (src-tauri/tauri.conf.json).
SHIMS = ("k2so",)
# Raw deflate: the windows a file is measured in, the share of a window
# zlib must fail to shrink for it to count as near-random, and the length
# of a run of such windows that is tried byte by byte.
WINDOW = 4096
RANDOM_SHARE = 0.97
RUN_MIN = 64 * 1024
# The bytes a raw stream is handed for its first try: a deflate block's
# header and the first two letters always fit.
RAW_PROBE = 2048


# ── BLAKE3 (hash mode, 32 bytes), as k5's build.rs takes it ────────────
# Python has no BLAKE3 of its own, and this check runs nothing, so it is
# here in full. Checked against the reference test vectors in
# tests/garden/no-bash-check.sh.

B3_IV = (0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19)
B3_PERMUTATION = (2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8)
B3_CHUNK_START, B3_CHUNK_END, B3_PARENT, B3_ROOT = 1, 2, 4, 8
M32 = 0xFFFFFFFF


def _b3_schedule():
    out, order = [], list(range(16))
    for _ in range(7):
        out.append(tuple(order))
        order = [order[p] for p in B3_PERMUTATION]
    return tuple(out)


B3_SCHEDULE = _b3_schedule()


def _b3_compress(cv, m, counter, block_len, flags):
    """The BLAKE3 compression function; returns the new chaining value."""
    s0, s1, s2, s3, s4, s5, s6, s7 = cv
    s8, s9, s10, s11 = B3_IV[0], B3_IV[1], B3_IV[2], B3_IV[3]
    s12, s13, s14, s15 = counter & M32, (counter >> 32) & M32, block_len, flags
    for o in B3_SCHEDULE:
        # The columns.
        s0 = (s0 + s4 + m[o[0]]) & M32; x = s12 ^ s0; s12 = ((x >> 16) | (x << 16)) & M32
        s8 = (s8 + s12) & M32; x = s4 ^ s8; s4 = ((x >> 12) | (x << 20)) & M32
        s0 = (s0 + s4 + m[o[1]]) & M32; x = s12 ^ s0; s12 = ((x >> 8) | (x << 24)) & M32
        s8 = (s8 + s12) & M32; x = s4 ^ s8; s4 = ((x >> 7) | (x << 25)) & M32
        s1 = (s1 + s5 + m[o[2]]) & M32; x = s13 ^ s1; s13 = ((x >> 16) | (x << 16)) & M32
        s9 = (s9 + s13) & M32; x = s5 ^ s9; s5 = ((x >> 12) | (x << 20)) & M32
        s1 = (s1 + s5 + m[o[3]]) & M32; x = s13 ^ s1; s13 = ((x >> 8) | (x << 24)) & M32
        s9 = (s9 + s13) & M32; x = s5 ^ s9; s5 = ((x >> 7) | (x << 25)) & M32
        s2 = (s2 + s6 + m[o[4]]) & M32; x = s14 ^ s2; s14 = ((x >> 16) | (x << 16)) & M32
        s10 = (s10 + s14) & M32; x = s6 ^ s10; s6 = ((x >> 12) | (x << 20)) & M32
        s2 = (s2 + s6 + m[o[5]]) & M32; x = s14 ^ s2; s14 = ((x >> 8) | (x << 24)) & M32
        s10 = (s10 + s14) & M32; x = s6 ^ s10; s6 = ((x >> 7) | (x << 25)) & M32
        s3 = (s3 + s7 + m[o[6]]) & M32; x = s15 ^ s3; s15 = ((x >> 16) | (x << 16)) & M32
        s11 = (s11 + s15) & M32; x = s7 ^ s11; s7 = ((x >> 12) | (x << 20)) & M32
        s3 = (s3 + s7 + m[o[7]]) & M32; x = s15 ^ s3; s15 = ((x >> 8) | (x << 24)) & M32
        s11 = (s11 + s15) & M32; x = s7 ^ s11; s7 = ((x >> 7) | (x << 25)) & M32
        # The diagonals.
        s0 = (s0 + s5 + m[o[8]]) & M32; x = s15 ^ s0; s15 = ((x >> 16) | (x << 16)) & M32
        s10 = (s10 + s15) & M32; x = s5 ^ s10; s5 = ((x >> 12) | (x << 20)) & M32
        s0 = (s0 + s5 + m[o[9]]) & M32; x = s15 ^ s0; s15 = ((x >> 8) | (x << 24)) & M32
        s10 = (s10 + s15) & M32; x = s5 ^ s10; s5 = ((x >> 7) | (x << 25)) & M32
        s1 = (s1 + s6 + m[o[10]]) & M32; x = s12 ^ s1; s12 = ((x >> 16) | (x << 16)) & M32
        s11 = (s11 + s12) & M32; x = s6 ^ s11; s6 = ((x >> 12) | (x << 20)) & M32
        s1 = (s1 + s6 + m[o[11]]) & M32; x = s12 ^ s1; s12 = ((x >> 8) | (x << 24)) & M32
        s11 = (s11 + s12) & M32; x = s6 ^ s11; s6 = ((x >> 7) | (x << 25)) & M32
        s2 = (s2 + s7 + m[o[12]]) & M32; x = s13 ^ s2; s13 = ((x >> 16) | (x << 16)) & M32
        s8 = (s8 + s13) & M32; x = s7 ^ s8; s7 = ((x >> 12) | (x << 20)) & M32
        s2 = (s2 + s7 + m[o[13]]) & M32; x = s13 ^ s2; s13 = ((x >> 8) | (x << 24)) & M32
        s8 = (s8 + s13) & M32; x = s7 ^ s8; s7 = ((x >> 7) | (x << 25)) & M32
        s3 = (s3 + s4 + m[o[14]]) & M32; x = s14 ^ s3; s14 = ((x >> 16) | (x << 16)) & M32
        s9 = (s9 + s14) & M32; x = s4 ^ s9; s4 = ((x >> 12) | (x << 20)) & M32
        s3 = (s3 + s4 + m[o[15]]) & M32; x = s14 ^ s3; s14 = ((x >> 8) | (x << 24)) & M32
        s9 = (s9 + s14) & M32; x = s4 ^ s9; s4 = ((x >> 7) | (x << 25)) & M32
    return (s0 ^ s8, s1 ^ s9, s2 ^ s10, s3 ^ s11, s4 ^ s12, s5 ^ s13, s6 ^ s14, s7 ^ s15)


def blake3_hex(data):
    """BLAKE3 of `data` (32 bytes, no key), in lowercase hex."""
    n = max(1, -(-len(data) // 1024))
    stack = []

    def chunk(i, root):
        c = data[i * 1024:(i + 1) * 1024]
        blocks = max(1, -(-len(c) // 64))
        cv = B3_IV
        for j in range(blocks):
            b = c[j * 64:(j + 1) * 64]
            flags = (B3_CHUNK_START if j == 0 else 0) | (B3_CHUNK_END | root if j == blocks - 1 else 0)
            cv = _b3_compress(cv, struct.unpack("<16I", b.ljust(64, b"\0")), i, len(b), flags)
        return cv

    for i in range(n - 1):
        cv = chunk(i, 0)
        total = i + 1
        while total & 1 == 0:
            cv = _b3_compress(B3_IV, stack.pop() + cv, 0, 64, B3_PARENT)
            total >>= 1
        stack.append(cv)
    if not stack:
        out = chunk(n - 1, B3_ROOT)
    else:
        cv = chunk(n - 1, 0)
        while len(stack) > 1:
            cv = _b3_compress(B3_IV, stack.pop() + cv, 0, 64, B3_PARENT)
        out = _b3_compress(B3_IV, stack.pop() + cv, 0, 64, B3_PARENT | B3_ROOT)
    return struct.pack("<8I", *out).hex()


# ── what to look for ──────────────────────────────────────────────────


def markers(cli):
    """The plain markers of the shell CLI, from <cli/k2>'s own bytes, and
    its two hashes in hex."""
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
    sha = hashlib.sha256(cli).hexdigest().encode()
    blake = blake3_hex(cli).encode()
    return [
        ("first two lines", head),
        ("version line", version + b"\n"),
        ("update-key line", keys + b"\n"),
        ("sha256 (hex)", sha),
        ("sha256 (hex, upper case)", sha.upper()),
        ("BLAKE3 (hex)", blake),
        ("BLAKE3 (hex, upper case)", blake.upper()),
    ]


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


def what_it_unpacks_to(inner, cli, marks, shims):
    """What a packed stream's contents are, of the shell CLI: a list of
    phrases that follow "that ...", empty when it is none of it."""
    if inner == cli:
        return ["inflates to the shell CLI, byte for byte"]
    if inner in shims:
        return ["inflates to the shell CLI's alias shim"]
    if is_shell_cli(inner):
        return ["inflates to a shell CLI script (another copy)"]
    return [f"holds the shell CLI's {what}" for _, what in plain_findings(inner, marks)]


# ── unpacking ─────────────────────────────────────────────────────────


def inflate(data, at, wbits):
    """What the zlib-family stream at `at` (gzip, zlib or raw deflate, by
    `wbits`) unpacks to, and where its packed bytes start, or None when it
    is not one."""
    d = zlib.decompressobj(wbits)
    try:
        out = d.decompress(memoryview(data)[at:], INFLATE_MAX)
    except zlib.error:
        return None
    if not d.eof and len(out) < 2:
        return None
    return out


def gzip_body(data, at):
    """Where the deflate bytes of the gzip member at `at` start (RFC 1952),
    or None when its header runs off the end."""
    if at + 10 > len(data):
        return None
    flags, p = data[at + 3], at + 10
    if flags & 4:  # FEXTRA
        if p + 2 > len(data):
            return None
        p += 2 + int.from_bytes(data[p:p + 2], "little")
    for bit in (8, 16):  # FNAME, FCOMMENT: zero-terminated
        if flags & bit:
            end = data.find(b"\0", p)
            if end < 0:
                return None
            p = end + 1
    if flags & 2:  # FHCRC
        p += 2
    return p if p <= len(data) else None


def unxz(data, at):
    d = lzma.LZMADecompressor(format=lzma.FORMAT_XZ)
    try:
        out = d.decompress(memoryview(data)[at:], INFLATE_MAX)
    except lzma.LZMAError:
        return None
    if not d.eof and len(out) < 2:
        return None
    return out


def random_runs(data):
    """(start, end) of each run of RUN_MIN or more near-random bytes, in
    whole WINDOW-byte windows."""
    runs, start = [], None
    last = len(data) - WINDOW
    for i in range(0, last + 1, WINDOW):
        near_random = len(zlib.compress(data[i:i + WINDOW], 1)) >= WINDOW * RANDOM_SHARE
        if near_random and start is None:
            start = i
        elif not near_random and start is not None:
            runs.append((start, i))
            start = None
    if start is not None:
        runs.append((start, len(data)))
    return [(a, b) for a, b in runs if b - a >= RUN_MIN]


# The first byte of a deflate stream whose first block is Huffman coded
# (fixed or dynamic). A stored first block holds the script's own bytes,
# so the plain markers find it.
HUFFMAN_FIRST = tuple(((b >> 1) & 3) in (1, 2) for b in range(256))


def raw_deflate_findings(data, cli, marks, shims, skip):
    """Raw deflate streams that unpack to a script, tried at every byte of
    each long near-random run; `skip` holds the starts of the gzip and zlib
    members' deflate bytes, already read."""
    out = []
    view = memoryview(data)
    first, err, dco = HUFFMAN_FIRST, zlib.error, zlib.decompressobj
    for start, end in random_runs(data):
        for at in range(max(0, start - WINDOW), end):
            if not first[data[at]] or at in skip:
                continue
            try:
                head = dco(-zlib.MAX_WBITS).decompress(view[at:at + RAW_PROBE], 2)
            except err:
                continue
            if head != b"#!":
                continue
            inner = inflate(data, at, -zlib.MAX_WBITS)
            if inner:
                out += [(at, f"a raw deflate stream that {w}") for w in what_it_unpacks_to(inner, cli, marks, shims)]
    return out


def packed_findings(data, cli, marks, shims):
    out, skip = [], set()
    at = data.find(GZIP_MAGIC)
    while at >= 0:
        inner = inflate(data, at, 16 + zlib.MAX_WBITS)
        if inner:
            body = gzip_body(data, at)
            if body is not None:
                skip.add(body)
            out += [(at, f"a gzip member that {w}") for w in what_it_unpacks_to(inner, cli, marks, shims)]
        at = data.find(GZIP_MAGIC, at + 1)
    for m in ZLIB_HEADER.finditer(data):
        at = m.start()
        inner = inflate(data, at, zlib.MAX_WBITS)
        if inner:
            skip.add(at + 2)
            out += [(at, f"a zlib stream that {w}") for w in what_it_unpacks_to(inner, cli, marks, shims)]
    at = data.find(XZ_MAGIC)
    while at >= 0:
        inner = unxz(data, at)
        if inner:
            out += [(at, f"an xz stream that {w}") for w in what_it_unpacks_to(inner, cli, marks, shims)]
        at = data.find(XZ_MAGIC, at + 1)
    out += raw_deflate_findings(data, cli, marks, shims, skip)
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
    found += packed_findings(data, cli, marks, shims)
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


def expected(list_path, folder):
    """The programs `list_path` names, as paths in `folder`, and the missing ones."""
    names = []
    with open(list_path, encoding="utf-8") as f:
        for line in f:
            words = line.split("#", 1)[0].split()
            if words:
                names.append(words[0])
    paths = [os.path.join(folder, n) for n in names]
    return names, paths, [p for p in paths if not os.path.isfile(p)]


USAGE = "usage: check-no-bash.py --cli <cli/k2> <file or folder>... | --cli <cli/k2> --bins <list> <folder>"


def main(argv):
    if len(argv) < 3 or argv[0] != "--cli":
        print(USAGE, file=sys.stderr)
        return 2
    script, paths = argv[1], argv[2:]
    if paths[0] == "--bins":
        if len(paths) != 3:
            print(USAGE, file=sys.stderr)
            return 2
        list_path, folder = paths[1], paths[2]
        try:
            names, paths, missing = expected(list_path, folder)
        except OSError as e:
            print(f"check-no-bash: can't read the list {list_path} ({e.strerror})", file=sys.stderr)
            return 2
        if not names:
            print(f"check-no-bash: the list {list_path} names no program", file=sys.stderr)
            return 2
        print(f"check-no-bash: expecting {len(names)} programs from {list_path}: {' '.join(names)}")
        if missing:
            for p in missing:
                print(f"check-no-bash: {p}: expected (listed in {list_path}) but not there: the build did not make it", file=sys.stderr)
            print("check-no-bash: REFUSED: a program the brand ships was not built, so it was not checked", file=sys.stderr)
            return 1
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
    bad, unread, n = [], 0, 0
    for path in files(paths):
        n += 1
        try:
            found = check_file(path, cli, marks, shims)
        except OSError as e:
            bad.append(f"{path}: can't read it ({e.strerror})")
            unread += 1
            continue
        bad += [f"{path}: at byte {at}: {why}" for at, why in found]
    if n == 0:
        print("check-no-bash: no files to read", file=sys.stderr)
        return 2
    if bad:
        for b in bad:
            print(f"check-no-bash: {b}", file=sys.stderr)
        if unread == len(bad):
            # Fail closed, but say what happened: nothing was found, the
            # file just wasn't there to read.
            print("check-no-bash: REFUSED: a file could not be read, so the build was not checked", file=sys.stderr)
        else:
            print("check-no-bash: REFUSED: the build carries the shell CLI", file=sys.stderr)
        return 1
    print(f"check-no-bash: {n} files, no copy of the shell CLI")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
