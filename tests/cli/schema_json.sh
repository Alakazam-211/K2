#!/usr/bin/env bash
# `k2 --schema` must print ONE valid JSON document and nothing on stderr.
#
# Regression: the manifest was an unquoted heredoc. Backtick spans in
# descriptions (`k2 agent context catalog`) ran as commands
# ("command substitution: syntax error near unexpected token `newline'"),
# and `\\` collapsed to `\`, which made invalid JSON escapes. Older smoke
# tests ran `--schema 2>/dev/null || true` and grepped substrings, so
# neither failure was seen.
#
# No daemon needed: --schema skips the connection gate. HOME is sandboxed.
# Requires python3. A missing python3 is a FAIL, not a skip.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required to parse the schema" >&2; exit 1; }

WORK="$(mktemp -d -t k2-schema-json-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
export HOME="$WORK/home"
mkdir -p "$HOME"

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

echo "== bash -n =="
if bash -n "$K2_CLI"; then ok "bash -n cli/k2"; else bad "bash -n cli/k2"; fi

for form in "--schema" "schema" "--schema json"; do
    echo "== k2 $form =="
    out="$WORK/out.json"
    err="$WORK/err.txt"
    set +e
    # shellcheck disable=SC2086 # split "--schema json" into two args on purpose
    "$K2_CLI" $form >"$out" 2>"$err"
    rc=$?
    set -e

    if [ "$rc" -eq 0 ]; then ok "exit 0"; else bad "exit $rc (want 0)"; fi

    if [ ! -s "$err" ]; then
        ok "stderr empty"
    else
        bad "stderr not empty:"
        sed 's/^/    /' "$err" >&2
    fi

    # Parse the WHOLE output and check its shape and pinned strings.
    set +e
    py_out="$(python3 - "$out" "$K2_CLI" <<'PY' 2>&1
import json, re, sys

out_path, cli_path = sys.argv[1], sys.argv[2]
raw = open(out_path, encoding="utf-8").read()
doc = json.loads(raw)  # strict: raises on bad escapes / raw control chars

want_version = re.search(r'^K2_CLI_VERSION="([^"]+)"', open(cli_path, encoding="utf-8").read(), re.M).group(1)
problems = []
if doc["tool"] != "k2":
    problems.append("tool=%r" % doc["tool"])
if doc["version"] != want_version:
    problems.append("version=%r want %r" % (doc["version"], want_version))
cmds = doc["commands"]
if not isinstance(cmds, list) or len(cmds) < 100:
    problems.append("commands is not a list of 100+ (got %r)" % type(cmds).__name__)
names = [c["name"] for c in cmds]
for need in ("agent context add", "agent context catalog", "mail draft", "--schema"):
    if need not in names:
        problems.append("missing command %r" % need)

def strings(node):
    if isinstance(node, str):
        yield node
    elif isinstance(node, dict):
        for v in node.values():
            yield from strings(v)
    elif isinstance(node, list):
        for v in node:
            yield from strings(v)

text = "\n".join(strings(doc))
# Literal bytes that the unquoted heredoc used to mangle.
for pin in ("`k2 agent context catalog`",   # backticks, not executed
            "APPEND \\Draft",                # \\ in source -> one backslash
            "\\, = literal comma",
            "safe for $(...) capture",       # $ stays literal
            "$K2_REMOTE_TOKEN"):
    if pin not in text:
        problems.append("missing literal %r" % pin)

if problems:
    print("\n".join(problems))
    sys.exit(1)
print("commands=%d" % len(cmds))
PY
)"
    py_rc=$?
    set -e
    if [ "$py_rc" -eq 0 ]; then
        ok "full output parses as JSON with expected shape ($py_out)"
    else
        bad "JSON check failed:"
        printf '%s\n' "$py_out" | sed 's/^/    /' >&2
    fi
done

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
