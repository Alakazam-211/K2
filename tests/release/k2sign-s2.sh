#!/usr/bin/env bash
# k2sign-s2.sh — prd-signing-user-v1 S2 acceptance (the approval), plus the
# release side through the signer (sign-updater.sh K2_SIGNER=k2sign,
# sign-approve.sh), in test mode: macOS, throwaway keys, an openssl P-256
# key standing in for the Secure Enclave one (the signer can't tell: both
# are DER ECDSA over SHA-256, S0 (c)). The real Touch ID run is S2-6, in
# the sitting.
#   bash tests/release/k2sign-s2.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
NOW="$(date +%s)"
export K2SIGN_TEST_NOW="$NOW"
ASSETS=(K2.app.tar.gz k2-daemon-linux-aarch64 k2-daemon-linux-x86_64 k2-daemon-macos-aarch64)
U2_ID="$(python3 -I -c 'import base64,sys; print(base64.b64decode(sys.argv[1])[2:10][::-1].hex().upper())' "$U2_LINE")"
/usr/bin/openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/keys/other.key" 2>/dev/null
printf 'x\nx\n' | "$MINISIGN" -G -p "$tmp/keys/other.pub" -s "$tmp/keys/other.minisign.key" >/dev/null 2>&1

# challenge_ok <version> <base> <args...> → $tmp/c.json
challenge_ok() {
    "$K" challenge "$@" > "$tmp/c.json" 2>"$tmp/c.err" || { echo "challenge failed: $(cat "$tmp/c.err")" >&2; return 1; }
}

echo "== a challenge is the signer's own canonical JSON =="
challenge_ok 0.46.6 "$B1" "${ASSETS[@]}"
python3 -I - "$tmp/c.json" "$B1" <<'PY' && pass "challenge fields: version, base, sorted assets, slots, windows false, max, 12 h expiry, nonce" || fail "challenge fields" "$(cat "$tmp/c.json")"
import json, sys
t = open(sys.argv[1]).read(); c = json.loads(t)
assert t == json.dumps(c, sort_keys=True, separators=(",", ":")), "not canonical"
assert c["kind"] == "k2-release-approval" and c["version"] == "0.46.6" and c["base_commit"] == sys.argv[2]
assert c["assets"] == sorted(c["assets"]) and len(c["assets"]) == 4 and c["slots"] == ["u1", "u2"]
assert c["windows"] is False and c["reapproval"] is False and c["max_signatures"] == 12
assert len(c["nonce"]) == 32
PY
N1="$(nonce_of "$tmp/c.json")"

echo "== S2-1: an approval by another key =="
approve_with "$tmp/keys/other.key" "$tmp/c.json" "$tmp/bad.der"
check_in "S2-1 another P-256 key's signature" "$tmp/bad.der" 3 "approval signature does not verify" "$K" approve "$N1"
grep -q $'\tREFUSED-APPROVAL\t' "$AUDIT/0.46.6/signatures.tsv" && pass "the refused approval is in the audit log" || fail "refused-approval audit"

echo "== an approval for the wrong version or assets (a challenge edited after issue) =="
python3 -I -c 'import sys; t=open(sys.argv[1]).read(); open(sys.argv[2],"w").write(t.replace("0.46.6","0.47.0"))' "$tmp/c.json" "$tmp/c-ver.json"
approve_with "$tmp/keys/approver.key" "$tmp/c-ver.json" "$tmp/ver.der"
check_in "a signature over another version's text" "$tmp/ver.der" 3 "approval signature does not verify" "$K" approve "$N1"
python3 -I -c 'import sys; t=open(sys.argv[1]).read(); open(sys.argv[2],"w").write(t.replace("\"K2.app.tar.gz\",",""))' "$tmp/c.json" "$tmp/c-as.json"
approve_with "$tmp/keys/approver.key" "$tmp/c-as.json" "$tmp/as.der"
check_in "a signature over another asset list" "$tmp/as.der" 3 "approval signature does not verify" "$K" approve "$N1"
: > "$tmp/empty"
check_in "no signature at all" "$tmp/empty" 3 "no approval signature on stdin" "$K" approve "$N1"
check "a nonce the signer never issued" 3 "unknown challenge" "$K" approve "0123456789abcdef0123456789abcdef"
check "a malformed nonce is usage" 2 "approve <nonce>" "$K" approve "../../etc/passwd"

echo "== the right approval opens the session =="
approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/good.der"
check_in "approve" "$tmp/good.der" 0 "approved until" "$K" approve "$N1"
check "S2-6 (test mode) status shows the session" 0 '"valid": true' "$K" status 0.46.6
python3 -I - "$tmp/out" "$NOW" <<'PY' && pass "status: the approved asset list, base and a 12 h expiry" || fail "status fields" "$(cat "$tmp/out")"
import json, sys, time, calendar
d = json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])
s = d["session"]
assert s["assets"] == ["K2.app.tar.gz", "k2-daemon-linux-aarch64", "k2-daemon-linux-x86_64", "k2-daemon-macos-aarch64"]
exp = calendar.timegm(time.strptime(s["expires"], "%Y-%m-%dT%H:%M:%SZ"))
assert exp == int(sys.argv[2]) + 12 * 3600, (exp, sys.argv[2])
assert d["last_version"] == "0.46.6" and s["by"] == "touchid"
PY
grep -q $'\tAPPROVED\t' "$AUDIT/0.46.6/signatures.tsv" && pass "APPROVED is in the audit log" || fail "approved audit"

echo "== S2-2: single use, 15 minutes =="
check_in "S2-2 reusing a nonce" "$tmp/good.der" 3 "challenge already used" "$K" approve "$N1"
challenge_ok 0.46.7 "$B1" K2.app.tar.gz
N2="$(nonce_of "$tmp/c.json")"; approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/n2.der"
K2SIGN_TEST_NOW=$((NOW + 16 * 60)) expect 3 "challenge expired" "$K" approve "$N2" < "$tmp/n2.der" > "$tmp/r" \
    && pass "S2-2 an expired challenge (16 min)" || fail "S2-2 expired" "$(cat "$tmp/r")"

echo "== S2-3: versions only go up =="
check "S2-3 a challenge for an older version" 3 "0.46.5 is not newer than 0.46.6" "$K" challenge 0.46.5 "$B1" K2.app.tar.gz
check "admin break-glass can't go back either" 3 "0.46.5 is not newer than 0.46.6" "$ADM" approve 0.46.5 "$B1" K2.app.tar.gz

echo "== S2-4: one session per version =="
check "S2-4 a second approval without --reapprove" 3 "already has an approved session" "$K" challenge 0.46.6 "$B1" "${ASSETS[@]}"
check "S2-4 --reapprove with another base commit" 3 "base commit differs" "$K" challenge 0.46.6 "$B2" "${ASSETS[@]}" --reapprove
check "--reapprove with no earlier session" 3 "no earlier session" "$K" challenge 0.46.9 "$B1" K2.app.tar.gz --reapprove

echo "== signing inside the session (the S3 checks stubbed by the test) =="
export K2SIGN_TEST_EVIDENCE=accept
printf 'linux daemon v1\n' > "$tmp/d1"; printf 'linux daemon v2 (swapped)\n' > "$tmp/d2"
check_in "u2 signs an approved asset" "$tmp/d1" 0 "" "$K" sign u2 k2-daemon-linux-x86_64 0.46.6 raw
cp "$tmp/out" "$tmp/d1.sig2"
"$MINISIGN" -V -P "$U2_LINE" -m "$tmp/d1" -x "$tmp/d1.sig2" -q && pass "it verifies with the pinned U2" || fail "u2 verify"
check_in "u1 signs the same bytes (b64)" "$tmp/d1" 0 "" "$K" sign u1 k2-daemon-linux-x86_64 0.46.6 b64
python3 -I -c 'import base64,sys; sys.stdout.write(base64.b64decode(open(sys.argv[1]).read()).decode())' "$tmp/out" > "$tmp/d1.sig"
"$MINISIGN" -V -P "$U1_LINE" -m "$tmp/d1" -x "$tmp/d1.sig" -q && pass "the b64 U1 signature decodes and verifies with the pinned U1" || fail "u1 verify"
grep -q "file:k2-daemon-linux-x86_64$" "$tmp/d1.sig" && ! grep -q "version:" "$tmp/d1.sig" \
    && pass "U1's trusted comment: file only (the bridge form)" || fail "u1 comment" "$(sed -n 3p "$tmp/d1.sig")"
grep -q $'\tfile:k2-daemon-linux-x86_64\tversion:0.46.6$' "$tmp/d1.sig2" && pass "U2's trusted comment: file + version" || fail "u2 comment"
check_in "an asset outside the session's list" "$tmp/d1" 3 "is not in 0.46.6's approved asset list" "$K" sign u1 setup.exe 0.46.6 raw
check_in "another version than the session's" "$tmp/d1" 3 "no approved signing session for 0.46.8" "$K" sign u2 k2-daemon-linux-x86_64 0.46.8 raw
check_in "REFUSED-SWAP: same asset, other bytes" "$tmp/d2" 3 "REFUSED-SWAP: k2-daemon-linux-x86_64 already signed as $(shasum -a 256 "$tmp/d1" | awk '{print $1}')" "$K" sign u2 k2-daemon-linux-x86_64 0.46.6 raw
grep -q $'\tREFUSED-SWAP\tu2\tk2-daemon-linux-x86_64\t' "$AUDIT/0.46.6/signatures.tsv" && pass "REFUSED-SWAP is in the audit log" || fail "swap audit"
before="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["count"])' "$TROOT/var/k2sign/state/sessions/0.46.6.json")"
check_in "re-signing the same bytes" "$tmp/d1" 0 "" "$K" sign u2 k2-daemon-linux-x86_64 0.46.6 raw
after="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["count"])' "$TROOT/var/k2sign/state/sessions/0.46.6.json")"
[ "$before" = "$after" ] && pass "re-signing the same bytes doesn't count against the limit" || fail "count" "$before -> $after"
K2SIGN_TEST_NOW=$((NOW + 12 * 3600 + 60)) expect 3 "the signing session for 0.46.6 expired" "$K" sign u2 k2-daemon-linux-aarch64 0.46.6 raw < "$tmp/d1" > "$tmp/r" \
    && pass "an expired session (12 h + 1 min)" || fail "expired session" "$(cat "$tmp/r")"
# The limit: a one-asset session allows 1×2+4 = 6 signatures.
"$ADM" approve 0.46.10 "$B1" k2-daemon-macos-aarch64 >/dev/null 2>"$tmp/e" || fail "admin approve" "$(cat "$tmp/e")"
grep -q $'\tAPPROVED-BY-ADMIN\t' "$AUDIT/0.46.10/signatures.tsv" && pass "break-glass: APPROVED-BY-ADMIN in the audit log" || fail "admin audit"
printf 'mac daemon\n' > "$tmp/m"
python3 -I - "$TROOT/var/k2sign/state/sessions/0.46.10.json" <<'PY'
import json, sys
p = sys.argv[1]; s = json.load(open(p)); s["count"] = s["max_signatures"]; open(p, "w").write(json.dumps(s))
PY
check_in "the session's signature limit (a belt behind one-sha-per-asset)" "$tmp/m" 3 "reached its 6 signatures" "$K" sign u2 k2-daemon-macos-aarch64 0.46.10 raw
unset K2SIGN_TEST_EVIDENCE

echo "== Windows only when the approval names it =="
check "a Windows setup without --windows" 3 "Windows setup not approved for 0.46.11" "$K" challenge 0.46.11 "$B1" K2.app.tar.gz setup.exe
check "--windows without a setup" 3 "--windows without a Windows setup" "$K" challenge 0.46.11 "$B1" K2.app.tar.gz --windows
challenge_ok 0.46.11 "$B1" K2.app.tar.gz setup.exe K2_0.46.11_x64-setup.exe --windows
grep -q '"windows":true' "$tmp/c.json" && pass "a --windows challenge says windows: true (the prompt shows it)" || fail "windows challenge"

echo "== release side: sign-updater.sh through the signer =="
export K2_UPDATER_SRC_ROOT="$TREE" K2_RELEASE_AUDIT_DIR="$tmp/home-audit" HOME="$tmp/home"
sbin="$tmp/sbin"; mkdir -p "$sbin"
# A security stub: items listed in $STUB_ITEMS "exist".
cat > "$sbin/security" <<'SH'
#!/bin/sh
[ "$1" = find-generic-password ] || exit 1
svc=""; while [ $# -gt 0 ]; do [ "$1" = -s ] && svc="$2"; shift; done
case " ${STUB_ITEMS:-} " in *" $svc "*) exit 0 ;; esac
exit 44
SH
chmod +x "$sbin/security"
export PATH="$sbin:$PATH" STUB_ITEMS=""
lib() { bash -c 'source "$1"; shift; "$@"' _ "$TREE/scripts/lib/sign-updater.sh" "$@"; }
challenge_ok 0.46.12 "$B1" "${ASSETS[@]}"
approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/r.der"
"$K" approve "$(nonce_of "$tmp/c.json")" < "$tmp/r.der" >/dev/null 2>&1 || fail "release-side session"
check "k2u_preflight (k2sign) ok" 0 "matching this tree" lib k2u_preflight 0.46.12
printf 'app tarball\n' > "$tmp/app.tgz"
check "k2u_sign without evidence: the signer refuses, k2u_sign fails" 1 "K2.app.tar.gz does not extract" lib k2u_sign u2 "$tmp/app.tgz" K2.app.tar.gz 0.46.12 "$tmp/app.sig2" raw
[ ! -e "$tmp/app.sig2" ] && pass "no signature file was written" || fail "sig written on refusal"
export K2SIGN_TEST_EVIDENCE=accept
check "k2u_sign u2 via k2sign" 0 "via k2sign" lib k2u_sign u2 "$tmp/app.tgz" K2.app.tar.gz 0.46.12 "$tmp/app.sig2" raw
check "k2u_sign u1 via k2sign (b64)" 0 "via k2sign" lib k2u_sign u1 "$tmp/app.tgz" K2.app.tar.gz 0.46.12 "$tmp/app.sig" b64
check "k2u_verify_u2 on it (what 0.46.6+ installs check)" 0 "" lib k2u_verify_u2 "$tmp/app.tgz" "$tmp/app.sig2" K2.app.tar.gz 0.46.12
check "k2u_verify_u1 on it (the bridge)" 0 "" lib k2u_verify_u1 "$tmp/app.tgz" "$tmp/app.sig" K2.app.tar.gz
grep -q "_k2sign (copy; the real log is /var/k2sign/audit/0.46.12)" "$tmp/home-audit/0.46.12/signatures.tsv" \
    && pass "the release side keeps a marked COPY of the audit line" || fail "audit copy" "$(cat "$tmp/home-audit/0.46.12/signatures.tsv")"
# The source tree disagreeing with the signer's pinned keys is an alarm.
cp "$TREE/crates/k2-release-keys/src/lib.rs" "$tmp/lib.rs.bak"
fixture_keys "$U1_LINE" "$(sed -n 2p "$tmp/keys/other.pub")" "$U3_LINE"
check "k2u_preflight: the tree's U2 is not the signer's" 1 "the signer's pinned U2 is $U2_ID" lib k2u_preflight 0.46.12
check "k2u_sign: a signature the tree's keys don't accept is not kept" 1 "does not verify with the source tree" lib k2u_sign u2 "$tmp/app.tgz" K2.app.tar.gz 0.46.12 "$tmp/x.sig2" raw
cp "$tmp/lib.rs.bak" "$TREE/crates/k2-release-keys/src/lib.rs"
unset K2SIGN_TEST_EVIDENCE
mkdir -p "$HOME/.k2-updater"; : > "$HOME/.k2-updater/k2-updater-u2.key"
check "preflight refuses while ~/.k2-updater still holds a key" 1 ".k2-updater/k2-updater-u2.key" lib k2u_preflight 0.46.12
rm -rf "$HOME/.k2-updater"
STUB_ITEMS="k2-updater-signing-u1" expect 1 "keychain:k2-updater-signing-u1" lib k2u_preflight 0.46.12 > "$tmp/r" \
    && pass "preflight refuses while a stopgap keychain item exists" || fail "keychain item" "$(cat "$tmp/r")"
printf 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x\n' > "$TREE/.env"
check "preflight refuses while the checkout's .env has TAURI_SIGNING_*" 1 ".env:TAURI_SIGNING_*" lib k2u_preflight 0.46.12
rm -f "$TREE/.env"
K2_SIGNER=vault expect 1 "K2_SIGNER must be k2sign or keychain" lib k2u_preflight 0.46.12 > "$tmp/r" && pass "an unknown K2_SIGNER is refused" || fail "K2_SIGNER" "$(cat "$tmp/r")"

mkdir -p "$tmp/cdir"
check "S3-9 sign-daemon.sh --from-dir under k2sign" 1 "compute builds are not signable through k2sign yet (S3b)" \
    bash "$REPO/scripts/sign-daemon.sh" 0.46.12 --from-dir "$tmp/cdir" --compute-verdict "$tmp/cdir"

echo "== sign-approve.sh: Step 0.46 =="
sa() { bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" "$@"; }
helper="$tmp/helper.sh"
cat > "$helper" <<SH
#!/bin/sh
# K2 Release Approve stand-in: signs the challenge with the test approver key.
c=""; o=""
while [ \$# -gt 0 ]; do case "\$1" in --challenge) c="\$2"; shift ;; --out) o="\$2"; shift ;; esac; shift; done
echo "\$c" >> "$tmp/helper.calls"
/usr/bin/openssl dgst -sha256 -sign "$tmp/keys/approver.key" -out "\$o" "\$c"
SH
chmod +x "$helper"
export K2_SIGN_APPROVE_CMD="$helper"
check "a new release: one approval" 0 "signing session" sa k2sa_open_session 0.46.13 "$B1" "${ASSETS[@]}"
[ "$(wc -l < "$tmp/helper.calls" | tr -d ' ')" = 1 ] && pass "the helper ran once" || fail "helper calls"
check "a resumed run reuses the valid session (no prompt)" 0 "using 0.46.13's approved signing session" sa k2sa_open_session 0.46.13 "$B1" "${ASSETS[@]}"
[ "$(wc -l < "$tmp/helper.calls" | tr -d ' ')" = 1 ] && pass "no second prompt" || fail "helper called again"
check "a session with other parameters: stop loudly" 1 "already has an approved session with different parameters" sa k2sa_open_session 0.46.13 "$B2" "${ASSETS[@]}"
check "…and with another asset list" 1 "Investigate before re-approving" sa k2sa_open_session 0.46.13 "$B1" K2.app.tar.gz
K2SIGN_TEST_NOW=$((NOW + 13 * 3600)) expect 0 "RE-APPROVE" sa k2sa_open_session 0.46.13 "$B1" "${ASSETS[@]}" > "$tmp/r" \
    && pass "an expired session asks to RE-APPROVE (same base)" || fail "reapprove" "$(cat "$tmp/r")"
[ "$(wc -l < "$tmp/helper.calls" | tr -d ' ')" = 2 ] && pass "the re-approval prompted once more" || fail "reapprove calls"
cat > "$tmp/cancel.sh" <<SH
#!/bin/sh
o=""; while [ \$# -gt 0 ]; do [ "\$1" = --out ] && o="\$2"; shift; done
echo "Authentication canceled." > "\$o.err"
SH
chmod +x "$tmp/cancel.sh"
K2_SIGN_APPROVE_CMD="$tmp/cancel.sh" expect 1 "No signing approval (Authentication canceled.) — nothing was built. Re-run: ./scripts/release.sh 0.46.14" \
    sa k2sa_open_session 0.46.14 "$B1" "${ASSETS[@]}" > "$tmp/r" && pass "a cancelled Touch ID: the reason and the re-run line" || fail "cancel" "$(cat "$tmp/r")"
cat > "$tmp/wrongkey.sh" <<SH
#!/bin/sh
c=""; o=""
while [ \$# -gt 0 ]; do case "\$1" in --challenge) c="\$2"; shift ;; --out) o="\$2"; shift ;; esac; shift; done
/usr/bin/openssl dgst -sha256 -sign "$tmp/keys/other.key" -out "\$o" "\$c"
SH
chmod +x "$tmp/wrongkey.sh"
K2_SIGN_APPROVE_CMD="$tmp/wrongkey.sh" expect 1 "the signer did not accept the approval" sa k2sa_open_session 0.46.15 "$B1" "${ASSETS[@]}" > "$tmp/r" \
    && pass "a helper that signs with another key: refused by the signer" || fail "wrong key helper" "$(cat "$tmp/r")"

echo "== S2-5: the watchdog (helper = sleep 99999, 1 minute) =="
printf '#!/bin/sh\nexec sleep 99999\n' > "$tmp/sleep.sh"; chmod +x "$tmp/sleep.sh"
t0="$(date +%s)"
K2_SIGN_APPROVE_CMD="$tmp/sleep.sh" K2_SIGN_APPROVE_TIMEOUT_MIN=1 expect 1 \
    "No signing approval (timed out after 1 min) — nothing was built. Re-run: ./scripts/release.sh 0.46.16" \
    sa k2sa_open_session 0.46.16 "$B1" "${ASSETS[@]}" > "$tmp/r" && rc=0 || rc=1
t1="$(date +%s)"
[ "$rc" = 0 ] && [ $((t1 - t0)) -le 90 ] && pass "S2-5 exit 1 within 90 s ($((t1 - t0)) s) with the re-run line" || fail "S2-5 watchdog" "$(cat "$tmp/r") ($((t1 - t0)) s)"
pgrep -f "$tmp/sleep.sh" >/dev/null && fail "the helper still runs after the watchdog" || pass "the watchdog killed the helper"
# release.sh: Step 0.46 sits after the preflight (0.45) and before every build
# step, and a failed approval exits.
python3 -I - "$REPO/scripts/release.sh" <<'PY' && pass "release.sh: Step 0.46 runs after 0.45, before 0.5 and any build; its failure exits 1" || fail "release.sh order"
import re, sys
s = open(sys.argv[1]).read()
i045 = s.index('bash "$PROJECT_DIR/scripts/sign-daemon.sh" --preflight "$VERSION"')
i046 = s.index('k2sa_open_session "$VERSION" "$K2_SIGN_BASE" "${K2_SIGN_ASSETS[@]}" || exit 1')
i05 = s.index('echo "Step 0.5: Pre-flight gates')
first_build = s.index('k2_build_mac_binaries "target/release/bundle/macos/K2.app"')
assert i045 < i046 < i05 < first_build, (i045, i046, i05, first_build)
assert re.search(r'for _k2_seam in [^;]*K2SIGN_TEST_ROOT[^;]*K2_SIGN_APPROVE_CMD', s)
PY

finish
