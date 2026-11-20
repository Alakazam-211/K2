#!/usr/bin/env bash
# Lint: the CLI never hands a daemon answer to python3 in argv.
#
# One argument over 128 KiB fails on Linux with "Argument list too long"
# (and all of argv over 1 MiB on macOS). A daemon answer, or a list that
# grows with one (k2 compute fetch kept its fetched-file list as a JSON
# string and passed it in argv: a job with 464 files died after about
# 260), has no such bound. It goes to Python on stdin, or in a temp file.
#
# Reads cli/src (the parts cli/k2 is built from), finds every python3
# command (python3 -c CODE ..., python3 - ... <<HEREDOC, python3 FILE ...,
# with or without VAR=value in front), and fails, naming file and line,
# when an argument after the code:
#   1. names a variable that holds an answer: the transports' last body
#      (_CP_BODY, _SC_BODY, _CI_BODY and every other _NAME_BODY), resp,
#      response, *_resp, *_response, reply, raw, rres, answer, results,
#      acc, list; or
#   2. runs a request in place: "$(cli_request ...)", "$(cli_send ...)",
#      "$(cli_post_json ...)", "$(_compute_call ...)", "$(_k2_idem_send ...)",
#      "$(curl ...)" and the other senders named in SENDERS below.
# Text inside the Python code, comments and heredoc bodies is not an
# argument and is not read.
#
# Reviewed exceptions, as "file|variable|why", are below (EXCEPTIONS).
# A listed exception that no longer matches fails too, so the list stays
# exact.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/python_argv_answer_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

EXCEPTIONS=(
    "msg-inbox-staging.sh|response|the inbox compose answer: one package's ids and title, a few hundred bytes"
    "msg-inbox-staging.sh|wake_resp|the wake answer for that package: a status, a reason and a session id"
    "vault.sh|reply|the value of --reply, typed by the person, not an answer"
)

lint() { # lint <dir> [file|var|why ...]: one line per problem; exit 1 when any
    python3 - "$@" <<'PYEOF'
import glob, os, re, sys

src = sys.argv[1]
allowed = {}
for row in sys.argv[2:]:
    f, var, _why = row.split("|", 2)
    allowed[(f, var)] = 0

ANSWER = re.compile(r"^(_[A-Z0-9]+_BODY|resp|response|[A-Za-z0-9_]+_resp|[A-Za-z0-9_]+_response|reply|raw|rres|answer|results|acc|list)$")
SENDERS = ("cli_request", "cli_send", "cli_send_json", "cli_post_json", "cli_post_form", "cli_print_or_fail",
           "_compute_call", "_compute_send", "_k2_idem_send", "_sidecar_send", "_vault_sock_call", "_vault_call",
           "_context_http", "_mw_post", "_zen_session_get", "_zen_session_post", "curl", "_k2_curl")
SENDER_RE = re.compile(r"\$\(\s*(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*(%s)\b" % "|".join(re.escape(s) for s in SENDERS))
REF = re.compile(r"\$\{?#?!?([A-Za-z_][A-Za-z0-9_]*)")

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

def python_calls(text):
    cls, dep = classify(text)
    for m in re.finditer(r"\bpython3\b", text):
        s = m.start()
        if cls[s] != "c":
            continue
        before = text[text.rfind("\n", 0, s) + 1:s]
        before = re.sub(r"(?:X )+$", "", ENV_PREFIX.sub("X ", before))
        if not CMD_START.search(before):
            continue
        d, i, words, cur = dep[s], m.end(), [], ""
        while i < len(text):
            c = text[i]
            if dep[i] == d and cls[i] in ("h", "#"):
                break
            if dep[i] == d and cls[i] == "c":
                if c == "\\" and text[i + 1:i + 2] == "\n":
                    i += 2; continue
                if c in " \t":
                    if cur: words.append(cur)
                    cur = ""; i += 1; continue
                if c in "\n;&|)`":
                    break
            cur += c
            i += 1
        if cur: words.append(cur)
        # Drop redirections (and the word after a bare operator).
        kept, skip = [], False
        for w in words:
            if skip:
                skip = False; continue
            r = REDIR.match(w)
            if r:
                skip = r.end() == len(w)
                continue
            kept.append(w)
        # Interpreter options, then the code (-c CODE, -, or a file); the rest is argv.
        k = 0
        while k < len(kept):
            w = kept[k]
            if w == "-c":
                k += 2; break
            if w == "-" or not w.startswith("-"):
                k += 1; break
            k += 1
        yield text.count("\n", 0, s) + 1, kept[k:]

problems = []
files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)
for f in files:
    name = os.path.basename(f)
    text = open(f, encoding="utf-8", errors="replace").read()
    for line, argv in python_calls(text):
        for a in argv:
            if a.startswith("'") and a.endswith("'") and a.count("'") == 2:
                continue  # a literal
            for r in REF.finditer(a):
                var = r.group(1)
                if not ANSWER.match(var):
                    continue
                if (name, var) in allowed:
                    allowed[(name, var)] += 1
                    continue
                problems.append("%s:%d: python3 gets $%s in argv (an answer has no size bound: send it on stdin)" % (name, line, var))
            sm = SENDER_RE.search(a)
            if sm:
                problems.append("%s:%d: python3 gets a request's answer in argv ($(%s ...)): send it on stdin" % (name, line, sm.group(1)))
for (f, var), n in sorted(allowed.items()):
    if n == 0:
        problems.append("%s: the exception for $%s matches no python3 argv any more (update EXCEPTIONS)" % (f, var))
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-py-argv-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good a.sh <<'EOF'
# python3 -c 'x' "$resp" in a comment is not a call
f() {
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(sys.argv[1], "resp", "$resp")' "$name"
    results+="$entry"$'\n'
    printf '%s' "$results" | K2_X="$out" python3 -c 'import sys; print(sys.stdin.read())'
    out="$(python3 -c 'import sys; print(sys.argv[1])' "$(urlencode "$name")" "${names[@]+"${names[@]}"}")"
    python3 - "$job" <<'PY'
import sys
print(sys.argv[1], "$_CP_BODY")
PY
    cat <<'TXT'
  python3 -c 'print(1)' "$resp"
TXT
    python3 -I "$script" "$file" 2>/dev/null
}
EOF
mk good b.sh <<'EOF'
g() { python3 -c 'import sys; print(sys.argv[1])' "$reply"; }
EOF
mk argv_results a.sh <<'EOF'
f() {
    results="$(K2_CP_PATH="$path" python3 -c '
import json, sys
acc = json.loads(sys.argv[1])
print(json.dumps(acc))' "$results")" || exit 3
}
EOF
mk argv_body a.sh <<'EOF'
f() { python3 -c 'import json,sys; print(json.loads(sys.argv[1]))' "$_CP_BODY"; }
EOF
mk argv_heredoc a.sh <<'EOF'
f() {
    python3 - "$job" "$resp" <<'PY'
import sys
PY
}
EOF
mk argv_sender a.sh <<'EOF'
f() { x="$(python3 -c 'import sys; print(sys.argv[1])' "$(cli_request "/cli/compute/jobs/list")")"; }
EOF
mk argv_continued a.sh <<'EOF'
f() {
    python3 -c 'import sys; print(sys.argv)' \
        "$name" \
        "$wake_resp"
}
EOF
self_ok=1
if ! out="$(lint "$FIX/good" "b.sh|reply|fixture")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
caught() { # caught <fixture> <text the problem must name> [exception rows]
    local fx="$1" want="$2" out; shift 2
    if out="$(lint "$FIX/$fx" "$@")"; then
        echo "SELF-TEST FAIL: $fx not caught" >&2; self_ok=0
    elif ! grep -qF -- "$want" <<<"$out"; then
        echo "SELF-TEST FAIL: $fx caught for another reason: $out" >&2; self_ok=0
    fi
}
caught argv_results 'a.sh:2: python3 gets $results in argv'
caught argv_body 'a.sh:1: python3 gets $_CP_BODY in argv'
caught argv_heredoc 'a.sh:2: python3 gets $resp in argv'
caught argv_sender "a.sh:1: python3 gets a request's answer in argv (\$(cli_request"
caught argv_continued 'a.sh:2: python3 gets $wake_resp in argv'
# The good parts with no exception for b.sh's $reply: caught. An exception
# that matches nothing: caught.
caught good 'b.sh:1: python3 gets $reply in argv'
caught good 'a.sh: the exception for $raw matches no python3 argv any more' "b.sh|reply|fixture" "a.sh|raw|fixture"
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good one passes"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
if out="$(lint "$DIR" "${EXCEPTIONS[@]}")"; then
    echo "python_argv_answer_lint: no python3 in cli/src gets a daemon answer in argv (${#EXCEPTIONS[@]} reviewed exceptions)"
else
    echo "$out" >&2
    echo "python_argv_answer_lint: FAILED" >&2
    exit 1
fi
