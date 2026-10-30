#!/usr/bin/env python3
"""Blank every full-line `//` comment of the .rs files under the given folders, in place
(with `--docs`: only doc-comment lines, `///` and `//!`; tools/test-bundle.sh's STRIP_DOCS).

The fuzz bundles travel in a node job's environment (tools/README.md: about 500 KB of
base64), and comments are a third of the kit's source. Blanking keeps every line number
(panics and coverage still point at the real lines). Refuses (exit 1, nothing changed) when
a line that looks like a comment sits inside a string literal: blanking it would change
the program.
"""
import os
import re
import sys

COMMENT = re.compile(r"^\s*//")
DOC = re.compile(r"^\s*//[/!]")


def string_lines(s):
    """1-based numbers of the lines that start inside a string literal."""
    out, i, n, line = set(), 0, len(s), 1
    mode, hashes = None, 0
    while i < n:
        c = s[i]
        if c == "\n":
            line += 1
            if mode:
                out.add(line)
            i += 1
            continue
        if mode == '"':
            if c == "\\":
                i += 2
                continue
            if c == '"':
                mode = None
            i += 1
            continue
        if mode == "r":
            if c == '"' and s[i + 1 : i + 1 + hashes] == "#" * hashes:
                mode = None
                i += 1 + hashes
                continue
            i += 1
            continue
        if s.startswith("//", i):
            j = s.find("\n", i)
            i = j if j >= 0 else n
            continue
        if s.startswith("/*", i):
            j = s.find("*/", i)
            j = n if j < 0 else j
            line += s[i:j].count("\n")
            i = j + 2
            continue
        m = re.match(r'b?r(#*)"', s[i : i + 12])
        if m and (i == 0 or not (s[i - 1].isalnum() or s[i - 1] == "_")):
            mode, hashes = "r", len(m.group(1))
            i += m.end()
            continue
        if c == "'":
            m = re.match(r"'(\\[^']{1,10}|[^\\'])'", s[i : i + 14])
            i += m.end() if m else 1
            continue
        if c == '"':
            mode = '"'
        i += 1
    return out


def main(dirs):
    pat = COMMENT
    if dirs and dirs[0] == "--docs":
        pat, dirs = DOC, dirs[1:]
    files = [
        os.path.join(d, f)
        for top in dirs
        for d, _, fs in os.walk(top)
        for f in fs
        if f.endswith(".rs")
    ]
    out = {}
    for f in files:
        s = open(f, encoding="utf-8").read()
        lines = s.split("\n")
        bad = [n for n in string_lines(s) if pat.match(lines[n - 1])]
        if bad:
            print(f"strip-comments: {f}:{bad[0]}: a comment-like line in a string; not stripping", file=sys.stderr)
            return 1
        out[f] = "\n".join("" if pat.match(l) else l for l in lines)
    for f, s in out.items():
        open(f, "w", encoding="utf-8").write(s)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
