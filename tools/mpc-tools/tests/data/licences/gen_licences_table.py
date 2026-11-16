#!/usr/bin/env python3
r"""Writes the pin tables that tests/licences.rs reads. Run by hand, never at test time:

    python3 -I tools/mpc-tools/tests/data/licences/gen_licences_table.py REPOS_DIR COPIES_FILE

REPOS_DIR holds the k2-mpc-* repos side by side. COPIES_FILE, kept outside the tree, names the
old copies of the licence rule, one per line in column order: `LABEL REPO SHA PATH` (REPO the
folder's name after `k2-mpc-`; the first line is the kit's, labelled `kit`). Every old rule is
read from its repo at that commit (git show), never from a working tree, and run as it is: the
old copies are the reference. The tables name the copies by label only. The random seed is
fixed: a second run writes the same bytes (compare them before committing).

expressions.tsv
    One row per licence expression: the expression (escaped: backslash, \n, \t, \r and
    other control characters as \xNN), then one cell per old copy, `SHIPPED,FUZZ`:
    `ok` or `no` as that copy judged the expression for a shipped package, and for
    libfuzzer-sys in a fuzz lock (`-` where the copy checks no fuzz lock; `died` where
    the old code raised).

metadata.tsv
    One row per `cargo metadata` fixture (metadata/<name>.json): the name, the old kit
    check's exit code, its flagged lines (`name version: licence`, joined by ` | `, or
    `-`), and the ruled difference: `-`, or `+LINE` for a package the new rule flags and
    the old kit check passed (the allow-list the kit rules now).
"""
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))


def copies(path):
    """LABEL -> (REPO, SHA, PATH), in the file's order (see the module doc)."""
    old = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.strip() and not line.startswith("#"):
                label, repo, sha, rule = line.split()
                old[label] = (repo, sha, rule)
    if list(old)[:1] != ["kit"]:
        sys.exit("the copies file's first line must be the kit's (label kit)")
    return old


def esc(s):
    out = []
    for c in s:
        if c == "\\":
            out.append("\\\\")
        elif c == "\n":
            out.append("\\n")
        elif c == "\t":
            out.append("\\t")
        elif c == "\r":
            out.append("\\r")
        elif ord(c) < 0x20 or c == "\x7f":
            out.append("\\x%02x" % ord(c))
        else:
            out.append(c)
    return "".join(out)


def blob(repos, repo, sha, path):
    return subprocess.run(["git", "-C", os.path.join(repos, "k2-mpc-" + repo), "show", sha + ":" + path],
                          check=True, capture_output=True, text=True).stdout


def shell_rule(text):
    """The copy's `ok` function and ALLOWED set, exec'd as written."""
    m = re.search(r"^ALLOWED = \{[^}]*\}", text, re.M)
    allowed_src = m.group(0)
    lines = text.split("\n")
    start = next(i for i, l in enumerate(lines) if l.startswith("def ok("))
    end = next(i for i in range(start, len(lines)) if lines[i].lstrip().startswith("return all("))
    ns = {"re": re}
    exec(allowed_src + "\n" + "\n".join(lines[start:end + 1]) + "\n", ns)
    ok, allowed = ns["ok"], ns["ALLOWED"]
    two = "allowed" in lines[start]
    fuzz = None
    if "libfuzzer-sys" in text:
        if 'TEST_ONLY = {"libfuzzer-sys"' in text:  # csv: the exact licence string only
            exact = re.search(r'TEST_ONLY = \{"libfuzzer-sys": "([^"]*)"\}', text).group(1)
            fuzz = lambda e: ok(e) or e == exact
        else:  # NCSA added to the list for libfuzzer-sys in the fuzz lock
            fuzz = lambda e: ok(e, allowed | {"NCSA"})
    ship = (lambda e: ok(e, allowed)) if two else ok
    return ship, fuzz


def gate_rule(text, repo):
    ns = {"__name__": "gate_" + repo}
    exec(compile(text, "licence-gate.py", "exec"), ns)
    ok, shipped, test_only = ns["ok"], ns["SHIPPED"], ns["TEST_ONLY"]
    fuzz = (lambda e: ok(e, test_only)) if "--test-only" in text else None
    return (lambda e: ok(e, shipped)), fuzz


def verdict(f, e):
    if f is None:
        return "-"
    try:
        return "ok" if f(e) else "no"
    except Exception:
        return "died"


ATOMS = ["MIT", "Apache-2.0", "ISC", "Zlib", "BSD-2-Clause", "BSD-3-Clause", "Unicode-3.0",
         "Unlicense", "NCSA", "GPL-3.0", "MPL-2.0", "CC0-1.0", "BSL-1.0", "0BSD",
         "Apache-2.0 WITH LLVM-exception", "GPL-2.0 WITH Classpath-exception-2.0",
         "Unicode-DFS-2016", "LGPL-2.1-or-later", "mit", "MIT+"]

FIXED = [
    "MIT", "Apache-2.0", "MIT OR Apache-2.0", "Apache-2.0 OR MIT", "MIT/Apache-2.0",
    "Unlicense OR MIT", "Unlicense/MIT", "MIT OR Unlicense", "Unlicense",
    "(MIT OR Apache-2.0) AND Unicode-3.0", "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    "Apache-2.0 WITH LLVM-exception", "Apache-2.0 AND ISC", "0BSD OR MIT OR Apache-2.0",
    "Apache-2.0 OR BSL-1.0", "MPL-2.0", "(MIT OR Apache-2.0) AND NCSA", "NCSA",
    "BSD-2-Clause OR Apache-2.0 OR MIT", "Zlib OR Apache-2.0 OR MIT", "CC0-1.0",
    "MIT AND (Apache-2.0 OR GPL-3.0)", "(Apache-2.0 OR GPL-3.0) AND MIT", "", "NONE", "(MIT)",
    "((MIT))", "MIT OR", "OR MIT", "( MIT OR Apache-2.0 )", "MIT AND", "AND", "()", "(",
    ")", "MIT)", "(MIT", "Apache-2.0 WITH", "WITH", "MIT WITH LLVM-exception",
    "Apache-2.0 WITH Classpath-exception-2.0", "GPL-2.0 WITH LLVM-exception",
    "MIT AND Apache-2.0 AND ISC", "MIT OR GPL-3.0 AND Apache-2.0", "GPL-3.0 OR MIT AND MPL-2.0",
    "(MIT AND GPL-3.0) OR Apache-2.0", "MIT  OR  Apache-2.0", "MIT\tOR\tApache-2.0",
    " MIT ", "MIT or Apache-2.0", "mit", "Apache-2.0/MIT/ISC", "MIT/GPL-3.0", "GPL-3.0/MIT",
    "Unicode-3.0 AND (MIT OR Apache-2.0)", "(MIT OR Apache-2.0) AND (Unicode-3.0 OR MPL-2.0)",
    "(MIT OR Apache-2.0) AND (MPL-2.0 OR GPL-3.0)", "Unlicense AND MIT", "MIT AND Unlicense",
    "(Unlicense OR MIT) AND NCSA", "Apache-2.0 WITH LLVM-exception AND MIT",
    "BSD-3-Clause AND Zlib", "Unicode-DFS-2016", "MIT OR Apache-2.0 OR Zlib",
    "(MIT OR Apache-2.0) AND BSD-3-Clause AND Unicode-3.0",
]


def gen_expr(rng, depth):
    if depth == 0 or rng.random() < 0.35:
        return rng.choice(ATOMS)
    op = rng.choice(["OR", "AND"])
    n = rng.randint(2, 3)
    parts = []
    for _ in range(n):
        p = gen_expr(rng, depth - 1)
        if (" OR " in p or " AND " in p) and rng.random() < 0.7:
            p = "(" + p + ")"
        parts.append(p)
    return (" %s " % op).join(parts)


def mangle(rng, e):
    r = rng.random()
    if r < 0.06:
        return e.replace(" OR ", "/", 1)
    if r < 0.10:
        return "(" + e + ")"
    if r < 0.13:
        toks = e.split(" ")
        del toks[rng.randrange(len(toks))]
        return " ".join(toks)
    if r < 0.15:
        return e.replace("(", "", 1)
    return e


def expressions():
    rng = random.Random(20261116)
    seen, out = set(), []
    for e in FIXED:
        if e not in seen:
            seen.add(e)
            out.append(e)
    while len(out) < 700:
        e = mangle(rng, gen_expr(rng, 3))
        if e not in seen:
            seen.add(e)
            out.append(e)
    return out


def pkg(name, version, lic, source="registry+https://github.com/rust-lang/crates.io-index",
        manifest=None):
    return {"name": name, "version": version, "license": lic, "source": source,
            "manifest_path": manifest or "/r/%s-%s/Cargo.toml" % (name, version)}


FIXTURES = {
    # name -> (packages, ruled difference)
    "all-allowed": ([pkg("serde", "1.0.0", "MIT OR Apache-2.0"), pkg("adler2", "2.0.0", "0BSD OR MIT OR Apache-2.0"),
                     pkg("unicode-ident", "1.0.0", "(MIT OR Apache-2.0) AND Unicode-3.0"),
                     pkg("ryu", "1.0.0", "Apache-2.0 OR BSL-1.0")], "-"),
    "gpl-dep": ([pkg("serde", "1.0.0", "MIT OR Apache-2.0"), pkg("readline", "0.1.0", "GPL-3.0")], "-"),
    "no-licence": ([pkg("serde", "1.0.0", "MIT OR Apache-2.0"), pkg("mystery", "0.2.0", None)], "-"),
    "ours-path": ([pkg("k2-mpc-base", "0.18.0", "MIT OR Apache-2.0", None, "/w/crates/base/Cargo.toml"),
                   pkg("mpce-model", "0.1.0", "MIT OR Apache-2.0", None, "/w/vendor/mpce-model/Cargo.toml"),
                   pkg("serde", "1.0.0", "MIT OR Apache-2.0")], "-"),
    "vendored-yrs": ([pkg("yrs", "0.21.0", "MIT", None, "/w/vendor/yrs/Cargo.toml"),
                      pkg("serde", "1.0.0", "MIT OR Apache-2.0")], "-"),
    "unlicense-alone": ([pkg("serde", "1.0.0", "MIT OR Apache-2.0"), pkg("tiny", "0.1.0", "Unlicense")],
                        "+tiny 0.1.0: Unlicense"),
    "unlicense-or-mit": ([pkg("memchr", "2.7.0", "Unlicense OR MIT"), pkg("aho-corasick", "1.1.0", "Unlicense/MIT")], "-"),
    "llvm-exception": ([pkg("wasmtime-x", "1.0.0", "Apache-2.0 WITH LLVM-exception"),
                        pkg("target-lexicon", "0.12.0", "Apache-2.0 WITH LLVM-exception")], "-"),
    "ncsa-in-main": ([pkg("serde", "1.0.0", "MIT OR Apache-2.0"),
                      pkg("libfuzzer-sys", "0.4.10", "(MIT OR Apache-2.0) AND NCSA")], "-"),
    "mpl-and-cc0": ([pkg("option-ext", "0.2.0", "MPL-2.0"), pkg("tiny-keccak", "2.0.2", "CC0-1.0"),
                     pkg("serde", "1.0.0", "MIT OR Apache-2.0")], "-"),
}


def write_fake_cargo(d, fixture):
    path = os.path.join(d, "cargo")
    with open(path, "w") as f:
        f.write("#!/bin/sh\ncat '%s'\n" % fixture)
    os.chmod(path, 0o755)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    repos = sys.argv[1]
    old = copies(sys.argv[2])
    labels = list(old)
    rules = {}
    texts = {}
    for label, (repo, sha, path) in old.items():
        text = blob(repos, repo, sha, path)
        texts[label] = text
        rules[label] = gate_rule(text, label) if path.endswith(".py") else shell_rule(text)

    rows = ["# expression\t" + "\t".join(labels),
            "# generated by gen_licences_table.py from %d copies of the licence rule, each read from its repo "
            "at a commit (the copies file, kept outside the tree): " % len(labels)
            + ", ".join("%s %s" % (l, p) for l, (_, _, p) in old.items())]
    for e in expressions():
        cells = ["%s,%s" % (verdict(rules[r][0], e), verdict(rules[r][1], e)) for r in labels]
        rows.append(esc(e) + "\t" + "\t".join(cells))
    with open(os.path.join(HERE, "expressions.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(rows) + "\n")

    # the old kit check, whole, on each fixture: its own script with `cargo metadata` faked
    md = os.path.join(HERE, "metadata")
    os.makedirs(md, exist_ok=True)
    rows = ["# fixture\told exit\told flagged\truled difference",
            "# generated by gen_licences_table.py: the kit's scripts/check-licences.sh before the tools binary "
            "(the copies file's kit row), run whole"]
    with tempfile.TemporaryDirectory() as t:
        os.makedirs(os.path.join(t, "kit", "scripts"))
        script = os.path.join(t, "kit", "scripts", "check-licences.sh")
        with open(script, "w") as f:
            f.write(texts["kit"])
        for name in sorted(FIXTURES):
            pkgs, ruled = FIXTURES[name]
            fx = os.path.join(md, name + ".json")
            with open(fx, "w", encoding="utf-8", newline="\n") as f:
                json.dump({"packages": pkgs, "version": 1}, f, indent=1, sort_keys=True)
                f.write("\n")
            bindir = os.path.join(t, "bin-" + name)
            os.makedirs(bindir)
            write_fake_cargo(bindir, fx)
            env = {"PATH": bindir + ":/usr/bin:/bin:" + os.path.dirname(shutil.which("python3")), "LC_ALL": "C.UTF-8"}
            p = subprocess.run(["sh", script], capture_output=True, text=True, env=env)
            flagged = [l for l in p.stdout.split("\n")[1:] if l] if p.returncode == 1 else []
            rows.append("\t".join([name, str(p.returncode), " | ".join(flagged) or "-", ruled]))
    with open(os.path.join(HERE, "metadata.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(rows) + "\n")


if __name__ == "__main__":
    main()
