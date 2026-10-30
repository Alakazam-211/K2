#!/usr/bin/env python3
"""check.py: every count in the evidence notes is cited, and every citation matches its run.

Usage:
  check.py [--runs DIR] [FILE.md ...]
      defaults: --runs docs/evidence/runs, files docs/evidence/*.md

A citation is `[run JOB/RUN]` (or `[run JOB/RUN:FIELD]`) right after the number it backs:
  5,977 [run 3ce98eac/m_g_2x2]                          a COMPLETE run: exactly its `states`
  156,237 / 23 [run 069b15e6/x_2x1_r1]                  ... and `N / D`: D is its `depth`
  ≥ 61,233, partial (cap 180 s) [run 3ce98eac/m_g_4x1]  a PARTIAL run: this form, N = `states`,
                                                        and the cap text names its `cap_s` (in s,
                                                        min or h; within 5 s or 2 %); a run with
                                                        no `cap_s` takes any cap text
  15 [run c3c883d8/m1_client_no_epoch_fence]            a VIOLATION run: N = `trace_states`
                                                        (or `depth` when it has no trace length)
  12,558,899 [run a69e7297/mc_fm_2x3:generated]         any numeric field, exactly
Bold (`**`) around the number is fine. It fails when:
  * a cited run has no record (docs/evidence/runs/JOB.json, run RUN);
  * a COMPLETE run is cited with a number other than `states`, or as a lower bound;
  * a PARTIAL run is cited without the "≥ N, partial (cap ...)" form, or with N != `states`;
  * a run with any other status (ERROR, NO_RESULT, UNKNOWN) is cited at all;
  * a count-like number (1,000 or more: `12,345` or `12345`, not followed by a unit) sits in
    an evidence table cell with no citation. A cell marked `[estimate]`, `[derived]` or
    `[unverified: <reason>]` is exempt, as are columns headed Job / Commit.
  * a VIOLATION run backs a least depth but is not a strict BFS. A least-depth context is a
    section whose heading, a table whose header row, or a prose line that says "least depth"
    (or "least violating / failing depth", "shortest trace / counterexample"). There a
    VIOLATION citation (its trace length or depth) must point at a TLC run with `workers: 1`
    or an enumerator run with BFS levels (`depth` not null). TLC with several workers is not
    level-synchronous, so its first violation need not be a shortest one. A citation labelled
    `-workers auto` (in its cell, its column header, or in prose since the previous citation)
    is a comparison, not a least-depth claim, and is exempt.
Exit status: 0 clean, 1 findings, 2 usage or a broken record file. Python 3 stdlib only.
"""
import glob
import json
import os
import re
import sys

CITE = re.compile(r"\[run ([0-9A-Za-z]{4,40})/([^\]\s:]+)(?::([A-Za-z][A-Za-z0-9_]*))?\]")
VALUE = re.compile(
    r"(?P<ge>≥\s*)?(?P<n>\d{1,3}(?:,\d{3})+|\d+)"
    r"(?:\s*/\s*(?P<d>\d+))?"
    r"(?:\s+(?:distinct\s+)?states)?"
    r"(?P<partial>\s*,\s*partial\s*\((?P<cap>[^)]*)\))?\s*$")
COUNT = re.compile(r"(?<![\w.,/#^×·-])(\d{1,3}(?:,\d{3})+|\d{4,})(?![\w,]|\.\d|-\d)")
UNIT = re.compile(r"^\s*(?:s|ms|min|h|%|B|KB|MB|GB|KiB|MiB|GiB|G|M|k|/s|×|x\b)\b|^\s*(?:%|×|/s)")
EXEMPT = re.compile(r"\[(?:estimate|derived|unverified:[^\]]*)\]")
LEAST = re.compile(r"\bleast\s+(?:violating\s+|failing\s+)?depths?\b|\bshortest\s+(?:traces?|counterexamples?)\b", re.I)
AUTO = re.compile(r"workers\s+auto", re.I)
HEADING = re.compile(r"^(#{1,6})\s")


def load_runs(d):
    runs = {}
    for p in sorted(glob.glob(os.path.join(d, "*.json"))):
        try:
            doc = json.load(open(p))
        except ValueError as e:
            print("%s: not JSON: %s" % (p, e))
            sys.exit(2)
        job = doc.get("job") or os.path.basename(p)[:-5]
        runs[job] = {r["run"]: r for r in doc.get("runs", [])}
    return runs


def cap_matches(text, cap):
    """the cap text names the run's cap: a number with s / min / h, within 5 s or 2 %
    (a cap read back from a log is the wall time, which can run a second over)"""
    for n, unit in re.findall(r"(\d+(?:\.\d+)?)\s*(s|sec|min|h)\b", text.replace(",", "")):
        secs = float(n) * {"s": 1, "sec": 1, "min": 60, "h": 3600}[unit]
        if abs(secs - cap) <= max(5, 0.02 * cap):
            return True
    return False


def as_int(s):
    return int(s.replace(",", ""))


def check_citation(text_before, job, run, field, runs):
    """the problem with this citation, or None"""
    rec = runs.get(job, {}).get(run)
    if rec is None:
        return "no record for run %s/%s" % (job, run)
    t = text_before.replace("**", "").rstrip()
    m = VALUE.search(t)
    if not m:
        return "no number before the citation of %s/%s" % (job, run)
    n = as_int(m.group("n"))
    st = rec.get("status")
    lower = bool(m.group("ge"))
    partial = bool(m.group("partial"))
    if field:
        if field not in rec or not isinstance(rec[field], int):
            return "run %s/%s has no numeric field %s" % (job, run, field)
        if lower or partial:
            return "%s/%s:%s is exact (%s): cite it without ≥ / partial" % (job, run, field, rec[field])
        return None if n == rec[field] else "%s/%s:%s is %s, cited %s" % (job, run, field, rec[field], n)
    if st == "COMPLETE":
        if lower or partial:
            return "%s/%s is COMPLETE (%s states): cite the exact count, not a lower bound" % (job, run, rec.get("states"))
        if n != rec.get("states"):
            return "%s/%s is COMPLETE with %s states, cited %s" % (job, run, rec.get("states"), n)
        if m.group("d") and as_int(m.group("d")) != rec.get("depth"):
            return "%s/%s has depth %s, cited %s" % (job, run, rec.get("depth"), m.group("d"))
        return None
    if st == "PARTIAL":
        if not (lower and partial):
            return ("%s/%s is PARTIAL (stopped with %s queued): write '≥ %s, partial (cap ...)'"
                    % (job, run, rec.get("queued"), "{:,}".format(rec.get("states") or 0)))
        if n != rec.get("states"):
            return "%s/%s is PARTIAL with ≥ %s states, cited ≥ %s" % (job, run, rec.get("states"), n)
        cap = rec.get("cap_s")
        if cap is not None and not cap_matches(m.group("cap"), cap):
            return "%s/%s stopped at its %s s cap; the citation says (%s)" % (job, run, cap, m.group("cap").strip())
        return None
    if st == "VIOLATION":
        want = rec.get("trace_states") if rec.get("trace_states") is not None else rec.get("depth")
        if lower or partial:
            return "%s/%s is a VIOLATION: cite its trace length, not a lower bound" % (job, run)
        return None if n == want else "%s/%s is a VIOLATION with trace length %s, cited %s" % (job, run, want, n)
    return "%s/%s has status %s: it backs no number" % (job, run, st)


def check_least(job, run, field, runs):
    """the problem with citing JOB/RUN for a least violating depth, or None"""
    rec = runs.get(job, {}).get(run)
    if rec is None or rec.get("status") != "VIOLATION" or field not in (None, "trace_states", "depth"):
        return None
    if rec.get("tool") == "enumerator":
        if rec.get("depth") is None:
            return ("%s/%s backs a least depth, but it is an enumerator %s search without BFS levels: "
                    "cite a BFS run" % (job, run, rec.get("mode") or "non-BFS"))
        return None
    w = rec.get("workers")
    if w == 1:
        return None
    return ("%s/%s backs a least depth, but it is TLC with %s: several workers are not a strict BFS, "
            "so cite a run with workers: 1 (TLC_WORKERS=1) or the enumerator's BFS, or label it -workers auto"
            % (job, run, "%s workers" % w if w is not None else "an unrecorded worker count"))


def cells(line):
    s = line.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|"):
        s = s[:-1]
    return [c for c in re.split(r"(?<!\\)\|", s)]


def check_file(path, runs):
    probs = []
    lines = open(path).read().split("\n")
    header = None
    table_least = False
    sections = []  # (level, says least depth) for the headings above this line
    in_code = False
    for i, line in enumerate(lines, 1):
        if line.strip().startswith("```"):
            in_code = not in_code
            continue
        if in_code:
            continue
        h = HEADING.match(line)
        if h:
            lvl = len(h.group(1))
            sections = [x for x in sections if x[0] < lvl] + [(lvl, bool(LEAST.search(line)))]
        section_least = any(x[1] for x in sections)
        is_row = line.strip().startswith("|")
        if not is_row:
            header = None
            table_least = False
        segments = []
        if is_row:
            cs = cells(line)
            if header is None:
                header = [c.strip().lower() for c in cs]
                table_least = bool(LEAST.search(line))
                continue
            if all(re.fullmatch(r"\s*:?-+:?\s*", c) for c in cs):
                continue
            for k, c in enumerate(cs):
                h = header[k] if k < len(header) else ""
                segments.append((c, h, True))
        else:
            segments.append((line, "", False))
        for text, head, in_table in segments:
            masked = text
            pos = 0
            least = section_least or (table_least if in_table else bool(LEAST.search(text)))
            for m in CITE.finditer(text):
                before = text[pos:m.start()]
                p = check_citation(before, m.group(1), m.group(2), m.group(3), runs)
                if p:
                    probs.append("%s:%d: %s" % (path, i, p))
                labelled = AUTO.search(head) or AUTO.search(text if in_table else before)
                if least and not labelled:
                    p = check_least(m.group(1), m.group(2), m.group(3), runs)
                    if p:
                        probs.append("%s:%d: %s" % (path, i, p))
                # mask the cited value and the citation
                v = VALUE.search(before.replace("**", "  ").rstrip())
                start = pos + (v.start() if v else len(before))
                masked = masked[:start] + " " * (m.end() - start) + masked[m.end():]
                pos = m.end()
            if not in_table or "job" in head or "commit" in head or EXEMPT.search(text):
                continue
            for m in COUNT.finditer(masked):
                if UNIT.match(masked[m.end():]):
                    continue
                probs.append("%s:%d: uncited count %s in a table (cite it as N [run JOB/RUN], or mark the cell [estimate] / [derived] / [unverified: ...])" % (path, i, m.group(1)))
    return probs


def main(argv):
    args = list(argv)
    runs_dir = "docs/evidence/runs"
    files = []
    while args:
        a = args.pop(0)
        if a == "--runs":
            runs_dir = args.pop(0)
        elif a in ("-h", "--help"):
            print(__doc__)
            return 0
        else:
            files.append(a)
    if not files:
        files = sorted(glob.glob("docs/evidence/*.md"))
    runs = load_runs(runs_dir)
    probs = []
    for f in files:
        probs += check_file(f, runs)
    for p in probs:
        print(p)
    print("evidence check: %d problem(s) in %d file(s), %d run record file(s)" % (len(probs), len(files), len(runs)))
    return 1 if probs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
