#!/usr/bin/env bash
# front443 S4 — Skin Direct's Caddyfile behind the front, against real
# Caddy binaries (Linux, root for the private network namespace).
#
# Inputs: the two files the k2-core renderer writes with
# `K2_FRONT443_CADDYFILE_DIR=<dir> cargo test -p k2-core --lib
#  skin_door::tests::behind_the_front` (front-proxy.Caddyfile = Caddy >= 2.7
# with the scoped proxy_protocol wrapper; front-noproxy.Caddyfile = distro
# Caddy 2.6.2, no wrapper).
#
# Checks:
#   1. Caddy 2.6.2 adapts the no-PROXY file, and REFUSES the PROXY one
#      (why the daemon gates the wrapper on `caddy version`);
#   2. Caddy >= 2.7 adapts the PROXY file with the wrapper on the
#      127.0.0.1:38473 server only (never the 38472 path filter);
#   3. running it (internal certs, private netns): a PROXY v2 header for
#      198.51.100.77 + TLS reaches the Skin Direct site and Caddy logs
#      198.51.100.77 as the client; the 38472 loopback door still answers
#      plain HTTP without any header.
#
# Usage: front443_caddy.sh <caddy-2.6.2> <caddy-2.7+> <dir with the Caddyfiles>
set -euo pipefail
C26="$(readlink -f "$1")"; C27="$(readlink -f "$2")"; D="$(readlink -f "$3")"
[ "$(id -u)" = 0 ] || { echo "needs root (unshare -n)"; exit 2; }
if [ "${K2_F443_IN_NETNS:-}" != 1 ]; then
	exec env K2_F443_IN_NETNS=1 unshare -n "$0" "$@"
fi
ip link set lo up
W="$(mktemp -d /tmp/k2-f443-caddy-XXXXXX)"
CPID=""; DPID=""
cleanup() { [ -n "$CPID" ] && kill "$CPID" 2>/dev/null; [ -n "$DPID" ] && kill "$DPID" 2>/dev/null; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
fail() { echo "FAIL: $*"; [ -f "$W/caddy.log" ] && tail -30 "$W/caddy.log"; exit 1; }
ok() { echo "ok - $*"; }
"$C26" version; "$C27" version

# 1. Distro Caddy.
"$C26" adapt --config "$D/front-noproxy.Caddyfile" >"$W/26.json" 2>"$W/26.err" || fail "2.6.2 refused the no-PROXY file: $(cat "$W/26.err")"
if "$C26" adapt --config "$D/front-proxy.Caddyfile" >/dev/null 2>"$W/26p.err"; then
	fail "2.6.2 accepted proxy_protocol (the version gate would be pointless)"
fi
ok "Caddy 2.6.2 adapts the no-PROXY file and refuses proxy_protocol ($(tr '\n' ' ' < "$W/26p.err" | cut -c1-120))"

# 2. The wrapper is scoped to the front's server.
"$C27" adapt --config "$D/front-proxy.Caddyfile" >"$W/27.json" 2>"$W/27.err" || fail "2.7+ refused the PROXY file: $(cat "$W/27.err")"
python3 - "$W/27.json" <<'PY' || exit 1
import json, sys
servers = json.load(open(sys.argv[1]))["apps"]["http"]["servers"]
front = [s for s in servers.values() if "127.0.0.1:38473" in s.get("listen", [])]
door = [s for s in servers.values() if any("38472" in l for l in s.get("listen", []))]
assert len(front) == 1, servers
wr = [w.get("wrapper") for w in front[0].get("listener_wrappers", [])]
assert wr[:2] == ["proxy_protocol", "tls"], wr
pp = front[0]["listener_wrappers"][0]
assert sorted(pp.get("allow", [])) == ["127.0.0.1/32", "::1/128"], pp
assert door and not any(s.get("listener_wrappers") for s in door), "the 38472 door must not parse PROXY"
print("ok - proxy_protocol + tls wrap only the 127.0.0.1:38473 server; the 38472 door has none")
PY

# 3. Run it: internal certs, an access log, a stand-in daemon on the
#    Caddyfile's daemon port (the path filter proxies /boot-status there).
DAEMON_PORT=$(grep -o 'reverse_proxy 127.0.0.1:[0-9]*' "$D/front-proxy.Caddyfile" | head -1 | cut -d: -f2)
sed -e 's/^\tadmin off$/\tadmin off\n\tlocal_certs\n\tskip_install_trust/' \
    -e "s|^\(https://skin.acme.dev:38473 {\)$|\1\n\tlog {\n\t\toutput file $W/access.log\n\t}|" \
    "$D/front-proxy.Caddyfile" > "$W/run.Caddyfile"
python3 -c 'import http.server,sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        b=b"{\"ok\":true}"; self.send_response(200); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
http.server.HTTPServer(("127.0.0.1",int(sys.argv[1])),H).serve_forever()' "$DAEMON_PORT" & DPID=$!
XDG_DATA_HOME="$W/data" XDG_CONFIG_HOME="$W/cfg" "$C27" run --config "$W/run.Caddyfile" --adapter caddyfile >"$W/caddy.log" 2>&1 & CPID=$!
for _ in $(seq 100); do ss -ltn | grep -q '127.0.0.1:38473' && break; sleep 0.1; done
ss -ltn | grep -q '127.0.0.1:38473' || fail "Caddy is not listening on 127.0.0.1:38473"
python3 - <<'PY' || fail "PROXY v2 request through Caddy"
import socket, ssl, struct, time
SIG = b"\r\n\r\n\x00\r\nQUIT\n"
hdr = SIG + bytes([0x21, 0x11]) + struct.pack("!H", 12) + socket.inet_aton("198.51.100.77") + socket.inet_aton("127.0.0.1") + struct.pack("!HH", 40123, 443)
for attempt in range(50):
    try:
        s = socket.create_connection(("127.0.0.1", 38473), timeout=5); s.sendall(hdr)
        ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
        t = ctx.wrap_socket(s, server_hostname="skin.acme.dev")
        t.sendall(b"GET /boot-status HTTP/1.1\r\nHost: skin.acme.dev\r\nConnection: close\r\n\r\n")
        r = b""
        while True:
            d = t.recv(4096)
            if not d: break
            r += d
        print("caddy answered:", r.split(b"\r\n", 1)[0].decode())
        assert r.startswith(b"HTTP/1.1 200"), r[:200]
        break
    except (ssl.SSLError, ConnectionError) as e:
        time.sleep(0.2)  # the internal cert is still being issued
else:
    raise SystemExit("no TLS answer from Caddy")
PY
for _ in $(seq 50); do grep -q '"remote_ip":"198.51.100.77"' "$W/access.log" 2>/dev/null && break; sleep 0.1; done
grep -q '"remote_ip":"198.51.100.77"' "$W/access.log" || fail "Caddy did not log the PROXY source: $(tail -2 "$W/access.log")"
ok "Caddy >= 2.7 behind the front: PROXY v2 source 198.51.100.77 is the client Caddy logs"
code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:38472/boot-status)
[ "$code" = 200 ] || fail "the 38472 door answered $code without a PROXY header"
ok "the 38472 loopback door still serves plain HTTP with no header"
echo "PASS"
