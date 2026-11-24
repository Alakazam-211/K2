#!/usr/bin/env python3
"""The brand gate: no new hard-coded product name or command name in Rust or
renderer code.

    python3 scripts/garden/brand-gate.py             check; exit 1 on a failure
    python3 scripts/garden/brand-gate.py --tighten   lower each count to today's and
                                                     drop rows at zero (reasons kept)
    python3 scripts/garden/brand-gate.py --list [KIND]
                                                     print every counted line
    python3 scripts/garden/brand-gate.py --list value --debt
                                                     print the `value` rows that are
                                                     the command, and their total
    python3 scripts/garden/brand-gate.py --unsorted  print the `value` rows still
                                                     `debt: unsorted value` (none
                                                     after B2; the check refuses one)

The product's names live in one module each: crates/k2-client/src/brand.rs
(`k2_core::brand`, with `product_name!()` / `cli_name!()` for `concat!`) and
src/renderer/lib/brand.ts, both fed from the brand file (Rust by the
crates/k2-brand leaf crate, the renderer by scripts/brand-values.mjs, which
writes src/renderer/lib/brand.generated.ts). This
gate counts the code lines outside them that still spell a name out, per file
and kind, and holds each to its row in an allowlist. Two rules, as the zone
gate:
  1. A listed file may not have more such lines than its row.
  2. A file with no row for that kind may have none.
So the counts can only fall. When this fails, use the brand module. If the
literal really must stay (a name another program reads), add a row whose
reason says so, starting `keeper:`, and say why in the commit.

Kinds in scripts/brand-allowlist.txt, the product's names:
  K2    the word K2 on its own ("K2", "K2 Connect", K2.app; not K2_X,
        K2Error, X-K2-Header, a path segment such as Who::K2, or k2): the
        brand-inventory product-name rule, less Rust paths
  K5    the word K5 on its own: today the K5 Space and old review labels;
        a new one is either the future name used too early or a collision
  repo  the release repo, Alakazam-211/K2 (not K2SO)

Kinds in scripts/cli-name-allowlist.txt, the command's name (`k2` and `k5`,
the word scripts/garden/cli-name-flip.py matches, either digit):
  cli    the command name typed out: the word reads as the command, by the
         flip's own reads_as_command (a verb, a flag or a placeholder after
         it, backticks, "run k2"). Rust: only inside a string literal.
         Renderer: anywhere on a code line. Not a `[k2 ...]` stamp and not a
         string that is only the name (that is `value`).
  value  a string that is exactly "k2" or "k5". Rust: a string literal.
         Renderer: '...', "..." or `...` on a code line, not after a `//`
         (a command built from parts, ['k2', 'msg'].join(' '), reads as a
         command to nobody, so `cli` cannot see it). Bash, the CLI's
         parts (cli/src, and cli/text where it exists): a line where the
         flip finds a string that is exactly the name, or the name as a
         word in shell code (a mode in a case pattern, the called-as
         rule), by the flip's own reader; the flip calls both `kept` and
         never rewrites them, and this row is what holds them. Each line
         has one class. Kept in every brand: stored (on this machine's disk or
         database, read back by our own code), wire (another program of
         ours reads it: another server, the app, an App, the CLI's JSON),
         outside (a program that is not ours holds it), inert (no reader
         cares). Or command (the command a person or agent types, or its
         file's name on disk), which a later step moves to CLI_NAME. A row
         is `keeper: <class>[+<class>]; <who reads it>`, or
         `debt: command (<n> lines); <step>` with `; keeps: <class>; <who
         reads it>` after it when the file has other lines too. A new
         exact value needs its class the day it is written: `debt:
         unsorted value`, B1's seed, is refused. The rule: a kept value
         never becomes CLI_NAME; a brand does not rename it. The line by
         line record is the gardener's cli-name-values.md. --list value
         --debt prints the command's lines and their total.
  glued  a brand name built into a token, with no space between: Rust
         `{CLI_NAME}`, `{PRODUCT_NAME}`, `{BRAND_ID}` in a string, or
         `cli_name!()`, `product_name!()` beside a literal in a `concat!`;
         renderer `${CLI_NAME}`, `${PRODUCT_NAME}` in a template literal.
         Glued means a neighbour in [A-Za-z0-9_/=:-], or a `.` then a letter
         or digit after it. A full stop, comma, quote, backtick, bracket or
         space beside the name is prose and not counted. A header, cookie,
         key, path or prefix built this way changes with the brand, and two
         brands stop talking. Every row is `keeper: <who reads this name>`;
         a `debt:` row is an error.
         The name passed as a value is glued the same way (C4, the
         interop step: a path no live test walks): Rust, a `{}`, `{0}`
         or `{x}` hole in any macro's format string whose argument is
         CLI_NAME, PRODUCT_NAME, BRAND_ID (any path to them, a `&` before
         them) or cli_name!(), product_name!(); a `+` between one of
         them and a string literal; one of them in `[...].concat()` or
         `[...].join(sep)` beside a literal or a gluing separator.
         Renderer: `+` between CLI_NAME or PRODUCT_NAME and a quoted
         literal, and the same `[...].join(sep)`. Known limit: a name
         held in a variable first (let n = CLI_NAME; format!("{n}-x")) is
         not seen.

And one count that is not per file, in scripts/garden/cli-name-unsure-ceilings.txt:
the flip's `unsure` uses in the bash CLI's parts (cli/src, and cli/text where
the flip reads it), one ceiling per reason (`<count> <reason>`). A reason may
not go over its ceiling, a reason with no row may have none, and --tighten
lowers them. Three reasons are closed and may not have a row at all: a
string that is exactly the name and the name as a word in shell code (the
flip keeps them; the `value` rows hold them), and interpreter code not in a
string (write it as text, a comment or a name set with `=`). The Rust area
has no ceiling here: its code lines are `cli` and `value`, and its tests may
name the command freely.

And one rule with no allowlist, the slot rule (rebrand B7, trap 1): a
`{cli}` or `{product}` slot in the format string of a format-family macro
(format!, write!, println!, eprintln!, format_args!, panic!, assert!,
anyhow!, log_debug! and the like: FORMAT_MACROS) fails, in every Rust file
of crates/ and src-tauri/, tests too, but not the k5 CLI (its `cli` is the
name it was called by; it fills slots, it never writes one) nor the brand
crate. Rust reads it as a capture: where a local `cli` or `product` exists
it builds and prints that local. The one form: a slot in a format string
is `{{cli}}` / `{{product}}`, this build's name `{CLI_NAME}` /
`{PRODUCT_NAME}`; a plain literal writes the slot `{cli}`.

`k2.dev` is the zone gate's (scripts/garden/zone-gate.py); scripts/brand-gate.sh
runs both.

What is counted: a code line, the zone gate's way. Rust under crates/ and
src-tauri/src/: not a `//` line, not after a `//` (for the command's kinds,
the flip's own reading of strings and comments), not from the file's first
`#[cfg(test)]` (or `#[cfg(all(test, …))]`) inline `mod name {` on (a
`mod name;` only declares a test file: the code after it counts), and test
files (tests.rs, *_test.rs, *_tests.rs, test_*.rs, tests_*.rs). Renderer under src/: .ts and
.tsx, not a comment line.
Test files are not counted (they pin what the names are), nor the brand and
hosts modules, nor anything under crates/k2-brand/.

Allowlist rows: `<lines> <kind> <path> <reason>`, the reason starting
`keeper:` (stays on purpose: say who reads it) or `debt:` (should move to the
module). A row without one of those is an error.

Each file is read once for every kind.

For tests: --root DIR, --allowlist FILE, --cli-allowlist FILE and
--unsure-ceilings FILE point it at another tree.
"""
import importlib.util
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
    "src/renderer/lib/brand.generated.ts",
    "src/renderer/lib/hosts.ts",
}
MODULE_DIRS = ("crates/k2-brand/",)
BRAND_KINDS = {
    "K2": re.compile(r"(?<![A-Za-z0-9_])(?<![Xx]-)(?<!::)K2(?![A-Za-z0-9_]|::)"),
    "K5": re.compile(r"(?<![A-Za-z0-9_])(?<![Xx]-)K5(?![A-Za-z0-9_])"),
    "repo": re.compile(r"Alakazam-211/K2(?![A-Za-z0-9_])"),
}
CLI_KINDS = ("cli", "value", "glued")
HINT = {
    "K2": "use k2_core::brand or @/lib/brand",
    "K5": "use k2_core::brand or @/lib/brand",
    "repo": "use k2_core::brand or @/lib/brand",
    "cli": "use CLI_NAME or cli_name!() (k2_core::brand, @/lib/brand)",
    "value": "use CLI_NAME, or add a keeper: row that says its class and who reads the value",
    "glued": "keep the name apart from the token, or add a keeper: row that says who reads this name",
}
AREAS = [
    {"dirs": ("crates", "src-tauri/src"), "ext": (".rs",), "rust": True,
     "test_file": re.compile(r"(^tests\.rs$|_tests?\.rs$|^tests?_.*\.rs$)"), "comment": ("//",)},
    {"dirs": ("src",), "ext": (".ts", ".tsx"), "rust": False,
     "test_file": re.compile(r"(\.test\.tsx?$|\.wktest\.ts$|\.spec\.tsx?$|-fixture\.ts$|\.d\.ts$|\.generated\.ts$)"),
     "comment": ("//", "*", "/*", "{/*")},
]

# The command's name: the flip's word and its rules, imported, not copied.
# Its file name has hyphens, so it is loaded by path. No bytecode is written,
# so a run leaves no __pycache__ in the tree.
sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location(
    "cli_name_flip", os.path.join(os.path.dirname(os.path.abspath(__file__)), "cli-name-flip.py"))
flip = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(flip)
if "k2" not in flip.WORD.pattern:
    sys.exit("brand-gate: cli-name-flip.py's WORD no longer spells k2; this gate builds its word from it")
NAME = re.compile(flip.WORD.pattern.replace("k2", "k[25]"))

# A neighbour that glues a brand name into a token.
GLUE = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_/=:-")
RUST_HOLE = re.compile(r"\{(CLI_NAME|PRODUCT_NAME|BRAND_ID)\}")
RUST_MACRO = re.compile(r"\b(cli_name|product_name)!\(\)")
TS_HOLE = re.compile(r"\$\{(CLI_NAME|PRODUCT_NAME)\}")
GLUE_HINT = re.compile(r"\{(CLI_NAME|PRODUCT_NAME|BRAND_ID)\}|\b(cli_name|product_name)!\(\)|\b(CLI_NAME|PRODUCT_NAME|BRAND_ID)\b")
# A brand value as a whole expression: a const by any path, with a `&`
# or a `.to_string()` and the like around it, or one of the two macros.
RUST_VALUE = re.compile(
    r"&?\s*(?:(?:[A-Za-z_][A-Za-z0-9_]*::)*(?:CLI_NAME|PRODUCT_NAME|BRAND_ID)|(?:\$crate::|(?:[A-Za-z_][A-Za-z0-9_]*::)*)?(?:cli_name|product_name)!\(\))"
    r"(?:\s*\.\s*(?:to_string|to_owned|into|as_str)\(\))?")
RUST_MACRO_CALL = re.compile(r"(?<![A-Za-z0-9_])([A-Za-z_][A-Za-z0-9_]*)!\s*\(")
TS_VALUE = r"(?:[A-Za-z_$][A-Za-z0-9_$]*\.)?(?:CLI_NAME|PRODUCT_NAME)\b"
TS_LIT = r"""(?:'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\$])*`)"""
TS_PLUS_AFTER = re.compile(r"(?<![A-Za-z0-9_$.])" + TS_VALUE + r"\s*\+\s*(" + TS_LIT + ")")
TS_PLUS_BEFORE = re.compile("(" + TS_LIT + r")\s*\+\s*(?<![A-Za-z0-9_$])" + TS_VALUE)
TS_JOIN = re.compile(r"\[([^\[\]]*)\]\s*\.\s*join\(\s*(" + TS_LIT + r")\s*\)")


def glued(before, after, after2):
    """The characters around a name glue it into a token. A `.` after it
    glues only before a letter or digit, and a `:` only before something
    that is not a space (`k2: no such command` is a sentence)."""
    if before in GLUE:
        return True
    if after == ".":
        return after2.isalnum()
    if after == ":":
        return after2 != "" and not after2.isspace()
    return after in GLUE


def rust_literal_end(text, ctx, i):
    """The last character inside a string literal that closes at or before
    `i` (skipping spaces and one comma back), or '' when there is none or
    it is an escape."""
    j = i
    while j >= 0 and text[j] in " \t\r\n":
        j -= 1
    if j < 0 or text[j] != ",":
        return ""
    j -= 1
    while j >= 0 and text[j] in " \t\r\n":
        j -= 1
    while j >= 0 and text[j] == "#" and ctx[j] == "string":
        j -= 1
    if j < 1 or text[j] != '"' or ctx[j] != "string":
        return ""
    c = text[j - 1]
    k, slashes = j - 2, 0
    while k >= 0 and text[k] == "\\":
        slashes += 1
        k -= 1
    if slashes % 2 or c == '"':
        return ""
    return c


def rust_literal_start(text, ctx, i):
    """The first two characters inside a string literal that opens at or
    after `i` (skipping spaces and one comma), or '' when there is none or
    it starts with an escape."""
    n = len(text)
    j = i
    while j < n and text[j] in " \t\r\n":
        j += 1
    if j >= n or text[j] != ",":
        return ""
    j += 1
    while j < n and text[j] in " \t\r\n":
        j += 1
    m = flip.RAW_OPEN.match(text, j)
    if m:
        j = m.end()
    elif j < n and text[j] == '"':
        j += 1
    else:
        return ""
    if j >= n or text[j] == "\\":
        return ""
    return text[j:j + 2]


def in_concat(text, ctx, i):
    """`i` is inside the parentheses of a `concat!(...)`."""
    depth = 0
    j = i - 1
    while j >= 0:
        if ctx[j] == "code":
            c = text[j]
            if c == ")":
                depth += 1
            elif c == "(":
                if depth == 0:
                    return text[:j].rstrip().endswith("concat!")
                depth -= 1
            elif c in ";{}":
                return False
        j -= 1
    return False


BRACKETS = {"(": ")", "[": "]", "{": "}"}


def rust_close(text, ctx, i):
    """The index of the code bracket that closes the one at `i`, or -1
    (no close, or a mismatch: the file is read as nothing glued there)."""
    stack = [BRACKETS[text[i]]]
    for j in range(i + 1, len(text)):
        if ctx[j] != "code":
            continue
        c = text[j]
        if c in BRACKETS:
            stack.append(BRACKETS[c])
        elif c in ")]}":
            if c != stack.pop():
                return -1
            if not stack:
                return j
    return -1


def rust_open(text, ctx, i):
    """The index of the code `[` that opens the `]` at `i`, or -1."""
    depth = 0
    for j in range(i, -1, -1):
        if ctx[j] != "code":
            continue
        c = text[j]
        if c in ")]}":
            depth += 1
        elif c in "([{":
            depth -= 1
            if depth == 0:
                return j if c == "[" else -1
        elif c == ";":
            return -1
    return -1


def rust_args(text, ctx, lo, hi):
    """The comma-separated parts of text[lo:hi] at depth 0, as trimmed
    (start, end) spans."""
    spans, depth, start = [], 0, lo
    for j in range(lo, hi):
        if ctx[j] != "code":
            continue
        c = text[j]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            spans.append((start, j))
            start = j + 1
    spans.append((start, hi))
    out = []
    for s, e in spans:
        while s < e and text[s].isspace():
            s += 1
        while e > s and text[e - 1].isspace():
            e -= 1
        if s < e:
            out.append((s, e))
    return out


def rust_literal(text, ctx, s, e):
    """The content span of the string literal that is exactly text[s:e]
    (`"..."`, `b"..."`, `r#"..."#`), or None."""
    if any(ctx[k] != "string" for k in range(s, e)):
        return None
    m = flip.RAW_OPEN.match(text, s)
    if m and m.start() == s:
        lo, hi = m.end(), e - 1 - len(m.group(1))
    elif text[s] == '"' or text.startswith('b"', s):
        lo, hi = s + (2 if text[s] == "b" else 1), e - 1
    else:
        return None
    return (lo, hi) if lo <= hi and text[hi] == '"' else None


def escaped(text, i, lo):
    """text[i] is the letter of an escape (`\\n`): an odd run of
    backslashes before it, not before `lo`."""
    k, n = i - 1, 0
    while k >= lo and text[k] == "\\":
        n += 1
        k -= 1
    return n % 2 == 1


def lit_last(text, lo, hi):
    """The last content character of the literal text[lo:hi], '' for none
    or an escape."""
    if hi <= lo or escaped(text, hi - 1, lo) or text[hi - 1] == "\\":
        return ""
    return text[hi - 1]


def lit_first(text, lo, hi):
    """The first two content characters of the literal text[lo:hi], ''
    for none or an escape."""
    if hi <= lo or text[lo] == "\\":
        return ""
    return text[lo:min(lo + 2, hi)]


def fmt_holes(text, lo, hi):
    """(start, end, argument) for each `{...}` hole of the format string
    text[lo:hi]; the argument is '' (the next one), digits or a name."""
    i = lo
    while i < hi:
        c = text[i]
        if c == "{" and text.startswith("{{", i):
            i += 2
        elif c == "{":
            j = text.find("}", i, hi)
            if j < 0:
                return
            yield i, j + 1, text[i + 1:j].split(":", 1)[0].strip()
            i = j + 1
        elif c == "}" and text.startswith("}}", i):
            i += 2
        else:
            i += 1


NAMED_ARG = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s*=(?!=)\s*")


def rust_value_glued(text, ctx, add):
    """Call `add(position)` for each brand value passed into a token: a
    format-string hole whose argument is the value, a `+` beside a
    literal, a `[...].concat()` or `[...].join(sep)` element."""
    def is_value(s, e):
        return RUST_VALUE.fullmatch(text, s, e) is not None

    for m in RUST_MACRO_CALL.finditer(text):
        if ctx[m.start()] != "code" or m.group(1) in ("concat", "cli_name", "product_name"):
            continue
        close = rust_close(text, ctx, m.end() - 1)
        if close < 0:
            continue
        args = rust_args(text, ctx, m.end(), close)
        tpl = None
        for k, (s, e) in enumerate(args):
            lit = rust_literal(text, ctx, s, e)
            if lit:
                tpl = (k, lit)
                break
        if not tpl:
            continue
        k, (lo, hi) = tpl
        positional, named = [], {}
        for s, e in args[k + 1:]:
            nm = NAMED_ARG.match(text, s, e)
            if nm:
                named[nm.group(1)] = (nm.end(), e)
            else:
                positional.append((s, e))
        nxt = 0
        for hs, he, arg in fmt_holes(text, lo, hi):
            if arg == "":
                span = positional[nxt] if nxt < len(positional) else None
                nxt += 1
            elif arg.isdigit():
                span = positional[int(arg)] if int(arg) < len(positional) else None
            else:
                span = named.get(arg)
            if not span or not is_value(*span):
                continue
            before = "" if hs == lo or escaped(text, hs - 1, lo) else text[hs - 1]
            after = lit_first(text, he, hi)
            if glued(before, after[:1], after[1:2]):
                add(hs)

    for m in re.finditer(r"\b(?:CLI_NAME|PRODUCT_NAME|BRAND_ID)\b|\b(?:cli_name|product_name)!\(\)", text):
        if ctx[m.start()] != "code":
            continue
        # The whole value: a path and a `&` before it, a `.to_string()` after.
        s = m.start()
        while s > 0 and (text[s - 1].isalnum() or text[s - 1] in "_:$"):
            s -= 1
        v = RUST_VALUE.match(text, s)
        if not v or v.end() < m.end():
            continue
        e = v.end()
        # value + "..."
        j = e
        while j < len(text) and text[j].isspace():
            j += 1
        if j < len(text) and text[j] == "+" and ctx[j] == "code":
            j += 1
            while j < len(text) and (text[j].isspace() or text[j] == "&"):
                j += 1
            if j < len(text) and ctx[j] == "string":
                ls = j
                while j < len(text) and ctx[j] == "string":
                    j += 1
                lit = rust_literal(text, ctx, ls, j)
                if lit:
                    a = lit_first(text, *lit)
                    if glued("", a[:1], a[1:2]):
                        add(m.start())
        # "..." + value, the literal maybe as "...".to_string() or String::from("...")
        j = s - 1
        while j >= 0 and text[j].isspace():
            j -= 1
        if j >= 0 and text[j] == "&":
            j -= 1
            while j >= 0 and text[j].isspace():
                j -= 1
        if j >= 0 and text[j] == "+" and ctx[j] == "code":
            j -= 1
            while j >= 0 and text[j].isspace():
                j -= 1
            head = text[max(0, j - 15):j + 1]
            for tail in (".to_string()", ".to_owned()", ".into()"):
                if head.endswith(tail):
                    j -= len(tail)
                    break
            else:
                if j >= 0 and text[j] == ")" and ctx[j] == "code":
                    j -= 1
            le = j + 1
            while j >= 0 and ctx[j] == "string":
                j -= 1
            lit = rust_literal(text, ctx, j + 1, le) if le > j + 1 else None
            if lit and glued(lit_last(text, *lit), "", ""):
                add(m.start())

    for m in re.finditer(r"\]\s*\.\s*(concat|join)\s*\(", text):
        if ctx[m.start()] != "code":
            continue
        open_i = rust_open(text, ctx, m.start())
        if open_i < 0:
            continue
        elems = rust_args(text, ctx, open_i + 1, m.start())
        if m.group(1) == "join":
            close = rust_close(text, ctx, m.end() - 1)
            sep_args = rust_args(text, ctx, m.end(), close) if close > 0 else []
            sep = rust_literal(text, ctx, *sep_args[0]) if len(sep_args) == 1 else None
            if not sep:
                continue
            sep_text = text[sep[0]:sep[1]]
        else:
            sep_text = ""
        for k, (s, e) in enumerate(elems):
            if not is_value(s, e):
                continue
            if sep_text:
                left = sep_text[-1] if k > 0 else ""
                right = sep_text[:2] if k + 1 < len(elems) else ""
            else:
                lp = rust_literal(text, ctx, *elems[k - 1]) if k > 0 else None
                rp = rust_literal(text, ctx, *elems[k + 1]) if k + 1 < len(elems) else None
                left = lit_last(text, *lp) if lp else ""
                right = lit_first(text, *rp) if rp else ""
            if glued(left, right[:1], right[1:2]):
                add(s)


# The slot rule (rebrand B7, trap 1). A `{cli}` or `{product}` slot in a
# format-family macro's format string is not a slot: Rust reads it as a
# capture of a local named `cli` or `product`, and where one exists (a
# harness name, a path) the line builds and prints that in its place. In a
# format string a slot is written `{{cli}}` and `{{product}}`; this build's
# name is `{CLI_NAME}` and `{PRODUCT_NAME}`. A plain literal (a const, a
# json! value, a `.replace` pattern) writes the slot `{cli}`. No allowlist:
# every hit fails, in any Rust file, tests too.
SLOT_NAMES = ("cli", "product")
# The macros whose arguments hold a format string, and that string's place
# among the arguments; None is the first argument that is a whole string
# literal (the log macros, which take `target:` and fields first).
FORMAT_MACROS = {
    "format": 0, "format_args": 0, "print": 0, "println": 0, "eprint": 0, "eprintln": 0,
    "write": 1, "writeln": 1, "panic": 0, "unreachable": 0, "todo": 0, "unimplemented": 0,
    "assert": 1, "debug_assert": 1, "assert_eq": 2, "assert_ne": 2, "debug_assert_eq": 2,
    "debug_assert_ne": 2, "anyhow": 0, "bail": 0, "ensure": 1, "format_err": 0, "eyre": 0,
    "log_debug": 0, "log_info": 0, "log_warn": 0, "log_error": 0, "nlog": 0,
    "trace": None, "debug": None, "info": None, "warn": None, "error": None,
}
SLOT_HINT = ("in a format string write the slot {{cli}} or {{product}}, and this build's name "
             "{CLI_NAME} or {PRODUCT_NAME}; a local named cli or product is captured under another name")


def rust_slot_captures(text, ctx):
    """(position, hole) of each `{cli}` or `{product}` hole in the format
    string of a format-family macro call (FORMAT_MACROS), with or without
    a format spec (`{cli:>8}`). `{{cli}}` is text, not a hole."""
    out = []
    for m in RUST_MACRO_CALL.finditer(text):
        if ctx[m.start()] != "code" or m.group(1) not in FORMAT_MACROS:
            continue
        close = rust_close(text, ctx, m.end() - 1)
        if close < 0:
            continue
        args = rust_args(text, ctx, m.end(), close)
        at = FORMAT_MACROS[m.group(1)]
        lit = None
        if at is None:
            for s, e in args:
                lit = rust_literal(text, ctx, s, e)
                if lit:
                    break
        elif at < len(args):
            lit = rust_literal(text, ctx, *args[at])
        if not lit:
            continue
        for hs, he, arg in fmt_holes(text, *lit):
            if arg in SLOT_NAMES:
                out.append((hs, text[hs:he]))
    return out


# Not read by the slot rule: the CLI, whose `cli` local is the name it was
# called by and which never writes a slot (it fills them), and the brand
# crate, whose `product` and `cli` are the brand file's values.
SLOT_SKIP = ("crates/k5-cli/", "crates/k2-brand/")


def slot_scan(root):
    """[(relative path, line number, hole, line)] of every slot captured
    in a format string, over every Rust file of crates/ and src-tauri/
    (tests too; not vendored code, build output or SLOT_SKIP)."""
    found = []
    skip = {"target", "node_modules", ".git", "vendor", "third_party"}
    for top in ("crates", "src-tauri"):
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, top)):
            dirnames[:] = sorted(d for d in dirnames if d not in skip)
            if (os.path.relpath(dirpath, root).replace(os.sep, "/") + "/").startswith(SLOT_SKIP):
                dirnames[:] = []
                continue
            for name in sorted(filenames):
                if not name.endswith(".rs"):
                    continue
                full = os.path.join(dirpath, name)
                with open(full, encoding="utf-8", errors="replace") as f:
                    text = f.read()
                if not re.search(r"\{(cli|product)[}:]", text):
                    continue
                ctx = flip.rust_contexts(text)
                rel = os.path.relpath(full, root).replace(os.sep, "/")
                for pos, hole in rust_slot_captures(text, ctx):
                    n = text.count("\n", 0, pos)
                    line = text.split("\n")[n]
                    found.append((rel, n + 1, hole, line.strip()))
    return found


def ts_value_glued(line):
    """True when a renderer code line passes CLI_NAME or PRODUCT_NAME into
    a token: a `+` beside a quoted literal, or `[...].join(sep)`."""
    for m in TS_PLUS_AFTER.finditer(line):
        body = m.group(1)[1:-1]
        if body and body[0] != "\\" and glued("", body[:1], body[1:2]):
            return True
    for m in TS_PLUS_BEFORE.finditer(line):
        body = m.group(1)[1:-1]
        if body and not body.endswith("\\") and (len(body) < 2 or body[-2] != "\\") and glued(body[-1], "", ""):
            return True
    for m in TS_JOIN.finditer(line):
        elems = [e.strip() for e in m.group(1).split(",")]
        sep = m.group(2)[1:-1]
        if sep and any(re.fullmatch(TS_VALUE, e) for e in elems) and (glued(sep[-1], "", "") or glued("", sep[:1], sep[1:2])):
            return True
    return False


def rust_cli_kinds(lines, code, known, out):
    """Add the command's kinds of one Rust file to `out`. `lines` is the
    file up to its test module, `code` the indexes of its code lines."""
    text = "\n".join(lines)
    if not (NAME.search(text) or GLUE_HINT.search(text)):
        return
    ctx = flip.rust_contexts(text)
    starts = [0]
    for line in lines[:-1]:
        starts.append(starts[-1] + len(line) + 1)

    def line_of(pos):
        lo, hi = 0, len(starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if starts[mid] <= pos:
                lo = mid
            else:
                hi = mid - 1
        return lo

    hits = {}
    for m in NAME.finditer(text):
        i = line_of(m.start())
        if i not in code or ctx[m.start()] != "string":
            continue
        line = lines[i]
        lm = NAME.match(line, m.start() - starts[i])
        if flip.is_stamp(line[:lm.start()], line[lm.end():]):
            continue
        if flip.whole_string_is_name(line, lm):
            hits.setdefault("value", set()).add(i)
        elif flip.reads_as_command(line, lm, known):
            hits.setdefault("cli", set()).add(i)
    for m in RUST_HOLE.finditer(text):
        i = line_of(m.start())
        if i not in code or ctx[m.start()] != "string":
            continue
        a = text[m.end():m.end() + 2]
        if glued(text[m.start() - 1:m.start()], a[:1], a[1:2]):
            hits.setdefault("glued", set()).add(i)
    for m in RUST_MACRO.finditer(text):
        i = line_of(m.start())
        if i not in code or ctx[m.start()] != "code" or not in_concat(text, ctx, m.start()):
            continue
        b = rust_literal_end(text, ctx, m.start() - 1)
        a = rust_literal_start(text, ctx, m.end())
        if (b and glued(b, "", "")) or (a and glued("", a[:1], a[1:2])):
            hits.setdefault("glued", set()).add(i)

    def add_value(pos):
        i = line_of(pos)
        if i in code:
            hits.setdefault("glued", set()).add(i)

    rust_value_glued(text, ctx, add_value)
    for kind, rows in hits.items():
        out[kind] = [(i + 1, lines[i].strip()) for i in sorted(rows)]


TS_QUOTES = "'\"`"


def ts_whole_string(line, m):
    """The match is a whole string in any of the three quote forms:
    'k2', "k2" or `k2`."""
    b = line[m.start() - 1] if m.start() > 0 else ""
    a = line[m.end()] if m.end() < len(line) else ""
    return b != "" and b in TS_QUOTES and a == b


def ts_code_end(line):
    """Where a `//` comment starts on a renderer line, outside a string;
    the line's length when it has none."""
    q, i = None, 0
    while i < len(line):
        c = line[i]
        if q:
            if c == "\\":
                i += 2
                continue
            if c == q:
                q = None
        elif c in TS_QUOTES:
            q = c
        elif line.startswith("//", i):
            return i
        i += 1
    return len(line)


def ts_cli_kinds(lines, code, known, out):
    """Add the command's kinds of one renderer file to `out`."""
    hits = {}
    for i in code:
        line = lines[i]
        end = ts_code_end(line)
        cli = False
        for lm in NAME.finditer(line):
            if ts_whole_string(line, lm):
                if lm.start() < end and i not in hits.get("value", []):
                    hits.setdefault("value", []).append(i)
                continue
            if cli or flip.is_stamp(line[:lm.start()], line[lm.end():]):
                continue
            if flip.reads_as_command(line, lm, known):
                hits.setdefault("cli", []).append(i)
                cli = True
        for m in TS_HOLE.finditer(line):
            a = line[m.end():m.end() + 2]
            if glued(line[m.start() - 1:m.start()] if m.start() else "", a[:1], a[1:2]):
                hits.setdefault("glued", []).append(i)
                break
        if i not in hits.get("glued", []) and ts_value_glued(line[:end]):
            hits.setdefault("glued", []).append(i)
    for kind, rows in hits.items():
        out[kind] = [(i + 1, lines[i].strip()) for i in rows]


def inline_test_mod(line):
    """`mod name {` with its body here. A `mod name;` only declares a test
    file (left out by its name or folder); the code after it still counts."""
    return bool(TEST_MOD.match(line)) and not line.rstrip().endswith(";")


def counted_lines(path, area, known):
    """{kind: [(line number, text)]} for one file, read once."""
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    out = {}
    code = []
    end = len(lines)
    for i, line in enumerate(lines):
        s = line.strip()
        if area["rust"] and TEST_CFG.match(s) and i + 1 < len(lines) and inline_test_mod(lines[i + 1]):
            end = i
            break
        if not s or s.startswith(area["comment"]):
            continue
        code.append(i)
        text = RUST_TAIL.sub("", line) if area["rust"] else line
        for kind, rx in BRAND_KINDS.items():
            if rx.search(text):
                out.setdefault(kind, []).append((i + 1, s))
    if area["rust"]:
        rust_cli_kinds(lines[:end], set(code), known, out)
    else:
        ts_cli_kinds(lines, code, known, out)
    return out


# The bash CLI's `value` lines: where the flip finds a string that is
# exactly the name, or the name as a word in shell code. The flip keeps both
# and never rewrites them; this gate holds each line to its file's row.
BASH_VALUE_WHY = (flip.VALUE_WHY, flip.SHELL_WHY)


def bash_values(root, known, found):
    """Add the `value` lines of the CLI's parts (cli/src, and cli/text where
    it exists) to `found`, by the flip's own reader. A line counts once."""
    for path in flip.parts(root):
        hits = sorted({n for n, _, _, _, why in flip.classify_file(path, known, NAME) if why in BASH_VALUE_WHY})
        if not hits:
            continue
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        found[("value", rel)] = [(n, lines[n - 1].strip()) for n in hits]


def scan(root):
    """{(kind, relative path): [(line number, text)]}."""
    known = flip.verbs(root)
    found = {}
    bash_values(root, known, found)
    for area in AREAS:
        for top in area["dirs"]:
            for dirpath, dirnames, filenames in os.walk(os.path.join(root, top)):
                dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
                for name in sorted(filenames):
                    if not name.endswith(area["ext"]) or area["test_file"].search(name):
                        continue
                    full = os.path.join(dirpath, name)
                    rel = os.path.relpath(full, root).replace(os.sep, "/")
                    if rel in MODULES or rel.startswith(MODULE_DIRS):
                        continue
                    for kind, hits in counted_lines(full, area, known).items():
                        found[(kind, rel)] = hits
    return found


VALUE_CLASSES = ("stored", "wire", "outside", "inert")
VALUE_KEEPER = re.compile(r"keeper: ([a-z]+(?:\+[a-z]+)*); \S")
VALUE_DEBT = re.compile(r"debt: command \((\d+) lines?\)(?:; ([BC][0-9]+[a-z]?)(?:; keeps: ([a-z]+(?:\+[a-z]+)*); \S.*)?)?$")
VALUE_UNSORTED = "debt: unsorted value"
VALUE_FORMS = ("'keeper: <class>[+<class>]; <who reads it>' or "
               "'debt: command (<n> lines); <step>[; keeps: <class>[+<class>]; <who reads it>]'")


def value_reason_error(lines, reason, allow_unsorted=False):
    """What is wrong with a `value` row's reason, or None. The classes:
    stored, wire, outside, inert (kept in every brand), and command (a
    debt that names the step that moves it). `debt: unsorted value` was the
    seed until every line had a class; only --unsorted still reads it."""
    if reason == VALUE_UNSORTED:
        if allow_unsorted:
            return None
        return (f"'{VALUE_UNSORTED}' is not a reason any more: read who reads each line and give it a class, "
                f"{VALUE_FORMS}")
    m = VALUE_KEEPER.match(reason)
    if reason.startswith("keeper: "):
        if not m:
            return f"want {VALUE_FORMS}, got '{reason}'"
        return classes_error(m.group(1))
    if not reason.startswith("debt: command"):
        return f"want {VALUE_FORMS}, got '{reason}'"
    m = VALUE_DEBT.match(reason)
    if not m:
        return f"want {VALUE_FORMS}, got '{reason}'"
    n = int(m.group(1))
    if not m.group(2):
        return f"a command debt names the step that moves it (B3, B9, ...), got '{reason}'"
    if n < 1 or n > lines:
        return f"'command ({n} lines)' must be 1 to the row's {lines} lines"
    if m.group(3):
        if n == lines:
            return f"all {lines} lines are the command, so the row keeps none: drop '; keeps: ...'"
        return classes_error(m.group(3))
    if n < lines:
        return f"{lines - n} of the row's {lines} lines are not the command: add '; keeps: <class>; <who reads it>'"
    return None


def classes_error(classes):
    for c in classes.split("+"):
        if c == "ask":
            return "class 'ask': the gardener has not answered yet, so this line has no class"
        if c not in VALUE_CLASSES:
            return f"unknown class '{c}'; a kept value is {', '.join(VALUE_CLASSES)}"
    return None


def read_allowlist(path, kinds, allow_unsorted=False):
    head, rows = [], {}
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f.read().split("\n"), 1):
            if raw.startswith("#") or (not raw.strip() and not rows):
                head.append(raw)
                continue
            if not raw.strip():
                continue
            parts = raw.split(" ", 3)
            if len(parts) != 4 or not parts[0].isdigit() or int(parts[0]) < 1 or parts[1] not in kinds:
                sys.exit(f"brand-gate: {path} line {n}: want '<lines> <{'|'.join(kinds)}> <path> <keeper|debt>: <reason>', got '{raw}'")
            if not re.match(r"(keeper|debt): \S", parts[3]):
                sys.exit(f"brand-gate: {path} line {n}: the reason must start 'keeper: ' or 'debt: ', got '{parts[3]}'")
            if parts[1] == "glued" and not parts[3].startswith("keeper: "):
                sys.exit(f"brand-gate: {path} line {n}: a glued row is always 'keeper: <who reads this name>', got '{parts[3]}'")
            if parts[1] == "value":
                why = value_reason_error(int(parts[0]), parts[3], allow_unsorted)
                if why:
                    sys.exit(f"brand-gate: {path} line {n}: the value row for {parts[2]}: {why}")
            key = (parts[1], parts[2])
            if key in rows:
                sys.exit(f"brand-gate: {path} line {n}: {parts[1]} {parts[2]} is listed twice")
            rows[key] = (int(parts[0]), parts[3])
    return head, rows


def write_allowlist(path, head, rows, kinds):
    order = sorted(rows.items(), key=lambda r: (list(kinds).index(r[0][0]), -r[1][0], r[0][1]))
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(head + [f"{n} {k} {p} {why}" for (k, p), (n, why) in order]) + "\n")


# Unsure reasons with no ceiling, ever: the first two are `value` lines (the
# flip calls them kept), the third is written as text, a comment or a name.
CLOSED = {
    flip.VALUE_WHY: "the flip keeps it and its file's value row holds it",
    flip.SHELL_WHY: "the flip keeps it and its file's value row holds it",
    flip.CODE_WHY: "write it as text, a comment, or a name set with =",
}


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
            if len(parts) != 2 or not parts[0].isdigit() or int(parts[0]) < 1 or not parts[1].strip():
                sys.exit(f"brand-gate: {path} line {n}: want '<count> <reason>', got '{raw}'")
            if parts[1] in rows:
                sys.exit(f"brand-gate: {path} line {n}: '{parts[1]}' is listed twice")
            if parts[1] in CLOSED:
                sys.exit(f"brand-gate: {path} line {n}: '{parts[1]}' is closed and has no ceiling: {CLOSED[parts[1]]}")
            rows[parts[1]] = int(parts[0])
    return head, rows


def unsure_counts(root):
    """{reason: uses} of the flip's `unsure` class in the CLI's parts."""
    counts = {}
    for row in flip.collect(root):
        if row[4] == "unsure":
            counts[row[5]] = counts.get(row[5], 0) + 1
    return counts


def check_rows(found, rows, kinds):
    """This file's counts, the (kind, path) keys over their row, and the
    rows --tighten keeps."""
    mine = {k: v for k, v in found.items() if k[0] in kinds}
    over = [k for k in sorted(mine) if len(mine[k]) > rows.get(k, (0, ""))[0]]
    kept = {k: (len(mine[k]), rows[k][1]) for k in mine if k in rows}
    return mine, over, kept


def main(argv):
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    allowlist = cli_allowlist = ceilings = None
    mode, list_kind, debt = "check", None, False
    all_kinds = list(BRAND_KINDS) + list(CLI_KINDS)
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--tighten":
            mode = "tighten"
        elif a == "--unsorted":
            mode = "unsorted"
        elif a == "--debt":
            debt = True
        elif a == "--list":
            mode = "list"
            if args and args[0] in all_kinds:
                list_kind = args.pop(0)
        elif a == "--root" and args:
            root = args.pop(0)
        elif a == "--allowlist" and args:
            allowlist = args.pop(0)
        elif a == "--cli-allowlist" and args:
            cli_allowlist = args.pop(0)
        elif a == "--unsure-ceilings" and args:
            ceilings = args.pop(0)
        else:
            sys.exit(f"brand-gate: unknown argument: {a}")
    allowlist = allowlist or os.path.join(root, "scripts", "brand-allowlist.txt")
    cli_allowlist = cli_allowlist or os.path.join(root, "scripts", "cli-name-allowlist.txt")
    ceilings = ceilings or os.path.join(root, "scripts", "garden", "cli-name-unsure-ceilings.txt")

    if mode == "unsorted":
        # Read as the seed allowed, so a tree that still has some can list
        # them; the check refuses them. Prints nothing when none are left.
        _, rows = read_allowlist(cli_allowlist, CLI_KINDS, allow_unsorted=True)
        left = sorted((k, v) for k, v in rows.items() if k[0] == "value" and v[1] == VALUE_UNSORTED)
        for (kind, rel), (n, _) in left:
            print(f"{n} value {rel}")
        if left:
            print(f"brand-gate: {len(left)} value row(s) unsorted ({sum(n for _, (n, _) in left)} lines).")
        return 0

    if debt:
        if mode != "list" or list_kind != "value":
            sys.exit("brand-gate: --debt goes with --list value")
        _, rows = read_allowlist(cli_allowlist, CLI_KINDS)
        steps = {}
        for (kind, rel), (n, why) in sorted(rows.items(), key=lambda r: r[0][1]):
            m = VALUE_DEBT.match(why) if kind == "value" else None
            if m:
                print(f"{m.group(1)} {rel} {m.group(2)}")
                steps[m.group(2)] = steps.get(m.group(2), 0) + int(m.group(1))
        by_step = ", ".join(f"{s} {steps[s]}" for s in sorted(steps))
        print(f"brand-gate: {sum(steps.values())} value lines are the command" + (f" ({by_step})." if steps else "."))
        return 0

    found = scan(root)
    if mode == "list":
        for kind, rel in sorted(found):
            if list_kind in (None, kind):
                for n, text in found[(kind, rel)]:
                    print(f"{kind} {rel}:{n}: {text}")
        return 0

    files = [(allowlist, BRAND_KINDS) + read_allowlist(allowlist, BRAND_KINDS),
             (cli_allowlist, CLI_KINDS) + read_allowlist(cli_allowlist, CLI_KINDS)]
    c_head, c_rows = read_ceilings(ceilings)
    unsure = unsure_counts(root)
    unsure_over = [r for r in sorted(unsure) if unsure[r] > c_rows.get(r, 0)]

    results = []
    for path, kinds, head, rows in files:
        mine, over, kept = check_rows(found, rows, kinds)
        results.append((path, kinds, head, rows, mine, over, kept))

    if mode == "tighten":
        bad = False
        for path, kinds, head, rows, mine, over, kept in results:
            for kind, rel in over:
                bad = True
                print(f"brand-gate: FAIL {rel} has {len(mine[(kind, rel)])} {kind} lines, its row allows {rows.get((kind, rel), (0, ''))[0]}; --tighten never raises one")
        for r in unsure_over:
            bad = True
            print(f"brand-gate: FAIL {unsure[r]} unsure uses in the CLI's parts: {r}; its ceiling is {c_rows.get(r, 0)}; --tighten never raises one")
        if bad:
            return 1
        for path, kinds, head, rows, mine, over, kept in results:
            write_allowlist(path, head, kept, kinds)
            print(f"brand-gate: {os.path.basename(path)} tightened: {sum(n for n, _ in kept.values())} lines in {len(kept)} rows.")
        new = {r: n for r, n in unsure.items() if n}
        with open(ceilings, "w", encoding="utf-8") as f:
            f.write("\n".join(c_head + [f"{n} {r}" for r, n in sorted(new.items(), key=lambda x: (-x[1], x[0]))]) + "\n")
        print(f"brand-gate: {os.path.basename(ceilings)} tightened: {sum(new.values())} unsure uses for {len(new)} reasons.")
        return 0

    bad = False
    for path, kinds, head, rows, mine, over, kept in results:
        for kind, rel in over:
            bad = True
            have = len(mine[(kind, rel)])
            if (kind, rel) not in rows:
                for n, text in mine[(kind, rel)]:
                    print(f"brand-gate: FAIL {rel}:{n} spells out {kind} and has no {kind} row: {HINT[kind]}. {text[:120]}")
            else:
                print(f"brand-gate: FAIL {rel} has {have} {kind} lines, its row allows {rows[(kind, rel)][0]}: {HINT[kind]} for the new one.")
    for r in unsure_over:
        bad = True
        fix = CLOSED.get(r, "write the line so it is plainly text, or plainly code")
        print(f"brand-gate: FAIL {unsure[r]} unsure uses of the command name in the CLI's parts ({r}), the ceiling is {c_rows.get(r, 0)}: "
              f"{fix} (scripts/garden/cli-name-flip.py --unsure lists them).")
    slots = slot_scan(root)
    for rel, n, hole, text in slots:
        bad = True
        print(f"brand-gate: FAIL {rel}:{n} has the slot {hole} in a format string, where Rust reads it as a capture: {SLOT_HINT}. {text[:120]}")
    if bad:
        return 1
    for path, kinds, head, rows, mine, over, kept in results:
        slack = [k for k in rows if len(mine.get(k, [])) < rows[k][0]]
        if slack:
            print(f"brand-gate: {len(slack)} row(s) are above today's count in {os.path.basename(path)}; run scripts/garden/brand-gate.py --tighten to lock that in.")
    slack = [r for r in c_rows if unsure.get(r, 0) < c_rows[r]]
    if slack:
        print(f"brand-gate: {len(slack)} unsure ceiling(s) are above today's count; run scripts/garden/brand-gate.py --tighten to lock that in.")

    def total(kind):
        return sum(len(v) for (k, _), v in found.items() if k == kind)

    brand_mine, cli_mine = results[0][4], results[1][4]
    command = sum(int(m.group(1)) for k, v in results[1][3].items() if k[0] == "value" for m in [VALUE_DEBT.match(v[1])] if m)
    print("brand-gate: ok (" + ", ".join(f"{total(k)} {k}" for k in BRAND_KINDS) + f" lines in {len(brand_mine)} rows still spell a name out).")
    print("brand-gate: ok (" + ", ".join(f"{total(k)} {k}" for k in CLI_KINDS) + f" lines in {len(cli_mine)} rows type the command name; {command} value lines are the command).")
    print("brand-gate: ok (no {cli} or {product} slot in a format string).")
    print("brand-gate: ok (unsure in the CLI's parts: " + (", ".join(f"{unsure[r]} {r}" for r in sorted(unsure, key=lambda r: (-unsure[r], r))) or "none") + ").")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
