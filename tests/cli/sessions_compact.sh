#!/usr/bin/env bash
# `k2 sessions compact <id>` end to end, with the system's gzip: on Linux
# (GNU stat and wc) and macOS (BSD stat and wc) alike.
#
# Regression (0.46.17): compact read each segment's size with
# `stat -f '%z' FILE || stat -c '%s' FILE`. GNU's stat reads -f as "the
# file system": it printed the file system's lines and failed, the second
# stat's size was added under them, and the arithmetic on those lines
# stopped the command. So compact broke on Linux. It now reads the size
# with `wc -c <FILE`, which every system has.
#
# Each case checks the files left behind, the JSON line's counts against
# the sizes on disk, and that the active segment and other names are left
# alone. The line's session id is a JSON string: a quote or a backslash in
# the id is escaped. No daemon: compact reads only the workspace's folders.
#
# Hermetic: throwaway HOME, a dead port and a dummy token, no real ~/.k2.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v gzip >/dev/null || { echo "FAIL: gzip is required" >&2; exit 1; }
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-sessions-compact-XXXXXX)" && pwd -P)"
trap 'rm -rf "$T"; hermetic_cli_cleanup' EXIT
pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WS="$T/ws"
mkdir -p "$WS/.k2/sessions"

# seed <id>: rotated segments to compress, one already compressed, the
# active segment and a name compact doesn't take.
seed() {
    local d="$WS/.k2/sessions/$1"
    mkdir -p "$d"
    python3 - "$d" <<'PY'
import os, sys
d = sys.argv[1]
def put(name, n):
    with open(os.path.join(d, name), "wb") as f:
        f.write((b"session line\n" * (n // 13 + 1))[:n])
put("archive.001.ndjson", 40000)
put("archive.002.ndjson", 7)
put("archive.ndjson", 500)
put("archive.1.ndjson", 300)
put("archive.003.ndjson.gz", 50)
PY
}

# run <args...>: RC, OUT, ERR.
run() {
    RC=0
    K2_PROJECT_PATH="$WS" "$K2_CLI" "$@" >"$T/out" 2>"$T/err" </dev/null || RC=$?
    OUT="$(cat "$T/out")"
    ERR="$(cat "$T/err")"
}

# check_compacted <id> <label>: the JSON line against the files on disk.
check_compacted() {
    local id="$1" label="$2" verdict
    verdict="$(python3 - "$WS/.k2/sessions/$id" "$id" "$T/out" <<'PY'
import gzip, json, os, sys
d, want_id, out = sys.argv[1], sys.argv[2], sys.argv[3]
raw = open(out, "rb").read().decode()
lines = raw.splitlines()
if len(lines) != 1:
    print("want one line, got %r" % raw); sys.exit()
v = json.loads(lines[0])
def size(n): return os.path.getsize(os.path.join(d, n))
names = sorted(os.listdir(d))
want_names = ["archive.001.ndjson.gz", "archive.002.ndjson.gz", "archive.003.ndjson.gz",
              "archive.1.ndjson", "archive.ndjson"]
after = size("archive.001.ndjson.gz") + size("archive.002.ndjson.gz")
want = {"sessionId": want_id, "compacted": 2, "bytesBefore": 40007,
        "bytesAfter": after, "bytesSaved": 40007 - after}
problems = []
if names != want_names: problems.append("files %r" % names)
if v != want: problems.append("line %r, want %r" % (v, want))
if list(v) != list(want): problems.append("key order %r" % list(v))
body = gzip.open(os.path.join(d, "archive.001.ndjson.gz")).read()
if body != (b"session line\n" * 3077)[:40000]: problems.append("archive.001 does not gunzip to its bytes")
if size("archive.ndjson") != 500 or size("archive.1.ndjson") != 300 or size("archive.003.ndjson.gz") != 50:
    problems.append("an untouched file changed")
print("; ".join(problems) or "ok")
PY
)"
    if [ "$RC" = 0 ] && [ -z "$ERR" ] && [ "$verdict" = ok ]; then
        ok "$label"
    else
        bad "$label: rc=$RC stderr=[$ERR] $verdict"
    fi
}

echo "== compact on this system's stat and wc ($(uname -s)) =="
seed s1
run sessions compact s1
check_compacted s1 "compact s1: two segments compressed, the line's sizes are the files'"

run sessions compact s1
if [ "$RC" = 0 ] && [ -z "$OUT" ] &&
    [ "$ERR" = "Nothing to compact for session s1 (no uncompressed rotated segments)." ]; then
    ok "compact s1 again: nothing to compact, exit 0"
else
    bad "compact s1 again: rc=$RC stdout=[$OUT] stderr=[$ERR]"
fi

echo "== an id with a quote and a backslash is a JSON string =="
# Before 0.46.17 the line printed the id as it is: {"sessionId":"we"ird\id",...
# is not JSON. Python's json reads the line back to the same id.
ODD='we"ird\id'
seed "$ODD"
run sessions compact "$ODD"
check_compacted "$ODD" "compact $ODD: the id escaped, the line parses back to it"

echo
echo "sessions_compact: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
