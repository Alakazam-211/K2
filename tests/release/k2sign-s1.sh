#!/usr/bin/env bash
# k2sign-s1.sh — prd-signing-user-v1 S1 acceptance, in test mode (macOS,
# throwaway keys; no user is created, no sudo, nothing outside a temp dir).
#   bash tests/release/k2sign-s1.sh
# The z3mbpZ half (real user, real sudo: S1 tests 1–2) is in
# scripts/k2sign/sitting.sh's checks.
source "$(dirname "$0")/k2sign_fixture.sh"

echo "== the installer =="
L="$TROOT/usr/local/libexec/k2sign"; E="$TROOT/usr/local/etc/k2sign"
for f in k2sign k2sign-admin k2sign.py minisign gh check-daemon-asset.py; do
    [ -f "$L/$f" ] && pass "installed libexec/$f" || fail "libexec/$f missing"
done
grep -q '@PYTHON@' "$L/k2sign" && fail "python path not filled in" || pass "wrappers carry the resolved python path"
grep -q '/Library/Developer/CommandLineTools/' "$L/k2sign" && pass "the signer's python is the Command Line Tools one (not Xcode.app: /Applications is admin-writable)" \
    || fail "python path" "$(grep K2SIGN_PYTHON= "$L/k2sign")"
[ "$(stat -f %Lp "$TROOT/var/k2sign")" = 700 ] && [ "$(stat -f %Lp "$TROOT/var/k2sign/keys")" = 700 ] \
    && pass "/var/k2sign and keys are 0700" || fail "var modes"
sud="$TROOT/etc/sudoers.d/k2sign"
[ "$(grep -v '^#' "$sud")" = "$(id -un) ALL=(_k2sign) NOPASSWD: /usr/local/libexec/k2sign/k2sign" ] \
    && pass "sudoers: exactly one rule, only the signer, only as _k2sign" || fail "sudoers" "$(cat "$sud")"
/usr/sbin/visudo -cf "$sud" >/dev/null && pass "sudoers drop-in parses (visudo -cf)" || fail "visudo"
grep -q '^u1 ' "$E/pubkeys" && ! grep -q '^u2 ' "$E/pubkeys" && pass "pre-ceremony tree: only U1 pinned" || fail "pubkeys" "$(cat "$E/pubkeys")"
grep -q 'no primary keys yet (pre-ceremony)' "$tmp/install.out" && pass "the installer says the tree has no U2 yet" || fail "pre-ceremony note"
fresh="$tmp/fresh"; mkdir -p "$fresh"
K2SIGN_TEST_ROOT="$fresh" bash "$TREE/scripts/k2sign/install.sh" --test --dry-run --minisign "$MINISIGN" --gh "$REPO/tests/release/fixtures/gh-stub.sh" > "$tmp/dry.out" 2>&1 \
    && [ -z "$(find "$fresh" -mindepth 1 -print -quit)" ] && grep -q 'would run' "$tmp/dry.out" \
    && pass "--dry-run prints the plan and changes nothing" || fail "dry run" "$(tail -3 "$tmp/dry.out"; find "$fresh" | head -3)"
if K2SIGN_TEST_ROOT="$fresh" bash "$TREE/scripts/k2sign/install.sh" --dry-run > "$tmp/x.out" 2>&1; then
    fail "production install with K2SIGN_TEST_ROOT set" "ran"
else
    grep -q 'K2SIGN_TEST_ROOT is set' "$tmp/x.out" && pass "a production install refuses K2SIGN_TEST_ROOT" || fail "prod+test root" "$(cat "$tmp/x.out")"
fi

echo "== preflight before keys and before the ceremony: refused =="
check "preflight before the ceremony and before any key" 3 "no U2 public key pinned" "$K" preflight 0.46.6
printf '%s\n' "$U1_PW" > "$tmp/u1pw"
check_in "import u1 (Tauri-format key file)" "$tmp/u1pw" 0 "U1 imported" "$ADM" import u1 "$tmp/keys/u1.tauri.key" --password-stdin
[ "$(stat -f %Lp "$TROOT/var/k2sign/keys/u1.key")" = 600 ] && [ "$(stat -f %Lp "$TROOT/var/k2sign/keys/u1.pw")" = 600 ] \
    && pass "u1.key and u1.pw are 0600" || fail "u1 modes"
check_in "a second import is refused" "$tmp/u1pw" 3 "already imported" "$ADM" import u1 "$tmp/keys/u1.tauri.key" --password-stdin
check "import needs an absolute path" 2 "absolute path" "$ADM" import u1 relative.key --password-stdin
check "preflight with no U2 pinned (pre-ceremony)" 3 "no U2 public key pinned" "$K" preflight 0.46.6

echo "== generate-u2 inside the signer: random password, never shown =="
check "generate-u2" 0 "its password is random and was never shown" "$ADM" generate-u2
U2_LINE="$(tr -d '[:space:]' < "$E/u2-generated.pub")"
grep -qF "$U2_LINE" "$tmp/out" && pass "generate-u2 prints the U2 public line" || fail "u2 line" "$(cat "$tmp/out")"
pw="$(cat "$TROOT/var/k2sign/keys/u2.pw")"
grep -qF -- "$pw" "$tmp/out" && fail "the U2 password was printed" || pass "the U2 password is not in the output"
[ "${#pw}" -ge 64 ] && pass "U2's password is 32 random bytes (hex)" || fail "u2 pw length" "${#pw}"
check "generate-u2 runs once" 3 "U2 already exists" "$ADM" generate-u2

echo "== the tree's U2 must be the one the signer made =="
printf 'x\nx\n' | "$MINISIGN" -G -p "$tmp/keys/other.pub" -s "$tmp/keys/other.key" >/dev/null 2>&1
fixture_keys "$U1_LINE" "$(sed -n 2p "$tmp/keys/other.pub")" "$U3_LINE"
if install_signer; then fail "install with a foreign U2 in the tree" "installed"; else
    grep -q "is not one of this tree's primary keys" "$tmp/install.out" && pass "install refuses a tree without this Mac's generated key" || fail "u2 mismatch" "$(tail -2 "$tmp/install.out")"
fi
fixture_keys "$U1_LINE" "$U2_LINE" "$U3_LINE"
install_signer && grep -q "this Mac's primary key (generate-u2) is one of the tree's 1 primaries" "$tmp/install.out" && pass "re-install from the pubkeys tree pins U2/U3" || fail "re-install" "$(tail -3 "$tmp/install.out")"
# Two Macs: the tree lists the other Mac's primary (U2b) FIRST; this Mac pins its own.
fixture_keys "$U1_LINE" "$U2_LINE" "$U3_LINE" "$(sed -n 2p "$tmp/keys/other.pub")"
install_signer && grep -q "one of the tree's 2 primaries" "$tmp/install.out" && grep -q "^u2 $U2_LINE\$" "$E/pubkeys" \
    && ! grep -qF "$(sed -n 2p "$tmp/keys/other.pub")" "$E/pubkeys" \
    && pass "two primaries in the tree: this Mac pins only its own key (the one generate-u2 made)" || fail "two primaries" "$(tail -3 "$tmp/install.out"; cat "$E/pubkeys")"
grep -q "^u2 $U2_LINE\$" "$E/pubkeys" && grep -q "^u3 $U3_LINE\$" "$E/pubkeys" && pass "pubkeys pins U2 and U3" || fail "pinned" "$(cat "$E/pubkeys")"

echo "== preflight =="
check "preflight needs an enrolled approver (policy approval on)" 3 "no approver key enrolled" "$K" preflight 0.46.6
/usr/bin/openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/keys/approver.key" 2>/dev/null
/usr/bin/openssl ec -in "$tmp/keys/approver.key" -pubout -out "$tmp/keys/approver.pem" 2>/dev/null
/usr/bin/openssl ecparam -name secp384r1 -genkey -noout -out "$tmp/keys/p384.key" 2>/dev/null
/usr/bin/openssl ec -in "$tmp/keys/p384.key" -pubout -out "$tmp/keys/p384.pem" 2>/dev/null
check "set-approver refuses a non-P-256 key" 3 "not a P-256 public key" "$ADM" set-approver "$tmp/keys/p384.pem"
check "set-approver" 0 "approver key installed" "$ADM" set-approver "$tmp/keys/approver.pem"
grep -q 'etc/approver.pem' "$E/pins.sha256" && pass "approver.pem is pinned" || fail "approver pin"
check "preflight needs the signer's own GitHub token (S3)" 3 "no GitHub token for the signer" "$K" preflight 0.46.6
printf 'not-a-token\n' > "$tmp/badtok"
check_in "set-gh-token refuses a non-token" "$tmp/badtok" 3 "doesn't look like a GitHub token" "$ADM" set-gh-token --stdin
printf 'ghp_%040d\n' 7 > "$tmp/wrongtok"
check_in "set-gh-token refuses a token GitHub rejects" "$tmp/wrongtok" 3 "GitHub API /repos/Alakazam-211/K2 failed" "$ADM" set-gh-token --stdin
if set_gh_token; then pass "set-gh-token (read-only token, probed against the repo)"; else fail "set-gh-token" "$(cat "$tmp/out")"; fi
grep -qF "$GH_TOKEN_VALUE" "$tmp/out" && fail "the token was printed" || pass "the token is not printed"
[ "$(stat -f %Lp "$TROOT/var/k2sign/gh/token")" = 600 ] && pass "gh/token is 0600" || fail "token mode"
check "preflight ok" 0 "preflight ok" "$K" preflight 0.46.6
U1_ID="$(python3 -I -c 'import base64,sys; print(base64.b64decode(sys.argv[1])[2:10][::-1].hex().upper())' "$U1_LINE")"
U2_ID="$(python3 -I -c 'import base64,sys; print(base64.b64decode(sys.argv[1])[2:10][::-1].hex().upper())' "$U2_LINE")"
grep -q "\"u1\": \"$U1_ID\"" "$tmp/out" && grep -q "\"u2\": \"$U2_ID\"" "$tmp/out" && pass "preflight reports the pinned key ids" || fail "ids" "$(cat "$tmp/out")"
n="$(grep -c $'\tSIGNED\t.*\tk2-preflight-self-test\t' "$AUDIT/0.46.6/signatures.tsv")"
[ "$n" = 2 ] && pass "two self-test SIGNED lines in the signer's audit log" || fail "self-test audit lines" "$n"
selfsha="$(printf 'K2 updater signing preflight 0.46.6\n' | shasum -a 256 | awk '{print $1}')"
grep -q $'\t'"$selfsha"$'\t' "$AUDIT/0.46.6/signatures.tsv" && pass "the audit line carries the sha256 of the signed bytes" || fail "audit sha"

echo "== S1 3–4: no session, not signable =="
printf 'daemon bytes\n' > "$tmp/f"
check_in "S1-3 sign without a session" "$tmp/f" 3 "no approved signing session for 0.46.6" "$K" sign u2 k2-daemon-linux-x86_64 0.46.6 raw
check_in "S1-4 an asset outside the allow-list" "$tmp/f" 3 "asset evil.bin is not signable" "$K" sign u2 evil.bin 0.46.6 raw
check_in "the self-test name is preflight's only" "$tmp/f" 3 "signed by preflight only" "$K" sign u2 k2-preflight-self-test 0.46.6 raw
check_in "U1 can't sign U2's Windows name" "$tmp/f" 3 "is not signable" "$K" sign u1 K2_0.46.6_x64-setup.exe 0.46.6 raw
check "an unknown verb is usage (2)" 2 "verbs:" "$K" export-keys
check "a path argument is no verb" 2 "usage" "$K" sign u2 /etc/passwd
check "a bad version is usage (2)" 2 "bad version" "$K" preflight 0.46

echo "== S1 5: a PATH trap is never run =="
trap_dir="$tmp/trap"; mkdir -p "$trap_dir"
for t in minisign python3 openssl env bash shasum; do
    printf '#!/bin/sh\necho trapped > "%s/marker"\nexit 0\n' "$trap_dir" > "$trap_dir/$t"; chmod +x "$trap_dir/$t"
done
PATH="$trap_dir:$PATH" PYTHONPATH="$trap_dir" PYTHONSTARTUP="$trap_dir/x.py" expect 0 "preflight ok" "$K" preflight 0.46.6 > "$tmp/r" \
    && [ ! -e "$trap_dir/marker" ] && pass "S1-5 preflight passes with PATH=trap:\$PATH and the trap never runs" \
    || fail "S1-5 PATH trap" "$(cat "$tmp/r"; ls "$trap_dir")"

echo "== S1 6: pins =="
cp "$E/policy" "$tmp/policy.bak"
printf '# edited\n' >> "$E/policy"
check "S1-6 a changed policy" 3 "pins mismatch: policy" "$K" preflight 0.46.6
cp "$tmp/policy.bak" "$E/policy"
cp "$L/k2sign.py" "$tmp/py.bak"; printf '\n# x\n' >> "$L/k2sign.py"
check "a changed signer" 3 "pins mismatch: k2sign.py" "$K" status 0.46.6
cp "$tmp/py.bak" "$L/k2sign.py"
cp "$E/pubkeys" "$tmp/pk.bak"; sed -i '' '/^u3 /d' "$E/pubkeys"
check "a changed pubkeys" 3 "pins mismatch: pubkeys" "$K" preflight 0.46.6
cp "$tmp/pk.bak" "$E/pubkeys"
cp "$L/minisign" "$tmp/ms.bak"; printf 'x' >> "$L/minisign"
check "a changed minisign" 3 "pins mismatch: minisign" "$K" preflight 0.46.6
cp "$tmp/ms.bak" "$L/minisign"
cp "$L/check-daemon-asset.py" "$tmp/cda.bak"; printf '\n# x\n' >> "$L/check-daemon-asset.py"
check "a changed check-daemon-asset.py" 3 "pins mismatch: check-daemon-asset.py" "$K" preflight 0.46.6
cp "$tmp/cda.bak" "$L/check-daemon-asset.py"
check "pins restored: preflight ok" 0 "preflight ok" "$K" preflight 0.46.6

echo "== key file modes =="
chmod 644 "$TROOT/var/k2sign/keys/u2.key"
check "a 0644 key is refused" 3 "must be mode 600" "$K" preflight 0.46.6
chmod 600 "$TROOT/var/k2sign/keys/u2.key"
mv "$TROOT/var/k2sign/keys/u1.pw" "$tmp/u1.pw.bak"
check "a missing key password file is refused" 3 "U1 key not installed: u1.pw missing" "$K" preflight 0.46.6
mv "$tmp/u1.pw.bak" "$TROOT/var/k2sign/keys/u1.pw"

echo "== who may run it: test mode refused for the signing user and root =="
K2SIGN_TEST_SIGNER_USER="$(id -un)" expect 3 "test mode (K2SIGN_TEST_ROOT) is refused" "$K" preflight 0.46.6 > "$tmp/r" \
    && pass "test mode is refused when this process is the signing user" || fail "test-mode refusal" "$(cat "$tmp/r")"
# The core itself (the wrappers hard-code the production path, absent here):
PYB="$(sed -n "s/^K2SIGN_PYTHON='\(.*\)'$/\1/p" "$L/k2sign")"
env -i PATH=/usr/bin:/bin "$PYB" -I "$L/k2sign.py" signer status 0.46.6 > "$tmp/o" 2>&1 && rc=0 || rc=$?
[ "$rc" = 3 ] && grep -q "k2sign runs only as _k2sign" "$tmp/o" && pass "production mode as another user: refused (3)" || fail "prod as user" "rc=$rc $(cat "$tmp/o")"
env -i PATH=/usr/bin:/bin "$PYB" -I "$L/k2sign.py" admin repin > "$tmp/o" 2>&1 && rc=0 || rc=$?
[ "$rc" = 3 ] && grep -q "runs only as root" "$tmp/o" && pass "k2sign-admin as a normal user: refused (3)" || fail "admin as user" "rc=$rc $(cat "$tmp/o")"
env -i PATH=/usr/bin:/bin K2SIGN_TEST_ROOT=relative/dir "$PYB" -I "$L/k2sign.py" signer status 0.46.6 > "$tmp/o" 2>&1 && rc=0 || rc=$?
[ "$rc" = 3 ] && grep -q "existing absolute directory" "$tmp/o" && pass "a relative test root is refused" || fail "relative test root" "rc=$rc $(cat "$tmp/o")"
grep -q 'if self.euid in refused_uids' "$L/k2sign.py" && grep -q 'refused_uids = {0}' "$L/k2sign.py" \
    && pass "(code) root is always in the refused set for test mode" || fail "root refusal code"

echo "== a real asset inside a session is still refused: the S3 checks aren't built =="
cp "$tmp/keys/approver.key" "$tmp/ak"
"$K" challenge 0.46.6 "$B1" k2-daemon-linux-x86_64 > "$tmp/ch.json"
approve_with "$tmp/ak" "$tmp/ch.json" "$tmp/ch.der"
check_in "approve" "$tmp/ch.der" 0 "" "$K" approve "$(nonce_of "$tmp/ch.json")"
check_in "a session alone never signs: the file needs its own evidence" "$tmp/f" 3 "no tag v0.46.6 on GitHub yet" "$K" sign u2 k2-daemon-linux-x86_64 0.46.6 raw
grep -q $'\tSIGNED\t.*k2-daemon-linux-x86_64' "$AUDIT/0.46.6/signatures.tsv" && fail "a refused asset was logged as signed" || pass "nothing was signed or logged as signed"

echo "== the sittings' dry runs print and change nothing =="
for n in "0" "1 --key U2" "2 --key U2" "2 --key U2b" "3 --key U2b" "reenrol --key U2"; do
    # shellcheck disable=SC2086
    if HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" $n --commit 0123456 > "$tmp/sit.out" 2>&1 \
        && grep -q -- "--run)" "$tmp/sit.out" && ! grep -q "Enter = run it" "$tmp/sit.out"; then
        pass "sitting.sh $n (dry run) prints its steps and how to run them"
    else fail "sitting.sh $n dry run" "$(tail -3 "$tmp/sit.out")"; fi
done
[ -z "$(find "$tmp/home" -mindepth 1 -print -quit 2>/dev/null)" ] && pass "the dry runs wrote nothing in HOME" || fail "dry runs wrote" "$(find "$tmp/home" | head -3)"
for n in "2 --key U2" "2 --key U2b"; do
    # shellcheck disable=SC2086
    HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" $n --commit 0123456 > "$tmp/sit.out" 2>&1
    awk '/Rebuild K2 Release Approve/{r=NR} /supports --timeout/{c=NR} /inside _k2sign/{g=NR} END{exit !(r && c && g && r<c && c<g)}' "$tmp/sit.out" \
        && pass "sitting $n rebuilds the approval app and checks --timeout before making any key" || fail "sitting $n app rebuild" "$(head -20 "$tmp/sit.out")"
done
# signer_current: the test install (from the fixture tree) matches that tree, and a
# changed installed k2sign.py or policy is caught (sitting 2/3 refuse a stale signer).
sc() { bash -c 'eval "$(sed -n "/^show()/,/^}/p;/^signer_current()/,/^}/p" "$1")"; HERE="$2"; TREE="$3"; signer_current "$4"' _ \
    "$REPO/scripts/k2sign/sitting.sh" "$TREE/scripts/k2sign" "$TREE" "$TROOT"; }
sc > "$tmp/sc.out" 2>&1 && pass "signer_current: the installed signer matches its checkout" || fail "signer_current" "$(cat "$tmp/sc.out")"
cp "$L/k2sign.py" "$tmp/py2.bak"; printf '\n# other commit\n' >> "$L/k2sign.py"
sc > "$tmp/sc.out" 2>&1 && fail "stale k2sign.py not caught" "" || { grep -q "differs from this checkout in: k2sign.py" "$tmp/sc.out" && pass "a signer installed from another commit is caught (k2sign.py)" || fail "stale msg" "$(cat "$tmp/sc.out")"; }
cp "$tmp/py2.bak" "$L/k2sign.py"
HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 2 --key U2 --commit 0123456 > "$tmp/sit.out" 2>&1
awk '/installed signer is this commit/{s=NR} /Rebuild K2 Release Approve/{r=NR} END{exit !(s && r && s<r)}' "$tmp/sit.out" \
    && pass "sitting 2 checks the installed signer first" || fail "sitting 2 signer check"
HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 3 --key U2 --commit 0123456 > "$tmp/sit.out" 2>&1
grep -q "supports --timeout" "$tmp/sit.out" && pass "sitting 3 checks the approval app supports --timeout" || fail "sitting 3 app check"
HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 0 --key U2b > "$tmp/sit.out" 2>&1
grep -q "ssh z3thon@z3mbp " "$tmp/sit.out" && pass "sitting 0 --key U2b prints the SSH line for z3mbp" || fail "sitting 0 host" "$(grep ssh "$tmp/sit.out")"
HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 2 --key U2b --commit 0123456 > "$tmp/sit.out" 2>&1
grep -q "U3 is not made here" "$tmp/sit.out" && grep -q "generate --signer k2sign --key-name U2b --skip-u3" "$tmp/sit.out" \
    && ! grep -q "NEW password for U3" "$tmp/sit.out" && pass "sitting 2 on z3mbp (U2b) makes no U3" || fail "U2b sitting 2" "$(head -5 "$tmp/sit.out")"
if HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 1 --commit 0123456 > "$tmp/sit.out" 2>&1; then fail "sitting 1 without --key" "ran"; else
    grep -q "add --key U2 (on z3mbpZ) or --key U2b (on z3mbp)" "$tmp/sit.out" && pass "sittings 1-3 and reenrol need --key" || fail "key gate" "$(cat "$tmp/sit.out")"; fi
if HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 1 --key U2 --commit 0123456 --run < /dev/null > "$tmp/sit.out" 2>&1; then fail "sitting 1 --run on the wrong Mac" "ran"; else
    grep -q "Key U2 belongs to z3mbpZ" "$tmp/sit.out" && pass "a sitting refuses to run on the other Mac" || fail "host gate" "$(cat "$tmp/sit.out")"; fi
if HOME="$tmp/home" bash "$REPO/scripts/k2sign/sitting.sh" 1 --key U2 --run < /dev/null > "$tmp/sit.out" 2>&1; then fail "sitting 1 --run without --commit" "ran"; else
    grep -qE "Add --commit <sha>|belongs to" "$tmp/sit.out" && pass "sitting 1 --run refuses without --commit (or on another Mac)" || fail "commit gate" "$(cat "$tmp/sit.out")"; fi

echo "== sitting 2's agent check: zero agents passes; one allowed claude; every other agent refuses =="
# Fake agents whose command lines are unique to this run ("claude 3517",
# "grok 3517"); the pattern is narrowed to them so real agents on the test
# machine don't count.
SIT="$REPO/scripts/k2sign/sitting.sh"
export K2SIGN_SITTING_AGENT_RE="(claude|grok) 3517"
ag() { HOME="$tmp/home" bash "$SIT" agents "$@" > "$tmp/ag.out" 2>&1; }
if ag; then grep -q "no other agent runs as" "$tmp/ag.out" && pass "zero agents: the check passes and says so (not a silent exit)" || fail "zero agents msg" "$(cat "$tmp/ag.out")"
else fail "zero agents" "rc!=0: $(cat "$tmp/ag.out")"; fi
bash -c 'exec -a claude sleep 3517' & A_PID=$!
bash -c 'exec -a grok sleep 3517' & G_PID=$!
sleep 0.5
if ag; then fail "an agent running, none allowed" "passed"; else
    grep -q "2 other agent process(es)" "$tmp/ag.out" && grep -q "$A_PID" "$tmp/ag.out" && grep -q "$G_PID" "$tmp/ag.out" \
        && pass "two agents, none allowed: refused, and both are listed" || fail "list" "$(cat "$tmp/ag.out")"; fi
if ag --allow-agent "$A_PID"; then fail "allowed claude + another agent" "passed"; else
    grep -q "ALLOWED.*pid $A_PID" "$tmp/ag.out" && grep -q "1 other agent process(es)" "$tmp/ag.out" && grep -q "$G_PID.*grok 3517" "$tmp/ag.out" \
        && pass "the allowed claude is excused; another agent still refuses (and is listed)" || fail "allow + other" "$(cat "$tmp/ag.out")"; fi
if ag --allow-agent "$G_PID"; then fail "a grok as the allowed agent" "passed"; else
    grep -q "is not a live claude process" "$tmp/ag.out" && pass "--allow-agent refuses a pid that isn't claude" || fail "non-claude" "$(cat "$tmp/ag.out")"; fi
kill "$G_PID" 2>/dev/null; wait "$G_PID" 2>/dev/null || true
if ag --allow-agent "$A_PID"; then
    grep -q "ALLOWED.*pid $A_PID" "$tmp/ag.out" && grep -q "no other agent runs as" "$tmp/ag.out" \
        && pass "only the allowed claude left: passes, and says one agent was allowed" || fail "allowed only" "$(cat "$tmp/ag.out")"
else fail "allowed only" "$(cat "$tmp/ag.out")"; fi
kill "$A_PID" 2>/dev/null; wait "$A_PID" 2>/dev/null || true
for bogus in "$A_PID" 99999999 abc ""; do
    if ag --allow-agent "$bogus"; then fail "--allow-agent '$bogus'" "passed"; else
        grep -q "is not a live claude process" "$tmp/ag.out" && pass "--allow-agent '${bogus:-<empty>}' (dead, bogus or empty) refuses" || fail "bogus $bogus" "$(cat "$tmp/ag.out")"; fi
done
if HOME="$tmp/home" bash "$SIT" 1 --key U2 --allow-agent 1 > "$tmp/ag.out" 2>&1; then fail "--allow-agent on sitting 1" "accepted"; else
    grep -q "only for sitting 2" "$tmp/ag.out" && pass "--allow-agent is refused outside sitting 2" || fail "allow scope" "$(cat "$tmp/ag.out")"; fi
HOME="$tmp/home" bash "$SIT" 2 --key U2b --commit 0123456 --allow-agent 4242 > "$tmp/sit.out" 2>&1
grep -q "ONE agent stays (--allow-agent 4242" "$tmp/sit.out" && grep -q "Secure Keyboard Entry" "$tmp/sit.out" && ! grep -q "run 'k2 daemon stop'" "$tmp/sit.out" \
    && grep -q "Summary: one agent was allowed" "$tmp/sit.out" && grep -q "copy any other text so it isn't left on the clipboard" "$tmp/sit.out" \
    && [ "$(grep -c 'still no other agent running (before a password step)' "$tmp/sit.out")" = 1 ] \
    && pass "sitting 2 --allow-agent: says so at the start and in the summary, keeps the daemon, re-checks before the password step, keeps the clipboard advice" \
    || fail "sitting 2 allow text" "$(head -30 "$tmp/sit.out")"
unset K2SIGN_SITTING_AGENT_RE
# agents_running (check's count) under pipefail with NO match: a count, not a silent exit (e283277cc).
out="$(bash -c 'set -euo pipefail; AGENT_RE="k2sign-no-such-process-$$"; eval "$(sed -n "/^agents_running()/p" "$1")"; n="$(agents_running)"; echo "n=$n"' _ "$SIT" 2>&1)" && rc=0 || rc=$?
[ "$rc" = 0 ] && [ "$out" = "n=0" ] && pass "agents_running with no match under pipefail: prints 0 and goes on" || fail "agents_running zero" "rc=$rc out=$out"

echo "== K2 Release Approve: Touch ID OR the login password, asked every time =="
SW="$REPO/scripts/k2sign/approve/main.swift"
code="$(grep -v '^[[:space:]]*//' "$SW")"
grep -qF '[.privateKeyUsage, .biometryAny, .or, .devicePasscode]' <<<"$code" && pass "the key's access control is Touch ID (any finger) or the device password" || fail "access control flags"
! grep -qE '\.biometryCurrentSet|\.userPresence|\.watch|\.applicationPassword' <<<"$code" && pass "no other factor (no .userPresence, Apple Watch or app password)" || fail "extra factors"
grep -qF 'touchIDAuthenticationAllowableReuseDuration = 0' <<<"$code" && ! grep -q 'evaluatePolicy(' <<<"$code" \
    && pass "nothing is pre-evaluated or reused: the key operation asks every time" || fail "reuse / pre-evaluation"
grep -q 'interactionNotAllowed = false' <<<"$code" && grep -q -- '--replace' <<<"$code" && pass "the sheet is required; --enrol --replace renames the old key" || fail "sheet / replace"
grep -q 'arg("--timeout")' <<<"$code" && grep -q 'ctx.invalidate()' <<<"$code" && grep -q 'no approval (timed out after' <<<"$code" \
    && pass "--timeout cancels an unanswered sheet (invalidate) and reports 'no approval (timed out …)'" || fail "helper timeout"
! grep -qiE "SSH session can.t (show|approve)" "$REPO/docs/release/updater-key-ceremony.md" "$REPO/scripts/k2sign/sitting.sh" "$SW" \
    && pass "no doc claims an SSH-started helper can't show the sheet (it can; the person answering is the boundary)" || fail "stale SSH claim"

echo "== S1 7: every signature verifies with the pinned U2 and is in the audit log =="
printf 'some daemon\n' > "$tmp/d1"
K2SIGN_TEST_EVIDENCE=accept "$K" sign u2 k2-daemon-linux-x86_64 0.46.6 raw < "$tmp/d1" > "$tmp/d1.sig" 2>"$tmp/e" || fail "sign (evidence stubbed)" "$(cat "$tmp/e")"
"$MINISIGN" -V -P "$U2_LINE" -m "$tmp/d1" -x "$tmp/d1.sig" -q && pass "S1-7 the signature verifies with minisign -V -P <pinned U2>" || fail "S1-7 verify"
d1sha="$(shasum -a 256 "$tmp/d1" | awk '{print $1}')"
grep -q $'\tSIGNED\tu2\tk2-daemon-linux-x86_64\t'"$d1sha"$'\t'"$U2_ID" "$AUDIT/0.46.6/signatures.tsv" \
    && pass "S1-7 its audit line has the sha256 of the stdin bytes and U2's id" || fail "S1-7 audit" "$(tail -2 "$AUDIT/0.46.6/signatures.tsv")"

finish
