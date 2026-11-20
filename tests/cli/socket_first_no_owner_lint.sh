#!/usr/bin/env bash
# Lint: a socket-first sender's TCP fallback never carries the owner token
# from disk in a session (0.46.17, BEGIN_CLI_SOCKET_FIRST_AUTH in
# cli-tools-policy.sh).
#
# A socket-first sender is a function in cli/src that sends over the
# session's cell socket (a curl with --unix-socket, not the vault's own
# socket, or a call to a *_uds arm) and also over loopback TCP (a curl to
# ${BASE_URL}, 127.0.0.1, http://$HOST or the URL _k2_daemon_base builds,
# or cli_send's "$fn" call). The
# TCP-only senders are not read here.
#
# Reads cli/src function by function and fails, naming file, line and
# function, when a socket-first sender:
#   1. puts $TOKEN in a credential itself (_k2_cfg_auth "$TOKEN", a
#      "Bearer $TOKEN" header, or a variable set from $TOKEN);
#   2. never calls _k2_sock_fallback_token, unless every credential it
#      sends is the passport ($SCOPED_TOKEN);
#   3. is on the reviewed list below but no longer flagged (fixed or
#      gone): take it off the list.
# And it fails when _k2_sock_fallback_token no longer goes through
# _k2_session_tcp_token, or _k2_session_tcp_token no longer swaps in the
# passport, or scripts/k2-open's owner arm no longer sends the session's
# passport in place of the owner token in a session.
#
# Reviewed (file:function, and why its TCP request never carries the
# owner token from disk in a session):
#   connect.sh:_connect_curl_daemon
#       tries the socket only when $TOKEN is the passport, so its TCP
#       fallback sends the passport
#   permission.sh:_permission_probe
#       tries the socket only when its credential is the passport; with
#       the owner's credential it goes over TCP alone
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/socket_first_no_owner_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

lint() { # lint <dir> [reviewed file:function list, comma separated] [whole]
    python3 - "$1" "${2:-}" "${3:-}" <<'PYEOF'
import glob, os, re, sys

src, reviewed, whole = sys.argv[1], [r for r in sys.argv[2].split(",") if r], sys.argv[3] == "whole"
FN = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\(\)\s*\{(.*)$")
HEREDOC = re.compile(r"<<(-?)\s*['\"]?([A-Za-z_][A-Za-z0-9_]*)['\"]?")
CURL = re.compile(r"(^|[\s;|&(`$\"])(_k2_curl|_cli_curl|curl)\s")
SOCK = re.compile(r"--unix-socket\s")
UDS_CALL = re.compile(r"(\"\$\{fn\}_uds\"|\b(cli_request_uds|cli_post_form_uds|cli_post_json_uds|cli_request_post_uds|cli_post_uds)\b)")
TCP = re.compile(r"\$\{BASE_URL\}|\$BASE_URL\b|127\.0\.0\.1|http://\$\{?HOST\}?|_k2_daemon_base\b")
FN_CALL = re.compile(r"^\"\$fn\"\s+\"\$endpoint\"")
TOK = r"\$(TOKEN\b|\{TOKEN(:-)?\})"
RAW = re.compile(r"_k2_cfg_auth\s+\"" + TOK + r"\"|Bearer\s+" + TOK)
ASSIGN = re.compile(r"(^|[\s;])([A-Za-z_][A-Za-z0-9_]*)=\"" + TOK + r"\"")
# The helper's own default and the selector's saved $TOKEN are not credentials.
ASSIGN_OK = {"_K2_TCP_TOKEN", "_K2_TOKEN_SAVED"}
CRED = re.compile(r"_k2_cfg_auth\s+\"([^\"]*)\"|Bearer\s+(\$\{?[A-Za-z_][A-Za-z0-9_]*\}?)")
PASSPORT = re.compile(r"^\$\{?SCOPED_TOKEN\}?$")
PY_C = re.compile(r"\bpython3\b[^']*\s-c\s+'")

files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)

# (file, name) -> {"name", "file", "line", "body": [(line no, logical line, is_python)]}
funcs = {}
for f in files:
    name = os.path.basename(f)
    raw = open(f, encoding="utf-8", errors="replace").read().split("\n")
    cur, buf, start, heredoc, py_str = None, "", None, None, False
    for i, line in enumerate(raw):
        s = line.strip()
        if py_str:
            if line.count("'") % 2 == 1:
                py_str = False
            continue
        if heredoc is not None:
            delim, strip_tabs = heredoc
            if (line.lstrip("\t") if strip_tabs else line) == delim:
                heredoc = None
            continue
        m = FN.match(line)
        if m and cur is None:
            cur = {"name": m.group(1), "file": name, "line": i + 1, "body": []}
            rest = m.group(2).strip()
            if rest.endswith("}"):
                cur["body"].append((i + 1, rest[:-1].strip()))
                funcs[(name, cur["name"])] = cur
                cur = None
            continue
        if cur is None:
            continue
        if re.match(r"^\}\s*(#.*)?$", line) and not buf:
            funcs[(name, cur["name"])] = cur
            cur = None
            continue
        if s.startswith("#") and not buf:
            continue
        if start is None:
            start = i + 1
        if s.endswith("\\"):
            buf += s[:-1] + " "
            continue
        logical = buf + s
        buf = ""
        cur["body"].append((start, logical))
        start = None
        pm = PY_C.search(logical)
        if pm and logical[pm.end():].count("'") % 2 == 0:
            py_str = True
        hm = HEREDOC.search(logical)
        if hm:
            heredoc = (hm.group(2), hm.group(1) == "-")

problems, hits, seen = [], set(), 0
for (fname, n), fn in sorted(funcs.items()):
    body = fn["body"]
    sock = any((CURL.search(l) and SOCK.search(l) and "_vault_sock" not in l) or UDS_CALL.search(l) for _, l in body)
    tcp = any((CURL.search(l) or "_k2_cfg_add url" in l) and TCP.search(l) for _, l in body) \
        or any(FN_CALL.search(l) for _, l in body)
    if not (sock and tcp):
        continue
    seen += 1
    where = "%s:%d: %s" % (fname, fn["line"], n)
    key = "%s:%s" % (fname, n)
    flagged = False
    for no, l in body:
        if RAW.search(l) or any(m.group(2) not in ASSIGN_OK for m in ASSIGN.finditer(l)):
            flagged = True
            if key not in reviewed:
                problems.append("%s: line %d puts $TOKEN in a credential itself; a socket-first sender's TCP "
                                "request takes it from _k2_sock_fallback_token ($_K2_TCP_TOKEN)" % (where, no))
    helper = any("_k2_sock_fallback_token" in l for _, l in body)
    creds = [c for _, l in body for m in CRED.finditer(l) for c in [m.group(1) or m.group(2)]]
    passport_only = bool(creds) and all(PASSPORT.match(c) for c in creds) and not any(FN_CALL.search(l) for _, l in body)
    if not helper and not passport_only:
        flagged = True
        if key not in reviewed:
            problems.append("%s: sends over the cell socket and then TCP, and never calls _k2_sock_fallback_token" % where)
    if flagged:
        hits.add(key)

for r in reviewed:
    if r not in hits:
        problems.append("%s: on the reviewed list but not flagged (fixed or gone); take it off the list" % r)

if whole:
    if seen < 6:
        problems.append("only %d socket-first senders found; the scan no longer sees them" % seen)
    text = "\n".join(open(f, encoding="utf-8", errors="replace").read() for f in files)
    m = re.search(r"^_k2_sock_fallback_token\(\) \{\n(.*?)^\}", text, re.S | re.M)
    if not m or "_k2_session_tcp_token" not in m.group(1):
        problems.append("_k2_sock_fallback_token no longer goes through _k2_session_tcp_token")
    m = re.search(r"^_k2_session_tcp_token\(\) \{\n(.*?)^\}", text, re.S | re.M)
    if not m or '_K2_TCP_TOKEN="$SCOPED_TOKEN"' not in m.group(1) or "_k2_end_command 1" not in m.group(1):
        problems.append("_k2_session_tcp_token no longer swaps in the passport, or no longer ends the command with none")
    shim = open(os.environ["K2_OPEN_SHIM"], encoding="utf-8").read()
    if '_tok="$_hook_tok"' not in shim:
        problems.append("scripts/k2-open's owner arm no longer sends the session's passport in a session")

for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-sock-first-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good senders.sh <<'EOF'
_hook_sock() { local s; _envn_into s HOOK_SOCK; echo "$s"; }
# The socket, then TCP with the helper's credential.
send_ok() {
    local method="$1" path="$2" sock out="" _K2_CFG=""
    sock="$(_hook_sock)"
    if [ -S "$sock" ]; then
        out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}")" || true
    fi
    local _K2_TCP_TOKEN="${TOKEN:-}"
    _k2_sock_fallback_token "$sock" "$out"
    _k2_cfg_add url "${BASE_URL}${path}"
    _k2_cfg_auth "$_K2_TCP_TOKEN"
    _k2_curl "$_K2_CFG" -s -X POST
}
# The selector: the TCP arm runs with the helper's credential.
select_ok() {
    local fn="$1" endpoint="$2" sock out=""; shift 2
    sock="$(_hook_sock)"
    out="$("${fn}_uds" "$endpoint" "$@")" || true
    local _K2_TCP_TOKEN="${TOKEN:-}" _K2_TOKEN_SAVED="${TOKEN:-}"
    _k2_sock_fallback_token "$sock" "$out"
    TOKEN="$_K2_TCP_TOKEN"
    "$fn" "$endpoint" "$@"
    TOKEN="$_K2_TOKEN_SAVED"
}
# The passport on both channels.
passport_only() {
    local sock _K2_CFG=""
    sock="$(_hook_sock)"
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost/hook/event" || \
        _k2_curl "$_K2_CFG" -s -X POST "http://127.0.0.1:${PORT}/hook/event"
}
# TCP only: not read here.
tcp_only() {
    local _K2_CFG=""
    _k2_cfg_auth "$TOKEN"
    _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/x"
}
# The vault's own socket: not the cell socket.
vault_then_tcp() {
    _k2_curl "" -s -X POST --unix-socket "$(_vault_sock)" "http://localhost/cli/vault/x"
    _k2_cfg_auth "$TOKEN"
    _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/x"
}
EOF
mk raw senders.sh <<'EOF'
send_raw() {
    local sock _K2_CFG=""
    sock="$(_hook_sock)"
    _k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost/cli/x" || true
    _k2_sock_fallback_token "$sock" ""
    _k2_cfg_auth "$TOKEN"
    _k2_curl "$_K2_CFG" -s -G "${BASE_URL}/cli/x"
}
EOF
mk no_helper senders.sh <<'EOF'
send_no_helper() {
    local sock _K2_CFG=""
    sock="$(_hook_sock)"
    _k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost/cli/x" || true
    _k2_cfg_auth "$_K2_TCP_TOKEN"
    _k2_curl "$_K2_CFG" -s -G "${BASE_URL}/cli/x"
}
EOF
mk selector senders.sh <<'EOF'
select_bare() {
    local fn="$1" endpoint="$2"; shift 2
    "${fn}_uds" "$endpoint" "$@" || true
    "$fn" "$endpoint" "$@"
}
EOF
mk via_var senders.sh <<'EOF'
send_var() {
    local tok="$TOKEN" _K2_CFG=""
    cli_post_form_uds /cli/x || true
    _k2_sock_fallback_token "$(_hook_sock)" ""
    _k2_cfg_add header "Authorization: Bearer $tok"
    _k2_curl "$_K2_CFG" -s -X POST "http://127.0.0.1:${PORT}/cli/x"
}
EOF
self_ok=1
if ! out="$(lint "$FIX/good")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
for b in raw no_helper selector via_var; do
    if out="$(lint "$FIX/$b")"; then echo "SELF-TEST FAIL: $b not caught" >&2; self_ok=0; fi
done
# A reviewed entry passes while flagged, and fails once it is not.
if ! out="$(lint "$FIX/raw" "senders.sh:send_raw")"; then echo "SELF-TEST FAIL: a reviewed entry: $out" >&2; self_ok=0; fi
if out="$(lint "$FIX/good" "senders.sh:send_ok")"; then echo "SELF-TEST FAIL: a reviewed entry that is not flagged passed" >&2; self_ok=0; fi
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good ones pass"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
REVIEWED="connect.sh:_connect_curl_daemon,permission.sh:_permission_probe"
if ! out="$(K2_OPEN_SHIM="$PROJECT_ROOT/scripts/k2-open" lint "$DIR" "$REVIEWED" whole)"; then
    echo "$out" >&2
    echo "socket_first_no_owner_lint: FAILED" >&2
    exit 1
fi
echo "socket_first_no_owner_lint: every socket-first sender in cli/src takes its TCP credential from _k2_sock_fallback_token (the reviewed exceptions excepted), and scripts/k2-open sends the passport in a session"
