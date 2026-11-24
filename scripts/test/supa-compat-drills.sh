#!/usr/bin/env bash
# Every real-Postgres drill of the Supabase-compatible layer
# (prd-app-supabase-compat-v1, lanes A to E) and the older real-Postgres
# tests the lanes touch, on one Linux x86_64 compute node (never the M5):
#
#   k2 compute run --sha <c> --stall-timeout off --needs cmake@3 \
#     --mem 28G --cpus 4 -- bash scripts/test/supa-compat-drills.sh
#
# Postgres 17 with its client tools (the import needs psql, pg_dump and
# pg_restore): the node's own when it has one, else PGDG's packages
# unpacked under the job cache without root. Test rigs only: never
# shipped. REALPG_BIN overrides.
#
# Each drill must report "test result: ok" with at least one test run, or
# the script fails; every drill runs even after one fails.
set -uo pipefail
for v in $(env | grep -o '^K2_[A-Za-z0-9_]*' | grep -v '^K2_JOB'); do unset "$v"; done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"

pg17() {
    local sys root b base codename p f
    sys="$(ls -d /usr/lib/postgresql/17/bin 2>/dev/null || true)"
    if [ -n "$sys" ] && [ -x "$sys/initdb" ] && [ -x "$sys/pg_restore" ]; then echo "$sys"; return 0; fi
    root="${K2_JOB_CACHE:-$HOME/.cache}/pgdg17"
    b="$root/usr/lib/postgresql/17/bin"
    if [ ! -x "$b/pg_restore" ] || [ ! -x "$b/initdb" ]; then
        # shellcheck disable=SC1091
        codename="$(. /etc/os-release && echo "${VERSION_CODENAME:-noble}")"
        base="https://apt.postgresql.org/pub/repos/apt"
        mkdir -p "$root/debs"
        curl -fsSL "$base/dists/$codename-pgdg/main/binary-amd64/Packages.gz" | gunzip >"$root/Packages" || return 1
        for p in postgresql-17 postgresql-client-17 libpq5; do
            f="$(awk -v p="$p" '$1=="Package:"{cur=$2} $1=="Filename:" && cur==p {print $2}' "$root/Packages" | sort -V | tail -1)"
            [ -n "$f" ] || { echo "pg17: $p is not in PGDG's $codename list" >&2; return 1; }
            curl -fsSL -o "$root/debs/$p.deb" "$base/$f" || return 1
            dpkg-deb -x "$root/debs/$p.deb" "$root" || return 1
        done
    fi
    echo "$b"
}

REALPG_BIN="${REALPG_BIN:-$(pg17)}"
[ -x "${REALPG_BIN:-/nonexistent}/initdb" ] || { echo "supa-compat-drills: no Postgres 17 binaries" >&2; exit 1; }
libdir="$(cd "$REALPG_BIN/../../../.." 2>/dev/null && pwd)/lib/x86_64-linux-gnu"
[ -d "$libdir" ] && export LD_LIBRARY_PATH="$libdir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export REALPG_BIN REALPG_MAJOR=17
"$REALPG_BIN/postgres" --version || { echo "supa-compat-drills: postgres does not run"; ldd "$REALPG_BIN/postgres" | grep "not found"; exit 1; }
"$REALPG_BIN/pg_restore" --version || exit 1

fail=0
out="$(mktemp)"
summary=""
# drill <name> <env…> -- <cargo args…>: the test lines, then a check that it
# ran at least one test and all passed.
drill() {
    local name="$1"; shift
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    echo "=== $name"
    env "${envs[@]}" cargo "$@" >"$out" 2>&1
    local rc=$?
    grep -E "^(warning|error)(\[|:)|^ *--> |^test |test result|panicked|FAILED|^failures:|PASS|FAIL" "$out" | grep -v "^test .* \.\.\. ignored" | head -400
    local n
    n="$(grep -oE "test result: ok\. [0-9]+ passed" "$out" | grep -oE "[0-9]+" | awk '{s+=$1} END {print s+0}')"
    if [ $rc -ne 0 ]; then
        echo "--- $name exited $rc; last lines:"; tail -150 "$out"; fail=1; summary="$summary\n$name: FAILED (exit $rc)"; return
    fi
    if [ "${n:-0}" -lt 1 ]; then
        echo "--- $name: no test ran"; fail=1; summary="$summary\n$name: NO TEST RAN"; return
    fi
    summary="$summary\n$name: ok ($n passed)"
}

T=(test --locked -p k2-daemon --lib)
I=(-- --ignored --nocapture --test-threads=1)
# Lane A: identity and row security.
drill "A compat identity and policy drill (REALPG_COMPAT)" REALPG_COMPAT=1 -- "${T[@]}" apps::compat::real_pg_compat::real_pg_compat_identity_and_policy_drill "${I[@]}"
drill "A principal lock owner (REALPG_LOCK)" REALPG_LOCK=1 -- "${T[@]}" sql::tests::real_pg_lock::real_pg_principal_lock_owner_and_boot_pass "${I[@]}"
# Lane B: the import.
drill "B import supabase end to end (REALPG_IMPORT)" REALPG_IMPORT=1 -- "${T[@]}" sql::import_supabase::real_pg::real_pg_import_supabase_end_to_end "${I[@]}"
# Lane C: the doors and their pool, and the grammar crate.
drill "C pool (REALPG_COMPAT)" REALPG_COMPAT=1 -- "${T[@]}" apps::compat::pool::real_pg:: "${I[@]}"
drill "C rest door (REALPG_COMPAT)" REALPG_COMPAT=1 -- "${T[@]}" apps::compat::rest_door::real_pg:: "${I[@]}"
drill "C drill HTTP pass (REALPG_COMPAT)" REALPG_COMPAT=1 -- "${T[@]}" apps::compat::drill_http::real_pg:: "${I[@]}"
drill "C k2-rest real Postgres (REALPG_COMPAT)" REALPG_COMPAT=1 -- test --locked -p k2-rest --features testkit real_pg "${I[@]}"
# Lane D: storage.
drill "D storage door and import (REALPG_STORAGE)" REALPG_STORAGE=1 -- "${T[@]}" apps::compat::storage_real_pg:: "${I[@]}"
# Lane E: point-in-time restore, the switch.
drill "E switch and rollback (REALPG_SWITCH)" REALPG_SWITCH=1 -- "${T[@]}" apps::db_connect::real_pg_switch::real_pg_switch_and_rollback "${I[@]}"
drill "E point-in-time restore (REALPG_PITR)" REALPG_PITR=1 -- test --locked -p k2-root-helper --lib pitr_real_pg "${I[@]}"
# Older real-Postgres tests the lanes touch (CONNECT, the read grant, doors).
drill "main: read grant (REALPG_READ)" REALPG_READ=1 -- "${T[@]}" db_read_routes::real_pg::real_pg_read_grant_reads_and_never_writes "${I[@]}"
drill "main: db door (REALPG_DOOR)" REALPG_DOOR=1 -- "${T[@]}" apps::db_door::real_pg_door::real_pg_door_end_to_end "${I[@]}"
drill "main: app db connect (REALPG_CONNECT)" REALPG_CONNECT=1 -- "${T[@]}" apps::db_connect::real_pg_connect::real_pg_connect_end_to_end "${I[@]}"

printf "=== summary%b\n" "$summary"
for d in /tmp/k2rest-* /tmp/k2-test-pg-*; do [ -e "$d" ] && rm -rf "$d"; done
exit $fail
