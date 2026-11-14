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
    "to_method": ["CC0-1.0"], "notify": ["CC0-1.0"], "tiny-keccak": ["CC0-1.0"],
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
want(s.get("allow-git") == ["https://github.com/Alakazam-211/sew-crates", "https://github.com/Alakazam-211/str0m"],
     f"sources.allow-git = sew-crates and str0m exactly, got {s.get('allow-git')}")
want(s.get("required-git-spec") == "rev", "sources.required-git-spec = rev")
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

echo ""
echo "cargo-deny-policy: $pass passed, $fail failed"
[ "$fail" = 0 ]
