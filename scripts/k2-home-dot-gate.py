#!/usr/bin/env python3
"""The `.k2` half of the k2-home gate: no new hard-coded `.k2` folder name
in Rust code. Run by scripts/k2-home-gate.sh.

    python3 scripts/k2-home-dot-gate.py           check; exit 1 on a failure
    python3 scripts/k2-home-dot-gate.py --list    print every counted line

A path into the K2 home comes from `k2_core::paths::k2_home()` (or
`k2_home_in(home)` for a given user home, `k2_home_opt()` with a fallback);
a path into a workspace's folder comes from `k2_core::workspace_dot_dir()`.
Those two modules (crates/k2-client/src/paths.rs and
crates/k2-core/src/workspace_dir.rs) are the only places that spell the
name. Everything else that still does is listed in
scripts/k2-home-dot-allowlist.txt, one line each:

    <path> :: <text the line contains> :: <why it stays>

Three rules:
  1. A counted line must match an entry (same path, text on the line).
  2. Every entry needs a reason.
  3. An entry that matches no line fails too, so the list only shrinks.

What is counted: a line of a .rs file under crates/ or src-tauri/src that,
outside a `//` comment, joins `.k2` onto a path (`join(".k2")`,
`join(".k2/…")`) or has the exact literal `".k2"`. Not counted: test files
(a path part named tests, benches, fuzz or fixtures, or a file named
tests.rs, *_tests.rs, *_test.rs or test_*.rs), and test-only items
(`#[cfg(test)]` modules and functions, to their closing brace). Tests keep
the literal on purpose where they pin a layout.

For tests: --root DIR and --allowlist FILE point it at another tree.
"""
import os
import re
import sys

SKIP_DIRS = {"target", "node_modules", ".git", "binaries", "tests", "benches", "fuzz", "fixtures"}
TEST_FILE = re.compile(r"(^tests\.rs$|_tests?\.rs$|^test_.*\.rs$)")
HELPERS = {"crates/k2-client/src/paths.rs", "crates/k2-core/src/workspace_dir.rs"}
COUNTED = re.compile(r'join\("\.k2["/]|"\.k2"')
STRINGS = re.compile(r'"(\\.|[^"\\])*"')


def code_of(line):
    """The line without its `//` comment (a `//` inside a string stays)."""
    out, i, in_str = [], 0, False
    while i < len(line):
        c = line[i]
        if in_str:
            out.append(c)
            if c == "\\" and i + 1 < len(line):
                out.append(line[i + 1])
                i += 2
                continue
            if c == '"':
                in_str = False
        else:
            if line.startswith("//", i):
                break
            if c == '"':
                in_str = True
            out.append(c)
        i += 1
    return "".join(out)


def test_only(lines):
    """Indexes of lines inside `#[cfg(test)]` items. An item runs from its
    attribute to the first later line that is its closing brace at the
    item's own indent (rustfmt's layout), or to its `;` when it has no body."""
    out = set()
    i = 0
    while i < len(lines):
        if not lines[i].strip().startswith("#[cfg(test)]"):
            i += 1
            continue
        j = i + 1
        while j < len(lines) and lines[j].strip().startswith("#["):
            j += 1
        if j >= len(lines):
            break
        item = lines[j]
        indent = item[: len(item) - len(item.lstrip())]
        k = j
        if not code_of(item).rstrip().endswith(";"):
            k = j + 1
            while k < len(lines) and not lines[k].rstrip() == indent + "}":
                k += 1
        out.update(range(i, min(k, len(lines) - 1) + 1))
        i = k + 1
    return out


def counted_lines(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    skip = test_only(lines)
    hits = []
    for i, line in enumerate(lines):
        if i in skip or line.strip().startswith(("//", "*", "/*")):
            continue
        if COUNTED.search(code_of(line)):
            hits.append((i + 1, line.strip()))
    return hits


def scan(root):
    found = {}
    for area in ("crates", "src-tauri/src"):
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, area)):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            for name in sorted(filenames):
                if not name.endswith(".rs") or TEST_FILE.search(name):
                    continue
                full = os.path.join(dirpath, name)
                rel = os.path.relpath(full, root).replace(os.sep, "/")
                if rel in HELPERS:
                    continue
                hits = counted_lines(full)
                if hits:
                    found[rel] = hits
    return found


def read_allowlist(path):
    rows = []
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f.read().split("\n"), 1):
            if not raw.strip() or raw.startswith("#"):
                continue
            parts = [p.strip() for p in raw.split(" :: ")]
            if len(parts) != 3 or not all(parts):
                sys.exit(f"k2-home-gate: {path} line {n}: want '<path> :: <text> :: <reason>', got '{raw}'")
            rows.append((n, parts[0], parts[1], parts[2]))
    return rows


def main(argv):
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    allowlist = None
    listing = False
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--root":
            root = argv[i + 1]
            i += 1
        elif a == "--allowlist":
            allowlist = argv[i + 1]
            i += 1
        elif a == "--list":
            listing = True
        else:
            sys.exit(f"k2-home-gate: unknown argument {a}")
        i += 1
    allowlist = allowlist or os.path.join(root, "scripts", "k2-home-dot-allowlist.txt")
    found = scan(root)
    if listing:
        for rel in sorted(found):
            for n, text in found[rel]:
                print(f"{rel}:{n}: {text}")
        return 0
    rows = read_allowlist(allowlist)
    used = set()
    bad = []
    for rel in sorted(found):
        for n, text in found[rel]:
            match = [r for r in rows if r[1] == rel and r[2] in text]
            if match:
                used.update(r[0] for r in match)
            else:
                bad.append(f"k2-home-gate VIOLATION: {rel}:{n}: {text}")
    stale = [f"k2-home-gate STALE ENTRY: {allowlist} line {r[0]}: {r[1]} :: {r[2]} matches nothing; remove it" for r in rows if r[0] not in used]
    for line in bad + stale:
        print(line)
    if bad:
        print("")
        print("k2-home-gate: a .k2 path spelled out. Use k2_core::paths::k2_home() (or")
        print("k2_home_in / k2_home_opt) for the K2 home, k2_core::workspace_dot_dir() for")
        print("a workspace's folder, or list the line in scripts/k2-home-dot-allowlist.txt")
        print("with the reason it stays.")
    if bad or stale:
        return 1
    print(f"k2-home-gate: .k2 clean ({len(rows)} allowlist entries)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
