#!/usr/bin/env bash
# updater-key-ceremony.sh — the update-key rotation ceremony
# (prd-security-sweep-0464-v1 SS5; runbook: docs/release/updater-key-ceremony.md).
#
#   bash scripts/updater-key-ceremony.sh generate
#       Rosson, ONCE, on z3mbpZ, in a GUI terminal (iTerm/Terminal, not SSH).
#       Makes U2 (the new primary: secret on z3mbpZ, password in the login
#       keychain) and U3 (the offline backup: into your password manager,
#       then deleted from disk). Prints the two PUBLIC key lines.
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

generate() {
    [ "$(uname -s)" = Darwin ] || die "run this on z3mbpZ (macOS), in its GUI session"
    [ -t 0 ] && [ -t 1 ] || die "run this in a terminal (passwords are typed at prompts, never passed in)"
    command -v minisign >/dev/null 2>&1 || die "minisign is not installed (brew install minisign)"
    command -v security >/dev/null 2>&1 || die "no security tool"
    local dir="${K2_UPDATER_KEY_DIR:-$HOME/.k2-updater}" svc="k2-updater-signing-u2" b
    [ ! -e "$dir/k2-updater-u2.key" ] || die "$dir/k2-updater-u2.key already exists; this ceremony runs once (rotation: docs/release/updater-key-ceremony.md)"
    if security find-generic-password -s "$svc" >/dev/null 2>&1; then
        die "the keychain already has an item '$svc'; remove it first if this is a deliberate redo"
    fi
    mkdir -p "$dir"; chmod 700 "$dir"

    say ""
    say "── 1/3  U2, the new primary update key (stays on this Mac) ──"
    say "minisign asks for a NEW password twice. Use a long random one from your password manager."
    minisign -G -p "$dir/k2-updater-u2.pub" -s "$dir/k2-updater-u2.key"
    chmod 600 "$dir/k2-updater-u2.key" "$dir/k2-updater-u2.pub"
    say ""
    say "Now the keychain asks for the SAME password once more (stored as '$svc'):"
    security add-generic-password -a "$USER" -s "$svc" -w
    b="$(mktemp -d "${TMPDIR:-/tmp}/k2-ceremony.XXXXXX")"; chmod 700 "$b"
    trap 'rm -rf "$b"' EXIT
    printf 'K2 update key ceremony self-test\n' > "$b/t"
    security find-generic-password -s "$svc" -w | minisign -S -s "$dir/k2-updater-u2.key" -m "$b/t" -x "$b/t.sig" -t "ceremony self-test" >/dev/null \
        || die "the keychain password does not open U2's key (typo?). Delete '$svc' and $dir/k2-updater-u2.* and start again."
    minisign -V -p "$dir/k2-updater-u2.pub" -m "$b/t" -x "$b/t.sig" -q || die "U2 self-test signature does not verify"
    say "U2 ok: the keychain password opens the key, and its signature verifies."

    say ""
    say "── 2/3  U3, the offline backup key (goes to your password manager, NOT this Mac) ──"
    say "minisign asks for a NEW password twice (different from U2's). Save it in the password manager."
    minisign -G -p "$b/u3.pub" -s "$b/u3.key"
    pbcopy < "$b/u3.key"
    say ""
    say "U3's secret key file is on the clipboard. Create a password-manager entry"
    say "'K2 update key U3 (offline backup)' and paste it in, with U3's password."
    read -r -p "Press Enter once it is saved (the clipboard is cleared next)... " _
    printf '' | pbcopy
    cp "$b/u3.pub" "$dir/k2-updater-u3.pub"; chmod 600 "$dir/k2-updater-u3.pub"
    rm -f "$b/u3.key"
    say "U3's secret key is deleted from this Mac; its public key is kept at $dir/k2-updater-u3.pub."

    say ""
    say "── 3/3  Send K2 these two PUBLIC key lines (safe to share) ──"
    say "U2 (primary): $(sed -n 2p "$dir/k2-updater-u2.pub")   id $(sed -n 1p "$dir/k2-updater-u2.pub" | sed 's/.*: //')"
    say "U3 (backup):  $(sed -n 2p "$dir/k2-updater-u3.pub")   id $(sed -n 1p "$dir/k2-updater-u3.pub" | sed 's/.*: //')"
    say ""
    say "K2 runs: bash scripts/updater-key-ceremony.sh pubkeys <U2 line> <U3 line>"
    say "and lands it on main. Then, here: bash scripts/sign-daemon.sh --preflight <next version>"
}

case "${1:-}" in
    generate) shift; generate "$@" ;;
    pubkeys) shift; pubkeys "$@" ;;
    *) die "usage: updater-key-ceremony.sh generate | pubkeys [--root DIR] <U2-line> <U3-line>" ;;
esac
