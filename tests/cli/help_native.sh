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
#                                   (study and chat among them; a study
#                                   file is a page too, its row's page
#                                   study:<name>)
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
# piped through the card filter fail rule 2, so they are not on the list.
#
# The daily tier's three forms (`k2`, `k2 help`, `k2 --help`, which the
# binary answers itself: crates/k5-cli/src/pages.rs, DAILY_FORMS) are
# proved on their own, in A, As, B, T, Ton, C and Con (below): the
# output is _daily, the cards line only where cards are on, then
# _daily-end; and the trace from the router's case to the EXIT trap is
# the same in all, but for that line's echo and the cards probe: plain
# steps as in rule 2 with exactly two cats, one call of
# _permission_cli_enabled, and inside that call only steps on lines of
# the probe's own functions (_permission_cli_enabled and every function
# its code names, again and again). The probe is the binary's
# crates/k5-cli/src/cards.rs, whose answers the Rust parity tests prove
# (crates/k5-cli/tests/help_daily_cards_parity.rs). Each run asks once
# where a passport and a socket or a port are set, never elsewhere.
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
#   As   A with K2_HOOK_SOCK set to C's live socket (an owner in a K2
#        terminal, no passport): nothing is asked. Daily tier only.
#   T    B over TCP: K2_HOST 127.0.0.1 and K5_PORT a stub daemon there
#        whose cards probe says off; curl runs for real only for a request
#        to it, python3 for real. Daily tier only.
#   Ton  T with cards on. Daily tier only.
# Every form runs in A, C and Con, and a family's --help in B too. The
# session checks pass in all of them (a passport, a live socket, no
# CODEX_ setting): the binary runs the same checks for a family's help
# (crates/k5-cli/src/pages.rs, session_checks). Before the forms, a check
# that the stub answers: `k2 study list` in Con shows the permissions
# line, in C it doesn't, and each asks once.
#
# The other help pages bash puts together, and the unknown command, each
# proved the same way (crates/k5-cli/src/pages.rs, Composed and
# unknown_page):
#   -h, an empty word, and help, --help or -h with a word cmd_help has no
#     arm for (every top level page name it lacks, and a few others): the
#     daily tier, proved as the three above.
#   help, --help or -h with deprecated or --deprecated, and
#     help-deprecated: _deprecated, then one printf step per row of
#     _K2_RETIRED_VERBS (read from cli/src/deprecation.sh as bash reads
#     it) on _retired_verbs_map's line, then _deprecated-end; nothing else
#     but plain steps and two cats; nothing asked, in all seven.
#   chat, chat --help, chat -h, chat help (A, B, C, Con, T, Ton: chat is
#     an id tool, so in As bash refuses it before the router) and help,
#     --help or -h with chat (all seven): chat/_head, chat/_manage,
#     chat/_end, plain steps with three cats; with _CHAT_HELP_MANAGE set to
#     anything but empty or 1, two, and no manage piece.
#   a word the router has no arm for, alone and with words after it (in
#     all seven): `Unknown command: <word>` on stderr, the daily tier for
#     the env's cards on stdout, exit 1, asking as the tier asks; the
#     trace is the daily tier's but for two calls of _retired_verb_exit,
#     inside which only its loop runs (local, for, a test of two words,
#     continue), and the line's echo.
#   every retired verb whose first word the router has no arm for, alone
#     and with a word after it (all seven): the row's line on stderr,
#     nothing on stdout, exit 1, nothing asked; inside _retired_verb_exit
#     its loop, then the row's assignment, case, one echo and exit.
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
import concurrent.futures, http.server, json, os, re, shlex, shutil, socketserver, subprocess, sys, threading

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
# A study file is a page too (`help study` prints the study usage), keyed
# study:<name>, after any help file with the same bytes.
study_dir = os.path.join(root, "cli/text/study")
for n in sorted(os.listdir(study_dir)):
    if n.endswith(".txt"):
        by_bytes.setdefault(open(os.path.join(study_dir, n), "rb").read(), []).append("study:" + n[:-4])

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
              "source", "jobs", "tickets", "cal", "study", "chat"]
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
class TCPServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
def handler(kind, on):
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
    return Cards
for kind, on in (("C", False), ("Con", True)):
    # In the environment's own folder, so a path in a trace reads <base>.
    os.makedirs(os.path.join(sandbox, kind), exist_ok=True)
    path = os.path.join(sandbox, kind, "cards.sock")
    srv = UnixServer(path, handler(kind, on))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    cards_sock[kind] = path
envs = {k: make_env(k, os.path.join(sandbox, k)) for k in ("A", "B", "C", "Con")}

# The daily tier's own environments: an owner with HOOK_SOCK set, and an
# agent whose probe goes over TCP to a stub daemon on 127.0.0.1.
tcp_port = {}
for kind, on in (("T", False), ("Ton", True)):
    hits[kind] = 0
    srv = TCPServer(("127.0.0.1", 0), handler(kind, on))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    tcp_port[kind] = srv.server_address[1]
os.makedirs(os.path.join(sandbox, "stub-tcp"))
for t in STUBBED:
    if t == "python3":
        continue
    p = os.path.join(sandbox, "stub-tcp", t)
    with open(p, "w") as f:
        if t == "curl":
            pats = "|".join("http://127.0.0.1:%d/*" % tcp_port[k] for k in ("T", "Ton"))
            f.write("#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in %s) exec %s \"$@\" ;; esac; done\n"
                    "echo \"stub curl $*\" >&2\nexit 97\n" % (pats, real_curl))
        else:
            f.write("#!/bin/sh\necho \"stub %s $*\" >&2\nexit 97\n" % t)
    os.chmod(p, 0o755)
def derive(kind, src, extra, stub_dir):
    env, cwd = make_env(src, os.path.join(sandbox, kind))
    env.update(extra)
    env["PATH"] = os.path.join(sandbox, stub_dir) + ":" + os.environ.get("PATH", "/usr/bin:/bin")
    envs[kind] = (env, cwd)
derive("As", "A", {"K2_HOOK_SOCK": cards_sock["C"]}, "stub-cell")
for kind in ("T", "Ton"):
    derive(kind, "B", {"K2_HOST": "127.0.0.1", "K5_PORT": str(tcp_port[kind])}, "stub-tcp")

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

def plain(steps, want_cats=1):
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
    if cats != want_cats:
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

# The daily tier: k2, k2 help, k2 --help.
k2_lines = open(script).read().split("\n")
def body_ranges(name):
    """The 1-based line ranges of each definition of a function in the script."""
    pat = re.compile(r"^%s\(\) *\{(.*)$" % re.escape(name))
    out = []
    for i, l in enumerate(k2_lines):
        m = pat.match(l)
        if not m:
            continue
        if m.group(1).rstrip().endswith("}"):
            out.append((i + 1, i + 1))
            continue
        end = next((j for j in range(i + 1, len(k2_lines)) if k2_lines[j] == "}"), None)
        if end is None:
            sys.exit("FAIL: no end to %s at line %d" % (name, i + 1))
        out.append((i + 1, end + 1))
    if not out:
        sys.exit("FAIL: no function %s in %s" % (name, script))
    return out
def names_in(name):
    found = set()
    for a, b in body_ranges(name):
        found |= set(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", "\n".join(k2_lines[a - 1:b])))
    return {f for f in found if f in functions and f != name}
probe_funcs, todo = {"_permission_cli_enabled"}, ["_permission_cli_enabled"]
while todo:
    for f in names_in(todo.pop()) - probe_funcs:
        probe_funcs.add(f)
        todo.append(f)
probe_lines = set()
for f in probe_funcs:
    for a, b in body_ranges(f):
        probe_lines.update(range(a, b + 1))
daily_def = body_ranges("cmd_help_v2_daily")
if len(daily_def) != 1:
    sys.exit("FAIL: cmd_help_v2_daily is defined %d times" % len(daily_def))
daily_lo, daily_hi = daily_def[0]
if k2_lines[daily_hi - 2] != "EOF":
    sys.exit("FAIL: cmd_help_v2_daily does not end with its second heredoc")
echo_lines = [n for n in range(daily_lo, daily_hi + 1) if "echo '  permission request " in k2_lines[n - 1]]
if len(echo_lines) != 1:
    sys.exit("FAIL: %d cards lines in cmd_help_v2_daily" % len(echo_lines))
echo_line = echo_lines[0]
m = re.search(r"^        echo '(  permission request [^']*)'$", open(os.path.join(root, "cli/src/help.sh"), encoding="utf-8").read(), re.M)
if not m:
    sys.exit("FAIL: no cards line in cli/src/help.sh")
cards_line = m.group(1).encode("utf-8") + b"\n"
DAILY = [[], ["help"], ["--help"]]
DAILY_ENVS = ["A", "As", "B", "T", "Ton", "C", "Con"]
ON = {"Ton", "Con"}
ASKS = {"T", "Ton", "C", "Con"}

def daily_split(steps):
    """The steps outside the probe, the cards line's echo steps, and the number of probe calls."""
    outside, echo, calls, in_probe = [], [], 0, False
    for l in steps:
        m = STEP.match(l)
        if in_probe:
            if m and len(m.group(1)) == 1 and daily_lo <= int(m.group(2)) <= daily_hi:
                in_probe = False
            elif m and int(m.group(2)) not in probe_lines:
                return None, "a step in the cards probe on a line of none of its functions: %r" % l
            else:
                # A probe step, or the next line of one that spans lines.
                continue
        if not m:
            return None, "a step that spans lines or is not a trace line: %r" % l
        n, body = int(m.group(2)), m.group(3)
        if n == echo_line:
            echo.append(l)
            continue
        outside.append(l)
        if body == "_permission_cli_enabled" and daily_lo <= n <= daily_hi:
            calls += 1
            in_probe = True
    if in_probe:
        return None, "the trace ends inside the cards probe"
    return (outside, echo, calls), None

def daily_check(argv):
    label = "k2 " + " ".join(argv) if argv else "k2"
    outsides = []
    for kind in DAILY_ENVS:
        want = files["_daily"] + (cards_line if kind in ON else b"") + files["_daily-end"]
        got = []
        for trace in (False, True):
            before = dict(hits)
            got.append(run(argv, kind, trace))
            asked = {k: hits[k] - before[k] for k in hits}
            want_asked = {k: (1 if k == kind and kind in ASKS else 0) for k in hits}
            if asked != want_asked:
                sys.exit("FAIL: %s in %s asked %r, want %r" % (label, kind, asked, want_asked))
        (code, out, err), (tcode, tout, trace_err) = got
        if (code, out, err) != (0, want, b""):
            sys.exit("FAIL: %s in %s: exit %d, %d bytes on stdout (want the tier's %d, cards %s), stderr %r"
                     % (label, kind, code, len(out), len(want), "on" if kind in ON else "off", err[-300:]))
        if (tcode, tout) != (code, out):
            sys.exit("FAIL: %s in %s: the traced run differs from the untraced run" % (label, kind))
        w, why = window(trace_err, kind)
        if w is None:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        split, why = daily_split(w)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outside, echo, calls = split
        if calls != 1:
            sys.exit("FAIL: %s in %s: %d calls of the cards probe, want 1" % (label, kind, calls))
        if len(echo) != (1 if kind in ON else 0) or any(not STEP.match(e).group(3).startswith("echo ") for e in echo):
            sys.exit("FAIL: %s in %s: the cards line's steps %r" % (label, kind, echo))
        why = plain(outside, want_cats=2)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outsides.append(outside)
    if any(o != outsides[0] for o in outsides):
        sys.exit("FAIL: %s: the trace outside the cards probe differs between the environments" % label)

for argv in DAILY:
    daily_check(argv)
print("help_native: the daily tier (k2, k2 help, k2 --help) in %s: bash prints _daily, the cards line "
      "only with cards on, then _daily-end, asking once with a passport and a socket or a port; the trace "
      "is plain but for the line and the cards probe (%d functions)"
      % (", ".join(DAILY_ENVS), len(probe_funcs)))
if os.environ.get("HELP_NATIVE_WHY"):
    print("  the cards probe's functions: %s" % " ".join(sorted(probe_funcs)))

# The other forms of the daily tier: -h, an empty word, and help, --help
# or -h with a word cmd_help has no arm for (crates/k5-cli/src/pages.rs,
# Composed::Daily). Each is proved as the three above.
wiki = open(os.path.join(root, "cli/src/wiki.sh")).read()
m = re.search(r"^cmd_help\(\) \{\n.*?^    case \"\$\{1:-\}\" in\n(.*?)^    esac\n", wiki, re.S | re.M)
if not m:
    sys.exit("FAIL: no cmd_help case in cli/src/wiki.sh")
help_arms = set()
for l in m.group(1).splitlines():
    pm = re.match(r"^        ([^ )][^)]*)\)", l)
    if pm:
        help_arms.update(w for w in pm.group(1).split("|") if w not in ("''", "*"))
if len(help_arms) < 50:
    sys.exit("FAIL: read only %d of cmd_help's words" % len(help_arms))
page_words = sorted(r for r in files if "/" not in r and not r.startswith("_") and r not in help_arms)
odd_words = ["nope", "", "HELP", "--json", "-h", "help", "a b"]
DAILY_MORE = [["-h"], [""]] + [["help", w] for w in page_words] + \
    [[first, w] for w in odd_words for first in HELP_FIRST]
for argv in DAILY_MORE:
    daily_check(argv)
print("help_native: the daily tier for -h, an empty word and %d forms of help <a word cmd_help has no arm "
      "for> (%d page names, %d others, each after help, --help and -h), proved as the three above"
      % (len(DAILY_MORE) - 2, len(page_words), len(odd_words)))

# The retired verbs' table, as bash reads it (cli/src/deprecation.sh).
dep = open(os.path.join(root, "cli/src/deprecation.sh"), encoding="utf-8").read()
m = re.search(r"^_K2_RETIRED_VERBS=\(\n(.*?)^\)\n", dep, re.S | re.M)
if not m:
    sys.exit("FAIL: no _K2_RETIRED_VERBS in cli/src/deprecation.sh")
retired = []
for l in m.group(1).splitlines():
    rm = re.match(r'^    "(.*)"$', l)
    if not rm or "$" in l or "`" in l or "\\" in rm.group(1).replace('\\"', ""):
        sys.exit("FAIL: a retired row this test can't read as bash does: %r" % l)
    k, v = rm.group(1).replace('\\"', '"').split("|", 1)
    if not k.isascii() or len(k) > 31:
        sys.exit("FAIL: a retired key wider than the map's column: %r" % k)
    retired.append((k, v))
if len(retired) < 20:
    sys.exit("FAIL: only %d retired rows" % len(retired))
retired_map = "".join("  %-31s → %s\n" % (k, v) for k, v in retired).encode("utf-8")
def retired_line(k, v):
    if v.startswith("("):
        return ("Error: '%s' is retired %s. Run 'k2 help-deprecated' for the full map.\n" % (k, v)).encode("utf-8")
    return ("Error: '%s' is retired. Use instead: %s\n" % (k, v)).encode("utf-8")
exit_lo, exit_hi = body_ranges("_retired_verb_exit")[0]
map_lo, map_hi = body_ranges("_retired_verbs_map")[0]
if len(body_ranges("_retired_verb_exit")) != 1 or len(body_ranges("_retired_verbs_map")) != 1:
    sys.exit("FAIL: the retired verbs' functions are defined more than once")
star = next((i + 1 for i, l in enumerate(k2_lines) if l == '        echo "Unknown command: $1" >&2'
             and k2_lines[i - 1] == '        _retired_verb_exit "$1"'), None)
if not star:
    sys.exit("FAIL: no Unknown command line in the router's *) arm")

def retired_steps(steps, matched):
    """The steps outside _retired_verb_exit, and the calls of it. Inside,
    only the loop's steps; with `matched`, then the row's line and exit."""
    outside, calls, echoes = [], 0, 0
    for l in steps:
        m = STEP.match(l)
        if not m or len(m.group(1)) != 1:
            # Not a step of its own (the cards probe's, or the next line
            # of one): the caller's checks judge it.
            outside.append(l)
            continue
        n, body = int(m.group(2)), m.group(3)
        if exit_lo <= n <= exit_hi:
            word = body.split(" ", 1)[0]
            if word in ("local", "for", "continue"):
                continue
            if word in ("'['", "["):
                try:
                    toks = shlex.split(body)
                except ValueError:
                    return None, "a test this check can't read: %r" % l
                if len(toks) == 5 and toks[2] == "=" and toks[4] == "]":
                    continue
                return None, "a test that is not two words compared: %r" % l
            if matched and (word.startswith("new=") or word in ("case", "exit")):
                continue
            if matched and word == "echo":
                echoes += 1
                continue
            return None, "a step of a retired verb's branch: %r" % l
        if body.startswith("_retired_verb_exit "):
            calls += 1
        outside.append(l)
    if echoes != (1 if matched else 0):
        return None, "%d echo steps in _retired_verb_exit" % echoes
    return (outside, calls), None

def form_label(argv):
    return "k2 " + " ".join(repr(a) if (not a or " " in a) else a for a in argv)

def unknown_check(argv):
    """`k2 <a word the router has no arm for> ...`: the router's *) arm."""
    label = form_label(argv)
    outsides = []
    for kind in DAILY_ENVS:
        want = files["_daily"] + (cards_line if kind in ON else b"") + files["_daily-end"]
        want_err = ("Unknown command: %s\n" % argv[0]).encode("utf-8")
        got = []
        for trace in (False, True):
            before = dict(hits)
            got.append(run(argv, kind, trace))
            asked = {k: hits[k] - before[k] for k in hits}
            want_asked = {k: (1 if k == kind and kind in ASKS else 0) for k in hits}
            if asked != want_asked:
                sys.exit("FAIL: %s in %s asked %r, want %r" % (label, kind, asked, want_asked))
        (code, out, err), (tcode, tout, trace_err) = got
        if (code, out, err) != (1, want, want_err):
            sys.exit("FAIL: %s in %s: exit %d, %d bytes on stdout (want the tier's %d, cards %s), stderr %r"
                     % (label, kind, code, len(out), len(want), "on" if kind in ON else "off", err[-300:]))
        if (tcode, tout) != (code, out) or err not in trace_err:
            sys.exit("FAIL: %s in %s: the traced run differs from the untraced run" % (label, kind))
        w, why = window(trace_err.replace(err, b"", 1), kind)
        if w is None:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        split, why = retired_steps(w, False)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        rest, calls = split
        if calls != 2:
            sys.exit("FAIL: %s in %s: %d calls of _retired_verb_exit, want 2" % (label, kind, calls))
        lead = [l for l in rest if STEP.match(l) and STEP.match(l).group(1) == "+"
                and int(STEP.match(l).group(2)) == star]
        if len(lead) != 1 or not STEP.match(lead[0]).group(3).startswith("echo "):
            sys.exit("FAIL: %s in %s: the Unknown command line's steps %r" % (label, kind, lead))
        rest = [l for l in rest if l not in lead]
        split, why = daily_split(rest)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outside, echo, calls = split
        if calls != 1 or len(echo) != (1 if kind in ON else 0):
            sys.exit("FAIL: %s in %s: %d probe calls, cards line steps %r" % (label, kind, calls, echo))
        why = plain(outside, want_cats=2)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outsides.append([l.replace(argv[0], "<w>") for l in outside])
    if any(o != outsides[0] for o in outsides):
        sys.exit("FAIL: %s: the trace outside the cards probe differs between the environments" % label)

arms = set()
rm_ = re.search(r'^case "\$\{1:-help\}" in\n(.*?)^esac\n', open(os.path.join(root, "cli/src/router.sh")).read(), re.S | re.M)
for l in rm_.group(1).splitlines():
    am = re.match(r"^    ([^ #(][^)]*)\)", l)
    if am:
        arms.update(w.strip("'\"") for w in am.group(1).split("|"))
if len(arms) < 100:
    sys.exit("FAIL: read only %d router words" % len(arms))
UNKNOWN = [["nope"], ["Help"], ["--nope"], ["-x"], ["x-y_z"], ["a b"], ["agents"], ["agents", "nope"],
           ["work"], ["whoo"], ["nope", "extra"], ["nope", "--json"], ["nope", "-h"], ["nope", "--help", "x"]]
for argv in UNKNOWN:
    if argv[0] in arms:
        sys.exit("FAIL: %s is a router word" % argv[0])
    unknown_check(argv)
print("help_native: the unknown command in %s: %d forms, each the line on stderr, then the daily tier "
      "(asking once with a passport and a socket or a port), exit 1; inside _retired_verb_exit only "
      "its loop, twice" % (", ".join(DAILY_ENVS), len(UNKNOWN)))

def quiet_check(argv, want_code, want_out, want_err, kinds, judge):
    """A form that asks nothing: the same output and the same trace in
    `kinds`, and `judge(window)` passes it."""
    label = form_label(argv)
    wins = []
    for kind in kinds:
        code, out, err = run(argv, kind, False)
        tcode, tout, terr = run(argv, kind, True)
        if (code, out, err) != (want_code, want_out, want_err):
            return "%s in %s: exit %d, stdout %r..., stderr %r" % (label, kind, code, out[:80], err[-200:])
        if (tcode, tout) != (code, out) or err not in terr:
            return "%s in %s: the traced run differs from the untraced run" % (label, kind)
        w, why = window(terr.replace(err, b"", 1) if err else terr, kind)
        if w is None:
            return "%s in %s: %s" % (label, kind, why)
        wins.append(w)
    if any(w != wins[0] for w in wins):
        return "%s: the trace differs between the environments" % label
    why = judge(wins[0])
    return "%s: %s" % (label, why) if why else None

def judge_deprecated(steps):
    bad = [l for l in steps if not STEP.match(l)]
    if bad:
        return "a step that spans lines or is not a trace line: %r" % bad[0]
    prints = [l for l in steps if map_lo <= int(STEP.match(l).group(2)) <= map_hi
              and STEP.match(l).group(3).startswith("printf ")]
    if len(prints) != len(retired):
        return "%d printf steps in _retired_verbs_map, want %d" % (len(prints), len(retired))
    return plain([l for l in steps if l not in prints], want_cats=2)

def judge_retired(steps):
    split, why = retired_steps(steps, True)
    if why:
        return why
    outside, calls = split
    if calls not in (1, 2):
        return "%d calls of _retired_verb_exit" % calls
    return plain(outside, want_cats=0)

chat_pieces = [files["chat/_head"], files["chat/_manage"], files["chat/_end"]]
QUIET = []
for first in HELP_FIRST:
    for w in ("deprecated", "--deprecated"):
        QUIET.append(([first, w], 0, files["_deprecated"] + retired_map + files["_deprecated-end"], b"",
                      DAILY_ENVS, judge_deprecated))
QUIET.append((["help-deprecated"], 0, files["_deprecated"] + retired_map + files["_deprecated-end"], b"",
              DAILY_ENVS, judge_deprecated))
# chat is an id tool: with K2_HOOK_SOCK set and no passport (As) its own
# forms are refused before the router (the Rust parity tests prove that);
# help chat is the open tool help.
for argv in (["chat"], ["chat", "--help"], ["chat", "-h"], ["chat", "help"]):
    QUIET.append((argv, 0, b"".join(chat_pieces), b"", ["A", "B", "C", "Con", "T", "Ton"],
                  lambda w: plain(w, want_cats=3)))
for first in HELP_FIRST:
    QUIET.append(([first, "chat"], 0, b"".join(chat_pieces), b"", DAILY_ENVS, lambda w: plain(w, want_cats=3)))
n_retired = 0
for k, v in retired:
    words = k.split(" ")
    if words[0] in arms:
        continue  # `agent create`: the agent arm's own lookup
    n_retired += 1
    for argv in (words, words + ["x"]):
        QUIET.append((argv, 1, b"", retired_line(k, v), DAILY_ENVS, judge_retired))
before = dict(hits)
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    fails = [f for f in pool.map(lambda q: quiet_check(*q), QUIET) if f]
if fails:
    sys.exit("FAIL: " + "\nFAIL: ".join(fails))
if hits != before:
    sys.exit("FAIL: a form that asks nothing asked: %r, before %r" % (hits, before))
# _CHAT_HELP_MANAGE: anything but empty or 1 leaves the manage piece out.
for val, want in (("0", chat_pieces[0] + chat_pieces[2]), ("", b"".join(chat_pieces)),
                  ("1", b"".join(chat_pieces)), ("yes", chat_pieces[0] + chat_pieces[2])):
    for kind in ("A", "C"):
        envs[kind][0]["_CHAT_HELP_MANAGE"] = val
    try:
        why = quiet_check(["chat", "--help"], 0, want, b"", ["A", "C"],
                          lambda w: plain(w, want_cats=2 if want != b"".join(chat_pieces) else 3))
    finally:
        for kind in ("A", "C"):
            del envs[kind][0]["_CHAT_HELP_MANAGE"]
    if why:
        sys.exit("FAIL: _CHAT_HELP_MANAGE=%r: %s" % (val, why))
print("help_native: the deprecated tier (%d forms: _deprecated, the %d rows' printf, _deprecated-end), "
      "chat's usage (%d forms: its three pieces, the manage one by _CHAT_HELP_MANAGE) and %d retired "
      "verbs (alone and with a word after: the row's line, exit 1) in their environments; nothing asked"
      % (7, len(retired), 7, n_retired))

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
