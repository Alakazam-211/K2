#!/bin/sh
# tools/oss-check.sh's self-test: a good tree passes, and each planted failure fails (a
# missing licence file, a wrong or missing licence field, a client copy that differs...).
# Each case is a small git repo in a temporary folder, built from the real LICENSE files.
# POSIX sh and git only; no cargo, no node.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
CHECK=$HERE/oss-check.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
LIC='MIT OR Apache-2.0'
fails=0; n=0

good() { # $1: the tree's folder
  mkdir -p "$1/crates/a" "$1/extra" "$1/client" "$1/vendor/other" "$1/crates/a/tests/corpus"
  cp "$REPO/LICENSE-MIT" "$REPO/LICENSE-APACHE" "$1/"
  printf '# Third-party notices\n' > "$1/NOTICE.md"
  printf '[workspace]\nmembers = ["crates/a"]\nexclude = ["vendor", "extra"]\n\n[workspace.package]\nedition = "2021"\npublish = false\nlicense = "%s"\n' "$LIC" > "$1/Cargo.toml"
  printf '[package]\nname = "a"\nversion = "0.1.0"\nedition.workspace = true\nlicense.workspace = true\npublish.workspace = true\n' > "$1/crates/a/Cargo.toml"
  printf '[package]\nname = "a-extra"\nversion = "0.0.0"\nlicense = "%s"\nedition = "2021"\npublish = false\n\n[workspace]\n' "$LIC" > "$1/extra/Cargo.toml"
  printf '{\n  "name": "c",\n  "version": "0.1.0",\n  "license": "%s",\n  "private": true,\n  "files": [\n    "src",\n    "LICENSE-MIT",\n    "LICENSE-APACHE"\n  ]\n}\n' "$LIC" > "$1/client/package.json"
  printf '{\n  "name": "c",\n  "lockfileVersion": 3,\n  "packages": {\n    "": {\n      "name": "c",\n      "version": "0.1.0",\n      "license": "%s"\n    },\n    "node_modules/x": {\n      "license": "ISC"\n    }\n  }\n}\n' "$LIC" > "$1/client/package-lock.json"
  cp "$REPO/LICENSE-MIT" "$REPO/LICENSE-APACHE" "$1/client/"
  # not ours: another project's crate with no licence field, and a test input
  printf '[package]\nname = "other"\nversion = "1.0.0"\n' > "$1/vendor/other/Cargo.toml"
  printf '[package]\nname = "sample"\nversion = "1.0.0"\n' > "$1/crates/a/tests/corpus/Cargo.toml"
}
case_() { # name, expected exit, a command run in the tree to plant the failure
  name=$1; want=$2; plant=$3; n=$((n + 1))
  d=$TMP/t$n; mkdir "$d"; good "$d"
  (cd "$d" && sh -c "$plant")
  (cd "$d" && git init -q . && git add -A)
  got=0; sh "$CHECK" "$d" > "$d.out" 2>&1 || got=$?
  if [ "$got" -eq "$want" ]; then echo "ok   $name"; else echo "FAIL $name: exit $got, want $want"; sed 's/^/     /' "$d.out"; fails=1; fi
}
case_ "the good tree passes"                    0 ":"
case_ "no LICENSE-MIT fails"                    1 "rm LICENSE-MIT"
case_ "no LICENSE-APACHE fails"                 1 "rm LICENSE-APACHE"
case_ "a LICENSE-MIT that isn't MIT fails"      1 ": > LICENSE-MIT"
case_ "a LICENSE-APACHE that isn't 2.0 fails"   1 "printf 'GNU General Public License\n' > LICENSE-APACHE"
case_ "no NOTICE.md fails"                      1 "rm NOTICE.md"
case_ "a crate's wrong licence fails"           1 "sed -i.bak 's/^license = .*/license = \"MIT\"/' extra/Cargo.toml"
case_ "a crate with no licence fails"           1 "sed -i.bak '/^license/d' extra/Cargo.toml"
case_ "a wrong workspace licence fails"         1 "sed -i.bak 's/^license = .*/license = \"Apache-2.0\"/' Cargo.toml"
case_ "a crate that can be published fails"     1 "sed -i.bak '/^publish/d' extra/Cargo.toml"
case_ "a package.json's wrong licence fails"    1 "sed -i.bak 's/\"license\": .*/\"license\": \"MIT\",/' client/package.json"
case_ "a package.json with no licence fails"    1 "sed -i.bak '/\"license\"/d' client/package.json"
case_ "a lockfile root's wrong licence fails"   1 "sed -i.bak 's/\"license\": \"MIT OR Apache-2.0\"/\"license\": \"MIT\"/' client/package-lock.json"
case_ "a client copy that differs fails"        1 "printf 'x\n' >> client/LICENSE-MIT"
case_ "a listed client copy missing fails"      1 "rm client/LICENSE-APACHE"
case_ "vendor/ and test corpus crates are skipped" 0 "printf '[package]\nname = \"v2\"\n' > vendor/other/Cargo2.toml"
[ $fails -eq 0 ] && echo "oss-check self-test: ok ($n cases)" || { echo "oss-check self-test: FAILED"; exit 1; }
