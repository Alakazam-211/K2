#!/usr/bin/env bash
# Lint: a shell value reaches python3 or node as data, never as code.
#
# The shell must not paste a value into the program text it hands an
# interpreter: a quote, a backslash or a newline in the value changes the
# program. Values go on stdin (one JSON object, or NUL-separated fields),
# in argv or in the environment; the program itself is a quoted heredoc
# (<<'PY') or a single-quoted string.
#
# Reads cli/src (the parts cli/k2 is built from), finds every python3 and
# node command (with or without VAR=value in front), and fails, naming file
# and line, when:
#   1. it reads its program from an UNQUOTED heredoc (<<PY, <<EOF) whose
#      body expands a shell value ($x, ${x}, $(...) or `...`); or
#   2. its -c / -e / -p / --eval program is a double-quoted string that
#      expands a shell value. A program held whole in one variable
#      (-c "$_SOME_PY") is fine: the lint checks that variable's own text
#      where it is a quoted literal.
# Text in comments and in other commands' heredoc bodies is not read.
#
# Reviewed exceptions, as "file|variable|why", are below (EXCEPTIONS): a
# value the caller typed itself (a flag or an argument), never a daemon
# answer, a message, a file's contents or a name someone else can set. A
# listed exception that no longer matches fails too, so the list stays
# exact.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/python_code_values_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

EXCEPTIONS=(
    "heartbeat.sh|frequency|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|time|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|days|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|months|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|every|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|unit|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|start|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "heartbeat.sh|end|hb_parse_spec: the heartbeat schedule flags, typed by the caller"
    "remind.sh|name|the heartbeat name argument of k2 heartbeat pin and status, typed by the caller"
    "remind.sh|enabled|true or false, set by k2 heartbeat pin from its own flag"
    "remind.sh|from|the old name argument of k2 heartbeat rename, typed by the caller"
    "remind.sh|to|the new name argument of k2 heartbeat rename, typed by the caller"
)

lint() { # lint <dir> [file|var|why ...]: one line per problem; exit 1 when any
    python3 - "$@" <<'PYEOF'
import glob, os, re, sys

src = sys.argv[1]
allowed = {}
for row in sys.argv[2:]:
    f, var, _why = row.split("|", 2)
    allowed[(f, var)] = 0

def classify(text):
    """Per character: c code, s single-quoted, d double-quoted, h heredoc
    body, # comment; and how deep in $( ) it sits."""
    n = len(text)
    cls = ["c"] * n
    dep = [0] * n
    stack = [["code", 0]]
    pending = []
    i = 0
    while i < n:
        kind = stack[-1][0]
        d = len(stack) - 1
        c = text[i]
        dep[i] = d
        if kind in ("code", "bt"):
            if c == "\\" and i + 1 < n:
                dep[i + 1] = d; i += 2; continue
            if c == "'" or (c == "$" and text[i + 1:i + 2] == "'"):
                j = i + 1 if c == "'" else i + 2
                while j < n and text[j] != "'":
                    j += 2 if (c == "$" and text[j] == "\\") else 1
                for k in range(i, min(j + 1, n)):
                    cls[k] = "s"; dep[k] = d
                i = j + 1; continue
            if c == '"':
                cls[i] = "d"; stack.append(["dq", 0]); i += 1; continue
            if c == "`":
                if kind == "bt": stack.pop()
                else: stack.append(["bt", 0])
                i += 1; continue
            if c == "$" and text[i + 1:i + 2] == "(":
                stack.append(["code", 0]); i += 2; continue
            if c == "(":
                stack[-1][1] += 1; i += 1; continue
            if c == ")":
                if stack[-1][1] > 0: stack[-1][1] -= 1
                elif len(stack) > 1: stack.pop()
                i += 1; continue
            if c == "#" and (i == 0 or text[i - 1] in " \t\n;|&("):
                j = text.find("\n", i)
                j = n if j < 0 else j
                for k in range(i, j): cls[k] = "#"
                i = j; continue
            if text.startswith("<<", i) and not text.startswith("<<<", i):
                m = re.match(r"<<(-?)\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\2", text[i:])
                if m:
                    pending.append((m.group(3), m.group(1) == "-"))
                    i += m.end(); continue
            if c == "\n" and pending:
                i += 1
                for delim, strip in pending:
                    while i < n:
                        j = text.find("\n", i)
                        j = n if j < 0 else j
                        line = text[i:j]
                        for k in range(i, min(j + 1, n)): cls[k] = "h"
                        i = j + 1
                        if (line.lstrip("\t") if strip else line) == delim:
                            break
                pending = []
                continue
            i += 1
        else:  # inside "..."
            cls[i] = "d"
            if c == "\\":
                if i + 1 < n: cls[i + 1] = "d"; dep[i + 1] = d
                i += 2; continue
            if c == '"':
                stack.pop(); i += 1; continue
            if c == "$" and text[i + 1:i + 2] == "(":
                cls[i] = cls[i + 1] = "c"
                stack.append(["code", 0]); i += 2; continue
            if c == "`":
                stack.append(["bt", 0]); i += 1; continue
            i += 1
    return cls, dep

CMD_START = re.compile(r"(?:^|[|;&(!`{]|\$\(|\|\||&&|\bthen|\bdo|\belse|\bif|\bwhile|\buntil|\bexec|\bcommand|\btime)\s*$")
ENV_PREFIX = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=(\"(?:[^\"\\]|\\.)*\"|'[^']*'|\$\([^)]*\)|\S*)\s+")
REDIR = re.compile(r"^\d*(<<<|<<-?|<|>>|>|&>)")


INTERP = re.compile(r"\b(python3|node)\b")
WHOLE_VAR = re.compile(r'^"?\$\{?[A-Za-z_][A-Za-z0-9_]*\}?"?$')
EXPANSION = re.compile(r"(?<!\\)\$(?:\{([A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*)|(\())|(?<!\\)(`)")

def names_in(text):
    out = []
    for m in EXPANSION.finditer(text):
        out.append(m.group(1) or m.group(2) or ("$(" if m.group(3) else "`"))
    return out

def dq_expansions(word):
    """Expansions inside the double-quoted parts of one shell word."""
    found, i, n, q = [], 0, len(word), None
    seg = ""
    while i < n:
        c = word[i]
        if q is None:
            if c == "'":
                j = word.find("'", i + 1); i = n if j < 0 else j + 1; continue
            if c == '"':
                q = '"'; seg = ""; i += 1; continue
            if c == "\\":
                i += 2; continue
            if c == "$":
                found.extend(names_in(word[i:i + 64].split('"')[0].split("'")[0][:64]) or ["?"])
            i += 1; continue
        if c == "\\":
            seg += word[i:i + 2]; i += 2; continue
        if c == '"':
            found.extend(names_in(seg)); q = None; i += 1; continue
        seg += c; i += 1
    if q is not None:
        found.extend(names_in(seg))
    return found

def calls(text):
    cls, dep = classify(text)
    for m in INTERP.finditer(text):
        s = m.start()
        if cls[s] != "c":
            continue
        before = text[text.rfind("\n", 0, s) + 1:s]
        before = re.sub(r"(?:X )+$", "", ENV_PREFIX.sub("X ", before))
        if not CMD_START.search(before):
            continue
        d, i, words, cur, start = dep[s], m.end(), [], "", None
        while i < len(text):
            c = text[i]
            if dep[i] == d and cls[i] in ("h", "#"):
                break
            if dep[i] == d and cls[i] == "c":
                if c == "\\" and text[i + 1:i + 2] == "\n":
                    i += 2; continue
                if c in " \t":
                    if cur: words.append((cur, start))
                    cur = ""; i += 1; continue
                if c in "\n;&|)`":
                    break
            if not cur: start = i
            cur += c
            i += 1
        if cur: words.append((cur, start))
        yield text.count("\n", 0, s) + 1, m.group(1), words, cls

def heredoc_body(text, cls, pos):
    """The body of the heredoc whose << is at pos: the 'h' run after the line."""
    j = text.find("\n", pos)
    if j < 0: return ""
    k = j + 1
    while k < len(text) and cls[k] == "h":
        k += 1
    body = text[j + 1:k].split("\n")
    return "\n".join(body[:-2] if len(body) >= 2 else body)

problems = []
files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)
# A program held in a variable is code, not a value, when every assignment
# of that variable in the parts is a single-quoted literal (NAME='...').
ASSIGN = re.compile(r"(?:^|[\s;(])(?:local\s+|readonly\s+|export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.?)", re.M)
literal, other = set(), set()
for f in files:
    for m in ASSIGN.finditer(open(f, encoding="utf-8", errors="replace").read()):
        (literal if m.group(2) == "'" else other).add(m.group(1))
code_vars = literal - other
for f in files:
    name = os.path.basename(f)
    text = open(f, encoding="utf-8", errors="replace").read()
    for line, prog, words, cls in calls(text):
        found = []
        ws = [w for w, _ in words]
        for k, (w, pos) in enumerate(words):
            hm = re.match(r"^\d*<<(-?)(.*)$", w)
            if hm and not w.startswith("<<<") and "<<<" not in w[:4]:
                delim = hm.group(2) or (ws[k + 1] if k + 1 < len(ws) else "")
                if delim and delim[0] not in "'\"" and "\\" not in delim:
                    found.extend(("heredoc", v) for v in names_in(heredoc_body(text, cls, pos)))
            if w in ("-c", "-e", "-p", "--eval") and k + 1 < len(ws):
                code = ws[k + 1]
                if not WHOLE_VAR.match(code):
                    found.extend(("code", v) for v in dq_expansions(code))
        for how, var in found:
            if var in code_vars:
                continue
            if (name, var) in allowed:
                allowed[(name, var)] += 1
                continue
            where = "an unquoted heredoc" if how == "heredoc" else "a double-quoted program"
            problems.append("%s:%d: %s gets %s pasted into %s: pass it as data (stdin, argv or env) and quote the program" % (name, line, prog, "$" + var if var not in ("$(", "`") else "a command's output (%s)" % var, where))
for (f, var), n in sorted(allowed.items()):
    if n == 0:
        problems.append("%s: the exception for $%s matches no pasted value any more (update EXCEPTIONS)" % (f, var))
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-py-code-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good a.sh <<'FIXTURE'
# python3 <<PY with $x in a comment is not a call
f() {
    printf '%s\0' "$kind" "$raw" | python3 -c "$_SIG_PY"
    python3 - "$job" <<'PY'
import sys
print(sys.argv[1], "$x")
PY
    body=$(python3 <<"PY"
print("$y")
PY
)
    python3 -c 'import sys; print("$z", sys.argv[1])' "$name"
    node -e 'console.log(process.argv[1])' "$name"
    cat <<EOF
python3 -c "print('$w')"
EOF
    python3 <<PY
print("no shell value here")
PY
    python3 -c "$_LIB_PY"'
print(lib())'
    python3 -c "$_LIB_PY
print(lib())"
}
_LIB_PY='
def lib(): return 1
'
FIXTURE
mk heredoc a.sh <<'FIXTURE'
f() {
    body=$(python3 <<PY
import json
print(json.dumps({"kind": "$kind"}))
PY
)
}
FIXTURE
mk heredoc_dash a.sh <<'FIXTURE'
f() {
	python3 - <<-EOF
	print("${target}")
	EOF
}
FIXTURE
mk heredoc_cmd a.sh <<'FIXTURE'
f() {
    node <<JS
console.log("$(cat "$file")")
JS
}
FIXTURE
mk dq a.sh <<'FIXTURE'
f() {
    printf '%s' "$_CP_BODY" | python3 -c "
import json, sys
print('Renamed: $from')
"
}
FIXTURE
mk dq_notcode a.sh <<'FIXTURE'
_X_PY='print(1)'
f() { _X_PY="$1"; python3 -c "$_X_PY
print(2)"; }
FIXTURE
mk dq_env a.sh <<'FIXTURE'
f() { K2_X=1 python3 -c "print('$name')"; }
FIXTURE
mk dq_node a.sh <<'FIXTURE'
f() { node -e "console.log('${who}')"; }
FIXTURE
self_ok=1
if ! out="$(lint "$FIX/good")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
caught() { # caught <fixture> <text the problem must name> [exception rows]
    local fx="$1" want="$2" out; shift 2
    if out="$(lint "$FIX/$fx" "$@")"; then
        echo "SELF-TEST FAIL: $fx not caught" >&2; self_ok=0
    elif ! grep -qF -- "$want" <<<"$out"; then
        echo "SELF-TEST FAIL: $fx caught for another reason: $out" >&2; self_ok=0
    fi
}
caught heredoc 'a.sh:2: python3 gets $kind pasted into an unquoted heredoc'
caught heredoc_dash 'a.sh:2: python3 gets $target pasted into an unquoted heredoc'
caught heredoc_cmd "a.sh:2: node gets a command's output (\$() pasted into an unquoted heredoc"
caught dq 'a.sh:2: python3 gets $from pasted into a double-quoted program'
caught dq_notcode 'a.sh:2: python3 gets $_X_PY pasted into a double-quoted program'
caught dq_env 'a.sh:1: python3 gets $name pasted into a double-quoted program'
caught dq_node 'a.sh:1: node gets $who pasted into a double-quoted program'
# An exception that matches nothing: caught.
caught good 'a.sh: the exception for $nope matches no pasted value any more' "a.sh|nope|fixture"
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good one passes"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
if out="$(lint "$DIR" "${EXCEPTIONS[@]}")"; then
    echo "python_code_values_lint: no shell value is pasted into a python3 or node program in cli/src (${#EXCEPTIONS[@]} reviewed exceptions)"
else
    echo "$out" >&2
    echo "python_code_values_lint: FAILED" >&2
    exit 1
fi
