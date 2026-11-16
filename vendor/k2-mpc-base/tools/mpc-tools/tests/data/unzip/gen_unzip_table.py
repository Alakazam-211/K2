#!/usr/bin/env python3
r"""Writes the zip fixtures (zips/*.zip) and unzip.tsv, the pin table tests/unzip.rs reads. Run
by hand, never at test time:

    python3 -I tools/mpc-tools/tests/data/unzip/gen_unzip_table.py

The reference is the two ways the packs unzip today, each run as a separate process:
  read     python3 -I -c 'import sys, zipfile; open(sys.argv[3], "wb").write(
               zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))' ZIP MEMBER OUT
           (one pack's scripts/fetch-corpus.sh)
  extract  python3 -I -m zipfile -e ZIP DIR      (another pack's scripts/fetch-corpus.sh)
Every fixture is made here: by Python's zipfile with fixed times, or byte by byte for the
shapes zipfile won't write (cp437 names, bad CRCs, unsafe paths, prepended bytes). The
random seed is fixed: a second run writes the same bytes (compare them before committing).

unzip.tsv, one row per case:
  (each row's fifth field is the exception class the old tool died with, or `-`)
  read     `read`, the zip, the member (escaped), the old exit code, and the output's sha256
           and size (`-` when the exit code isn't 0);
  extract  `extract`, the zip, `-`, the old exit code, and what the folder holds after it:
           `PATH:SHA256` per file and `PATH/` per folder, sorted, joined by ` | `
           (`-` when empty).
Escaping: backslash, \n, \t, \r and other control characters as \xNN.
"""
import hashlib
import io
import os
import random
import struct
import subprocess
import sys
import tempfile
import zipfile
import zlib

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
ZIPS = os.path.join(HERE, "zips")
READ = 'import sys, zipfile; open(sys.argv[3], "wb").write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))'
DT = (2020, 1, 2, 3, 4, 6)


def esc(s):
    out = []
    for c in s:
        if c == "\\":
            out.append("\\\\")
        elif c == "\n":
            out.append("\\n")
        elif c == "\t":
            out.append("\\t")
        elif c == "\r":
            out.append("\\r")
        elif ord(c) < 0x20 or c == "\x7f":
            out.append("\\x%02x" % ord(c))
        else:
            out.append(c)
    return "".join(out)


def by_zipfile(entries, force_zip64=False, comment=b""):
    """entries: (name, data, method); a name ending in / is a folder."""
    b = io.BytesIO()
    with zipfile.ZipFile(b, "w") as z:
        z.comment = comment
        for name, data, method in entries:
            zi = zipfile.ZipInfo(name, DT)
            zi.compress_type = method
            zi.external_attr = (0o40755 << 16) | 0x10 if name.endswith("/") else (0o100644 << 16)
            with z.open(zi, "w", force_zip64=force_zip64) as f:
                f.write(data)
    return b.getvalue()


def by_hand(entries, prepend=b"", comment=b""):
    """entries: dicts with name (bytes), data, method (0 or 8), utf8 (flag), crc (override),
    local_name (bytes, override), flags (extra bits)."""
    out = bytearray(prepend)
    cd = bytearray()
    for e in entries:
        data = e["data"]
        if e.get("method", 0) == 8:
            c = zlib.compressobj(9, zlib.DEFLATED, -15)
            comp = c.compress(data) + c.flush()
        else:
            comp = data
        crc = e.get("crc", zlib.crc32(data) & 0xFFFFFFFF)
        flags = (0x800 if e.get("utf8") else 0) | e.get("flags", 0)
        name = e["name"]
        lname = e.get("local_name", name)
        off = len(out) - len(prepend) if e.get("relative", True) else len(out)
        out += struct.pack("<IHHHHHIIIHH", 0x04034B50, 20, flags, e.get("method", 0), 0x2000, 0x5021,
                           crc, len(comp), len(data), len(lname), 0) + lname + comp
        cd += struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, 20, 20, flags, e.get("method", 0), 0x2000, 0x5021,
                          crc, len(comp), len(data), len(name), 0, 0, 0, 0, 0o100644 << 16, off) + name
    cd_off = len(out) - len(prepend)
    out += cd
    out += struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, len(entries), len(entries), len(cd), cd_off, len(comment)) + comment
    return bytes(out)


def fixtures():
    rng = random.Random(20261121)
    text = b"".join(b"line %d of the sample\n" % i for i in range(400))
    blob = bytes(rng.randrange(256) for _ in range(5000))
    z = {}
    z["stored-one"] = by_zipfile([("a.txt", b"hello\n", zipfile.ZIP_STORED)])
    z["deflated-one"] = by_zipfile([("a.txt", text, zipfile.ZIP_DEFLATED)])
    z["tree"] = by_zipfile([("top/", b"", zipfile.ZIP_STORED), ("top/a.txt", b"A\n", zipfile.ZIP_DEFLATED),
                            ("top/sub/b.bin", blob, zipfile.ZIP_DEFLATED), ("c.txt", text, zipfile.ZIP_STORED),
                            ("implicit/dir/d.txt", b"D", zipfile.ZIP_DEFLATED)])
    z["empty-member"] = by_zipfile([("empty.txt", b"", zipfile.ZIP_DEFLATED), ("e2", b"", zipfile.ZIP_STORED)])
    z["unicode-names"] = by_zipfile([("café/über.txt", b"u", zipfile.ZIP_DEFLATED),
                                     ("日本.txt", b"j", zipfile.ZIP_STORED)])
    z["zip64"] = by_zipfile([("big.txt", text, zipfile.ZIP_DEFLATED), ("s.txt", b"s", zipfile.ZIP_STORED)],
                            force_zip64=True)
    z["comment"] = by_zipfile([("a.txt", b"x", zipfile.ZIP_STORED)], comment=b"a zip comment")
    z["cp437-name"] = by_hand([{"name": b"r\x82sum\x82.txt", "data": b"cp"}, {"name": b"\xe1\x9b.txt", "data": b"z"}])
    z["utf8-flag"] = by_hand([{"name": "naïve.txt".encode(), "data": b"n", "utf8": True}])
    z["bad-crc"] = by_hand([{"name": b"bad.txt", "data": b"payload", "crc": 0x12345678}])
    z["bad-crc-deflate"] = by_hand([{"name": b"bad.txt", "data": text, "method": 8, "crc": 1}])
    z["name-mismatch"] = by_hand([{"name": b"a.txt", "local_name": b"b.txt", "data": b"x"}])
    z["unsafe-paths"] = by_hand([{"name": b"../evil.txt", "data": b"1"}, {"name": b"/abs/x.txt", "data": b"2"},
                                 {"name": b"a/../../b.txt", "data": b"3"}, {"name": b"./c.txt", "data": b"4"},
                                 {"name": b"d//e.txt", "data": b"5"}, {"name": b"back\\slash.txt", "data": b"6"}])
    z["dotdot-file"] = by_hand([{"name": b"..", "data": b"x"}])
    z["dupes"] = by_hand([{"name": b"same.txt", "data": b"first"}, {"name": b"same.txt", "data": b"second"}])
    z["prepended"] = by_hand([{"name": b"p.txt", "data": b"after a stub", "method": 8}], prepend=b"#!stub\n" * 9)
    z["encrypted-flag"] = by_hand([{"name": b"x.txt", "data": b"x", "flags": 1}])
    z["many"] = by_zipfile([("f%03d.txt" % i, (b"%d" % i) * i, rng.choice([zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED]))
                            for i in range(60)])
    z["not-a-zip"] = b"this is not a zip file at all\n" * 3
    z["truncated"] = z["tree"][: len(z["tree"]) // 2]
    z["empty-zip"] = by_zipfile([])
    return z


def members(data):
    try:
        return [i.filename for i in zipfile.ZipFile(io.BytesIO(data)).infolist()]
    except Exception:
        return []


def error(r):
    """The exception class the old tool died with (`-` when it didn't)."""
    if r.returncode == 0:
        return "-"
    last = [l for l in r.stderr.decode("utf-8", "replace").split("\n") if l.strip()][-1]
    return last.split(":")[0].strip()


def listing(d):
    out = []
    for dp, dn, fn in os.walk(d):
        for x in dn:
            out.append(os.path.relpath(os.path.join(dp, x), d) + "/")
        for x in fn:
            with open(os.path.join(dp, x), "rb") as f:
                out.append(os.path.relpath(os.path.join(dp, x), d) + ":" + hashlib.sha256(f.read()).hexdigest())
    return " | ".join(sorted(out)) or "-"


def main():
    os.makedirs(ZIPS, exist_ok=True)
    zs = fixtures()
    rows = ["# mode\tzip\tmember\told exit\told error\tresult", "# generated by gen_unzip_table.py"]
    for name in sorted(zs):
        path = os.path.join(ZIPS, name + ".zip")
        with open(path, "wb") as f:
            f.write(zs[name])
        asked = members(zs[name])
        asked = list(dict.fromkeys(asked)) + ["no-such-member.txt"]
        with tempfile.TemporaryDirectory() as t:
            for i, m in enumerate(asked):
                out = os.path.join(t, "out%d" % i)
                r = subprocess.run([sys.executable, "-I", "-c", READ, path, m, out], capture_output=True)
                res = "-"
                if r.returncode == 0:
                    with open(out, "rb") as f:
                        b = f.read()
                    res = "%s %d" % (hashlib.sha256(b).hexdigest(), len(b))
                rows.append("\t".join(["read", name, esc(m), str(r.returncode), error(r), res]))
            dest = os.path.join(t, "x", "dest")
            os.makedirs(dest)
            r = subprocess.run([sys.executable, "-I", "-m", "zipfile", "-e", path, dest], capture_output=True)
            rows.append("\t".join(["extract", name, "-", str(r.returncode), error(r), esc(listing(dest))]))
    with open(os.path.join(HERE, "unzip.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(rows) + "\n")


if __name__ == "__main__":
    main()
