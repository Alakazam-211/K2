#!/usr/bin/env bash
# scripts/check-third-party-notices.sh on a copy of the tree: it passes on
# the real files, and fails on each way an output can lose the notices
# (third-party licence notices must ship in every package). No build; temp dirs only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CHECK="$ROOT/scripts/check-third-party-notices.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
no() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

fresh() {
    rm -rf "$WORK/t"
    mkdir -p "$WORK/t/src-tauri" "$WORK/t/scripts/notices" "$WORK/t/packaging/arch"
    cp "$ROOT/NOTICE.md" "$ROOT/vite.config.web.ts" "$WORK/t/"
    cp -R "$ROOT/resources" "$WORK/t/resources"
    cp "$ROOT/src-tauri/tauri.conf.json" "$WORK/t/src-tauri/"
    cp "$ROOT/scripts/package-daemon-deb.sh" "$WORK/t/scripts/"
    cp "$ROOT/scripts/notices/web-notices-plugin.mjs" "$WORK/t/scripts/notices/"
    cp "$ROOT/packaging/arch/PKGBUILD" "$WORK/t/packaging/arch/"
    # Every file part 5 copies (its BEGIN FILE blocks name them), so the
    # copy checks against the same bytes.
    python3 -I - "$ROOT" "$WORK/t" <<'PY'
import os, re, shutil, sys
root, dst = sys.argv[1], sys.argv[2]
s = open(os.path.join(root, "resources/THIRD-PARTY-NOTICES.md"), encoding="utf-8").read()
for names in re.findall(r"<!-- BEGIN (?:FILE(?: lines=\d+-\d+)?|COMMENTS): ([^>]+?) -->", s):
    for n in names.split():
        os.makedirs(os.path.dirname(os.path.join(dst, n)), exist_ok=True)
        shutil.copyfile(os.path.join(root, n), os.path.join(dst, n))
PY
}

run() { K2_NOTICES_ROOT="$WORK/t" bash "$CHECK" "$@" >"$WORK/out" 2>&1; }

expect_pass() {
    if run "${@:2}"; then ok "$1"; else no "$1"; cat "$WORK/out" >&2; fi
}

expect_fail() {
    local label="$1" want="$2"
    shift 2
    if run "$@"; then
        no "$label (the check passed)"
    elif grep -qF -- "$want" "$WORK/out"; then
        ok "$label"
    else
        no "$label (no '$want' in the output)"
        cat "$WORK/out" >&2
    fi
}

# Python edits: no sed -i portability trouble, and fail loud when the text
# to change is not there.
edit() {
    python3 -I - "$WORK/t/$1" "$2" "$3" <<'PY'
import sys
path, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path, encoding="utf-8").read()
if old not in s:
    sys.exit(f"edit: {old!r} not in {path}")
open(path, "w", encoding="utf-8").write(s.replace(old, new))
PY
}

echo "── third-party notices check ──"
fresh
expect_pass "the real tree passes"

fresh
edit NOTICE.md "## Trademarks" "## Trademarks (edited)"
expect_fail "NOTICE.md changed, its copy not" "differs from NOTICE.md"
expect_pass "--sync copies NOTICE.md in again" --sync
grep -qF "## Trademarks (edited)" "$WORK/t/resources/THIRD-PARTY-NOTICES.md" \
    && ok "--sync wrote the edited NOTICE.md" || no "--sync wrote the edited NOTICE.md"

fresh
rm "$WORK/t/resources/THIRD-PARTY-NOTICES.md"
expect_fail "the file is gone" "is missing"

fresh
edit resources/THIRD-PARTY-NOTICES.md "PostgreSQL Database Management System" "PostgreSQL"
expect_fail "the PostgreSQL License is gone" "lacks: PostgreSQL Database Management System"

fresh
edit resources/THIRD-PARTY-NOTICES.md "The author disclaims copyright to this source code." "SQLite."
expect_fail "SQLite's dedication is gone" "lacks: The author disclaims copyright"

fresh
edit resources/THIRD-PARTY-NOTICES.md "Written offer:" "Offer:"
expect_fail "the libgit2 source offer is gone" "lacks: Written offer:"

fresh
edit resources/THIRD-PARTY-NOTICES.md "OpenSSL 3 ships no NOTICE file" "OpenSSL 3"
expect_fail "the OpenSSL note is gone" "lacks: OpenSSL 3 ships no NOTICE file"

# Part 5: a pointed-to licence file changes, its copy does not.
fresh
printf 'one more line\n' >> "$WORK/t/third_party/wry/LICENSE-MIT"
expect_fail "a licence file changed, part 5's copy not" "part 5's copy of third_party/wry/LICENSE-MIT differs"

fresh
printf 'one more line\n' >> "$WORK/t/vendor/k2-mpc-cad/LICENSE-APACHE"
expect_fail "one of several same-bytes files changed" "part 5's copy of vendor/k2-mpc-cad/LICENSE-APACHE differs"

fresh
printf 'changed\n' >> "$WORK/t/crates/k2-vault/data/public_suffix_list.dat"
expect_pass "a line range copy ignores lines after it"

# Mermaid's bundled licence comments change in the vendored file.
fresh
edit src/renderer/public/vendor/mermaid.min.js "js-yaml 4.1.1" "js-yaml 4.1.2"
expect_fail "the bundle's licence comments changed, part 5's copy not" "bundled licence comments differs"

# The MPL-2.0 text twice, or not at all.
fresh
printf '\nMozilla Public License Version 2.0\n' >> "$WORK/t/resources/THIRD-PARTY-NOTICES.md"
expect_fail "the MPL-2.0 text twice" "MPL-2.0 text exactly once"

fresh
edit resources/THIRD-PARTY-NOTICES.md "https://github.com/Alakazam-211/K2/blob/v<version>/crates/k2-vault/data/public_suffix_list.dat" "the repo"
expect_fail "the Public Suffix List's repo path is gone" "lacks: https://github.com/Alakazam-211/K2/blob"

# A new NOTICE.md entry pointing at a licence file that does not ship.
fresh
mkdir -p "$WORK/t/third_party/newdep"
printf 'MIT\n' > "$WORK/t/third_party/newdep/LICENSE"
printf '\n| **License** | **MIT** — full text in `third_party/newdep/LICENSE` |\n' >> "$WORK/t/NOTICE.md"
K2_NOTICES_ROOT="$WORK/t" bash "$CHECK" --sync >/dev/null 2>&1 || true
expect_fail "a new pointer to a non-shipping file" "NOTICE.md points to third_party/newdep/LICENSE, which does not ship"

fresh
edit src-tauri/tauri.conf.json '"../resources/THIRD-PARTY-NOTICES.md", ' ''
expect_fail "the app bundle drops the file" "lacks ../resources/THIRD-PARTY-NOTICES.md"

fresh
edit src-tauri/tauri.conf.json '"../resources/notices/shiki-hcl-SOURCE.tmGrammar.json", ' ''
expect_fail "the app bundle drops an MPL Source Code Form" "lacks ../resources/notices/shiki-hcl-SOURCE.tmGrammar.json"

fresh
edit scripts/package-daemon-deb.sh '"$PKGROOT/usr/share/doc/k2-daemon/THIRD-PARTY-NOTICES.md"' '"$PKGROOT/usr/share/doc/k2-daemon/X"'
expect_fail "the server .deb drops the file" "server .deb"

fresh
edit packaging/arch/PKGBUILD '"${tree}"/resources/notices/*' '"${tree}"/resources/none/*'
expect_fail "the Arch package drops resources/notices" "Arch package"

fresh
edit vite.config.web.ts ', webNoticesPlugin(__dirname)' ''
expect_fail "the web build drops the plugin" "web app"

# A web build's notices folder, made with the plugin's own functions (what
# its generateBundle emits), then broken.
fresh
mkdir -p "$WORK/web/notices"
node --input-type=module - "$WORK/t" "$WORK/web" <<'JS'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
const [root, out] = process.argv.slice(2)
const { webNoticeFiles, noticesIndexHtml, WEB_NOTICES_DIR } = await import(join(root, 'scripts/notices/web-notices-plugin.mjs'))
const files = webNoticeFiles(root).map((f) => {
  const bytes = readFileSync(f.path)
  return { ...f, bytes, text: bytes.toString('utf8') }
})
for (const f of files) writeFileSync(join(out, WEB_NOTICES_DIR, f.name), f.bytes)
writeFileSync(join(out, WEB_NOTICES_DIR, 'index.html'), noticesIndexHtml(files))
JS
expect_pass "a web build with the plugin's files passes" --web-out "$WORK/web"
grep -qF "<pre>" "$WORK/web/notices/index.html" && ok "the notices page shows the texts" || no "the notices page shows the texts"
rm "$WORK/web/notices/shiki-bird2-SOURCE.tmLanguage.json"
expect_fail "a web build without bird2's Source Code Form" "lacks notices/shiki-bird2-SOURCE.tmLanguage.json" --web-out "$WORK/web"
rm "$WORK/web/notices/THIRD-PARTY-NOTICES.md"
expect_fail "a web build without the file" "lacks notices/THIRD-PARTY-NOTICES.md" --web-out "$WORK/web"

# A built app's resources, Tauri's _up_ layout.
fresh
mkdir -p "$WORK/app/_up_/resources"
cp -R "$WORK/t/resources/notices" "$WORK/app/_up_/resources/notices"
cp "$WORK/t/resources/THIRD-PARTY-NOTICES.md" "$WORK/app/_up_/resources/"
expect_pass "app resources with the files pass" --app "$WORK/app"
rm "$WORK/app/_up_/resources/THIRD-PARTY-NOTICES.md"
expect_fail "app resources without the file" "lack _up_/resources/THIRD-PARTY-NOTICES.md" --app "$WORK/app"

echo ""
echo "third-party notices: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
