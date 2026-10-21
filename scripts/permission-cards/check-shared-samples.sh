#!/usr/bin/env bash
# Permission cards: fail if the shared samples folder differs from the repo
# (PRD rev 3.1 §13.4, §17.10). Mac only; run by integration (I).
#
#   scripts/permission-cards/check-shared-samples.sh [shared-dir]
#   shared-dir defaults to /Users/Shared/k2-companion/permission-cards/samples
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/crates/k2-daemon/tests/golden/permission_cards"
shared="${1:-/Users/Shared/k2-companion/permission-cards/samples}"

if [ ! -d "$shared" ]; then
  echo "FAIL: no shared samples at $shared (run scripts/permission-cards/publish-samples.sh)" >&2
  exit 1
fi
# The JSON files only (the shared folder may carry a README).
if ! diff -r -x 'README.md' "$src" "$shared"; then
  echo "FAIL: $shared differs from $src: publish again (scripts/permission-cards/publish-samples.sh)" >&2
  exit 1
fi
# index.json's hashes match the files they name.
python3 - "$shared" <<'PY'
import hashlib, json, os, sys
d = sys.argv[1]
idx = json.load(open(os.path.join(d, "index.json")))
bad = []
for row in idx["files"]:
    p = os.path.join(d, row["file"])
    h = hashlib.sha256(open(p, "rb").read()).hexdigest()
    if h != row["sha256"]:
        bad.append(f'{row["file"]}: sha256 {h} != index {row["sha256"]}')
listed = {r["file"] for r in idx["files"]} | {"index.json"}
extra = sorted(f for f in os.listdir(d) if f.endswith(".json") and f not in listed)
bad += [f"{f}: not in index.json" for f in extra]
if bad:
    print("FAIL:\n" + "\n".join(bad), file=sys.stderr)
    sys.exit(1)
print(f"ok: {len(idx['files'])} samples match index.json")
PY
