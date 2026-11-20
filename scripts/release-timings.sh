#!/usr/bin/env bash
# release-timings.sh — print how long each step of a release took, from the
# timings release.sh writes (scripts/lib/release-timer.sh has the format).
#
#   scripts/release-timings.sh <version>          every run of that release
#   scripts/release-timings.sh --last             the release run last written to
#   scripts/release-timings.sh --dry <version>    rehearsals (release.sh --dry-run)
#   scripts/release-timings.sh --dry --last
#   scripts/release-timings.sh --file <timings.jsonl>
#   add --run <run id> for one run only
#
# One table per run (sub-steps indented under their step), then the totals:
# machine time (this Mac, Apple, GitHub, the compute nodes working) and
# waiting time (waiting for a person: the signing approval), for each run
# and for all of them. A step that began but never ended in a run that has
# no end record shows as "unfinished" (the run was killed).
#
# Files: ${K2_RELEASE_AUDIT_DIR:-~/k2-release-audit}/<version>/timings.jsonl
# (real runs) and ${K2_RELEASE_DRY_ROOT:-<main checkout>/.claude/worktrees/
# release-dry}/timings/<version>/timings.jsonl (rehearsals).
# Exit: 0 printed, 1 no timings found or a file it can't read, 2 usage.
set -euo pipefail

usage() {
    sed -n '3,9p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

dry=0 last=0 version="" file="" run=""
while [ $# -gt 0 ]; do
    case "$1" in
        --dry) dry=1 ;;
        --last) last=1 ;;
        --file) [ $# -ge 2 ] || usage; file="$2"; shift ;;
        --run) [ $# -ge 2 ] || usage; run="$2"; shift ;;
        -h|--help) usage ;;
        -*) echo "release-timings: unknown option $1" >&2; usage ;;
        *) [ -z "$version" ] || usage; version="$1" ;;
    esac
    shift
done

if [ -z "$file" ]; then
    if [ "$dry" = 1 ]; then
        if [ -n "${K2_RELEASE_DRY_ROOT:-}" ]; then
            base="$K2_RELEASE_DRY_ROOT"
        else
            # As scripts/lib/release-dry.sh picks it: beside the main checkout's worktrees.
            here="$(cd "$(dirname "$0")/.." && pwd)"
            common="$(cd "$here" && cd "$(git rev-parse --git-common-dir)" && pwd -P)" \
                || { echo "release-timings: $here is not a git checkout; set K2_RELEASE_DRY_ROOT" >&2; exit 1; }
            if [ "$(basename "$common")" = .git ]; then
                base="$(dirname "$common")/.claude/worktrees/release-dry"
            else
                base="$HOME/k2-release-dry"
            fi
        fi
        base="$base/timings"
    else
        base="${K2_RELEASE_AUDIT_DIR:-$HOME/k2-release-audit}"
    fi
    if [ "$last" = 1 ]; then
        [ -z "$version" ] || usage
        # The file written to last (ls -t: newest first).
        file="$(ls -t "$base"/*/timings.jsonl 2>/dev/null | head -1 || true)"
        [ -n "$file" ] || { echo "release-timings: no timings under $base" >&2; exit 1; }
    else
        [ -n "$version" ] || usage
        case "$version" in *[!0-9A-Za-z._-]*|.*) echo "release-timings: not a version: $version" >&2; exit 2 ;; esac
        file="$base/$version/timings.jsonl"
    fi
elif [ -n "$version" ] || [ "$last" = 1 ]; then
    usage
fi
[ -f "$file" ] || { echo "release-timings: no timings file $file" >&2; exit 1; }

python3 -I - "$file" "$run" <<'PY'
import json, sys

path, only = sys.argv[1], sys.argv[2]

def dur(s):
    s = max(0, int(s))
    if s >= 3600:
        return "%dh%02dm%02ds" % (s // 3600, (s % 3600) // 60, s % 60)
    return "%dm%02ds" % (s // 60, s % 60)

runs, order = {}, []
with open(path, encoding="utf-8") as f:
    for n, line in enumerate(f, 1):
        line = line.strip()
        if not line:
            continue
        try:
            r = json.loads(line)
        except ValueError:
            print("release-timings: %s line %d is not JSON; skipped" % (path, n), file=sys.stderr)
            continue
        if not isinstance(r, dict):
            continue
        r["_line"] = n
        rid = r.get("run")
        if rid not in runs:
            runs[rid] = {"run": None, "begins": [], "steps": [], "end": None}
            order.append(rid)
        t = r.get("type")
        if t == "run":
            runs[rid]["run"] = r
        elif t == "begin":
            runs[rid]["begins"].append(r)
        elif t == "step":
            runs[rid]["steps"].append(r)
        elif t == "end":
            runs[rid]["end"] = r

if only:
    if only not in runs:
        print("release-timings: no run %s in %s" % (only, path), file=sys.stderr)
        sys.exit(1)
    order = [only]
if not order:
    print("release-timings: %s has no runs" % path, file=sys.stderr)
    sys.exit(1)

def rows_of(d):
    """The run's steps in start order; a begin with no step is unfinished."""
    rows = list(d["steps"])
    ended = {(s.get("id"), s.get("start_epoch")) for s in rows}
    # Start order is the order of the begin records (a skipped step has
    # none: its own line is where it falls).
    began = {(b.get("id"), b.get("start_epoch")): b["_line"] for b in d["begins"]}
    for s in rows:
        s["_order"] = began.get((s.get("id"), s.get("start_epoch")), s["_line"])
    for b in d["begins"]:
        if (b.get("id"), b.get("start_epoch")) not in ended:
            u = dict(b)
            u["status"] = "unfinished" if d["end"] is None else "unended"
            u["seconds"] = None
            u["depth"] = None
            u["_order"] = b["_line"]
            rows.append(u)
    # Depth from the parent chain when a row doesn't say (unfinished ones).
    for r in rows:
        if r.get("depth") is None:
            r["depth"] = 0 if not r.get("parent") else 1
    rows.sort(key=lambda r: r["_order"])
    return rows

def totals(rows):
    total = sum(r["seconds"] for r in rows if r.get("depth") == 0 and isinstance(r.get("seconds"), int))
    waiting = 0
    waits = [r for r in rows if r.get("kind") == "waiting" and isinstance(r.get("seconds"), int)]
    for r in waits:
        # A waiting row inside another waiting row is counted once (the outer one).
        inner = any(o is not r and o.get("depth", 0) < r.get("depth", 0)
                    and (o.get("start_epoch") or 0) <= (r.get("start_epoch") or 0)
                    and (o.get("end_epoch") or 0) >= (r.get("end_epoch") or 0) for o in waits)
        if not inner:
            waiting += r["seconds"]
    return total, waiting

grand_m = grand_w = 0
print("Release timings: %s" % path)
for rid in order:
    d = runs[rid]
    h = d["run"] or {}
    e = d["end"]
    rows = rows_of(d)
    print("")
    print("Run %s: v%s, %s, host %s, %s, started %s" % (
        rid, h.get("version", "?"), h.get("mode", "?"), h.get("host") or "?",
        (h.get("git_sha") or "?")[:12], h.get("started", "?")))
    if e is None:
        print("  (no end record: the run was killed or is still going)")
    print("  %-26s %-8s %-11s %9s  %s" % ("step", "kind", "status", "time", "what"))
    for r in rows:
        pad = "  " * int(r.get("depth") or 0)
        secs = r.get("seconds")
        print("  %-26s %-8s %-11s %9s  %s" % (pad + str(r.get("id")), r.get("kind", "?"), r.get("status", "?"),
                                              dur(secs) if isinstance(secs, int) else "-", r.get("label", "")))
    total, waiting = totals(rows)
    grand_m += total - waiting
    grand_w += waiting
    wall = ""
    if e is not None and isinstance(e.get("seconds"), int):
        wall = ", wall clock %s, exit %s (%s)" % (dur(e["seconds"]), e.get("rc"), e.get("status"))
    print("  machine %s, waiting for a person %s, steps %s%s" % (dur(total - waiting), dur(waiting), dur(total), wall))

print("")
print("Totals (%d run%s): machine %s, waiting for a person %s, all %s" % (
    len(order), "" if len(order) == 1 else "s", dur(grand_m), dur(grand_w), dur(grand_m + grand_w)))
PY
