#!/usr/bin/env bash
# App runtime update continuity, slice S1, on a REAL prepared Linux box
# (prd-app-runtime-update-continuity-v1 §8: T15 (b, c), T16, T25, T27; T17
# is a reboot and is listed as manual steps at the end). VM ONLY: it swaps
# the App engine binary by hand, restarts k2-app-engine and k2-daemon, and
# reads their journals. Never run it on sewlab or a customer box, and never
# on a Mac (the compute nodes can't run it either: no Podman, no helper).
#
# The box it needs (Debian 13 VM, the 0.46.4 candidate):
#   - k2-daemon (the candidate) as the `k2` user's systemd unit `k2-daemon`
#     (system unit, like the fleet; `K2_DAEMON_UNIT` to override), with the
#     hand-off drop-in;
#   - the root helper at protocol 9+, the box prepared (`k2 app box prepare`:
#     Podman 5, the k2app user, linger, nft, AppArmor) and the candidate's
#     engine installed (`k2 app engine install`);
#   - one guarded App deployed and live: `K2UC_APP` (default `uc-probe`), a
#     tiny HTTP server; the owner token in `K2UC_TOKEN` for `cli/k2`;
#   - the 0.46.0 engine build (protocol 2) at `K2UC_OLD_ENGINE` (the v0.46.0
#     release asset `k2-app-engine-linux-<arch>`), for T16/T25;
#   - the harness user may `sudo` (engine swap, unit restarts, journals).
#
# Usage: scripts/test/app-update-continuity-vm.sh [scenario ...]   (`list`)
# Fails loudly at the first failed check, with its evidence. Exit 0 only
# when every chosen scenario passed.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$REPO/cli/k2"
APP="${K2UC_APP:-uc-probe}"
UNIT="${K2_DAEMON_UNIT:-k2-daemon}"
PORT="${K2UC_DAEMON_PORT:-}"
OLD_ENGINE="${K2UC_OLD_ENGINE:-}"
ENGINE=/usr/local/libexec/k2-app-engine
K2APP_UID="$(id -u k2app 2>/dev/null || echo 999)"
ALL=(t15c-engine-restart t27-adopt-wait t15b-plain-restart t16-protocol2-heal t25-replay)

say() { printf '[k2uc %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { say "FAIL: $*"; exit 1; }
pass() { say "PASS: $*"; }

[ "$(uname -s)" = "Linux" ] || die "Linux VM only"
[ -n "${K2UC_TOKEN:-}" ] || die "set K2UC_TOKEN (the owner token)"
[ -f "$ENGINE" ] || die "no App engine installed at $ENGINE (k2 app engine install first)"

k2() { K2_TOKEN="$K2UC_TOKEN" "$CLI" "$@"; }
boot_status() { curl -fsS "http://127.0.0.1:${PORT:-$(k2 daemon port 2>/dev/null || echo 4646)}/boot-status"; }
engine_ok() { [ "$(engine_state)" = "ok" ]; }
engine_state() { boot_status | python3 -c 'import json,sys; print(json.load(sys.stdin).get("appEngine",{}).get("state"))'; }
unit_of_app() { k2 app inspect "$APP" --json | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["runtime"]["live"]["deployment"])'; }
quadlet_unit() { echo "k2app-$APP--$(unit_of_app).service"; }
user_ctl() { sudo systemctl --user -M "k2app@" "$@"; }
main_pid() { user_ctl show -p MainPID --value "$(quadlet_unit)"; }
deploy_count() { k2 app history "$APP" --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["deployments"]))'; }
front() { k2 app inspect "$APP" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["app"]["frontPort"])'; }
serving() { curl -fsS -m 3 "http://127.0.0.1:$(front)/" >/dev/null; }
linger_on() { [ "$(loginctl show-user "$K2APP_UID" -p Linger --value)" = "yes" ] && sudo systemctl is-active --quiet "user@$K2APP_UID.service"; }
wait_for() { # wait_for <secs> <what> <cmd...>
    local secs="$1" what="$2"; shift 2
    local end=$((SECONDS + secs))
    until "$@" >/dev/null 2>&1; do
        [ "$SECONDS" -lt "$end" ] || die "timed out after ${secs}s: $what"
        sleep 1
    done
}
journal_since() { sudo journalctl -u "$UNIT" --since "$1" --no-pager -o cat; }

# A curl loop against the App while something happens; prints the failures.
loop_start() { ( fails=0; while [ ! -f "$1.stop" ]; do curl -fsS -m 2 "http://127.0.0.1:$(front)/" >/dev/null 2>&1 || fails=$((fails+1)); sleep 0.2; done; echo "$fails" > "$1" ) & }
loop_stop() { touch "$1.stop"; wait; cat "$1"; }

restart_daemon_like_the_fleet() {
    sudo systemd-run --no-block systemctl restart "$UNIT"
    sleep 2
    wait_for 120 "the daemon answers /boot-status" boot_status
}

swap_engine_to_old() {
    [ -n "$OLD_ENGINE" ] && [ -f "$OLD_ENGINE" ] || die "set K2UC_OLD_ENGINE to the v0.46.0 engine build"
    sudo install -m 0755 "$OLD_ENGINE" "$ENGINE"
    user_ctl restart k2-app-engine
    sleep 2
}

# T15 (c): an engine-only restart and an engine reinstall keep the App's
# unit (and the rootless pause scope) running; no failed requests; linger.
t15c-engine-restart() {
    linger_on || die "linger is off for k2app before the test"
    local pid pause tmp; pid="$(main_pid)"; tmp="$(mktemp)"
    pause="$(user_ctl list-units --type=scope --no-legend | awk '/podman-pause/ {print $1; exit}')"
    loop_start "$tmp"
    user_ctl restart k2-app-engine
    sleep 5
    k2 app engine install >/dev/null
    sleep 5
    local fails; fails="$(loop_stop "$tmp")"
    [ "$fails" = "0" ] || die "T15c: $fails failed requests across the engine restarts"
    [ "$(main_pid)" = "$pid" ] || die "T15c: the App's MainPID changed ($pid → $(main_pid))"
    [ -z "$pause" ] || user_ctl is-active --quiet "$pause" || die "T15c: the pause scope $pause died"
    linger_on || die "T15c: linger went off"
    pass "T15c: engine restart + reinstall: App unit $pid untouched, 0 failed requests, linger on"
}

# T27 (A3/R33): the engine down while the daemon boots → no second
# instance; the survivor is adopted (same MainPID) once the engine answers;
# past 120 s the daemon says not_answering and still starts nothing.
t27-adopt-wait() {
    local pid n since; pid="$(main_pid)"; n="$(deploy_count)"; since="$(date '+%F %T')"
    user_ctl stop k2-app-engine
    restart_daemon_like_the_fleet
    sleep 130
    [ "$(engine_state)" = "not_answering" ] || die "T27: appEngine.state is $(engine_state) after 130 s, want not_answering"
    [ "$(deploy_count)" = "$n" ] || die "T27: a deployment started before adopt answered"
    journal_since "$since" | grep -q "ERROR \[apps\] the App engine hasn't answered adopt" || die "T27: no not-answering ERROR line"
    user_ctl start k2-app-engine
    wait_for 30 "the App is routed again" serving
    [ "$(deploy_count)" = "$n" ] || die "T27: a second instance was started"
    [ "$(main_pid)" = "$pid" ] || die "T27: the survivor's MainPID changed"
    pass "T27: waited for adopt; survivor $pid adopted; no second start"
}

# T15 (b): the fleet's way (installer with K2_NO_SERVICE=1, then a plain
# restart via systemd-run): the container survives and is adopted with no
# CLI call; only the boot window drops requests.
t15b-plain-restart() {
    local pid n tmp; pid="$(main_pid)"; n="$(deploy_count)"; tmp="$(mktemp)"
    loop_start "$tmp"
    K2_NO_SERVICE=1 sh "$REPO/scripts/install-daemon.sh" ${K2UC_MANIFEST:+--manifest-url "$K2UC_MANIFEST"}
    restart_daemon_like_the_fleet
    wait_for 60 "the App answers after the restart" serving
    local fails; fails="$(loop_stop "$tmp")"
    [ "$(main_pid)" = "$pid" ] || die "T15b: the container restarted"
    [ "$(deploy_count)" = "$n" ] || die "T15b: a new deployment ran (adopt should have claimed the survivor)"
    [ "$(engine_state)" = "ok" ] || die "T15b: appEngine.state $(engine_state)"
    linger_on || die "T15b: linger went off"
    pass "T15b: plain restart: unit $pid kept, adopted, $fails failed requests (the boot window)"
}

# T16: a protocol-2 engine installed by hand → the daemon's heal reinstalls
# the matching one at boot; the App never sees a prepare failure.
t16-protocol2-heal() {
    swap_engine_to_old
    local since; since="$(date '+%F %T')"
    restart_daemon_like_the_fleet
    wait_for 300 "the heal reaches ok" engine_ok
    k2 app engine status --json | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["running"]["protocol"]==3 and v["installed"]["version"]==v["want"]["version"], v'
    journal_since "$since" | grep -q "ERROR \[apps\] engine protocol 2 vs 3" || die "T16: no mismatch ERROR line"
    wait_for 60 "the App serves" serving
    pass "T16: protocol-2 engine healed to the candidate's at boot"
}

# T25: the 10-23 replay (H1 + H3): protocol-2 engine, the App's container
# gone (as after a reboot), daemon restart → ERROR lines with next tries,
# waiting_engine (never crashlooping), one owner notice, then the heal and
# the R43 kick bring it back with no CLI call. An owner-stopped App stays
# stopped.
t25-replay() {
    local stopped="${K2UC_STOPPED_APP:-}"
    swap_engine_to_old
    user_ctl stop "$(quadlet_unit)"
    local since; since="$(date '+%F %T')"
    restart_daemon_like_the_fleet
    wait_for 600 "the App serves again on its own" serving
    local j; j="$(journal_since "$since")"
    printf '%s\n' "$j" | grep -q "ERROR \[apps\] $APP restart d-[0-9a-f]* failed (engine_" || die "T25: no ERROR line for the failed restart"
    printf '%s\n' "$j" | grep -q "next try on engine change" || die "T25: the line has no next try"
    k2 app history "$APP" --json | python3 -c 'import json,sys; d=json.load(sys.stdin)["deployments"]; assert d[0]["result"]=="live", d[0]' || die "T25: not live"
    [ "$(k2 app inspect "$APP" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["app"]["state"])')" = "running" ] || die "T25: state"
    k2 tickets list --json 2>/dev/null | grep -q "waiting for K2's App engine" || die "T25: no owner notice"
    if [ -n "$stopped" ]; then
        [ "$(k2 app inspect "$stopped" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["app"]["state"])')" = "stopped" ] || die "T25: $stopped started"
    fi
    pass "T25: replay: K2-side waits, loud, back by itself"
}

if [ "${1:-}" = "list" ]; then printf '%s\n' "${ALL[@]}"; exit 0; fi
[ $# -gt 0 ] || set -- "${ALL[@]}"
for s in "$@"; do
    declare -F "$s" >/dev/null || die "unknown scenario $s (list)"
    say "== $s =="
    "$s"
done
say "all chosen scenarios passed"

# T17 (manual, needs a reboot): swap_engine_to_old (above), `sudo reboot`,
# then within boot + 2 min: `k2 app engine status` → ok, the App serves,
# `k2 app history` shows a `restart` row that went live and no crashlooping.
