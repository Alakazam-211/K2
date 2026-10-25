#!/usr/bin/env bash
# Live dual-run and per-App Postgres majors on REAL Postgres
# (prd-app-pg-choice-dualrun-v1 §12 "Real Postgres" drills 1–8; the 0.46.4
# runbook). THROWAWAY VM ONLY: it restarts Postgres clusters and the K2
# daemon, kills walsenders, fills WAL on purpose and creates and drops
# databases and roles. Never run it on k2-sandbox-01's apt, sewlab, a
# customer box or a Mac (the compute nodes can't run it either: no systemd
# daemon, no root helper).
#
# The box it needs (a fresh Ubuntu 24.04 or Debian 12/13 VM; the runbook's
# §0 setup from runbook-pg-drills-0463):
#   - the 0.46.4 candidate: k2-daemon as the system unit `k2-daemon`
#     (`K2_DAEMON_UNIT` to override) and the root helper at protocol 10
#     with `namespaces.pgdual` (`helper version`);
#   - distro postgresql-16 with 16/main on 5432 and wal_level = replica (the
#     Debian default; the prepare drill needs it), PGDG reachable (the first
#     live move installs 17 through `pg add-major`);
#   - `k2 db enable` done; one registered workspace folder in `K2DR_WS`
#     with db_agent_access = write, a second one in `K2DR_WS2` (drill 1);
#   - the owner token in `K2DR_TOKEN` for cli/k2;
#   - drill 1 only (`per-app`): Apps (beta) prepared (`k2 app box prepare`,
#     `k2 app engine install`); the drill builds two tiny Seeds itself;
#   - the harness user may `sudo` (psql as postgres, pg_ctlcluster,
#     journalctl, systemctl).
#
# Usage: scripts/test/pg-dualrun-vm.sh [scenario ...]   (`list` prints them)
# Order matters on one box: prepare first (it needs a replica 16), then the
# rest. Fails loudly at the first failed check, with its evidence; exit 0
# only when every chosen scenario passed. Each scenario cleans up its own
# databases (a failure leaves them for the post-mortem).
#
# Knobs: K2DR_STREAM_SECS (how long the happy path keeps writing while it
# streams, default 600), K2DR_BIG_GB (drill 8's table, default 2).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$REPO/cli/k2"
UNIT="${K2_DAEMON_UNIT:-k2-daemon}"
WS="${K2DR_WS:-}"
WS2="${K2DR_WS2:-}"
STREAM_SECS="${K2DR_STREAM_SECS:-600}"
BIG_GB="${K2DR_BIG_GB:-2}"
WRITER_PW="drill-writer-Pw-0464"
ALL=(prepare happy security freeze failures rollback statement-timeout per-app)

say() { printf '[k2dr %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
# Writers still running are stopped on any exit (a failed check included).
WRITER_LOGS=()
stop_writers() { local l; for l in "${WRITER_LOGS[@]}"; do touch "$l.stop"; done; wait 2>/dev/null || true; }
trap stop_writers EXIT
die() { say "FAIL: $*"; stop_writers; exit 1; }
pass() { say "PASS: $*"; }

[ "$(uname -s)" = "Linux" ] || die "Linux VM only"
[ -n "${K2DR_TOKEN:-}" ] || die "set K2DR_TOKEN (the owner token)"
[ -n "$WS" ] && [ -d "$WS" ] || die "set K2DR_WS to a registered workspace folder"

k2() { ( cd "$WS" && K2_TOKEN="$K2DR_TOKEN" "$CLI" "$@" ); }
k2in() { local d="$1"; shift; ( cd "$d" && K2_TOKEN="$K2DR_TOKEN" "$CLI" "$@" ); }
jget() { python3 -c "import json,sys; v=json.load(sys.stdin); print($1)"; }
# psql as postgres on a major's cluster: pg_sql <major> <db> <sql>
pg_port() { pg_lsclusters -h | awk -v m="$1" '$1 == m && $2 == "main" {print $3}'; }
pg_sql() { sudo -u postgres psql -X -qtA -v ON_ERROR_STOP=1 -p "$(pg_port "$1")" -d "$2" -c "$3"; }
# The database's current major (K2's record).
db_major() { k2 db status --json | python3 -c "import json,sys; v=json.load(sys.stdin); print([d['major'] for d in v.get('databases', []) if d.get('name') == '$1'][0])"; }
wait_for() { # wait_for <secs> <what> <cmd...>
    local secs="$1" what="$2"; shift 2
    local end=$((SECONDS + secs))
    until "$@" >/dev/null 2>&1; do
        [ "$SECONDS" -lt "$end" ] || die "timed out after ${secs}s: $what"
        sleep 2
    done
}
live_state() { k2 db upgrade "$1" --status --json | python3 -c 'import json,sys; v=json.load(sys.stdin); r=(v.get("live") or [{}])[0]; print(r.get("state", "none"))'; }
is_state() { [ "$(live_state "$1")" = "$2" ]; }
not_state() { [ "$(live_state "$1")" != "$2" ]; }
streaming_or_switched() { local s; s="$(live_state "$1")"; [ "$s" = streaming ] || [ "$s" = switched ]; }
restart_daemon() { sudo systemctl restart "$UNIT"; sleep 3; wait_for 120 "the daemon answers" k2 db status --json; }
journal_since() { sudo journalctl -u "$UNIT" --since "$1" --no-pager -o cat; }

# Roles a database of this name left on any cluster when the database
# itself is gone (an earlier failed run): a fresh database of the same name
# must not inherit them.
clean_name() {
    local name="$1" m
    for m in $(pg_lsclusters -h | awk '$4 == "online" {print $1}'); do
        if [ "$(pg_sql "$m" postgres "SELECT count(*) FROM pg_database WHERE datname = '$name'")" = "0" ]; then
            pg_sql "$m" postgres "DROP ROLE IF EXISTS ${name}_migrator; DROP ROLE IF EXISTS ${name}_agent;" >/dev/null 2>&1 || true
        fi
    done
}
# A fresh K2 database in the workspace (its name on stdout).
new_db() {
    local name="$1"
    clean_name "$name"
    k2 db create --name "$name" --json >/dev/null
    echo "$name"
}
# `k2 db drop` takes no name: it drops the workspace's live database. The
# name is checked so a drill never drops the wrong one.
drop_db() {
    local name="$1" live
    live="$(k2 db list --json 2>/dev/null | python3 -c 'import json,sys; v=json.load(sys.stdin); d=v.get("databases", v) if isinstance(v, dict) else v; print(next((x.get("name","") for x in d if x.get("status","active") == "active"), ""))' 2>/dev/null || true)"
    if [ -n "$live" ] && [ "$live" != "$name" ]; then
        say "note: the live database is $live, not $name; $name not dropped"
        return 0
    fi
    k2 db drop --yes >/dev/null 2>&1 || true
}

# The fixture (PRD §12 drill 3): RLS + FORCE, a policy, an identity column,
# a bigserial, a materialized view, a partitioned table, a SECURITY DEFINER
# function, default privileges, and the writer login.
fixture() {
    local db="$1" m; m="$(db_major "$db")"
    pg_sql "$m" "$db" "
      CREATE SCHEMA IF NOT EXISTS drill AUTHORIZATION ${db}_migrator;
      SET ROLE ${db}_migrator;
      CREATE TABLE drill.ticks (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, n bigint NOT NULL UNIQUE, at timestamptz NOT NULL DEFAULT now());
      CREATE TABLE drill.items (id bigserial PRIMARY KEY, owner text NOT NULL DEFAULT current_user, body text);
      ALTER TABLE drill.items ENABLE ROW LEVEL SECURITY;
      ALTER TABLE drill.items FORCE ROW LEVEL SECURITY;
      CREATE POLICY items_mine ON drill.items USING (true) WITH CHECK (true);
      CREATE TABLE drill.events (id bigint NOT NULL, day date NOT NULL, PRIMARY KEY (id, day)) PARTITION BY RANGE (day);
      CREATE TABLE drill.events_2026 PARTITION OF drill.events FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
      INSERT INTO drill.events SELECT g, date '2026-06-01' FROM generate_series(1, 1000) g;
      INSERT INTO drill.items (body) SELECT md5(g::text) FROM generate_series(1, 5000) g;
      CREATE MATERIALIZED VIEW drill.item_count AS SELECT count(*) AS n FROM drill.items;
      CREATE FUNCTION drill.bump() RETURNS bigint LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS 'SELECT count(*) FROM drill.items';
      ALTER DEFAULT PRIVILEGES IN SCHEMA drill GRANT SELECT ON TABLES TO ${db}_agent;
      RESET ROLE;
      DO \$\$BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'drill_writer') THEN CREATE ROLE drill_writer LOGIN PASSWORD '$WRITER_PW'; END IF; END\$\$;
      GRANT CONNECT ON DATABASE $db TO drill_writer;
      GRANT USAGE ON SCHEMA drill TO drill_writer;
      GRANT SELECT, INSERT ON drill.ticks TO drill_writer;"
}

# The App stand-in: one insert per ~100 ms as a non-superuser over TCP on
# the database's CURRENT cluster (it follows K2's record, as an App's DSN
# does after its re-host). Each line of the log: <n> <ok|fail> <epoch ms>.
# `async`: every insert commits with synchronous_commit = off (security
# review B2: an acknowledged commit can sit past the WAL write position
# when the write block lands; it must still reach 17). Its n are
# -(100000 + i), apart from the sync writer's.
writer_start() { # writer_start <db> <log> [async]
    local db="$1" log="$2" mode="${3:-sync}"
    WRITER_LOGS+=("$log")
    : >"$log"
    (
        n=0
        while [ ! -f "$log.stop" ]; do
            n=$((n + 1))
            port="$(pg_port "$(db_major "$db" 2>/dev/null || echo 16)")"
            if [ "$mode" = async ]; then
                q="SET synchronous_commit = off; INSERT INTO drill.ticks (n) VALUES (-$((100000 + n)))"
            else
                q="INSERT INTO drill.ticks (n) VALUES ($n)"
            fi
            if PGPASSWORD="$WRITER_PW" psql -X -qtA -h 127.0.0.1 -p "$port" -U drill_writer -d "$db" \
                -c "$q" >/dev/null 2>&1; then
                echo "$n ok $(date +%s%3N)" >>"$log"
            else
                echo "$n fail $(date +%s%3N)" >>"$log"
            fi
            sleep 0.1
        done
    ) &
    echo $! >"$log.pid"
}
writer_stop() { touch "$1.stop"; wait "$(cat "$1.pid")" 2>/dev/null || true; rm -f "$1.stop"; }
# The longest gap between two successful writes, in ms.
max_gap_ms() { awk '$2 == "ok" { if (last) { g = $3 - last; if (g > max) max = g } last = $3 } END { print max + 0 }' "$1"; }
oks() { awk '$2 == "ok"' "$1" | wc -l | tr -d ' '; }

no_dual_objects() { # every cluster: no k2_dual_* slot/publication/subscription, no k2_repl_* role, no run's hba line
    local m
    for m in $(pg_lsclusters -h | awk '$4 == "online" {print $1}'); do
        local left
        left="$(pg_sql "$m" postgres "SELECT (SELECT count(*) FROM pg_replication_slots WHERE slot_name LIKE 'k2\_dual\_%') + (SELECT count(*) FROM pg_roles WHERE rolname LIKE 'k2\_repl\_%') + (SELECT count(*) FROM pg_subscription WHERE subname LIKE 'k2\_dual\_%')")"
        [ "$left" = "0" ] || die "cluster $m still holds $left k2_dual/k2_repl object(s)"
        # Security review A-M2: a run's allow lines go at teardown (the guards stay).
        if sudo grep -Eq '^host [^ ]+ k2_repl_[0-9a-f]{12} ' "/etc/postgresql/$m/main/pg_hba.conf"; then
            die "cluster $m's pg_hba.conf still lets a run's k2_repl_ role in"
        fi
    done
}

start_live() { # start_live <db> [extra flags…]
    local db="$1"; shift
    k2 db upgrade "$db" --live --to 17 "$@" >/dev/null || die "--live $db refused: $(k2 db upgrade "$db" --check --live --to 17 2>&1 | tail -20)"
    wait_for 1800 "$db streams" is_state "$db" streaming
}

# ── drill 2: prepare ────────────────────────────────────────────────────
prepare() {
    [ "$(pg_sql 16 postgres 'SHOW wal_level')" = "replica" ] || die "prepare needs 16 on wal_level = replica (a fresh distro cluster)"
    local a b; a="$(new_db drill_prep_a)"; b="$(new_db drill_prep_b)"
    [ "$(db_major "$a")" = "16" ] || die "the drill databases must be on 16 (k2 db default-major 16 first)"
    local out; out="$(k2 db upgrade "$a" --check --live --to 17 2>&1 || true)"
    grep -q "source_not_logical" <<<"$out" || die "--check --live did not refuse source_not_logical: $out"
    if k2 db upgrade "$a" --live --to 17 >/dev/null 2>&1; then die "--live started on a replica cluster"; fi
    k2 db upgrade --prepare-live 16 --now >/dev/null
    [ "$(pg_sql 16 postgres 'SHOW wal_level')" = "logical" ] || die "16 is not logical after --prepare-live"
    pg_sql 16 "$a" "SELECT 1" >/dev/null && pg_sql 16 "$b" "SELECT 1" >/dev/null || die "a database didn't come back"
    sudo grep -q "host replication /^k2_repl_ all reject" /etc/postgresql/16/main/pg_hba.conf || die "no replication guard line in 16's pg_hba.conf"
    local again; again="$(k2 db upgrade --prepare-live 16 --now --json)"
    [ "$(jget 'v["restarted"]' <<<"$again")" = "False" ] || die "a second --prepare-live restarted again: $again"
    drop_db "$a"; drop_db "$b"
    pass "prepare: refused while replica; one restart made 16 logical; both databases back; guard line in; idempotent"
}

# ── drill 3: the happy path ─────────────────────────────────────────────
happy() {
    local db log alog; db="$(new_db drill_live)"; log="$(mktemp)"; alog="$(mktemp)"
    fixture "$db"
    writer_start "$db" "$log"
    writer_start "$db" "$alog" async
    sleep 5
    start_live "$db"
    say "streaming; writing for ${STREAM_SECS}s"
    sleep "$STREAM_SECS"
    k2 db upgrade "$db" --switch --now >/dev/null || die "--switch failed: $(k2 db upgrade "$db" --status)"
    sleep 10
    writer_stop "$log"
    writer_stop "$alog"
    [ "$(db_major "$db")" = "17" ] || die "the record isn't on 17"
    local gap written awritten on17 aon17; gap="$(max_gap_ms "$log")"; written="$(oks "$log")"; awritten="$(oks "$alog")"
    on17="$(pg_sql 17 "$db" 'SELECT count(*) FROM drill.ticks WHERE n > 0')"
    [ "$on17" = "$written" ] || die "rows lost or extra: the writer wrote $written, 17 holds $on17"
    # B2: every acknowledged synchronous_commit = off insert reached 17.
    aon17="$(pg_sql 17 "$db" 'SELECT count(*) FROM drill.ticks WHERE n < -100000')"
    [ "$aon17" = "$awritten" ] || die "async commits lost at the cut-over: the writer had $awritten acknowledged, 17 holds $aon17"
    local amissing; amissing="$(awk '$2 == "ok" {print -(100000 + $1)}' "$alog" | while read -r n; do [ "$(pg_sql 17 "$db" "SELECT count(*) FROM drill.ticks WHERE n = $n")" = "1" ] || echo "$n"; done | head -5)"
    [ -z "$amissing" ] || die "acknowledged async commits missing on 17: $amissing"
    # Sequences continue without collision (identity and bigserial).
    pg_sql 17 "$db" "SET ROLE ${db}_migrator; INSERT INTO drill.items (body) VALUES ('after'); INSERT INTO drill.ticks (n) VALUES (-1);" >/dev/null || die "a sequence collided on 17"
    [ "$(pg_sql 17 "$db" 'SELECT n FROM drill.item_count')" = "5000" ] || die "the materialized view wasn't refreshed"
    [ "$(pg_sql 17 "$db" "SELECT relforcerowsecurity FROM pg_class WHERE oid = 'drill.items'::regclass")" = "t" ] || die "FORCE RLS isn't back on 17"
    [ "$(pg_sql 16 postgres "SELECT datallowconn FROM pg_database WHERE datname = '$db'")" = "f" ] || die "the 16 copy isn't closed"
    no_dual_objects
    say "write gap: ${gap} ms over ${written} writes"
    sudo grep -qF "host all /^k2_repl_ all reject" /etc/postgresql/16/main/pg_hba.conf || die "the k2_repl_ guard isn't in 16's pg_hba.conf"
    pass "happy: streamed ${STREAM_SECS}s, switched; longest write gap ${gap} ms; no row lost (sync $written, async $awritten); sequences continue; FORCE back; source closed; nothing left"
    echo "$db" >"$log.db"
}

# ── drill 4: security ───────────────────────────────────────────────────
security() {
    local db; db="$(new_db drill_sec)"
    fixture "$db"
    # The H1 plant: owner code that tries to make the owner a superuser
    # whenever it runs as one (a trigger set ENABLE ALWAYS, a CHECK, a
    # generated column), logging who it ran as.
    pg_sql 16 "$db" "
      SET ROLE ${db}_migrator;
      CREATE TABLE drill.h1_log (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), who text, sup boolean, at timestamptz DEFAULT now());
      CREATE FUNCTION drill.h1() RETURNS boolean LANGUAGE plpgsql AS \$\$
      BEGIN
        INSERT INTO drill.h1_log (who, sup) VALUES (current_user, (SELECT rolsuper FROM pg_roles WHERE rolname = current_user));
        IF (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN EXECUTE 'ALTER ROLE ${db}_migrator SUPERUSER'; END IF;
        RETURN true;
      END \$\$;
      -- IMMUTABLE with no argument: CREATE TABLE plans the generated
      -- column and runs it then (the 0.46.4 drill's superuser path).
      CREATE FUNCTION drill.h1_imm() RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT drill.h1()';
      CREATE FUNCTION drill.h1_t() RETURNS trigger LANGUAGE plpgsql AS \$\$ BEGIN PERFORM drill.h1(); RETURN NEW; END \$\$;
      CREATE TABLE drill.planted (id bigint PRIMARY KEY, v int CHECK (drill.h1()), g boolean GENERATED ALWAYS AS (drill.h1_imm()) STORED);
      CREATE TRIGGER planted_t BEFORE INSERT OR UPDATE ON drill.planted FOR EACH ROW EXECUTE FUNCTION drill.h1_t();
      ALTER TABLE drill.planted ENABLE ALWAYS TRIGGER planted_t;
      INSERT INTO drill.planted (id, v) SELECT g, 1 FROM generate_series(1, 50) g;
      GRANT SELECT ON drill.h1_log TO drill_writer;" >/dev/null
    start_live "$db"
    # Apply too: rows written on the source while it streams.
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; INSERT INTO drill.planted (id, v) SELECT g, 1 FROM generate_series(51, 60) g;" >/dev/null
    sleep 10
    for m in 16 17; do
        [ "$(pg_sql "$m" postgres "SELECT rolsuper FROM pg_roles WHERE rolname = '${db}_migrator'")" = "f" ] || die "H1: ${db}_migrator became a superuser on $m"
    done
    [ "$(pg_sql 17 "$db" 'SELECT count(*) FROM drill.h1_log WHERE sup')" = "0" ] || die "H1: owner code ran as a superuser on 17"
    # Physical replication by the replication role: rejected by the guard.
    local id role; id="$(pg_sql 16 postgres "SELECT substr(slot_name, 9) FROM pg_replication_slots WHERE slot_name LIKE 'k2\_dual\_%' LIMIT 1")"
    role="k2_repl_$id"
    local pw; pw="$(pg_sql 17 "$db" "SELECT substring(subconninfo from 'password=([^ ]+)') FROM pg_subscription WHERE subname = 'k2_dual_$id'")"
    [ -n "$pw" ] || die "no subscription conninfo to read the password from"
    if PGPASSWORD="$pw" timeout 30 pg_basebackup -h 127.0.0.1 -p "$(pg_port 16)" -U "$role" -D "$(mktemp -d)" -X none >/dev/null 2>&1; then
        die "pg_basebackup as $role succeeded: the hba guard didn't hold"
    fi
    # Security review A-M2: the role reaches its own database (the run's
    # allow line) and nothing else: another tenant's database (PUBLIC has
    # CONNECT there) and `postgres`, plainly or as a logical walsender, are
    # refused by pg_hba before any password check.
    local other; other="$(new_db drill_sec_other)"
    PGPASSWORD="$pw" psql -X -qtA -h 127.0.0.1 -p "$(pg_port 16)" -U "$role" -d "$db" -c "SELECT 1" >/dev/null 2>&1 \
        || die "$role can't reach its own database: the drill would prove nothing"
    local tgt out2
    for tgt in "$other" postgres; do
        out2="$(PGPASSWORD="$pw" psql -X -qtA -h 127.0.0.1 -p "$(pg_port 16)" -U "$role" -d "$tgt" -c "SELECT 1" 2>&1 || true)"
        grep -q "pg_hba.conf rejects connection" <<<"$out2" || die "$role reached $tgt: $out2"
        out2="$(PGPASSWORD="$pw" psql -X -qtA "host=127.0.0.1 port=$(pg_port 16) user=$role dbname=$tgt replication=database" -c "IDENTIFY_SYSTEM" 2>&1 || true)"
        grep -q "pg_hba.conf rejects" <<<"$out2" || die "$role opened a logical replication connection to $tgt: $out2"
        out2="$(PGPASSWORD="$pw" psql -X -qtA "host=::1 port=$(pg_port 16) user=$role dbname=$tgt" -c "SELECT 1" 2>&1 || true)"
        grep -q "pg_hba.conf rejects\|could not connect\|Connection refused" <<<"$out2" || die "$role reached $tgt over ::1: $out2"
    done
    drop_db "$other"
    # A-L2: the run refuses names it would later drop. Abort later checks
    # the source has no _k2_dual schema left (K2's, dropped by name).
    [ "$(pg_sql 16 "$db" "SELECT count(*) FROM pg_namespace WHERE nspname = '_k2_dual'")" = "1" ] || die "the freeze schema isn't there while streaming"
    # A migrator can't create a subscription.
    local m_out; m_out="$(pg_sql 16 "$db" "SET ROLE ${db}_migrator; CREATE SUBSCRIPTION evil CONNECTION 'host=127.0.0.1 dbname=$db' PUBLICATION x WITH (connect = false)" 2>&1 || true)"
    # Refused either way: the run's DDL freeze answers first while it
    # streams; without it, Postgres's own privilege check does.
    grep -qi "k2_dual_ddl_frozen\|permission denied\|must be superuser\|must have privileges" <<<"$m_out" || die "a migrator's CREATE SUBSCRIPTION wasn't refused: $m_out"
    # The password: in no log, argv or vault file.
    sudo journalctl -u "$UNIT" --since "-2h" --no-pager -o cat | grep -qF "$pw" && die "the replication password is in the daemon journal"
    sudo journalctl -u "postgresql@16-main" -u "postgresql@17-main" --since "-2h" --no-pager -o cat | grep -qF "$pw" && die "the replication password is in a Postgres log"
    ps -eo args | grep -F "$pw" | grep -vq grep && die "the replication password is in a process's argv"
    sudo grep -rqF "$pw" /var/log/k2-root-helper* /home/*/.k2/vault 2>/dev/null && die "the replication password is in the helper's audit or the vault"
    k2 db upgrade "$db" --abort >/dev/null
    no_dual_objects
    drop_db "$db"
    pass "security: H1 ran as the owner (first copy and apply); pg_basebackup refused; migrator CREATE SUBSCRIPTION refused; password in no log, argv or vault"
}

# ── drill 5: the DDL freeze ─────────────────────────────────────────────
freeze() {
    local db; db="$(new_db drill_freeze)"
    fixture "$db"
    start_live "$db"
    mkdir -p "$WS/.k2/db/migrations"
    printf 'CREATE TABLE drill.later (id int PRIMARY KEY);\n' >"$WS/.k2/db/migrations/9999_drill_freeze.sql"
    local out; out="$(k2 db migrate 2>&1 || true)"
    grep -q "db_dual_running" <<<"$out" || die "k2 db migrate ran during a live move: $out"
    local ddl; ddl="$(pg_sql 16 "$db" "SET ROLE ${db}_migrator; ALTER TABLE drill.ticks ADD COLUMN x int" 2>&1 || true)"
    grep -q "k2_dual_ddl_frozen" <<<"$ddl" || die "the App's own DDL wasn't frozen: $ddl"
    k2 db upgrade "$db" --abort >/dev/null
    [ "$(live_state "$db")" = "aborted" ] || die "the run isn't aborted"
    k2 db migrate >/dev/null || die "migrate still refused after --abort"
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; ALTER TABLE drill.ticks ADD COLUMN y int" >/dev/null || die "DDL still frozen after --abort"
    rm -f "$WS/.k2/db/migrations/9999_drill_freeze.sql"
    no_dual_objects
    drop_db "$db"
    pass "freeze: migrate → db_dual_running and App DDL → k2_dual_ddl_frozen during the run; both work after --abort"
}

# ── drill 6: failures ───────────────────────────────────────────────────
failures() {
    local db out
    # A table without a primary key: refused before anything is published;
    # the owner's --replica-identity-full then passes.
    db="$(new_db drill_nopk)"
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; CREATE TABLE public.log (msg text); INSERT INTO public.log VALUES ('a');" >/dev/null
    out="$(k2 db upgrade "$db" --live --to 17 2>&1 || true)"
    grep -q "no_primary_key" <<<"$out" || die "a table without a key wasn't refused: $out"
    [ "$(pg_sql 16 "$db" "SELECT count(*) FROM pg_publication")" = "0" ] || die "a publication exists after the refusal"
    start_live "$db" --replica-identity-full
    [ "$(pg_sql 16 "$db" "SELECT relreplident FROM pg_class WHERE oid = 'public.log'::regclass")" = "f" ] || die "REPLICA IDENTITY FULL wasn't set"
    k2 db upgrade "$db" --abort >/dev/null
    no_dual_objects
    drop_db "$db"
    pass "failures/no-pk: refused, nothing published; --replica-identity-full passes; abort cleans"

    # Security review A-L2: a tenant's own `_k2_dual` schema: refused at
    # setup, never dropped by a teardown.
    db="$(new_db drill_reserved)"; fixture "$db"
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; CREATE SCHEMA _k2_dual; CREATE TABLE _k2_dual.mine (id int PRIMARY KEY); INSERT INTO _k2_dual.mine VALUES (1);" >/dev/null
    out="$(k2 db upgrade "$db" --live --to 17 2>&1 || true)"
    grep -q "reserved_names" <<<"$out" || die "a tenant _k2_dual schema wasn't refused: $out"
    wait_for 120 "the refused run ends" not_state "$db" setting_up
    [ "$(pg_sql 16 "$db" "SELECT count(*) FROM _k2_dual.mine")" = "1" ] || die "the tenant's _k2_dual schema was touched"
    [ "$(pg_sql 16 "$db" "SELECT count(*) FROM pg_publication")" = "0" ] || die "a publication exists after the refusal"
    no_dual_objects
    drop_db "$db"
    pass "failures/reserved-names: refused; the tenant's schema kept; nothing left"

    # Security review B3: a large object made while streaming (lo_create
    # isn't DDL): the switch is undone under the write block; abort cleans.
    db="$(new_db drill_lob)"; fixture "$db"
    start_live "$db"
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; SELECT lo_from_bytea(0, 'drill')" >/dev/null
    k2 db upgrade "$db" --switch --now >/dev/null 2>&1 || true
    wait_for 300 "the switch is undone" is_state "$db" streaming
    [ "$(db_major "$db")" = "16" ] || die "the record moved with a large object left behind"
    k2 db upgrade "$db" --status --json | grep -q "large_objects" || die "the run doesn't say large_objects: $(k2 db upgrade "$db" --status)"
    k2 db upgrade "$db" --abort >/dev/null
    no_dual_objects
    drop_db "$db"
    pass "failures/large-object: the switch undid itself and said why; abort cleaned"

    # The target cluster killed mid-sync: stalled (or failed), --abort
    # cleans, the source unaffected.
    db="$(new_db drill_kill)"; fixture "$db"
    pg_sql 16 "$db" "INSERT INTO drill.items (body) SELECT repeat(md5(g::text), 40) FROM generate_series(1, 300000) g" >/dev/null
    k2 db upgrade "$db" --live --to 17 >/dev/null
    sleep 3
    sudo pg_ctlcluster 17 main stop -m immediate
    sleep 70
    sudo pg_ctlcluster 17 main start
    wait_for 300 "the run notices (stalled or syncing again)" not_state "$db" setting_up
    k2 db upgrade "$db" --abort >/dev/null || true
    [ "$(pg_sql 16 "$db" 'SELECT count(*) FROM drill.items')" -ge 300000 ] || die "the source lost rows"
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; ALTER TABLE drill.items ADD COLUMN z int" >/dev/null || die "the source is still frozen after --abort"
    no_dual_objects
    drop_db "$db"
    pass "failures/target-killed: the run ended, --abort cleaned, the source kept every row and its DDL"

    # The WAL cap: a tiny max_slot_wal_keep_size and a stopped subscriber →
    # the slot is lost → the run fails and cleans; the source is fine.
    db="$(new_db drill_wal)"; fixture "$db"
    start_live "$db"
    # ALTER SYSTEM can't run in a transaction block: one statement per call.
    pg_sql 16 postgres "ALTER SYSTEM SET max_slot_wal_keep_size = '64MB'" >/dev/null
    pg_sql 16 postgres "SELECT pg_reload_conf()" >/dev/null
    sudo pg_ctlcluster 17 main stop -m fast
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; CREATE TEMP TABLE IF NOT EXISTS t AS SELECT 1" >/dev/null 2>&1 || true
    for _ in 1 2 3 4 5 6; do pg_sql 16 "$db" "INSERT INTO drill.ticks (n) SELECT g FROM generate_series((SELECT coalesce(max(n),0)+1 FROM drill.ticks), (SELECT coalesce(max(n),0)+200000 FROM drill.ticks)) g" >/dev/null; pg_sql 16 postgres "CHECKPOINT" >/dev/null; done
    sudo pg_ctlcluster 17 main start
    wait_for 300 "the run fails on the lost slot" is_state "$db" failed
    pg_sql 16 postgres "ALTER SYSTEM RESET max_slot_wal_keep_size" >/dev/null
    pg_sql 16 postgres "SELECT pg_reload_conf()" >/dev/null
    pg_sql 16 "$db" "SELECT count(*) FROM drill.ticks" >/dev/null || die "the source doesn't answer"
    no_dual_objects
    drop_db "$db"
    pass "failures/wal-cap: the lost slot failed the run and cleaned it; the source is fine"

    # Daemon restarts: while streaming (keeps streaming), during the
    # switch's notice (undone, streams on).
    db="$(new_db drill_restart)"; fixture "$db"
    start_live "$db"
    restart_daemon
    sleep 40
    is_state "$db" streaming || die "the run stopped streaming across a daemon restart ($(live_state "$db"))"
    k2 db upgrade "$db" --switch --no-wait >/dev/null
    sleep 20
    restart_daemon
    wait_for 300 "the interrupted switch is undone" is_state "$db" streaming
    [ "$(db_major "$db")" = "16" ] || die "the record moved during an undone switch"
    pg_sql 16 postgres "SELECT datconnlimit FROM pg_database WHERE datname = '$db'" | grep -qx -- "-1" || die "the source wasn't re-opened"
    # The walsender killed during the write block: the cut-over is undone.
    k2 db upgrade "$db" --switch --no-wait >/dev/null
    local end=$((SECONDS + 90))
    while [ "$SECONDS" -lt "$end" ]; do
        if [ "$(pg_sql 16 postgres "SELECT datconnlimit FROM pg_database WHERE datname = '$db'")" = "0" ]; then
            pg_sql 16 postgres "SELECT pg_terminate_backend(pid) FROM pg_stat_replication WHERE usename LIKE 'k2\_repl\_%'" >/dev/null
            break
        fi
        sleep 0.2
    done
    # Two right outcomes: the drop lands before the drain caught up (the
    # cut-over is undone, the run streams on, on 16), or the target had
    # already caught up (the switch completes and passed its verify, on 17).
    wait_for 300 "the cut-over settles after the walsender drop" streaming_or_switched "$db"
    local outcome
    if is_state "$db" switched; then
        [ "$(db_major "$db")" = "17" ] || die "the run says switched but the record isn't on 17"
        outcome="switched and verified (the target had caught up)"
    else
        [ "$(db_major "$db")" = "16" ] || die "the record moved after an undone cut-over"
        k2 db upgrade "$db" --abort >/dev/null
        outcome="undone, streamed on, aborted"
    fi
    no_dual_objects
    drop_db "$db"
    pass "failures/restarts: streams across a restart; a restart in the switch undoes it; a walsender drop in the write block: $outcome"
}

# ── drill 7: rollback after a live switch ───────────────────────────────
rollback() {
    local db; db="$(new_db drill_rb)"; fixture "$db"
    start_live "$db"
    k2 db upgrade "$db" --switch --now >/dev/null
    [ "$(db_major "$db")" = "17" ] || die "not on 17 after the switch"
    pg_sql 17 "$db" "INSERT INTO drill.ticks (n) VALUES (424242)" >/dev/null
    k2 db upgrade "$db" --rollback --yes --now >/dev/null
    [ "$(db_major "$db")" = "16" ] || die "not back on 16 after --rollback"
    [ "$(pg_sql 16 "$db" 'SELECT count(*) FROM drill.ticks WHERE n = 424242')" = "0" ] || die "the 17 write came back (it must not)"
    k2 db upgrade "$db" --forget-old --force-now >/dev/null || true
    drop_db "$db"
    pass "rollback: back on 16, the write made on 17 not carried back (as 0.46.3)"
}

# ── drill 8: a big table past the 60 s statement_timeout ────────────────
statement-timeout() {
    local db; db="$(new_db drill_big)"
    pg_sql 16 "$db" "SET ROLE ${db}_migrator; CREATE TABLE public.big (id bigint PRIMARY KEY, pad text);
      INSERT INTO public.big SELECT g, repeat('x', 1000) FROM generate_series(1, ${BIG_GB} * 1000000) g;" >/dev/null
    say "the big table: $(pg_sql 16 "$db" "SELECT pg_size_pretty(pg_total_relation_size('public.big'))")"
    pg_sql 16 postgres "SHOW statement_timeout" | grep -q "1min\|60s" || say "note: the cluster's statement_timeout is $(pg_sql 16 postgres 'SHOW statement_timeout')"
    start_live "$db"
    [ "$(pg_sql 17 "$db" 'SELECT count(*) FROM public.big')" = "$((BIG_GB * 1000000))" ] || die "the big table didn't copy whole"
    k2 db upgrade "$db" --abort >/dev/null
    no_dual_objects
    drop_db "$db"
    pass "statement-timeout: a ${BIG_GB} GB table copied past the cluster's 60 s statement_timeout"
}

# ── drill 1: per-App majors ─────────────────────────────────────────────
seed_dir() { # seed_dir <dir> <app-id> <requires.postgres>
    mkdir -p "$1"
    cat >"$1/manifest.json" <<EOF
{"schema":1,"kind":"app","id":"$2","name":"$2","version":"1.0.0",
 "requires":{"k2":">=0.46.3","postgres":"$3"},
 "host":{"mode":"cmd","start":["sh","-c","exec python3 -m http.server \\"\$PORT\\""],"health":{"path":"/"}}}
EOF
    echo ok >"$1/index.html"
}
per-app() {
    [ -n "$WS2" ] && [ -d "$WS2" ] || die "per-app needs K2DR_WS2 (a second workspace with no database)"
    local tmp; tmp="$(mktemp -d)"
    # No database + requires >=17 → created on 17 with its DATABASE_URL ref.
    seed_dir "$tmp/a" "drill-pg17" ">=17"
    local v; v="$(k2in "$WS2" app install drill-pg17 --folder "$tmp/a" --engine process --json)"
    [ "$(jget 'v["database"]["major"]' <<<"$v")" = "17" ] || die "install didn't create the database on 17: $v"
    [ -n "$(jget 'v["database"]["ref"]' <<<"$v")" ] || die "no DATABASE_URL reference: $v"
    # "16" on that 17 database → refused, nothing installed.
    seed_dir "$tmp/b" "drill-pg16" "16"
    local o; o="$(k2in "$WS2" app install drill-pg16 --folder "$tmp/b" --engine process 2>&1 || true)"
    grep -q "requires_unmet" <<<"$o" || die "an App needing 16 installed on a 17 database: $o"
    # A 16 database used by two Apps, one "16" → the move refused naming it.
    local db; db="$(new_db drill_shared)"
    seed_dir "$tmp/c" "drill-any" ">=16"; seed_dir "$tmp/d" "drill-old" "16"
    k2 app install drill-any --folder "$tmp/c" --engine process --pg-major 16 >/dev/null
    k2 app install drill-old --folder "$tmp/d" --engine process --pg-major 16 >/dev/null
    o="$(k2 db upgrade --app drill-any --check --to 17 2>&1 || true)"
    grep -q "app_requires_major" <<<"$o" && grep -q "drill-old" <<<"$o" || die "the move wasn't refused naming drill-old: $o"
    # --app moves only the App's database when the workspace has two.
    local before; before="$(db_major "$db")"
    k2 db upgrade --app drill-any --check >/dev/null 2>&1 || true
    [ "$(db_major "$db")" = "$before" ] || die "--check moved a database"
    for a in drill-pg17 drill-any drill-old; do k2 app uninstall "$a" >/dev/null 2>&1 || k2in "$WS2" app uninstall "$a" >/dev/null 2>&1 || true; done
    drop_db "$db"
    pass "per-app: created on 17 with its ref; '16' refused on 17; a shared 16 database's move refused naming drill-old"
}

case "${1:-}" in
    list) printf '%s\n' "${ALL[@]}"; exit 0 ;;
esac
CHOSEN=("$@")
[ "${#CHOSEN[@]}" -gt 0 ] || CHOSEN=("${ALL[@]}")
for s in "${CHOSEN[@]}"; do
    say "── $s"
    "$s"
done
say "all chosen scenarios passed: ${CHOSEN[*]}"
