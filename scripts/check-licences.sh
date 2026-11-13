#!/bin/sh
# Fail if any Rust dependency (transitive, all targets) has a licence outside the allow-list.
# Allowed: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, and Unicode-3.0 (unicode-ident, build-time).
# An "A OR B" expression passes if any alternative is allowed; "A AND B" needs every part allowed.
#
#   scripts/check-licences.sh                     every dependency (`cargo metadata --locked`)
#   scripts/check-licences.sh --expr NAME=EXPR…   each licence expression, with no cargo: the same
#                                                 allow-list and rule (for a vendored crate's own
#                                                 manifest, checked before it is vendored)
set -eu
cd "$(dirname "$0")/.."
RULE='
import json, re, sys
ALLOWED = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0",
           "Apache-2.0 WITH LLVM-exception", "Unlicense"}
def ok(expr):
    expr = expr.replace("/", " OR ").strip()
    if expr.startswith("(") and expr.endswith(")") and expr.count("(") == 1:
        expr = expr[1:-1]
    # split top-level AND, then OR inside parentheses
    parts = [p.strip() for p in re.split(r"\)\s+AND\s+|\s+AND\s+\(|\s+AND\s+", expr)]
    return all(any(alt.strip(" ()") in ALLOWED for alt in p.split(" OR ")) for p in parts)
'
if [ "${1:-}" = --expr ]; then
  shift
  [ $# -gt 0 ] || { echo "usage: check-licences.sh --expr NAME=EXPR..." >&2; exit 2; }
  python3 -I -c "$RULE"'
args = [(a.split("=", 1) + ["NONE"])[:2] for a in sys.argv[1:]]
bad = [n + ": " + (e or "NONE") for n, e in args if not ok(e or "NONE")]
if bad:
    print("Licences outside the allow-list:"); print("\n".join(bad)); sys.exit(1)
print("licences ok (%d: %s)" % (len(args), ", ".join(n + " (" + e + ")" for n, e in args)))
' "$@"
  exit
fi
cargo metadata --format-version 1 --locked | python3 -I -c "$RULE"'
meta = json.load(sys.stdin)
bad = []
for p in meta["packages"]:
    if p["source"] is None and p["name"].startswith(("k2-mpc-", "mpce-")):
        continue  # our own crates (a vendored copy of another project, vendor/yrs, is checked)
    lic = p.get("license") or "NONE"
    if not ok(lic):
        bad.append(p["name"] + " " + p["version"] + ": " + lic)
if bad:
    print("Licences outside the allow-list:"); print("\n".join(bad)); sys.exit(1)
print("licences ok (%d dependencies, %d of them vendored: %s)" % (
    sum(1 for p in meta["packages"] if not p["name"].startswith(("k2-mpc-", "mpce-"))),
    sum(1 for p in meta["packages"] if p["source"] is None and not p["name"].startswith(("k2-mpc-", "mpce-"))),
    ", ".join(p["name"] + " " + p["version"] + " (" + (p.get("license") or "NONE") + ")" for p in meta["packages"] if p["source"] is None and not p["name"].startswith(("k2-mpc-", "mpce-")))))
'
