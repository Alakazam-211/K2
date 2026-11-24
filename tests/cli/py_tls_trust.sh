#!/usr/bin/env bash
# bash-only: it reads _K2_PY_TLS and _K2_PY_PROXY_RULE out of cli/k2 itself.
#
# Which CAs the CLI's embedded Python trusts for https (_K2_PY_TLS). With
# SSL_CERT_FILE or SSL_CERT_DIR set, only the CAs they name: a server whose
# certificate one of them signed verifies, and one signed by another CA is
# refused. With neither set (an empty value counts as unset), Python's own
# default trust, which a throwaway CA is not in: refused. A variable naming
# a file that isn't there fails the request. A refused server gets no
# request. Each row runs through the three ways cli/src opens https: the
# rule's opener (K2TLSHandler), k2_tls_install() for a block without the
# rule, and k2_tls_context() on a raw socket (k2 app shell).
#
# The point on a Mac: Apple's python3 (LibreSSL) ignores SSL_CERT_FILE by
# itself, so without the helper the "verifies" rows fail there.
#
# Run on compute (it starts a TLS stub and opens a port):
#   bash tests/cli/py_tls_trust.sh
# No daemon, no CLI run; never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"

for v in $(env | grep -E '^(K2|K2SO|K5)_' | cut -d= -f1); do unset "$v"; done
unset PYTHONPATH SSL_CERT_FILE SSL_CERT_DIR

PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }
command -v openssl >/dev/null || { echo "FAIL: openssl not found" >&2; exit 1; }

W="$(mktemp -d /tmp/k2tls-XXXXXX)"
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    rm -rf "$W"
}
trap cleanup EXIT

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

# The two snippets, as cli/k2 defines them (eval of their assignments
# only): the rule's literal, then _K2_PY_TLS, cut from its first part.
RULE_SRC="$(awk -v q="'" '$0 == "_K2_PY_PROXY_RULE=" q {on=1} on {print} on && $0 == q {exit}' "$CLI_BASH_SOURCE")"
TLS_SRC="$(grep '^_K2_PY_TLS=' "$CLI_BASH_SOURCE")"
[ -n "$RULE_SRC" ] || { echo "FAIL: no _K2_PY_PROXY_RULE in $CLI_BASH_SOURCE" >&2; exit 1; }
[ -n "$TLS_SRC" ] || { echo "FAIL: no _K2_PY_TLS in $CLI_BASH_SOURCE" >&2; exit 1; }
eval "$RULE_SRC"
eval "$TLS_SRC"
expect_has "_K2_PY_TLS holds the helper" "$_K2_PY_TLS" "def k2_tls_install"
case "$_K2_PY_TLS" in *install_opener\(_k2_ur*) bad "_K2_PY_TLS holds the rule's own opener too" ;; *) ok "_K2_PY_TLS stops before the rule's own code" ;; esac
expect_has "the rule carries the helper" "$_K2_PY_PROXY_RULE" "def k2_tls_context"
expect_has "the rule's opener uses K2TLSHandler" "$_K2_PY_PROXY_RULE" "K2TLSHandler()))"
export _K2_PY_TLS _K2_PY_PROXY_RULE

# Two throwaway CAs; a leaf for localhost signed by the first. Each CA is
# a real one (basicConstraints CA:TRUE, keyUsage keyCertSign): Python 3.13
# and newer verify strictly and refuse a CA without keyUsage. Made from a
# CSR self-signed with an extfile, which OpenSSL and LibreSSL both take.
mkdir -p "$W/tls" "$W/certdir"
printf 'basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n' >"$W/tls/ca.ext"
for ca in ca other; do
    openssl req -new -newkey rsa:2048 -nodes -subj "/CN=Acme throwaway $ca CA" \
        -keyout "$W/tls/$ca.key" -out "$W/tls/$ca.csr" >>"$W/tls/log" 2>&1 \
        && openssl x509 -req -in "$W/tls/$ca.csr" -signkey "$W/tls/$ca.key" -days 2 \
            -extfile "$W/tls/ca.ext" -out "$W/tls/$ca.pem" >>"$W/tls/log" 2>&1 \
        || { cat "$W/tls/log" >&2; echo "FAIL: openssl could not make the $ca CA" >&2; exit 1; }
done
printf 'subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n' >"$W/tls/leaf.ext"
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout "$W/tls/leaf.key" -out "$W/tls/leaf.csr" >>"$W/tls/log" 2>&1 \
    && openssl x509 -req -in "$W/tls/leaf.csr" -CA "$W/tls/ca.pem" -CAkey "$W/tls/ca.key" -CAcreateserial \
        -days 2 -extfile "$W/tls/leaf.ext" -out "$W/tls/leaf.pem" >>"$W/tls/log" 2>&1 \
    || { cat "$W/tls/log" >&2; echo "FAIL: openssl could not make the leaf" >&2; exit 1; }
# SSL_CERT_DIR holds the CA under its subject hash, as OpenSSL looks it up.
cp "$W/tls/ca.pem" "$W/certdir/$(openssl x509 -hash -noout -in "$W/tls/ca.pem").0"

# The stub: https on loopback with the leaf; one line per request it got.
cat >"$W/stub.py" <<'PYEOF'
import http.server, os, ssl, sys
W, port_file = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(os.path.join(W, "requests"), "a") as f:
            f.write(self.path + "\n")
        data = b'{"ok": true}'
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(os.path.join(W, "tls", "leaf.pem"), os.path.join(W, "tls", "leaf.key"))
srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
with open(port_file + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(port_file + ".tmp", port_file)
srv.serve_forever()
PYEOF
"$PY" "$W/stub.py" "$W" "$W/port" 2>"$W/stub.err" & PIDS+=("$!")
for _ in $(seq 1 100); do [ -s "$W/port" ] && break; sleep 0.1; done
[ -s "$W/port" ] || { cat "$W/stub.err" >&2; echo "FAIL: the https stub never came up" >&2; exit 1; }
URL="https://localhost:$(cat "$W/port")"

# client.py WAY URL: one GET to URL the WAY cli/src does it; prints "ok
# <status>" or "refused <error type>: <reason>".
cat >"$W/client.py" <<'PYEOF'
import os, socket, ssl, sys, urllib.error, urllib.parse, urllib.request
way, url = sys.argv[1], sys.argv[2]
try:
    if way == "rule":
        exec(os.environ["_K2_PY_PROXY_RULE"])
        r = urllib.request.urlopen(url + "/rule", timeout=10)
        print("ok", r.status)
    elif way == "install":
        exec(os.environ["_K2_PY_TLS"]); k2_tls_install()
        r = urllib.request.urlopen(url + "/install", timeout=10)
        print("ok", r.status)
    elif way == "socket":
        exec(os.environ["_K2_PY_TLS"])
        u = urllib.parse.urlparse(url)
        s = socket.create_connection((u.hostname, u.port), timeout=10)
        s = (k2_tls_context() or ssl.create_default_context()).wrap_socket(s, server_hostname=u.hostname)
        s.sendall(b"GET /socket HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        head = s.recv(64)
        print("ok", head.split(b" ")[1].decode())
except urllib.error.URLError as e:
    print("refused URLError:", e.reason)
except OSError as e:
    print("refused %s: %s" % (type(e).__name__, e))
PYEOF

# row LABEL WANT ENV…: each way, with only ENV set for trust, gives WANT
# ("ok" or "refused", the reason holding $WHY); a refused way leaves the
# stub without its request.
row() {
    local label="$1" want="$2"; shift 2
    local way got
    for way in rule install socket; do
        : >"$W/requests"
        got="$(env -u SSL_CERT_FILE -u SSL_CERT_DIR "$@" "$PY" "$W/client.py" "$way" "$URL" 2>&1)"
        if [ "$want" = ok ]; then
            expect_eq "$label ($way): verifies" "$got" "ok 200"
            expect_eq "$label ($way): the stub got the request" "$(cat "$W/requests")" "/$way"
        else
            expect_has "$label ($way): refused" "$got" "refused"
            expect_has "$label ($way): says $WHY" "$got" "$WHY"
            expect_eq "$label ($way): the stub got nothing" "$(cat "$W/requests")" ""
        fi
    done
}

echo "== trust from SSL_CERT_FILE and SSL_CERT_DIR =="
row "SSL_CERT_FILE names the CA" ok SSL_CERT_FILE="$W/tls/ca.pem"
row "SSL_CERT_DIR holds the CA" ok SSL_CERT_DIR="$W/certdir"
row "both set, the CA in the file" ok SSL_CERT_FILE="$W/tls/ca.pem" SSL_CERT_DIR="$W/tls"

echo "== the named CAs replace the default trust =="
WHY="CERTIFICATE_VERIFY_FAILED"
row "SSL_CERT_FILE names another CA" refused SSL_CERT_FILE="$W/tls/other.pem"
row "no variable (default trust, a throwaway CA is not in it)" refused
row "SSL_CERT_FILE empty counts as unset" refused SSL_CERT_FILE=
row "SSL_CERT_DIR empty counts as unset" refused SSL_CERT_DIR=

echo "== a variable naming no file fails the request =="
WHY="No such file"
row "SSL_CERT_FILE names no file" refused SSL_CERT_FILE="$W/tls/missing.pem"

echo "py_tls_trust: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
