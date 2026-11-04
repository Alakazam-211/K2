#!/bin/sh
# The codec stands alone: fail unless the crate's direct dependencies are exactly the allowed ones
# and no package in the lock belongs to a multiplayer kit or an engine.
set -eu
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --locked | python3 -I -c '
import json, sys
ALLOWED = {"miniz_oxide", "sha2"}
FORBIDDEN = ("k2-mpc-", "mpce-", "yrs")
meta = json.load(sys.stdin)
bad = []
for p in meta["packages"]:
    if p["name"] == "k2-mpc-board":
        for d in p["dependencies"]:
            if d["name"] not in ALLOWED:
                bad.append("direct dependency: " + d["name"])
    elif p["name"].startswith(FORBIDDEN):
        bad.append("in the lock: " + p["name"])
if bad:
    print("check-no-kit: " + "; ".join(bad)); sys.exit(1)
print("check-no-kit: clean (%d packages)" % len(meta["packages"]))
'
