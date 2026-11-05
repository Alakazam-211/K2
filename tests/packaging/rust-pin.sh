#!/usr/bin/env bash
# Tests for the Rust pin (rust-toolchain.toml, scripts/lib/rust-pin.sh):
# one version in the file, the GitHub workflows and every K2 Compute
# submitter; the reader at a commit; release.sh's Step 0.4 check with stub
# rustup/rustc. Nothing here runs cargo, rustup or a daemon. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
unset K2_SKIP_RUST_PIN_CHECK RUSTUP_TOOLCHAIN K2_RUST_PIN_REPO || true

echo "rust-toolchain.toml:"
PIN="$(python3 - "$ROOT/rust-toolchain.toml" <<'PY'
import sys, tomllib
t = tomllib.load(open(sys.argv[1], "rb"))["toolchain"]
assert set(t) <= {"channel", "components", "profile", "targets"}, t
assert {"rustfmt", "clippy"} <= set(t.get("components", [])), t
assert t.get("profile") == "minimal", t
ch = t["channel"]
parts = ch.split(".")
assert len(parts) == 3 and all(p.isdigit() for p in parts), "channel must be an exact x.y.z, not %r" % ch
print(ch)
PY
)" && pass "an exact version ($PIN), rustfmt + clippy, minimal profile" || fail "toolchain file" "$PIN"

echo "GitHub workflows name the same version:"
python3 - "$ROOT/.github/workflows" "$PIN" > "$tmp/wf" <<'PY' && pass "every dtolnay/rust-toolchain step has toolchain: $PIN" || fail "workflow drift" "$(cat "$tmp/wf")"
import os, re, sys
d, pin = sys.argv[1], sys.argv[2]
bad, n = [], 0
for f in sorted(os.listdir(d)):
    lines = open(os.path.join(d, f)).read().splitlines()
    for i, l in enumerate(lines):
        if "uses: dtolnay/rust-toolchain@" in l:
            n += 1
            step = []
            for m in lines[i + 1:]:
                if re.match(r"^\s*- ", m):
                    break
                step.append(m.strip())
            tc = [x for x in step if x.startswith("toolchain:")]
            if tc != ["toolchain: " + pin]:
                bad.append("%s:%d: %s" % (f, i + 1, tc or "no toolchain:"))
print("\n".join(bad))
sys.exit(1 if bad or n == 0 else 0)
PY

echo "K2 Compute submitters send the pin, never rust@stable:"
for f in scripts/ci/run-checks.sh scripts/lib/compute-release.sh scripts/test-on-nodes.sh scripts/ci/build-linux-app.sh \
        scripts/measure-clean-build.sh scripts/test/compute-tiers-gates.sh docs/ci-on-k2-compute.md; do
    if grep -n "rust@stable" "$ROOT/$f" | tr -d '`' | grep -v -i "before the pin\|would override\|rust@stable would\|never rust@stable\|not rust@stable\|overrides the pin" > "$tmp/stable"; then
        fail "$f still sends rust@stable" "$(cat "$tmp/stable")"
    else
        pass "$f: no rust@stable"
    fi
done
for f in scripts/ci/run-checks.sh scripts/lib/compute-release.sh scripts/test-on-nodes.sh; do
    grep -q "k2_rust_need" "$ROOT/$f" && pass "$f reads the pin (k2_rust_need)" || fail "$f" "doesn't call k2_rust_need"
done

echo "the reader (scripts/lib/rust-pin.sh):"
repo="$tmp/repo"
mkdir -p "$repo/scripts/lib"
cp "$ROOT/scripts/lib/rust-pin.sh" "$repo/scripts/lib/"
git -C "$repo" init -q
git -C "$repo" add scripts
git -C "$repo" -c user.email=t@t -c user.name=t commit -q -m old
OLD="$(git -C "$repo" rev-parse HEAD)"
printf '# a comment\n[toolchain]\nprofile = "minimal"\n  channel   =  "1.98.7"  # trailing\n' > "$repo/rust-toolchain.toml"
git -C "$repo" add rust-toolchain.toml
git -C "$repo" -c user.email=t@t -c user.name=t commit -q -m pin
NEW="$(git -C "$repo" rev-parse HEAD)"
printf '[toolchain]\nchannel = "9.9.9"\n' > "$repo/rust-toolchain.toml"
r() { (cd /; bash -c 'source "$1"; shift; "$@"' _ "$repo/scripts/lib/rust-pin.sh" "$@"); }
[ "$(r k2_rust_pin "$NEW")" = 1.98.7 ] && pass "k2_rust_pin REV reads the file at that commit" || fail "pin at rev" "$(r k2_rust_pin "$NEW" 2>&1)"
[ "$(r k2_rust_pin)" = 9.9.9 ] && pass "k2_rust_pin (no REV) reads the working tree" || fail "pin tree" "$(r k2_rust_pin 2>&1)"
[ "$(r k2_rust_need "$NEW")" = rust@1.98.7 ] && pass "k2_rust_need REV → rust@<pin>" || fail "need" "$(r k2_rust_need "$NEW" 2>&1)"
out="$(r k2_rust_need "$OLD" 2>"$tmp/note")"
[ "$out" = rust@stable ] && grep -q "before the pin" "$tmp/note" && pass "a commit before the pin → rust@stable with a note on stderr" || fail "old commit" "$out $(cat "$tmp/note")"
rc=0; r k2_rust_pin "$OLD" >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] && pass "k2_rust_pin fails (exit 1) when the commit has no pin" || fail "old pin rc" "rc=$rc"
rc=0; out="$(r k2_rust_need 0123456789abcdef0123456789abcdef01234567 2>"$tmp/miss")" || rc=$?
[ "$rc" = 1 ] && [ -z "$out" ] && grep -q "is not in" "$tmp/miss" && ! grep -q "before the pin" "$tmp/miss" \
    && pass "a commit that isn't here fails loudly (no rust@stable guess)" || fail "missing commit" "rc=$rc out=$out $(cat "$tmp/miss")"
other="$tmp/other"; git init -q "$other"
printf '[toolchain]\nchannel = "1.11.1"\n' > "$other/rust-toolchain.toml"
git -C "$other" add rust-toolchain.toml; git -C "$other" -c user.email=t@t -c user.name=t commit -q -m o
OSHA="$(git -C "$other" rev-parse HEAD)"
[ "$(K2_RUST_PIN_REPO="$other" r k2_rust_pin "$OSHA")" = 1.11.1 ] && pass "K2_RUST_PIN_REPO: the commit is read in the repo it lives in" || fail "pin repo" "?"

echo "the release check (k2_rust_pin_check, release.sh Step 0.4):"
printf '[toolchain]\nchannel = "1.98.7"\n' > "$repo/rust-toolchain.toml"
mkdir -p "$tmp/ok" "$tmp/wrong" "$tmp/noinst" "$tmp/norustup"
for d in ok wrong noinst; do
    cat > "$tmp/$d/rustup" <<STUB
#!/bin/sh
echo "rustup \$*" >> "$tmp/$d.calls"
case "\$1" in
  toolchain) case "\$2" in list) [ "$d" = noinst ] && echo "stable-x86_64-unknown-linux-gnu (default)" || echo "1.98.7-aarch64-apple-darwin"; exit 0 ;;
                           install) exit 0 ;; esac ;;
esac
exit 0
STUB
    chmod +x "$tmp/$d/rustup"
done
printf '#!/bin/sh\necho "rustc 1.98.7 (abc 2026-01-01)"\n' > "$tmp/ok/rustc"
printf '#!/bin/sh\necho "rustc 1.94.0 (def 2025-12-01)"\n' > "$tmp/wrong/rustc"
printf '#!/bin/sh\necho "rustc 1.98.7 (abc 2026-01-01)"\n' > "$tmp/noinst/rustc"
printf '#!/bin/sh\necho "rustc 1.95.0 (brew)"\n' > "$tmp/norustup/rustc"
chmod +x "$tmp"/*/rustc
SYS="/usr/bin:/bin"
rc=0; out="$(PATH="$tmp/ok:$SYS" r k2_rust_pin_check 2>&1)" || rc=$?
[ "$rc" = 0 ] && [[ "$out" == *"rustc 1.98.7"* ]] && ! grep -q "toolchain install" "$tmp/ok.calls" \
    && pass "the pin installed and active: passes, no install" || fail "ok" "rc=$rc $out"
rc=0; out="$(PATH="$tmp/noinst:$SYS" r k2_rust_pin_check 2>&1)" || rc=$?
[ "$rc" = 0 ] && grep -q "toolchain install 1.98.7 --profile minimal -c rustfmt,clippy" "$tmp/noinst.calls" \
    && pass "the pin missing: rustup installs it, then passes" || fail "install" "rc=$rc $out $(cat "$tmp/noinst.calls")"
rc=0; out="$(PATH="$tmp/wrong:$SYS" r k2_rust_pin_check 2>&1)" || rc=$?
[ "$rc" = 1 ] && [[ "$out" == *"rustc here is 'rustc 1.94.0"* && "$out" == *"pins 1.98.7"* ]] \
    && pass "another rustc active: refused, says which" || fail "wrong rustc" "rc=$rc $out"
rc=0; out="$(PATH="$tmp/norustup:$SYS" r k2_rust_pin_check 2>&1)" || rc=$?
[ "$rc" = 1 ] && [[ "$out" == *"rustup is not installed"* && "$out" == *"rustup toolchain install 1.98.7"* ]] \
    && pass "no rustup (Homebrew cargo ignores the file): refused with the fix" || fail "no rustup" "rc=$rc $out"
rc=0; out="$(K2_SKIP_RUST_PIN_CHECK=1 PATH="$tmp/norustup:$SYS" r k2_rust_pin_check 2>&1)" || rc=$?
[ "$rc" = 0 ] && [[ "$out" == *"SKIPPED"* ]] && pass "K2_SKIP_RUST_PIN_CHECK=1: loud skip" || fail "skip" "rc=$rc $out"
python3 - "$ROOT/scripts/release.sh" <<'PY' && pass "release.sh runs the check (Step 0.4) before Step 0.5's cargo" || fail "release.sh" "k2_rust_pin_check missing or after Step 0.5"
import sys
s = open(sys.argv[1]).read()
i = s.index("\nk2_rust_pin_check\n")
assert i < s.index('RUSTFLAGS="-D warnings" cargo check --workspace'), "order"
assert s.index('source "$PROJECT_DIR/scripts/lib/rust-pin.sh"') < i
PY

echo "syntax:"
bash -n "$ROOT/scripts/lib/rust-pin.sh" && pass "bash -n rust-pin.sh" || fail "bash -n" "rust-pin.sh"

echo ""
echo "rust-pin: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
