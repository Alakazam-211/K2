#!/bin/sh
# Pack-template check: a pack that keeps a carrier runs the kit's byte-exact row.
# Run from a pack repo's root: ../k2-mpc-base/scripts/pack-checks/carrier-row.sh
#
# A pack whose source (outside tests/, target/ and the fuzz crate) implements
# `Pack::wants_carrier` with a body other than a plain `false` keeps a carrier, and must call
# `k2_mpc_base::session::test_support::carrier_judge` over its corpus in a test under a
# `tests/` folder (docs/PACK_TEMPLATE.md, "A format the pack models only in part"). Exit 0: no
# carrier, or the row is there; 1: a carrier pack without the row; 2: usage.
set -eu
ROOT="${1:-.}"
[ -d "$ROOT" ] || { echo "carrier-row: no folder $ROOT" >&2; exit 2; }
cd "$ROOT"
python3 -I - <<'PY'
import os, re, sys
SKIP = {"node_modules", "target", ".git", "fuzz"}
impl = re.compile(r"fn\s+wants_carrier\s*\(\s*&\s*self\s*\)\s*->\s*bool\s*\{(?P<body>[^}]*)\}")
keeps, judged = [], []
for dirpath, dirnames, files in os.walk("."):
    dirnames[:] = [d for d in dirnames if d not in SKIP]
    in_tests = "tests" in dirpath.split(os.sep)
    for f in files:
        if not f.endswith(".rs"):
            continue
        path = os.path.join(dirpath, f)
        text = open(path, encoding="utf-8", errors="replace").read()
        if in_tests:
            if "carrier_judge(" in text:
                judged.append(path)
            continue
        for m in impl.finditer(text):
            if m.group("body").strip() != "false":
                keeps.append(path)
if keeps and not judged:
    print("carrier-row: the pack keeps a carrier (wants_carrier in " + ", ".join(sorted(set(keeps)))
          + ") but no test under tests/ calls test_support::carrier_judge over its corpus")
    sys.exit(1)
if keeps:
    print("carrier-row: ok (carrier pack; the row runs in " + ", ".join(sorted(judged)) + ")")
else:
    print("carrier-row: ok (no carrier)")
PY
