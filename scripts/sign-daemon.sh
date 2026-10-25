#!/usr/bin/env bash
# sign-daemon.sh — sign a release's Linux k2-daemon builds on z3mbpZ and
# publish daemon-latest.json with them (prd-security-sweep-0464-v1 SS1).
# The update key never goes to GitHub: daemon-binaries.yml (or K2 Compute)
# builds the daemons UNSIGNED, and this script signs them here, the same
# way sign-root-helper.sh signs the root helper.
#
#   bash scripts/sign-daemon.sh --preflight [<version>]   # release.sh Step 0.45
#   bash scripts/sign-daemon.sh <version>                 # release.sh Step 10.5
#   bash scripts/sign-daemon.sh <version> --from-dir <dir> --compute-verdict <state-dir>
#                                                         # K2 Compute files
#   bash scripts/sign-daemon.sh <version> --dry --from-dir <dir> --compute-verdict <state-dir> \
#        --manifest <daemon-latest.json> --out <dir>      # no GitHub at all
#
# For each arch (x86_64, aarch64):
#   1. get k2-daemon-linux-<arch> + .sha256: from --from-dir when it has
#      them (K2 Compute, after compute_release.py checked the receipts),
#      else wait for daemon-binaries.yml to upload them to the release;
#   1b. provenance (security review of SS1): a daemon is signed only when
#      we know where it came from.
#      GitHub builds: (a) `gh attestation verify` passes for this repo, the
#      daemon-binaries.yml workflow, the tag's ref and the tag's commit (the
#      workflow attests each daemon with actions/attest-build-provenance);
#      (b) the release asset and its .sha256 were uploaded exactly once, by
#      github-actions[bot], after the release was created (the workflow
#      uploads them without --clobber).
#      K2 Compute builds (--from-dir): --compute-verdict names the state dir
#      where compute_release.py wrote <label>.verdict.json after checking
#      each job's signed receipt; the daemon's sha256 must be in a verdict
#      with ok:true (and, off --dry, that verdict's commit is the tag's);
#   2. check the sha256, then scripts/lib/check-daemon-asset.py (ELF of the
#      right arch, exactly one K2_DAEMON_BUILD marker for this version, no
#      test-keys, k2-ptyd embedded), then the OAuth bake check
#      (assert_daemon_oauth_not_placeholder);
#   3. sign twice (scripts/lib/sign-updater.sh, each verified against the
#      source tree's keys): `.sig` with the retired key U1 (raw minisig,
#      `timestamp:…\tfile:k2-daemon-linux-<arch>`, for installs older than
#      0.46.4) and `.sig2` with U2 (`…\tversion:<v>`, the only one 0.46.4+
#      accepts; SS4/SS5);
# then merge both into daemon-latest.json with scripts/merge-daemon-manifest.py
# (the same script CI's old merge job ran: same keys and fields, plus
# `sig2`) and upload the .sig/.sig2 files, THEN the manifest, so the
# manifest never lists a daemon whose signatures aren't up yet. Finally it
# re-downloads the published manifest, each published daemon (its hash must
# be the one it signed) and the .sig2 files, and checks them the way a
# 0.46.4 install does.
#
# What this does NOT fix: a poisoned build. The attestation proves the bytes
# came out of daemon-binaries.yml at the tag's commit, not that the runner
# was clean: a compromised self-hosted runner can still build a bad daemon
# that passes every check here, and this script will sign it. SS1 stops the
# key from being copied off GitHub and stops a swapped release asset. The
# real fix is two independent builds (GitHub + K2 Compute) whose hashes
# must match, which needs reproducible builds: 0.46.5 work, owner K2 (plan
# G12; docs/release/updater-key-ceremony.md "Next: two independent builds").
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
FROM_DIR="" MANIFEST="" OUT="" DRY=0 VERDICTS=""
while [ $# -gt 0 ]; do
    case "$1" in
        --from-dir) FROM_DIR="${2:?--from-dir DIR}"; shift 2 ;;
        --compute-verdict) VERDICTS="${2:?--compute-verdict DIR}"; shift 2 ;;
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
if [ -n "$FROM_DIR" ]; then
    [ -n "$VERDICTS" ] && [ -d "$VERDICTS" ] || die "--from-dir needs --compute-verdict <state dir> (compute_release.py's receipt checks): no provenance, no signature"
fi
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

# Provenance.
TAG_COMMIT=""
if [ "$DRY" = 0 ]; then
    TAG_COMMIT="$(gh api "repos/$REPO/commits/$TAG" --jq .sha)" || die "can't resolve the commit of $TAG"
    [[ "$TAG_COMMIT" =~ ^[0-9a-f]{40}$ ]] || die "$TAG resolves to '$TAG_COMMIT', not a commit"
fi

check_github_provenance() { # <file> <asset>
    local f="$1" a="$2"
    if ! gh attestation verify "$f" -R "$REPO" \
        --signer-workflow "$REPO/.github/workflows/daemon-binaries.yml" \
        --source-ref "refs/tags/$TAG" --source-digest "$TAG_COMMIT" >"$WORK/attest-$a.out" 2>&1; then
        sed 's/^/    /' "$WORK/attest-$a.out" >&2
        die "$a: no valid build-provenance attestation from daemon-binaries.yml at $TAG ($TAG_COMMIT) — not signing"
    fi
    gh api "repos/$REPO/releases/tags/$TAG" > "$WORK/release.json" || die "can't read the $TAG release"
    python3 -I - "$WORK/release.json" "$a" <<'PY' || die "$a: the release asset's uploader or upload time is wrong — not signing"
import json, sys
rel, name = json.load(open(sys.argv[1])), sys.argv[2]
created = rel["created_at"]
for n in (name, name + ".sha256"):
    hits = [x for x in rel["assets"] if x["name"] == n]
    if len(hits) != 1:
        sys.exit("%s: %d assets with that name" % (n, len(hits)))
    x = hits[0]
    if (x.get("uploader") or {}).get("login") != "github-actions[bot]":
        sys.exit("%s: uploaded by %r, not github-actions[bot]" % (n, (x.get("uploader") or {}).get("login")))
    if x["created_at"] < created:
        sys.exit("%s: uploaded %s, before the release (%s)" % (n, x["created_at"], created))
PY
    log "$a: provenance ok (attested by daemon-binaries.yml at $TAG, uploaded by github-actions[bot])"
}

check_compute_provenance() { # <file> <asset>
    python3 -I - "$VERDICTS" "$2" "$(sha256_of "$1")" "$TAG_COMMIT" <<'PY' || die "$2: no ok compute verdict lists these bytes — not signing"
import glob, json, os, sys
d, name, sha, commit = sys.argv[1:5]
for p in sorted(glob.glob(os.path.join(d, "*.verdict.json"))):
    v = json.load(open(p))
    if v.get("ok") is not True or v.get("problems"):
        continue
    if commit and v.get("commit") != commit:
        continue
    for k, meta in (v.get("files") or {}).items():
        if k.rsplit("/", 1)[-1] == name and meta.get("sha256") == sha:
            print("%s: in %s (job %s)" % (name, os.path.basename(p), v.get("job")))
            sys.exit(0)
sys.exit(1)
PY
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
    if [ -n "$FROM_DIR" ] && [ -f "$FROM_DIR/$A" ]; then
        check_compute_provenance "$ASSETS/$A" "$A"
    else
        check_github_provenance "$ASSETS/$A" "$A"
    fi
    python3 -I "$ROOT/scripts/lib/check-daemon-asset.py" "$ASSETS/$A" "$ARCH" "$VERSION" || die "$A failed the byte checks (above) — not signing"
    ( # shellcheck source=require-mail-oauth-build-env.sh
      source "$ROOT/scripts/require-mail-oauth-build-env.sh"
      assert_daemon_oauth_not_placeholder "$ASSETS/$A" ) || die "$A: OAuth bake check failed — not signing a placeholder build"
    k2u_sign u1 "$ASSETS/$A" "$A" "$VERSION" "$ASSETS/$A.sig" raw || die "$A: U1 signing failed"
    k2u_sign u2 "$ASSETS/$A" "$A" "$VERSION" "$ASSETS/$A.sig2" raw || die "$A: U2 signing failed"
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
    for ARCH in "${ARCHES[@]}"; do cp "$ASSETS/k2-daemon-linux-$ARCH.sig" "$ASSETS/k2-daemon-linux-$ARCH.sig2" "$OUT/"; done
    cp "$ASSETS/daemon-latest.json" "$OUT/"
    log "dry run: signatures + manifest in $OUT (nothing uploaded)"
    exit 0
fi

# 5. Upload: signatures (and compute-built bytes) first, the manifest last.
up=()
for ARCH in "${ARCHES[@]}"; do
    A="k2-daemon-linux-$ARCH"
    up+=("$ASSETS/$A.sig" "$ASSETS/$A.sig2")
    # Files that came from compute aren't on the release yet.
    if [ -n "$FROM_DIR" ] && [ -f "$FROM_DIR/$A" ]; then up+=("$ASSETS/$A" "$ASSETS/$A.sha256"); fi
done
gh release upload "$TAG" -R "$REPO" "${up[@]}" --clobber
gh release upload "$TAG" -R "$REPO" "$ASSETS/daemon-latest.json" --clobber
[ -z "$OUT" ] || cp "$ASSETS"/*.sig "$ASSETS"/*.sig2 "$ASSETS/daemon-latest.json" "$OUT/"

# 6. What a 0.46.4 install sees: the published manifest's sig2 for each
# Linux daemon, downloaded, verified with the source tree's UPDATER_KEYS.
CHECK="$WORK/published"; mkdir -p "$CHECK"
gh release download "$TAG" -R "$REPO" -p daemon-latest.json -D "$CHECK" --clobber
for ARCH in "${ARCHES[@]}"; do
    A="k2-daemon-linux-$ARCH"
    python3 -I - "$CHECK/daemon-latest.json" "linux-$ARCH" "$(sha256_of "$ASSETS/$A")" "https://github.com/$REPO/releases/download/$TAG/$A.sig2" <<'PY' \
        || die "published daemon-latest.json linux-$ARCH is wrong (sha256 or sig2): no 0.46.4+ Linux install could update"
import json, sys
e = json.load(open(sys.argv[1]))["artifacts"][sys.argv[2]]
sys.exit(0 if e.get("sha256") == sys.argv[3] and e.get("sig2") == sys.argv[4] else 1)
PY
    gh release download "$TAG" -R "$REPO" -p "$A" -p "$A.sig2" -D "$CHECK" --clobber
    [ "$(sha256_of "$CHECK/$A")" = "$(sha256_of "$ASSETS/$A")" ] \
        || die "the published $A is not the binary that was signed (swapped after signing?) — delete its .sig/.sig2 from $TAG now"
    k2u_verify_u2 "$CHECK/$A" "$CHECK/$A.sig2" "$A" "$VERSION" || die "published $A.sig2 does not verify with UPDATER_KEYS"
done
log "done: $TAG Linux daemons signed (U1 + U2); daemon-latest.json lists them; the published binaries are the signed ones and their sig2 verify"
