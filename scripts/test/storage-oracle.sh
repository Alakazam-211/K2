#!/usr/bin/env bash
# The /storage/v1 oracle diff (prd-app-supabase-compat-v1 §12, lane D): a
# real Supabase Storage (storage-api, file backend, on its own Postgres) beside
# K2's door, the same requests to both, every difference printed. A test
# oracle only: never shipped, never run on a box. Runs on a Linux compute node
# as an ordinary user, never on the M5:
#
#   k2 compute run --sha <c> --stall-timeout off --mem 28G \
#     --needs rust@1.99.1,cmake@3,cargo-nextest,node@26 \
#     -- bash scripts/test/storage-oracle.sh
#
# Env: REALPG_BIN (Postgres binaries; found under /usr/lib/postgresql when
# unset), STORAGE_API_REF (default v1.98.5), CARGO_TARGET_DIR (the cache).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
REF="${STORAGE_API_REF:-v1.98.5}"
REALPG_BIN="${REALPG_BIN:-$(bash "$ROOT/scripts/test/pg-binaries.sh")}"
[ -x "$REALPG_BIN/initdb" ] || { echo "storage-oracle: no Postgres binaries (REALPG_BIN=$REALPG_BIN)" >&2; exit 1; }
command -v node >/dev/null || { echo "storage-oracle: no node (--needs node@26)" >&2; exit 1; }
echo "postgres: $("$REALPG_BIN/postgres" --version); node: $(node --version); storage-api: $REF"

WORK="$(mktemp -d /tmp/k2so-XXXXXX)"
PGPORT=55441
API_PORT=55442
SECRET="oracle-jwt-secret-0123456789abcdef0123456789"
cleanup() {
    [ -n "${API_PID:-}" ] && kill "$API_PID" 2>/dev/null || true
    "$REALPG_BIN/pg_ctl" -D "$WORK/pg" -m immediate -w stop >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# The oracle's own Postgres (trust on loopback; storage-api's password is ignored).
"$REALPG_BIN/initdb" -D "$WORK/pg" -U postgres --auth=trust -E UTF8 >/dev/null
mkdir -p "$WORK/sock"
"$REALPG_BIN/pg_ctl" -D "$WORK/pg" -o "-p $PGPORT -k $WORK/sock -c listen_addresses=127.0.0.1" -l "$WORK/pg.log" -w start >/dev/null

# storage-api at a pinned release, built from source.
git clone -q --depth 1 --branch "$REF" https://github.com/supabase/storage.git "$WORK/storage"
# storage-api asks for npm 12 (its .npmrc makes the engines check strict).
NPM="npx -y npm@12"
( cd "$WORK/storage" && $NPM ci --ignore-scripts --no-audit --no-fund >/dev/null && $NPM rebuild >/dev/null && $NPM run build >/dev/null )
mkdir -p "$WORK/files"
(
    cd "$WORK/storage"
    env DATABASE_URL="postgresql://postgres:postgres@127.0.0.1:$PGPORT/postgres" \
        AUTH_JWT_SECRET="$SECRET" AUTH_JWT_ALGORITHM=HS256 \
        DB_INSTALL_ROLES=true DB_ANON_ROLE=anon DB_SERVICE_ROLE=service_role DB_AUTHENTICATED_ROLE=authenticated \
        DB_SUPER_USER=postgres DB_ALLOW_MIGRATION_REFRESH=true \
        STORAGE_BACKEND=file STORAGE_FILE_BACKEND_PATH="$WORK/files" STORAGE_FILE_ETAG_ALGORITHM=md5 \
        STORAGE_S3_BUCKET=oracle TENANT_ID=oracle SERVER_REGION=local REGION=local GLOBAL_S3_BUCKET=oracle \
        SERVER_HOST=127.0.0.1 SERVER_PORT="$API_PORT" SERVER_ADMIN_PORT=$((API_PORT + 1)) \
        UPLOAD_FILE_SIZE_LIMIT=524288000 UPLOAD_FILE_SIZE_LIMIT_STANDARD=52428800 \
        IMAGE_TRANSFORMATION_ENABLED=false NODE_ENV=production \
        node dist/start/server.js >"$WORK/api.log" 2>&1
) &
API_PID=$!
for _ in $(seq 1 120); do
    if curl -fsS "http://127.0.0.1:$API_PORT/status" >/dev/null 2>&1; then break; fi
    if ! kill -0 "$API_PID" 2>/dev/null; then echo "storage-api exited:" >&2; tail -50 "$WORK/api.log" >&2; exit 1; fi
    sleep 1
done
curl -fsS "http://127.0.0.1:$API_PORT/status" >/dev/null || { echo "storage-api never came up:" >&2; tail -50 "$WORK/api.log" >&2; exit 1; }
echo "storage-api up on :$API_PORT"

cd "$ROOT"
set +e
env ORACLE_URL="http://127.0.0.1:$API_PORT" \
    ORACLE_DB_URL="postgresql://postgres:postgres@127.0.0.1:$PGPORT/postgres" \
    ORACLE_JWT_SECRET="$SECRET" REALPG_STORAGE=1 REALPG_BIN="$REALPG_BIN" \
    cargo test -p k2-daemon --lib compat::storage_oracle -- --ignored --nocapture --test-threads=1 2>&1 | tee "$WORK/test.log"
rc=${PIPESTATUS[0]}
set -e
grep -q "oracle: [0-9]* steps" "$WORK/test.log" || { echo "storage-oracle: the oracle test never ran" >&2; tail -40 "$WORK/api.log" >&2; exit 1; }
[ "$rc" = 0 ] || { echo "--- storage-api log (tail) ---"; tail -40 "$WORK/api.log"; }
exit "$rc"
