#!/bin/sh
# Fail unless every test target, test-only feature and #[ignore] of the kit is accounted for in
# tools/targets.txt, and each gate part named there really runs what it is named for. Text only:
# the manifests, the sources and tools/tests/full.sh are read, nothing is built. POSIX sh and
# python3 (3.11 or later, for tomllib; run isolated, -I).
#
#   tools/check-targets.sh [ROOT]       (default: the repo root)
#
# The targets: those of every workspace member (the root Cargo.toml's `members`) and of each
# manifest in EXTRA_MANIFESTS below (crates with a workspace of their own), found as cargo finds
# them: the lib (its unit tests, kind `lib`; its doc examples, kind `doctest`), bins, integration
# tests, examples and benches, auto-discovered or declared, with their `required-features` and
# `test` flags. Every [features] entry of those manifests is a `feature` row.
#
# tools/targets.txt, one row per line (`#` comments and blank lines aside):
#   PACKAGE|KIND|NAME|FEATURES|PARTS
# KIND is lib, doctest, bin, test, example, bench, fuzz-bin, feature or ignore. FEATURES is the
# `--features` list the parts pass (comma-separated, sorted; empty for none, `all` for
# --all-features). A target may have one row per feature list. PARTS is one of:
#   PART[,PART...]            each part runs the target with exactly those features;
#   built-only:PART: REASON   the part builds it (cargo build, check, clippy, --no-run) and no
#                             part runs it, for REASON;
#   excluded: REASON          no part runs or builds it, for REASON (a doctest row may say so
#                             only while the crate's docs hold no runnable example);
# for a feature row: the parts that enable it (a feature with no `product:` reason is
# test-only), or `product: REASON` (a feature a host may turn on; not checked);
# for an ignore row (NAME is FILE::FN, FILE relative to the package): `pending ruling: ...`, an
# #[ignore] whose runner is not settled yet, allowed while the ruling is outstanding.
# A row whose PARTS holds `pending ruling` is pending: CHECK_TARGETS_LANDING=1 (the mode for the
# commit that lands a change) reports each pending row as a finding, so the list must be empty
# when a change lands; without it pending rows are allowed and counted.
#
# A part's commands are read from its `case` arm in tools/tests/full.sh, with any file the arm
# sources (`. tools/tests/X.sh`): each `cargo test`, `cargo nextest run`, `tests` (the file's
# nextest runner: no doc examples), `cargo build`, `check` and `clippy`, its
# -p, --manifest-path, --features, --all-features, and target flags (--lib, --doc, --bins,
# --bin, --tests, --test, --examples, --example, --benches, --bench, --all-targets) read as cargo
# reads them; `--no-run` and a harness `--list` build without running. An arm variable set by
# `NAME=$(for f in GLOB; do printf ' --test %s' ...; done)` is expanded from the files on disk; a
# command with any other `$` is unreadable: a row its part's other commands don't satisfy is
# reported as unreadable, not as unselected.
#
# Findings, each `check-targets: KIND: WHAT`:
#   new         a target or feature with no row;
#   stale       a row for no target, feature or #[ignore] (or a pending ignore that names its
#               runner now);
#   unselected  a row's part doesn't run (or, built-only, build) the target with the row's
#               features, or doesn't exist; an excluded doctest whose crate has doc examples;
#   feature     a test-only feature that a part named on its row doesn't enable, or with no
#               part: enabled means the part's command turns it on, through --features,
#               --all-features, another feature or a workspace dependency's `features` (its
#               dev-dependencies too, for a test build);
#   ignore      an #[ignore] whose reason names no runner: a script (`PATH.sh`) that names the
#               test, or `run by name: PATH`, a file that names it; and no pending row;
#   pending     (landing mode only) a pending row;
#   format      a row that can't be read, or a second row for the same target and features;
#   unreadable  a part named on a row whose commands can't be read.
# Exit 0 clean, 1 findings, 2 usage.
set -eu
case $# in 0|1) ;; *) echo "usage: check-targets.sh [ROOT]" >&2; exit 2 ;; esac
case ${1:-} in -*) echo "usage: check-targets.sh [ROOT]" >&2; exit 2 ;; esac
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-targets: no directory $ROOT" >&2; exit 2; }
# manifests with a workspace of their own whose targets count too, paths from the root
EXTRA_MANIFESTS="tools/mpc-tools/Cargo.toml"
LANDING=${CHECK_TARGETS_LANDING:-0}
echo "check-targets: landing mode ${LANDING} (1: pending rows are findings)"
exec python3 -I - "$ROOT" "$EXTRA_MANIFESTS" "$LANDING" <<'PY'
import glob, os, re, shlex, sys, tomllib

# the package metadata key that makes a crate's bins fuzz targets (none in a public cut)
FUZZ_KEY = None

ROOT, EXTRA, LANDING = sys.argv[1], sys.argv[2].split(), sys.argv[3] == "1"
TARGETS, FULL = "tools/targets.txt", "tools/tests/full.sh"
KINDS = ("lib", "doctest", "bin", "test", "example", "bench", "fuzz-bin")
out = []


def finding(kind, what):
    out.append(f"check-targets: {kind}: {what}")


def rd(p):
    with open(os.path.join(ROOT, p), encoding="utf-8") as f:
        return f.read()


def exists(p):
    return os.path.isfile(os.path.join(ROOT, p))


# ---- packages and their targets -----------------------------------------------------------
ws = tomllib.loads(rd("Cargo.toml")).get("workspace", {})
members = []
for m in ws.get("members", []):
    members += sorted(os.path.relpath(p, ROOT) for p in glob.glob(os.path.join(ROOT, m)))
pkgs = {}  # name -> package
by_dir = {}


def load(d, member):
    t = tomllib.loads(rd(os.path.join(d, "Cargo.toml") if d != "." else "Cargo.toml"))
    pk = t["package"]
    p = {"name": pk["name"], "dir": os.path.normpath(d), "toml": t, "member": member,
         "fuzz_bins": bool(FUZZ_KEY and pk.get("metadata", {}).get(FUZZ_KEY)),
         "features": t.get("features", {}), "targets": []}
    pkgs[p["name"]] = p
    by_dir[p["dir"]] = p["name"]
    return p


for m in members:
    load(m, True)
for m in EXTRA:
    if exists(m):
        load(os.path.dirname(m) or ".", False)
    else:
        finding("format", f"EXTRA_MANIFESTS names {m}, which is missing")


def files(d, sub):
    base = os.path.join(ROOT, d, sub)
    if not os.path.isdir(base):
        return []
    r = []
    for e in sorted(os.listdir(base)):
        if e.endswith(".rs") and os.path.isfile(os.path.join(base, e)):
            r.append((e[:-3], os.path.join(sub, e)))
        elif os.path.isfile(os.path.join(base, e, "main.rs")):
            r.append((e, os.path.join(sub, e, "main.rs")))
    return r


for p in pkgs.values():
    t, d, pk = p["toml"], p["dir"], p["toml"]["package"]
    tg = p["targets"]
    lib = t.get("lib")
    if lib is not None or exists(os.path.join(d, "src/lib.rs")):
        lib = lib or {}
        name = lib.get("name", p["name"].replace("-", "_"))
        tg.append({"kind": "lib", "name": name, "req": set(), "test": lib.get("test", True)})
        if lib.get("doctest", True):
            tg.append({"kind": "doctest", "name": name, "req": set(), "test": True})

    def section(kind, key, auto_key, autos, deftest, default_path):
        expl = t.get(key, [])
        seen = set()
        for e in expl:
            path = e.get("path") or default_path(e["name"])
            seen.add(e["name"]); seen.add(os.path.normpath(path))
            tg.append({"kind": kind, "name": e["name"], "req": set(e.get("required-features", [])),
                       "test": e.get("test", deftest)})
        if pk.get(auto_key, True):
            for n, path in autos:
                if n not in seen and os.path.normpath(path) not in seen:
                    tg.append({"kind": kind, "name": n, "req": set(), "test": deftest})

    bin_autos = ([(pk["name"], "src/main.rs")] if exists(os.path.join(d, "src/main.rs")) else []) + files(d, "src/bin")
    section("fuzz-bin" if p["fuzz_bins"] else "bin", "bin", "autobins", bin_autos, True,
            lambda n: "src/main.rs" if n == pk["name"] else f"src/bin/{n}.rs")
    section("test", "test", "autotests", files(d, "tests"), True, lambda n: f"tests/{n}.rs")
    section("example", "example", "autoexamples", files(d, "examples"), False, lambda n: f"examples/{n}.rs")
    section("bench", "bench", "autobenches", files(d, "benches"), False, lambda n: f"benches/{n}.rs")

# ---- the rows -------------------------------------------------------------------------------
rows, keys = [], {}
if not exists(TARGETS):
    print(f"check-targets: no {TARGETS}", file=sys.stderr)
    sys.exit(2)
for i, line in enumerate(rd(TARGETS).split("\n"), 1):
    s = line.strip()
    if not s or s.startswith("#"):
        continue
    f = s.split("|")
    if len(f) != 5 or f[1] not in KINDS + ("feature", "ignore") or not f[0] or not f[2] or not f[4]:
        finding("format", f"{TARGETS}:{i}: {s}")
        continue
    feats = [x for x in f[3].split(",") if x]
    if feats != sorted(set(feats)):
        finding("format", f"{TARGETS}:{i}: features not sorted or repeated: {s}")
    r = {"line": i, "text": s, "pkg": f[0], "kind": f[1], "name": f[2], "feats": frozenset(feats),
         "parts": f[4], "pending": "pending ruling" in f[4]}
    k = (r["pkg"], r["kind"], r["name"], r["feats"])
    if k in keys:
        finding("format", f"{TARGETS}:{i}: a second row for {f[0]}|{f[1]}|{f[2]}|{f[3]} (first at line {keys[k]})")
        continue
    keys[k] = i
    rows.append(r)

# ---- the parts' commands --------------------------------------------------------------------
VALUED = {"-p", "--package", "--manifest-path", "--features", "-F", "--test", "--bin", "--example",
          "--bench", "--exclude", "--profile", "--cargo-profile", "-j", "--jobs", "--target",
          "--target-dir", "--color", "--message-format", "--config", "-Z", "--partition",
          "--retries", "--test-threads", "--run-ignored", "-E", "--filter-expr"}
CMD = re.compile(r"(?:(?<=^)|(?<=[\s;&|('\x22]))(cargo\s+(?:\+\S+\s+)?(test|nextest\s+run|build|check|clippy|fuzz\s+build)|tests)(?=\s)")


def parse_arms(text):
    lines = text.split("\n")
    start = next((i for i, l in enumerate(lines) if re.match(r"\s*case \$\{PART", l)), None)
    arms, cur, name = {}, [], None
    if start is None:
        return arms
    for l in lines[start + 1:]:
        if name is None:
            if re.match(r"\s*esac\b", l):
                break
            m = re.match(r"\s+([a-z0-9][a-z0-9-]*)\)\s?(.*)$", l)
            if m and not l.strip().startswith("#"):
                name, cur = m.group(1), [m.group(2)]
                if m.group(2).rstrip().endswith(";;"):
                    arms[name], name = cur, None
            continue
        cur.append(l)
        if l.rstrip().endswith(";;") and not l.strip().startswith("#"):
            arms[name], name = cur, None
    return arms


def commands(part, lines):
    """the cargo commands of one arm: (list of dicts, error or None)"""
    text = "\n".join(l for l in lines if not l.strip().startswith("#"))
    for m in re.finditer(r"(?:^|\s)\.\s+(tools/[A-Za-z0-9_/.-]+\.sh)", text):
        if not exists(m.group(1)):
            return [], f"it sources {m.group(1)}, which is missing"
        text += "\n" + "\n".join(l for l in rd(m.group(1)).split("\n") if not l.strip().startswith("#"))
    text = text.replace("\\\n", " ")
    env = {}
    for m in re.finditer(r"""([A-Z][A-Z0-9_]*)=\$\(for f in ([^;]+); do printf ' --test %s' "\$\(basename "\$f" \.rs\)"; done\)""", text):
        fs = sorted(glob.glob(os.path.join(ROOT, m.group(2).strip())))
        env[m.group(1)] = " ".join(f"--test {os.path.basename(x)[:-3]}" for x in fs)
    cmds, err = [], None
    for m in CMD.finditer(text):
        sub = re.sub(r"\s+", " ", m.group(2) or "tests")
        rest = text[m.end():]
        cut = re.search(r"\n|&&|\|\||;|\||'|\"|\)", rest)
        args = rest[:cut.start()] if cut else rest
        for v, val in env.items():
            args = re.sub(r"\$\{?" + v + r"\}?(?![A-Za-z0-9_])", val, args)
        if "$" in args:
            err = err or f"a command it can't read: {m.group(1)}{args.rstrip()}"
            continue
        try:
            toks = shlex.split(args)
        except ValueError as e:
            err = err or f"a command it can't read ({e}): {m.group(1)}{args.rstrip()}"
            continue
        c = {"sub": sub, "pkgs": [], "manifest": None, "feats": set(), "all": False, "flags": set(),
             "named": {"test": set(), "bin": set(), "example": set(), "bench": set()},
             "norun": False, "list": False, "text": (m.group(1) + args).strip()}
        k, harness = 0, False
        while k < len(toks):
            a = toks[k]
            if harness:
                if a == "--list":
                    c["list"] = True
                k += 1
                continue
            if a == "--":
                harness = True
            elif "=" in a and a.split("=", 1)[0] in VALUED:
                a, v = a.split("=", 1)
                toks[k:k + 1] = [a, v]
                continue
            elif a in VALUED:
                v = toks[k + 1] if k + 1 < len(toks) else ""
                k += 1
                if a in ("-p", "--package"):
                    c["pkgs"].append(v)
                elif a == "--manifest-path":
                    c["manifest"] = os.path.normpath(os.path.dirname(v) or ".")
                elif a in ("--features", "-F"):
                    c["feats"] |= {x for x in re.split(r"[ ,]+", v) if x}
                elif a[2:] in c["named"]:
                    c["named"][a[2:]].add(v)
            elif a == "--all-features":
                c["all"] = True
            elif a == "--no-run":
                c["norun"] = True
            elif a in ("--lib", "--doc", "--bins", "--tests", "--examples", "--benches", "--all-targets"):
                c["flags"].add(a[2:])
            k += 1
        cmds.append(c)
    return cmds, err


arms = parse_arms(rd(FULL)) if exists(FULL) else {}
parts = {}
for n, ls in arms.items():
    parts[n] = commands(n, ls)


def selected(c):
    """the packages a command selects"""
    if c["pkgs"]:
        return [p for p in c["pkgs"] if p in pkgs]
    if c["manifest"] is not None:
        return [by_dir[c["manifest"]]] if c["manifest"] in by_dir else []
    if c["sub"] == "fuzz build":
        return [p["name"] for p in pkgs.values() if p["fuzz_bins"]]
    return [p["name"] for p in pkgs.values() if p["member"]]


def runs(c):
    return c["sub"] in ("test", "nextest run", "tests") and not c["norun"] and not c["list"]


def testish(c):
    return c["sub"] in ("test", "nextest run", "tests") or bool(c["flags"] & {"tests", "all-targets"}) \
        or any(c["named"][x] for x in ("test", "bench", "example"))


def dep_edges(p, dev):
    """(package, features, default) for each workspace dependency of p"""
    t, r = p["toml"], []
    tables = [t.get("dependencies", {}), t.get("build-dependencies", {})]
    if dev:
        tables.append(t.get("dev-dependencies", {}))
    for tt in t.get("target", {}).values():
        tables += [tt.get("dependencies", {}), tt.get("build-dependencies", {})] + ([tt.get("dev-dependencies", {})] if dev else [])
    for tab in tables:
        for key, spec in tab.items():
            if not isinstance(spec, dict) or "path" not in spec:
                continue
            d = os.path.normpath(os.path.join(p["dir"], spec["path"]))
            if d in by_dir:
                r.append((by_dir[d], key, spec.get("features", []), spec.get("default-features", True), spec.get("optional", False)))
    return r


def enabled(c):
    """every (package, feature) a command turns on"""
    on, seen_pkg, work = set(), set(), []

    def activate(name, dev):
        if (name, dev) in seen_pkg:
            return
        seen_pkg.add((name, dev))
        p = pkgs[name]
        if "default" in p["features"]:
            work.append((name, "default"))
        for dn, key, fs, dflt, opt in dep_edges(p, dev):
            if opt:
                continue
            activate(dn, False)
            work.extend((dn, f) for f in fs)
            if dflt and "default" in pkgs[dn]["features"]:
                work.append((dn, "default"))

    def keyed(p, key):
        for dn, k, fs, dflt, opt in dep_edges(pkgs[p], True):
            if k == key:
                return dn
        return None

    for name in selected(c):
        activate(name, testish(c))
        req = set(pkgs[name]["features"]) if c["all"] else c["feats"]
        for f in req:
            if "/" in f:
                dp, ff = f.split("/", 1)
                if dp in pkgs:
                    work.append((dp, ff))
            else:
                work.append((name, f))
    while work:
        p, f = work.pop()
        if (p, f) in on or f not in pkgs[p]["features"]:
            continue
        on.add((p, f))
        for item in pkgs[p]["features"][f]:
            if item.startswith("dep:"):
                continue
            if "/" in item:
                dk, ff = item.split("/", 1)
                dn = keyed(p, dk.rstrip("?"))
                if dn:
                    activate(dn, False)
                    work.append((dn, ff))
            else:
                work.append((p, item))
    return on


def picks(c, p, t, on):
    """('run' | 'build' | None): what command c does with target t of package p"""
    if t["req"] and not all((p["name"], f) in on for f in t["req"]):
        return None
    fl, named, kind = c["flags"], c["named"], t["kind"]
    explicit = bool(fl) or any(named.values())
    sub = c["sub"]
    if sub == "fuzz build":
        return "build" if kind == "fuzz-bin" else None
    if kind == "doctest":
        if sub == "test" and ("doc" in fl or not explicit):
            return "build" if (c["norun"] or c["list"]) else "run"
        return None
    if "doc" in fl:
        return None
    tk = {"lib": "lib", "bin": "bin", "fuzz-bin": "bin", "test": "test", "example": "example", "bench": "bench"}[kind]
    if explicit:
        hit = (tk == "lib" and "lib" in fl) or (tk == "bin" and "bins" in fl) \
            or (tk in ("example", "bench") and tk + "s" in fl) or ("all-targets" in fl) \
            or ("tests" in fl and t["test"]) or (tk != "lib" and t["name"] in named[tk])
    elif sub in ("test", "nextest run", "tests"):
        hit = tk in ("lib", "bin", "test", "example") or (tk == "bench" and t["test"])
    else:
        hit = tk in ("lib", "bin")
    if not hit:
        return None
    if runs(c) and t["test"]:
        return "run"
    return "build"


def doc_examples(p):
    """runnable doc-comment code blocks in the package's src"""
    n = 0
    for dp, dn, fn in os.walk(os.path.join(ROOT, p["dir"], "src")):
        for f in fn:
            if not f.endswith(".rs"):
                continue
            with open(os.path.join(dp, f), encoding="utf-8") as fh:
                block = None
                for l in fh:
                    if re.search(r"#!?\[doc\s*=", l):
                        n += 1
                    m = re.match(r"\s*//[/!]\s?(.*)$", l)
                    if not m:
                        block = None
                        continue
                    s = m.group(1).strip()
                    if s.startswith("```"):
                        if block is None:
                            toks = [x.strip() for x in s[3:].split(",") if x.strip()]
                            if all(x in ("rust", "should_panic", "no_run", "compile_fail") or x.startswith("edition") for x in toks):
                                n += 1
                            block = True
                        else:
                            block = None
    return n


# ---- (a) new, (b) stale: targets and features ----------------------------------------------
have = {(r["pkg"], r["kind"], r["name"]) for r in rows}
real = set()
for p in pkgs.values():
    for t in p["targets"]:
        real.add((p["name"], t["kind"], t["name"]))
    for f in p["features"]:
        real.add((p["name"], "feature", f))
for k in sorted(real - have):
    finding("new", f"{k[0]}|{k[1]}|{k[2]} (no row in {TARGETS})")
for r in rows:
    if r["kind"] != "ignore" and (r["pkg"], r["kind"], r["name"]) not in real:
        finding("stale", f"{TARGETS}:{r['line']}: {r['text']} (no such {r['kind']})")


def part_names(spec):
    return [x.strip() for x in spec.split(",") if x.strip()]


def part_cmds(r, name):
    """the commands of part `name` this check can read; None (and a finding) if no such part"""
    if name not in parts:
        finding("unselected", f"{TARGETS}:{r['line']}: {r['text']} (no part {name} in {FULL})")
        return None
    return parts[name][0]


def miss(r, name, kind, what):
    """a row the part doesn't satisfy: `kind`, or unreadable when the part has a command this
    check can't read (it may be the one)"""
    err = parts[name][1]
    if err:
        finding("unreadable", f"{TARGETS}:{r['line']}: {r['text']} (part {name}: {err})")
    else:
        finding(kind, f"{TARGETS}:{r['line']}: {r['text']} ({what})")


def norm(c):
    return frozenset(["all"]) if c["all"] else frozenset(c["feats"])


# ---- (c) unselected, (d) feature -------------------------------------------------------------
for r in rows:
    k = r["kind"]
    if k == "ignore" or r["pkg"] not in pkgs:
        continue
    p = pkgs[r["pkg"]]
    if k == "feature":
        if r["parts"].startswith("product:") or r["pending"]:
            continue
        names = part_names(r["parts"])
        if not names:
            finding("feature", f"{TARGETS}:{r['line']}: {r['text']} (no part enables it)")
        for n in names:
            cmds = part_cmds(r, n)
            if cmds is not None and not any((r["pkg"], r["name"]) in enabled(c) for c in cmds):
                miss(r, n, "feature", f"part {n} doesn't enable it")
        continue
    t = next((t for t in p["targets"] if t["kind"] == k and t["name"] == r["name"]), None)
    if t is None:
        continue
    spec = r["parts"]
    if spec.startswith("excluded:"):
        if k == "doctest":
            n = doc_examples(p)
            if n:
                finding("unselected", f"{TARGETS}:{r['line']}: {r['text']} (excluded, but {r['pkg']}'s docs have {n} doc examples)")
        continue
    if spec.startswith("built-only:"):
        names, want = [spec[len("built-only:"):].split(":", 1)[0].strip()], ("build", "run")
    else:
        names, want = part_names(spec), ("run",)
    for n in names:
        cmds = part_cmds(r, n)
        if cmds is None:
            continue
        ok = False
        for c in cmds:
            if norm(c) != r["feats"] or r["pkg"] not in selected(c):
                continue
            if picks(c, p, t, enabled(c)) in want:
                ok = True
                break
        if not ok:
            how = "build" if want == ("build", "run") else "run"
            feats = ",".join(sorted(r["feats"])) or "no features"
            miss(r, n, "unselected", f"part {n} doesn't {how} it with {feats}")

# ---- (e) ignore --------------------------------------------------------------------------------
pending_ignores = {(r["pkg"], r["name"]): r for r in rows if r["kind"] == "ignore"}
for r in rows:
    if r["kind"] == "ignore" and not r["pending"]:
        finding("format", f"{TARGETS}:{r['line']}: {r['text']} (an ignore row is a pending ruling only)")
seen_ignores = set()


def names_test(path, fn):
    path = path.rstrip(".,;:)")
    if not exists(path):
        return False
    return re.search(r"(?<![A-Za-z0-9_])" + re.escape(fn) + r"(?![A-Za-z0-9_])", rd(path)) is not None


for p in pkgs.values():
    pdir = os.path.join(ROOT, p["dir"])
    for dp, dn, fn in os.walk(pdir):
        rel_dp = os.path.relpath(dp, pdir)
        dn[:] = sorted(x for x in dn if x != "target" and not os.path.isfile(os.path.join(dp, x, "Cargo.toml")))
        for f in sorted(fn):
            if not f.endswith(".rs"):
                continue
            rel = os.path.normpath(os.path.join(rel_dp, f))
            with open(os.path.join(dp, f), encoding="utf-8") as fh:
                lines = fh.read().split("\n")
            for i, l in enumerate(lines):
                s = l.strip()
                if not (s.startswith("#[ignore") or (s.startswith("#[cfg_attr") and re.search(r"\bignore\b", s))):
                    continue
                m = re.match(r'#\[ignore\s*=\s*"(.*)"\]\s*$', s)
                reason = m.group(1) if m else ""
                name = next((mm.group(1) for x in lines[i + 1:i + 12] for mm in [re.match(r"\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)", x)] if mm), None)
                where = f"{p['dir']}/{rel}:{i + 1}"
                if name is None or (not m and s != "#[ignore]"):
                    finding("ignore", f"{where}: an #[ignore] this check can't read: {s}")
                    continue
                key = (p["name"], f"{rel}::{name}")
                seen_ignores.add(key)
                scripts = [x for x in re.findall(r"(?:tools|scripts)/[A-Za-z0-9_./-]+\.sh", reason)]
                byname = re.findall(r"run by name: ([A-Za-z0-9_./-]+)", reason)
                ok = any(names_test(x, name) for x in scripts + byname)
                pr = pending_ignores.get(key)
                if ok and pr:
                    finding("stale", f"{TARGETS}:{pr['line']}: {pr['text']} (its #[ignore] names its runner now)")
                elif not ok and not pr:
                    finding("ignore", f"{where}: {name}: the reason names no runner that names it ({reason or 'no reason'})")
for key, r in pending_ignores.items():
    if key not in seen_ignores:
        finding("stale", f"{TARGETS}:{r['line']}: {r['text']} (no such #[ignore])")

# ---- pending ---------------------------------------------------------------------------------
pending = [r for r in rows if r["pending"]]
if LANDING:
    for r in pending:
        finding("pending", f"{TARGETS}:{r['line']}: {r['text']}")

for l in out:
    print(l)
ntargets = sum(len(p["targets"]) for p in pkgs.values())
if out:
    print(f"check-targets: {len(out)} findings ({ntargets} targets, {len(rows)} rows, {len(pending)} pending)")
    sys.exit(1)
print(f"check-targets: clean ({ntargets} targets, {len(rows)} rows, {len(pending)} pending)")
PY
