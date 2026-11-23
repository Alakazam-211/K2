#!/usr/bin/env bash
# The root helper's size gate (scripts/lib/root-helper-size.sh).
#
# Helpers already on boxes (0.46.17 and older) refuse a self-update payload
# (<u32 sig len><sig><ELF>) over 8 MiB, so a release whose
# k2-root-helper-linux-<arch> doesn't fit could never reach them by
# self-update (0.46.17's did not: 8,836,200 bytes). The gate fails such a
# build in the Linux build job, at release staging and before signing.
#
#  - a helper at the limit passes and prints its size;
#  - one byte over fails, names the file, both numbers and why;
#  - a missing file fails (never a silent pass);
#  - several files: one bad one fails the lot, the good ones still print;
#  - the wiring: build-linux-bins.sh, k2cr_stage and sign-root-helper.sh
#    call the gate on the helper.
#
# Run with: bash tests/release/root-helper-size.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$ROOT/scripts/lib/root-helper-size.sh"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

[ -f "$LIB" ] || { echo "FAIL: $LIB does not exist" >&2; exit 1; }
# shellcheck source=scripts/lib/root-helper-size.sh
source "$LIB"

cap=8388608
room=4096
max=$((cap - room))
[ "$K2_ROOT_HELPER_FIELDED_CAP" = "$cap" ] && pass "the fielded cap is 8 MiB" \
    || fail "fielded cap" "K2_ROOT_HELPER_FIELDED_CAP=$K2_ROOT_HELPER_FIELDED_CAP"
[ "$K2_ROOT_HELPER_SIG_ROOM" = "$room" ] && pass "4 KiB is kept for the signature" \
    || fail "sig room" "K2_ROOT_HELPER_SIG_ROOM=$K2_ROOT_HELPER_SIG_ROOM"

mk() { head -c "$2" /dev/zero > "$tmp/$1"; }

# At the limit: passes, prints the size.
mk k2-root-helper-linux-x86_64 "$max"
rc=0; out="$(k2_root_helper_size_gate "$tmp/k2-root-helper-linux-x86_64" 2>&1)" || rc=$?
[ "$rc" = 0 ] && pass "a helper at the limit passes" || fail "at the limit" "rc=$rc: $out"
[[ "$out" == *"k2-root-helper-linux-x86_64"*"$max bytes"* ]] && pass "it prints the size" || fail "size line" "$out"
# Within 1 MiB of the cap: passes, warns.
[[ "$out" == *WARNING* ]] && pass "within 1 MiB of the cap it warns" || fail "near-cap warning" "$out"

# Comfortably under: passes with no warning.
mk k2-root-helper-linux-aarch64 $((3 * 1024 * 1024))
rc=0; out="$(k2_root_helper_size_gate "$tmp/k2-root-helper-linux-aarch64" 2>&1)" || rc=$?
[ "$rc" = 0 ] && [[ "$out" != *WARNING* ]] && pass "3 MiB passes quietly" || fail "3 MiB" "rc=$rc: $out"

# One byte over: fails, says why.
mkdir -p "$tmp/big"
mk big/k2-root-helper-linux-x86_64 $((max + 1))
rc=0; out="$(k2_root_helper_size_gate "$tmp/big/k2-root-helper-linux-x86_64" 2>&1)" || rc=$?
[ "$rc" != 0 ] && pass "one byte over fails" || fail "one byte over" "rc=0: $out"
[[ "$out" == *"$((max + 1))"* && "$out" == *"$max"* ]] && pass "the refusal names both numbers" || fail "numbers" "$out"
[[ "$out" == *"8 MiB"* && "$out" == *"self-update"* ]] && pass "the refusal says why (fielded helpers' 8 MiB self-update cap)" \
    || fail "why" "$out"

# 0.46.17's real size fails.
mk big/k2-root-helper-linux-x86_64 8836200
rc=0; k2_root_helper_size_gate "$tmp/big/k2-root-helper-linux-x86_64" >/dev/null 2>&1 || rc=$?
[ "$rc" != 0 ] && pass "0.46.17's x86_64 size (8,836,200) fails" || fail "0.46.17 size" "passed"

# Missing: fails.
rc=0; out="$(k2_root_helper_size_gate "$tmp/nope" 2>&1)" || rc=$?
[ "$rc" != 0 ] && [[ "$out" == *missing* ]] && pass "a missing helper fails" || fail "missing" "rc=$rc: $out"

# No files at all: fails (a gate that checked nothing never passes).
rc=0; out="$(k2_root_helper_size_gate 2>&1)" || rc=$?
[ "$rc" != 0 ] && pass "no files fails" || fail "no files" "rc=0: $out"

# Several: one bad one fails the lot; the good one still prints.
rc=0; out="$(k2_root_helper_size_gate "$tmp/k2-root-helper-linux-aarch64" "$tmp/big/k2-root-helper-linux-x86_64" 2>&1)" || rc=$?
[ "$rc" != 0 ] && [[ "$out" == *"k2-root-helper-linux-aarch64"* ]] && pass "one bad file fails the set" || fail "set" "rc=$rc: $out"

# Wiring (static): every place a helper leaves the build calls the gate.
grep -q 'k2_root_helper_size_gate "\$OUT/\$HELPER"' "$ROOT/scripts/ci/build-linux-bins.sh" \
    && pass "build-linux-bins.sh gates the helper it built" || fail "build wiring" "no gate call in build-linux-bins.sh"
awk '/^k2cr_stage\(\)/,/^}/' "$ROOT/scripts/lib/compute-release.sh" | grep -q 'k2_root_helper_size_gate' \
    && pass "k2cr_stage gates the helpers before release.sh publishes them" || fail "stage wiring" "no gate call in k2cr_stage"
grep -q 'k2_root_helper_size_gate "\$TMP/\$A"' "$ROOT/scripts/sign-root-helper.sh" \
    && pass "sign-root-helper.sh refuses to sign an oversize helper" || fail "sign wiring" "no gate call in sign-root-helper.sh"

echo
echo "root-helper-size: $pass passed, $fail failed"
[ "$fail" = 0 ]
