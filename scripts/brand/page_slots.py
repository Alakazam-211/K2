#!/usr/bin/env python3
"""The brand slots in the CLI's printed text (rebrand B7).

A cli/text page or a cli/src/schema entry names the company, the
sign-in service, the contact address and the source repo by a slot, never
by name:

  {company}        names.company, who makes it (Alakazam Labs in ours)
  {login}          names.login, the account people sign in with (K2 Connect in ours)
  {contact_email}  links.contact_email, where people write when a server
                   can't send feedback (hello@alakazamlabs.com in ours)
  {repo}           links.repo, the public source repo's page
                   (https://github.com/Alakazam-211/K2 in ours)

Each build fills them before the text reaches anyone: the k5 binary in
crates/k5-cli/build.rs (brand_text::fill, with this build's brand), the
bash CLI in scripts/build-cli.sh (with ours, brand/k2: bash is built in
the default brand only). The tests that compare a page with what a CLI
prints fill the page first, with this file, so cli/k2 and the default
binary print the very bytes they printed before the slots.

    python3 scripts/brand/page_slots.py < page > filled    fill with ours (brand/k2)
    python3 scripts/brand/page_slots.py --brand DIR < page > filled

The brand file's [names] and [links] tables are read with a small reader
of its own (`key = "value"` lines), not tomllib, so this runs on any
Python 3: build-cli.sh runs it on every machine that builds cli/k2.
"""
import json
import os
import re
import sys

HERE = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
OURS = os.path.join(HERE, "brand", "k2")
# Each slot, and the table and key whose value fills it. A value is named
# by its key (`names()["contact_email"]`): the keys are unique across both
# tables.
SLOTS = (
    ("{company}", "names", "company"),
    ("{login}", "names", "login"),
    ("{contact_email}", "links", "contact_email"),
    ("{repo}", "links", "repo"),
)
_TABLE = re.compile(r"^\s*\[([^\]]*)\]\s*(?:#.*)?$")
_KEY = re.compile(r'^\s*([A-Za-z_][A-Za-z0-9_-]*)\s*=\s*("(?:[^"\\]|\\.)*")\s*(?:#.*)?$')


def _tables(brand_dir):
    """{(table, key): value} of the brand file's `key = "value"` lines in
    the tables the slots read."""
    path = os.path.join(brand_dir, "brand.toml")
    wanted = {table for _, table, _ in SLOTS}
    out, table = {}, None
    with open(path, encoding="utf-8") as f:
        for line in f:
            t = _TABLE.match(line)
            if t:
                table = t.group(1).strip()
                continue
            m = _KEY.match(line) if table in wanted else None
            if m:
                out[(table, m.group(1))] = json.loads(m.group(2))
    return out


def names(brand_dir=OURS):
    """{key: value} for each slot's key: the brand's, or the default
    brand's where the brand file leaves the key out."""
    ours, theirs = _tables(OURS), _tables(brand_dir)
    out = {}
    for _, table, key in SLOTS:
        value = theirs.get((table, key), ours.get((table, key)))
        if not value:
            sys.exit(f"page_slots: {OURS}/brand.toml has no [{table}] {key}")
        out[key] = value
    return out


def fill(text, values=None):
    """`text` with every slot filled (ours when `values` is None)."""
    values = values or names()
    for slot, _, key in SLOTS:
        text = text.replace(slot, values[key])
    return text


def raw_slots(text):
    """The slots `text` still holds."""
    return [slot for slot, _, _ in SLOTS if slot in text]


def main(argv):
    brand_dir = OURS
    if argv[:1] == ["--brand"] and len(argv) == 2:
        brand_dir = argv[1]
    elif argv:
        sys.exit("usage: page_slots.py [--brand DIR] < text > filled")
    text = sys.stdin.buffer.read().decode("utf-8")
    sys.stdout.buffer.write(fill(text, names(brand_dir)).encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
