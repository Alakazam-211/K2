#!/usr/bin/env python3
"""The name policy for a brand: its command, its aliases, its id and its
product name, each checked on its own rules.

    bash scripts/brand.sh lint <brand folder> [--live] [--hosted] [--registry <file>]
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
  package  Homebrew's formulae (names, aliases, old names and the programs
           they install, from its formula.json), Debian's file index
           (usr/bin, bin, usr/sbin), Arch's pkgfile and winget when they
           are here: a hit is a soft collision (it needs the sign-off)
The live checks fail loudly when a source can't be read: a name is never
called free without its check. Their sources can be pointed elsewhere
(BRAND_LINT_BREW_JSON, BRAND_LINT_DEBIAN_CONTENTS:
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

Every brand's other fields are checked for their shapes (the same shapes
crates/k2-brand's reader refuses): a table or key it does not know
(unknown), a value of the wrong shape (shape: URLs https, domains in lower
case, emails, minisign key lines, colours #RRGGBB, relative folders), and a
[machine] name other than the default brand's (machine: the code still
uses those names everywhere).

--hosted also checks a brand (not ours) as the brand of a hosted box: every
field such a box needs is set (hosted), the links that may not be none are
not "", and none of them is the default brand's own value (ours).

--name checks one command name (and --product one product) against the
brands this repo holds and prints `<name><TAB><answer>`: the first rule it
breaks in the order above, or `ok`. Exit 0 when nothing is refused, 1 when
something is, 2 for a usage error.
"""
import gzip
import io
import json
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
# Homebrew's formula list: one object per formula. (formula_names.txt is a
# file brew writes in its local cache, not one the API serves.)
BREW_JSON = "https://formulae.brew.sh/api/formula.json"
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


# ── the other fields ──────────────────────────────────────────────────

# The tables a brand file may hold, and the keys of each one this lint
# knows. [cli] and [package] are checked by their own readers; [update] and
# [pins] are lists of keys.
TABLES = {
    "names": ("product", "short", "company", "lockup", "server", "login"),
    "cli": None,
    "package": None,
    "update": ("keys",),
    "zones": ("label", "internal"),
    "control": ("connect", "cert", "push", "relay", "relay_list", "web_api"),
    "feeds": ("daemon", "downloads"),
    "pins": ("keys",),
    "machine": ("home", "binary_prefix", "unit_prefix"),
    "look": ("icons", "accent"),
    "links": ("site", "support_email", "support_url", "terms", "privacy", "dashboard", "feedback"),
    "mail": ("system_from", "system_from_name", "postal"),
    "phone": ("bundle_id", "team_id", "push_topic"),
    "sew": ("call_name",),
}
# Each field's shape. "url?" is an https URL or "" (none).
FIELD_SHAPES = {
    "names.short": "name", "names.company": "name", "names.lockup": "name",
    "names.server": "name", "names.login": "name",
    "zones.label": "domain", "zones.internal": "bool",
    "control.connect": "url", "control.cert": "url", "control.push": "url",
    "control.relay": "domain", "control.relay_list": "url", "control.web_api": "url",
    "feeds.daemon": "url", "feeds.downloads": "url",
    "update.keys": "keys", "pins.keys": "keys",
    "machine.home": "home", "machine.binary_prefix": "prefix", "machine.unit_prefix": "prefix",
    "look.icons": "path", "look.accent": "colour",
    "links.site": "url", "links.support_email": "email", "links.support_url": "url?",
    "links.terms": "url?", "links.privacy": "url?", "links.dashboard": "url?", "links.feedback": "url?",
    "mail.system_from": "email", "mail.system_from_name": "name", "mail.postal": "bool",
    "phone.bundle_id": "rdns", "phone.team_id": "team", "phone.push_topic": "rdns",
    "sew.call_name": "name",
}
# What a hosted box of a brand (not ours) needs set. The links in
# HOSTED_SOME may not be "" either; the fields in HOSTED_NOT_OURS may not
# be the default brand's own value.
HOSTED = (
    "names.company", "names.login", "zones.label", "zones.internal", "package.update_feed",
    "update.keys", "feeds.daemon", "feeds.downloads", "look.icons", "links.site",
    "links.support_email", "links.support_url", "links.terms", "links.privacy",
    "links.dashboard", "links.feedback", "mail.system_from", "mail.postal",
)
HOSTED_SOME = ("links.support_url", "links.terms", "links.privacy")
HOSTED_NOT_OURS = (
    "names.company", "names.login", "zones.label", "package.update_feed", "update.keys",
    "feeds.daemon", "feeds.downloads", "links.site", "links.support_email", "links.support_url",
    "links.terms", "links.privacy", "links.dashboard", "links.feedback", "mail.system_from",
)
DOMAIN_LABEL = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?")
URL_PATH = re.compile(r"(/[!#-&(-;=?-\[\]-_a-~]*)?")


def is_domain(v):
    labels = v.split(".")
    return (len(v) <= 253 and len(labels) >= 2 and all(DOMAIN_LABEL.fullmatch(l) for l in labels)
            and labels[-1].isalpha() and labels[-1].isascii())


def is_url(v):
    if not v.startswith("https://"):
        return False
    rest = v[len("https://"):]
    host, slash, path = rest.partition("/")
    return is_domain(host) and URL_PATH.fullmatch(slash + path) is not None


def minisign_key_id(line):
    """minisign's key id of a public key line, or None when it is not one."""
    import base64, binascii
    if not re.fullmatch(r"[A-Za-z0-9+/]+={0,2}", line or "") or len(line) % 4:
        return None
    try:
        raw = base64.b64decode(line, validate=True)
    except (binascii.Error, ValueError):
        return None
    if len(raw) != 42 or raw[:2] != b"Ed":
        return None
    return f"{int.from_bytes(raw[2:10], 'little'):016X}"


def key_problem(keys):
    if not isinstance(keys, list) or not keys:
        return "a list of { role, key_id, pubkey } tables, at least one"
    seen = set()
    for i, k in enumerate(keys):
        if not isinstance(k, dict) or set(k) != {"role", "key_id", "pubkey"}:
            return f"[{i}] must be a table of role, key_id and pubkey"
        if not all(isinstance(k[x], str) for x in k):
            return f"[{i}]: role, key_id and pubkey are strings"
        if k["role"] not in ("primary", "backup"):
            return f"[{i}].role {k['role']!r} must be primary or backup"
        got = minisign_key_id(k["pubkey"])
        if got is None:
            return f"[{i}].pubkey is not a minisign public key line"
        if got != k["key_id"]:
            return f"[{i}].key_id {k['key_id']!r} is not the pubkey's key id ({got})"
        if got in seen:
            return f"lists {got} twice"
        seen.add(got)
    if keys[0]["role"] != "primary":
        return "must start with a primary key"
    return None


def shape_problem(shape, v, ours):
    """Why `v` is not of `shape`, or None."""
    if shape == "bool":
        return None if isinstance(v, bool) else "true or false"
    if shape == "keys":
        return key_problem(v)
    if not isinstance(v, str):
        return "a string"
    if shape == "url?" and v == "":
        return None
    if v == "":
        return "not empty"
    if shape == "name":
        ok = len(v) <= 40 and v == v.strip() and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in v)
        return None if ok else "one line of 1 to 40 characters with no space at either end"
    if shape == "domain":
        return None if is_domain(v) else "a domain name in lower case (example.invalid)"
    if shape in ("url", "url?"):
        return None if is_url(v) else "an https URL: https://<domain>[/<path>]"
    if shape == "email":
        local, at, domain = v.partition("@")
        ok = at and re.fullmatch(r"[A-Za-z0-9._+-]+", local) and is_domain(domain)
        return None if ok else "an email address: <name>@<domain>"
    if shape == "colour":
        return None if re.fullmatch(r"#[0-9A-Fa-f]{6}", v) else "a colour in hex: #RRGGBB"
    if shape == "path":
        parts = v.split("/")
        ok = not v.startswith("/") and "\\" not in v and all(parts) and not any(ord(c) < 32 for c in v)
        if ok and not ours and ".." in parts:
            return "a folder inside the brand folder (no ..)"
        return None if ok else "a folder from the brand folder: relative, parts split by /"
    if shape == "rdns":
        return None if re.fullmatch(r"[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)+", v) else "a reverse-DNS id in lower case (com.example.app)"
    if shape == "team":
        return None if re.fullmatch(r"[A-Z0-9]{10}", v) else "an Apple team id: 10 capital letters and digits"
    if shape == "home":
        return None if re.fullmatch(r"\.[a-z][a-z0-9]{1,11}", v) else "a dot and a name: ^\\.[a-z][a-z0-9]{1,11}$"
    if shape == "prefix":
        return None if SHAPE.fullmatch(v) else "^[a-z][a-z0-9]{1,11}$"
    raise AssertionError(shape)


def read_doc(folder):
    with open(os.path.join(folder, "brand.toml"), "rb") as f:
        return tomllib.load(f)


def field_value(doc, field):
    """(set, value) of `table.key` in a brand file."""
    table, _, key = field.partition(".")
    t = doc.get(table)
    if not isinstance(t, dict) or key not in t:
        return False, None
    return True, t[key]


def field_findings(doc, ours, hosted, default):
    """(field, rule, why) for every other field: unknown tables and keys,
    shapes, the machine names, and with `hosted` what a hosted box needs."""
    out = []
    for key, value in doc.items():
        if key in ("id", "schema"):
            continue
        if key not in TABLES:
            out.append((key, "unknown", f"a table this build does not know (it knows {', '.join(TABLES)})"))
            continue
        if not isinstance(value, dict):
            out.append((key, "shape", "must be a table"))
            continue
        known = TABLES[key]
        if known is None:
            continue
        for k in value:
            if k not in known:
                out.append((f"{key}.{k}", "unknown", f"a key [{key}] does not hold (it knows {', '.join(known)})"))
    for field, shape in FIELD_SHAPES.items():
        present, v = field_value(doc, field)
        if not present:
            continue
        why = shape_problem(shape, v, ours)
        if why:
            out.append((field, "shape", f"must be {why}" if not why.startswith(("[", "lists", "must")) else why))
            continue
        if field.startswith("machine."):
            _, want = field_value(default, field)
            if v != want:
                out.append((field, "machine", f"must be {want!r} for now: the code still uses that name everywhere"))
    if hosted and not ours:
        for field in HOSTED:
            present, v = field_value(doc, field)
            if not present:
                out.append((field, "hosted", "a hosted box needs it set"))
                continue
            if field in HOSTED_SOME and v == "":
                out.append((field, "hosted", "a hosted box needs a link here, not \"\""))
                continue
            if field in HOSTED_NOT_OURS and v != "":
                _, theirs = field_value(default, field)
                if v == theirs:
                    out.append((field, "ours", "is the default brand's own value; a hosted box sets its own"))
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
        try:
            formulae = json.loads(fetch("BRAND_LINT_BREW_JSON", BREW_JSON))
        except ValueError as e:
            raise SystemExit(f"brand lint: Homebrew's formula list is not JSON ({e}): the package check did not run")
        if not isinstance(formulae, list) or not formulae:
            raise SystemExit("brand lint: Homebrew's formula list is empty or not a list: the package check did not run")
        names = set()
        for f in formulae:
            if not isinstance(f, dict) or not isinstance(f.get("name"), str):
                raise SystemExit("brand lint: a Homebrew formula with no name: the package check did not run")
            names.add(f["name"])
            for key in ("aliases", "oldnames", "executables"):
                names.update(v for v in (f.get(key) or []) if isinstance(v, str))
        _index["brew"] = names
        names = set()
        for line in fetch("BRAND_LINT_DEBIAN_CONTENTS", DEBIAN_CONTENTS).splitlines():
            path = line.split()[0] if line.split() else ""
            for prefix in ("usr/bin/", "bin/", "usr/sbin/"):
                if path.startswith(prefix) and "/" not in path[len(prefix):]:
                    names.add(path[len(prefix):])
        _index["debian"] = names
    hits = []
    if name in _index["brew"]:
        hits.append("a Homebrew formula, alias or program")
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


def lint_brand(folder, live, registry=None, hosted=False):
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
    default = read_doc(os.path.join(HERE, "brand", "k2"))
    for field, rule, why in field_findings(read_doc(folder), is_ours(brand), hosted, default):
        findings.append(("field", field, rule, why))
    for what, name, rule, why in findings:
        print(f"brand lint: {brand['id']}: {what} {name}: {rule}: {why}", file=sys.stderr)
    if findings:
        print(f"brand lint: {brand['id']}: REFUSED ({len(findings)} finding(s))", file=sys.stderr)
        return 1
    live_note = ", the host and the package indexes" if live and not is_ours(brand) else ""
    if registry is not None and not is_ours(brand):
        live_note += f", the registry ({len(registry)} brands)"
    if hosted and not is_ours(brand):
        live_note += ", the hosted fields"
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
    usage = ("usage: brand.sh lint <brand folder> [--live] [--hosted] [--registry <file>]"
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
    hosted = "--hosted" in args
    if hosted:
        if args.count("--hosted") != 1 or args[:1] == ["--name"]:
            print(usage, file=sys.stderr)
            return 2
        args.remove("--hosted")
    if args[:1] == ["--name"] and len(args) in (2, 4) and (len(args) == 2 or args[2] == "--product"):
        return lint_name(args[1], args[3] if len(args) == 4 else None, registry)
    if len(args) in (1, 2) and not args[0].startswith("-") and args[1:] in ([], ["--live"]):
        if not os.path.isfile(os.path.join(args[0], "brand.toml")):
            print(f"brand lint: no brand.toml in {args[0]}", file=sys.stderr)
            return 2
        return lint_brand(args[0], live=args[1:] == ["--live"], registry=registry, hosted=hosted)
    print(usage, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
