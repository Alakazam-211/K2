#!/usr/bin/env python3
"""Cut cli/k2 into the parts it is then built from.

    python3 scripts/garden/split-cli.py [--dry-run]

cli/k2 is cut at its top-level `# ── … ──` section headers into
cli/src/<part>.sh, and cli/src/ORDER lists the parts in today's order.
scripts/build-cli.sh joins them back: the join is cli/k2 byte for byte,
and this script checks that before it writes anything. cli/k2 itself is
not changed.

scripts/garden/cli-parts.txt names the parts: one line per section, in
order, `<part name><TAB><header text>`. A name of `+` folds a section
that is only a header into the part that follows. The headers must match
the file's, in order: a lane that added a section adds its line there and
runs this again on its own branch.
"""
import os
import re
import sys

CLI = "cli/k2"
SRC = "cli/src"
NAMES = "scripts/garden/cli-parts.txt"
ALLOW = "scripts/k2-home-allowlist.txt"
HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z_0-9]*)\1")
START = "(file start)"


def header_text(line):
    return re.sub(r"[─ ]+$", "", line[len("# ── "):])


def headers(lines):
    """(index, text) of each `# ── ` line at column 0 outside a heredoc body."""
    out, here = [], None
    for i, line in enumerate(lines):
        if here:
            word, dash = here
            if line.strip() == word and (dash or line == word):
                here = None
            continue
        code = "" if line.lstrip().startswith("#") else line.split(" #")[0]
        m = None
        for m in HEREDOC.finditer(code):
            pass
        if m:
            here = (m.group(2), code[m.start():m.start() + 3] == "<<-")
            continue
        if line.startswith("# ── "):
            out.append((i, header_text(line)))
    if here:
        sys.exit("split-cli: a heredoc (%s) never ends; the header scan can't be trusted" % here[0])
    return out


def main():
    flags = sys.argv[1:]
    if any(f != "--dry-run" for f in flags):
        sys.exit(__doc__)
    dry = bool(flags)
    if os.path.exists(SRC):
        print("split-cli: %s already exists; nothing to do" % SRC)
        return
    with open(CLI, encoding="utf-8", newline="") as f:
        text = f.read()
    lines = text.split("\n")
    found = [(0, START)] + headers(lines)
    with open(NAMES, encoding="utf-8") as f:
        want = [l.rstrip("\n").split("\t", 1) for l in f if l.strip() and not l.startswith("#")]
    for k, row in enumerate(want):
        if len(row) != 2 or not re.fullmatch(r"\+|[a-z0-9][a-z0-9-]*", row[0]):
            sys.exit("split-cli: %s line %d is not `<part name><TAB><header text>`" % (NAMES, k + 1))
    for k in range(max(len(found), len(want))):
        have = found[k][1] if k < len(found) else None
        named = want[k][1] if k < len(want) else None
        if have != named:
            sys.exit("split-cli: section %d of cli/k2 is %r but %s names %r there. "
                     "Add, remove or fix that line in %s." % (k + 1, have, NAMES, named, NAMES))

    parts, start, seen = [], None, set()
    for k, (name, _) in enumerate(want):
        if start is None:
            start = found[k][0]
        if name == "+":
            continue
        end = found[k + 1][0] if k + 1 < len(found) else len(lines)
        if name in seen:
            sys.exit("split-cli: two parts are named %s" % name)
        seen.add(name)
        body = "\n".join(lines[start:end])
        if end < len(lines):
            body += "\n"
        parts.append((name + ".sh", body))
        start = None
    if start is not None:
        sys.exit("split-cli: the last section is folded (`+`) into a part that does not exist")
    if "".join(b for _, b in parts) != text:
        sys.exit("split-cli: the parts do not join back to cli/k2 byte for byte; nothing written")

    print("split-cli: %d parts, %d lines; the join equals cli/k2" % (len(parts), text.count("\n")))
    if dry:
        for name, body in parts:
            print("  %6d  %s" % (body.count("\n"), name))
        return
    os.makedirs(SRC)
    for name, body in parts:
        with open(os.path.join(SRC, name), "w", encoding="utf-8", newline="") as f:
            f.write(body)
    with open(os.path.join(SRC, "ORDER"), "w", encoding="utf-8", newline="") as f:
        f.write("# The parts of cli/k2, in the order scripts/build-cli.sh joins them.\n"
                "# One file name per line. A new part goes where its commands belong;\n"
                "# router.sh stays last.\n")
        f.write("".join(name + "\n" for name, _ in parts))
    allow_parts(parts)


def allow_parts(parts):
    """The k2-home gate allows a few literals in cli/k2 by path. The same
    text now also sits in a part, so each `cli/k2 :: <text>` row gets a
    twin for the part that holds that text, right under it."""
    if not os.path.isfile(ALLOW):
        return
    with open(ALLOW, encoding="utf-8", newline="") as f:
        rows = f.read().split("\n")
    out = []
    for row in rows:
        out.append(row)
        if not row.startswith(CLI + " :: "):
            continue
        text = row[len(CLI + " :: "):]
        for name, body in parts:
            twin = "%s/%s :: %s" % (SRC, name, text)
            if text != "*" and text in body and twin not in rows:
                out.append(twin)
    with open(ALLOW, "w", encoding="utf-8", newline="") as f:
        f.write("\n".join(out))


if __name__ == "__main__":
    main()
