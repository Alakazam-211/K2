#!/usr/bin/env bash
# Rosson's 0.46.0 runtime decisions (2026-10-16), CLI side, against a tiny
# stub daemon on loopback (python3 http.server) plus a stub vault.sock. No
# real daemon, vault, keychain or ~/.k2: HOME is a throwaway dir.
#
# Covers:
#  1. k2 app box prepare prints the AppArmor status line when box prep
#     installed the distro's apparmor package (and the dnf note).
#  2. One confirm code: k2 app exception <app> on surfaces the daemon's
#     vault_confirm_required (exit 3, the code on stderr).
#  3. The publish-run gate is warn-only: k2 publish run says the service
#     ran, then what changes in 0.46.1; owner-token tracing off Linux says
#     "not available on macOS" in k2 app warnings and clean-mode, and says
#     nothing extra where it is on.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/rd-0460-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, os, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG, PORTFILE, MODE_FILE, SOCK = sys.argv[1:5]
NOTE = "owner-token calls are traced on Linux only for now; the code scan still names any read of the owner token file"

def mode():
    with open(MODE_FILE) as f:
        return f.read().strip()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        return json.loads(raw or b"{}")
    def record(self, body):
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": self.command, "path": urlparse(self.path).path, "body": body}) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        self.record(None)
        if u.path == "/cli/app/warnings":
            t = "on" if mode() == "linux" else "not available on macOS"
            rep = {"app": {"handle": "shop", "envMode": "clean"}, "cutover": None, "readyForCutover": True, "breaks": 0,
                   "warnings": [], "scan": {}, "ownerTokenAttribution": t}
            return self.reply(200, {"cutover": None, "services": [rep] if mode() != "empty" else [], "breaks": 0, "ownerTokenAttribution": t})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        u = urlparse(self.path)
        b = self.body()
        self.record(b)
        if u.path == "/cli/vault/confirm/start":
            return self.reply(200, {"methods": ["vault-password"]})
        if u.path == "/cli/vault/confirm/finish":
            return self.reply(200, {"token": "rd-tok-1"})
        if u.path == "/cli/app/exception":
            return self.reply(403, {"error": {"code": "vault_confirm_required", "hint": "confirm again: a confirm works once, for this App, within 60 seconds"}})
        if u.path == "/cli/sources/remove":
            code = "confirm_required" if mode() == "old" else "vault_confirm_required"
            return self.reply(403, {"error": {"code": code, "hint": "binding a secret needs a vault confirm"}})
        if u.path == "/cli/app/clean-mode":
            out = {"ok": True, "app": {"handle": "shop"}, "cleanMode": True, "envMode": "clean", "restart": "applies at the next start",
                   "ownerTokenAttribution": "on" if mode() == "linux" else "not available on macOS"}
            if mode() != "linux":
                out["ownerTokenAttributionNote"] = NOTE
            return self.reply(200, out)
        if u.path == "/cli/publish/run":
            why = "from K2 0.46.1 a new --cmd service from an agent is an open-path App: it needs the switches, or the run is held for the owner"
            return self.reply(200, {"name": b.get("name"), "status": "running", "port": b.get("port"), "url": "",
                                    "crossover": {"from": "0.46.1", "mode": "warn", "held": False, "wouldHold": True, "hint": why,
                                                  "message": "%s ran: this release only warns. F%s" % (b.get("name"), why[1:])}})
        if u.path == "/cli/app/box/prepare":
            aa = ({"needed": True, "installed": ["apparmor"], "parser": "/usr/sbin/apparmor_parser"} if mode() != "dnf" else
                  {"needed": True, "installed": [], "parser": None, "note": "AppArmor is on in the kernel but K2 has no fixed AppArmor package for dnf: install apparmor_parser with the distro's tools, then prepare again"})
            steps = [{"verb": "apps packages install", "ok": True, "result": {"podman": "5.4.2", "podmanOk": True, "apparmor": aa}},
                     {"verb": "apps apparmor on", "ok": True, "result": {"lsm": "apparmor", "applied": True, "changed": True}}]
            return self.reply(200, {"ok": True, "runtime": "crun", "steps": steps,
                                    "box": {"ready": True, "runtime": "crun", "runtimeRecorded": True,
                                            "checks": [{"id": "mac", "status": "ok", "detail": "AppArmor: Apps run under the k2-app profile (enforce)"}]}})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
class UH(H):
    def address_string(self):
        return "unix"

if SOCK != "-":
    if os.path.exists(SOCK):
        os.unlink(SOCK)
    us = U(SOCK, UH)
    threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(PORTFILE, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
MODE="$SANDBOX/mode"
: > "$LOG"
echo mac > "$MODE"
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$MODE" "$HOME/.k2/vault.sock" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && [ -S "$HOME/.k2/vault.sock" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"
[ -S "$HOME/.k2/vault.sock" ] || { bad "stub vault.sock is up"; exit 1; }
# Owner-style environment: port + token from the throwaway ~/.k2, no session.
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
mkdir -p "$SANDBOX/ws"
cd "$SANDBOX/ws"

last_body() { tail -1 "$LOG" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["body"], sort_keys=True))'; }

echo "== 1. box prepare: the AppArmor status line =="
out="$("$K2" app box prepare 2>&1)"
expect_has "prepare lists the packages step" "$out" "ok   apps packages install"
expect_has "prepare says it installed AppArmor's package" "$out" "AppArmor: installed the distro package apparmor (apparmor_parser /usr/sbin/apparmor_parser) for the k2-app profile"
expect_has "then the profile step" "$out" "ok   apps apparmor on  (changed)"
echo dnf > "$MODE"
out="$("$K2" app box prepare 2>&1)"
expect_has "dnf: prepare says why nothing was added" "$out" "AppArmor: AppArmor is on in the kernel but K2 has no fixed AppArmor package for dnf"
echo mac > "$MODE"

echo "== 2. one confirm code =="
set +e
err="$(printf 'pw\n' | "$K2" app exception shop on 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "exception on refused without a valid confirm exits 3" "$rc" "3"
expect_has "the CLI surfaces vault_confirm_required" "$err" '"error": "vault_confirm_required"'
expect_not "never the retired owner_confirm_required" "$err" '"owner_confirm_required"'
expect_eq "the confirm token rode the request" "$(last_body)" '{"app": "shop", "confirmToken": "rd-tok-1", "devices": [], "on": true}'
# External Data (sources): the same code; a 0.45.x daemon's confirm_required
# is still read as "needs a person" for one release.
for m in new old; do
    echo "$m" > "$MODE"
    set +e
    err="$("$K2" sources remove fleet --yes 2>&1 >/dev/null)"; rc=$?
    set -e
    expect_eq "sources: a confirm refusal ($m daemon) exits 3" "$rc" "3"
    expect_has "sources: it says a person is needed ($m daemon)" "$err" "This needs a person (run it outside K2"
done
expect_has "sources: a 0.46 daemon's code is shown" "$(echo new > "$MODE"; "$K2" sources remove fleet --yes 2>&1 || true)" "vault_confirm_required"
echo mac > "$MODE"

echo "== 3. warn-only publish run + Linux-only tracing =="
set +e
err="$("$K2" publish run shop --cmd 'python3 -m http.server $PORT' --port 4321 --no-tunnel 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "a gated run still succeeds (warn-only)" "$rc" "0"
expect_has "the warning says it ran first" "$err" "warning: shop ran: this release only warns. From K2 0.46.1"
out="$("$K2" app warnings 2>&1)"
expect_has "warnings on a Mac say tracing isn't available" "$out" "owner-token tracing: not available on macOS"
echo empty > "$MODE"
out="$("$K2" app warnings 2>&1)"
expect_has "an empty list still says so" "$out" "owner-token tracing: not available on macOS"
echo mac > "$MODE"
out="$("$K2" app clean-mode shop on 2>&1)"
expect_has "clean-mode on a Mac says tracing isn't available" "$out" "owner-token tracing: not available on macOS"
echo linux > "$MODE"
out="$("$K2" app warnings 2>&1)"
expect_not "Linux: no tracing line" "$out" "owner-token tracing"
out="$("$K2" app clean-mode shop on 2>&1)"
expect_not "Linux: clean-mode has no tracing line" "$out" "owner-token tracing"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
