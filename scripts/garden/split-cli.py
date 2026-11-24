#!/usr/bin/env python3
"""Cut cli/k2 into the parts it is then built from.

    python3 scripts/garden/split-cli.py [--dry-run] [--recut]

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

Under a section's line, cut rows cut that section further:
`<part name><TAB>@fn <function>` starts a part at that function, and the
comment block right above it goes along. Exactly one line of the section
must start with `<function>() {`. The section's own name is then the
name of its first part.

`<part name><TAB>@entry <command>` cuts inside the `k2 --schema` manifest,
at the entry with that name. A part named `schema/<noun>` is written as
cli/src/schema/<noun>.entries: a run of entries with no shell around it,
which scripts/build-cli.sh checks file by file.

--recut is for a tree that already has cli/src: cli/k2 must be the join
of the parts (each `%include cli/text/...` line with its file in its
place and the brand slots filled with our names, as scripts/build-cli.sh
builds it: scripts/brand/page_slots.py). The parts as written, include
lines and all, are then cut again by the table, so the pages stay in
cli/text, and ORDER is rewritten.
"""
import importlib.util
import os
import re
import sys

# The brand slots' filler (scripts/brand/page_slots.py), loaded by path.
sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location(
    "page_slots", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "brand", "page_slots.py"))
slots = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(slots)

CLI = "cli/k2"
SRC = "cli/src"
NAMES = "scripts/garden/cli-parts.txt"
ALLOW = "scripts/k2-home-allowlist.txt"
HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z_0-9]*)\1")
START = "(file start)"
INCLUDE = re.compile(r"^%include (cli/text/[a-z0-9_/-]+\.txt)$", re.M)


def inline(rel):
    """A text file's bytes as build-cli.sh inlines them: the file without
    its last new line (the line it replaces keeps its own)."""
    with open(rel, encoding="utf-8", newline="") as f:
        body = f.read()
    return body[:-1] if body.endswith("\n") else body


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


def cut_at(lines, start, end, rule, part):
    """The line index a cut row names, inside the section [start, end)."""
    kind, _, text = rule.partition(" ")
    if kind == "@entry" and text:
        # A `k2 --schema` entry: `    {` then its name line, or both on one line.
        hits = [i - 1 for i in range(start + 1, end)
                if lines[i] == '      "name": "%s",' % text and lines[i - 1] == "    {"]
        hits += [i for i in range(start, end) if lines[i].startswith('    {"name": "%s",' % text)]
        if len(hits) != 1:
            sys.exit("split-cli: %s: %d schema entries of its section are named %r; a cut needs exactly one"
                     % (part, len(hits), text))
        return hits[0]
    if kind != "@fn" or not re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", text):
        sys.exit("split-cli: %s: a cut row is `@fn <function>` or `@entry <command>`, not %r" % (part, rule))
    want = text + "() {"
    hits = [i for i in range(start, end) if lines[i].startswith(want)]
    if len(hits) != 1:
        sys.exit("split-cli: %s: %d lines of its section start with %r; a cut needs exactly one"
                 % (part, len(hits), want))
    i = hits[0]
    # Its comment block goes along, but not the `# <<< generated` line that
    # ends a generated block above it: both marker lines stay in one part.
    while i > start and lines[i - 1].startswith("#") and not lines[i - 1].startswith("# <<< "):
        i -= 1
    return i


def main():
    flags = sys.argv[1:]
    if any(f not in ("--dry-run", "--recut") for f in flags):
        sys.exit(__doc__)
    dry, recut = "--dry-run" in flags, "--recut" in flags
    with open(CLI, encoding="utf-8", newline="") as f:
        text = f.read()
    old = []
    if os.path.exists(SRC):
        if not recut:
            print("split-cli: %s already exists; nothing to do (--recut cuts it again by %s)" % (SRC, NAMES))
            return
        with open(os.path.join(SRC, "ORDER"), encoding="utf-8") as f:
            old = [l.strip() for l in f if l.strip() and not l.startswith("#")]
        raw = ""
        for name in old:
            with open(os.path.join(SRC, name), encoding="utf-8", newline="") as f:
                raw += f.read()
        # build-cli.sh puts each `%include cli/text/...` line's file in its
        # place (and checks the includes); compare cli/k2 to that join.
        joined = slots.fill(INCLUDE.sub(lambda m: inline(m.group(1)), raw))
        if joined != text:
            sys.exit("split-cli: cli/k2 is not the join of cli/src; run scripts/build-cli.sh first. Nothing written.")
        # Cut the parts as written, `%include` lines and all, so the pages
        # stay in cli/text.
        text = raw
    elif recut:
        sys.exit("split-cli: --recut needs an existing %s" % SRC)
    lines = text.split("\n")
    found = [(0, START)] + headers(lines)
    with open(NAMES, encoding="utf-8") as f:
        rows = [l.rstrip("\n").split("\t", 1) for l in f if l.strip() and not l.startswith("#")]
    for k, row in enumerate(rows):
        if len(row) != 2 or not re.fullmatch(r"\+|(schema/)?[a-z0-9][a-z0-9-]*", row[0]):
            sys.exit("split-cli: %s row %d is not `<part name><TAB><header text>`" % (NAMES, k + 1))
    # A section row, then the cut rows (`@…`) that cut that section further.
    want, cuts = [], []
    for name, what in rows:
        if not what.startswith("@"):
            want.append((name, what))
            cuts.append([])
        elif not want or want[-1][0] == "+" or name == "+":
            sys.exit("split-cli: the cut row %r has no part to cut (a folded `+` section can't be cut)" % what)
        else:
            cuts[-1].append((name, what))
    for k in range(max(len(found), len(want))):
        have = found[k][1] if k < len(found) else None
        named = want[k][1] if k < len(want) else None
        if have != named:
            sys.exit("split-cli: section %d of cli/k2 is %r but %s names %r there. "
                     "Add, remove or fix that line in %s." % (k + 1, have, NAMES, named, NAMES))

    spans, start, seen = [], None, set()
    for k, (name, _) in enumerate(want):
        if start is None:
            start = found[k][0]
        if name == "+":
            continue
        end = found[k + 1][0] if k + 1 < len(found) else len(lines)
        at = start
        for sub, rule in cuts[k]:
            cut = cut_at(lines, start, end, rule, sub)
            if cut <= at:
                sys.exit("split-cli: the cut for %s is not below the one before it; cut rows go in the file's order" % sub)
            spans.append((name, at, cut))
            name, at = sub, cut
        spans.append((name, at, end))
        start = None
    if start is not None:
        sys.exit("split-cli: the last section is folded (`+`) into a part that does not exist")
    parts = []
    for name, a, b in spans:
        if name in seen:
            sys.exit("split-cli: two parts are named %s" % name)
        seen.add(name)
        body = "\n".join(lines[a:b])
        if b < len(lines):
            body += "\n"
        parts.append((name + (".entries" if name.startswith("schema/") else ".sh"), body))
    for name, body in parts:
        # contract-gen rewrites a generated block inside one part.
        opens = len(re.findall(r"^# >>> generated", body, re.M))
        closes = len(re.findall(r"^# <<< generated", body, re.M))
        if opens != closes:
            sys.exit("split-cli: %s would hold %d `# >>> generated` and %d `# <<< generated` lines; "
                     "a cut may not split a generated block" % (name, opens, closes))
    if "".join(b for _, b in parts) != text:
        sys.exit("split-cli: the parts do not join back to cli/k2 byte for byte; nothing written")

    print("split-cli: %d parts, %d lines; the join equals cli/k2" % (len(parts), text.count("\n")))
    if dry:
        for name, body in parts:
            print("  %6d  %s" % (body.count("\n"), name))
        return
    os.makedirs(SRC, exist_ok=True)
    for name in old:
        os.remove(os.path.join(SRC, name))
    folder = os.path.join(SRC, "schema")
    if any(name.startswith("schema/") for name, _ in parts):
        os.makedirs(folder, exist_ok=True)
    elif os.path.isdir(folder):
        os.rmdir(folder)
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
    bodies = dict(parts)
    held = []
    for row in rows:
        # A row for a part that is gone, or no longer holds the text, is stale.
        m = re.match(r"cli/src/([^ ]+) :: (.*)$", row)
        if m and m.group(2) != "*" and m.group(2) not in bodies.get(m.group(1), ""):
            continue
        held.append(row)
    rows, out = held, []
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
