#!/usr/bin/env bash
# sign-updater.sh — the ONLY place a release reads the update-signing key
# (prd-security-sweep-0464-v1 SS1/SS2, A7). Sourced by release.sh,
# scripts/sign-daemon.sh and scripts/windows-patch-latest-json.sh. Runs on
# z3mbpZ only. Sets no shell options.
#
#   k2u_env_guard                         die if TAURI_SIGNING_* is in the env
#   k2u_preflight <version>               key file, keychain item, minisign,
#                                         and a self-test signature
#   k2u_sign u1 <file> <asset> <version> <out> raw|b64
#
# How the secret is handled:
#   - The key file is read only inside k2u_sign, through a process
#     substitution (a /dev/fd pipe, never a temp file, never a variable).
#   - The password goes from `security … -w` straight into minisign's stdin
#     (a pipe). It is never in a shell variable, never exported, never on
#     argv (`ps` shows only paths and the keychain service name).
#   - Signing is plain `minisign -S` (no npm package resolved at signing
#     time). U1 is today's key E9111446F65EBB05, a Tauri-format key: base64
#     of a standard minisign secret key, which minisign reads once decoded.
#     The output is the same format the tauri signer wrote: prehashed (ED),
#     untrusted comment "signature from tauri secret key", trusted comment
#     "timestamp:<unix>\tfile:<asset>". `raw` writes the minisig text (the
#     daemon .sig); `b64` writes it base64-wrapped on one line (the Tauri
#     app updater's `signature` in latest.json and the .app.tar.gz.sig).
#   - Every signature is verified with the public key from the SOURCE tree
#     (src-tauri/tauri.conf.json) before it is kept, and is logged to
#     $K2_RELEASE_AUDIT_DIR/<version>/signatures.tsv.
#
# TODO(vault, Rosson 2026-10-24): the key and its password belong in
# z3mbpZ's K2 vault, handed to this helper only for the sign step. The vault
# can't do that cleanly yet (see k2u_password): its file/env-file delivery
# is for agent sessions only, a person caller must `reveal` with a per-use
# Touch ID confirm (blocks an unattended release), env-file refuses
# multi-line values, and macOS has no memory-backed folder for secret files.
# Until a vault verb exists, the 0.46.4 fallback is the login keychain, the
# same as the root-helper key. Swapping the source is k2u_password and
# k2u_key_text only.
#
# ── One-time setup on z3mbpZ (Rosson, GUI terminal, before 0.46.4) ─────────
#   mkdir -p ~/.k2-updater && chmod 700 ~/.k2-updater
#   cp ~/.tauri/k2-updater.key ~/.k2-updater/k2-updater-u1.key
#   chmod 600 ~/.k2-updater/k2-updater-u1.key
#   security add-generic-password -a "$USER" -s k2-updater-signing-u1 -w
#     (prompts for the key's password; never type it on the command line)
#   Then delete the TAURI_SIGNING_PRIVATE_KEY_PASSWORD line from the release
#   checkout's .env (release.sh refuses to run while it is there), and run
#   `bash scripts/sign-daemon.sh --preflight` to prove the setup.
#   After the first green release: rm ~/.tauri/k2-updater.key ~/.tauri/k2so-updater.key
#
# Settings (env):
#   K2_UPDATER_U1_KEY       default ~/.k2-updater/k2-updater-u1.key
#   K2_UPDATER_U1_KEYCHAIN  default k2-updater-signing-u1 (keychain service)
#   K2_RELEASE_AUDIT_DIR    default ~/k2-release-audit
#   K2_UPDATER_SRC_ROOT     the source tree public keys are read from
#                           (default: this checkout; tests point it at a
#                           fixture tree)

K2U_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K2U_SRC_ROOT="${K2_UPDATER_SRC_ROOT:-$(cd "$K2U_LIB_DIR/../.." && pwd)}"

k2u_log() { printf '[sign-updater] %s\n' "$*" >&2; }
k2u_fail() { printf '[sign-updater] ERROR: %s\n' "$*" >&2; return 1; }

# The signing key must never reach a build. release.sh calls this right
# after it sources .env, before any build step.
k2u_env_guard() {
    local bad="" n
    for n in TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD; do
        if [ -n "${!n:-}" ]; then bad="$bad $n"; fi
    done
    [ -z "$bad" ] && return 0
    k2u_fail "set in this environment:$bad. Release signing reads the key file and the keychain itself (scripts/lib/sign-updater.sh); an exported key reaches every build script and package in the run. Delete the line(s) from .env (and your shell profile), then run again."
}

k2u_key_path() {
    case "$1" in
        u1) printf '%s' "${K2_UPDATER_U1_KEY:-$HOME/.k2-updater/k2-updater-u1.key}" ;;
        *) return 1 ;;
    esac
}

k2u_keychain_service() {
    case "$1" in
        u1) printf '%s' "${K2_UPDATER_U1_KEYCHAIN:-k2-updater-signing-u1}" ;;
        *) return 1 ;;
    esac
}

# The public key (the minisign base64 line) a slot's signatures must verify
# with, from the source tree, so a signature is only kept when what
# installs trust accepts it.
k2u_pubkey() {
    case "$1" in
        u1)
            python3 -I - "$K2U_SRC_ROOT/src-tauri/tauri.conf.json" <<'PY'
import base64, json, sys
pk = json.load(open(sys.argv[1]))["plugins"]["updater"]["pubkey"]
lines = base64.b64decode(pk).decode().splitlines()
assert lines[0].startswith("untrusted comment:"), "pubkey is not a minisign public key"
print(lines[1].strip())
PY
            ;;
        *) return 1 ;;
    esac
}

# Key id (16 hex, as minisign prints it) of a base64 key or signature line.
k2u_key_id() {
    python3 -I -c 'import base64,sys; b=base64.b64decode(sys.argv[1]); print(b[2:10][::-1].hex().upper())' "$1"
}

# The secret key as plain minisign text on stdout (decodes a Tauri-format
# key). Only ever read through a pipe by k2u_sign.
k2u_key_text() {
    python3 -I - "$1" <<'PY'
import base64, sys
raw = open(sys.argv[1], "rb").read()
if not raw.startswith(b"untrusted comment:"):
    raw = base64.b64decode(b"".join(raw.split()))
if not raw.startswith(b"untrusted comment:"):
    sys.exit("not a minisign or Tauri secret key")
sys.stdout.buffer.write(raw)
PY
}

# The key's password on stdout, for minisign's stdin only.
# TODO(vault): replace with the vault verb once it exists (header).
k2u_password() {
    security find-generic-password -s "$1" -w
}

k2u_mode() { stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"; }

# Append one line per signature: utc time, asset, sha256, key id, trusted
# comment (tabs written as \t), user.
k2u_audit() {
    local version="$1" asset="$2" file="$3" keyid="$4" comment="$5" dir f sha
    dir="${K2_RELEASE_AUDIT_DIR:-$HOME/k2-release-audit}/$version"
    mkdir -p "$dir" || return 1
    f="$dir/signatures.tsv"
    [ -s "$f" ] || printf 'time\tasset\tsha256\tkey_id\ttrusted_comment\tuser\n' > "$f"
    sha="$(python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$file")" || return 1
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$asset" "$sha" "$keyid" \
        "${comment//$'\t'/\\t}" "${USER:-$(id -un)}" >> "$f"
}

# k2u_sign <slot> <file> <asset> <version> <out> raw|b64
k2u_sign() {
    local slot="$1" file="$2" asset="$3" version="$4" out="$5" form="$6"
    local key svc pub want_id tc uc work got algo id
    case "$form" in raw|b64) ;; *) k2u_fail "k2u_sign: form must be raw or b64 (got '$form')"; return 1 ;; esac
    [ -f "$file" ] || { k2u_fail "k2u_sign: no file $file"; return 1; }
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { k2u_fail "k2u_sign: bad version '$version'"; return 1; }
    [[ "$asset" =~ ^[A-Za-z0-9._-]+$ ]] || { k2u_fail "k2u_sign: bad asset name '$asset'"; return 1; }
    key="$(k2u_key_path "$slot")" && svc="$(k2u_keychain_service "$slot")" \
        || { k2u_fail "k2u_sign: unknown key slot '$slot'"; return 1; }
    [ -f "$key" ] || { k2u_fail "secret key missing: $key (one-time setup in scripts/lib/sign-updater.sh)"; return 1; }
    pub="$(k2u_pubkey "$slot")" || { k2u_fail "no $slot public key in the source tree"; return 1; }
    want_id="$(k2u_key_id "$pub")" || return 1
    case "$slot" in
        u1)
            tc="timestamp:$(date +%s)"$'\t'"file:$asset"
            uc="signature from tauri secret key"
            ;;
    esac
    work="$(mktemp -d "${TMPDIR:-/tmp}/k2u-sign.XXXXXX")" || return 1
    # Subshell: pipefail and the pipe stay local; nothing is exported.
    if ! (
        set -o pipefail
        k2u_password "$svc" \
            | minisign -S -s <(k2u_key_text "$key") -m "$file" -x "$work/sig" -c "$uc" -t "$tc" \
                >"$work/out" 2>"$work/err"
    ); then
        k2u_fail "$asset: minisign could not sign with the $slot key ($(tr '\n' ' ' < "$work/err" | head -c 300))"
        rm -rf "$work"; return 1
    fi
    got="$(minisign -V -P "$pub" -m "$file" -x "$work/sig" -Q 2>"$work/err")" \
        || { k2u_fail "$asset: the new $slot signature does not verify with the source tree's key $want_id"; rm -rf "$work"; return 1; }
    [ "$(printf '%s\n' "$got" | tail -n1)" = "$tc" ] \
        || { k2u_fail "$asset: trusted comment round-trip mismatch: $got"; rm -rf "$work"; return 1; }
    algo="$(sed -n 2p "$work/sig" | python3 -I -c 'import base64,sys; print(base64.b64decode(sys.stdin.read().strip())[:2].decode("latin-1"))')"
    [ "$algo" = "ED" ] || { k2u_fail "$asset: signature is not prehashed (algorithm '$algo'); installs expect ED"; rm -rf "$work"; return 1; }
    id="$(k2u_key_id "$(sed -n 2p "$work/sig")")"
    [ "$id" = "$want_id" ] || { k2u_fail "$asset: signed by key $id, expected $want_id"; rm -rf "$work"; return 1; }
    if [ "$form" = "b64" ]; then
        python3 -I -c 'import base64,sys; sys.stdout.write(base64.b64encode(open(sys.argv[1],"rb").read()).decode())' "$work/sig" > "$out.tmp" \
            && mv "$out.tmp" "$out"
    else
        mv "$work/sig" "$out"
    fi || { k2u_fail "$asset: could not write $out"; rm -rf "$work"; return 1; }
    rm -rf "$work"
    k2u_audit "$version" "$asset" "$file" "$id" "$tc" || { k2u_fail "$asset: audit log write failed"; return 1; }
    k2u_log "$asset signed with $slot ($id), $form → $out"
}

k2u_preflight_slot() {
    local slot="$1" version="$2" key svc dir v tmp
    key="$(k2u_key_path "$slot")"; svc="$(k2u_keychain_service "$slot")"
    [ -f "$key" ] || { k2u_fail "$slot secret key missing: $key (one-time setup in scripts/lib/sign-updater.sh)"; return 1; }
    dir="$(dirname "$key")"
    [ "$(k2u_mode "$key")" = "600" ] || { k2u_fail "$key must be mode 600 (chmod 600 '$key')"; return 1; }
    [ "$(k2u_mode "$dir")" = "700" ] || { k2u_fail "$dir must be mode 700 (chmod 700 '$dir')"; return 1; }
    security find-generic-password -s "$svc" >/dev/null 2>&1 \
        || { k2u_fail "no keychain item for service '$svc' (security add-generic-password -a \"\$USER\" -s $svc -w, from z3mbpZ's GUI session)"; return 1; }
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/k2u-pre.XXXXXX")" || return 1
    printf 'K2 updater signing preflight %s\n' "$version" > "$tmp/k2-preflight-self-test"
    if ! k2u_sign "$slot" "$tmp/k2-preflight-self-test" k2-preflight-self-test "$version" "$tmp/self.sig" raw; then
        rm -rf "$tmp"; k2u_fail "$slot self-test signature failed (wrong password in '$svc', or the key is not the one the source tree trusts)"; return 1
    fi
    rm -rf "$tmp"
    k2u_log "preflight ok: $slot key $(k2u_key_id "$(k2u_pubkey "$slot")") signs and verifies"
}

k2u_preflight() {
    local version="${1:-}" v
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { k2u_fail "k2u_preflight <x.y.z>"; return 1; }
    k2u_env_guard || return 1
    command -v minisign >/dev/null 2>&1 || { k2u_fail "minisign is not installed (brew install minisign)"; return 1; }
    v="$(minisign -v 2>&1 | sed -n 's/^minisign \([0-9][0-9.]*\).*/\1/p' | head -1)"
    python3 -I -c 'import sys; p=[int(x) for x in sys.argv[1].split(".")]; sys.exit(0 if p >= [0,11] else 1)' "${v:-0}" \
        || { k2u_fail "minisign ${v:-?} is too old; 0.11 or newer signs prehashed by default (brew upgrade minisign)"; return 1; }
    k2u_preflight_slot u1 "$version" || return 1
}
