#!/usr/bin/env python3
"""Compare two interop sessions' wire by structure (brand step C4).

    python3 tests/interop/wire_compare.py BASELINE.jsonl MIXED.jsonl [--report FILE]

BASELINE is two servers of the default brand talking; MIXED is the same
script with one server of the default brand and one of another brand
(tests/interop/two-brands.sh writes both). The rule (the eval's 3.5): every
name a peer process or another server reads keeps one spelling in every
brand build. So the two sessions must have the same structure on the wire.
Nonces, times, signatures and ports differ on every run, so bytes are not
compared (the eval's 9.11). What is compared, per exchange key (channel,
from, to, method, route):

  - the route itself (each path segment that is an id is its class);
  - the query's keys, and their values' classes;
  - the request's and the answer's header names, and their values after
    normalising (a version is {version}, a token is its prefix class);
  - every key path in the request and answer bodies, with the class of its
    value: object, list, num, bool, null, a random class (<uuid>, <ulid>,
    <hex>, <b64>, <pem>, <time>, <token:prefix_>), or str; a string that
    carries a brand word of any brand in the run is a tag and is kept as
    its normalised value, so `k2-sas-v2` stays `k2-sas-v2` and a build
    that sends `xy-sas-v2` differs;
  - for the owner channel (the scripted session against each server, whose
    calls are deterministic), the answer's status. Background traffic (the
    federation drain, the Postal loop) polls, so its statuses are compared
    as a note only, and so is a body path that is null on one side and a
    value on the other (an optional field not filled yet).

The owner's /boot-status is left out of the structure compare (its keys
follow boot timing; STRUCTURE_SKIP says why); its names are check 3's.

Display fields are dropped before anything is compared: `hint`, `doc`,
`message`, `command`, `use`, a string `error`, and the /boot-status names
(`productName`, `cliName`, `productLockup`). The local brand headers'
values (`X-K5-Server-Brand`, `X-K5-Server-Cli`) are display too; their
names are compared.

Three checks on top, each a failure:
  1. No `X-K5-*` header on the federation (peer) or Postal channel, in a
     request or an answer: the local brand headers never leave the machine,
     and the CLI's own headers are never sent by a server.
  2. No brand word of a brand other than the default one on the peer or
     Postal channel, outside display fields: in a route, a header or a body.
     The default brand's words are the neutral wire spelling (keeper rows).
  3. Each server answered its owner's `/cli/*` calls with its own brand in
     `X-K5-Server-Brand`, and its /boot-status `cliName` is its own command:
     proof the session really ran the builds it names.

Capture file (JSON lines). The first line is the session:
  {"kind": "session", "session": "mixed",
   "brands": {"A": {"id": "k2", "cli": "k2", "product": "K2"}, "B": {...}},
   "default": "k2", "vars": {"<run value>": "<placeholder>", ...}}
Each other line is one exchange:
  {"kind": "exchange", "seq": 7, "step": "fed-msg", "channel": "owner|peer|postal",
   "from": "owner|A|B", "to": "A|B|plane", "poll": false,
   "request": {"method": "POST", "target": "/cli/x?y=1", "headers": [[n, v]], "body": ...},
   "answer": {"status": 200, "headers": [[n, v]], "body": ...}}
A body is the parsed JSON, null for none, or {"$text": "..."} or
{"$bytes": n}. `vars` maps a run's own values (ports, home folders) to a
placeholder; they are put back before anything is classed. For a poll
(`"poll": true`, the harness waiting on a state), only the last exchange of
each run of the same key counts: the state it waited for.

Exit 0 when the sessions match and the checks pass, 1 with the differences
named, 2 on a bad input. Python stdlib only.
"""
import json
import re
import sys

DISPLAY_KEYS = {"hint", "doc", "message", "command", "use", "productName", "cliName", "productLockup"}
# `error` is display when it is a string; an `{"code", ...}` object is compared.
DISPLAY_IF_STRING = {"error"}
DISPLAY_HEADERS = {"x-k5-server-brand", "x-k5-server-cli"}
# Header values that differ on every run, or that the transport sets.
SKIP_HEADER_VALUES = {
    "date", "content-length", "host", "etag", "x-forwarded-for", "keep-alive",
    "transfer-encoding", "connection", "last-modified", "expires", "age",
}
# Routes whose structure is not compared, and why. /boot-status: its keys
# follow the boot's own timing (probes, the home database switch), not the
# brand; the names in it (cliName, productName) are check 3's, and it is a
# local route no other server reads.
STRUCTURE_SKIP = {("owner", "/boot-status")}
LOCAL_BRAND_HEADER = "x-k5-server-brand"
LOCAL_HEADER_PREFIX = "x-k5-"

CLASSED = re.compile(r"<(?:redacted|uuid|ulid|hex|time|jwt|b64|token:[a-z0-9_]+|pem:[A-Z ]+)>")
UUID = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
ULID = re.compile(r"[0-9A-HJKMNP-TV-Z]{26}")
HEX = re.compile(r"[0-9a-fA-F]{16,}")
TIME = re.compile(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?")
B64 = re.compile(r"[A-Za-z0-9+/_-]{24,}={0,2}")
JWT = re.compile(r"[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")
TOKEN = re.compile(r"([a-z][a-z0-9]*_(?:[a-z0-9]+_)*)([A-Za-z0-9_-]{12,})")
VERSION = re.compile(r"(?<![0-9.])\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?(?![0-9]|\.\d)")
# A number of three digits or more on its own (a port, a count, a time);
# a short one stays, so a tag's version (`k2-vault/1`) is compared.
NUMBER = re.compile(r"(?<![A-Za-z0-9_.])\d{3,}(?![A-Za-z0-9_]|\.\d)")


def is_random(tail):
    """A tail of letters, digits, `_` and `-` that reads as random: a digit
    or a capital in it (a code like `remote_instruction_disabled` has
    neither)."""
    return any(c.isdigit() or c.isupper() for c in tail)


def whole_class(s):
    """The random class of a whole string, or None. A value the harness
    already replaced by its class (`<token:pst_>`, `<redacted>`) is that."""
    if CLASSED.fullmatch(s):
        return s
    if s.startswith("-----BEGIN "):
        kind = s[len("-----BEGIN "):].split("-----", 1)[0].strip()
        return f"<pem:{kind}>"
    if UUID.fullmatch(s):
        return "<uuid>"
    if ULID.fullmatch(s):
        return "<ulid>"
    if HEX.fullmatch(s):
        return "<hex>"
    if TIME.fullmatch(s):
        return "<time>"
    if JWT.fullmatch(s) and is_random(s):
        return "<jwt>"
    m = TOKEN.fullmatch(s)
    if m and is_random(m.group(2)):
        return f"<token:{m.group(1)}>"
    if B64.fullmatch(s) and is_random(s) and not s.isdigit():
        return "<b64>"
    return None


class Session:
    """One capture file: its meta line and its exchanges."""

    def __init__(self, path):
        self.path = path
        self.meta = None
        self.exchanges = []
        with open(path, encoding="utf-8") as f:
            for n, line in enumerate(f, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as e:
                    raise ValueError(f"{path}:{n}: not JSON: {e}") from e
                kind = row.get("kind")
                if kind == "session":
                    if self.meta is not None:
                        raise ValueError(f"{path}:{n}: a second session line")
                    self.meta = row
                elif kind == "exchange":
                    self.exchanges.append(row)
                else:
                    raise ValueError(f"{path}:{n}: kind must be session or exchange, got {kind!r}")
        if self.meta is None:
            raise ValueError(f"{path}: no session line")
        for k in ("session", "brands", "default"):
            if k not in self.meta:
                raise ValueError(f"{path}: the session line has no {k!r}")
        if not self.exchanges:
            raise ValueError(f"{path}: no exchanges")
        self.name = self.meta["session"]
        self.brands = self.meta["brands"]
        self.default = self.meta["default"]
        vars_ = self.meta.get("vars") or {}
        # Longest first, so a home folder goes before a folder inside it.
        self.vars = sorted(vars_.items(), key=lambda kv: -len(kv[0]))
        self._var_rx = [(re.compile(r"(?<![0-9A-Za-z])" + re.escape(v) + r"(?![0-9A-Za-z])"), ph) for v, ph in self.vars if v]

    def put_back(self, s):
        for rx, ph in self._var_rx:
            s = rx.sub(ph, s)
        return s


def brand_words(sessions):
    """Every brand word of every brand in the run, and of the brands the
    session line lists as known (`known_brands`): ids, commands, products."""
    words = set()
    for s in sessions:
        for b in list(s.brands.values()) + list(s.meta.get("known_brands") or []):
            for k in ("id", "cli", "product"):
                if b.get(k):
                    words.add(b[k])
    return words


def word_rx(word, prefix=False):
    """A brand word as a word: not inside a longer run of letters or digits.
    With `prefix`, also at the start of a longer token (`k2skn_`, `k2v://`),
    which is how a tag is found; a failure needs the whole word."""
    tail = "" if prefix else r"(?![A-Za-z0-9])"
    flags = re.IGNORECASE if word.isupper() or " " in word else 0
    return re.compile(r"(?<![A-Za-z0-9])" + re.escape(word) + tail, flags)


class Shaper:
    """Turns exchanges into keys and features for one session."""

    def __init__(self, session, words):
        self.s = session
        self.word_rx = [word_rx(w, prefix=True) for w in sorted(words)]

    def has_brand_word(self, s):
        return any(rx.search(s) for rx in self.word_rx)

    def text(self, s):
        """A string with no whole random class, normalised: run values put
        back, embedded ids, versions and long numbers made classes."""
        s = self.s.put_back(s)
        s = UUID.sub("<uuid>", s)
        s = TIME.sub("<time>", s)
        s = TOKEN.sub(lambda m: f"<token:{m.group(1)}>" if is_random(m.group(2)) else m.group(0), s)
        s = HEX.sub("<hex>", s)
        s = VERSION.sub("{version}", s)
        s = NUMBER.sub("{n}", s)
        return s

    def value_class(self, s):
        """What a string body value compares as: its random class, a tag
        (its normalised text, when it carries a brand word), or str."""
        c = whole_class(s)
        if c:
            return c
        t = self.text(s)
        if self.has_brand_word(t):
            return f"tag:{t}"
        return "str"

    def segment(self, seg):
        c = whole_class(seg)
        if c:
            return c
        if seg.isdigit():
            return "{n}"
        return self.text(seg)

    def route(self, target):
        path = target.split("?", 1)[0]
        return "/".join(self.segment(p) if p else "" for p in path.split("/"))

    def query(self, target):
        if "?" not in target:
            return []
        out = []
        for part in target.split("?", 1)[1].split("&"):
            if not part:
                continue
            k, _, v = part.partition("=")
            k = unquote(k)
            v = unquote(v)
            if k == "token":
                out.append(f"q:{k}")
                continue
            out.append(f"q:{k}={self.value_class(v)}")
        return out

    def header(self, side, name, value):
        n = name.lower()
        out = [f"{side}h:{n}"]
        if n in DISPLAY_HEADERS or n in SKIP_HEADER_VALUES:
            return out
        if n in ("authorization", "proxy-authorization"):
            scheme, _, rest = value.partition(" ")
            out.append(f"{side}h:{n}={scheme} {whole_class(rest.strip()) or ('<redacted>' if rest.strip() == '<redacted>' else 'str')}")
            return out
        if n in ("cookie", "set-cookie"):
            parts = []
            for p in value.split(";"):
                k, eq, _ = p.strip().partition("=")
                parts.append(f"{k}=<v>" if eq else k)
            out.append(f"{side}h:{n}={'; '.join(parts)}")
            return out
        c = whole_class(value)
        out.append(f"{side}h:{n}={c or self.text(value)}")
        return out

    def body(self, side, value):
        out = []
        self._walk(side, "$", value, out)
        return out

    def _walk(self, side, path, v, out):
        if isinstance(v, dict):
            if path == "$" and set(v) == {"$text"} and isinstance(v["$text"], str):
                out.append(f"{side}b:$text={self.value_class(v['$text'])}")
                return
            if path == "$" and set(v) == {"$bytes"}:
                out.append(f"{side}b:$=<bytes>")
                return
            out.append(f"{side}b:{path}=object")
            for k in sorted(v):
                kk = whole_class(k) or self.text(k)
                x = v[k]
                if k in DISPLAY_KEYS or (k in DISPLAY_IF_STRING and isinstance(x, str)):
                    out.append(f"{side}b:{path}.{kk}=<display>")
                    continue
                self._walk(side, f"{path}.{kk}", x, out)
        elif isinstance(v, list):
            out.append(f"{side}b:{path}=list")
            for x in v:
                self._walk(side, f"{path}[]", x, out)
        elif isinstance(v, bool):
            out.append(f"{side}b:{path}=bool")
        elif isinstance(v, (int, float)):
            out.append(f"{side}b:{path}=num")
        elif v is None:
            out.append(f"{side}b:{path}=null")
        elif isinstance(v, str):
            out.append(f"{side}b:{path}={self.value_class(v)}")
        else:
            raise ValueError(f"{self.s.path}: a body value of type {type(v).__name__}")

    def key(self, ex):
        req = ex["request"]
        return (ex["channel"], ex["from"], ex["to"], req["method"].upper(), self.route(req["target"]))

    def features(self, ex):
        req, ans = ex["request"], ex["answer"]
        f = set(self.query(req["target"]))
        for n, v in req.get("headers") or []:
            f.update(self.header("r", n, v))
        f.update(self.body("r", req.get("body")))
        for n, v in ans.get("headers") or []:
            f.update(self.header("a", n, v))
        f.update(self.body("a", ans.get("body")))
        return f

    def status(self, ex):
        return f"st:{ex['answer']['status']}"


def counted(session):
    """The exchanges that count: a run of owner polls (the harness waiting
    on a state) keeps only its last exchange, the state it waited for.
    Background traffic between the polls does not break the run."""
    owner = [i for i, ex in enumerate(session.exchanges) if ex.get("channel") == "owner"]
    nxt_owner = {a: b for a, b in zip(owner, owner[1:])}

    def poll_key(ex):
        req = ex["request"]
        return (ex.get("step"), req["method"], req["target"].split("?")[0], ex["to"])

    out = []
    for i, ex in enumerate(session.exchanges):
        if ex.get("channel") == "owner" and ex.get("poll") and i in nxt_owner:
            nxt = session.exchanges[nxt_owner[i]]
            if nxt.get("poll") and poll_key(nxt) == poll_key(ex):
                continue
        out.append(ex)
    return out


def shape(session, words):
    """{key: (features, statuses)} for a session. A route in
    STRUCTURE_SKIP is left out (its names are check 3's)."""
    sh = Shaper(session, words)
    shapes = {}
    for ex in counted(session):
        for k in ("channel", "from", "to", "request", "answer"):
            if k not in ex:
                raise ValueError(f"{session.path}: exchange {ex.get('seq')} has no {k!r}")
        key = sh.key(ex)
        if (key[0], key[4]) in STRUCTURE_SKIP:
            continue
        feats, stats = shapes.setdefault(key, (set(), set()))
        feats.update(sh.features(ex))
        stats.add(sh.status(ex))
    return shapes


def fmt_key(key):
    ch, fr, to, method, route = key
    return f"{ch} {fr}->{to} {method} {route}"


def null_swap(f, other):
    """`f` differs from `other` only by a null: the same body path is null
    on one side and a value on the other (an optional field that was not
    filled yet when that side asked)."""
    head, sep, cls = f.rpartition("=")
    if not sep or "b:" not in head[:3]:
        return False
    if cls == "null":
        return any(g.startswith(head + "=") and g != f for g in other)
    return head + "=null" in other


def compare(base, mixed, words):
    """(failures, notes) between two sessions' shapes."""
    fails, notes = [], []
    sb, sm = shape(base, words), shape(mixed, words)
    for key in sorted(set(sb) - set(sm)):
        fails.append(f"only in {base.name}: {fmt_key(key)}")
    for key in sorted(set(sm) - set(sb)):
        fails.append(f"only in {mixed.name}: {fmt_key(key)}")
    for key in sorted(set(sb) & set(sm)):
        fb, stb = sb[key]
        fm, stm = sm[key]
        for here, there, name in ((fb, fm, base.name), (fm, fb, mixed.name)):
            for f in sorted(here - there):
                line = f"{fmt_key(key)}: only in {name}: {f}"
                (notes if null_swap(f, there) else fails).append(line + (" (a null on the other side)" if null_swap(f, there) else ""))
        if stb != stm:
            line = f"{fmt_key(key)}: statuses {sorted(stb)} in {base.name}, {sorted(stm)} in {mixed.name}"
            (fails if key[0] == "owner" else notes).append(line)
    return fails, notes


def strings_in(v, path="$"):
    """(path, string, display) for every string in a body, keys included."""
    if isinstance(v, dict):
        for k, x in v.items():
            yield f"{path}.{k}", k, False
            disp = k in DISPLAY_KEYS or (k in DISPLAY_IF_STRING and isinstance(x, str))
            for p, s, d in strings_in(x, f"{path}.{k}"):
                yield p, s, d or disp
    elif isinstance(v, list):
        for x in v:
            yield from strings_in(x, f"{path}[]")
    elif isinstance(v, str):
        yield path, v, False


def checks(session):
    """(failures, notes) of the three checks for one session."""
    fails, notes = [], []
    default = session.brands.get(session.default) or next(
        (b for b in session.brands.values() if b.get("id") == session.default), None)
    others = set()
    for b in session.brands.values():
        if b.get("id") != session.default:
            for k in ("cli", "product", "id"):
                w = b.get(k)
                if w and (default is None or w not in (default.get("cli"), default.get("product"), default.get("id"))):
                    others.add(w)
    rxs = [(w, word_rx(w)) for w in sorted(others)]
    seen_brand = {}
    seen_cli = {}
    for ex in session.exchanges:
        ch = ex["channel"]
        req, ans = ex["request"], ex["answer"]
        where = f"{session.name} #{ex.get('seq')} {ch} {ex['from']}->{ex['to']} {req['method']} {req['target'].split('?')[0]}"
        if ch in ("peer", "postal"):
            for side, hs in (("request", req.get("headers") or []), ("answer", ans.get("headers") or [])):
                for n, _ in hs:
                    if n.lower().startswith(LOCAL_HEADER_PREFIX):
                        fails.append(f"{where}: the {side} carries {n}, a local header, off the machine")
            if rxs:
                found = []
                texts = [("route", unquote(req["target"]), False)]
                for side, hs in (("request", req.get("headers") or []), ("answer", ans.get("headers") or [])):
                    for n, v in hs:
                        texts.append((f"{side} header {n}", f"{n}: {v}", n.lower() in DISPLAY_HEADERS))
                for side, body in (("request", req.get("body")), ("answer", ans.get("body"))):
                    for p, s, d in strings_in(body):
                        if whole_class(s):
                            continue
                        texts.append((f"{side} body {p}", s, d))
                for label, s, disp in texts:
                    for w, rx in rxs:
                        if rx.search(s):
                            found.append((label, w, disp))
                for label, w, disp in found:
                    line = f"{where}: the brand word {w!r} in the {label}"
                    if disp:
                        notes.append(line + " (a display field: allowed, but text from another server)")
                    else:
                        fails.append(line + ": a brand name on the wire")
        elif ch == "owner":
            role = ex["to"]
            path = req["target"].split("?", 1)[0]
            if path.startswith("/cli/"):
                for n, v in ans.get("headers") or []:
                    if n.lower() == LOCAL_BRAND_HEADER:
                        seen_brand.setdefault(role, set()).add(v.strip())
            if path == "/boot-status" and isinstance(ans.get("body"), dict) and "cliName" in ans["body"]:
                seen_cli.setdefault(role, set()).add(ans["body"]["cliName"])
    for role, b in sorted(session.brands.items()):
        got = seen_brand.get(role)
        if not got:
            fails.append(f"{session.name}: no /cli/* answer from {role} carried {LOCAL_BRAND_HEADER}; the session did not show which build {role} ran")
        elif got != {b["id"]}:
            fails.append(f"{session.name}: {role} answered {LOCAL_BRAND_HEADER} {sorted(got)}, want {b['id']!r}: not the build the session names")
        cli = seen_cli.get(role)
        if not cli:
            fails.append(f"{session.name}: no /boot-status answer from {role} named its cliName")
        elif cli != {b["cli"]}:
            fails.append(f"{session.name}: {role}'s /boot-status cliName is {sorted(cli)}, want {b['cli']!r}")
    return fails, notes


def unquote(s):
    """Percent-decoding, stdlib only (urllib is stdlib too, but this keeps
    a `+` as it is)."""
    from urllib.parse import unquote as uq
    return uq(s)


def run(base_path, mixed_path, report=None):
    try:
        base, mixed = Session(base_path), Session(mixed_path)
    except (OSError, ValueError) as e:
        print(f"wire-compare: {e}", file=sys.stderr)
        return 2
    words = brand_words([base, mixed])
    fails, notes = compare(base, mixed, words)
    for s in (base, mixed):
        f, n = checks(s)
        fails += f
        notes += n
    keys = shape(mixed, words)
    lines = [f"wire-compare: FAIL {f}" for f in fails] + [f"wire-compare: note {n}" for n in notes]
    if fails:
        lines.append(f"wire-compare: FAILED ({len(fails)} differences or failed checks, {len(notes)} notes)")
    else:
        nfeat = sum(len(f) for f, _ in keys.values())
        lines.append(
            f"wire-compare: ok ({len(keys)} exchange keys, {nfeat} features alike in {base.name} and {mixed.name}; "
            f"no X-K5 header and no other brand's name on the peer or Postal wire; {len(notes)} notes)")
    text = "\n".join(lines) + "\n"
    if report:
        with open(report, "w", encoding="utf-8") as f:
            f.write(text)
    sys.stdout.write(text)
    return 1 if fails else 0


def main(argv):
    args = list(argv)
    report = None
    if "--report" in args:
        i = args.index("--report")
        if i + 1 >= len(args):
            print("wire-compare: --report needs a file", file=sys.stderr)
            return 2
        report = args[i + 1]
        del args[i:i + 2]
    if len(args) != 2 or any(a.startswith("-") for a in args):
        print(__doc__.split("\n\n", 2)[1], file=sys.stderr)
        return 2
    return run(args[0], args[1], report)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
