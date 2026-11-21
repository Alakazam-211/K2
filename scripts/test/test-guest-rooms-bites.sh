#!/usr/bin/env bash
# 0.46.17 apps.test-guest-rooms: the bites. Each breaks one rule, runs the
# test that pins it and expects it to FAIL, then puts the file back. Run on a
# compute node from a clean checkout of the branch, one bite per job:
#   BITES="<words of its name>" bash scripts/test/test-guest-rooms-bites.sh
set -uo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
pass=0; fail=0

# bite <name> <file> <old> <new> <nextest filter>
bite() {
    local name="$1" file="$2" old="$3" new="$4" filter="$5"
    case "$name" in *"${BITES:-}"*) ;; *) return ;; esac
    cp "$file" "/tmp/bite.bak"
    if ! python3 - "$file" "$old" "$new" <<'PY'
import sys
p, old, new = sys.argv[1:4]
s = open(p).read()
if old not in s:
    sys.exit(1)
open(p, "w").write(s.replace(old, new, 1))
PY
    then
        echo "BITE $name: the line to break wasn't found (stale bite)"; fail=$((fail + 1)); cp /tmp/bite.bak "$file"; return
    fi
    if cargo nextest run -p k2-daemon --lib -E "test(/$filter/)" >/tmp/bite.log 2>&1; then
        echo "BITE $name: NOT caught (the test still passes)"; fail=$((fail + 1))
    elif grep -q "error\[E" /tmp/bite.log; then
        echo "BITE $name: didn't compile"; grep -E -A6 "error\[E" /tmp/bite.log | head -20; fail=$((fail + 1))
    else
        echo "BITE $name: caught"; pass=$((pass + 1))
    fi
    cp /tmp/bite.bak "$file"
}

T=crates/k2-core/src/skin_ties.rs
G=crates/k2-core/src/skin_test_guests.rs
R=crates/k2-daemon/src/apps/test_guest_rooms.rs
bite "opened rooms are ignored" "$T" \
    "|| (app != HOST_APP_ID && tie.opened(app, room))" "|| false" \
    "an_agents_test_login_reaches_its_roles_rooms_only_after"
bite "a room outside the card lets through" "$T" \
    "|| (app != HOST_APP_ID && tie.opened(app, room))" "|| (app != HOST_APP_ID && !tie.rooms_ok.is_empty())" \
    "rooms_outside_the_card_stay_refused"
bite "a changed role keeps the old role's rooms" "$G" \
    "AND g.enabled != 0 AND g.role_id = r.role_id" "AND g.enabled != 0" \
    "rooms_outside_the_card_stay_refused"
bite "a stale card applies anyway" "$R" \
    "    if p.role != role {" "    if false {" \
    "rooms_outside_the_card_stay_refused"
bite "a login that isn't --test takes it" "$R" \
    "let t = tg::get(&p.id).map_err(store_err)?.filter(|t| t.app == app.service_id).ok_or_else(not_test)?;" \
    "let t = tg::get(&p.id).map_err(store_err)?.or(Some(tg::TestGuest { principal_id: p.id.clone(), app: app.service_id.clone(), workspace_id: String::new(), created_by: String::new(), created_at: 0, expires_at: i64::MAX })).filter(|t| t.app == app.service_id).ok_or_else(not_test)?;" \
    "only_a_test_login_an_agent_made_takes_it"
bite "an agent answers its own card" crates/k2-daemon/src/overlay_routes.rs \
    "    if passport {
        return Err(action_card_person_only(" "    if false {
        return Err(action_card_person_only(" \
    "an_agents_test_login_reaches_its_roles_rooms_only_after"

echo "bites: $pass caught, $fail not"
[ "$fail" -eq 0 ]
