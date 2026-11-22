#!/usr/bin/env python3
"""The name policy for a brand: its command, its aliases, its id and its
product name, each checked on its own rules.

    bash scripts/brand.sh lint <brand folder> [--live] [--registry <file>]
    bash scripts/brand.sh lint --name <command> [--product <name>] [--registry <file>]

A brand has two names that are never made from each other: the product's
(prose and screens) and the command's (anything a person or agent types).

The command, each alias and the brand id:
  shape    ^[a-z][a-z0-9]{1,11}$: lowercase letters and digits, a letter
           first, 2 to 12 long (Windows ignores case; our helpers are
           <name>-<part>; the env prefix is <NAME>_)
  ours     k2, k5, k2so, kessel, k0-k9, ks, kz, or a name starting k2 or
           k5: our names, their look-alikes, and every <name>-<part>
           helper of ours
  hard     on the hard list of brand/reserved-names.txt
  unique   another brand we hold has it as an id, a command or an alias
  registry a brand in the registry (--registry) has it as its command or
           its id
  text     a bare word of our own text (the default build's outputs:
           cli/k2, cli/text, the goldens, the CLI catalog): a word rule
           could not rewrite or check it, and an agent could not read it
  soft     on the soft list (a package index, or an env prefix other
           tools read), unless signed off (below)
The product:
  product  empty, longer than 40, a control character, or space at an end
  ours     K2, K5 or Kessel, as the product or a word of it
  unique   another brand we hold has it as its product (any case)
  registry a brand in the registry has it as its product (any case)
  text     the product, whole, is in our own text
Ours (brand/k2, brand/k5 in this repo) are checked for shape only.

A soft collision passes when the brand folder holds collision-ok.toml with
both sign-offs for that name (K2's ruling 6):

    [[ok]]
    name = "<the name>"
    k2 = "<K2's sign-off: where and when>"
    steward = "<the Kessel Steward's sign-off: where and when>"
    why = "<why the collision is fine>"

--live adds the checks a written list can't make, on this build host:
  host     the name resolves in bash (command -v, type -a), in zsh
           (whence -a) when zsh is here, and on Windows in PowerShell
           (Get-Command), pwsh and where.exe: anything found is refused
  package  Homebrew's formula names and aliases, Debian's file index
           (usr/bin, bin, usr/sbin), Arch's pkgfile and winget when they
           are here: a hit is a soft collision (it needs the sign-off)
The live checks fail loudly when a source can't be read: a name is never
called free without its check. Their sources can be pointed elsewhere
(BRAND_LINT_BREW_NAMES, BRAND_LINT_BREW_ALIASES, BRAND_LINT_DEBIAN_CONTENTS:
a URL, file:// too); the tests use that, and are offline.

--registry <file> also checks the names against the brands we have
reserved for distributors, whose brand files we never hold. The file is
private (it never goes in this repo) and only our onboarding passes it.
One brand per line, tab-separated:

    <command><TAB><product><TAB><brand id>

Blank lines and lines starting with # are skipped; any other line that is
not three fields, or a command or id not of the command's shape, stops the
lint. The row whose brand id is the brand being linted is that brand's own
reservation and is not a collision. A missing file stops the lint: a name
is never called free without its check.

--name checks one command name (and --product one product) against the
brands this repo holds and prints `<name><TAB><answer>`: the first rule it
breaks in the order above, or `ok`. Exit 0 when nothing is refused, 1 when
something is, 2 for a usage error.
"""
import gzip
import io
import os
import re
import shutil
import subprocess
import sys
import urllib.request

try:
    import tomllib
except ImportError:  # Python before 3.11
    sys.exit("brand lint: needs Python 3.11 or newer (tomllib)")

HERE = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
SHAPE = re.compile(r"[a-z][a-z0-9]{1,11}")
OURS = {"k2", "k5", "k2so", "kessel", "ks", "kz"} | {f"k{d}" for d in range(10)}
OUR_IDS = {"k2", "k5"}
OUR_PRODUCT_WORDS = ("K2", "K5", "Kessel")
# Our helper programs, <command>-<part>: a brand's <name>-<part> may not be one.
OUR_HELPERS = (
    "k2-daemon", "k2-ptyd", "k2-root-helper", "k2-node", "k2-app-engine", "k2-approve",
    "k2-export", "k2-deskd", "k2-deckrender", "k2-scenerender", "k2-signer", "k2-open",
    "k2-pg-helper", "k2-mail-helper", "k5-cli",
)
# The default build's outputs: what our own text prints.
TEXT_PLACES = ("cli/k2", "cli/text", "tests/golden", "src/shared/cli-catalog.generated.json")
BREW_NAMES = "https://formulae.brew.sh/api/formula_names.txt"
BREW_ALIASES = "https://formulae.brew.sh/api/formula_aliases.txt"
DEBIAN_CONTENTS = "https://deb.debian.org/debian/dists/stable/main/Contents-amd64.gz"


def reserved():
    hard, soft = {}, {}
    with open(os.path.join(HERE, "brand", "reserved-names.txt"), encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) != 3 or cols[1] not in ("hard", "soft"):
                sys.exit(f"brand lint: brand/reserved-names.txt:{n}: not <name> <hard|soft> <sources>: {line!r}")
            (hard if cols[1] == "hard" else soft)[cols[0]] = cols[2]
    return hard, soft


def read_brand(folder):
    path = os.path.join(folder, "brand.toml")
    with open(path, "rb") as f:
        doc = tomllib.load(f)
    try:
        return {
            "folder": os.path.realpath(folder),
            "id": doc["id"],
            "product": doc["names"]["product"],
            "name": doc["cli"]["name"],
            "aliases": [a["name"] for a in doc["cli"].get("aliases", [])],
        }
    except (KeyError, TypeError) as e:
        sys.exit(f"brand lint: {path}: no {e}")


def held():
    """The brands this repo holds."""
    out = []
    top = os.path.join(HERE, "brand")
    for d in sorted(os.listdir(top)):
        if os.path.isfile(os.path.join(top, d, "brand.toml")):
            out.append(read_brand(os.path.join(top, d)))
    return out


def is_ours(brand):
    return brand["id"] in OUR_IDS and brand["folder"] == os.path.realpath(os.path.join(HERE, "brand", brand["id"]))


def signoffs(folder):
    path = os.path.join(folder, "collision-ok.toml")
    if not os.path.isfile(path):
        return {}
    with open(path, "rb") as f:
        doc = tomllib.load(f)
    ok = {}
    for row in doc.get("ok", []):
        if all(isinstance(row.get(k), str) and row[k].strip() for k in ("name", "k2", "steward", "why")):
            ok[row["name"]] = row
        else:
            sys.exit(f"brand lint: {path}: every [[ok]] needs name, k2, steward and why")
    return ok


_text_cache = {}


def text_hits(word, case):
    """Lines of our own text where `word` stands as a word: (count, first)."""
    key = (word, case)
    if key in _text_cache:
        return _text_cache[key]
    rx = re.compile(r"(?<![A-Za-z0-9_])" + re.escape(word) + r"(?![A-Za-z0-9_])", 0 if case else re.I)
    count, first = 0, None
    for place in TEXT_PLACES:
        top = os.path.join(HERE, place)
        files = [top] if os.path.isfile(top) else []
        for dirpath, dirnames, filenames in os.walk(top):
            dirnames.sort()
            files += [os.path.join(dirpath, n) for n in sorted(filenames)]
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as f:
                for n, line in enumerate(f, 1):
                    if rx.search(line):
                        count += 1
                        if first is None:
                            first = f"{os.path.relpath(path, HERE)}:{n}"
    _text_cache[key] = (count, first)
    return count, first


def read_registry(path):
    """The reserved brands: [{"name", "product", "id"}] from a private file."""
    rows = []
    try:
        f = open(path, encoding="utf-8")
    except OSError as e:
        sys.exit(f"brand lint: can't read the registry {path} ({e.strerror}): the registry check did not run")
    with f:
        for n, line in enumerate(f.read().split("\n"), 1):
            if not line.strip() or line.startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) != 3 or not all(c.strip() for c in cols):
                sys.exit(f"brand lint: {path}:{n}: not <command> <product> <brand id>, tab-separated")
            name, product, bid = (c.strip() for c in cols)
            if not SHAPE.fullmatch(name) or not SHAPE.fullmatch(bid):
                sys.exit(f"brand lint: {path}:{n}: the command and the id must match ^[a-z][a-z0-9]{{1,11}}$")
            rows.append({"name": name, "product": product, "id": bid})
    return rows


def registry_findings(names, product, own_id, registry):
    """(what, value, rule, why) for each name or the product a reserved
    brand other than `own_id` already has."""
    out = []
    for r in registry:
        if r["id"] == own_id:
            continue
        for what, name in names:
            for kind in ("name", "id"):
                if name == r[kind]:
                    shown = "command" if kind == "name" else "id"
                    out.append((what, name, "registry", f"the reserved brand {r['id']} has it as its {shown}"))
        if product is not None and product.casefold() == r["product"].casefold():
            out.append(("product", product, "registry", f"the reserved brand {r['id']} has it as its product"))
    return out


def name_findings(name, what, others, hard, soft, ok):
    """(rule, why) for every rule `name` breaks, in the order of the header."""
    out = []
    if not SHAPE.fullmatch(name):
        return [("shape", f"{name!r} must match ^[a-z][a-z0-9]{{1,11}}$")]
    if name in OURS or name.startswith(("k2", "k5")):
        out.append(("ours", "one of our names or a look-alike"))
    elif any(h.startswith(name + "-") for h in OUR_HELPERS):
        out.append(("ours", "its helpers would be named like ours"))
    if name in hard:
        out.append(("hard", hard[name]))
    for b in others:
        for kind, value in (("id", b["id"]), ("command", b["name"])) + tuple(("alias", a) for a in b["aliases"]):
            if value == name:
                out.append(("unique", f"brand {b['id']} has it as its {kind}"))
    count, first = text_hits(name, True)
    if count:
        out.append(("text", f"a word in {count} line(s) of our own text, first {first}"))
    if name in soft:
        if name in ok:
            pass
        else:
            out.append(("soft", f"{soft[name]}: needs K2's and the Kessel Steward's sign-off in collision-ok.toml"))
    return out


def product_findings(product, others):
    out = []
    if not product or len(product) > 40 or product != product.strip() or any(ord(c) < 32 or ord(c) == 127 for c in product):
        return [("product", "1 to 40 characters, no control character, no space at either end")]
    for word in OUR_PRODUCT_WORDS:
        if re.search(r"(?<![A-Za-z0-9_])" + re.escape(word) + r"(?![A-Za-z0-9_])", product, re.I):
            out.append(("ours", f"has our product word {word}"))
    for b in others:
        if b["product"].casefold() == product.casefold():
            out.append(("unique", f"brand {b['id']} has it as its product"))
    count, first = text_hits(product, True)
    if count:
        out.append(("text", f"in {count} line(s) of our own text, first {first}"))
    return out


# ── the live checks ────────────────────────────────────────────────────

def fetch(env, default):
    url = os.environ.get(env) or default
    try:
        with urllib.request.urlopen(url, timeout=120) as r:
            data = r.read()
    except Exception as e:  # any failure: the check did not run
        raise SystemExit(f"brand lint: can't read {url} ({e}): the package check did not run")
    if url.endswith(".gz"):
        data = gzip.decompress(data)
    return data.decode("utf-8", errors="replace")


def host_findings(name):
    out = []
    r = subprocess.run(["bash", "-c", 'command -v -- "$1"; type -a -- "$1"', "bash", name], capture_output=True, text=True)
    if r.stdout.strip():
        out.append(("host", f"bash finds it: {r.stdout.strip().splitlines()[0]}"))
    if shutil.which("zsh"):
        r = subprocess.run(["zsh", "-f", "-c", 'whence -a -- "$1"', "zsh", name], capture_output=True, text=True)
        if r.stdout.strip():
            out.append(("host", f"zsh finds it: {r.stdout.strip().splitlines()[0]}"))
    for shell in ("powershell.exe", "pwsh"):
        if shutil.which(shell):
            r = subprocess.run([shell, "-NoProfile", "-Command", f"Get-Command -All -Name '{name}' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name"], capture_output=True, text=True)
            if r.stdout.strip():
                out.append(("host", f"{shell} finds it: {r.stdout.strip().splitlines()[0]}"))
    if shutil.which("where.exe"):
        r = subprocess.run(["where.exe", name], capture_output=True, text=True)
        if r.returncode == 0 and r.stdout.strip():
            out.append(("host", f"where.exe finds it: {r.stdout.strip().splitlines()[0]}"))
    return out


_index = {}


def package_findings(name, ok):
    if "brew" not in _index:
        _index["brew"] = set(fetch("BRAND_LINT_BREW_NAMES", BREW_NAMES).split())
        aliases = set()
        for line in fetch("BRAND_LINT_BREW_ALIASES", BREW_ALIASES).splitlines():
            aliases.update(w for w in re.split(r"[\s|:]+", line) if w)
        _index["brew_aliases"] = aliases
        names = set()
        for line in fetch("BRAND_LINT_DEBIAN_CONTENTS", DEBIAN_CONTENTS).splitlines():
            path = line.split()[0] if line.split() else ""
            for prefix in ("usr/bin/", "bin/", "usr/sbin/"):
                if path.startswith(prefix) and "/" not in path[len(prefix):]:
                    names.add(path[len(prefix):])
        _index["debian"] = names
    hits = []
    if name in _index["brew"] or name in _index["brew_aliases"]:
        hits.append("a Homebrew formula or alias")
    if name in _index["debian"]:
        hits.append("a Debian package's program")
    if shutil.which("pkgfile"):
        r = subprocess.run(["pkgfile", "-b", name], capture_output=True, text=True)
        if r.stdout.strip():
            hits.append(f"an Arch package's program ({r.stdout.strip().splitlines()[0]})")
    if shutil.which("winget"):
        r = subprocess.run(["winget", "search", "--exact", "--name", name], capture_output=True, text=True)
        if r.returncode == 0 and name in r.stdout:
            hits.append("a winget package")
    if hits and name not in ok:
        return [("package", f"{', '.join(hits)}: needs K2's and the Kessel Steward's sign-off in collision-ok.toml")]
    return []


# ── the commands ───────────────────────────────────────────────────────

ORDER = ("shape", "product", "ours", "hard", "unique", "registry", "text", "soft", "host", "package")


def lint_brand(folder, live, registry=None):
    brand = read_brand(folder)
    others = [b for b in held() if b["folder"] != brand["folder"]]
    hard, soft = reserved()
    ok = signoffs(folder)
    findings = []
    names = [("id", brand["id"]), ("command", brand["name"])] + [("alias", a) for a in brand["aliases"]]
    if is_ours(brand):
        for what, name in names:
            if not SHAPE.fullmatch(name):
                findings.append((what, name, "shape", "must match ^[a-z][a-z0-9]{1,11}$"))
    else:
        for what, name in names:
            for rule, why in name_findings(name, what, others, hard, soft, ok):
                if what == "id" and rule in ("text", "soft"):
                    continue  # an id is never typed or printed as a word
                findings.append((what, name, rule, why))
        for rule, why in product_findings(brand["product"], others):
            findings.append(("product", brand["product"], rule, why))
        if registry is not None:
            findings += registry_findings(names, brand["product"], brand["id"], registry)
        if live:
            for what, name in names[1:]:
                for rule, why in host_findings(name) + package_findings(name, ok):
                    findings.append((what, name, rule, why))
    for what, name, rule, why in findings:
        print(f"brand lint: {brand['id']}: {what} {name}: {rule}: {why}", file=sys.stderr)
    if findings:
        print(f"brand lint: {brand['id']}: REFUSED ({len(findings)} finding(s))", file=sys.stderr)
        return 1
    live_note = ", the host and the package indexes" if live and not is_ours(brand) else ""
    if registry is not None and not is_ours(brand):
        live_note += f", the registry ({len(registry)} brands)"
    print(f"brand lint: {brand['id']}: command {brand['name']}, product {brand['product']!r}: ok (the written lists{live_note})")
    return 0


def lint_name(name, product, registry=None):
    hard, soft = reserved()
    others = held()
    found = name_findings(name, "command", others, hard, soft, {})
    if product is not None:
        found += product_findings(product, others)
    if registry is not None:
        found += [(rule, why) for _, _, rule, why in registry_findings([("command", name)], product, None, registry)]
    rules = sorted({r for r, _ in found}, key=ORDER.index)
    answer = rules[0] if rules else "ok"
    print(f"{name}\t{answer}")
    for rule, why in found:
        print(f"  {rule}: {why}", file=sys.stderr)
    return 1 if rules else 0


def main(argv):
    usage = ("usage: brand.sh lint <brand folder> [--live] [--registry <file>]"
             " | brand.sh lint --name <command> [--product <name>] [--registry <file>]")
    args = list(argv)
    registry = None
    if "--registry" in args:
        i = args.index("--registry")
        if i + 1 >= len(args) or args[i + 1].startswith("-") or args.count("--registry") != 1:
            print(usage, file=sys.stderr)
            return 2
        registry = read_registry(args[i + 1])
        del args[i:i + 2]
    if args[:1] == ["--name"] and len(args) in (2, 4) and (len(args) == 2 or args[2] == "--product"):
        return lint_name(args[1], args[3] if len(args) == 4 else None, registry)
    if len(args) in (1, 2) and not args[0].startswith("-") and args[1:] in ([], ["--live"]):
        if not os.path.isfile(os.path.join(args[0], "brand.toml")):
            print(f"brand lint: no brand.toml in {args[0]}", file=sys.stderr)
            return 2
        return lint_brand(args[0], live=args[1:] == ["--live"], registry=registry)
    print(usage, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
