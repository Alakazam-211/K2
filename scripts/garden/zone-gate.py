#!/usr/bin/env python3
"""The zone gate: no new hard-coded `k2.dev` in Rust or renderer code.

    python3 scripts/garden/zone-gate.py             check; exit 1 on a failure
    python3 scripts/garden/zone-gate.py --tighten   lower each ceiling to today's
                                                    count and drop rows at zero
    python3 scripts/garden/zone-gate.py --list      print every counted line
    ... --area renderer                             the same for src/ (TypeScript);
                                                    the default area is rust

The zone lives in crates/k2-client/src/hosts.rs (`k2_core::hosts`). Code
that builds a box's name or asks whether a host is ours calls that module.
This gate counts the lines of Rust code that still spell the zone out, per
file, and holds each file to its ceiling in scripts/garden/zone-ceilings.txt
(`<lines> <path>`). Two rules:
  1. A listed file may not have more such lines than its ceiling.
  2. A file that is not listed may have none.
So the count can only fall. When this fails, call `hosts::box_host`,
`hosts::in_zone`, `hosts::ZONE` or `hosts::ZONE_SUFFIX`, or put a new
service address beside the others and say why in the commit.

What is counted: a line under crates/ or src-tauri/src/ in a .rs file that
has `k2.dev` outside a `//` comment. Not counted: the hosts and brand modules,
test files (a path part named tests, benches, fuzz or fixtures, or a file
named *_tests.rs, *_test.rs, tests.rs or test_*.rs), and everything from a
file's first `#[cfg(test)] mod` on. Tests keep the literal on purpose: they
pin what the zone is.

The renderer area (--area renderer) is the same idea for src/: .ts and
.tsx lines with `k2.dev` outside a comment, test files (*.test.ts, *.test.tsx,
*.wktest.ts, fixtures) left out, the modules being src/renderer/lib/hosts.ts and brand.ts, and
the ceilings scripts/garden/zone-ceilings-renderer.txt. Much of what is left
there is copy shown to people ("your k2.dev account"): it stays listed until
the copy itself is decided.

For tests: --root DIR and --ceilings FILE point it at another tree.
"""
import os
import re
import sys

SKIP_DIRS = {"target", "node_modules", ".git", "binaries", "tests", "benches", "fuzz", "fixtures", "__tests__", "dist"}
TEST_MOD = re.compile(r"\s*(pub(\([a-z]+\))? )?mod \w+")
COMMENT_TAIL = re.compile(r"\s//.*$")

# Per area: where to look, which files, which are tests, the module that may
# spell the zone out, the ceilings file, and what the failure tells you to use.
AREA_RULES = {
    "rust": {
        "dirs": ("crates", "src-tauri/src"),
        "ext": (".rs",),
        "test_file": re.compile(r"(^tests\.rs$|_tests?\.rs$|^test_.*\.rs$)"),
        "modules": ("crates/k2-client/src/hosts.rs", "crates/k2-client/src/brand.rs"),
        "ceilings": "zone-ceilings.txt",
        "use": "k2_core::hosts",
        "comment": ("//",),
        "cut_at_test_mod": True,
    },
    "renderer": {
        "dirs": ("src",),
        "ext": (".ts", ".tsx"),
        "test_file": re.compile(r"(\.test\.tsx?$|\.wktest\.ts$|\.spec\.tsx?$|-fixture\.ts$|\.d\.ts$|\.generated\.ts$)"),
        "modules": ("src/renderer/lib/hosts.ts", "src/renderer/lib/brand.ts"),
        "ceilings": "zone-ceilings-renderer.txt",
        "use": "@/lib/hosts",
        "comment": ("//", "*", "/*", "{/*"),
        "cut_at_test_mod": False,
    },
}


def counted_lines(path, rules):
    """(line number, text) for each counted line of one file."""
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    out = []
    for i, line in enumerate(lines):
        s = line.strip()
        if rules["cut_at_test_mod"] and s.startswith("#[cfg(test)]") and i + 1 < len(lines) and TEST_MOD.match(lines[i + 1]):
            break
        if "k2.dev" not in line or s.startswith(rules["comment"]):
            continue
        if "k2.dev" in COMMENT_TAIL.sub("", line):
            out.append((i + 1, s))
    return out


def scan(root, rules=None):
    """{relative path: [(line number, text)]} for files with a counted line."""
    rules = rules or AREA_RULES["rust"]
    found = {}
    for area in rules["dirs"]:
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, area)):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            for name in sorted(filenames):
                if not name.endswith(rules["ext"]) or rules["test_file"].search(name):
                    continue
                full = os.path.join(dirpath, name)
                rel = os.path.relpath(full, root).replace(os.sep, "/")
                if rel in rules["modules"]:
                    continue
                hits = counted_lines(full, rules)
                if hits:
                    found[rel] = hits
    return found


def read_ceilings(path):
    head, rows = [], {}
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f.read().split("\n"), 1):
            if raw.startswith("#") or (not raw.strip() and not rows):
                head.append(raw)
                continue
            if not raw.strip():
                continue
            parts = raw.split(" ", 1)
            if len(parts) != 2 or not parts[0].isdigit() or int(parts[0]) < 1:
                sys.exit(f"zone-gate: {path} line {n}: want '<lines> <path>' with lines above 0, got '{raw}'")
            if parts[1] in rows:
                sys.exit(f"zone-gate: {path} line {n}: {parts[1]} is listed twice")
            rows[parts[1]] = int(parts[0])
    return head, rows


def main(argv):
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    ceilings = None
    mode = "check"
    area = "rust"
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--tighten":
            mode = "tighten"
        elif a == "--list":
            mode = "list"
        elif a == "--root" and args:
            root = args.pop(0)
        elif a == "--area" and args and args[0] in AREA_RULES:
            area = args.pop(0)
        elif a == "--ceilings" and args:
            ceilings = args.pop(0)
        else:
            sys.exit(f"zone-gate: unknown argument: {a}")
    rules = AREA_RULES[area]
    use = rules["use"]
    if ceilings is None:
        ceilings = os.path.join(root, "scripts", "garden", rules["ceilings"])

    found = scan(root, rules)
    if mode == "list":
        for rel in sorted(found):
            for n, text in found[rel]:
                print(f"{rel}:{n}: {text}")
        return 0

    head, rows = read_ceilings(ceilings)
    if mode == "tighten":
        over = [p for p in sorted(found) if len(found[p]) > rows.get(p, 0)]
        if over:
            for p in over:
                print(f"zone-gate: FAIL {p} has {len(found[p])} lines with k2.dev, its ceiling is {rows.get(p, 0)}; --tighten never raises one")
            return 1
        kept = sorted(((len(found[p]), p) for p in found), key=lambda r: (-r[0], r[1]))
        with open(ceilings, "w", encoding="utf-8") as f:
            f.write("\n".join(head + [f"{n} {p}" for n, p in kept]) + "\n")
        print(f"zone-gate: ceilings tightened: {sum(n for n, _ in kept)} lines in {len(kept)} files.")
        return 0

    failed = False
    for p in sorted(found):
        have, cap = len(found[p]), rows.get(p)
        if cap is None:
            failed = True
            for n, text in found[p]:
                print(f"zone-gate: FAIL {p}:{n} spells out k2.dev and the file is not listed: use {use}. {text[:120]}")
        elif have > cap:
            failed = True
            print(f"zone-gate: FAIL {p} has {have} lines with k2.dev, its ceiling is {cap}: use {use} for the new one.")
    if failed:
        return 1
    slack = [p for p in rows if len(found.get(p, [])) < rows[p]]
    if slack:
        print(f"zone-gate: {len(slack)} file(s) are below their ceiling; run scripts/garden/zone-gate.py --tighten to lock that in.")
    total = sum(len(v) for v in found.values())
    print(f"zone-gate: ok ({total} lines in {len(found)} files still spell out k2.dev).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
