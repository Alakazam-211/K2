#!/usr/bin/env bash
# Every urlopen in cli/src goes through the hand-over helper, so the
# embedded Python's requests carry what curl's carry (X-K5-Cli-Via,
# X-K5-Cli-Noun, X-K5-Would-Refuse, X-K5-Owner-Source; see _k5_headers in
# cli/src/cli-tools-policy.sh). The helper is the text in _K5_PY_HTTP.
# This gate fails, naming the file and line, when a urlopen call:
#   1. does not wrap its request in k5_request(): urlopen(k5_request(...));
#   2. sits in a python3 block whose command line does not pass
#      _K5_PY_HTTP="$_K5_PY_HTTP" and either
#      _K5_PY_HEADERS="$(_k5_py_headers)" (the daemon) or
#      _K5_PY_HEADERS="" (not the daemon);
#   3. sits in a block that does not run the helper first:
#      exec(os.environ.pop("_K5_PY_HTTP")) between python3 and the call.
# It also fails on build_opener (a request that would skip urlopen), except
# a build_opener handed straight to install_opener on the same line: that
# sets the opener urlopen itself uses (the proxy rule, _K2_PY_PROXY_RULE),
# so the request still goes through urlopen(k5_request(...)). And it fails
# when the helper text does not compile or does not define k5_request.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/k5_urlopen_gate.sh
# bash-only: reads cli/src as text; there is no program under test.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }

# scan <dir>: one "<file>:<line>: <problem>" per problem, for <dir>/*.sh.
# Prints "checked <n>" last, the urlopen calls it looked at.
scan() {
    python3 - "$1" <<'PY'
import glob, os, re, sys
src = sys.argv[1]
START = re.compile(r"\bpython3\b.*(<<|-c\b)")
EXEC = re.compile(r"""^exec\(os\.environ\.pop\((["'])_K5_PY_HTTP\1\)\)$""")
HTTP = '_K5_PY_HTTP="$_K5_PY_HTTP"'
HEADERS = ('_K5_PY_HEADERS="$(_k5_py_headers)"', '_K5_PY_HEADERS=""')
checked = 0
for path in sorted(glob.glob(os.path.join(src, "*.sh"))):
    name = os.path.basename(path)
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    for i, line in enumerate(lines):
        if line.lstrip().startswith("#"):
            continue
        at = "%s:%d" % (name, i + 1)
        if "build_opener(" in line and not re.search(r"install_opener\([\w.]*build_opener\(", line):
            print("%s: build_opener sends requests that skip k5_request; use urlopen(k5_request(...))" % at)
        if "urlopen(" not in line:
            continue
        checked += 1
        if not re.search(r"urlopen\(k5_request\(", line):
            print("%s: urlopen without k5_request(): wrap the request, urlopen(k5_request(req), ...)" % at)
        j = i
        while j >= 0 and not START.search(lines[j]):
            j -= 1
        if j < 0:
            print("%s: urlopen with no python3 block above it" % at)
            continue
        k = j
        while k > 0 and lines[k - 1].rstrip().endswith("\\"):
            k -= 1
        cmd = " ".join(lines[k:j + 1])
        if HTTP not in cmd or not any(h in cmd for h in HEADERS):
            print("%s: its python3 (line %d) does not pass %s and %s or %s" % (at, j + 1, HTTP, HEADERS[0], HEADERS[1]))
        if not any(EXEC.match(l.strip()) for l in lines[j + 1:i]):
            print('%s: its python3 block (line %d) does not run exec(os.environ.pop("_K5_PY_HTTP")) before the call' % (at, j + 1))
print("checked %d" % checked)
PY
}

# The helper text itself: it compiles, defines k5_request, adds exactly
# the lines it was given and leaves neither variable in the environment.
helper_ok() {
    local text
    text="$(sed -n "/^_K5_PY_HTTP='\$/,/^'\$/p" "$1")"
    [ -n "$text" ] || { echo "no _K5_PY_HTTP='...' block in $1"; return 1; }
    eval "$text"
    _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS=$'X-K5-Cli-Via: fallthrough\nX-K5-Cli-Noun: preset\n' python3 -I -c '
import os, sys
exec(os.environ.pop("_K5_PY_HTTP"))
r = k5_request("http://127.0.0.1:9/x")
got = sorted(r.header_items())
want = [("X-k5-cli-noun", "preset"), ("X-k5-cli-via", "fallthrough")]
left = [n for n in ("_K5_PY_HTTP", "_K5_PY_HEADERS") if n in os.environ]
if got != want or left:
    sys.exit("headers %r (wanted %r), still in the environment: %r" % (got, want, left))
'
}

fail=0
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

echo "== the gate catches broken parts =="
FX="$(mktemp -d)"
trap 'rm -rf "$FX"' EXIT
D='_K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="$(_k5_py_headers)"'
N='_K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS=""'
X='exec(os.environ.pop("_K5_PY_HTTP"))'
fixture() { # fixture <name> <prefix> <exec line> <call line>
    printf 'f() {\n    A=1 \\\n    %s python3 - <<'\''PY'\''\nimport os, urllib.request\n%s\n# urlopen(req) in a comment does not count\n%s\nPY\n}\n' "$2" "$3" "$4" >"$FX/$1.sh"
}
fixture good_daemon "$D" "$X" 'urllib.request.urlopen(k5_request(req), timeout=5)'
fixture good_not_daemon "$N" "$X" 'urllib.request.urlopen(k5_request("http://x/"), timeout=5)'
fixture bad_no_wrap "$D" "$X" 'urllib.request.urlopen(req, timeout=5)'
fixture bad_no_prefix "A=2" "$X" 'urllib.request.urlopen(k5_request(req), timeout=5)'
fixture bad_no_headers '_K5_PY_HTTP="$_K5_PY_HTTP"' "$X" 'urllib.request.urlopen(k5_request(req), timeout=5)'
fixture bad_no_exec "$D" "pass" 'urllib.request.urlopen(k5_request(req), timeout=5)'
fixture bad_opener "$D" "$X" 'urllib.request.build_opener().open(req)'
fixture good_installed_opener "$D" "$X"$'\n''urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler()))' 'urllib.request.urlopen(k5_request(req), timeout=5)'
got="$(scan "$FX" | grep -v '^checked ' | cut -d: -f1 | sort -u | tr '\n' ' ')"
want="bad_no_exec.sh bad_no_headers.sh bad_no_prefix.sh bad_no_wrap.sh bad_opener.sh "
if [ "$got" = "$want" ]; then
    echo "  PASS: it names exactly the broken fixtures"
else
    bad "on the fixtures it named '$got', wanted '$want'"
fi

echo "== cli/src =="
out="$(scan "$ROOT/cli/src")"
problems="$(grep -v '^checked ' <<<"$out" || true)"
checked="$(sed -n 's/^checked //p' <<<"$out")"
if [ -n "$problems" ]; then
    while IFS= read -r p; do bad "cli/src/$p"; done <<<"$problems"
elif [ "${checked:-0}" -gt 0 ]; then
    echo "  PASS: all $checked urlopen calls go through k5_request"
else
    bad "found no urlopen call in cli/src (did the scan break?)"
fi
if why="$(helper_ok "$ROOT/cli/src/cli-tools-policy.sh" 2>&1)"; then
    echo "  PASS: the helper text compiles and adds exactly the lines it is given"
else
    bad "the helper text: $why"
fi

echo
if [ "$fail" -eq 0 ]; then echo "k5_urlopen_gate: OK"; else echo "k5_urlopen_gate: $fail failed"; fi
[ "$fail" -eq 0 ]
