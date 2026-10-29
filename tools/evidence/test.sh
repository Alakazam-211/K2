#!/bin/sh
# Self-test of the evidence tools: the fixture that must pass, the planted negatives that must
# fail, and the log parser on a synthetic log. POSIX sh + Python 3; run from anywhere.
set -u
here=$(cd "$(dirname "$0")" && pwd)
PY=${PYTHON:-python3}
t=$here/tests
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
if "$PY" "$here/check.py" --runs "$t/runs" "$t/docs/good.md" > /dev/null; then ok "good.md passes"; else bad "good.md must pass:"; "$PY" "$here/check.py" --runs "$t/runs" "$t/docs/good.md"; fi
for f in "$t"/docs/bad_*.md; do
  n=$(basename "$f")
  if out=$("$PY" "$here/check.py" --runs "$t/runs" "$f"); then bad "$n must fail"; else ok "$n fails: $(echo "$out" | head -1 | sed 's/^[^:]*:[0-9]*: //')"; fi
done
got=$("$PY" "$here/collect.py" --print --log "$t/logs/sample.txt" --job ff000002 | "$PY" -c '
import json, sys
for r in json.load(sys.stdin):
    print(r["run"], r["tool"], r["status"], r["states"], r["depth"], r["queued"], r["cap_s"], r.get("trace_states"))')
if [ "$got" = "$(cat "$t/logs/sample.expected")" ]; then ok "collect.py parses the sample log"; else bad "collect.py on the sample log:"; echo "$got"; fi
[ $fail = 0 ] && echo "evidence self-test: ok" || echo "evidence self-test: FAILED"
exit $fail
