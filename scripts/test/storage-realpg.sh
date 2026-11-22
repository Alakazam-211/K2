#!/usr/bin/env bash
# Lane D's tests on a Linux compute node (never the M5): the storage unit
# tests, then the real-Postgres door and import tests (REALPG_STORAGE=1).
#
#   k2 compute run --sha <c> --stall-timeout off --mem 28G \
#     --needs rust@1.99.1,cmake@3,cargo-nextest \
#     -- bash scripts/test/storage-realpg.sh
#
# Fails when any test fails or when no real-Postgres test ran.
set -uo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
B="${REALPG_BIN:-$(bash "$(dirname "$0")/pg-binaries.sh")}"
[ -x "$B/initdb" ] || { echo "storage-realpg: no Postgres binaries" >&2; exit 1; }
LOG="$(mktemp -d)"

cargo test -p k2-daemon --lib -- compat:: route_policy >"$LOG/unit.log" 2>&1
u=$?
grep -E "^error|^warning|-->|panicked|FAILED|test result" "$LOG/unit.log" | head -80

REALPG_STORAGE=1 REALPG_BIN="$B" cargo test -p k2-daemon --lib compat::storage_real_pg -- --ignored --nocapture --test-threads=1 >"$LOG/pg.log" 2>&1
r=$?
grep -vE "^\s+(Compiling|Downloaded|Downloading)" "$LOG/pg.log" | tail -120
ran=$(grep -oE "test result: ok\. [0-9]+ passed" "$LOG/pg.log" | grep -oE "[0-9]+" | head -1)
ran=${ran:-0}
echo "storage-realpg: unit exit $u, real-pg exit $r, real-pg tests passed: $ran"
[ "$ran" -ge 2 ] || { echo "storage-realpg: the real-Postgres tests did not all run" >&2; exit 1; }
exit $((u | r))
