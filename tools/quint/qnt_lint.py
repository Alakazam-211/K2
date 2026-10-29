#!/usr/bin/env python3
r"""qnt_lint.py: flag Quint expressions that Quint's TLA+ export mis-compiles (LET swallow).

Usage:
  qnt_lint.py [--quint CMD] [--allow DEF[,DEF...]] FILE.qnt [FILE.qnt ...]

  Runs `quint parse` on each file (no Java, no instance needed: every module in the file and
  the files it imports from are linted) and reports every place where the TLA+ printer would
  emit a hoisted `LET ... IN` as the right operand of an infix operator with more formula
  after it, which TLA+ then reads as part of the LET body. One line per finding:

    FILE:LINE:COL: DEF: <op> operand <expr> is printed as LET ... IN and swallows what follows (<outer op>)

  Exit status: 0 no findings, 1 findings, 2 a tool error. --allow skips named definitions
  (e.g. a fixture you know is harmless); the exact check is export_check.py.

  Needs: Python 3 (stdlib only) and Quint on PATH (or --quint "npx quint", or $QUINT).

The shape (Quint 0.33 / Apalache 0.62 printer):
  * "Hoisted" expressions print as `LET <lambda> IN <call>` with no parentheses: a list
    `foldl` or `select`, a set `fold`, `range`, and any call of a user-defined operator with
    a lambda argument; also a postfix use of one of those that keeps the LET in front:
    `[i]`/`nth`, `get`, a tuple item `._1`, a record field `.f`, `concat`, `union`,
    `intersect`, `exclude`. (Wrapped uses are safe: `length`, `head`, `tail`, `append`,
    `slice`, `size`, `indices`, `keys`, `contains`, `filter`, `map`, `forall`, ...)
  * Such an expression is the right operand of an infix operator (`==`, `!=`, `<`, `+`,
    `implies`, `and`, `in`, ...), and that operator's expression is followed, within the
    same printed group, by more of an enclosing infix expression (another conjunct in
    `and`/`all {}`, `or`, `implies`, `==`, ...). Source parentheses do not help: Quint drops
    them and the printer re-parenthesises by TLA+ precedence.
  * Harmless (not flagged): the operator before the LET and everything it swallows are one
    associative junction (`a and l.foldl(..) and c`), or a bracket / call / quantifier /
    IF / CASE / `not(...)` / assignment `x' = ...` closes the group first.
  Fix: bind the expression first (`val v = l.foldl(..)`, then `x == v and c`).
"""
import json
import os
import shlex
import subprocess
import sys
import tempfile

# TLA+ precedence of the infix operators Quint prints (higher binds tighter)
PREC = {
    "implies": 1, "iff": 2,
    "and": 3, "or": 3, "actionAll": 3, "actionAny": 3,
    "eq": 5, "neq": 5, "ilt": 5, "ilte": 5, "igt": 5, "igte": 5, "in": 5, "contains": 5,
    "subseteq": 5,
    "union": 8, "intersect": 8, "exclude": 8,
    "iadd": 10, "imod": 10, "isub": 11, "imul": 13, "idiv": 13, "concat": 13, "ipow": 14,
}
JUNCTION = {"and": "and", "actionAll": "and", "or": "or", "actionAny": "or"}
SHOW = {"eq": "==", "neq": "!=", "ilt": "<", "ilte": "<=", "igt": ">", "igte": ">=", "iadd": "+",
        "isub": "-", "imul": "*", "idiv": "/", "imod": "%", "ipow": "^", "actionAll": "all {}",
        "actionAny": "any {}"}
# printed `LET L IN Op(L, ..)` with the LET in front
HOIST = {"foldl", "fold", "select", "range"}
# postfix uses that keep a LET-led base in front: base is args[0]
POSTFIX = {"nth", "get", "item", "field"}


def printed_operands(app):
    """the operands of an infix app in printed order (contains(S, x) prints `x \\in S`)"""
    a = app["args"]
    if app["opcode"] == "contains" and len(a) == 2:
        return [a[1], a[0]]
    return list(a)


def is_infix(e):
    return e.get("kind") == "app" and e.get("opcode") in PREC


class Linter:
    def __init__(self, table, smap, path_of):
        self.table = table
        self.smap = smap
        self.path_of = path_of
        self.findings = []

    def user_op(self, e):
        t = self.table.get(str(e["id"]))
        return t is not None and t.get("kind") == "def"

    def let_led(self, e):
        if e.get("kind") != "app":
            return False
        op = e["opcode"]
        if op in HOIST:
            return True
        if self.user_op(e) and any(a.get("kind") == "lambda" for a in e["args"]):
            return True
        if op in POSTFIX and e["args"]:
            return self.let_led(e["args"][0])
        if op in PREC:
            first = printed_operands(e)[0]
            return self.let_led(first) and not parenthesised(first, e)
        return False

    def where(self, e):
        m = self.smap.get(str(e["id"]))
        if not m:
            return "?:?:?"
        return "%s:%d:%d" % (self.path_of(m[0]), m[1]["line"] + 1, m[1]["col"] + 1)

    def walk(self, e, stack, defname):
        """stack: list of (parent app, index of e in the parent's printed operands or None)"""
        k = e.get("kind")
        if k == "app":
            if is_infix(e):
                ops = printed_operands(e)
                for i, r in enumerate(ops):
                    if i == 0:
                        continue
                    if self.let_led(r) and not parenthesised(r, e):
                        self.check(e, i, r, stack, defname)
                for i, a in enumerate(ops):
                    self.walk(a, stack + [(e, i)], defname)
            else:
                for a in e["args"]:
                    self.walk(a, stack + [(e, None)], defname)
        elif k == "lambda":
            self.walk(e["expr"], stack + [(e, None)], defname)
        elif k == "let":
            self.walk(e["opdef"]["expr"], stack + [(e, None)], defname)
            self.walk(e["expr"], stack + [(e, None)], defname)

    def check(self, p, i, r, stack, defname):
        """r (LET-led) is printed operand i > 0 of infix p: does anything follow it?"""
        nops = len(printed_operands(p))
        crossed = [p["opcode"]]
        follows = None
        if i < nops - 1:
            follows = p["opcode"]
        else:
            child = p
            for parent, idx in reversed(stack):
                if parent.get("kind") != "app" or not is_infix(parent) or idx is None:
                    break  # a bracket, call, quantifier, IF, LET, lambda: the group closes
                if parenthesised(child, parent):
                    break
                n = len(printed_operands(parent))
                if idx < n - 1:
                    follows = parent["opcode"]
                    break
                crossed.append(parent["opcode"])
                child = parent
        if follows is None:
            return
        # harmless: one associative junction all the way (a and F and c)
        fam = {JUNCTION.get(o, o) for o in crossed + [follows]}
        if len(fam) == 1 and next(iter(fam)) in ("and", "or"):
            return
        self.findings.append("%s: %s: right operand of `%s` is printed as LET ... IN (%s) and "
                             "swallows the rest of the enclosing `%s`" % (
                                 self.where(r), defname, SHOW.get(p["opcode"], p["opcode"]),
                                 describe(r), SHOW.get(follows, follows)))


def parenthesised(c, p):
    """does the printer put parentheses around infix child c of infix parent p?"""
    if not is_infix(c):
        return False
    # the printer parenthesises every infix child that does not bind strictly tighter,
    # same operator included (`(x + LET ..) + 1`, `(a /\ b) /\ c`); an n-ary `and`/`all {}`
    # is one node, its conjuncts siblings
    return PREC[c["opcode"]] <= PREC[p["opcode"]]


def describe(e):
    if e.get("kind") != "app":
        return e.get("kind", "?")
    op = e["opcode"]
    if op in POSTFIX or op in PREC:
        inner = printed_operands(e)[0] if op in PREC else e["args"][0]
        return "%s on %s" % (op, describe(inner))
    return op + "(...)"


def lint_file(path, quint, allow):
    tmp = tempfile.mkdtemp(prefix="qnt_lint_")
    out, sm = os.path.join(tmp, "p.json"), os.path.join(tmp, "m.json")
    p = subprocess.run(quint + ["parse", "--out=" + out, "--source-map=" + sm, os.path.abspath(path)],
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                       cwd=os.path.dirname(os.path.abspath(path)))
    if p.returncode != 0 or not os.path.exists(out):
        print("qnt_lint: quint parse failed on %s:\n%s%s" % (path, p.stdout[-2000:], p.stderr[-2000:]),
              file=sys.stderr)
        return None
    ir = json.load(open(out))
    smap_doc = json.load(open(sm))
    idx = smap_doc.get("sourceIndex", {})
    here = os.getcwd()

    def path_of(i):
        f = idx.get(str(i), "?")
        try:
            return os.path.relpath(f, here)
        except ValueError:
            return f
    L = Linter(ir.get("table", {}), smap_doc.get("map", {}), path_of)
    seen = set()
    for m in ir["modules"]:
        for d in m.get("declarations", []):
            if d.get("kind") != "def" or "expr" not in d or d.get("name") in allow:
                continue
            if d["id"] in seen:
                continue
            seen.add(d["id"])
            L.walk(d["expr"], [], m["name"] + "::" + d["name"])
    # one module's findings can repeat through imports: dedupe, keep order
    uniq = []
    for f in L.findings:
        if f not in uniq:
            uniq.append(f)
    return uniq


def main(argv):
    quint = shlex.split(os.environ.get("QUINT", "quint"))
    allow = set()
    files = []
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--quint":
            quint = shlex.split(args.pop(0))
        elif a == "--allow":
            allow |= {x for x in args.pop(0).split(",") if x}
        elif a in ("-h", "--help"):
            print(__doc__)
            return 0
        else:
            files.append(a)
    if not files:
        print(__doc__)
        return 2
    bad, err = 0, False
    for f in files:
        r = lint_file(f, quint, allow)
        if r is None:
            err = True
            continue
        for line in r:
            print(line)
        bad += len(r)
    print("qnt_lint: %d finding(s) in %d file(s)%s" % (bad, len(files), " (and a parse error)" if err else ""))
    return 2 if err else (1 if bad else 0)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
