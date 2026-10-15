#!/usr/bin/env bash
# Hermetic test for scripts/sign-app-engine.sh (prd-k2-app-runtime-v1 S11).
# No real key, no keychain, no GitHub: a throwaway UNENCRYPTED minisign key
# in a temp dir, a fake keys.rs (K2_RELEASE_KEYS_RS) listing it, and PATH
# shims for gh (a local "release" dir) and security (prints a dummy
# password; minisign ignores it for an unencrypted key). Fake binaries carry
# real ELF headers and real build-marker strings. Needs the real `minisign`
# (fails loudly without it). Run with: bash tests/cli/sign_app_engine.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/sign-app-engine.sh"
[ -f "$SCRIPT" ] || { echo "FAIL: missing $SCRIPT" >&2; exit 1; }
command -v minisign >/dev/null 2>&1 || { echo "FAIL: minisign is required for this test (brew install minisign / apt install minisign)" >&2; exit 1; }
REAL_MINISIGN="$(command -v minisign)"

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d -t k2-sign-app-engine-test.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
REL="$WORK/release"
UP="$WORK/uploaded"
mkdir -p "$SHIMS" "$REL" "$UP"
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

marker() { printf 'K2_APP_ENGINE_BUILD v=%s proto=%s arch=%s;' "$1" "$2" "$3"; }

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

check_ok() { # <name> <file> <arch> <version>
	local out
	out="$(bash "$SCRIPT" --check "$2" "$3" "$4" 2>"$WORK/err")" || fail "$1: --check refused: $(cat "$WORK/err")"
	[ "$out" = 1 ] || fail "$1: expected protocol 1, got '$out'"
	ok "$1"
}
check_refused() { # <name> <needle> <file> <arch> <version>
	if bash "$SCRIPT" --check "$3" "$4" "$5" >/dev/null 2>"$WORK/err"; then
		fail "$1: --check accepted it"
	fi
	grep -q -- "$2" "$WORK/err" || fail "$1: expected '$2' in: $(cat "$WORK/err")"
	ok "$1"
}

V=0.46.1
echo "--check (the byte checks Step 10.5 runs)"
mk_elf x86_64 "$WORK/lx" "$(marker $V 1 x86_64)"
check_ok "x86_64 build passes" "$WORK/lx" x86_64 $V
mk_elf aarch64 "$WORK/la" "$(marker $V 1 aarch64)"
check_ok "aarch64 build passes (v-prefixed version)" "$WORK/la" aarch64 v$V
mk_elf x86_64 "$WORK/t" "$(marker $V 1 x86_64)" K2_APP_ENGINE_TEST_KEYS_ENABLED
check_refused "test-keys build refused" "TEST-KEYS build" "$WORK/t" x86_64 $V
mk_elf x86_64 "$WORK/t" "$(marker $V 1 x86_64)" "$(marker $V 1 x86_64)"
check_refused "two markers refused" "exactly one" "$WORK/t" x86_64 $V
mk_elf x86_64 "$WORK/t" "no marker here"
check_refused "missing marker refused" "exactly one" "$WORK/t" x86_64 $V
mk_elf x86_64 "$WORK/t" "K2_APP_ENGINE_BUILD v=$V proto=1 arch=x86_64"
check_refused "unterminated marker refused" "unreadable" "$WORK/t" x86_64 $V
check_refused "other release refused" "release is v0.46.2" "$WORK/lx" x86_64 0.46.2
mk_elf aarch64 "$WORK/t" "$(marker $V 1 x86_64)"
check_refused "marker arch differs from the asset" "built for arch x86_64" "$WORK/t" aarch64 $V
check_refused "x86_64 ELF as the aarch64 asset" "not an aarch64 ELF" "$WORK/lx" aarch64 $V
printf 'MZnotanelf' >"$WORK/pe"
check_refused "not an ELF refused" "not an ELF" "$WORK/pe" x86_64 $V
check_refused "unsupported arch refused" "unsupported" "$WORK/lx" riscv64 $V
# A k2-node or root-helper marker is not an engine marker.
mk_elf x86_64 "$WORK/t" "K2_NODE_BUILD v=$V proto=2 os=linux arch=x86_64;"
check_refused "a k2-node build refused" "exactly one" "$WORK/t" x86_64 $V
mk_elf x86_64 "$WORK/t" "K2_ROOT_HELPER_BUILD v=$V proto=6 arch=x86_64;"
check_refused "a root-helper build refused" "exactly one" "$WORK/t" x86_64 $V

echo "full run (shimmed gh/security, throwaway key)"
KEYDIR="$WORK/key"
mkdir -p "$KEYDIR"
"$REAL_MINISIGN" -G -W -p "$KEYDIR/k.pub" -s "$KEYDIR/k.key" >/dev/null
PUBLINE="$(sed -n 2p "$KEYDIR/k.pub")"
printf 'pub const RELEASE_KEYS: &[TrustedKey] = &[\n    TrustedKey { role: "primary", key_id: "X", public_key_b64: "%s" },\n];\n' "$PUBLINE" >"$WORK/keys.rs"
printf 'pub const RELEASE_KEYS: &[TrustedKey] = &[\n    TrustedKey { role: "primary", key_id: "X", public_key_b64: "RWQnotThisKeyAtAllxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" },\n];\n' >"$WORK/keys-other.rs"
# An unencrypted key never reads the password, so minisign may exit before
# this writes: ignore SIGPIPE (pipefail would otherwise fail the signing).
printf '#!/bin/sh\ntrap "" PIPE\necho "dummy-password" 2>/dev/null\nexit 0\n' >"$SHIMS/security"
{
	echo '#!/usr/bin/env bash'
	echo 'set -euo pipefail'
	echo '[ "$1" = release ] || exit 2'
	echo 'case "$2" in'
	echo "view) ls \"$REL\" ;;"
	echo 'download)'
	echo '	shift 3; dir=""; pats=()'
	echo '	while [ $# -gt 0 ]; do'
	echo '		case "$1" in -R) shift 2 ;; -p) pats+=("$2"); shift 2 ;; -D) dir="$2"; shift 2 ;; *) shift ;; esac'
	echo '	done'
	echo "	for p in \"\${pats[@]}\"; do cp \"$REL/\$p\" \"\$dir/\$p\"; done ;;"
	echo 'upload)'
	echo '	shift 3'
	echo '	while [ $# -gt 0 ]; do'
	echo "		case \"\$1\" in -R) shift 2 ;; --clobber) shift ;; *) cp \"\$1\" \"$UP/\"; shift ;; esac"
	echo '	done ;;'
	echo '*) exit 2 ;;'
	echo 'esac'
} >"$SHIMS/gh"
chmod +x "$SHIMS"/*

publish() { # <asset> <file>
	cp "$2" "$REL/$1"
	(cd "$REL" && sha "$1" >"$1.sha256")
}
publish k2-app-engine-linux-x86_64 "$WORK/lx"
publish k2-app-engine-linux-aarch64 "$WORK/la"

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
for A in k2-app-engine-linux-x86_64 k2-app-engine-linux-aarch64; do
	[ -f "$UP/$A.sig" ] || fail "$A.sig not uploaded"
	tc="$("$REAL_MINISIGN" -V -m "$REL/$A" -p "$KEYDIR/k.pub" -x "$UP/$A.sig" -Q)" || fail "$A.sig does not verify"
	# Exactly k2_root_proto::engine::trusted_comment(arch, version, protocol).
	[ "$(printf '%s\n' "$tc" | tail -n1)" = "file:$A version:$V protocol:1" ] || fail "$A trusted comment: $tc"
	ok "$A signed: file:$A version:$V protocol:1"
done
[ "$(ls "$UP" | wc -l | tr -d ' ')" = 2 ] || fail "uploaded more than the two .sig files: $(ls "$UP")"
ok "only the .sig files are uploaded"

rm -f "$UP"/*
printf 'x' >>"$REL/k2-app-engine-linux-x86_64"
run_sign $V >"$WORK/out" 2>&1 && fail "signed an asset whose sha256 differs"
grep -q "sha256 differs" "$WORK/out" || fail "sha refusal: $(cat "$WORK/out")"
[ -z "$(ls "$UP")" ] || fail "uploaded something after a sha mismatch: $(ls "$UP")"
ok "sha256 mismatch refused before signing"

# A test-keys build in the SECOND asset: nothing is signed, not even the first.
publish k2-app-engine-linux-x86_64 "$WORK/lx"
mk_elf aarch64 "$WORK/t" "$(marker $V 1 aarch64)" K2_APP_ENGINE_TEST_KEYS_ENABLED
publish k2-app-engine-linux-aarch64 "$WORK/t"
run_sign $V >"$WORK/out" 2>&1 && fail "signed a test-keys build"
grep -q "TEST-KEYS build" "$WORK/out" || fail "test-keys refusal: $(cat "$WORK/out")"
[ -z "$(ls "$UP")" ] || fail "uploaded something for a test-keys build: $(ls "$UP")"
ok "test-keys build refused; nothing signed"

publish k2-app-engine-linux-aarch64 "$WORK/la"
rm "$REL/k2-app-engine-linux-aarch64.sha256"
run_sign $V >"$WORK/out" 2>&1 && fail "signed with an asset missing"
grep -q "missing within 0 min: k2-app-engine-linux-aarch64.sha256" "$WORK/out" || fail "missing-asset refusal: $(cat "$WORK/out")"
ok "a missing asset stops the run"

echo "sign_app_engine: $pass passed"
