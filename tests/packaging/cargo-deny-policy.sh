#!/usr/bin/env bash
# cargo-deny-policy.sh — the app-wide cargo-deny gate without cargo: the
# root deny.toml holds the Kessel Steward's policy, and
# `scripts/ci/cargo-deny.sh offline` refuses an unpinned git dependency and
# a shipped crate missing from its list. The real cargo-deny run is
# `scripts/ci/checks.sh audit` (it needs the RustSec database). No skips.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/ci/cargo-deny.sh"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "deny.toml holds the policy:"
rc=0; out="$(python3 - "$ROOT/deny.toml" <<'PY' 2>&1
import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))
errs = []
def want(cond, what):
    if not cond:
        errs.append(what)
g = d.get("graph", {})
want(g.get("exclude-dev") is True, "graph.exclude-dev = true")
want("all-features" not in g, "no graph.all-features")
want(set(g.get("targets", [])) == {"x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"},
     f"graph.targets = the shipped targets, got {g.get('targets')}")
l = d.get("licenses", {})
want(sorted(l.get("allow", [])) == sorted([
    "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause",
    "ISC", "Zlib", "Unicode-3.0", "BSL-1.0", "0BSD", "CDLA-Permissive-2.0"]),
     f"licenses.allow = the Kessel allowlist, got {l.get('allow')}")
want(l.get("confidence-threshold") == 0.93, f"licenses.confidence-threshold = 0.93, got {l.get('confidence-threshold')}")
# The crate-scoped exceptions the Steward ruled, exactly: no version pins, no others.
ruled = {
    "notify": ["CC0-1.0"], "tiny-keccak": ["CC0-1.0"],
    "cssparser": ["MPL-2.0"], "cssparser-macros": ["MPL-2.0"], "selectors": ["MPL-2.0"],
    "dtoa-short": ["MPL-2.0"], "option-ext": ["MPL-2.0"],
}
ex = l.get("exceptions", [])
got = {e.get("crate"): e.get("allow") for e in ex}
want(len(ex) == len(ruled) and got == ruled and all(set(e) == {"crate", "allow"} for e in ex),
     f"licenses.exceptions = the ruled crate-scoped list, got {ex}")
want(not {"MPL-2.0", "CC0-1.0"} & set(l.get("allow", [])), "MPL-2.0 and CC0-1.0 never in licenses.allow")
want("private" not in l, "no licenses.private (our own crates are checked too)")
b = d.get("bans", {})
want({"crate": "epaint_default_fonts"} in b.get("deny", []), "bans.deny has epaint_default_fonts")
s = d.get("sources", {})
want(s.get("unknown-registry") == "deny", "sources.unknown-registry = deny")
want(s.get("unknown-git") == "deny", "sources.unknown-git = deny")
want(s.get("allow-registry") == ["https://github.com/rust-lang/crates.io-index"], "sources.allow-registry = crates.io only")
want(s.get("allow-git") == [],
     f"sources.allow-git = none (every Sew crate is on crates.io since Sew 0.10.4; the sew-crates mirror retired), got {s.get('allow-git')}")
want(s.get("required-git-spec") == "rev", "sources.required-git-spec = rev")
want(d.get("advisories", {}).get("unused-ignored-advisory") == "warn", "advisories.unused-ignored-advisory = warn (the per-tree note stays on)")
want(l.get("unused-license-exception", "warn") == "warn", "the per-tree unused-exception note stays on")
for e in d.get("advisories", {}).get("ignore", []):
    want(isinstance(e, dict) and len(str(e.get("reason", "")).strip()) >= 20, f"advisory ignore {e!r} has a written reason")
for e in errs:
    print(e)
sys.exit(1 if errs else 0)
PY
)" || rc=$?
[ "$rc" = 0 ] && pass "licences, sources, graph and bans as ruled" || fail "deny.toml policy" "$out"

text="$(grep -v '^ *#' "$SCRIPT")"
[[ "$text" != *"--all-features"* ]] && pass "the gate never passes --all-features" || fail "--all-features" "scripts/ci/cargo-deny.sh passes it"
[[ "$text" == *'--config "$ROOT/deny.toml"'* ]] && pass "every run reads the root deny.toml" || fail "--config" "scripts/ci/cargo-deny.sh doesn't pass --config \$ROOT/deny.toml"

echo "offline checks:"
rc=0; out="$(bash "$SCRIPT" offline 2>&1)" || rc=$?
[ "$rc" = 0 ] && pass "the real tree passes" || fail "real tree" "rc=$rc $out"

# A fixture tree: a manifest, a lock and the four release build scripts.
SHA=85f5c87f93b301823629d920da5bc31015bd64b0
fixture() {
    local d="$tmp/$1"
    mkdir -p "$d/scripts/ci" "$d/scripts/windows" "$d/crates/a"
    printf '[package]\nname = "a"\nversion = "0.1.0"\n\n[dependencies]\nfoo = { git = "https://example.test/foo", rev = "%s" }\n' "$SHA" > "$d/crates/a/Cargo.toml"
    printf '[[package]]\nname = "foo"\nversion = "0.1.0"\nsource = "git+https://example.test/foo?rev=%s#%s"\n' "$SHA" "$SHA" > "$d/Cargo.lock"
    printf 'ARGS=(\n    -p k2-daemon --bin k2-daemon\n    -p k2-ptyd --bin k2-ptyd\n)\n' > "$d/scripts/macos-daemon-helper-app.sh"
    printf 'cargo build --release --target "$T" -p k2-root-helper\n' > "$d/scripts/ci/build-linux-bins.sh"
    printf 'cargo build --release -p k2-export --bin k2-export\n' > "$d/scripts/linux-ptyd-embed.sh"
    printf "Run 'cargo build k2-daemon' { cargo build --release -p k2-daemon }\n" > "$d/scripts/windows/build-nsis-release.ps1"
    echo "$d"
}
refused() {
    local name="$1" want="$2" d="$3" rc=0 out
    out="$(bash "$SCRIPT" offline "$d" 2>&1)" || rc=$?
    [ "$rc" = 1 ] && [[ "$out" == *"$want"* ]] && pass "$name refused" || fail "$name not refused" "rc=$rc $out"
}

d="$(fixture good)"
rc=0; out="$(bash "$SCRIPT" offline "$d" 2>&1)" || rc=$?
[ "$rc" = 0 ] && pass "a pinned fixture passes" || fail "pinned fixture" "rc=$rc $out"

d="$(fixture branch)"
printf '[package]\nname = "a"\nversion = "0.1.0"\n\n[dependencies]\nfoo = { git = "https://example.test/foo", branch = "main" }\n' > "$d/crates/a/Cargo.toml"
refused "a branch dependency" "foo: git dependency not pinned by rev" "$d"

d="$(fixture shortrev)"
printf '[package]\nname = "a"\nversion = "0.1.0"\n\n[target.'"'"'cfg(unix)'"'"'.dependencies]\nfoo = { git = "https://example.test/foo", rev = "85f5c87" }\n' > "$d/crates/a/Cargo.toml"
refused "a short rev (target table)" "foo: git dependency not pinned by rev" "$d"

d="$(fixture tag)"
printf '[package]\nname = "a"\nversion = "0.1.0"\n\n[patch.crates-io]\nfoo = { git = "https://example.test/foo", rev = "%s", tag = "v1" }\n' "$SHA" > "$d/crates/a/Cargo.toml"
refused "a rev with a tag (patch table)" "foo: git dependency names a tag" "$d"

d="$(fixture lock)"
printf '[[package]]\nname = "foo"\nversion = "0.1.0"\nsource = "git+https://example.test/foo?branch=main#%s"\n' "$SHA" > "$d/Cargo.lock"
refused "a lock entry without its rev" "Cargo.lock foo 0.1.0: git source not locked" "$d"

d="$(fixture newcrate)"
printf 'cargo build --release --target "$T" -p k2-root-helper\ncargo build --release --target "$T" -p k2-new-worker\n' > "$d/scripts/ci/build-linux-bins.sh"
refused "a release crate missing from SHIPPED" "builds -p k2-new-worker for release, but it is not in SHIPPED" "$d"

echo "stale exceptions and ignores (cargo-deny output per tree, stubbed):"
# The shipped package names, from the script's SHIPPED list.
pkgs=()
while IFS= read -r p; do pkgs+=("$p"); done < <(sed -n '/^SHIPPED=(/,/^)/s/^ *"[^:]*:\([^"]*\)"$/\1/p' "$SCRIPT")
[ "${#pkgs[@]}" = 12 ] && pass "12 shipped trees listed" || fail "shipped list" "got ${#pkgs[@]}: ${pkgs[*]}"
A1=RUSTSEC-2099-0001
A2=RUSTSEC-2099-0002
printf '[advisories]\nignore = [\n    { id = "%s", reason = "a test ignore with a long enough reason" },\n    { id = "%s", reason = "a test ignore with a long enough reason" },\n]\n\n[licenses]\nallow = ["MIT"]\n\n[[licenses.exceptions]]\ncrate = "alpha"\nallow = ["CC0-1.0"]\n\n[[licenses.exceptions]]\ncrate = "beta"\nallow = ["MPL-2.0"]\n' "$A1" "$A2" > "$tmp/deny-ex.toml"
# One tree output: the warning cargo-deny prints for each unused entry named
# (a crate for an exception, a RUSTSEC id for an ignore).
unused_log() {
    local c
    for c in "$@"; do
        case "$c" in
            RUSTSEC-*) printf 'warning[advisory-not-detected]: advisory was not encountered\n   ┌─ /job/src/deny.toml:3:13\n   │\n 3 │     { id = "%s", reason = "a test ignore with a long enough reason" },\n   │             ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria\n\n' "$c" ;;
            *) printf 'warning[license-exception-not-encountered]: license exception was not encountered\n    ┌─ /job/src/deny.toml:5:10\n    │\n  5 │ crate = "%s"\n    │          ━━━━━ unmatched license exception\n\n' "$c" ;;
        esac
    done
    echo "advisories ok, bans ok, licenses ok, sources ok"
}
run_stale() { bash "$SCRIPT" stale "$tmp/$1" "$tmp/deny-ex.toml" 2>&1; }
stale_case() {
    local name="$1" want="$2" rc=0 out
    out="$(run_stale "$name")" || rc=$?
    if [ "$want" = ok ]; then
        [ "$rc" = 0 ] && pass "$name passes" || fail "$name" "rc=$rc $out"
    else
        [ "$rc" = 1 ] && [[ "$out" == *"$want"* ]] && pass "$name refused" || fail "$name not refused" "rc=$rc $out"
    fi
}
# Every entry used somewhere: alpha and A1 by the first tree only, beta and A2 by the last only.
mkdir -p "$tmp/used"
for i in "${!pkgs[@]}"; do
    if [ "$i" = 0 ]; then unused_log beta "$A2"; elif [ "$i" = 11 ]; then unused_log alpha "$A1"; else unused_log alpha beta "$A1" "$A2"; fi > "$tmp/used/${pkgs[$i]}.log"
done
stale_case used ok
# alpha unused in every tree (beta still used once): alpha is stale, beta is not.
mkdir -p "$tmp/stale"
for i in "${!pkgs[@]}"; do
    if [ "$i" = 3 ]; then unused_log alpha; else unused_log alpha beta; fi > "$tmp/stale/${pkgs[$i]}.log"
done
stale_case stale 'exception for "alpha" is unused in all 12 shipped trees'
out="$(run_stale stale)" || true
[[ "$out" != *'"beta"'* ]] && pass "an exception one tree uses is not called stale" || fail "beta called stale" "$out"
# A1 matches no tree (A2 matches one): A1 is stale, A2 is not.
mkdir -p "$tmp/staleignore"
for i in "${!pkgs[@]}"; do
    if [ "$i" = 7 ]; then unused_log "$A1"; else unused_log "$A1" "$A2"; fi > "$tmp/staleignore/${pkgs[$i]}.log"
done
stale_case staleignore "the advisory ignore $A1 matches none of the 12 shipped trees"
out="$(run_stale staleignore)" || true
[[ "$out" != *"$A2"* ]] && pass "an ignore one tree matches is not called stale" || fail "$A2 called stale" "$out"
# A tree with no output at all can't count as using anything.
mkdir -p "$tmp/missing"
cp "$tmp/used/"*.log "$tmp/missing/"
rm "$tmp/missing/${pkgs[5]}.log"
stale_case missing "no cargo-deny output for ${pkgs[5]}"
# The warnings in colour are still read.
mkdir -p "$tmp/colour"
for p in "${pkgs[@]}"; do unused_log alpha "$A2" | sed $'s/crate = /\e[1mcrate\e[0m = /; s/{ id = /{ \e[1mid\e[0m = /' > "$tmp/colour/$p.log"; done
stale_case colour 'exception for "alpha" is unused in all 12 shipped trees'
out="$(run_stale colour)" || true
[[ "$out" == *"advisory ignore $A2 matches none"* ]] && pass "a coloured ignore warning is read" || fail "coloured ignore" "$out"

echo ""
echo "cargo-deny-policy: $pass passed, $fail failed"
[ "$fail" = 0 ]
