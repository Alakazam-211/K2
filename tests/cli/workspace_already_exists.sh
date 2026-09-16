#!/usr/bin/env bash
# prd-workspace-already-exists-copy-v1 C6: k2 workspace open/create unwrap
# the daemon error string and exit non-zero (curl is not --fail).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -x "$K2" ] || fail "$K2 not executable"
bash -n "$K2" || fail "bash -n cli/k2"

# Helper must exist; open + create must use it (not dump raw JSON at exit 0).
grep -q '^cli_print_or_fail()' "$K2" || fail "cli_print_or_fail helper missing"
python3 - "$K2" <<'PY' || exit 1
import pathlib, re, sys
src = pathlib.Path(sys.argv[1]).read_text()
def body(name):
    m = re.search(rf'^{name}\(\) \{{(.*?)^}}\n', src, re.M | re.S)
    if not m:
        print(f'FAIL: {name} not found', file=sys.stderr)
        sys.exit(1)
    return m.group(1)
for name in ('cmd_workspace_open', 'cmd_workspace_create'):
    b = body(name)
    if 'cli_print_or_fail' not in b:
        print(f'FAIL: {name} must call cli_print_or_fail (C6)', file=sys.stderr)
        sys.exit(1)
print('PASS: open/create unwrap daemon error via cli_print_or_fail')
PY
