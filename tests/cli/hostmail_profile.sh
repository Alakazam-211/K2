#!/usr/bin/env bash
# hostmail profile (calendars S6: Apple setup profile) CLI wiring — no
# daemon, no Stalwart. Static wiring + help + schema + usage errors, then a
# local echo server checks the exact request, the 0600 file write, the
# overwrite refusal (before any request), and that the app-password secret
# never reaches human output or stderr.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'profile) cmd_hostmail_profile' "$K2" || fail "cmd_hostmail must route profile"
grep -A8 'verb == "profile"' "$K2" | grep -q '"/cli/mail/profile"' \
  || fail "profile verb must POST /cli/mail/profile"
# Never named connect (top-level `k2 connect` is the Connect account).
grep -q 'hostmail connect' "$K2" && fail "there must be no 'hostmail connect'"

# Help.
help="$("$K2" hostmail profile --help)"
for want in '--apple' '--out' '--force' 'k2-profile-<YYYY-MM-DD>' 'app-password revoke' \
            'AirDrop' 'Profile Downloaded' 'Install' 'Unverified' 'k2 hostmail bans' \
            'allowlist' 'Owner/admin' '/dav/cal' '/dav/card' '993' '465' 'k2.dev' \
            'linked' 'retired'; do
  printf '%s' "$help" | grep -qF -- "$want" || fail "profile --help must mention '$want'"
done
printf '%s' "$help" | grep -q 'never read back or reused' || fail "help must say a stored secret is never reused"
"$K2" hostmail --help | grep -q 'profile <addr> --apple' || fail "hostmail --help must list profile"
"$K2" study mail | grep -q 'k2 hostmail profile' || fail "study mail must list hostmail profile as owner-only"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
grep -qF '"name": "hostmail profile"' <<<"$schema" || fail "schema missing hostmail profile"
grep -qF 'calendar|profile> ... (server admin)' <<<"$schema" || fail "hostmail usage must list profile"

# Usage errors exit 2 before any request.
expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
}
expect_usage hostmail profile
expect_usage hostmail profile alice@example.com
expect_usage hostmail profile --apple
expect_usage hostmail profile alice@example.com --apple --force
expect_usage hostmail profile alice@example.com --apple --out
expect_usage hostmail profile alice@example.com bob@example.com --apple
expect_usage hostmail profile alice@example.com --apple --bogus

# Live: a local echo server records method, path and body.
WORK="$(mktemp -d -t k2-hostmail-profile-XXXXXX)"
SRV_PID=""
cleanup() {
  if [ -n "$SRV_PID" ]; then
    kill "$SRV_PID" 2>/dev/null || true
    wait "$SRV_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
  hermetic_cli_cleanup
}
trap cleanup EXIT

SECRET="app_FixtureSecret-0000"
cat >"$WORK/srv.py" <<'PY'
import http.server, json, sys
log = open(sys.argv[1], "a")
secret = sys.argv[2]
PROFILE = ("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
           "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" "
           "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n"
           "<plist version=\"1.0\">\n<dict>\n"
           "\t<key>PayloadType</key>\n\t<string>Configuration</string>\n"
           "\t<key>PayloadContent</key>\n\t<array>\n\t\t<dict>\n"
           "\t\t\t<key>PayloadType</key>\n\t\t\t<string>com.apple.mail.managed</string>\n"
           "\t\t\t<key>IncomingPassword</key>\n\t\t\t<string>%s</string>\n"
           "\t\t</dict>\n\t</array>\n</dict>\n</plist>\n") % secret
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        log.write("%s %s %s\n" % (self.command, self.path.split("?")[0], body))
        log.flush()
        addr = json.loads(body).get("address")
        if addr == "agent@example.com":
            code, reply = 403, {"ok": False, "error": {"code": "owner_only",
                "hint": "requires owner/admin — 'k2 hostmail profile' stays owner-only. Ask your human."}}
        else:
            code, reply = 200, {"ok": True, "address": addr, "format": "apple",
                "mailHost": "mail.example.com", "includes": ["mail", "caldav", "carddav"],
                "appPasswordId": "ap-7", "description": "k2-profile-2026-10-06",
                "filename": addr + ".mobileconfig", "signed": False, "profile": PROFILE,
                "notes": ["a device signing in with a stale or wrong password gets its network's IP banned — see k2 hostmail bans list / k2 hostmail allowlist"],
                "revoke": "k2 hostmail app-password revoke %s ap-7" % addr}
        out = json.dumps(reply).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
: >"$WORK/port"
: >"$WORK/requests"
python3 "$WORK/srv.py" "$WORK/requests" "$SECRET" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
  env -i PATH="$PATH" HOME="$K2_TEST_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
    "$K2" "$@"
}

mode_of() {
  if stat -c '%a' "$1" >/dev/null 2>&1; then stat -c '%a' "$1"; else stat -f '%Lp' "$1"; fi
}

# --out: file written 0600, content = the profile, secret not on the terminal.
human="$(k2 hostmail profile alice@example.com --apple --out "$WORK/alice.mobileconfig" 2>"$WORK/err1")"
[ -f "$WORK/alice.mobileconfig" ] || fail "--out must write the file"
[ "$(mode_of "$WORK/alice.mobileconfig")" = "600" ] || fail "--out file must be mode 0600, got $(mode_of "$WORK/alice.mobileconfig")"
grep -qF "$SECRET" "$WORK/alice.mobileconfig" || fail "the profile file must carry the app password"
python3 -c 'import plistlib,sys; d=plistlib.load(open(sys.argv[1],"rb")); assert d["PayloadType"]=="Configuration", d' \
  "$WORK/alice.mobileconfig" || fail "written file must parse as a plist"
printf '%s' "$human" | grep -qF "$SECRET" && fail "human output must never print the secret: $human"
grep -qF "$SECRET" "$WORK/err1" && fail "stderr must never print the secret"
printf '%s' "$human" | grep -q '^wrote    : ' || fail "human output: $human"
printf '%s' "$human" | grep -q 'mail, caldav, carddav' || fail "human output must list includes: $human"
printf '%s' "$human" | grep -q 'k2 hostmail app-password revoke alice@example.com ap-7' || fail "human output must print revoke: $human"
printf '%s' "$human" | grep -q 'AirDrop' || fail "human output must say how to install: $human"
printf '%s' "$human" | grep -q 'k2 hostmail bans' || fail "human output must carry the ban note: $human"

# Existing file without --force: refused, exit 2, and NO request (nothing minted).
before="$(wc -l <"$WORK/requests")"
expect_usage hostmail profile alice@example.com --apple --out "$WORK/alice.mobileconfig"
set +e
k2 hostmail profile alice@example.com --apple --out "$WORK/alice.mobileconfig" >/dev/null 2>&1
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "existing --out without --force must exit 2, got $rc"
[ "$(wc -l <"$WORK/requests")" = "$before" ] || fail "an overwrite refusal must not reach the daemon"
expect_usage hostmail profile alice@example.com --apple --out "$WORK"
expect_usage hostmail profile alice@example.com --apple --out "$WORK/missing-dir/x.mobileconfig"

# --force overwrites and re-tightens the mode to 0600.
chmod 644 "$WORK/alice.mobileconfig"
k2 hostmail profile alice@example.com --apple --out "$WORK/alice.mobileconfig" --force >/dev/null
[ "$(mode_of "$WORK/alice.mobileconfig")" = "600" ] || fail "--force must leave mode 0600"

# --json with --out: no profile body in the JSON, out path instead.
j="$(k2 hostmail profile alice@example.com --apple --out "$WORK/j.mobileconfig" --json)"
printf '%s' "$j" | grep -qF "$SECRET" && fail "--json with --out must not echo the profile"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert "profile" not in d and d["out"].endswith("j.mobileconfig") and d["appPasswordId"]=="ap-7" and d["includes"]==["mail","caldav","carddav"], d' "$j" \
  || fail "--json --out shape: $j"

# No --out: XML alone on stdout, summary on stderr without the secret.
k2 hostmail profile alice@example.com --apple >"$WORK/stdout.xml" 2>"$WORK/err2"
python3 -c 'import plistlib,sys; plistlib.load(open(sys.argv[1],"rb"))' "$WORK/stdout.xml" \
  || fail "stdout must be the plist alone"
grep -qF "$SECRET" "$WORK/err2" && fail "stderr must never print the secret"
grep -q 'app-password revoke' "$WORK/err2" || fail "stderr must carry the revoke line"

# --json without --out carries the XML.
k2 hostmail profile alice@example.com --apple --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["profile"].startswith("<?xml"), d'

# Owner-only from the daemon → exit 3.
set +e
k2 hostmail profile agent@example.com --apple >/dev/null 2>"$WORK/err3"
rc=$?
set -e
[ "$rc" -eq 3 ] || fail "owner_only must exit 3, got $rc"
grep -q 'owner_only' "$WORK/err3" || fail "owner_only error on stderr"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
if len(rows) != 6:
    sys.exit("FAIL: expected 6 requests, got %d: %r" % (len(rows), rows))
for m, p, b in rows:
    if (m, p) != ("POST", "/cli/mail/profile"):
        sys.exit("FAIL: expected POST /cli/mail/profile, got %s %s" % (m, p))
    body = json.loads(b)
    if body.get("apple") is not True or set(body) != {"address", "apple"}:
        sys.exit("FAIL: body must be {address, apple:true}, got %s" % b)
PY
python3 "$WORK/check.py" "$WORK/requests"

echo "OK: hostmail profile CLI wiring"
