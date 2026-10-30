#!/usr/bin/env python3
"""ck-from-log.py LOG: rebuild the checkpoint files of the `undo_enum dfs` runs in a job's log
(tools/undo/g1.sh, "Resume"): for each run, its header line, then each finished seed's
`ckpt seed K states N max_depth D` line as `seed K states N max_depth D viol -`. Writes
ck-<name>.txt in the current directory, <name> as g1.sh names the run (3x1u2r1, 2x2u2r1,
3x2u2r0), and prints each file's seed count and sha256. A seed is checkpointed only when it
finished without a violation, so `viol` is always `-`. Python 3, stdlib only."""
import hashlib
import re
import sys


def name(header):
    b = dict(kv.split("=", 1) for kv in header.split() if "=" in kv)
    return "%sx%su%sr%s" % (b["pids"], b["events"], b["undos"], b["reverts"])


def main(argv):
    if len(argv) != 2:
        sys.exit(__doc__)
    files, cur = {}, None
    with open(argv[1], errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("undo_enum dfs "):
                cur = name(line)
                files[cur] = [line]
            elif cur and re.fullmatch(r"ckpt seed \d+ states \d+ max_depth \d+", line):
                w = line.split()
                files[cur].append("seed %s states %s max_depth %s viol -" % (w[2], w[4], w[6]))
    for n, lines in files.items():
        seeds = [l.split()[1] for l in lines[1:]]
        if len(set(seeds)) != len(seeds):
            sys.exit("ck-from-log: a seed twice in %s" % n)
        text = "\n".join(lines) + "\n"
        with open("ck-%s.txt" % n, "w") as out:
            out.write(text)
        print("ck-%s.txt: %d seeds, sha256 %s" % (n, len(seeds), hashlib.sha256(text.encode()).hexdigest()))


if __name__ == "__main__":
    main(sys.argv)
