#!/usr/bin/env bash
# k2sign-replace-session.sh — a new base or asset list for a version whose
# earlier approval signed nothing: one Touch ID replaces it, no sudo
# (0.46.11: a gate failed after the approval, the fix commit's run wanted
# another base). The person still approves through K2 Release Approve; the
# signer refuses once anything was signed. Test mode: macOS, throwaway keys,
# an openssl P-256 key standing in for the Secure Enclave one, and the real
# app built with swiftc for its sheet text (--describe).
#   bash tests/release/k2sign-replace-session.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
NOW="$(date +%s)"
export K2SIGN_TEST_NOW="$NOW"
ASSETS=(K2.app.tar.gz k2-daemon-linux-aarch64 k2-daemon-linux-x86_64 k2-daemon-macos-aarch64)
STATE="$TROOT/var/k2sign/state"
SESS="$STATE/sessions"
B3="$(printf 'c%.0s' $(seq 40))"
APPBIN="$tmp/approve"
/usr/bin/swiftc -O "$REPO/scripts/k2sign/approve/main.swift" -o "$APPBIN" 2>"$tmp/swift.err" \
    || { echo "FAIL: swiftc could not build K2 Release Approve: $(head -5 "$tmp/swift.err")" >&2; exit 1; }

field() { python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get(sys.argv[2], "<absent>")))' "$1" "$2"; }
last_v() { tr -d '\n' < "$STATE/last_version"; }
sa() { bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" "$@"; }
helper="$tmp/helper.sh"
cat > "$helper" <<SH
#!/bin/sh
# K2 Release Approve stand-in: keeps each challenge it was shown, signs it with the test approver key.
c=""; o=""
while [ \$# -gt 0 ]; do case "\$1" in --challenge) c="\$2"; shift ;; --out) o="\$2"; shift ;; esac; shift; done
n=\$(( \$(cat "$tmp/calls" 2>/dev/null || echo 0) + 1 )); echo "\$n" > "$tmp/calls"
cp "\$c" "$tmp/shown-\$n.json"
/usr/bin/openssl dgst -sha256 -sign "$tmp/keys/approver.key" -out "\$o" "\$c"
SH
chmod +x "$helper"
export K2SA_HELPER="$helper"
calls() { cat "$tmp/calls" 2>/dev/null || echo 0; }

echo "== the app: the replacement sheet text and capability =="
"$APPBIN" --capabilities | grep -q " replace-session" && pass "K2 Release Approve lists replace-session" || fail "capability"
printf '{"v":1,"kind":"k2-release-approval","version":"0.46.11","base_commit":"%s","assets":["K2.app.tar.gz"],"slots":["u1","u2"],"windows":false,"reapproval":false,"replacement":true,"replaces_base":"%s","max_signatures":6,"expires":"2027-01-01T00:00:00Z","nonce":"00000000000000000000000000000000","issued":"2027-01-01T00:00:00Z"}' "$B2" "$B1" > "$tmp/r.json"
check "the sheet: RE-approve for new code, naming the unused earlier approval" 0 \
    "RE-approve K2 0.46.11 for new code bbbbbbb (the earlier approval for aaaaaaa was never used): approve K2 release 0.46.11 at base commit $B2: sign K2.app.tar.gz until" \
    "$APPBIN" --describe "$tmp/r.json"
sed 's/"replaces_base":"[a-f]*"/"replaces_base":"aaaa"/' "$tmp/r.json" > "$tmp/r-short.json"
check "a replaces_base that isn't a whole commit is not a challenge" 2 "not a K2 release approval" "$APPBIN" --describe "$tmp/r-short.json"
sed 's/"reapproval":false/"reapproval":true/' "$tmp/r.json" > "$tmp/r-both.json"
check "replacement and reapproval together is not a challenge" 2 "not a K2 release approval" "$APPBIN" --describe "$tmp/r-both.json"

echo "== an unused session, a new base: replaced with one approval =="
"$ADM" approve 0.46.10 "$B1" K2.app.tar.gz >/dev/null 2>"$tmp/e" || fail "admin approve 0.46.10" "$(cat "$tmp/e")"
check "0.46.11 approved on base B1" 0 "signing session" sa k2sa_open_session 0.46.11 "$B1" "${ASSETS[@]}"
check "Step 0.45's preflight self-test for 0.46.11 (not a release signature)" 0 "preflight ok" "$K" preflight 0.46.11
check "status: unused" 0 '"unused": true' "$K" status 0.46.11
check "challenge --replace with the same base and assets: nothing to replace" 3 "nothing to replace" "$K" challenge 0.46.11 "$B1" "${ASSETS[@]}" --replace
check "--replace with --reapprove is usage" 2 "don't go together" "$K" challenge 0.46.11 "$B2" "${ASSETS[@]}" --replace --reapprove
check "admin approve takes no --replace (Touch ID only)" 2 "Touch ID approval only" "$ADM" approve 0.46.11 "$B2" "${ASSETS[@]}" --replace
check "without --replace the signer still refuses, and names it" 3 "or --replace for a new base" "$K" challenge 0.46.11 "$B2" "${ASSETS[@]}"
check "the fix commit's run (base B2): a replacement, one approval" 0 "signing session" sa k2sa_open_session 0.46.11 "$B2" "${ASSETS[@]}"
grep -qF "RE-approve K2 0.46.11 for new code bbbbbbb (the earlier approval for aaaaaaa was never used)" "$tmp/out" \
    && pass "the banner says RE-approve for new code, naming the unused one" || fail "banner" "$(cat "$tmp/out")"
[ "$(calls)" = 2 ] && pass "the approval app ran once for it (2 in all)" || fail "helper calls" "$(calls)"
check "the challenge it signed carries the replacement label" 0 \
    "RE-approve K2 0.46.11 for new code bbbbbbb (the earlier approval for aaaaaaa was never used)" "$APPBIN" --describe "$tmp/shown-2.json"
[ "$(field "$tmp/shown-2.json" replacement)" = true ] && [ "$(field "$tmp/shown-2.json" replaces_base)" = "\"$B1\"" ] \
    && pass "challenge JSON: replacement true, replaces_base B1" || fail "challenge fields" "$(cat "$tmp/shown-2.json")"
[ "$(field "$SESS/0.46.11.json" base_commit)" = "\"$B2\"" ] && [ "$(field "$SESS/0.46.11.json" prev_last_version)" = '"0.46.10"' ] \
    && [ "$(last_v)" = 0.46.11 ] && pass "new session on B2; prev_last_version 0.46.10 carried over; last_version stays 0.46.11" \
    || fail "new session" "$(cat "$SESS/0.46.11.json") / $(last_v)"
aside="$(cd "$SESS" && ls 0.46.11.json.replaced-* 2>/dev/null)"
[ "$(printf '%s\n' "$aside" | grep -c .)" = 1 ] && [ "$(field "$SESS/$aside" base_commit)" = "\"$B1\"" ] \
    && [ "$(stat -f %l "$SESS/$aside")" = 1 ] && [ "$(stat -f %l "$SESS/0.46.11.json")" = 1 ] \
    && pass "the old session is kept as sessions/$aside (one link each)" || fail "aside" "$(ls -l "$SESS")"
grep -q $'\tSESSION-REPLACED\t.*old base '"$B1"'; new base '"$B2"'; old session .*kept as sessions/'"$aside" "$AUDIT/0.46.11/signatures.tsv" \
    && pass "audit: SESSION-REPLACED with the old and new bases" || fail "replace audit" "$(cat "$AUDIT/0.46.11/signatures.tsv")"
check "a resumed run on B2 reuses the new session (no prompt)" 0 "using 0.46.11's approved signing session" sa k2sa_open_session 0.46.11 "$B2" "${ASSETS[@]}"
[ "$(calls)" = 2 ] && pass "no prompt" || fail "helper calls" "$(calls)"
check "another asset list on the same base: replaced too" 0 "signing session" sa k2sa_open_session 0.46.11 "$B2" K2.app.tar.gz k2-daemon-macos-aarch64
[ "$(field "$SESS/0.46.11.json" assets)" = '["K2.app.tar.gz", "k2-daemon-macos-aarch64"]' ] && pass "…the new asset list" || fail "assets" "$(cat "$SESS/0.46.11.json")"

echo "== the session changed between the challenge and the approval =="
"$K" challenge 0.46.11 "$B3" "${ASSETS[@]}" --replace > "$tmp/c3.json" 2>"$tmp/e" || fail "challenge B3" "$(cat "$tmp/e")"
check "a second replacement (B1) wins first" 0 "signing session" sa k2sa_open_session 0.46.11 "$B1" "${ASSETS[@]}"
approve_with "$tmp/keys/approver.key" "$tmp/c3.json" "$tmp/c3.der"
check_in "the B3 challenge named the replaced session's base: refused now" "$tmp/c3.der" 3 "changed since this replacement challenge" "$K" approve "$(nonce_of "$tmp/c3.json")"
[ "$(field "$SESS/0.46.11.json" base_commit)" = "\"$B1\"" ] && pass "…the session is still B1" || fail "B3 took over"

echo "== refused after a signature =="
"$K" challenge 0.46.11 "$B2" "${ASSETS[@]}" --replace > "$tmp/c4.json" 2>"$tmp/e" || fail "challenge before the signature" "$(cat "$tmp/e")"
export K2SIGN_TEST_EVIDENCE=accept
printf 'linux daemon\n' > "$tmp/d1"
check_in "u2 signs an approved asset under 0.46.11" "$tmp/d1" 0 "" "$K" sign u2 k2-daemon-linux-x86_64 0.46.11 raw
unset K2SIGN_TEST_EVIDENCE
check "status: not unused" 0 '"unused": false' "$K" status 0.46.11
check "challenge --replace after the signature" 3 "earlier approval was used" "$K" challenge 0.46.11 "$B2" "${ASSETS[@]}" --replace
approve_with "$tmp/keys/approver.key" "$tmp/c4.json" "$tmp/c4.der"
check_in "a replacement challenge issued before the signature: refused at approve" "$tmp/c4.der" 3 "earlier approval was used" "$K" approve "$(nonce_of "$tmp/c4.json")"
n="$(calls)"
check "sign-approve: the new base is refused as today" 1 "so it is never replaced" sa k2sa_open_session 0.46.11 "$B2" "${ASSETS[@]}"
[ "$(calls)" = "$n" ] && [ "$(field "$SESS/0.46.11.json" base_commit)" = "\"$B1\"" ] \
    && pass "…no prompt, the session unchanged" || fail "refusal prompted or changed" "$(calls)"
# A re-approval resets the session's count; the audit log still refuses.
K2SIGN_TEST_NOW=$((NOW + 13 * 3600)) expect 0 "RE-APPROVE" sa k2sa_open_session 0.46.11 "$B1" "${ASSETS[@]}" > "$tmp/r" \
    && pass "an expired session re-approved (count back to 0)" || fail "reapprove" "$(cat "$tmp/r")"
K2SIGN_TEST_NOW=$((NOW + 13 * 3600)) expect 3 "audit: " "$K" challenge 0.46.11 "$B2" "${ASSETS[@]}" --replace > "$tmp/r" \
    && pass "…and --replace still refuses on the audit log's SIGNED line" || fail "replace after reapproval" "$(cat "$tmp/r")"
# Compute evidence alone refuses too.
check "0.46.12 approved" 0 "signing session" sa k2sa_open_session 0.46.12 "$B1" "${ASSETS[@]}"
mkdir -p "$STATE/compute"; printf '{"summary": {}}\n' > "$STATE/compute/0.46.12.json"
check "registered compute evidence refuses a replacement" 3 "K2 Compute evidence registered" "$K" challenge 0.46.12 "$B2" "${ASSETS[@]}" --replace
rm "$STATE/compute/0.46.12.json"

echo "== no session file (moved aside by hand), last_version is this version =="
mv "$SESS/0.46.12.json" "$tmp/by-hand.json"
check "a plain challenge: not newer (as on 0.46.11 try 3)" 3 "0.46.12 is not newer than 0.46.12" "$K" challenge 0.46.12 "$B2" "${ASSETS[@]}"
"$K" challenge 0.46.12 "$B2" "${ASSETS[@]}" --replace > "$tmp/c5.json" 2>"$tmp/e" || fail "replace with no session" "$(cat "$tmp/e")"
[ "$(field "$tmp/c5.json" replaces_base)" = null ] && pass "the challenge: replaces_base null" || fail "replaces_base" "$(cat "$tmp/c5.json")"
check "its sheet text" 0 "RE-approve K2 0.46.12 for new code bbbbbbb (an earlier approval of 0.46.12 was never used): approve K2 release 0.46.12 at base commit $B2" \
    "$APPBIN" --describe "$tmp/c5.json"
check "sign-approve: the run gets one approval" 0 "signing session" sa k2sa_open_session 0.46.12 "$B2" "${ASSETS[@]}"
grep -qF "RE-approve K2 0.46.12 for new code bbbbbbb (an earlier approval of 0.46.12 was never used)" "$tmp/out" \
    && pass "…with the RE-approve banner" || fail "banner" "$(cat "$tmp/out")"
[ "$(field "$SESS/0.46.12.json" base_commit)" = "\"$B2\"" ] && [ "$(field "$SESS/0.46.12.json" prev_last_version)" = '"<absent>"' ] \
    && [ "$(last_v)" = 0.46.12 ] && pass "new session on B2 (no prev_last_version to carry), last_version 0.46.12" \
    || fail "hand-moved session" "$(cat "$SESS/0.46.12.json") / $(last_v)"
grep -q $'\tSESSION-REPLACED\t.*old base unknown; new base '"$B2"'; no session file' "$AUDIT/0.46.12/signatures.tsv" \
    && pass "audit: SESSION-REPLACED (no session file)" || fail "audit" "$(cat "$AUDIT/0.46.12/signatures.tsv")"
# Moved aside by hand after a signature: never.
check "0.46.13 approved" 0 "signing session" sa k2sa_open_session 0.46.13 "$B1" "${ASSETS[@]}"
export K2SIGN_TEST_EVIDENCE=accept
check_in "u2 signs under 0.46.13" "$tmp/d1" 0 "" "$K" sign u2 k2-daemon-linux-x86_64 0.46.13 raw
unset K2SIGN_TEST_EVIDENCE
mv "$SESS/0.46.13.json" "$tmp/by-hand-13.json"
check "no session, signed: --replace refused" 3 "earlier approval was used" "$K" challenge 0.46.13 "$B2" "${ASSETS[@]}" --replace
check "sign-approve: no replacement asked; the signer refuses the plain approval" 1 "0.46.13 is not newer than 0.46.13" sa k2sa_open_session 0.46.13 "$B2" "${ASSETS[@]}"
check "--replace for a version never approved" 3 "no earlier approval to replace" "$K" challenge 0.46.14 "$B2" "${ASSETS[@]}" --replace

echo "== tests can't reach the app, open or osascript =="
# HOME is an empty folder: even if the guard broke, there is no app to open.
mkdir -p "$tmp/nohome"
check "test mode with no helper and no open stub: refused before the app" 1 "refusing to open the approval app" \
    env -u K2SA_HELPER -u K2_SIGN_APPROVE_CMD -u K2SA_OPEN HOME="$tmp/nohome" bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ \
    "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" k2sa_run_helper "$tmp/none.json" "$tmp/none.der" 0.46.99
check "…and with the open stub but no K2_SIGN_APPROVE_APP" 1 "refusing to open the approval app" \
    env -u K2SA_HELPER -u K2_SIGN_APPROVE_CMD -u K2_SIGN_APPROVE_APP HOME="$tmp/nohome" bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ \
    "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" k2sa_run_helper "$tmp/none.json" "$tmp/none.der" 0.46.99
# (Checked by reading the code: running k2sa_notify unstubbed would post one.)
grep -q '^    \[ "${K2SA_NOTIFY:-1}" = 0 \] && return 0$' "$TREE/scripts/lib/sign-approve.sh" \
    && [ "$(grep -v '^[[:space:]]*#' "$TREE/scripts/lib/sign-approve.sh" | grep -c '/usr/bin/osascript')" = 1 ] \
    && [ "$(grep -v '^[[:space:]]*#' "$TREE/scripts/lib/sign-approve.sh" | grep -cE '/usr/bin/open([^a-z]|$)')" = 1 ] \
    && grep -q 'open_cmd="${K2SA_OPEN:-/usr/bin/open}"' "$TREE/scripts/lib/sign-approve.sh" \
    && pass "sign-approve.sh: one osascript (behind K2SA_NOTIFY), one /usr/bin/open (behind K2SA_OPEN)" || fail "guard wiring"

echo "== an approval app without replace-session is refused for a replacement =="
old="$tmp/Old Approve.app/Contents/MacOS"; mkdir -p "$old"
printf '#!/bin/sh\n[ "$1" = --capabilities ] && echo "k2-release-approve 2 timeout replace key-dir describe" && exit 0\nexit 7\n' > "$old/K2 Release Approve"
chmod +x "$old/K2 Release Approve"
check "0.46.14 approved" 0 "signing session" sa k2sa_open_session 0.46.14 "$B1" "${ASSETS[@]}"
env -u K2SA_HELPER -u K2_SIGN_APPROVE_CMD K2_SIGN_APPROVE_APP="$tmp/Old Approve.app" bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ \
    "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" k2sa_open_session 0.46.14 "$B2" "${ASSETS[@]}" > "$tmp/r" 2>&1 && rc=0 || rc=$?
[ "$rc" = 1 ] && grep -q "too old (no replace-session" "$tmp/r" && grep -q "bash scripts/k2sign/approve/build.sh" "$tmp/r" \
    && pass "an old app: refused before opening, with the rebuild line" || fail "old app" "rc=$rc $(cat "$tmp/r")"
[ "$(field "$SESS/0.46.14.json" base_commit)" = "\"$B1\"" ] && pass "…the session unchanged" || fail "old app changed the session"

finish
