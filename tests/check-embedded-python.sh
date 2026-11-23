#!/usr/bin/env bash
# Guardrail: compile-check every Python block embedded in our shell scripts.
#
# WHY THIS EXISTS
# Our bash CLI (cli/k2, about 1,400 blocks) and the test and script files
# embed Python as `python3 -c '...'`, `python3 - <<'PY'` heredocs, and
# `python3 -c "$VAR"` with VAR='...' set in the same file. A broken block
# is invisible to every other suite (the Python lives in strings inside a
# bash file) and only fails when someone runs that one verb. The classic
# trap: a backslash inside an f-string field, `f"{d[\"k\"]}"`, is a hard
# SyntaxError on Python 3.12+ in a single-quoted block (bash passes `\"`
# through) and fine in a double-quoted one (bash turns `\"` into `"`).
#
# HOW
# A small bash lexer (quotes, escapes, $-expansions, command substitution,
# heredocs, comments, case patterns) finds each simple command whose word
# is python3 and works out the exact text Python receives, as bash would.
# It also lexes scripts run by `sh -c '...'` / `bash -s <<EOF` and heredocs
# that write a sh/bash script (first line a shebang). A $-expansion has no
# static value: it becomes the identifier XVAR, so the rest still compiles.
# Each block is compile()d under the running interpreter.
#
# A helper function that runs one of its arguments as Python (`python3 -c
# "$1"`, or `PYSRC="$1" python3 -c '... exec(os.environ["PYSRC"])'`, or
# a call of another such helper) counts as python3 too: each call's code
# argument is checked, and a `NAME='...'` the call passes as "$NAME".
# A quote inside a program that closes its shell string early leaves a
# cut program (it fails to compile) or, for a `NAME='...'` assignment
# over several lines, a command after it (`NAME='...'mail ...` runs mail
# when the file loads): both fail here, with the file and line.
#
# Skipped, counted and printed, by one rule: the code is only a bash
# expansion with no `NAME='...'` assignment in the same file (a function
# argument like "$1", or a $(...) substitution). Not looked at: text that
# is not run as a command (comments, help text, heredocs that are data).
#
# Usage:  bash tests/check-embedded-python.sh
# Exit:   0 = every block compiles, 1 = at least one broken block,
#         2 = setup error (no python3 3.12+, or a file the lexer cannot read)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 not found, cannot run the embedded-Python check." >&2
    exit 2
fi
# The f-string trap only shows on 3.12+.
if ! python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 12) else 1)'; then
    echo "ERROR: $(python3 --version 2>&1) is older than 3.12; this check needs 3.12+." >&2
    exit 2
fi

echo "Embedded-Python compile check (interpreter: $(python3 --version 2>&1))"
echo "Repo: $REPO_ROOT"
echo

python3 - "$REPO_ROOT" <<'PYEOF'
import os, sys, glob, re, bisect, warnings

repo = sys.argv[1]

# Shell sources that embed Python. Globbed relative to the repo root.
# cli/k2 is the real CLI (most of the embedded Python lives there);
# cli/k2so is the thin shim.
PATTERNS = ["cli/k2", "cli/k2so", "cli/*.sh", "tests/**/*.sh", "scripts/**/*.sh"]
EXCLUDE_SUBSTR = ("/.claude/worktrees/", "/target/", "/node_modules/", "/.git/")

files = []
for pat in PATTERNS:
    for f in glob.glob(os.path.join(repo, pat), recursive=True):
        if not any(s in f for s in EXCLUDE_SUBSTR) and os.path.isfile(f):
            files.append(f)
files = sorted(set(files))

# A bash expansion ($x, ${x}, $(cmd), `cmd`) has no static value. It is
# replaced by this identifier, so `x = $n` or `open("$dir/f")` still
# compiles and the rest of the block is still checked.
PH = "XVAR"
PY_RE = re.compile(r"python3(\.[0-9]+)?")
SHELLS = ("sh", "bash")
BLANK = " \t"
WORD_END = " \t\n;&|<>()"


class LexError(Exception):
    pass


class Word:
    __slots__ = ("parts", "pos", "sq_bare")

    def __init__(self, pos):
        self.parts = []  # ("lit", text) | ("exp", raw bash)
        self.pos = pos
        # A '...' piece with bare text right after it ('...'k2): what a
        # quote inside a single-quoted program leaves behind.
        self.sq_bare = False

    def lit(self, s):
        self.parts.append(("lit", s))

    def exp(self, s):
        self.parts.append(("exp", s))

    def value(self):
        return "".join(PH if k == "exp" else s for k, s in self.parts)

    def literal(self):
        if any(k == "exp" for k, _ in self.parts):
            return None
        return "".join(s for _, s in self.parts)

    def expansion_only(self):
        return any(k == "exp" for k, _ in self.parts) and not any(
            k == "lit" and s.strip() for k, s in self.parts)


class Heredoc:
    __slots__ = ("delim", "quoted", "strip", "fd", "here_string", "body")

    def __init__(self, delim, quoted, strip, fd, here_string=False):
        self.delim, self.quoted, self.strip, self.fd = delim, quoted, strip, fd
        self.here_string = here_string
        self.body = None  # a Word, filled when the body is read


class Cmd:
    __slots__ = ("words", "redirs", "func")

    def __init__(self):
        self.words = []
        self.redirs = []
        self.func = None  # the shell function it is in, if any


class Lexer:
    """A small bash lexer: quotes, escapes, $-expansions, command
    substitution, heredocs, comments, `case` patterns. It finds simple
    commands and their words; it does not run anything."""

    def __init__(self, text, base_line=1):
        self.t = text
        self.n = len(text)
        self.nl = [i for i, c in enumerate(text) if c == "\n"]
        self.base_line = base_line
        self.cmds = []
        self.pending = []
        # Shell functions: `name() {` opens one; its body ends at the `}`
        # that closes that brace.
        self.fn_pending = None
        self.fn_stack = []  # (name, brace depth inside its body)
        self.brace = 0

    def line(self, pos):
        return self.base_line + bisect.bisect_left(self.nl, pos)

    def fail(self, pos, msg):
        raise LexError(f"line {self.line(pos)}: {msg}")

    # -- lists of commands -------------------------------------------------
    def parse_list(self, i, end):
        """Parse commands from i until `end` (")" or "`") or EOF."""
        t, n = self.t, self.n
        cmd = Cmd()
        depth = 0
        case_pat = []  # one flag per open `case`: True while reading a pattern
        while True:
            if i >= n:
                if end is not None:
                    self.fail(i, f"unterminated {end!r}")
                self.finish(cmd)
                return i
            c = t[i]
            if c in BLANK:
                i += 1
                continue
            if c == "\\" and t.startswith("\n", i + 1):
                i += 2
                continue
            if c == "\n":
                self.finish(cmd)
                cmd = Cmd()
                i = self.read_heredocs(i + 1)
                continue
            if c == "#":
                j = t.find("\n", i)
                i = n if j == -1 else j
                continue
            if end == "`" and c == "`":
                self.finish(cmd)
                return i + 1
            if c == ")":
                if case_pat and case_pat[-1]:
                    case_pat[-1] = False  # end of a case pattern
                    self.finish(cmd)
                    cmd = Cmd()
                    i += 1
                    continue
                if end == ")" and depth == 0:
                    self.finish(cmd)
                    return i + 1
                depth = max(0, depth - 1)
                self.finish(cmd)
                cmd = Cmd()
                i += 1
                continue
            if c == "(":
                if t.startswith("(", i + 1) and not cmd.words:
                    i = self.skip_arith(i + 2)
                    continue
                if case_pat and case_pat[-1]:
                    i += 1  # optional leading ( of a case pattern
                    continue
                m = re.match(r"\([ \t]*\)", t[i:i + 16])
                names = [w.literal() for w in cmd.words]
                if m and (len(names) == 1 or (len(names) == 2 and names[0] == "function")):
                    self.fn_pending = names[-1]  # `name()`: a function
                    cmd = Cmd()
                    i += m.end()
                    continue
                depth += 1
                self.finish(cmd)
                cmd = Cmd()
                i += 1
                continue
            if c == "&" and t.startswith(">", i + 1):
                i = self.redirect(i + 1, cmd, None)
                continue
            if c in ";&|":
                if case_pat and (t.startswith(";;", i) or t.startswith(";&", i)):
                    case_pat[-1] = True
                self.finish(cmd)
                cmd = Cmd()
                while i < n and t[i] in ";&|":
                    i += 1
                continue
            if c in "<>" and not t.startswith("(", i + 1):
                i = self.redirect(i, cmd, None)
                continue
            m = re.match(r"[0-9]+(?=[<>])", t[i:i + 6])
            if m and not t.startswith("(", i + m.end() + 1):
                i = self.redirect(i + m.end(), cmd, m.group(0))
                continue
            w, i = self.read_word(i)
            lit = w.literal()
            if case_pat and case_pat[-1]:
                if lit == "esac":
                    case_pat.pop()
                    self.finish(cmd)
                    cmd = Cmd()
                continue  # pattern words are not commands
            if not cmd.words and lit == "case":
                case_pat.append(None)  # waiting for `in`
            elif case_pat and case_pat[-1] is None and lit == "in":
                case_pat[-1] = True
                self.finish(cmd)
                cmd = Cmd()
                continue
            elif not cmd.words and lit == "esac" and case_pat:
                case_pat.pop()
            cmd.words.append(w)

    def finish(self, cmd):
        if not (cmd.words or cmd.redirs):
            return
        first = cmd.words[0].literal() if cmd.words else None
        if first == "{":
            self.brace += 1
            if self.fn_pending:
                self.fn_stack.append((self.fn_pending, self.brace))
        self.fn_pending = None
        cmd.func = self.fn_stack[-1][0] if self.fn_stack else None
        if first == "}":
            if self.fn_stack and self.fn_stack[-1][1] == self.brace:
                self.fn_stack.pop()
            self.brace = max(0, self.brace - 1)
        self.cmds.append(cmd)

    def redirect(self, i, cmd, fd):
        t = self.t
        if t.startswith("<<<", i):
            op, i = "<<<", i + 3
        elif t.startswith("<<-", i):
            op, i = "<<-", i + 3
        elif t.startswith("<<", i):
            op, i = "<<", i + 2
        else:
            op = t[i]
            i += 1
            if i < self.n and t[i] in ">&|":
                i += 1
        while i < self.n and t[i] in BLANK:
            i += 1
        w, i = self.read_word(i)
        if op == "<<<":
            hd = Heredoc(None, False, False, fd, here_string=True)
            hd.body = w
            cmd.redirs.append(hd)
        elif op in ("<<", "<<-"):
            raw = self.t[w.pos:i]
            hd = Heredoc(w.value(), any(q in raw for q in "'\"\\"), op == "<<-", fd)
            cmd.redirs.append(hd)
            self.pending.append(hd)
        return i

    def read_heredocs(self, i):
        pending, self.pending = self.pending, []
        t = self.t
        for hd in pending:
            start = i
            while True:
                if i >= self.n:
                    self.fail(start, f"heredoc {hd.delim!r} has no end line")
                j = t.find("\n", i)
                j = self.n if j == -1 else j
                ln = t[i:j]
                if (ln.lstrip("\t") if hd.strip else ln) == hd.delim:
                    body_end = i
                    i = min(j + 1, self.n)
                    break
                i = j + 1
            w = Word(start)
            if hd.quoted:
                body = t[start:body_end]
                if hd.strip:
                    body = "".join(l.lstrip("\t") for l in body.splitlines(True))
                w.lit(body)
            else:
                self.parse_heredoc_body(start, body_end, w)
            hd.body = w
        return i

    # -- words ---------------------------------------------------------------
    def read_word(self, i):
        t, n = self.t, self.n
        w = Word(i)
        after_sq = False
        while i < n:
            c = t[i]
            if c in "<>" and t.startswith("(", i + 1):
                j = self.parse_list(i + 2, ")")
                w.exp(t[i:j])
                i = j
                after_sq = False
                continue
            if c in WORD_END:
                break
            sq = False
            if c == "\\":
                if t.startswith("\n", i + 1):
                    i += 2
                    continue
                w.lit(t[i + 1:i + 2])
                i += 2
            elif c == "'":
                j = t.find("'", i + 1)
                if j == -1:
                    self.fail(i, "unterminated single quote")
                w.lit(t[i + 1:j])
                i = j + 1
                sq = True
            elif c == '"':
                i = self.parse_dq(i + 1, w)
            elif c == "$":
                i = self.parse_dollar(i, w, in_dq=False)
            elif c == "`":
                i = self.parse_backtick(i + 1, w)
            else:
                if after_sq:
                    w.sq_bare = True
                w.lit(c)
                i += 1
            after_sq = sq
        return w, i

    def parse_dq(self, i, w):
        """From just after an opening ", to just after the closing "."""
        t, n = self.t, self.n
        start = i
        while True:
            if i >= n:
                self.fail(start - 1, "unterminated double quote")
            c = t[i]
            if c == '"':
                return i + 1
            if c == "\\":
                nxt = t[i + 1:i + 2]
                if nxt in ('"', "\\", "$", "`"):
                    w.lit(nxt)
                    i += 2
                elif nxt == "\n":
                    i += 2
                else:
                    w.lit("\\")
                    i += 1
            elif c == "$":
                i = self.parse_dollar(i, w, in_dq=True)
            elif c == "`":
                i = self.parse_backtick(i + 1, w)
            else:
                w.lit(c)
                i += 1

    def parse_heredoc_body(self, i, end, w):
        """An unquoted heredoc: like double quotes, but " is not special."""
        t = self.t
        while i < end:
            c = t[i]
            if c == "\\":
                nxt = t[i + 1:i + 2]
                if nxt in ("\\", "$", "`"):
                    w.lit(nxt)
                    i += 2
                elif nxt == "\n":
                    i += 2
                else:
                    w.lit("\\")
                    i += 1
            elif c == "$":
                i = self.parse_dollar(i, w, in_dq=True)
            elif c == "`":
                i = self.parse_backtick(i + 1, w)
            else:
                w.lit(c)
                i += 1

    def parse_dollar(self, i, w, in_dq):
        t = self.t
        nxt = t[i + 1:i + 2]
        if nxt == "'" and not in_dq:
            j = i + 2
            while j < self.n and t[j] != "'":
                j += 2 if t[j] == "\\" else 1
            if j >= self.n:
                self.fail(i, "unterminated $'...'")
            raw = t[i + 2:j]
            try:
                w.lit(raw.encode("latin-1", "backslashreplace").decode("unicode_escape"))
            except UnicodeDecodeError:
                w.lit(raw)
            return j + 1
        if nxt == '"' and not in_dq:
            return self.parse_dq(i + 2, w)
        if t.startswith("((", i + 1):
            j = self.skip_arith(i + 3)
            w.exp(t[i:j])
            return j
        if nxt == "(":
            j = self.parse_list(i + 2, ")")
            w.exp(t[i:j])
            return j
        if nxt == "{":
            j = self.skip_brace(i + 2, in_dq)
            w.exp(t[i:j])
            return j
        m = re.match(r"[A-Za-z_][A-Za-z0-9_]*|[0-9@*?#!$-]", t[i + 1:i + 65])
        if m:
            w.exp(t[i:i + 1 + m.end()])
            return i + 1 + m.end()
        w.lit("$")
        return i + 1

    def parse_backtick(self, i, w):
        t = self.t
        j = i
        while j < self.n and t[j] != "`":
            j += 2 if t[j] == "\\" else 1
        if j >= self.n:
            self.fail(i - 1, "unterminated backtick")
        inner = re.sub(r"\\([\\$`])", r"\1", t[i:j])
        sub = Lexer(inner, self.line(i))
        sub.parse_list(0, None)
        here = self.fn_stack[-1][0] if self.fn_stack else None
        for c in sub.cmds:
            c.func = c.func or here
        self.cmds.extend(sub.cmds)
        w.exp(t[i - 1:j + 1])
        return j + 1

    def skip_brace(self, i, in_dq):
        """From just after ${, to just after the matching }."""
        t = self.t
        dummy = Word(i)
        start = i
        while True:
            if i >= self.n:
                self.fail(start - 2, "unterminated ${")
            c = t[i]
            if c == "}":
                return i + 1
            if c == "\\":
                i += 2
            elif c == '"':
                i = self.parse_dq(i + 1, dummy)
            elif c == "'" and not in_dq:
                j = t.find("'", i + 1)
                if j == -1:
                    self.fail(i, "unterminated single quote in ${")
                i = j + 1
            elif c == "$":
                i = self.parse_dollar(i, dummy, in_dq)
            elif c == "`":
                i = self.parse_backtick(i + 1, dummy)
            else:
                i += 1

    def skip_arith(self, i):
        """From just after ((, to just after the matching ))."""
        t = self.t
        dummy = Word(i)
        depth = 0
        start = i
        while True:
            if i >= self.n:
                self.fail(start - 2, "unterminated ((")
            c = t[i]
            if c == ")" and depth == 0:
                if not t.startswith(")", i + 1):
                    self.fail(i, "arithmetic (( closed by a single )")
                return i + 2
            if c == "(":
                depth += 1
                i += 1
            elif c == ")":
                depth -= 1
                i += 1
            elif c == "$":
                i = self.parse_dollar(i, dummy, True)
            elif c == '"':
                i = self.parse_dq(i + 1, dummy)
            else:
                i += 1


# Python's short options that take no argument, so `python3 -I -c ...`
# and `python3 -Ic ...` are both read right.
PY_FLAGS = set("bBdEhiIOPqsSuvx")


def python_code(cmd, k):
    """The Python a `python3` command runs, as ("c"|"stdin", Word, Heredoc
    or None), or None when it runs a file or module (not embedded)."""
    ws = cmd.words
    j = k + 1
    while j < len(ws):
        a = ws[j].literal()
        if a is None:
            return None  # a script path from a variable
        if a in ("-W", "-X"):
            j += 2
            continue
        if a == "-":
            break
        if a.startswith("-") and len(a) > 1:
            flags = a[1:]
            if flags.endswith("c") and set(flags[:-1]) <= PY_FLAGS:
                return ("c", ws[j + 1], None) if j + 1 < len(ws) else None
            if "m" in flags:
                return None
            j += 1
            continue
        return None  # a script file
    r = stdin_body(cmd)
    if r is not None:
        return ("stdin", r.body, r)
    return None  # stdin from a pipe or a file: not in this source


def stdin_body(cmd):
    for r in cmd.redirs:
        if r.fd in (None, "0") and r.body is not None:
            return r
    return None


def shell_code(cmd, k):
    """The script a `sh`/`bash` command runs from this source (`-c '...'`,
    or a heredoc on stdin), as (Word, Heredoc-or-None), or None."""
    ws = cmd.words
    j = k + 1
    stdin = False
    while j < len(ws):
        a = ws[j].literal()
        if a is None or not a.startswith(("-", "+")) or a in ("-", "+"):
            break
        if a == "--":
            j += 1
            break
        if a in ("-o", "+o", "-O", "+O"):
            j += 2
            continue
        if a.startswith("-") and a.endswith("c"):
            return (ws[j + 1], None) if j + 1 < len(ws) else None
        if a.startswith("-") and "s" in a:
            stdin = True
        j += 1
    if stdin or j >= len(ws):
        r = stdin_body(cmd)
        if r is not None:
            return r.body, r
    return None


SHEBANG_RE = re.compile(r"#![^\n]*\b(ba)?sh\b")


VAR_RE = re.compile(r"\$\{?([A-Za-z_][A-Za-z0-9_]*|[0-9]+)\}?")


def code_var(word):
    """NAME when the code word is just "$NAME" or "${NAME}", else None."""
    exps = [s for k, s in word.parts if k == "exp"]
    if len(exps) != 1:
        return None
    m = VAR_RE.fullmatch(exps[0])
    return m.group(1) if m else None


ASSIGN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
POS_RE = re.compile(r"\$\{?([0-9])\}?")


def is_assignment(word):
    """True for a NAME=value word (bash reads it as an assignment)."""
    return bool(ASSIGN_RE.match(word.value()))


def command_index(cmd):
    """Index of the command word, after any NAME=value prefixes."""
    k = 0
    while k < len(cmd.words) and is_assignment(cmd.words[k]):
        k += 1
    return k


def assigned_positional(word):
    """(NAME, N) for a `NAME="$N"` word, else None."""
    lits, exps, seen_exp = [], [], False
    for k, s in word.parts:
        if k == "exp":
            exps.append(s)
            seen_exp = True
        elif seen_exp:
            if s:
                return None
        else:
            lits.append(s)
    m = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)=", "".join(lits))
    if not m or len(exps) != 1:
        return None
    p = POS_RE.fullmatch(exps[0])
    return (m.group(1), p.group(1)) if p else None


def runner_functions(lx):
    """{function: N} for each shell function that runs its argument $N as
    a Python program: `python3 -c "$N"` (or a local copy of $N), an
    environment variable set to it that the Python exec()s, or a call of
    another such function with it."""
    alias = {}  # (function, NAME) -> N
    for cmd in lx.cmds:
        if cmd.func is None:
            continue
        for w in cmd.words:
            got = assigned_positional(w)
            if got:
                alias[(cmd.func, got[0])] = got[1]

    def positional(func, word):
        name = code_var(word) if word.expansion_only() else None
        if name is None:
            return None
        return name if name.isdigit() else alias.get((func, name))

    def env_positional(func, word):
        exps = [x for kk, x in word.parts if kk == "exp"]
        vm = VAR_RE.fullmatch(exps[0]) if len(exps) == 1 else None
        if not vm:
            return None
        name = vm.group(1)
        return name if name.isdigit() else alias.get((func, name))

    runners = {}
    changed = True
    while changed:
        changed = False
        for cmd in lx.cmds:
            f = cmd.func
            if f is None or f in runners:
                continue
            found = None
            for k, w in enumerate(cmd.words):
                lit = w.literal()
                if lit is None:
                    continue
                base = lit.rsplit("/", 1)[-1]
                if PY_RE.fullmatch(base):
                    got = python_code(cmd, k)
                    if got and got[0] == "c":
                        found = positional(f, got[1])
                    if found is None and got:
                        code = got[1].value()
                        for pw in cmd.words[:k]:
                            m = ASSIGN_RE.match(pw.value())
                            if m and pw.value() == m.group(0) + PH:
                                n = env_positional(f, pw)
                                if n and exec_reads(code, m.group(0)[:-1]):
                                    found = n
                                    break
                    break
                if lit in runners and k == command_index(cmd):
                    j = k + int(runners[lit])
                    if j < len(cmd.words):
                        found = positional(f, cmd.words[j])
                    break
            if found:
                runners[f] = found
                changed = True
    return runners, alias


def exec_reads(code, var):
    """True when the Python exec()s the environment variable var."""
    return re.search(r"exec\(\s*os\.environ(\[|\.get\(|\.pop\()\s*[\"']%s[\"']"
                     % re.escape(var), code) is not None


def stray_assignments(lx):
    """Yield (line, message) for each `NAME='...'` over several lines that
    ends in bare text ('...'k2) with a command after it: a quote inside the
    value closed it early."""
    for cmd in lx.cmds:
        k = command_index(cmd)
        if k == 0 or k >= len(cmd.words):
            continue
        multi = [w for w in cmd.words[:k] if w.sq_bare and "\n" in w.value()]
        if not multi:
            continue
        w = multi[-1]
        name = w.value().split("=", 1)[0]
        cmd_word = cmd.words[k].value()
        yield lx.line(w.pos), (
            "%s= is a value over several lines with a command after it "
            "(%r on line %d): a quote inside the value closed it early"
            % (name, cmd_word[:40], lx.line(cmd.words[k].pos)))


def blocks(text, base_line=1, depth=0):
    """Yield (kind, Word-or-str-or-None, line, skip_reason) per block."""
    lx = Lexer(text, base_line)
    lx.parse_list(0, None)
    runners, alias = runner_functions(lx)
    var_uses = []  # (name, line) for `python3 -c "$NAME"`
    # NAME='...' programs a block may start with: `python3 -c
    # "$NAME"'more code'` (or "$A$B") runs NAME's text and then the rest,
    # so the block is checked with that text in place, as Python gets it.
    programs = {}
    for cmd in lx.cmds:
        for w in cmd.words:
            lit = w.literal()
            m = ASSIGN_RE.match(lit or "")
            if m and lx.t[w.pos + m.end():w.pos + m.end() + 1] == "'":
                programs[m.group(0)[:-1]] = lit[m.end():]

    def lead(word):
        """The word's leading "$NAME" parts that are such programs, and
        how many there are."""
        head, n = "", 0
        for k, s in word.parts:
            m = VAR_RE.fullmatch(s) if k == "exp" else None
            if not m or m.group(1) not in programs:
                break
            head += programs[m.group(1)]
            n += 1
        return head, n

    def resolved(word):
        """The word's text, its leading programs written out."""
        head, n = lead(word)
        return head + "".join(PH if k == "exp" else s for k, s in word.parts[n:])

    def own_program(cmd, name):
        """True when python3 -c "$name" runs the argument that makes the
        function it is in a runner (each call's argument is checked)."""
        pos = name if name.isdigit() else alias.get((cmd.func, name))
        return pos is not None and runners.get(cmd.func) == pos

    for ci, cmd in enumerate(lx.cmds):
        used = None
        # A call of a helper that runs one of its arguments as Python.
        k0 = command_index(cmd)
        if k0 < len(cmd.words) and cmd.words[k0].literal() in runners:
            j = k0 + int(runners[cmd.words[k0].literal()])
            if j < len(cmd.words):
                arg = cmd.words[j]
                line = lx.line(arg.pos)
                cv = code_var(arg) if arg.expansion_only() else None
                if not arg.expansion_only():
                    yield "helper", arg.value(), line, None
                elif cv is None:
                    yield "helper", None, line, "the code is a bash substitution"
                elif not cv.isdigit() and (cmd.func, cv) not in alias:
                    var_uses.append((cv, line))
                # "$N" or a copy of it: a helper passing its own argument
                # on; its callers are checked.
        for k, w in enumerate(cmd.words):
            lit = w.literal()
            if lit is None:
                continue
            base = lit.rsplit("/", 1)[-1]
            if PY_RE.fullmatch(base):
                got = python_code(cmd, k)
                if got:
                    kind, code, used = got
                    line = lx.line(code.pos)
                    if not code.expansion_only():
                        yield kind, resolved(code), line, None
                    elif lead(code)[1] > 1 and lead(code)[1] == len(code.parts):
                        yield kind, resolved(code), line, None
                    elif code_var(code) and own_program(cmd, code_var(code)):
                        pass
                    elif code_var(code):
                        var_uses.append((code_var(code), line))
                        # `{ _k2_stdin_secrets …; cat; } <<'PY' | … python3 -c
                        # "$_K2_PY_STDIN_BOOT"`: the script is the heredoc on
                        # the `}` before the pipe (the commands of any $(…)
                        # in between come first in lx.cmds).
                        if code_var(code) == "_K2_PY_STDIN_BOOT":
                            for pc in reversed(lx.cmds[max(0, ci - 12):ci]):
                                prev = stdin_body(pc)
                                if prev is not None and pc.words and pc.words[0].literal() == "}":
                                    yield "stdin", prev.body.value(), lx.line(prev.body.pos), None
                                    break
                    else:
                        yield kind, None, line, "the code is a bash substitution"
                break
            if base in SHELLS:
                got = shell_code(cmd, k)
                if got and depth < 4 and not got[0].expansion_only():
                    # A script run by `sh -c '...'` or `bash -s <<EOF`:
                    # lex it as its own script.
                    code, used = got
                    yield from blocks(code.value(), lx.line(code.pos), depth + 1)
                break
        for r in cmd.redirs:
            # A heredoc that writes a shell script (a stub, an example the
            # CLI prints): its first line is a sh/bash shebang. Lex it too.
            if r is used or r.here_string or r.body is None or depth >= 4:
                continue
            body = r.body.value()
            if SHEBANG_RE.match(body):
                yield from blocks(body, lx.line(r.body.pos), depth + 1)
    # `python3 -c "$NAME"`: check each `NAME='...'` assignment in the same
    # source. A NAME with no such assignment (a function argument like $1,
    # or set in another file) cannot be judged here.
    names = {name for name, _ in var_uses}
    found = set()
    for cmd in lx.cmds:
        for w in cmd.words:
            v = w.value()
            m = re.match(r"([A-Za-z_][A-Za-z0-9_]*)=", v)
            if not m or m.group(1) not in names:
                continue
            code = v[m.end():]
            if lx.t[w.pos + m.end():w.pos + m.end() + 1] not in ("'", '"', "$"):
                continue  # NAME=0: an unquoted flag, not code
            if code.strip() in ("", PH):
                continue  # NAME="$other" or NAME="": not code
            found.add(m.group(1))
            yield "var", code, lx.line(w.pos), None
    for name, line in var_uses:
        if name not in found:
            yield "c", None, line, f"the code is ${name}, not assigned in this file"
    for line, msg in stray_assignments(lx):
        yield "stray", msg, line, None


KINDS = ("c", "stdin", "var", "helper")
checked = {"cli/k2": dict.fromkeys(KINDS, 0), "other": dict.fromkeys(KINDS, 0)}
skipped = []
broken = []
setup_errors = []
for f in files:
    rel = os.path.relpath(f, repo)
    group = "cli/k2" if rel == "cli/k2" else "other"
    with open(f, encoding="utf-8") as fh:
        txt = fh.read()
    try:
        found = list(blocks(txt))
    except LexError as e:
        setup_errors.append(f"{rel}: {e}")
        continue
    for kind, code, line, reason in found:
        if kind == "stray":
            broken.append((rel, line, code, ""))
            continue
        if reason:
            skipped.append((rel, line, reason))
            continue
        if not code.strip():
            skipped.append((rel, line, "the code is empty"))
            continue
        checked[group][kind] += 1
        try:
            # SyntaxWarnings (an odd escape) are not this check's job.
            with warnings.catch_warnings():
                warnings.simplefilter("ignore")
                compile(code, f"{rel}:{line}", "exec", dont_inherit=True)
        except SyntaxError as e:
            at = line + (e.lineno or 1) - 1
            src = (e.text or "").strip()[:70]
            broken.append((rel, at, e.msg, src))

if setup_errors:
    print("ERROR: could not read these files as bash (the check cannot judge them):")
    for s in setup_errors:
        print(f"  {s}")
    sys.exit(2)

print(f"Scanned {len(files)} shell files")
for g, c in checked.items():
    label = "cli/k2" if g == "cli/k2" else "other files"
    print(f"  {label:12} compiled {c['c']:5} python3 -c blocks, {c['stdin']:4} heredoc"
          f" blocks, {c['var']:3} blocks held in a variable, {c['helper']:3} passed to a"
          f" helper")
print(f"  skipped by rule: {len(skipped)}")
for rel, line, reason in skipped:
    print(f"    {rel}:{line}  {reason}")

if broken:
    print(f"\nFAILED: {len(broken)} broken block(s):\n")
    for rel, at, msg, src in broken:
        print(f"  {rel}:{at}  {msg}")
        print(f"      {src!r}")
    print("\nHint: a backslash inside an f-string {} field breaks on Python 3.12+.")
    print("Fix: hoist the quoted sub-expression to a variable, e.g.")
    print('  BEFORE  print(f"x: {d[\\"k\\"]}")')
    print('  AFTER   v = d["k"]; print(f"x: {v}")')
    sys.exit(1)

print("OK: every embedded Python block compiles.")
PYEOF
