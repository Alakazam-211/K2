#!/usr/bin/env bash
# k2sign-cancel-session.sh — `k2sign-admin cancel-session` (0.46.11: a gate
# failed after the approval, the fix commit's re-run wanted another base, and
# the only way out was a by-hand sudo mv in /var/k2sign/state plus a stale
# state/last_version). In test mode: macOS, throwaway keys, an openssl P-256
# key standing in for the Secure Enclave one.
#   bash tests/release/k2sign-cancel-session.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
NOW="$(date +%s)"
export K2SIGN_TEST_NOW="$NOW"
ASSETS=(K2.app.tar.gz k2-daemon-linux-aarch64 k2-daemon-linux-x86_64 k2-daemon-macos-aarch64)
STATE="$TROOT/var/k2sign/state"
SESS="$STATE/sessions"
CMD_LINE="sudo /usr/local/libexec/k2sign/k2sign-admin cancel-session"

# open_session <version> <base>: a Touch ID approval (challenge + approve).
open_session() {
    "$K" challenge "$1" "$2" "${ASSETS[@]}" > "$tmp/c.json" 2>"$tmp/c.err" || { echo "challenge: $(cat "$tmp/c.err")"; return 1; }
    approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/a.der"
    "$K" approve "$(nonce_of "$tmp/c.json")" < "$tmp/a.der" > /dev/null 2>"$tmp/a.err" || { echo "approve: $(cat "$tmp/a.err")"; return 1; }
}
field() { python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get(sys.argv[2], "<absent>")))' "$1" "$2"; }
last_v() { tr -d '\n' < "$STATE/last_version"; }
n_lines() { { grep -c "$1" "$2" || true; } | tr -d ' '; }

echo "== the session records the last_version before its first approval =="
"$ADM" approve 0.46.10 "$B1" K2.app.tar.gz >/dev/null 2>"$tmp/e" || fail "admin approve 0.46.10" "$(cat "$tmp/e")"
[ "$(field "$SESS/0.46.10.json" prev_last_version)" = null ] && pass "the first session ever: prev_last_version null" || fail "prev null" "$(field "$SESS/0.46.10.json" prev_last_version)"
r="$(open_session 0.46.11 "$B1")" && pass "0.46.11 approved on base B1" || fail "open 0.46.11" "$r"
[ "$(field "$SESS/0.46.11.json" prev_last_version)" = '"0.46.10"' ] && [ "$(last_v)" = 0.46.11 ] \
    && pass "prev_last_version 0.46.10, last_version 0.46.11" || fail "prev 0.46.10" "$(cat "$SESS/0.46.11.json") / $(last_v)"
# Step 0.45's preflight signs its self-test under the version; that is not a release signature.
check "the preflight self-test for 0.46.11 (SIGNED lines in the audit log)" 0 "preflight ok" "$K" preflight 0.46.11
grep -q $'\tSIGNED\tu2\tk2-preflight-self-test\t' "$AUDIT/0.46.11/signatures.tsv" && pass "the self-test is in the audit log" || fail "self-test audit"

echo "== the 0.46.11 case: a fix commit's run wants base B2 =="
check "a challenge for the same version, another base: refused" 3 "already has an approved session" "$K" challenge 0.46.11 "$B2" "${ASSETS[@]}"
sa() { bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" "$@"; }
helper="$tmp/helper.sh"
cat > "$helper" <<SH
#!/bin/sh
c=""; o=""
while [ \$# -gt 0 ]; do case "\$1" in --challenge) c="\$2"; shift ;; --out) o="\$2"; shift ;; esac; shift; done
/usr/bin/openssl dgst -sha256 -sign "$tmp/keys/approver.key" -out "\$o" "\$c"
SH
chmod +x "$helper"
export K2_SIGN_APPROVE_CMD="$helper"
check "sign-approve: the mismatch error prints the cancel command" 1 "$CMD_LINE 0.46.11" sa k2sa_open_session 0.46.11 "$B2" "${ASSETS[@]}"
grep -qF "./scripts/release.sh 0.46.11" "$tmp/out" && grep -qF "stopped before it built or signed anything" "$tmp/out" \
    && pass "…with the re-run line and when to use it" || fail "mismatch text" "$(cat "$tmp/out")"

echo "== cancel-session on the unused session =="
check "cancel-session 0.46.11" 0 "approval session cancelled" "$ADM" cancel-session 0.46.11
grep -qF "state/last_version is 0.46.10 again" "$tmp/out" && pass "it says last_version went back" || fail "restore message" "$(cat "$tmp/out")"
[ ! -e "$SESS/0.46.11.json" ] && pass "the session file is gone from its name" || fail "session still there"
aside="$(cd "$SESS" && ls 0.46.11.json.cancelled-* 2>/dev/null)"
[ "$(printf '%s\n' "$aside" | grep -c .)" = 1 ] && [ "$(field "$SESS/$aside" base_commit)" = "\"$B1\"" ] \
    && pass "…kept as sessions/$aside (evidence, not deleted)" || fail "aside file" "$(ls "$SESS")"
[ "$(last_v)" = 0.46.10 ] && pass "state/last_version is 0.46.10 again" || fail "last_version" "$(last_v)"
grep -q $'\tSESSION-CANCELLED-BY-ADMIN\t.*base '"$B1"'; .*moved to sessions/'"$aside" "$AUDIT/0.46.11/signatures.tsv" \
    && pass "audit: SESSION-CANCELLED-BY-ADMIN with the old base" || fail "cancel audit" "$(cat "$AUDIT/0.46.11/signatures.tsv")"
grep -q $'\tLAST-VERSION-RESTORED-BY-ADMIN\t.*last_version 0.46.11 -> 0.46.10 (recorded in the session)' "$AUDIT/0.46.11/signatures.tsv" \
    && pass "audit: LAST-VERSION-RESTORED-BY-ADMIN 0.46.11 -> 0.46.10" || fail "restore audit" "$(cat "$AUDIT/0.46.11/signatures.tsv")"
check "cancelling again: nothing to cancel" 3 "0.46.11 has no approval session to cancel" "$ADM" cancel-session 0.46.11

echo "== after the cancel: a fresh approval for the SAME version on the new base =="
check "the re-run (k2sa_open_session, base B2) gets one approval" 0 "signing session" sa k2sa_open_session 0.46.11 "$B2" "${ASSETS[@]}"
[ "$(field "$SESS/0.46.11.json" base_commit)" = "\"$B2\"" ] && [ "$(field "$SESS/0.46.11.json" prev_last_version)" = '"0.46.10"' ] \
    && [ "$(last_v)" = 0.46.11 ] && pass "the new session: base B2, prev_last_version 0.46.10, last_version 0.46.11" \
    || fail "new session" "$(cat "$SESS/0.46.11.json") / $(last_v)"

echo "== refused once anything was signed =="
export K2SIGN_TEST_EVIDENCE=accept
printf 'linux daemon\n' > "$tmp/d1"
check_in "u2 signs an approved asset under 0.46.11" "$tmp/d1" 0 "" "$K" sign u2 k2-daemon-linux-x86_64 0.46.11 raw
unset K2SIGN_TEST_EVIDENCE
cp "$SESS/0.46.11.json" "$tmp/sess-before.json"
n_cancel="$(n_lines SESSION-CANCELLED "$AUDIT/0.46.11/signatures.tsv")"
check "cancel-session after a signature" 3 "0.46.11 has signatures" "$ADM" cancel-session 0.46.11
grep -qF "never cancelled" "$tmp/out" && pass "…says a session that signed anything is never cancelled" || fail "signed text" "$(cat "$tmp/out")"
cmp -s "$SESS/0.46.11.json" "$tmp/sess-before.json" && [ "$(last_v)" = 0.46.11 ] \
    && [ "$(n_lines SESSION-CANCELLED "$AUDIT/0.46.11/signatures.tsv")" = "$n_cancel" ] \
    && pass "…and changed nothing (session, last_version, no audit line)" || fail "refusal changed state"
check "…the explicit restore flag doesn't get past it" 3 "has signatures" "$ADM" cancel-session 0.46.11 --restore-last-version 0.46.10
check "sign-approve: a signed session's mismatch says it can't be cancelled" 1 "so it can't be cancelled" sa k2sa_open_session 0.46.11 "$B1" "${ASSETS[@]}"
grep -qF "$CMD_LINE" "$tmp/out" && fail "no cancel command for a signed session" "$(cat "$tmp/out")" || pass "…and prints no cancel command"
# A re-approval starts the session's count at 0; the audit log still knows.
K2SIGN_TEST_NOW=$((NOW + 13 * 3600)) expect 0 "RE-APPROVE" sa k2sa_open_session 0.46.11 "$B2" "${ASSETS[@]}" > "$tmp/r" \
    && pass "an expired session re-approved (count back to 0)" || fail "reapprove" "$(cat "$tmp/r")"
[ "$(field "$SESS/0.46.11.json" count)" = 0 ] && [ "$(field "$SESS/0.46.11.json" prev_last_version)" = '"0.46.10"' ] \
    && pass "…the re-approval carries prev_last_version over" || fail "reapproval fields" "$(cat "$SESS/0.46.11.json")"
check "cancel-session after a re-approval: the audit log's SIGNED line still refuses" 3 "audit: " "$ADM" cancel-session 0.46.11
# The registry alone (one-sha-per-asset) refuses too.
"$ADM" approve 0.46.12 "$B1" K2.app.tar.gz >/dev/null 2>"$tmp/e" || fail "admin approve 0.46.12" "$(cat "$tmp/e")"
printf 'K2.app.tar.gz\t%s\n' "$(printf '0%.0s' $(seq 64))" > "$STATE/assets/0.46.12.tsv"
check "an asset in the registry refuses" 3 "registry: K2.app.tar.gz" "$ADM" cancel-session 0.46.12
rm "$STATE/assets/0.46.12.tsv"
mkdir -p "$STATE/compute"; printf '{"summary": {}}\n' > "$STATE/compute/0.46.12.json"
check "registered compute evidence refuses" 3 "has K2 Compute evidence registered" "$ADM" cancel-session 0.46.12
rm "$STATE/compute/0.46.12.json"
check "with those gone, 0.46.12 cancels (last_version back to 0.46.11)" 0 "state/last_version is 0.46.11 again" "$ADM" cancel-session 0.46.12

echo "== a session from before prev_last_version: the admin names it, never guessed =="
"$ADM" approve 0.46.20 "$B1" K2.app.tar.gz >/dev/null 2>"$tmp/e" || fail "admin approve 0.46.20" "$(cat "$tmp/e")"
python3 -I - "$SESS/0.46.20.json" <<'PY'
import json, sys
p = sys.argv[1]; s = json.load(open(p)); del s["prev_last_version"]; open(p, "w").write(json.dumps(s, sort_keys=True))
PY
check "no record, no flag: refused" 3 "name the version before it" "$ADM" cancel-session 0.46.20
[ -e "$SESS/0.46.20.json" ] && [ "$(last_v)" = 0.46.20 ] && pass "…nothing changed" || fail "legacy refusal changed state"
check "--restore-last-version equal to the version" 3 "must be lower than 0.46.20" "$ADM" cancel-session 0.46.20 --restore-last-version 0.46.20
check "--restore-last-version above it" 3 "must be lower than 0.46.20" "$ADM" cancel-session 0.46.20 --restore-last-version 0.47.0
check "--restore-last-version not a version" 2 "bad version" "$ADM" cancel-session 0.46.20 --restore-last-version latest
check "--restore-last-version 0.46.11" 0 "state/last_version is 0.46.11 again" "$ADM" cancel-session 0.46.20 --restore-last-version 0.46.11
grep -q $'\tLAST-VERSION-RESTORED-BY-ADMIN\t.*0.46.20 -> 0.46.11 (named by the admin)' "$AUDIT/0.46.20/signatures.tsv" \
    && pass "audit: the restore says the admin named it" || fail "admin restore audit" "$(cat "$AUDIT/0.46.20/signatures.tsv")"
r="$(open_session 0.46.20 "$B2")" && pass "a fresh approval for 0.46.20 works" || fail "fresh 0.46.20" "$r"
check "a flag that disagrees with the session's record" 3 "the session records last_version 0.46.11" "$ADM" cancel-session 0.46.20 --restore-last-version 0.46.10

echo "== a session moved aside by hand (the 0.46.11 sudo mv): only last_version is left =="
mv "$SESS/0.46.20.json" "$tmp/by-hand.json"
check "the stale last_version refuses a new approval" 3 "0.46.20 is not newer than 0.46.20" "$K" challenge 0.46.20 "$B2" "${ASSETS[@]}"
check "cancel-session without a session or flag" 3 "no approval session to cancel" "$ADM" cancel-session 0.46.20
check "cancel-session --restore-last-version 0.46.11 puts it back" 0 "state/last_version is 0.46.11 again" "$ADM" cancel-session 0.46.20 --restore-last-version 0.46.11
r="$(open_session 0.46.20 "$B2")" && pass "…and a fresh approval works" || fail "fresh after by-hand" "$r"

echo "== other refusals =="
check "a flag when last_version is another version" 3 "nothing to restore" "$ADM" cancel-session 0.46.12 --restore-last-version 0.46.11
check "no session at all" 3 "0.46.40 has no approval session to cancel" "$ADM" cancel-session 0.46.40
check "no version is usage" 2 "cancel-session <version>" "$ADM" cancel-session
check "an unknown flag is usage" 2 "unknown flag --force" "$ADM" cancel-session 0.46.20 --force
check "a bad version is usage" 2 "bad version" "$ADM" cancel-session ../0.46.20
# Never through a symlink.
mv "$SESS/0.46.20.json" "$tmp/real.json"
ln -s "$tmp/real.json" "$SESS/0.46.20.json"
check "a symlinked session file is refused" 3 "a symlink is refused" "$ADM" cancel-session 0.46.20
[ -L "$SESS/0.46.20.json" ] && [ -f "$tmp/real.json" ] && pass "…and nothing moved" || fail "symlink moved"
rm "$SESS/0.46.20.json"; mv "$tmp/real.json" "$SESS/0.46.20.json"
check "the verb is listed among the admin verbs" 2 "cancel-session" "$ADM" no-such-verb

finish
