#!/usr/bin/env bash
# front443 with a LIVE daemon, inside a throwaway systemd container (see
# run.sh). Real k2-root-helper (sudoers + daemon.toml as a provisioned box
# has them), real k2-daemon under k2-daemon.service (Restart=always, the
# hand-off drop-in the daemon installs itself), real `k2 front` CLI.
#
#   1. the daemon is up and active BEFORE the front exists (the field case);
#   2. `k2 front enable` → helper `front install` (never starts the socket:
#      systemd refuses that while the daemon is active) → the daemon
#      restarts once → systemd starts the socket first and hands the fd;
#   3. voice in Sew's order: the TURN route is proved through the front
#      before any reconfigure (a stand-in Sew on 127.0.0.1:5349);
#   4. routing: passthrough TURN name → the backend's own leaf, PROXY v2
#      with the client; unknown SNI closed;
#   5. kill -9 and systemctl restart keep the front;
#   6. `k2 front disable` with the daemon live: no restart, socket gone,
#      :443 free; then re-enable.
set -euo pipefail
EV=/evidence/log.txt
: > "$EV"
say() { echo "$*" | tee -a "$EV"; }
fail() {
	say "FAIL: $*"
	{ echo "--- daemon journal ---"; journalctl -u k2-daemon.service --no-pager | grep -E "front443|handoff|rlimit|FATAL|panicked" | tail -60
	  echo "--- socket journal ---"; journalctl -u k2-front-443.socket --no-pager | tail -20; } >> "$EV"
	exit 1
}
ok() { say "ok - $*"; }
wait_until() { local n=$(( $1 * 10 )) what=$2; shift 2; for _ in $(seq "$n"); do "$@" && return 0; sleep 0.1; done; fail "timed out: $what"; }
K2H=/home/k2/.k2
k2() { sudo -u k2 env HOME=/home/k2 PATH=/opt/k2:/usr/bin:/bin /opt/k2/k2 "$@"; }
status() { local p t; p=$(cat $K2H/daemon.port 2>/dev/null) || return 1; t=$(cat $K2H/daemon.token 2>/dev/null) || return 1; curl -s "http://127.0.0.1:$p/cli/front/status?token=$t"; }
jq_() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
ready() { [ "$(status | jq_ 'd.get("enabled") is not None' 2>/dev/null)" = True ]; }
main_pid() { systemctl show -p MainPID --value k2-daemon.service; }
serving_from() { [ "$(status | jq_ 'd["serving"] and d["socket"]["source"]' 2>/dev/null)" = "$1" ]; }
export K2H; export -f status jq_ main_pid

# ── a provisioned box: user, helper, sudoers, daemon.toml, unit ───────────
id k2 >/dev/null 2>&1 || useradd -m -s /bin/bash k2
install -d -m 0755 /usr/local/libexec
install -o root -g root -m 0755 /opt/k2/k2-root-helper /usr/local/libexec/k2-root-helper
echo 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-root-helper' > /etc/sudoers.d/k2-root-helper
chmod 0440 /etc/sudoers.d/k2-root-helper
install -d -o root -g root -m 0755 /etc/k2-root-helper
printf '[[daemon]]\nuser = "k2"\nunit = "k2-daemon.service"\nscope = "system"\n' > /etc/k2-root-helper/daemon.toml
chmod 0644 /etc/k2-root-helper/daemon.toml
install -d -o k2 -g k2 /home/k2/.k2/sew /home/k2/shim
chown -R k2:k2 /home/k2
cat > /etc/systemd/system/k2-daemon.service <<'EOF'
[Unit]
Description=K2 daemon (front443 live test)
[Service]
Type=simple
User=k2
Environment=HOME=/home/k2 PATH=/opt/k2:/usr/local/bin:/usr/bin:/bin SHELL=/bin/false
Environment=K2_TUNNEL_LEASE=off K2_CONNECT_BASE=http://127.0.0.1:1 K2_CERT_BROKER_URL=http://127.0.0.1:1
Environment=K2_HEARTBEAT_NO_SELF_HEAL=1 K2SO_WATCHDOG_DISABLED=1
ExecStart=/opt/k2/k2-daemon
Restart=always
RestartSec=1
[Install]
WantedBy=multi-user.target
EOF

# ── a stand-in Sew on 127.0.0.1:5349 ─────────────────────────────────────
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj "/CN=turn-live.k2.dev" \
	-addext "subjectAltName=DNS:turn-live.k2.dev" -keyout /evidence/turn.key -out /evidence/turn.crt >/dev/null 2>&1
LEAF=$(openssl x509 -in /evidence/turn.crt -outform der | sha256sum | cut -d' ' -f1)
cat > /evidence/sew.py <<'PY'
# direct mode: TLS straight away (sew-media's public 5349 listener);
# passthrough mode: PROXY v2 first (logged), then TLS. A probe's PROXY
# header reaching the direct mode is logged too.
import socket, ssl, struct, sys, threading
mode_file, cert, key, log = sys.argv[1:5]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(cert, key)
SIG = b"\r\n\r\n\x00\r\nQUIT\n"
def L(m): open(log, "a").write(m + "\n")
def serve(c):
    try:
        mode = open(mode_file).read().strip()
        head = c.recv(16, socket.MSG_PEEK)
        if head[:12] == SIG:
            h = c.recv(16); body = c.recv(struct.unpack("!H", h[14:16])[0])
            fam = h[13]
            src = socket.inet_ntop(socket.AF_INET if fam == 0x11 else socket.AF_INET6, body[0:4] if fam == 0x11 else body[0:16])
            L("%s proxy-v2 client=%s" % (mode, src))
            if mode != "passthrough":
                L("direct: PROXY header refused (a probe)"); return
        elif mode == "passthrough":
            L("passthrough: missing PROXY header"); return
        t = ctx.wrap_socket(c, server_side=True)
        t.sendall(b"pong")
    except Exception as e:
        L("error %r" % (e,))
    finally:
        c.close()
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", 5349)); s.listen(64)
while True:
    c, _ = s.accept(); threading.Thread(target=serve, args=(c,), daemon=True).start()
PY
echo direct > /evidence/sew.mode
python3 /evidence/sew.py /evidence/sew.mode /evidence/turn.crt /evidence/turn.key /evidence/sew.log &
voice() { # voice <installed> <tier>
	cat > $K2H/sew/voice.json <<EOF
{"installed":$1,"turnTier":"$2","turnHost":"turn-live.k2.dev","label":"live","nodeIp":"127.0.0.1","certLeafSha256":"$LEAF","updatedAt":0}
EOF
	chown k2:k2 $K2H/sew/voice.json; chmod 600 $K2H/sew/voice.json
}
voice true direct

# ── 1. the daemon is live before the front exists ────────────────────────
systemctl daemon-reload
systemctl start k2-daemon.service
wait_until 120 "daemon ready" ready
wait_until 120 "hand-off drop-in installed by the daemon" test -f /etc/systemd/system/k2-daemon.service.d/k2-handoff.conf
P1=$(main_pid)
[ "$(systemctl is-active k2-daemon.service)" = active ] || fail "daemon not active"
[ ! -e /etc/systemd/system/k2-front-443.socket ] || fail "socket unit present before enable"
ok "daemon live (MainPID $P1, hand-off drop-in in place), no front yet"

# ── 1b. :443 taken: refused before anything is written, naming the holder ─
python3 -m http.server 443 --bind :: >/dev/null 2>&1 & SQUAT=$!
wait_until 10 "squatter on :443" bash -c "ss -ltn '( sport = :443 )' | grep -q LISTEN"
if OUT=$(k2 front enable 2>&1); then fail "enable succeeded with :443 taken: $OUT"; fi
echo "$OUT" | sed 's/^/  | /' | tee -a "$EV"
echo "$OUT" | grep -q "port 443 is already in use on this box by \[::\]:443 (python3\[$SQUAT\])" || fail "the refusal does not name the holder"
[ ! -e /etc/systemd/system/k2-front-443.socket ] || fail "socket unit written despite the refusal"
[ "$(main_pid)" = "$P1" ] || fail "the daemon restarted on a refused enable"
kill "$SQUAT"; wait "$SQUAT" 2>/dev/null || true
ok ":443 taken: enable refused with the holder named, nothing written, no restart"

# ── 2. enable with the daemon live ───────────────────────────────────────
OUT=$(k2 front enable 2>&1) || fail "k2 front enable: $OUT"
echo "$OUT" | sed 's/^/  | /' | tee -a "$EV"
echo "$OUT" | grep -q "K2 will restart its daemon once to take port 443" || fail "no restart message"
wait_until 60 "daemon restarted (MainPID off $P1)" bash -c "p=\$(systemctl show -p MainPID --value k2-daemon.service); [ \"\$p\" != '$P1' ] && [ \"\$p\" != 0 ]"
P2=$(main_pid)
wait_until 90 "front serving the systemd socket" serving_from systemd
[ "$(systemctl is-active k2-front-443.socket)" = active ] || fail "socket unit not active"
ss -ltnp '( sport = :443 )' | tail -n +2 | tee -a "$EV"
ok "enable: helper install + one daemon restart ($P1 -> $P2); systemd started the socket and passed the fd (serving from systemd)"

# ── 3. voice in Sew's order ──────────────────────────────────────────────
wait_until 60 "the TURN route proved through the front before any reconfigure" \
	bash -c "journalctl -u k2-daemon.service --no-pager | grep -q 'TURN route proved through the front'"
grep -q "direct proxy-v2 client=" /evidence/sew.log || fail "the probe never reached the stand-in Sew"
journalctl -u k2-daemon.service --no-pager | grep -E "TURN route proved|voice (moved|tier not changed)" | tail -3 | sed 's/^/  | /' | tee -a "$EV"
ok "voice: the probe went through the front to 127.0.0.1:5349 first; the reconfigure step ran only after it"

# ── 4. routing on :443 ───────────────────────────────────────────────────
echo passthrough > /evidence/sew.mode
voice true passthrough
wait_until 45 "TURN route installed" bash -c "status | grep -q turn-live.k2.dev"
CN=$(timeout 10 openssl s_client -connect 127.0.0.1:443 -servername turn-live.k2.dev </dev/null 2>/dev/null | openssl x509 -noout -subject | sed -n 's/.*CN *= *//p')
[ "$CN" = turn-live.k2.dev ] || fail "turn-live.k2.dev not routed (got '$CN')"
grep -q "passthrough proxy-v2 client=127.0.0.1" /evidence/sew.log || fail "no PROXY v2 client on the passthrough backend"
OTHER=$(timeout 10 openssl s_client -connect 127.0.0.1:443 -servername nope.example </dev/null 2>/dev/null | openssl x509 -noout -subject 2>/dev/null || true)
[ -z "$OTHER" ] || fail "unknown SNI answered"
ok "routing: TURN SNI -> the backend's own leaf, PROXY v2 carried the client; unknown SNI closed"

# ── 5. kill -9 and systemctl restart keep the front ──────────────────────
kill -9 "$P2"
wait_until 60 "restart after kill -9" bash -c "p=\$(systemctl show -p MainPID --value k2-daemon.service); [ \"\$p\" != '$P2' ] && [ \"\$p\" != 0 ]"
wait_until 90 "serving after kill -9" serving_from systemd
systemctl restart k2-daemon.service
wait_until 90 "serving after systemctl restart" serving_from systemd
ok "kill -9 and systemctl restart: the daemon comes back on the systemd socket"

# ── 6. disable with the daemon live, then re-enable ──────────────────────
voice false direct
P3=$(main_pid)
OUT=$(k2 front disable 2>&1) || fail "k2 front disable: $OUT"
echo "$OUT" | sed 's/^/  | /' | tee -a "$EV"
sleep 2
[ "$(main_pid)" = "$P3" ] || fail "disable restarted the daemon"
[ "$(systemctl is-active k2-front-443.socket || true)" != active ] || fail "socket still active"
[ ! -e /etc/systemd/system/k2-front-443.socket ] && [ ! -e /etc/systemd/system/k2-daemon.service.d/k2-front.conf ] || fail "files left"
! ss -ltn '( sport = :443 )' | grep -q LISTEN || fail ":443 still listening after disable"
[ "$(status | jq_ 'd["serving"]')" = False ] || fail "still serving"
ok "disable with the daemon live: no restart (MainPID $P3), socket stopped, files gone, :443 free"

OUT=$(k2 front enable 2>&1) || fail "re-enable: $OUT"
wait_until 60 "re-enable restart" bash -c "p=\$(systemctl show -p MainPID --value k2-daemon.service); [ \"\$p\" != '$P3' ] && [ \"\$p\" != 0 ]"
wait_until 90 "serving after re-enable" serving_from systemd
ok "re-enable with the daemon live: one restart, serving the systemd socket again"
say "PASS"
