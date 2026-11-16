#!/usr/bin/env python3
r"""Writes the Python parity tables that tests/quote_python_parity.rs reads.

Run by hand, once, from any directory:

    python3 crates/k5-cli/tests/data/gen_quote_table.py

It is NOT run at test time. The random seed is fixed, so a second run writes
the same bytes. Python is the reference: every expected value here is what
`urllib.parse.quote(value, safe='')` returned, the call the shell CLI's
`urlencode` makes for every value it puts in a query string.

Files written next to this script:

quote.tsv
    One row per text: `<input>\t<python output>`. `<input>` uses the escaping
    of gen_float_table.py (backslash -> \\, newline -> \n, tab -> \t,
    carriage return -> \r, any other character below 0x20 -> \xNN); nothing
    else is changed, so the file holds raw non-ASCII text and a raw DEL.
    `<output>` is plain ASCII and is not escaped.

quote_bytes.tsv
    One row per byte string: `<input as lower-case hex>\t<python output>`,
    where the output is `urllib.parse.quote(bytes, safe='')`. Many inputs are
    not valid UTF-8. The empty byte string is an empty first field.
"""

import os
import random
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True
sys.path.insert(0, HERE)
from gen_float_table import escape  # noqa: E402

SEED = 20261116


def quote(value):
    return urllib.parse.quote(value, safe="")


def random_code_point(rng):
    while True:
        cp = rng.choice([
            rng.randint(0x80, 0x7FF),
            rng.randint(0x800, 0xFFFF),
            rng.randint(0x10000, 0x10FFFF),
        ])
        if not 0xD800 <= cp <= 0xDFFF:
            return chr(cp)


def text_inputs():
    rng = random.Random(SEED)
    rows = []

    # Every ASCII character on its own, NUL and DEL too.
    rows += [chr(c) for c in range(0x80)]

    # Every pair of a safe character and a special one, both ways round.
    safe = "_.-~aZ09"
    special = " /+&=%?#:@!$'()*,;[]\"<>\\^`{|}\x00\t\n\r\x1f\x7f\u00e9\u00a0"
    pool = safe + special
    for a in pool:
        for b in pool:
            rows.append(a + b)

    # Latin-1 above ASCII, one at a time, and some words.
    rows += [chr(c) for c in range(0x80, 0x100)]
    rows += ["caf\u00e9", "na\u00efve", "\u00fcber", "Ren\u00e9e",
             "\u00a9 2026", "\u00bfQu\u00e9?", "\u00df\u00e6\u00f8\u00e5"]

    # CJK, emoji (4-byte UTF-8), joiners and flags, combining marks.
    rows += ["\u4e2d\u6587", "\u65e5\u672c\u8a9e", "\ud55c\uad6d\uc5b4",
             "\U0001F600", "\U0001F331 garden", "\U0001F468\u200d\U0001F469\u200d\U0001F467",
             "\U0001F1FA\U0001F1F8", "\U00010000", "\U0010FFFF", "\uffff", "\ufeff",
             "\ue000", "\ud7ff", "\u0800", "\u07ff", "e\u0301", "a\u0308\u0304",
             "\u0915\u094d\u0937", "\u2028\u2029", "\u0085", "\u200b", "\u0416"]

    # Empty, all percent signs, all plus signs, already-encoded text.
    rows += ["", "%", "%%", "%" * 50, "+", "++", "+" * 50, "%2F", "%25",
             "a+b%20c", "100%", "%%+%+%%", "%zz", "%C3%A9"]

    # Paths and other things a CLI puts in a query string.
    rows += ["/", "//", "/a/b/c", "a/b", "/Users/acme/Dev Projects/K5",
             "C:\\dir\\file.txt", "../x", "./x", "~/notes.md", "~",
             "https://example.com/a?b=c&d=e#f", "key=value&other=1",
             "name with spaces", "tab\there", "line1\nline2", "a;b,c",
             "user@example.com", "[::1]:8080", "--flag", "-n", "-e"]

    # A long string.
    rows.append(("abc/def ghi+\u00e9\U0001F600%~" * 400))

    # Random strings from ASCII, controls and random code points.
    ascii_pool = [chr(c) for c in range(0x20, 0x7F)]
    control_pool = [chr(c) for c in list(range(0x20)) + [0x7F]]
    for _ in range(400):
        length = rng.randint(0, 24)
        chars = []
        for _ in range(length):
            kind = rng.randint(0, 9)
            if kind < 5:
                chars.append(rng.choice(ascii_pool))
            elif kind < 7:
                chars.append(rng.choice(control_pool))
            else:
                chars.append(random_code_point(rng))
        rows.append("".join(chars))

    return rows


def byte_inputs():
    rng = random.Random(SEED + 1)
    rows = []

    # Every byte on its own.
    rows += [bytes([b]) for b in range(256)]

    # Empty, valid UTF-8, and the usual kinds of invalid UTF-8.
    rows += [
        b"", b"abc", "caf\u00e9".encode(), "\U0001F600".encode(),
        b"\xc3", b"\xc3\x28", b"\xa0\xa1", b"\xe2\x28\xa1", b"\xe2\x82",
        b"\xf0\x9f\x98", b"\xf0\x28\x8c\xbc", b"\xc0\xaf", b"\xe0\x80\xaf",
        b"\xed\xa0\x80", b"\xed\xbf\xbf", b"\xf4\x90\x80\x80", b"\xf8\x88\x80\x80\x80",
        b"\xfe\xff", b"\xff\xfe\x00a", b"a\xffb", b"a\x80/b c", b"\x00\x00",
        b"%\xff+", b"~_.-\xff",
    ]

    # Random byte strings: some any bytes, some mostly ASCII.
    for _ in range(300):
        length = rng.randint(0, 20)
        if rng.randint(0, 1):
            data = bytes(rng.randrange(256) for _ in range(length))
        else:
            data = bytes(
                rng.randrange(0x20, 0x7F) if rng.randint(0, 3) else rng.randrange(256)
                for _ in range(length))
        rows.append(data)

    return rows


def write(name, lines):
    path = os.path.join(HERE, name)
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        for line in lines:
            assert "\n" not in line and "\r" not in line, repr(line)
            out.write(line + "\n")
    print("%s: %d rows" % (name, len(lines)))


def main():
    seen = set()
    lines = []
    for text in text_inputs():
        if text in seen:
            continue
        seen.add(text)
        output = quote(text)
        assert output.isascii() and "\t" not in output, repr(output)
        lines.append(escape(text) + "\t" + output)
    write("quote.tsv", lines)

    seen = set()
    lines = []
    invalid = 0
    for data in byte_inputs():
        if data in seen:
            continue
        seen.add(data)
        try:
            data.decode("utf-8")
        except UnicodeDecodeError:
            invalid += 1
        lines.append(data.hex() + "\t" + quote(data))
    assert invalid >= 100, invalid
    write("quote_bytes.tsv", lines)


if __name__ == "__main__":
    main()
