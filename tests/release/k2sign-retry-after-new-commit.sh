#!/usr/bin/env bash
# k2sign-retry-after-new-commit.sh — the 0.46.11 case end to end: the signing
# approval is given, the run stops before anything is signed, a fix commit
# lands (the base changes), and the release is run again, several times.
# Designed behaviour: each retry on new code asks ONCE more (Touch ID or the
# login password, the sheet saying "RE-approve … for new code …"), with no
# sudo and no by-hand reset; a retry on the same code asks nothing. Once
# anything was signed under the version, new code is refused (release a new
# version), still without any reset.
#
# It runs release.sh's own approval block (the `if [ "$K2_SIGNER" = k2sign ]`
# block that computes K2_SIGN_BASE from git HEAD and calls k2sa_open_session),
# cut out of scripts/release.sh, in a throwaway git repo. Test mode: macOS,
# throwaway keys, an openssl P-256 key standing in for the Secure Enclave one;
# nothing is opened, notified or asked.
#   bash tests/release/k2sign-retry-after-new-commit.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
NOW="$(date +%s)"
export K2SIGN_TEST_NOW="$NOW"
V=0.46.11
SESS="$TROOT/var/k2sign/state/sessions"
LOG="$AUDIT/$V/signatures.tsv"

# The approval helper (K2 Release Approve's stand-in): counts how often the
# person would have been asked, signs with the enrolled test approver key.
helper="$tmp/helper.sh"
cat > "$helper" <<SH
#!/bin/sh
c=""; o=""
while [ \$# -gt 0 ]; do case "\$1" in --challenge) c="\$2"; shift ;; --out) o="\$2"; shift ;; esac; shift; done
n=\$(( \$(cat "$tmp/asks" 2>/dev/null || echo 0) + 1 )); echo "\$n" > "$tmp/asks"
cp "\$c" "$tmp/asked-\$n.json"
/usr/bin/openssl dgst -sha256 -sign "$tmp/keys/approver.key" -out "\$o" "\$c"
SH
chmod +x "$helper"
export K2SA_HELPER="$helper"
asks() { cat "$tmp/asks" 2>/dev/null || echo 0; }
field() { python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get(sys.argv[2], "<absent>"))' "$1" "$2"; }

# release.sh's approval block, verbatim.
python3 -I - "$REPO/scripts/release.sh" "$tmp/approval-block.sh" <<'PY' || { echo "FAIL: could not cut the approval block out of release.sh" >&2; exit 1; }
import sys
lines = open(sys.argv[1]).read().split("\n")
at = [i for i, l in enumerate(lines) if l.strip() == 'K2_SIGN_BASE="$(git rev-parse HEAD)"']
assert len(at) == 1, at
start = at[0] - 1
assert lines[start] == 'if [ "$K2_SIGNER" = k2sign ]; then', lines[start]
end = next(i for i in range(at[0], len(lines)) if lines[i] == "fi")
block = "\n".join(lines[start:end + 1])
assert 'k2sa_open_session "$VERSION" "$K2_SIGN_BASE" "${K2_SIGN_ASSETS[@]}" || exit 1' in block, block
open(sys.argv[2], "w").write(block + "\n")
PY
# step155: one release run's approval step, in the repo, as release.sh runs it.
step155() {
    (cd "$tmp/repo" && VERSION="$V" TAG="v$V" K2_SIGNER=k2sign K2_SKIP_WINDOWS_NSIS=1 bash -c \
        'set -euo pipefail; source "$1"; source "$2"; source "$3"; echo "approval step ok"' _ \
        "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" "$tmp/approval-block.sh")
}
# A throwaway repo; no global or system git config (no signing, no hooks).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
g() { git -C "$tmp/repo" -c user.name=k2sign-test -c user.email=k2sign-test@example.invalid -c commit.gpgsign=false "$@"; }
commit() { printf '%s\n' "$1" >> "$tmp/repo/fix.txt"; g add fix.txt && g commit -q -m "$1"; g rev-parse HEAD; }
mkdir -p "$tmp/repo"; git -C "$tmp/repo" init -q
C1="$(commit "the release candidate")"

"$ADM" approve 0.46.10 "$C1" K2.app.tar.gz >/dev/null 2>"$tmp/e" || fail "the previous release (0.46.10)" "$(cat "$tmp/e")"

echo "== try 1: the approval, then the run stops before anything is signed =="
check "Step 0.45's preflight (its self-test signature is not a release signature)" 0 "preflight ok" "$K" preflight "$V"
check "try 1: one approval" 0 "approval step ok" step155
[ "$(asks)" = 1 ] && pass "asked once" || fail "asks after try 1" "$(asks)"
[ "$(field "$SESS/$V.json" base_commit)" = "$C1" ] && pass "the session is bound to try 1's commit" || fail "session base" "$(cat "$SESS/$V.json")"

echo "== tries 2-6 (0.46.11 took six): a fix commit lands each time, the run is retried =="
prev="$C1"
for n in 2 3 4 5 6; do
    C="$(commit "fix for try $((n - 1))")"
    check "Step 0.45 again on try $n" 0 "preflight ok" "$K" preflight "$V"
    check "try $n on new code: the approval step passes (no reset)" 0 "approval step ok" step155
    grep -qF "RE-approve K2 $V for new code ${C:0:7} (the earlier approval for ${prev:0:7} was never used)" "$tmp/out" \
        && pass "try $n: the banner says RE-approve for new code ${C:0:7}" || fail "try $n banner" "$(cat "$tmp/out")"
    [ "$(asks)" = "$n" ] && pass "try $n: asked exactly once more ($n in all)" || fail "asks after try $n" "$(asks)"
    [ "$(python3 -I -c 'import json,sys; c=json.load(open(sys.argv[1])); print(c["replacement"], c["replaces_base"], c["base_commit"])' "$tmp/asked-$n.json")" = "True $prev $C" ] \
        && pass "try $n: the approved challenge names the replaced base and the new one" || fail "try $n challenge" "$(cat "$tmp/asked-$n.json")"
    [ "$(field "$SESS/$V.json" base_commit)" = "$C" ] && pass "try $n: the session now binds ${C:0:7}" || fail "try $n session" "$(cat "$SESS/$V.json")"
    prev="$C"
done

echo "== a retry on the same code asks nothing =="
check "try 7, no new commit: the approval step reuses the session" 0 "using $V's approved signing session" step155
[ "$(asks)" = 6 ] && pass "no prompt" || fail "asks after try 7" "$(asks)"

echo "== the release goes on: the session signs; the bump commit's resumed run reuses it =="
export K2SIGN_TEST_EVIDENCE=accept
printf 'linux daemon\n' > "$tmp/d1"
check_in "u2 signs an approved asset under the replaced session" "$tmp/d1" 0 "" "$K" sign u2 k2-daemon-linux-x86_64 "$V" raw
unset K2SIGN_TEST_EVIDENCE
LAST="$prev"
printf 'bump\n' >> "$tmp/repo/fix.txt"; g add fix.txt && g commit -q -m "chore(release): v$V"
check "K2_RELEASE_RESUME after Step 8.9: base = the commit under the bump, no prompt" 0 "using $V's approved signing session" step155
[ "$(asks)" = 6 ] && pass "no prompt on the resumed run" || fail "asks after resume" "$(asks)"
[ "$(field "$SESS/$V.json" base_commit)" = "$LAST" ] && pass "the session still binds ${LAST:0:7}" || fail "resume session" "$(cat "$SESS/$V.json")"

echo "== after a signature, new code is refused (a new version), with no prompt and no reset asked =="
C8="$(commit "a commit after something was signed")"
check "the run on new code stops loudly" 1 "so it is never replaced" step155
[ "$(asks)" = 6 ] && pass "no prompt" || fail "asks after the refusal" "$(asks)"
grep -qE "cancel-session|sudo" "$tmp/out" && fail "the refusal points at a sudo reset" "$(cat "$tmp/out")" || pass "the refusal names no sudo reset"
[ "$(field "$SESS/$V.json" base_commit)" = "$LAST" ] && pass "the signed session is unchanged" || fail "signed session changed" "$(cat "$SESS/$V.json")"

echo "== the audit log: six Touch ID approvals, five replacements, nothing by an admin =="
n_ev() { awk -F'\t' -v e="$1" '$2 == e' "$LOG" | wc -l | tr -d ' '; }
[ "$(n_ev APPROVED)" = 6 ] && pass "6 APPROVED" || fail "APPROVED count" "$(n_ev APPROVED): $(cat "$LOG")"
[ "$(n_ev SESSION-REPLACED)" = 5 ] && pass "5 SESSION-REPLACED" || fail "SESSION-REPLACED count" "$(n_ev SESSION-REPLACED)"
! grep -q -- '-BY-ADMIN' "$LOG" && pass "no admin (sudo) event for $V" || fail "admin events" "$(grep -- '-BY-ADMIN' "$LOG")"
[ "$(cd "$SESS" && ls "$V".json.replaced-* | wc -l | tr -d ' ')" = 5 ] && pass "the five replaced sessions are kept aside" || fail "aside files" "$(ls "$SESS")"
[ "$(tr -d '\n' < "$TROOT/var/k2sign/state/last_version")" = "$V" ] && pass "last_version $V" || fail "last_version"

finish
