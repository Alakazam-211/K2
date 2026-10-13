#!/usr/bin/env bash
# Hermetic test for scripts/sign-k2-node.sh (prd-k2-node-admin-v1 §8.1).
# No real key, no keychain, no GitHub, no codesign: a throwaway UNENCRYPTED
# minisign key in a temp dir, a fake keys.rs (K2_RELEASE_KEYS_RS) listing
# it, and PATH shims for gh (a local "release" dir), security (prints a
# dummy password; minisign ignores it for an unencrypted key) and codesign
# (passes unless SHIM_CODESIGN_FAIL=1). Fake binaries carry real headers
# and real build-marker strings. Needs the real `minisign` (fails loudly
# without it). Run with: bash tests/cli/sign_k2_node.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/sign-k2-node.sh"
[ -f "$SCRIPT" ] || { echo "FAIL: missing $SCRIPT" >&2; exit 1; }
command -v minisign >/dev/null 2>&1 || { echo "FAIL: minisign is required for this test (brew install minisign / apt install minisign)" >&2; exit 1; }
REAL_MINISIGN="$(command -v minisign)"

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d -t k2-sign-k2-node-test.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
REL="$WORK/release"
UP="$WORK/uploaded"
mkdir -p "$SHIMS" "$REL" "$UP"
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

marker() { printf 'K2_NODE_BUILD v=%s proto=%s os=%s arch=%s;' "$1" "$2" "$3" "$4"; }

# mk_elf <arch> <out> <payload…>: 64-bit LE ELF header with e_machine.
mk_elf() {
	local arch="$1" out="$2"
	shift 2
	printf '\x7fELF\x02\x01\x01\x00' >"$out"
	printf '\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00' >>"$out" # pad + e_type
	case "$arch" in
	x86_64) printf '\x3e\x00' >>"$out" ;;
	aarch64) printf '\xb7\x00' >>"$out" ;;
	esac
	printf '\x00pad\x00' >>"$out"
	for p in "$@"; do printf '%s\x00' "$p" >>"$out"; done
}
# mk_macho <arch> <out> <payload…>: thin 64-bit Mach-O executable header.
mk_macho() {
	local arch="$1" out="$2"
	shift 2
	printf '\xcf\xfa\xed\xfe' >"$out"
	case "$arch" in
	x86_64) printf '\x07\x00\x00\x01' >>"$out" ;;
	aarch64) printf '\x0c\x00\x00\x01' >>"$out" ;;
	esac
	printf '\x00\x00\x00\x00\x02\x00\x00\x00' >>"$out"
	printf '\x00pad\x00' >>"$out"
	for p in "$@"; do printf '%s\x00' "$p" >>"$out"; done
}

check_ok() { # <name> <file> <os> <arch> <version>
	local out
	out="$(bash "$SCRIPT" --check "$2" "$3" "$4" "$5" 2>"$WORK/err")" || fail "$1: --check refused: $(cat "$WORK/err")"
	[ "$out" = 2 ] || fail "$1: expected protocol 2, got '$out'"
	ok "$1"
}
check_refused() { # <name> <needle> <file> <os> <arch> <version>
	if bash "$SCRIPT" --check "$3" "$4" "$5" "$6" >/dev/null 2>"$WORK/err"; then
		fail "$1: --check accepted it"
	fi
	grep -q -- "$2" "$WORK/err" || fail "$1: expected '$2' in: $(cat "$WORK/err")"
	ok "$1"
}

V=0.45.5
echo "--check (the byte checks release.sh Step 8.55 and Step 10.5 run)"
mk_elf x86_64 "$WORK/lx" "$(marker $V 2 linux x86_64)"
check_ok "linux x86_64 build passes" "$WORK/lx" linux x86_64 $V
mk_elf aarch64 "$WORK/la" "$(marker $V 2 linux aarch64)"
check_ok "linux aarch64 build passes" "$WORK/la" linux aarch64 v$V
mk_macho aarch64 "$WORK/ma" "$(marker $V 2 macos aarch64)"
check_ok "macos aarch64 build passes" "$WORK/ma" macos aarch64 $V
mk_elf x86_64 "$WORK/t" "$(marker $V 2 linux x86_64)" K2_NODE_TEST_KEYS_ENABLED
check_refused "test-keys build refused" "TEST-KEYS build" "$WORK/t" linux x86_64 $V
mk_elf x86_64 "$WORK/t" "$(marker $V 2 linux x86_64)" "$(marker $V 2 linux x86_64)"
check_refused "two markers refused" "exactly one" "$WORK/t" linux x86_64 $V
mk_elf x86_64 "$WORK/t" "no marker here"
check_refused "missing marker refused" "exactly one" "$WORK/t" linux x86_64 $V
mk_elf x86_64 "$WORK/t" "K2_NODE_BUILD v=$V proto=2 os=linux arch=x86_64"
check_refused "unterminated marker refused" "unreadable" "$WORK/t" linux x86_64 $V
check_refused "other release refused" "release is v0.45.6" "$WORK/lx" linux x86_64 0.45.6
mk_elf aarch64 "$WORK/t" "$(marker $V 2 linux x86_64)"
check_refused "marker arch differs from the asset" "built for arch x86_64" "$WORK/t" linux aarch64 $V
check_refused "x86_64 ELF as the aarch64 asset" "not an aarch64 ELF" "$WORK/lx" linux aarch64 $V
mk_elf x86_64 "$WORK/t" "$(marker $V 2 macos x86_64)"
check_refused "macos marker in a linux asset" "built for os macos" "$WORK/t" linux x86_64 $V
check_refused "ELF as the macos asset" "not a thin 64-bit Mach-O" "$WORK/lx" macos aarch64 $V
check_refused "Mach-O as the linux asset" "not an ELF" "$WORK/ma" linux x86_64 $V
cp "$WORK/ma" "$WORK/fat"
printf '\xca\xfe\xba\xbe' | dd of="$WORK/fat" bs=1 count=4 conv=notrunc 2>/dev/null
check_refused "universal Mach-O refused" "universal" "$WORK/fat" macos aarch64 $V
check_refused "unsupported os refused" "unsupported" "$WORK/lx" windows x86_64 $V

echo "full run (shimmed gh/security/codesign, throwaway key)"
KEYDIR="$WORK/key"
mkdir -p "$KEYDIR"
"$REAL_MINISIGN" -G -W -p "$KEYDIR/k.pub" -s "$KEYDIR/k.key" >/dev/null
PUBLINE="$(sed -n 2p "$KEYDIR/k.pub")"
cat >"$WORK/keys.rs" <<RS
pub const RELEASE_KEYS: &[TrustedKey] = &[
    TrustedKey { role: "primary", key_id: "X", public_key_b64: "$PUBLINE" },
];
RS
cat >"$WORK/keys-other.rs" <<'RS'
pub const RELEASE_KEYS: &[TrustedKey] = &[
    TrustedKey { role: "primary", key_id: "X", public_key_b64: "RWQnotThisKeyAtAllxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" },
];
RS
# An unencrypted key never reads the password, so minisign may exit before
# this writes: ignore SIGPIPE (pipefail would otherwise fail the signing).
cat >"$SHIMS/security" <<'SH'
#!/bin/sh
trap '' PIPE
echo "dummy-password" 2>/dev/null
exit 0
SH
cat >"$SHIMS/codesign" <<'SH'
#!/bin/sh
[ "${SHIM_CODESIGN_FAIL:-0}" = 1 ] && exit 1
exit 0
SH
cat >"$SHIMS/gh" <<SH
#!/usr/bin/env bash
set -euo pipefail
[ "\$1" = release ] || exit 2
case "\$2" in
view) ls "$REL" ;;
download)
	shift 3; dir=""; pats=()
	while [ \$# -gt 0 ]; do
		case "\$1" in -R) shift 2 ;; -p) pats+=("\$2"); shift 2 ;; -D) dir="\$2"; shift 2 ;; *) shift ;; esac
	done
	for p in "\${pats[@]}"; do cp "$REL/\$p" "\$dir/\$p"; done ;;
upload)
	shift 3
	while [ \$# -gt 0 ]; do
		case "\$1" in -R) shift 2 ;; --clobber) shift ;; *) cp "\$1" "$UP/"; shift ;; esac
	done ;;
*) exit 2 ;;
esac
SH
chmod +x "$SHIMS"/*

publish() { # <asset> <file>
	cp "$2" "$REL/$1"
	(cd "$REL" && sha "$1" >"$1.sha256")
}
publish k2-node-linux-x86_64 "$WORK/lx"
publish k2-node-linux-aarch64 "$WORK/la"
publish k2-node-macos-aarch64 "$WORK/ma"

run_sign() {
	env PATH="$SHIMS:$PATH" K2_ROOT_HELPER_KEY="$KEYDIR/k.key" K2_ROOT_HELPER_PUB="$KEYDIR/k.pub" \
		K2_RELEASE_KEYS_RS="${KEYS_RS:-$WORK/keys.rs}" K2_SIGN_WAIT_MIN=0 \
		bash "$SCRIPT" "$@"
}

KEYS_RS="$WORK/keys-other.rs" run_sign --preflight >/dev/null 2>"$WORK/err" && fail "preflight accepted a key that is not in RELEASE_KEYS"
grep -q "is not one of RELEASE_KEYS" "$WORK/err" || fail "preflight: $(cat "$WORK/err")"
ok "preflight refuses a key that is not in RELEASE_KEYS"
run_sign --preflight >/dev/null 2>"$WORK/err" || fail "preflight: $(cat "$WORK/err")"
ok "preflight passes with the listed key"

run_sign $V >"$WORK/out" 2>&1 || fail "sign run failed: $(cat "$WORK/out")"
for A in k2-node-linux-x86_64 k2-node-linux-aarch64 k2-node-macos-aarch64; do
	[ -f "$UP/$A.sig" ] || fail "$A.sig not uploaded"
	tc="$("$REAL_MINISIGN" -V -m "$REL/$A" -p "$KEYDIR/k.pub" -x "$UP/$A.sig" -Q)" || fail "$A.sig does not verify"
	# Exactly k2_root_proto::node_build::trusted_comment(os, arch, version, protocol).
	[ "$(printf '%s\n' "$tc" | tail -n1)" = "file:$A version:$V protocol:2" ] || fail "$A trusted comment: $tc"
	ok "$A signed: file:$A version:$V protocol:2"
done
[ "$(ls "$UP" | wc -l | tr -d ' ')" = 3 ] || fail "uploaded more than the three .sig files: $(ls "$UP")"
ok "only the .sig files are uploaded"

rm -f "$UP"/*
SHIM_CODESIGN_FAIL=1 run_sign $V >"$WORK/out" 2>&1 && fail "signed a macOS build without K2's Developer ID"
grep -q "not signed by K2's Developer ID" "$WORK/out" || fail "codesign refusal: $(cat "$WORK/out")"
[ ! -f "$UP/k2-node-macos-aarch64.sig" ] || fail "macOS .sig uploaded despite the codesign refusal"
ok "macOS asset without the Developer ID is refused"

rm -f "$UP"/*
printf 'x' >>"$REL/k2-node-linux-x86_64"
run_sign $V >"$WORK/out" 2>&1 && fail "signed an asset whose sha256 differs"
grep -q "sha256 differs" "$WORK/out" || fail "sha refusal: $(cat "$WORK/out")"
[ -z "$(ls "$UP")" ] || fail "uploaded something after a sha mismatch: $(ls "$UP")"
ok "sha256 mismatch refused before signing"

mk_elf x86_64 "$WORK/t" "$(marker $V 2 linux x86_64)" K2_NODE_TEST_KEYS_ENABLED
publish k2-node-linux-x86_64 "$WORK/t"
run_sign $V >"$WORK/out" 2>&1 && fail "signed a test-keys build"
grep -q "TEST-KEYS build" "$WORK/out" || fail "test-keys refusal: $(cat "$WORK/out")"
[ -z "$(ls "$UP")" ] || fail "uploaded something for a test-keys build: $(ls "$UP")"
ok "test-keys build refused in the full run"

rm "$REL/k2-node-macos-aarch64.sha256"
publish k2-node-linux-x86_64 "$WORK/lx"
run_sign $V >"$WORK/out" 2>&1 && fail "signed with an asset missing"
grep -q "missing within 0 min: k2-node-macos-aarch64.sha256" "$WORK/out" || fail "missing-asset refusal: $(cat "$WORK/out")"
ok "a missing asset stops the run"

echo "sign_k2_node: $pass passed"
