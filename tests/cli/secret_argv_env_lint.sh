#!/usr/bin/env bash
# Lint: the CLI never puts a password, or a value for _compute_json, in a
# child process's argv or environment. ps shows a command line to every user
# on this computer, and the environment to this user's processes, so neither
# is private. Secrets go on stdin (_k2_stdin_secrets, a pipe, or curl's
# config on stdin).
#
# Reads cli/src (the parts cli/k2 is built from) and fails, naming file
# and line, when:
#   1. a K2_CJ_* value rides a child's argv or environment: a K2_CJ_name=
#      word before a command (an assignment for that command), after env
#      or export, or a $K2_CJ_* expansion; or embedded code (in single
#      quotes or a heredoc) reads K2_CJ_ from its environment
#      (os.environ, getenv). Every _compute_json field reaches python3 on
#      stdin, the plain ones too.
#   2. a password-like value rides a child's argv or environment: an
#      assignment before a command (or after env or export) whose name, or
#      a variable it expands, reads as a password (password, passwd,
#      passphrase, pass, pw as a part of the name, any case); or such a
#      variable in the arguments of a program (anything that is not a
#      shell builtin or a function defined in cli/src).
#   3. a value the person types is an argument of python3: a variable
#      filled by a read of k2's own stdin (not a read from a here-string,
#      a file, a pipe or a while loop) or by $(cat), or a global another
#      function fills that way without declaring it local (as
#      _cli_read_secret_stdin fills _K2_SECRET), or a variable or array
#      built from one (the recovery words typed back are the model).
#   4. a flag whose name reads as a secret (code, ticket, token, password,
#      pass, pw, secret, key, passphrase, pin, otp, cred) takes its value
#      only as an argument: k2's own command line, which every user on this
#      computer can see while the command runs. The same function must offer a stdin
#      form sharing that word (--code with --code-stdin, --ticket with
#      --ticket-stdin, --token with --token-stdin). Flags that name where a
#      secret is (-file, -ref, -env, -name, -id, -path, ...) and
#      --idempotency-* are not secrets.
# Comments, heredoc bodies and single-quoted code are not commands and are
# not read for rule 2.
#
# Reviewed exceptions, as "file|name|why", are below (EXCEPTIONS): name is
# the assignment's name or the expanded variable. A listed exception that
# no longer matches fails too, so the list stays exact.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/secret_argv_env_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

EXCEPTIONS=(
    "app-wallet.sh|K2_SKIN_PW_SET|0 or 1: whether --password-stdin was given; the password itself is piped"
    "app-wallet.sh|temp_password|0 or 1: the --temp-password flag, no password"
    "clone.sh|keep_password|0 or 1: the --keep-password flag, no password"
    "clone.sh|fixed_password|0, 1 or empty: the --fixed-password flags, no password"
    "mail.sh|M_PASS_REF|where the password is kept (env:<VAR> or a file path), not the password; the password rides stdin"
    "permission.sh|K2_PW|the --why text or a workspace name, not a password"
    "vault.sh|login|the owner login's username typed at the prompt, not a secret; the password typed with it rides stdin"
    "agent-hire.sh|--key|an idempotency key or an API key's id, not a secret"
    "app-auth.sh|--apns-key|the path of the APNs key file (.p8), not the key"
    "compute-provenance.sh|--owner-key|the owner's public key fingerprint"
    "connect.sh|--pin-days|a number of days"
    "garden-data.sh|--key|a store key's name"
    "sim.sh|--secret|the vault secret's name; k2 sim types the value itself"
    "chat.sh|--key|an idempotency key for the send, not a secret"
    "ci.sh|--statuses-token|the NAME of the vault secret that holds the token"
    "compute-run.sh|--secret|the NAME of a vault secret the job gets"
    "migrate.sh|--secret|the NAME of a vault secret"
    "msg-whoami-done.sh|--secret|the NAME of a vault secret"
    "sources.sh|--secret|the NAME of a vault secret"
    "postal.sh|--code|the 6-digit comparison code both sides show, not a secret"
    "doc.sh|--token|FOLLOW-UP for K2: the remote server's token rides k2's own argv (k2 doc --host --token); needs --token-stdin"
    "clone.sh|--token|FOLLOW-UP for K2: the other server's token rides k2's own argv (k2 clone --host --token); needs --token-stdin"
    "connect-account.sh|--token|FOLLOW-UP for K2: the account token rides k2's own argv (k2 connect login --token); needs --token-stdin"
)

lint() { # lint <dir> [file|name|why ...]: one line per problem; exit 1 when any
    python3 - "$@" <<'PYEOF'
import glob, os, re, sys

src = sys.argv[1]
allowed = {}
for row in sys.argv[2:]:
    f, name, _why = row.split("|", 2)
    allowed[(f, name)] = 0

PW = re.compile(r"[Pp]ass(word|wd|phrase)|PASS(WORD|WD|PHRASE)|(^|_)([Pp]ass|PASS|[Pp]w|PW)(_|$)|^([Pp]ass|[Pp]w)[A-Z0-9]|[a-z0-9](Pass|Pw)([A-Z0-9_]|$)")
REF = re.compile(r"\$\{?[#!]?([A-Za-z_][A-Za-z0-9_]*)")
ASSIGN = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)(\[[^\]]*\])?\+?=(.*)$", re.S)
BUILTINS = set("""
. : [ [[ alias bg bind break builtin caller case cd command compgen complete continue declare dirs
disown echo enable eval exit export false fc fg for function getopts hash help history if jobs kill
let local logout mapfile popd printf pushd pwd read readarray readonly return select set shift shopt
source suspend test then time times trap true type typeset ulimit umask unalias unset until wait
while do done esac fi else elif in { } ! ]] ;;
""".split())
SECRET_FLAG = re.compile(r"(^|-)(code|ticket|token|password|passwd|pass|pw|secret|key|passphrase|pin|otp|cred|credential|credentials)(-|$)")
NOT_SECRET_FLAG = re.compile(r"-(stdin|file|ref|env|name|names|id|ids|path|dir|url|label|kind|type|scope|header|prefix|format|encoding|template)$|^--idempotency-")
PREFIX_WORDS = {"!", "{", "if", "then", "do", "else", "elif", "while", "until", "time", "command", "builtin", "exec", "nohup", "sudo", "timeout"}

def classify(text):
    """Per character: c code, s single-quoted, d double-quoted, h heredoc
    body, # comment; and how deep in $( ) it sits. (The same reading as
    python_argv_answer_lint.sh.)"""
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

def commands(text, cls, dep):
    """Every simple command as (line, [words]). A word keeps the text of a
    $( ) inside it; that inner text is read as commands of its own too."""
    n = len(text)
    open_cmds = {}  # depth -> [start line, words, current word]
    out = []
    def finish(d):
        st = open_cmds.pop(d, None)
        if st is None:
            return
        if st[2]:
            st[1].append(st[2])
        if st[1]:
            out.append((st[0], st[1]))
    def add(d, c, line):
        st = open_cmds.get(d)
        if st is None:
            st = open_cmds[d] = [line, [], ""]
        st[2] += c
        for dd, other in open_cmds.items():
            if dd < d:
                other[2] += c
    i = 0
    line = 1
    while i < n:
        c, d, k = text[i], dep[i], cls[i]
        if k in ("c", "d") and c == "$" and text[i + 1:i + 2] == "(":
            add(d, "$(", line)
            i += 2; continue
        for dd in [x for x in open_cmds if x > d]:
            finish(dd)
        if k == "h":
            finish(d)
            if c == "\n": line += 1
            i += 1; continue
        if k == "#":
            i += 1; continue
        if k == "c":
            if c == "\\" and text[i + 1:i + 2] == "\n":
                line += 1; i += 2; continue
            if c in " \t":
                st = open_cmds.get(d)
                if st and st[2]:
                    st[1].append(st[2]); st[2] = ""
                i += 1; continue
            if c in "\n;&|()`":
                finish(d)
                if c == "\n": line += 1
                i += 1; continue
        if c == "\n": line += 1
        add(d, c, line)
        i += 1
    for d in sorted(open_cmds, reverse=True):
        finish(d)
    return out

def split_words(words):
    """(assignments, command word, arguments) of a simple command."""
    assigns = []
    w = list(words)
    while w:
        if ASSIGN.match(w[0]):
            assigns.append(w.pop(0)); continue
        if w[0] in PREFIX_WORDS:
            w.pop(0); continue
        break
    if not w:
        return assigns, None, []
    return assigns, w[0], w[1:]

def strip_quotes(word):
    if len(word) >= 2 and word[0] == word[-1] and word[0] in "\"'":
        return word[1:-1]
    return word

def refs(word):
    return [m.group(1) for m in REF.finditer(word)]


def typed_values(text, cmds, lines, seed=()):
    """Variables a person types: filled by a read of k2's own stdin (no
    redirection on that line, not a while loop's read, not after a pipe),
    or by $(cat), or a global another function fills that way (seed); then
    any variable or array built from one."""
    typed = set(seed)
    for line, words in cmds:
        assigns, cmd, args = split_words(words)
        src = lines[line - 1] if line - 1 < len(lines) else ""
        if cmd == "read" and not re.search(r"<|\bwhile\b|\|\s*(\{\s*)?(IFS=\S*\s+)?read\b", src):
            i = 0
            while i < len(args):
                a = args[i]
                if a.startswith("-") and len(a) > 1:
                    if a[-1] == "a" and i + 1 < len(args):
                        typed.add(args[i + 1])
                    i += 2 if a[-1] in "pdtnNua" else 1
                    continue
                if re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", a):
                    typed.add(a)
                i += 1
        for a in assigns + (args if cmd in ("local", "declare", "typeset", "readonly") else []):
            m = ASSIGN.match(a)
            if m and re.match(r'^"?\$\(\s*cat\s*\)?"?$', m.group(3)):
                typed.add(m.group(1))
    grow = True
    while grow:
        grow = False
        for line, words in cmds:
            assigns, cmd, args = split_words(words)
            for a in assigns + (args if cmd in ("local", "declare", "typeset", "readonly") else []):
                m = ASSIGN.match(a)
                if m and m.group(1) not in typed and "$(" not in m.group(3) and any(r in typed for r in refs(m.group(3))):
                    typed.add(m.group(1)); grow = True
        # Arrays: NAME=( ... ) and NAME+=( ... ) holding a typed value.
        for m in re.finditer(r"\b([A-Za-z_][A-Za-z0-9_]*)\+?=\(([^)]*)\)", text):
            if m.group(1) not in typed and any(r in typed for r in refs(m.group(2))):
                typed.add(m.group(1)); grow = True
    return typed

problems = []
def flag(f, line, key, msg):
    if (f, key) in allowed:
        allowed[(f, key)] += 1
        return
    problems.append("%s:%d: %s" % (f, line, msg))

files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)
FUNCS = set()
parts = []
for path in files:
    text = open(path, encoding="utf-8", errors="replace").read()
    FUNCS.update(re.findall(r"(?m)^[ \t]*(?:function[ \t]+)?([A-Za-z_][A-Za-z0-9_]*)[ \t]*\(\)", text))
    parts.append((os.path.basename(path), text))
# A function named after a program (prelude.sh's python3 shim on Windows)
# still runs that program with the same argv.
FUNCS -= {"python3", "python", "curl", "env", "sudo"}

def functions(text, cmds, lines):
    """(first line, last line, its commands, its text) per function, and
    the top level before the first one."""
    starts = [text.count("\n", 0, m.start()) + 1 for m in re.finditer(r"(?m)^[A-Za-z_][A-Za-z0-9_]*\(\)\s*\{", text)]
    for a, b in zip([1] + starts, starts + [10 ** 9]):
        yield a, b, [(l, w) for l, w in cmds if a <= l < b], "\n".join(lines[a - 1:min(b - 1, len(lines))])

def declared(fc):
    """The names a function declares local."""
    out = set()
    for line, words in fc:
        assigns, cmd, args = split_words(words)
        if cmd in ("local", "declare", "typeset", "readonly"):
            out.update(re.split(r"[=+\[]", x, maxsplit=1)[0] for x in args if not x.startswith("-"))
    return out

# Globals a person types: a function reads them from stdin without
# declaring them local (_cli_read_secret_stdin fills _K2_SECRET).
GLOBAL_TYPED = set()
for f, text in parts:
    cls, dep = classify(text)
    lines = text.split("\n")
    for a, b, fc, body in functions(text, commands(text, cls, dep), lines):
        GLOBAL_TYPED |= typed_values(body, fc, lines) - declared(fc)

for f, text in parts:
    cls, dep = classify(text)
    # Rule 1: embedded code (a run of single-quoted or heredoc text) that
    # names K2_CJ_ and reads its environment.
    i, n = 0, len(text)
    while i < n:
        if cls[i] not in ("s", "h"):
            i += 1; continue
        j = i
        while j < n and cls[j] == cls[i]:
            j += 1
        run = text[i:j]
        if "K2_CJ_" in run and re.search(r"\benviron\b|\bgetenv\b|\$ENV\b|process\.env", run):
            flag(f, text.count("\n", 0, i + run.index("K2_CJ_")) + 1, "K2_CJ_",
                 "embedded code reads K2_CJ_ values from its environment (send them on stdin)")
        i = j
    # Rule 1: a $K2_CJ_ expansion.
    for m in re.finditer(r"\$\{?K2_CJ_", text):
        if cls[m.start()] in ("c", "d"):
            flag(f, text.count("\n", 0, m.start()) + 1, "K2_CJ_",
                 "a K2_CJ_ value is read from the environment ($K2_CJ_...)")
    for line, words in commands(text, cls, dep):
        assigns, cmd, args = split_words(words)
        if cmd is None:
            continue  # plain assignments: shell variables, no child sees them
        bare = strip_quotes(cmd)
        is_env = bare == "env"
        is_export = bare == "export" or (bare in ("declare", "typeset", "local", "readonly")
                                          and any(a.startswith("-") and "x" in a for a in args))
        env_words = [(a, "a child's environment") for a in assigns]
        if is_env:
            env_words += [(a, "env's argv") for a in args if ASSIGN.match(a)]
        if is_export:
            env_words += [(a if "=" in a else a + "=", "the environment (export)") for a in args if not a.startswith("-")]
        for a, where in env_words:
            m = ASSIGN.match(a)
            if not m:
                continue
            name, value = m.group(1), m.group(3)
            if name.startswith("K2_CJ_"):
                flag(f, line, name, "%s rides %s (send it on stdin)" % (name, where))
            elif PW.search(name):
                flag(f, line, name, "%s, a password-like name, rides %s (send it on stdin)" % (name, where))
            else:
                for r in refs(value):
                    if PW.search(r):
                        flag(f, line, r, "$%s, a password-like value, rides %s as %s (send it on stdin)" % (r, where, name))
        if is_export or bare in BUILTINS or bare in FUNCS:
            continue
        for a in args:
            if is_env and ASSIGN.match(a):
                continue
            for r in refs(a):
                if PW.search(r):
                    flag(f, line, r, "$%s, a password-like value, is in the argv of %s (send it on stdin)" % (r, bare))
    # Rule 3: a value the person typed, as an argument of python3.
    lines = text.split("\n")
    cmds = commands(text, cls, dep)
    for a, b, fc, body in functions(text, cmds, lines):
        typed = typed_values(body, fc, lines, GLOBAL_TYPED - declared(fc))
        for line, words in fc:
            assigns, cmd, args = split_words(words)
            if cmd is None or strip_quotes(cmd) != "python3":
                continue
            for arg in args:
                for r in refs(arg):
                    if r in typed:
                        flag(f, line, r, "$%s, a value typed on stdin, is in the argv of python3 (send it on stdin)" % r)
    # Rule 4: a secret-named flag taken only as an argument.
    funcs4 = [m.start() for m in re.finditer(r"(?m)^[A-Za-z_][A-Za-z0-9_]*\(\)\s*\{", text)]
    for a, b in zip([0] + funcs4, funcs4 + [len(text)]):
        body = text[a:b]
        arms = [m for m in re.finditer(r"(?m)^[ \t]*((?:--[a-z0-9-]+(?:=\*)?)(?:\|--[a-z0-9-]+(?:=\*)?)*)\)", body)
                if cls[a + m.start(1)] == "c"]
        # The stdin forms this function really takes: case labels, not text.
        stdin_flags = set(fl for m in arms for fl in m.group(1).split("|") if fl.endswith("-stdin"))
        for m in arms:
            if cls[a + m.start(1)] != "c":
                continue
            arm = body[m.end():body.find(";;", m.end()) if ";;" in body[m.end():] else len(body)]
            if not re.search(r"\$\{?2\b|\$\{?2[:}-]|\$\{1#", arm):
                continue
            for fl in m.group(1).split("|"):
                fl = fl[:-2] if fl.endswith("=*") else fl
                name = fl[2:]
                if not SECRET_FLAG.search(name) or NOT_SECRET_FLAG.search(fl):
                    continue
                words = [w for w in name.split("-") if SECRET_FLAG.search(w)]
                if any(any(w in sf for w in words) for sf in stdin_flags):
                    continue
                line = text.count("\n", 0, a + m.start(1)) + 1
                flag(f, line, fl, "%s reads as a secret and k2 takes it only as an argument, which every user on this computer can see while the command runs (add a %s-stdin form)" % (fl, fl))
for (f, name), n in sorted(allowed.items()):
    if n == 0:
        problems.append("%s: the exception for %s matches nothing any more (update EXCEPTIONS)" % (f, name))
for p in sorted(set(problems), key=problems.index):
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-secret-argv-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good a.sh <<'EOF'
# curl -u "admin:$password" https://example.test in a comment is not a call
_send_pw() { printf '%s' "$1"; }
_compute_json() { :; }
f() {
    local M_PASS="$pass" pw_stdin=0
    [ -n "$pw" ] || return 2
    body="$(printf '%s' "$pw" | K2_X="$name" python3 -c 'import os, sys; print(sys.stdin.read(), os.environ["K2_X"])')"
    _compute_json K2_CJ_app="$app" K2_CJ_password_s="$pw"
    _send_pw "$password"
    python3 - "$name" <<'PY'
import os
print("K2_CJ_x", "$password")
PY
    cat <<'TXT'
  env K2_CJ_a=1 python3 -c 'x' "$pw"
TXT
}
EOF
mk good b.sh <<'EOF'
g() { K2_PW="$why" python3 -c 'import os; print(os.environ["K2_PW"])'; }
EOF
mk cj_env a.sh <<'EOF'
f() { env K2_CJ_a="$x" python3 -c 'print(1)'; }
EOF
mk cj_prefix a.sh <<'EOF'
f() {
    K2_CJ_seen="$x" python3 -c 'print(1)'
}
EOF
mk cj_environ a.sh <<'EOF'
_compute_json() {
    env "$@" python3 -c '
import json, os
print(json.dumps({k[6:]: v for k, v in os.environ.items() if k.startswith("K2_CJ_")}))
'
}
EOF
mk pw_prefix a.sh <<'EOF'
f() { body="$(K2_SKIN_PW="$secret" python3 -c 'print(1)')"; }
EOF
mk pw_value a.sh <<'EOF'
f() { BODY="$password" curl -s https://example.test; }
EOF
mk pw_argv a.sh <<'EOF'
f() {
    python3 -c 'import sys; print(sys.argv)' \
        "$name" \
        "$new_password"
}
EOF
mk pw_env_argv a.sh <<'EOF'
f() { env SECRET="$pw" python3 -c 'print(1)'; }
EOF
mk pw_export a.sh <<'EOF'
f() { export DB_PASSWORD; }
EOF
mk typed_words a.sh <<'EOF'
f() {
    local words=() wd n
    for n in 3 11 20; do
        IFS= read -r wd || wd=""
        words+=("$wd")
    done
    body="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[2:5]))' "$id" "${words[@]}")"
}
EOF
mk typed_cat a.sh <<'EOF'
f() {
    local code
    code="$(cat)"
    python3 -c 'import sys; print(sys.argv[1])' "$code"
}
EOF
mk good c.sh <<'EOF'
h() {
    local kind field1 line
    { IFS= read -r kind; IFS= read -r field1; } <<<"$mapped"
    python3 -c 'import sys; print(sys.argv[1])' "$field1"
    while IFS= read -r line; do python3 -c 'import sys' "$line"; done <"$file"
    IFS= read -r typed
    printf '%s' "$typed" | python3 -c 'import sys; print(sys.stdin.read())'
}
EOF
mk typed_global a.sh <<'EOF'
_rd_secret() { IFS= read -r _MY_SECRET || true; }
f() {
    local code
    _rd_secret
    code="${_MY_SECRET%$'\r'}"; _MY_SECRET=""
    python3 -c 'import sys; print(sys.argv[1])' "$code"
}
g() {
    local _MY_SECRET="from an answer"
    python3 -c 'import sys; print(sys.argv[1])' "$_MY_SECRET"
}
EOF
mk flag_argonly a.sh <<'EOF'
f() {
    echo "pipe it with --token-stdin" >&2
    while [ $# -gt 0 ]; do
        case "$1" in
            --host) host="${2:-}"; shift 2 ;;
            --api-token|--token) token="${2:-}"; shift 2 ;;
            --token-file) tfile="${2:-}"; shift 2 ;;
            *) shift ;;
        esac
    done
}
g() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --ticket) ticket="${2:-}"; shift 2 ;;
            --ticket-stdin) ticket_stdin=1; shift ;;
            --pin=*) pin="${1#--pin=}"; shift ;;
        esac
    done
}
EOF
mk pw_nested a.sh <<'EOF'
f() { out="$(curl -s -u "admin:${userPass}" https://example.test)"; }
EOF
self_ok=1
if ! out="$(lint "$FIX/good" "b.sh|K2_PW|fixture")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
caught() { # caught <fixture> <text the problem must name> [exception rows]
    local fx="$1" want="$2" out; shift 2
    if out="$(lint "$FIX/$fx" "$@")"; then
        echo "SELF-TEST FAIL: $fx not caught" >&2; self_ok=0
    elif ! grep -qF -- "$want" <<<"$out"; then
        echo "SELF-TEST FAIL: $fx caught for another reason: $out" >&2; self_ok=0
    fi
}
caught cj_env "a.sh:1: K2_CJ_a rides env's argv"
caught cj_prefix "a.sh:2: K2_CJ_seen rides a child's environment"
caught cj_environ "a.sh:4: embedded code reads K2_CJ_ values from its environment"
caught pw_prefix "a.sh:1: K2_SKIN_PW, a password-like name, rides a child's environment"
caught pw_value "a.sh:1: \$password, a password-like value, rides a child's environment as BODY"
caught pw_argv "a.sh:2: \$new_password, a password-like value, is in the argv of python3"
caught pw_env_argv "a.sh:1: \$pw, a password-like value, rides env's argv as SECRET"
caught pw_export "a.sh:1: DB_PASSWORD, a password-like name, rides the environment (export)"
caught typed_words "a.sh:7: \$words, a value typed on stdin, is in the argv of python3"
caught typed_cat "a.sh:4: \$code, a value typed on stdin, is in the argv of python3"
caught typed_global "a.sh:6: \$code, a value typed on stdin, is in the argv of python3"
caught flag_argonly "a.sh:6: --api-token reads as a secret and k2 takes it only as an argument"
caught flag_argonly "a.sh:17: --pin reads as a secret and k2 takes it only as an argument"
caught pw_nested "a.sh:1: \$userPass, a password-like value, is in the argv of curl"
# The good parts with no exception for b.sh's K2_PW: caught. An exception
# that matches nothing: caught.
caught good "b.sh:1: K2_PW, a password-like name, rides a child's environment"
caught good "a.sh: the exception for K2_PW matches nothing any more" "b.sh|K2_PW|fixture" "a.sh|K2_PW|fixture"
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good one passes"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
if out="$(lint "$DIR" "${EXCEPTIONS[@]}")"; then
    echo "secret_argv_env_lint: no K2_CJ_ field, password or typed value in a child's argv or environment in cli/src (${#EXCEPTIONS[@]} reviewed exceptions)"
else
    echo "$out" >&2
    echo "secret_argv_env_lint: FAILED" >&2
    exit 1
fi
