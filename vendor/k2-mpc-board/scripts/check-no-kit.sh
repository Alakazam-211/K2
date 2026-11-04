#!/bin/sh
# The codec stands alone: fail unless the crate's direct dependencies are exactly the allowed ones,
# no package in the lock belongs to a multiplayer kit or an engine, and the bridge (bridge/, which
# does depend on the kit) is its own workspace: never a member of the repo's, never in its lock,
# never a dependency of the crate.
#
#   scripts/check-no-kit.sh          the repo's `cargo metadata`
#   scripts/check-no-kit.sh FILE     one `cargo metadata` output, for the self-test
set -eu
cd "$(dirname "$0")/.."
check() {
  python3 -I -c '
import json, sys
ALLOWED = {"miniz_oxide", "sha2"}
FORBIDDEN = ("k2-mpc-", "mpce-", "yrs")
BRIDGE = "k2-mpc-board-bridge"
meta = json.load(sys.stdin)
bad = []
seen_crate = False
for p in meta["packages"]:
    if p["name"] == "k2-mpc-board":
        seen_crate = True
        for d in p["dependencies"]:
            if d["name"] not in ALLOWED:
                bad.append("direct dependency: " + d["name"])
    elif p["name"] == BRIDGE:
        bad.append("the bridge is in the lock")
    elif p["name"].startswith(FORBIDDEN):
        bad.append("in the lock: " + p["name"])
for m in meta.get("workspace_members", []):
    if BRIDGE in m:
        bad.append("the bridge is a workspace member: " + m)
if not seen_crate:
    bad.append("no k2-mpc-board package in the metadata")
if bad:
    print("check-no-kit: " + "; ".join(bad)); sys.exit(1)
print("check-no-kit: clean (%d packages; the bridge is outside the workspace)" % len(meta["packages"]))
'
}
if [ $# -eq 1 ]; then
  check < "$1"
  exit 0
fi
grep -q '^\[workspace\]' bridge/Cargo.toml || { echo "check-no-kit: bridge/Cargo.toml has no [workspace] of its own"; exit 1; }
cargo metadata --format-version 1 --locked | check
