#!/usr/bin/env bash
# check-daemon-test-targets.sh: no k2-daemon integration test can drop out of
# the build silently. No cargo.
#
# k2-daemon's integration tests are grouped by area: crates/k2-daemon/tests/
# it_<area>/main.rs is one test binary and every other .rs file next to it is
# one module of it (each was its own binary before). Cargo finds
# tests/*.rs and tests/*/main.rs by itself, but a module file only compiles
# when main.rs declares it, so a file someone adds or moves into a group and
# forgets to declare is never built and its tests never run.
#
# Checks, for every tests/it_*/ directory:
#   - it has a main.rs;
#   - main.rs declares `mod <name>;` for every other .rs file there, and has a
#     file for every `mod <name>;` it declares outside its `support` block;
#   - main.rs has the `runner_guard` test (support::env::require_nextest), so
#     a plain `cargo test` of the whole group fails loudly instead of mixing
#     modules that expect a process of their own;
#   - no module file still includes support with `#[path = "support/..."]`
#     (in a group it is `use crate::support::<name>;`);
#   - no test name is both a top-level tests/<name>.rs and a group module, or
#     in two groups (its tests would run twice, and a filter would be unclear).
#
# Usage: scripts/check-daemon-test-targets.sh [repo-root]
#   repo-root defaults to this script's repo; tests/packaging/
#   daemon-test-targets.sh points it at a temp copy with a broken layout.
#   Exit 0 when every check passes, else 1.
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
ROOT="$(cd "$ROOT" && pwd)"
TESTS="$ROOT/crates/k2-daemon/tests"
[ -d "$TESTS" ] || { echo "check-daemon-test-targets: no $TESTS" >&2; exit 1; }

python3 -I - "$TESTS" <<'PY'
import os, re, sys

tests = sys.argv[1]
bad = []
seen = {}  # module name -> where it is

for f in sorted(os.listdir(tests)):
    if f.endswith(".rs"):
        seen[f[:-3]] = "tests/" + f

groups = sorted(d for d in os.listdir(tests) if d.startswith("it_") and os.path.isdir(os.path.join(tests, d)))
for g in groups:
    gdir = os.path.join(tests, g)
    main = os.path.join(gdir, "main.rs")
    if not os.path.isfile(main):
        bad.append(f"tests/{g}/ has no main.rs, so cargo builds none of its modules")
        continue
    text = open(main).read()
    # `mod name;` at the top level: support's modules sit inside `mod support { ... }`.
    top = re.sub(r"mod support \{.*?\n\}", "", text, flags=re.S)
    declared = set(re.findall(r"^\s*mod (\w+);", top, re.M))
    files = sorted(f[:-3] for f in os.listdir(gdir) if f.endswith(".rs") and f != "main.rs")
    for d in sorted(os.listdir(gdir)):
        if os.path.isdir(os.path.join(gdir, d)):
            bad.append(f"tests/{g}/{d}/: a group holds flat module files only")
    for n in files:
        if n not in declared:
            bad.append(f"tests/{g}/{n}.rs is not declared in tests/{g}/main.rs (add `mod {n};`), so it never compiles")
        src = open(os.path.join(gdir, n + ".rs")).read()
        if re.search(r'#\[path = "support/', src):
            bad.append(f"tests/{g}/{n}.rs includes support with #[path]; in a group use `use crate::support::<name>;`")
        if n in seen:
            bad.append(f"{n} is both {seen[n]} and tests/{g}/{n}.rs")
        else:
            seen[n] = f"tests/{g}/{n}.rs"
    for n in sorted(declared - set(files)):
        bad.append(f"tests/{g}/main.rs declares `mod {n};` but tests/{g}/{n}.rs doesn't exist")
    if not re.search(r"fn runner_guard\(\)\s*\{\s*support::env::require_nextest\(\);", text):
        bad.append(f"tests/{g}/main.rs has no runner_guard test calling support::env::require_nextest()")

for b in bad:
    print("check-daemon-test-targets: " + b)
if bad:
    sys.exit(1)
print(f"check-daemon-test-targets: ok ({len(groups)} groups, {len(seen)} test modules and binaries)")
PY
