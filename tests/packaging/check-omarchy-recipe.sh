#!/usr/bin/env bash
# scripts/check-omarchy-recipe.sh passes on this tree and fails on each kind of
# broken recipe, with the reason. No Arch, no network.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CHECK="$ROOT/scripts/check-omarchy-recipe.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || echo "        $2" >&2; nfail=$((nfail + 1)); }

fresh() {
    rm -rf "$tmp/t"
    mkdir -p "$tmp/t/packaging"
    cp -R "$ROOT/packaging/arch" "$ROOT/packaging/omarchy" "$tmp/t/packaging/"
    cp "$ROOT/LICENSE.md" "$tmp/t/"
}
R="packaging/omarchy/k2-bin"

echo "== this tree =="
if out="$(bash "$CHECK" 2>&1)"; then pass "the real recipe passes"; else fail "the real recipe" "$out"; fi

# broken <name> <wanted text> <python edit expression over file text s> <file>
broken() {
    local name="$1" want="$2" expr="$3" file="$4" rc=0
    fresh
    python3 -I - "$tmp/t/$file" "$expr" <<'PY'
import sys
p, expr = sys.argv[1:3]
s = open(p).read()
new = eval(expr)
assert new != s, "the fixture edit changed nothing: " + expr
open(p, "w").write(new)
PY
    out="$(bash "$CHECK" "$tmp/t" 2>&1)" || rc=$?
    if [ "$rc" != 1 ]; then fail "$name" "exit $rc, wanted 1: $out"; return; fi
    if ! grep -qF -- "$want" <<<"$out"; then fail "$name" "no '$want' in: $out"; return; fi
    pass "$name"
}

echo "== broken recipes =="
broken "install hooks drift from packaging/arch" "k2-bin.install differs" 's + "# drift\n"' "$R/k2-bin.install"
broken "a stale sum for the install hooks" "k2-bin.install's sum is" 's.replace("44073b55", "00000000")' "$R/PKGBUILD"
broken "a licence LICENSE.md doesn't state" "but LICENSE.md is MIT" 's.replace("license=(\x27MIT\x27)", "license=(\x27FSL-1.1-ALv2\x27)")' "$R/PKGBUILD"
broken "the old non-SPDX licence id" "but LICENSE.md is MIT" 's.replace("license=(\x27MIT\x27)", "license=(\x27FSL-1.1-Apache-2.0\x27)")' "$R/PKGBUILD"
broken "LICENSE.md changes licence and the recipe does not follow" "but LICENSE.md is FSL-1.1-ALv2" '"# Functional Source License, Version 1.1, Apache 2.0 Future License\n\n## Abbreviation\n\nFSL-1.1-Apache-2.0\n\n## Notice\n"' "LICENSE.md"
broken "MIT without the licence text installed" "doesn't install LICENSE.md" 's.replace("/usr/share/licenses/${pkgname}/LICENSE.md", "/usr/share/doc/${pkgname}/LICENSE.md")' "$R/PKGBUILD"
broken "a licence the table doesn't know" "can't tell which licence" '"Some other licence\n"' "LICENSE.md"
broken "a dropped dependency" "depends differ from packaging/arch/PKGBUILD" "s.replace(\"    'libcap'\n\", '')" "$R/PKGBUILD"
broken "a dependency packaging/arch gained" "depends differ from packaging/arch/PKGBUILD" "s.replace(\"    'libcap'\n\", \"    'libcap'\n    'libfoo'\n\")" "packaging/arch/PKGBUILD"
broken "a build() function" "has a build(), prepare() or pkgver()" 's + "\nbuild() {\n    true\n}\n"' "$R/PKGBUILD"
broken "an OAuth setting" "names OAuth or signing settings" 's + "\n# K2_GMAIL_CLIENT_ID\n"' "$R/PKGBUILD"
broken "a source that isn't our release" "source is https://example.com" 's.replace("https://github.com/Alakazam-211/K2/releases/download", "https://example.com/d")' "$R/PKGBUILD"
broken "SKIP instead of a sum" "not a sha256" 's.replace("d4aa363d4a3f26c441467651ae9a372a30f770d9f45cf891bdc2ac6ad3bcb28c", "SKIP")' "$R/PKGBUILD"
broken "noextract dropped" "noextract is" 's.replace("noextract=(\"${_pkgname}-${pkgver}-x86_64.pkg.tar.zst\")\n", "")' "$R/PKGBUILD"
broken "no 24 h hold" "min_release_age is None" 's.replace("  \"min_release_age\": \"24h\",\n", "")' "$R/.omarchy/package.json"
broken "a watch on someone else's repo" "watch.github is" 's.replace("Alakazam-211/K2", "someone/else")' "$R/.omarchy/package.json"
broken "a pattern that takes our prerelease tags" "watch.pattern" 's.replace("[0-9]+(?:\\\\.[0-9]+)*", "[0-9a-z.]+")' "$R/.omarchy/package.json"
broken "the fast ring" "release_ring is set" 's.replace("\"source\": \"local\",", "\"source\": \"local\",\n  \"release_ring\": \"fast\",")' "$R/.omarchy/package.json"
broken "a PKGBUILD that doesn't parse" "bash -n PKGBUILD" 's + "\nif then\n"' "$R/PKGBUILD"

echo
echo "check-omarchy-recipe: $npass passed, $nfail failed"
[ "$nfail" -eq 0 ]
