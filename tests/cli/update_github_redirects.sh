#!/usr/bin/env bash
# `k2 update` / `--app` / `--list`, `k2 daemon install`, the public
# installer (scripts/install-daemon.sh) and the provisioning script's fetch
# of it (scripts/provision-k2-server.sh, which also allows
# raw.githubusercontent.com) follow GitHub's own redirects, and only those.
#
# Regression: `k2 update` asked GitHub with `curl -fsS` and no -L. After a
# repository move api.github.com answers the old path with a 301 to the
# new one, so the check read the 301's own body and printed
# "Latest:  unknown". `k2 daemon install` used `curl -fsSL`, which follows
# a redirect to any host, plain http too, and so did install-daemon.sh.
# Now each hop is followed by the CLI (and the installer) itself: https only, on GitHub's hosts only (api.github.com,
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
# Needs python3, openssl and curl. The installer runs as a dry run with
# K2_NO_SERVICE=1 and K2_NO_ROOT_HELPER=1, under sh, with stand-ins for
# systemctl, launchctl, sudo, runuser and minisign that log any call (none
# may come). K2_TEST_INSTALLER names another copy of the installer (the
# bite). The provisioning script never runs: only its GitHub fetch block and
# the line that fetches install-daemon.sh, cut out and run alone under bash
# with the same stand-ins (apt-get, useradd, chown and visudo too);
# K2_TEST_PROVISION names another copy (the bite).
# Run with: bash tests/cli/update_github_redirects.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
for t in python3 openssl; do
    command -v "$t" >/dev/null || { echo "FAIL: $t required" >&2; exit 1; }
done
# The system curl, the one a person's k2 runs.
REAL_CURL="$(PATH=/usr/bin:/bin command -v curl)" || { echo "FAIL: no curl in /usr/bin or /bin" >&2; exit 1; }
INSTALLER="${K2_TEST_INSTALLER:-$ROOT/scripts/install-daemon.sh}"
PROVISION="${K2_TEST_PROVISION:-$ROOT/scripts/provision-k2-server.sh}"

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
# Allowed only for the provisioning script's fetch.
RAW_HOST="raw.githubusercontent.com"

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
subjectAltName = DNS:api.github.com,DNS:github.com,DNS:objects.githubusercontent.com,DNS:release-assets.githubusercontent.com,DNS:raw.githubusercontent.com
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
    "/Alakazam-211/K2/main/scripts/install-daemon.sh": "raw",
    # The provisioning script's other GitHub fetches.
    "/fatedier/frp/releases/download/v0.61.1/frp_0.61.1_linux_amd64.tar.gz": "file:frp",
    "/Alakazam-211/K2/v0.46.7/cli/k2": "file:cli",
    "/Alakazam-211/K2/releases/download/v0.45.3/install-mail-helper.sh": "file:legacy",
    "/Alakazam-211/K2/main/scripts/k2-pg-helper": "file:pg",
}
# A moved repository: the owner part of each start path becomes Moved-Org.
def moved_path(p):
    return "/Moved-Org/" + p.split("/", 2)[2]
moved = {moved_path(p): k for p, k in starts.items()}
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
        if kind.startswith("file:"):
            self.send(200, ("stand-in %s\n" % kind[5:]).encode())
            return
        if kind == "raw":
            self.send(200, b"#!/bin/sh\necho stand-in installer\n")
            return
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
            if mode == "moved" and (kind in ("raw", "manifest") or kind.startswith("file:")):
                # A moved repository's release download or raw file, on the
                # same host.
                self.send(301, b"", "https://%s%s" % (host, moved_path(self.path)))
            elif mode == "moved":
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
        if self.path in moved:
            self.answer(moved[self.path])
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
    for h in $GH_HOSTS $RAW_HOST; do printf ' --connect-to %s:443:127.0.0.1:%s' "$h" "$GH_PORT"; done
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
for t in systemctl launchctl sudo runuser minisign apt-get useradd chown visudo; do
    printf '#!/bin/sh\necho "STANDIN %s $*" >>"%s"\nexit 97\n' "$t" "$WORK/standin.log" >"$STUBS/$t"
    chmod +x "$STUBS/$t"
done

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

echo "== scripts/install-daemon.sh: the manifest follows GitHub's redirects only =="
# inst <mode>: the installer's dry run; sets OUT, ERR, RC; clears the logs.
inst() {
    printf '%s\n' "$1" >"$WORK/mode"
    : >"$WORK/gh.log"; : >"$WORK/off.log"; : >"$WORK/curl.log"; : >"$WORK/standin.log"
    set +e
    env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" TMPDIR="$T" K2_TOKEN="$TOKEN" \
        K2_DRY_RUN=1 K2_NO_SERVICE=1 K2_NO_ROOT_HELPER=1 K2_BIN_DIR="$WORK/bin" \
        sh "$INSTALLER" </dev/null >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
    [ ! -s "$WORK/standin.log" ] && ok "installer $1: no systemctl, launchctl, sudo, runuser or minisign" \
        || { bad "installer $1: a stand-in ran"; show "$(cat "$WORK/standin.log")"; }
    [ ! -e "$WORK/bin" ] && ok "installer $1: nothing installed" || bad "installer $1: wrote $WORK/bin"
}
INST_URL="https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json"
INST_FAILED="Failed to fetch: $INST_URL"
inst moved
same "installer, a moved repository: exit 0" "$RC" 0
case "$OUT" in *"  manifest ver:   0.46.7"*) ok "installer, a moved repository: the manifest's version" ;; *) bad "installer, a moved repository: no manifest version"; show "$OUT$ERR" ;; esac
same "installer, a moved repository: the old path, then the new one" "$(cat "$WORK/gh.log")" \
    "$(printf 'github.com /Alakazam-211/K2/releases/latest/download/daemon-latest.json noauth\ngithub.com /Moved-Org/K2/releases/latest/download/daemon-latest.json noauth')"
nothing_else "installer, a moved repository"
inst chain:5
same "installer, 5 redirects: exit 0" "$RC" 0
case "$OUT" in *"  manifest ver:   0.46.7"*) ok "installer, 5 redirects: the manifest's version" ;; *) bad "installer, 5 redirects: no manifest version"; show "$OUT$ERR" ;; esac
gh_count "installer, 5 redirects" 6
nothing_else "installer, 5 redirects"
inst chain:6
same "installer, 6 redirects: exit 1" "$RC" 1
same "installer, 6 redirects: the lines" "$ERR" \
    "$(printf 'Error: GitHub redirected more than 5 times (the last to https://objects.githubusercontent.com); stopped there\n%s' "$INST_FAILED")"
gh_count "installer, 6 redirects" 6
nothing_else "installer, 6 redirects"
while IFS='|' read -r to want; do
    inst "to:$to"
    same "installer, to $to: exit 1" "$RC" 1
    same "installer, to $to: the lines" "$ERR" "$(printf '%s\n%s' "$want" "$INST_FAILED")"
    case "$OUT" in *"DRY RUN"*) bad "installer, to $to: went on to the dry run"; show "$OUT" ;; *) ok "installer, to $to: stopped before the dry run" ;; esac
    gh_count "installer, to $to" 1
    nothing_else "installer, to $to"
done <<'EOF'
https://evil.example/daemon-latest.json|Error: GitHub redirected to https://evil.example, which is not GitHub; nothing was sent there
https://github.com.evil.example/x|Error: GitHub redirected to https://github.com.evil.example, which is not GitHub; nothing was sent there
https://github.com@evil.example/x|Error: GitHub redirected to https://github.com@evil.example, which is not GitHub; nothing was sent there
http://github.com/x|Error: GitHub redirected to http://github.com, which is not https; nothing was sent there
http://evil.example/x|Error: GitHub redirected to http://evil.example, which is not https; nothing was sent there
EOF

echo "== scripts/provision-k2-server.sh: its fetch of install-daemon.sh follows GitHub's redirects only =="
RAW_URL="https://raw.githubusercontent.com/Alakazam-211/K2/main/scripts/install-daemon.sh"
grep -q 'curl -fsSL "$RAW_BASE/scripts/install-daemon.sh"' "$PROVISION" \
    && bad "provision: still fetches install-daemon.sh with curl -fsSL" || ok "provision: no curl -fsSL for install-daemon.sh"
FETCH_BLOCK="$(sed -n '/^# ── GitHub fetch: begin/,/^# ── GitHub fetch: end/p' "$PROVISION")"
CALL="$(sed -n '/^[[:space:]]*provision_github_fetch "\$RAW_BASE\/scripts\/install-daemon.sh" "\$INSTALLER"/,/|| die /p' "$PROVISION")"
DIE="$(grep -m1 '^die() {' "$PROVISION")"
RAW_BASE_LINE="$(grep -m1 '^RAW_BASE=' "$PROVISION")"
[ -n "$FETCH_BLOCK" ] && ok "provision: the GitHub fetch block" || bad "provision: no GitHub fetch block"
[ "$(printf '%s\n' "$CALL" | grep -c .)" = 2 ] && ok "provision: the line that fetches install-daemon.sh uses it" \
    || { bad "provision: the install-daemon.sh fetch line"; show "$CALL"; }
{
    echo 'set -euo pipefail'
    printf '%s\n' "$DIE" "$RAW_BASE_LINE" "$FETCH_BLOCK"
    echo 'INSTALLER="$1"'
    printf '%s\n' "$CALL"
} >"$WORK/prov-fetch.sh"
# prov <mode>: the cut-out fetch; sets OUT, ERR, RC; clears the logs.
prov() {
    printf '%s\n' "$1" >"$WORK/mode"
    : >"$WORK/gh.log"; : >"$WORK/off.log"; : >"$WORK/curl.log"; : >"$WORK/standin.log"
    rm -rf "$WORK/prov-out"; mkdir -p "$WORK/prov-out"
    set +e
    env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" TMPDIR="$T" K2_TOKEN="$TOKEN" \
        bash "$WORK/prov-fetch.sh" "$WORK/prov-out/install-daemon.sh" </dev/null >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
    [ ! -s "$WORK/standin.log" ] && ok "provision $1: no system tool ran" \
        || { bad "provision $1: a stand-in ran"; show "$(cat "$WORK/standin.log")"; }
}
PROV_FAILED="$(printf '\033[1;31m[provision] ERROR:\033[0m could not fetch https://raw.githubusercontent.com/Alakazam-211/K2/main/scripts/install-daemon.sh')"
prov moved
same "provision, a renamed repository: exit 0" "$RC" 0
same "provision, a renamed repository: stderr empty" "$ERR" ""
same "provision, a renamed repository: the file" "$(cat "$WORK/prov-out/install-daemon.sh" 2>&1)" "$(printf '#!/bin/sh\necho stand-in installer')"
same "provision, a renamed repository: the old path, then the new one" "$(cat "$WORK/gh.log")" \
    "$(printf 'raw.githubusercontent.com /Alakazam-211/K2/main/scripts/install-daemon.sh noauth\nraw.githubusercontent.com /Moved-Org/K2/main/scripts/install-daemon.sh noauth')"
nothing_else "provision, a renamed repository"
prov chain:5
same "provision, 5 redirects: exit 0" "$RC" 0
gh_count "provision, 5 redirects" 6
for h in $GH_HOSTS $RAW_HOST; do
    grep -q "^$h " "$WORK/gh.log" && ok "provision, 5 redirects: reached $h" || { bad "provision, 5 redirects: never reached $h"; show "$(cat "$WORK/gh.log")"; }
done
nothing_else "provision, 5 redirects"
prov chain:6
same "provision, 6 redirects: exit 1" "$RC" 1
same "provision, 6 redirects: the lines" "$ERR" \
    "$(printf 'Error: GitHub redirected more than 5 times (the last to https://objects.githubusercontent.com); stopped there\n%s' "$PROV_FAILED")"
gh_count "provision, 6 redirects" 6
nothing_else "provision, 6 redirects"
while IFS='|' read -r to want; do
    prov "to:$to"
    same "provision, to $to: exit 1" "$RC" 1
    same "provision, to $to: the lines" "$ERR" "$(printf '%s\n%s' "$want" "$PROV_FAILED")"
    gh_count "provision, to $to" 1
    nothing_else "provision, to $to"
done <<'EOF'
https://evil.example/install-daemon.sh|Error: GitHub redirected to https://evil.example, which is not GitHub; nothing was sent there
https://raw.githubusercontent.com.evil.example/x|Error: GitHub redirected to https://raw.githubusercontent.com.evil.example, which is not GitHub; nothing was sent there
https://raw.githubusercontent.com@evil.example/x|Error: GitHub redirected to https://raw.githubusercontent.com@evil.example, which is not GitHub; nothing was sent there
https://raw.githubusercontent.com:8443/x|Error: GitHub redirected to https://raw.githubusercontent.com:8443, which is not GitHub; nothing was sent there
http://raw.githubusercontent.com/x|Error: GitHub redirected to http://raw.githubusercontent.com, which is not https; nothing was sent there
http://evil.example/x|Error: GitHub redirected to http://evil.example, which is not https; nothing was sent there
EOF
echo "== scripts/provision-k2-server.sh: no GitHub fetch can follow a redirect on curl's own =="
PROV_CODE="$(grep -v '^[[:space:]]*#' "$PROVISION")"
if printf '%s\n' "$PROV_CODE" | grep -nE 'curl([[:space:]][^|;]*)?[[:space:]](-[a-zA-Z]*L[a-zA-Z]*|--location(-trusted)?)([[:space:]]|$)' >"$WORK/curl-L.txt"; then
    bad "provision: a curl that follows redirects itself"; show "$(cat "$WORK/curl-L.txt")"
else
    ok "provision: no curl -L or --location anywhere"
fi
if printf '%s\n' "$PROV_CODE" | grep -nE '(^|[^_])curl[[:space:]][^#]*(github|githubusercontent)' >"$WORK/curl-gh.txt"; then
    bad "provision: a GitHub URL fetched with plain curl"; show "$(cat "$WORK/curl-gh.txt")"
else
    ok "provision: every GitHub URL goes through provision_github_fetch"
fi
[ "$(grep -c 'provision_github_fetch "' "$PROVISION")" = 6 ] && ok "provision: six GitHub fetches through provision_github_fetch" \
    || { bad "provision: $(grep -c 'provision_github_fetch "' "$PROVISION") calls of provision_github_fetch (want 6)"; show "$(grep -n 'provision_github_fetch "' "$PROVISION")"; }

echo "== scripts/provision-k2-server.sh: each GitHub fetch, cut out and run alone =="
# stmt <fixed text>: the statement that starts on the line holding <fixed
# text>, through its continuation lines (each ending in a backslash).
stmt() {
    awk -v want="$1" 'go == 0 && index($0, want) { go = 1 }
        go == 1 { print; if ($0 !~ /\\$/) exit }' "$PROVISION"
}
RH_DIE="$(sed -n '/^root_helper_die() {/,/^}/p' "$PROVISION")"
SITE_FRP="$(stmt 'provision_github_fetch "https://github.com/fatedier/frp/')"
SITE_MANIFEST="$(sed -n '/^[[:space:]]*_MANIFEST_TMP="\$(mktemp)"/,/^[[:space:]]*rm -f "\$_MANIFEST_TMP"$/p' "$PROVISION")"
# (The locator stops before the path so the CLI gate doesn't read it as building cli/k2.)
SITE_CLI="$(stmt 'provision_github_fetch "https://raw.githubusercontent.com/Alakazam-211/K2/v')"
SITE_LEGACY="$(stmt 'provision_github_fetch "$LEGACY_URL"')"
SITE_PG="$(stmt 'provision_github_fetch "$RAW_BASE/scripts/k2-pg-helper"')"
LEGACY_URL_LINE="$(grep -m1 '^[[:space:]]*LEGACY_URL=' "$PROVISION")"
for v in RH_DIE SITE_FRP SITE_MANIFEST SITE_CLI SITE_LEGACY SITE_PG LEGACY_URL_LINE; do
    eval "x=\$$v"
    [ -n "$x" ] && ok "provision: found $v" || bad "provision: $v not found"
done
# The CLI's line writes /usr/local/bin/k2: here it writes the scratch copy.
SITE_CLI="$(printf '%s\n' "$SITE_CLI" | sed "s#/usr/local/bin/k2#\"\$O/k2\"#")"
# site_run <name> <mode> <the site's lines> <setup>: sets OUT, ERR, RC.
site_run() {
    printf '%s\n' "$2" >"$WORK/mode"
    : >"$WORK/gh.log"; : >"$WORK/off.log"; : >"$WORK/curl.log"; : >"$WORK/standin.log"
    rm -rf "$WORK/site-out"; mkdir -p "$WORK/site-out"
    {
        echo 'set -euo pipefail'
        printf '%s\n' "$DIE" "$RH_DIE" "$RAW_BASE_LINE" "$FETCH_BLOCK"
        echo 'O="$1"'
        printf '%s\n' "$4" "$3"
    } >"$WORK/site.sh"
    set +e
    env -i PATH="$STUBS:/usr/bin:/bin" HOME="$H" TMPDIR="$T" K2_TOKEN="$TOKEN" \
        bash "$WORK/site.sh" "$WORK/site-out" </dev/null >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
    [ ! -s "$WORK/standin.log" ] && ok "provision $1 $2: no system tool ran" \
        || { bad "provision $1 $2: a stand-in ran"; show "$(cat "$WORK/standin.log")"; }
}
PROV_RED='\033[1;31m[provision] ERROR:\033[0m'
OFF_LINE="Error: GitHub redirected to https://evil.example, which is not GitHub; nothing was sent there"
# name|the site's variable|its setup|the file it writes|what lands there|its stop line after OFF_LINE (empty: none)
while IFS='|' read -r name var setup file want stop; do
    eval "site=\$$var"
    # The legacy line's URL comes from the script; the manifest's version is
    # printed so the row can read it.
    [ "$name" = legacy ] && setup="$setup
$LEGACY_URL_LINE"
    [ "$name" = manifest ] && site="$site
echo \"K2_VERSION=\$K2_VERSION\""
    site_run "$name" moved "$site" "$setup"
    same "provision $name, a moved repository: exit 0" "$RC" 0
    same "provision $name, a moved repository: stderr empty" "$ERR" ""
    if [ "$name" = manifest ]; then
        same "provision manifest, a moved repository: the version read" "$OUT" "K2_VERSION=0.46.7"
    else
        same "provision $name, a moved repository: the file" "$(cat "$WORK/site-out/$file" 2>&1)" "$want"
    fi
    gh_count "provision $name, a moved repository" 2
    grep -q ' /Moved-Org/' "$WORK/gh.log" && ok "provision $name: asked the new path" || { bad "provision $name: never asked the new path"; show "$(cat "$WORK/gh.log")"; }
    nothing_else "provision $name, a moved repository"
    site_run "$name" "to:https://evil.example/x" "$site" "$setup"
    [ "$RC" -ne 0 ] && ok "provision $name, off GitHub: exit $RC" || bad "provision $name, off GitHub: exit 0"
    if [ -n "$stop" ]; then
        same "provision $name, off GitHub: the lines" "$ERR" "$(printf "%s\n$PROV_RED %s" "$OFF_LINE" "$stop")"
    else
        same "provision $name, off GitHub: the line" "$ERR" "$OFF_LINE"
    fi
    gh_count "provision $name, off GitHub" 1
    nothing_else "provision $name, off GitHub"
done <<'EOF'
frp|SITE_FRP|FRP_VERSION=0.61.1; FRP_DIR=frp_0.61.1_linux_amd64; TMP="$O"|frp.tar.gz|stand-in frp|
manifest|SITE_MANIFEST|K2_VERSION=""|-|-|could not resolve the latest daemon release; set K2_VERSION=<x.y.z>
cli|SITE_CLI|K2_VERSION=0.46.7|k2|stand-in cli|could not fetch the k2 CLI for v0.46.7
legacy|SITE_LEGACY|DAEMON_VERSION=0.45.3; LEGACY_TMP="$O"|install-mail-helper.sh|stand-in legacy|root helper: could not download https://github.com/Alakazam-211/K2/releases/download/v0.45.3/install-mail-helper.sh — provisioning stopped. Fix it and re-run this script (idempotent), or run install-root-helper.sh --version <daemon version> by hand.
pg|SITE_PG|PG_HELPER_SRC="$O/k2-pg-helper"|k2-pg-helper|stand-in pg|could not download k2-pg-helper for the legacy daemon
EOF

echo "== raw.githubusercontent.com is the provisioning script's only: k2 update refuses it =="
run "to:https://raw.githubusercontent.com/x" update --app
same "k2 update to raw: exit 1" "$RC" 1
same "k2 update to raw: the one stderr line" "$ERR" \
    "Error: GitHub redirected to https://raw.githubusercontent.com, which is not GitHub; nothing was sent there"
gh_count "k2 update to raw" 1
nothing_else "k2 update to raw"

echo ""
echo "update_github_redirects: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
