#!/usr/bin/env bash
# 0.46.13 — `k2 update --cli` never downloads or replaces the CLI.
#
# It used to fetch cli/k2 from GitHub's raw main branch and write it over
# the installed CLI: unsigned code from a moving branch, which could also
# leave the CLI out of step with the daemon. The CLI now updates only with
# K2 itself (app bundle, Linux packages, the daemon's CLI staging and the
# root helper's cli-refresh), so `k2 update --cli [--check]` only reports
# versions and how K2 updates.
#
# How: a stub daemon on a loopback port answers /boot-status (the only call
# allowed) and logs each request. A `curl` shim first on PATH passes that
# one URL to the real curl (the bash CLI asks with curl, the k5 binary
# asks itself) and fails loudly on anything else; a `sudo` shim fails
# loudly if called at all. The CLI file must be byte-for-byte unchanged
# afterwards.
#
# No real daemon, no real ~/.k2 (HOME is a temp dir), no network.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_contains() {
    case "$2" in
        *"$3"*) ok "$1" ;;
        *) bad "$1 (missing $(printf %q "$3"))"; printf '%s\n' "$2" | sed 's/^/      | /' >&2 ;;
    esac
}
assert_not_contains() {
    case "$2" in
        *"$3"*) bad "$1 (found $(printf %q "$3"))"; printf '%s\n' "$2" | sed 's/^/      | /' >&2 ;;
        *) ok "$1" ;;
    esac
}

SHIM="$K2_TEST_HOME/shim"
mkdir -p "$SHIM"
FORBIDDEN_LOG="$K2_TEST_HOME/forbidden.log"
HITS_LOG="$K2_TEST_HOME/boot-status.log"
VERSION_FILE="$K2_TEST_HOME/daemon-version"
: >"$FORBIDDEN_LOG"
: >"$HITS_LOG"
REAL_CURL="$(command -v curl)"
[ -n "$REAL_CURL" ] || { echo "FAIL: no curl on PATH" >&2; exit 1; }

# The stub daemon: /boot-status answers with the version in $VERSION_FILE
# and logs the path; anything else is logged as forbidden and gets a 404.
python3 -I -c '
import os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
vfile, hits, forbidden, portfile = sys.argv[1:5]
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_GET(self):
        path = self.path
        if path == "/boot-status":
            with open(hits, "a") as f:
                f.write(path + "\n")
            v = open(vfile).read()
            body = ("{\"phase\":\"ready\",\"version\":\"%s\"}" % v).encode()
            self.send_response(200)
        else:
            with open(forbidden, "a") as f:
                f.write("daemon GET " + path + "\n")
            body = b"{}"
            self.send_response(404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True
    def log_message(self, *a):
        pass
s = HTTPServer(("127.0.0.1", 0), H)
open(portfile + ".tmp", "w").write(str(s.server_port))
os.rename(portfile + ".tmp", portfile)
s.serve_forever()
' "$VERSION_FILE" "$HITS_LOG" "$FORBIDDEN_LOG" "$K2_TEST_HOME/stub.port" &
STUB_PID=$!
cleanup() { kill "$STUB_PID" 2>/dev/null || true; hermetic_cli_cleanup; }
trap cleanup EXIT
for _ in $(seq 1 100); do [ -s "$K2_TEST_HOME/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$K2_TEST_HOME/stub.port")"
[ -n "$STUB_PORT" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
# A loopback port nothing listens on, for a daemon that isn't running.
DEAD_PORT="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"

# curl shim: only the stub's /boot-status goes on, to the real curl.
cat >"$SHIM/curl" <<EOF
#!/bin/sh
url=""
for a in "\$@"; do
    case "\$a" in http://*|https://*) url="\$a" ;; esac
done
case "\$url" in
    "http://127.0.0.1:$STUB_PORT/boot-status"|"http://127.0.0.1:$DEAD_PORT/boot-status")
        exec '$REAL_CURL' "\$@" ;;
esac
printf 'curl %s\n' "\$*" >> '$FORBIDDEN_LOG'
echo "update test: k2 update made a network call it must not make: \$*" >&2
exit 97
EOF
cat >"$SHIM/sudo" <<EOF
#!/bin/sh
printf 'sudo %s\n' "\$*" >> '$FORBIDDEN_LOG'
echo "update test: k2 update ran sudo: \$*" >&2
exit 97
EOF
chmod +x "$SHIM/curl" "$SHIM/sudo"
export PATH="$SHIM:$PATH"
[ "$(command -v curl)" = "$SHIM/curl" ] || { echo "FAIL: curl shim not first on PATH" >&2; exit 1; }

CLI_VERSION="$("$K2_CLI" version)"
CLI_VERSION="${CLI_VERSION#k2 }"
[ -n "$CLI_VERSION" ] || { echo "FAIL: k2 version printed nothing" >&2; exit 1; }
# sha256 of a file: shasum where it exists (macOS), else sha256sum (some
# Linux images ship only that).
sum256() {
    if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1
}
CLI_SUM_BEFORE="$(sum256 "$K2_CLI")"

# run <daemon version|down> <k2 args...>: sets OUT and RC. "down" points
# the CLI at a port nothing listens on.
run() {
    local dv="$1" port="$STUB_PORT"; shift
    if [ "$dv" = "down" ]; then
        port="$DEAD_PORT"
    else
        printf '%s' "$dv" >"$VERSION_FILE"
    fi
    RC=0
    OUT="$(K2_PORT="$port" "$K2_CLI" "$@" 2>&1)" || RC=$?
}

echo "== k2 update --cli (daemon on the same version) =="
run "$CLI_VERSION" update --cli
[ "$RC" -eq 0 ] && ok "exit 0" || bad "exit $RC (want 0)"
assert_contains "installed version" "$OUT" "k2 CLI $CLI_VERSION"
assert_contains "daemon matches" "$OUT" "K2 daemon: $CLI_VERSION (same as the CLI)"
assert_contains "updates with K2" "$OUT" "The CLI updates together with K2; k2 never downloads itself."
assert_contains "desktop route" "$OUT" "Desktop: k2 update --app, or the app's own updater."
assert_contains "linux route" "$OUT" "Linux:   your K2 package, or the install script."
assert_contains "kept matched" "$OUT" "The K2 daemon and root helper keep this CLI matched to K2."
assert_contains "says nothing changed" "$OUT" "Nothing was changed."
assert_not_contains "no false 'Updated'" "$OUT" "Updated k2"
assert_not_contains "no GitHub check" "$OUT" "GitHub"

echo "== k2 update --cli --check (daemon on another version) =="
run "v0.0.1" update --cli --check
[ "$RC" -eq 0 ] && ok "exit 0" || bad "exit $RC (want 0)"
assert_contains "installed version" "$OUT" "k2 CLI $CLI_VERSION"
assert_contains "daemon differs" "$OUT" "K2 daemon: 0.0.1 (differs from the CLI's $CLI_VERSION)"
assert_contains "updates with K2" "$OUT" "The CLI updates together with K2; k2 never downloads itself."
assert_not_contains "--check reaches the CLI form (no closing line)" "$OUT" "Nothing was changed."
assert_not_contains "no 'Update available'" "$OUT" "Update available"

echo "== k2 update --cli -c (daemon not running) =="
run down update --cli -c
[ "$RC" -eq 0 ] && ok "exit 0" || bad "exit $RC (want 0)"
assert_contains "daemon unknown" "$OUT" "K2 daemon: didn't answer, so its version is unknown."
assert_contains "updates with K2" "$OUT" "The CLI updates together with K2; k2 never downloads itself."

echo "== no network, no sudo, CLI untouched =="
if [ -s "$FORBIDDEN_LOG" ]; then
    bad "forbidden calls made:"
    sed 's/^/      | /' "$FORBIDDEN_LOG" >&2
else
    ok "only the loopback /boot-status was called"
fi
[ "$(grep -c '^/boot-status$' "$HITS_LOG")" -eq 2 ] \
    && ok "one /boot-status read per run with the daemon up" \
    || { bad "expected 2 /boot-status reads"; sed 's/^/      | /' "$HITS_LOG" >&2; }
[ "$(sum256 "$K2_CLI")" = "$CLI_SUM_BEFORE" ] \
    && ok "cli/k2 unchanged" || bad "cli/k2 was rewritten"
if grep -q 'raw.githubusercontent.com/Alakazam-211/K2/main/cli/k2' "$CLI_BASH_SOURCE"; then
    bad "the CLI still names the raw-branch cli/k2 URL"
else
    ok "no raw-branch URL left in the CLI"
fi

echo ""
echo "update_cli_no_download: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
