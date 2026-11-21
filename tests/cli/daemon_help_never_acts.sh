#!/usr/bin/env bash
# -h or --help anywhere on a `k2 daemon` line prints
# help and does nothing: no service call, no request, no file touched, no
# prompt. Stand-in service managers only (tests/cli/_daemon_stand_ins.sh):
# no row can reach a real systemctl, launchctl, loginctl, journalctl or
# sudo, and the guard fails the test if one is reachable.
#
# Regression (0.46.17):
#   - `k2 daemon survive on -h` and `survive off --help` sent the setting;
#     on macOS `start -h` ran `launchctl load`; `uninstall -h` asked and
#     then uninstalled; `stop -h` and `restart -h` were usage errors.
#     Now every verb but install (which has its own help) prints the
#     daemon usage on stdout, exit 0; survive prints its own help. Help
#     needs no connection to K2.
#   - uninstall took --yes or -y only as its first word. Now anywhere.
#   - `restart --host` or `--token` as the last word exited 1 with no
#     message. Now: "daemon restart: --host needs a value", exit 2,
#     nothing sent.
#
# The daemon usage expected is what `k2 daemon <a word it has no arm for>`
# prints on stderr; survive's help is what `k2 daemon survive help` prints.
# Run with: bash tests/cli/daemon_help_never_acts.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
. "$SCRIPT_DIR/_daemon_stand_ins.sh"
for v in $(env | grep -oE '^K[25](SO)?_[A-Z0-9_]+' || true); do
    [ "$v" = K5_CLI_UNDER_TEST ] || unset "$v"
done
ds_setup

U=k2-daemon.service
nl=$'\n'

# The two help texts, from the forms that print them today.
ds_reset
ds_run nosuch
eq "the usage: exit" "$RC" 1
USAGE="$ERR"
eq "the usage: its first line" "$(printf '%s\n' "$USAGE" | head -n 1)" "Usage: k2 daemon <status|start|stop|restart|survive|log|install|uninstall>"
ds_reset
ds_run survive help
eq "survive help: exit" "$RC" 0
SURVIVE_HELP="$OUT"
eq "survive help: its first line" "$(printf '%s\n' "$SURVIVE_HELP" | head -n 1)" "Usage: k2 daemon survive [on|off|status]"

# nothing_done <label> <want stdout> <files>: exit 0, the help on stdout,
# nothing on stderr, no service call, no request, the files as they were.
nothing_done() {
    eq "$1: exit" "$RC" 0
    eq "$1: stdout" "$OUT" "$2"
    eq "$1: stderr" "$ERR" ""
    eq "$1: service calls" "$(ds_calls)" ""
    eq "$1: files left" "$(ds_files)" "$3"
    if [ -f "$DS_SVC/state/unit" ]; then eq "$1: unit state" "$(ds_state)" "active enabled"; fi
    if [ -s "$DS_REQS" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$DS_REQS" >&2
    else
        ok "$1: sent nothing"
    fi
}

echo "== help on Linux, a user unit running =="
help_linux() {
    local want="$1"; shift
    ds_reset; ds_unit user active enabled
    DS_STDIN=$'y\n' ds_run "$@"
    nothing_done "daemon $*" "$want" "daemon.port daemon.token"
}
help_linux "$USAGE" -h
help_linux "$USAGE" --help
help_linux "$USAGE" status -h
help_linux "$USAGE" start -h
help_linux "$USAGE" start --help
help_linux "$USAGE" stop -h
help_linux "$USAGE" stop --keep-agents --help
help_linux "$USAGE" restart -h
help_linux "$USAGE" restart --wait --help
help_linux "$USAGE" restart --sessions -h
help_linux "$USAGE" restart --host "http://127.0.0.1:$DS_PORT" --help
help_linux "$USAGE" restart --token -h
help_linux "$USAGE" log -n 2 -h
help_linux "$USAGE" uninstall -h
help_linux "$USAGE" uninstall --yes --help
help_linux "$USAGE" uninstall --help --yes
help_linux "$SURVIVE_HELP" survive -h
help_linux "$SURVIVE_HELP" survive on -h
help_linux "$SURVIVE_HELP" survive off --help
help_linux "$SURVIVE_HELP" survive --help on
help_linux "$SURVIVE_HELP" survive status -h

echo "== help on macOS, the launch agent installed =="
help_mac() {
    ds_reset; ds_mac
    DS_STDIN=$'y\n' ds_run "$@"
    nothing_done "macOS: daemon $*" "$USAGE" "daemon.port daemon.token plist"
}
help_mac start -h
help_mac start --help
help_mac stop -h
help_mac restart --help
help_mac uninstall -y -h
help_mac uninstall --help

echo "== help needs no connection to K2 =="
ds_reset; ds_unit user active enabled
rm -f "$DS_K2HOME/daemon.port" "$DS_K2HOME/daemon.token"
DS_NOCONN=1 ds_run survive on --help
nothing_done "no connection: daemon survive on --help" "$SURVIVE_HELP" ""
ds_reset; ds_unit user active enabled
rm -f "$DS_K2HOME/daemon.port" "$DS_K2HOME/daemon.token"
DS_NOCONN=1 ds_run start -h
nothing_done "no connection: daemon start -h" "$USAGE" ""

echo "== uninstall takes --yes or -y anywhere =="
USER_DOWN="systemctl --user cat $U${nl}systemctl --user stop $U${nl}systemctl --user disable $U${nl}systemctl --user is-active $U"
ds_reset; ds_unit user active enabled
ds_run uninstall now --yes
eq "uninstall now --yes: exit" "$RC" 0
eq "uninstall now --yes: stdout" "$OUT" "Daemon uninstalled."
eq "uninstall now --yes: stderr" "$ERR" ""
eq "uninstall now --yes: service calls" "$(ds_calls)" "$USER_DOWN"
eq "uninstall now --yes: files left" "$(ds_files)" "run/stop-intent.json"
ds_reset; ds_mac
ds_run uninstall x -y
eq "macOS: uninstall x -y: exit" "$RC" 0
eq "macOS: uninstall x -y: stdout" "$OUT" "Daemon uninstalled."
eq "macOS: uninstall x -y: service calls" "$(ds_calls)" "launchctl unload -w $DS_HOME/Library/LaunchAgents/dev.k2.daemon.plist"
eq "macOS: uninstall x -y: files left" "$(ds_files)" ""
ds_reset; ds_mac
DS_STDIN=$'n\n' ds_run uninstall x
eq "macOS: uninstall x, n: exit" "$RC" 1
eq "macOS: uninstall x, n: stdout" "$OUT" "This will remove the K2 daemon launch agent and stop the daemon.
Your workspace data (~/.k2 database + agent files) is preserved.
Proceed? [y/N] Cancelled."
eq "macOS: uninstall x, n: service calls" "$(ds_calls)" ""
eq "macOS: uninstall x, n: files left" "$(ds_files)" "daemon.port daemon.token plist"

echo "== restart: a value flag as the last word =="
last_flag() {
    local msg="$1"; shift
    ds_reset; ds_unit user active enabled
    ds_run "$@"
    eq "daemon $*: exit" "$RC" 2
    eq "daemon $*: stdout" "$OUT" ""
    eq "daemon $*: stderr" "$ERR" "$msg"
    eq "daemon $*: service calls" "$(ds_calls)" ""
    if [ -s "$DS_REQS" ]; then
        bad "daemon $*: sent a request:"; sed 's/^/      | /' "$DS_REQS" >&2
    else
        ok "daemon $*: sent nothing"
    fi
}
last_flag "daemon restart: --host needs a value" restart --host
last_flag "daemon restart: --token needs a value" restart --wait --token
last_flag "daemon restart: --token needs a value" restart --host "http://127.0.0.1:$DS_PORT" --token

echo "== the plain forms still act =="
ds_reset
echo '{"terminal":{"ptyd":"on"}}' >"$DS_WORK/reply"
ds_run survive on
eq "survive on: exit" "$RC" 0
eq "survive on: the request" "$(cut -d' ' -f1,2 "$DS_REQS")" "POST /cli/settings/update"
ds_reset; ds_mac
ds_run start
eq "macOS: start: exit" "$RC" 0
eq "macOS: start: service calls" "$(ds_calls)" "launchctl load -w $DS_HOME/Library/LaunchAgents/dev.k2.daemon.plist"

echo
echo "daemon_help_never_acts: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
