#!/usr/bin/env python3
"""pick-node.py <cpus> <node>... < `k2 compute nodes --json`: print the allowed node to submit
to now: online and available, an empty queue and <cpus> free CPUs, spreading a run's jobs (FUZZ_USED) (the node's `running`
counts every server's jobs, so it is no guide), the most free CPU first; when none has all that, the one with the
shortest queue. Exit 1 when none of the nodes is online."""
import json
import os
import sys


def main():
    cpus = int(sys.argv[1]) * 1000
    allowed = sys.argv[2:]
    nodes = [n for n in json.load(sys.stdin).get("nodes", []) if n.get("name") in allowed]
    up = [n for n in nodes if n.get("online") and (n.get("offer") or {}).get("available")]
    if not up:
        return 1

    def free(n):
        return ((n.get("offer") or {}).get("free") or {}).get("cpu_millis", 0)

    # nodes this run already sent jobs to (FUZZ_USED, space-separated, repeats count) go
    # last: the listing's queue counts lag behind a burst of submissions
    used = os.environ.get("FUZZ_USED", "").split()
    ready = [n for n in up if not n.get("queued") and free(n) >= cpus]
    pool = ready or up
    best = min(pool, key=lambda n: (used.count(n["name"]), n.get("queued") or 0, -free(n)))
    print(best["name"])
    return 0


sys.exit(main())
