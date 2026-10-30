#!/usr/bin/env python3
"""Check the golden tool corpus against the real GNU tools (K-PAR-7).

Run on Linux with GNU coreutils, grep, sed, diffutils and patch in PATH, in a UTF-8 locale:

    LC_ALL=C.UTF-8 python3 -I scripts/gnu-parity.py [crates/base/tests/golden/tools.golden]

For every case marked `gnu: same` it writes the doc to `notes.md` in a temp folder, runs the
GNU tool the verb mirrors, and compares stdout (or, for edit verbs, the file afterwards and
patch's report) and the exit code. Cases marked `gnu: k2` are K2-specific and skipped.
Exits 1 on any mismatch, listing each one. Uses only the standard library.
"""

import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT = os.path.join(HERE, "..", "crates", "base", "tests", "golden", "tools.golden")


def strings(s, line):
    out, i = [], 0
    while True:
        while i < len(s) and s[i].isspace():
            i += 1
        if i == len(s):
            return out
        if s[i] != '"':
            raise SystemExit(f"line {line}: expected a quoted string")
        i += 1
        v = []
        while True:
            c = s[i]
            i += 1
            if c == '"':
                break
            if c == "\\":
                e = s[i]
                i += 1
                if e == "u":
                    j = s.index("}", i)
                    v.append(chr(int(s[i + 1:j], 16)))
                    i = j + 1
                else:
                    v.append({"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"'}[e])
            else:
                v.append(c)
        out.append("".join(v))


def corpus(path):
    cases = []
    for n, raw in enumerate(open(path, encoding="utf-8").read().split("\n"), 1):
        if not raw.strip() or raw.startswith("#"):
            continue
        if raw.startswith("=== "):
            cases.append({"id": raw[4:].strip(), "line": n, "stdin": "", "gnu": "same"})
            continue
        key, val = raw.split(":", 1)
        c = cases[-1]
        if key == "argv":
            c["argv"] = strings(val, n)
        elif key in ("exit",):
            c[key] = int(val)
        elif key in ("err", "gnu"):
            c[key] = val.strip()
        else:
            (c[key],) = strings(val, n)
    return cases


# grep's options that take a value (GNU grep 3.11): short letters, and the long names
# (matched by unambiguous prefix, as getopt_long does, when given without `=VALUE`)
GREP_SHORT_ARG = "efmABCdD"
GREP_LONG_ARG = ("regexp", "file", "max-count", "after-context", "before-context", "context",
                 "label", "binary-files", "devices", "directories", "exclude", "exclude-from",
                 "exclude-dir", "include", "group-separator")


def grep_has_pattern(args):
    """True when grep's arguments name a pattern (an operand, `-e`/`-f` or their long forms).

    With no pattern, GNU grep exits 2 with its usage message. The doc path must then not be
    appended: grep would take it as the pattern and read stdin (case `grep-no-pattern`)."""
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--":
            return i + 1 < len(args)
        if a.startswith("--"):
            name, eq, _ = a[2:].partition("=")
            hits = [o for o in GREP_LONG_ARG if o.startswith(name)] if name else []
            takes = len(hits) == 1 or name in GREP_LONG_ARG
            full = name if name in GREP_LONG_ARG else (hits[0] if len(hits) == 1 else "")
            if takes:
                if full in ("regexp", "file"):
                    return True
                if not eq:
                    i += 1
        elif a.startswith("-") and a != "-":
            for j, ch in enumerate(a[1:], 1):
                if ch in GREP_SHORT_ARG:
                    if ch in "ef":
                        return True
                    if j == len(a) - 1:
                        i += 1  # the value is the next argument
                    break  # the rest of the cluster is the value
        else:
            return True
        i += 1
    return False


def run(cmd, cwd, stdin=b""):
    p = subprocess.run(cmd, cwd=cwd, input=stdin, capture_output=True)
    return p.stdout, p.returncode


def check(c):
    argv = c["argv"]
    verb, args = argv[0], argv[1:]
    data = (c.get("file") if "file" in c else c["text"]).encode("utf-8")
    name = c.get("name", "notes.md")
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, name)
        with open(path, "wb") as f:
            f.write(data)
        stdin = c["stdin"].encode("utf-8")
        if verb in ("cat", "head", "tail", "wc", "grep") or (verb == "sed" and not any(
                a == "-i" or a.startswith("--in-place") for a in args)):
            files = [] if verb == "grep" and not grep_has_pattern(args) else [name]
            out, code = run([verb, *args, *files], d)
            return out, code, None
        if verb == "view":
            spec = args[-1].split("=", 1)[-1]
            a, _, b = spec.partition(",")
            out, code = run(["sed", "-n", f"{a},{b or a}p", name], d)
            return out, code, None
        if verb == "diff":
            with open(os.path.join(d, "old"), "wb") as f:
                f.write(stdin)
            labels = [] if "--label" in args else ["--label", name, "--label", name]
            out, code = run(["diff", *args, *labels, "old", name], d)
            return out, code, None
        if verb == "sed":
            out, code = run(["sed", *args, name], d)
            return None, code, open(path, "rb").read()
        if verb == "patch":
            out, code = run(["patch", *args, name], d, stdin)
            # GNU names the file as given on the command line
            return out, code, open(path, "rb").read()
        raise ValueError(f"no GNU mapping for {verb}")


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT
    bad, ran = [], 0
    for c in corpus(path):
        if c.get("gnu", "same") != "same":
            continue
        ran += 1
        try:
            out, code, after = check(c)
        except Exception as e:  # report and continue
            bad.append(f"{c['id']} (line {c['line']}): {e}")
            continue
        want_exit = c["exit"]
        problems = []
        if code != want_exit:
            problems.append(f"exit {code}, want {want_exit}")
        if after is None:
            if want_exit != 2 and out != c["out"].encode("utf-8"):
                problems.append(f"stdout {out!r}, want {c['out'].encode()!r}")
        else:
            want = c.get("out")
            if want is not None and after != want.encode("utf-8"):
                problems.append(f"file {after!r}, want {want.encode()!r}")
            if "say" in c and out is not None and out != c["say"].encode("utf-8"):
                problems.append(f"report {out!r}, want {c['say'].encode()!r}")
        if problems:
            bad.append(f"{c['id']} (line {c['line']}): " + "; ".join(problems))
    print(f"gnu-parity: {ran} cases compared, {len(bad)} mismatches")
    for b in bad:
        print("  " + b)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
