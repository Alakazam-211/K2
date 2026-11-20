#!/usr/bin/env bash
# scripts/lib/release-timer.sh (release.sh's per-step timings) and
# scripts/release-timings.sh (the reader), on their own: no release.sh, no
# network, temp dirs only. Runs on macOS (bash 3.2) and Linux.
#
# Asserts:
#  - every record is one JSON line; the run record has version, mode, host,
#    git sha; steps nest (parent, depth), close in order, and a numbered
#    step ends the one before it;
#  - a waiting sub-step is counted as waiting, not machine time;
#  - _rt_mark / _rt_skip give skipped rows; a resumed run appends (the first
#    run's lines stay as they were) and marks every row resumed;
#  - _rt_finish closes what is open (fail / interrupted / ok), writes the end
#    record once, prints the table, and never changes the exit code;
#  - the timer never stops or skips anything: the caller's -e/-u/pipefail
#    come back as they were; an unwritable folder, a clock that prints junk
#    (inside an if-block, where a bash arithmetic error would skip the rest
#    of the block) or an unknown step only warn;
#  - labels only: nothing from the environment is written;
#  - the reader prints every run, the totals (machine vs waiting) and
#    "unfinished" for a killed run; --last, --run, --dry; bad input exits 1/2.
# Run with: bash tests/packaging/release-timer.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$ROOT/scripts/lib/release-timer.sh"
READER="$ROOT/scripts/release-timings.sh"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || printf '        %s\n' "$2" >&2; fail=$((fail + 1)); }
check() { if eval "$1"; then pass "$2"; else fail "$2" "${3:-}"; fi; }

# The test process only: no K2_* from the calling shell reaches the runs.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
work="$(mktemp -d -t k2-release-timer-test.XXXXXX)"
trap 'rm -rf "$work"' EXIT
export HOME="$work/home"
mkdir -p "$HOME" "$work/repo"
git -C "$work/repo" init -q
git -C "$work/repo" -c user.email=t@example.invalid -c user.name=t commit -q --allow-empty -m base
SHA="$(git -C "$work/repo" rev-parse HEAD)"

# A run under release.sh's shell options. $1 = script body; the rest = env.
run_script() { # <name> <body> [env...]; prints rc
    local name="$1" body="$2" rc=0
    shift 2
    printf 'set -euo pipefail\nsource "%s"\n%s\n' "$LIB" "$body" > "$work/$name.sh"
    env "$@" bash "$work/$name.sh" > "$work/$name.out" 2>&1 || rc=$?
    echo "$rc"
}
jsonl_ok() { # every line of $1 is a JSON object
    python3 -I -c '
import json, sys
n = 0
for line in open(sys.argv[1]):
    assert isinstance(json.loads(line), dict); n += 1
assert n > 0' "$1"
}
q() { # <file> <python expression over R (all records)>: exit 0 when true
    python3 -I -c '
import json, sys
R = [json.loads(l) for l in open(sys.argv[1])]
S = [r for r in R if r["type"] == "step"]
def step(i, run=None):
    m = [r for r in S if r["id"] == i and (run is None or r["run"] == run)]
    assert len(m) == 1, (i, m)
    return m[0]
sys.exit(0 if eval(sys.argv[2]) else 1)' "$1" "$2"
}

echo "A run: steps, sub-steps, waiting, the end record:"
TF="$work/audit/9.9.1/timings.jsonl"
body='_rt_init 9.9.1 "'"$work/repo"'"
trap '"'"'_rt_finish "$?"'"'"' EXIT
_rt_step start "Lock and settings"
_rt_step 1 "Step one"
_rt_start 1:sub "A sub-step"
_rt_start 1:inner "Inside the sub-step"
_rt_end 1:inner
_rt_end 1:sub 0
_rt_step 2 "Step two"
_rt_start 2:approval "Waiting for a person" waiting
_rt_end 2:approval
_rt_start 2:bad "A sub-step that failed"
_rt_end 2:bad 3
_rt_step 3 "Step three with \"quotes\", a \\ and a	tab"
echo "opts=$-"
shopt -qo pipefail && echo "pipefail=on"'
rc="$(run_script a "$body" K2_RELEASE_AUDIT_DIR="$work/audit" SECRET_VALUE=hunter2-not-for-files)"
out="$(cat "$work/a.out")"
check '[ "$rc" = 0 ]' "a run exits 0" "rc=$rc; $out"
check 'jsonl_ok "$TF"' "every line is a JSON object" "$(cat "$TF" 2>&1)"
check 'q "$TF" "R[0][\"type\"] == \"run\" and R[0][\"version\"] == \"9.9.1\" and R[0][\"mode\"] == \"real\" and R[0][\"dry\"] is False and R[0][\"resume_from\"] is None and R[0][\"git_sha\"] == \"$SHA\" and R[0][\"host\"]"' \
    "the run record: version, mode real, git sha, host" "$(head -1 "$TF")"
check 'q "$TF" "[s[\"id\"] for s in S if s[\"depth\"] == 0] == [\"start\", \"1\", \"2\", \"3\"]"' "the numbered steps, in order" "$(cat "$TF")"
check 'q "$TF" "step(\"1:sub\")[\"parent\"] == \"1\" and step(\"1:inner\")[\"parent\"] == \"1:sub\" and step(\"1:inner\")[\"depth\"] == 2"' "sub-steps nest (parent, depth)"
check 'q "$TF" "step(\"2:bad\")[\"status\"] == \"fail\" and step(\"1:sub\")[\"status\"] == \"ok\" and step(\"3\")[\"status\"] == \"ok\""' "an rc of 3 ends a sub-step as fail; 0 as ok; exit 0 closes the last step ok"
check 'q "$TF" "step(\"2:approval\")[\"kind\"] == \"waiting\" and step(\"1\")[\"kind\"] == \"machine\""' "kinds: waiting and machine"
check 'q "$TF" "all(s[\"seconds\"] == s[\"end_epoch\"] - s[\"start_epoch\"] >= 0 for s in S)"' "seconds = end - start, never negative"
check 'q "$TF" "all(s[\"end_epoch\"] >= s[\"start_epoch\"] and s[\"start\"].endswith(\"Z\") for s in S)"' "wall-clock start and end (UTC)"
check 'q "$TF" "R[-1][\"type\"] == \"end\" and R[-1][\"rc\"] == 0 and R[-1][\"status\"] == \"ok\" and R[-1][\"timer_errors\"] == 0"' "the end record: rc 0, ok, no timer errors"
check 'q "$TF" "len([r for r in R if r[\"type\"] == \"begin\"]) == len(S)"' "a begin record for every step"
check 'q "$TF" "step(\"3\")[\"label\"] == \"Step three with \\\"quotes\\\", a \\\\ and a tab\""' "labels are escaped as JSON"
check '! grep -q hunter2 "$TF"' "nothing from the environment is written"
check 'grep -q "^opts=.*e.*" <<<"$out" && grep -q "^opts=.*u.*" <<<"$out" && grep -qx "pipefail=on" <<<"$out"' "the caller's -e, -u and pipefail are as they were" "$out"
check 'grep -q "^Step timings: v9.9.1, real, run " <<<"$out" && grep -qE "^  2 +machine +ok" <<<"$out" && grep -qE "^    1:sub +machine +ok" <<<"$out" && grep -qE "^      1:inner +machine" <<<"$out"' "the table at the end, sub-steps indented" "$out"
check 'grep -q "  machine [0-9]*m[0-9]*s, waiting for a person [0-9]*m[0-9]*s, steps" <<<"$out" && grep -qF "timings: $TF" <<<"$out"' "the totals line and where the file is" "$out"

echo "Totals: machine vs waiting (a fake clock):"
body='T=1000
_rt_clock() { printf "%s 2026-11-20T00:00:00Z\n" "$T"; }
_rt_init 9.9.2 "'"$work/repo"'"
trap '"'"'_rt_finish "$?"'"'"' EXIT
_rt_step start "Start"; T=1060
_rt_step 1 "One"; T=1080
_rt_start 1:approval "Waiting for a person" waiting; T=1120
_rt_start 1:nested "A wait inside the wait" waiting; T=1130
_rt_end 1:nested; _rt_end 1:approval; T=1160
_rt_step 2 "Two"; T=1200'
rc="$(run_script b "$body" K2_RELEASE_AUDIT_DIR="$work/audit")"
out="$(cat "$work/b.out")"
TF2="$work/audit/9.9.2/timings.jsonl"
check '[ "$rc" = 0 ] && q "$TF2" "step(\"start\")[\"seconds\"] == 60 and step(\"1\")[\"seconds\"] == 100 and step(\"1:approval\")[\"seconds\"] == 50 and step(\"2\")[\"seconds\"] == 40"' \
    "the seconds come from the clock" "$out $(cat "$TF2")"
# steps 200 s; waiting 50 s (the nested wait is inside it, counted once).
check 'grep -qF "  machine 2m30s, waiting for a person 0m50s, steps 3m20s, wall clock 3m20s" <<<"$out"' \
    "the table: machine 150 s, waiting 50 s (a wait inside a wait counted once)" "$out"
rout="$(bash "$READER" --file "$TF2" 2>&1)" || true
check 'grep -qF "machine 2m30s, waiting for a person 0m50s, steps 3m20s, wall clock 3m20s, exit 0 (ok)" <<<"$rout" && grep -qF "Totals (1 run): machine 2m30s, waiting for a person 0m50s, all 3m20s" <<<"$rout"' \
    "the reader prints the same totals" "$rout"
check 'grep -qE "^      1:nested +waiting +ok" <<<"$rout" && grep -qE "^    1:approval +waiting" <<<"$rout"' "the reader indents sub-steps" "$rout"

echo "A failing run, _rt_mark, _rt_skip, unended sub-steps:"
body='_rt_init 9.9.3 "'"$work/repo"'"
trap '"'"'_rt_finish "$?"'"'"' EXIT
_rt_step start "Start"
_rt_step 0.5 "Gates"
_rt_mark skipped
_rt_skip 0.55 0.6
_rt_step 2 "Build"
_rt_start 2:left-open "Never ended"
_rt_step 3 "Sign"
_rt_start 3:notarize "Notarize"
false
echo "not reached"'
rc="$(run_script c "$body" K2_RELEASE_AUDIT_DIR="$work/audit")"
out="$(cat "$work/c.out")"
TF3="$work/audit/9.9.3/timings.jsonl"
check '[ "$rc" = 1 ]' "the run's own exit code (1) is kept" "rc=$rc; $out"
check 'q "$TF3" "step(\"0.5\")[\"status\"] == \"skipped\" and step(\"0.55\")[\"status\"] == \"skipped\" and step(\"0.6\")[\"seconds\"] == 0"' "_rt_mark and _rt_skip: skipped rows"
check 'q "$TF3" "step(\"2:left-open\")[\"status\"] == \"unended\" and step(\"2\")[\"status\"] == \"ok\""' "a sub-step never ended is closed as unended at the next step"
check 'q "$TF3" "step(\"3:notarize\")[\"status\"] == \"fail\" and step(\"3\")[\"status\"] == \"fail\" and R[-1][\"rc\"] == 1 and R[-1][\"status\"] == \"fail\""' \
    "what was open when it failed: fail; the end record has rc 1" "$(tail -3 "$TF3")"
check 'grep -q "(exit 1)" <<<"$out" && grep -qE "^    3:notarize +machine +fail" <<<"$out"' "the table is printed on failure too" "$out"
# As release.sh: TERM exits 143, the EXIT trap does the rest.
rc="$(run_script d '_rt_init 9.9.4 "'"$work/repo"'"; trap '"'"'_rt_finish "$?"'"'"' EXIT; trap "exit 143" TERM; _rt_step start S; _rt_step 1 One; kill -TERM $$; sleep 5' K2_RELEASE_AUDIT_DIR="$work/audit")"
check '[ "$rc" = 143 ] && q "$work/audit/9.9.4/timings.jsonl" "R[-1][\"status\"] == \"interrupted\" and R[-1][\"rc\"] == 143 and step(\"1\")[\"status\"] == \"interrupted\""' "TERM: interrupted, exit 143 kept" "rc=$rc; $(cat "$work/d.out")"

echo "A resumed run appends:"
before="$(cat "$TF")"
body='_rt_init 9.9.1 "'"$work/repo"'"
trap '"'"'_rt_finish "$?"'"'"' EXIT
_rt_step start "Start"
_rt_skip 2 3
_rt_step 8 "latest.json"'
rc="$(run_script e "$body" K2_RELEASE_AUDIT_DIR="$work/audit" K2_RELEASE_RESUME=8)"
out="$(cat "$work/e.out")"
check '[ "$rc" = 0 ] && [ "$(head -n "$(printf "%s\n" "$before" | wc -l)" "$TF")" = "$before" ]' "the first run's lines are untouched" "$out"
check 'q "$TF" "len([r for r in R if r[\"type\"] == \"run\"]) == 2 and R[-1][\"type\"] == \"end\""' "a second run record and its end"
run2="$(python3 -I -c 'import json,sys; print([json.loads(l) for l in open(sys.argv[1]) if "\"type\":\"run\"" in l][-1]["run"])' "$TF")"
check 'q "$TF" "[r for r in R if r[\"type\"] == \"run\"][-1][\"mode\"] == \"resume-from-8\" and [r for r in R if r[\"type\"] == \"run\"][-1][\"resume_from\"] == \"8\""' "mode resume-from-8"
check 'q "$TF" "all(s[\"resumed\"] for s in S if s[\"run\"] == \"$run2\") and not any(s[\"resumed\"] for s in S if s[\"run\"] != \"$run2\")"' "every row of the resumed run is marked resumed (and only those)"
check 'q "$TF" "step(\"2\", \"$run2\")[\"status\"] == \"skipped\" and \"resumed run\" in step(\"2\", \"$run2\")[\"label\"]"' "its skipped steps say why"
check 'grep -q "1 earlier run(s) of v9.9.1" <<<"$out"' "the table says the file has an earlier run" "$out"
rout="$(K2_RELEASE_AUDIT_DIR="$work/audit" bash "$READER" 9.9.1 2>&1)" || true
check 'grep -q "^Totals (2 runs): " <<<"$rout" && [ "$(grep -c "^Run " <<<"$rout")" = 2 ] && grep -q "resume-from-8" <<<"$rout"' "the reader shows both runs and the totals over both" "$rout"
rout="$(K2_RELEASE_AUDIT_DIR="$work/audit" bash "$READER" 9.9.1 --run "$run2" 2>&1)" || true
check '[ "$(grep -c "^Run " <<<"$rout")" = 1 ] && grep -q "^Totals (1 run)" <<<"$rout"' "--run: one run" "$rout"

echo "The timer never stops a release:"
body='_rt_init 9.9.5 "'"$work/repo"'"
trap '"'"'_rt_finish "$?"'"'"' EXIT
_rt_step start "Start"
if true; then
    _rt_clock() { echo "not a clock"; }
    _rt_step 1 "One"
    _rt_start 1:x "X"
    _rt_end 1:x
    echo "after the junk clock, still inside the if"
    _rt_clock() { date -u "+%s %Y-%m-%dT%H:%M:%SZ"; }
fi
_rt_end not-open
_rt_start "bad id with spaces" "Bad"
_rt_start 1:k "Odd kind" sideways
_rt_end 1:k
echo "reached the end"
exit 7'
rc="$(run_script f "$body" K2_RELEASE_AUDIT_DIR="$work/audit")"
out="$(cat "$work/f.out")"
check '[ "$rc" = 7 ]' "the exit code is the run's (7)" "rc=$rc; $out"
check 'grep -qx "after the junk clock, still inside the if" <<<"$out" && grep -qx "reached the end" <<<"$out"' "a junk clock inside an if-block skips nothing" "$out"
check 'grep -q "\[release-timer\] warning: the clock" <<<"$out" && grep -q "no such step is open" <<<"$out" && grep -q "bad sub-step id" <<<"$out" && grep -q "unknown kind" <<<"$out"' "each problem is one warning line" "$out"
check 'jsonl_ok "$work/audit/9.9.5/timings.jsonl"' "the file is still JSON lines"
: > "$work/audit-is-a-file"
rc="$(run_script g '_rt_init 9.9.6 "'"$work/repo"'"; trap '"'"'_rt_finish "$?"'"'"' EXIT; _rt_step start S; _rt_step 1 One; echo body-ran; exit 4' K2_RELEASE_AUDIT_DIR="$work/audit-is-a-file")"
out="$(cat "$work/g.out")"
check '[ "$rc" = 4 ] && grep -qx body-ran <<<"$out"' "an unwritable timings folder: the run goes on, same exit code" "rc=$rc; $out"
check 'grep -q "\[release-timer\] warning: can.t make" <<<"$out" && grep -q "timings: not written" <<<"$out" && grep -qE "^  1 +machine +fail" <<<"$out"' "it warns, and the table is still printed" "$out"
# A folder where the file goes: no write succeeds, even as root.
mkdir -p "$work/ro/9.9.7/timings.jsonl"
rc="$(run_script h '_rt_init 9.9.7 "'"$work/repo"'"; trap '"'"'_rt_finish "$?"'"'"' EXIT; _rt_step start S; echo body-ran' K2_RELEASE_AUDIT_DIR="$work/ro")"
check '[ "$rc" = 0 ] && grep -qx body-ran "$work/h.out" && [ "$(grep -c "can.t write" "$work/h.out")" = 1 ]' "a timings file it can't write: one warning (not one per line), exit 0" "rc=$rc; $(cat "$work/h.out")"
rc="$(run_script i '_rt_step 1 One; _rt_start 1:a A; _rt_end 1:a; _rt_finish 0; echo body-ran' K2_RELEASE_AUDIT_DIR="$work/audit")"
check '[ "$rc" = 0 ] && [ "$(cat "$work/i.out")" = body-ran ]' "before _rt_init every call does nothing" "$(cat "$work/i.out")"
rc="$(run_script j '_rt_init "../x" "'"$work/repo"'"; _rt_step 1 One; echo body-ran' K2_RELEASE_AUDIT_DIR="$work/audit")"
check '[ "$rc" = 0 ] && grep -qx body-ran "$work/j.out" && [ ! -e "$work/x" ]' "a version that can't name a folder: no timings, no folder outside" "$(cat "$work/j.out")"
rc="$(run_script k 'f() { _rt_step 1 One; return 3; }; if f; then echo no; else echo "f returned $?"; fi' )"
check 'grep -qx "f returned 3" "$work/k.out"' "the caller's own status after a timer call is its own"

echo "Dry runs:"
body='_rt_init 9.9.8 "'"$work/repo"'"; echo "RUN=$K2_RELEASE_TIMER_RUN T0=$K2_RELEASE_TIMER_T0"'
rc="$(run_script l "$body" K2_RELEASE_DRY=1 K2_RELEASE_AUDIT_DIR="$work/audit")"
check '[ "$rc" = 0 ] && [ ! -e "$work/audit/9.9.8" ] && grep -qE "^RUN=[0-9]{8}T[0-9]{6}Z-[0-9]+ T0=[0-9]+ " "$work/l.out"' \
    "the dry-run switch writes nothing and hands the run id and start on" "$(cat "$work/l.out")"
mkdir -p "$work/dry/runs/9.9.8-x"
body='_rt_init 9.9.8 "'"$work/repo"'"; trap '"'"'_rt_finish "$?"'"'"' EXIT; _rt_step start S; _rt_step 1 One'
rc="$(run_script m "$body" K2_RELEASE_DRY=1 K2_RELEASE_DRY_INNER=1 K2_RELEASE_DRY_RUN_DIR="$work/dry/runs/9.9.8-x" \
    K2_RELEASE_TIMER_RUN=20261120T000000Z-42 K2_RELEASE_TIMER_T0="1763596800 2025-11-20T00:00:00Z")"
DF="$work/dry/timings/9.9.8/timings.jsonl"
check '[ "$rc" = 0 ] && q "$DF" "R[0][\"run\"] == \"20261120T000000Z-42\" and R[0][\"mode\"] == \"dry-run\" and R[0][\"dry\"] is True and step(\"start\")[\"start_epoch\"] == 1763596800"' \
    "in the scratch tree: <dry root>/timings/<version>, the switch's run id and start" "$(cat "$work/m.out")"
rc="$(run_script n '_rt_init 9.9.9 "'"$work/repo"'"; trap '"'"'_rt_finish "$?"'"'"' EXIT; _rt_step start S' \
    K2_RELEASE_AUDIT_DIR="$work/audit" K2_RELEASE_TIMER_RUN=20261120T000000Z-42 K2_RELEASE_TIMER_T0="1 2025-11-20T00:00:00Z")"
check 'q "$work/audit/9.9.9/timings.jsonl" "R[0][\"run\"] != \"20261120T000000Z-42\" and R[0][\"started_epoch\"] > 1"' "a real run ignores a handed-on run id"
rout="$(K2_RELEASE_DRY_ROOT="$work/dry" bash "$READER" --dry 9.9.8 2>&1)" || true
check 'grep -q "Run 20261120T000000Z-42: v9.9.8, dry-run" <<<"$rout"' "reader --dry <version>" "$rout"
rout="$(K2_RELEASE_DRY_ROOT="$work/dry" bash "$READER" --dry --last 2>&1)" || true
check 'grep -qF "$DF" <<<"$rout"' "reader --dry --last" "$rout"

echo "The reader:"
touch -t 203001010000 "$TF"
rout="$(K2_RELEASE_AUDIT_DIR="$work/audit" bash "$READER" --last 2>&1)" || true
check 'grep -qF "Release timings: $TF" <<<"$rout"' "--last: the file written to last" "$rout"
cat > "$work/killed.jsonl" <<'EOF'
{"type":"run","run":"r1","version":"1.0.0","mode":"real","dry":false,"resume_from":null,"host":"h","git_sha":"abc","started":"2026-11-20T00:00:00Z","started_epoch":100,"format":1}
{"type":"begin","run":"r1","id":"start","parent":null,"label":"Start","kind":"machine","start":"x","start_epoch":100,"resumed":false}
{"type":"step","run":"r1","id":"start","parent":null,"depth":0,"label":"Start","kind":"machine","status":"ok","start":"x","start_epoch":100,"end":"y","end_epoch":130,"seconds":30,"resumed":false}
{"type":"begin","run":"r1","id":"2","parent":null,"label":"Build","kind":"machine","start":"y","start_epoch":130,"resumed":false}
not json
EOF
rc=0; rout="$(bash "$READER" --file "$work/killed.jsonl" 2>&1)" || rc=$?
check '[ "$rc" = 0 ] && grep -qE "^  2 +machine +unfinished +-  Build" <<<"$rout" && grep -q "no end record" <<<"$rout" && grep -q "line 5 is not JSON" <<<"$rout"' \
    "a killed run: its open step is unfinished; a bad line is skipped and said" "$rout"
rc=0; bash "$READER" 0.0.404 > /dev/null 2>&1 || rc=$?
check '[ "$rc" = 1 ]' "no timings for the version: exit 1"
rc=0; bash "$READER" > /dev/null 2>&1 || rc=$?
check '[ "$rc" = 2 ]' "no version: usage, exit 2"
rc=0; bash "$READER" "../etc" > /dev/null 2>&1 || rc=$?
check '[ "$rc" = 2 ]' "not a version: exit 2"
rc=0; bash "$READER" --file "$TF" --run nope > /dev/null 2>&1 || rc=$?
check '[ "$rc" = 1 ]' "--run with no such run: exit 1"
check 'bash -n "$LIB" && bash -n "$READER"' "bash -n"

echo ""
echo "release-timer: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
