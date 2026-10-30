#!/usr/bin/env python3
"""collect.py: turn job logs into run records (docs/evidence/runs/<job>.json).

Usage:
  collect.py --log-cmd 'CMD {job}' [--out DIR] JOB [JOB ...]
  collect.py --log FILE --job JOB [--out DIR]
  collect.py --print --log FILE --job JOB          (records to stdout, nothing written)

  --log-cmd   a shell command that prints one job's log; {job} is replaced by the job id
              (or set RUNREC_LOG_CMD). --out defaults to docs/evidence/runs.

Each run in a log becomes one record (schema in tools/evidence/README.md). A log line
`RUNREC {json}` (printed by the job's helpers) is taken as is. Older logs, which have no
RUNREC lines, are parsed:
  * TLC: the lines before `=== tlc NAME ...: exit RC, wall Ws` (or `=== live NAME ...`) are
    that run's TLC output; COMPLETE needs "depth of the complete state graph search" and
    "0 states left on queue"; exit 124 (stopped by the cap) gives cap_s = W;
  * `### NAME [... Ns cap]: <summary>` lines (a summary of a run whose output went to a
    file) are parsed the same way, with cap_s = N;
  * enumerator: `SUMMARY key=value ...` (states, levels, violations, complete) followed by
    `=== LABEL: exit RC, wall Ws`; the run is LABEL with spaces as underscores.
A name that repeats in one job gets `#2`, `#3`, ... in log order. Exit status: 0 on
success, 2 if a log cannot be read (no file is written for that job).
"""
import datetime
import json
import os
import re
import shlex
import subprocess
import sys

NUM = r"[0-9][0-9,]*"


def num(s):
    return None if s is None else int(s.replace(",", ""))


VIOL = re.compile(r"(?:Invariant|property) (\S+)(?: of module \S+)? (?:is|was|were) violated|properties were violated")


def ours(line, run):
    """a violation line belongs to RUN unless it names another instance's invariant"""
    if run is None:
        return True
    base = run.split("#")[0] + "_"
    mod = re.search(r"of module (\S+)", line)
    if mod:
        return mod.group(1).startswith(base)
    m = re.search(r"(?:Invariant|property) (\S+)", line)
    if not m or "_" not in m.group(1):
        return True  # wrapper-level names (Mono, SCatchesUp, ...)
    return m.group(1).startswith(base)


def tlc_record(lines, run=None, rc=None):
    """status and counts from a run's TLC output lines (the same rules as tlc_verdict);
    rc: TLC's exit (0 ok, 10-14 a violation, 124 the cap, anything else an error)"""
    viol = err = None
    trace = 0
    last = None
    depth = None
    generated = None
    for l in lines:
        if l.lstrip().startswith("trace "):
            continue  # a recipe's own trace summary of an earlier run
        mv = VIOL.search(l)
        if mv and not ours(l, run):
            continue  # another run's violation (a trace printed after it)
        if mv and viol is None:
            viol = mv.group(0)
        elif l.startswith("Error:") and "behavior up to this point" not in l and err is None:
            err = l.strip()
        if re.match(r"^State [0-9]+:", l):
            trace += 1
        if "distinct states found" in l:
            last = l
        m = re.search(r"depth of the complete state graph search is ([0-9]+)", l)
        if m:
            depth = int(m.group(1))
    states = queued = None
    progress = False
    if last:
        m = re.search(r"(" + NUM + r") distinct states found", last)
        states = num(m.group(1)) if m else None
        m = re.search(r"(" + NUM + r") states left on queue", last)
        queued = num(m.group(1)) if m else None
        m = re.search(r"(" + NUM + r") states generated", last)
        generated = num(m.group(1)) if m else None
        progress = last.lstrip().startswith("Progress")
    rec = {"states": states, "depth": None, "queued": queued, "generated": generated}
    if viol:
        # trace_states only from a full trace (helpers that print just "State 1:" truncate it)
        rec.update(status="VIOLATION", violation=viol, trace_states=trace if trace >= 2 else None, depth=depth)
    elif rc is not None and rc not in (0, 124) and not (10 <= rc <= 14):
        rec.update(status="ERROR", error=err or "TLC exit %d" % rc)
    elif err:
        rec.update(status="ERROR", error=err)
    elif depth is not None and queued == 0 and not progress:
        rec.update(status="COMPLETE", depth=depth)
    elif states is not None:
        rec.update(status="PARTIAL")
    else:
        rec.update(status="NO_RESULT")
    return rec


def parse_log(text, job):
    recs = []
    buf = []
    pending_summary = None
    for raw in text.splitlines():
        line = raw.rstrip("\n")
        s = line.strip()
        if s.startswith("RUNREC "):
            try:
                r = json.loads(s[len("RUNREC "):])
            except ValueError:
                continue
            r["job"] = job
            r["source"] = "RUNREC"
            recs.append(r)
            buf = []
            continue
        m = re.match(r"^SUMMARY (.*)$", s)
        if m:
            kv = dict(re.findall(r"(\w+)=(\S+)", m.group(1)))
            pending_summary = kv
            continue
        m = re.match(r"^=== (tlc|live) (\S+).*?: exit (-?[0-9]+), wall ([0-9]+)s", s)
        if m:
            r = {"job": job, "run": m.group(2), "tool": "tlc", "exit": int(m.group(3)),
                 "wall_s": int(m.group(4)), "cap_s": int(m.group(4)) if m.group(3) == "124" else None,
                 "source": "log"}
            r.update(tlc_record(buf, m.group(2), int(m.group(3))))
            if r["status"] == "NO_RESULT" and r["exit"] == 0:
                r["status"] = "UNKNOWN"  # TLC finished but its counts are not in the log
            recs.append(r)
            buf = []
            pending_summary = None
            continue
        m = re.match(r"^### (\S+) \[([^\]]*)\]: (.*)$", s)
        if m:
            parts = [p.strip() for p in m.group(3).split("|")]
            capm = re.search(r"([0-9]+)s cap", m.group(2))
            r = {"job": job, "run": m.group(1), "tool": "tlc", "cap_s": None, "source": "log"}
            prev = recs[-1] if recs and recs[-1].get("run") == r["run"] and recs[-1].get("source") == "log" else None
            r.update(tlc_record(parts, m.group(1), prev.get("exit") if prev else None))
            if "trace states:" in m.group(3):
                t = re.search(r"trace states: ([0-9]+)", m.group(3))
                r["trace_states"] = int(t.group(1))
            r["cap_s"] = int(capm.group(1)) if capm else None
            if prev is not None:
                # the `=== tlc` line before it had no TLC output (it went to a file or a grep)
                for k in ("exit", "wall_s"):
                    r[k] = prev.get(k)
                if r["cap_s"] is None and prev.get("exit") == 124:
                    r["cap_s"] = prev.get("wall_s")
                recs[-1] = r
            else:
                recs.append(r)
            buf = []
            continue
        m = re.match(r"^=== (.+?): exit (-?[0-9]+), wall ([0-9]+)s", s)
        if m:
            if pending_summary is not None:
                kv = pending_summary
                v = kv.get("violations", "none")
                complete = kv.get("complete") == "true"
                r = {"job": job, "run": re.sub(r"\s+", "_", m.group(1).strip()), "tool": "enumerator",
                     "states": num(kv.get("states")), "depth": num(kv.get("levels")),
                     "queued": None, "cap_s": None, "exit": int(m.group(2)), "wall_s": int(m.group(3)),
                     "mode": kv.get("mode"), "source": "log"}
                # "none", "" and an empty list ("[]") all mean no violation
                if v.strip() not in ("none", "", "[]", "0"):
                    vd = kv.get("violation_depth")
                    r.update(status="VIOLATION", violation=v,
                             depth=num(vd) if vd and vd != "-" else None)
                elif complete:
                    r["status"] = "COMPLETE"
                else:
                    r["status"] = "PARTIAL"
                recs.append(r)
            pending_summary = None
            buf = []
            continue
        m = re.match(r"^trace \S+: ([0-9]+) states", s)
        if m and recs:
            # a recipe's per-run trace length (one TLC worker: the shortest trace); it follows
            # its run, or names the run's instance in the invariant / module it quotes
            n = re.search(r"of module (\S+)|Invariant (\S+)", s)
            name = (n.group(1) or n.group(2)) if n else None
            target = recs[-1]
            if name:
                for r in reversed(recs):
                    if name.startswith(r["run"] + "_") or name == r["run"]:
                        target = r
                        break
            target["trace_states"] = int(m.group(1))
            continue
        if s.startswith("==="):
            buf = []
            continue
        buf.append(line)
    # a log with RUNREC lines: those are the TLC records (the parsed ones would repeat them)
    if any(r.get("source") == "RUNREC" for r in recs):
        recs = [r for r in recs if not (r.get("source") == "log" and r.get("tool") == "tlc")]
    # unique names: #2, #3 in log order
    seen = {}
    for r in recs:
        n = r["run"]
        seen[n] = seen.get(n, 0) + 1
        if seen[n] > 1:
            r["run"] = "%s#%d" % (n, seen[n])
    for r in recs:
        for k in ("job", "run", "tool", "status", "states", "depth", "queued", "cap_s"):
            r.setdefault(k, None)
    return recs


def write(job, recs, out, how):
    os.makedirs(out, exist_ok=True)
    doc = {"job": job, "collected": datetime.date.today().isoformat(), "from": how, "runs": recs}
    p = os.path.join(out, job + ".json")
    with open(p, "w") as f:
        json.dump(doc, f, indent=1, sort_keys=False)
        f.write("\n")
    return p


def main(argv):
    args = list(argv)
    cmd = os.environ.get("RUNREC_LOG_CMD")
    out = "docs/evidence/runs"
    log = job = None
    to_stdout = False
    jobs = []
    while args:
        a = args.pop(0)
        if a == "--log-cmd":
            cmd = args.pop(0)
        elif a == "--out":
            out = args.pop(0)
        elif a == "--log":
            log = args.pop(0)
        elif a == "--job":
            job = args.pop(0)
        elif a == "--print":
            to_stdout = True
        elif a in ("-h", "--help"):
            print(__doc__)
            return 0
        else:
            jobs.append(a)
    rc = 0
    if log:
        if not job:
            print("collect.py: --log needs --job", file=sys.stderr)
            return 2
        recs = parse_log(open(log, errors="replace").read(), job)
        if to_stdout:
            json.dump(recs, sys.stdout, indent=1)
            print()
        else:
            print("%s: %d runs -> %s" % (job, len(recs), write(job, recs, out, "file " + os.path.basename(log))))
        return 0
    if not jobs or not cmd:
        print(__doc__)
        return 2
    for j in jobs:
        c = cmd.replace("{job}", shlex.quote(j))
        p = subprocess.run(c, shell=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace")
        if p.returncode != 0 or not p.stdout.strip():
            print("collect.py: %s: log not readable (%s)" % (j, (p.stderr or "empty").strip()[:200]), file=sys.stderr)
            rc = 2
            continue
        recs = parse_log(p.stdout, j)
        print("%s: %d runs -> %s" % (j, len(recs), write(j, recs, out, "log command")))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
