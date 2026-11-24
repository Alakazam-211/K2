#!/usr/bin/env python3
"""The output judges: what a brand's build prints names that brand only.

    python3 scripts/brand/output-judges.py --brand brand/xy CORPUS
    python3 scripts/brand/output-judges.py --brand brand/xy --write-debt CORPUS

CORPUS is a folder of what the brand's build printed (one file per command
line, as scripts/brand/collect-outputs.sh writes them). Every file is read
as text, line by line. Five judges, each on its own:

  a  the CLI name rule (scripts/garden/cli-name-flip.py's, the rewrite's
     own): no line uses another name as the command. "Another name" is
     k2, k5, and every command and alias of the other brands under brand/.
     No allow list: one use fails.
  b  a crude word scan that does not share the rule's eyes: every bare
     word k2 or k5 (or another brand's command). A line passes when the
     reviewed patterns of the keep file (`keep` rows) take out every such
     word, or when the line is a `debt` row for judge b. A new hit fails.
  c  each name in its place (a brand has two names, the product's for
     prose and screens and the command's for anything typed, and neither
     is made from the other):
       - the command in another case (XY, Xy for the command xy) is
         printed nowhere, unless that is the product's own spelling;
       - the product is never where a command belongs: before a flag or a
         placeholder, inside backticks, after `Usage:`, `run` or `$`;
       - the command is only where a command belongs: every bare use of
         it reads as the command by the CLI name rule, or is the program's
         name before its version (`xy 0.46.17`).
     A hit fails unless it is a `debt` row for judge c.
  d  no slot is left unfilled: no line holds `{cli}` or `{product}` (rebrand
     B7, trap 2), nor a page's `{company}`, `{login}`, `{contact_email}` or
     `{repo}` (the build fills those: crates/k5-cli/build.rs). A template is filled by the CLI that
     prints it, or by the server where it is made; a slot that reaches a
     person or an agent is a command nobody can type, or a name nobody
     reads. No allow list, no debt: one hit fails.
  e  no other brand's sign-in name: the account people sign in with is the
     brand's own (`names.login`: K2 Connect in ours, Globex Account in xy),
     so another brand's login name (every brand file's `names.login` but
     this one's) is printed nowhere. The pages and the --schema entries
     name it by the {login} slot, so no row is left: a hit fails unless
     it is a `debt` row for judge e.

Debt rows are lines found before the judges existed, each to be fixed and
then dropped. A debt row that matches nothing any more fails (stale): the
list only falls. --write-debt prints the debt rows for every hit of b and
c, for a person to read before committing them; it still exits 1 when
there are hits.

The keep file (default scripts/brand/output-keep.txt) is tab-separated:
    keep<TAB><python regex><TAB><who reads it, why it stays>
    debt<TAB><b|c><TAB><the line, exactly>
A debt row is the line's text, wherever in the corpus it is printed (many
command lines print the same page).
"""
import importlib.util
import os
import re
import sys

try:
    import tomllib
except ImportError:  # Python before 3.11
    sys.exit("output-judges: needs Python 3.11 or newer (tomllib)")

HERE = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
OURS = ("k2", "k5")


def load_flip():
    spec = importlib.util.spec_from_file_location("cli_name_flip", os.path.join(HERE, "scripts", "garden", "cli-name-flip.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def read_brand(folder):
    with open(os.path.join(folder, "brand.toml"), "rb") as f:
        doc = tomllib.load(f)
    cli = doc["cli"]
    return {
        "id": doc["id"],
        "product": doc["names"]["product"],
        "name": cli["name"],
        "aliases": [a["name"] for a in cli.get("aliases", [])],
        "login": doc["names"].get("login", ""),
    }


def other_names(brand, root):
    """The command names this brand's outputs must not print: ours, every
    other brand's command and aliases, and this brand's own aliases (an
    alias is only ever a name it answers to, never one it teaches)."""
    names = set(OURS)
    top = os.path.join(root, "brand")
    for d in sorted(os.listdir(top)):
        if os.path.isfile(os.path.join(top, d, "brand.toml")):
            b = read_brand(os.path.join(top, d))
            names.add(b["name"])
            names.update(b["aliases"])
    names.update(brand["aliases"])
    names.discard(brand["name"])
    return sorted(names)


def other_logins(brand, root):
    """The sign-in names of every other brand file under brand/."""
    out = set()
    top = os.path.join(root, "brand")
    for d in sorted(os.listdir(top)):
        if os.path.isfile(os.path.join(top, d, "brand.toml")):
            login = read_brand(os.path.join(top, d))["login"]
            if login:
                out.add(login)
    out.discard(brand["login"])
    return sorted(out)


def word_re(name):
    """The CLI name rule's word for `name` (the flip's WORD)."""
    return re.compile(r"(?<![\w./$~-])" + re.escape(name) + r"(?![\w.:/-])")


def bare_re(names):
    return re.compile(r"(?<![A-Za-z0-9_])(" + "|".join(re.escape(n) for n in names) + r")(?![A-Za-z0-9_])")


def read_keep(path):
    keeps, debts = [], {}
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f.read().split("\n"), 1):
            if not line or line.startswith("#"):
                continue
            cols = line.split("\t")
            if cols[0] == "keep" and len(cols) == 3 and cols[2].strip():
                keeps.append(re.compile(cols[1]))
            elif cols[0] == "debt" and len(cols) == 3 and cols[1] in ("b", "c", "e"):
                debts[(cols[1], cols[2])] = n
            else:
                sys.exit(f"output-judges: {path}:{n}: not a keep or debt row: {line!r}")
    return keeps, debts


def corpus_files(corpus):
    out = []
    for dirpath, dirnames, filenames in os.walk(corpus):
        dirnames.sort()
        for name in sorted(filenames):
            p = os.path.join(dirpath, name)
            out.append((os.path.relpath(p, corpus).replace(os.sep, "/"), p))
    return out


# The slots a template carries (k2_core::brand::CLI_SLOT, PRODUCT_SLOT), and
# the brand slots of the CLI's pages (crates/k5-cli/src/brand_text.rs).
SLOTS = ("{cli}", "{product}", "{company}", "{login}", "{contact_email}", "{repo}")
COMMAND_LEAD = re.compile(r"(^\s*|(Usage:|usage:|\brun|\bRun|\btry|\bTry|\$)\s+)[`'\"]?$")
FLAG_AFTER = re.compile(r" (--?[a-z]|<|\[|…|\.\.\.)")


def judge(brand, corpus, keep_path, flip):
    known = flip.verbs(HERE)
    others = other_names(brand, HERE)
    words = [(n, word_re(n)) for n in others]
    crude = bare_re(others)
    own = word_re(brand["name"])
    variants = re.compile(r"(?<![A-Za-z0-9_])(?i:" + re.escape(brand["name"]) + r")(?![A-Za-z0-9_])")
    product = re.compile(r"(?<![\w])" + re.escape(brand["product"]) + r"(?![\w])")
    keeps, debts = read_keep(keep_path)
    seen = set()
    hits = {"a": [], "b": [], "c": [], "d": [], "e": []}
    logins = [re.compile(r"(?<![\w])" + re.escape(n) + r"(?![\w])") for n in other_logins(brand, HERE)]
    files = corpus_files(corpus)
    if not files:
        sys.exit(f"output-judges: {corpus} holds no files")
    for rel, path in files:
        with open(path, encoding="utf-8", errors="replace") as f:
            lines = f.read().split("\n")
        for n, line in enumerate(lines, 1):
            at = f"{rel}:{n}"
            # a: another name, read as the command by the rule.
            for name, rx in words:
                for m in rx.finditer(line):
                    cls, why = flip.classify(line, m, "heredoc-quoted", known)
                    if cls == "text":
                        hits["a"].append(f"{at}: {name} as the command: {line.strip()}")
            # b: any bare other name the keep patterns do not take out.
            rest = line
            for k in keeps:
                rest = k.sub(" ", rest)
            if crude.search(rest):
                key = ("b", line)
                if key in debts:
                    seen.add(key)
                else:
                    hits["b"].append((rel, line, f"{at}: {line.strip()}"))
            # d: an unfilled slot.
            left = [slot for slot in SLOTS if slot in line]
            if left:
                hits["d"].append(f"{at}: the slot {' and '.join(left)} is not filled: {line.strip()}")
            # e: another brand's sign-in name.
            if any(rx.search(line) for rx in logins):
                key = ("e", line)
                if key in debts:
                    seen.add(key)
                else:
                    hits["e"].append((rel, line, f"{at}: another brand's sign-in name: {line.strip()}"))
            # c: the names in their places.
            why = None
            for m in variants.finditer(line):
                if m.group(0) != brand["name"] and m.group(0) != brand["product"]:
                    why = f"the command in another case ({m.group(0)})"
            for m in product.finditer(line):
                before, after = line[:m.start()], line[m.end():]
                if FLAG_AFTER.match(after) or (before.endswith("`") and after.startswith("`")):
                    why = "the product where a command belongs"
                w = flip.NEXT_WORD.match(after)
                if w and w.group(1) in known and COMMAND_LEAD.search(before):
                    why = "the product where a command belongs"
            for m in own.finditer(line):
                cls, _ = flip.classify(line, m, "heredoc-quoted", known)
                if cls != "text" and not re.match(r" \d", line[m.end():]):
                    why = "the command where the product (or a word) belongs"
            if why:
                key = ("c", line)
                if key in debts:
                    seen.add(key)
                else:
                    hits["c"].append((rel, line, f"{at}: {why}: {line.strip()}"))
    stale = [k for k in debts if k not in seen]
    return hits, stale, debts


def main(argv):
    brand_dir, corpus, keep, write = None, None, os.path.join(HERE, "scripts", "brand", "output-keep.txt"), False
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--brand" and args:
            brand_dir = args.pop(0)
        elif a == "--keep" and args:
            keep = args.pop(0)
        elif a == "--write-debt":
            write = True
        elif not a.startswith("-") and corpus is None:
            corpus = a
        else:
            corpus = None
            brand_dir = None
            break
    if not brand_dir or not corpus or not os.path.isdir(corpus):
        sys.exit("usage: output-judges.py --brand <brand folder> [--keep FILE] [--write-debt] <corpus folder>")
    brand = read_brand(brand_dir)
    hits, stale, debts = judge(brand, corpus, keep, load_flip())
    if write:
        rows = []
        for j in ("b", "c", "e"):
            for _, line, _ in hits[j]:
                if (j, line) not in rows:
                    rows.append((j, line))
        for j, line in rows:
            print(f"debt\t{j}\t{line}")
    failed = []
    for j, title in (("a", "another name read as the command"), ("b", "a bare other name"), ("c", "a name out of its place"), ("d", "a slot not filled"), ("e", "another brand's sign-in name")):
        found = [h if isinstance(h, str) else h[2] for h in hits[j]]
        print(f"judge {j} ({title}): {len(found)} hit(s)", file=sys.stderr)
        for h in found[:50]:
            print(f"  {h}", file=sys.stderr)
        if found:
            failed.append(j)
    for k in stale:
        print(f"  stale debt row ({keep}:{debts[k]}): judge {k[0]}: {k[1].strip()}", file=sys.stderr)
    if stale:
        failed.append("stale")
    if failed:
        print(f"output-judges: {brand['id']}: FAILED ({', '.join(failed)})", file=sys.stderr)
        return 1
    print(f"output-judges: {brand['id']}: the five judges pass on {len(corpus_files(corpus))} files")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
