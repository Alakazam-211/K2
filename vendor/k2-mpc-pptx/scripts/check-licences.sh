#!/bin/sh
# Fail if any Rust dependency (transitive, all targets) has a licence outside the allow-list, in the
# repo's lock, or the fuzz crate's when the tree has one (the main branch does; a release tag and
# a vendored copy leave it out). The fuzz crate is test tooling, never shipped, and it alone may
# hold libfuzzer-sys, whose bundled libFuzzer is NCSA.
# Allowed: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, and Unicode-3.0 (unicode-ident, build-time).
# An "A OR B" expression passes if any alternative is allowed; "A AND B" needs every part allowed.
# Prints every judged package as `<where> <name> <version>: <licence>`.
#
#   scripts/check-licences.sh                  the real locks
#   scripts/check-licences.sh main|fuzz FILE   one `cargo metadata` output, for a self-test
set -eu
cd "$(dirname "$0")/.."
check() { # $1: main or fuzz; reads cargo metadata on stdin
  python3 -I -c '
import json, re, sys
ALLOWED = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0"}
where = sys.argv[1]
def ok(expr, allowed):
    expr = expr.replace("/", " OR ").strip()
    if expr.startswith("(") and expr.endswith(")") and expr.count("(") == 1:
        expr = expr[1:-1]
    parts = [p.strip() for p in re.split(r"\)\s+AND\s+|\s+AND\s+\(|\s+AND\s+", expr)]
    return all(any(alt.strip(" ()") in allowed for alt in p.split(" OR ")) for p in parts)
meta = json.load(sys.stdin)
bad = []
judged = 0
for p in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
    if p["source"] is None:
        continue  # our own crates and the vendored one
    judged += 1
    allowed = ALLOWED | {"NCSA"} if where == "fuzz" and p["name"] == "libfuzzer-sys" else ALLOWED
    lic = p.get("license") or "NONE"
    print(where, p["name"], p["version"] + ":", lic)
    if not ok(lic, allowed):
        bad.append(p["name"] + " " + p["version"] + ": " + lic)
if bad:
    print("Licences outside the allow-list (" + where + "):"); print("\n".join(bad)); sys.exit(1)
print("licences ok, " + where + " (%d packages judged)" % judged)
' "$1"
}
if [ $# -eq 2 ]; then
  check "$1" < "$2"
  exit 0
fi
cargo metadata --format-version 1 --locked --all-features | check main
if [ -f fuzz/Cargo.toml ]; then
  cargo metadata --format-version 1 --locked --all-features --manifest-path fuzz/Cargo.toml | check fuzz
fi
