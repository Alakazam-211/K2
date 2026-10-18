#!/usr/bin/env bash
# front443 — the :443 SNI front under a REAL systemd socket (Linux, root).
#
# Writes the same pair the root helper's `front install` writes — a socket
# unit (`BindIPv6Only=both`, `FileDescriptorName=k2-front-443`,
# `Service=<daemon unit>`) and the daemon's `Sockets=` + `Wants/After` —
# except that the socket listens on [::]:$FRONT_PORT (default 44300), not
# :443, so the smoke runs on a box whose :443 is taken. The daemon unit
# carries the stage-H settings (KillMode=process, NotifyAccess=main,
# Restart=always) and a scratch HOME whose voice record says TURN runs on
# the passthrough tier for turn-smoke.k2.dev, so the front routes that
# name to 127.0.0.1:$TURN_PORT, where a stand-in backend reads the PROXY
# v2 header and then terminates TLS itself with its OWN certificate.
#
# Checks:
#   1. LISTEN_FDS pickup: /cli/front/status serving, socket from systemd;
#   2. per SNI with openssl s_client: the routed name shows the BACKEND's
#      leaf (the front never ends TLS) and the backend logs the real client
#      address from PROXY v2 (a non-loopback source when the box has one);
#      an unknown name and a no-SNI hello are closed;
#   3. hand-off (POST /cli/daemon/handoff): the socket moves as
#      Role::Front443, an established spliced session survives, new
#      connections are served by the successor;
#   4. kill -9: systemd keeps :443 (the socket) and the restarted daemon
#      serves a connection that queued meanwhile;
#   5. systemctl stop: systemd still listens; a connection starts the
#      daemon again (Service=).
#
# Usage: front443_systemd_smoke.sh <path/to/k2-daemon>
# Leaves nothing behind (trap: units, scratch dir, stray pids).
set -euo pipefail
BIN="$(readlink -f "${1:?usage: $0 <k2-daemon binary>}")"
[ "$(id -u)" = 0 ] || { echo "needs root (system socket + service units)"; exit 2; }
FRONT_PORT="${FRONT_PORT:-44300}"
TURN_PORT=5349
U="k2f443-smoke-$$"
W="$(mktemp -d /tmp/k2-f443-smoke-XXXXXX)"
HOME_D="$W/home"; RUN="$W/run"
mkdir -p "$HOME_D/.k2/sew" "$RUN" "$W/shim"; chmod 700 "$RUN"
BACKEND_PID=""; HOLD_PID=""
cleanup() {
	systemctl stop "$U.service" "$U.socket" >/dev/null 2>&1 || true
	rm -f "/run/systemd/system/$U.socket" "/run/systemd/system/$U.service"
	systemctl daemon-reload || true
	systemctl reset-failed "$U.service" "$U.socket" >/dev/null 2>&1 || true
	[ -n "$BACKEND_PID" ] && kill "$BACKEND_PID" 2>/dev/null || true
	[ -n "$HOLD_PID" ] && kill "$HOLD_PID" 2>/dev/null || true
	pkill -9 -f "$W/" 2>/dev/null || true
	rm -rf "$W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*"; echo "--- daemon log ---"; journalctl -u "$U.service" --no-pager | tail -60; echo "--- backend log ---"; cat "$W/backend.log" 2>/dev/null | tail -20; exit 1; }
ok() { echo "ok - $*"; }
wait_until() { # wait_until <secs> <what> <cmd...>
	local n=$(( $1 * 10 )) what=$2; shift 2
	for _ in $(seq "$n"); do "$@" && return 0; sleep 0.1; done
	fail "timed out: $what"
}
ss -ltn "( sport = :$FRONT_PORT or sport = :$TURN_PORT )" | grep -q LISTEN && { echo "SKIP-FAIL: :$FRONT_PORT or :$TURN_PORT already taken"; exit 1; }

# The backend's own certificate (the front holds no key).
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
	-subj "/CN=turn-smoke.k2.dev" -addext "subjectAltName=DNS:turn-smoke.k2.dev" \
	-keyout "$W/key.pem" -out "$W/cert.pem" >/dev/null 2>&1
cat > "$W/backend.py" <<'PY'
# Stand-in for sew-media's passthrough listener: PROXY v2 first (logged),
# then TLS with its own certificate on the same stream, then echo.
import socket, ssl, struct, sys, threading
port, cert, key, log = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(cert, key)
SIG = b"\r\n\r\n\x00\r\nQUIT\n"
def recvn(c, n):
    b = b""
    while len(b) < n:
        d = c.recv(n - len(b))
        if not d: raise EOFError
        b += d
    return b
def serve(c):
    try:
        h = recvn(c, 16)
        assert h[:12] == SIG, "no PROXY v2 signature"
        ver_cmd, fam, ln = h[12], h[13], struct.unpack("!H", h[14:16])[0]
        body = recvn(c, ln)
        if fam == 0x11:
            src = socket.inet_ntop(socket.AF_INET, body[0:4]); sport = struct.unpack("!H", body[8:10])[0]
        else:
            src = socket.inet_ntop(socket.AF_INET6, body[0:16]); sport = struct.unpack("!H", body[32:34])[0]
        open(log, "a").write("proxy cmd=%#x client=%s:%d\n" % (ver_cmd, src, sport))
        t = ctx.wrap_socket(c, server_side=True)
        while True:
            d = t.recv(4096)
            if not d: break
            t.sendall(d)
    except Exception as e:
        open(log, "a").write("error %r\n" % (e,))
    finally:
        c.close()
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port)); s.listen(64)
while True:
    c, _ = s.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
PY
python3 "$W/backend.py" "$TURN_PORT" "$W/cert.pem" "$W/key.pem" "$W/backend.log" & BACKEND_PID=$!

# The daemon's state: the front on, voice on the passthrough tier.
echo '{"enabled":true,"mail":false}' > "$HOME_D/.k2/front443.json"
cat > "$HOME_D/.k2/sew/voice.json" <<EOF
{"installed":true,"turnTier":"passthrough","turnHost":"turn-smoke.k2.dev","label":"smoke","updatedAt":0}
EOF

# The pair `front install` writes, on a test port.
cat > "/run/systemd/system/$U.socket" <<EOF
[Unit]
Description=K2 front443 smoke socket

[Socket]
ListenStream=[::]:$FRONT_PORT
BindIPv6Only=both
FileDescriptorName=k2-front-443
Service=$U.service
NoDelay=true
Backlog=4096
EOF
cat > "/run/systemd/system/$U.service" <<EOF
[Unit]
Description=K2 front443 smoke daemon
Wants=$U.socket
After=$U.socket

[Service]
Type=simple
Sockets=$U.socket
KillMode=process
NotifyAccess=main
Restart=always
RestartSec=1
ExecReload=/bin/kill -USR2 \$MAINPID
Environment=HOME=$HOME_D PATH=/usr/local/bin:/usr/bin:/bin SHELL=/bin/false XDG_RUNTIME_DIR=$RUN
Environment=K2_TUNNEL_LEASE=off K2_CONNECT_BASE=http://127.0.0.1:1 K2_CERT_BROKER_URL=http://127.0.0.1:1
Environment=K2_HANDOFF_DRAIN_SECS=30 K2_HANDOFF_DROPIN=off K2_TEST_AGENT_SHIM_DIR=$W/shim
Environment=K2_HEARTBEAT_NO_SELF_HEAL=1 K2SO_WATCHDOG_DISABLED=1
ExecStart=$BIN
EOF
systemctl daemon-reload
systemctl start "$U.socket"
systemctl start "$U.service"

K2="$HOME_D/.k2"
main_pid() { systemctl show -p MainPID --value "$U.service"; }
status() { local p t; p=$(cat "$K2/daemon.port" 2>/dev/null) || return 1; t=$(cat "$K2/daemon.token" 2>/dev/null) || return 1; curl -s "http://127.0.0.1:$p/cli/front/status?token=$t"; }
serving_from() { status | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d.get("serving") and d["socket"]["source"]==sys.argv[1] else 1)' "$1" 2>/dev/null; }
leaf_cn() { # leaf_cn <sni|-> [connect-ip]
	local args=(-connect "${2:-127.0.0.1}:$FRONT_PORT")
	if [ "$1" = - ]; then args+=(-noservername); else args+=(-servername "$1"); fi
	timeout "${LEAF_TIMEOUT:-10}" openssl s_client "${args[@]}" </dev/null 2>/dev/null | openssl x509 -noout -subject 2>/dev/null | sed -n 's/.*CN *= *//p'
}

# 1. LISTEN_FDS.
wait_until 60 "front serving from systemd" serving_from systemd
status | grep -q '"host":"turn-smoke.k2.dev"' || fail "no TURN route: $(status)"
ok "LISTEN_FDS: the daemon serves the systemd socket ([::]:$FRONT_PORT), TURN route present"

# 2. Per SNI.
[ "$(leaf_cn turn-smoke.k2.dev)" = "turn-smoke.k2.dev" ] || fail "routed name did not reach the backend's TLS"
grep -q "proxy cmd=0x21 client=127.0.0.1:" "$W/backend.log" || fail "backend saw no PROXY v2 PROXY header with the client"
PUB=$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)
if [ -n "$PUB" ]; then
	[ "$(leaf_cn turn-smoke.k2.dev "$PUB")" = "turn-smoke.k2.dev" ] || fail "via $PUB: not routed"
	grep -q "client=$PUB:" "$W/backend.log" || fail "the backend did not log the real client $PUB"
	ok "s_client turn-smoke.k2.dev: backend's own leaf (front holds no key); PROXY v2 carried 127.0.0.1 and $PUB"
else
	ok "s_client turn-smoke.k2.dev: backend's own leaf; PROXY v2 carried 127.0.0.1"
fi
[ -z "$(leaf_cn nope.example)" ] || fail "unknown SNI was answered"
[ -z "$(leaf_cn -)" ] || fail "a no-SNI hello was answered"
status | python3 -c 'import json,sys; c=json.load(sys.stdin)["connections"]["closed"]; assert c["unknownSni"]>=1 and c["noSni"]>=1, c' || fail "close counters"
ok "unknown SNI and no-SNI hellos closed (counted)"

# 3. Hand-off keeps an established session and passes the socket on.
python3 - "$FRONT_PORT" "$W/hold.out" <<'PY' & HOLD_PID=$!
import socket, ssl, sys, time
port, out = int(sys.argv[1]), sys.argv[2]
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
t = ctx.wrap_socket(socket.create_connection(("127.0.0.1", port)), server_hostname="turn-smoke.k2.dev")
for i in range(40):
    t.sendall(b"ping%d" % i); r = t.recv(64)
    open(out, "a").write(r.decode() + "\n"); time.sleep(0.5)
PY
sleep 1
P1=$(main_pid); T=$(cat "$K2/daemon.token"); PORT=$(cat "$K2/daemon.port")
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/cli/daemon/handoff?token=$T")
[ "$code" = 202 ] || fail "handoff route answered $code"
wait_until 60 "MainPID moves off $P1" bash -c "p=\$(systemctl show -p MainPID --value '$U.service'); [ \"\$p\" != '$P1' ] && [ \"\$p\" != 0 ]"
wait_until 60 "successor serves the handed-over socket" serving_from handoff
[ "$(leaf_cn turn-smoke.k2.dev)" = "turn-smoke.k2.dev" ] || fail "successor does not route"
wait "$HOLD_PID" || fail "the held session died in the hand-off"; HOLD_PID=""
[ "$(grep -c ping "$W/hold.out")" = 40 ] || fail "held session lost echoes: $(wc -l < "$W/hold.out")"
ok "hand-off: Role::Front443 moved to $(main_pid); an established session kept echoing through it"

# 4. kill -9: systemd holds the socket, a queued connection is served.
P2=$(main_pid)
kill -9 "$P2"
LEAF_TIMEOUT=60 leaf_cn turn-smoke.k2.dev > "$W/queued.cn" &
Q=$!
wait "$Q" || true
[ "$(cat "$W/queued.cn")" = "turn-smoke.k2.dev" ] || fail "a connection made during the restart was not served"
wait_until 60 "restarted daemon serves from systemd" serving_from systemd
ok "kill -9: systemd kept the socket; the connection queued meanwhile was served after the restart"

# 5. Stop the daemon: systemd still listens; a connection starts it.
systemctl stop "$U.service"
ss -ltn "( sport = :$FRONT_PORT )" | grep -q LISTEN || fail "the socket stopped listening with the daemon"
[ "$(systemctl is-active "$U.socket")" = active ] || fail "socket unit not active"
LEAF_TIMEOUT=60 leaf_cn turn-smoke.k2.dev > "$W/activated.cn" || true
[ "$(cat "$W/activated.cn")" = "turn-smoke.k2.dev" ] || fail "the activating connection was not served"
wait_until 60 "socket activation started the daemon" bash -c "[ \"\$(systemctl is-active '$U.service')\" = active ]"
ok "systemctl stop: systemd kept :$FRONT_PORT; a connection started the daemon again (Service=)"
echo "PASS"
