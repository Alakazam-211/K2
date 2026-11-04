#!/bin/sh
# Fail if any Rust dependency (transitive, all targets) has a licence outside the allow-list, in the
# repo's lock, the fuzz crate's or the bridge's. The fuzz crate is test tooling, never shipped, and it
# alone may hold libfuzzer-sys, whose bundled libFuzzer is NCSA. The bridge's lock holds the kit and
# the kit's vendored crates as path packages: there every package is judged, path ones included,
# and a path package with no licence fails.
# Allowed: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, and Unicode-3.0 (unicode-ident, build-time).
# An "A OR B" expression passes if any alternative is allowed; "A AND B" needs every part allowed.
#
#   scripts/check-licences.sh                 the real locks
#   scripts/check-licences.sh main|fuzz|bridge FILE  one `cargo metadata` output, for the self-test
set -eu
cd "$(dirname "$0")/.."
check() { # $1: main, fuzz or bridge; reads cargo metadata on stdin
  python3 -I -c '
import json, re, sys
ALLOWED = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0",
           "Apache-2.0 WITH LLVM-exception", "Unlicense"}
where = sys.argv[1]
def ok(expr, allowed):
    expr = expr.replace("/", " OR ").strip()
    if expr.startswith("(") and expr.endswith(")") and expr.count("(") == 1:
        expr = expr[1:-1]
    # split top-level AND, then OR inside parentheses
    parts = [p.strip() for p in re.split(r"\)\s+AND\s+|\s+AND\s+\(|\s+AND\s+", expr)]
    return all(any(alt.strip(" ()") in allowed for alt in p.split(" OR ")) for p in parts)
meta = json.load(sys.stdin)
bad = []
for p in meta["packages"]:
    if p["source"] is None and where != "bridge":
        continue  # our own crates
    allowed = ALLOWED | {"NCSA"} if where == "fuzz" and p["name"] == "libfuzzer-sys" else ALLOWED
    lic = p.get("license") or "NONE"
    if not ok(lic, allowed):
        bad.append(p["name"] + " " + p["version"] + ": " + lic)
if bad:
    print("Licences outside the allow-list (" + where + "):"); print("\n".join(bad)); sys.exit(1)
judged = sum(1 for p in meta["packages"] if p["source"] or where == "bridge")
print("licences ok, " + where + " (%d packages judged)" % judged)
' "$1"
}
if [ $# -eq 2 ]; then
  check "$1" < "$2"
  exit 0
fi
cargo metadata --format-version 1 --locked | check main
cargo metadata --format-version 1 --locked --manifest-path fuzz/Cargo.toml | check fuzz
cargo metadata --format-version 1 --locked --manifest-path bridge/Cargo.toml | check bridge
