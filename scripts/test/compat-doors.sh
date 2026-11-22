#!/usr/bin/env bash
# The Supabase-compatible doors' tests (prd-app-supabase-compat-v1, lane C),
# for a Linux compute node with Postgres:
#
#   k2 compute run --sha <c> --stall-timeout off \
#     --needs rust@1.99.1,cmake@3,cargo-nextest,sys:postgresql --mem 28G \
#     -- bash scripts/test/compat-doors.sh [rest|unit|realpg|oracle|licences|all]
#
# k2-rest's own tests, the daemon's compat unit tests, and (realpg) the
# tests marked #[ignore] that need a throwaway Postgres (REALPG_COMPAT=1;
# k2_rest::testkit starts it from REALPG_BIN). Every step must report
# "test result: ok" with at least one test run, or the script fails.
set -uo pipefail
what="${1:-all}"
for v in $(env | grep -o '^K2_[A-Za-z0-9_]*' | grep -v '^K2_JOB'); do unset "$v"; done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
REALPG_BIN="${REALPG_BIN:-$(ls -d /usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -1)}"
# No Postgres installed on the node (its package list is the owner's): fetch
# the distro's own packages without root (apt-get download) and unpack them
# under the job cache. Postgres finds its share files relative to its bin.
if [ ! -x "${REALPG_BIN:-/nonexistent}/initdb" ] && command -v apt-get >/dev/null; then
    pgroot="${K2_JOB_CACHE:-$HOME/.cache}/k2rest-pg"
    if [ ! -x "$(ls -d "$pgroot"/usr/lib/postgresql/*/bin 2>/dev/null | tail -1)/initdb" ]; then
        mkdir -p "$pgroot/debs" && (cd "$pgroot/debs" && apt-get download postgresql-16 postgresql-client-16 libpq5 >/dev/null) \
            && for d in "$pgroot"/debs/*.deb; do dpkg-deb -x "$d" "$pgroot"; done
    fi
    REALPG_BIN="$(ls -d "$pgroot"/usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -1)"
    export LD_LIBRARY_PATH="$pgroot/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
export REALPG_BIN
fail=0
out="$(mktemp)"

# run <name> <cmd…>: the command's test lines, then a check that it ran
# tests and all passed.
run() {
    local name="$1"; shift
    echo "=== $name"
    "$@" >"$out" 2>&1
    local rc=$?
    grep -E "^(warning|error)(\[|:)|^ *--> |^test |test result|panicked|FAILED|^failures:|\[testkit\]" "$out" | grep -v "^test .* \.\.\. ignored" | head -300
    if [ $rc -ne 0 ]; then
        echo "--- $name exited $rc; last lines:"; tail -200 "$out"; fail=1; return
    fi
    if ! grep -qE "test result: ok\. [1-9][0-9]* passed" "$out"; then
        echo "--- $name: no test ran"; fail=1
    fi
}

if [ "$what" = licences ] || [ "$what" = all ]; then
    echo "=== licences of the crates the doors added"
    cargo metadata --format-version 1 2>/dev/null | python3 -c '
import json, sys
want = {"deadpool", "deadpool-postgres", "deadpool-runtime", "fallible-iterator", "md-5", "postgres-protocol", "postgres-types",
        "stringprep", "tokio-postgres", "unicode-normalization", "unicode-properties", "whoami", "wasite", "wasi", "objc2-system-configuration"}
for p in sorted(json.load(sys.stdin)["packages"], key=lambda p: (p["name"], p["version"])):
    if p["name"] in want:
        print(p["name"], p["version"] + ":", p.get("license"))
' || { echo "cargo metadata failed"; fail=1; }
fi
# rest: the grammar crate alone (fast: no daemon build), with its real
# Postgres tests and the oracle.
case "$what" in rest|unit|all) run "k2-rest" cargo test -p k2-rest --features testkit ;; esac
if [ "$what" = unit ] || [ "$what" = all ]; then
    run "k2-daemon apps::compat" cargo test -p k2-daemon --lib apps::compat
fi
case "$what" in rest|realpg|all)
    [ -x "$REALPG_BIN/initdb" ] || { echo "no Postgres at $REALPG_BIN"; exit 1; }
    run "k2-rest real Postgres" env REALPG_COMPAT=1 cargo test -p k2-rest --features testkit real_pg -- --ignored ;;
esac
if [ "$what" = realpg ] || [ "$what" = all ]; then
    run "k2-daemon compat real Postgres" env REALPG_COMPAT=1 cargo test -p k2-daemon --lib apps::compat -- --ignored
fi
if [ "$what" = oracle ] || [ "$what" = rest ] || [ "$what" = all ]; then
    # PostgREST as a TEST ORACLE (never shipped): the static release binary,
    # kept in the job cache.
    pgrst_v="${ORACLE_POSTGREST_VERSION:-12.2.3}"
    pgrst_dir="${K2_JOB_CACHE:-$HOME/.cache}/postgrest-$pgrst_v"
    if [ ! -x "$pgrst_dir/postgrest" ]; then
        mkdir -p "$pgrst_dir"
        curl -fsSL "https://github.com/PostgREST/postgrest/releases/download/v$pgrst_v/postgrest-v$pgrst_v-linux-static-x64.tar.xz" \
            | tar -xJ -C "$pgrst_dir" || { echo "could not fetch PostgREST $pgrst_v"; exit 1; }
    fi
    "$pgrst_dir/postgrest" --version || { echo "PostgREST does not run"; exit 1; }
    run "oracle: K2 vs PostgREST $pgrst_v" env REALPG_COMPAT=1 ORACLE_POSTGREST="$pgrst_dir/postgrest" cargo test -p k2-rest --features testkit oracle_ -- --ignored --nocapture
fi
for d in /tmp/k2rest-*; do
    [ -d "$d/data" ] && "$REALPG_BIN/pg_ctl" -D "$d/data" -m immediate stop >/dev/null 2>&1
    rm -rf "$d"
done
rm -f "$out"
echo "=== RESULT $([ $fail = 0 ] && echo PASS || echo FAIL)"
exit $fail
