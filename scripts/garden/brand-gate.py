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
src/renderer/lib/brand.ts, both fed by the crates/k2-brand leaf crate. This
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
         a `debt:` row is an error. Known limit: a name passed as a value
         (path.join(CLI_NAME), format!("{}", CLI_NAME)) is not seen.

And one count that is not per file, in scripts/garden/cli-name-unsure-ceilings.txt:
the flip's `unsure` uses in the bash CLI's parts (cli/src, and cli/text where
the flip reads it), one ceiling per reason (`<count> <reason>`). A reason may
not go over its ceiling, a reason with no row may have none, and --tighten
lowers them. The Rust area has no ceiling here: its code lines are `cli` and
`value`, and its tests may name the command freely.

`k2.dev` is the zone gate's (scripts/garden/zone-gate.py); scripts/brand-gate.sh
runs both.

What is counted: a code line, the zone gate's way. Rust under crates/ and
src-tauri/src/: not a `//` line, not after a `//` (for the command's kinds,
the flip's own reading of strings and comments), not from the file's first
`#[cfg(test)]` (or `#[cfg(all(test, …))]`) mod on, and test files (tests.rs,
*_test.rs, *_tests.rs, test_*.rs, tests_*.rs). Renderer under src/: .ts and
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
GLUE_HINT = re.compile(r"\{(CLI_NAME|PRODUCT_NAME|BRAND_ID)\}|\b(cli_name|product_name)!\(\)")


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
    for kind, rows in hits.items():
        out[kind] = [(i + 1, lines[i].strip()) for i in rows]


def counted_lines(path, area, known):
    """{kind: [(line number, text)]} for one file, read once."""
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    out = {}
    code = []
    end = len(lines)
    for i, line in enumerate(lines):
        s = line.strip()
        if area["rust"] and TEST_CFG.match(s) and i + 1 < len(lines) and TEST_MOD.match(lines[i + 1]):
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
        print(f"brand-gate: FAIL {unsure[r]} unsure uses of the command name in the CLI's parts ({r}), the ceiling is {c_rows.get(r, 0)}: "
              "write the line so it is plainly text, or plainly code (scripts/garden/cli-name-flip.py --unsure lists them).")
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
    print("brand-gate: ok (unsure in the CLI's parts: " + (", ".join(f"{unsure[r]} {r}" for r in sorted(unsure, key=lambda r: (-unsure[r], r))) or "none") + ").")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
