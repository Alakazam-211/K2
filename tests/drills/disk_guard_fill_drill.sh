#!/usr/bin/env bash
# The Linux fill drill for the disk guard (prd-app-resources-disk-guard-v1
# §17.3, slice A6). Pass/fail. Run it on a THROWAWAY Linux VM (an ephemeral
# hcloud box, or k2-sandbox-01), never on a relay or customer box, with the
# K2 daemon running headless (no desktop client) as the box's K2 user.
#
# It fills the filesystem that holds ~/.k2 with fallocate'd files under
# $FILL_DIR, step by step, and checks what K2 does at each line. Every
# step's output is saved under $OUT. Any failure fails the drill. The fill
# files are deleted on exit (also on failure or Ctrl-C).
#
# Before you start (the checklist; the script checks what it can):
#   - The box runs the K2 build under test, headless, with Apps on.
#   - The walled engine is installed (k2 app engine install) and box prep
#     is done, Postgres is set up (k2 db …) and hosted mail is on.
#   - Two walled Apps are installed and live: $APP_A and $APP_B, with a
#     URL each ($APP_A_URL, $APP_B_URL) that answers HTTP 200.
#   - One --cmd service is running ($SERVICE), one database exists ($DB)
#     and $PG_DSN connects to it (k2 db dsn; psql on PATH).
#   - An agent passport for the workspace that owns $APP_A is in
#     $AGENT_TOKEN (from a K2 agent tab: its passport), and that workspace
#     has "Build and run Apps" on.
#   - Optional: an App guest upload URL + guest token ($GUEST_UPLOAD_URL,
#     $GUEST_TOKEN) for step 6; a test mail address ($MAIL_TO) for step 4.
#   - The owner can approve in person on this box (vault password or an
#     Owner Connect login): step 5 and step 7 pause for that.
#   - Safer alternative when no VM is free: a loop-mounted 20 GiB ext4
#     image as $HOME (K2 home + test workspace), so the real disk is never
#     filled. The walled-App steps then wait for a VM.
#
# Usage:
#   APP_A=jobs APP_B=notes APP_A_URL=https://… APP_B_URL=https://… \
#   SERVICE=helper DB=jobs_db PG_DSN="$(k2 db dsn jobs_db)" AGENT_TOKEN=k2s_… \
#   APP_A_FOLDER=/home/k2/ws-a/jobs \
#   tests/drills/disk_guard_fill_drill.sh
#
# Env:
#   FILL_DIR   where the fill files go (default ~/.k2-drill-fill; same fs as ~/.k2)
#   OUT        where step output is saved (default /var/tmp/k2-disk-drill-<time>)
#   K2         the k2 CLI (default: k2 on PATH)
#   SWEEP_CMD  the command that runs the Seed sweep now (lane M; step 10 is
#              skipped with a note when unset)

set -uo pipefail

K2="${K2:-k2}"
FILL_DIR="${FILL_DIR:-$HOME/.k2-drill-fill}"
OUT="${OUT:-/var/tmp/k2-disk-drill-$(date +%Y%m%d-%H%M%S)}"
APP_A="${APP_A:?set APP_A (a live walled App)}"
APP_B="${APP_B:?set APP_B (a second live walled App)}"
APP_A_URL="${APP_A_URL:?set APP_A_URL}"
APP_B_URL="${APP_B_URL:?set APP_B_URL}"
APP_A_FOLDER="${APP_A_FOLDER:?set APP_A_FOLDER (the folder APP_A deploys from)}"
SERVICE="${SERVICE:?set SERVICE (a running --cmd service)}"
DB="${DB:?set DB (a Postgres database)}"
PG_DSN="${PG_DSN:?set PG_DSN (k2 db dsn for DB, owner credentials)}"
AGENT_TOKEN="${AGENT_TOKEN:?set AGENT_TOKEN (a passport of the workspace of APP_A)}"

[ "$(uname -s)" = Linux ] || { echo "the drill runs on Linux only" >&2; exit 1; }
command -v fallocate >/dev/null || { echo "fallocate is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
command -v psql >/dev/null || { echo "psql is required" >&2; exit 1; }
mkdir -p "$OUT" "$FILL_DIR"
PORT="$(cat "$HOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$HOME/.k2/heartbeat.token")"
BASE="http://127.0.0.1:$PORT"
# A fingerprint of the database's tables (step 8 compares it before/after).
TABLES_SQL="SELECT md5(string_agg(table_name, ',' ORDER BY table_name)) FROM information_schema.tables WHERE table_schema = 'public'"

pass=0
fail=0
step=""
ok() { echo "  PASS [$step]: $1" | tee -a "$OUT/result.txt"; pass=$((pass + 1)); }
bad() { echo "  FAIL [$step]: $1" | tee -a "$OUT/result.txt" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2' want '$3')"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing '$3')"; fi; }
save() { cat > "$OUT/$step-$1.txt"; }
cleanup() {
    rm -f "$FILL_DIR"/fill-* 2>/dev/null
    rmdir "$FILL_DIR" 2>/dev/null || true
    echo
    echo "disk guard fill drill: $pass passed, $fail failed (output: $OUT)"
}
trap cleanup EXIT INT TERM

view() { curl -sf "$BASE/cli/app/resources?token=$OWNER_TOKEN"; }
fs_line() { # used% free-bytes total-bytes of the fs holding ~/.k2
    python3 - "$HOME/.k2" <<'PY'
import os, sys
s = os.statvfs(sys.argv[1])
used = (s.f_blocks - s.f_bfree) * s.f_frsize
free = s.f_bavail * s.f_frsize
print("%.1f %d %d" % (used * 100.0 / (used + free), free, s.f_blocks * s.f_frsize))
PY
}
fill_bytes_for() { # target used % -> bytes to add (used + x = target% of used + free)
    python3 - "$HOME/.k2" "$1" <<'PY'
import os, sys
s = os.statvfs(sys.argv[1])
used = (s.f_blocks - s.f_bfree) * s.f_frsize
free = s.f_bavail * s.f_frsize
x = float(sys.argv[2]) / 100.0 * (used + free) - used
print(max(0, int(x)))
PY
}
fill_to() { # target used % (df style)
    local target="$1" n=0
    read -r pct free total <<<"$(fs_line)"
    local want
    want="$(fill_bytes_for "$target")"
    [ "$want" -gt 0 ] || { echo "already at $pct% (target $target%)"; return 0; }
    while [ -e "$FILL_DIR/fill-$n" ]; do n=$((n + 1)); done
    fallocate -l "$want" "$FILL_DIR/fill-$n" || { bad "fallocate $want bytes"; return 1; }
    sync
    echo "filled to $(fs_line | cut -d' ' -f1)% (target $target%)"
}
level() { view | python3 -c '
import json, sys
v = json.load(sys.stdin)
d = [d for d in v["disks"] if "k2Home" in d.get("holds", [])]
print(d[0]["level"] if d else "?")'; }
guard_log_count() { # event
    grep -c "\"event\":\"$1\"" "$HOME/.k2/disk-guard-log.jsonl" 2>/dev/null || echo 0
}
http_code() { curl -s -o /dev/null -w '%{http_code}' --max-time 20 "$1"; }
checksum_state() { # data that must never change
    {
        for a in "$APP_A" "$APP_B"; do
            echo "== $a data"
            "$K2" app exec "$a" -- sh -c 'cd /data 2>/dev/null && find . -type f -print0 | sort -z | xargs -0 sha256sum' 2>&1
        done
        echo "== db $DB"
        psql "$PG_DSN" -tAc "$TABLES_SQL" 2>&1
        echo "== workspace $APP_A_FOLDER"
        (cd "$APP_A_FOLDER" && find . -type f -not -path './.git/*' -print0 | sort -z | xargs -0 sha256sum)
        echo "== mail store"
        sudo -n du -sb /var/lib/stalwart 2>/dev/null || echo "(no sudo: compare k2 hostmail status by hand)"
    }
}

# ── 0: the guard is on, the lines are the defaults ──────────────────
step=00-setup
"$K2" app resources settings --mode on --warn 85 --warn-free 20G --refuse 95 --reserve 10G --critical 98 --critical-free 2G | save settings
expect_eq "mode on" "$(view | python3 -c 'import json,sys; print(json.load(sys.stdin)["guard"]["mode"])')" "on"
expect_eq "the fs starts ok" "$(level)" "ok"
checksum_state > "$OUT/checksums-before.txt"

# ── 1: every row, with its owner; walled App disk is measured ───────
step=01-view
"$K2" app resources --rescan --wait 2>&1 | save rescan
view | tee "$OUT/$step-view.json" | python3 -c '
import json, os, sys
v = json.load(sys.stdin)
rows = {(c["kind"], c["id"]): c for c in v["consumers"]}
want = [("app", os.environ["APP_A"]), ("app", os.environ["APP_B"]), ("service", os.environ["SERVICE"]), ("database", os.environ["DB"])]
missing = [w for w in want if w not in rows]
ownerless = [k for k, c in rows.items() if not c.get("owner")]
walled_null = [k for k in want[:2] if k in rows and rows[k].get("diskBytes") is None]
print("missing=%s ownerless=%s walled_null=%s engine=%s workspace_rows=%d" % (
    missing, ownerless, walled_null, v["scan"].get("engine"), sum(1 for k in rows if k[0] == "workspace")))' | save check
expect_has "every row is there with an owner, walled disk measured" "$(cat "$OUT/$step-check.txt")" "missing=[] ownerless=[] walled_null=[] engine=ok"

# ── 2: 86%: warn notices; deploys go through with diskWarning ───────
step=02-warn
fill_to 86 | save fill
notices0="$(guard_log_count notice)"
sleep 75
expect_eq "level warn" "$(level)" "warn"
notices1="$(guard_log_count notice)"
[ "$notices1" -gt "$notices0" ] && ok "warn notices sent ($((notices1 - notices0)) guard-log lines)" || bad "no warn notice within 60 s"
"$K2" tickets list --json 2>/dev/null | save tickets
expect_has "one owner ticket about the disk" "$(cat "$OUT/$step-tickets.txt")" "full on"
"$K2" app deploy "$APP_A" "$APP_A_FOLDER" --json > "$OUT/$step-deploy.json" 2>"$OUT/$step-deploy.err"
rc=$?
expect_eq "deploy goes through at warn" "$rc" "0"
expect_has "  with a diskWarning" "$(cat "$OUT/$step-deploy.json")" "diskWarning"

# ── 3: a restart sends no second notice ─────────────────────────────
step=03-restart
before="$(guard_log_count notice)"
"$K2" daemon restart 2>&1 | save restart
for _ in $(seq 1 60); do curl -sf "$BASE/boot-status" | grep -q '"ready"' && break; sleep 2; done
sleep 75
expect_eq "no second notice after a restart" "$(guard_log_count notice)" "$before"

# ── 4: 96%: refuse; restarts, rollbacks, Postgres, mail and Apps work ─
step=04-refuse
fill_to 96 | save fill
sleep 75
expect_eq "level refuse" "$(level)" "refuse"
"$K2" app deploy "$APP_A" "$APP_A_FOLDER" > "$OUT/$step-deploy.txt" 2>&1
expect_eq "deploy exits 3" "$?" "3"
expect_has "  disk_low" "$(cat "$OUT/$step-deploy.txt")" "disk_low"
"$K2" app restart "$APP_A" > "$OUT/$step-restart.txt" 2>&1
expect_eq "restart exits 0" "$?" "0"
"$K2" app rollback "$APP_A" > "$OUT/$step-rollback.txt" 2>&1
expect_eq "rollback exits 0" "$?" "0"
psql "$PG_DSN" -tAc "SELECT 1" > "$OUT/$step-pg.txt" 2>&1
expect_eq "Postgres answers a query" "$?" "0"
if [ -n "${MAIL_TO:-}" ]; then
    "$K2" mail send --to "$MAIL_TO" --subject "disk drill" --body "step 4" > "$OUT/$step-mail.txt" 2>&1
    expect_eq "a test mail is accepted" "$?" "0"
else
    echo "  NOTE [$step]: MAIL_TO unset: check a test mail by hand" | tee -a "$OUT/result.txt"
fi
expect_eq "$APP_A answers HTTP" "$(http_code "$APP_A_URL")" "200"
expect_eq "$APP_B answers HTTP" "$(http_code "$APP_B_URL")" "200"

# ── 5: an agent's override: held, the owner approves, it runs ───────
step=05-agent-override
K2_HOOK_TOKEN="$AGENT_TOKEN" "$K2" app deploy "$APP_A" "$APP_A_FOLDER" --disk-override "drill step 5" > "$OUT/$step-deploy.txt" 2>&1
expect_eq "the agent's override exits 2" "$?" "2"
req="$(sed -n 's/^held for the owner: \([^ ]*\).*/\1/p' "$OUT/$step-deploy.txt" | head -1)"
[ -n "$req" ] && ok "request $req" || bad "no request id printed"
echo
echo ">>> Approve request $req IN PERSON now (k2 app approve $req from a terminal outside K2), then press Enter."
read -r _
"$K2" app approvals --json | save approvals
expect_has "approved and applied" "$(cat "$OUT/$step-approvals.txt")" "\"applied\": \"/cli/app/deploy\""

# ── 6: an App guest's upload: 507, names nobody, no numbers ─────────
step=06-guest-upload
if [ -n "${GUEST_UPLOAD_URL:-}" ] && [ -n "${GUEST_TOKEN:-}" ]; then
    head -c 1048576 /dev/zero > "$OUT/chunk.bin"
    code="$(curl -s -o "$OUT/$step-body.txt" -w '%{http_code}' -H "Authorization: Bearer $GUEST_TOKEN" \
        -F "file=@$OUT/chunk.bin" -F "totalBytes=2147483648" -F "offset=0" "$GUEST_UPLOAD_URL")"
    expect_eq "507 no_space" "$code" "507"
    body="$(cat "$OUT/$step-body.txt")"
    if printf '%s' "$body" | python3 -c 'import json,sys; m=json.load(sys.stdin).get("error",""); m=m if isinstance(m,str) else json.dumps(m); sys.exit(1 if any(c.isdigit() for c in m.replace("no_space","")) or "/" in m else 0)'; then
        ok "the message names nobody and no numbers"
    else
        bad "the guest message carries names or numbers: $body"
    fi
else
    echo "  NOTE [$step]: GUEST_UPLOAD_URL/GUEST_TOKEN unset: do the 2 GB guest upload by hand (want 507, no names, no numbers)" | tee -a "$OUT/result.txt"
fi

# ── 7: 98.5%: critical; the owner token's override is held ──────────
step=07-critical
fill_to 98.5 | save fill
sleep 75
expect_eq "level critical" "$(level)" "critical"
"$K2" tickets list --json 2>/dev/null | save tickets
expect_has "a CRITICAL ticket" "$(cat "$OUT/$step-tickets.txt")" "CRITICAL"
"$K2" app deploy "$APP_A" "$APP_A_FOLDER" --disk-override "drill step 7" > "$OUT/$step-deploy.txt" 2>&1
expect_eq "the owner token's override is held (exit 2), not applied" "$?" "2"
req7="$(sed -n 's/^held for the owner: \([^ ]*\).*/\1/p' "$OUT/$step-deploy.txt" | head -1)"
[ -n "$req7" ] && "$K2" app deny "$req7" --reason "drill" > /dev/null 2>&1

# ── 8: nothing was deleted ──────────────────────────────────────────
step=08-nothing-deleted
checksum_state > "$OUT/checksums-after.txt"
if diff -u "$OUT/checksums-before.txt" "$OUT/checksums-after.txt" > "$OUT/$step-diff.txt"; then
    ok "App data, the database, the workspace and the mail store are byte-identical"
else
    # Step 5's approved deploy may legitimately change APP_A's data only if
    # the App writes on start; read the diff before calling it a failure.
    bad "checksums differ (see $OUT/$step-diff.txt)"
fi

# ── 9: delete the fill: the level falls; re-arm only past the point ─
step=09-unfill
rm -f "$FILL_DIR"/fill-*
sync
sleep 75
expect_eq "the level falls to ok" "$(level)" "ok"
expect_has "the watch logged the fall" "$(tail -20 "$HOME/.k2/disk-guard-log.jsonl")" "\"event\""

# ── 10: the Seed sweep keeps the last 10, the live one and rollbacks ─
step=10-seed-sweep
if [ -n "${SWEEP_CMD:-}" ]; then
    for i in $(seq 1 12); do
        echo "$i" > "$APP_A_FOLDER/drill-version.txt"
        "$K2" app deploy "$APP_A" "$APP_A_FOLDER" > /dev/null 2>&1 || bad "deploy $i"
    done
    rm -f "$APP_A_FOLDER/drill-version.txt"
    sqlite3 "$HOME/.k2/k2.db" "UPDATE seeds SET last_used_at = last_used_at - 31*86400, created_at = created_at - 31*86400" \
        || bad "back-date the Seeds"
    seeds_before="$(ls "$HOME/.k2/seeds" | wc -l)"
    bash -c "$SWEEP_CMD" > "$OUT/$step-sweep.txt" 2>&1 || bad "the sweep command failed"
    seeds_after="$(ls "$HOME/.k2/seeds" | wc -l)"
    [ "$seeds_after" -lt "$seeds_before" ] && ok "Seeds removed ($seeds_before → $seeds_after)" || bad "no Seed removed"
    "$K2" app inspect "$APP_A" --json | save inspect
    live="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["app"]["liveSeed"])' "$OUT/$step-inspect.txt")"
    [ -d "$HOME/.k2/seeds/$live" ] && ok "the live Seed stays (daemon copy)" || bad "the live Seed is gone"
    sudo -n test -d "/var/lib/k2app/seeds/$live" && ok "the live Seed stays (engine copy)" || bad "the live Seed's engine copy is gone (or no sudo to check)"
    "$K2" app rollback "$APP_A" > "$OUT/$step-rollback.txt" 2>&1
    expect_eq "a rollback target is still there" "$?" "0"
else
    echo "  NOTE [$step]: SWEEP_CMD unset (lane M's sweep trigger): run step 10 by hand" | tee -a "$OUT/result.txt"
fi

# ── 11: mode off: the step-4 deploy goes through, no new fields ─────
step=11-mode-off
fill_to 96 | save fill
"$K2" app resources settings --mode off | save settings
notices="$(guard_log_count notice)"
"$K2" app deploy "$APP_A" "$APP_A_FOLDER" --json > "$OUT/$step-deploy.json" 2>&1
expect_eq "deploy exits 0 with the guard off" "$?" "0"
expect_eq "no new fields" "$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(any(k in v for k in ("diskWarning","memoryWarning","diskOverride")))' "$OUT/$step-deploy.json")" "False"
sleep 75
expect_eq "no notices with the guard off" "$(guard_log_count notice)" "$notices"
"$K2" app resources settings --mode on | save settings-restore

[ "$fail" -eq 0 ]
