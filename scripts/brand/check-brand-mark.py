#!/usr/bin/env python3
"""The release's brand check: every binary of a bundle carries one brand's
mark, the brand being released.

    python3 scripts/brand/check-brand-mark.py --brand <id> <binary>...

Each binary that asks for its brand or keeps its mark carries
`K5_BRAND_MARK=<id>` and a NUL (crates/k2-brand/src/mark.rs, BRAND_MARK;
today the daemon, the k5 binary, ptyd, the node runtime and the App
engine). A release names those files here. The check reads their
bytes and runs nothing:

  - every file carries a mark, exactly one id;
  - every id is <id>: one brand in the bundle, the one being built.

So one cargo call that missed K2_BRAND_DIR (a default-brand daemon in a
partner's bundle, or the reverse) stops the release. Like the version
check of scripts/lib/check-k5-cli.py, a mark is matched with its NUL, so
`xy` is never read inside `xyz`.

Each refusal names the binary and the mark it found. Exit 0 when the
bundle is one brand, 1 when it is not, 2 for a usage error.
"""
import re
import sys

MARK = re.compile(rb"K5_BRAND_MARK=([a-z][a-z0-9]{1,11})\x00")
NAME = re.compile(r"[a-z][a-z0-9]{1,11}")


def marks(path):
    with open(path, "rb") as f:
        data = f.read()
    return sorted({m.group(1).decode() for m in MARK.finditer(data)})


def main(argv):
    if len(argv) < 3 or argv[0] != "--brand" or not NAME.fullmatch(argv[1]):
        print("usage: check-brand-mark.py --brand <id> <binary>...", file=sys.stderr)
        return 2
    want, files = argv[1], argv[2:]
    bad = []
    for path in files:
        try:
            found = marks(path)
        except OSError as e:
            bad.append(f"{path}: can't read it ({e.strerror})")
            continue
        shown = ", ".join(f"K5_BRAND_MARK={m}" for m in found)
        if not found:
            bad.append(f"{path}: found no brand mark (want K5_BRAND_MARK={want})")
        elif len(found) > 1:
            bad.append(f"{path}: found {len(found)} brand marks, {shown} (want K5_BRAND_MARK={want} alone)")
        elif found[0] != want:
            bad.append(f"{path}: found {shown}, a {found[0]} build (want K5_BRAND_MARK={want})")
    if bad:
        for b in bad:
            print(f"check-brand-mark: {b}", file=sys.stderr)
        print(f"check-brand-mark: REFUSED: the bundle is not all {want}", file=sys.stderr)
        return 1
    print(f"check-brand-mark: {len(files)} binaries, all {want}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
