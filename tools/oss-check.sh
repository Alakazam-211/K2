#!/bin/sh
# K-OSS-3: the repository carries its licence, MIT OR Apache-2.0. Fail unless
#   - LICENSE-MIT (the MIT licence) and LICENSE-APACHE (the Apache License, Version 2.0) are at
#     the root, and NOTICE.md is there too;
#   - every crate of ours ([package] in a tracked Cargo.toml) says
#     `license = "MIT OR Apache-2.0"`, or `license.workspace = true` with the root Cargo.toml's
#     [workspace.package] saying it, and `publish = false` (directly or by the workspace);
#   - every package.json of ours says `"license": "MIT OR Apache-2.0"`, and so does its
#     package-lock.json's root entry when it has one;
#   - a package.json that lists LICENSE-MIT or LICENSE-APACHE (in "files") has the file beside
#     it, byte-identical to the root's (npm packs only what is under the package's folder).
# Not ours, so skipped: vendor/ (copies of other projects, each with its own licence),
# node_modules/, target/, and test data under a tests/corpus/ folder (a sample Cargo.toml or
# package.json there is a test input). A crate inheriting from a workspace is resolved against
# its own [workspace.package] if it has one, else the root's. POSIX sh, grep, awk and cmp only.
#
#   tools/oss-check.sh [ROOT]       (default: the repo root)
#
# Files: `git ls-files` when ROOT is a git checkout, else every file under ROOT (a node job's
# unpacked bundle). Exit 0 clean, 1 findings (each printed), 2 usage. tools/test-oss-check.sh
# is its self-test (planted failures).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "oss-check: no directory $ROOT" >&2; exit 2; }
cd "$ROOT"
LIC='MIT OR Apache-2.0'
SKIP='^(vendor|target|node_modules)/|/node_modules/|(^|/)tests/corpus/'
LIST=$(mktemp); OUT=$(mktemp)
trap 'rm -f "$LIST" "$OUT"' EXIT
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -z | tr '\0' '\n' > "$LIST"
else
  find . -type f ! -path './target/*' ! -path './.git/*' | sed 's|^\./||' > "$LIST"
fi
: > "$OUT"
bad() { echo "$*" >> "$OUT"; }

# the licence texts and the notice
if [ ! -f LICENSE-MIT ]; then bad "LICENSE-MIT: missing at the root"
elif ! grep -q '^MIT License' LICENSE-MIT; then bad "LICENSE-MIT: not the MIT licence (no 'MIT License' line)"; fi
if [ ! -f LICENSE-APACHE ]; then bad "LICENSE-APACHE: missing at the root"
elif ! grep -q 'Apache License' LICENSE-APACHE || ! grep -q 'Version 2.0' LICENSE-APACHE; then
  bad "LICENSE-APACHE: not the Apache License, Version 2.0"; fi
[ -f NOTICE.md ] || bad "NOTICE.md: missing at the root"

# `key value` lines from a manifest's [package] (pkg.*) and [workspace.package] (ws.*):
# license and publish, a value as written or `workspace` for `key.workspace = true`
fields() {
  awk '
    /^[ \t]*\[/ { sec = $0; gsub(/[ \t]/, "", sec); next }
    sec == "[package]" || sec == "[workspace.package]" {
      p = (sec == "[package]") ? "pkg." : "ws."
      if ($0 ~ /^[ \t]*(license|publish)\.workspace[ \t]*=[ \t]*true/) {
        k = $0; sub(/^[ \t]*/, "", k); sub(/\..*/, "", k); print p k " workspace"; next
      }
      if ($0 ~ /^[ \t]*(license|publish)[ \t]*=/) {
        k = $0; sub(/^[ \t]*/, "", k); sub(/[ \t]*=.*/, "", k)
        v = $0; sub(/^[^=]*=[ \t]*/, "", v); sub(/[ \t]+#.*$/, "", v); sub(/[ \t]+$/, "", v)
        print p k " " v
      }
    }' "$1"
}
get() { # file key -> its value, or empty
  fields "$1" | awk -v k="$2" '$1 == k { sub(/^[^ ]+ /, ""); print; exit }'
}
ROOTLIC=""; ROOTPUB=""
if [ -f Cargo.toml ]; then ROOTLIC=$(get Cargo.toml ws.license); ROOTPUB=$(get Cargo.toml ws.publish); fi

crates=0; pkgs=0
while IFS= read -r f; do
  [ -f "$f" ] || continue
  printf '%s\n' "$f" | grep -Eq "$SKIP" && continue
  case $f in
    Cargo.toml|*/Cargo.toml)
      fields "$f" | grep -q '^pkg\.' || grep -q '^\[package\]' "$f" || continue
      crates=$((crates + 1))
      wl=$(get "$f" ws.license); [ -n "$wl" ] || wl=$ROOTLIC
      wp=$(get "$f" ws.publish); [ -n "$wp" ] || wp=$ROOTPUB
      l=$(get "$f" pkg.license); p=$(get "$f" pkg.publish)
      [ "$l" = workspace ] && l=$wl
      [ "$p" = workspace ] && p=$wp
      [ "$l" = "\"$LIC\"" ] || bad "$f: license is ${l:-missing}, not \"$LIC\""
      [ "$p" = false ] || bad "$f: publish is ${p:-missing, which cargo reads as true}, not false"
      ;;
    package.json|*/package.json)
      pkgs=$((pkgs + 1))
      d=$(dirname "$f")
      grep -Eq "^  \"license\": \"$LIC\",?\$" "$f" || bad "$f: no top-level \"license\": \"$LIC\""
      if [ -f "$d/package-lock.json" ]; then
        awk -v want="      \"license\": \"$LIC\"," '
          /^    "": \{/ { inroot = 1; next }
          inroot && /^    \}/ { exit }
          inroot && ($0 == want || $0 "," == want) { found = 1 }
          END { exit !found }' "$d/package-lock.json" ||
          bad "$d/package-lock.json: its root entry has no \"license\": \"$LIC\""
      fi
      for n in LICENSE-MIT LICENSE-APACHE; do
        grep -q "\"$n\"" "$f" || continue
        if [ ! -f "$d/$n" ]; then bad "$f lists $n, and $d/$n is missing"
        elif [ -f "$n" ] && ! cmp -s "$n" "$d/$n"; then bad "$d/$n: differs from the root's $n"; fi
      done
      ;;
  esac
done < "$LIST"

if [ -s "$OUT" ]; then
  cat "$OUT"
  echo "oss-check: $(wc -l < "$OUT" | tr -d ' ') problem(s) ($crates crates, $pkgs package.json checked); K-OSS-3: MIT OR Apache-2.0" >&2
  exit 1
fi
echo "oss-check: ok (LICENSE-MIT, LICENSE-APACHE, NOTICE.md; $crates crates, $pkgs package.json: $LIC)"
