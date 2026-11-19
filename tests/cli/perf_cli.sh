#!/usr/bin/env bash
# perf_cli.sh: the k5 CLI's speed targets (PRD 7.5), on a built k5-cli.
# bash-only: it times the binary given as its argument against the bash cli/k2 itself, so it never runs through K5_CLI_UNDER_TEST and cli-parity.sh skips it.
#
#   bash tests/cli/perf_cli.sh <path to a built k5-cli>
#
# Each measure is the p95 of 50 timed runs (after 3 untimed ones), wall
# time from spawn to exit, against a stub daemon that answers at once (a
# Python http.server; its own time per request is measured and printed):
#
#   --version      k5 --version                       <= 5 ms
#   whoami         k5 whoami (an agent's: passport,   <= 30 ms + the stub's
#                  project, session names)               p95 handler time
#   done           k5 done (the agent's name set)     <= 30 ms + the stub's
#                                                        p95 handler time
#   respond        k5 respond <line> (a host          <= 30 ms + the stub's
#                  session: TCP, passport as Bearer)     p95 handler time
#   inbox          k5 inbox (two items, formatted     <= 30 ms + the stub's
#                  as the shell's Python does)           p95 handler time
#   read           k5 read <workspace> (a terminal's  <= 30 ms + the stub's
#                  last lines)                           p95 handler time
#   activity       k5 activity (three events, local   <= 30 ms + the stub's
#                  times)                                p95 handler time
#   sessions       k5 sessions (the roster, three     <= 30 ms + the stub's
#                  rows)                                 p95 handler time
#   connections    k5 connections list (this          <= 30 ms + the stub's
#                  workspace's rows, three peers)        p95 handler time
#   feedback list  k5 feedback list (three tickets)   <= 30 ms + the stub's
#                                                        p95 handler time
#   feedback show  k5 feedback show <id> (a brief,    <= 30 ms + the stub's
#                  a thread with a file)                 p95 handler time
#   update --cli   k5 update --cli (the daemon's      <= 30 ms + the stub's
#                  version from /boot-status)            p95 handler time
#   setup          k5 setup (the setup list the       <= 30 ms + the stub's
#                  daemon draws, five lines)             p95 handler time
#   connect status k5 connect status (no pairing on   <= 30 ms + the stub's
#                  disk: the daemon's token, then the    p95 handler time
#                  tunnel; two requests)
#   fallthrough    k5 heartbeat list (a verb the      median <= cli/k2's
#                  shell answers)                        median for the same
#                                                        + 5 ms (see below)
#   agent help     k5 mail send --help in a K2        <= 30 ms
#                  terminal (the passport, a live
#                  cell socket: the session checks
#                  pass, nothing is sent)
#   study list     k5 study with a passport (the      <= 30 ms + the stub's
#   (passport)     cards probe over TCP, cards on)       p95 handler time
#   --help         k5 --help in an agent's terminal   <= 30 ms + the stub's
#   (passport)     (the cards probe over TCP, cards      p95 handler time
#                  on: the daily tier and its line)
#   thread read    k5 thread read                     <= 40 ms: pending
#                                                        (R3.5, not timed)
#
# and the pages the binary prints itself, in an owner's terminal instead:
# a port and an owner token in a second K2 home, no K2_* set, run from the
# same workspace folder (.k2/), so the binary does every check of the
# shell's prelude it makes for a page (the rename heads-up's git call
# included). No daemon: pages never ask one.
#
#   page --version k5 --version                       <= 5 ms
#   study errors   k5 study errors                    <= 30 ms
#   help page      k5 mail send --help                <= 30 ms
#   bare k5        k5 alone (the daily tier, no       <= 30 ms
#                  passport: nothing asked)
#   unknown        k5 nope (Unknown command, then     <= 30 ms
#                  the daily tier, exit 1)
#
# with cli/k2's p95 for the two pages as info. Before timing, each page
# runs once on both CLIs and the two must print the same and exit the
# same; and once on the binary with its hand-over to bash broken
# (K5_HOME_OVERRIDE relative), so the time is the binary's own answer.
#
# Then, for the fallthrough, info lines with no target: cli/k2, the shell
# run as the front hands it over but started straight, and the front, and
# the front's own phase times (K5_CLI_TRACE_TIMING), with a passport and
# without one (where the passport rule walks the process ancestry).
#
# The fallthrough row compares MEDIANS, not p95s. Both sides are mostly
# the shell's own run (about 185 ms on a quiet Linux node), and the p95 of
# 50 runs of that is one of its three slowest runs: it moves by several
# ms from one batch to the next with nothing changed, so a p95 against a
# p95 + 5 ms failed now and then on noise alone. The median of 50 moves
# well under a millisecond. The front's own share of a hand-over is about
# 2.6 ms (its phase times below: spawn to main plus total, with a
# passport), so k5's median sits about 3 ms over cli/k2's; the fixed
# allowance of 5 ms on top of cli/k2's median keeps the PRD's "today + 5
# ms" and still catches a front that grows by about 2 ms or more. Both
# p95s are printed as info. The native rows keep their absolute p95
# targets: they sit far under them.
#
# Before timing, each measure's command runs once on both CLIs where both
# answer it, and the two must print the same and exit the same: a fast
# wrong answer is no pass. A measure that misses its target fails the
# script, naming the measure and both numbers.
#
# NOT a builder-loop check (not in scripts/ci/checks.sh): run it once per
# family flip, on a quiet Linux node, with a release build, for example:
#   k2 compute run --sha <full sha> --os linux --cpus 8 --mem 16G \
#     --needs rust@1.99.1,cmake@3 -- bash -c 'set -o pipefail;
#     cargo build --release --locked -p k5-cli --bin k5-cli &&
#     bash tests/cli/perf_cli.sh "$CARGO_TARGET_DIR/release/k5-cli"'
# Timing on a loaded machine says little; the script prints the load.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
BASH_CLI="$CLI_BASH_SOURCE"

[ $# -eq 1 ] || { echo "usage: bash tests/cli/perf_cli.sh <path to a built k5-cli>" >&2; exit 2; }
BIN="$1"
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
[ -f "$BIN" ] && [ -x "$BIN" ] || { echo "perf_cli: not an executable file: $BIN" >&2; exit 2; }
command -v python3 >/dev/null || { echo "perf_cli: python3 is needed" >&2; exit 2; }

WORK="$(mktemp -d -t k5-perf-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/home/.k2" "$WORK/page-home/.k2" "$WORK/ws/.k2" "$WORK/tmp"
ln -s "$BIN" "$WORK/bin/k5"
# The pages' owner terminal: a port and an owner token on disk, in a home
# of its own, so the agent measures above see no owner token.
printf '1\n' > "$WORK/page-home/.k2/heartbeat.port"
printf 'perf-owner-token\n' > "$WORK/page-home/.k2/heartbeat.token"

echo "perf_cli: binary $BIN ($("$WORK/bin/k5" --version)), bash $BASH_CLI"
echo "perf_cli: host $(uname -sm), $(getconf _NPROCESSORS_ONLN 2>/dev/null || echo '?') cpus, load $(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null || uptime | sed 's/.*load averages*: //')"

python3 -I - "$WORK" "$BIN" "$BASH_CLI" <<'PY'
import json, math, os, statistics, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

work, binary, bash_cli = sys.argv[1], sys.argv[2], sys.argv[3]
RUNS, WARM = 50, 3
WHOAMI = json.dumps({"workspace": "acme", "role": "canonical", "address": "acme", "primary": "acme",
                     "session": "s1", "path": work + "/ws", "siblings": {"live": 1}}).encode()
INBOX = json.dumps([{"id": "i1", "folder": "projects", "priority": "high", "title": "Perf item", "summary": "s"},
                    {"id": "i2", "filename": "notes.md", "priority": "low"}]).encode()
LINES = json.dumps({"lines": ["$ cargo test", "test result: ok. 3 passed"]}).encode()
FEED = json.dumps({"feed": [
    {"at": 1763400000, "agent": "acme", "type": "msg", "from": "acme", "to": "beta", "summary": "hello"},
    {"at": 1763400061, "agent": "beta", "type": "done", "summary": "shipped"},
    {"at": 1763400122, "agent": "perf", "type": "inbox"}]}).encode()
ROSTER = json.dumps({"workspace": {"handle": "acme"}, "sidecars": {"cap": 4, "live": 1}, "sessions": [
    {"address": "acme", "role": "main", "cli": "claude", "state": "working", "title": "Perf", "you": True},
    {"address": "acme/h", "role": "sidecar", "cli": "codex", "state": "idle", "title": "Helper"},
    {"address": "acme/z", "state": "asleep"}]}).encode()
CONNECTIONS = json.dumps({"workspace": {"name": "acme", "path": work + "/ws"},
    "thisWorkspace": {"handle": "acme", "sessions": [{"address": "acme", "role": "main", "cli": "claude", "state": "working", "you": True}]},
    "connections": [{"projectName": "beta"}, {"projectName": "gamma", "reachable": False},
                    {"remote": True, "agent": "delta", "host": "box.example"}]}).encode()
TICKETS = json.dumps({"items": [
    {"id": "fb_000001aa", "status": "waiting", "priority": 1, "kind": "question", "title": "Ship it?"},
    {"id": "fb_000002bb", "status": "answered", "priority": 2, "kind": "decision", "title": "Pick a name"},
    {"id": "fb_000003cc", "status": "planned", "priority": 3, "kind": "note", "title": "Later"}]}).encode()
TICKET = json.dumps({"id": "fb_000001aa", "status": "waiting", "title": "Ship it?", "kind": "question",
    "priority": 1, "agentName": "perf", "sessionId": "s1", "createdAt": 1763400000, "assignees": ["owner"],
    "options": ["yes", "no"], "body": "Ready to ship.\nAll green.", "hasBrief": True, "briefBytes": 2048,
    "brief": {"text": "Problem\nThe release is ready."}, "projectPath": work + "/ws",
    "comments": [{"author": "perf", "at": 1763400061, "body": "See the log.",
                  "attachments": [{"path": "out/log.txt", "size": 4096}]}]}).encode()
SETUP = (b"Setup: 1 of 3 done\n  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)\n"
         b"  [x] Sign in to an AI tool\n  [-] Link your subdomain\nNext: meet Kessel, the agent that runs K2: k2 setup kessel\n")
BODIES = {"/cli/permission/enabled": b'{"enabled":true}', "/cli/whoami": WHOAMI, "/cli/heartbeat/list": b"[]", "/cli/inbox/list": INBOX, "/cli/terminal/read": LINES,
          "/cli/feed": FEED, "/cli/sessions/roster": ROSTER, "/cli/connections": CONNECTIONS,
          "/cli/feedback/list": TICKETS, "/cli/feedback/show": TICKET,
          "/boot-status": b'{"phase":"ready","version":"0.0.0"}',
          "/cli/first-run/status": SETUP,
          "/cli/tunnel/config": b'{"tokenSet":true}',
          "/cli/tunnel/status": b'{"running":true,"public_url":"https://acme.k2.dev"}'}
handler_ns = []
lock = threading.Lock()

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_GET(self):
        t0 = time.perf_counter_ns()
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        body = BODIES.get(path, b'{"ok":true}')
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True
        with lock:
            handler_ns.append(time.perf_counter_ns() - t0)
    do_POST = do_GET
    def log_message(self, *a):
        pass

srv = ThreadingHTTPServer(("127.0.0.1", 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
port = str(srv.server_address[1])

# An agent's call: its passport, its workspace, its session's names; no
# owner token on disk, no K2_* from this shell.
env = {
    "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    "HOME": work + "/home",
    "TMPDIR": work + "/tmp",
    "K2_PORT": port,
    "K2_HOOK_TOKEN": "sessP.perfsecret1",
    "K2_PROJECT_PATH": work + "/ws",
    "K2_SESSION_ID": "s1",
    "K2_CELL": "canonical",
    "K2_AGENT_NAME": "perf",
}
k5 = work + "/bin/k5"

def run_once(argv):
    p = subprocess.run(argv, env=env, cwd=work + "/ws", stdin=subprocess.DEVNULL, capture_output=True)
    return p.returncode, p.stdout, p.stderr

def timed(argv):
    """Wall ms of each timed run: spawn to exit, output to /dev/null."""
    devnull = os.open(os.devnull, os.O_WRONLY)
    fa = [(os.POSIX_SPAWN_DUP2, devnull, 1), (os.POSIX_SPAWN_DUP2, devnull, 2)]
    out = []
    try:
        for i in range(WARM + RUNS):
            t0 = time.perf_counter_ns()
            pid = os.posix_spawn(argv[0], argv, env, file_actions=fa)
            _, status = os.waitpid(pid, 0)
            ms = (time.perf_counter_ns() - t0) / 1e6
            if os.waitstatus_to_exitcode(status) != expected[tuple(argv)]:
                sys.exit(f"perf_cli: FAIL: {argv} exited {os.waitstatus_to_exitcode(status)} on timed run {i}, "
                         f"{expected[tuple(argv)]} before")
            if i >= WARM:
                out.append(ms)
    finally:
        os.close(devnull)
    return out

def p95(xs):
    return sorted(xs)[math.ceil(0.95 * len(xs)) - 1]

expected = {}
def check_same(name, ours, theirs):
    a, b = run_once(ours), run_once(theirs)
    if a != b:
        sys.exit(f"perf_cli: FAIL: {name}: the two CLIs differ before timing\n  k5:   {a!r}\n  bash: {b!r}")
    expected[tuple(ours)] = a[0]
    expected[tuple(theirs)] = b[0]
    print(f"perf_cli: {name}: both exit {a[0]}, same output ({len(a[1])} bytes)")

# cli/k2 runs as bash runs it: through its #! line, from its path.
bash_argv = lambda *a: ["/bin/bash", bash_cli, *a]
os.chdir(work + "/ws")
# --version: bash wants a port and a token even for it; both are set.
check_same("--version", [k5, "--version"], bash_argv("--version"))
check_same("whoami", [k5, "whoami"], bash_argv("whoami"))
check_same("done", [k5, "done"], bash_argv("done"))
check_same("respond", [k5, "respond", "perf line"], bash_argv("respond", "perf line"))
check_same("inbox", [k5, "inbox"], bash_argv("inbox"))
check_same("read", [k5, "read", "acme"], bash_argv("read", "acme"))
check_same("activity", [k5, "activity"], bash_argv("activity"))
check_same("sessions", [k5, "sessions"], bash_argv("sessions"))
check_same("connections list", [k5, "connections", "list"], bash_argv("connections", "list"))
check_same("feedback list", [k5, "feedback", "list"], bash_argv("feedback", "list"))
check_same("feedback show", [k5, "feedback", "show", "fb_1"], bash_argv("feedback", "show", "fb_1"))
check_same("update --cli", [k5, "update", "--cli"], bash_argv("update", "--cli"))
check_same("setup", [k5, "setup"], bash_argv("setup"))
check_same("connect status", [k5, "connect", "status"], bash_argv("connect", "status"))
check_same("fallthrough (heartbeat list)", [k5, "heartbeat", "list"], bash_argv("heartbeat", "list"))

results = []
def measure(name, argv, target_ms, why):
    xs = timed(argv)
    got = p95(xs)
    ok = got <= target_ms
    results.append(ok)
    word = "pass" if ok else "FAIL"
    print(f"perf_cli: {word}: {name}: p95 {got:.2f} ms, target <= {target_ms:.2f} ms ({why}); "
          f"median {statistics.median(xs):.2f} ms, min {min(xs):.2f}, max {max(xs):.2f}")
    return got

measure("k5 --version", [k5, "--version"], 5.0, "PRD 7.5")

def measure_native(name, words):
    """A native family: <= 30 ms plus the stub's p95 handler time."""
    handler_ns.clear()
    w = timed([k5, *words])
    stub_p95 = p95([n / 1e6 for n in handler_ns]) if handler_ns else 0.0
    got = p95(w)
    target = 30.0 + stub_p95
    results.append(got <= target)
    print(f"perf_cli: {'pass' if got <= target else 'FAIL'}: k5 {name}: p95 {got:.2f} ms, target <= {target:.2f} ms "
          f"(30 ms + the stub's p95 handler time {stub_p95:.3f} ms over {len(handler_ns)} requests); "
          f"median {statistics.median(w):.2f} ms, min {min(w):.2f}, max {max(w):.2f}")

measure_native("whoami", ["whoami"])
measure_native("done", ["done"])
measure_native("respond", ["respond", "perf line"])
measure_native("inbox", ["inbox"])
measure_native("read", ["read", "acme"])
measure_native("activity", ["activity"])
measure_native("sessions", ["sessions"])
measure_native("connections list", ["connections", "list"])
measure_native("feedback list", ["feedback", "list"])
measure_native("feedback show", ["feedback", "show", "fb_1"])
measure_native("update --cli", ["update", "--cli"])
measure_native("setup", ["setup"])
measure_native("connect status", ["connect", "status"])
FALLTHROUGH_ALLOWANCE_MS = 5.0
def measure_fallthrough(words):
    """k5's median against cli/k2's median + a fixed allowance (see the top)."""
    theirs = timed(bash_argv(*words))
    ours = timed([k5, *words])
    base, got = statistics.median(theirs), statistics.median(ours)
    target = base + FALLTHROUGH_ALLOWANCE_MS
    ok = got <= target
    results.append(ok)
    name = " ".join(words)
    print(f"perf_cli: info: cli/k2 {name}: median {base:.2f} ms, p95 {p95(theirs):.2f} ms (the fallthrough's base)")
    print(f"perf_cli: {'pass' if ok else 'FAIL'}: k5 {name} (fallthrough): median {got:.2f} ms, target <= {target:.2f} ms "
          f"(cli/k2's median + {FALLTHROUGH_ALLOWANCE_MS:.0f} ms); p95 {p95(ours):.2f} ms (info), "
          f"min {min(ours):.2f}, max {max(ours):.2f}")

measure_fallthrough(["heartbeat", "list"])
for words in (["whoami"], ["done"], ["respond", "perf line"], ["inbox"], ["read", "acme"],
              ["activity"], ["sessions"], ["connections", "list"], ["feedback", "list"],
              ["feedback", "show", "fb_1"], ["update", "--cli"], ["setup"], ["connect", "status"]):
    bash_p95 = p95(timed(bash_argv(*words)))
    name = ' '.join(words[:2]) if words[0] in ('connections', 'feedback', 'update', 'connect') else words[0]
    print(f"perf_cli: info: cli/k2 {name}: p95 {bash_p95:.2f} ms (for comparison)")
print("perf_cli: pending: k5 thread read <= 40 ms (R3.5: thread read is not native yet)")

# Where the fallthrough's time goes. Info only, no target: the front's
# phases from its own timing (K5_CLI_TRACE_TIMING, crates/k5-cli/src/
# trace.rs), and the shell run exactly as the front hands it over
# (/dev/fd/3 on the cache file, the front's four variables) but started
# straight, so the front's cost and the shell's own are told apart.
import re, tempfile
ft = ["heartbeat", "list"]
caches = [os.path.join(d, n) for d, _, names in os.walk(work + "/home") for n in names
          if d.endswith("/run/cli") and n.startswith("k2-") and n.endswith(".sh")]
if len(caches) != 1:
    sys.exit(f"perf_cli: FAIL: want one cache file under {work}/home, found {caches}")
script_dir = os.path.dirname(os.path.realpath(binary))
handed = dict(env, K5_CLI_CALLED_AS="k5", K5_CLI_NOTICE_DONE="1", K5_CLI_VIA="rust", K5_CLI_SCRIPT_DIR=script_dir)

def spawn_runs(argv, run_env, extra=(), runs=RUNS, want=None, trace=False):
    """Wall ms of each run, and (with trace) each run's stderr text."""
    devnull = os.open(os.devnull, os.O_WRONLY)
    walls, errs = [], []
    try:
        for i in range(WARM + runs):
            err = tempfile.TemporaryFile(dir=work + "/tmp")
            fa = [(os.POSIX_SPAWN_DUP2, devnull, 1), (os.POSIX_SPAWN_DUP2, err.fileno() if trace else devnull, 2), *extra]
            t0 = time.perf_counter_ns()
            e = dict(run_env, K5_CLI_TRACE_TIMING=str(t0)) if trace else run_env
            pid = os.posix_spawn(argv[0], argv, e, file_actions=fa)
            _, status = os.waitpid(pid, 0)
            ms = (time.perf_counter_ns() - t0) / 1e6
            code = os.waitstatus_to_exitcode(status)
            if want is not None and code != want:
                sys.exit(f"perf_cli: FAIL: {argv} exited {code}, want {want}")
            err.seek(0)
            text = err.read().decode("utf-8", "replace")
            err.close()
            if i >= WARM:
                walls.append(ms)
                errs.append(text)
    finally:
        os.close(devnull)
    return walls, errs

ft_code = expected[tuple([k5, *ft])]
open3 = [(os.POSIX_SPAWN_OPEN, 3, caches[0], os.O_RDONLY, 0)]
a, _ = spawn_runs(["/bin/bash", bash_cli, *ft], env, want=ft_code)
b, _ = spawn_runs(["/bin/bash", "/dev/fd/3", *ft], handed, extra=open3, want=ft_code)
c, _ = spawn_runs([k5, *ft], env, want=ft_code)
for name, xs in [("cli/k2 as is", a), ("the shell as the front hands it over, started straight", b), ("k5 (the front, then the shell)", c)]:
    print(f"perf_cli: info: fallthrough: {name}: p95 {p95(xs):.2f} ms, median {statistics.median(xs):.2f} ms")

TIMING = re.compile(r"^k5 timing: (.+) ([0-9.]+) ms$")
def phases(label, run_env):
    walls, errs = spawn_runs([k5, *ft], run_env, runs=20, trace=True)
    rows = {}
    for text in errs:
        for line in text.splitlines():
            m = TIMING.match(line)
            if m:
                rows.setdefault(m.group(1), []).append(float(m.group(2)))
    if "total" not in rows:
        sys.exit(f"perf_cli: FAIL: no k5 timing lines with K5_CLI_TRACE_TIMING set:\n{errs[0]}")
    print(f"perf_cli: info: timing, {label} (median of {len(walls)} runs):")
    for name, xs in rows.items():
        print(f"perf_cli: info:   {name:<28} {statistics.median(xs):8.3f} ms")
    front = statistics.median(rows.get("spawn to main", [0.0])) + statistics.median(rows["total"])
    print(f"perf_cli: info:   {'wall':<28} {statistics.median(walls):8.3f} ms "
          f"(the front's part {front:.3f} ms, the shell's the rest)")

# An agent in a K2 terminal: its passport, the port and a live cell
# socket (a bound socket file: the session checks only test that it is
# one). Each page runs once on both CLIs, then once on the binary with
# its hand-over to bash broken, before timing.
import socket
cell_sock = work + "/cell.sock"
socket.socket(socket.AF_UNIX).bind(cell_sock)
cell_env = dict(env, K2_HOOK_SOCK=cell_sock)

def agent_page(name, words, run_env, target, with_stub):
    def once(argv, extra=None):
        p = subprocess.run(argv, env=dict(run_env, **(extra or {})), cwd=work + "/ws",
                           stdin=subprocess.DEVNULL, capture_output=True)
        return p.returncode, p.stdout, p.stderr
    a, b = once([k5, *words]), once(bash_argv(*words))
    if a != b:
        sys.exit(f"perf_cli: FAIL: {name}: the two CLIs differ before timing\n  k5:   {a!r}\n  bash: {b!r}")
    own = once([k5, *words], {"K5_HOME_OVERRIDE": "not/absolute"})
    if own != a:
        sys.exit(f"perf_cli: FAIL: {name}: not the binary's own answer (it went to bash): {own!r}")
    print(f"perf_cli: {name}: both exit {a[0]}, same output ({len(a[1])} bytes), the binary's own answer")
    handler_ns.clear()
    xs, _ = spawn_runs([k5, *words], run_env, want=a[0])
    stub_p95 = p95([n / 1e6 for n in handler_ns]) if (with_stub and handler_ns) else 0.0
    if with_stub and len(handler_ns) != WARM + RUNS:
        sys.exit(f"perf_cli: FAIL: {name}: {len(handler_ns)} requests in {WARM + RUNS} runs, want one each")
    got, goal = p95(xs), target + stub_p95
    results.append(got <= goal)
    extra = f" + the stub's p95 handler time {stub_p95:.3f} ms" if with_stub else ""
    print(f"perf_cli: {'pass' if got <= goal else 'FAIL'}: k5 {name}: p95 {got:.2f} ms, target <= {goal:.2f} ms "
          f"({target:.0f} ms{extra}); median {statistics.median(xs):.2f} ms, min {min(xs):.2f}, max {max(xs):.2f}")
    theirs, _ = spawn_runs(bash_argv(*words), run_env, want=a[0])
    print(f"perf_cli: info: cli/k2 {name}: p95 {p95(theirs):.2f} ms (for comparison)")

agent_page("agent help (mail send --help, HOOK_SOCK set)", ["mail", "send", "--help"], cell_env, 30.0, False)
agent_page("study list with a passport (cards probe)", ["study"], env, 30.0, True)
agent_page("--help with a passport (cards probe over TCP)", ["--help"], env, 30.0, True)

phases("a passport (this script's call)", env)
owner = {k: v for k, v in env.items() if k not in ("K2_HOOK_TOKEN", "K2_CELL", "K2_SESSION_ID")}
phases("no passport and no session marker: the ancestry walk runs", owner)
srv.shutdown()

# The pages the binary prints itself, in an owner's terminal: its own
# home with a port and an owner token, no K2_*, no daemon.
page_env = {
    "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    "HOME": work + "/page-home",
    "TMPDIR": work + "/tmp",
}

def page_once(argv, extra=None):
    p = subprocess.run(argv, env=dict(page_env, **(extra or {})), cwd=work + "/ws",
                       stdin=subprocess.DEVNULL, capture_output=True)
    return p.returncode, p.stdout, p.stderr

def page_timed(argv, want):
    walls, _ = spawn_runs(argv, page_env, want=want)
    return walls

def measure_page(name, words, target_ms):
    ours = [k5, *words]
    theirs = bash_argv(*words)
    if words != ["--version"]:
        a, b = page_once(ours), page_once(theirs)
        if a != b:
            sys.exit(f"perf_cli: FAIL: {name}: the two CLIs differ before timing\n  k5:   {a!r}\n  bash: {b!r}")
        own = page_once(ours, {"K5_HOME_OVERRIDE": "not/absolute"})
        if own != a:
            sys.exit(f"perf_cli: FAIL: {name}: not the binary's own answer (it went to bash): {own!r}")
        print(f"perf_cli: {name}: both exit {a[0]}, same output ({len(a[1])} bytes), the binary's own answer")
    code = page_once(ours)[0]
    xs = page_timed(ours, code)
    got = p95(xs)
    ok = got <= target_ms
    results.append(ok)
    print(f"perf_cli: {'pass' if ok else 'FAIL'}: k5 {name}: p95 {got:.2f} ms, target <= {target_ms:.2f} ms "
          f"(an owner's terminal, no daemon); "
          f"median {statistics.median(xs):.2f} ms, min {min(xs):.2f}, max {max(xs):.2f}")
    if words != ["--version"]:
        print(f"perf_cli: info: cli/k2 {name}: p95 {p95(page_timed(theirs, code)):.2f} ms (for comparison)")

measure_page("page --version", ["--version"], 5.0)
measure_page("study errors", ["study", "errors"], 30.0)
measure_page("help page (mail send --help)", ["mail", "send", "--help"], 30.0)
measure_page("bare k5 (the daily tier)", [], 30.0)
measure_page("unknown command (k5 nope)", ["nope"], 30.0)
if not all(results):
    sys.exit(f"perf_cli: {results.count(False)} of {len(results)} measures missed their target")
print(f"perf_cli: {len(results)} measures met their targets")
PY
