#!/usr/bin/env bash
# What's New from fragments: one small file per change, joined into
# WHATS_NEW.md at release, so lanes stop editing the same lines.
#
#   scripts/whatsnew.sh build <version>            write the version's section into WHATS_NEW.md
#   scripts/whatsnew.sh build <version> --stdout   print the section, write nothing
#   scripts/whatsnew.sh check [<version>]          are the fragments well formed (every version if none is named)
#   scripts/whatsnew.sh check <version> --built    and does WHATS_NEW.md hold exactly their join
#   scripts/whatsnew.sh split <version>            turn a section already in WHATS_NEW.md into fragments
#
# One folder per version:
#   whatsnew.d/<version>/title       one line: the words after "## <version> — "
#   whatsnew.d/<version>/sections    the "###" headings in the order they print, one per line
#   whatsnew.d/<version>/intro.md    optional: text printed before the first heading
#   whatsnew.d/<version>/<name>.md   a fragment: line 1 is "### <Section>" (a line of
#                                    `sections`), the rest is printed under that heading
# Fragments of one section print in file-name order (prefix 10-, 20- to
# place one). A version with no folder is left as it is in WHATS_NEW.md.
#
# For tests: --root DIR works on another tree.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "$ROOT" "$@" <<'PY'
import os
import re
import sys

root, args = sys.argv[1], sys.argv[2:]
if "--root" in args:
    i = args.index("--root")
    if i + 1 >= len(args):
        sys.exit("whatsnew: --root needs a directory")
    root = args[i + 1]
    del args[i:i + 2]
flags = [a for a in args if a.startswith("--")]
words = [a for a in args if not a.startswith("--")]
USAGE = "usage: scripts/whatsnew.sh build <version> [--stdout] | check [<version>] [--built] | split <version>"
if not words or words[0] not in ("build", "check", "split"):
    sys.exit(USAGE)
cmd, rest = words[0], words[1:]
allowed = {"build": {"--stdout"}, "check": {"--built"}, "split": set()}[cmd]
if any(f not in allowed for f in flags) or len(rest) > 1 or (cmd != "check" and len(rest) != 1):
    sys.exit(USAGE)
if "--built" in flags and not rest:
    sys.exit("whatsnew: --built needs a version")
VERSION = re.compile(r"\d+\.\d+\.\d+")
if rest and not VERSION.fullmatch(rest[0]):
    sys.exit("whatsnew: not a version: %s" % rest[0])

NOTES = os.path.join(root, "WHATS_NEW.md")
BASE = os.path.join(root, "whatsnew.d")


def fail(msg):
    sys.exit("whatsnew: " + msg)


def read(path):
    with open(path, encoding="utf-8", newline="") as f:
        return f.read()


def join(version):
    """The version's section as it prints, or a list of what is wrong."""
    d = os.path.join(BASE, version)
    rel = "whatsnew.d/%s" % version
    if not os.path.isdir(d):
        return None, ["%s does not exist" % rel]
    errs = []
    title = ""
    if not os.path.isfile(os.path.join(d, "title")):
        errs.append("%s/title is missing: one line, the words after '## %s — '" % (rel, version))
    else:
        t = read(os.path.join(d, "title"))
        title = t.strip()
        if not title:
            errs.append("%s/title is empty: one line, the words after '## %s — '" % (rel, version))
        elif "\n" in t.rstrip("\n"):
            errs.append("%s/title has more than one line" % rel)
    sections = []
    if not os.path.isfile(os.path.join(d, "sections")):
        errs.append("%s/sections is missing: the '###' headings in order, one per line" % rel)
    else:
        sections = [s for s in read(os.path.join(d, "sections")).split("\n") if s.strip()]
        for s in sections:
            if s != s.strip() or s.startswith("#"):
                errs.append("%s/sections: write the heading's words only, no '#' and no spaces around: %r" % (rel, s))
        if len(set(sections)) != len(sections):
            errs.append("%s/sections names a heading twice" % rel)
    bodies = {s: [] for s in sections}
    intro = ""
    for name in sorted(os.listdir(d)):
        path = os.path.join(d, name)
        if name in ("title", "sections"):
            continue
        if not name.endswith(".md") or not os.path.isfile(path):
            errs.append("%s/%s: only title, sections, intro.md and <name>.md fragments belong here" % (rel, name))
            continue
        text = read(path)
        if "\r" in text:
            errs.append("%s/%s has CR line ends" % (rel, name))
            continue
        if name == "intro.md":
            intro = text.strip("\n")
            if not intro.strip():
                errs.append("%s/intro.md is empty" % rel)
            if re.search(r"(?m)^#{2,3} ", intro):
                errs.append("%s/intro.md holds a '##' or '###' heading" % rel)
            continue
        first, _, body = text.partition("\n")
        if not first.startswith("### ") or first[4:] not in bodies:
            errs.append("%s/%s: line 1 must be '### <Section>' with a heading listed in sections; it is %r" % (rel, name, first))
            continue
        body = body.strip("\n")
        if not body.strip():
            errs.append("%s/%s is empty under its heading" % (rel, name))
            continue
        if re.search(r"(?m)^#{2,3} ", body):
            errs.append("%s/%s holds a second '##' or '###' heading: one section per fragment" % (rel, name))
            continue
        bodies[first[4:]].append(body)
    for s in sections:
        if not bodies[s]:
            errs.append("%s/sections lists '%s' but no fragment is under it" % (rel, s))
    if errs:
        return None, errs
    out = ["## %s — %s\n\n" % (version, title)]
    if intro:
        out.append(intro + "\n\n")
    for s in sections:
        out.append("### %s\n" % s)
        for b in bodies[s]:
            out.append(b + "\n")
        out.append("\n")
    return "".join(out), []


def section_span(notes, version):
    """(start, end) of '## <version> ...' in WHATS_NEW.md, or None."""
    m = re.search(r"(?m)^## %s[ \n]" % re.escape(version), notes)
    if not m:
        return None
    nxt = re.search(r"(?m)^## ", notes[m.end():])
    return m.start(), (m.end() + nxt.start() if nxt else len(notes))


def versions():
    if not os.path.isdir(BASE):
        return []
    return sorted(v for v in os.listdir(BASE) if os.path.isdir(os.path.join(BASE, v)))


if cmd == "check":
    todo = rest or versions()
    bad = []
    for v in todo:
        if not VERSION.fullmatch(v):
            bad.append("whatsnew.d/%s is not a version folder" % v)
            continue
        text, errs = join(v)
        bad.extend(errs)
        if text is not None and "--built" in flags:
            notes = read(NOTES)
            span = section_span(notes, v)
            if span is None:
                bad.append("WHATS_NEW.md has no '## %s' section: run scripts/whatsnew.sh build %s" % (v, v))
            elif notes[span[0]:span[1]] != text:
                bad.append("WHATS_NEW.md's '## %s' section is not the join of whatsnew.d/%s: run scripts/whatsnew.sh build %s" % (v, v, v))
    if bad:
        for b in bad:
            print("whatsnew: " + b, file=sys.stderr)
        sys.exit(1)
    print("whatsnew: ok (%s)" % (", ".join(todo) if todo else "no version folders"))
    sys.exit(0)

version = rest[0]

if cmd == "build":
    text, errs = join(version)
    if errs:
        for e in errs:
            print("whatsnew: " + e, file=sys.stderr)
        sys.exit(1)
    if "--stdout" in flags:
        sys.stdout.write(text)
        sys.exit(0)
    notes = read(NOTES)
    span = section_span(notes, version)
    if span is None:
        first = re.search(r"(?m)^## ", notes)
        if not first:
            fail("WHATS_NEW.md has no '## ' section to put %s above" % version)
        span = (first.start(), first.start())
    new = notes[:span[0]] + text + notes[span[1]:]
    if new == notes:
        print("whatsnew: WHATS_NEW.md already holds %s" % version)
    else:
        with open(NOTES, "w", encoding="utf-8", newline="") as f:
            f.write(new)
        print("whatsnew: wrote the %s section into WHATS_NEW.md" % version)
    sys.exit(0)

# split: a section that is already in WHATS_NEW.md becomes fragments, one
# per heading, whose join is that section byte for byte.
d = os.path.join(BASE, version)
if os.path.exists(d):
    fail("whatsnew.d/%s already exists" % version)
notes = read(NOTES)
span = section_span(notes, version)
if span is None:
    fail("WHATS_NEW.md has no '## %s' section" % version)
block = notes[span[0]:span[1]]
head, _, body = block.partition("\n")
prefix = "## %s — " % version
if not head.startswith(prefix) or not head[len(prefix):].strip():
    fail("the heading %r has no ' — title'" % head)
parts = re.split(r"(?m)^(?=### )", body)
intro, chunks = parts[0].strip("\n"), parts[1:]
names = [c.partition("\n")[0][4:] for c in chunks]
if len(set(names)) != len(names):
    fail("the %s section has the same '###' heading twice; split it by hand" % version)
os.makedirs(d)


def write(name, text):
    with open(os.path.join(d, name), "w", encoding="utf-8", newline="") as f:
        f.write(text)


write("title", head[len(prefix):] + "\n")
write("sections", "".join(n + "\n" for n in names))
if intro:
    write("intro.md", intro + "\n")
for n, (name, chunk) in enumerate(zip(names, chunks), 1):
    slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-") or "section"
    write("%03d-%s.md" % (n * 10, slug), chunk.strip("\n") + "\n")
text, errs = join(version)
if errs or text != block:
    # Nothing half-made is left behind.
    import shutil
    shutil.rmtree(d)
    for e in errs:
        print("whatsnew: " + e, file=sys.stderr)
    where = ""
    if text is not None:
        old, new = block.split("\n"), text.split("\n")
        n = next((i for i, (x, y) in enumerate(zip(old, new)) if x != y), min(len(old), len(new)))
        where = " First difference at line %d of the section: it has %r, the join would have %r." % (
            n + 1, old[n] if n < len(old) else "(end)", new[n] if n < len(new) else "(end)")
    fail("the %s section is not laid out the way a join prints (one blank line after each section, none inside the heading line), so nothing was written.%s" % (version, where))
print("whatsnew: split %s into %d fragment(s) in whatsnew.d/%s; their join equals the section" % (version, len(chunks), version))
PY
