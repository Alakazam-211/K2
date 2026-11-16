#!/usr/bin/env python3
r"""Writes the Python parity tables that tests/json_python_parity.rs reads.

Run by hand, once, from any directory:

    python3 crates/k5-cli/tests/data/gen_float_table.py

It is NOT run at test time. The random seed is fixed, so a second run writes
the same bytes. Python is the reference: every expected value here is what
`json.dumps(json.loads(text), **options)` returned.

Files written next to this script:

float_repr.tsv
    One row per float: `<input text>\t<python output>`. No escaping, the
    fields are plain ASCII.

documents.tsv
    One row per (options, document): `<options>\t<input>\t<output>`.
    `<options>` is a comma list of `compact`, `indent=N`, `raw` (ensure_ascii
    off) and `sort`, or `-` for the defaults.
    `<input>` and `<output>` are escaped so a row stays on one line:
        backslash -> \\    newline -> \n    tab -> \t    carriage return -> \r
        any other character below 0x20 -> \xNN (two hex digits)
    Nothing else is changed, so the file holds raw non-ASCII text and a raw
    DEL (0x7f).
    `<output>` is the single word `!ENCODE-ERROR` when Python cannot write the
    result as UTF-8 (a lone surrogate with ensure_ascii off).

rejects.tsv
    One escaped input per row (same escaping), wrapped in `<` and `>` so an
    empty or all-blank input is still a visible row. Python's json.loads
    raised on every one of them when this script ran.
"""

import json
import math
import os
import random
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
SEED = 20261115


def roundtrip(text, **options):
    return json.dumps(json.loads(text), **options)


def escape(field):
    out = []
    for ch in field:
        if ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ord(ch) < 0x20:
            out.append("\\x%02x" % ord(ch))
        else:
            out.append(ch)
    return "".join(out)


def float_inputs():
    rng = random.Random(SEED)
    rows = []

    # Powers of ten, through underflow to zero and overflow to infinity.
    for e in range(-330, 331):
        rows.append("1e%d" % e)
    for e in range(-330, 331, 7):
        rows.append("-1e%d" % e)
        rows.append("9.999e%d" % e)
        rows.append("1.0000000000000002e%d" % e)

    # Where fixed notation ends and exponent notation starts.
    rows += [
        "9999999999999998.0", "9999999999999999.0", "1e16", "1e15",
        "10000000000000000.0", "10000000000000002.0", "999999999999999.9",
        "1234567890123456.0", "12345678901234567.0", "123456789012345678.0",
        "0.0001", "0.00009999", "0.0001000000000000001", "0.00001", "1e-4",
        "1e-5", "1.0e-7", "1.5e-7", "0.001", "0.00012345", "0.000012345",
        "99999.99999e-9", "1e22", "1e23", "9007199254740993.0",
        "9007199254740992.0", "4503599627370496.5",
    ]

    # Subnormals, the smallest and largest doubles, and just past them.
    rows += [
        "5e-324", "4.9e-324", "2.5e-324", "2.4e-324", "2.4703282292062328e-324",
        "1e-323", "2.2250738585072014e-308", "2.2250738585072011e-308",
        "2.225073858507201e-308", "1.7976931348623157e308",
        "1.7976931348623157e+308", "1.7976931348623158e308",
        "1.7976931348623159e308", "1.8e308", "-1.8e308", "-1e400", "1e400",
        "1e-400", "-1e-400", "4.4501477170144023e-308",
        "2.2250738585072012e-308", "1.1125369292536007e-308",
    ]

    # Sums that are not exact, written with repr.
    for value in (0.1 + 0.2, 0.1 * 3, 1.1 + 2.2, 1 / 3, 2 / 3, 1e16 + 2,
                  math.pi, math.e, 100 * 1.1, 0.7 + 0.1, 4.35 * 100):
        rows.append(repr(value))

    # Many digits.
    rows += [
        "123456789.123456789", "0.1000000000000000055511151231257827",
        "3.14159265358979323846264338327950288419716939937510",
        "0.30000000000000004440892098500626", "1.00000000000000011102230246251565",
        "9.99999999999999999999999999", "0.000000000000000000000000000001",
        "123456789012345678901234567890.0", "1" + "0" * 40 + ".0",
        "0." + "0" * 40 + "1", "2.5000000000000000000000000001",
        "1.4999999999999999999999999999", "8.5e-5", "17.0e3",
    ]

    # Zeros and negative zero.
    rows += ["0.0", "-0.0", "0e0", "-0e0", "0E5", "-0.000", "0.0e-10", "-0e+99"]

    # Integers written as floats, and the other spellings of an exponent.
    rows += [
        "2.0", "2e0", "20e-1", "0.2e1", "1E5", "1e+5", "1e5", "1E+5", "1e05",
        "1E-5", "1e-05", "100e-2", "1.50", "1.5", "0.1", "0.5", "-1.5", "-2e0",
        "12e3", "120e-1", "1.0e0", "1e1", "1e2", "42.0", "-42.0e0", "7E0",
        "1.25e2", "125e-2", "1.0E+0", "65536.0", "4294967296.0", "1e00",
    ]

    # Random doubles from random 64-bit patterns, written with repr.
    made = 0
    while made < 400:
        value = struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0]
        if math.isnan(value) or math.isinf(value):
            continue
        rows.append(repr(value))
        made += 1

    # Random doubles with a small exponent, where fixed notation is likely.
    for _ in range(150):
        value = rng.uniform(-1, 1) * 10 ** rng.randint(-8, 20)
        rows.append(repr(value))

    # Random decimal strings: 1 to 25 digits, a point somewhere, an exponent.
    for _ in range(300):
        count = rng.randint(1, 25)
        digits = str(rng.randint(1, 9)) + "".join(
            str(rng.randint(0, 9)) for _ in range(count - 1))
        point = rng.randint(1, count)
        text = digits[:point]
        if point < count:
            text += "." + digits[point:]
        style = rng.randint(0, 3)
        if style == 0 and "." in text:
            pass
        else:
            exponent = rng.choice([rng.randint(-340, 320), rng.randint(-25, 25)])
            sign = rng.choice(["", "+"]) if exponent >= 0 else ""
            text += rng.choice(["e", "E"]) + sign + str(exponent)
        if rng.randint(0, 3) == 0:
            text = "-" + text
        rows.append(text)

    return rows


# Every control character below 0x20, as JSON escapes.
ALL_CONTROLS = "".join("\\u%04x" % c for c in range(0x20))

DOCUMENTS = [
    # Nested objects and arrays, empty ones, every literal.
    '{"zeta": {"b": [1, 2, {"y": null, "x": true}], "a": {}}, "alpha": [], "m": [[], {}, [[]]], "f": false}',
    '[]',
    '{}',
    '[[]]',
    '[{}]',
    '{"a": {}}',
    '{"a": []}',
    '[1]',
    '{"k": "v"}',
    'null',
    'true',
    'false',
    '"just a string"',
    '0',
    '""',
    '{"": ""}',
    # Whitespace of all four kinds around every token.
    ' \t\r\n{ \t"a"\r\n:\t[ 1 ,\n2 ,\r3 ] , "b" : { } }\n\t ',
    # Duplicate keys: last value, first position.
    '{"a": 1, "a": 2, "b": 3}',
    '{"x": 1, "y": 2, "x": {"q": 1, "q": [], "p": 0}, "z": 3, "y": null, "x": "last"}',
    '{"b": 1, "a": 2, "b": 3, "a": 4}',
    # Integers keep every digit; -0 is 0.
    '[123456789012345678901234567890, -123456789012345678901234567890, -0, 0, -1, 9007199254740993, 18446744073709551616, 100000000000000000000000000000000000000000000000000]',
    '-0',
    # Floats of each kind.
    '[1.50, 1E5, 1e16, 1e15, 1.0e-7, 0.1, 123456789.123456789, 5e-324, -0.0, 0.0, 1.5e-7, 1.7976931348623157e308, 0.0001, 0.00001, 2.0, 20e-1, 1e22, -2.5E-3]',
    '{"big": 1e400, "small": -1e400, "tiny": 1e-400, "negtiny": -1e-400}',
    '[NaN, Infinity, -Infinity]',
    '{"nan": NaN, "inf": Infinity, "ninf": -Infinity, "list": [NaN, 1, Infinity]}',
    # Strings: emoji, accents, DEL, controls, quotes, backslashes, slashes.
    '["\U0001F600", "caf\u00e9", "na\u00efve \u00fc\u00f1\u00ee", "\u4e2d\u6587", "\u0416", "\U0001F468\u200d\U0001F469\u200d\U0001F467", "\U00010000 \U0010FFFF \uffff \ue000 \ud7ff"]',
    '["\\ud83d\\ude00", "\\uD83D\\uDE00", "\\u00e9", "\\u00E9", "\\u0041\\u0062", "\\uffff", "\\u2028\\u2029"]',
    '["\x7f", "a\x7fb", "\\u007f", "\\u007F", "\\u0080", "\x80", "\\u00a0", "\\u00ff"]',
    '["' + ALL_CONTROLS + '"]',
    '["\\n\\r\\t\\b\\f", "line1\\nline2", "tab\\there", "\\u000a\\u000d\\u0009\\u0008\\u000c", "\\u0000", "\\u001f", "\\u001b[0m"]',
    '["say \\"hi\\"", "back\\\\slash", "\\\\\\\\", "\\"", "\\\\", "a/b", "a\\/b", "http://example.com/x?y=1&z=2", "</script>", "\\\\n", "\\\\u0041", "it\'s"]',
    '{"\U0001F600": 1, "\u00e9": 2, "e": 3, "E": 4, "\x7f": 5, "~": 6, "\\n": 7, "": 8, "\\"": 9, "\\\\": 10, "/": 11, " ": 12, "10": 13, "9": 14, "a b": 15, "ab": 16, "a": 17}',
    # Key sorting is by code point: U+FFFF sorts before an emoji, and upper
    # case before lower case.
    '{"\U0001F600": 1, "\uffff": 2, "\ue000": 3, "\ud7ff": 4, "z": 5, "Z": 6, "\u00e9": 7, "e\u0301": 8, "\U00010000": 9, "aa": 10, "a": 11, "B": 12}',
    '{"b": {"d": 1, "c": {"f": [{"h": 1, "g": 2}], "e": 0}}, "a": [{"z": 1, "y": 2}, {"x": 3, "w": 4}]}',
    # Lone surrogates: Python reads them; it can only write them as escapes.
    '"\\ud83d"',
    '["\\ud83d", "\\ude00", "\\ud83dA", "\\ude00\\ud83d", "\\ud83d\\ud83d\\ude00", "\\ud83d\\u00e9", "x\\udfffy", "\\ud800\\ud800"]',
    '{"\\ud83d": 1, "\\ud7ff": 2, "\U0001F600": 3, "\\ue000": 4, "\\udc00": 5, "a": 6}',
    '{"ok": [1, 2], "bad": {"deep": ["\\udead"]}}',
    # A mixed document, the shape the daemon sends.
    '{"ok": true, "data": {"id": 18446744073709551615, "name": "Ren\u00e9e \U0001F331", "tags": ["a", "b"], "score": 0.30000000000000004, "ratio": 1e-7, "path": "C:\\\\dir\\\\file.txt", "url": "https://example.com/a/b", "note": "line1\\nline2\\ttabbed", "empty": {}, "none": null, "list": [], "nested": [[1, [2, [3, []]]]]}, "error": null}',
    '[[1, 2], [3, [4, [5, {"a": [6, {"b": 7}]}]]], [], [[], []], {"c": {}}]',
    '[1,2,3]',
    # A large object with repeated keys (40 keys, then four of them again).
    "{" + ", ".join('"k%d": %d' % (n, n) for n in range(40))
    + ', "k3": "three", "k20": [20], "k39": null, "k0": {"k0": 0, "k0": 1}, "new": true}',
    '{"a":1,"b":[true,false,null],"c":{"d":"e"}}',
]

OPTION_SETS = []
for layout in ("", "compact", "indent=1", "indent=2", "indent=0", "indent=4",
               "compact,indent=1", "compact,indent=2"):
    for raw in (False, True):
        for sort in (False, True):
            names = [layout] if layout else []
            if raw:
                names.append("raw")
            if sort:
                names.append("sort")
            OPTION_SETS.append(",".join(names) or "-")


def python_options(names):
    options = {}
    if names == "-":
        return options
    for name in names.split(","):
        if name == "compact":
            options["separators"] = (",", ":")
        elif name.startswith("indent="):
            options["indent"] = int(name[len("indent="):])
        elif name == "raw":
            options["ensure_ascii"] = False
        elif name == "sort":
            options["sort_keys"] = True
        else:
            raise ValueError(name)
    return options


REJECTS = [
    "", "  ", "\n", "[1,]", '{"a":1,}', "[,1]", "[1,,2]", "{,}", "[", "]",
    "{", "}", '{"a"', '{"a":', '{"a":1', "[1", "[1 2]", '{"a" 1}',
    '{"a":1 "b":2}', "{1:2}", "{'a':1}", "'a'", '{a:1}', '"abc', '"abc\\"',
    '"\t"', '"\n"', '"\r"', '"\x00"', '"\x1f"', '"a\x01b"', '"\\x41"',
    '"\\U0041"', '"\\a"', '"\\\'"', '"\\u12g4"', '"\\u+123"', '"\\u00"',
    '"\\u"', '"\\ud83d\\u12"', '"\\ud83d\\uzzzz"', '"\\', "01", "-01", "00",
    "1.", "2.", "1.e5", ".5", "-.5", "1e", "1e+", "1E-", "1.5e", "+1", "-",
    "--1", "- 1", "0x10", "1_0", "1e5x", "1.2.3", "nan", "inf", "-inf",
    "-NaN", "Nan", "infinity", "-Infinit", "+Infinity", "tru", "True",
    "nul", "None", "NULL", "falsee", "truefalse", "[1] x", "1 2", "{} {}",
    "[] []", "null null", "[//c\n1]", "/* c */ 1", "# c\n1", "\ufeff1",
    "\x0c1", "\x0b1", "\u00a01", "1\u00a0", "\u0661", "1.5e\u0663",
    "[1;2]", '{"a"=1}', '["a":1]', '{"a":1,"b"}', "[1}", '{"a":1]',
    '"a" "b"', "undefined", "1," , ",", ":", '{"a"::1}', "[\x00]",
]


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
    for text in float_inputs():
        if text in seen:
            continue
        seen.add(text)
        assert isinstance(json.loads(text), float), text
        output = roundtrip(text)
        assert "\t" not in text and "\t" not in output
        lines.append(text + "\t" + output)
    write("float_repr.tsv", lines)

    lines = []
    for document in DOCUMENTS:
        for names in OPTION_SETS:
            output = roundtrip(document, **python_options(names))
            try:
                output.encode("utf-8")
            except UnicodeEncodeError:
                output = "!ENCODE-ERROR"
            lines.append("\t".join([names, escape(document), escape(output)]))
    write("documents.tsv", lines)

    lines = []
    for text in REJECTS:
        try:
            json.loads(text)
        except ValueError:
            lines.append("<" + escape(text) + ">")
        else:
            raise AssertionError("python accepts %r" % text)
    write("rejects.tsv", lines)


if __name__ == "__main__":
    main()
