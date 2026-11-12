#!/usr/bin/env bash
# Tests for scripts/lib/linux-sums-step.sh (release.sh Step 10.6) against the
# real scripts/linux-sums.sh, with a stub gh on PATH: a "release" is a folder
# of files and the Arch run is whatever the case sets. Also pins how
# release.sh is wired now that the pre-tag Arch gate is gone. No GitHub. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
STEP="$ROOT/scripts/lib/linux-sums-step.sh"
RS="$ROOT/scripts/release.sh"
WF="$ROOT/.github/workflows/arch-package.yml"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi; }
case "$1 $2" in
    "run list")
        # Only ever asked about the tag's own push run.
        case "$*" in
            *"--workflow arch-package.yml --branch v9.9.9 --event push"*) ;;
            *) echo "gh stub: wrong run list: $*" >&2; exit 1 ;;
        esac
        [ "${STUB_RUN_LIST_FAILS:-0}" = 0 ] || exit 1
        printf '%s' "${STUB_ARCH_RUN:-}"
        ;;
    "release view")
        ls -1 "$STUB_REL/$3"
        ;;
    "release download")
        tag="$3"; shift 3
        pattern=""; dir="."
        while [ $# -gt 0 ]; do
            case "$1" in
                --pattern) pattern="$2"; shift 2 ;;
                --dir) dir="$2"; shift 2 ;;
                *) shift ;;
            esac
        done
        cp "$STUB_REL/$tag/$pattern" "$dir/$pattern"
        ;;
    "api repos/"*)
        [ "${STUB_API_FAILS:-0}" = 0 ] || { echo "stub: api down" >&2; exit 1; }
        tag="${2##*/}"
        # STUB_LATE: files that reach the release on the Nth pass.
        n=$(( $(cat "$STUB_REL/.passes") + 1 ))
        echo "$n" > "$STUB_REL/.passes"
        if [ -n "${STUB_LATE_AFTER:-}" ] && [ "$n" -ge "$STUB_LATE_AFTER" ]; then
            cp "$STUB_REL/.late/"* "$STUB_REL/$tag/" 2>/dev/null || true
        fi
        for f in "$STUB_REL/$tag"/*; do
            [ -e "$f" ] || continue
            n="$(basename "$f")"
            if [ -f "$STUB_REL/.digest-override/$n" ]; then
                d="$(cat "$STUB_REL/.digest-override/$n")"
            else
                d="sha256:$(sha "$f")"
            fi
            printf '%s\t%s\n' "$n" "$d"
        done
        ;;
    *)
        echo "gh stub: unexpected $*" >&2
        exit 1
        ;;
esac
STUB
chmod +x "$tmp/bin/gh"
export PATH="$tmp/bin:$PATH"
export K2_SUMS_POLL_SECS=0
export K2_RELEASE_REPO="stub/K2"

V=9.9.9
TAG="v$V"
PKG="k2-${V}-x86_64.pkg.tar.zst"
SUMS_FILE="SHA256SUMS-linux-x86_64.txt"
FAILED_RUN="completed failure 4242 https://example.test/runs/4242"
RUNNING_RUN="in_progress none 4242 https://example.test/runs/4242"
GOOD_RUN="completed success 4242 https://example.test/runs/4242"

sha_of() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi; }

# put <dir> <names...>: writes the files and appends their lines to <dir>'s SUMS.
put() {
    local dir="$1" n; shift
    for n in "$@"; do
        printf 'payload %s\n' "$n" > "$dir/$n"
        printf '%s  %s\n' "$(sha_of "$dir/$n")" "$n" >> "$dir/$SUMS_FILE"
    done
}

fresh() {
    export STUB_REL="$tmp/$1"
    REL="$STUB_REL/$TAG"
    mkdir -p "$REL" "$STUB_REL/.digest-override" "$STUB_REL/.late"
    echo 0 > "$STUB_REL/.passes"
    unset STUB_ARCH_RUN STUB_LATE_AFTER STUB_API_FAILS STUB_RUN_LIST_FAILS || true
}

# run_step <wait-min> → sets O (stderr text) and the step's flags, in a subshell
# that writes them out, so a bug that exits the shell fails loudly here.
run_step() {
    local out="$tmp/step.out" flags="$tmp/step.flags" rc=0
    (
        set -euo pipefail
        # shellcheck source=../../scripts/lib/linux-sums-step.sh
        source "$STEP"
        k2_linux_sums_step "$V" "$1" ${2:+"$2"}
        printf 'LINUX=%s\nARCH=%s\nURL=%s\nPENDING=%s\n' "$LINUX_SUMS_FAILED" "$ARCH_PKG_FAILED" "$ARCH_PKG_RUN_URL" "$ARCH_PKG_PENDING" > "$flags"
    ) >"$out" 2>&1 || rc=$?
    O="$(cat "$out")"
    [ "$rc" = 0 ] || { fail "step exits 0 (it never stops the script)" "rc=$rc: $O"; LINUX=x; ARCH=x; URL=x; return 0; }
    LINUX="$(sed -n 's/^LINUX=//p' "$flags")"
    ARCH="$(sed -n 's/^ARCH=//p' "$flags")"
    URL="$(sed -n 's/^URL=//p' "$flags")"
    PENDING="$(sed -n 's/^PENDING=//p' "$flags")"
}

check() { # check <name> <expected LINUX> <expected ARCH>
    if [ "$LINUX" = "$2" ] && [ "$ARCH" = "$3" ]; then
        pass "$1"
    else
        fail "$1" "LINUX='$LINUX' (want '$2') ARCH='$ARCH' (want '$3'): $O"
    fi
}
says() { # says <name> <fixed text>
    if printf '%s' "$O" | grep -qF -- "$2"; then pass "$1"; else fail "$1" "no '$2' in: $O"; fi
}
never_says() {
    if printf '%s' "$O" | grep -qF -- "$2"; then fail "$1" "'$2' in: $O"; else pass "$1"; fi
}

APP=("K2_${V}_amd64.deb" "K2-${V}-1.x86_64.rpm" "K2_${V}_amd64.AppImage")

echo "== all four packages up =="
fresh complete
put "$REL" "${APP[@]}" "$PKG"
run_step 0
check "complete release passes" "" ""

echo "== the Arch build failed, the rest is fine =="
fresh arch-failed
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$FAILED_RUN"
t0=$(date +%s)
run_step 1
took=$(( $(date +%s) - t0 ))
check "failed Arch build sets only ARCH_PKG_FAILED" "" "1"
[ "$took" -lt 30 ] && pass "does not sit out the wait (${took}s of a 1 min cap)" || fail "not blocked" "took ${took}s"
[ "$URL" = "https://example.test/runs/4242" ] && pass "keeps the run URL for the banner" || fail "run URL" "$URL"
says "says the build failed" "The Arch package build for v${V} FAILED (failure)"
says "links the run" "https://example.test/runs/4242"
says "says the release is live" "The release is live and stays live"
says "names the missing package" "$PKG is not published"
says "says what Arch installs see" "has no line for $PKG"
says "gives the re-run" "gh run rerun 4242 --failed --repo stub/K2"
says "gives the new-commit path" "gh workflow run arch-package.yml --repo stub/K2 -f tag=$TAG -f ref=main"
says "says the others passed" "deb, rpm and AppImage match"
[ "$(printf '%s' "$O" | grep -c 'FAILED (failure)')" = 1 ] && pass "reports it once" || fail "reports once" "$O"
never_says "no 'still missing after 0 min' noise" "still missing after 0 min"

echo "== other ways a run can fail =="
for c in cancelled timed_out startup_failure; do
    fresh "arch-$c"
    put "$REL" "${APP[@]}"
    export STUB_ARCH_RUN="completed $c 4242 https://example.test/runs/4242"
    run_step 1
    check "conclusion $c counts as failed" "" "1"
done

echo "== asset uploaded, then the sums job failed =="
fresh arch-no-line
put "$REL" "${APP[@]}"
printf 'payload\n' > "$REL/$PKG"
export STUB_ARCH_RUN="$FAILED_RUN"
run_step 1
check "package with no line, failed run: Arch only" "" "1"

echo "== Arch failed early, app-linux finishes later =="
fresh arch-first
export STUB_ARCH_RUN="$FAILED_RUN"
export STUB_LATE_AFTER=4
put "$STUB_REL/.late" "${APP[@]}"
run_step 1
check "keeps waiting for deb, rpm, AppImage, then Arch only" "" "1"
says "reports Arch before app-linux is up" "FAILED (failure)"
says "then the others passed" "deb, rpm and AppImage match"
[ "$(cat "$STUB_REL/.passes")" -ge 4 ] && pass "polled until app-linux was up" || fail "polled" "$(cat "$STUB_REL/.passes")"

echo "== Arch failed and app-linux never arrives =="
fresh both-missing
export STUB_ARCH_RUN="$FAILED_RUN"
run_step 0
check "both flags, nothing hidden" "1" "1"
says "names what is missing" "not on the release yet: $SUMS_FILE"
says "says how long it waited" "still missing after 0 min."

echo "== Arch failed and a deb line is wrong =="
fresh arch-and-wrong
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$FAILED_RUN"
printf 'sha256:%s\n' "$(printf 'a%.0s' $(seq 1 64))" > "$STUB_REL/.digest-override/K2_${V}_amd64.deb"
run_step 1
check "a wrong line is a real failure" "1" ""
says "names the wrong line" "K2_${V}_amd64.deb: SUMS says"

echo "== a re-run publishes the package during the wait =="
fresh rerun-lands
put "$REL" "${APP[@]}"
sed -i.bak '$d' "$REL/$SUMS_FILE" && rm -f "$REL/$SUMS_FILE.bak"   # rpm/AppImage line pending too
export STUB_ARCH_RUN="$FAILED_RUN"
export STUB_LATE_AFTER=3
cp "$REL/${APP[2]}" "$STUB_REL/.late/${APP[2]}"
put "$STUB_REL/.late" "$PKG"
cat "$REL/$SUMS_FILE" > "$tmp/merged"
printf '%s  %s\n' "$(sha_of "$REL/${APP[2]}")" "${APP[2]}" >> "$tmp/merged"
printf '%s  %s\n' "$(sha_of "$STUB_REL/.late/$PKG")" "$PKG" >> "$tmp/merged"
cp "$tmp/merged" "$STUB_REL/.late/$SUMS_FILE"
run_step 1
check "package arrived: the failure is cleared" "" ""

echo "== a slow Arch build is not a failure =="
fresh arch-slow
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$RUNNING_RUN"
export STUB_LATE_AFTER=3
cp "$REL/$SUMS_FILE" "$STUB_REL/.late/$SUMS_FILE"
put "$STUB_REL/.late" "$PKG"
run_step 1
check "waits and passes when it lands" "" ""
never_says "never reports a failure" "FAILED ("

fresh arch-stuck
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$RUNNING_RUN"
run_step 0
check "still running at the deadline is incomplete, not 'Arch failed'" "1" ""
says "says the run is still going" "arch-package run is still in_progress: https://example.test/runs/4242"

echo "== what the Arch run lookup cannot prove =="
fresh no-run
put "$REL" "${APP[@]}"
run_step 0
check "no run listed: incomplete, not 'Arch failed'" "1" ""

fresh lookup-down
put "$REL" "${APP[@]}"
export STUB_RUN_LIST_FAILS=1
run_step 0
check "gh run list failing is not 'Arch failed'" "1" ""

fresh good-run-no-asset
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$GOOD_RUN"
run_step 0
check "green run with the package missing is still incomplete" "1" ""

fresh api-down
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$FAILED_RUN"
export STUB_API_FAILS=1
run_step 0
check "release listing down: others not counted as checked" "1" "1"
never_says "does not claim the others passed" "deb, rpm and AppImage match"

echo "== compute mode (arch-later): one check, the Arch package not waited for =="
fresh later-pending
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$RUNNING_RUN"
t0=$(date +%s)
run_step 60 arch-later
took=$(( $(date +%s) - t0 ))
check "deb, rpm, AppImage up, Arch still building: no failure" "" ""
[ "$PENDING" = 1 ] && pass "ARCH_PKG_PENDING=1 for the final summary" || fail "pending flag" "PENDING='$PENDING'"
[ "$took" -lt 30 ] && pass "no wait for the Arch package (${took}s of a 60 min cap)" || fail "arch-later waited" "took ${took}s"
says "says it is not waited for" "is not waited for"
says "gives the later check" "bash scripts/linux-sums.sh check ${V} --wait-min 60"
fresh later-complete
put "$REL" "${APP[@]}" "$PKG"
run_step 0 arch-later
check "everything up: passes" "" ""
[ -z "$PENDING" ] && pass "…and nothing pending" || fail "pending when complete" "PENDING='$PENDING'"
fresh later-missing
put "$REL" "${APP[0]}"
export STUB_ARCH_RUN="$RUNNING_RUN"
run_step 0 arch-later
check "a missing rpm/AppImage is still a failure" "1" ""
[ -z "$PENDING" ] && pass "…not 'Arch pending'" || fail "pending with others missing" "PENDING='$PENDING'"
fresh later-wrong
put "$REL" "${APP[@]}"
printf 'sha256:%s\n' "$(printf 'a%.0s' $(seq 1 64))" > "$STUB_REL/.digest-override/K2_${V}_amd64.deb"
run_step 0 arch-later
check "a wrong line is still a failure" "1" ""
fresh later-failed
put "$REL" "${APP[@]}"
export STUB_ARCH_RUN="$FAILED_RUN"
run_step 0 arch-later
check "a failed Arch build is still reported as failed" "" "1"

echo "== release.sh wiring =="
bash -n "$RS" && pass "release.sh parses" || fail "release.sh parses" "bash -n"
bash -n "$STEP" && pass "the step parses" || fail "the step parses" "bash -n"
if grep -nE 'arch-build-gate|K2_SKIP_ARCH_GATE|K2_ARCH_GATE|ARCH_GATE_|k2_arch_gate_(start|check|wait|report)' "$RS"; then
    fail "the pre-tag Arch gate is gone from release.sh" "see above"
else
    pass "the pre-tag Arch gate is gone from release.sh"
fi
[ ! -e "$ROOT/scripts/arch-build-gate.sh" ] && pass "arch-build-gate.sh is gone" || fail "gate script" "still there"
grep -q '^trap k2_arch_gate_on_exit EXIT$' "$RS" && pass "the exit trap is still set" || fail "exit trap" "not set"
trap_body="$(awk '/^k2_arch_gate_on_exit\(\) \{/,/^\}/' "$RS")"
printf '%s' "$trap_body" | grep -q 'k2cr_cancel_open' && printf '%s' "$trap_body" | grep -q 'k2_release_lock_release' \
    && printf '%s' "$trap_body" | grep -q 'exit "\$rc"' \
    && pass "the exit trap still cancels shadow jobs, drops the lock and keeps the exit code" || fail "trap body" "$trap_body"

line() { grep -n "$1" "$RS" | head -1 | cut -d: -f1; }
shadow_start="$(grep -n '^k2_shadow_start$' "$RS" | cut -d: -f1)"
shadow_collect="$(grep -n '^k2_shadow_collect$' "$RS" | cut -d: -f1)"
step2="$(line '^# ── Step 2: Build K2.app')"
step8="$(line '^# ── Step 8: Generate latest.json')"
[ -n "$shadow_start" ] && [ "$shadow_start" -lt "$step2" ] && pass "shadow builds still start before the Mac build" || fail "shadow start" "$shadow_start vs $step2"
[ -n "$shadow_collect" ] && [ "$shadow_collect" -lt "$step8" ] && pass "shadow builds are still collected before Step 8" || fail "shadow collect" "$shadow_collect vs $step8"
grep -q '^k2_release_preview_commit() {$' "$RS" && pass "the preview commit helper stays" || fail "preview commit" "missing"

grep -q 'source "$PROJECT_DIR/scripts/lib/linux-sums-step.sh"' "$RS" && pass "release.sh sources the step" || fail "source step" "missing"
grep -q 'k2_linux_sums_step "${VERSION}" "${K2_LINUX_SUMS_WAIT_MIN:-120}"' "$RS" && pass "Step 10.6 runs the step" || fail "step call" "missing"
grep -q '\[ -n "\$ARCH_PKG_FAILED" \] && echo "  ⚠  ARCH PACKAGE NOT PUBLISHED' "$RS" && pass "the final banner names a failed Arch build" || fail "banner" "missing"
grep -q '\[ -z "\$ARCH_PKG_FAILED" \] || exit 1$' "$RS" && pass "a failed Arch build still ends with exit 1" || fail "exit 1" "missing"

echo "== the package still publishes after the tag =="
python3 - "$WF" <<'PY' && pass "arch-package.yml still runs on v* tags and uploads" || fail "arch-package.yml trigger" "see above"
import re, sys
s = open(sys.argv[1]).read()
assert re.search(r'^on:\n  push:\n    tags:\n      - "v\*"\n', s, re.M), "no v* tag trigger"
assert "scripts/arch-upload-pkg.sh" in s, "no upload"
assert "linux-sums.sh merge" in s or "scripts/linux-sums.sh" in s, "no SUMS merge"
assert "gate_id" not in s, "gate_id left over"
PY
grep -q '"k2-${v}-x86_64.pkg.tar.zst"' "$ROOT/scripts/linux-sums.sh" && pass "linux-sums.sh still expects the package" || fail "expected assets" "missing"

echo ""
echo "linux-sums-step: ${pass} passed, ${fail} failed"
[ "$fail" = 0 ]
