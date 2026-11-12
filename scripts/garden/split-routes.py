#!/usr/bin/env python3
"""Move the rows of ROUTES (routes/route_policy.rs) into one file per area.

    python3 scripts/garden/split-routes.py [--dry-run]

The table's rows, with the comments above them, go to
`crates/k2-daemon/src/routes/policy/<area>.rs`: one file for each busy
first path segment (`/cli/<area>/...`) and `core.rs` for every other
route. `ROUTES` stays `pub const ROUTES: &[Route]`: it becomes the
compile-time merge of those files, sorted by path, so nothing that reads
it changes. No row is edited.

A lane with unlanded rows in the old table re-runs this on its own branch
rather than resolving the conflict by hand. Run on a tree that is already
split, it changes nothing.
"""
import os
import re
import sys

AREAS = ["app", "compute", "mail", "sew", "skin", "vault", "zen"]  # and "core"
SRC = "crates/k2-daemon/src/routes/route_policy.rs"
OUT = "crates/k2-daemon/src/routes/policy"
OPEN = "pub const ROUTES: &[Route] = &[\n"
DOC = ("/// Every `/cli/*` route the daemon answers, sorted by path (the lookup is\n"
       "/// a binary search; a unit test pins the order and uniqueness).\n")
ROW = re.compile(r'    (get|post|both|split)\("(/cli(?:/[^"/]+)+)", ([A-Za-z]+)(?:, ([A-Za-z]+))?\),\n')
FLOORS = ["Admin", "Member", "NoLogin", "Owner", "Public", "VoiceChat"]

MERGE = '''// The rows of [`ROUTES`]: one file per busy first path segment
// (`/cli/<area>/...`) and `core.rs` for every other route, each sorted by
// path. Add a row in its area's file.
%(mods)s
/// Every `/cli/*` route the daemon answers, sorted by path (the lookup is
/// a binary search; a unit test pins the order and uniqueness). The rows
/// live in `routes/policy/*.rs` and are merged here when the daemon is
/// compiled: a row out of order in its file, or a path listed twice, fails
/// the build.
pub const ROUTES: &[Route] = &MERGED;
const PARTS: &[&[Route]] = &[%(parts)s];
const TOTAL: usize = %(total)s;
const MERGED: [Route; TOTAL] = merge_rows(PARTS);

/// The row files merged into one table sorted by path.
const fn merge_rows<const N: usize>(parts: &[&[Route]]) -> [Route; N] {
    let mut out = [Route { path: "", get: None, post: None }; N];
    let mut at = [0usize; 16];
    assert!(parts.len() <= at.len(), "more row files than merge_rows has room for");
    let mut n = 0;
    while n < N {
        // The file whose next row has the smallest path.
        let mut best = usize::MAX;
        let mut p = 0;
        while p < parts.len() {
            if at[p] < parts[p].len() && (best == usize::MAX || path_lt(parts[p][at[p]].path, parts[best][at[best]].path)) {
                best = p;
            }
            p += 1;
        }
        assert!(best != usize::MAX, "the row files hold fewer rows than TOTAL");
        let row = parts[best][at[best]];
        assert!(n == 0 || path_lt(out[n - 1].path, row.path), "a route row is out of order in its file, or a path is listed twice");
        out[n] = row;
        at[best] += 1;
        n += 1;
    }
    out
}

/// `a < b` by bytes: the order `str::cmp` gives, which the lookup uses.
const fn path_lt(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}
'''


IMPORT = "use Floor::{Admin, Member, NoLogin, Owner, Public, VoiceChat};\n"


def bare_uses(text):
    """The floor names `text` uses without a path (`Member`, not `Floor::Member`)."""
    # Code only: the Rust lexer of move-test-module.py marks every byte of
    # a comment or a string literal (they may span lines).
    import importlib.util
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "move_test_module", os.path.join(os.path.dirname(os.path.abspath(__file__)), "move-test-module.py"))
    lexer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(lexer)
    kinds = lexer.lex(text)
    code = "".join(c if kinds[i] == 0 or c == "\n" else " " for i, c in enumerate(text))
    return [f for f in FLOORS if re.search(r"(?<![:\w])%s\b" % f, code)]


def fix_floor_import(src):
    """With the rows gone, the file's own code may no longer use the short
    floor names the table needed. Keep the import for what still uses it:
    the names the code uses, and in the test module the names only the
    tests use. An unused import fails the build under -D warnings."""
    if src.count(IMPORT) != 1:
        sys.exit("split-routes: the `use Floor::{...}` line is not as expected")
    cut = src.index("\n#[cfg(test)]\nmod tests {\n")
    lib, tests = src[:cut].replace(IMPORT, ""), src[cut:]
    # The enum's own declaration names its variants; that is not a use.
    lib = re.sub(r"(?s)\benum Floor \{.*?\n\}\n", "", lib, count=1)
    in_lib = bare_uses(lib)
    in_tests = [f for f in bare_uses(tests) if f not in in_lib]
    src = src.replace(IMPORT + "\n", "use Floor::{%s};\n\n" % ", ".join(in_lib) if in_lib else "")
    if in_tests:
        anchor = "\n#[cfg(test)]\nmod tests {\n    use super::*;\n"
        if src.count(anchor) != 1:
            sys.exit("split-routes: the test module does not start with `use super::*;`")
        one = "{%s}" % ", ".join(in_tests) if len(in_tests) > 1 else in_tests[0]
        src = src.replace(anchor, anchor + "    use super::Floor::%s;\n" % one)
    return src


def main():
    flags = sys.argv[1:]
    if any(f != "--dry-run" for f in flags):
        sys.exit(__doc__)
    dry = bool(flags)
    with open(SRC, encoding="utf-8", newline="") as f:
        src = f.read()
    if OPEN not in src:
        if "const MERGED: [Route; TOTAL] = merge_rows(PARTS);" in src:
            print("split-routes: ROUTES is already split; nothing to do")
            return
        sys.exit("split-routes: `%s` not found in %s" % (OPEN.strip(), SRC))
    if src.count(OPEN) != 1 or DOC + OPEN not in src:
        sys.exit("split-routes: the table's opening or its doc comment is not as expected")
    start = src.index(DOC + OPEN)
    body_start = src.index(OPEN) + len(OPEN)
    close = src.index("\n];\n", body_start)
    body = src[body_start:close + 1]

    # Each row takes the comment lines above it along.
    chunks = {a: [] for a in AREAS + ["core"]}
    count = {a: 0 for a in chunks}
    last = {}
    pending = []
    for line in body.splitlines(True):
        m = ROW.fullmatch(line)
        if m:
            seg = m.group(2).split("/")[2]
            area = seg if seg in AREAS else "core"
            if area in last and not last[area] < m.group(2):
                sys.exit("split-routes: the table is not sorted at %s" % m.group(2))
            last[area] = m.group(2)
            chunks[area].append("".join(pending) + line)
            count[area] += 1
            pending = []
        elif line.startswith("    //"):
            pending.append(line)
        else:
            sys.exit("split-routes: a line in the table is neither a row nor a comment: %r" % line)
    if pending:
        sys.exit("split-routes: a comment at the end of the table has no row under it")
    if sum(count.values()) != len(ROW.findall(body)):
        sys.exit("split-routes: lost a row; nothing written")

    files = {}
    for area in sorted(chunks):
        rows = "".join(chunks[area])
        if not count[area]:
            sys.exit("split-routes: no rows for %s" % area)
        ctors = sorted(set(m.group(1) for m in ROW.finditer(rows)))
        floors = sorted(set(f for m in ROW.finditer(rows) for f in (m.group(3), m.group(4)) if f))
        unknown = [f for f in floors if f not in FLOORS]
        if unknown:
            sys.exit("split-routes: unknown floor(s) %s" % unknown)
        what = ("every `/cli/*` route outside the areas that have a file of their own"
                if area == "core" else "the `/cli/%s` routes" % area)
        files[area] = (
            "//! Rows of the route policy table (`routes::route_policy::ROUTES`):\n"
            "//! %s, sorted by path.\n"
            "//! The floors and what a row means are documented in `route_policy.rs`.\n\n"
            "use super::Floor::{%s};\n"
            "use super::{%s, Route};\n\n"
            "pub(super) const ROWS: &[Route] = &[\n%s];\n"
        ) % (what, ", ".join(floors), ", ".join(ctors), rows)

    names = sorted(files)
    mods = "".join('#[path = "policy/%s.rs"]\nmod rows_%s;\n' % (a, a) for a in names)
    block = MERGE % {
        "mods": mods,
        "parts": ", ".join("rows_%s::ROWS" % a for a in names),
        "total": " + ".join("rows_%s::ROWS.len()" % a for a in names),
    }
    new_src = src[:start] + block + src[close + len("\n];\n"):]
    new_src = fix_floor_import(new_src)

    for a in names:
        print("split-routes: %-8s %4d rows -> %s/%s.rs" % (a, count[a], OUT, a))
    print("split-routes: %d rows in all; %s: %d -> %d lines"
          % (sum(count.values()), SRC, src.count("\n"), new_src.count("\n")))
    if dry:
        return
    os.makedirs(OUT, exist_ok=True)
    for a in names:
        target = os.path.join(OUT, a + ".rs")
        if os.path.exists(target):
            sys.exit("split-routes: %s already exists" % target)
    for a in names:
        with open(os.path.join(OUT, a + ".rs"), "w", encoding="utf-8", newline="") as f:
            f.write(files[a])
    with open(SRC, "w", encoding="utf-8", newline="") as f:
        f.write(new_src)


if __name__ == "__main__":
    main()
