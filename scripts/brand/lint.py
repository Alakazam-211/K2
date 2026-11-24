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

A table or key about the root helper's keys (root_helper, root_helper_keys,
...) is refused (roothelper): every brand's build ships our signed root
helper.

[control] is all six keys or none (control). On our brands it is held to
the bash CLI's own copy of the Connect address (cli/src/connect.sh): only
brand/k2 may set it, and its connect must be the bash copy's (bash), until
the bash CLI reads the brand file.

A brand that isn't ours (anything but brand/k2 and brand/k5 in this repo:
is_ours, the rule crates/k2-brand/src/ours.rs keeps) sets every field whose
default names us or reaches us, to a value of its own: SETS_ITS_OWN, the
same list crates/k2-brand refuses at build time. Left out (required) or set
to the default brand's own value (ours), a partner's build would show our
company, our sign-in name, our site, our support or contact address or our
source repo, or take our updates. A link there may be "" (none). The fields in SETS_ON_PURPOSE
(mail.postal, the phone app) are required too, but may be ours.

--hosted also checks a brand (not ours) as the brand of a hosted box: every
other field such a box needs is set (hosted), the links that may not be
none are not "", none of them is the default brand's own value (ours), and
its update keys are not the distributor template's test-only key (testkey).

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

# An escape in a string of our text (JSON goldens and the catalog, printf and
# echo -e strings in cli/k2): \n, \t, \r, \", \\, \/, \b, \f, \a, \e, \v,
# \', \uXXXX, \xHH, after any run of backslashes. It is read as a
# separator, so "\n2. CONNECT USERS" is the step "2.", not the word n2.
TEXT_ESCAPE = re.compile(r"\\+(?:u[0-9A-Fa-f]{4}|x[0-9A-Fa-f]{2}|[ntrbfaev\"'/\\])")


def word_rx(word, case):
    """`word` standing as a word (no letter, digit or _ on either side)."""
    return re.compile(r"(?<![A-Za-z0-9_])" + re.escape(word) + r"(?![A-Za-z0-9_])", 0 if case else re.I)


def stands_in(rx, line):
    """Whether `rx` (word_rx) finds its word in `line` once escapes are separators."""
    return bool(rx.search(TEXT_ESCAPE.sub(" ", line)))


def text_hits(word, case):
    """Lines of our own text where `word` stands as a word: (count, first)."""
    key = (word, case)
    if key in _text_cache:
        return _text_cache[key]
    rx = word_rx(word, case)
    # The prefilter can only let through more lines than the rule, never
    # fewer: TEXT_ESCAPE only puts spaces in place of escapes, so a word the
    # rule finds is a literal run of the raw line; and without `case`,
    # casefold() is wider than re.I (it folds the Kelvin sign to k and the
    # long s to s, as re.I matches them).
    needle = word if case else word.casefold()
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
                    # The word's letters first (cheap), then the rule.
                    if (needle in (line if case else line.casefold())) and stands_in(rx, line):
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
    "zones": ("label",),
    "control": ("connect", "cert", "push", "relay", "relay_list", "web_api"),
    "feeds": ("daemon", "downloads"),
    "machine": ("home", "binary_prefix", "unit_prefix"),
    "look": ("icons", "accent"),
    "links": ("site", "support_email", "contact_email", "repo", "support_url", "terms", "privacy",
              "dashboard", "feedback"),
    "mail": ("system_from", "system_from_name", "postal"),
    "phone": ("bundle_id", "team_id", "push_topic"),
    "sew": ("call_name",),
}
# Each field's shape. "url?" is an https URL or "" (none).
FIELD_SHAPES = {
    "names.short": "name", "names.company": "name", "names.lockup": "name",
    "names.server": "name", "names.login": "name",
    "zones.label": "domain",
    "control.connect": "url", "control.cert": "url", "control.push": "url",
    "control.relay": "domain", "control.relay_list": "url", "control.web_api": "url",
    "feeds.daemon": "url", "feeds.downloads": "url",
    "update.keys": "keys",
    "machine.home": "home", "machine.binary_prefix": "prefix", "machine.unit_prefix": "prefix",
    "look.icons": "path", "look.accent": "colour",
    "links.site": "url", "links.support_email": "email", "links.contact_email": "email",
    "links.repo": "url", "links.support_url": "url?",
    "links.terms": "url?", "links.privacy": "url?", "links.dashboard": "url?", "links.feedback": "url?",
    "mail.system_from": "from", "mail.system_from_name": "name", "mail.postal": "bool",
    "cli.no_bash": "bool",
    "phone.bundle_id": "rdns", "phone.team_id": "team", "phone.push_topic": "rdns",
    "sew.call_name": "name",
}
# What a brand that isn't ours sets to a value of its own, and what it sets
# to any value: the lists crates/k2-brand/src/brand_file.rs keeps (K2's
# ruling, B7). look.icons joins SETS_ON_PURPOSE once something reads it;
# until then only --hosted asks for it.
SETS_ITS_OWN = (
    "names.company", "names.login", "zones.label", "links.site", "links.support_email",
    "links.contact_email", "links.repo", "links.dashboard", "links.feedback", "feeds.daemon",
    "feeds.downloads",
    "package.update_feed", "update.keys",
)
SETS_ON_PURPOSE = ("mail.postal", "phone.bundle_id", "phone.team_id", "phone.push_topic")
# What a hosted box of a brand (not ours) needs set. The links in
# HOSTED_SOME may not be "" either; the fields in HOSTED_NOT_OURS may not
# be the default brand's own value.
HOSTED = (
    "names.company", "names.login", "zones.label", "package.update_feed",
    "update.keys", "feeds.daemon", "feeds.downloads", "look.icons", "links.site",
    "links.support_email", "links.contact_email", "links.repo", "links.support_url", "links.terms",
    "links.privacy", "links.dashboard", "links.feedback", "mail.postal",
)
HOSTED_SOME = ("links.support_url", "links.terms", "links.privacy")
HOSTED_NOT_OURS = (
    "names.company", "names.login", "zones.label", "package.update_feed", "update.keys",
    "feeds.daemon", "feeds.downloads", "links.site", "links.support_email", "links.contact_email",
    "links.repo", "links.support_url", "links.terms", "links.privacy", "links.dashboard",
    "links.feedback",
)
# Every brand's build ships our signed root helper (K2's ruling): a brand
# file names no root-helper key.
ROOT_HELPER_REFUSED = ("the root helper runs as root and is signed only by K5's release keys; partner "
                       "builds ship ours (a new owner decision is needed if a partner runs its own servers)")
# A brand that is not ours carries no shell CLI (Rosson's rule).
NO_BASH_REFUSED = ("must not be false: a partner build carries no shell CLI (cli/k2), no copy and no "
                   "fallback; the k5 program is the command (leave it out or set it true)")
# [control] is all six or none (K2's ruling).
CONTROL = ("relay", "relay_list", "connect", "cert", "push", "web_api")
# The bash CLI's own copy of control.connect: only our default brand runs
# the bash CLI, so brand/k2's [control] must match it, and no other brand
# of ours may set [control], until the bash CLI reads the brand file.
BASH_CONNECT = ("cli/src/connect.sh", re.compile(r"^_envn_into K2_CONNECT_BASE CONNECT_BASE (\S+)$"))
# The distributor template's [update] key is made only for tests: a
# hosted brand that ships it is refused.
TEMPLATE_BRAND = os.path.join("templates", "distributor", "brand", "brand.toml")
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
    if shape == "from":
        local, at, domain = v.partition("@")
        if at and domain == "{server}":
            v = f"{local}@server.invalid"
        local, at, domain = v.partition("@")
        ok = at and re.fullmatch(r"[A-Za-z0-9._+-]+", local) and is_domain(domain)
        return None if ok else "an email address: <name>@{server} or <name>@<domain>"
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


def bash_connect():
    """(where, url) of the bash CLI's copy of the Connect address."""
    rel, rx = BASH_CONNECT
    with open(os.path.join(HERE, rel), encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            m = rx.match(line.rstrip("\n"))
            if m:
                return f"{rel}:{n}", m.group(1)
    sys.exit(f"brand lint: no `_envn_into K2_CONNECT_BASE CONNECT_BASE <url>` line in {rel}: "
             "the check of our [control] against the bash CLI did not run")


def test_only_keys():
    """The distributor template's [update] keys: (key ids, key lines)."""
    path = os.path.join(HERE, TEMPLATE_BRAND)
    try:
        with open(path, "rb") as f:
            keys = tomllib.load(f)["update"]["keys"]
    except (OSError, KeyError, TypeError) as e:
        sys.exit(f"brand lint: can't read the template's test-only key from {TEMPLATE_BRAND} ({e}): "
                 "the --hosted key check did not run")
    return {k["key_id"] for k in keys}, {k["pubkey"] for k in keys}


def control_findings(doc, brand_id, ours, default_id):
    out = []
    t = doc.get("control")
    if not isinstance(t, dict):
        return out
    for k in CONTROL:
        if k not in t:
            out.append((f"control.{k}", "control",
                        f"a brand that sets [control] sets all six ({', '.join(CONTROL)}); this one is left out"))
    if ours:
        where, url = bash_connect()
        if brand_id != default_id:
            out.append(("control", "bash",
                        f"our brand {brand_id} may not set [control]: only the default brand ({default_id}) runs "
                        f"the bash CLI, whose own copy is {where}, until it reads the brand file"))
        elif t.get("connect") != url:
            out.append(("control.connect", "bash",
                        f"{t.get('connect')!r} is not the bash CLI's copy {url!r} ({where}); "
                        "change both, or teach the bash CLI to read the brand file first"))
    return out


def not_ours_findings(doc, default):
    """(field, rule, why) for a brand that isn't ours: each field of
    SETS_ITS_OWN or SETS_ON_PURPOSE it leaves out (required), and each field
    of SETS_ITS_OWN it sets to the default brand's own value (ours)."""
    out = []
    for field in SETS_ITS_OWN + SETS_ON_PURPOSE:
        if not field_value(doc, field)[0]:
            why = ("a brand that isn't ours sets its own (the default brand's would show ours)"
                   if field in SETS_ITS_OWN else
                   "a brand that isn't ours sets it on purpose (the default brand's is ours)")
            out.append((field, "required", f"is missing: {why}"))
    for field in SETS_ITS_OWN:
        present, v = field_value(doc, field)
        _, theirs = field_value(default, field)
        if not present:
            continue
        if field == "update.keys":
            ours_keys = theirs if isinstance(theirs, list) else []
            for k in v if isinstance(v, list) else []:
                for o in ours_keys:
                    if isinstance(k, dict) and isinstance(o, dict) and (
                            k.get("key_id") == o.get("key_id") or k.get("pubkey") == o.get("pubkey")):
                        out.append((field, "ours", f"holds the default brand's own update key {o.get('key_id')}: "
                                                   "a brand that isn't ours sets its own"))
            continue
        if v != "" and v == theirs:
            out.append((field, "ours", f"is the default brand's own value {json.dumps(v)}: "
                                       "a brand that isn't ours sets its own"))
    return out


def field_findings(doc, ours, hosted, default):
    """(field, rule, why) for every other field: unknown tables and keys,
    shapes, the machine names, and with `hosted` what a hosted box needs."""
    out = []
    for key, value in doc.items():
        if key.startswith("root_helper"):
            out.append((key, "roothelper", ROOT_HELPER_REFUSED))
            continue
        if isinstance(value, dict):
            for k in value:
                if k.startswith("root_helper"):
                    out.append((f"{key}.{k}", "roothelper", ROOT_HELPER_REFUSED))
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
            if k not in known and not k.startswith("root_helper"):
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
    out += control_findings(doc, doc.get("id"), ours, default.get("id"))
    if not ours:
        out += not_ours_findings(doc, default)
    if hosted and not ours:
        # Rosson's rule: a partner build never carries the shell CLI. Left
        # out, cli.no_bash is true (k2-brand and brand-values.mjs read it so).
        if (doc.get("cli") or {}).get("no_bash") is False:
            out.append(("cli.no_bash", "shell", NO_BASH_REFUSED))
        ids, lines = test_only_keys()
        for k in (doc.get("update") or {}).get("keys") or []:
            if isinstance(k, dict) and (k.get("key_id") in ids or k.get("pubkey") in lines):
                out.append(("update.keys", "testkey",
                            f"holds the distributor template's test-only key {k.get('key_id')}: "
                            "its private half never existed; put your own update key here"))
        for field in HOSTED:
            if field in SETS_ITS_OWN or field in SETS_ON_PURPOSE:
                continue  # asked of every brand that isn't ours, above
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
