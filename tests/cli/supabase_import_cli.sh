#!/usr/bin/env bash
# prd-app-supabase-compat-v1 S2: the CLI half of `k2 db import supabase` and
# `k2 app user import`: usage errors send nothing; the dump's report prints
# (also on a refusal, before the error); exit 3 for an unresolved reference;
# the users file rides in the request body; clashes exit 1 and no hash is
# ever printed. A local fake daemon stands in and logs each request. No real
# daemon, no ~/.k2. Synthetic users only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-supa-imp-XXXXXX)"
FAKE_PID=""
cleanup() {
    if [ -n "$FAKE_PID" ]; then kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; fi
    rm -rf "$FAKE_DIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$FAKE_DIR/fake.py" <<'PY'
import http.server, json, sys, urllib.parse
log_path, port_path = sys.argv[1], sys.argv[2]
REPORT = {"ok": True, "db": "supa", "dryRun": True,
          "dropped": {"schemas": {"auth": 12, "realtime": 3}, "extensions": ["pg_graphql"]},
          "carried": {"extensions": ["pgcrypto"], "tables": 4, "functions": 2, "policies": 6, "onSupabaseTables": ["trigger on auth.users on_auth_user_created (calls public.handle_new_user)"]},
          "grants": {"kept": 9, "droppedSupabase": 4, "droppedRoles": ["app_reporter"]},
          "forceLifted": ["public.notes"], "authUsers": 3, "storage": {"buckets": 1, "objects": 2},
          "columnsNotCarried": ["storage.objects.level"],
          "migrations": {"count": 2, "mapFile": ".k2/db/supabase-migrations.json", "map": [{"supabase": "20240101120000", "name": "init", "renamed": "0001_init"}]},
          "unresolved": [], "notes": ["publication supabase_realtime: Realtime isn't carried; dropped"]}
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        u = urllib.parse.urlsplit(self.path)
        with open(log_path, "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path, body.replace("\n", "\\n")))
        b = json.loads(body or "{}") if body.startswith("{") else {}
        status, out = 200, {"ok": True}
        if u.path == "/cli/db/import/supabase":
            if b.get("db") == "unres":
                r = dict(REPORT, ok=False, unresolved=[{"object": "FUNCTION public.read_secret(text)", "reference": "vault.decrypted_secrets"}])
                status, out = 409, {"ok": False, "error": {"code": "import_unresolved", "hint": "1 thing(s) in the dump refer to what K2 doesn't carry"}, "report": r}
            else:
                out = dict(REPORT, dryRun=bool(b.get("dryRun")))
        elif u.path == "/cli/app/user/import":
            clash = b.get("app") == "clash"
            rep = {"app": b.get("app"), "apply": b.get("apply"), "written": bool(b.get("apply")) and not clash, "total": 2, "new": 2,
                   "updated": 0, "unchanged": 0, "keptK2Password": 0, "passwordless": 0, "disabled": 0,
                   "derivedUsernames": [{"id": "0a1d0000-0000-4000-8000-00000000a0da", "username": "ada"}],
                   "clashes": [{"line": 2, "id": "x", "email": "dup@example.com", "reason": "email_taken: another login on this box has this email"}] if clash else []}
            if clash and b.get("apply"):
                rep["error"] = {"code": "import_clashes", "hint": "1 line(s) clash: nothing was written"}
                status = 409
            out = rep
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
open(port_path, "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$FAKE_DIR/fake.py" "$FAKE_DIR/requests.log" "$FAKE_DIR/port" &
FAKE_PID=$!
for _ in $(seq 1 50); do [ -s "$FAKE_DIR/port" ] && break; sleep 0.1; done
[ -s "$FAKE_DIR/port" ] || fail "fake daemon did not start"
export K2_PORT="$(cat "$FAKE_DIR/port")"
export K2_HOOK_TOKEN=owner-token
cd "$FAKE_DIR"

run() {
    set +e
    OUT="$("$CLI" "$@" 2>"$FAKE_DIR/stderr")"
    RC=$?
    ERR="$(cat "$FAKE_DIR/stderr")"
    set -e
}
last_req() { tail -n 1 "$FAKE_DIR/requests.log"; }

# ── k2 db import supabase ────────────────────────────────────────────
: >"$FAKE_DIR/requests.log"
run db import mysql x.dump --db supa
[ "$RC" -eq 2 ] || fail "k2 db import <not supabase> is usage (2), got $RC: $ERR"
run db import supabase x.dump
[ "$RC" -eq 2 ] || fail "k2 db import supabase without --db is usage, got $RC"
grep -q '"code":"usage"' <<<"$ERR" || fail "says usage: $ERR"
run db import supabase --db supa
[ "$RC" -eq 2 ] || fail "k2 db import supabase without a dump is usage, got $RC"
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"

# covers: db import supabase
run db import supabase .k2/db/dumps/s.dump --db supa --dry-run
[ "$RC" -eq 0 ] || fail "a dry run exits 0, got $RC: $ERR"
last_req | grep -q '^POST /cli/db/import/supabase ' || fail "posts the import: $(last_req)"
last_req | grep -q '"file":".k2/db/dumps/s.dump"' && last_req | grep -q '"db":"supa"' && last_req | grep -q '"dryRun":true' || fail "sends file, db, dryRun: $(last_req)"
last_req | grep -q 'allowUnresolved' && fail "--allow-unresolved only when given: $(last_req)"
grep -q '^would import into supa' <<<"$OUT" || fail "dry run heading: $OUT"
grep -q '^warning:    FORCE ROW LEVEL SECURITY lifted on public.notes' <<<"$OUT" || fail "FORCE warning: $OUT"
grep -q '^warning:    grants to app_reporter dropped' <<<"$OUT" || fail "dropped role named: $OUT"
grep -q '^migrations: 2 renamed (20240101120000 -> 0001_init' <<<"$OUT" || fail "the migration map: $OUT"
grep -q '^kept:       trigger on auth.users' <<<"$OUT" || fail "kept trigger: $OUT"

run db import supabase .k2/db/dumps/s.dump --db unres --allow-unresolved
grep -q '"allowUnresolved":true' <<<"$(last_req)" || fail "sends allowUnresolved: $(last_req)"
[ "$RC" -eq 3 ] || fail "an unresolved reference is refused (3), got $RC: $ERR"
grep -q '^unresolved: FUNCTION public.read_secret(text) -> vault.decrypted_secrets' <<<"$OUT" || fail "the report prints before the error: $OUT"
grep -q 'import_unresolved' <<<"$ERR" || fail "names the refusal: $ERR"

# covers: db import supabase --json
run db import supabase .k2/db/dumps/s.dump --db supa --json
[ "$RC" -eq 0 ] || fail "--json exits 0"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert v["db"] == "supa" and v["authUsers"] == 3' "$OUT" || fail "--json is the report: $OUT"

# ── k2 app user import ───────────────────────────────────────────────
H='$2a$08$abcdefghijklmnopqrstuuABCDEFGHIJKLMNOPQRSTUVWXYZ012345'
printf '%s\n' "{\"id\":\"0a1d0000-0000-4000-8000-00000000a0da\",\"email\":\"ada@example.com\",\"encrypted_password\":\"$H\"}" >"$FAKE_DIR/users.jsonl"
: >"$FAKE_DIR/requests.log"
run app user import
[ "$RC" -eq 1 ] || fail "k2 app user import without args is usage (1), got $RC: $ERR"
run app user import feedback
[ "$RC" -eq 1 ] || fail "k2 app user import without --file is usage, got $RC"
run app user import feedback --file nope.jsonl
[ "$RC" -eq 1 ] || fail "an unreadable file is usage, got $RC"
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"

# covers: app user import
run app user import feedback --file users.jsonl
[ "$RC" -eq 0 ] || fail "a dry run exits 0, got $RC: $ERR"
last_req | grep -q '^POST /cli/app/user/import ' || fail "posts the import: $(last_req)"
last_req | grep -q '"apply": false' || fail "a dry run by default: $(last_req)"
last_req | grep -q 'ada@example.com' || fail "the file rides in the body: $(last_req)"
grep -q 'dry run, nothing written' <<<"$OUT" || fail "says dry run: $OUT"
grep -q 'username ada for 0a1d0000' <<<"$OUT" || fail "derived usernames: $OUT"
case "$OUT$ERR" in *'$2a$'*) fail "a hash was printed: $OUT $ERR" ;; esac

run app user import feedback --file users.jsonl --apply --role member
last_req | grep -q '"apply": true' && last_req | grep -q '"role": "member"' || fail "sends apply and role: $(last_req)"
[ "$RC" -eq 0 ] || fail "an apply exits 0, got $RC"
grep -q 'App users for feedback: imported' <<<"$OUT" || fail "says imported: $OUT"

run app user import clash --file users.jsonl --apply
[ "$RC" -eq 1 ] || fail "clashes exit 1, got $RC: $OUT $ERR"
grep -q 'refused, nothing written' <<<"$OUT" || fail "says refused: $OUT"
grep -q 'clash line 2: email_taken' <<<"$OUT" || fail "lists the clash: $OUT"

# covers: app user import --json
run app user import clash --file users.jsonl --json
[ "$RC" -eq 1 ] || fail "--json with clashes exits 1, got $RC"

# ── k2 app storage: usage errors (0.46.18) ───────────────────────────
# The compat and cron shape: {"error":"usage","hint":…} on stderr, exit 1,
# nothing sent. A value flag with no value says so (exit 2), and
# --flag=value is the same as --flag value.
usage_json() {
    python3 -c 'import json,sys
v = json.loads(sys.argv[1])
assert v["error"] == "usage", v
assert sys.argv[2] in v["hint"], (sys.argv[2], v["hint"])' "$ERR" "$1"
}
: >"$FAKE_DIR/requests.log"
for row in "frobnicate shop|k2 app storage status|enable" "status|name the App: k2 app storage status" \
    "buckets shop --frob|unknown flag --frob" "ls shop|k2 app storage ls <app-id> <bucket>" \
    "import shop --from https://acme.supabase.co|k2 app storage import <app-id> --from" \
    "bucket create shop|k2 app storage bucket create <app-id> <bucket>" "key spin shop|key rotate <app-id> --yes" \
    "verify|name the App: k2 app storage verify"; do
    args="${row%%|*}"; want="${row#*|}"
    # shellcheck disable=SC2086
    run app storage $args
    [ "$RC" -eq 1 ] || fail "k2 app storage $args is a usage error (1), got $RC: $ERR"
    [ -z "$OUT" ] || fail "k2 app storage $args prints nothing on stdout: $OUT"
    usage_json "$want" || fail "k2 app storage $args: the usage JSON naming '$want': $ERR"
done
for row in "import shop --key-secret SUPA --from|--from" "import shop --from https://acme.supabase.co --key-secret|--key-secret" \
    "import shop --from https://acme.supabase.co --key-secret SUPA --bucket|--bucket" \
    "import shop --from https://acme.supabase.co --key-secret SUPA --concurrency|--concurrency" \
    "verify shop --from|--from" "verify shop --from https://acme.supabase.co --key-secret|--key-secret" \
    "bucket create shop logos --file-size-limit|--file-size-limit" "bucket update shop logos --mime|--mime" "ls shop logos --limit|--limit"; do
    args="${row%%|*}"; flag="${row#*|}"
    # shellcheck disable=SC2086
    run app storage $args
    [ "$RC" -eq 2 ] || fail "k2 app storage $args: a flag with no value is refused (2), got $RC: $ERR"
    [ "$ERR" = "Error: $flag takes a value" ] || fail "k2 app storage $args says $flag takes a value: $ERR"
done
# A --concurrency that isn't a whole number from 1 is refused before
# anything is built or sent (it used to fail inside the body and post an
# empty one).
for n in eight 2.5 0 -3; do
    run app storage import shop --from https://acme.supabase.co --key-secret SUPA --concurrency "$n"
    [ "$RC" -eq 1 ] || fail "--concurrency $n is a usage error (1), got $RC: $ERR"
    usage_json "--concurrency takes a whole number from 1" || fail "--concurrency $n: the usage JSON: $ERR"
    case "$ERR" in *Traceback*) fail "--concurrency $n: no traceback: $ERR" ;; esac
done
[ ! -s "$FAKE_DIR/requests.log" ] || fail "storage usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"
run app storage import shop --from=https://acme.supabase.co --key-secret=SUPA --bucket=logos --bucket avatars --concurrency=4
[ "$RC" -eq 0 ] || fail "k2 app storage import with --flag=value exits 0, got $RC: $ERR"
last_req | grep -q '^POST /cli/app/storage/import ' || fail "posts the storage import: $(last_req)"
python3 -c 'import json,sys
b = json.loads(sys.argv[1].split(" ", 2)[2])
assert b == {"app": "shop", "from": "https://acme.supabase.co", "keySecret": "SUPA", "buckets": ["logos", "avatars"], "concurrency": 4}, b' "$(last_req)" \
    || fail "the storage import body: $(last_req)"
run app storage bucket create shop logos --public --file-size-limit=50MB --mime=image/png --mime image/jpeg
[ "$RC" -eq 0 ] || fail "k2 app storage bucket create with --flag=value exits 0, got $RC: $ERR"
python3 -c 'import json,sys
b = json.loads(sys.argv[1].split(" ", 2)[2])
assert b == {"app": "shop", "op": "create", "id": "logos", "public": True, "fileSizeLimit": "50MB", "allowedMimeTypes": ["image/png", "image/jpeg"]}, b' "$(last_req)" \
    || fail "the bucket body: $(last_req)"

# ── k2 app identity issuer --help prints the identity help ───────────
: >"$FAKE_DIR/requests.log"
for h in --help -h help; do
    run app identity issuer "$h"
    [ "$RC" -eq 0 ] || fail "k2 app identity issuer $h exits 0, got $RC: $ERR"
    grep -qF 'third-party tokens an App trusts' <<<"$OUT" || fail "k2 app identity issuer $h prints the identity help: $OUT"
    grep -qF 'identity issuer add <app-id> --key <name>' <<<"$OUT" || fail "k2 app identity issuer $h names issuer add: $OUT"
    [ -z "$ERR" ] || fail "k2 app identity issuer $h: nothing on stderr: $ERR"
done
run app identity issuer frob shop
[ "$RC" -eq 1 ] || fail "k2 app identity issuer <not add|rm> stays a usage error (1), got $RC"
usage_json "k2 app identity issuer add|rm" || fail "the issuer usage JSON: $ERR"
[ ! -s "$FAKE_DIR/requests.log" ] || fail "identity help sent requests: $(cat "$FAKE_DIR/requests.log")"

study="$("$CLI" study db)"
grep -qF 'FROM SUPABASE' <<<"$study" || fail "k2 study db has FROM SUPABASE"
grep -qF 'k2 app user import <app> --file users.jsonl' <<<"$study" || fail "k2 study db names the user import"
help="$("$CLI" db --help)"
grep -qF 'import supabase <dump> --db <name>' <<<"$help" || fail "k2 db --help lists import supabase"
echo "PASS supabase_import_cli"
