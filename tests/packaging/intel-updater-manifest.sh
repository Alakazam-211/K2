#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER="$ROOT/scripts/intel-updater-manifest.py"
PYTHON="$(command -v python3)"
tmp="$(mktemp -d)"
trap 'echo "RETAINED test evidence: $tmp"' EXIT
export TMPDIR="$tmp"
printf 'sentinel\n' >"$tmp/sentinel"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_fail() {
    local label="$1"; shift
    if "$@" >"$tmp/out" 2>"$tmp/err"; then fail "$label"; else pass "$label"; fi
}
expect_fail_contains() {
    local label="$1" needle="$2"; shift 2
    if "$@" >"$tmp/out" 2>"$tmp/err"; then
        fail "$label (unexpected success)"
    elif grep -F -q "$needle" "$tmp/err"; then
        pass "$label"
    else
        fail "$label (wrong gate: $(cat "$tmp/err"))"
    fi
}

mkdir -p "$tmp/bin"
cat >"$tmp/bin/minisign" <<'SH'
#!/bin/sh
[ -z "${HARNESS_LOG:-}" ] || echo "minisign $*" >>"$HARNESS_LOG"
archive=""; pub=""; sig=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -Vm) archive="$2"; shift 2 ;;
        -p) pub="$2"; shift 2 ;;
        -x) sig="$2"; shift 2 ;;
        *) shift ;;
    esac
done
[ "$(cat "$archive")" = "signed intel" ] \
    && grep -q 'RWQFu172RhQR6QBqzmij2E9btDGhDWmpdH+ah1/M4PmuHPINUWvKLf6o' "$pub" \
    && grep -q 'timestamp:1556193335' "$sig"
SH
chmod +x "$tmp/bin/minisign"
export PATH="$tmp/bin:$PATH"

raw_sig='untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1556193335 file:test
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg=='
wrapped_sig="$(printf '%s' "$raw_sig" | base64 | tr -d '\n\r')"

cat >"$tmp/latest.json" <<'JSON'
{"version":"0.41.3","platforms":{"darwin-aarch64":{"signature":"arm","url":"https://example/arm"},"windows-x86_64":{"signature":"win","url":"https://example/win"}}}
JSON
printf 'signed intel\n' >"$tmp/K2_0.41.3_x86_64.app.tar.gz"
printf '%s\n' "$wrapped_sig" >"$tmp/K2_0.41.3_x86_64.app.tar.gz.sig"
url='https://github.com/Alakazam-211/K2/releases/download/v0.41.3/K2_0.41.3_x86_64.app.tar.gz'
config="$ROOT/src-tauri/tauri.conf.json"

echo "== Intel updater manifest seam =="
stage_record="$(python3 "$HELPER" stage 0.41.3 \
    "$tmp/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config")"
stage="${stage_record%%$'\t'*}"
stage_token="${stage_record#*$'\t'}"
staged_archive="$stage/K2_0.41.3_x86_64.app.tar.gz"
staged_signature="$staged_archive.sig"
python3 "$HELPER" merge "$tmp/latest.json" 0.41.3 \
    "$staged_archive" "$staged_signature" "$url" "$config"

python3 - "$tmp/latest.json" "$url" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert set(data["platforms"]) == {"darwin-aarch64", "darwin-x86_64", "windows-x86_64"}
assert data["platforms"]["darwin-x86_64"]["url"] == sys.argv[2]
assert data["platforms"]["darwin-x86_64"]["signature"]
PY
pass "merge preserves platforms and adds Intel URL/signature"

python3 "$HELPER" verify "$tmp/latest.json" 0.41.3 "$url" \
    "$staged_archive" "$staged_signature" "$config"
pass "verification accepts the exact Intel entry"

cp "$tmp/latest.json" "$tmp/empty-endpoint-signature.json"
python3 - "$tmp/empty-endpoint-signature.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["platforms"]["darwin-x86_64"]["signature"] = ""
with open(p, "w") as f: json.dump(d, f)
PY
expect_fail "verification rejects an empty Intel signature" python3 "$HELPER" verify \
    "$tmp/empty-endpoint-signature.json" 0.41.3 "$url" \
    "$staged_archive" "$staged_signature" "$config"
expect_fail "verification rejects a mismatched Intel URL" python3 "$HELPER" verify \
    "$tmp/latest.json" 0.41.3 "${url}.wrong" \
    "$staged_archive" "$staged_signature" "$config"

expect_fail "duplicate Intel platform is rejected" python3 "$HELPER" merge \
    "$tmp/latest.json" 0.41.3 "$staged_archive" "$staged_signature" "$url" "$config"

for bad in malformed-json duplicate-json-key; do
    cp "$tmp/latest.json" "$tmp/$bad.json"
    python3 - "$tmp/$bad.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["platforms"].pop("darwin-x86_64")
with open(p, "w") as f: json.dump(d, f)
PY
done
printf '{not json\n' >"$tmp/malformed-json.json"
printf '{"version":"0.41.3","version":"0.41.3","platforms":{}}\n' >"$tmp/duplicate-json-key.json"

expect_fail_contains "missing archive is rejected at the file gate" "Intel archive is unavailable" \
    python3 "$HELPER" stage 0.41.3 "$tmp/missing/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/missing/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"
mkdir -p "$tmp/missing-signature"
printf 'signed intel\n' >"$tmp/missing-signature/K2_0.41.3_x86_64.app.tar.gz"
expect_fail_contains "missing signature is rejected at the file gate" "Intel signature is unavailable" \
    python3 "$HELPER" stage 0.41.3 "$tmp/missing-signature/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/missing-signature/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"

mkdir -p "$tmp/arbitrary"
printf 'signed intel\n' >"$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz"
printf 'YQ==\n' >"$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig"
expect_fail_contains "arbitrary base64 is not a Tauri/minisign signature" \
    "not a Tauri-wrapped minisign signature" python3 "$HELPER" stage 0.41.3 \
    "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"
printf '%s\n' '%%%' >"$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig"
expect_fail_contains "malformed base64 signature is rejected" "malformed Intel signature" \
    python3 "$HELPER" stage 0.41.3 "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"
printf 'YQ==\n' >"$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig"
chmod 444 "$tmp/arbitrary/"* && chmod 555 "$tmp/arbitrary"
printf '{"version":"0.41.3","platforms":{}}\n' >"$tmp/arbitrary-manifest.json"
expect_fail_contains "arbitrary base64 cannot pass merge" \
    "not a Tauri-wrapped minisign signature" python3 "$HELPER" merge \
    "$tmp/arbitrary-manifest.json" 0.41.3 "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"
expect_fail_contains "arbitrary base64 cannot pass endpoint verification" \
    "not a Tauri-wrapped minisign signature" python3 "$HELPER" verify \
    "$tmp/latest.json" 0.41.3 "$url" "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/arbitrary/K2_0.41.3_x86_64.app.tar.gz.sig" "$config"
chmod 755 "$tmp/arbitrary" && chmod 600 "$tmp/arbitrary/"*

mkdir -p "$tmp/tampered"
printf 'tampered intel\n' >"$tmp/tampered/K2_0.41.3_x86_64.app.tar.gz"
printf '%s\n' "$wrapped_sig" >"$tmp/tampered/K2_0.41.3_x86_64.app.tar.gz.sig"
expect_fail_contains "signature must bind the exact archive bytes" "minisign verification failed" \
    python3 "$HELPER" stage 0.41.3 "$tmp/tampered/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/tampered/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"

expect_fail_contains "malformed URL is rejected at the URL gate" "malformed Intel updater URL" \
    python3 "$HELPER" stage 0.41.3 "$tmp/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/K2_0.41.3_x86_64.app.tar.gz.sig" "http://example/intel" "$config"
expect_fail_contains "version mismatch is rejected at the name gate" \
    "Intel archive must be named K2_0.41.4_x86_64.app.tar.gz" \
    python3 "$HELPER" stage 0.41.4 "$tmp/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"
expect_fail "malformed JSON is rejected" python3 "$HELPER" merge "$tmp/malformed-json.json" \
    0.41.3 "$staged_archive" "$staged_signature" "$url" "$config"
expect_fail "duplicate JSON keys are rejected" python3 "$HELPER" merge "$tmp/duplicate-json-key.json" \
    0.41.3 "$staged_archive" "$staged_signature" "$url" "$config"

printf 'swapped source\n' >"$tmp/K2_0.41.3_x86_64.app.tar.gz"
if [ "$(cat "$staged_archive")" = "signed intel" ]; then
    pass "source-path swap cannot change the staged archive"
else
    fail "source-path swap cannot change the staged archive"
fi
python3 - "$stage" "$staged_archive" "$staged_signature" <<'PY'
import os, stat, sys
assert os.lstat(sys.argv[1]).st_mode & 0o222 == 0
for path in sys.argv[2:]:
    mode = os.lstat(path).st_mode
    assert stat.S_ISREG(mode) and mode & 0o222 == 0
PY
pass "stage directory and snapshot files are non-writable regular paths"

mkdir -p "$tmp/symlink"
ln -s "$staged_archive" "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz"
printf '%s\n' "$wrapped_sig" >"$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz.sig"
expect_fail_contains "symlink archive is rejected" "regular non-symlink" \
    python3 "$HELPER" stage 0.41.3 "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"
rm "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz"
printf 'signed intel\n' >"$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz"
rm "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz.sig"
ln -s "$staged_signature" "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz.sig"
expect_fail_contains "symlink signature is rejected" "regular non-symlink" \
    python3 "$HELPER" stage 0.41.3 "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/symlink/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"

mkdir -p "$tmp/collision"
printf 'signed intel\n' >"$tmp/collision/K2_0.41.3_x86_64.app.tar.gz"
ln "$tmp/collision/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/collision/K2_0.41.3_x86_64.app.tar.gz.sig"
expect_fail_contains "archive/signature inode collision is rejected" "must be distinct files" \
    python3 "$HELPER" stage 0.41.3 "$tmp/collision/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/collision/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"

mkdir -p "$tmp/no-verifier"
printf 'signed intel\n' >"$tmp/no-verifier/K2_0.41.3_x86_64.app.tar.gz"
printf '%s\n' "$wrapped_sig" >"$tmp/no-verifier/K2_0.41.3_x86_64.app.tar.gz.sig"
mkdir -p "$tmp/empty-path"
expect_fail_contains "missing verifier fails closed" "minisign verifier is required" \
    env PATH="$tmp/empty-path" "$PYTHON" "$HELPER" stage 0.41.3 \
    "$tmp/no-verifier/K2_0.41.3_x86_64.app.tar.gz" \
    "$tmp/no-verifier/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config"

echo "== Helper-owned staging cleanup =="
python3 "$HELPER" cleanup 0.41.3 "$stage" "$stage_token"
if [ ! -e "$stage" ] && [ -f "$tmp/sentinel" ]; then
    pass "cleanup removes only the exact helper-owned stage"
else
    fail "cleanup removes only the exact helper-owned stage"
fi
python3 "$HELPER" cleanup 0.41.3 "$stage" "$stage_token"
pass "repeated cleanup of the exact absent stage is a no-op"

printf 'signed intel\n' >"$tmp/K2_0.41.3_x86_64.app.tar.gz"
new_stage() {
    local record
    record="$(python3 "$HELPER" stage 0.41.3 \
        "$tmp/K2_0.41.3_x86_64.app.tar.gz" \
        "$tmp/K2_0.41.3_x86_64.app.tar.gz.sig" "$url" "$config")"
    new_stage_dir="${record%%$'\t'*}"
    new_stage_token="${record#*$'\t'}"
}

new_stage
expect_fail_contains "wrong cleanup token refuses" "owner token does not match" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" \
    0000000000000000000000000000000000000000000000000000000000000000
[ -d "$new_stage_dir" ] && pass "wrong token leaves the stage intact" \
    || fail "wrong token leaves the stage intact"
python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
expect_fail_contains "malformed cleanup token refuses" "64 lowercase hexadecimal" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" BAD
python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
chmod 755 "$new_stage_dir"
expect_fail_contains "wrong staging-directory mode refuses" "wrong owner or mode" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"
chmod 555 "$new_stage_dir"
python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
chmod 700 "$new_stage_dir"
chmod 644 "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz"
chmod 555 "$new_stage_dir"
expect_fail_contains "wrong entry mode refuses" "wrong type, owner, links, size, or mode" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"
chmod 700 "$new_stage_dir"
chmod 444 "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz"
chmod 555 "$new_stage_dir"
python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
chmod 700 "$new_stage_dir"
printf 'extra\n' >"$new_stage_dir/extra"
chmod 555 "$new_stage_dir"
expect_fail_contains "extra staging entry refuses" "missing or unexpected entries" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"
[ -f "$new_stage_dir/extra" ] && pass "extra-entry refusal performs no deletion" \
    || fail "extra-entry refusal performs no deletion"

new_stage
chmod 700 "$new_stage_dir"
rm "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz.sig"
chmod 555 "$new_stage_dir"
expect_fail_contains "missing staging entry refuses" "missing or unexpected entries" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
chmod 700 "$new_stage_dir"
rm "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz.sig"
ln "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz" \
    "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz.sig"
chmod 555 "$new_stage_dir"
expect_fail_contains "hard-linked staging entries refuse" "wrong type, owner, links, size, or mode" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
chmod 700 "$new_stage_dir"
rm "$new_stage_dir/.k2-intel-updater-owner"
ln -s "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz" \
    "$new_stage_dir/.k2-intel-updater-owner"
chmod 555 "$new_stage_dir"
expect_fail "symlinked owner marker refuses" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
chmod 700 "$new_stage_dir"
rm "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz.sig"
ln -s "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz" \
    "$new_stage_dir/K2_0.41.3_x86_64.app.tar.gz.sig"
chmod 555 "$new_stage_dir"
expect_fail "symlinked staged file refuses" \
    python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

new_stage
ln -s "$new_stage_dir" "$tmp/k2-intel-updater-symlink"
expect_fail_contains "symlinked staging directory refuses" "non-symlink directory" \
    python3 "$HELPER" cleanup 0.41.3 "$tmp/k2-intel-updater-symlink" "$new_stage_token"
python3 "$HELPER" cleanup 0.41.3 "$new_stage_dir" "$new_stage_token"

expect_fail_contains "outside-temp staging path refuses" "directly under the temporary directory" \
    python3 "$HELPER" cleanup 0.41.3 "/private/k2-intel-updater-outside" \
    0000000000000000000000000000000000000000000000000000000000000000
[ -f "$tmp/sentinel" ] && pass "sentinel sibling survives every cleanup path" \
    || fail "sentinel sibling survives every cleanup path"

echo "== Command-stubbed release.sh seam =="
harness="$tmp/harness"
mkdir -p "$harness/scripts" "$harness/src-tauri" "$harness/bin" \
    "$harness/target/release/bundle/dmg" "$harness/target/release/bundle/macos"
cp "$ROOT/scripts/release.sh" "$ROOT/scripts/intel-updater-manifest.py" "$harness/scripts/"
mv "$harness/scripts/intel-updater-manifest.py" "$harness/scripts/intel-updater-manifest-real.py"
cat >"$harness/scripts/intel-updater-manifest.py" <<'PY'
#!/usr/bin/env python3
import os, sys
if len(sys.argv) > 1 and sys.argv[1] == "stage" and os.environ.get("HARNESS_STAGE_MALFORMED"):
    print("malformed")
    raise SystemExit(0)
if len(sys.argv) > 1 and sys.argv[1] == "cleanup" and os.environ.get("HARNESS_CLEANUP_FAIL"):
    raise SystemExit(66)
os.execv(sys.executable, [sys.executable, os.path.join(os.path.dirname(__file__),
    "intel-updater-manifest-real.py"), *sys.argv[1:]])
PY
cp "$ROOT/src-tauri/tauri.conf.json" "$harness/src-tauri/"
cp "$tmp/bin/minisign" "$harness/bin/"
printf 'dmg\n' >"$harness/target/release/bundle/dmg/K2_0.41.3_aarch64.dmg"
printf 'daemon\n' >"$harness/target/release/k2-daemon"
printf 'arm archive\n' >"$harness/target/release/bundle/macos/K2.app.tar.gz"
printf 'notes\n' >"$harness/notes.md"

cat >"$harness/scripts/require-mail-oauth-build-env.sh" <<'SH'
require_mail_oauth_build_env() { :; }
assert_daemon_oauth_not_placeholder() { :; }
SH
cat >"$harness/scripts/publish-web-bundles.sh" <<'SH'
#!/bin/sh
echo "publish-web $*" >>"$HARNESS_LOG"
SH
cat >"$harness/bin/cargo" <<'SH'
#!/bin/sh
echo "CARGO-CALLED $*" >>"$HARNESS_LOG"
exit 91
SH
cat >"$harness/bin/bunx" <<'SH'
#!/bin/sh
echo "bunx $*" >>"$HARNESS_LOG"
printf '%s\n' "$HARNESS_WRAPPED_SIG" >"$4.sig"
SH
cat >"$harness/bin/git" <<'SH'
#!/bin/sh
echo "git $*" >>"$HARNESS_LOG"
if [ "$1 ${2:-} ${3:-}" = "diff --cached --quiet" ]; then exit 0; fi
if [ "$1 ${2:-} ${3:-}" = "rev-parse --short HEAD" ]; then echo deadbee; fi
SH
cat >"$harness/bin/gh" <<'SH'
#!/bin/sh
printf 'gh' >>"$HARNESS_LOG"
for arg in "$@"; do printf ' <%s>' "$arg" >>"$HARNESS_LOG"; done
printf '\n' >>"$HARNESS_LOG"
exit "${HARNESS_GH_RC:-73}"
SH
cat >"$harness/bin/curl" <<'SH'
#!/bin/sh
case "$*" in
    *daemon-latest.json*) cat "$HARNESS_ROOT/target/release/daemon-dist/daemon-latest.json" ;;
    *) cat /tmp/latest.json ;;
esac
SH
chmod +x "$harness/scripts/"*.sh "$harness/bin/"*

mkdir -p "$tmp/harness-source"
harness_archive="$tmp/harness-source/K2_0.41.3_x86_64.app.tar.gz"
harness_signature="$harness_archive.sig"
printf 'signed intel\n' >"$harness_archive"
printf '%s\n' "$wrapped_sig" >"$harness_signature"

run_harness() {
    local mode="$1"
    : >"$tmp/harness.log"
    set +e
    (
        export PATH="$harness/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        export HARNESS_LOG="$tmp/harness.log" HARNESS_WRAPPED_SIG="$wrapped_sig"
        export HARNESS_ROOT="$harness"
        unset HARNESS_GH_RC HARNESS_CLEANUP_FAIL HARNESS_STAGE_MALFORMED
        export K2_RELEASE_RESUME=8 K2_RELEASE_REPO=Example/Private K2_SKIP_WINDOWS_NSIS=1
        export TAURI_SIGNING_PRIVATE_KEY=fake TAURI_SIGNING_PRIVATE_KEY_PASSWORD=fake
        export K2_GMAIL_CLIENT_ID=fake K2_GMAIL_CLIENT_SECRET=fake HOME="$harness/home"
        unset K2_INTEL_UPDATER_ARCHIVE K2_INTEL_UPDATER_SIGNATURE
        case "$mode" in
            valid)
                export K2_INTEL_UPDATER_ARCHIVE="$harness_archive"
                export K2_INTEL_UPDATER_SIGNATURE="$harness_signature"
                ;;
            success)
                export K2_INTEL_UPDATER_ARCHIVE="$harness_archive"
                export K2_INTEL_UPDATER_SIGNATURE="$harness_signature"
                export HARNESS_GH_RC=0
                ;;
            cleanup-failure-primary)
                export K2_INTEL_UPDATER_ARCHIVE="$harness_archive"
                export K2_INTEL_UPDATER_SIGNATURE="$harness_signature"
                export HARNESS_CLEANUP_FAIL=1
                ;;
            cleanup-failure-success)
                export K2_INTEL_UPDATER_ARCHIVE="$harness_archive"
                export K2_INTEL_UPDATER_SIGNATURE="$harness_signature"
                export HARNESS_CLEANUP_FAIL=1 HARNESS_GH_RC=0
                ;;
            malformed-stage)
                export K2_INTEL_UPDATER_ARCHIVE="$harness_archive"
                export K2_INTEL_UPDATER_SIGNATURE="$harness_signature"
                export HARNESS_STAGE_MALFORMED=1
                ;;
            partial) export K2_INTEL_UPDATER_ARCHIVE="$harness_archive" ;;
            partial-signature) export K2_INTEL_UPDATER_SIGNATURE="$harness_signature" ;;
        esac
        bash "$harness/scripts/release.sh" 0.41.3 "$harness/notes.md"
    ) >"$tmp/$mode.out" 2>"$tmp/$mode.err"
    harness_rc=$?
    set -e
}

run_harness partial
if [ "$harness_rc" -eq 1 ] && [ ! -s "$tmp/harness.log" ] \
    && grep -F -q 'must be supplied together' "$tmp/partial.err"; then
    pass "partial release inputs stop before verifier/build/sign/git/gh"
else
    fail "partial release inputs stop before verifier/build/sign/git/gh"
fi

run_harness partial-signature
if [ "$harness_rc" -eq 1 ] && [ ! -s "$tmp/harness.log" ] \
    && grep -F -q 'must be supplied together' "$tmp/partial-signature.err"; then
    pass "signature-only release input stops before verifier/build/sign/git/gh"
else
    fail "signature-only release input stops before verifier/build/sign/git/gh"
fi

run_harness none
cp /tmp/latest.json "$tmp/latest-none.json"
if [ "$harness_rc" -eq 73 ] && grep -F -q 'gh <release> <create>' "$tmp/harness.log" \
    && ! grep -F -q 'darwin-x86_64' "$tmp/latest-none.json" \
    && ! grep -F -q 'minisign ' "$tmp/harness.log"; then
    pass "neither-input release preserves the arm64-only path"
else
    fail "neither-input release preserves the arm64-only path"
fi

run_harness valid
cp /tmp/latest.json "$tmp/latest-valid.json"
stage_path="$(sed -n 's/.*verified + snapshotted: //p' "$tmp/valid.out" | head -1)"
if [ "$harness_rc" -eq 73 ] && [ -n "$stage_path" ] && [ ! -e "$stage_path" ] \
    && grep -F -q "<$stage_path/K2_0.41.3_x86_64.app.tar.gz>" "$tmp/harness.log" \
    && grep -F -q "<$stage_path/K2_0.41.3_x86_64.app.tar.gz.sig>" "$tmp/harness.log" \
    && ! grep -F -q "<$harness_archive>" "$tmp/harness.log" \
    && grep -F -q 'https://github.com/Example/Private/releases/download/v0.41.3/K2_0.41.3_x86_64.app.tar.gz' "$tmp/latest-valid.json" \
    && grep -F -q 'minisign ' "$tmp/harness.log" \
    && ! grep -F -q 'CARGO-CALLED' "$tmp/harness.log"; then
    pass "valid release uses verified snapshot assets and custom-repo manifest URL"
else
    fail "valid release uses verified snapshot assets and custom-repo manifest URL"
    cat "$tmp/valid.err" >&2
    cat "$tmp/harness.log" >&2
fi

run_harness success
success_stage_path="$(sed -n 's/.*verified + snapshotted: //p' "$tmp/success.out" | head -1)"
if [ "$harness_rc" -eq 0 ] && [ -n "$success_stage_path" ] \
    && [ ! -e "$success_stage_path" ]; then
    pass "successful release harness removes its helper-owned stage"
else
    fail "successful release harness removes its helper-owned stage"
fi

run_harness cleanup-failure-primary
if [ "$harness_rc" -eq 73 ] \
    && grep -F -q 'secondary Intel staging cleanup failed' "$tmp/cleanup-failure-primary.err"; then
    pass "cleanup failure preserves the primary release status"
else
    fail "cleanup failure preserves the primary release status"
fi

run_harness cleanup-failure-success
if [ "$harness_rc" -eq 66 ]; then
    pass "cleanup-only failure makes successful release harness nonzero"
else
    fail "cleanup-only failure makes successful release harness nonzero"
fi

run_harness malformed-stage
if [ "$harness_rc" -eq 1 ] && grep -F -q 'malformed Intel staging record' \
    "$tmp/malformed-stage.err" && ! grep -F -q 'gh ' "$tmp/harness.log"; then
    pass "malformed stage output refuses before release work"
else
    fail "malformed stage output refuses before release work"
fi

if [ "$fail" -ne 0 ]; then
    echo "FAILED: $fail test(s)" >&2
    exit 1
fi
echo "OK: $pass tests"
