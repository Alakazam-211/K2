#!/usr/bin/env bash
# `k2 update` / `--app` / `--list` and `k2 daemon install` follow GitHub's
# own redirects, and only those.
#
# Regression: `k2 update` asked GitHub with `curl -fsS` and no -L. After a
# repository move api.github.com answers the old path with a 301 to the
# new one, so the check read the 301's own body and printed
# "Latest:  unknown". `k2 daemon install` used `curl -fsSL`, which follows
# a redirect to any host, plain http too. Now each hop is followed by the
# CLI itself: https only, on GitHub's hosts only (api.github.com,
# github.com, objects.githubusercontent.com,
# release-assets.githubusercontent.com), at most 5 hops. Anything else is
# refused with one line on stderr, exit 1, and the other address is never
# contacted.
#
# How: real curl and a stub GitHub on 127.0.0.1. A small CA made here signs
# a certificate for GitHub's four hosts; a curl wrapper first on PATH runs
# the system curl with --cacert for that CA and --connect-to sending
# GitHub's four hosts (port 443) to the stub and every other host and port
# to a second listener that only logs each connection it gets. The stub
# logs each request's Host, path and whether it carried an Authorization
# header. Each row writes the stub's mode to a file first.
# Hermetic: env -i, a temp HOME and TMPDIR, no daemon, no network. Loud.
# Needs python3, openssl and curl.
# Run with: bash tests/cli/update_github_redirects.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
for t in python3 openssl; do
    command -v "$t" >/dev/null || { echo "FAIL: $t required" >&2; exit 1; }
done
# The system curl, the one a person's k2 runs.
REAL_CURL="$(PATH=/usr/bin:/bin command -v curl)" || { echo "FAIL: no curl in /usr/bin or /bin" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

WORK="$(mktemp -d -t k2-update-redirects-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT
STUBS="$WORK/stubs"
H="$WORK/home"
T="$WORK/tmp"
TLS="$WORK/tls"
mkdir -p "$STUBS" "$H/.k2" "$T" "$TLS"
# A token where k2 keeps one: a GitHub fetch must never send it.
TOKEN="k2-redirect-test-token-5f1c"
printf '%s' "$TOKEN" >"$H/.k2/heartbeat.token"
printf '1' >"$H/.k2/heartbeat.port"

GH_HOSTS="api.github.com github.com objects.githubusercontent.com release-assets.githubusercontent.com"

echo "== a test CA and a certificate for GitHub's hosts =="
cat >"$TLS/ca.cnf" <<'EOF'
[req]
distinguished_name = dn
prompt = no
x509_extensions = v3ca
[dn]
CN = k2 redirect test CA
[v3ca]
basicConstraints = critical,CA:true
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
EOF
cat >"$TLS/leaf.cnf" <<'EOF'
[req]
distinguished_name = dn
prompt = no
[dn]
CN = api.github.com
[leaf]
basicConstraints = CA:false
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:api.github.com,DNS:github.com,DNS:objects.githubusercontent.com,DNS:release-assets.githubusercontent.com
EOF
openssl req -x509 -new -newkey rsa:2048 -nodes -sha256 -days 2 -config "$TLS/ca.cnf" \
    -keyout "$TLS/ca.key" -out "$TLS/ca.pem" 2>"$TLS/err" || { show "$(cat "$TLS/err")"; echo "FAIL: openssl (CA)" >&2; exit 1; }
openssl req -new -newkey rsa:2048 -nodes -sha256 -config "$TLS/leaf.cnf" \
    -keyout "$TLS/leaf.key" -out "$TLS/leaf.csr" 2>"$TLS/err" || { show "$(cat "$TLS/err")"; echo "FAIL: openssl (CSR)" >&2; exit 1; }
openssl x509 -req -sha256 -days 2 -set_serial 2 -in "$TLS/leaf.csr" -CA "$TLS/ca.pem" -CAkey "$TLS/ca.key" \
    -extfile "$TLS/leaf.cnf" -extensions leaf -out "$TLS/leaf.pem" 2>"$TLS/err" || { show "$(cat "$TLS/err")"; echo "FAIL: openssl (certificate)" >&2; exit 1; }
ok "made the CA and the certificate"

# The stub GitHub (TLS) and the listener for everything else (it logs each
# connection and closes it). Each request reads the mode file:
#   moved        the start path answers 301 to /repositories/42/<rest> on
#                the same host, which answers.
#   chain:<n>    <n> redirects through GitHub's four hosts, then the answer.
#   to:<url>     the start path answers 302 to <url>.
# The answer is the newest release, the release list or the daemon
# manifest, by the start path.
cat >"$WORK/stub.py" <<'PY'
import json, os, socket, ssl, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

work = sys.argv[1]
hosts = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com", "api.github.com"]
latest = {"tag_name": "v9.9.9", "published_at": "2026-01-02T00:00:00Z"}
listing = [{"tag_name": "v9.9.9", "published_at": "2026-01-02T00:00:00Z", "prerelease": False}]
manifest = {"version": "0.46.7", "platforms": {"linux-x86_64": {
    "url": "https://github.com/Alakazam-211/K2/releases/download/v0.46.7/k2-daemon-linux-x86_64",
    "sha256": "0" * 64,
    "sig2": "https://github.com/Alakazam-211/K2/releases/download/v0.46.7/k2-daemon-linux-x86_64.sig2"}}}
starts = {
    "/repos/Alakazam-211/K2/releases/latest": "latest",
    "/repos/Alakazam-211/K2/releases?per_page=10": "list",
    "/Alakazam-211/K2/releases/latest/download/daemon-latest.json": "manifest",
}
repo = "/repos/Alakazam-211/K2"
lock = threading.Lock()

def log(name, line):
    with lock:
        with open(os.path.join(work, name), "a") as f:
            f.write(line + "\n")

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, code, body=b"", location=None):
        self.send_response(code)
        if location:
            self.send_header("Location", location)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def answer(self, kind):
        body = {"latest": latest, "list": listing, "manifest": manifest}[kind]
        self.send(200, json.dumps(body).encode())

    def do_GET(self):
        host = self.headers.get("Host", "")
        auth = "auth" if self.headers.get("Authorization") is not None else "noauth"
        log("gh.log", "%s %s %s" % (host, self.path, auth))
        for v in self.headers.values():
            if os.environ["STUB_TOKEN"] in v:
                log("gh.log", "TOKEN-SEEN %s" % host)
        with open(os.path.join(work, "mode")) as f:
            mode = f.read().strip()
        kind = starts.get(self.path)
        if kind is not None:
            if mode == "moved":
                # GitHub's own shape: /repos/<owner>/<repo>/... moves
                # to /repositories/<id>/... on the same host.
                self.send(301, b'{"message":"Moved Permanently"}',
                          "https://%s/repositories/42%s" % (host, self.path[len(repo):]))
            elif mode.startswith("chain:"):
                n = int(mode.split(":", 1)[1])
                self.send(302, b"", "https://%s/chain/1/%d/%s" % (hosts[0], n, kind))
            elif mode.startswith("to:"):
                self.send(302, b"", mode[3:])
            else:
                self.send(500, b"bad mode")
            return
        if self.path.startswith("/repositories/42/"):
            self.answer(starts[repo + self.path[len("/repositories/42"):]])
            return
        if self.path.startswith("/chain/"):
            _, _, i, n, kind = self.path.split("/")
            i, n = int(i), int(n)
            if i >= n:
                self.answer(kind)
            else:
                self.send(302, b"", "https://%s/chain/%d/%d/%s" % (hosts[i % len(hosts)], i + 1, n, kind))
            return
        self.send(404, b"no such path")

gh = ThreadingHTTPServer(("127.0.0.1", 0), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(os.path.join(work, "tls", "leaf.pem"), os.path.join(work, "tls", "leaf.key"))
gh.socket = ctx.wrap_socket(gh.socket, server_side=True)

off = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
off.bind(("127.0.0.1", 0))
off.listen(16)

def off_loop():
    while True:
        c, _ = off.accept()
        log("off.log", "connection")
        c.close()

threading.Thread(target=off_loop, daemon=True).start()
with open(os.path.join(work, "ports.tmp"), "w") as f:
    f.write("%d %d\n" % (gh.server_address[1], off.getsockname()[1]))
os.rename(os.path.join(work, "ports.tmp"), os.path.join(work, "ports"))
gh.serve_forever()
PY
: >"$WORK/mode"
STUB_TOKEN="$TOKEN" python3 "$WORK/stub.py" "$WORK" 2>"$WORK/stub.err" &
STUB_PID=$!
i=0
while [ ! -s "$WORK/ports" ]; do
    i=$((i + 1))
    if [ "$i" -gt 100 ] || ! kill -0 "$STUB_PID" 2>/dev/null; then
        show "$(cat "$WORK/stub.err")"
        echo "FAIL: the stub GitHub did not start" >&2
        exit 1
    fi
    sleep 0.1
done
read -r GH_PORT OFF_PORT <"$WORK/ports"
ok "the stub GitHub listens on $GH_PORT, the other listener on $OFF_PORT"

# The curl wrapper: GitHub's hosts to the stub, everything else to the
# listener that only logs. curl takes the first --connect-to that matches.
{
    echo '#!/bin/sh'
    printf 'printf "curl %%s\\n" "$*" >>"%s"\n' "$WORK/curl.log"
    printf 'exec "%s" --noproxy "*" --cacert "%s"' "$REAL_CURL" "$TLS/ca.pem"
    for h in $GH_HOSTS; do printf ' --connect-to %s:443:127.0.0.1:%s' "$h" "$GH_PORT"; done
    printf ' --connect-to ::127.0.0.1:%s "$@"\n' "$OFF_PORT"
} >"$STUBS/curl"
# daemon install reads the platform from uname: Linux x86_64 everywhere.
REAL_UNAME="$(command -v uname)"
cat >"$STUBS/uname" <<EOF
#!/bin/sh
case "\${1:-}" in
    -s) echo Linux ;;
    -m) echo x86_64 ;;
    *) exec "$REAL_UNAME" "\$@" ;;
esac
EOF
chmod +x "$STUBS/curl" "$STUBS/uname"

# run <mode> <k2 args...>: sets OUT, ERR, RC; clears the logs.
run() {
    local mode="$1"; shift
    printf '%s\n' "$mode" >"$WORK/mode"
    : >"$WORK/gh.log"; : >"$WORK/off.log"; : >"$WORK/curl.log"
    set +e
    env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" TMPDIR="$T" K2_TOKEN="$TOKEN" \
        "$K2" "$@" </dev/null >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
}

# gh_count <label> <n>: the stub GitHub got exactly <n> requests.
gh_count() {
    local n
    n="$(grep -c . "$WORK/gh.log" || true)"
    [ "$n" = "$2" ] && ok "$1: $2 request(s) to GitHub" || { bad "$1: $n requests to GitHub (want $2)"; show "$(cat "$WORK/gh.log")"; }
}
# nothing_else <label>: no connection off GitHub, no token, no temp file.
nothing_else() {
    [ ! -s "$WORK/off.log" ] && ok "$1: no connection to any other host" \
        || { bad "$1: another host was contacted"; show "$(cat "$WORK/off.log")"; }
    if grep -q -e ' auth$' -e '^TOKEN-SEEN' "$WORK/gh.log"; then
        bad "$1: a request carried a credential"; show "$(cat "$WORK/gh.log")"
    else
        ok "$1: no Authorization header, no token"
    fi
    if grep -q -e ' --location ' -e ' -[a-zA-Z]*L[a-zA-Z]* ' "$WORK/curl.log"; then
        bad "$1: curl was run with -L"; show "$(cat "$WORK/curl.log")"
    else
        ok "$1: curl never ran with -L"
    fi
    [ -z "$(ls -A "$T")" ] && ok "$1: no temp file left" || bad "$1: left $(ls -A "$T")"
}
same() {
    [ "$2" = "$3" ] && ok "$1" || { bad "$1"; show "want: $3"; show "got:  $2"; }
}

LIST_OUT="$(printf 'Available K2 releases:\n  v9.9.9  2026-01-02')"

echo "== a moved repository: the 301 to the new path is followed =="
for form in "" "--app"; do
    label="moved: k2 update${form:+ $form}"
    # shellcheck disable=SC2086
    run moved update $form
    same "$label: exit 0" "$RC" 0
    same "$label: stderr empty" "$ERR" ""
    case "$OUT" in *"  Latest:  v9.9.9"*) ok "$label: the latest tag" ;; *) bad "$label: no latest tag"; show "$OUT" ;; esac
    gh_count "$label" 2
    same "$label: the old path, then the new one" "$(cat "$WORK/gh.log")" \
        "$(printf 'api.github.com /repos/Alakazam-211/K2/releases/latest noauth\napi.github.com /repositories/42/releases/latest noauth')"
    nothing_else "$label"
done
run moved update --list
same "moved: k2 update --list: exit 0" "$RC" 0
same "moved: k2 update --list: stderr empty" "$ERR" ""
same "moved: k2 update --list: the release" "$OUT" "$LIST_OUT"
same "moved: k2 update --list: the old path, then the new one" "$(cat "$WORK/gh.log")" \
    "$(printf 'api.github.com /repos/Alakazam-211/K2/releases?per_page=10 noauth\napi.github.com /repositories/42/releases?per_page=10 noauth')"
nothing_else "moved: k2 update --list"

echo "== five redirects through GitHub's four hosts: followed =="
run chain:5 update --app
same "chain of 5: exit 0" "$RC" 0
same "chain of 5: stderr empty" "$ERR" ""
case "$OUT" in *"  Latest:  v9.9.9"*) ok "chain of 5: the latest tag" ;; *) bad "chain of 5: no latest tag"; show "$OUT" ;; esac
gh_count "chain of 5" 6
for h in $GH_HOSTS; do
    grep -q "^$h " "$WORK/gh.log" && ok "chain of 5: reached $h" || { bad "chain of 5: never reached $h"; show "$(cat "$WORK/gh.log")"; }
done
nothing_else "chain of 5"

echo "== more than five redirects: refused =="
for form in "--app" "--list"; do
    label="chain of 6: k2 update $form"
    run chain:6 update "$form"
    same "$label: exit 1" "$RC" 1
    same "$label: the one stderr line" "$ERR" \
        "Error: GitHub redirected more than 5 times (the last to https://objects.githubusercontent.com); stopped there"
    if [ "$form" = "--list" ]; then same "$label: nothing on stdout" "$OUT" ""; else same "$label: stdout stops at the check line" "$OUT" "Checking for app updates..."; fi
    gh_count "$label" 6
    nothing_else "$label"
done

echo "== a redirect off GitHub: refused, never contacted =="
# to:<url> | the host the error line names
while IFS='|' read -r to where; do
    label="to $to"
    run "to:$to" update --app
    same "$label: exit 1" "$RC" 1
    same "$label: the one stderr line" "$ERR" "Error: GitHub redirected to $where, which is not GitHub; nothing was sent there"
    same "$label: stdout stops at the check line" "$OUT" "Checking for app updates..."
    gh_count "$label" 1
    nothing_else "$label"
done <<'EOF'
https://evil.example/repos/Alakazam-211/K2/releases/latest|https://evil.example
https://github.com.evil.example/x|https://github.com.evil.example
https://api.github.com:8443/repos/Alakazam-211/K2/releases/latest|https://api.github.com:8443
https://api.github.com@evil.example/x|https://api.github.com@evil.example
EOF
run "to:https://evil.example/x" update --list
same "to evil.example: k2 update --list: exit 1" "$RC" 1
same "to evil.example: k2 update --list: the one stderr line" "$ERR" \
    "Error: GitHub redirected to https://evil.example, which is not GitHub; nothing was sent there"
same "to evil.example: k2 update --list: nothing on stdout" "$OUT" ""
nothing_else "to evil.example: k2 update --list"

echo "== a redirect to plain http: refused, never contacted =="
for to in "http://api.github.com/repos/Alakazam-211/K2/releases/latest" "http://evil.example/x"; do
    label="to $to"
    where="${to%/*}"; where="${where#http://}"; where="http://${where%%/*}"
    run "to:$to" update --app
    same "$label: exit 1" "$RC" 1
    same "$label: the one stderr line" "$ERR" "Error: GitHub redirected to $where, which is not https; nothing was sent there"
    gh_count "$label" 1
    nothing_else "$label"
done

echo "== k2 daemon install: the manifest follows GitHub's redirects only =="
run chain:3 daemon install --dry-run --no-service
same "daemon install, 3 redirects: exit 0" "$RC" 0
case "$OUT" in *"  manifest ver:   0.46.7"*) ok "daemon install, 3 redirects: the manifest's version" ;; *) bad "daemon install, 3 redirects: no manifest version"; show "$OUT$ERR" ;; esac
gh_count "daemon install, 3 redirects" 4
nothing_else "daemon install, 3 redirects"
run "to:https://evil.example/daemon-latest.json" daemon install --dry-run --no-service
same "daemon install, off GitHub: exit 1" "$RC" 1
same "daemon install, off GitHub: the lines" "$OUT$ERR" \
    "$(printf 'Error: GitHub redirected to https://evil.example, which is not GitHub; nothing was sent there\nFailed to fetch daemon manifest: https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json')"
gh_count "daemon install, off GitHub" 1
nothing_else "daemon install, off GitHub"
run "to:http://github.com/x" daemon install --dry-run --no-service
same "daemon install, to http: exit 1" "$RC" 1
case "$ERR" in "Error: GitHub redirected to http://github.com, which is not https; nothing was sent there"*) ok "daemon install, to http: says so" ;; *) bad "daemon install, to http: stderr"; show "$ERR" ;; esac
nothing_else "daemon install, to http"

echo ""
echo "update_github_redirects: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
