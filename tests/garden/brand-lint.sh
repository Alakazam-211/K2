#!/usr/bin/env bash
# Tests for scripts/brand.sh lint (scripts/brand/lint.py): the table of
# names and their answers (tests/garden/brand-lint-names.tsv), the brands
# this repo holds, a scratch brand's command, aliases, id and product, the
# sign-off for a soft collision, the live host and package checks against
# a stand-in PATH and local index files, the table of the other fields'
# shapes (tests/garden/brand-lint-fields.tsv), the fields a brand that isn't
# ours must set itself, and --hosted. Offline: no network, no CLI, no cargo.
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
bare() { # bare <folder> <id> <command> <product> [<alias>...]: its names only
    local d="$T/$1" id="$2" cli="$3" product="$4"; shift 4
    mkdir -p "$d"
    local aliases=""
    for a in "$@"; do aliases="$aliases{ name = \"$a\", mode = \"notice\" }, "; done
    printf 'id = "%s"\nschema = 1\n\n[names]\nproduct = "%s"\n\n[cli]\nname = "%s"\naliases = [%s]\n' "$id" "$product" "$cli" "$aliases" > "$d/brand.toml"
}
# The fields a scratch brand (never ours) must set itself, neutral values:
# SETS_ITS_OWN and SETS_ON_PURPOSE in scripts/brand/lint.py.
OWN_NAMES='company = "Initech"
login = "Initech Account"'
OWN_TABLES='
[zones]
label = "q8.example.invalid"

[feeds]
daemon = "https://updates.example.invalid/q8/daemon-latest.json"
downloads = "https://updates.example.invalid/q8/download"

[package]
update_feed = "https://updates.example.invalid/q8/latest.json"

[update]
keys = [{ role = "primary", key_id = "25B77D84DA3C24B8", pubkey = "RWS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m" }]

[links]
site = "https://q8.example.invalid"
support_email = "help@q8.example.invalid"
contact_email = "hello@q8.example.invalid"
repo = "https://q8.example.invalid/source"
dashboard = ""
feedback = ""

[mail]
postal = false

[phone]
bundle_id = "invalid.example.q8"
team_id = "ABCDE12345"
push_topic = "invalid.example.q8"
'
REQUIRED_OWN="names.company names.login zones.label links.site links.support_email links.contact_email links.repo links.dashboard links.feedback feeds.daemon feeds.downloads package.update_feed update.keys"
REQUIRED_SET="mail.postal phone.bundle_id phone.team_id phone.push_topic"
brand() { # brand <folder> <id> <command> <product> [<alias>...]: names and the fields above
    bare "$@"
    local f="$T/$1/brand.toml"
    { sed -n '1,/^\[names\]$/p' "$f"; printf '%s\n' "$OWN_NAMES"; sed '1,/^\[names\]$/d' "$f"; printf '%s' "$OWN_TABLES"; } > "$f.new"
    mv "$f.new" "$f"
}
# set_field <file> <table> <key> <TOML value, or - to take the line out>:
# in the table when the file has it, else in a new table at the end.
set_field() {
    python3 -c '
import sys
path, table, key, value = sys.argv[1:]
lines = open(path, encoding="utf-8").read().split("\n")
head = f"[{table}]"
if head in lines:
    at = lines.index(head)
    end = next((i for i in range(at + 1, len(lines)) if lines[i].startswith("[")), len(lines))
    body = [l for l in lines[at + 1:end] if not l.startswith(f"{key} = ")]
    if value != "-":
        body.insert(0, f"{key} = {value}")
    lines = lines[:at + 1] + body + lines[end:]
elif value != "-":
    lines += ["", head, f"{key} = {value}", ""]
open(path, "w", encoding="utf-8").write("\n".join(lines))
' "$@"
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
    else
        # Into the table when the scratch brand has it (it sets the
        # fields a brand that isn't ours must), else a new table.
        set_field "$T/f$rows/brand.toml" "$table" "${field#*.}" "$value"
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
set_field "$T/f-why/brand.toml" links site '"http://example.invalid"'
lint "$T/f-why"
[[ "$out" == *"brand lint: q8: field links.site: shape: must be an https URL"* ]] || die "a shape finding's line: $out"; ok
# A root-helper key is refused with the reason, and only once.
brand f-rh q8 q8 "Initech Hub"
printf '\n[root_helper]\nkeys = []\n' >> "$T/f-rh/brand.toml"
lint "$T/f-rh"
[ "$rc" = 1 ] && [[ "$out" == *"field root_helper: roothelper: the root helper runs as root and is signed only by K5's release keys; partner builds ship ours (a new owner decision is needed if a partner runs its own servers)"* ]] \
    && [[ "$out" == *"REFUSED (1 finding(s))"* ]] || die "a root-helper table: $out"; ok

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
# A partner build carries no shell CLI (Rosson's rule): --hosted refuses
# cli.no_bash = false for a brand that is not ours, and nothing else; the
# plain lint and our own brands (which spell false) pass.
cp -R "$T/h-own" "$T/h-bash"
sed -i.bak 's#^no_bash = true$#no_bash = false#' "$T/h-bash/brand.toml" && rm -f "$T/h-bash/brand.toml.bak"
grep -qx 'no_bash = false' "$T/h-bash/brand.toml" || die "the template has no no_bash = true line to flip"
lint "$T/h-bash" --hosted
[ "$rc" = 1 ] && [[ "$out" == *"field cli.no_bash: shell: must not be false: a partner build carries no shell CLI"* ]] \
    && [[ "$out" == *"REFUSED (1 finding(s))"* ]] || die "no_bash = false --hosted: $out"; ok
lint "$T/h-bash"; [ "$rc" = 0 ] || die "no_bash = false without --hosted: $out"; ok
lint "$HERE/brand/k5" --hosted
[ "$rc" = 0 ] || die "brand/k5 (no_bash = false, ours) --hosted: $out"; ok
cp -R "$T/h-own" "$T/h-bashword"
sed -i.bak 's#^no_bash = true$#no_bash = "yes"#' "$T/h-bashword/brand.toml" && rm -f "$T/h-bashword/brand.toml.bak"
lint "$T/h-bashword"
[ "$rc" = 1 ] && [[ "$out" == *"field cli.no_bash: shape:"* ]] || die "no_bash = \"yes\": $out"; ok
# Ours are never a partner's box: --hosted checks their shapes only.
lint "$HERE/brand/k2" --hosted
[ "$rc" = 0 ] && [[ "$out" == *"ok (the written lists)"* ]] || die "brand/k2 --hosted: $out"; ok
# A brand with only its names lacks every field a brand that isn't ours
# must set, each named once; --hosted adds the rest a hosted box needs.
bare h-bare q8 q8 "Initech Hub"
lint "$T/h-bare"
[ "$rc" = 1 ] || die "a bare brand: $out"
for f in $REQUIRED_OWN; do
    [[ "$out" == *"field $f: required: is missing: a brand that isn't ours sets its own (the default brand's would show ours)"* ]] \
        || die "a bare brand names $f: $out"
done
for f in $REQUIRED_SET; do
    [[ "$out" == *"field $f: required: is missing: a brand that isn't ours sets it on purpose (the default brand's is ours)"* ]] \
        || die "a bare brand names $f: $out"
done
[ "$(printf '%s\n' "$out" | grep -c ': required: ')" = 17 ] || die "a bare brand: 17 fields: $out"
[[ "$out" != *": hosted: "* ]] || die "without --hosted the hosted fields are not asked for: $out"; ok
lint "$T/h-bare" --hosted
[ "$rc" = 1 ] || die "a bare brand --hosted: $out"
for f in look.icons links.support_url links.terms links.privacy; do
    [[ "$out" == *"field $f: hosted: a hosted box needs it set"* ]] || die "a bare brand --hosted names $f: $out"
done
[ "$(printf '%s\n' "$out" | grep -c ': hosted: ')" = 4 ] || die "a bare brand --hosted: 4 more fields: $out"
[ "$(printf '%s\n' "$out" | grep -c ': required: ')" = 17 ] || die "a bare brand --hosted: each required field once: $out"; ok
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
lint "$T/h-ours"
[ "$rc" = 1 ] && [[ "$out" == *"field links.site: ours: is the default brand's own value $site: a brand that isn't ours sets its own"* ]] \
    && [[ "$out" == *"field links.support_email: ours: is the default brand's own value $mail"* ]] \
    || die "the default brand's site and support address are refused without --hosted too: $out"; ok

# ── a brand that isn't ours sets the fields that name us ──────────────
# Each one left out is refused, naming it; so is each one set to the
# default brand's own value. The same lists and words as crates/k2-brand.
i=0
for f in $REQUIRED_OWN $REQUIRED_SET; do
    i=$((i + 1))
    brand "n-miss-$i" q8 q8 "Initech Hub"
    set_field "$T/n-miss-$i/brand.toml" "${f%%.*}" "${f#*.}" -
    lint "$T/n-miss-$i"
    [ "$rc" = 1 ] && [[ "$out" == *"field $f: required: is missing: a brand that isn't ours"* ]] \
        && [[ "$out" == *"REFUSED (1 finding(s))"* ]] || die "a brand that leaves out $f: $out"
done
ok
k2val() { # k2val <table.key>: brand/k2's value, as TOML (a string or true/false)
    python3 -c '
import json, sys, tomllib
t, k = sys.argv[2].split(".")
v = tomllib.load(open(sys.argv[1], "rb"))[t][k]
print(json.dumps(v) if isinstance(v, (str, bool)) else "")
' "$HERE/brand/k2/brand.toml" "$1"
}
i=0
for f in $REQUIRED_OWN; do
    [ "$f" = update.keys ] && continue
    i=$((i + 1))
    v="$(k2val "$f")"
    [ -n "$v" ] && [ "$v" != '""' ] || die "brand/k2 has no value of ours for $f"
    brand "n-ours-$i" q8 q8 "Initech Hub"
    set_field "$T/n-ours-$i/brand.toml" "${f%%.*}" "${f#*.}" "$v"
    lint "$T/n-ours-$i"
    [ "$rc" = 1 ] && [[ "$out" == *"field $f: ours: is the default brand's own value $v: a brand that isn't ours sets its own"* ]] \
        && [[ "$out" == *"REFUSED (1 finding(s))"* ]] || die "a brand that keeps our $f: $out"
done
ok
k2key="$(sed -n 's/.*role = "primary", key_id = "\([0-9A-F]*\)", pubkey = "\([^"]*\)".*/\1 \2/p' "$HERE/brand/k2/brand.toml")"
[ -n "$k2key" ] || die "brand/k2 has no primary update key line"
brand n-key q8 q8 "Initech Hub"
set_field "$T/n-key/brand.toml" update keys "[{ role = \"primary\", key_id = \"${k2key%% *}\", pubkey = \"${k2key#* }\" }]"
lint "$T/n-key"
[ "$rc" = 1 ] && [[ "$out" == *"field update.keys: ours: holds the default brand's own update key ${k2key%% *}: a brand that isn't ours sets its own"* ]] \
    || die "a brand that carries our update key: $out"; ok
# The phone app and Postal may be ours, written on purpose.
brand n-phone q8 q8 "Initech Hub"
for k in bundle_id team_id push_topic; do set_field "$T/n-phone/brand.toml" phone "$k" "$(k2val "phone.$k")"; done
set_field "$T/n-phone/brand.toml" mail postal true
lint "$T/n-phone"; [ "$rc" = 0 ] || die "our phone app and Postal, written on purpose: $out"; ok
# A copy of brand/k2 in a folder named k2 outside the repo's brand/ is
# not ours: it keeps our company and our update keys, and with its login
# line out, that is named too.
mkdir -p "$T/copy/brand/k2"
cp "$HERE/brand/k2/brand.toml" "$T/copy/brand/k2/brand.toml"
lint "$T/copy/brand/k2"
[ "$rc" = 1 ] && [[ "$out" == *"field names.company: ours: is the default brand's own value $(k2val names.company)"* ]] \
    && [[ "$out" == *"field update.keys: ours: holds the default brand's own update key"* ]] \
    || die "a copy of brand/k2 elsewhere: $out"; ok
grep -v '^login = ' "$HERE/brand/k2/brand.toml" > "$T/copy/brand/k2/brand.toml"
lint "$T/copy/brand/k2"
[ "$rc" = 1 ] && [[ "$out" == *"field names.login: required: is missing: a brand that isn't ours sets its own"* ]] \
    || die "a copy of brand/k2 elsewhere, its login left out: $out"; ok

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
