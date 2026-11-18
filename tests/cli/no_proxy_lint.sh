#!/usr/bin/env bash
# Lint: the CLI never sends a request to the daemon through a proxy, and
# never puts a credential in a request's query string.
#
# Reads cli/src (the parts cli/k2 is built from) and fails, naming file and
# line, when:
#   1. _k2_curl no longer adds --noproxy '*' for daemon calls;
#   2. a curl runs outside _k2_curl, in a file not on the outside-host list
#      below (a daemon call must go through _k2_curl);
#   3. a Python block calls urlopen without installing an opener with an
#      empty ProxyHandler first, in a file not on the outside-host list;
#   4. a request line builds a token query parameter (?token=, &token=,
#      "token=$…", data-urlencode "token=…", dict(…, token=token),
#      {"token": tok…}, q["token"] = …). The credential rides Authorization: Bearer.
#
# Outside hosts keep the person's proxy settings. They are named here, with
# what they fetch, so a new one is a reviewed change:
#   update.sh          GitHub releases API and the app download
#   daemon-install.sh  the daemon manifest, binary and signature downloads
#   connect-account.sh the K2 Connect account service (sign-in, logout)
#
# It also reads the scripts outside cli/src that call the daemon with curl
# (NAMED below) and fails on a curl command line there without
# --noproxy '*', except the outside-host lines named with them.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/no_proxy_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

lint() { # lint <dir>: prints one line per problem; exit 1 when any
    python3 - "$1" <<'PYEOF'
import glob, os, re, sys

src = sys.argv[1]
OUTSIDE = {"update.sh", "daemon-install.sh", "connect-account.sh"}
TOKEN_Q = [
    re.compile(r"[?&]token="),
    re.compile(r'data-urlencode\s+"token='),
    re.compile(r"[\"']token=\$"),
    re.compile(r"dict\([^)]*\btoken=token\b"),
    re.compile(r'\{\s*"token"\s*:\s*(tok|token)\b'),
    re.compile(r'\[\s*"token"\s*\]\s*='),
]
HEREDOC = re.compile(r"<<(-?)\s*['\"]?([A-Za-z_][A-Za-z0-9_]*)['\"]?")
REQUEST_LINE = re.compile(r"_k2_cfg_add|\bcurl\b|urlopen|Request\(|\burl\s*=|\bqs\s*=|\bparams\s*=|\bq\s*=|\bpath=")
problems = []
files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)

curl_fn_ok = False
for f in files:
    name = os.path.basename(f)
    lines = open(f, encoding="utf-8", errors="replace").read().split("\n")
    in_k2_curl = False
    py_start = None  # line index of the nearest python3 launch
    heredoc = None   # (delimiter, strip_tabs, is_python) while inside one
    for i, line in enumerate(lines):
        s = line.strip()
        if heredoc is not None:
            delim, strip_tabs, is_py = heredoc
            if (line.lstrip("\t") if strip_tabs else line) == delim:
                heredoc = None
                continue
            if not is_py:
                continue  # help text and other data: not run as a request
        else:
            m = HEREDOC.search(line)
            if m and not s.startswith("#"):
                heredoc = (m.group(2), m.group(1) == "-", bool(re.search(r"\bpython3\b", line)))
        if line.startswith("_k2_curl() {"):
            in_k2_curl = True
        elif in_k2_curl and line.startswith("}"):
            in_k2_curl = False
        if in_k2_curl and "set -- --noproxy '*' \"$@\"" in line:
            curl_fn_ok = True
        if re.search(r"\bpython3\b", line):
            py_start = i
        if s.startswith("#"):
            continue
        # 2. a curl outside _k2_curl
        if heredoc is None and not in_k2_curl and name not in OUTSIDE and re.search(r"(^|[\s;|&(`$])curl\s+-", line) \
                and "_k2_curl" not in line and "command -v curl" not in line:
            problems.append("%s:%d: curl outside _k2_curl: %s" % (name, i + 1, s[:100]))
        # 3. urlopen with no proxy-free opener in its block
        if "urlopen(" in line and name not in OUTSIDE:
            block = "\n".join(lines[(py_start if py_start is not None else 0):i])
            if "install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))" not in block:
                problems.append("%s:%d: urlopen without an empty ProxyHandler opener" % (name, i + 1))
        # 4. a token in the query
        if name not in OUTSIDE and REQUEST_LINE.search(line) and any(p.search(line) for p in TOKEN_Q):
            problems.append("%s:%d: a credential in the query string: %s" % (name, i + 1, s[:100]))

if not curl_fn_ok:
    problems.append("_k2_curl does not add --noproxy '*' for daemon calls")
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# Scripts outside cli/src that reach the daemon with curl, as
# "<path>|<outside-host line marker>|...". A marker is text on a curl line
# that fetches an outside host and keeps the proxy settings.
NAMED=(
    "scripts/install-root-helper.sh|fetch() { curl -fsSL"   # GitHub release downloads
    "scripts/k2-open"
    "crates/k2-daemon/src/update_routes.rs"                 # the update swap script
    "crates/k2-core/src/agent_hooks/install.rs"             # notify.sh
)

lint_named() { # lint_named <root> <entry>...: one line per problem; exit 1 when any
    python3 - "$@" <<'PYEOF'
import os, re, sys

root = sys.argv[1]
problems = []
CURL = re.compile(r"(^|[\s;|&(`$\"])curl\s+-")
for entry in sys.argv[2:]:
    rel, *outside = entry.split("|")
    path = os.path.join(root, rel)
    if not os.path.isfile(path):
        problems.append("%s: missing (update NAMED in the lint)" % rel)
        continue
    curls = 0
    for i, line in enumerate(open(path, encoding="utf-8", errors="replace").read().split("\n")):
        s = line.strip()
        if s.startswith("#") or s.startswith("//") or not CURL.search(line):
            continue
        curls += 1
        if any(m in line for m in outside):
            continue
        if "--noproxy '*'" not in line:
            problems.append("%s:%d: curl without --noproxy '*': %s" % (rel, i + 1, s[:100]))
    if curls == 0:
        problems.append("%s: no curl command line found (update NAMED in the lint)" % rel)
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-noproxy-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
f() { _k2_cfg_add url "${BASE_URL}/cli/x"; _k2_cfg_auth "$TOKEN"; }
EOF
mk good py.sh <<'EOF'
g() {
    python3 - <<'PY'
import urllib.request
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))
urllib.request.urlopen(req)
PY
}
EOF
mk good help.sh <<'EOF'
help() {
    cat <<'TXT'
  curl -sS "$K2_GATEWAY_URL/cli/x?token=$TOKEN"
TXT
}
EOF
mk good update.sh <<'EOF'
u() { curl -sH "Accept: x" https://api.example.test/; }
EOF
mk no_noproxy policy.sh <<'EOF'
_k2_curl() {
    curl "$@"
}
EOF
mk bare_curl policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
h() { curl -s "${BASE_URL}/cli/x"; }
EOF
mk py_proxy policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
g() {
    python3 - <<'PY'
import urllib.request
urllib.request.urlopen(req)
PY
}
EOF
mk query_token policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
f() { _k2_cfg_add url "${BASE_URL}/cli/x?token=${TOKEN}"; }
EOF
mk py_query_token policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
g() {
    python3 - <<'PY'
import urllib.request
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))
qs = urllib.parse.urlencode(dict(params, token=token))
PY
}
EOF
self_ok=1
if ! out="$(lint "$FIX/good")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
for b in no_noproxy bare_curl py_proxy query_token py_query_token; do
    if out="$(lint "$FIX/$b")"; then echo "SELF-TEST FAIL: $b not caught" >&2; self_ok=0; fi
done
mkdir -p "$FIX/named/s"
cat >"$FIX/named/s/good.sh" <<'EOF'
# curl -s http://127.0.0.1/ in a comment is not a call
fetch() { curl -fsSL -o "$2" "$1"; }
body=$(curl -fsS --noproxy '*' "http://127.0.0.1:$PORT/boot-status")
EOF
cat >"$FIX/named/s/bad.sh" <<'EOF'
body=$(curl -fsS "http://127.0.0.1:$PORT/boot-status")
EOF
cat >"$FIX/named/s/none.sh" <<'EOF'
echo no request here
EOF
if ! out="$(lint_named "$FIX/named" "s/good.sh|fetch() { curl -fsSL")"; then echo "SELF-TEST FAIL: good named script flagged: $out" >&2; self_ok=0; fi
# good.sh without its outside-host marker: the fetch line must be caught.
for b in "s/bad.sh" "s/none.sh" "s/gone.sh" "s/good.sh"; do
    if out="$(lint_named "$FIX/named" "$b")"; then echo "SELF-TEST FAIL: named $b not caught" >&2; self_ok=0; fi
done
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good one passes"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
if out="$(lint "$DIR")"; then
    echo "no_proxy_lint: cli/src sends no daemon request through a proxy and no credential in a query"
else
    echo "$out" >&2
    echo "no_proxy_lint: FAILED" >&2
    exit 1
fi
if out="$(lint_named "$PROJECT_ROOT" "${NAMED[@]}")"; then
    echo "no_proxy_lint: ${#NAMED[@]} scripts outside cli/src send every daemon curl with --noproxy '*'"
else
    echo "$out" >&2
    echo "no_proxy_lint: FAILED" >&2
    exit 1
fi
