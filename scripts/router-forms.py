#!/usr/bin/env python3
"""Every form the shell router takes, found from its code (the native bar's
list of what to run).

    python3 scripts/router-forms.py            write crates/k5-cli/tests/data/router_forms.tsv
    python3 scripts/router-forms.py --check    change nothing; exit 1 if the file is stale
    python3 scripts/router-forms.py --summary  one line: the native bar's ceiling, per family

The census (scripts/spec-census.py) starts from the `--schema` entries,
so a verb with an arm but no entry is not in it. This list starts from
the router instead: each arm of its top-level `case`, then, through the
code that arm runs, each arm of a `case` (or each `[ "$x" = word ]`
test) on the next word, to four words. The next word is followed
through `shift`, a local that holds it (`local sub="${1:-}"`), a
positional array's first slot (`${pos[0]}`), and calls that pass the
words on (`cmd_x "$@"`, `cmd_x "$sub" "$@"`, `cmd_x "${@:2}"`). A call
in a `*)` arm, or outside every arm, hands the same word on, so the
callee's arms are more forms at the same depth (`app` -> `cmd_skin`).
`*`, `""`, globs and words that start with `-` are not forms.

Every word of an arm is a form: an alias row names its arm's first word
in `alias_of`, so `publish ls` is checked as well as `publish ps`. The
retired verbs (`_K2_RETIRED_VERBS`) are forms too: the router still
answers them, with a pointer.

What the parser can't see (a sub-verb read after a positional, a verb
the daemon names) is kept by hand in
crates/k5-cli/tests/data/router_forms_extra.tsv: one form per row, the
words to run it with, its anchor and the reason. A row there that the parser also finds fails --check and
the write, so the list can't go stale.

Columns of router_forms.tsv:

  form      the words, separated by one space
  argv      the words the native bar runs for it, when they are not the
            form's own (a hand-kept row whose verb follows a positional:
            `compute node servers n1 add`); else `-`
  alias_of  the form of the arm's first word, when this word is another
            word of the same arm (an alias, or a verb that shares the
            arm's code); else `-`
  anchor    cli/src/<file>:<function> that holds the arm (no function:
            the router's own case); a hand-kept row's own anchor
  entry     the `--schema` entry of these words, or `-`
  json      `y` when that entry takes --json (its `<form> --json` is a
            form too); else `-`
  source    parsed, retired, or extra (hand-kept)

--summary reads the native bar's ceiling
(crates/k5-cli/tests/data/native_bar_ceiling.tsv, written by
crates/k5-cli/tests/native_bar.rs) and prints how many forms still fall
through, per family, counted classes only.
"""

import importlib.util
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "cli", "src")
OUT = "crates/k5-cli/tests/data/router_forms.tsv"
EXTRA = "crates/k5-cli/tests/data/router_forms_extra.tsv"
CEILING = "crates/k5-cli/tests/data/native_bar_ceiling.tsv"
# The classes the ship bar counts: V a verb (or its --json
# twin), H a help form, F a documented flag shape, E an error or
# environment edge on a k5 code path. P (an edge that exists only
# because python3 is missing: the native-without-python3 difference) is
# listed but not counted.
COUNTED = ("V", "H", "F", "E")
DEPTH = 4
# Helpers that read the words only to look for a help flag or a hint:
# never a verb's dispatch.
NOT_DISPATCH = {"_wants_help", "_retired_verb_exit", "_zen_garden_changes_hint"}


def functions(order):
    """name -> (file, line of `name() {`, body lines). A function ends at
    a line that is `}` with a blank line, a comment, another function or
    the end of the file after it: a `}` alone inside a quoted Python
    block (a dict's close) is not its end."""
    funcs = {}
    for name in order:
        if not name.endswith(".sh"):
            continue
        with open(os.path.join(SRC, name), encoding="utf-8") as f:
            lines = f.read().split("\n")
        i = 0
        while i < len(lines):
            m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\(\) \{\s*$", lines[i])
            if m:
                j = i + 1
                while j < len(lines):
                    nxt = lines[j + 1] if j + 1 < len(lines) else ""
                    if lines[j] == "}" and (not nxt.strip() or nxt.startswith("#") or re.match(r"^[A-Za-z_][A-Za-z0-9_]*\(\) \{", nxt)):
                        break
                    j += 1
                funcs[m.group(1)] = (name, i + 1, lines[i + 1 : j])
                i = j
            i += 1
    return funcs


def census_module():
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("spec_census", os.path.join(ROOT, "scripts", "spec-census.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


SHIFT = re.compile(r"(?:^\s*|[;&|{]\s*|\bthen\s+|\bdo\s+|\belse\s+)shift(?:\s+(\d+)(?![\d>]))?\b")
CALL = re.compile(r"(?:^\s*|[;&|({]\s*|\bthen\s+|\bdo\s+|\belse\s+|\)\s+)((?:cmd|_)[A-Za-z0-9_]*)((?:\s+[^\s;|&]+)*?)\s+\"\$(?:@|\{@(?::(\d+))?\})\"")
CASE = re.compile(r"""^\s*case\s+"?(\$\{?[A-Za-z0-9_]+(?:\[\d\])?(?::-[^}"]*)?\}?)"?\s+in\b(.*)$""")


def code_of(line):
    """The line without a trailing comment (a `#` after a space)."""
    if line.lstrip().startswith("#"):
        return ""
    return re.split(r"\s#", line, maxsplit=1)[0]


def indent(line):
    return len(line) - len(line.lstrip())


def shifts(line):
    """The words a line shifts for sure: not a shift behind a test
    (`if ...; then shift; fi`, `[ ... ] && shift`)."""
    c = code_of(line)
    # A shift when there is a word: on a form's path there always is.
    c = re.sub(r"^\s*\[ \$# -(?:gt 0|ge 1) \] && (?=shift\b)", "", c)
    if re.match(r"^\s*(?:if|elif|while|until|\[\[?)\s", c):
        return 0
    return sum(int(m.group(1) or 1) for m in SHIFT.finditer(c) if not re.search(r"&&|\|\|", c[: m.start()]))


def subject_pos(subject, s, vars_):
    """The word a case subject or test operand holds, as a position in the
    words this code was given (1 is the first), or None."""
    m = re.match(r"^\$\{?(\d+)(?::-[^}]*)?\}?$", subject)
    if m:
        return int(m.group(1)) + s
    m = re.match(r"^\$\{?([A-Za-z_][A-Za-z0-9_]*)(?:\[(\d)\])?(?::-[^}]*)?\}?$", subject)
    if m:
        if m.group(2) is not None:
            # A slot of an array the positional words were gathered in.
            first = vars_.get(m.group(1) + "[0]")
            return None if first is None else first + int(m.group(2))
        return vars_.get(m.group(1))
    return None


def arms_of_case(lines, k):
    """The arms of the `case` at lines[k]: (patterns, first line, last line),
    each pattern list as written."""
    out = []
    depth = 0
    cur = None
    head = code_of(lines[k])
    inline = CASE.match(head).group(2).strip()
    if inline:
        # case "$x" in a) ...;; b) ...;; esac   on one line.
        for pm in re.finditer(r"(?:^|;;)\s*\(?([^\s()|;]+(?:\s*\|\s*[^\s()|;]+)*)\)", inline):
            out.append(([p.strip() for p in pm.group(1).split("|")], k, k))
        return out
    i = k + 1
    while i < len(lines):
        s = code_of(lines[i])
        if depth == 0 and re.match(r"^\s*esac\b", s):
            break
        if depth == 0 and (cur is None or cur[3]):
            pm = re.match(r"^\s*\(?((?:[^\s()|]+)(?:\s*\|\s*[^\s()|]+)*)\)(.*)$", s)
            if pm:
                if cur is not None:
                    out.append((cur[0], cur[1], i - 1))
                rest = pm.group(2)
                cur = [[p.strip() for p in pm.group(1).split("|")], i, None, rest.rstrip().endswith(";;")]
                depth += len(re.findall(r"(?:^|[\s;(])case\b.*\bin\b", rest))
                depth -= len(re.findall(r"(?:^|[\s;])esac\b", rest))
                depth = max(depth, 0)
                if depth > 0:
                    cur[3] = False
                i += 1
                continue
        depth += len(re.findall(r"(?:^|[\s;(])case\b.*\bin\b", s))
        depth -= len(re.findall(r"(?:^|[\s;])esac\b", s))
        depth = max(depth, 0)
        if cur is not None and depth == 0 and s.rstrip().endswith(";;"):
            cur[3] = True
        i += 1
    if cur is not None:
        out.append((cur[0], cur[1], i - 1))
    return out


def word_of(p):
    w = p.strip()
    if len(w) >= 2 and w[0] == w[-1] and w[0] in "\"'":
        w = w[1:-1]
    return w


def is_form_word(w):
    return bool(w) and not w.startswith("-") and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:@-]*", w) is not None


class Walker:
    """Reads the forms from the shell's code.

    Positions are of the words a piece of code was given ($1 is 1). The
    walk knows the words before the one it looks for (`known`: position
    -> the arm's words), so a `case` on an earlier word takes only the
    arm for that word (or its `*)` arm), and a case on the next word that
    sits in another word's arm is not this word's."""

    def __init__(self, funcs):
        self.funcs = funcs

    def scan(self, lines, target, known, vars0=None):
        """The arms of `lines` on the word at `target`, and the calls that
        hand that word on.

        Returns (arms, passes): arms is a list of (words, body lines, the
        shifts before it); passes is a list of (function, the word's
        position in its words, what it knows there)."""
        s = 0
        vars_ = dict(vars0 or {})
        arms, passes = [], []
        k = 0
        loop, each, flags = None, None, None
        while k < len(lines):
            line = lines[k]
            c = code_of(line)
            for m in re.finditer(r"\b([A-Za-z_][A-Za-z0-9_]*)=\"?\$\{?(\d+)(?::-[^}\"]*)?\}?\"?(?=\s|;|$)", c):
                vars_[m.group(1)] = int(m.group(2)) + s
            for m in re.finditer(r"\b([A-Za-z_][A-Za-z0-9_]*)=\"?\$\{([A-Za-z_][A-Za-z0-9_]*)\[(\d)\](?::-[^}\"]*)?\}\"?(?=\s|;|$)", c):
                slot = vars_.get(m.group(2) + "[0]")
                if slot is not None:
                    vars_[m.group(1)] = slot + int(m.group(3))
            if re.match(r"^\s*while \[\[? \$# -g[te] [01] \]", c):
                loop, each, flags = s, "1", indent(line)
            elif flags is not None and indent(line) == flags and re.match(r"^\s*done\b", c):
                flags = None
            m = re.match(r"^\s*for ([A-Za-z_][A-Za-z0-9_]*) in \"\$@\"", c)
            if m:
                loop, each = s, m.group(1)
            for m in re.finditer(r"\b([A-Za-z_][A-Za-z0-9_]*)\+=\(\s*\"\$\{?([A-Za-z_0-9]+)\}?\"", c):
                # A loop over the words gathers the positional ones in an
                # array: its first slot is the word the loop started on.
                if loop is not None and m.group(2) == each:
                    vars_.setdefault(m.group(1) + "[0]", loop + 1)
            m = CASE.match(c)
            if m:
                pos = subject_pos(m.group(1), s, vars_)
                found = arms_of_case(lines, k)
                last = max([b for _, _, b in found] + [k])
                if pos == target:
                    for pats, first, end in found:
                        arms.append((pats, self.body(lines, first, end), s, dict(vars_)))
                    if flags is None or not re.match(r"^\$\{?1\b", m.group(1)):
                        k = self.esac(lines, k, last)
                        continue
                    # A case on $1 in a `while [ $# -gt 0 ]` loop: its arms
                    # are words, and its lines gather the positional ones.
                    s += shifts(line)
                    k += 1
                    continue
                if isinstance(pos, int) and pos in known:
                    arm = self.arm_for(found, known[pos])
                    if arm is not None:
                        a, p = self.scan(self.body(lines, arm[1], arm[2]), target - s, shifted(known, s), shifted(vars_, s))
                        arms.extend((w, b, s2 + s, {n: q + s for n, q in v.items()}) for w, b, s2, v in a)
                        passes.extend(p)
                    k = self.esac(lines, k, last)
                    continue
            t = re.search(r"\[\[?\s+\"?(\$\{?[A-Za-z0-9_]+(?:\[\d\])?(?::-[^}\"]*)?\}?)\"?\s+==?\s+\"?([A-Za-z0-9][A-Za-z0-9_.:@-]*)\"?\s+\]", c)
            if t and re.match(r"^\s*(?:el)?if\b", c) and not re.search(r"\bfi\b", c):
                pos = subject_pos(t.group(1), s, vars_)
                end = self.if_end(lines, k)
                if pos == target:
                    words = [w for w in re.findall(r"\"?\$\{?[A-Za-z0-9_]+(?:\[0\])?(?::-[^}\"]*)?\}?\"?\s+==?\s+\"?([A-Za-z0-9][A-Za-z0-9_.:@-]*)\"?", c)]
                    arms.append((words, lines[k + 1 : end + 1], s, dict(vars_)))
                    k = end + 1
                    continue
                if isinstance(pos, int) and pos in known:
                    if t.group(2) in known[pos]:
                        a, p = self.scan(lines[k + 1 : end + 1], target - s, shifted(known, s), shifted(vars_, s))
                        arms.extend((w, b, s2 + s, {n: q + s for n, q in v.items()}) for w, b, s2, v in a)
                        passes.extend(p)
                    k = end + 1
                    continue
            if t and subject_pos(t.group(1), s, vars_) == target:
                # A one-line test (`[ "${pos[0]:-}" = set ] || usage`, or
                # `if [ "${1:-}" = rotate ]; then ...; fi`): the word it
                # wants is a form.
                arms.append(([t.group(2)], [re.split(r"\b(?:else|elif|fi)\b", c[t.end():])[0]], s, dict(vars_)))
            passes.extend(self.calls(line, target, s, known, vars_))
            m = re.match(r"^\s*set -- \"\$\{([A-Za-z_][A-Za-z0-9_]*)\[@\]\}\"\s*$", c)
            if m and m.group(1) + "[0]" in vars_:
                # The words again, from an array that gathered them.
                s = vars_[m.group(1) + "[0]"] - 1
            else:
                s += shifts(line)
            k += 1
        return arms, passes

    @staticmethod
    def body(lines, first, end):
        head = lines[first]
        rest = head.split(")", 1)[1] if ")" in head else ""
        return [rest] + lines[first + 1 : end + 1]

    @staticmethod
    def esac(lines, k, last):
        """The line after the case that starts at k (its arms end at `last`)."""
        if re.search(r"\besac\b", code_of(lines[k])):
            return k + 1
        for i in range(last + 1, len(lines)):
            if re.match(r"^\s*esac\b", code_of(lines[i])):
                return i + 1
        return last + 1

    @staticmethod
    def arm_for(found, words):
        for pats, first, end in found:
            if any(word_of(p) in words for p in pats):
                return (pats, first, end)
        for pats, first, end in found:
            if any(word_of(p) == "*" for p in pats):
                return (pats, first, end)
        return None

    def if_end(self, lines, k):
        """The last line of the `if` that starts at k: the line before its
        `elif`, `else` or `fi` at the same indent."""
        indent = len(lines[k]) - len(lines[k].lstrip())
        for i in range(k + 1, len(lines)):
            s = lines[i]
            if len(s) - len(s.lstrip()) == indent and re.match(r"^\s*(?:elif|else|fi)\b", s):
                return i - 1 if not re.match(r"^\s*fi\b", s) else i
        return len(lines) - 1

    def calls(self, line, target, s, known, vars_):
        """Calls on `line` that pass "$@": (function, the position the word
        at `target` has in the callee's words, what the callee knows), when
        the word is still there."""
        out = []
        c = code_of(line)
        for m in CALL.finditer(c):
            fn = m.group(1)
            if fn not in self.funcs or fn in NOT_DISPATCH:
                continue
            lead = [a for a in m.group(2).split() if a]
            if any(a.startswith(">") or a.startswith("<") or "=" in a for a in lead):
                continue
            gone = s + shifts(c[: m.start(1)]) + int(m.group(3) or 1) - 1
            at = target - gone
            if at < 1:
                continue
            there = {len(lead) + p - gone: w for p, w in known.items() if p - gone >= 1}
            for i, a in enumerate(lead, 1):
                # A word handed on by name: a literal, or a local that holds
                # a word the walk knows.
                lit = word_of(a)
                if is_form_word(lit):
                    there[i] = {lit}
                    continue
                q = subject_pos(lit, s, vars_)
                if isinstance(q, int) and q in known:
                    there[i] = known[q]
            out.append((fn, len(lead) + at, there))
        return out

    def forms(self, lines, target, known, depth, seen, where, vars0=None):
        """Forms reached from `lines`, where the next word is at `target`:
        a list of (words, where, children)."""
        if depth > DEPTH:
            return []
        arms, passes = self.scan(lines, target, known, vars0)
        out = []
        for pats, body, s, vs in arms:
            words = [w for w in (word_of(p) for p in pats) if is_form_word(w) or (depth == 1 and re.fullmatch(r"--?[a-z][a-z-]*", w))]
            here = shifted(known, s)
            hv = shifted(vs, s)
            at = target - s
            if words:
                # Each word on its own: an arm can hold verbs that its code
                # tells apart later (`pause|drain|resume)`), not only aliases.
                kids = []
                for w in words:
                    mine = dict(here)
                    mine[at] = {w}
                    kids.append(self.forms(body, at + 1, mine, depth + 1, seen, where, hv) if depth < DEPTH else [])
                out.append((words, where, kids))
            elif at >= 1:
                # `*)` and other arms with no form word: the same word,
                # read further on (often by the function it calls).
                out.extend(self.forms(body, at, here, depth, seen, where, hv))
        for fn, pos, there in passes:
            out.extend(self.follow(fn, pos, there, depth, seen))
        return out

    def follow(self, fn, pos, known, depth, seen):
        key = (fn, pos, tuple(sorted((p, tuple(sorted(w))) for p, w in known.items())))
        if key in seen:
            return []
        f = self.funcs[fn]
        return self.forms(f[2], pos, known, depth, seen | {key}, "cli/src/%s:%s" % (f[0], fn))


def shifted(known, s):
    """Positions (of known words, or of the words locals hold) after `s`
    shifts. A word shifted away keeps a position below 1: a local can
    still hold it."""
    return {k: p - s for k, p in known.items()} if known and isinstance(next(iter(known)), str) else {
        p - s: w for p, w in known.items()
    }


def shape(tree):
    """The words of a subtree, to tell an alias's subtree from its arm's."""
    return sorted((tuple(words), [shape(k) for k in kids]) for words, _, kids in tree)


def flatten(tree, prefix, rows, seen):
    """rows: (form, alias_of, anchor) in walk order, each form once. A word
    of an arm after its first gets its own subtree only where it differs
    from the first word's."""
    for words, where, kids in tree:
        canon = (prefix + " " + words[0]).strip()
        for i, w in enumerate(words):
            form = (prefix + " " + w).strip()
            if form in seen:
                continue
            seen.add(form)
            rows.append((form, "-" if i == 0 else canon, where))
        for i, w in enumerate(words):
            if i == 0 or shape(kids[i]) != shape(kids[0]):
                flatten(kids[i], (prefix + " " + w).strip(), rows, seen)


def retired(path):
    with open(path, encoding="utf-8") as f:
        text = f.read()
    block = text[text.index("_K2_RETIRED_VERBS=(") : text.index("\n)\n", text.index("_K2_RETIRED_VERBS=("))]
    return [m.group(1) for m in re.finditer(r'^\s+"([^|"]+)\|', block, re.M)]


def read_extra(root):
    path = os.path.join(root, EXTRA)
    rows = []
    with open(path, encoding="utf-8") as f:
        lines = [(n, l.rstrip("\n")) for n, l in enumerate(f, 1) if l.strip() and not l.startswith("#")]
    if not lines or lines[0][1].split("\t") != ["form", "argv", "anchor", "reason"]:
        raise SystemExit("router-forms: %s: the first row is not form<TAB>argv<TAB>anchor<TAB>reason" % EXTRA)
    for n, l in lines[1:]:
        cols = l.split("\t")
        if len(cols) != 4 or not all(c.strip() for c in cols):
            raise SystemExit("router-forms: %s:%d: want form, argv, anchor and a reason" % (EXTRA, n))
        rows.append((n, cols[0], cols[1], cols[2]))
    return rows


def build(root):
    census = census_module()
    order = census.parts()
    funcs = functions(order)
    ents = census.entries(order)
    entry_json = {e["name"]: any(f.get("name") == "--json" for f in e.get("flags", [])) for e in ents}
    with open(os.path.join(SRC, "router.sh"), encoding="utf-8") as f:
        lines = f.read().split("\n")
    start = next(i for i, l in enumerate(lines) if l.startswith('case "${1:-help}" in'))
    end = next(i for i in range(start + 1, len(lines)) if lines[i] == "esac")
    router = lines[start : end + 1]
    w = Walker(funcs)
    tree = w.forms(router, 1, {}, 1, frozenset(), "cli/src/router.sh:")
    rows, seen = [], set()
    flatten(tree, "", rows, seen)
    parsed = {r[0] for r in rows}
    out = [(form, "-", alias, where, "parsed") for form, alias, where in rows]
    for form in retired(os.path.join(SRC, "deprecation.sh")):
        if form not in parsed and form not in seen:
            seen.add(form)
            out.append((form, "-", "-", "cli/src/deprecation.sh:_K2_RETIRED_VERBS", "retired"))
    bad = []
    for n, form, argv, anchor in read_extra(root):
        if form in parsed:
            bad.append("%s:%d: %r is parsed now: take its row out" % (EXTRA, n, form))
            continue
        if form in seen:
            bad.append("%s:%d: %r is listed twice" % (EXTRA, n, form))
            continue
        seen.add(form)
        out.append((form, argv, "-", anchor, "extra"))
    table = []
    for form, argv, alias, where, source in out:
        entry = form if form in entry_json else "-"
        table.append([form, argv, alias, where, entry, "y" if entry != "-" and entry_json[entry] else "-", source])
    return table, bad


def text_of(table):
    head = ["form", "argv", "alias_of", "anchor", "entry", "json", "source"]
    counts = {}
    for r in table:
        counts[r[6]] = counts.get(r[6], 0) + 1
    aliases = sum(1 for r in table if r[2] != "-")
    lines = [
        "# GENERATED by scripts/router-forms.py from cli/src and %s. Do not edit: run it again." % EXTRA,
        "# %d forms (%d of them aliases): %s; %d with a --schema entry"
        % (len(table), aliases, ", ".join("%s %d" % (k, counts[k]) for k in sorted(counts)), sum(1 for r in table if r[4] != "-")),
        "\t".join(head),
    ] + ["\t".join(r) for r in table]
    return "\n".join(lines) + "\n"


def summary(root):
    path = os.path.join(root, CEILING)
    if not os.path.isfile(path):
        print("router-forms: no ceiling at %s" % CEILING, file=sys.stderr)
        return 1
    by, classes, left = {}, {}, 0
    with open(path, encoding="utf-8") as f:
        rows = [l.rstrip("\n").split("\t") for l in f if l.strip() and not l.startswith("#")]
    for r in rows[1:]:
        cls, fam = r[0], r[1]
        classes[cls] = classes.get(cls, 0) + 1
        if cls in COUNTED:
            by[fam] = by.get(fam, 0) + 1
            left += 1
    fams = sorted(by, key=lambda k: (-by[k], k))
    print(
        "native bar: %d forms fall through (%s; P %d not counted): %s"
        % (left, " ".join("%s %d" % (c, classes.get(c, 0)) for c in COUNTED), classes.get("P", 0),
           ", ".join("%s %d" % (f, by[f]) for f in fams) or "none")
    )
    return 0


def main(argv):
    if argv == ["--summary"]:
        return summary(ROOT)
    check = argv == ["--check"]
    if argv not in ([], ["--check"]):
        print("router-forms: unknown arguments: %s" % " ".join(argv), file=sys.stderr)
        return 2
    table, bad = build(ROOT)
    for b in bad:
        print("router-forms: " + b, file=sys.stderr)
    if bad:
        return 1
    text = text_of(table)
    path = os.path.join(ROOT, OUT)
    current = open(path, encoding="utf-8").read() if os.path.isfile(path) else None
    note = text.split("\n")[1][2:]
    if check:
        if current == text:
            print("router-forms: %s is current (%s)" % (OUT, note))
            return 0
        print("router-forms: %s is stale: run python3 scripts/router-forms.py and commit it" % OUT, file=sys.stderr)
        return 1
    if current != text:
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        print("router-forms: wrote %s (%s)" % (OUT, note))
    else:
        print("router-forms: %s is already current" % OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
