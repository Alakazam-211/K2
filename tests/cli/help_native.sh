#!/usr/bin/env bash
# bash-only: it traces the bash CLI itself (bash -x) to prove which help forms are a plain print.
#
# Which help forms the k5 binary may answer itself: the ones where the bash
# CLI does nothing but print one cli/text/help file. The list is
# crates/k5-cli/src/help_native.txt; the binary embeds it (build.rs).
#
#   bash tests/cli/help_native.sh           derive the list from bash, and fail
#                                           unless it is the committed one
#   bash tests/cli/help_native.sh --write   derive it and write it
# HELP_NATIVE_WHY=1 also prints each form bash keeps, with the reason.
#
# The forms tried:
#   <words> --help and <words> -h   for each help/<words>.txt that is not in
#                                   help_text.sh's ENTRY table (those 19
#                                   pages print from other commands)
#   help <w>, --help <w>, -h <w>    for each top level page and tier word
#   help-advanced, help-internal
# A form is on the list only when, run as bash under each of the
# environments below, all of this holds:
#   1. stdout and stderr and the exit code are the same in all, and one
#      stream is exactly one cli/text/help file while the other is empty;
#   2. the trace (bash -x) from the router's case to the EXIT trap is the
#      same in all, and each step in it is one of: case, shift, local,
#      for, return, exit, true, :, break, continue, a variable assignment,
#      a call of a function the script defines, `[` or `[[` comparing two
#      words, and exactly one `cat` with no arguments (the quoted heredoc);
#      never a subshell or command substitution, an echo or printf, a file
#      or string test (-f, -n, ...), another program, or a step that spans
#      lines.
# The k2 help tiers made of pieces (`k2 --help` is _daily plus _daily-end,
# with a line between them when permission cards are on) and every page
# piped through the card filter fail rule 2, so bash keeps them.
#
# The environments, each env -i with a scratch HOME, stubs for curl and
# the system tools (a call fails the form), stdin from /dev/null:
#   A    the owner's terminal: a port and an owner token in HOME/.k2.
#   B    an agent's settings the front lets through: a passport
#        (K2_HOOK_TOKEN) and K5_PORT, no token files, the virtual desktop's
#        switch, air-gap on, a project path, another TMPDIR and LANG, and
#        session names. A family's --help only (help/--help/-h forms run
#        in C instead).
#   C    an agent in a K2 terminal: B and K2_HOOK_SOCK, a live socket. A
#        stub daemon answers there, and only there: the permission cards
#        probe (GET /cli/permission/enabled) says cards are off. curl runs
#        for real only for a request to that socket, python3 for real
#        always, so a form that asks reads the answer as it would.
#   Con  C with cards on.
# Every form runs in A, C and Con, and a family's --help in B too. The
# session checks pass in all of them (a passport, a live socket, no
# CODEX_ setting): the binary runs the same checks for a family's help
# (crates/k5-cli/src/pages.rs, session_checks). Before the forms, a check
# that the stub answers: `k2 study list` in Con shows the permissions
# line, in C it doesn't, and each asks once.
#
# Never touches ~/.k2. The stub daemon is this script's own, on two Unix
# sockets in the scratch folder.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
LIST="$ROOT/crates/k5-cli/src/help_native.txt"

WRITE=0
case "${1:-}" in
    "") ;;
    --write) WRITE=1 ;;
    *) echo "help_native: unknown argument: $1" >&2; exit 2 ;;
esac

BASH_BIN=/bin/bash
[ -x "$BASH_BIN" ] || BASH_BIN=/usr/bin/bash
[ -x "$BASH_BIN" ] || { echo "FAIL: no /bin/bash or /usr/bin/bash" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is needed" >&2; exit 1; }

SANDBOX="$(mktemp -d -t k2-help-native-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT

python3 -I - "$ROOT" "$CLI_BASH_SOURCE" "$BASH_BIN" "$SANDBOX" "$LIST" "$WRITE" <<'PY'
import concurrent.futures, http.server, json, os, re, shutil, socketserver, subprocess, sys, threading

root, script, bash_bin, sandbox, list_path, write = sys.argv[1:7]
text_dir = os.path.join(root, "cli/text/help")

# The pages and their bytes.
files = {}
for d, _, names in os.walk(text_dir):
    for n in names:
        if n.endswith(".txt"):
            p = os.path.join(d, n)
            files[os.path.relpath(p, text_dir)[:-4]] = open(p, "rb").read()
if len(files) < 200:
    sys.exit("FAIL: only %d help files under %s" % (len(files), text_dir))
by_bytes = {}
for rel in sorted(files):
    by_bytes.setdefault(files[rel], []).append(rel)

# help_text.sh's ENTRY table: pages another command prints. They stay bash.
ht = open(os.path.join(root, "tests/cli/help_text.sh")).read()
m = re.search(r"^ENTRY='\n(.*?)^'", ht, re.S | re.M)
if not m:
    sys.exit("FAIL: no ENTRY table in tests/cli/help_text.sh")
entry = {l.split("|", 1)[0][:-4] for l in m.group(1).splitlines() if l.strip()}
if len(entry) != 19:
    sys.exit("FAIL: help_text.sh's ENTRY table has %d rows, this test knows 19" % len(entry))
for rel in entry:
    if rel not in files:
        sys.exit("FAIL: ENTRY names %s, which is not under cli/text/help" % rel)

# The functions the script defines: a call of one is a step of its own.
functions = set(re.findall(r"^([A-Za-z_][A-Za-z0-9_]*)\(\) *\{", open(script).read(), re.M))

HELP_FIRST = ("help", "--help", "-h")
cands = []
for rel in sorted(files):
    if "/_" in "/" + rel or rel in entry:
        continue
    words = rel.split("/")
    cands.append(words + ["--help"])
    cands.append(words + ["-h"])
tier_words = ["--advanced", "advanced", "-a", "--internal", "internal", "deprecated", "--deprecated",
              "source", "jobs", "tickets", "cal"]
for w in sorted({r for r in files if "/" not in r and not r.startswith("_")} | set(tier_words)):
    for first in HELP_FIRST:
        cands.append([first, w])
cands.append(["help-advanced"])
cands.append(["help-internal"])

def make_env(kind, base):
    home = os.path.join(base, "home")
    os.makedirs(os.path.join(home, ".k2"), exist_ok=True)
    os.makedirs(os.path.join(base, "cwd"), exist_ok=True)
    stub = os.path.join(sandbox, "stub")
    path = stub + ":" + os.environ.get("PATH", "/usr/bin:/bin")
    env = {"HOME": home, "PATH": path, "LC_ALL": "C"}
    cwd = os.path.join(base, "cwd")
    if kind == "A":
        with open(os.path.join(home, ".k2/heartbeat.port"), "w") as f:
            f.write("1\n")
        with open(os.path.join(home, ".k2/heartbeat.token"), "w") as f:
            f.write("test-token\n")
    else:
        ws = os.path.join(base, "ws")
        for d in ("ws/.k2", "ws/sub", "tmp", "sec"):
            os.makedirs(os.path.join(base, d), exist_ok=True)
        cwd = os.path.join(ws, "sub")
        env.update({
            "LANG": "en_US.UTF-8", "TMPDIR": os.path.join(base, "tmp"),
            "K5_PORT": "4100", "K2_HOST": "127.0.0.9", "K2_HOOK_TOKEN": "sessQ.notreal1",
            "K2_PROJECT_PATH": ws, "K2_AIRGAP": "1", "K2_NO_OWNER_FALLBACK": "1",
            "K2_SECRETS_DIR": os.path.join(base, "sec"), "K2_FORCE_TCP": "1", "K2_CLI_SKIP_UDS": "1",
            "K2_WORKSPACE": "globex", "K2_AGENT_NAME": "globex", "K2_SESSION_ID": "s9",
            "K2_CELL": "canonical", "K2_PANE_ID": "p9", "CLAUDECODE": "1", "NO_COLOR": "1",
            "TERM": "dumb", "COLUMNS": "40", "K2_JSON": "1", "K2_DEBUG": "1",
        })
        if kind in ("C", "Con"):
            env["K2_HOOK_SOCK"] = cards_sock[kind]
            env["PATH"] = os.path.join(sandbox, "stub-cell") + ":" + os.environ.get("PATH", "/usr/bin:/bin")
    return env, cwd

STUBBED = ["curl", "wget", "nc", "launchctl", "systemctl", "sudo", "security", "open", "osascript",
           "pkill", "killall", "ssh", "k2", "k5", "git", "python3", "sqlite3"]
for d, real in (("stub", ()), ("stub-cell", ("curl", "python3"))):
    os.makedirs(os.path.join(sandbox, d))
    for t in STUBBED:
        if t in real:
            continue
        p = os.path.join(sandbox, d, t)
        with open(p, "w") as f:
            f.write("#!/bin/sh\necho \"stub %s $*\" >&2\nexit 97\n" % t)
        os.chmod(p, 0o755)
real_curl = shutil.which("curl")
if not real_curl:
    sys.exit("FAIL: no curl on PATH for the cell environments")
# In a cell, curl runs for a request to a Unix socket (the stub's) only.
with open(os.path.join(sandbox, "stub-cell", "curl"), "w") as f:
    f.write("#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --unix-socket ] && exec %s \"$@\"; done\n"
            "echo \"stub curl $*\" >&2\nexit 97\n" % real_curl)
os.chmod(os.path.join(sandbox, "stub-cell", "curl"), 0o755)

# The stub daemon: the cards probe off on one socket, on on the other.
hits = {"C": 0, "Con": 0}
hits_lock = threading.Lock()
cards_sock = {}
class UnixServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
for kind, on in (("C", False), ("Con", True)):
    class Cards(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        kind_, on_ = kind, on
        def do_GET(self):
            with hits_lock:
                hits[self.kind_] += 1
            if self.path.split("?", 1)[0] == "/cli/permission/enabled":
                status, body = 200, json.dumps({"enabled": self.on_}).encode()
            else:
                status, body = 404, b'{"error":"not found"}'
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True
        do_POST = do_GET
        def address_string(self):
            return "cell"
        def log_message(self, *a):
            pass
    # In the environment's own folder, so a path in a trace reads <base>.
    os.makedirs(os.path.join(sandbox, kind), exist_ok=True)
    path = os.path.join(sandbox, kind, "cards.sock")
    srv = UnixServer(path, Cards)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    cards_sock[kind] = path
envs = {k: make_env(k, os.path.join(sandbox, k)) for k in ("A", "B", "C", "Con")}

STEP = re.compile(r"^(\++)([0-9]+): (.*)$")
OK_WORDS = {"case", "shift", "local", "for", "return", "exit", "true", ":", "break", "continue"}

def run(argv, kind, trace):
    env, cwd = envs[kind]
    env = dict(env)
    cmd = [bash_bin, script] + argv
    if trace:
        env["PS4"] = "+${LINENO}: "
        cmd = [bash_bin, "-x", script] + argv
    p = subprocess.run(cmd, env=env, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, timeout=60)
    return p.returncode, p.stdout, p.stderr

def window(trace_err, kind):
    """The steps from the router's case to the EXIT trap, normalised."""
    lines = trace_err.decode("utf-8", "replace").split("\n")
    start = next((i for i, l in enumerate(lines) if re.match(r'^\+[0-9]+: case "\$\{1:-help\}" in$', l)), None)
    if start is None:
        return None, "no router case in the trace"
    end = next((i for i in range(start, len(lines)) if re.match(r"^\+[0-9]+: _permission_note_flush", lines[i])), None)
    if end is None:
        return None, "no EXIT trap in the trace"
    base = os.path.join(sandbox, kind)
    out = []
    for l in lines[start:end]:
        out.append(l.replace(base, "<base>"))
    return out, None

def plain(steps):
    cats = 0
    for l in steps:
        m = STEP.match(l)
        if not m:
            return "a step that spans lines or is not a trace line: %r" % l
        if len(m.group(1)) != 1:
            return "a subshell or command substitution: %r" % l
        body = m.group(3)
        word = body.split(" ", 1)[0]
        rest = body[len(word):].strip()
        if word == "cat":
            if rest:
                return "cat with arguments: %r" % l
            cats += 1
        elif word in OK_WORDS:
            continue
        elif re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", word):
            continue
        elif word in ("'['", "[", "[["):
            toks = rest.split(" ")
            if len(toks) != 4 or toks[1] not in ("=", "==", "!=", "-gt", "-ge", "-lt", "-le", "-eq", "-ne") \
                    or toks[-1] not in ("']'", "]", "]]"):
                return "a test that is not two words compared: %r" % l
        elif word in functions:
            continue
        else:
            return "a step that is not plain: %r" % l
    if cats != 1:
        return "%d cat steps" % cats
    return None

def classify(argv):
    if argv[0] in HELP_FIRST or argv[0].startswith("help-"):
        kinds = ["A", "C", "Con"]
    else:
        kinds = ["A", "B", "C", "Con"]
    outs, wins = [], []
    for k in kinds:
        outs.append(run(argv, k, False))
        t = run(argv, k, True)
        # macOS's bash 3.2 has no BASH_XTRACEFD: the trace and the
        # command's own stderr share fd 2. The command's stderr is the
        # untraced run's, written whole by one step: take it out once.
        err = outs[-1][2]
        trace = t[2]
        if err:
            if err not in trace:
                return argv, None, "the traced run's stderr does not hold the untraced run's"
            trace = trace.replace(err, b"", 1)
        if (t[0], t[1]) != outs[-1][:2]:
            return argv, None, "the traced run differs from the untraced run"
        w, why = window(trace, k)
        if w is None:
            return argv, None, why
        wins.append(w)
    if any(o != outs[0] for o in outs):
        return argv, None, "the output differs between the environments"
    if any(w != wins[0] for w in wins):
        return argv, None, "the trace differs between the environments"
    why = plain(wins[0])
    if why:
        return argv, None, why
    code, out, err = outs[0]
    if out and not err:
        stream, body = "out", out
    elif err and not out:
        stream, body = "err", err
    else:
        return argv, None, "both streams, or neither"
    rels = by_bytes.get(body)
    if not rels:
        return argv, None, "not one help file's bytes"
    want = "/".join(w for w in argv if w not in ("--help", "-h", "help")) if argv[0] not in HELP_FIRST else argv[1]
    rel = want if want in rels else rels[0]
    return argv, (rel, stream, code), None

# The stub answers, both ways, once per command.
for kind, want in (("C", False), ("Con", True)):
    before = hits[kind]
    code, out, err = run(["study", "list"], kind, False)
    line = b"\n  permissions " in out
    if code != 0 or line != want or hits[kind] != before + 1:
        sys.exit("FAIL: k2 study list in %s: exit %d, the permissions line %s, %d requests (want %s, 1): %r"
                 % (kind, code, "shown" if line else "not shown", hits[kind] - before,
                    "shown" if want else "not shown", err[-300:]))
print("help_native: the stub answers the cards probe: off in C, on in Con")

rows, kept = [], {}
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    for argv, row, why in pool.map(classify, cands):
        if row:
            rows.append("%s|%s|%s|%d" % (" ".join(argv), row[0], row[1], row[2]))
        else:
            kept[" ".join(argv)] = why

HEAD = """# The help forms the k5 binary answers itself, each a plain print of one
# cli/text/help file in bash. Written by tests/cli/help_native.sh --write,
# which proves each row from bash's own trace; do not edit by hand.
# <argv>|<file under cli/text/help, no .txt>|<out or err>|<exit code>
"""
text = HEAD + "".join(r + "\n" for r in sorted(rows))
print("help_native: %d forms tried, %d plain prints, %d stay bash" % (len(cands), len(rows), len(kept)))
reasons = {}
for why in kept.values():
    key = re.sub(r": .*", "", why)
    reasons[key] = reasons.get(key, 0) + 1
for k in sorted(reasons):
    print("  bash: %d x %s" % (reasons[k], k))
if os.environ.get("HELP_NATIVE_WHY"):
    for a in sorted(kept):
        print("  bash: %s: %s" % (a, kept[a]))
if write == "1":
    with open(list_path, "w") as f:
        f.write(text)
    print("help_native: wrote %s" % list_path)
    sys.exit(0)
have = open(list_path).read() if os.path.exists(list_path) else ""
if have != text:
    import difflib
    sys.stderr.write("".join(difflib.unified_diff(have.splitlines(True), text.splitlines(True), "committed", "derived")))
    sys.exit("FAIL: %s is not what bash proves (run tests/cli/help_native.sh --write)" % list_path)
print("help_native: PASS: the committed list is what bash proves")
PY
