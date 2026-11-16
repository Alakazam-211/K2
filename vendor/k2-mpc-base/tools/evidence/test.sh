#!/bin/sh
# Self-test of the evidence tools: the fixture that must pass, the planted negatives that must
# fail, and the log parser on a synthetic log. POSIX sh and the tools binary (`mpc-tools
# evidence`, tools/mpc-tools.sh: a Linux job); run from anywhere.
set -u
here=$(cd "$(dirname "$0")" && pwd)
T="$here/../mpc-tools.sh"
t=$here/tests
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
if sh "$T" evidence check --runs "$t/runs" "$t/docs/good.md" > /dev/null; then ok "good.md passes"; else bad "good.md must pass:"; sh "$T" evidence check --runs "$t/runs" "$t/docs/good.md"; fi
nb=0
for f in "$t"/docs/bad_*.md; do
  # an unmatched glob is a missing fixture set, never a pass
  [ -f "$f" ] || { bad "no bad_*.md fixtures in $t/docs"; continue; }
  nb=$((nb + 1))
  n=$(basename "$f")
  if out=$(sh "$T" evidence check --runs "$t/runs" "$f"); then bad "$n must fail"; else ok "$n fails: $(echo "$out" | head -1 | sed 's/^[^:]*:[0-9]*: //')"; fi
done
# records that say a run ran nothing fail, cited or not; the acknowledged and the normal ones don't
if out=$(sh "$T" evidence check --runs "$t/runs-notrun" "$t/docs/empty.md"); then bad "a run that didn't run must fail the check"; else
  for w in r_error r_noresult r_zero_samples r_zero_states; do
    case $out in *"ff000003/$w "*) ok "$w fails" ;; *) bad "$w must be named: $out" ;; esac
  done
  for w in r_known r_caught r_fine; do
    case $out in *"ff000003/$w "*) bad "$w must not be named" ;; *) ok "$w passes" ;; esac
  done
fi
# the sample log's records, byte for byte as the Python collector printed them (pinned when
# it was replaced; tools/mpc-tools/tests/data/evidence-collect holds 221 more)
if got=$(sh "$T" evidence collect --print --log "$t/logs/sample.txt" --job ff000002) \
   && [ "$got" = "$(cat "$t/logs/sample.records.json")" ]; then ok "collect parses the sample log"; else bad "collect on the sample log:"; echo "$got"; fi
[ $fail = 0 ] && echo "evidence self-test: ok" || echo "evidence self-test: FAILED"
exit $fail
