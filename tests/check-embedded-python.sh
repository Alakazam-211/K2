#!/usr/bin/env bash
# Guardrail: compile-check every embedded Python snippet in the repo.
#
# WHY THIS EXISTS
# --------------
# Our bash CLIs (`cli/k2so`, test scripts) embed Python via `python3 -c '...'`.
# A backslash inside an f-string replacement field — e.g. `f"{d[\"k\"]}"` — is a
# HARD SyntaxError on Python 3.12+ (PEP 701 changed f-string tokenization). The
# bug is invisible to every other test suite (the Python lives in *strings*
# inside a bash file) and is quote-sensitive:
#   * single-quoted bash block  `python3 -c '...'`  → bash passes `\"` through
#     literally → Python sees the backslash → SyntaxError on 3.12+.  ← BITES
#   * double-quoted bash block  `python3 -c "..."`  → bash unescapes `\"`→`"`
#     BEFORE Python compiles → fine on 3.12+.                          ← safe
# It shipped broken in `k2so tunnel|companion` (fixed cedc8b3 / 3452e92) because
# nothing executed those blocks under a modern Python. This check closes that
# gap: it extracts and `compile()`s every block under the running interpreter,
# simulating bash's own unescaping for double-quoted blocks, and FAILS LOUDLY
# (exit 1) on the first SyntaxError — file, and a snippet of the block.
#
# Run it under the NEWEST Python you support (3.12+) to catch the f-string trap.
#
# Usage:  ./tests/check-embedded-python.sh
# Exit:   0 = all blocks compile · 1 = at least one broken block · 2 = setup error
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 not found — cannot run the embedded-Python check." >&2
    exit 2
fi

echo "Embedded-Python compile check (interpreter: $(python3 --version 2>&1))"
echo "Repo: $REPO_ROOT"
echo

# The checker is Python (it needs the interpreter to compile-check anyway).
# It receives the repo root on argv and walks the shell sources itself.
REPO_ROOT="$REPO_ROOT" python3 - "$REPO_ROOT" <<'PYEOF'
import os, sys, glob, re

repo = sys.argv[1]

# Shell sources that embed `python3 -c`. Globbed relative to the repo root.
PATTERNS = ["cli/k2so", "cli/*.sh", "tests/**/*.sh", "scripts/**/*.sh"]
EXCLUDE_SUBSTR = ("/.claude/worktrees/", "/target/", "/node_modules/", "/.git/")
# Never scan this checker itself — its source literally contains `python3 -c `
# markers + extractor code, which would self-match into noise.
SELF = os.path.realpath(__file__) if "__file__" in dir() else ""

files = []
for pat in PATTERNS:
    for f in glob.glob(os.path.join(repo, pat), recursive=True):
        rp = os.path.realpath(f)
        if rp.endswith("check-embedded-python.sh"):
            continue
        if not any(s in f for s in EXCLUDE_SUBSTR) and os.path.isfile(f):
            files.append(f)
files = sorted(set(files))


def unescape_bash_dquote(s):
    r"""Mimic bash's processing INSIDE a double-quoted string: only the chars
    backslash, double-quote, dollar, backtick and a backslash-newline (line
    continuation) are special; every other backslash is preserved verbatim.
    This is what reaches Python's compiler for a `python3 -c "..."` block, and
    it's why double-quoted blocks don't hit the f-string-backslash trap."""
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            nxt = s[i + 1]
            if nxt in ('"', "\\", "$", "`"):
                out.append(nxt); i += 2; continue
            if nxt == "\n":
                i += 2; continue  # line continuation: both chars vanish
            out.append(c); i += 1; continue  # backslash preserved literally
        out.append(c); i += 1
    return "".join(out)


# Bash expansions that get substituted at RUNTIME in a double-quoted block.
# We can't know their values statically, so we swap each for a neutral
# placeholder identifier and structurally compile-check what remains — that
# still catches the f-string-backslash trap (independent of $-substitution)
# without false-flagging `fallback = $fallback` or `python3 -c "$script"`.
_BASH_EXPANSIONS = [
    re.compile(r"\$\([^()]*\)"),          # $( ... ) command substitution
    re.compile(r"`[^`]*`"),               # ` ... ` command substitution
    re.compile(r"\$\{[^}]*\}"),           # ${ ... } parameter expansion
    re.compile(r"\$[A-Za-z_][A-Za-z0-9_]*"),  # $NAME
    re.compile(r"\$[0-9@*?#!$-]"),        # $1, $@, $?, ...
]


def neutralize_bash_expansions(s):
    for rx in _BASH_EXPANSIONS:
        s = rx.sub("XVAR", s)
    return s


def extract_blocks(text):
    """Yield (quote_char, raw_code, start_line) for every `python3 -c <q>...<q>`
    block. Handles env-var-prefixed invocations, single+double quotes, and
    multi-line bodies. Single quotes have no bash escaping (closing = next ');
    double quotes honor backslash-escaped closing quotes."""
    n = len(text)
    idx = 0
    marker = "python3 -c "
    while True:
        p = text.find(marker, idx)
        if p == -1:
            return
        q_pos = p + len(marker)
        if q_pos >= n or text[q_pos] not in ("'", '"'):
            idx = q_pos
            continue
        quote = text[q_pos]
        j = q_pos + 1
        if quote == "'":
            end = text.find("'", j)            # no escaping inside '...'
        else:
            end = j
            while end < n:                     # honor \" inside "..."
                if text[end] == "\\":
                    end += 2; continue
                if text[end] == '"':
                    break
                end += 1
        if end == -1 or end >= n:
            return
        code = text[j:end]
        start_line = text.count("\n", 0, j) + 1
        yield quote, code, start_line
        idx = end + 1


total = 0
broken = []
for f in files:
    try:
        txt = open(f, encoding="utf-8").read()
    except Exception:
        continue
    for quote, raw, line in extract_blocks(txt):
        if quote == "'":
            # Single quotes: no bash interpolation. Compile verbatim — this is
            # exactly what Python sees, and where the f-string-backslash trap
            # actually bites.
            code = raw
        else:
            # Double quotes: bash unescapes, then interpolates $-expansions.
            code = neutralize_bash_expansions(unescape_bash_dquote(raw))
        if not code.strip():
            continue
        total += 1
        try:
            compile(code, f, "exec")
        except SyntaxError as e:
            snippet = (code.strip().split("\n") or [""])[0][:60]
            rel = os.path.relpath(f, repo)
            broken.append((rel, line, e.msg, snippet))

print(f"Scanned {len(files)} shell files · {total} embedded python3 -c blocks")
if broken:
    print(f"\nFAILED: {len(broken)} broken block(s):\n")
    for rel, line, msg, snip in broken:
        print(f"  {rel}:{line}  {msg}")
        print(f"      ↳ {snip!r}")
    print("\nHint: a backslash inside an f-string {{}} field breaks on Python 3.12+.")
    print("Fix: hoist the quoted sub-expression to a variable, e.g.")
    print('  BEFORE  print(f"x: {d[\\"k\\"]}")')
    print('  AFTER   v = d["k"]; print(f"x: {v}")')
    sys.exit(1)

print("OK: every embedded Python block compiles cleanly.")
PYEOF
