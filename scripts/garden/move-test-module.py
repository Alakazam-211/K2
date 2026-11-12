#!/usr/bin/env python3
"""Move one inline `#[cfg(test)] mod NAME { ... }` out of a Rust file.

    python3 scripts/garden/move-test-module.py <file.rs> [NAME] [--dry-run]

NAME defaults to `tests`. The module body goes, unchanged apart from one
level of indent, into a file of its own and the old file keeps only the
declaration. No production line changes and the tests keep `use super::*`.

Where the body goes:
  * `foo_tests.rs` beside `foo.rs` (`foo_<NAME>.rs` for other names), named
    with `#[path = "..."]`;
  * `foo/NAME/mod.rs` with a plain `mod NAME;` when the body declares file
    modules of its own (`mod x;`), because those already live in
    `foo/NAME/` and stay where they are.

A lane with unlanded work on the file re-runs this on its own branch
rather than resolving the conflict by hand.

Lines that begin inside a string literal are copied byte for byte, so
test data never changes. The script checks that itself before writing.
"""
import os
import re
import sys

IDENT = re.compile(r"[A-Za-z0-9_]")
RAW = re.compile(r'r(#*)"')


def lex(src):
    """Yield (index, kind) spans: kind is 'code', 'comment' or 'string'.

    Returns two things: a list `kind_at_line_start` (one per line) and a
    function giving the kind of any index, as a list the size of src.
    """
    kinds = bytearray(len(src))  # 0 code, 1 comment, 2 string
    i, n = 0, len(src)

    def mark(a, b, k):
        kinds[a:b] = bytes([k]) * (b - a)

    while i < n:
        c = src[i]
        if c == "/" and src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            mark(i, j, 1)
            i = j
        elif c == "/" and src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            if depth:
                sys.exit("move-test-module: a block comment never ends")
            mark(i, j, 1)
            i = j
        elif c == "r" and (i == 0 or not IDENT.match(src[i - 1]) or
                           (src[i - 1] in "bc" and (i < 2 or not IDENT.match(src[i - 2])))) \
                and RAW.match(src, i):
            m = RAW.match(src, i)
            close = '"' + m.group(1)
            j = src.find(close, m.end())
            if j < 0:
                sys.exit("move-test-module: a raw string never ends")
            j += len(close)
            mark(i, j, 2)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            if j >= n:
                sys.exit("move-test-module: a string never ends")
            j += 1
            mark(i, j, 2)
            i = j
        elif c == "'":
            # A char literal ('a', '\n', '\u{1F600}') or a lifetime ('a).
            if i + 1 < n and src[i + 1] == "\\":
                j = src.find("'", i + 3)
                if j < 0:
                    sys.exit("move-test-module: a char literal never ends")
                mark(i, j + 1, 2)
                i = j + 1
            elif i + 2 < n and src[i + 2] == "'":
                mark(i, i + 3, 2)
                i += 3
            else:
                i += 1
        else:
            i += 1
    return kinds


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = [a for a in sys.argv[1:] if a.startswith("--")]
    if not args or len(args) > 2 or any(f != "--dry-run" for f in flags):
        sys.exit(__doc__)
    path, name = args[0], (args[1] if len(args) == 2 else "tests")
    dry = "--dry-run" in flags
    if not path.endswith(".rs"):
        sys.exit("move-test-module: not a .rs file: " + path)
    with open(path, encoding="utf-8", newline="") as f:
        src = f.read()
    if "\r" in src:
        sys.exit("move-test-module: the file has CR line ends")
    kinds = lex(src)

    # The declaration: `#[cfg(test)]` on its own line, then `mod NAME {`,
    # both at column 0 and both in code.
    decl = re.compile(
        r"^#\[cfg\(test\)\]\n((?:pub(?:\([a-z]+\))? )?mod " + re.escape(name) + r") \{\n", re.M)
    found = [m for m in decl.finditer(src) if kinds[m.start()] == 0]
    if len(found) != 1:
        sys.exit("move-test-module: want exactly one top-level `#[cfg(test)] mod %s {` in %s, found %d"
                 % (name, path, len(found)))
    m = found[0]
    body_start = m.end()
    depth, i = 1, body_start
    while i < len(src) and depth:
        if kinds[i] == 0:
            if src[i] == "{":
                depth += 1
            elif src[i] == "}":
                depth -= 1
        i += 1
    if depth:
        sys.exit("move-test-module: the module's closing brace was not found")
    close = i - 1
    if src[close - 1] != "\n" or src[close + 1:close + 2] not in ("\n", ""):
        sys.exit("move-test-module: the closing brace is not alone on its line")
    body = src[body_start:close]

    out, kept, offset = [], 0, body_start
    for line in body.split("\n")[:-1]:
        in_string = kinds[offset] == 2 and offset > 0 and kinds[offset - 1] == 2
        if in_string:
            out.append(line)
            kept += 1
        elif line.startswith("    "):
            out.append(line[4:])
        elif line.strip() == "":
            out.append("")
        else:
            sys.exit("move-test-module: a code line in the module is indented less than four spaces: %r" % line)
        offset += len(line) + 1
    new_body = "\n".join(out) + "\n"

    # Self-check: putting the indent back gives the old body, byte for byte.
    offset, back = body_start, []
    for old, new in zip(body.split("\n")[:-1], out):
        in_string = kinds[offset] == 2 and offset > 0 and kinds[offset - 1] == 2
        back.append(new if in_string or (new == "" and old.strip() == "") else "    " + new)
        offset += len(old) + 1
    old_norm = [l if l.strip() else "" for l in body.split("\n")[:-1]]
    if back != old_norm:
        sys.exit("move-test-module: self-check failed, nothing written")

    stem = path[:-3]
    has_file_mods = any(
        kinds[x.start(1)] == 0
        for x in re.finditer(r"^\s*(?:pub(?:\([a-z]+\))? )?(mod) [A-Za-z0-9_]+;", body, re.M))
    if has_file_mods:
        target = os.path.join(stem, name, "mod.rs")
        declaration = "#[cfg(test)]\n%s;\n" % m.group(1)
    else:
        base = os.path.basename(stem)
        fname = "%s_tests.rs" % base if name == "tests" else "%s_%s.rs" % (base, name)
        target = os.path.join(os.path.dirname(path), fname)
        declaration = '#[cfg(test)]\n#[path = "%s"]\n%s;\n' % (fname, m.group(1))
    if os.path.exists(target):
        sys.exit("move-test-module: %s already exists" % target)

    new_src = src[:m.start()] + declaration + src[close + 2:]
    print("%s: mod %s, %d lines -> %s (%d lines kept as they are inside strings)"
          % (path, name, len(out), target, kept))
    print("%s: %d -> %d lines" % (path, src.count("\n"), new_src.count("\n")))
    if dry:
        return
    os.makedirs(os.path.dirname(target), exist_ok=True)
    with open(target, "w", encoding="utf-8", newline="") as f:
        f.write(new_body)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(new_src)


if __name__ == "__main__":
    main()
