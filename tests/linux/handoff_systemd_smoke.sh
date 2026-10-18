#!/usr/bin/env bash
# Runtime stage H — the hand-off under a REAL systemd unit (Linux, root).
#
# Runs a k2-daemon build as a transient unit carrying exactly the stage-H
# drop-in settings (Type=simple, KillMode=process, NotifyAccess=main,
# OOMPolicy=continue, Restart=always), with a scratch HOME, a private
# XDG_RUNTIME_DIR and an
# frpc stand-in under the tunnel supervisor. Checks:
#   1. POST /cli/daemon/handoff moves the unit's MainPID to the successor,
#      the unit stays active after the old daemon exits, frpc is the same
#      process (adopted);
#   2. `systemctl reload` (ExecReload = SIGUSR2) hands off again;
#   3. kill -9 of the main process: systemd restarts the daemon, frpc
#      survives (KillMode=process) and the new daemon adopts it;
#   4. `systemctl stop` stops the daemon (graceful: it stops frpc).
#
# Usage: handoff_systemd_smoke.sh <path/to/k2-daemon>
# Leaves nothing behind (trap cleans the unit, the scratch dir, stray pids).
set -euo pipefail
BIN="$(readlink -f "${1:?usage: $0 <k2-daemon binary>}")"
[ "$(id -u)" = 0 ] || { echo "needs root (systemd-run system unit)"; exit 2; }
UNIT="k2-rth-smoke-$$"
W="$(mktemp -d /tmp/k2-rth-smoke-XXXXXX)"
HOME_D="$W/home"; RUN="$W/run"; BIN_D="$W/bin"
mkdir -p "$HOME_D/.k2" "$RUN" "$BIN_D" "$W/shim"; chmod 700 "$RUN"
FRPC_PID=""
cleanup() {
	systemctl stop "$UNIT" >/dev/null 2>&1 || true
	systemctl reset-failed "$UNIT" >/dev/null 2>&1 || true
	[ -n "$FRPC_PID" ] && kill -9 "$FRPC_PID" 2>/dev/null || true
	pkill -9 -f "$W/" 2>/dev/null || true
	rm -rf "$W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*"; echo "--- daemon log ---"; journalctl -u "$UNIT" --no-pager | tail -60; exit 1; }
ok() { echo "ok - $*"; }

cat > "$BIN_D/frpc" <<'EOF'
#!/bin/sh
echo '[I] [proxy_wrapper.go:204] [x] [k2so-hof] start proxy success'
while :; do sleep 1; done
EOF
chmod 755 "$BIN_D/frpc"
cat > "$HOME_D/.k2/tunnel.json" <<'EOF'
{"server_addr":"127.0.0.1","server_port":1,"token":"tok-rth","subdomain":"hof","auto_start":true,"e2e":true}
EOF

systemd-run --quiet --unit="$UNIT" \
	-p Type=simple -p KillMode=process -p NotifyAccess=main -p OOMPolicy=continue \
	-p Restart=always -p RestartSec=1 \
	-p "ExecReload=/bin/kill -USR2 \$MAINPID" \
	--setenv=HOME="$HOME_D" --setenv=PATH="$BIN_D:/usr/local/bin:/usr/bin:/bin" \
	--setenv=SHELL=/bin/false --setenv=XDG_RUNTIME_DIR="$RUN" \
	--setenv=K2_E2E_SELF_SIGNED=1 --setenv=K2_TUNNEL_LEASE=off \
	--setenv=K2_CONNECT_BASE=http://127.0.0.1:1 --setenv=K2_CERT_BROKER_URL=http://127.0.0.1:1 \
	--setenv=K2_HANDOFF_DRAIN_SECS=30 --setenv=K2_HANDOFF_DROPIN=off \
	--setenv=K2_TEST_AGENT_SHIM_DIR="$W/shim" --setenv=K2_HEARTBEAT_NO_SELF_HEAL=1 \
	--setenv=K2SO_WATCHDOG_DISABLED=1 \
	"$BIN"

K2="$HOME_D/.k2"
main_pid() { systemctl show -p MainPID --value "$UNIT"; }
lock_pid() { tr -d '[:space:]' < "$K2/daemon.lock" 2>/dev/null || true; }
wait_until() { # wait_until <secs> <what> <cmd...>
	local n=$(( $1 * 10 )) what=$2; shift 2
	for _ in $(seq "$n"); do "$@" && return 0; sleep 0.1; done
	fail "timed out: $what"
}
ready() {
	[ -f "$K2/daemon.port" ] && [ -f "$K2/daemon.token" ] || return 1
	local p t; p=$(cat "$K2/daemon.port"); t=$(cat "$K2/daemon.token")
	[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$p/cli/daemon/handoff/status?token=$t")" = 200 ]
}
frpc_rec_pid() { sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$RUN/k2/frpc.pid" 2>/dev/null; }
alive() { [ -n "$1" ] && kill -0 "$1" 2>/dev/null && ! grep -q '^[^)]*) Z' "/proc/$1/stat" 2>/dev/null; }

wait_until 60 "daemon ready" ready
PORT=$(cat "$K2/daemon.port")
wait_until 60 "frpc started" test -n "$(frpc_rec_pid)"
FRPC_PID=$(frpc_rec_pid)
alive "$FRPC_PID" || fail "frpc not running"
TLS_PORT=$(cat "$K2/tunnel-https.port")
P1=$(main_pid)
[ "$P1" = "$(lock_pid)" ] || fail "MainPID $P1 != lock pid $(lock_pid)"
ok "unit up: MainPID $P1, frpc $FRPC_PID, TLS :$TLS_PORT"

# 1. Hand-off through the route.
T=$(cat "$K2/daemon.token")
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/cli/daemon/handoff?token=$T")
[ "$code" = 202 ] || fail "handoff route answered $code"
wait_until 60 "lock moves off $P1" bash -c "[ \"\$(tr -d '[:space:]' < '$K2/daemon.lock')\" != '$P1' ]"
P2=$(lock_pid)
wait_until 10 "MainPID = successor $P2" bash -c "[ \"\$(systemctl show -p MainPID --value '$UNIT')\" = '$P2' ]"
wait_until 40 "old daemon $P1 exits" bash -c "! kill -0 $P1 2>/dev/null"
[ "$(systemctl is-active "$UNIT")" = active ] || fail "unit not active after the old daemon exited"
[ "$(main_pid)" = "$P2" ] || fail "MainPID moved again: $(main_pid)"
wait_until 30 "successor ready" ready
[ "$(frpc_rec_pid)" = "$FRPC_PID" ] && alive "$FRPC_PID" || fail "frpc not adopted (record $(frpc_rec_pid))"
[ "$(cat "$K2/tunnel-https.port")" = "$TLS_PORT" ] || fail "TLS port moved"
ok "route hand-off: MainPID $P1 -> $P2, unit active, frpc $FRPC_PID adopted"

# 2. systemctl reload = SIGUSR2 = hand-off.
systemctl reload "$UNIT"
wait_until 60 "lock moves off $P2" bash -c "[ \"\$(tr -d '[:space:]' < '$K2/daemon.lock')\" != '$P2' ]"
P3=$(lock_pid)
wait_until 10 "MainPID = $P3" bash -c "[ \"\$(systemctl show -p MainPID --value '$UNIT')\" = '$P3' ]"
wait_until 40 "daemon $P2 exits" bash -c "! kill -0 $P2 2>/dev/null"
[ "$(systemctl is-active "$UNIT")" = active ] || fail "unit not active after reload"
wait_until 30 "daemon ready after reload" ready
[ "$(frpc_rec_pid)" = "$FRPC_PID" ] && alive "$FRPC_PID" || fail "frpc not adopted after reload"
ok "systemctl reload hand-off: MainPID $P2 -> $P3, frpc still $FRPC_PID"

# 3. kill -9 the main process: systemd restarts the daemon; frpc survives
#    (KillMode=process) and is adopted.
kill -9 "$P3"
wait_until 60 "systemd restarts the daemon" bash -c "p=\$(systemctl show -p MainPID --value '$UNIT'); [ -n \"\$p\" ] && [ \"\$p\" != 0 ] && [ \"\$p\" != '$P3' ]"
P4=$(main_pid)
wait_until 60 "restarted daemon ready" ready
wait_until 30 "restarted daemon owns the lock" bash -c "[ \"\$(tr -d '[:space:]' < '$K2/daemon.lock')\" = '$P4' ]"
T4=$(cat "$K2/daemon.token"); PORT4=$(cat "$K2/daemon.port")
wait_until 30 "tunnel running after the crash" bash -c "curl -s 'http://127.0.0.1:$PORT4/cli/tunnel/status?token=$T4' | grep -q '\"running\":true'"
[ "$(frpc_rec_pid)" = "$FRPC_PID" ] && alive "$FRPC_PID" || fail "frpc did not survive kill -9 / was not adopted (record $(frpc_rec_pid))"
[ "$(cat "$K2/tunnel-https.port")" = "$TLS_PORT" ] || fail "TLS port moved after the crash"
ok "kill -9: systemd restarted $P3 -> $P4, frpc $FRPC_PID survived and was adopted, TLS port kept"

# 4. systemctl stop: graceful stop (stops frpc).
systemctl stop "$UNIT"
wait_until 20 "frpc stopped with the daemon" bash -c "! kill -0 $FRPC_PID 2>/dev/null"
ok "systemctl stop: daemon and frpc stopped"
echo "PASS"
