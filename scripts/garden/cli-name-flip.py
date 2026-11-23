#!/usr/bin/env python3
"""Find where the CLI's own name, `k2`, is text for a reader, and (one day)
rewrite it to `k5`.

    python3 scripts/garden/cli-name-flip.py                 dry run: counts per class
    python3 scripts/garden/cli-name-flip.py --unsure        the lines it will not touch, for hand review
    python3 scripts/garden/cli-name-flip.py --show CLASS    the lines of one class
    python3 scripts/garden/cli-name-flip.py --apply --root DIR [--to NAME]
                                                            rewrite DIR/cli/src (text and comments only)
                                                            to NAME (default k5)

    ... --area rust        the same for Rust sources (crates/, src-tauri/src/);
                           the default area is cli

The dry run is the normal mode. It changes nothing. It reads the CLI's
parts (cli/src/*.sh and cli/src/schema/*.entries, and the printed pages
cli/text/**/*.txt where that folder exists), or with --area rust
every .rs file under crates/ and src-tauri/src/, and sorts every use of
the bare word `k2` into a class:

  text      inside words a person or agent reads: a heredoc that is printed,
            or a quoted string (a Usage line, a hint, an error). Will change.
  comment   in a shell comment. Will change.
  kept      never changed by this tool: a name that is not the CLI's
            (`[k2 ...]` log and stamp prefixes, a variable set with `k2 = `
            in embedded code), and the two forms the brand gate owns line by
            line, each with its class in scripts/cli-name-allowlist.txt
            (kind `value`): a string that is exactly "k2" (a value, not a
            sentence), and `k2` as a word in shell code (a mode in a case
            pattern, the called-as rule).
  unsure    everything else: `$(k2 ...)` where the shell would run it (not
            in a quoted heredoc, which prints it), a use in embedded code
            (python, node, sql) that is not in a string, a comment or a
            name set with `=`, and `k2` before a word that is not one of the
            CLI's commands. Never changed by this tool: someone reads each
            one and either rewrites the line so it is plainly text, or
            leaves it.

Names that only contain k2 are not matched at all: k2.dev, k2so, k2skn_,
K2_PORT, ~/.k2, cli/k2, dev.k2.daemon, k2-daemon, .k2/. Those are other
kinds of name with their own rules.

In Rust, text is a string literal where k2 reads as the command (same
rule), comments are `//` and `/* */` comments and doc comments, and a use
in code outside a string is unsure. Test files are read too: a test that
asserts a sentence has to change with the sentence.

--apply is for the release that changes the name, and for the test that
proves the rewrite leaves a script that still parses. It needs --root, so
it can never run on the tree it lives in by accident; it rewrites `text`
and `comment` uses to k5 (or to --to NAME, a command name such as a CI
brand's) and leaves `kept` and `unsure` alone. crates/k5-cli/src/brand_text.rs
is this rule for printed text, in Rust; its test compares the two.
"""
import os
import re
import sys

WORD = re.compile(r"(?<![\w./$~-])k2(?![\w.:/-])")
HEREDOC = re.compile(r"<<(-?)\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\2")
# A heredoc or a quoted string that starts after one of these is a program
# for that interpreter, not words for a reader.
# The word must be a command: not a variable ($node, ${node#-}), a flag
# (--node), an assignment (node="") or a file's extension (.sh). Without
# this, `[ -n "$node" ] || echo "usage: k2 ..."` read as a Node program.
INTERPRETER = re.compile(r"(?<![$\w{.-])(\bpython3?\b|\$_k2_py\b|\$\{?PY\b|\bnode\b|\bsqlite3\b|\bpsql\b|\bosascript\b|\bawk\b|\bjq\b|\bsed\b|\bruby\b|\bperl\b|\bssh\b|\bbash\b|\bsh\b|\beval\b)(?![\w=-])")
CLASSES = ("text", "comment", "kept", "unsure")
NAME_WHY = "a name in interpreter code (k2 = ...)"
# The two forms the brand gate counts as `value` lines (brand-gate.py reads
# these names, so they are the one spelling of each reason).
VALUE_WHY = "a string that is exactly k2 (a value)"
SHELL_WHY = "k2 as a word in shell code"
CODE_WHY = "interpreter code, not in a string"
# After `k2 `, these mark a use as the command even when the word is not a
# verb: a flag, a placeholder, an ellipsis.
COMMAND_TAIL = re.compile(r" (--?[a-z]|<|\[|…|\.\.\.)")
NEXT_WORD = re.compile(r" ([a-z][a-z0-9-]*)")
# Exact phrases where the name is the command though no rule below says so:
# (the text just before it, the text just after it, why). A person reads
# the program here, so a brand's build prints its own command. Kept short
# and exact on purpose: a new row needs a reason. crates/k5-cli/src/
# brand_text.rs holds the same list (COMMAND_PHRASES); its test holds the
# two rewrites equal.
COMMAND_PHRASES = (
    ("ran ", " from", "the program a person ran, in a folder (compute's job folder)"),
    ("run ", " from a terminal", "the program run from a terminal outside the app (llm account)"),
    ("", " asks for the confirm", "the program that asks the person at its terminal (migrate export-secrets)"),
    ("where ", " uses the owner's token", "the program run in a plain shell (compute git-access)"),
    ("No ", " hostdb", "a verb the command does not have (db bind)"),
)
EXTRA_VERBS = {"help", "version", "study", "update", "setup", "explain", "capabilities", "whoami", "cli", "command", "commands", "verb", "verbs"}


def parts(root):
    """The CLI's parts: cli/src/*.sh and its schema entries, then the
    printed pages under cli/text (where the folder exists)."""
    out = []
    for top, ends in (("src", (".sh", ".entries")), ("text", (".txt",))):
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, "cli", top)):
            dirnames.sort()
            for name in sorted(filenames):
                if name.endswith(ends):
                    out.append(os.path.join(dirpath, name))
    return out


def verbs(root):
    """The first words of the CLI's commands: every `"name": "<verb> ..."` in
    the schema entries, plus the top-level words the router answers."""
    found = set(EXTRA_VERBS)
    name = re.compile(r'"name":\s*"([a-z][a-z0-9-]*)')
    label = re.compile(r"^\s+([a-z][a-z0-9-]*(?:\|[a-z][a-z0-9-]*)*)\)")
    for path in parts(root):
        with open(path, encoding="utf-8") as f:
            text = f.read()
        if path.endswith(".entries") or os.path.basename(path).startswith("schema"):
            found.update(name.findall(text))
        if os.path.basename(path) == "router.sh":
            for line in text.split("\n"):
                m = label.match(line)
                if m:
                    found.update(w for w in m.group(1).split("|") if w)
    return found


def contexts(line, stack):
    """The context of each character of a shell line: 'bare', 'comment',
    'squote', 'dquote', each of the last two with '-code' when the string
    is a program. `stack` is the open quotes and `$(` groups carried from
    the line before: frames ('s'|'d', is code) and ('(', depth). Returns
    (list, stack, heredoc or None)."""
    stack = list(stack)
    ctx = []
    heredoc = None
    i = 0
    n = len(line)

    def tag():
        for kind, extra in reversed(stack):
            if kind in "sd":
                return ("squote" if kind == "s" else "dquote") + ("-code" if extra else "")
            # a $( group: what is inside it is shell code again
            return "bare"
        return "bare"

    while i < n:
        c = line[i]
        top = stack[-1][0] if stack else None
        if top == "s":
            ctx.append(tag())
            if c == "'":
                stack.pop()
            i += 1
            continue
        if top == "d":
            if c == "\\" and i + 1 < n:
                ctx += [tag(), tag()]
                i += 2
                continue
            if c == "$" and line[i + 1:i + 2] == "(":
                ctx += [tag(), tag()]
                stack.append(("(", 1))
                i += 2
                continue
            ctx.append(tag())
            if c == '"':
                stack.pop()
            i += 1
            continue
        # bare: the top level, or inside a $( group
        if c == "\\" and i + 1 < n:
            ctx += ["bare", "bare"]
            i += 2
            continue
        if c == "#" and (i == 0 or line[i - 1] in " \t;("):
            ctx += ["comment"] * (n - i)
            break
        if c in "'\"":
            if c == "'" and i > 0 and line[i - 1] == "$":
                pass  # $'...' is a quote like any other for our purpose
            is_code = bool(INTERPRETER.search(line[:i]))
            stack.append(("s" if c == "'" else "d", is_code))
            ctx.append(tag())
            i += 1
            continue
        if top == "(":
            if c == "(":
                stack[-1] = ("(", stack[-1][1] + 1)
            elif c == ")":
                depth = stack[-1][1] - 1
                if depth == 0:
                    stack.pop()
                    ctx.append(tag())
                    i += 1
                    continue
                stack[-1] = ("(", depth)
        h = HEREDOC.match(line, i)
        if h and heredoc is None and not line[:i].rstrip().endswith("$(("):
            heredoc = (h.group(3), h.group(1) == "-", bool(INTERPRETER.search(line[:i])), bool(h.group(2)))
            ctx += ["bare"] * (h.end() - i)
            i = h.end()
            continue
        ctx.append("bare")
        i += 1
    return ctx, stack, heredoc


def code_place(line, pos, outer=None):
    """Where `pos` sits in this line of interpreter code: 'string' (inside
    one of the code's string literals), 'comment' (after a `#` that starts
    a comment) or 'code'. Counts quotes from the line's start. `outer` is
    the shell quote the code sits in, which is not a quote of the code
    itself. Inside shell double quotes the shell reads its escapes first:
    `\\"` reaches the code as `"`, so `print(f\\"... k2 msg\\")` is a string."""
    chars = []
    i = 0
    while i < pos:
        c = line[i]
        if outer == '"' and c == "\\" and i + 1 < pos and line[i + 1] in '"\\$`':
            chars.append(line[i + 1])
            i += 2
            continue
        if c != outer:
            chars.append(c)
        i += 1
    quote = None
    prev = " "
    j = 0
    while j < len(chars):
        c = chars[j]
        if c == "\\":
            j += 2
            prev = "x"
            continue
        if quote is None and c == "#" and prev in " \t;":
            return "comment"
        if c in "'\"":
            if quote is None:
                quote = c
            elif quote == c:
                quote = None
        prev = c
        j += 1
    return "string" if quote else "code"


# A name on the left of `=` in interpreter code (`k2 = [...]`, not `==`):
# a variable, never words for a reader.
ASSIGNED = re.compile(r"\s*=(?!=)")


def whole_string_is_name(line, m):
    b = line[m.start() - 1] if m.start() > 0 else ""
    a = line[m.end()] if m.end() < len(line) else ""
    return b in "'\"" and a == b


def reads_as_command(line, m, known):
    """`k2` here names the command for a reader: it is followed by one of
    the CLI's verbs, a flag or a placeholder, or it stands in backticks, or
    the words around it say so."""
    after = line[m.end():]
    before = line[:m.start()]
    if COMMAND_TAIL.match(after):
        return True
    w = NEXT_WORD.match(after)
    if w and w.group(1) in known:
        return True
    if before.endswith("`") and after.lstrip("\\").startswith("`"):
        return True
    if re.search(r"(Usage:|usage:|run|Run|try|Try|use|Use|via|with|the|The|see|See|:)\s+[`'\"]?$", before) and re.match(r"[`'\"]?( CLI\b|[`'\".,;)]|$)", after):
        return True
    return False


def in_command_phrase(before, after):
    """The name sits in one of the COMMAND_PHRASES. Read for the CLI's
    printed text only (classify), not by --area rust or the brand gate's
    Rust reading: a Rust string with such a phrase is the B7 steps'."""
    return any(before.endswith(b) and after.startswith(a) for b, a, _ in COMMAND_PHRASES)


def is_stamp(before, after):
    """`[k2 ...]` or `[k2]`: a log or stamp prefix, a name that is not the
    CLI's. `before` and `after` are the text on each side of the word."""
    return before.endswith("[") and (after.startswith("]") or after.startswith(" "))


def classify(line, m, where, known):
    before = line[:m.start()]
    after = line[m.end():]
    if is_stamp(before, after):
        return "kept", "a [k2 ...] prefix or stamp"
    if where == "comment":
        return "comment", "comment"
    if where == "bare":
        return "kept", SHELL_WHY
    if before.rstrip().endswith("$(") and where != "heredoc-quoted":
        return "unsure", "$(k2 ...): runs the command"
    if whole_string_is_name(line, m):
        return "kept", VALUE_WHY
    if where.endswith("-code"):
        place = code_place(line, m.start(), outer=None if where == "heredoc-code" else ("'" if where[0] == "s" else '"'))
        if place == "comment":
            return "comment", "comment in interpreter code"
        if place == "code":
            # (A quote just before it can only be the shell's, where the
            # code starts: inside a code string it would not be code.)
            if ASSIGNED.match(after) and (not before or before[-1] in " \t;(,'\""):
                return "kept", NAME_WHY
            return "unsure", CODE_WHY
    if not reads_as_command(line, m, known) and not in_command_phrase(before, after):
        return "unsure", "k2 followed by a word that is not a command"
    return "text", "printed text" if not where.endswith("-code") else "string in interpreter code"


def classify_file(path, known, word=WORD):
    """Yield (line number, start, end, class, why) for each bare `k2`.
    `word` is the name to find; the brand gate passes its own, which also
    finds k5."""
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    if path.endswith((".entries", ".txt")):
        # Schema entries are JSON inside one printed, quoted heredoc (it
        # opens in schema-head.sh), and a cli/text page is printed as it
        # is: all text, and nothing in it runs.
        for n, line in enumerate(lines, 1):
            for m in word.finditer(line):
                yield n, m.start(), m.end(), *classify(line, m, "heredoc-quoted", known)
        return
    stack, heredoc = [], None
    rows = []
    for n, line in enumerate(lines, 1):
        if heredoc:
            term, strip, is_code, is_quoted = heredoc
            if (line.lstrip("\t") if strip else line) == term:
                heredoc = None
                continue
            for m in word.finditer(line):
                # A quoted heredoc (<<'EOF') expands nothing, so $(k2 ...)
                # in it is an example for the reader, not a command run.
                where = "heredoc-code" if is_code else ("heredoc-quoted" if is_quoted else "heredoc")
                if is_code and line.strip().startswith("#"):
                    where = "comment"
                rows.append((n, m.start(), m.end(), *classify(line, m, where, known)))
            continue
        ctx, stack, heredoc = contexts(line, stack)
        for m in word.finditer(line):
            where = ctx[m.start()] if m.start() < len(ctx) else "bare"
            rows.append((n, m.start(), m.end(), *classify(line, m, where, known)))
    # A part must end with every quote closed. If it does not, this reader
    # lost its place somewhere in it, and nothing it says about the part
    # can be trusted: every use there is left for a person. (schema-head.sh
    # ends inside the manifest's heredoc on purpose; the entries follow.)
    if stack or (heredoc and os.path.basename(path) != "schema-head.sh"):
        rows = [(n, s, e, cls if cls == "kept" else "unsure", why if cls == "kept" else "the reader lost its place in this part")
                for n, s, e, cls, why in rows]
    yield from rows


def rust_files(root):
    out = []
    for area in ("crates", os.path.join("src-tauri", "src")):
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, area)):
            dirnames[:] = sorted(d for d in dirnames if d not in ("target", "node_modules", ".git", "binaries"))
            for name in sorted(filenames):
                if name.endswith(".rs"):
                    out.append(os.path.join(dirpath, name))
    return out


RAW_OPEN = re.compile(r'b?r(#*)"')
CHAR_LIT = re.compile(r"'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\n])'")


def rust_contexts(text):
    """The context of each character of a Rust file: 'code', 'comment',
    'string'. Reads line and block comments (nested), strings with escapes,
    raw strings, and tells a char literal from a lifetime."""
    ctx = ["code"] * len(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            ctx[i:j] = ["comment"] * (j - i)
            i = j
        elif c == "/" and text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            ctx[i:j] = ["comment"] * (j - i)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            j = min(j + 1, n)
            ctx[i:j] = ["string"] * (j - i)
            i = j
        elif c in "rb" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")) and RAW_OPEN.match(text, i):
            m = RAW_OPEN.match(text, i)
            close = '"' + m.group(1)
            j = text.find(close, m.end())
            j = n if j < 0 else j + len(close)
            ctx[i:j] = ["string"] * (j - i)
            i = j
        elif c == "'":
            m = CHAR_LIT.match(text, i)
            i = m.end() if m else i + 1   # a char literal, or a lifetime's tick
        else:
            i += 1
    return ctx


def classify_rust_file(path, known):
    with open(path, encoding="utf-8", errors="replace") as f:
        text = f.read()
    if "k2" not in text:
        return
    ctx = rust_contexts(text)
    starts = [0]
    for m in re.finditer("\n", text):
        starts.append(m.end())
    line_no = 0
    for m in WORD.finditer(text):
        while line_no + 1 < len(starts) and starts[line_no + 1] <= m.start():
            line_no += 1
        ls = starts[line_no]
        le = text.find("\n", ls)
        le = len(text) if le < 0 else le
        line = text[ls:le]
        col = m.start() - ls
        lm = WORD.match(line, col)
        where = ctx[m.start()]
        before, after = line[:col], line[col + 2:]
        if is_stamp(before, after):
            cls = ("kept", "a [k2 ...] prefix or stamp")
        elif where == "comment":
            cls = ("comment", "comment")
        elif where == "code":
            cls = ("unsure", "k2 in Rust code, not in a string")
        elif whole_string_is_name(line, lm):
            cls = ("unsure", "a string that is exactly k2 (a value)")
        elif before.rstrip().endswith("$("):
            cls = ("unsure", "$(k2 ...): runs the command")
        elif not reads_as_command(line, lm, known):
            cls = ("unsure", "k2 followed by a word that is not a command")
        else:
            cls = ("text", "string literal")
        yield line_no + 1, col, col + 2, *cls


def collect_rust(root):
    rows = []
    known = verbs(root)
    for path in rust_files(root):
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        for n, s, e, cls, why in classify_rust_file(path, known):
            rows.append((rel, n, s, e, cls, why))
    return rows


def collect(root):
    rows = []
    known = verbs(root)
    for path in parts(root):
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        for n, s, e, cls, why in classify_file(path, known):
            rows.append((rel, n, s, e, cls, why))
    return rows


def apply(root, rows, to="k5"):
    by_file = {}
    for rel, n, s, e, cls, _ in rows:
        if cls in ("text", "comment"):
            by_file.setdefault(rel, []).append((n, s, e))
    for rel, edits in by_file.items():
        path = os.path.join(root, rel)
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
        for n, s, e in sorted(edits, reverse=True):
            line = lines[n - 1]
            assert line[s:e] == "k2", f"{rel}:{n}: expected k2 at {s}"
            lines[n - 1] = line[:s] + to + line[e:]
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
    return sum(len(v) for v in by_file.values()), len(by_file)


def main(argv):
    here = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    root = None
    mode = "count"
    show = None
    area = "cli"
    to = "k5"
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--unsure":
            mode, show = "show", "unsure"
        elif a == "--show" and args and args[0] in CLASSES:
            mode, show = "show", args.pop(0)
        elif a == "--apply":
            mode = "apply"
        elif a == "--root" and args:
            root = args.pop(0)
        elif a == "--to" and args and re.fullmatch(r"[a-z][a-z0-9]{1,11}", args[0]):
            to = args.pop(0)
        elif a == "--area" and args and args[0] in ("cli", "rust"):
            area = args.pop(0)
        else:
            sys.exit("cli-name-flip: usage: cli-name-flip.py [--area cli|rust] [--unsure | --show text|comment|kept|unsure] [--root DIR]\n"
                     "                      cli-name-flip.py [--area cli|rust] --apply --root DIR [--to NAME]")
    if mode == "apply":
        if root is None:
            sys.exit("cli-name-flip: --apply needs --root DIR (it never rewrites the tree it runs from by default)")
        if os.path.realpath(root) == os.path.realpath(here) and os.environ.get("CLI_NAME_FLIP_THIS_TREE") != "yes":
            sys.exit("cli-name-flip: --apply on this tree itself is refused; set CLI_NAME_FLIP_THIS_TREE=yes for the real flip")
    root = root or here
    rows = collect(root) if area == "cli" else collect_rust(root)
    place = ("cli/src and cli/text" if os.path.isdir(os.path.join(root, "cli", "text")) else "cli/src") if area == "cli" else "crates and src-tauri/src"
    if mode == "show":
        for rel, n, s, e, cls, why in rows:
            if cls == show:
                with open(os.path.join(root, rel), encoding="utf-8") as f:
                    text = f.read().split("\n")[n - 1].strip()
                print(f"{rel}:{n}: [{why}] {text[:170]}")
        return 0
    if mode == "apply":
        changed, files = apply(root, rows, to)
        print(f"cli-name-flip: rewrote {changed} uses of k2 to {to} in {files} files under {root}/{place}.")
        return 0
    totals = {c: 0 for c in CLASSES}
    for row in rows:
        totals[row[4]] += 1
    files = len({r[0] for r in rows})
    print(f"cli-name-flip (dry run): {len(rows)} uses of the word k2 in {files} files of {place}")
    print(f"  will change: text {totals['text']}, comment {totals['comment']}")
    print(f"  will not:    kept {totals['kept']}, unsure {totals['unsure']}")
    why = {}
    for r in rows:
        if r[4] == "unsure":
            why[r[5]] = why.get(r[5], 0) + 1
    for k in sorted(why, key=lambda k: -why[k]):
        print(f"    unsure: {why[k]:5d}  {k}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
