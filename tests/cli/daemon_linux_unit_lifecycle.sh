#!/usr/bin/env bash
# `k2 daemon uninstall` and `k2 daemon start` on Linux act on the daemon's
# systemd unit, found the way `k2 daemon status` finds it (user, system or
# none). Stand-in service managers only (tests/cli/_daemon_stand_ins.sh):
# no row can reach a real systemctl, launchctl, loginctl, journalctl or
# sudo, and the guard fails the test if one is reachable.
#
# Regression (0.46.17):
#   - uninstall on Linux deleted daemon.port and daemon.token and left the
#     unit running: a daemon k2 could no longer reach. Now a user unit is
#     stopped and disabled, then checked down (is-active says inactive or
#     failed), and only then are the files removed. A failed stop or
#     disable, or a unit still running, removes nothing and says so. A
#     system unit needs root: unless it is already down and disabled, the
#     two sudo lines are printed, nothing is removed, exit 1, and no prompt
#     comes first.
#   - start had no Linux branch (it looked for a launchd plist). Now a user
#     unit is started, a system unit gets the exact sudo line, and with no
#     unit it says to run `k2 daemon install`.
#   - macOS is unchanged: uninstall unloads and removes the plist, start
#     loads it.
#
# Each row checks stdout, stderr, the exit code, the exact service calls,
# the files left (port, token, stop intent, plist), the unit's state, and
# that no request reached the stub daemon.
# Run with: bash tests/cli/daemon_linux_unit_lifecycle.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
. "$SCRIPT_DIR/_daemon_stand_ins.sh"
for v in $(env | grep -oE '^K[25](SO)?_[A-Z0-9_]+' || true); do
    [ "$v" = K5_CLI_UNDER_TEST ] || unset "$v"
done
ds_setup

U=k2-daemon.service
CAT_USER="systemctl --user cat $U"
CAT_USER_OLD="systemctl --user cat k2so-daemon.service"
CAT_SYSTEM="systemctl cat $U"
CAT_SYSTEM_OLD="systemctl cat k2so-daemon.service"
nl=$'\n'

# row <label> <rc> <stdout> <stderr> <calls> <files> [state]
row() {
    local label="$1"
    eq "$label: exit" "$RC" "$2"
    eq "$label: stdout" "$OUT" "$3"
    eq "$label: stderr" "$ERR" "$4"
    eq "$label: service calls" "$(ds_calls)" "$5"
    eq "$label: files left" "$(ds_files)" "$6"
    if [ $# -ge 7 ]; then eq "$label: unit state" "$(ds_state)" "$7"; fi
    if [ -s "$DS_REQS" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$DS_REQS" >&2
    else
        ok "$label: sent nothing"
    fi
}

USER_DOWN="$CAT_USER${nl}systemctl --user stop $U${nl}systemctl --user disable $U${nl}systemctl --user is-active $U"
SYS_FIND="$CAT_USER${nl}$CAT_USER_OLD${nl}$CAT_SYSTEM"
SYS_REFUSED="The daemon's unit is a system unit ($U): stopping and disabling it needs root. Nothing was removed.
Run:
  sudo systemctl stop $U
  sudo systemctl disable $U
then run k2 daemon uninstall again."

echo "== uninstall: a user unit =="
ds_reset; ds_unit user active enabled
ds_run uninstall --yes
row "uninstall --yes, user unit" 0 "Daemon uninstalled." "" "$USER_DOWN" "run/stop-intent.json" "inactive disabled"

ds_reset; ds_unit user active enabled
DS_STDIN=$'y\n' ds_run uninstall
row "uninstall, user unit, y" 0 "This will stop and disable the K2 daemon's systemd unit ($U) and remove its port and token files.
Your workspace data (~/.k2 database + agent files) is preserved.
Proceed? [y/N] Daemon uninstalled." "" "$USER_DOWN" "run/stop-intent.json" "inactive disabled"

ds_reset; ds_unit user active enabled
DS_STDIN=$'n\n' ds_run uninstall
row "uninstall, user unit, n" 1 "This will stop and disable the K2 daemon's systemd unit ($U) and remove its port and token files.
Your workspace data (~/.k2 database + agent files) is preserved.
Proceed? [y/N] Cancelled." "" "$CAT_USER" "daemon.port daemon.token" "active enabled"

ds_reset; ds_unit user active enabled; ds_fails stop-fails
ds_run uninstall --yes
row "uninstall --yes, user unit, the stop fails" 1 "" "Failed to stop $U: Access denied
daemon uninstall: systemctl --user stop $U failed. Nothing was removed." \
    "$CAT_USER${nl}systemctl --user stop $U" "daemon.port daemon.token" "active enabled"

ds_reset; ds_unit user active enabled; ds_fails stop-keeps-running
ds_run uninstall --yes
row "uninstall --yes, user unit, still running after the stop" 1 "" \
    "daemon uninstall: $U is still running after the stop. Nothing was removed." \
    "$USER_DOWN" "daemon.port daemon.token" "active disabled"

ds_reset; ds_unit user active enabled; ds_fails disable-fails
ds_run uninstall --yes
row "uninstall --yes, user unit, the disable fails" 1 "" "Failed to disable $U: Access denied
daemon uninstall: systemctl --user disable $U failed. Nothing was removed." \
    "$CAT_USER${nl}systemctl --user stop $U${nl}systemctl --user disable $U" \
    "daemon.port daemon.token run/stop-intent.json" "inactive enabled"

ds_reset; ds_unit user active enabled k2so-daemon.service
ds_run uninstall -y
row "uninstall -y, a user unit by the old name" 0 "Daemon uninstalled." "" \
    "$CAT_USER${nl}$CAT_USER_OLD${nl}systemctl --user stop k2so-daemon.service${nl}systemctl --user disable k2so-daemon.service${nl}systemctl --user is-active k2so-daemon.service" \
    "run/stop-intent.json" "inactive disabled"

echo "== uninstall: a system unit =="
ds_reset; ds_unit system active enabled
ds_run uninstall --yes
row "uninstall --yes, system unit running" 1 "" "$SYS_REFUSED" \
    "$SYS_FIND${nl}systemctl is-active $U" "daemon.port daemon.token" "active enabled"

ds_reset; ds_unit system active enabled
DS_STDIN=$'y\n' ds_run uninstall
row "uninstall, system unit running: no prompt first" 1 "" "$SYS_REFUSED" \
    "$SYS_FIND${nl}systemctl is-active $U" "daemon.port daemon.token" "active enabled"

ds_reset; ds_unit system inactive enabled
ds_run uninstall --yes
row "uninstall --yes, system unit stopped but enabled" 1 "" "$SYS_REFUSED" \
    "$SYS_FIND${nl}systemctl is-active $U${nl}systemctl is-enabled $U" "daemon.port daemon.token" "inactive enabled"

ds_reset; ds_unit system inactive disabled
ds_run uninstall --yes
row "uninstall --yes, system unit stopped and disabled" 0 "Daemon uninstalled." "" \
    "$SYS_FIND${nl}systemctl is-active $U${nl}systemctl is-enabled $U" "" "inactive disabled"

ds_reset; ds_unit system failed disabled
ds_run uninstall --yes
row "uninstall --yes, system unit failed and disabled" 0 "Daemon uninstalled." "" \
    "$SYS_FIND${nl}systemctl is-active $U${nl}systemctl is-enabled $U" "" "failed disabled"

echo "== uninstall: no unit, and macOS =="
ds_reset
ds_run uninstall --yes
row "uninstall --yes, no unit" 0 "Daemon uninstalled." "" "$SYS_FIND${nl}$CAT_SYSTEM_OLD" ""

ds_reset; ds_mac
ds_run uninstall -y
row "uninstall -y on macOS" 0 "Daemon uninstalled." "" \
    "launchctl unload -w $DS_HOME/Library/LaunchAgents/dev.k2.daemon.plist" ""

echo "== start =="
ds_reset; ds_unit user inactive enabled
ds_run start
row "start, user unit" 0 "Daemon started ($U)." "" "$CAT_USER${nl}systemctl --user start $U" \
    "daemon.port daemon.token" "active enabled"

ds_reset; ds_unit user inactive enabled; ds_fails start-fails
ds_run start
row "start, user unit, the start fails" 1 "" "Failed to start $U: Access denied" \
    "$CAT_USER${nl}systemctl --user start $U" "daemon.port daemon.token" "inactive enabled"

ds_reset; ds_unit system inactive enabled
ds_run start
row "start, system unit" 1 "" "Starting a system unit needs root: sudo systemctl start $U" \
    "$SYS_FIND" "daemon.port daemon.token" "inactive enabled"

ds_reset
ds_run start
row "start, no unit" 1 "" "daemon start: no systemd unit for the daemon (k2-daemon.service, user or system).
Install it first: k2 daemon install" "$SYS_FIND${nl}$CAT_SYSTEM_OLD" "daemon.port daemon.token"

ds_reset; ds_mac
ds_run start
row "start on macOS" 0 "Daemon started." "" \
    "launchctl load -w $DS_HOME/Library/LaunchAgents/dev.k2.daemon.plist" "daemon.port daemon.token plist"

echo
echo "daemon_linux_unit_lifecycle: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
