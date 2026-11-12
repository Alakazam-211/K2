#!/usr/bin/env bash
# k2sign-sa1.sh — k2sign as the first signed-approval relying party
# (prd-signed-approvals-v1 S1), in test mode: macOS, throwaway keys, openssl
# P-256 keys standing in for the phone's Secure Enclave keys. Nothing is
# opened, notified or asked; no real approval request is sent.
#   A. k2sign's reader reaches the shared fixtures' verdicts
#      (crates/k2-approve/fixtures/sa1), like the Rust crate and the phone.
#   B. challenge --format sa1, approve --record, require, add/remove/list-approvers,
#      mac-legacy until the phone is enrolled.
#   C. sign-approve.sh: the phone alone, and the phone raced with the Mac.
#   bash tests/release/k2sign-sa1.sh
source "$(dirname "$0")/k2sign_fixture.sh"
FIX="$REPO/crates/k2-approve/fixtures/sa1"

echo "== A. the shared fixtures, read by k2sign =="
if python3 -I - "$TREE/scripts/k2sign/k2sign.py" "$FIX" > "$tmp/a.out" 2>&1 <<'PY'
import hashlib, importlib.util, json, os, shutil, sys, tempfile, types
src, fix = sys.argv[1], sys.argv[2]
spec = importlib.util.spec_from_file_location("k2sign_under_test", src)
k = importlib.util.module_from_spec(spec)
spec.loader.exec_module(k)
bad = []
n = 0

def want(cond, what):
    global n
    n += 1
    if not cond:
        bad.append(what)

wl = open(os.path.join(fix, "wordlist.txt")).read().split()
want(k.WORDS == wl, "k2sign's WORDS differ from fixtures/sa1/wordlist.txt")
keys = json.load(open(os.path.join(fix, "keys.json")))
for name, e in keys.items():
    want(k.key_words(e["key_id"]) == e["words"], "words of %s" % name)

m = json.load(open(os.path.join(fix, "payloads.json")))
for c in m["valid"]:
    try:
        k.sa1_parse_payload(open(os.path.join(fix, c["payload"]), "rb").read())
        want(True, c["name"])
    except k.Sa1Refused as e:
        want(False, "valid %s refused: %s" % (c["name"], e))
for c in m["invalid"]:
    try:
        k.sa1_parse_payload(open(os.path.join(fix, c["payload"]), "rb").read())
        want(False, "invalid %s accepted (want %s)" % (c["name"], c["code"]))
    except k.Sa1Refused as e:
        want(e.code == c["code"], "invalid %s: got %s, want %s (%s)" % (c["name"], e.code, c["code"], e))

# k2sign pins P-256 keys only: the fixtures' RSA key (Windows Hello, later) is left out.
d = tempfile.mkdtemp()
ad = os.path.join(d, "etc", "approvers")
os.makedirs(ad)
lines = []
for line in open(os.path.join(fix, "keys", "approvers.tsv")).read().splitlines():
    if not line.strip() or line.startswith("#"):
        continue
    name = line.split("\t")[0]
    if keys[name]["alg"] != "ES256":
        continue
    lines.append(line)
    shutil.copy(os.path.join(fix, "keys", name + ".pem"), ad)
open(os.path.join(ad, "approvers.tsv"), "w").write("\n".join(lines) + "\n")
ctx = types.SimpleNamespace(etc=os.path.join(d, "etc"), test=True, admin=False, signer=None)
approvers = k.load_approvers(ctx)
rsa = {e["key_id"] for e in keys.values() if e["alg"] != "ES256"}
scratch = os.path.join(d, "s")
os.mkdir(scratch)
skipped = []
cases = json.load(open(os.path.join(fix, "cases.json")))["cases"]
for c in cases:
    want_v = "ok" if c["expect"]["ok"] else c["expect"]["code"]
    try:
        r = k.sa1_parse_record(open(os.path.join(fix, c["record"]), "rb").read())
        if r["key_id"] in rsa and r["alg"] == "RS256":
            skipped.append(c["name"])
            continue
        stored = open(os.path.join(fix, c["stored_payload"]), "rb").read() if c["stored_payload"] else None
        q = c["require"]
        need = {"rp": q["rp"], "kind": q["kind"], "check": q["check"], "view": q["view"], "attest": q["attest"],
                "now": k.sa1_utc(q["now"]), "nonce": q["nonce"]}
        p, a = k.sa1_check(ctx, approvers, r, stored, need, scratch)
        got = "nonce_used" if p["nonce"] in c["used_nonces"] else "ok"
        if got == "ok":
            e = c["expect"]
            want((a["name"], a["person"], a["key_id"], a["check"], a["view"], None if a["attest"] == "none" else a["attest"])
                 == (e["key_name"], e["person"], e["key_id"], e["check"], e["view"], e["attest"]), "%s: the key" % c["name"])
    except k.Sa1Refused as e:
        got = e.code
    want(got == want_v, "case %s: got %s, want %s" % (c["name"], got, want_v))
want(skipped == ["ok-rs256-host-when-allowed"], "skipped %r" % skipped)
want(len(cases) - len(skipped) >= 30, "too few cases")

# The S0 iPhone's App Attest attestation (real-device fixtures).
import base64
want(open(os.path.join(fix, "apple-app-attest-root.pem")).read() == k.APPLE_APP_ATTEST_ROOT, "the pinned Apple root differs")
exp = json.load(open(os.path.join(fix, "real-device", "expect.json")))["attest"]
s0 = json.load(open(os.path.join(fix, "real-device", "s0-attest.json")))
att, cd = base64.b64decode(s0["attestation_b64"]), base64.b64decode(s0["client_data_b64"])
r = k.verify_app_attest(ctx, att, cd, s0["attest_key_id"], exp["app_id"], k.sa1_utc(exp["check_at"]), scratch)
want(r["environment"] == exp["environment"] and r["leaf_not_after"] == k.sa1_utc(exp["leaf_not_after"]), "S0 attest %r" % r)
for what, args in (("leaf expired", (att, cd, s0["attest_key_id"], exp["app_id"], k.sa1_utc(exp["refused_at"]))),
                   ("other app", (att, cd, s0["attest_key_id"], "36B8R93HXV.com.example.other", k.sa1_utc(exp["check_at"]))),
                   ("other data", (att, cd[:-1] + bytes([cd[-1] ^ 1]), s0["attest_key_id"], exp["app_id"], k.sa1_utc(exp["check_at"]))),
                   ("other key id", (att, cd, "A" * 43 + "=", exp["app_id"], k.sa1_utc(exp["check_at"])))):
    try:
        k.verify_app_attest(ctx, *args, scratch)
        want(False, "S0 attest with %s was accepted" % what)
    except k.Refused:
        want(True, what)
try:
    k.parse_enrol_client_data(cd)
    want(False, "the spike clientData passed as an enrolment")
except k.Refused as e:
    want("K2-APPROVER-ENROL-1" in str(e), str(e))
ch = bytes(range(32))
c2, bio, pas = k.parse_enrol_client_data(k.ENROL_PREFIX + ch + cd)
want((c2, bio, pas) == (ch, cd[:91], cd[91:]), "the enrolment form splits")
want(hashlib.sha256(bio).hexdigest() == s0["biometric_key_id"], "biometric key id")
print("%d checks, %d failed" % (n, len(bad)))
for b in bad:
    print("  " + b)
sys.exit(1 if bad else 0)
PY
then pass "k2sign reaches every fixture verdict ($(tail -1 "$tmp/a.out"))"; else fail "fixture verdicts" "$(head -40 "$tmp/a.out")"; fi

echo "== B. k2sign as the relying party =="
ready
NOW="$(date +%s)"
export K2SIGN_TEST_NOW="$NOW"
POL="$TROOT/usr/local/etc/k2sign/policy"
grep -qx "require_attest = production" "$POL" && pass "the installed policy needs a production-attested key for releases" || fail "require_attest" "$(cat "$POL")"
check "status: require_attest production, nothing approves but the Mac" 0 '"require_attest": "production", "sa1": []' "$K" status 0.46.20
# These tests pin openssl keys (no App Attest): the policy takes unattested keys.
sed -i '' 's/^require_attest = production$/require_attest = none/' "$POL"
"$ADM" repin >/dev/null 2>&1
ASSETS=(K2.app.tar.gz k2-daemon-linux-x86_64)
for n in phone-bio phone-cred other-person rogue; do
    /usr/bin/openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/keys/$n.key" 2>/dev/null
    /usr/bin/openssl ec -in "$tmp/keys/$n.key" -pubout -out "$tmp/keys/$n.pem" 2>/dev/null
done
/usr/bin/openssl genrsa -out "$tmp/keys/rsa.key" 2048 2>/dev/null
/usr/bin/openssl rsa -in "$tmp/keys/rsa.key" -pubout -out "$tmp/keys/rsa.pem" 2>/dev/null
kid() { /usr/bin/openssl pkey -pubin -in "$1" -outform DER 2>/dev/null | /usr/bin/openssl dgst -sha256 -r | cut -c1-64; }
words() { python3 -I -c 'import sys; w=open(sys.argv[1]).read().split(); print(" ".join(w[b] for b in bytes.fromhex(sys.argv[2])[:6]))' "$FIX/wordlist.txt" "$1"; }
BIO_ID="$(kid "$tmp/keys/phone-bio.pem")"
CRED_ID="$(kid "$tmp/keys/phone-cred.pem")"
RP="k2sign@$(python3 -I -c 'import os,re; print(re.sub(r"[^A-Za-z0-9-]", "-", os.uname().nodename.split(".")[0])[:63] or "mac")')"
# record <payload> <key> <key id> <out> [<python edit of the record dict>]
record() {
    /usr/bin/openssl dgst -sha256 -sign "$2" -out "$4.sig" "$1"
    python3 -I - "$1" "$3" "$4.sig" "$4" "${5:-}" <<'PY'
import base64, json, sys
p, kid, sig, out, edit = sys.argv[1:6]
r = {"v": 1, "payload_b64": base64.b64encode(open(p, "rb").read()).decode(), "key_id": kid, "alg": "ES256",
     "sig_b64": base64.b64encode(open(sig, "rb").read()).decode()}
if edit:
    exec(edit)
open(out, "w").write(json.dumps(r, separators=(",", ":")))
PY
}
pfield() { python3 -I -c '
import json, sys
b = open(sys.argv[1], "rb").read(); p = b"K2-SIGNED-APPROVAL-1\x00"; assert b.startswith(p)
d = json.loads(b[len(p):]); v = d
for k in sys.argv[2].split("."): v = v[k]
print(json.dumps(v) if not isinstance(v, str) else v)' "$1" "$2"; }

check "status: only the Mac can approve (no signed-approval key yet)" 0 '"approvers": {"legacy": true, "require": "biometric device", "require_attest": "none", "sa1": []}' "$K" status 0.46.20
check "challenge --format sa1 with no key that meets require" 3 "no enrolled approver key meets the policy (require = biometric device, require_attest = none)" \
    "$K" challenge 0.46.20 "$B1" "${ASSETS[@]}" --format sa1
check "challenge --format takes sa1 only" 2 "--format takes sa1" "$K" challenge 0.46.20 "$B1" "${ASSETS[@]}" --format cbor

echo "-- mac-legacy until the phone is enrolled --"
"$K" challenge 0.46.20 "$B1" "${ASSETS[@]}" > "$tmp/l.json" 2>"$tmp/e" || fail "legacy challenge" "$(cat "$tmp/e")"
approve_with "$tmp/keys/approver.key" "$tmp/l.json" "$tmp/l.der"
check_in "a legacy challenge takes no --record" "$tmp/l.der" 3 "send its DER signature (no --record)" "$K" approve "$(nonce_of "$tmp/l.json")" --record
check_in "the Mac's Touch ID approval opens the session" "$tmp/l.der" 0 "approved until" "$K" approve "$(nonce_of "$tmp/l.json")"
check "the session says mac-legacy, credential, host" 0 '"by": "mac-legacy:credential:host"' "$K" status 0.46.20
grep -qF "by mac-legacy:credential:host" "$AUDIT/0.46.20/signatures.tsv" && pass "the audit log names mac-legacy" || fail "audit by" "$(cat "$AUDIT/0.46.20/signatures.tsv")"

echo "-- add-approver: the phone's keys --"
check "an RSA key is refused (P-256 only for now)" 3 "only P-256 (ES256) keys" \
    "$ADM" add-approver phone-rsa "$tmp/keys/rsa.pem" --person alex --check biometric --view device
check "words that don't match the key: nothing is added" 3 "are not this key's" \
    "$ADM" add-approver phone-bio "$tmp/keys/phone-bio.pem" --person alex --check biometric --view device --words "acid acid acid acid acid acid"
check "the name mac-legacy is reserved" 2 "mac-legacy" \
    "$ADM" add-approver mac-legacy "$tmp/keys/phone-bio.pem" --person alex --check biometric --view device
check "a relative path is usage" 2 "absolute path" \
    "$ADM" add-approver phone-bio keys/phone-bio.pem --person alex --check biometric --view device
check "--person is required" 2 "add-approver <name>" \
    "$ADM" add-approver phone-bio "$tmp/keys/phone-bio.pem" --check biometric --view device
check "add the phone's Face ID key, with the words from its screen" 0 "words:  $(words "$BIO_ID")" \
    "$ADM" add-approver phone-bio "$tmp/keys/phone-bio.pem" --person alex --check biometric --view device --words "$(words "$BIO_ID")"
grep -qF "key id: $BIO_ID" "$tmp/out" && pass "it prints the key id" || fail "key id printout" "$(cat "$tmp/out")"
grep -qF "the Mac's K2 Release Approve key no longer can" "$tmp/out" && pass "it says the Mac key no longer approves releases" || fail "legacy note" "$(cat "$tmp/out")"
check "add the phone's passcode key (not for releases)" 0 "approved by phone-bio" \
    "$ADM" add-approver phone-cred "$tmp/keys/phone-cred.pem" --person alex --check credential --view device
check "the same name twice" 3 "already an approver named phone-bio" \
    "$ADM" add-approver phone-bio "$tmp/keys/other-person.pem" --person alex --check biometric --view device
check "the same key twice" 3 "already approver phone-bio" \
    "$ADM" add-approver phone-bio-2 "$tmp/keys/phone-bio.pem" --person alex --check biometric --view device
check "the approver files are pinned (status runs)" 0 '"sa1": ["phone-bio"]' "$K" status 0.46.21
grep -qF '"legacy": false' "$tmp/out" && pass "status: the Mac no longer approves releases" || fail "status legacy" "$(cat "$tmp/out")"
grep -qF "etc/approvers/approvers.tsv" "$TROOT/usr/local/etc/k2sign/pins.sha256" && grep -qF "etc/approvers/phone-bio.pem" "$TROOT/usr/local/etc/k2sign/pins.sha256" \
    && pass "pins.sha256 covers approvers.tsv and each key" || fail "pins" "$(cat "$TROOT/usr/local/etc/k2sign/pins.sha256")"
check "list-approvers" 0 "phone-bio	alex	biometric device	attest none	approves releases	$BIO_ID	$(words "$BIO_ID")" "$ADM" list-approvers
grep -qF "phone-cred	alex	credential device	attest none	not for releases" "$tmp/out" && grep -qF "mac-legacy	(the Mac's K2 Release Approve key)	credential host	not for releases" "$tmp/out" \
    && pass "list-approvers: the passcode key and mac-legacy are not for releases" || fail "list" "$(cat "$tmp/out")"
cp "$TROOT/usr/local/etc/k2sign/approvers/approvers.tsv" "$tmp/tsv.bak"
printf 'rogue\talex\tbiometric\tdevice\n' >> "$TROOT/usr/local/etc/k2sign/approvers/approvers.tsv"
check "an approver line added by hand breaks the pins" 3 "pins mismatch: approvers/approvers.tsv" "$K" status 0.46.21
cp "$tmp/tsv.bak" "$TROOT/usr/local/etc/k2sign/approvers/approvers.tsv"
cp "$tmp/keys/rogue.pem" "$TROOT/usr/local/etc/k2sign/approvers/rogue.pem"
check "a key file dropped in by hand breaks the pins" 3 "pins mismatch: approvers/rogue.pem (not pinned)" "$K" status 0.46.21
rm "$TROOT/usr/local/etc/k2sign/approvers/rogue.pem"

echo "-- App Attest: a release key must be production-attested --"
sed -i '' 's/^require_attest = none$/require_attest = production/' "$POL"
"$ADM" repin >/dev/null 2>&1
check "require_attest = production: the unattested phone key doesn't approve releases" 0 '"sa1": []' "$K" status 0.46.21
check "enrol-challenge prints a 32-byte challenge" 0 "" "$ADM" enrol-challenge
grep -qE '^[0-9a-f]{64}$' "$tmp/out" && pass "64 hex" || fail "enrol-challenge" "$(cat "$tmp/out")"
check "the S0 spike attestation is not an enrolment (no K2-APPROVER-ENROL-1 form)" 3 "K2-APPROVER-ENROL-1" \
    "$ADM" add-approver phone2 --attest "$FIX/real-device/s0-attest.json" --person alex
check "--attest takes no --check" 2 "add-approver <name>" \
    "$ADM" add-approver phone2 --attest "$FIX/real-device/s0-attest.json" --person alex --check biometric
sed -i '' 's/^require_attest = production$/require_attest = none/' "$POL"
"$ADM" repin >/dev/null 2>&1

echo "-- challenge --format sa1 and approve --record --"
check "the Mac's challenge is refused once the phone is enrolled" 3 "approve on that device" "$K" challenge 0.46.21 "$B1" "${ASSETS[@]}"
"$K" challenge 0.46.21 "$B1" "${ASSETS[@]}" --format sa1 > "$tmp/p.bin" 2>"$tmp/e" || fail "sa1 challenge" "$(cat "$tmp/e")"
python3 -I - "$tmp/p.bin" "$BIO_ID" "$RP" "$NOW" "$B1" <<'PY' && pass "the payload: k2.release v1, rp, approver, the phone's Face ID key only, 15 min to answer, a 12 h session" || fail "payload" "$(tr '\0' '|' < "$tmp/p.bin")"
import calendar, json, sys, time
b = open(sys.argv[1], "rb").read()
p = b"K2-SIGNED-APPROVAL-1\x00"
assert b.startswith(p)
d = json.loads(b[len(p):])
t = lambda s: calendar.timegm(time.strptime(s, "%Y-%m-%dT%H:%M:%SZ"))
now = int(sys.argv[4])
assert d["kind"] == "k2.release" and d["kind_v"] == 1 and d["rp"] == sys.argv[3] and d["approver"] == "alex", d
assert d["approver_keys"] == [sys.argv[2]], d["approver_keys"]
assert t(d["issued"]) == now and t(d["answer_by"]) == now + 15 * 60
b_ = d["body"]
assert b_["version"] == "0.46.21" and b_["base_commit"] == sys.argv[5] and b_["assets"] == ["K2.app.tar.gz", "k2-daemon-linux-x86_64"]
assert b_["windows"] is False and b_["reapproval"] is False and b_["replacement"] is False and b_["replaces_base"] == ""
assert b_["max_signatures"] == 8 and t(b_["session_until"]) == now + 12 * 3600 and b_["slots"] == ["u1", "u2"]
PY
SN="$(pfield "$tmp/p.bin" nonce)"
record "$tmp/p.bin" "$tmp/keys/phone-bio.key" "$BIO_ID" "$tmp/good.json"
approve_with "$tmp/keys/phone-bio.key" "$tmp/p.bin" "$tmp/p.der"
check_in "an sa1 challenge takes a record, not a bare signature" "$tmp/p.der" 3 "send its record" "$K" approve "$SN"
record "$tmp/p.bin" "$tmp/keys/phone-cred.key" "$CRED_ID" "$tmp/cred.json"
check_in "the passcode key, enrolled but not for releases" "$tmp/cred.json" 3 "key_not_listed" "$K" approve "$SN" --record
record "$tmp/p.bin" "$tmp/keys/rogue.key" "$(kid "$tmp/keys/rogue.pem")" "$tmp/rogue.json"
check_in "a key k2sign never enrolled" "$tmp/rogue.json" 3 "unknown_key" "$K" approve "$SN" --record
record "$tmp/p.bin" "$tmp/keys/rogue.key" "$BIO_ID" "$tmp/forged.json"
check_in "another key's signature under the phone's key id" "$tmp/forged.json" 3 "bad_signature" "$K" approve "$SN" --record
record "$tmp/p.bin" "$tmp/keys/phone-bio.key" "$BIO_ID" "$tmp/claims.json" 'r["check"] = "biometric"'
check_in "a record that claims its own check" "$tmp/claims.json" 3 "record: unknown record field 'check'" "$K" approve "$SN" --record
python3 -I -c 'import sys; b=open(sys.argv[1],"rb").read(); open(sys.argv[2],"wb").write(b.replace(b"0.46.21", b"0.46.22", 1))' "$tmp/p.bin" "$tmp/p2.bin"
record "$tmp/p2.bin" "$tmp/keys/phone-bio.key" "$BIO_ID" "$tmp/other-bytes.json"
check_in "a record over bytes k2sign didn't issue (another version)" "$tmp/other-bytes.json" 3 "payload_mismatch" "$K" approve "$SN" --record
check_in "a record for another nonce" "$tmp/good.json" 3 "unknown challenge" "$K" approve "ffeeddccbbaa99887766554433221100" --record
: > "$tmp/empty"
check_in "no record at all" "$tmp/empty" 3 "no approval record on stdin" "$K" approve "$SN" --record
grep -qF "REFUSED-APPROVAL" "$AUDIT/0.46.21/signatures.tsv" && pass "refused records are in the audit log" || fail "refused audit"
check_in "the phone's Face ID approval opens the session" "$tmp/good.json" 0 "approved until" "$K" approve "$SN" --record
grep -qF "(by phone-bio:biometric:device)" "$tmp/out" && pass "approve names phone-bio, biometric, device" || fail "approve by" "$(cat "$tmp/out")"
check "status: by phone-bio:biometric:device" 0 '"by": "phone-bio:biometric:device"' "$K" status 0.46.21
python3 -I - "$tmp/out" "$tmp/p.bin" <<'PY' && pass "the session ends at the payload's session_until (what the screen said)" || fail "session end" "$(cat "$tmp/out")"
import json, sys
d = json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])
b = open(sys.argv[2], "rb").read()
p = json.loads(b[len(b"K2-SIGNED-APPROVAL-1\x00"):])
assert d["session"]["expires"] == p["body"]["session_until"], (d["session"]["expires"], p["body"]["session_until"])
PY
check_in "the same record again" "$tmp/good.json" 3 "challenge already used" "$K" approve "$SN" --record

echo "-- the policy decides: answer_by, require --"
"$K" challenge 0.46.22 "$B1" "${ASSETS[@]}" --format sa1 > "$tmp/p3.bin" 2>"$tmp/e" || fail "sa1 challenge 3" "$(cat "$tmp/e")"
record "$tmp/p3.bin" "$tmp/keys/phone-bio.key" "$BIO_ID" "$tmp/late.json"
K2SIGN_TEST_NOW=$((NOW + 15 * 60 + 1)) check_in "16 minutes later" "$tmp/late.json" 3 "challenge expired" "$K" approve "$(pfield "$tmp/p3.bin" nonce)" --record
sed -i '' 's/^require = biometric device$/require = biometric/' "$POL"
"$ADM" repin >/dev/null 2>&1
check "a bad require line is refused" 3 "policy: require must be" "$K" status 0.46.22
sed -i '' 's/^require = biometric$/require = credential device/' "$POL"
"$ADM" repin >/dev/null 2>&1
check "require = credential device: the passcode key approves too" 0 '"sa1": ["phone-bio", "phone-cred"]' "$K" status 0.46.22
"$K" challenge 0.46.22 "$B1" "${ASSETS[@]}" --format sa1 > "$tmp/p4.bin" 2>"$tmp/e" || fail "sa1 challenge 4" "$(cat "$tmp/e")"
[ "$(pfield "$tmp/p4.bin" approver_keys)" = "[\"$BIO_ID\", \"$CRED_ID\"]" ] && pass "both keys are in approver_keys" || fail "approver_keys" "$(pfield "$tmp/p4.bin" approver_keys)"
sed -i '' 's/^require = credential device$/require = biometric device/' "$POL"
"$ADM" repin >/dev/null 2>&1
record "$tmp/p4.bin" "$tmp/keys/phone-cred.key" "$CRED_ID" "$tmp/cred4.json"
check_in "the policy is read at approve time: the passcode key no longer passes" "$tmp/cred4.json" 3 "weak_check" "$K" approve "$(pfield "$tmp/p4.bin" nonce)" --record

echo "-- a second person's key --"
check "add another person's Face ID key" 0 "approver other added" \
    "$ADM" add-approver other "$tmp/keys/other-person.pem" --person sam --check biometric --view device
check "two people's keys meet require: the challenge asks for one" 3 "belong to alex, sam" "$K" challenge 0.46.22 "$B1" "${ASSETS[@]}" --format sa1
record "$tmp/p4.bin" "$tmp/keys/other-person.key" "$(kid "$tmp/keys/other-person.pem")" "$tmp/sam.json"
check_in "sam's key on alex's request" "$tmp/sam.json" 3 "key_not_listed" "$K" approve "$(pfield "$tmp/p4.bin" nonce)" --record
check "remove-approver" 0 "approver other removed" "$ADM" remove-approver other
check "remove-approver of a name that isn't there" 3 "no approver named other" "$ADM" remove-approver other

echo "== C. sign-approve.sh: the phone, and the phone raced with the Mac =="
sa() { bash -c 'source "$1"; source "$2"; shift 2; "$@"' _ "$TREE/scripts/lib/sign-updater.sh" "$TREE/scripts/lib/sign-approve.sh" "$@"; }
# The phone's stand-in for `k2 approve request … --wait --out R`: after
# $tmp/phone.delay seconds, signs the payload with $tmp/phone.key as $tmp/phone.kid.
phone="$tmp/phone.sh"
cat > "$phone" <<SH
#!/bin/sh
echo "k2 \$*" >> "$tmp/phone.asked"
pf=""; rf=""; to=""
while [ \$# -gt 0 ]; do case "\$1" in --payload) pf="\$2"; shift ;; --out) rf="\$2"; shift ;; --to) to="\$2"; shift ;; esac; shift; done
cp "\$pf" "$tmp/phone-shown.bin"
sleep "\$(cat "$tmp/phone.delay" 2>/dev/null || echo 0)"
[ -f "$tmp/phone.decline" ] && exit 1
/usr/bin/openssl dgst -sha256 -sign "\$(cat "$tmp/phone.keyfile")" -out "\$rf.sig" "\$pf" || exit 1
python3 -I -c 'import base64,json,sys; print(json.dumps({"v":1,"payload_b64":base64.b64encode(open(sys.argv[1],"rb").read()).decode(),"key_id":sys.argv[2],"alg":"ES256","sig_b64":base64.b64encode(open(sys.argv[3],"rb").read()).decode()},separators=(",",":")))' "\$pf" "\$(cat "$tmp/phone.kid")" "\$rf.sig" > "\$rf"
SH
chmod +x "$phone"
printf '%s' "$tmp/keys/phone-bio.key" > "$tmp/phone.keyfile"
printf '%s' "$BIO_ID" > "$tmp/phone.kid"
# The Mac's stand-in, slow or fast, counting its calls.
mac="$tmp/mac.sh"
cat > "$mac" <<SH
#!/bin/sh
c=""; o=""
while [ \$# -gt 0 ]; do case "\$1" in --challenge) c="\$2"; shift ;; --out) o="\$2"; shift ;; esac; shift; done
echo x >> "$tmp/mac.asked"
sleep "\$(cat "$tmp/mac.delay" 2>/dev/null || echo 0)"
/usr/bin/openssl dgst -sha256 -sign "$tmp/keys/approver.key" -out "\$o" "\$c"
SH
chmod +x "$mac"
export K2SA_HELPER="$mac" K2SA_PHONE_CMD="$phone" K2_SIGN_APPROVE_TIMEOUT_MIN=1
asked() { [ -f "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }

check "the phone alone (default policy): approved on the phone" 0 "approved on the phone" sa k2sa_open_session 0.46.23 "$B1" "${ASSETS[@]}"
grep -qF "Approval needed on your phone (the K2 app, Face ID), as alex: approve K2 0.46.23" "$tmp/out" && pass "the banner names the phone and the person" || fail "banner" "$(cat "$tmp/out")"
grep -q -- "approve request --to alex --payload .* --wait --out " "$tmp/phone.asked" && pass "k2 approve request --to alex --payload … --wait --out …" || fail "request args" "$(cat "$tmp/phone.asked")"
[ "$(asked "$tmp/mac.asked")" = 0 ] && pass "the Mac was not asked (the policy takes the phone only)" || fail "mac asked" "$(asked "$tmp/mac.asked")"
check "status: by phone-bio" 0 '"by": "phone-bio:biometric:device"' "$K" status 0.46.23
touch "$tmp/phone.decline"
check "a declined card: no session" 1 "the phone gave no approval" sa k2sa_open_session 0.46.24 "$B1" "${ASSETS[@]}"
rm "$tmp/phone.decline"
printf '%s' "$tmp/keys/phone-cred.key" > "$tmp/phone.keyfile"; printf '%s' "$CRED_ID" > "$tmp/phone.kid"
check "the phone signs with the passcode key: refused, no session" 1 "the signer refused the phone's approval" sa k2sa_open_session 0.46.24 "$B1" "${ASSETS[@]}"
printf '%s' "$tmp/keys/phone-bio.key" > "$tmp/phone.keyfile"; printf '%s' "$BIO_ID" > "$tmp/phone.kid"

echo "-- both at once (require = credential host: the policy takes the Mac too) --"
sed -i '' 's/^require = biometric device$/require = credential host/' "$POL"
"$ADM" repin >/dev/null 2>&1
check "status: both paths open" 0 '"legacy": true' "$K" status 0.46.24
: > "$tmp/mac.asked"; : > "$tmp/phone.asked"
echo 3 > "$tmp/phone.delay"; echo 0 > "$tmp/mac.delay"
check "the Mac answers first: its approval wins" 0 "approved on this Mac" sa k2sa_open_session 0.46.24 "$B1" "${ASSETS[@]}"
[ "$(asked "$tmp/phone.asked")" = 1 ] && [ "$(asked "$tmp/mac.asked")" = 1 ] && pass "both were asked at once" || fail "asked" "phone $(asked "$tmp/phone.asked") mac $(asked "$tmp/mac.asked")"
grep -qF "or on " "$tmp/out" && pass "the banner offers the phone or this Mac" || fail "race banner" "$(cat "$tmp/out")"
check "status: by mac-legacy" 0 '"by": "mac-legacy:credential:host"' "$K" status 0.46.24
echo 0 > "$tmp/phone.delay"; echo 3 > "$tmp/mac.delay"
check "the phone answers first: its approval wins" 0 "approved on the phone" sa k2sa_open_session 0.46.25 "$B1" "${ASSETS[@]}"
check "status: by phone-bio" 0 '"by": "phone-bio:biometric:device"' "$K" status 0.46.25
touch "$tmp/phone.decline"; echo 1 > "$tmp/mac.delay"
check "the phone declines, the Mac still approves" 0 "approved on this Mac" sa k2sa_open_session 0.46.26 "$B1" "${ASSETS[@]}"
rm "$tmp/phone.decline"
sed -i '' 's/^require = credential host$/require = biometric device/' "$POL"
"$ADM" repin >/dev/null 2>&1

echo "-- replacing an unused approval on the phone --"
echo 0 > "$tmp/phone.delay"
check "0.46.27 approved on base B1" 0 "approved on the phone" sa k2sa_open_session 0.46.27 "$B1" "${ASSETS[@]}"
check "a fix commit (B2): a replacement, approved on the phone" 0 "approved on the phone" sa k2sa_open_session 0.46.27 "$B2" "${ASSETS[@]}"
[ "$(pfield "$tmp/phone-shown.bin" body.replacement) $(pfield "$tmp/phone-shown.bin" body.replaces_base)" = "true $B1" ] \
    && pass "the phone was shown replacement true, replaces_base B1" || fail "replacement payload" "$(tr '\0' '|' < "$tmp/phone-shown.bin")"
check "status: the session binds B2" 0 "\"base_commit\": \"$B2\"" "$K" status 0.46.27

echo "-- removing the phone puts the Mac back --"
check "remove phone-bio" 0 "approver phone-bio removed" "$ADM" remove-approver phone-bio
check "status: the Mac approves again" 0 '"legacy": true' "$K" status 0.46.28
: > "$tmp/mac.asked"; echo 0 > "$tmp/mac.delay"
check "sign-approve uses the Mac" 0 "signing session" sa k2sa_open_session 0.46.28 "$B1" "${ASSETS[@]}"
[ "$(asked "$tmp/mac.asked")" = 1 ] && pass "the Mac was asked" || fail "mac asked" "$(asked "$tmp/mac.asked")"

finish
