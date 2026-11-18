#!/usr/bin/env bash
# Lint: a POST that tried the cell socket goes on to loopback TCP only
# under the 0.46.15 send-once rule (curl 7 or "not served on cell socket";
# any other failure ends in _cli_send_unanswered).
#
# Reads cli/src (the parts cli/k2 is built from), function by function,
# and fails, naming file, line and function, when:
#   1. a function sends over the cell socket and over loopback TCP, can
#      send a non-GET on the socket, and has no guard: no call to
#      _cli_send_unanswered, or no test of curl's exit 7;
#   2. a Python block connects to a Unix socket in a function that reads
#      the cell socket (K2_HOOK_SOCK / _hook_sock): such a client is not
#      checked here, so it needs the rule and a row in this lint first;
#   3. a function named on the known-open list below no longer is one
#      (fixed or gone): take it off the list.
#
# What counts:
#   cell socket send  a curl line with --unix-socket in a function that
#                     reads the cell socket (the vault's own socket is
#                     another channel), a call to a function that only
#                     does such sends (cli_post_form_uds and the other
#                     arms), or cli_send's "${fn}_uds" (any method).
#   TCP send          a curl line to BASE_URL, 127.0.0.1 or http://$HOST
#                     (on the line or in its `url =` config line), a call
#                     to a function that only does such sends, or
#                     cli_send's "$fn" call.
#   non-GET           -X POST / PUT / PATCH / DELETE or -X "$var" on a
#                     socket send, a socket-only function that does so, or
#                     the "${fn}_uds" call.
#   either-or         after each socket send an unconditional return comes
#                     before any TCP send (k2 respond: the socket, else
#                     TCP). Such a function never sends twice.
# Calls are followed one level (a sender split over two helpers that each
# call a third would be missed); every sender in cli/src today is one
# function. Comments and help text are skipped.
#
# Known open (reported, not failing; each needs K2's sign-off to change):
#   _connect_curl_daemon (connect.sh): K2 Connect requests proxied by the
#     daemon (POST /cli/connect/proxy). Any socket failure goes on to TCP.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/post_no_resend_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

lint() { # lint <dir> [known-open names, comma separated]
    python3 - "$1" "${2:-}" <<'PYEOF'
import glob, os, re, sys

src, known = sys.argv[1], [k for k in sys.argv[2].split(",") if k]
FN = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\(\)\s*\{(.*)$")
HEREDOC = re.compile(r"<<(-?)\s*['\"]?([A-Za-z_][A-Za-z0-9_]*)['\"]?")
CURL = re.compile(r"(^|[\s;|&(`$\"])(_k2_curl|_cli_curl|curl)\s")
TCP_URL = re.compile(r"BASE_URL|127\.0\.0\.1|http://\$\{?HOST\b")
NON_GET = re.compile(r"-X\s+(POST|PUT|PATCH|DELETE|\"?\$)")
DYN_UDS = re.compile(r"\"\$\{[A-Za-z_]+\}_uds\"")
DYN_TCP = re.compile(r"^\"\$[A-Za-z_]+\"\s")
OPEN = re.compile(r"(^|[;&|]\s*|\bthen\s+|\bdo\s+|\belse\s+)(if|case|while|for|until)\b")
CLOSE = re.compile(r"(^|[;&|]\s*)(fi|esac|done)\b")
PY_C = re.compile(r"\bpython3\b[^']*\s-c\s+'")

files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)

# name -> {"file", "line", "body": [(line no, logical line, is_python)]}
funcs = {}
for f in files:
    name = os.path.basename(f)
    raw = open(f, encoding="utf-8", errors="replace").read().split("\n")
    cur, buf, heredoc, py_str = None, "", None, False
    for i, line in enumerate(raw):
        s = line.strip()
        # Inside python3 -c '...': Python, until the closing quote.
        if py_str:
            if line.count("'") % 2 == 1:
                py_str = False
            if cur is not None:
                cur["body"].append((i + 1, s, True))
            continue
        if heredoc is not None:
            delim, strip_tabs, is_py = heredoc
            if (line.lstrip("\t") if strip_tabs else line) == delim:
                heredoc = None
            elif cur is not None and is_py:
                cur["body"].append((i + 1, s, True))
            continue
        m = FN.match(line)
        if m and cur is None:
            cur = {"name": m.group(1), "file": name, "line": i + 1, "body": []}
            rest = m.group(2).strip()
            if rest.endswith("}"):  # a one-line function
                cur["body"].append((i + 1, rest[:-1].strip(), False))
                funcs[cur["name"]] = cur
                cur = None
            continue
        if cur is None:
            continue
        if line.startswith("}"):
            funcs[cur["name"]] = cur
            cur, buf = None, ""
            continue
        if s.startswith("#"):
            continue
        pm = PY_C.search(line)
        if pm and line[pm.end():].count("'") % 2 == 0:
            py_str = True
        hm = HEREDOC.search(line)
        if hm:
            heredoc = (hm.group(2), hm.group(1) == "-", bool(re.search(r"\bpython3\b", line)))
        if s.endswith("\\"):
            buf += s[:-1] + " "
            continue
        cur["body"].append((i + 1, buf + s, False))
        buf = ""

def text(fn):
    return "\n".join(l for _, l, _ in fn["body"])

def own_sends(fn):
    t = text(fn)
    hook = "_hook_sock" in t or "HOOK_SOCK" in t
    cfg_url = bool(re.search(r"_k2_cfg_add url .*(" + TCP_URL.pattern + ")", t))
    sock, tcp = [], []
    for no, l, py in fn["body"]:
        if py or not CURL.search(l) or "command -v" in l:
            continue
        if "--unix-socket" in l:
            if hook and "_vault_sock" not in l:
                sock.append((no, l, bool(NON_GET.search(l))))
        elif TCP_URL.search(l) or (cfg_url and "http://localhost" not in l):
            tcp.append((no, l))
    return sock, tcp

own = {n: own_sends(fn) for n, fn in funcs.items()}
SOCK_ONLY = {n for n, (s, t) in own.items() if s and not t}
TCP_ONLY = {n for n, (s, t) in own.items() if t and not s}

def call_re(n):
    return re.compile(r"(^|[\s;|&(`$\"])" + re.escape(n) + r"(\s|$|\"|\)|;)")
CALL_RES = {n: call_re(n) for n in SOCK_ONLY | TCP_ONLY}

def depth_delta(l):
    return len(OPEN.findall(l)) - len(CLOSE.findall(l))

problems, notes = [], []
for n, fn in sorted(funcs.items()):
    sock, tcp = own[n]
    # Lines as (no, kind, non_get) in order: kind "s" (socket), "t" (TCP).
    events = {}
    for no, l, ng in sock:
        events[no] = ("s", ng)
    for no, l in tcp:
        events[no] = ("t", False)
    for no, l, py in fn["body"]:
        if py or no in events:
            continue
        if DYN_UDS.search(l):
            events[no] = ("s", True)
            continue
        if DYN_TCP.search(l):
            events[no] = ("t", False)
            continue
        for c in SOCK_ONLY:
            if c != n and CALL_RES[c].search(l):
                events[no] = ("s", any(ng for _, _, ng in own[c][0]))
                break
        else:
            for c in TCP_ONLY:
                if c != n and CALL_RES[c].search(l):
                    events[no] = ("t", False)
                    break
    kinds = {k for k, _ in events.values()}
    if kinds != {"s", "t"}:
        continue
    if not any(ng for k, ng in events.values() if k == "s"):
        continue  # GETs keep the fallback
    # Either-or: after every socket send, an unconditional return (at or
    # above the socket line's depth, and above every block opened since)
    # comes before the next TCP send.
    body = [(no, l) for no, l, py in fn["body"] if not py]
    either_or = True
    for idx, (no, l) in enumerate(body):
        if events.get(no, ("", False))[0] != "s":
            continue
        depth, low, ok = 0, 0, False
        for no2, l2 in body[idx + 1:]:
            if events.get(no2, ("", False))[0] == "t":
                break
            if re.match(r"(return|exit)\b", l2) and depth <= low:
                ok = True
                break
            depth += depth_delta(l2)
            low = min(low, depth)
        if not ok:
            either_or = False
            break
    if either_or:
        continue
    t = text(fn)
    guarded = "_cli_send_unanswered" in t and re.search(r"-ne 7\b|-eq 7\b|(^|\s)7\|", t)
    where = "%s:%d: %s" % (fn["file"], fn["line"], n)
    if n in known:
        if guarded:
            problems.append("%s: on the known-open list but guarded now; take it off the list" % where)
        else:
            notes.append("%s: known open (cell socket, then TCP, for a non-GET; needs K2's sign-off)" % where)
        continue
    if not guarded:
        problems.append("%s: sends a non-GET over the cell socket and then TCP without the send-once guard "
                        "(_cli_send_unanswered unless curl 7 or \"not served on cell socket\")" % where)

for n, fn in sorted(funcs.items()):
    t = text(fn)
    if ("HOOK_SOCK" in t or "_hook_sock" in t) and any(py and "AF_UNIX" in l for _, l, py in fn["body"]):
        problems.append("%s:%d: %s: a Python client on the cell socket; give it the send-once rule and a row here"
                        % (fn["file"], fn["line"], n))
for k in known:
    if k not in funcs:
        problems.append("%s: on the known-open list but not in %s; take it off the list" % (k, src))

for x in notes:
    print("note: " + x)
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-norest-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good senders.sh <<'EOF'
_hook_sock() { local s; _envn_into s HOOK_SOCK; echo "$s"; }
# Guarded, as _compute_send.
send_guarded() {
    local method="$1" path="$2" sock out="" rc=0 t0="$SECONDS"
    sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ]; then
        if [ "$method" = "GET" ]; then
            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                -w $'\n%{http_code}')" || rc=$?
        else
            out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                -w $'\n%{http_code}')" || rc=$?
        fi
        if [ "$rc" -eq 0 ] && [ -n "$out" ]; then
            printf '%s' "$out"
            return 0
        fi
        if [ "$method" != "GET" ] && [ "$rc" -ne 7 ] && [[ "$out" != *"not served on cell socket"* ]]; then
            _cli_send_unanswered "$path" "$t0" 0 $'\n000'
            return
        fi
    fi
    _k2_cfg_add url "${BASE_URL}${path}"
    _k2_curl "$_K2_CFG" -s -X POST -w $'\n%{http_code}'
}
# A GET only: it keeps the fallback.
get_only() {
    local sock out=""
    sock="$(_hook_sock)"
    if [ -S "$sock" ]; then
        out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost/cli/whoami")" || out=""
    fi
    if [ -z "$out" ]; then
        out="$(_k2_curl "$_K2_CFG" -s -G "${BASE_URL}/cli/whoami")" || out=""
    fi
}
# Socket-only arm and an either-or caller (k2 respond).
post_form_uds() {
    _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost$1"
}
either_or() {
    local sock; sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ]; then
        if [ -n "$2" ]; then
            post_form_uds /cli/respond "$2"
        else
            post_form_uds /cli/respond
        fi
        return $?
    fi
    _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/respond"
    return $?
}
# The vault's own socket is another channel.
vault_then_tcp() {
    out="$(_k2_curl "" -s -X POST --unix-socket "$(_vault_sock)" "http://localhost/cli/vault/x")" || out=""
    [ -n "$out" ] || _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/vault/x"
}
# A dispatcher with the guard, as cli_send.
dispatch() {
    local fn="$1" endpoint="$2" rc=0 out t0="$SECONDS"; shift 2
    out="$("${fn}_uds" "$endpoint" "$@")" || rc=$?
    case "$rc" in
        7|75) ;;
        *) _cli_send_unanswered "$endpoint" "$t0" "$rc" "$out"; return ;;
    esac
    "$fn" "$endpoint" "$@"
}
EOF
mk post_fallback senders.sh <<'EOF'
_hook_sock() { local s; _envn_into s HOOK_SOCK; echo "$s"; }
send_twice() {
    local path="$1" sock out=""
    sock="$(_hook_sock)"
    if [ -S "$sock" ]; then
        out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
            -w $'\n%{http_code}')" || out=""
        if [ -n "$out" ]; then
            printf '%s' "$out"
            return 0
        fi
    fi
    _k2_cfg_add url "${BASE_URL}${path}"
    _k2_curl "$_K2_CFG" -s -X POST -w $'\n%{http_code}'
}
EOF
mk via_arm senders.sh <<'EOF'
_hook_sock() { local s; _envn_into s HOOK_SOCK; echo "$s"; }
post_form_uds() {
    _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost$1"
}
post_form() {
    _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}$1"
}
send_twice() {
    local out=""
    out="$(post_form_uds /cli/x)" || out=""
    [ -n "$out" ] || post_form /cli/x
}
EOF
mk dynamic senders.sh <<'EOF'
dispatch() {
    local fn="$1" endpoint="$2" rc=0 out; shift 2
    out="$("${fn}_uds" "$endpoint" "$@")" || rc=$?
    if [ "$rc" -eq 0 ]; then printf '%s' "$out"; return 0; fi
    "$fn" "$endpoint" "$@"
}
EOF
mk no_seven senders.sh <<'EOF'
_hook_sock() { local s; _envn_into s HOOK_SOCK; echo "$s"; }
send_half() {
    local path="$1" sock out="" rc=0 t0="$SECONDS"
    sock="$(_hook_sock)"
    if [ -S "$sock" ]; then
        out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}")" || rc=$?
        if [ "$rc" -ne 0 ] && [[ "$out" != *"not served on cell socket"* ]]; then
            _cli_send_unanswered "$path" "$t0" 0 ""
        fi
    fi
    _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}${path}"
}
EOF
mk python senders.sh <<'EOF'
py_send() {
    K2_S="$(_hook_sock)" python3 - <<'PY'
import os, socket
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(os.environ["K2_S"])
PY
}
EOF
self_ok=1
if ! out="$(lint "$FIX/good")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
for b in post_fallback via_arm dynamic no_seven python; do
    if out="$(lint "$FIX/$b")"; then echo "SELF-TEST FAIL: $b not caught" >&2; self_ok=0; fi
done
# A known-open entry that is guarded, or not there, fails.
if out="$(lint "$FIX/good" send_guarded)"; then echo "SELF-TEST FAIL: a guarded known-open entry not caught" >&2; self_ok=0; fi
if out="$(lint "$FIX/good" no_such_fn)"; then echo "SELF-TEST FAIL: a missing known-open entry not caught" >&2; self_ok=0; fi
# A known-open entry that is still open passes, with its note.
if ! out="$(lint "$FIX/post_fallback" send_twice)" || [[ "$out" != *"note: senders.sh:2: send_twice: known open"* ]]; then
    echo "SELF-TEST FAIL: a known-open entry: $out" >&2; self_ok=0
fi
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good ones pass"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
KNOWN_OPEN="_connect_curl_daemon"
if out="$(lint "$DIR" "$KNOWN_OPEN")"; then
    [ -z "$out" ] || echo "$out"
    echo "post_no_resend_lint: every other cell-socket-then-TCP sender in cli/src keeps the send-once rule for non-GETs"
else
    echo "$out" >&2
    echo "post_no_resend_lint: FAILED" >&2
    exit 1
fi
