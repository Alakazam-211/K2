#!/usr/bin/env python3
"""The parity coverage gate: every verb form of the census
(crates/k5-cli/tests/data/spec_census.tsv) that no parity case drives.

    python3 scripts/parity-coverage.py           print the count and the uncovered forms by family
    python3 scripts/parity-coverage.py --check   the same, and exit 1 unless the uncovered forms
                                                 are exactly the committed ceiling
    python3 scripts/parity-coverage.py --write   write the ceiling (refuses a form that is newly
                                                 uncovered unless --accept-new is given too, and a
                                                 form that left the census unless --accept-gone)
    --root DIR                                   read the tree at DIR (the gate's own bite test)

A form is one census entry, or its --json twin when the entry takes
--json (K2's R4 ruling 7): `whoami` and `whoami --json`.

The link: a parity case says which forms it drives with a tag on the
line just above it, one form per tag, the form spelled as here:

    // covers: whoami --json        crates/k5-cli/tests/*_parity.rs
    # covers: compute stats         tests/cli/*.sh that cli-parity runs

A tag is plain text, so a reader sees the claim beside the case, grep
finds every case of a form, and a deleted case takes its tag with it.
The census names forms by words, not by test names, so neither side
has to be renamed for the other. A tag that names no census form fails
the gate, so a typo can't count.

The ceiling (crates/k5-cli/tests/data/parity_uncovered.tsv) is the list
of forms with no case. It may only fall: --check fails when a form is
uncovered that the ceiling doesn't list (a new census form with no case,
or a form whose last case or tag is gone), and when a form the ceiling
lists now has a case (run --write so the ceiling falls with it). The
ship bar is an empty ceiling.

K5-only forms (crates/k5-cli/tests/data/k5_only_forms.tsv, K2's ruling)
are census forms the shell has no answer of its own for: they are out
of the count, and each names the unit tests that cover it. The gate
fails when a listed form is no census form, when a named test is not a
#[test] fn in its file, and when a parity tag names a K5-only form. A
form leaves the count only by that list: a ceiling form that left the
census fails --check, and --write refuses it unless --accept-gone (a
form the shell really dropped). A covered form that leaves the census
fails by its tags.

A tag says a parity case drives the form, not that k5 answers it: a
case can run where both sides are bash. The gate prints the tagged
forms the native bar's ceiling (crates/k5-cli/tests/data/
native_bar_ceiling.tsv) still lists; the native bar, not this gate,
fails on them.
"""

import os
import re
import sys

CENSUS = "crates/k5-cli/tests/data/spec_census.tsv"
CEILING = "crates/k5-cli/tests/data/parity_uncovered.tsv"
K5_ONLY = "crates/k5-cli/tests/data/k5_only_forms.tsv"
NATIVE_BAR = "crates/k5-cli/tests/data/native_bar_ceiling.tsv"
RUST_TAG = re.compile(r"^\s*// covers: (.+?)\s*$")
SHELL_TAG = re.compile(r"^\s*# covers: (.+?)\s*$")


def forms(root):
    """The census forms in its order, each with its first word."""
    out = []
    with open(os.path.join(root, CENSUS), encoding="utf-8") as f:
        rows = [l.rstrip("\n").split("\t") for l in f if not l.startswith("#")]
    assert rows[0][:2] == ["entry", "forms"], rows[0]
    for r in rows[1:]:
        out.append(r[0])
        if r[1] == "2":
            out.append(r[0] + " --json")
    return out


def k5_only(root, census):
    """The K5-only forms -> their unit tests; and what is wrong with the list."""
    only, bad = {}, []
    with open(os.path.join(root, K5_ONLY), encoding="utf-8") as f:
        rows = [(n, l.rstrip("\n").split("\t")) for n, l in enumerate(f, 1)
                if l.strip() and not l.startswith("#")]
    if not rows or rows[0][1] != ["form", "tests"]:
        return only, ["%s: the first row is not form<TAB>tests" % K5_ONLY]
    for n, r in rows[1:]:
        at = "%s:%d" % (K5_ONLY, n)
        if len(r) != 2:
            bad.append("%s: want two columns, got %d" % (at, len(r)))
            continue
        form, tests = r
        if form not in census:
            bad.append("%s: %r is no census form" % (at, form))
        if form in only:
            bad.append("%s: %r is listed twice" % (at, form))
        names = [t.strip() for t in tests.split(",") if t.strip()]
        if not names:
            bad.append("%s: %r names no unit test" % (at, form))
        for t in names:
            rel, _, fn = t.partition("::")
            path = os.path.join(root, rel)
            text = ""
            if fn and os.path.isfile(path):
                with open(path, encoding="utf-8") as f:
                    text = f.read()
            if not re.search(r"#\[test\]\s*(?:#\[[^\]]*\]\s*)*fn %s\s*\(" % re.escape(fn or "-"), text):
                bad.append("%s: %r: no #[test] fn %s in %s" % (at, form, fn, rel))
        only[form] = names
    return only, bad


def families(root):
    """first word -> its family: the module under crates/k5-cli/src/families
    whose WORDS name it, or `(front)` for the front's own rows."""
    words = {}
    src = os.path.join(root, "crates/k5-cli/src")
    fam = os.path.join(src, "families")
    for name in sorted(os.listdir(fam)):
        if not name.endswith(".rs") or name == "mod.rs":
            continue
        with open(os.path.join(fam, name), encoding="utf-8") as f:
            text = f.read()
        m = re.search(r"shell_family!\(&\[(.*?)\]\);", text) or re.search(
            r"^pub const WORDS: &\[&str\] = &\[(.*?)\];", text, re.M
        )
        if not m:
            raise SystemExit("parity-coverage: no WORDS in families/%s" % name)
        for w in re.findall(r'"([^"]*)"', m.group(1)):
            words[w] = name[:-3]
    with open(os.path.join(src, "route.rs"), encoding="utf-8") as f:
        for m in re.finditer(r"^    front\(&\[(.*?)\]\),$", f.read(), re.M):
            for w in re.findall(r'"([^"]*)"', m.group(1)):
                words[w] = "(front)"
    return words


def parity_files(root):
    """The files whose tags count: every crates/k5-cli/tests/*_parity.rs,
    and the tests/cli scripts scripts/cli-parity.sh runs (not `_*`, not
    `# bash-only`)."""
    out = []
    tests = os.path.join(root, "crates/k5-cli/tests")
    for name in sorted(os.listdir(tests)):
        if name.endswith("_parity.rs"):
            out.append((os.path.join("crates/k5-cli/tests", name), RUST_TAG))
    cli = os.path.join(root, "tests/cli")
    for name in sorted(os.listdir(cli)) if os.path.isdir(cli) else []:
        if not name.endswith(".sh") or name.startswith("_"):
            continue
        with open(os.path.join(cli, name), encoding="utf-8", errors="replace") as f:
            if any(l.startswith("# bash-only") for l in f):
                continue
        out.append((os.path.join("tests/cli", name), SHELL_TAG))
    return out


def tags(root, known, only):
    """form -> the places that tag it; and the tags that name no form."""
    covered, bad = {}, []
    for rel, pat in parity_files(root):
        with open(os.path.join(root, rel), encoding="utf-8", errors="replace") as f:
            for n, line in enumerate(f, 1):
                m = pat.match(line)
                if not m:
                    continue
                if m.group(1) in known:
                    covered.setdefault(m.group(1), []).append("%s:%d" % (rel, n))
                elif m.group(1) in only:
                    bad.append("%s:%d: %r is K5-only (%s): its unit tests cover it, not a parity case"
                               % (rel, n, m.group(1), K5_ONLY))
                else:
                    bad.append("%s:%d: %r is no census form" % (rel, n, m.group(1)))
    return covered, bad


def still_bash(root):
    """The forms the native bar's ceiling lists in a counted class: k5
    still hands them to bash (crates/k5-cli/tests/native_bar.rs)."""
    path = os.path.join(root, NATIVE_BAR)
    if not os.path.isfile(path):
        return set()
    with open(path, encoding="utf-8") as f:
        rows = [l.rstrip("\n").split("\t") for l in f if l.strip() and not l.startswith("#")]
    return {r[2] for r in rows[1:] if r[0] != "P"}


def family_of(form, words):
    return words.get(form.split(" ")[0], "(no row)")


def read_ceiling(root):
    path = os.path.join(root, CEILING)
    if not os.path.isfile(path):
        return None
    with open(path, encoding="utf-8") as f:
        return [l.rstrip("\n").split("\t")[1] for l in f if l.strip() and not l.startswith("#")]


def ceiling_text(uncovered, total, words, only):
    lines = [
        "# GENERATED by scripts/parity-coverage.py: the census forms no parity case drives.",
        "# It may only fall (the gate fails when it rises); the ship bar is none.",
        "# %d of %d forms uncovered; %d K5-only forms out of the count (%s)"
        % (len(uncovered), total, len(only), K5_ONLY),
    ]
    lines += ["%s\t%s" % (family_of(f, words), f) for f in uncovered]
    return "\n".join(lines) + "\n"


def main(argv):
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    mode, accept_new, accept_gone = "print", False, False
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--root" and args:
            root = args.pop(0)
        elif a in ("--check", "--write"):
            mode = a[2:]
        elif a == "--accept-new":
            accept_new = True
        elif a == "--accept-gone":
            accept_gone = True
        else:
            print("parity-coverage: unknown argument: %s" % a, file=sys.stderr)
            return 2
    census = forms(root)
    only, bad = k5_only(root, set(census))
    all_forms = [f for f in census if f not in only]
    words = families(root)
    covered, tag_bad = tags(root, set(all_forms), only)
    bad += tag_bad
    uncovered = [f for f in all_forms if f not in covered]
    by = {}
    for f in uncovered:
        by.setdefault(family_of(f, words), []).append(f)
    print("parity-coverage: %d of %d forms have no parity case (%d covered); %d K5-only out of the count: %s"
          % (len(uncovered), len(all_forms), len(all_forms) - len(uncovered), len(only),
             ", ".join(only) or "none"))
    for fam in sorted(by, key=lambda k: (-len(by[k]), k)):
        print("  %-16s %4d  %s" % (fam, len(by[fam]), ", ".join(by[fam])))
    # A tag counts for parity, not nativeness: a tagged form the native
    # bar still lists falls through, so both sides ran bash. Reported, not
    # counted against the parity ceiling: the native bar is the gate for it.
    bash = sorted(f for f in covered if f in still_bash(root))
    if bash:
        print("parity-coverage: %d tagged forms still fall through (%s lists them): %s"
              % (len(bash), NATIVE_BAR, ", ".join(bash)))
    for b in bad:
        print("parity-coverage: " + b, file=sys.stderr)
    ceiling = read_ceiling(root)
    if mode == "print":
        return 1 if bad else 0
    new = [f for f in uncovered if ceiling is None or f not in ceiling]
    # Ceiling forms that left the census and are not K5-only: a form may leave the count only
    # by the K5-only list (or by --accept-gone, when the shell really dropped it).
    gone = [f for f in (ceiling or []) if f not in census]
    if mode == "write":
        if bad:
            return 1
        if gone and not accept_gone:
            print("parity-coverage: these left the census and are not on the K5-only list (%s); list them there, or add --accept-gone if the shell dropped them:" % K5_ONLY, file=sys.stderr)
            for f in gone:
                print("  " + f, file=sys.stderr)
            return 1
        if new and ceiling is not None and not accept_new:
            print("parity-coverage: these forms are newly uncovered; give a case for each, or add --accept-new (a new census form):", file=sys.stderr)
            for f in new:
                print("  " + f, file=sys.stderr)
            return 1
        with open(os.path.join(root, CEILING), "w", encoding="utf-8") as f:
            f.write(ceiling_text(uncovered, len(all_forms), words, only))
        print("parity-coverage: wrote %s" % CEILING)
        return 0
    rc = 1 if bad else 0
    if ceiling is None:
        print("parity-coverage: no ceiling at %s: run --write" % CEILING, file=sys.stderr)
        return 1
    if new:
        rc = 1
        print("parity-coverage: the uncovered forms ROSE: no parity case drives these, and the ceiling doesn't list them:", file=sys.stderr)
        for f in new:
            print("  %s\t%s" % (family_of(f, words), f), file=sys.stderr)
    if gone:
        rc = 1
        print("parity-coverage: these left the census and are not on the K5-only list (%s); list them there, or run --write --accept-gone if the shell dropped them:" % K5_ONLY, file=sys.stderr)
        for f in gone:
            print("  " + f, file=sys.stderr)
    fell = [f for f in ceiling if f not in uncovered and f not in gone]
    if fell:
        rc = 1
        print("parity-coverage: the ceiling is stale: these now have a case or are K5-only; run python3 scripts/parity-coverage.py --write and commit it:", file=sys.stderr)
        for f in fell:
            print("  " + f, file=sys.stderr)
    if rc == 0:
        print("parity-coverage: the ceiling holds (%d)" % len(uncovered))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
