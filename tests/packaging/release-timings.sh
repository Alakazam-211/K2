#!/usr/bin/env bash
# release.sh's per-step timings (scripts/lib/release-timer.sh), end to end:
# release.sh --dry-run on the fixture repo of tests/packaging/
# release-dry-run.sh (every external command stubbed; nothing is built,
# signed or published), then scripts/release-timings.sh on what it wrote.
#
# Asserts:
#  - every numbered step of release.sh (each `k2dry_step <id>` and each
#    "# ── Step <id>" header, read from release.sh itself) gets a timing row,
#    and the sub-steps (notarizations, staples, compute waits, VM smoke,
#    push + tag, Windows build, the signing approval as "waiting") nest
#    under their step; the run record says dry-run, the version, the sha;
#  - a failing step (notarization refused) still leaves the file: every
#    step before it ok, it and its sub-step fail, an end record with the
#    exit code; the table is printed on the way out;
#  - a resumed run (K2_RELEASE_RESUME=8) appends to the same file, marks its
#    rows resumed and its not-run steps skipped, and still has every step;
#  - the reader prints each run and the totals (machine vs waiting);
#  - a timer error (the timings folder can't be made) changes nothing: the
#    same exit code and DRY_EXIT, for a passing and a failing run;
#  - bite: a release.sh with one step's _rt_step line removed fails the
#    "every step has a row" check, naming that step.
#
# release.sh is macOS-only (BSD sed -i '', ditto), so this test is too.
# Run with: bash tests/packaging/release-timings.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || printf '        %s\n' "$2" >&2; fail=$((fail + 1)); }
check() { if eval "$1"; then pass "$2"; else fail "$2" "${3:-}"; fi; }

if [ "$(uname -s)" != Darwin ]; then
    echo "FAIL: release.sh (and so this test) runs on macOS only (BSD sed -i '', ditto)." >&2
    exit 1
fi

# The test process only: no K2_* from the calling shell reaches release.sh.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
REAL_GIT="$(command -v git)"
work="$(mktemp -d -t k2-release-timings-test)"
work="$(cd "$work" && pwd -P)"
V=9.8.6
trap 'rm -rf "$work" "/tmp/K2_${V}.zip" "/tmp/k2-node-macos-aarch64_${V}.zip"' EXIT
src="$work/src"
stub="$work/stub"
T_LOG="$work/log"
mkdir -p "$src" "$stub" "$T_LOG" "$work/home"
: > "$T_LOG/forbidden.log"
export T_LOG REAL_GIT
export HOME="$work/home"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
export GIT_CONFIG_NOSYSTEM=1
# shellcheck source=tests/packaging/fixtures/release-dry-fixture.sh
source "$ROOT/tests/packaging/fixtures/release-dry-fixture.sh"
k2_release_dry_fixture
export PATH="$stub:$PATH"
export K2_RELEASE_LOCK="$work/k2-release.lock"
export K2_RELEASE_NODE_BIN="$work/no-k2-node"
export K2_RELEASE_DRY_ROOT="$work/dry"
TF="$work/dry/timings/$V/timings.jsonl"

rehearse() { # <out file> [env...]: release.sh --dry-run $V in the fixture; prints rc
    local out="$1" rc=0
    shift
    (cd "$src" && env "$@" bash scripts/release.sh --dry-run "$V") > "$out" 2>&1 || rc=$?
    echo "$rc"
}

# Every step id of a release.sh, in file order: its `k2dry_step <id>` lines
# and its "# ── Step <id>" headers.
step_ids() {
    python3 -I - "$1" <<'PY'
import re, sys
seen = []
for line in open(sys.argv[1], encoding="utf-8"):
    m = re.match(r"\s*k2dry_step ([0-9A-Za-z.]+) ", line) or re.match(r"# ── Step ([0-9][0-9a-z.]*)[: ]", line)
    if m and m.group(1).rstrip(".") not in seen:
        seen.append(m.group(1).rstrip("."))
print("\n".join(seen))
PY
}
# missing_rows <release.sh> <timings file> <run: n-th run record, 1-based or -1>
# [ok|any]: prints the step ids with no row in that run (ok: or not ok); 1 if any.
missing_rows() {
    python3 -I - "$2" "$3" "${4:-any}" "$(step_ids "$1")" <<'PY'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
runs = [r["run"] for r in recs if r["type"] == "run"]
run = runs[int(sys.argv[2]) - 1] if int(sys.argv[2]) > 0 else runs[-1]
rows = {r["id"]: r for r in recs if r["type"] == "step" and r["run"] == run and r["depth"] == 0}
bad = [i for i in sys.argv[4].split() if i not in rows or (sys.argv[3] == "ok" and rows[i]["status"] != "ok")]
print(" ".join(bad))
sys.exit(1 if bad else 0)
PY
}
q() { # <python expression over R (records), S (steps); run(n) = n-th run's id>
    python3 -I -c '
import json, sys
R = [json.loads(l) for l in open(sys.argv[1])]
S = [r for r in R if r["type"] == "step"]
RUNS = [r for r in R if r["type"] == "run"]
def run(n): return RUNS[n - 1]["run"]
def step(i, n):
    m = [r for r in S if r["id"] == i and r["run"] == run(n)]
    assert len(m) == 1, (i, n, m)
    return m[0]
sys.exit(0 if eval(sys.argv[2]) else 1)' "$TF" "$1"
}

IDS="$(step_ids "$ROOT/scripts/release.sh" | tr '\n' ' ')"
check '[ "$(wc -w <<<"$IDS")" -ge 30 ] && grep -qw 0.5 <<<"$IDS" && grep -qw 8.5 <<<"$IDS" && grep -qw 9.5 <<<"$IDS" && grep -qw 10.5 <<<"$IDS" && grep -qw 10.6 <<<"$IDS" && grep -qw 1.55 <<<"$IDS"' \
    "release.sh's steps are read from release.sh ($(wc -w <<<"$IDS" | tr -d ' ') ids)" "$IDS"

echo "A full rehearsal:"
rc="$(rehearse "$work/run1.out")"
out1="$(cat "$work/run1.out")"
check '[ "$rc" = 0 ] && grep -qx "DRY_EXIT=0" "$work/run1.out"' "the rehearsal passes (DRY_EXIT=0)" "rc=$rc; $(tail -40 "$work/run1.out")"
check '[ -s "$TF" ] && python3 -I -c "import json,sys; [json.loads(l) for l in open(sys.argv[1])]" "$TF"' "the timings file is JSON lines at <dry root>/timings/<version>/" "$(ls -R "$work/dry/timings" 2>&1)"
m="$(missing_rows "$ROOT/scripts/release.sh" "$TF" 1 ok || true)"
check '[ -z "$m" ]' "every numbered step has a timing row, all ok" "missing or not ok: $m"
check 'q "R[0][\"type\"] == \"run\" and R[0][\"mode\"] == \"dry-run\" and R[0][\"version\"] == \"$V\" and R[0][\"git_sha\"] == \"$("$REAL_GIT" -C "$src" rev-parse HEAD)\" and R[0][\"host\"]"' \
    "the run record: dry-run, the version, the commit, the host" "$(head -1 "$TF")"
for s in start:node-idle 1.55:approval 4:node-submit 4:app-notarize 4:app-staple 7:dmg-notarize 7:dmg-staple 7.5:vm-smoke \
    7.9c:compute-wait 7.9c:fetch-verify 8.5:standalone-daemon 8.55:node-notarize 8.9:push-tag 9.5:build; do
    check 'q "step(\"$s\", 1)[\"parent\"] == \"${s%%:*}\" and step(\"$s\", 1)[\"depth\"] == 1 and step(\"$s\", 1)[\"status\"] == \"ok\""' "sub-step $s, under ${s%%:*}"
done
check 'q "step(\"1.55:approval\", 1)[\"kind\"] == \"waiting\" and all(s[\"kind\"] == \"machine\" for s in S if s[\"id\"] != \"1.55:approval\")"' "the approval is waiting; the rest is machine time"
check 'q "R[-1][\"type\"] == \"end\" and R[-1][\"rc\"] == 0 and R[-1][\"status\"] == \"ok\" and R[-1][\"timer_errors\"] == 0"' "the end record: rc 0, no timer errors" "$(tail -1 "$TF")"
check '! grep -q unended "$TF"' "every sub-step was ended where it should be" "$(grep unended "$TF")"
check '! grep -q "T_FROM_DOTENV\|$work" "$TF"' "no environment values or paths in the file (labels only)" "$(grep -n "T_FROM_DOTENV\|$work" "$TF" | head -3)"
check 'grep -q "^Step timings: v$V, dry-run, run " <<<"$out1" && grep -qF "timings: $TF" <<<"$out1" && grep -q "^  machine .*waiting for a person" <<<"$out1"' \
    "the table is printed at the end" "$(tail -60 "$work/run1.out")"
check '[ "$(grep -n "^Step timings" "$work/run1.out" | cut -d: -f1)" -lt "$(grep -n "^DRY_EXIT=" "$work/run1.out" | cut -d: -f1)" ]' "before the dry summary (DRY_EXIT stays its last line)"
check '[ ! -s "$T_LOG/forbidden.log" ] && ! grep -q GUARD "$work/run1.out" && ! grep -q "release-timer\] warning" "$work/run1.out"' \
    "nothing publishing ran, no guard, no timer warning" "$(cat "$T_LOG/forbidden.log"; grep "GUARD\|release-timer" "$work/run1.out")"
first="$(cat "$TF")"

echo "The reader:"
rout="$(bash "$ROOT/scripts/release-timings.sh" --dry "$V" 2>&1)" || true
check 'grep -q "^Totals (1 run): machine [0-9]*m[0-9]*s, waiting for a person [0-9]*m[0-9]*s, all " <<<"$rout" && grep -q "^Run .*: v$V, dry-run" <<<"$rout"' "release-timings.sh --dry <version>: the run and the totals" "$rout"
missing=""
for i in $IDS; do grep -qE "^  $i +machine" <<<"$rout" || missing="$missing $i"; done
check '[ -z "$missing" ]' "every step is in its table" "missing:$missing"
check 'grep -qE "^    1.55:approval +waiting +ok" <<<"$rout"' "the approval wait, indented, kind waiting" "$rout"
rout2="$(bash "$ROOT/scripts/release-timings.sh" --dry --last 2>&1)" || true
check '[ "$rout2" = "$rout" ]' "--dry --last is the same file" "$rout2"

echo "A resumed rehearsal (K2_RELEASE_RESUME=8) appends:"
rc="$(rehearse "$work/run2.out" K2_RELEASE_RESUME=8)"
check '[ "$rc" = 0 ] && grep -qx "DRY_EXIT=0" "$work/run2.out"' "the resumed rehearsal passes" "rc=$rc; $(tail -40 "$work/run2.out")"
check '[ "$(head -n "$(printf "%s\n" "$first" | wc -l)" "$TF")" = "$first" ]' "the first run's lines are untouched"
check 'q "len(RUNS) == 2 and RUNS[1][\"mode\"] == \"dry-run-resume-from-8\" and RUNS[1][\"resume_from\"] == \"8\""' "a second run: dry-run-resume-from-8"
m="$(missing_rows "$ROOT/scripts/release.sh" "$TF" 2 any || true)"
check '[ -z "$m" ]' "every numbered step has a row in the resumed run too" "missing: $m"
check 'q "all(s[\"resumed\"] for s in S if s[\"run\"] == run(2)) and not any(s[\"resumed\"] for s in S if s[\"run\"] == run(1))"' "its rows are marked resumed (the first run's are not)"
check 'q "all(step(i, 2)[\"status\"] == \"skipped\" for i in [\"0.5\", \"0.55\", \"0.6\", \"1\", \"1.5\", \"1.6c\", \"2\", \"3\", \"4\", \"7\", \"7.5\"]) and step(\"8\", 2)[\"status\"] == \"ok\" and step(\"1.55\", 2)[\"status\"] == \"ok\""' \
    "the steps it didn't run are skipped; 1.55 and 8 on ran" "$(grep "$(python3 -I -c 'import json,sys; print([json.loads(l) for l in open(sys.argv[1]) if "\"type\":\"run\"" in l][1]["run"])' "$TF")" "$TF" | head -20)"
check 'grep -q "1 earlier run(s) of v$V" "$work/run2.out"' "its table says the file has an earlier run" "$(grep -A3 "timings:" "$work/run2.out")"
rout="$(bash "$ROOT/scripts/release-timings.sh" --dry "$V" 2>&1)" || true
check '[ "$(grep -c "^Run " <<<"$rout")" = 2 ] && grep -q "^Totals (2 runs): machine" <<<"$rout"' "the reader: both runs, the totals over both" "$rout"

echo "A failing step (notarization refused):"
rc="$(rehearse "$work/run3.out" T_NOTARY_FAIL=1)"
check '[ "$rc" != 0 ]' "the rehearsal fails" "rc=$rc"
check 'q "len(RUNS) == 3 and R[-1][\"type\"] == \"end\" and R[-1][\"rc\"] == $rc and R[-1][\"status\"] == \"fail\""' "the file gets the run and an end record with its exit code" "$(tail -2 "$TF")"
before4="$(step_ids "$ROOT/scripts/release.sh" | awk '$0 == "4" { exit } { printf "%s ", $0 }')"
check 'q "all(step(i, 3)[\"status\"] == \"ok\" for i in \"$before4\".split())"' "every step before Step 4 is there, ok ($before4)"
check 'q "step(\"4\", 3)[\"status\"] == \"fail\" and step(\"4:node-submit\", 3)[\"status\"] == \"fail\" and not [s for s in S if s[\"run\"] == run(3) and s[\"id\"] in (\"5\", \"6\", \"8\")]"' \
    "Step 4 and its sub-step fail; nothing after it has a row" "$(tail -6 "$TF")"
check 'grep -q "^Step timings: v$V, dry-run, run .*(exit $rc)" "$work/run3.out" && grep -qE "^  4 +machine +fail" "$work/run3.out"' "the table is printed on failure" "$(tail -50 "$work/run3.out")"
fail_rc="$rc"

echo "A timer error changes nothing:"
mkdir -p "$work/dry-broken"
: > "$work/dry-broken/timings"   # a file where the timings folder goes: mkdir fails
rc="$(rehearse "$work/run4.out" K2_RELEASE_DRY_ROOT="$work/dry-broken")"
check '[ "$rc" = 0 ] && grep -qx "DRY_EXIT=0" "$work/run4.out"' "a passing rehearsal still passes (exit 0, DRY_EXIT=0)" "rc=$rc; $(tail -40 "$work/run4.out")"
check 'grep -q "\[release-timer\] warning: can.t make" "$work/run4.out" && grep -q "timings: not written" "$work/run4.out" && grep -qE "^  10.7 +machine +ok" "$work/run4.out"' \
    "it says so once, and the table is still printed" "$(grep -n "release-timer\|timings:" "$work/run4.out")"
rc="$(rehearse "$work/run5.out" K2_RELEASE_DRY_ROOT="$work/dry-broken" T_NOTARY_FAIL=1)"
check '[ "$rc" = "$fail_rc" ] && [ "$(grep "^DRY_EXIT=" "$work/run5.out")" = "$(grep "^DRY_EXIT=" "$work/run3.out")" ]' \
    "a failing rehearsal ends with the same exit code and DRY_EXIT as with a working timer" "rc=$rc vs $fail_rc"
check '[ ! -e "$K2_RELEASE_LOCK" ] && [ ! -s "$T_LOG/forbidden.log" ]' "the lock is released; nothing publishing ran"

echo "Bite: a step without its timer line:"
# Step 6's _rt_step line removed, as an uncommitted change (a rehearsal
# carries those into its scratch tree).
grep -q '^_rt_step 6 "DMG"$' "$src/scripts/release.sh" || fail "the fixture's release.sh has Step 6's _rt_step line" ""
sed -i '' '/^_rt_step 6 "DMG"$/d' "$src/scripts/release.sh"
rc="$(rehearse "$work/run6.out" K2_RELEASE_DRY_ROOT="$work/dry-bite")"
BF="$work/dry-bite/timings/$V/timings.jsonl"
m="$(missing_rows "$src/scripts/release.sh" "$BF" -1 any || true)"
check '[ "$rc" = 0 ] && [ "$m" = 6 ]' "the 'every step has a row' check fails, naming Step 6" "rc=$rc missing='$m'"
"$REAL_GIT" -C "$src" checkout -q -- scripts/release.sh

echo ""
echo "release-timings: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
