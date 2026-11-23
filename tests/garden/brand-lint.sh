#!/usr/bin/env bash
# Tests for scripts/brand.sh lint (scripts/brand/lint.py): the table of
# names and their answers (tests/garden/brand-lint-names.tsv), the brands
# this repo holds, a scratch brand's command, aliases, id and product, the
# sign-off for a soft collision, and the live host and package checks
# against a stand-in PATH and local index files. Offline: no network, no
# CLI, no cargo.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
B="$HERE/scripts/brand.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export PYTHONDONTWRITEBYTECODE=1
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $*" >&2; exit 1; }
lint() { rc=0; out="$(bash "$B" lint "$@" 2>&1)" || rc=$?; }

# ── the table ──────────────────────────────────────────────────────────
rows=0
while IFS=$'\t' read -r name product want; do
    case "$name" in ''|'#'*|command) continue ;; esac
    if [ "$product" = - ]; then
        got="$(bash "$B" lint --name "$name" 2>/dev/null || true)"
    else
        got="$(bash "$B" lint --name "$name" --product "$product" 2>/dev/null || true)"
    fi
    [ "$got" = "$(printf '%s\t%s' "$name" "$want")" ] || die "the table: $name / $product: want $want, got ${got#*	}"
    rows=$((rows + 1))
done < "$HERE/tests/garden/brand-lint-names.tsv"
[ "$rows" -ge 45 ] || die "the table has only $rows rows"; ok
lint --name q7; [ "$rc" = 0 ] || die "an ok name exits 0: $out"; ok
lint --name cd; [ "$rc" = 1 ] && [[ "$out" == *"hard: posix"* ]] || die "a hard name exits 1 and says why: $out"; ok

# ── the brands we hold ─────────────────────────────────────────────────
for b in k2 k5 xy; do
    lint "$HERE/brand/$b"
    [ "$rc" = 0 ] || die "brand/$b: $out"; ok
done
case "$out" in *"brand lint: xy: command xy, product 'Globex Desk': ok"*) ok ;; *) die "brand/xy: $out" ;; esac

# ── a scratch brand ───────────────────────────────────────────────────
brand() { # brand <folder> <id> <command> <product> [<alias>...]
    local d="$T/$1" id="$2" cli="$3" product="$4"; shift 4
    mkdir -p "$d"
    local aliases=""
    for a in "$@"; do aliases="$aliases{ name = \"$a\", mode = \"notice\" }, "; done
    printf 'id = "%s"\nschema = 1\n\n[names]\nproduct = "%s"\n\n[cli]\nname = "%s"\naliases = [%s]\n' "$id" "$product" "$cli" "$aliases" > "$d/brand.toml"
}
brand good q8 q8 "Initech Hub"
lint "$T/good"; [ "$rc" = 0 ] || die "a good scratch brand: $out"; ok
# The command and the product are checked each on its own rules.
brand mixed q8 cd "K5 Desk"
lint "$T/mixed"
[ "$rc" = 1 ] && [[ "$out" == *"command cd: hard"* ]] && [[ "$out" == *"product K5 Desk: ours"* ]] || die "command and product: $out"; ok
# An alias: no alias of ours, and none another brand holds.
brand alias q8 q8 "Initech Hub" k5 xy
lint "$T/alias"
[ "$rc" = 1 ] && [[ "$out" == *"alias k5: ours"* ]] && [[ "$out" == *"alias xy: unique: brand xy has it as its command"* ]] || die "aliases: $out"; ok
# An id another brand holds, an id that is not a name's shape.
brand ids xy q8 "Initech Hub"
lint "$T/ids"; [ "$rc" = 1 ] && [[ "$out" == *"id xy: unique"* ]] || die "a held id: $out"; ok
brand ids2 Q-8 q8 "Initech Hub"
lint "$T/ids2"; [ "$rc" = 1 ] && [[ "$out" == *"id Q-8: shape"* ]] || die "an id's shape: $out"; ok
# A brand named k2 outside brand/ is not ours.
brand fake k2 k2 "K2"
lint "$T/fake"; [ "$rc" = 1 ] && [[ "$out" == *"command k2: ours"* ]] && [[ "$out" == *"product K2: ours"* ]] || die "a k2 brand outside brand/: $out"; ok
# Another brand's product, in any case.
brand prod q8 q8 "globex desk"
lint "$T/prod"; [ "$rc" = 1 ] && [[ "$out" == *"product globex desk: unique: brand xy has it as its product"* ]] || die "a held product: $out"; ok

# ── a soft collision and its sign-off ─────────────────────────────────
brand soft zf zf "Initech Hub"
lint "$T/soft"; [ "$rc" = 1 ] && [[ "$out" == *"command zf: soft: homebrew"* ]] || die "a soft name: $out"; ok
printf '[[ok]]\nname = "zf"\nk2 = "K2, a message id"\nsteward = "the Steward, a message id"\nwhy = "a little-used formula"\n' > "$T/soft/collision-ok.toml"
lint "$T/soft"; [ "$rc" = 0 ] || die "a signed-off soft name: $out"; ok
printf '[[ok]]\nname = "zf"\nk2 = "K2, a message id"\nwhy = "only one sign-off"\n' > "$T/soft/collision-ok.toml"
lint "$T/soft"; [ "$rc" = 1 ] && [[ "$out" == *"needs name, k2, steward and why"* ]] || die "one sign-off is not enough: $out"; ok
brand hard cd cd "Initech Hub"
printf '[[ok]]\nname = "cd"\nk2 = "x"\nsteward = "y"\nwhy = "z"\n' > "$T/hard/collision-ok.toml"
lint "$T/hard"; [ "$rc" = 1 ] && [[ "$out" == *"command cd: hard"* ]] || die "a hard name can't be signed off: $out"; ok

# ── --live, against a stand-in host and local indexes ─────────────────
mkdir -p "$T/bin"
ln -s "$(command -v bash)" "$T/bin/bash"
if command -v zsh >/dev/null; then ln -s "$(command -v zsh)" "$T/bin/zsh"; fi
printf '#!/bin/sh\nexit 0\n' > "$T/bin/zk"; chmod +x "$T/bin/zk"
printf '[{"name": "abc", "aliases": [], "oldnames": []}, {"name": "qq9", "aliases": ["qq8"], "oldnames": ["qq5"], "executables": ["qq4"]}]' > "$T/brew.json"
printf 'usr/bin/qq7    utils/qq\nusr/share/doc/q9/README    doc/q9\nusr/lib/qq6/bin/qq6 x/y\n' | gzip > "$T/contents.gz"
live() {
    rc=0
    out="$(PATH="$T/bin" BRAND_LINT_BREW_JSON="file://$T/brew.json" \
        BRAND_LINT_DEBIAN_CONTENTS="file://$T/contents.gz" "$(command -v python3)" "$HERE/scripts/brand/lint.py" "$1" --live 2>&1)" || rc=$?
}
brand l-host q8 zk "Initech Hub"
live "$T/l-host"; [ "$rc" = 1 ] && [[ "$out" == *"command zk: host: bash finds it: $T/bin/zk"* ]] || die "a name on the host's PATH: $out"; ok
for n in qq9 qq8 qq5 qq4 qq7; do
    brand "l-$n" q8 "$n" "Initech Hub"
    live "$T/l-$n"; [ "$rc" = 1 ] && [[ "$out" == *"command $n: package:"* ]] || die "a package name $n: $out"; ok
done
brand l-free q8 qq6 "Initech Hub"
live "$T/l-free"; [ "$rc" = 0 ] && [[ "$out" == *"ok (the written lists, the host and the package indexes)"* ]] || die "a free name, live: $out"; ok
printf '[[ok]]\nname = "qq9"\nk2 = "x"\nsteward = "y"\nwhy = "z"\n' > "$T/l-qq9/collision-ok.toml"
live "$T/l-qq9"; [ "$rc" = 0 ] || die "a signed-off package name: $out"; ok
# A source that can't be read fails: a name is never free unchecked.
rc=0
out="$(PATH="$T/bin" BRAND_LINT_BREW_JSON="file://$T/nope.json" "$(command -v python3)" "$HERE/scripts/brand/lint.py" "$T/l-free" --live 2>&1)" || rc=$?
[ "$rc" = 1 ] && [[ "$out" == *"the package check did not run"* ]] || die "an index that can't be read: $out"; ok
# Ours get no live checks (our own install may be on the host).
live "$HERE/brand/k2"; [ "$rc" = 0 ] || die "brand/k2 --live: $out"; ok

# ── --registry: the private list of reserved distributor brands ───────
# A stand-in registry with neutral names (the real one never goes in the
# repo): command, product, brand id, tab-separated.
REG="$T/registry.tsv"
printf '# reserved brands\n\nq4\tInitech Hub\tinitech\nq5\tUmbrella Desk\tumbrella\n' > "$REG"
brand r-free hooli q6 "Hooli Works"
lint "$T/r-free" --registry "$REG"
[ "$rc" = 0 ] && [[ "$out" == *"ok (the written lists, the registry (2 brands))"* ]] || die "a free brand, with the registry: $out"; ok
brand r-cmd hooli q4 "Hooli Works"
lint "$T/r-cmd"; [ "$rc" = 0 ] || die "without --registry the registry is not read: $out"; ok
lint "$T/r-cmd" --registry "$REG"
[ "$rc" = 1 ] && [[ "$out" == *"command q4: registry: the reserved brand initech has it as its command"* ]] || die "a reserved command: $out"; ok
brand r-prod hooli q6 "umbrella desk"
lint "$T/r-prod" --registry "$REG"
[ "$rc" = 1 ] && [[ "$out" == *"product umbrella desk: registry: the reserved brand umbrella has it as its product"* ]] || die "a reserved product, any case: $out"; ok
brand r-id initech q6 "Hooli Works"
lint "$T/r-id" --registry "$REG"
[ "$rc" = 0 ] || die "a brand's own reservation is not a collision: $out"; ok
brand r-id2 q5 q6 "Hooli Works"
lint "$T/r-id2" --registry "$REG"
[ "$rc" = 1 ] && [[ "$out" == *"id q5: registry: the reserved brand umbrella has it as its command"* ]] || die "an id that is a reserved command: $out"; ok
brand r-alias hooli q6 "Hooli Works" q5
lint "$T/r-alias" --registry "$REG"
[ "$rc" = 1 ] && [[ "$out" == *"alias q5: registry"* ]] || die "a reserved alias: $out"; ok
got="$(bash "$B" lint --name q4 --product "Hooli Works" --registry "$REG" 2>/dev/null || true)"
[ "$got" = "$(printf 'q4\tregistry')" ] || die "--name with the registry: $got"; ok
got="$(bash "$B" lint --name q6 --product "initech hub" --registry "$REG" 2>/dev/null || true)"
[ "$got" = "$(printf 'q6\tregistry')" ] || die "--product with the registry: $got"; ok
# A missing or malformed registry stops the lint: never free unchecked.
lint "$T/r-free" --registry "$T/nope.tsv"
[ "$rc" = 1 ] && [[ "$out" == *"the registry check did not run"* ]] || die "a missing registry: $out"; ok
printf 'q4 Initech Hub initech\n' > "$T/bad.tsv"
lint "$T/r-free" --registry "$T/bad.tsv"
[ "$rc" = 1 ] && [[ "$out" == *"bad.tsv:1: not <command> <product> <brand id>"* ]] || die "a row that is not tab-separated: $out"; ok
printf 'Q-4\tInitech Hub\tinitech\n' > "$T/bad.tsv"
lint "$T/r-free" --registry "$T/bad.tsv"
[ "$rc" = 1 ] && [[ "$out" == *"must match"* ]] || die "a row whose command is not a name: $out"; ok

# ── usage ─────────────────────────────────────────────────────────────
for args in "" "--name" "--name a b" "$T/nope" "$HERE/brand/xy --nope" "$HERE/brand/xy --registry" \
    "--registry $REG"; do
    # shellcheck disable=SC2086
    lint $args
    [ "$rc" = 2 ] || die "args '$args': rc $rc: $out"; ok
done
rc=0; bash "$B" nope >/dev/null 2>&1 || rc=$?; [ "$rc" = 2 ] || die "brand.sh nope: rc $rc"; ok

echo "brand lint tests: $pass passed"
