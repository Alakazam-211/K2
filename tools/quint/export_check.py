#!/usr/bin/env python3
r"""export_check.py: does Quint's TLA+ export say what the Quint source says?

Usage:
  export_check.py [options] FILE.qnt MODULE [MODULE ...]

  For each MODULE (a module in FILE.qnt, e.g. an instance module), it:
    0. finds what to export: `quint compile` keeps only what init, the step and the
       --invariant list reach, so the check copies the sources to a work directory, appends
       `action __ec_step = any { <every nullary action but init> }` to MODULE's copy and
       passes every nullary boolean `val`/`def` of MODULE (and of the modules it imports
       with `.*`) as an invariant (override with --steps / --invariants);
    1. `quint compile --target json`  -> the Quint IR (the intended formula), read by
       Apalache (`apalache parse X.qnt.json`);
    2. `quint compile --target tlaplus` -> the .tla that TLC runs, read back through SANY
       (`apalache parse X.tla`);
    3. compares the two, definition by definition, after normalising away what does not
       change meaning: types and source ids, parentheses, the placement and scope of
       LET-bound operators (lambdas and `val`s; their bodies are compared separately), the
       nesting of /\ and \/ chains, and the printer's renaming of `_` binders.
  Any definition whose parsed TLA+ differs is reported with its first differing subtree and
  a conjunct check: every top-level conjunct of the intended body must be a top-level
  conjunct of the parsed body. Exit status: 0 when every module matches, 1 when any
  definition differs, 2 on a tool error (compile or parse failed).

Options:
  --quint CMD        the Quint command (default: $QUINT, else `quint`; e.g. "npx quint")
  --apalache-jar J   Apalache's jar (default: $APALACHE_JAR, else the newest
                     ~/.quint/apalache-dist-*/apalache/lib/apalache.jar that `quint verify`
                     downloads; run `quint verify` once to fetch it)
  --java CMD         the java command (default: $JAVA, else `java`); -Xss32m is added
  --keep DIR         keep the intermediate files (.qnt.json, .tla, parsed IR) in DIR
  --roots A,B,...    definitions to report reachability for (default: every definition
                     that is not used by another one: the invariants, actions, View, ...)
  --init NAME        the init action (default: init); the printer writes its `x' := e` as
                     `x = e`, so in it and the definitions only it uses, that is no difference
  --invariants A,B   check exactly these invariants (default: every nullary boolean val/def)
  --steps A,B        reach exactly these actions (default: every nullary action but init)
  --quiet            print only the per-module verdict and the differing definitions

Needs: Python 3 (stdlib only), Quint on PATH (or --quint), a JVM. Nothing repo-specific.

Why: Apalache's TLA+ printer hoists the LET that binds a lambda argument (Quint `foldl`,
`select`, set `fold`, `range`, any user operator called with a lambda) to wrap the whole
call, without parentheses. When that call is the right operand of an infix operator and
something follows it, the LET body swallows what follows: `a == l.foldl(..) and c` exports
as `a = LET L IN Fold(..) /\ c`, which SANY reads as `a = (Fold(..) /\ c)`.
"""
import glob
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile


def die(msg, code=2):
    print("export_check: " + msg, file=sys.stderr)
    sys.exit(code)


def find_jar(arg):
    if arg:
        return arg
    if os.environ.get("APALACHE_JAR"):
        return os.environ["APALACHE_JAR"]
    jars = sorted(glob.glob(os.path.expanduser("~/.quint/apalache-dist-*/apalache/lib/apalache.jar")))
    if not jars:
        die("no Apalache jar: pass --apalache-jar, set APALACHE_JAR, or run `quint verify` once")
    return jars[-1]


def run(cmd, out_path=None, cwd=None):
    with open(out_path, "w") if out_path else open(os.devnull, "w") as out:
        p = subprocess.run(cmd, stdout=out if out_path else subprocess.PIPE, stderr=subprocess.PIPE,
                           cwd=cwd, text=True)
    if p.returncode != 0:
        msg = (p.stderr or "") + ("" if out_path else (p.stdout or ""))
        lines = [l for l in msg.splitlines() if "protobuf" not in l.lower() and l.strip()]
        die("failed (%d): %s\n%s" % (p.returncode, " ".join(cmd), "\n".join(lines[-25:])))


def apalache_parse(java, jar, src, out_json, workdir):
    run(java + ["-Xss32m", "-jar", jar, "parse", "--output=" + out_json, src], cwd=workdir)
    with open(out_json) as f:
        return json.load(f)


# ---------------------------------------------------------------- normalisation

AC = {"AND", "OR"}


class Norm:
    """Normalise one definition body; LET-bound operators go to self.lets (name -> body)."""

    def __init__(self):
        self.lets = {}

    def decl(self, d):
        params = tuple((canon_param(p.get("name", "")), p.get("arity", 0)) for p in d.get("formalParams", []))
        return ("decl", params, self.expr(d["body"]))

    def expr(self, e):
        k = e.get("kind")
        if k == "LetInEx":
            for d in e["decls"]:
                nd = self.decl(d)
                dn = canon_name(d["name"])
                prev = self.lets.get(dn)
                if prev is not None and prev != nd:
                    # the same LET name bound twice with different bodies (an inner `val`
                    # reused across scopes): keep both under distinct keys
                    i = 2
                    while (dn + "#%d" % i) in self.lets and self.lets[dn + "#%d" % i] != nd:
                        i += 1
                    self.lets[dn + "#%d" % i] = nd
                else:
                    self.lets[dn] = nd
            return self.expr(e["body"])
        if k == "OperEx":
            op = e["oper"]
            args = [self.expr(a) for a in e["args"]]
            if op in AC:
                flat = []
                for a in args:
                    if isinstance(a, tuple) and a[0] == "op" and a[1] == op:
                        flat.extend(a[2])
                    else:
                        flat.append(a)
                if len(flat) == 1:
                    return flat[0]
                return ("op", op, tuple(flat))
            return ("op", op, tuple(args))
        if k == "NameEx":
            return ("name", canon_name(e["name"]))
        if k == "ValEx":
            v = e["value"]
            return ("val", v.get("kind"), json.dumps(v.get("value"), sort_keys=True))
        if k == "OperParam":
            return ("param", e.get("name"))
        # anything else: compare it verbatim, minus types and source ids
        return ("raw", k, json.dumps(strip(e), sort_keys=True))


def unprime(t):
    """x' := e  ->  x = e (how the printer writes init)."""
    if not isinstance(t, tuple):
        return t
    if t[0] == "op" and t[1].endswith(":=") and len(t[2]) == 2:
        lhs = t[2][0]
        if isinstance(lhs, tuple) and lhs[0] == "op" and lhs[1] == "PRIME":
            return ("op", "EQ", (lhs[2][0], unprime(t[2][1])))
    if t[0] == "op":
        return ("op", t[1], tuple(unprime(a) for a in t[2]))
    if t[0] == "decl":
        return ("decl", t[1], unprime(t[2]))
    return t


def strip(x):
    if isinstance(x, dict):
        return {k: strip(v) for k, v in x.items() if k not in ("type", "source")}
    if isinstance(x, list):
        return [strip(v) for v in x]
    return x


def canon_param(n):
    return canon_name(n)


_ID_UNDERSCORE = re.compile(r"(^|_)id__(\d+)$")


def canon_name(n):
    """`a::b` -> `a_b` (the printer's qualified names); `id__N` -> `__N` (the printer's name
    for an unused `_` binder)"""
    return _ID_UNDERSCORE.sub(r"\1__\2", n.replace("::", "_"))


def top_decls(ir):
    out = {}
    for m in ir["modules"]:
        for d in m["declarations"]:
            if d.get("kind") == "TlaOperDecl":
                out[canon_name(d["name"])] = d
    return out


def names_in(t, acc):
    if isinstance(t, tuple):
        if t and t[0] == "name":
            acc.add(t[1])
            return acc
        for x in t:
            names_in(x, acc)
    return acc


def show(t, depth=0, budget=[0]):
    """Compact rendering of a normalised tree (for diagnostics)."""
    if not isinstance(t, tuple):
        return str(t)
    if t[0] == "name":
        return t[1]
    if t[0] == "val":
        return t[2]
    if t[0] == "op":
        op = t[1].split("!")[-1]
        if depth > 6:
            return op + "(..)"
        return op + "(" + ", ".join(show(a, depth + 1) for a in t[2]) + ")"
    if t[0] == "decl":
        return "decl" + str([p[0] for p in t[1]]) + " = " + show(t[2], depth)
    return t[0]


def first_diff(a, b, path=()):
    if a == b:
        return None
    if (isinstance(a, tuple) and isinstance(b, tuple) and len(a) == len(b) and a[:2] == b[:2]
            and a and a[0] == "op"):
        if len(a[2]) == len(b[2]):
            for i, (x, y) in enumerate(zip(a[2], b[2])):
                d = first_diff(x, y, path + (a[1].split("!")[-1] + "#%d" % i,))
                if d:
                    return d
    return (path, a, b)


def conjuncts(t):
    if isinstance(t, tuple) and t[0] == "op" and t[1] == "AND":
        return list(t[2])
    return [t]


def clip(s, n=400):
    return s if len(s) <= n else s[:n] + " ..."


# ---------------------------------------------------------------- what to export

def module_span(text, mod):
    """(start of `module MOD {`, index of its closing brace) in text, by brace matching"""
    m = re.search(r"\bmodule\s+%s\s*\{" % re.escape(mod), text)
    if not m:
        die("module %s not found" % mod)
    i, depth, n = m.end(), 1, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            i = text.find("\n", i)
            i = n if i < 0 else i
            continue
        if text.startswith("/*", i):
            j = text.find("*/", i + 2)
            i = n if j < 0 else j + 2
            continue
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            i = j + 1
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return m.start(), i
        i += 1
    die("module %s: no closing brace" % mod)


def nullary(d):
    return d.get("kind") == "def" and d.get("expr", {}).get("kind") != "lambda"


def candidates(parse_ir, mod):
    """nullary actions and vals/defs visible unqualified in MOD"""
    mods = {m["name"]: m for m in parse_ir["modules"]}
    if mod not in mods:
        die("module %s not in the parse output" % mod)
    visible = [mods[mod]]
    for d in mods[mod]["declarations"]:
        if d.get("kind") in ("import", "instance") and d.get("protoName") in mods:
            if d.get("kind") == "instance" and d.get("qualifiedName"):
                continue
            if d.get("kind") == "import" and d.get("defName") not in ("*", None):
                continue
            visible.append(mods[d["protoName"]])
    acts, vals = [], []
    for m in visible:
        for d in m["declarations"]:
            if not nullary(d):
                continue
            q = d.get("qualifier")
            if q == "action" and d["name"] not in acts:
                acts.append(d["name"])
            elif q in ("val", "def", "pureval", "puredef") and d["name"] not in vals:
                vals.append(d["name"])
    return acts, vals


def prepare(qnt, mod, quint, work, init_name, invs_arg, steps_arg):
    """copy the sources, add the step wrapper; return (file in work, compile flags)"""
    src_dir = os.path.dirname(os.path.abspath(qnt))
    wsrc = os.path.join(work, "src")
    os.makedirs(wsrc, exist_ok=True)
    for f in glob.glob(os.path.join(src_dir, "*.qnt")):
        with open(f) as a, open(os.path.join(wsrc, os.path.basename(f)), "w") as b:
            b.write(a.read())
    wq = os.path.join(wsrc, os.path.basename(qnt))
    pj = os.path.join(work, mod + ".parse.json")
    run(quint + ["parse", "--out=" + pj, wq], cwd=wsrc)
    acts, vals = candidates(json.load(open(pj)), mod)
    steps = steps_arg if steps_arg is not None else [a for a in acts if a != init_name]
    flags = ["--main=" + mod, "--init=" + init_name]
    if steps:
        text = open(wq).read()
        _, close = module_span(text, mod)
        text = text[:close] + "\n  action __ec_step = any { %s }\n" % ", ".join(steps) + text[close:]
        open(wq, "w").write(text)
        flags.append("--step=__ec_step")
    if invs_arg is not None:
        invs = invs_arg
    else:
        # keep the boolean ones: compile once with all of them and read their types
        tj = os.path.join(work, mod + ".types.json")
        run(quint + ["compile", "--target", "json"] + flags + ["--invariant=" + ",".join(vals), wq], tj, cwd=wsrc)
        ir = json.load(open(tj))
        types = ir.get("types", {})
        bools = set()
        for m in ir["modules"]:
            for d in m["declarations"]:
                if d.get("kind") == "def" and types.get(str(d["id"]), {}).get("type", {}).get("kind") == "bool":
                    bools.add(d["name"].split("::")[-1])
        invs = [v for v in vals if v in bools]
    if invs:
        flags.append("--invariant=" + ",".join(invs))
    return wq, wsrc, flags, steps, invs


# ---------------------------------------------------------------- main

def check_module(qnt, mod, quint, java, jar, work, roots_arg, quiet, init_name="init",
                 invs_arg=None, steps_arg=None):
    base = os.path.join(work, mod)
    wq, wsrc, flags, steps, invs = prepare(qnt, mod, quint, work, init_name, invs_arg, steps_arg)
    run(quint + ["compile", "--target", "json"] + flags + [wq], base + ".qnt.json", cwd=wsrc)
    run(quint + ["compile", "--target", "tlaplus"] + flags + [wq], base + ".tla", cwd=wsrc)
    # Apalache names the TLA+ module after the file; the .tla must be MODULE <mod>.tla
    want = apalache_parse(java, jar, base + ".qnt.json", base + ".intended.json", work)
    got = apalache_parse(java, jar, base + ".tla", base + ".parsed.json", work)
    W, G = top_decls(want), top_decls(got)
    nw, ng = {}, {}
    for name, d in W.items():
        n = Norm()
        nw[name] = (n.decl(d), n.lets)
    for name in W:
        if name in G:
            n = Norm()
            ng[name] = (n.decl(G[name]), n.lets)
    missing = sorted(set(W) - set(G))
    # call graph (intended) for reachability
    uses = {name: names_in(nw[name][0], set()) | set().union(*[names_in(v, set()) for v in nw[name][1].values()] or [set()])
            for name in nw}
    used = set().union(*uses.values()) if uses else set()

    def reach(r, seen):
        if r in seen or r not in uses:
            return seen
        seen.add(r)
        for u in uses[r]:
            reach(u, seen)
        return seen
    # definitions used only under init: the printer unprimes their assignments
    from_init = reach(init_name, set()) | reach("q_init", set())
    others = set()
    for r in nw:
        if r not in from_init and r not in used:
            reach(r, others)
    for name in from_init - others:
        b, lets = nw[name]
        nw[name] = (unprime(b), {k: unprime(v) for k, v in lets.items()})
    want_roots = roots_arg or (invs + steps + [init_name])
    roots = sorted(n for n in nw if any(n == r or n.endswith("_" + r) for r in want_roots))

    bad = {}
    for name in sorted(nw):
        if name not in ng:
            continue
        (wb, wl), (gb, gl) = nw[name], ng[name]
        problems = []
        if wb != gb:
            path, a, b = first_diff(wb[2], gb[2])
            lost = [c for c in conjuncts(wb[2]) if c not in conjuncts(gb[2])]
            problems.append({"where": "body", "path": "/".join(path), "intended": show(a), "parsed": show(b),
                             "lost_conjuncts": [show(c) for c in lost],
                             "n_conjuncts": (len(conjuncts(wb[2])), len(conjuncts(gb[2])))})
        for ln in sorted(set(wl) | set(gl)):
            if wl.get(ln) != gl.get(ln):
                if ln not in gl or ln not in wl:
                    # a LET that moved between definitions is not a meaning change unless
                    # its body differs; compare by name across the whole module below
                    continue
                d = first_diff(wl[ln][2], gl[ln][2])
                if d is None:
                    problems.append({"where": "LET " + ln + " (parameters)", "path": "",
                                     "intended": str(wl[ln][1]), "parsed": str(gl[ln][1])})
                else:
                    path, a, b = d
                    problems.append({"where": "LET " + ln, "path": "/".join(path), "intended": show(a), "parsed": show(b)})
        if problems:
            bad[name] = problems

    # who reaches the bad definitions
    affected_roots = {}
    for r in roots:
        hit = sorted(reach(r, set()) & set(bad))
        if hit:
            affected_roots[r] = hit

    print("== %s %s: %d definitions compared (%d actions as steps, %d invariants), %d differ%s" % (
        qnt, mod, len(ng), len(steps), len(invs), len(bad),
        (", %d missing from the TLA+" % len(missing)) if missing else ""))
    for name, probs in bad.items():
        print("  DIFFERS %s" % name)
        for p in probs:
            print("    in %s at %s" % (p["where"], p["path"] or "(top)"))
            print("      intended: %s" % clip(p["intended"]))
            print("      parsed:   %s" % clip(p["parsed"]))
            if p.get("lost_conjuncts") is not None:
                print("      top-level conjuncts intended/parsed: %d/%d; intended conjuncts not found: %d"
                      % (p["n_conjuncts"][0], p["n_conjuncts"][1], len(p["lost_conjuncts"])))
                for c in p["lost_conjuncts"][:6]:
                    print("        lost: %s" % clip(c, 240))
    if bad and not quiet:
        print("  roots reaching a differing definition:")
        for r, hit in affected_roots.items():
            print("    %s -> %s" % (r, ", ".join(hit)))
    if missing and not quiet:
        print("  (in the IR but not the TLA+: %s)" % ", ".join(missing[:20]))
    return bad


def main(argv):
    args = list(argv)
    quint = shlex.split(os.environ.get("QUINT", "quint"))
    java = shlex.split(os.environ.get("JAVA", "java"))
    jar = None
    keep = None
    roots = None
    quiet = False
    init_name = "init"
    invs_arg = steps_arg = None
    pos = []
    while args:
        a = args.pop(0)
        if a == "--quint":
            quint = shlex.split(args.pop(0))
        elif a == "--apalache-jar":
            jar = args.pop(0)
        elif a == "--java":
            java = shlex.split(args.pop(0))
        elif a == "--keep":
            keep = args.pop(0)
        elif a == "--roots":
            roots = [r for r in args.pop(0).split(",") if r]
        elif a == "--invariants":
            invs_arg = [x for x in args.pop(0).split(",") if x]
        elif a == "--steps":
            steps_arg = [x for x in args.pop(0).split(",") if x]
        elif a == "--init":
            init_name = args.pop(0)
        elif a == "--quiet":
            quiet = True
        elif a in ("-h", "--help"):
            print(__doc__)
            return 0
        else:
            pos.append(a)
    if len(pos) < 2:
        print(__doc__)
        return 2
    qnt, mods = pos[0], pos[1:]
    jar = find_jar(jar)
    if keep:
        os.makedirs(keep, exist_ok=True)
        work = os.path.abspath(keep)
    else:
        work = tempfile.mkdtemp(prefix="export_check_")
    any_bad = False
    for m in mods:
        os.makedirs(os.path.join(work, m), exist_ok=True)
        if check_module(qnt, m, quint, java, jar, os.path.join(work, m), roots, quiet, init_name,
                        invs_arg, steps_arg):
            any_bad = True
    print("export_check: %s" % ("FAIL (the TLA+ export changes the meaning of the definitions above)"
                               if any_bad else "OK (the TLA+ export matches the Quint IR)"))
    return 1 if any_bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
