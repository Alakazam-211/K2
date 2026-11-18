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
# Normalizer (documented; on the bash CLI it never drops a line or a field):
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
#   * ONLY when the CLI under test is not the bash source (the k5 binary,
#     K5_CLI_UNDER_TEST): two request log lines are dropped per request,
#     for the bash a line that is exactly `> X-K5-Cli-Via: fallthrough`
#     and a line that is exactly `> X-K5-Cli-Noun: <noun>`, where <noun> is
#     the capture's first word (`project` for `k2 project create demo`;
#     `other` when that word is not [a-z][a-z0-9-]{0,31}, as the bash
#     sends it). The binary hands a verb it doesn't answer to the bash with
#     K5_CLI_VIA=rust, and the bash then marks every daemon request it
#     sends, with curl and from its embedded python's urlopen alike, with
#     those two headers (tests/cli/k5_cli_via.sh); the goldens are bash's
#     bytes. A verb the binary answers itself sends its own request, as
#     curl would but with `User-Agent: k5/<version>`, then
#     `X-K5-Cli-Version: <version>` and `X-K5-Cli-Via: native`
#     (crates/k5-cli/src/transport/mod.rs); for those the two lines are
#     dropped and the User-Agent becomes `curl/<VERSION>`. It is counted,
#     not trusted (drop_via): each request in the log sent by curl
#     (`User-Agent: curl/…`) or by the embedded python
#     (`User-Agent: Python-urllib/…`, e.g. `k2 project create`) must carry
#     exactly one `Via: fallthrough` and one Noun line and no Version line;
#     each sent by the binary (`User-Agent: k5/<version>`, <version> the
#     CLI's) exactly one `Via: native` and one Version line naming that
#     version, and no Noun line; any other request none of them. A Noun
#     line naming another word is not dropped. Anything else fails the
#     capture loudly, naming it and what it found; the bites before the
#     captures prove the rule refuses. On the bash CLI none of this runs: a
#     Via, Noun or Version line there stays and fails against its golden.
#
# Modes:
#   bash tests/cli/permission_off_golden.sh              compare (fails loudly on any diff or missing fixture)
#   PERMISSION_GOLDEN_BLESS=1 bash tests/cli/permission_off_golden.sh
#                                                        (re)capture — only from a base WITHOUT cards code,
#                                                        e.g. after another lane changes help text (S10, lane I).
#                                                        Only from the bash CLI: refused when
#                                                        K5_CLI_UNDER_TEST names another program.
#
# Named exceptions (§2.4): none on the CLI side. The Thread hygiene fixes and
# the keep-as-mine card are daemon-side and have their own tests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# PERMISSION_GOLDEN_CLI: run another copy of the CLI against the same
# goldens (the version-bump test uses it). Default: the CLI under test (K2) and the
# bash source (K2_SRC), from tests/cli/_cli_under_test.sh.
. "$SCRIPT_DIR/_cli_under_test.sh"
K2="${PERMISSION_GOLDEN_CLI:-$CLI_UNDER_TEST}"
K2_SRC="${PERMISSION_GOLDEN_CLI:-$CLI_BASH_SOURCE}"
GOLDEN="$PROJECT_ROOT/tests/golden/permission_off/cli"
BLESS="${PERMISSION_GOLDEN_BLESS:-0}"
# The k5 binary under test: its requests carry the fallthrough marker (see
# the normalizer notes above). Goldens are captured from the bash only.
VIA_MODE=0
[ "$CLI_UNDER_TEST" = "$CLI_BASH_SOURCE" ] || VIA_MODE=1
if [ "$BLESS" = "1" ] && [ "$VIA_MODE" = "1" ]; then
    echo "FAIL: PERMISSION_GOLDEN_BLESS=1 captures goldens from the bash CLI only, not from $CLI_UNDER_TEST (unset K5_CLI_UNDER_TEST)" >&2
    exit 1
fi

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

CLI_VERSION="$(grep -m1 '^K2_CLI_VERSION=' "$K2_SRC" | cut -d'"' -f2)"
[ -n "$CLI_VERSION" ] || { echo "FAIL: no K2_CLI_VERSION in $K2_SRC" >&2; exit 1; }

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
# These are the "permission cards OFF" goldens, and the server switch
# defaults on from 0.46.11: the stub says off explicitly to the CLI's probe.
PERMISSION_OFF = json.dumps({"enabled": False}).encode()

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
        off = self.path.split("?", 1)[0] == "/cli/permission/enabled"
        out = PERMISSION_OFF if off else BODY
        self.send_response(200 if off else 403)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(out)
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
    # stdin → stdout. Never drops a line (the hand-over lines go before
    # it, in drop_via, and only for the binary).
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

# drop_via <capture> <log> <out> <first word>: the binary's request log
# with the lines its own requests and the bash's hand-over add taken out,
# so it compares with the bash's golden. Per request, by its User-Agent:
#   * curl/… or Python-urllib/… (the bash, run by the binary): exactly one
#     `X-K5-Cli-Via: fallthrough` and one `X-K5-Cli-Noun: <noun>` (the
#     noun of <first word>), both dropped; no X-K5-Cli-Version.
#   * k5/<version> (the binary's own, native): exactly one
#     `X-K5-Cli-Via: native` and one `X-K5-Cli-Version: <version>`, both
#     dropped, <version> the CLI's and the User-Agent's; no X-K5-Cli-Noun
#     line at all. Its `User-Agent: k5/<version>` becomes curl's
#     `User-Agent: curl/<VERSION>`.
#   * anything else: none of these lines.
# Fails (non-zero, naming the capture and what it found) on anything else,
# or on such a line before any request. The bites below prove it refuses.
drop_via() {
    python3 -I -c '
import re, sys
name, src, dst, word, version = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
# The bash sends a word the daemon would not take as `other` (_k5_headers).
noun = word if re.fullmatch(r"[a-z][a-z0-9-]{0,31}", word) else "other"
request = re.compile(r"> [A-Z]+ /")
via = re.compile(r"> X-K5-Cli-Via: (fallthrough|native)\r?")
noun_line = re.compile(r"> X-K5-Cli-Noun: " + re.escape(noun) + r"\r?")
any_noun = re.compile(r"> X-K5-Cli-Noun:")
cli_version = re.compile(r"> X-K5-Cli-Version: (.*?)\r?")
bash_agent = re.compile(r"> User-Agent: (curl|Python-urllib)/")
k5_agent = re.compile(r"> User-Agent: k5/(.*?)(\r?)")
with open(src, encoding="utf-8", errors="surrogateescape", newline="") as f:
    lines = f.read().split("\n")
# Per request: who sent it, the Via values, the Noun lines (this noun, any
# noun), the X-K5-Cli-Version values, the User-Agent version (k5 only).
kept, reqs, stray = [], [], 0
for line in lines:
    if request.match(line):
        reqs.append({"by": None, "via": [], "noun": 0, "anynoun": 0, "ver": [], "ua": None})
        kept.append(line)
        continue
    r = reqs[-1] if reqs else None
    m = k5_agent.fullmatch(line)
    if r is not None and m:
        r["by"], r["ua"] = "k5", m.group(1)
        kept.append("> User-Agent: curl/<VERSION>" + m.group(2))
        continue
    if r is not None and bash_agent.match(line):
        r["by"] = "bash"
    if any_noun.match(line) and r is not None:
        r["anynoun"] += 1
    v, n, c = via.fullmatch(line), noun_line.fullmatch(line), cli_version.fullmatch(line)
    if v or n or c:
        if r is None:
            stray += 1
        elif v:
            r["via"].append(v.group(1))
        elif n:
            r["noun"] += 1
        else:
            r["ver"].append(c.group(1))
        continue
    kept.append(line)
bad = []
for i, r in enumerate(reqs, 1):
    if r["by"] == "bash":
        ok = r["via"] == ["fallthrough"] and r["noun"] == 1 and r["anynoun"] == 1 and not r["ver"]
        want = "one X-K5-Cli-Via: fallthrough, one X-K5-Cli-Noun: %s, no X-K5-Cli-Version" % noun
    elif r["by"] == "k5":
        ok = r["via"] == ["native"] and r["anynoun"] == 0 and r["ver"] == [version] and r["ua"] == version
        want = "one X-K5-Cli-Via: native, one X-K5-Cli-Version: %s, User-Agent k5/%s, no X-K5-Cli-Noun" % (version, version)
    else:
        ok = not r["via"] and r["noun"] == 0 and not r["ver"]
        want = "none of the hand-over lines"
    if not ok:
        bad.append("request %d (%s): Via %s, %d Noun line(s), Version %s, User-Agent version %s; want %s" % (
            i, r["by"] or "other", r["via"], r["anynoun"], r["ver"], r["ua"], want))
if stray or bad:
    sys.stderr.write("  FAIL: %s: %d hand-over line(s) before any request; %d of %d request(s) wrong%s\n" % (
        name, stray, len(bad), len(reqs), "".join("\n    " + b for b in bad)))
    sys.exit(1)
with open(dst, "w", encoding="utf-8", errors="surrogateescape", newline="") as f:
    f.write("\n".join(kept))
' "$1" "$2" "$3" "$4" "$CLI_VERSION"
}

# drop_via's bites, run every time (bash or binary): each log below must be
# refused, and a well-formed one of each kind must pass as the golden's.
drop_via_bites() {
    local d="$SANDBOX/bites" k5ua="> User-Agent: k5/$CLI_VERSION"
    mkdir -p "$d"
    local head="> GET /cli/sessions/roster?token=t
> Host: 127.0.0.1:1"
    printf '%s\n%s\n> Accept: */*\n> X-K5-Cli-Version: %s\n> X-K5-Cli-Via: native\n> body: \n' "$head" "$k5ua" "$CLI_VERSION" > "$d/good_k5"
    printf '%s\n> User-Agent: curl/8.5.0\n> Accept: */*\n> X-K5-Cli-Via: fallthrough\n> X-K5-Cli-Noun: sessions\n> body: \n' "$head" > "$d/good_bash"
    printf '%s\n> User-Agent: curl/<VERSION>\n> Accept: */*\n> body: \n' "$head" > "$d/want_k5"
    printf '%s\n> User-Agent: curl/8.5.0\n> Accept: */*\n> body: \n' "$head" > "$d/want_bash"
    # (1) a binary request with two Via lines
    printf '%s\n%s\n> Accept: */*\n> X-K5-Cli-Version: %s\n> X-K5-Cli-Via: native\n> X-K5-Cli-Via: native\n> body: \n' "$head" "$k5ua" "$CLI_VERSION" > "$d/bite_two_vias"
    # (2) a curl request marked as the binary's own
    printf '%s\n> User-Agent: curl/8.5.0\n> Accept: */*\n> X-K5-Cli-Via: native\n> X-K5-Cli-Noun: sessions\n> body: \n' "$head" > "$d/bite_curl_native"
    # (3) a binary request carrying a Noun line
    printf '%s\n%s\n> Accept: */*\n> X-K5-Cli-Version: %s\n> X-K5-Cli-Via: native\n> X-K5-Cli-Noun: sessions\n> body: \n' "$head" "$k5ua" "$CLI_VERSION" > "$d/bite_k5_noun"
    # (4) a binary request with no X-K5-Cli-Version, (5) two of them
    printf '%s\n%s\n> Accept: */*\n> X-K5-Cli-Via: native\n> body: \n' "$head" "$k5ua" > "$d/bite_k5_no_version"
    printf '%s\n%s\n> Accept: */*\n> X-K5-Cli-Version: %s\n> X-K5-Cli-Version: %s\n> X-K5-Cli-Via: native\n> body: \n' "$head" "$k5ua" "$CLI_VERSION" "$CLI_VERSION" > "$d/bite_k5_two_versions"
    # (6) a binary request whose version is not the CLI's
    printf '%s\n> User-Agent: k5/0.0.0-other\n> Accept: */*\n> X-K5-Cli-Version: 0.0.0-other\n> X-K5-Cli-Via: native\n> body: \n' "$head" > "$d/bite_k5_other_version"
    # (7) a curl request carrying the binary's X-K5-Cli-Version
    printf '%s\n> User-Agent: curl/8.5.0\n> Accept: */*\n> X-K5-Cli-Version: %s\n> X-K5-Cli-Via: fallthrough\n> X-K5-Cli-Noun: sessions\n> body: \n' "$head" "$CLI_VERSION" > "$d/bite_curl_version"
    local k
    for k in k5 bash; do
        if ! drop_via "bite good_$k" "$d/good_$k" "$d/got_$k" sessions || ! cmp -s "$d/got_$k" "$d/want_$k"; then
            echo "FAIL: drop_via refused or changed a well-formed $k request (bites)" >&2
            exit 1
        fi
    done
    for k in two_vias curl_native k5_noun k5_no_version k5_two_versions k5_other_version curl_version; do
        if drop_via "bite $k" "$d/bite_$k" "$d/got" sessions 2>"$d/err"; then
            echo "FAIL: drop_via took the bite $k; it must refuse it" >&2
            exit 1
        fi
        grep -q "FAIL: bite $k:" "$d/err" || { echo "FAIL: drop_via refused the bite $k without naming it" >&2; cat "$d/err" >&2; exit 1; }
    done
    echo "drop_via: 2 well-formed requests kept, 7 bites refused"
}
drop_via_bites

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
    local reqlog="$LOG"
    if [ "$VIA_MODE" = "1" ]; then
        reqlog="$SANDBOX/requests.via-dropped"
        if ! drop_via "$name" "$LOG" "$reqlog" "${1:-help}"; then
            fail=$((fail + 1))
            return
        fi
    fi
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
        cat "$reqlog"
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
TOPICS="$(grep -m1 '^_STUDY_TOPIC_IDS=' "$K2_SRC" | cut -d'"' -f2)"
[ -n "$TOPICS" ] || { echo "FAIL: no _STUDY_TOPIC_IDS in $K2_SRC" >&2; exit 1; }
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
# k2 sessions (prd-workspace-sessions-cli-v1 WS-T8): open (switch off, and a
# sidecar refused by the main-only rule) prints the daemon's refusal as is,
# no `Ask with:` line while off; the roster read sends the passport.
capture refused_sessions_open          passport -- sessions open 4e9a77c2 --name scroll-notes
capture refused_sessions_open_sidecar  passport -- sessions open 8c1f2a9e
capture sessions_roster_passport       passport -- sessions
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
