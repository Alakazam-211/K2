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
#   fallthrough    k5 heartbeat list (a verb the      <= cli/k2's p95 for
#                  shell answers)                        the same + 5 ms
#   thread read    k5 thread read                     <= 40 ms: pending
#                                                        (R3.5, not timed)
#
# Then, for the fallthrough, info lines with no target: cli/k2, the shell
# run as the front hands it over but started straight, and the front, and
# the front's own phase times (K5_CLI_TRACE_TIMING), with a passport and
# without one (where the passport rule walks the process ancestry).
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
mkdir -p "$WORK/bin" "$WORK/home/.k2" "$WORK/ws/.k2" "$WORK/tmp"
ln -s "$BIN" "$WORK/bin/k5"

echo "perf_cli: binary $BIN ($("$WORK/bin/k5" --version)), bash $BASH_CLI"
echo "perf_cli: host $(uname -sm), $(getconf _NPROCESSORS_ONLN 2>/dev/null || echo '?') cpus, load $(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null || uptime | sed 's/.*load averages*: //')"

python3 -I - "$WORK" "$BIN" "$BASH_CLI" <<'PY'
import json, math, os, statistics, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

work, binary, bash_cli = sys.argv[1], sys.argv[2], sys.argv[3]
RUNS, WARM = 50, 3
WHOAMI = json.dumps({"workspace": "acme", "role": "canonical", "address": "acme", "primary": "acme",
                     "session": "s1", "path": work + "/ws", "siblings": {"live": 1}}).encode()
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
        body = WHOAMI if path == "/cli/whoami" else b"[]" if path == "/cli/heartbeat/list" else b'{"ok":true}'
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
handler_ns.clear()
w = timed([k5, "whoami"])
stub_p95 = p95([n / 1e6 for n in handler_ns]) if handler_ns else 0.0
got = p95(w)
target = 30.0 + stub_p95
results.append(got <= target)
print(f"perf_cli: {'pass' if got <= target else 'FAIL'}: k5 whoami: p95 {got:.2f} ms, target <= {target:.2f} ms "
      f"(30 ms + the stub's p95 handler time {stub_p95:.3f} ms over {len(handler_ns)} requests); "
      f"median {statistics.median(w):.2f} ms, min {min(w):.2f}, max {max(w):.2f}")
bash_ft = p95(timed(bash_argv("heartbeat", "list")))
print(f"perf_cli: info: cli/k2 heartbeat list: p95 {bash_ft:.2f} ms (the fallthrough's base)")
measure("k5 heartbeat list (fallthrough)", [k5, "heartbeat", "list"], bash_ft + 5.0, "cli/k2's p95 + 5 ms")
bash_who = p95(timed(bash_argv("whoami")))
print(f"perf_cli: info: cli/k2 whoami: p95 {bash_who:.2f} ms (for comparison)")
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

phases("a passport (this script's call)", env)
owner = {k: v for k, v in env.items() if k not in ("K2_HOOK_TOKEN", "K2_CELL", "K2_SESSION_ID")}
phases("no passport and no session marker: the ancestry walk runs", owner)
srv.shutdown()
if not all(results):
    sys.exit(f"perf_cli: {results.count(False)} of {len(results)} measures missed their target")
print(f"perf_cli: {len(results)} measures met their targets")
PY
