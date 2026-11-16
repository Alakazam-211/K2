#!/usr/bin/env python3
"""The brand gate: no new hard-coded product name in Rust or renderer code.

    python3 scripts/garden/brand-gate.py             check; exit 1 on a failure
    python3 scripts/garden/brand-gate.py --tighten   lower each count to today's and
                                                     drop rows at zero (reasons kept)
    python3 scripts/garden/brand-gate.py --list [KIND]
                                                     print every counted line

The product's names live in one module each: crates/k2-client/src/brand.rs
(`k2_core::brand`, with `product_name!()` / `cli_name!()` for `concat!`) and
src/renderer/lib/brand.ts. This gate counts the code lines outside them that
still spell a name out, per file and kind, and holds each to its row in
scripts/brand-allowlist.txt. Two rules, as the zone gate:
  1. A listed file may not have more such lines than its row.
  2. A file with no row for that kind may have none.
So the counts can only fall. When this fails, use the brand module. If the
literal really must stay (a name another program reads), add a row whose
reason says so, starting `keeper:`, and say why in the commit.

Kinds:
  K2    the word K2 on its own ("K2", "K2 Connect", K2.app; not K2_X,
        K2Error, X-K2-Header, a path segment such as Who::K2, or k2): the
        brand-inventory product-name rule, less Rust paths
  K5    the word K5 on its own: today the K5 Space and old review labels;
        a new one is either the future name used too early or a collision
  repo  the release repo, Alakazam-211/K2 (not K2SO)
`k2.dev` is the zone gate's (scripts/garden/zone-gate.py); scripts/brand-gate.sh
runs both.

What is counted: a code line, the zone gate's way. Rust under crates/ and
src-tauri/src/: not a `//` line, not after a `//`, not from the file's first
`#[cfg(test)]` (or `#[cfg(all(test, …))]`) mod on, and test files (tests.rs,
*_test.rs, *_tests.rs, test_*.rs, tests_*.rs). Renderer under src/: .ts and
.tsx, not a comment line.
Test files are not counted (they pin what the names are), nor the brand and
hosts modules.

Allowlist rows: `<lines> <kind> <path> <reason>`, the reason starting
`keeper:` (stays on purpose: say who reads it) or `debt:` (should move to the
module). A row without one of those is an error.

For tests: --root DIR and --allowlist FILE point it at another tree.
"""
import os
import re
import sys

SKIP_DIRS = {"target", "node_modules", ".git", "binaries", "tests", "benches", "fuzz", "fixtures", "__tests__", "dist"}
TEST_MOD = re.compile(r"\s*(pub(\([a-z]+\))? )?mod \w+")
# `#[cfg(test)]` or `#[cfg(all(test, …))]`, not `#[cfg(not(test))]`.
TEST_CFG = re.compile(r"#\[cfg\((all\()?test[,)]")
RUST_TAIL = re.compile(r"\s//.*$")
MODULES = {
    "crates/k2-client/src/brand.rs",
    "crates/k2-client/src/hosts.rs",
    "src/renderer/lib/brand.ts",
    "src/renderer/lib/hosts.ts",
}
KINDS = {
    "K2": re.compile(r"(?<![A-Za-z0-9_])(?<![Xx]-)(?<!::)K2(?![A-Za-z0-9_]|::)"),
    "K5": re.compile(r"(?<![A-Za-z0-9_])(?<![Xx]-)K5(?![A-Za-z0-9_])"),
    "repo": re.compile(r"Alakazam-211/K2(?![A-Za-z0-9_])"),
}
AREAS = [
    {"dirs": ("crates", "src-tauri/src"), "ext": (".rs",), "rust": True,
     "test_file": re.compile(r"(^tests\.rs$|_tests?\.rs$|^tests?_.*\.rs$)"), "comment": ("//",)},
    {"dirs": ("src",), "ext": (".ts", ".tsx"), "rust": False,
     "test_file": re.compile(r"(\.test\.tsx?$|\.wktest\.ts$|\.spec\.tsx?$|-fixture\.ts$|\.d\.ts$|\.generated\.ts$)"),
     "comment": ("//", "*", "/*", "{/*")},
]


def counted_lines(path, area):
    """{kind: [(line number, text)]} for one file."""
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    out = {}
    for i, line in enumerate(lines):
        s = line.strip()
        if area["rust"] and TEST_CFG.match(s) and i + 1 < len(lines) and TEST_MOD.match(lines[i + 1]):
            break
        if not s or s.startswith(area["comment"]):
            continue
        code = RUST_TAIL.sub("", line) if area["rust"] else line
        for kind, rx in KINDS.items():
            if rx.search(code):
                out.setdefault(kind, []).append((i + 1, s))
    return out


def scan(root):
    """{(kind, relative path): [(line number, text)]}."""
    found = {}
    for area in AREAS:
        for top in area["dirs"]:
            for dirpath, dirnames, filenames in os.walk(os.path.join(root, top)):
                dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
                for name in sorted(filenames):
                    if not name.endswith(area["ext"]) or area["test_file"].search(name):
                        continue
                    full = os.path.join(dirpath, name)
                    rel = os.path.relpath(full, root).replace(os.sep, "/")
                    if rel in MODULES:
                        continue
                    for kind, hits in counted_lines(full, area).items():
                        found[(kind, rel)] = hits
    return found


def read_allowlist(path):
    head, rows = [], {}
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f.read().split("\n"), 1):
            if raw.startswith("#") or (not raw.strip() and not rows):
                head.append(raw)
                continue
            if not raw.strip():
                continue
            parts = raw.split(" ", 3)
            if len(parts) != 4 or not parts[0].isdigit() or int(parts[0]) < 1 or parts[1] not in KINDS:
                sys.exit(f"brand-gate: {path} line {n}: want '<lines> <K2|K5|repo> <path> <keeper|debt>: <reason>', got '{raw}'")
            if not re.match(r"(keeper|debt): \S", parts[3]):
                sys.exit(f"brand-gate: {path} line {n}: the reason must start 'keeper: ' or 'debt: ', got '{parts[3]}'")
            key = (parts[1], parts[2])
            if key in rows:
                sys.exit(f"brand-gate: {path} line {n}: {parts[1]} {parts[2]} is listed twice")
            rows[key] = (int(parts[0]), parts[3])
    return head, rows


def write_allowlist(path, head, rows):
    order = sorted(rows.items(), key=lambda r: (r[0][0], -r[1][0], r[0][1]))
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(head + [f"{n} {k} {p} {why}" for (k, p), (n, why) in order]) + "\n")


def main(argv):
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    allowlist = None
    mode, list_kind = "check", None
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--tighten":
            mode = "tighten"
        elif a == "--list":
            mode = "list"
            if args and args[0] in KINDS:
                list_kind = args.pop(0)
        elif a == "--root" and args:
            root = args.pop(0)
        elif a == "--allowlist" and args:
            allowlist = args.pop(0)
        else:
            sys.exit(f"brand-gate: unknown argument: {a}")
    if allowlist is None:
        allowlist = os.path.join(root, "scripts", "brand-allowlist.txt")

    found = scan(root)
    if mode == "list":
        for kind, rel in sorted(found):
            if list_kind in (None, kind):
                for n, text in found[(kind, rel)]:
                    print(f"{kind} {rel}:{n}: {text}")
        return 0

    head, rows = read_allowlist(allowlist)
    over = [k for k in sorted(found) if len(found[k]) > rows.get(k, (0, ""))[0]]
    if mode == "tighten":
        if over:
            for kind, rel in over:
                print(f"brand-gate: FAIL {rel} has {len(found[(kind, rel)])} {kind} lines, its row allows {rows.get((kind, rel), (0, ''))[0]}; --tighten never raises one")
            return 1
        kept = {k: (len(found[k]), rows[k][1]) for k in found}
        write_allowlist(allowlist, head, kept)
        print(f"brand-gate: allowlist tightened: {sum(n for n, _ in kept.values())} lines in {len(kept)} rows.")
        return 0

    for kind, rel in over:
        have = len(found[(kind, rel)])
        if (kind, rel) not in rows:
            for n, text in found[(kind, rel)]:
                print(f"brand-gate: FAIL {rel}:{n} spells out {kind} and has no {kind} row: use k2_core::brand or @/lib/brand. {text[:120]}")
        else:
            print(f"brand-gate: FAIL {rel} has {have} {kind} lines, its row allows {rows[(kind, rel)][0]}: use k2_core::brand or @/lib/brand for the new one.")
    if over:
        return 1
    slack = [k for k in rows if len(found.get(k, [])) < rows[k][0]]
    if slack:
        print(f"brand-gate: {len(slack)} row(s) are above today's count; run scripts/garden/brand-gate.py --tighten to lock that in.")
    by_kind = {k: sum(len(v) for (kind, _), v in found.items() if kind == k) for k in KINDS}
    print("brand-gate: ok (" + ", ".join(f"{by_kind[k]} {k}" for k in KINDS) + f" lines in {len(found)} rows still spell a name out).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
