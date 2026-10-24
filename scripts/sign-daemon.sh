#!/usr/bin/env bash
# sign-daemon.sh — sign a release's Linux k2-daemon builds on z3mbpZ and
# publish daemon-latest.json with them (prd-security-sweep-0464-v1 SS1).
# The update key never goes to GitHub: daemon-binaries.yml (or K2 Compute)
# builds the daemons UNSIGNED, and this script signs them here, the same
# way sign-root-helper.sh signs the root helper.
#
#   bash scripts/sign-daemon.sh --preflight [<version>]   # release.sh Step 0.45
#   bash scripts/sign-daemon.sh <version>                 # release.sh Step 10.5
#   bash scripts/sign-daemon.sh <version> --from-dir <dir>  # K2 Compute files
#   bash scripts/sign-daemon.sh <version> --dry --from-dir <dir> \
#        --manifest <daemon-latest.json> --out <dir>      # no GitHub at all
#
# For each arch (x86_64, aarch64):
#   1. get k2-daemon-linux-<arch> + .sha256: from --from-dir when it has
#      them (K2 Compute, after compute_release.py checked the receipts),
#      else wait for daemon-binaries.yml to upload them to the release;
#   2. check the sha256, then scripts/lib/check-daemon-asset.py (ELF of the
#      right arch, exactly one K2_DAEMON_BUILD marker for this version, no
#      test-keys, k2-ptyd embedded), then the OAuth bake check
#      (assert_daemon_oauth_not_placeholder);
#   3. sign with the update key (scripts/lib/sign-updater.sh: raw minisig
#      text, trusted comment `timestamp:…\tfile:k2-daemon-linux-<arch>`),
#      verified against the source tree's public key;
# then merge both into daemon-latest.json with scripts/merge-daemon-manifest.py
# (the same script and output CI's old merge job had, so installs see the
# same keys and fields) and upload the .sig files, THEN the manifest, so the
# manifest never lists a daemon whose .sig isn't up yet.
#
# What this does NOT fix: a poisoned build. A compromised runner or compute
# node can still build a bad daemon that passes these byte checks, and this
# script will sign it. SS1 stops the key from being copied off GitHub.
#
# Environment:
#   K2_RELEASE_REPO           default Alakazam-211/K2
#   K2_SIGN_WAIT_MIN          minutes to wait for CI assets (default 60)
#   K2_ALLOW_GH_SIGNING_SECRETS=1  preflight only warns while the old GitHub
#                             secrets TAURI_SIGNING_* still exist (loud)
#   K2_GMAIL_CLIENT_ID / _SECRET (+ K2_MICROSOFT_CLIENT_ID): the OAuth bake
#                             check; release.sh has them from .env. By hand,
#                             they are read from the checkout's .env (only
#                             these names).
#   plus the key settings in scripts/lib/sign-updater.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
WAIT_MIN="${K2_SIGN_WAIT_MIN:-60}"
ARCHES=(x86_64 aarch64)
# shellcheck source=lib/sign-updater.sh
source "$ROOT/scripts/lib/sign-updater.sh"

log() { printf '[sign-daemon] %s\n' "$*"; }
die() { printf '[sign-daemon] ERROR: %s\n' "$*" >&2; exit 1; }

sha256_of() { python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"; }

gh_secrets_check() {
    local names
    if ! names="$(gh secret list -R "$REPO" 2>/dev/null | awk '{print $1}')"; then
        log "WARNING: this gh token can't list $REPO's Actions secrets; check by hand that TAURI_SIGNING_PRIVATE_KEY(_PASSWORD) are deleted (Settings → Secrets and variables → Actions)."
        return 0
    fi
    if printf '%s\n' "$names" | grep -qx 'TAURI_SIGNING_PRIVATE_KEY\(_PASSWORD\)\{0,1\}'; then
        if [ "${K2_ALLOW_GH_SIGNING_SECRETS:-0}" = "1" ]; then
            log "WARNING ⚠⚠: $REPO still has the GitHub secrets TAURI_SIGNING_PRIVATE_KEY(_PASSWORD) (K2_ALLOW_GH_SIGNING_SECRETS=1). Nothing uses them since SS1; delete them: gh secret delete TAURI_SIGNING_PRIVATE_KEY -R $REPO; gh secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R $REPO"
            return 0
        fi
        die "$REPO still has the GitHub secrets TAURI_SIGNING_PRIVATE_KEY(_PASSWORD). Since SS1 only z3mbpZ signs; delete them (gh secret delete TAURI_SIGNING_PRIVATE_KEY -R $REPO; gh secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R $REPO), or set K2_ALLOW_GH_SIGNING_SECRETS=1 for this run."
    fi
    log "GitHub has no TAURI_SIGNING_* secrets."
}

preflight() {
    local version="$1"
    command -v gh >/dev/null 2>&1 || die "gh is not installed"
    k2u_preflight "$version" || die "update-key preflight failed (above)"
    python3 -I -c 'import sys' || die "python3 missing"
    gh_secrets_check
}

if [ "${1:-}" = "--preflight" ]; then
    v="${2:-0.0.0}"; v="${v#v}"
    preflight "$v"
    log "preflight ok"
    exit 0
fi

VERSION="${1:-}"
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: sign-daemon.sh <x.y.z> [--from-dir DIR] [--dry --manifest FILE --out DIR] | --preflight [<x.y.z>]"
shift
FROM_DIR="" MANIFEST="" OUT="" DRY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --from-dir) FROM_DIR="${2:?--from-dir DIR}"; shift 2 ;;
        --manifest) MANIFEST="${2:?--manifest FILE}"; shift 2 ;;
        --out) OUT="${2:?--out DIR}"; shift 2 ;;
        --dry) DRY=1; shift ;;
        *) die "unknown argument: $1" ;;
    esac
done
if [ "$DRY" = 1 ]; then
    [ -n "$FROM_DIR" ] && [ -n "$MANIFEST" ] && [ -n "$OUT" ] || die "--dry needs --from-dir, --manifest and --out (it never talks to GitHub)"
else
    command -v gh >/dev/null 2>&1 || die "gh is not installed"
fi
[ -z "$FROM_DIR" ] || [ -d "$FROM_DIR" ] || die "--from-dir $FROM_DIR is not a directory"
k2u_env_guard || exit 1

# The OAuth bake check needs the real values; take only these names from .env.
if [ -z "${K2_GMAIL_CLIENT_ID:-}" ] && [ -f "$ROOT/.env" ]; then
    eval "$(python3 -I - "$ROOT/.env" <<'PY'
import shlex, sys
for line in open(sys.argv[1]):
    k, sep, v = line.strip().partition("=")
    k = k.removeprefix("export ").strip()
    if sep and k in ("K2_GMAIL_CLIENT_ID", "K2_GMAIL_CLIENT_SECRET", "K2_MICROSOFT_CLIENT_ID"):
        v = v.strip()
        if len(v) >= 2 and v[0] == v[-1] and v[0] in "'\"":
            v = v[1:-1]
        print("%s=%s" % (k, shlex.quote(v)))
PY
)"
fi

TAG="v$VERSION"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-sign-daemon.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
ASSETS="$WORK/assets"
mkdir -p "$ASSETS"
[ -z "$OUT" ] || mkdir -p "$OUT"

wait_for_github() {
    local deadline=$(( $(date +%s) + WAIT_MIN * 60 )) names a missing
    while :; do
        names="$(gh release view "$TAG" -R "$REPO" --json assets -q '.assets[].name' 2>/dev/null || true)"
        missing=""
        for a in "$@"; do printf '%s\n' "$names" | grep -qxF "$a" || missing="$missing $a"; done
        [ -z "$missing" ] && return 0
        [ "$(date +%s)" -lt "$deadline" ] || die "not on $TAG within ${WAIT_MIN} min:$missing (daemon-binaries.yml)"
        log "waiting for daemon-binaries.yml to upload:$missing"
        sleep 30
    done
}

# 1. Fetch.
gh_needed=()
for ARCH in "${ARCHES[@]}"; do
    A="k2-daemon-linux-$ARCH"
    if [ -n "$FROM_DIR" ] && [ -f "$FROM_DIR/$A" ] && [ -f "$FROM_DIR/$A.sha256" ]; then
        cp "$FROM_DIR/$A" "$FROM_DIR/$A.sha256" "$ASSETS/"
        log "$A: from $FROM_DIR"
    elif [ "$DRY" = 1 ]; then
        die "$A (+ .sha256) not in $FROM_DIR"
    else
        gh_needed+=("$A" "$A.sha256")
        log "$A: from the GitHub release $TAG"
    fi
    # The App engines are listed in the manifest too (their .sig comes from
    # sign-app-engine.sh); merge-daemon-manifest.py hashes them.
    E="k2-app-engine-linux-$ARCH"
    if [ -n "$FROM_DIR" ] && [ -f "$FROM_DIR/$E" ]; then
        cp "$FROM_DIR/$E" "$ASSETS/"
    elif [ "$DRY" = 0 ]; then
        gh_needed+=("$E")
    fi
done
if [ "${#gh_needed[@]}" -gt 0 ]; then
    wait_for_github "${gh_needed[@]}"
    pats=()
    for a in "${gh_needed[@]}"; do pats+=(-p "$a"); done
    gh release download "$TAG" -R "$REPO" "${pats[@]}" -D "$ASSETS" --clobber
fi

# 2. Check, 3. sign.
for ARCH in "${ARCHES[@]}"; do
    A="k2-daemon-linux-$ARCH"
    want="$(awk '{print $1; exit}' "$ASSETS/$A.sha256")"
    got="$(sha256_of "$ASSETS/$A")"
    [ -n "$want" ] && [ "$want" = "$got" ] || die "$A: sha256 differs from its .sha256 ($got vs $want)"
    python3 -I "$ROOT/scripts/lib/check-daemon-asset.py" "$ASSETS/$A" "$ARCH" "$VERSION" || die "$A failed the byte checks (above) — not signing"
    ( # shellcheck source=require-mail-oauth-build-env.sh
      source "$ROOT/scripts/require-mail-oauth-build-env.sh"
      assert_daemon_oauth_not_placeholder "$ASSETS/$A" ) || die "$A: OAuth bake check failed — not signing a placeholder build"
    k2u_sign u1 "$ASSETS/$A" "$A" "$VERSION" "$ASSETS/$A.sig" raw || die "$A: signing failed"
done

# 4. Manifest: the released one (Step 9 published it with macos-aarch64),
# or --manifest.
if [ -n "$MANIFEST" ]; then
    cp "$MANIFEST" "$ASSETS/daemon-latest.json"
else
    gh release download "$TAG" -R "$REPO" -p daemon-latest.json -D "$ASSETS" --clobber
fi
python3 -I -c 'import json,sys; v=json.load(open(sys.argv[1])).get("version"); sys.exit(0 if v==sys.argv[2] else "daemon-latest.json is for %r, not %s" % (v, sys.argv[2]))' \
    "$ASSETS/daemon-latest.json" "$VERSION"
python3 -I "$ROOT/scripts/merge-daemon-manifest.py" "$ASSETS/daemon-latest.json" \
    "https://github.com/$REPO/releases/download/$TAG" "$ASSETS"

if [ "$DRY" = 1 ]; then
    for ARCH in "${ARCHES[@]}"; do cp "$ASSETS/k2-daemon-linux-$ARCH.sig" "$OUT/"; done
    cp "$ASSETS/daemon-latest.json" "$OUT/"
    log "dry run: signatures + manifest in $OUT (nothing uploaded)"
    exit 0
fi

# 5. Upload: signatures (and compute-built bytes) first, the manifest last.
up=()
for ARCH in "${ARCHES[@]}"; do
    A="k2-daemon-linux-$ARCH"
    up+=("$ASSETS/$A.sig")
    # Files that came from compute aren't on the release yet.
    if [ -n "$FROM_DIR" ] && [ -f "$FROM_DIR/$A" ]; then up+=("$ASSETS/$A" "$ASSETS/$A.sha256"); fi
done
gh release upload "$TAG" -R "$REPO" "${up[@]}" --clobber
gh release upload "$TAG" -R "$REPO" "$ASSETS/daemon-latest.json" --clobber
[ -z "$OUT" ] || cp "$ASSETS"/*.sig "$ASSETS/daemon-latest.json" "$OUT/"
log "done: $TAG Linux daemons signed; daemon-latest.json lists them"
