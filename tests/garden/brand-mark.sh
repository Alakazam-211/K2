#!/usr/bin/env bash
# Tests for scripts/brand/check-brand-mark.py on made-up binaries: one
# brand passes; a file of another brand, a file with no mark, a file with
# two brands and an unreadable file are refused. Bytes only: no cargo.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
C="$HERE/scripts/brand/check-brand-mark.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $*" >&2; exit 1; }

# bin <file> <mark ids...>: a file of junk bytes with each mark in it, as
# the binary holds it (the id, then NULs to 32 bytes).
bin() {
    local f="$1"; shift
    python3 - "$T/$f" "$@" <<'PY'
import os, sys
out = bytearray(os.urandom(64).replace(b"K", b"x"))
for i in sys.argv[2:]:
    m = ("K5_BRAND_MARK=" + i).encode()
    out += m + b"\0" * (32 - len(m)) + b"\x7fELF junk K5_CLI_VERSION=0.46.17\0"
with open(sys.argv[1], "wb") as f:
    f.write(bytes(out))
PY
}
check() { rc=0; out="$(python3 "$C" "$@" 2>&1)" || rc=$?; }

bin daemon xy; bin k5-cli xy; bin k2-daemon k2; bin plain; bin two xy k2; bin twice xy xy; bin longer xyz
check --brand xy "$T/daemon" "$T/k5-cli"
[ "$rc" = 0 ] && [ "$out" = "check-brand-mark: 2 binaries, all xy" ] || die "one brand: $rc $out"; ok
check --brand xy "$T/twice"
[ "$rc" = 0 ] || die "one brand marked twice: $out"; ok
check --brand xy "$T/daemon" "$T/k2-daemon"
[ "$rc" = 1 ] && [[ "$out" == *"k2-daemon: found K5_BRAND_MARK=k2, a k2 build (want K5_BRAND_MARK=xy)"* ]] && [[ "$out" == *"REFUSED: the bundle is not all xy"* ]] || die "a default-brand daemon in an xy bundle: $out"; ok
check --brand xy "$T/plain"
[ "$rc" = 1 ] && [[ "$out" == *"plain: found no brand mark (want K5_BRAND_MARK=xy)"* ]] || die "no mark: $out"; ok
check --brand xy "$T/two"
[ "$rc" = 1 ] && [[ "$out" == *"two: found 2 brand marks, K5_BRAND_MARK=k2, K5_BRAND_MARK=xy (want K5_BRAND_MARK=xy alone)"* ]] || die "two brands: $out"; ok
check --brand xy "$T/longer"
[ "$rc" = 1 ] && [[ "$out" == *"longer: found K5_BRAND_MARK=xyz, a xyz build (want K5_BRAND_MARK=xy)"* ]] || die "xyz is not xy: $out"; ok
check --brand xy "$T/nope"
[ "$rc" = 1 ] && [[ "$out" == *"nope: can't read it"* ]] || die "a missing file: $out"; ok
for args in "" "--brand xy" "--brand X-Y $T/daemon" "--nope xy $T/daemon"; do
    # shellcheck disable=SC2086
    check $args
    [ "$rc" = 2 ] && [[ "$out" == usage:* ]] || die "args '$args': $rc $out"; ok
done
# The mark's spelling here is the one the Rust writes.
grep -q 'pub const BRAND_MARK_PREFIX: &str = "K5_BRAND_MARK=";' "$HERE/crates/k2-brand/src/mark.rs" || die "the Rust mark's prefix moved"; ok

# ── the static-data gate (scripts/brand/mark-static-gate.sh) ──────────
G="$HERE/scripts/brand/mark-static-gate.sh"
gate() { rc=0; out="$(bash "$G" "$R" 2>&1)" || rc=$?; }
R="$T/tree"
mkdir -p "$R/crates/k2-brand/src" "$R/crates/k2-client/src/brand" "$R/crates/k2-ptyd/src" "$R/crates/k2-daemon/src"
cp "$HERE/crates/k2-brand/src/mark.rs" "$R/crates/k2-brand/src/"
cp "$HERE/crates/k2-client/src/brand/session.rs" "$R/crates/k2-client/src/brand/"
printf 'fn main() {\n    // The brand mark.\n    k2_brand::mark::keep();\n}\n' > "$R/crates/k2-ptyd/src/main.rs"
printf 'pub fn f() {}\n' > "$R/crates/k2-ptyd/src/lib.rs"
gate; [ "$rc" = 0 ] || die "a clean tree: $out"; ok
printf 'pub fn f() -> bool { k2_brand::mark::marked_brand_id() == "xy" }\n' > "$R/crates/k2-ptyd/src/lib.rs"
gate; [ "$rc" = 1 ] && [[ "$out" == *"k2-ptyd/src/lib.rs:1:"*"names the mark's bytes or its reader"* ]] && [[ "$out" == *"a helper names the mark other than"* ]] || die "a helper that reads the mark: $out"; ok
printf 'pub fn f() { k2_brand::mark::keep(); }\n' > "$R/crates/k2-ptyd/src/lib.rs"
gate; [ "$rc" = 1 ] && [[ "$out" == *"keeps the mark outside a program's main.rs"* ]] || die "keep() outside main: $out"; ok
printf 'pub fn f() {}\n' > "$R/crates/k2-ptyd/src/lib.rs"
printf 'fn g() -> usize { k2_client::brand::session::BRAND_MARK.len() }\n' > "$R/crates/k2-daemon/src/x.rs"
gate; [ "$rc" = 1 ] && [[ "$out" == *"k2-daemon/src/x.rs:1:"*"names the mark's bytes"* ]] || die "the daemon naming the bytes: $out"; ok
rm "$R/crates/k2-daemon/src/x.rs"
sed -i.bak 's/^pub fn keep() {$/pub fn keep() -> bool {/' "$R/crates/k2-brand/src/mark.rs" && rm -f "$R/crates/k2-brand/src/mark.rs.bak"
gate; [ "$rc" = 1 ] && [[ "$out" == *"no pub fn keep() returning nothing"* ]] || die "a keep() that returns a value: $out"; ok
rc=0; out="$(bash "$G" 2>&1)" || rc=$?
[ "$rc" = 0 ] || die "the real tree: $out"; ok
echo "brand mark check tests: $pass passed"
