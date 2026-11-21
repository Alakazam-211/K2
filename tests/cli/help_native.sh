#!/usr/bin/env bash
# bash-only: it traces the bash CLI itself (bash -x) to prove which help forms are a plain print.
#
# Which help forms the k5 binary may answer itself: the ones where the bash
# CLI does nothing but print one cli/text/help file. The list is
# crates/k5-cli/src/help_native.txt; the binary embeds it (build.rs).
# Then every other help form of the router whose output bash only prints,
# the same everywhere: crates/k5-cli/src/help_captured.txt, what bash
# printed for each (the captured forms, at the end of this script).
#
#   bash tests/cli/help_native.sh           derive both lists from bash, and
#                                           fail unless they are the committed ones
#   bash tests/cli/help_native.sh --write   derive them and write them
# HELP_NATIVE_WHY=1 also prints each form bash keeps, with the reason.
# HELP_NATIVE_DUMP=<file> writes the captured forms and the kept ones'
# reasons there as JSON. HELP_NATIVE_CAPTURED=<file> reads (or writes) the
# captured list there instead (the bite: a list with one page changed must
# fail). HELP_NATIVE_JOBS sets how many forms run at once (4).
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
# The k2 help tiers made of pieces (`k2 --help` is _daily, _daily-feedback
# unless an agent's server says agents may not report, a line when
# permission cards are on, then _daily-end) and every page piped through
# the card filter fail rule 2, so they are not on the list.
#
# The daily tier's three forms (`k2`, `k2 help`, `k2 --help`, which the
# binary answers itself: crates/k5-cli/src/pages.rs, DAILY_FORMS) are
# proved on their own, in A, As, B, T, Ton, Toff, C, Con and Coff
# (below): the output is _daily, _daily-feedback but where agents are off
# (Toff, Coff), the cards line only where cards are on, then _daily-end;
# and the trace from the router's case to the EXIT trap is the same in
# all, but for the block's cat, that line's echo and the two probes:
# plain steps as in rule 2 with exactly two cats besides the block's, one
# call of _sf_agents_off_here (the feedback probe) then one of
# _permission_cli_enabled (the cards probe), the block's cat between them
# exactly where agents are not off, and inside each call only steps on
# lines of that probe's own functions (the function and every function
# its code names, again and again). The probes are the binary's
# crates/k5-cli/src/feedback_optin.rs and cards.rs, whose answers the
# Rust parity tests prove (crates/k5-cli/tests/help_feedback_optin_parity.rs
# and help_daily_cards_parity.rs). Each run asks twice (the settings,
# then the cards) where a passport and a socket or a port are set, never
# elsewhere.
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
#        probe (GET /cli/permission/enabled) says cards are off, and the
#        feedback settings (GET /cli/product-feedback/settings) that
#        agents may report. curl runs for real only for a request to that
#        socket, python3 for real always, so a form that asks reads the
#        answer as it would.
#   Con  C with cards on.
#   Coff C with cards on and agents off (send-feedback hidden). Daily
#        tier only.
#   As   A with K2_HOOK_SOCK set to C's live socket (an owner in a K2
#        terminal, no passport): nothing is asked. Daily tier only.
#   T    B over TCP: K2_HOST 127.0.0.1 and K5_PORT a stub daemon there
#        whose cards probe says off; curl runs for real only for a request
#        to it, python3 for real. Daily tier only.
#   Ton  T with cards on. Daily tier only.
#   Toff T with agents off. Daily tier only.
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
#     but plain steps and two cats; nothing asked, in all nine.
#   chat, chat --help, chat -h, chat help (A, B, C, Con, T, Ton: chat is
#     an id tool, so in As bash refuses it before the router) and help,
#     --help or -h with chat (all nine): chat/_head, chat/_manage,
#     chat/_end, plain steps with three cats; with _CHAT_HELP_MANAGE set to
#     anything but empty or 1, two, and no manage piece.
#   a word the router has no arm for, alone and with words after it (in
#     all nine): `Unknown command: <word>` on stderr, the daily tier for
#     the env's cards and agents on stdout, exit 1, asking as the tier asks; the
#     trace is the daily tier's but for two calls of _retired_verb_exit,
#     inside which only its loop runs (local, for, a test of two words,
#     continue), and the line's echo.
#   every retired verb whose first word the router has no arm for, alone
#     and with a word after it (all nine): the row's line on stderr,
#     nothing on stdout, exit 1, nothing asked; inside _retired_verb_exit
#     its loop, then the row's assignment, case, one echo and exit.
#
# Never touches ~/.k2. The stub daemon is this script's own, on three Unix
# sockets in the scratch folder and three loopback ports.
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
        if kind in ("C", "Con", "Coff"):
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

# The stub daemon: the cards probe off on one socket, on on the others;
# the feedback settings say agents may report but where AGENTS_OFF.
AGENTS_OFF = {"Coff", "Toff"}
hits = {"C": 0, "Con": 0, "Coff": 0}
hits_lock = threading.Lock()
# Every request: the environment whose stub took it, and its path and
# headers as one string. The captured forms run with a passport of their
# own, so a request names its form.
requests_seen = []
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
                requests_seen.append((self.kind_, self.path + "\n" + str(self.headers)))
            route = self.path.split("?", 1)[0]
            if route == "/cli/permission/enabled":
                status, body = 200, json.dumps({"enabled": self.on_}).encode()
            elif route == "/cli/product-feedback/settings":
                off = self.kind_ in AGENTS_OFF
                status, body = 200, json.dumps({"enabled": True, "agents": not off, "canSend": not off,
                                                "senderKind": "agent", "ok": True}).encode()
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
for kind, on in (("C", False), ("Con", True), ("Coff", True)):
    # In the environment's own folder, so a path in a trace reads <base>.
    os.makedirs(os.path.join(sandbox, kind), exist_ok=True)
    path = os.path.join(sandbox, kind, "cards.sock")
    srv = UnixServer(path, handler(kind, on))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    cards_sock[kind] = path
envs = {k: make_env(k, os.path.join(sandbox, k)) for k in ("A", "B", "C", "Con", "Coff")}

# The daily tier's own environments: an owner with HOOK_SOCK set, and an
# agent whose probe goes over TCP to a stub daemon on 127.0.0.1.
tcp_port = {}
for kind, on in (("T", False), ("Ton", True), ("Toff", False)):
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
            pats = "|".join("http://127.0.0.1:%d/*" % tcp_port[k] for k in ("T", "Ton", "Toff"))
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
for kind in ("T", "Ton", "Toff"):
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
def closure(name):
    """The function and every function its code names, again and again,
    and the lines of all their bodies."""
    funcs, todo = {name}, [name]
    while todo:
        for f in names_in(todo.pop()) - funcs:
            funcs.add(f)
            todo.append(f)
    lines = set()
    for f in funcs:
        for a, b in body_ranges(f):
            lines.update(range(a, b + 1))
    return funcs, lines
# The two probes in cmd_help_v2_daily's order: the feedback settings, then
# the cards.
FEEDBACK_PROBE, CARDS_PROBE = "_sf_agents_off_here", "_permission_cli_enabled"
feedback_funcs, feedback_lines = closure(FEEDBACK_PROBE)
probe_funcs, probe_lines = closure(CARDS_PROBE)
PROBES = {FEEDBACK_PROBE: feedback_lines, CARDS_PROBE: probe_lines}
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
DAILY_ENVS = ["A", "As", "B", "T", "Ton", "Toff", "C", "Con", "Coff"]
ON = {"Ton", "Con", "Coff"}
ASKS = {"T", "Ton", "Toff", "C", "Con", "Coff"}
HIDE = AGENTS_OFF

def daily_want(kind):
    """The daily tier as bash prints it in `kind`."""
    return (files["_daily"] + (b"" if kind in HIDE else files["_daily-feedback"])
            + (cards_line if kind in ON else b"") + files["_daily-end"])

def daily_split(steps, kind):
    """The steps outside the probes (the block's cat taken out), the cards
    line's echo steps, and the number of calls of each probe."""
    outside, echo, calls, in_probe = [], [], {p: 0 for p in PROBES}, None
    for l in steps:
        m = STEP.match(l)
        if in_probe:
            if m and len(m.group(1)) == 1 and daily_lo <= int(m.group(2)) <= daily_hi:
                in_probe = None
            elif m and int(m.group(2)) not in PROBES[in_probe]:
                return None, "a step in %s on a line of none of its functions: %r" % (in_probe, l)
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
        if body in PROBES and daily_lo <= n <= daily_hi:
            calls[body] += 1
            in_probe = body
    if in_probe:
        return None, "the trace ends inside %s" % in_probe
    calls_at = [i for i, l in enumerate(outside) if STEP.match(l).group(3) in PROBES
                and daily_lo <= int(STEP.match(l).group(2)) <= daily_hi]
    order = [STEP.match(outside[i]).group(3) for i in calls_at]
    if order != [FEEDBACK_PROBE, CARDS_PROBE]:
        return None, "the probes ran as %r, want %s then %s" % (order, FEEDBACK_PROBE, CARDS_PROBE)
    between = outside[calls_at[0] + 1:calls_at[1]]
    want_between = [] if kind in HIDE else ["cat"]
    if [STEP.match(l).group(3) for l in between] != want_between:
        return None, "between the probes %r, want %r" % (between, want_between)
    outside = outside[:calls_at[0] + 1] + outside[calls_at[1]:]
    return (outside, echo, calls), None

def daily_check(argv):
    label = "k2 " + " ".join(argv) if argv else "k2"
    outsides = []
    for kind in DAILY_ENVS:
        want = daily_want(kind)
        got = []
        for trace in (False, True):
            before = dict(hits)
            got.append(run(argv, kind, trace))
            asked = {k: hits[k] - before[k] for k in hits}
            want_asked = {k: (2 if k == kind and kind in ASKS else 0) for k in hits}
            if asked != want_asked:
                sys.exit("FAIL: %s in %s asked %r, want %r" % (label, kind, asked, want_asked))
        (code, out, err), (tcode, tout, trace_err) = got
        if (code, out, err) != (0, want, b""):
            sys.exit("FAIL: %s in %s: exit %d, %d bytes on stdout (want the tier's %d, cards %s, agents %s), stderr %r"
                     % (label, kind, code, len(out), len(want), "on" if kind in ON else "off",
                        "off" if kind in HIDE else "on", err[-300:]))
        if (tcode, tout) != (code, out):
            sys.exit("FAIL: %s in %s: the traced run differs from the untraced run" % (label, kind))
        w, why = window(trace_err, kind)
        if w is None:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        split, why = daily_split(w, kind)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outside, echo, calls = split
        if calls != {p: 1 for p in PROBES}:
            sys.exit("FAIL: %s in %s: the probes' calls %r, want one each" % (label, kind, calls))
        if len(echo) != (1 if kind in ON else 0) or any(not STEP.match(e).group(3).startswith("echo ") for e in echo):
            sys.exit("FAIL: %s in %s: the cards line's steps %r" % (label, kind, echo))
        why = plain(outside, want_cats=2)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outsides.append(outside)
    if any(o != outsides[0] for o in outsides):
        sys.exit("FAIL: %s: the trace outside the probes differs between the environments" % label)

for argv in DAILY:
    daily_check(argv)
print("help_native: the daily tier (k2, k2 help, k2 --help) in %s: bash prints _daily, _daily-feedback "
      "but where agents are off, the cards line only with cards on, then _daily-end, asking twice (the "
      "settings, then the cards) with a passport and a socket or a port; the trace is plain but for the "
      "block, the line and the two probes (%d and %d functions)"
      % (", ".join(DAILY_ENVS), len(feedback_funcs), len(probe_funcs)))
if os.environ.get("HELP_NATIVE_WHY"):
    print("  the feedback probe's functions: %s" % " ".join(sorted(feedback_funcs)))
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
        want = daily_want(kind)
        want_err = ("Unknown command: %s\n" % argv[0]).encode("utf-8")
        got = []
        for trace in (False, True):
            before = dict(hits)
            got.append(run(argv, kind, trace))
            asked = {k: hits[k] - before[k] for k in hits}
            want_asked = {k: (2 if k == kind and kind in ASKS else 0) for k in hits}
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
        split, why = daily_split(rest, kind)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outside, echo, calls = split
        if calls != {p: 1 for p in PROBES} or len(echo) != (1 if kind in ON else 0):
            sys.exit("FAIL: %s in %s: %d probe calls, cards line steps %r" % (label, kind, calls, echo))
        why = plain(outside, want_cats=2)
        if why:
            sys.exit("FAIL: %s in %s: %s" % (label, kind, why))
        outsides.append([l.replace(argv[0], "<w>") for l in outside])
    if any(o != outsides[0] for o in outsides):
        sys.exit("FAIL: %s: the trace outside the probes differs between the environments" % label)

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
      "(asking twice with a passport and a socket or a port), exit 1; inside _retired_verb_exit only "
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

rows, kept, file_forms = [], {}, set()
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    for argv, row, why in pool.map(classify, cands):
        if row:
            rows.append("%s|%s|%s|%d" % (" ".join(argv), row[0], row[1], row[2]))
            file_forms.add(tuple(argv))
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

# ---- The captured forms ------------------------------------------------
# Every other help form of the router (crates/k5-cli/tests/data/
# router_forms.tsv, scripts/router-forms.py): each form with help, --help
# or -h among its words, and each form that is not an alias with --help and
# with -h after it (the native bar's H class, crates/k5-cli/tests/
# native_bar.rs, and its -h twin). Leaving out the forms above (a file's
# print), the pages the binary puts together (pages.rs, Composed: chat's
# usage, help/--help/-h with a word), the top level help words, and `cli`
# and `study`, whose pages the front words itself.
#
# A form is captured when, run as bash in A, B, C and Con:
#   1. stdout, stderr and the exit code are the same in all four, the
#      exit code is one the binary has (crates/k5-cli/src/exit.rs), and
#      both streams are UTF-8 with no control byte but newline and tab;
#   2. the trace from the router's case to the EXIT trap is the same in
#      all four, and each step in it, at any depth (a command
#      substitution's too), is one of: case, shift, local, for, return,
#      exit, true, :, break, continue, set, a variable assignment, a call
#      of a function the script defines, echo, printf, cat with no
#      arguments (a heredoc), and a `[` or `[[` that compares two words or
#      asks whether one is empty (-n, -z); never another program, a file
#      test, read, eval or source;
#   3. no line of the script those steps run reads a variable the script
#      never assigns (one only the environment can set), but the shell's
#      own (the arguments, $?, $#, LINENO, FUNCNAME and the like), nor the
#      CLI's version (K2_CLI_VERSION: every release would move the list).
# The traced run (BASH_XTRACEFD, so the trace has its own file) gives the
# output; the untraced run in A must give the same.
# Two kinds of form have a page for each way one switch goes, each page
# proved as above within its own environments (cards_capture and
# manage_capture, below, say exactly how):
#   - a page through the permission cards filter (_perm_lines: the help of
#     app and skin), off in A, B, C and T, on in Con and Ton, the probe's
#     call left out of the trace (only its own functions' lines inside)
#     and asked once a run where a stub answers (a C row);
#   - chat's usage (_chat_usage), with _CHAT_HELP_MANAGE unset and set
#     to 0, that one read allowed (an M row).
# The binary prints what was captured (crates/k5-cli/src/help_captured.txt,
# embedded by build.rs), behind the same gate as a file's print: a port and
# a token, and with K2_HOOK_SOCK set the session checks (pages.rs,
# help_page). Its text names the command and the product by the brand:
# `k2` before a space is the command's name (\c), and `K2` the product's
# (\p) where nothing a name goes on with is after it (a `.` or `:` only
# when no letter, digit, `_` or `/` follows), each where no letter, digit,
# `_`, `.`, `/`, `~`, `|`, `<`, `:` or `-` is before it; everything else
# (`~/.k2`, `K2_HOME`, `k2.dev`, `--managed-by k2|owner`) is kept as bash
# printed it.
CAPTURED = os.environ.get("HELP_NATIVE_CAPTURED") or os.path.join(root, "crates/k5-cli/src/help_captured.txt")
FORMS_TSV = os.path.join(root, "crates/k5-cli/tests/data/router_forms.tsv")
EXIT_CODES = {0, 1, 2, 3, 4, 5, 6, 7, 8, 26, 69, 127, 130}
xtracefd = subprocess.run([bash_bin, "-c", 'echo "${BASH_VERSINFO[0]}${BASH_VERSINFO[1]}"'],
                          capture_output=True).stdout.decode().strip()
XTRACEFD = xtracefd.isdigit() and int(xtracefd) >= 41
if XTRACEFD:
    # bash takes BASH_XTRACEFD from the environment: the trace goes there.
    import tempfile
    with tempfile.TemporaryFile(dir=sandbox) as tf:
        p = subprocess.run([bash_bin, "-x", "-c", "true"], env={"PATH": "/usr/bin:/bin", "PS4": "+T ",
                           "BASH_XTRACEFD": str(tf.fileno())}, capture_output=True, pass_fds=(tf.fileno(),))
        tf.seek(0)
        XTRACEFD = tf.read() == b"+T true\n" and p.stderr == b""
print("help_native: the captured forms' trace goes %s" % ("to its own file (BASH_XTRACEFD)" if XTRACEFD
                                                          else "with stderr (two runs each)"))

form_lines = [l for l in open(FORMS_TSV, encoding="utf-8").read().split("\n") if l and not l.startswith("#")]
if form_lines[0].split("\t") != ["form", "argv", "alias_of", "anchor", "entry", "json", "source"]:
    sys.exit("FAIL: %s: not the header this test reads" % FORMS_TSV)
HELP_WORDS = {"help", "--help", "-h"}
def composed_form(argv):
    """pages.rs, composed(): the pages the binary puts together."""
    if argv in (["-h"], [""], ["help-deprecated"], ["chat"]):
        return True
    if len(argv) == 2 and argv[0] == "chat" and argv[1] in HELP_WORDS:
        return True
    return len(argv) == 2 and argv[0] in HELP_FIRST
# The front's own pages: `cli` (crates/k5-cli/src/cli_status.rs) and
# `study` (pages.rs) are the binary's, worded for it.
FRONT_OWN = ("cli", "study")
ccands, cseen = [], set()
for l in form_lines[1:]:
    c = l.split("\t")
    if len(c) != 7:
        sys.exit("FAIL: %s: a row of %d columns: %r" % (FORMS_TSV, len(c), l))
    form, argv_col, alias = c[0], c[1], c[2]
    argv = (form if argv_col == "-" else argv_col).split(" ")
    if any(not w or "\t" in w or "|" in w for w in argv):
        sys.exit("FAIL: %s: a word this list can't hold: %r" % (FORMS_TSV, l))
    if any(w in HELP_WORDS for w in form.split(" ")):
        tries = [argv]
    elif alias == "-":
        tries = [argv + ["--help"], argv + ["-h"]]
    else:
        tries = []
    for t in tries:
        key = tuple(t)
        if key in cseen or key in file_forms or composed_form(t) or t[0] in HELP_FIRST or t[0].startswith("help-") \
                or t[0] in FRONT_OWN:
            continue
        cseen.add(key)
        ccands.append(t)
if len(ccands) < 1000:
    sys.exit("FAIL: only %d help forms from %s" % (len(ccands), FORMS_TSV))
ccand_no = {tuple(a): i for i, a in enumerate(ccands)}

# The variables the script assigns somewhere (any other is the environment's).
script_text = open(script, encoding="utf-8", errors="replace").read()
assigned = set(re.findall(r"(?:^|[\s;&|(`{])(?:local |export |declare(?: -[a-zA-Z]+)? |readonly )?([A-Za-z_][A-Za-z0-9_]*)\+?=", script_text, re.M))
assigned |= set(re.findall(r"\b(?:for|read(?: -[a-zA-Z]+)*|local|declare(?: -[a-zA-Z]+)?)\s+([A-Za-z_][A-Za-z0-9_]*)", script_text))
assigned |= set(re.findall(r"printf -v ([A-Za-z_][A-Za-z0-9_]*)", script_text))
SHELL_OWN = {"LINENO", "FUNCNAME", "BASH_SOURCE", "BASH_LINENO", "BASH_VERSINFO", "BASH_VERSION", "BASHPID",
             "PIPESTATUS", "IFS", "OPTIND", "OPTARG", "BASH_REMATCH"}
def env_reads(line):
    """Variables a source line reads that only the environment can set."""
    out = set()
    for m in re.finditer(r"\$\{?#?!?([A-Za-z_][A-Za-z0-9_]*)", line):
        name = m.group(1)
        if name not in assigned and name not in SHELL_OWN:
            out.add(name)
    return out

TEST_ONE = ("-n", "-z")
TEST_TWO = ("=", "==", "!=", "-gt", "-ge", "-lt", "-le", "-eq", "-ne")
WRITE_WORDS = {"echo", "printf", "set"}
def judge_captured(steps, allow_env=None, read_ok=frozenset()):
    """Rules 2 and 3 for a trace window; None when it passes. `allow_env`
    names a variable only the environment sets and the lines that may read
    it (a variant's own switch); `read_ok`, the lines where `read -r line`
    may run (the cards filter's loop, reading its pipe)."""
    allow_env = allow_env or {}
    prev_ok = False
    for l in steps:
        m = STEP.match(l)
        if not m:
            # The next line of a step that spans lines (a word with a
            # newline in it: an echo's text, a local's value), after a step
            # that passed. bash writes a word with a newline as it is or
            # quoted on one line, by its version.
            if prev_ok:
                continue
            return "a line that is not a step: %r" % l
        n, body = int(m.group(2)), m.group(3)
        src = k2_lines[n - 1] if 0 < n <= len(k2_lines) else ""
        if "K2_CLI_VERSION" in src:
            return "prints the CLI's version, which every release moves: line %d" % n
        reads = {r for r in env_reads(src) if n not in allow_env.get(r, ())}
        if reads:
            return "reads %s from the environment: line %d: %r" % (" ".join(sorted(reads)), n, src.strip())
        word = body.split(" ", 1)[0]
        rest = body[len(word):].strip()
        prev_ok = True
        if word == "cat":
            if rest:
                return "cat with arguments: %r" % l
            # An unquoted heredoc's body expands variables too.
            hm = re.search(r"<<(-?)\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\2", src)
            if not hm:
                return "a cat with no heredoc on its line %d: %r" % (n, src.strip())
            if not hm.group(2):
                j = n
                while j < len(k2_lines):
                    body_line = k2_lines[j]
                    j += 1
                    if (body_line.lstrip("\t") if hm.group(1) else body_line) == hm.group(3):
                        break
                    reads = env_reads(body_line)
                    if reads:
                        return "reads %s from the environment: line %d: %r" % (" ".join(sorted(reads)), j, body_line.strip())
        elif word in OK_WORDS:
            pass
        elif word == "read" and n in read_ok and rest == "-r line":
            pass
        elif word in WRITE_WORDS or re.match(r"^[A-Za-z_][A-Za-z0-9_]*\+?=", word):
            pass
        elif word in ("'['", "[", "[["):
            try:
                toks = shlex.split(body)
            except ValueError:
                return "a test this check can't read: %r" % l
            if toks[-1] not in ("]", "]]"):
                return "a test this check can't read: %r" % l
            inner = toks[1:-1]
            ok = (len(inner) == 3 and inner[1] in TEST_TWO) or (len(inner) == 2 and inner[0] in TEST_ONE)
            if not ok:
                return "a test that is not two words compared or one asked about: %r" % l
        elif word in functions:
            pass
        else:
            return "a step that is not plain: %r" % l
    return None

def form_passport(argv):
    """The passport a captured form runs with in B, C and Con: its own,
    so a request the stub sees names the form."""
    return "sessQ.form%d" % ccand_no[tuple(argv)]

# How many runs each form made in each environment with its passport:
# the stub must have seen one probe per run for a form the cards answer
# decides, and none for any other captured form.
runs_by, runs_lock = {}, threading.Lock()
def count_run(argv, kind, env):
    if "K2_HOOK_TOKEN" in env:
        with runs_lock:
            key = (tuple(argv), kind)
            runs_by[key] = runs_by.get(key, 0) + 1

def run_traced(argv, kind, extra=None):
    """(code, stdout, stderr, trace stderr) of one traced run, with
    `extra` settings on top of the environment's."""
    env, cwd = envs[kind]
    env = dict(env)
    env.update(extra or {})
    env["PS4"] = "+${LINENO}: "
    if "K2_HOOK_TOKEN" in env:
        env["K2_HOOK_TOKEN"] = form_passport(argv)
    if not XTRACEFD:
        code, out, err = run_plain(argv, kind, extra)
        count_run(argv, kind, env)
        t = subprocess.run([bash_bin, "-x", script] + argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                           capture_output=True, timeout=60)
        tcode, tout, terr = t.returncode, t.stdout, t.stderr
        if (tcode, tout) != (code, out) or (err and err not in terr):
            return None
        return code, out, err, (terr.replace(err, b"", 1) if err else terr)
    import tempfile
    with tempfile.TemporaryFile(dir=sandbox) as tf:
        env["BASH_XTRACEFD"] = str(tf.fileno())
        count_run(argv, kind, env)
        p = subprocess.run([bash_bin, "-x", script] + argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                           capture_output=True, timeout=60, pass_fds=(tf.fileno(),))
        tf.seek(0)
        return p.returncode, p.stdout, p.stderr, tf.read()

def utf8_text(b):
    try:
        s = b.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if any(ord(ch) < 32 and ch not in "\n\t" for ch in s) or "\x7f" in s:
        return None
    return s

def run_plain(argv, kind, extra=None):
    env, cwd = envs[kind]
    env = dict(env)
    env.update(extra or {})
    if "K2_HOOK_TOKEN" in env:
        env["K2_HOOK_TOKEN"] = form_passport(argv)
    count_run(argv, kind, env)
    p = subprocess.run([bash_bin, script] + argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                       capture_output=True, timeout=60)
    return p.returncode, p.stdout, p.stderr

def groups(kinds, values):
    """`A=B=C | Con`: the environments with the same value, together."""
    seen = []
    for k, v in zip(kinds, values):
        for g in seen:
            if g[1] == v:
                g[0].append(k)
                break
        else:
            seen.append(([k], v))
    return " | ".join("=".join(g[0]) for g in seen)

def first_difference(kinds, wins):
    """Where the first window that differs from the first one parts from it."""
    for k, w in zip(kinds[1:], wins[1:]):
        if w != wins[0]:
            i = next((i for i, (a, b) in enumerate(zip(wins[0], w)) if a != b), min(len(w), len(wins[0])))
            a = wins[0][i] if i < len(wins[0]) else "(the end)"
            b = w[i] if i < len(w) else "(the end)"
            return "%s; step %d, %s %r, %s %r" % (groups(kinds, [tuple(x) for x in wins]), i, kinds[0], a[:160],
                                                   k, b[:160])
    return ""

def page_of(code, out, err):
    """The page (code, stdout, stderr as text), or why it can't be one."""
    if code not in EXIT_CODES:
        return None, "exit %d, a code the binary has no Exit for" % code
    o, e = utf8_text(out), utf8_text(err)
    if o is None or e is None:
        return None, "not UTF-8 text with newlines and tabs only"
    return (code, o, e), None

# The forms whose page goes through the permission cards filter
# (_perm_lines): each is captured twice, the page while cards are off and
# the page while they are on. Off: A (the owner, so no probe), B (a
# passport; the probe's curl is a stub that fails), C (the cell socket
# says off) and T (TCP says off). On: Con (the cell socket) and Ton (TCP).
# Within each, the output and the trace outside the probe's call are the
# same in all, the probe is called once and the stub sees one request per
# run where it answers (C, Con, T, Ton: GET /cli/permission/enabled with
# the form's passport), and the steps pass rules 2 and 3 with the
# filter's own `read -r line` allowed. The probe is
# _permission_cli_enabled, the one the binary's crates/k5-cli/src/cards.rs
# is proved against: inside its call only steps on lines of its own
# functions. The pipeline's two sides run at once (`cat <<'EOF' |
# _perm_lines`), so the cat's step is put back before the filter's call
# wherever bash wrote it.
CARDS_OFF_KINDS = ("A", "B", "C", "T")
CARDS_ON_KINDS = ("Con", "Ton")
CARDS_ASK_KINDS = ("C", "Con", "T", "Ton")
PERM_FILTER = "_perm_lines"
perm_def = body_ranges(PERM_FILTER)
if len(perm_def) != 1:
    sys.exit("FAIL: %s is defined %d times" % (PERM_FILTER, len(perm_def)))
perm_body = set(range(perm_def[0][0], perm_def[0][1] + 1))
if perm_body & probe_lines:
    sys.exit("FAIL: a line of %s is a line of the cards probe's functions" % PERM_FILTER)
perm_read = {n for n in perm_body if k2_lines[n - 1].strip() == 'while IFS= read -r line || [ -n "$line" ]; do'}
if len(perm_read) != 1:
    sys.exit("FAIL: %d read loops in %s, this test knows one" % (len(perm_read), PERM_FILTER))
perm_calls = [n for n in perm_body if k2_lines[n - 1].strip() == "if %s; then on=1; fi" % CARDS_PROBE]
if len(perm_calls) != 1:
    sys.exit("FAIL: %s asks the probe on %d lines, this test knows one" % (PERM_FILTER, len(perm_calls)))
PIPE = re.compile(r"^\s*cat <<'EOF' \| %s( >&2)?$" % PERM_FILTER)
pipe_lines = {i + 1 for i, l in enumerate(k2_lines) if PIPE.match(l)}
if len(pipe_lines) < 2:
    sys.exit("FAIL: only %d pages through %s" % (len(pipe_lines), PERM_FILTER))

def cards_split(steps):
    """The steps outside the cards probe's calls, each pipeline's cat put
    back before its filter, and the number of calls."""
    cats, rest = {}, []
    for l in steps:
        m = STEP.match(l)
        if m and int(m.group(2)) in pipe_lines and m.group(3) == "cat":
            cats.setdefault(int(m.group(2)), []).append(l)
        else:
            rest.append(l)
    outside, calls, in_probe = [], 0, False
    for l in rest:
        m = STEP.match(l)
        if in_probe:
            if not m or int(m.group(2)) in probe_lines:
                continue
            in_probe = False
        if m and int(m.group(2)) in probe_lines:
            return None, "a step on a line of the cards probe outside its call: %r" % l
        outside.append(l)
        if m and m.group(3) == CARDS_PROBE:
            if int(m.group(2)) not in perm_calls:
                return None, "the cards probe called outside the filter: %r" % l
            calls += 1
            in_probe = True
    out = []
    for l in outside:
        m = STEP.match(l)
        if m and int(m.group(2)) in pipe_lines and m.group(3) == PERM_FILTER:
            got = cats.get(int(m.group(2)))
            if not got:
                return None, "the filter's pipeline with no cat: %r" % l
            out.append(got.pop(0))
        out.append(l)
    if any(cats.values()):
        return None, "a cat of the filter's pipeline with no filter after it"
    return (out, calls), None

def cards_capture(argv, first):
    """The two pages of a form through the cards filter, or why not."""
    got = dict(first)
    for kind in ("T", "Ton"):
        r = run_traced(argv, kind)
        if r is None:
            return None, "the traced run differs from the untraced run"
        code, out, err, trace = r
        w, why = window(trace, kind)
        if w is None:
            return None, why
        got[kind] = (code, out, err, w)
    pages = []
    for on, kinds in ((False, CARDS_OFF_KINDS), (True, CARDS_ON_KINDS)):
        state = "on" if on else "off"
        outs, wins = [], []
        for kind in kinds:
            code, out, err, w = got[kind]
            split, why = cards_split(w)
            if why:
                return None, "with cards, %s (in %s)" % (why, kind)
            outside, calls = split
            if calls != 1:
                return None, "with cards, the probe called %d times: in %s" % (calls, kind)
            outs.append((code, out, err))
            wins.append(outside)
        if any(o != outs[0] for o in outs):
            return None, "with cards %s, the output differs between the environments: %s" % (state, groups(kinds, outs))
        if any(w != wins[0] for w in wins):
            return None, "with cards %s, the trace differs between the environments: %s" % (
                state, first_difference(kinds, wins))
        why = judge_captured(wins[0], read_ok=perm_read)
        if why:
            return None, "with cards %s, %s" % (state, why)
        if XTRACEFD and run_plain(argv, kinds[0]) != outs[0]:
            return None, "with cards %s, the traced run differs from the untraced run" % state
        page, why = page_of(*outs[0])
        if why:
            return None, "with cards %s, %s" % (state, why)
        pages.append(page)
    return pages, None

# The forms that print chat's usage (_chat_usage): its manage piece only
# while _CHAT_HELP_MANAGE is unset, empty or 1. Each is captured twice,
# unset and set to 0, the same in A, B, C and Con each way, with that one
# read allowed on _chat_usage's lines; untraced in A, empty and 1 print
# the first page and `yes` the second.
CHAT_MANAGE = "_CHAT_HELP_MANAGE"
chat_def = body_ranges("_chat_usage")
if len(chat_def) != 1:
    sys.exit("FAIL: _chat_usage is defined %d times" % len(chat_def))
chat_body = set(range(chat_def[0][0], chat_def[0][1] + 1))
if sum(1 for n in chat_body if CHAT_MANAGE in k2_lines[n - 1]) != 1:
    sys.exit("FAIL: _chat_usage reads %s on other than one line" % CHAT_MANAGE)
if sum(l.count(CHAT_MANAGE) for l in k2_lines) != 1:
    sys.exit("FAIL: the script reads %s outside _chat_usage" % CHAT_MANAGE)
MANAGE_ALLOW = {CHAT_MANAGE: chat_body}

def manage_capture(argv, first_page):
    """The two pages of a form that prints chat's usage, or why not."""
    outs, wins = [], []
    kinds = ("A", "B", "C", "Con")
    for kind in kinds:
        r = run_traced(argv, kind, {CHAT_MANAGE: "0"})
        if r is None:
            return None, "the traced run differs from the untraced run"
        code, out, err, trace = r
        w, why = window(trace, kind)
        if w is None:
            return None, why
        outs.append((code, out, err))
        wins.append(w)
    if any(o != outs[0] for o in outs):
        return None, "with %s=0, the output differs between the environments: %s" % (CHAT_MANAGE, groups(kinds, outs))
    if any(w != wins[0] for w in wins):
        return None, "with %s=0, the trace differs between the environments: %s" % (
            CHAT_MANAGE, first_difference(kinds, wins))
    why = judge_captured(wins[0], allow_env=MANAGE_ALLOW)
    if why:
        return None, "with %s=0, %s" % (CHAT_MANAGE, why)
    page, why = page_of(*outs[0])
    if why:
        return None, "with %s=0, %s" % (CHAT_MANAGE, why)
    for val, want in (("0", outs[0]), ("", first_page), ("1", first_page), ("yes", outs[0])):
        o = run_plain(argv, "A", {CHAT_MANAGE: val})
        w = want if val in ("0", "yes") else (want[0], want[1].encode("utf-8"), want[2].encode("utf-8"))
        if o != w:
            return None, "with %s=%r, the untraced run is not the page for that value" % (CHAT_MANAGE, val)
    return page, None

def capture(argv):
    """(argv, the row's kind and pages, why not)."""
    first, outs, wins = {}, [], []
    kinds = ("A", "B", "C", "Con")
    for kind in kinds:
        r = run_traced(argv, kind)
        if r is None:
            return argv, None, "the traced run differs from the untraced run"
        code, out, err, trace = r
        w, why = window(trace, kind)
        if w is None:
            return argv, None, why
        first[kind] = (code, out, err, w)
        outs.append((code, out, err))
        wins.append(w)
    if any(STEP.match(l) and STEP.match(l).group(3) == CARDS_PROBE for w in wins for l in w):
        pages, why = cards_capture(argv, first)
        return argv, (("C", pages[0], pages[1]) if pages else None), why
    if any(o != outs[0] for o in outs):
        return argv, None, "the output differs between the environments: %s" % groups(kinds, outs)
    if any(w != wins[0] for w in wins):
        return argv, None, "the trace differs between the environments: %s" % first_difference(kinds, wins)
    if XTRACEFD and run_plain(argv, "A") != outs[0]:
        return argv, None, "the traced run differs from the untraced run"
    why = judge_captured(wins[0])
    manage = bool(why) and judge_captured(wins[0], allow_env=MANAGE_ALLOW) is None
    if why and not manage:
        return argv, None, why
    page, why = page_of(*outs[0])
    if why:
        return argv, None, why
    if manage:
        other, why = manage_capture(argv, page)
        return argv, (("M", page, other) if other else None), why
    return argv, ("F", page), None

CLI_WORD = re.compile(r"(?<![A-Za-z0-9_./~|<:-])k2(?= )")
PRODUCT_WORD = re.compile(r"(?<![A-Za-z0-9_./~|<:-])K2(?![A-Za-z0-9_/~|<-])(?![.:][A-Za-z0-9_/])")
def escaped(s):
    """The text as the list holds it: \\ \\n \\t escaped, \\c for the
    command's name and \\p for the product's."""
    if "\x01" in s or "\x02" in s:
        sys.exit("FAIL: a help text with a byte this list uses: %r" % s[:80])
    s = CLI_WORD.sub("\x01", s)
    s = PRODUCT_WORD.sub("\x02", s)
    s = s.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t")
    return s.replace("\x01", "\\c").replace("\x02", "\\p")

seen_before = len(requests_seen)
captured, ckept = {}, {}
with concurrent.futures.ThreadPoolExecutor(max_workers=int(os.environ.get("HELP_NATIVE_JOBS", "4"))) as pool:
    for argv, got, why in pool.map(capture, ccands):
        if got:
            captured[tuple(argv)] = got
        else:
            ckept[" ".join(argv)] = why
# A form that asked is never captured (its curl is a step that is not
# plain) but where the cards answer decides its page; this proves it from
# the stub's side: no request names another captured form's passport, and
# each run of a cards form where a stub answers sent it exactly one
# request, the probe's.
asked = {}
for kind, r in requests_seen[seen_before:]:
    route = r.split("\n", 1)[0].split("?", 1)[0]
    for no in {int(m.group(1)) for m in re.finditer(r"sessQ\.form([0-9]+)\b", r)}:
        asked.setdefault(tuple(ccands[no]), {}).setdefault(kind, []).append(route)
bad_asks = []
for argv, got in sorted(captured.items()):
    seen = asked.get(argv, {})
    if got[0] != "C":
        if seen:
            bad_asks.append("%s asked %r" % (" ".join(argv), seen))
        continue
    want = {k: ["/cli/permission/enabled"] * runs_by.get((argv, k), 0) for k in CARDS_ASK_KINDS}
    if seen != want or not all(want.values()):
        bad_asks.append("%s asked %r, want %r" % (" ".join(argv), seen, want))
if bad_asks:
    sys.exit("FAIL: captured forms asked the stub daemon other than the cards probe once a run: %s"
             % "; ".join(bad_asks[:20]))
n_cards = sum(1 for g in captured.values() if g[0] == "C")
n_manage = sum(1 for g in captured.values() if g[0] == "M")
print("help_native: %d other help forms asked the stub daemon; %d captured by the cards answer asked "
      "once a run, the probe, and no other captured form asked" % (len(asked), n_cards))
pages_, page_no, form_rows = [], {}, []
def page_number(page):
    code, o, e = page
    key = (code, escaped(o), escaped(e))
    if key not in page_no:
        page_no[key] = len(pages_)
        pages_.append(key)
    return page_no[key]
for argv in sorted(captured):
    got = captured[argv]
    form_rows.append("\t".join([got[0], " ".join(argv)] + ["%d" % page_number(p) for p in got[1:]]))
CHEAD = """# The other help forms the k5 binary answers itself: what bash printed for
# each, proved the same in every environment from bash's own trace. Written
# by tests/cli/help_native.sh --write; do not edit by hand.
# P<tab><page><tab><exit code><tab><stdout><tab><stderr>: the text with
#   \\\\, \\n and \\t escaped, \\c for the command's name and \\p for the product's.
# F<tab><argv, words between single spaces><tab><page>
# C<tab><argv><tab><page while permission cards are off><tab><page while on>:
#   a page through the cards filter; the binary asks as bash asks.
# M<tab><argv><tab><page><tab><page while _CHAT_HELP_MANAGE is set, not empty, not 1>
"""
ctext = CHEAD + "".join("P\t%d\t%d\t%s\t%s\n" % (i, k[0], k[1], k[2]) for i, k in enumerate(pages_)) \
    + "".join(r + "\n" for r in form_rows)
print("help_native: %d other help forms tried, %d captured (%d by the cards answer, %d by %s; %d pages), "
      "%d stay bash" % (len(ccands), len(captured), n_cards, n_manage, CHAT_MANAGE, len(pages_), len(ckept)))
creasons = {}
for why in ckept.values():
    key = re.sub(r": .*", "", why)
    creasons[key] = creasons.get(key, 0) + 1
for k in sorted(creasons):
    print("  bash: %d x %s" % (creasons[k], k))
if os.environ.get("HELP_NATIVE_WHY"):
    for a in sorted(ckept):
        print("  bash: %s: %s" % (a, ckept[a]))
dump = os.environ.get("HELP_NATIVE_DUMP")
if dump:
    with open(dump, "w") as f:
        json.dump({"kept": ckept, "captured": sorted(" ".join(a) for a in captured)}, f, indent=0, sort_keys=True)

if write == "1":
    with open(list_path, "w") as f:
        f.write(text)
    with open(CAPTURED, "w") as f:
        f.write(ctext)
    print("help_native: wrote %s and %s" % (list_path, CAPTURED))
    sys.exit(0)
bad = False
import difflib
for path, want in ((list_path, text), (CAPTURED, ctext)):
    have = open(path).read() if os.path.exists(path) else ""
    if have != want:
        sys.stderr.write("".join(list(difflib.unified_diff(have.splitlines(True), want.splitlines(True),
                                                           "committed", "derived"))[:200]))
        print("FAIL: %s is not what bash proves (run tests/cli/help_native.sh --write)" % path)
        bad = True
if bad:
    sys.exit(1)
print("help_native: PASS: the committed lists are what bash proves")
PY
