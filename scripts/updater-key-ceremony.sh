#!/usr/bin/env bash
# updater-key-ceremony.sh — the update-key rotation ceremony
# (prd-security-sweep-0464-v1 SS5; runbook: docs/release/updater-key-ceremony.md).
#
#   bash scripts/updater-key-ceremony.sh generate
#       Rosson, ONCE, on z3mbpZ, in a GUI terminal (iTerm/Terminal, not SSH).
#       Makes U2 (the new primary: secret on z3mbpZ, password in the login
#       keychain) and U3 (the offline backup: its encrypted key FILE goes into
#       one password-manager entry, its password into a SEPARATE entry, then
#       the file is overwritten and deleted; never the clipboard). Stopping
#       part-way removes everything the run made. Prints the two PUBLIC key
#       lines.
#
#   bash scripts/updater-key-ceremony.sh drill <U3 key file> [--root DIR]
#       The U3 restore drill: sign a throwaway file with U3 restored from the
#       password manager, check it against UPDATER_KEYS' backup key.
#
#   bash scripts/updater-key-ceremony.sh pubkeys [--root DIR] <U2-line> <U3-line>
#       K2 (a builder), in the repo: writes the two public keys into every
#       verifier copy: crates/k2-release-keys/src/lib.rs (UPDATER_KEYS),
#       cli/k2 (_DAEMON_UPDATE_PUBKEYS), scripts/install-daemon.sh
#       (K2_DAEMON_PUBKEYS) and src-tauri/tauri.conf.json (the primary).
#       The k2-release-keys `copies` test then checks they agree.
#
# Secrets never go on argv or into the environment: minisign and `security`
# prompt for passwords themselves. Nothing here touches U1 (today's key).
#
# Where the keys live (Q4): the K2 vault was the target, but it can't hand a
# person's release run a secret for one step today (file/env-file delivery
# is for agent sessions; a person must `reveal` with a per-use Touch ID
# confirm; env-file refuses multi-line values; macOS has no memory-backed
# secrets folder). Until a vault verb exists, U2 follows the root-helper
# key: an encrypted key file in ~/.k2-updater (0600, dir 0700) and its
# password in the login keychain. TODO(vault): move both when the verb lands
# (scripts/lib/sign-updater.sh, k2u_password / k2u_key_text).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
U1_LINE="RWQFu172RhQR6QBqzmij2E9btDGhDWmpdH+ah1/M4PmuHPINUWvKLf6o"

die() { printf 'updater-key-ceremony: ERROR: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

pubkeys() {
    local root="$ROOT"
    if [ "${1:-}" = "--root" ]; then root="${2:?--root DIR}"; shift 2; fi
    [ $# -eq 2 ] || die "usage: updater-key-ceremony.sh pubkeys [--root DIR] <U2 public key line> <U3 public key line>"
    python3 -I - "$root" "$1" "$2" "$U1_LINE" <<'PY'
import base64, json, re, sys
root, primary, backup, u1 = sys.argv[1:5]

def key_id(line):
    try:
        raw = base64.b64decode(line, validate=True)
    except Exception:
        sys.exit("not a minisign public key line: %r" % line)
    if len(raw) != 42 or raw[:2] != b"Ed":
        sys.exit("not a minisign public key line: %r" % line)
    return raw[2:10][::-1].hex().upper()

ids = [key_id(primary), key_id(backup)]
if primary == backup or ids[0] == ids[1]:
    sys.exit("U2 and U3 must be two different keys")
if u1 in (primary, backup):
    sys.exit("that is U1, the retired key; it must never be a trusted update key")

def edit(rel, fn):
    p = "%s/%s" % (root, rel)
    s = open(p).read()
    t = fn(s)
    if t == s:
        sys.exit("no change made to %s (already done?)" % rel)
    open(p, "w").write(t)

def rust(s):
    a, b = s.index("// BEGIN UPDATER_KEYS"), s.index("// END UPDATER_KEYS")
    body = '''// BEGIN UPDATER_KEYS (written by scripts/updater-key-ceremony.sh pubkeys)
/// The update keys: U2 (primary, secret on z3mbpZ) then U3 (offline backup,
/// in Rosson's password manager), from the SS5 key ceremony.
pub const UPDATER_KEYS: &[TrustedKey] = &[
    TrustedKey {
        role: "primary",
        key_id: "%s",
        public_key_b64: "%s",
    },
    TrustedKey {
        role: "backup",
        key_id: "%s",
        public_key_b64: "%s",
    },
];
''' % (ids[0], primary, ids[1], backup)
    return s[:a] + body + s[b:]

def shell(var):
    def f(s):
        pat = re.compile(r'^%s="[^"]*"$' % re.escape(var), re.M)
        if len(pat.findall(s)) != 1:
            sys.exit("%s must be assigned exactly once" % var)
        return pat.sub('%s="%s %s"' % (var, primary, backup), s)
    return f

def conf(s):
    wrapped = base64.b64encode(("untrusted comment: minisign public key: %s\n%s\n" % (ids[0], primary)).encode()).decode()
    pat = re.compile(r'("pubkey":\s*")[^"]*(")')
    if len(pat.findall(s)) != 1:
        sys.exit("tauri.conf.json must have exactly one pubkey")
    t = pat.sub(lambda m: m.group(1) + wrapped + m.group(2), s)
    json.loads(t)
    return t

edit("crates/k2-release-keys/src/lib.rs", rust)
edit("cli/k2", shell("_DAEMON_UPDATE_PUBKEYS"))
edit("scripts/install-daemon.sh", shell("K2_DAEMON_PUBKEYS"))
edit("src-tauri/tauri.conf.json", conf)
print("wrote U2 %s (primary) and U3 %s (backup) into the four verifier copies under %s" % (ids[0], ids[1], root))
PY
}

# Overwrite a file with random bytes, then delete it. On an SSD/APFS this is
# best effort (the old blocks may survive in the flash translation layer);
# the U3 file is encrypted with U3's password either way.
shred_file() {
    [ -f "$1" ] || return 0
    python3 -I -c 'import os,sys; p=sys.argv[1]; n=os.path.getsize(p); f=open(p,"r+b"); f.write(os.urandom(n)); f.flush(); os.fsync(f.fileno()); f.close()' "$1"
    rm -f "$1"
}

# Abort (Ctrl-C, a failed step): remove what this run made so a redo starts
# clean: the U3 folder (shredded), and U2's files and keychain item if this
# run created them. A finished run removes only the U3 folder.
CEREMONY_DONE=0 U3DIR="" SELFTEST="" MADE_U2="" MADE_KC="" CEREMONY_DIR="" CEREMONY_SVC=""
ceremony_cleanup() {
    local rc=$?
    if [ -n "$U3DIR" ] && [ -d "$U3DIR" ]; then
        shred_file "$U3DIR/k2-updater-u3.key"
        rm -rf "$U3DIR"
    fi
    [ -z "$SELFTEST" ] || rm -rf "$SELFTEST"
    if [ "$CEREMONY_DONE" != 1 ]; then
        if [ -n "$MADE_U2" ]; then
            shred_file "$CEREMONY_DIR/k2-updater-u2.key"
            rm -f "$CEREMONY_DIR/k2-updater-u2.pub" "$CEREMONY_DIR/k2-updater-u3.pub"
        fi
        if [ -n "$MADE_KC" ]; then
            security delete-generic-password -s "$CEREMONY_SVC" >/dev/null 2>&1 || true
        fi
        printf '\nupdater-key-ceremony: stopped before the end; removed everything this run made (U3 folder%s%s). Run it again from the start.\n' \
            "${MADE_U2:+, U2 key files}" "${MADE_KC:+, keychain item $CEREMONY_SVC}" >&2
    fi
    exit "$rc"
}

generate() {
    [ "$(uname -s)" = Darwin ] || die "run this on z3mbpZ (macOS), in its GUI session"
    [ -t 0 ] && [ -t 1 ] || die "run this in a terminal (passwords are typed at prompts, never passed in)"
    command -v minisign >/dev/null 2>&1 || die "minisign is not installed (brew install minisign)"
    command -v security >/dev/null 2>&1 || die "no security tool"
    local dir="${K2_UPDATER_KEY_DIR:-$HOME/.k2-updater}" svc="k2-updater-signing-u2" ans
    [ ! -e "$dir/k2-updater-u2.key" ] || die "$dir/k2-updater-u2.key already exists; this ceremony runs once (rotation: docs/release/updater-key-ceremony.md)"
    if security find-generic-password -s "$svc" >/dev/null 2>&1; then
        die "the keychain already has an item '$svc'; remove it first if this is a deliberate redo"
    fi
    CEREMONY_DIR="$dir" CEREMONY_SVC="$svc"
    trap ceremony_cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    mkdir -p "$dir"; chmod 700 "$dir"

    say ""
    say "── 1/3  U2, the new primary update key (stays on this Mac) ──"
    say "minisign asks for a NEW password twice. Use a long random one from your password manager."
    MADE_U2=1
    minisign -G -p "$dir/k2-updater-u2.pub" -s "$dir/k2-updater-u2.key"
    chmod 600 "$dir/k2-updater-u2.key" "$dir/k2-updater-u2.pub"
    say ""
    say "Now the keychain asks for the SAME password once more (stored as '$svc'):"
    MADE_KC=1
    security add-generic-password -a "$USER" -s "$svc" -w
    SELFTEST="$(mktemp -d "${TMPDIR:-/tmp}/k2-ceremony.XXXXXX")"; chmod 700 "$SELFTEST"
    printf 'K2 update key ceremony self-test\n' > "$SELFTEST/t"
    security find-generic-password -s "$svc" -w | minisign -S -s "$dir/k2-updater-u2.key" -m "$SELFTEST/t" -x "$SELFTEST/t.sig" -t "ceremony self-test" >/dev/null \
        || die "the keychain password does not open U2's key (typo?)"
    minisign -V -p "$dir/k2-updater-u2.pub" -m "$SELFTEST/t" -x "$SELFTEST/t.sig" -q || die "U2 self-test signature does not verify"
    say "U2 ok: the keychain password opens the key, and its signature verifies."

    say ""
    say "── 2/3  U3, the offline backup key (goes to your password manager, NOT this Mac) ──"
    U3DIR="$(mktemp -d "$HOME/K2-U3-backup.XXXXXX")"; chmod 700 "$U3DIR"
    say "minisign asks for a NEW password twice (different from U2's)."
    minisign -G -p "$U3DIR/k2-updater-u3.pub" -s "$U3DIR/k2-updater-u3.key"
    chmod 600 "$U3DIR/k2-updater-u3.key"
    say ""
    say "U3's secret key is in $U3DIR/k2-updater-u3.key (still encrypted with U3's password)."
    say "  1. In your password manager, make an entry 'K2 update key U3 - key file' and"
    say "     ATTACH that file to it (drag it from the Finder window that opens now)."
    say "  2. Make a SEPARATE entry 'K2 update key U3 - password' holding U3's password."
    say "     Never put the password in the same entry as the file."
    open -R "$U3DIR/k2-updater-u3.key" 2>/dev/null || true
    read -r -p "Type 'saved' once BOTH entries exist (anything else stops and removes everything): " ans
    [ "$ans" = "saved" ] || die "not saved"
    cp "$U3DIR/k2-updater-u3.pub" "$dir/k2-updater-u3.pub"; chmod 600 "$dir/k2-updater-u3.pub"
    shred_file "$U3DIR/k2-updater-u3.key"
    rm -rf "$U3DIR"
    say "U3's secret key is overwritten and deleted from this Mac; its public key is kept at $dir/k2-updater-u3.pub."
    say "(Run the U3 drill from the runbook once, from the password manager's copy.)"

    CEREMONY_DONE=1
    say ""
    say "── 3/3  Send K2 these two PUBLIC key lines (safe to share) ──"
    say "U2 (primary): $(sed -n 2p "$dir/k2-updater-u2.pub")   id $(sed -n 1p "$dir/k2-updater-u2.pub" | sed 's/.*: //')"
    say "U3 (backup):  $(sed -n 2p "$dir/k2-updater-u3.pub")   id $(sed -n 1p "$dir/k2-updater-u3.pub" | sed 's/.*: //')"
    say ""
    say "K2 runs: bash scripts/updater-key-ceremony.sh pubkeys <U2 line> <U3 line>"
    say "and lands it with the SS4/SS5 code. Then, here: bash scripts/sign-daemon.sh --preflight <next version>"
}

# The U3 drill (runbook): restore U3's key file from the password manager
# to a temp path, sign a throwaway file with it, and check the signature
# with the BACKUP key in the source tree's UPDATER_KEYS, the way installs
# would. minisign asks for U3's password itself. Nothing is published; the
# signed file names `k2-u3-drill`, which no installer accepts.
drill() {
    local root="$ROOT" key="" line tc out
    while [ $# -gt 0 ]; do
        case "$1" in
            --root) root="${2:?--root DIR}"; shift 2 ;;
            *) [ -z "$key" ] || die "usage: updater-key-ceremony.sh drill <U3 secret key file> [--root DIR]"; key="$1"; shift ;;
        esac
    done
    [ -n "$key" ] && [ -f "$key" ] || die "usage: updater-key-ceremony.sh drill <U3 secret key file> [--root DIR]"
    command -v minisign >/dev/null 2>&1 || die "minisign is not installed (brew install minisign)"
    line="$(python3 -I - "$root/crates/k2-release-keys/src/lib.rs" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
block = src[src.index("// BEGIN UPDATER_KEYS"):src.index("// END UPDATER_KEYS")]
keys = re.findall(r'role: "backup",\s*key_id: "[0-9A-F]{16}",\s*public_key_b64: "([^"]+)"', block)
print(keys[0]) if keys else sys.exit("no backup key in UPDATER_KEYS: run `pubkeys` first")
PY
)" || die "the source tree has no U3 (backup) key yet"
    DRILL_TMP="$(mktemp -d "${TMPDIR:-/tmp}/k2-u3-drill.XXXXXX")"; chmod 700 "$DRILL_TMP"
    trap 'rm -rf "$DRILL_TMP"' EXIT
    local t="$DRILL_TMP"
    printf 'K2 U3 drill %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$t/drill"
    tc="timestamp:$(date +%s)"$'\t'"file:k2-u3-drill"$'\t'"version:0.0.0"
    minisign -S -s "$key" -m "$t/drill" -x "$t/drill.sig" -t "$tc" >/dev/null || die "U3 did not sign (wrong password, or not U3's key file)"
    case "$(sed -n 2p "$t/drill.sig")" in RU[QRST]*) ;; *) die "the signature is not prehashed" ;; esac
    out="$(minisign -V -P "$line" -m "$t/drill" -x "$t/drill.sig" -Q 2>/dev/null)" \
        || die "this key is NOT the backup key the installs trust (UPDATER_KEYS backup)"
    [ "$(printf '%s\n' "$out" | tail -n1)" = "$tc" ] || die "trusted comment round-trip mismatch"
    say "U3 drill ok: the restored key signs, and the installs' backup key accepts it."
    say "Now delete the restored copy: $key"
}

case "${1:-}" in
    generate) shift; generate "$@" ;;
    pubkeys) shift; pubkeys "$@" ;;
    drill) shift; drill "$@" ;;
    *) die "usage: updater-key-ceremony.sh generate | pubkeys [--root DIR] <U2-line> <U3-line> | drill <U3 key file> [--root DIR]" ;;
esac
