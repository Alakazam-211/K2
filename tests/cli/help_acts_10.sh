#!/usr/bin/env bash
# Help never acts, range 10 (0.46.17): -h or --help on each form below
# prints its help page on stdout, nothing on stderr, exits 0, and sends no
# request at all; it needs no daemon either.
#
# Regressions, each found by the k5 native bar:
#   - `k2 onboarding scan|defer|start-fresh|later|fresh --help` ran
#     `cd` on the word: cd's error, or (bash 4 and later, --help) the
#     request sent with cd's help text as the folder. `adopt --help` sent
#     an adopt of a file named --help.
#   - `k2 migrate status --help` asked for the status, and a value flag
#     took the help word: `export-secrets --to -h --out f` sent the export
#     with -h as the move key.
#   - `k2 reserve --help` and `k2 release --help` were the usage error
#     "'--help' is not a path" (exit 2): they had no help.
#   - `k2 read --help` asked to read a workspace named --help, and
#     `--lines`, `--agent`, `--session` and `--token` took the help word.
#   - `k2 reviews --help` asked for the review list (it reads no words).
#   - `k2 trust-folders --help` sent the trust POST (it reads no words).
#   - `k2 glossary --help` looked up a term named --help.
#   - `k2 setup skip --help` skipped a step named --help, and `--name`,
#     `--tool`, `--about`, `--handle`, `--focus-group` and `--attach` took
#     the help word.
#   - `k2 sessions live --help` asked for the live sessions of a workspace
#     named --help; `set-label <id> --help` set the label to --help;
#     `compact --help` looked for a session named --help; `archives
#     --help` listed the archives instead.
#   - `k2 notifications email status --help` asked for the status, `sender
#     resume --help` resumed a sender named --help, `system-sender set
#     --help` set it, `clear --help` cleared it, `test --help` sent the
#     test mail, `relay rm --help` removed a relay named --help.
#   - `k2 notify email mute --help` muted a workspace named --help (and
#     unmute unmuted it); --subject and --text took the help word.
#   - `k2 mail create --help` printed the moved error through python3
#     (exit 2): no page, and not a print the k5 binary could take.
#
# Each section: every help form, with --help and with -h, prints exactly
# the page's cli/text/help file and the stub daemon records nothing; the
# same forms with no daemon at all (no port, no token) print the same
# page; then a plain form still sends its request, so the stub is known to
# see one.
#
# How: a person's terminal outside K2 (the owner token and port on disk
# in a temp HOME, no passport) and a python stub daemon on a free
# loopback port that records every request. No real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/help_acts_10.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
for v in $(env | grep -E '^K[25](SO)?_' | cut -d= -f1); do unset "$v"; done
TEXT="$(cd "$SCRIPT_DIR/../../cli/text/help" && pwd)"

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-help-acts-10-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        raw = json.dumps({"ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
    def do_PUT(self):
        self._handle()
    def do_DELETE(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
HOME_ON="$WORK/home"
HOME_OFF="$WORK/nohome"
mkdir -p "$HOME_ON/.k2" "$HOME_OFF" "$WORK/proj"
cat "$WORK/port" >"$HOME_ON/.k2/heartbeat.port"
echo owner-token-for-test >"$HOME_ON/.k2/heartbeat.token"
cd "$WORK/proj"

# run <home> <args...>: stdout to $WORK/out, stderr to $WORK/err, RC set.
run() {
    local home="$1"; shift
    : >"$REQ_LOG"
    set +e
    HOME="$home" "$K2" "$@" </dev/null >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
}

show() { sed 's/^/      | /' "$1" >&2; }

# page_once <home> <page> <args...>: exactly the page on stdout, nothing on
# stderr, exit 0, nothing sent.
page_once() {
    local home="$1" page="$2"; shift 2
    local label="k2 $*"
    [ "$home" = "$HOME_OFF" ] && label="$label (no daemon)"
    run "$home" "$@"
    [ "$RC" -eq 0 ] && ok || bad "$label: exit $RC (want 0)"
    cmp -s "$WORK/out" "$TEXT/$page" && ok \
        || { bad "$label: stdout is not $page"; show "$WORK/out"; }
    [ ! -s "$WORK/err" ] && ok || { bad "$label: stderr"; show "$WORK/err"; }
    [ ! -s "$REQ_LOG" ] && ok || { bad "$label: sent a request"; show "$REQ_LOG"; }
}

# page <page> <args with @H for the help word...>: page_once with --help
# and with -h in @H's place, with the stub and with no daemon.
page() {
    local p="$1"; shift
    local h a args
    for h in --help -h; do
        args=()
        for a in "$@"; do
            if [ "$a" = "@H" ]; then args+=("$h"); else args+=("$a"); fi
        done
        page_once "$HOME_ON" "$p" "${args[@]}"
        page_once "$HOME_OFF" "$p" "${args[@]}"
    done
}

# sends <pattern> <args...>: the plain form sends a request matching
# <pattern> (grep -E on "METHOD /path body").
sends() {
    local pat="$1"; shift
    run "$HOME_ON" "$@"
    grep -Eq -- "$pat" "$REQ_LOG" && ok \
        || { bad "k2 $*: never sent $pat"; show "$REQ_LOG"; show "$WORK/err"; }
}

echo "== onboarding =="
page onboarding.txt onboarding @H
page onboarding.txt onboarding @H scan
for verb in scan defer start-fresh later fresh; do
    page onboarding.txt onboarding "$verb" @H
    page onboarding.txt onboarding "$verb" "$WORK/proj" @H
done
page onboarding.txt onboarding adopt @H
page onboarding.txt onboarding adopt CLAUDE.md @H
page onboarding.txt onboarding adopt CLAUDE.md "$WORK/proj" @H
sends '^GET /cli/onboarding/scan ' onboarding scan "$WORK/proj"
sends '^POST /cli/onboarding/skip ' onboarding defer "$WORK/proj"

echo "== migrate =="
page migrate.txt migrate status @H
page migrate.txt migrate status --json @H
page migrate.txt migrate @H status
page migrate.txt migrate export-secrets --to @H --out f.k2mv
page migrate.txt migrate export-secrets --to age1x --out @H
page migrate.txt migrate export-secrets --to age1x --out f.k2mv @H
page migrate.txt migrate export-secrets --unfreeze @H
page migrate.txt migrate import-secrets begin @H
page migrate.txt migrate import-secrets f.k2mv --apply @H
page migrate.txt migrate import-secrets --undo @H
sends '^GET /cli/migrate/secrets/status ' migrate status

echo "== reserve and release =="
page reserve.txt reserve @H
page reserve.txt reserve some/path @H
page reserve.txt reserve --agent @H some/path
page reserve.txt reserve --agent tester some/path @H
page release.txt release @H
page release.txt release some/path @H
page release.txt release --agent @H
page release.txt release --agent tester @H
sends '^POST /cli/reserve ' reserve --agent tester some/path
sends '^POST /cli/release ' release --agent tester

echo "== read =="
page read.txt read @H
page read.txt read scout @H
page read.txt read scout --lines @H
page read.txt read scout --agent @H
page read.txt read --session @H
page read.txt read scout@remote.example --token @H
sends '^GET /cli/terminal/read ' read scout

echo "== reviews =="
page reviews.txt reviews @H
page reviews.txt reviews --json @H
sends '^GET /cli/reviews ' reviews

echo "== trust-folders =="
page trust-folders.txt trust-folders @H
page trust-folders.txt trust-folders now @H
sends '^POST /cli/trust-folders ' trust-folders

echo "== glossary =="
page glossary.txt glossary @H
page glossary.txt glossary inbox @H
sends '^GET /cli/glossary/get ' glossary inbox
sends '^GET /cli/glossary/list ' glossary

echo "== setup =="
page setup.txt setup @H
page setup.txt setup skip @H
page setup.txt setup skip dev_tools @H
page setup.txt setup hide @H
page setup.txt setup kessel --name @H
page setup.txt setup kessel --tool claude --handle @H
page setup.txt setup chief-of-staff --name Ada --about @H --tool claude
page setup.txt setup chief-of-staff --focus-group @H
page setup.txt setup agent-reports --attach @H
sends '^POST /cli/first-run/skip \{"row": "dev_tools"\}' setup skip dev_tools

echo "== sessions =="
page sessions.txt sessions @H
page sessions.txt sessions past @H
page sessions.txt sessions live @H
page sessions.txt sessions live myws @H
page sessions.txt sessions live myws --json @H
page sessions.txt sessions archives @H
page sessions.txt sessions archives --json @H
page sessions.txt sessions compact @H
page sessions.txt sessions compact sess-1 @H
page sessions.txt sessions set-label @H
page sessions.txt sessions set-label sess-1 @H
page sessions.txt sessions set-label sess-1 new label @H
sends '^POST /cli/sessions/label ' sessions set-label sess-1 new label

echo "== notifications =="
page notifications.txt notifications @H email
page notifications.txt notifications email status @H
page notifications.txt notifications email status --json @H
page notifications.txt notifications email sender resume @H
page notifications.txt notifications email sender resume s1 @H
page notifications.txt notifications email system-sender show @H
page notifications.txt notifications email system-sender set @H
page notifications.txt notifications email system-sender set s1 @H
page notifications.txt notifications email system-sender clear @H
page notifications.txt notifications email system-sender test @H
page notifications.txt notifications email relay list @H
page notifications.txt notifications email relay rm @H
page notifications.txt notifications email relay rm r1 @H
page notifications.txt notifications email relay test r1 --from @H
page notifications.txt notifications email settings --owner-email @H
sends '^POST /cli/notifications/email/relay/rm ' notifications email relay rm r1
sends '^POST /cli/notifications/email/system-sender/clear ' notifications email system-sender clear

echo "== notify =="
page notify.txt notify @H email
page notify.txt notify email mute @H
page notify.txt notify email mute myws @H
page notify.txt notify email unmute @H
page notify.txt notify email off @H
page notify.txt notify email owner --subject @H --text hi
page notify.txt notify email owner --subject hi --text @H
sends '^POST /cli/notify/email/mute .*"muted": true' notify email mute myws

echo "== mail create (moved to hostmail create) =="
page hostmail/create.txt mail create @H
page hostmail/create.txt mail create bot @H
page hostmail/create.txt mail create bot --domain @H
# The same page `k2 hostmail create --help` prints (that one asks for a
# connection to K2 first).
page_once "$HOME_ON" hostmail/create.txt hostmail create --help
# The other moved verbs keep the moved error: exit 2, nothing sent.
run "$HOME_ON" mail status --help
[ "$RC" -eq 2 ] && ok || bad "k2 mail status --help: exit $RC (want 2)"
grep -q "moved to 'k2 hostmail status'" "$WORK/err" && ok || { bad "k2 mail status --help: stderr"; show "$WORK/err"; }
[ ! -s "$REQ_LOG" ] && ok || { bad "k2 mail status --help: sent a request"; show "$REQ_LOG"; }
run "$HOME_ON" mail create bot
[ "$RC" -eq 2 ] && ok || bad "k2 mail create bot: exit $RC (want 2)"
grep -q "moved to 'k2 hostmail create'" "$WORK/err" && ok || { bad "k2 mail create bot: stderr"; show "$WORK/err"; }

echo ""
echo "help_acts_10: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
