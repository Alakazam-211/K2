#!/usr/bin/env python3
"""The k2-literal gate: text the server sends to people may not gain a
literal `k2 <word>` while rebrand step B7 turns those lines into templates.

    python3 scripts/garden/k2-literal-gate.py             check; exit 1 on a failure
    python3 scripts/garden/k2-literal-gate.py --tighten   lower each ceiling to today's
                                                          count, drop rows at zero
    python3 scripts/garden/k2-literal-gate.py --list [FAMILY]
                                                          print every counted line
    python3 scripts/garden/k2-literal-gate.py --base REF  name the new lines against
                                                          REF when a family is over

What is counted: a line of crates/k2-daemon/src, crates/k2-core/src or
crates/k2-client/src where a Rust string literal holds `k2 ` and a lowercase
letter, with no letter, digit, `_`, `.`, `/`, `$` or `-` just before the k2
(the pattern of the gardener's hint inventory). Each line counts once.

Not counted, so the count is the text a CLI prints for a person:
  - test files (tests.rs, *_test.rs, *_tests.rs, test_*.rs, testkit.rs,
    anything under a tests/ folder) and a file's inline test module (the
    first `#[cfg(test)]` or `#[cfg(all(test, ...))]` line followed by a
    `mod name {` with a body; `mod name;` is a declaration and is skipped
    over, the module's file is a test file);
  - comments, doc comments and code outside strings;
  - a line whose statement writes to a log or the audit trail:
    log_debug!, log_info!, log_warn!, log_error!, eprintln!, println!,
    tracing::, log::, audit( or audit:: in the statement (read back to the
    line before it that ends in `;`, `{`, `}` or `=>`, at most 30 lines);
  - the `skip` rows of the ceilings file: a path (a file, or a folder
    ending in `/`) and the reason the text is not a CLI's (skill files
    written for agents, the refusal registry the CLI carries its own copy
    of, App templates written into a repo, and so on). A skip row for a
    path that does not exist is an error.

The ceilings file, scripts/garden/k2-literal-ceilings.txt, holds one
`<count> <family>` row per family (the families of the hint inventory, read
from the file's path by FAMILY below). Two rules, as the brand gate:
  1. A family may not have more lines than its row.
  2. A family with no row may have none.
So the counts can only fall. When this fails, send the text as a template
with {cli} (hintTemplate and the like) or build it with CLI_NAME. It prints
the lines it takes for new: those not in the file at --base (default: HEAD
when the family's files have changes not committed, else HEAD~1).

For tests: --root DIR and --ceilings FILE point it at another tree.
"""
import collections
import importlib.util
import os
import re
import subprocess
import sys

CRATES = ("crates/k2-daemon/src", "crates/k2-core/src", "crates/k2-client/src")
LITERAL = re.compile(r"(?:^|[^A-Za-z0-9_./$-])(k2 [a-z])")
TEST_FILE = re.compile(r"(^tests?\.rs$|_tests?\.rs$|^tests?_.*\.rs$|^testkit\.rs$)")
TEST_CFG = re.compile(r"\s*#\[cfg\((all\()?test[,)]")
TEST_MOD = re.compile(r"\s*(pub(\([a-z]+\))? )?mod \w+")
LOG = re.compile(r"log_(debug|info|warn|error)!|eprintln!|println!|tracing::|\blog::|audit\(|audit::")
HINT = "send it as a {cli} template (hintTemplate and the like) or build it with CLI_NAME"

# Family by path, first match wins: a prefix of the path under src/, or a
# word inside its first part. The hint inventory's table, so its families
# and these rows line up.
FAMILY = [
    ("permission/actions/", "permission card checks and applies"), ("permission/", "permission"),
    ("compute", "compute"), ("ci_", "ci"), ("apps/", "apps"), ("app_", "apps"), ("skin", "apps"),
    ("sew_chat", "chat"), ("sew/", "apps (Sew)"), ("hostchat", "chat"), ("chat", "chat"),
    ("vault", "vault"), ("migrate_secrets", "vault"), ("mail/", "mail"), ("email_notify", "mail"),
    ("hire", "agent hire"), ("agents_routes", "agent"), ("agent", "agent"), ("sql/", "db"), ("db_", "db"),
    ("docs/", "doc"), ("doc", "doc"), ("inbox", "inbox"), ("postal", "msg/postal"), ("msg", "msg/postal"),
    ("firewall", "firewall"), ("org_chart", "organization"), ("zen", "zen"), ("garden", "zen"),
    ("workspace", "workspace"), ("publish", "publish"), ("connect", "connect"), ("voice", "voice"),
    ("sim", "sim"), ("desktop", "desktop"), ("heartbeat", "heartbeat"), ("feedback", "feedback"),
    ("passport", "passport"), ("update", "update"), ("node", "compute"), ("jscheck", "zen"),
    ("overlay", "thread"), ("sidecar", "agent sidecar"), ("clone", "clone"), ("federation", "fed"),
    ("front443", "front"), ("board", "board"), ("domains", "domain"), ("jobs", "job"), ("llm", "llm"),
    ("folder_trust", "workspace trust"), ("cli_folder_trust", "workspace trust"), ("first_run", "setup"),
    ("approve", "approve"), ("capabilities", "setup"), ("handoff", "vault"), ("site_gate", "publish"),
    ("ticket", "tickets"), ("terminal", "terminal"), ("git", "git tools"), ("deploy", "apps"),
    ("suite", "apps"), ("contract", "contract (API docs)"), ("sources", "sources"),
]

# Strings and comments, by the CLI-name flip's own reader (loaded by path:
# its file name has hyphens). No bytecode is written.
sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location(
    "cli_name_flip", os.path.join(os.path.dirname(os.path.abspath(__file__)), "cli-name-flip.py"))
flip = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(flip)


def family(rel):
    if rel.startswith("crates/k2-client/"):
        return "k2-client"
    tail = re.sub(r"^crates/k2-(daemon|core)/src/", "", rel)
    for key, name in FAMILY:
        if tail.startswith(key) or ("/" not in key and key in tail.split("/")[0]):
            return name
    return "other"


def test_module_start(lines):
    """The index of the first inline test module's `#[cfg(test)]`, or the
    file's length."""
    for i, line in enumerate(lines[:-1]):
        nxt = lines[i + 1].rstrip()
        if TEST_CFG.match(line) and TEST_MOD.match(nxt) and not nxt.endswith(";"):
            return i
    return len(lines)


def logged(lines, i):
    """The statement line `i` sits in writes to a log or the audit trail."""
    j = i
    while j > 0 and i - j < 30:
        prev = lines[j - 1].rstrip()
        if prev.endswith((";", "{", "}")) or (prev.endswith("=>") and not prev.strip().startswith('"')):
            break
        j -= 1
    return bool(LOG.search("\n".join(lines[j:i + 1])))


def file_hits(path):
    """[(line number, text)] of one file's counted lines."""
    with open(path, encoding="utf-8", errors="replace") as f:
        text = f.read()
    if not LITERAL.search(text):
        return []
    lines = text.split("\n")
    end = test_module_start(lines)
    text = "\n".join(lines[:end])
    ctx = flip.rust_contexts(text)
    starts = [0]
    for line in lines[:end - 1]:
        starts.append(starts[-1] + len(line) + 1)
    seen = set()
    for i, line in enumerate(lines[:end]):
        for m in LITERAL.finditer(line):
            if ctx[starts[i] + m.start(1)] == "string":
                seen.add(i)
                break
    return [(i + 1, lines[i].strip()) for i in sorted(seen) if not logged(lines, i)]


def read_ceilings(path, root):
    head, rows, skips = [], {}, {}
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f.read().split("\n"), 1):
            if raw.startswith("#") or not raw.strip():
                if not rows and not skips:
                    head.append(raw)
                continue
            if raw.startswith("skip "):
                parts = raw.split(" ", 2)
                if len(parts) != 3 or not parts[2].strip():
                    sys.exit(f"k2-literal-gate: {path} line {n}: want 'skip <path> <reason>', got '{raw}'")
                p = parts[1]
                if not p.startswith(tuple(c + "/" for c in CRATES)):
                    sys.exit(f"k2-literal-gate: {path} line {n}: {p} is not under {', '.join(CRATES)}")
                full = os.path.join(root, p)
                if not (os.path.isdir(full) if p.endswith("/") else os.path.isfile(full)):
                    sys.exit(f"k2-literal-gate: {path} line {n}: skip {p}: no such {'folder' if p.endswith('/') else 'file'}; drop the row")
                skips[p] = parts[2]
                continue
            parts = raw.split(" ", 1)
            if len(parts) != 2 or not parts[0].isdigit() or int(parts[0]) < 1 or not parts[1].strip():
                sys.exit(f"k2-literal-gate: {path} line {n}: want '<count> <family>' or 'skip <path> <reason>', got '{raw}'")
            if parts[1] in rows:
                sys.exit(f"k2-literal-gate: {path} line {n}: {parts[1]} is listed twice")
            rows[parts[1]] = int(parts[0])
    return head, rows, skips


def write_ceilings(path, head, counts, skips):
    body = [f"skip {p} {why}" for p, why in sorted(skips.items())] + [""]
    body += [f"{n} {fam}" for fam, n in sorted(counts.items(), key=lambda r: (-r[1], r[0])) if n]
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(head + body) + "\n")


def scan(root, skips):
    """{family: [(relative path, line number, text)]}."""
    found = collections.defaultdict(list)
    for top in CRATES:
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, top)):
            dirnames[:] = sorted(d for d in dirnames if d != "tests")
            for name in sorted(filenames):
                if not name.endswith(".rs") or TEST_FILE.search(name):
                    continue
                full = os.path.join(dirpath, name)
                rel = os.path.relpath(full, root).replace(os.sep, "/")
                if rel in skips or any(s.endswith("/") and rel.startswith(s) for s in skips):
                    continue
                for n, text in file_hits(full):
                    found[family(rel)].append((rel, n, text))
    return found


def git(root, *args):
    return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True)


def new_lines(root, hits, base):
    """The hits whose text is not in their file at `base` (a line that is
    there more times now than then counts as new), or None when git can't
    tell."""
    files = sorted({rel for rel, _, _ in hits})
    top = git(root, "rev-parse", "--show-toplevel")
    if top.returncode != 0 or os.path.realpath(top.stdout.strip()) != os.path.realpath(root):
        return None
    if base is None:
        if git(root, "rev-parse", "--verify", "-q", "HEAD").returncode != 0:
            return None
        base = "HEAD" if git(root, "diff", "--quiet", "HEAD", "--", *files).returncode == 1 else "HEAD~1"
    if git(root, "rev-parse", "--verify", "-q", base + "^{commit}").returncode != 0:
        return None
    out = []
    for rel in files:
        old = git(root, "show", f"{base}:{rel}")
        have = collections.Counter(l.strip() for l in old.stdout.split("\n")) if old.returncode == 0 else collections.Counter()
        for r, n, text in hits:
            if r != rel:
                continue
            if have[text]:
                have[text] -= 1
            else:
                out.append((r, n, text))
    return base, out


def main(argv):
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    ceilings, mode, only, base = None, "check", None, None
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--tighten":
            mode = "tighten"
        elif a == "--list":
            mode = "list"
            if args and not args[0].startswith("--"):
                only = args.pop(0)
        elif a == "--base" and args:
            base = args.pop(0)
        elif a == "--root" and args:
            root = args.pop(0)
        elif a == "--ceilings" and args:
            ceilings = args.pop(0)
        else:
            sys.exit(f"k2-literal-gate: unknown argument: {a}")
    ceilings = ceilings or os.path.join(root, "scripts", "garden", "k2-literal-ceilings.txt")
    head, rows, skips = read_ceilings(ceilings, root)
    found = scan(root, skips)

    if mode == "list":
        for fam in sorted(found):
            if only in (None, fam):
                for rel, n, text in found[fam]:
                    print(f"{fam}: {rel}:{n}: {text}")
        return 0

    counts = {fam: len(v) for fam, v in found.items()}
    over = sorted(fam for fam in counts if counts[fam] > rows.get(fam, 0))
    for fam in over:
        print(f"k2-literal-gate: FAIL {fam} has {counts[fam]} lines with a literal 'k2 <word>', "
              f"its ceiling is {rows.get(fam, 0)}: {HINT}.")
        told = new_lines(root, found[fam], base)
        if told is None:
            print(f"  (git can't tell which lines are new; --list '{fam}' prints them all)")
            continue
        ref, new = told
        for rel, n, text in new:
            print(f"  new since {ref}: {rel}:{n}: {text[:160]}")
        if not new:
            print(f"  (no line is new since {ref}; --list '{fam}' prints them all)")
    if over:
        if mode == "tighten":
            print("k2-literal-gate: --tighten never raises a ceiling.")
        return 1
    if mode == "tighten":
        write_ceilings(ceilings, head, counts, skips)
        lowered = [f"{fam} {rows[fam]} -> {counts.get(fam, 0)}" for fam in sorted(rows) if counts.get(fam, 0) < rows[fam]]
        for line in lowered:
            print(f"k2-literal-gate: {line}")
        print(f"k2-literal-gate: ceilings tightened: {sum(counts.values())} lines in {len([n for n in counts.values() if n])} families.")
        return 0
    slack = [fam for fam in rows if counts.get(fam, 0) < rows[fam]]
    if slack:
        print(f"k2-literal-gate: {len(slack)} famil{'y is' if len(slack) == 1 else 'ies are'} below the ceiling "
              f"({', '.join(sorted(slack))}); run scripts/garden/k2-literal-gate.py --tighten to lock that in.")
    print(f"k2-literal-gate: ok ({sum(counts.values())} lines in {len(counts)} families still type 'k2 <word>' "
          f"in text for a person; {len(skips)} paths skipped).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
