#!/usr/bin/env bash
# Permission cards (prd-agent-settings-cards-v1 rev 3, §2.4 / §13.1):
# "off = today, byte for byte" — the CLI half of the off-goldens.
#
# The fixtures under tests/golden/permission_off/cli/ were captured from
# integ/0.46.3-r3 BEFORE any permission-cards code existed. While the
# "Permission cards" switch is off (the default), and for an agent whose own
# "Can ask for permission cards" switch is off, every byte below must stay
# the same: the help tiers, `k2 study` (catalog, every page, the JSON forms,
# the unknown-topic text for `permissions`), `k2 --schema`, the unknown
# command text for `k2 permission …`, and how a refused verb is printed plus
# the request line and headers it sends (no `X-K2-Cmd` header while off).
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, the
# daemon is a tiny python stub on loopback that answers every request with
# one fixed refusal and logs the request line + headers. No real daemon,
# no ~/.k2, no keychain.
#
# Normalizer (documented; it never drops a line or a field):
#   * the CLI's OWN version (K2_CLI_VERSION, bumped by release.sh) → <K2_CLI_VERSION>,
#     only where the CLI prints it as its version: `k2 --schema`'s top-level
#     `"version": "…"` field and a `k2 <version>` line. Prose that names a
#     release ("FILES (0.46.4, App only…)", "Gardens in 0.46.3") stays
#     literal, so a release.sh bump never moves a golden (0.46.5: the old
#     blanket replace turned whichever release matched the current version
#     into the placeholder, and 32 pages broke on every bump).
#     tests/cli/permission_off_golden_version_bump.sh proves it: the same
#     goldens pass against a copy of the CLI with a fake version.
#   * the stub's ephemeral port → <PORT>
#   * the sandbox path → <SANDBOX>
#   * percent-escapes → upper case (macOS and Linux encoders differ in case
#     only), then the sandbox path's URL-encoded form → <SANDBOX>
#   * a request's Content-Length, when its body carries the sandbox path →
#     "<bytes without it>+<k>x<SANDBOX>" (still exact; the path's length
#     differs between macOS /private/tmp and Linux /tmp)
#   * a `User-Agent: curl/x.y.z` / `Python-urllib/x.y` value → …/<VERSION>
#
# Modes:
#   bash tests/cli/permission_off_golden.sh              compare (fails loudly on any diff or missing fixture)
#   PERMISSION_GOLDEN_BLESS=1 bash tests/cli/permission_off_golden.sh
#                                                        (re)capture — only from a base WITHOUT cards code,
#                                                        e.g. after another lane changes help text (S10, lane I)
#
# Named exceptions (§2.4): none on the CLI side. The Thread hygiene fixes and
# the keep-as-mine card are daemon-side and have their own tests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# PERMISSION_GOLDEN_CLI: run another copy of the CLI against the same
# goldens (the version-bump test uses it). Default: this checkout's cli/k2.
K2="${PERMISSION_GOLDEN_CLI:-$PROJECT_ROOT/cli/k2}"
GOLDEN="$PROJECT_ROOT/tests/golden/permission_off/cli"
BLESS="${PERMISSION_GOLDEN_BLESS:-0}"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

CLI_VERSION="$(grep -m1 '^K2_CLI_VERSION=' "$K2" | cut -d'"' -f2)"
[ -n "$CLI_VERSION" ] || { echo "FAIL: no K2_CLI_VERSION in $K2" >&2; exit 1; }

SANDBOX="$(mktemp -d /tmp/k2pog-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# ── the stub daemon: one fixed refusal for everything, request log ──────
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = sys.argv[1]
BODY = json.dumps({"error": {"code": "owner_only", "hint": "only the server's owner may do this"}}).encode()

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        with open(LOG, "a") as f:
            f.write("> %s %s\n" % (self.command, self.path))
            for k, v in self.headers.items():
                f.write("> %s: %s\n" % (k, v))
            f.write("> body: %s\n" % body)
        self.send_response(403)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(BODY)
    do_GET = handle_any
    do_POST = handle_any
    do_PUT = handle_any
    do_DELETE = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
: > "$LOG"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
# A fixed cwd: verbs that send `project=<cwd>` send the sandbox path, which
# the normalizer folds to <SANDBOX>.
cd "$SANDBOX/ws"

normalize() {
    # stdin → stdout. Never drops a line.
    python3 -I -c '
import re, sys
ver, port, sandbox = sys.argv[1], sys.argv[2], sys.argv[3]
from urllib.parse import quote
t = sys.stdin.read()
# A logged request body that carries the sandbox path has a Content-Length
# that depends on that path (/private/tmp/… on macOS, /tmp/… on Linux).
# Keep it exact, relative to the path: "<bytes without it>+<k>x<SANDBOX>".
lines = t.split("\n")
cl_at = None
for i, line in enumerate(lines):
    if re.match(r"> [A-Z]+ /", line):
        cl_at = None
    elif line.lower().startswith("> content-length: "):
        cl_at = i
    elif line.startswith("> body: ") and cl_at is not None:
        k = line.count(sandbox)
        if k:
            n = int(lines[cl_at].split(":", 1)[1].strip()) - k * len(sandbox.encode())
            lines[cl_at] = lines[cl_at].split(":", 1)[0] + ": %d+%dx<SANDBOX>" % (n, k)
        cl_at = None
t = "\n".join(lines)
# Percent-escapes in upper case: macOS and Linux encoders differ in case only.
t = re.sub(r"%[0-9a-fA-F]{2}", lambda m: m.group(0).upper(), t)
for form in (sandbox, quote(sandbox, safe="")):
    t = t.replace(form, "<SANDBOX>")
# The CLI own version, only where it is printed AS the version (see the
# header): never in prose that names a release.
t = re.sub(r"(?m)^(  \"version\": \")" + re.escape(ver) + r"(\",)$", r"\1<K2_CLI_VERSION>\2", t)
t = re.sub(r"(?m)^k2 " + re.escape(ver) + r"$", "k2 <K2_CLI_VERSION>", t)
t = re.sub(r"(?<![0-9])" + re.escape(port) + r"(?![0-9])", "<PORT>", t)
t = re.sub(r"curl/[0-9][0-9.]*", "curl/<VERSION>", t)
t = re.sub(r"Python-urllib/[0-9][0-9.]*", "Python-urllib/<VERSION>", t)
sys.stdout.write(t)
' "$CLI_VERSION" "$PORT" "$SANDBOX"
}

pass=0
fail=0
blessed=0

# capture <name> <env-mode> -- <args…>
#   env-mode: offline   K2_PORT=1 + a dummy token (help/study/usage paths; nothing reaches a daemon)
#             passport  K2_PORT=<stub> + K2_HOOK_TOKEN (an agent in a K2 terminal over TCP)
capture() {
    local name="$1" mode="$2"
    shift 3
    local out="$SANDBOX/out" err="$SANDBOX/err" rc
    : > "$LOG"
    set +e
    case "$mode" in
        offline) K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2" "$@" </dev/null >"$out" 2>"$err"; rc=$? ;;
        passport) K2_PORT="$PORT" K2_HOOK_TOKEN=passport-golden "$K2" "$@" </dev/null >"$out" 2>"$err"; rc=$? ;;
        *) echo "FAIL: bad mode $mode" >&2; exit 1 ;;
    esac
    set -e
    local got="$SANDBOX/got"
    {
        printf '$ k2'
        printf ' %q' "$@"
        printf '\n'
        printf 'exit: %s\n' "$rc"
        printf -- '--- stdout\n'
        cat "$out"
        printf -- '--- stderr\n'
        cat "$err"
        printf -- '--- requests\n'
        cat "$LOG"
    } | normalize > "$got"
    local want="$GOLDEN/$name.txt"
    if [ "$BLESS" = "1" ]; then
        mkdir -p "$GOLDEN"
        cp "$got" "$want"
        blessed=$((blessed + 1))
        return
    fi
    if [ ! -f "$want" ]; then
        echo "  FAIL: $name: no golden at $want (capture it from the base with PERMISSION_GOLDEN_BLESS=1)" >&2
        fail=$((fail + 1))
        return
    fi
    if cmp -s "$got" "$want"; then
        pass=$((pass + 1))
    else
        echo "  FAIL: $name differs from today's output:" >&2
        diff -u "$want" "$got" | head -40 >&2 || true
        fail=$((fail + 1))
    fi
}

echo "== help tiers =="
capture help              offline -- help
capture help_dashdash     offline -- --help
capture help_h            offline -- -h
capture help_advanced     offline -- help --advanced
capture help_internal     offline -- help --internal
capture help_deprecated   offline -- help --deprecated
capture help_study        offline -- help study
capture help_permission   offline -- help permission
capture schema            offline -- --schema

echo "== k2 study =="
capture study             offline -- study
capture study_list        offline -- study list
capture study_json        offline -- study --json
capture study_help        offline -- study --help
capture study_permissions offline -- study permissions
capture study_permissions_json offline -- study permissions --json
TOPICS="$(grep -m1 '^_STUDY_TOPIC_IDS=' "$K2" | cut -d'"' -f2)"
[ -n "$TOPICS" ] || { echo "FAIL: no _STUDY_TOPIC_IDS in $K2" >&2; exit 1; }
for t in $TOPICS; do
    capture "study_page_$t" offline -- study "$t"
    capture "study_page_${t}_json" offline -- study "$t" --json
done

echo "== k2 permission (unknown today) =="
capture permission              offline -- permission
capture permission_request      offline -- permission request compute.use --why x
capture permission_list         offline -- permission list
capture permission_beta_status  offline -- permission beta status

echo "== refused verbs, as an agent over TCP (request line + headers) =="
capture refused_compute_nodes    passport -- compute nodes
capture refused_connections_add  passport -- connections add billing
capture refused_compute_node_rm  passport -- compute node remove mini-9
capture refused_dns_record_add   passport -- dns record add example.com A www 192.0.2.1
capture refused_sidecar_new      passport -- sidecar new reviewer --harness claude
capture refused_app_approve      passport -- app approve ar_22
capture refused_project_create   passport -- project create demo
capture refused_vault_reveal     passport -- vault reveal STRIPE_KEY
capture refused_settings_set     passport -- settings set computePreview true

if [ "$BLESS" = "1" ]; then
    echo "blessed $blessed golden files into $GOLDEN"
    exit 0
fi
# A golden on disk that no capture compared is a dropped case: fail loudly.
expected=$((pass + fail))
on_disk="$(find "$GOLDEN" -name '*.txt' | wc -l | tr -d ' ')"
if [ "$on_disk" != "$expected" ]; then
    echo "  FAIL: $on_disk golden files on disk but $expected compared (a case was dropped or renamed)" >&2
    fail=$((fail + 1))
fi
echo "permission_off_golden: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
