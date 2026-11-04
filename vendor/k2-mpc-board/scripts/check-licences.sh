#!/bin/sh
# Fail if any Rust dependency (transitive, all targets) has a licence outside the allow-list.
# Allowed: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, and Unicode-3.0 (unicode-ident, build-time).
# An "A OR B" expression passes if any alternative is allowed; "A AND B" needs every part allowed.
set -eu
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --locked | python3 -I -c '
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
meta = json.load(sys.stdin)
bad = []
for p in meta["packages"]:
    if p["source"] is None:
        continue  # our own crates
    lic = p.get("license") or "NONE"
    if not ok(lic):
        bad.append(p["name"] + " " + p["version"] + ": " + lic)
if bad:
    print("Licences outside the allow-list:"); print("\n".join(bad)); sys.exit(1)
print("licences ok (%d dependencies)" % sum(1 for p in meta["packages"] if p["source"]))
'
