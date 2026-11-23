#!/usr/bin/env bash
# Tests for scripts/brand.sh lint (scripts/brand/lint.py): the table of
# names and their answers (tests/garden/brand-lint-names.tsv), the brands
# this repo holds, a scratch brand's command, aliases, id and product, the
# sign-off for a soft collision, the live host and package checks against
# a stand-in PATH and local index files, the table of the other fields'
# shapes (tests/garden/brand-lint-fields.tsv) and --hosted. Offline: no
# network, no CLI, no cargo.
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

# ── the other fields: the shapes table ───────────────────────────────
rows=0
while IFS=$'\t' read -r field value want; do
    case "$field" in ''|'#'*|field) continue ;; esac
    rows=$((rows + 1))
    table="${field%%.*}"
    brand "f$rows" q8 q8 "Initech Hub"
    if [ "$table" = control ]; then
        # [control] is all six or none: the row's key on a whole table.
        {
            printf '\n[control]\n'
            for k in relay relay_list connect cert push web_api; do
                case "$k" in "${field#*.}") [ "$value" = - ] || printf '%s = %s\n' "$k" "$value" ;;
                    relay) printf 'relay = "relay.example.invalid"\n' ;;
                    *) printf '%s = "https://%s.example.invalid"\n' "$k" "${k//_/-}" ;;
                esac
            done
        } >> "$T/f$rows/brand.toml"
    elif [ "$table" = names ]; then
        # The scratch brand has [names] already: the key goes in it.
        awk -v kv="${field#*.} = $value" '{ print } /^\[names\]$/ { print kv }' "$T/f$rows/brand.toml" > "$T/f$rows/b" \
            && mv "$T/f$rows/b" "$T/f$rows/brand.toml"
    else
        printf '\n[%s]\n%s = %s\n' "$table" "${field#*.}" "$value" >> "$T/f$rows/brand.toml"
    fi
    lint "$T/f$rows"
    if [ "$want" = ok ]; then
        [ "$rc" = 0 ] || die "the fields table: $field = $value: want ok, got: $out"
    else
        [ "$rc" = 1 ] && [[ "$out" == *"field $table"*": $want: "* ]] \
            || die "the fields table: $field = $value: want $want, got: $out"
    fi
done < "$HERE/tests/garden/brand-lint-fields.tsv"
[ "$rows" -ge 60 ] || die "the fields table has only $rows rows"; ok
# A finding names the field and says what shape it wants.
brand f-why q8 q8 "Initech Hub"
printf '\n[links]\nsite = "http://example.invalid"\n' >> "$T/f-why/brand.toml"
lint "$T/f-why"
[[ "$out" == *"brand lint: q8: field links.site: shape: must be an https URL"* ]] || die "a shape finding's line: $out"; ok

# ── --hosted: what a hosted box of a brand needs ──────────────────────
lint "$HERE/brand/xy" --hosted
[ "$rc" = 0 ] && [[ "$out" == *"ok (the written lists, the hosted fields)"* ]] || die "brand/xy --hosted: $out"; ok
# The template is whole but for its test-only update key: --hosted (a
# release's lint) refuses that key and nothing else.
lint "$HERE/templates/distributor/brand" --hosted
[ "$rc" = 1 ] && [[ "$out" == *"field update.keys: testkey: holds the distributor template's test-only key"* ]] \
    && [[ "$out" == *"REFUSED (1 finding(s))"* ]] || die "the template --hosted: $out"; ok
lint "$HERE/templates/distributor/brand"; [ "$rc" = 0 ] || die "the template without --hosted: $out"; ok
# With a key of its own it passes; the same key by its line alone is still refused.
cp -R "$HERE/templates/distributor/brand" "$T/h-own"
tkey="$(sed -n 's/.*key_id = "\([0-9A-F]*\)", pubkey = "\([^"]*\)".*/\1 \2/p' "$HERE/templates/distributor/brand/brand.toml")"
[ -n "$tkey" ] || die "the template has no update key line"
own_id=25B77D84DA3C24B8 own_line=RWS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m
sed -i.bak -e "s#key_id = \"${tkey%% *}\", pubkey = \"${tkey#* }\"#key_id = \"$own_id\", pubkey = \"$own_line\"#" "$T/h-own/brand.toml" && rm -f "$T/h-own/brand.toml.bak"
lint "$T/h-own" --hosted
[ "$rc" = 0 ] && [[ "$out" == *"ok (the written lists, the hosted fields)"* ]] || die "the template with its own key --hosted: $out"; ok
# Ours are never a partner's box: --hosted checks their shapes only.
lint "$HERE/brand/k2" --hosted
[ "$rc" = 0 ] && [[ "$out" == *"ok (the written lists)"* ]] || die "brand/k2 --hosted: $out"; ok
# A brand with only its names lacks every field a hosted box needs.
brand h-bare q8 q8 "Initech Hub"
lint "$T/h-bare"; [ "$rc" = 0 ] || die "without --hosted the hosted fields are not asked for: $out"; ok
lint "$T/h-bare" --hosted
[ "$rc" = 1 ] || die "a bare brand --hosted: $out"
for f in names.company names.login zones.label zones.internal package.update_feed update.keys \
    feeds.daemon feeds.downloads look.icons links.site links.support_email links.support_url \
    links.terms links.privacy links.dashboard links.feedback mail.postal; do
    [[ "$out" == *"field $f: hosted: a hosted box needs it set"* ]] || die "a bare brand --hosted names $f: $out"
done
[ "$(printf '%s\n' "$out" | grep -c ': hosted: ')" = 17 ] || die "a bare brand --hosted: 17 fields: $out"; ok
# System mail has no fleet-wide sender: left out, it is noreply@{server}.
[[ "$out" != *"mail.system_from"* ]] || die "mail.system_from is not asked of a hosted brand: $out"; ok
# A link that may not be none, and a value that is the default brand's own.
cp -R "$HERE/templates/distributor/brand" "$T/h-terms"
sed -i.bak 's#^terms = .*#terms = ""#' "$T/h-terms/brand.toml" && rm -f "$T/h-terms/brand.toml.bak"
lint "$T/h-terms" --hosted
[ "$rc" = 1 ] && [[ "$out" == *"field links.terms: hosted: a hosted box needs a link here"* ]] || die "an empty terms link: $out"; ok
cp -R "$HERE/templates/distributor/brand" "$T/h-ours"
site="$(sed -n 's/^site = //p' "$HERE/brand/k2/brand.toml")"
mail="$(sed -n 's/^support_email = //p' "$HERE/brand/k2/brand.toml")"
[ -n "$site" ] && [ -n "$mail" ] || die "brand/k2 has no links.site or links.support_email"
sed -i.bak -e "s#^site = .*#site = $site#" -e "s#^support_email = .*#support_email = $mail#" "$T/h-ours/brand.toml" && rm -f "$T/h-ours/brand.toml.bak"
lint "$T/h-ours" --hosted
[ "$rc" = 1 ] && [[ "$out" == *"field links.site: ours: is the default brand's own value"* ]] \
    && [[ "$out" == *"field links.support_email: ours:"* ]] || die "the default brand's own values: $out"; ok
lint "$T/h-ours"; [ "$rc" = 0 ] || die "the default brand's values are only refused with --hosted: $out"; ok

# ── [control] on our brands: the bash CLI's copy ──────────────────────
# A stand-in repo: the lint, our brand folders, the template and the bash
# CLI's part that holds the Connect address.
R2="$T/repo"
mkdir -p "$R2/scripts" "$R2/cli/src" "$R2/templates/distributor"
cp -R "$HERE/scripts/brand" "$R2/scripts/brand"
cp -R "$HERE/brand" "$R2/brand"
cp -R "$HERE/templates/distributor/brand" "$R2/templates/distributor/brand"
cp "$HERE/cli/src/connect.sh" "$R2/cli/src/connect.sh"
rlint() { rc=0; out="$("$(command -v python3)" "$R2/scripts/brand/lint.py" "$@" 2>&1)" || rc=$?; }
where="cli/src/connect.sh:$(grep -n '^_envn_into K2_CONNECT_BASE CONNECT_BASE ' "$R2/cli/src/connect.sh" | cut -d: -f1)"
[ "$where" != "cli/src/connect.sh:" ] || die "cli/src/connect.sh has no _envn_into K2_CONNECT_BASE line"
rlint "$R2/brand/k2"; [ "$rc" = 0 ] || die "brand/k2's [control] matches the bash copy: $out"; ok
sed -i.bak 's#^connect = .*#connect = "https://connect.example.invalid"#' "$R2/brand/k2/brand.toml" && rm -f "$R2/brand/k2/brand.toml.bak"
rlint "$R2/brand/k2"
[ "$rc" = 1 ] && [[ "$out" == *"field control.connect: bash: "*"is not the bash CLI's copy"*"($where)"* ]] || die "brand/k2's connect off the bash copy: $out"; ok
cp "$HERE/brand/k2/brand.toml" "$R2/brand/k2/brand.toml"
sed -n '/^\[control\]$/,/^$/p' "$HERE/brand/k2/brand.toml" >> "$R2/brand/k5/brand.toml"
rlint "$R2/brand/k5"
[ "$rc" = 1 ] && [[ "$out" == *"field control: bash: our brand k5 may not set [control]"*"$where"* ]] || die "[control] on brand/k5: $out"; ok
cp "$HERE/brand/k5/brand.toml" "$R2/brand/k5/brand.toml"
grep -v '^_envn_into K2_CONNECT_BASE' "$HERE/cli/src/connect.sh" > "$R2/cli/src/connect.sh"
rlint "$R2/brand/k2"
[ "$rc" = 1 ] && [[ "$out" == *"the check of our [control] against the bash CLI did not run"* ]] || die "no bash copy: $out"; ok
# A brand not ours may set a whole [control]; the bash rule is not its.
brand c-own q8 q8 "Initech Hub"
sed -n '/^\[control\]$/,/^$/p' "$HERE/brand/k2/brand.toml" | sed 's#k2\.dev#example.invalid#' >> "$T/c-own/brand.toml"
lint "$T/c-own"; [ "$rc" = 0 ] || die "a whole [control] on a brand not ours: $out"; ok

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
    "--registry $REG" "--name q7 --hosted" "$HERE/brand/xy --hosted --hosted" "--hosted"; do
    # shellcheck disable=SC2086
    lint $args
    [ "$rc" = 2 ] || die "args '$args': rc $rc: $out"; ok
done
rc=0; bash "$B" nope >/dev/null 2>&1 || rc=$?; [ "$rc" = 2 ] || die "brand.sh nope: rc $rc"; ok

echo "brand lint tests: $pass passed"
