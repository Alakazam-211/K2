#!/usr/bin/env bash
# tests/release/k2sign-kc-items.sh: scripts/k2sign/kc-items.awk turns
# `security dump-keychain` output (attributes only, no -d) into one line
# per item: label, class, mdat, and for keys and certificates their public
# key hash. run-suites-isolated.sh compares these lines before and after
# the suites to watch the live login keychain by its items.
#
# Runs anywhere (awk only, a fixture dump; no keychain is read).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
AWK="$ROOT/scripts/k2sign/kc-items.awk"
pass=0; fail=0
ok()  { pass=$((pass + 1)); echo "PASS: $1"; }
bad() { fail=$((fail + 1)); echo "FAIL: $1"; }

T="$(mktemp -d "${TMPDIR:-/tmp}/kc-items.XXXXXX")"
trap 'rm -rf "$T"' EXIT

# A password (mdat, label in labl), a password whose labl is NULL (label
# from its print name), a certificate (no mdat; hpky), and a key (no mdat;
# print name in 0x00000001, public key hash in 0x00000006).
cat > "$T/dump" <<'DUMP'
keychain: "/Users/alice/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    0x00000007 <blob>="Acme sync"
    0x00000008 <blob>=<NULL>
    "acct"<blob>="alice"
    "cdat"<timedate>=0x32303236313130313132303030305A00  "20261101120000Z\000"
    "labl"<blob>="Acme sync"
    "mdat"<timedate>=0x32303236313131383031333530305A00  "20261118013500Z\000"
    "svce"<blob>="Acme sync"
keychain: "/Users/alice/Library/Keychains/login.keychain-db"
version: 512
class: "inet"
attributes:
    0x00000007 <blob>="example.com"
    "labl"<blob>=<NULL>
    "mdat"<timedate>=0x32303236313031303130303030305A00  "20261010100000Z\000"
    "srvr"<blob>="example.com"
keychain: "/Users/alice/Library/Keychains/login.keychain-db"
version: 512
class: 0x80001000
attributes:
    "alis"<blob>="Globex Root"
    "hpky"<blob>=0x1A2B3C4D5E6F708192A3B4C5D6E7F8091A2B3C4D  "\032+<MNo"
    "labl"<blob>="Globex Root"
keychain: "/Users/alice/Library/Keychains/login.keychain-db"
version: 512
class: 0x00000010
attributes:
    0x00000000 <uint32>=0x00000010
    0x00000001 <blob>="Acme key"
    0x00000006 <blob>=0xAABBCCDDEEFF00112233445566778899AABBCCDD  "\252\273\314"
    0x00000002 <blob>=<NULL>
DUMP

out="$(/usr/bin/awk -f "$AWK" < "$T/dump")"
printf '%s\n' "$out" > "$T/out"
n="$(wc -l < "$T/out" | tr -d ' ')"
[ "$n" = 4 ] && ok "one line per item (4)" || bad "one line per item: got $n"

line() { sed -n "${1}p" "$T/out"; }
tab="$(printf '\t')"
exp1="\"Acme sync\"${tab}\"genp\"${tab}0x32303236313131383031333530305A00  \"20261118013500Z\\000\"${tab}"
[ "$(line 1)" = "$exp1" ] && ok "a password: labl, class and mdat; no hash" || bad "a password: [$(line 1)]"
case "$(line 2)" in "\"example.com\"${tab}\"inet\"${tab}0x3230"*) ok "labl NULL: the print name is the label" ;; *) bad "labl NULL: [$(line 2)]" ;; esac
case "$(line 3)" in "\"Globex Root\"${tab}0x80001000${tab}${tab}0x1A2B3C4D5E6F"*) ok "a certificate: no mdat; its hpky is its identity" ;; *) bad "a certificate: [$(line 3)]" ;; esac
case "$(line 4)" in "\"Acme key\"${tab}0x00000010${tab}${tab}0xAABBCCDDEEFF"*) ok "a key: print name from 0x00000001; its 0x00000006 hash is its identity" ;; *) bad "a key: [$(line 4)]" ;; esac

# What the watch catches: a changed mdat, a key swapped for another with
# the same label, an item gone, an item added.
sorted() { /usr/bin/awk -f "$AWK" < "$1" | LC_ALL=C sort; }
sorted "$T/dump" > "$T/before"
check_differs() {  # name, sed expression applied to the dump
    sed "$2" "$T/dump" > "$T/after.dump"
    sorted "$T/after.dump" > "$T/after"
    if cmp -s "$T/before" "$T/after"; then bad "$1 is caught"; else ok "$1 is caught"; fi
}
check_differs "a password's mdat changing" 's/20261118013500Z/20261118020000Z/'
check_differs "a key swapped for another with the same label" 's/0xAABBCCDDEEFF/0xFFFFCCDDEEFF/'
check_differs "a certificate swapped for another with the same label" 's/0x1A2B3C4D5E6F/0x9A2B3C4D5E6F/'
# An item gone: drop the last item (the key) from the dump.
awk '/^keychain: /{k++} k<4' "$T/dump" > "$T/after.dump"
sorted "$T/after.dump" > "$T/after"
cmp -s "$T/before" "$T/after" && bad "an item gone is caught" || ok "an item gone is caught"
# An item added: the dump twice holds every item twice.
cat "$T/dump" "$T/dump" > "$T/after.dump"
sorted "$T/after.dump" > "$T/after"
cmp -s "$T/before" "$T/after" && bad "an item added (the count) is caught" || ok "an item added (the count) is caught"
# And the same dump again is unchanged.
sorted "$T/dump" > "$T/after"
cmp -s "$T/before" "$T/after" && ok "the same items are unchanged" || bad "the same items are unchanged"
# Nothing secret: the awk never reads a -d data line, and its output holds
# only these four fields. A dump made with -d adds 'data:' lines; they are
# never printed.
{ cat "$T/dump"; printf 'data:\n"s3cret-value"\n'; } > "$T/withdata"
/usr/bin/awk -f "$AWK" < "$T/withdata" | grep -q 's3cret-value' && bad "data lines are never printed" || ok "data lines are never printed"

echo "k2sign-kc-items: $pass passed, $fail failed"
[ "$fail" = 0 ]
