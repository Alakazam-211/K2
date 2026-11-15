#!/usr/bin/env python3
"""Read the route match of the daemon's dispatcher, arm by arm.

    python3 scripts/garden/dispatcher-arms.py heads [FILE]
        every arm's head (its pattern and guard), in order, one per line
    python3 scripts/garden/dispatcher-arms.py list [FILE]
        the same with each arm's first line and how many lines it has
    python3 scripts/garden/dispatcher-arms.py body N [FILE]
        the body of arm N (counted from 1, as `list` numbers them)
    python3 scripts/garden/dispatcher-arms.py same-heads OLD NEW
        exit 0 when both files have the same heads in the same order
    python3 scripts/garden/dispatcher-arms.py same-body OLD N NEW FN
        exit 0 when the body of function FN in NEW is the body of arm N
        in OLD, moved

FILE defaults to crates/k2-daemon/src/routes/dispatcher.rs. The match is
the first `match path.as_str() {` in it.

This is the proof tool for moving an arm's body out of the dispatcher
into a function of its own while the arm, its pattern, its guard and its
place in the match stay as they are. Two things have to hold for such a
move, and neither is checked by eye:

  same-heads  no arm was added, dropped, reordered or re-guarded. The
              first arm that matches wins, so order is behaviour.
  same-body   the moved body is the old one. Allowed differences, and no
              others: the indentation; `return DispatchOutcome::X;`
              written as `return Some(DispatchOutcome::X);`; a first
              statement that takes the old local names out of the
              context (`let Cx { .. } = cx;`, one statement, any number
              of lines); and a final `None` where the old body fell off
              its end.

It reads the text only. Strings, raw strings, character literals and
comments are skipped when counting brackets, so a brace inside a string
does not end an arm.
"""
import re
import sys

DEFAULT = "crates/k2-daemon/src/routes/dispatcher.rs"
MATCH = "match path.as_str() {"
RAW = re.compile(r'b?r(#*)"')


def lex(src):
    """Two bytearrays the size of src: `code` is 1 where the character is
    code (0 inside a comment, a string, a raw string or a character
    literal), `comment` is 1 inside a comment."""
    mask = bytearray(b"\x01") * len(src)
    comment = bytearray(len(src))
    i, n = 0, len(src)

    def blank(a, b, is_comment=False):
        for k in range(a, min(b, n)):
            if src[k] != "\n":
                mask[k] = 0
                if is_comment:
                    comment[k] = 1

    while i < n:
        c = src[i]
        if c == "/" and src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j, True)
            i = j
        elif c == "/" and src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j, True)
            i = j
        elif c in "rb" and RAW.match(src, i) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            m = RAW.match(src, i)
            close = '"' + m.group(1)
            j = src.find(close, m.end())
            j = n if j < 0 else j + len(close)
            blank(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            blank(i, j + 1)
            i = j + 1
        elif c == "'":
            # A character literal ('a', '\n', '\u{1F600}') or a lifetime ('a).
            if i + 1 < n and src[i + 1] == "\\":
                j = src.find("'", i + 2)
                # '\'' : the quote right after the backslash is escaped.
                if j == i + 2:
                    j = src.find("'", i + 3)
                j = n if j < 0 else j + 1
                blank(i, j)
                i = j
            elif i + 2 < n and src[i + 2] == "'":
                blank(i, i + 3)
                i += 3
            else:
                i += 1
        else:
            i += 1
    return mask, comment


def code_mask(src):
    return lex(src)[0]


def find_match(src, mask):
    """(index of the match's `{`, index of its closing `}`)."""
    at = -1
    while True:
        at = src.find(MATCH, at + 1)
        if at < 0:
            raise SystemExit(f"dispatcher-arms: no `{MATCH}` in the file")
        if mask[at]:
            break
    open_at = at + len(MATCH) - 1
    depth = 0
    for k in range(open_at, len(src)):
        if not mask[k]:
            continue
        if src[k] == "{":
            depth += 1
        elif src[k] == "}":
            depth -= 1
            if depth == 0:
                return open_at, k
    raise SystemExit("dispatcher-arms: the match never closes")


def arms(src):
    """A list of arms: dict(head, head_line, body, body_start, body_end, braced).

    head is the text from the pattern's first character to just before
    `=>`, with runs of whitespace made one space. body is the source text
    of the arm's body: between the braces for `=> { ... }`, or the
    expression for `=> expr,`.
    """
    mask, comment = lex(src)
    open_at, close_at = find_match(src, mask)
    out = []
    i = open_at + 1
    while True:
        # The next arm starts at the next code character that is not space.
        while i < close_at and (comment[i] or src[i].isspace()):
            i += 1
        if i >= close_at:
            break
        head_start = i
        depth = 0
        arrow = -1
        k = i
        while k < close_at:
            if mask[k]:
                ch = src[k]
                if ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                elif depth == 0 and src.startswith("=>", k):
                    arrow = k
                    break
            k += 1
        if arrow < 0:
            line = src.count("\n", 0, head_start) + 1
            raise SystemExit(f"dispatcher-arms: line {line}: an arm with no `=>`")
        # A path in quotes is the pattern; a comment between two patterns
        # of a multi-line head is not part of it.
        head = " ".join("".join(" " if comment[x] else src[x] for x in range(head_start, arrow)).split())
        j = arrow + 2
        while j < close_at and (comment[j] or src[j].isspace()):
            j += 1
        if mask[j] and src[j] == "{":
            depth, e = 0, j
            while e <= close_at:
                if mask[e]:
                    if src[e] == "{":
                        depth += 1
                    elif src[e] == "}":
                        depth -= 1
                        if depth == 0:
                            break
                e += 1
            body_start, body_end, braced = j + 1, e, True
            i = e + 1
            # An optional comma after the block.
            t = i
            while t < close_at and (comment[t] or src[t].isspace()):
                t += 1
            if t < close_at and src[t] == ",":
                i = t + 1
        else:
            depth, e = 0, j
            while e < close_at:
                if mask[e]:
                    ch = src[e]
                    if ch in "([{":
                        depth += 1
                    elif ch in ")]}":
                        depth -= 1
                    elif ch == "," and depth == 0:
                        break
                e += 1
            body_start, body_end, braced = j, e, False
            i = e + 1
        out.append(
            {
                "head": head,
                "head_line": src.count("\n", 0, head_start) + 1,
                "body": src[body_start:body_end],
                "body_start": body_start,
                "body_end": body_end,
                "end_line": src.count("\n", 0, body_end) + 1,
                "braced": braced,
            }
        )
    return out


def dedent(text):
    lines = text.split("\n")
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    indents = [len(l) - len(l.lstrip(" ")) for l in lines if l.strip()]
    cut = min(indents) if indents else 0
    return [l[cut:] if l.strip() else "" for l in lines]


def function_body(src, name):
    mask = code_mask(src)
    pat = re.compile(r"\bfn\s+" + re.escape(name) + r"\b")
    found = [m for m in pat.finditer(src) if mask[m.start()]]
    if len(found) != 1:
        raise SystemExit(f"dispatcher-arms: `fn {name}` is defined {len(found)} times in the new file, wanted 1")
    k = found[0].end()
    depth = 0
    # The body's `{` is the first one outside the parameter list and the
    # return type's generics.
    paren = 0
    while k < len(src):
        if mask[k]:
            if src[k] in "(<":
                paren += 1
            elif src[k] in ")>" and not src.startswith("->", k - 1):
                paren -= 1
            elif src[k] == "{" and paren == 0:
                break
        k += 1
    start = k
    while k < len(src):
        if mask[k]:
            if src[k] == "{":
                depth += 1
            elif src[k] == "}":
                depth -= 1
                if depth == 0:
                    return src[start + 1:k]
        k += 1
    raise SystemExit(f"dispatcher-arms: `fn {name}` never closes")


def undo_move(lines):
    """Take the allowed differences out of a moved body (see the module doc)."""
    text = "\n".join(lines)
    mask = code_mask(text)
    # 1. The first statement, when it takes names out of the context.
    first = re.match(r"let\s+Cx\s*\{", text)
    if first:
        depth, k = 0, first.end() - 1
        while k < len(text):
            if mask[k]:
                if text[k] == "{":
                    depth += 1
                elif text[k] == "}":
                    depth -= 1
                    if depth == 0:
                        break
            k += 1
        tail = re.match(r"\s*=\s*cx\s*;[ \t]*\n?", text[k + 1:])
        if not tail:
            raise SystemExit("dispatcher-arms: the first statement starts `let Cx {` but does not end `} = cx;`")
        text = text[k + 1 + tail.end():]
    lines = dedent(text)
    # 2. A final `None`.
    if lines and lines[-1].strip() == "None":
        lines = lines[:-1]
        while lines and not lines[-1].strip():
            lines.pop()
    # 3. The return rewrite, in code only.
    text = "\n".join(lines)
    mask = code_mask(text)
    out, k = [], 0
    pat = re.compile(r"return Some\((DispatchOutcome::[A-Za-z]+)\);")
    for m in pat.finditer(text):
        if not mask[m.start()]:
            continue
        out.append(text[k:m.start()])
        out.append(f"return {m.group(1)};")
        k = m.end()
    out.append(text[k:])
    return "".join(out).split("\n")


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def main(argv):
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0
    cmd, rest = argv[0], argv[1:]
    if cmd in ("heads", "list"):
        found = arms(read(rest[0] if rest else DEFAULT))
        for n, a in enumerate(found, 1):
            if cmd == "heads":
                print(a["head"])
            else:
                print(f"{n}\t{a['head_line']}\t{a['end_line'] - a['head_line'] + 1}\t{a['head']}")
        return 0
    if cmd == "body":
        if not rest:
            raise SystemExit("usage: dispatcher-arms.py body N [FILE]")
        found = arms(read(rest[1] if len(rest) > 1 else DEFAULT))
        n = int(rest[0])
        if not 1 <= n <= len(found):
            raise SystemExit(f"dispatcher-arms: no arm {n}: the match has {len(found)}")
        print("\n".join(dedent(found[n - 1]["body"])))
        return 0
    if cmd == "same-heads":
        if len(rest) != 2:
            raise SystemExit("usage: dispatcher-arms.py same-heads OLD NEW")
        old = [a["head"] for a in arms(read(rest[0]))]
        new = [a["head"] for a in arms(read(rest[1]))]
        if old == new:
            print(f"dispatcher-arms: same heads ({len(old)} arms, same order).")
            return 0
        for n, (o, w) in enumerate(zip(old, new), 1):
            if o != w:
                print(f"dispatcher-arms: FAIL arm {n} differs.\n  old: {o}\n  new: {w}")
                return 1
        print(f"dispatcher-arms: FAIL the old match has {len(old)} arms, the new one {len(new)}.")
        return 1
    if cmd == "same-body":
        if len(rest) != 4:
            raise SystemExit("usage: dispatcher-arms.py same-body OLD N NEW FN")
        found = arms(read(rest[0]))
        n = int(rest[1])
        if not 1 <= n <= len(found):
            raise SystemExit(f"dispatcher-arms: no arm {n}: the match has {len(found)}")
        if not found[n - 1]["braced"]:
            raise SystemExit(f"dispatcher-arms: arm {n} has no block body to move")
        old = dedent(found[n - 1]["body"])
        new = undo_move(dedent(function_body(read(rest[2]), rest[3])))
        if old == new:
            print(f"dispatcher-arms: same body (arm {n}, {len(old)} lines, fn {rest[3]}).")
            return 0
        for k in range(max(len(old), len(new))):
            o = old[k] if k < len(old) else "<the old body ends>"
            w = new[k] if k < len(new) else "<the new body ends>"
            if o != w:
                print(f"dispatcher-arms: FAIL arm {n} and fn {rest[3]} differ at body line {k + 1}.\n  old: {o}\n  new: {w}")
                return 1
    raise SystemExit(f"dispatcher-arms: unknown command {cmd}")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
