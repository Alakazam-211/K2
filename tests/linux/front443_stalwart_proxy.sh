#!/usr/bin/env bash
# front443 S5 — Stalwart's HTTPS listener behind the front, against the
# REAL pinned Stalwart binary (Linux, root, a private network namespace so
# no port on the box is touched).
#
# Bootstraps a scratch Stalwart, applies exactly the registry update K2
# makes (`mail::front_listener::https_update_args`: https → 127.0.0.1:8443
# + overrideProxyTrustedNetworks {127.0.0.1, ::1}), restarts it, then:
#   1. a loopback TLS connection WITHOUT a PROXY header is dropped before
#      any session (it can never authenticate or fail, so it can't be
#      banned);
#   2. with a PROXY v2 header for 198.51.100.77, TLS works and failed
#      logins ban 198.51.100.77 — never 127.0.0.1;
#   3. the management listener on loopback keeps answering;
#   4. the reverse update (bind [::]:443, overrideProxyTrustedNetworks {})
#      clears the trust and plain TLS works again.
#
# Usage: front443_stalwart_proxy.sh <path/to/stalwart 0.16.25 binary>
set -euo pipefail
BIN="$(readlink -f "${1:?usage: $0 <stalwart binary>}")"
[ "$(id -u)" = 0 ] || { echo "needs root (unshare -n)"; exit 2; }
if [ "${K2_F443_IN_NETNS:-}" != 1 ]; then
	exec env K2_F443_IN_NETNS=1 unshare -n "$0" "$@"
fi
ip link set lo up
W="$(mktemp -d /tmp/k2-f443-stalwart-XXXXXX)"
SPID=""
cleanup() { [ -n "$SPID" ] && kill "$SPID" 2>/dev/null; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
mkdir -p "$W/etc" "$W/data" "$W/log"
PW="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
"$BIN" --version || true

start() { # start [recovery]
	if [ "${1:-}" = recovery ]; then
		STALWART_RECOVERY_ADMIN="admin:$PW" "$BIN" --config "$W/etc/config.json" >>"$W/log/stalwart.log" 2>&1 &
	else
		"$BIN" --config "$W/etc/config.json" >>"$W/log/stalwart.log" 2>&1 &
	fi
	SPID=$!
}
stop() { kill "$SPID"; wait "$SPID" 2>/dev/null || true; SPID=""; }
fail() { echo "FAIL: $*"; tail -40 "$W/log/stalwart.log"; exit 1; }

cat > "$W/drive.py" <<'PY'
import base64, json, os, socket, ssl, struct, sys, time, urllib.request
W, PW, step = sys.argv[1], sys.argv[2], sys.argv[3]
CREDS = os.path.join(W, "creds.json")
def auth():
    if os.path.exists(CREDS):
        c = json.load(open(CREDS)); return c["username"], c["secret"]
    return "admin", PW
def http(path, body=None):
    u, s = auth()
    req = urllib.request.Request("http://127.0.0.1:8080" + path, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Authorization": "Basic " + base64.b64encode(f"{u}:{s}".encode()).decode(),
                                          "Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=10).read())
def call(method, args):
    sess = http("/jmap/session")
    args.setdefault("accountId", sess["primaryAccounts"].get("urn:stalwart:jmap") or next(iter(sess["accounts"])))
    api = "/" + sess["apiUrl"].split("://", 1)[-1].split("/", 1)[1]
    r = http(api, {"using": ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"], "methodCalls": [[method, args, "0"]]})
    name, out, _ = r["methodResponses"][0]
    assert name == method, (method, r)
    return out
def wait_up(tries=150):
    for _ in range(tries):
        try:
            http("/jmap/session"); return
        except Exception:
            time.sleep(0.2)
    raise SystemExit("stalwart admin API never came up")
def listeners():
    return {l["name"]: l for l in call("x:NetworkListener/get", {"ids": None})["list"]}
SIG = b"\r\n\r\n\x00\r\nQUIT\n"
def proxy_v2(src_ip, src_port, dst_ip="127.0.0.1", dst_port=8443):
    return SIG + bytes([0x21, 0x11]) + struct.pack("!H", 12) + socket.inet_aton(src_ip) + socket.inet_aton(dst_ip) + struct.pack("!HH", src_port, dst_port)
def tls(port, header=b"", alpn=None):
    s = socket.create_connection(("127.0.0.1", port), timeout=5)
    if header: s.sendall(header)
    ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
    if alpn: ctx.set_alpn_protocols(alpn)
    return ctx.wrap_socket(s, server_hostname="mail.front.test")
def get_session_401(t):
    t.sendall(b"GET /jmap/session HTTP/1.1\r\nHost: mail.front.test\r\nAuthorization: Basic " + base64.b64encode(b"nobody@front.test:wrong") + b"\r\nConnection: close\r\n\r\n")
    data = b""
    while True:
        d = t.recv(4096)
        if not d: break
        data += d
    return data.split(b"\r\n", 1)[0].decode(errors="replace")

if step == "bootstrap":
    wait_up()
    out = call("x:Bootstrap/set", {"update": {"singleton": {
        "serverHostname": "mail.front.test", "defaultDomain": "front.test", "requestTlsCertificate": False,
        "generateDkimKeys": False, "dataStore": {"@type": "RocksDb", "path": os.path.join(W, "data")}}}})
    u = out["updated"]["singleton"]
    json.dump({"username": u["username"], "secret": u["secret"]}, open(CREDS, "w"))
    print("bootstrapped; admin", u["username"])
elif step == "front-on":
    wait_up()
    ls = listeners()
    print("listeners:", {n: sorted(l.get("bind") or {}) for n, l in ls.items()})
    https = ls["https"]
    out = call("x:Security/set", {"update": {"singleton": {"authBanRate": {"count": 3, "period": 60000}}}})
    assert "singleton" in (out.get("updated") or {}), out
    # Exactly K2's update (mail::front_listener::https_update_args, on).
    out = call("x:NetworkListener/set", {"update": {https["id"]: {"bind": {"127.0.0.1:8443": True},
        "overrideProxyTrustedNetworks": {"127.0.0.1": True, "::1": True}}}})
    assert not out.get("notUpdated"), out
    got = listeners()["https"]
    assert sorted(got["bind"]) == ["127.0.0.1:8443"], got
    assert sorted(got["overrideProxyTrustedNetworks"]) == ["127.0.0.1", "::1"], got
    print("ok - https listener stored at 127.0.0.1:8443 trusting loopback PROXY headers")
elif step == "verify-on":
    wait_up()
    # 1. No header from a trusted (loopback) address: dropped pre-session.
    try:
        t = tls(8443)
        raise SystemExit("FAIL: a header-less loopback TLS connection was served")
    except (ssl.SSLError, ConnectionError, OSError) as e:
        print("ok - loopback without a PROXY header is dropped before TLS:", type(e).__name__)
    # 2. With PROXY v2 from 198.51.100.77: TLS works; failed logins ban it.
    lines = []
    for i in range(6):
        try:
            lines.append(get_session_401(tls(8443, proxy_v2("198.51.100.77", 40000 + i), alpn=["http/1.1"])))
        except (ssl.SSLError, ConnectionError, OSError) as e:
            lines.append("closed: " + type(e).__name__)
        time.sleep(0.2)
    print("attempts:", lines)
    assert lines[0].startswith("HTTP/1.1 401"), lines
    assert any(l.startswith("closed") for l in lines[3:]), "the proxied client was never banned: %s" % lines
    blocked = [b["address"] for b in call("x:BlockedIp/get", {"ids": None})["list"]]
    print("blocked:", blocked)
    assert any(a.startswith("198.51.100.77") for a in blocked), blocked
    assert not any(a.startswith("127.") or a.startswith("::1") for a in blocked), "loopback was banned: %s" % blocked
    print("ok - PROXY v2 source reached Stalwart: the ban hit 198.51.100.77, never loopback")
    # 3. Loopback management keeps working (this very call).
    call("x:NetworkListener/get", {"ids": None})
    print("ok - the loopback management listener still answers")
    # Another proxied address is untouched by the ban.
    print("other client:", get_session_401(tls(8443, proxy_v2("203.0.113.9", 41000))))
elif step == "front-off":
    wait_up()
    https = listeners()["https"]
    out = call("x:NetworkListener/set", {"update": {https["id"]: {"bind": {"[::]:443": True}, "overrideProxyTrustedNetworks": {}}}})
    assert not out.get("notUpdated"), out
    got = listeners()["https"]
    assert sorted(got["bind"]) == ["[::]:443"], got
    assert not got.get("overrideProxyTrustedNetworks"), got
    print("ok - reverse update: [::]:443, no PROXY trust left")
elif step == "verify-off":
    wait_up()
    t = tls(443, alpn=["http/1.1"])
    print("ok - plain TLS on :443 again:", get_session_401(t))
PY

start recovery
python3 "$W/drive.py" "$W" "$PW" bootstrap || fail "bootstrap"
stop
start
python3 "$W/drive.py" "$W" "$PW" front-on || fail "front-on"
stop
start
python3 "$W/drive.py" "$W" "$PW" verify-on || fail "verify-on"
python3 "$W/drive.py" "$W" "$PW" front-off || fail "front-off"
stop
start
python3 "$W/drive.py" "$W" "$PW" verify-off || fail "verify-off"
echo "PASS"
