# kc-items.awk: one line per keychain item, from `security dump-keychain`
# run WITHOUT -d (attributes only; this never sees a secret).
#
# Out: label <TAB> class <TAB> mdat <TAB> hash, one line per item, for the
# caller to sort and compare (so the count is compared too).
# - label: the item's "labl", else its print name (0x00000007 for
#   passwords, 0x00000001 for keys).
# - mdat: the item's own modification date (passwords and the like).
# - hash: for items with no mdat, their identity: a key's label attribute
#   (0x00000006, the public key hash) or a certificate's "hpky" (its public
#   key hash). Keys and certificates have no mdat, so label plus hash is
#   what tells one from another.
#
# Used by run-suites-isolated.sh to watch the live login keychain by its
# items, not by the file's mtime. Tested by tests/release/k2sign-kc-items.sh.

function val(line) { return substr(line, index(line, "=") + 1) }
function usable(v) { return v != "" && v != "<NULL>" }
function emit() {
    lab = usable(labl) ? labl : (usable(l7) ? l7 : l1)
    hash = usable(hpky) ? hpky : (usable(l6) ? l6 : "")
    printf "%s\t%s\t%s\t%s\n", lab, cls, mdat, hash
}
/^keychain: / {
    if (n) emit()
    n = 1; cls = ""; labl = ""; l7 = ""; l6 = ""; l1 = ""; mdat = ""; hpky = ""
    next
}
/^class: /                   { cls = substr($0, 8); next }
/^ +"labl"<blob>=/           { labl = val($0); next }
/^ +"hpky"<blob>=/           { hpky = val($0); next }
/^ +0x00000007 <blob>=/      { l7 = val($0); next }
/^ +0x00000006 <blob>=/      { l6 = val($0); next }
/^ +0x00000001 <blob>=/      { l1 = val($0); next }
/^ +"mdat"<timedate>=/       { mdat = val($0); next }
END { if (n) emit() }
