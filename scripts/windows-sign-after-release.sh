#!/bin/bash
# windows-sign-after-release.sh — add the Windows (Alpha) setup to a release
# that already shipped without it: one approval, both update signatures,
# then the setup, its checksum and the windows-x86_64 entry in latest.json.
#
# Why a separate script: release.sh opens ONE signing session per release
# before it builds (Step 0.46), and that session names the asset list and
# whether the Windows setup is in it. A release that shipped with Windows
# off has a session without it, and k2sa_open_session refuses to change an
# existing session's parameters ("Investigate before re-approving"). The
# signer itself allows a RE-APPROVAL on the same base commit; this script
# asks for exactly that, for the two Windows asset names only, through the
# same approval app (Touch ID or the login password). The macOS and Linux
# files were signed under the first session and are not touched.
#
# Where: z3mbpZ, in Rosson's GUI session (the approval sheet), from the
# main checkout, after scripts/windows-nsis-build.sh <version> brought the
# setup back. The setup is the exact bytes that are signed and uploaded.
#
# Secrets: none on argv or in the environment. The approval is a Secure
# Enclave signature the app writes to a 0600 file in a private temp dir;
# signing goes through scripts/lib/sign-updater.sh (`_k2sign`).
#
# It changes a LIVE release (assets, latest.json, notes), so it asks you to
# type the version before the first upload.
#
# Usage:
#   scripts/windows-sign-after-release.sh <version> [setup.exe]
#     setup.exe defaults to dist-windows/K2_<version>_x64-setup.exe and must
#     match dist-windows/SHA256SUMS-windows-x86_64.txt.
# Env: K2_RELEASE_REPO (default Alakazam-211/K2),
#      K2_SIGN_APPROVE_TIMEOUT_MIN / K2_SIGN_APPROVE_APP (sign-approve.sh).
set -euo pipefail

VERSION="${1:-}"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
RELEASE_REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
TAG="v${VERSION}"
WIN_ASSET="K2_${VERSION}_x64-setup.exe"
SETUP="${2:-$PROJECT_DIR/dist-windows/$WIN_ASSET}"
SUMS="$(dirname "$SETUP")/SHA256SUMS-windows-x86_64.txt"
# The Windows (Alpha) notes' "Known issues", per version (public text: no
# client names). A version without an entry gets no list.
KNOWN_ISSUES=()
case "$VERSION" in
    0.46.6)
        KNOWN_ISSUES=(
            "A sign-in K2 is still checking may stay as a separate entry instead of joining the account it belongs to; remove it by hand."
            "Restoring the vault from a recovery phrase or backup isn't working on Windows yet."
        )
        ;;
esac

die() { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }
say() { printf '[win-after-release] %s\n' "$*" >&2; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: $0 <version> [setup.exe]"
[ "$(uname -s)" = Darwin ] || die "run this on z3mbpZ (macOS), where the signer lives"
[ -z "${K2_SIGN_APPROVE_CMD:-}" ] || die "K2_SIGN_APPROVE_CMD is for tests only"
[ -z "${K2SIGN_TEST_ROOT:-}" ] || die "K2SIGN_TEST_ROOT is for tests only"
[ -z "${K2_SIGNER:-}" ] || [ "$K2_SIGNER" = k2sign ] || die "K2_SIGNER=$K2_SIGNER: this script signs through k2sign only"

cd "$PROJECT_DIR"
# shellcheck source=lib/sign-updater.sh
source "$PROJECT_DIR/scripts/lib/sign-updater.sh"
# shellcheck source=lib/sign-approve.sh
source "$PROJECT_DIR/scripts/lib/sign-approve.sh"
k2u_env_guard

# ── The release and its base commit ──
TAG_COMMIT="$(git rev-parse --verify --quiet "${TAG}^{commit}")" || die "no tag ${TAG} here (git fetch --tags)"
[ "$(git log -1 --format=%s "$TAG_COMMIT")" = "chore(release): ${TAG}" ] \
    || die "${TAG} is not a release.sh release commit; can't work out the approved base"
BASE="$(git rev-parse "${TAG_COMMIT}^")"

# ── The setup: the bytes windows-nsis-build.sh brought back ──
[ -s "$SETUP" ] || die "no setup at $SETUP (scripts/windows-nsis-build.sh ${VERSION})"
[ "$(basename "$SETUP")" = "$WIN_ASSET" ] || die "the setup must be named $WIN_ASSET"
[ -s "$SUMS" ] || die "no $SUMS next to the setup"
WANT="$(awk -v n="$WIN_ASSET" '$2 == n || $2 == "*"n {print $1}' "$SUMS")"
HAVE="$(shasum -a 256 "$SETUP" | awk '{print $1}')"
[ -n "$WANT" ] && [ "$WANT" = "$HAVE" ] || die "$WIN_ASSET is $HAVE, $SUMS says ${WANT:-nothing}"
say "setup $WIN_ASSET sha256 $HAVE"

# ── The live release must exist, have a latest.json, and no setup yet ──
command -v gh >/dev/null || die "gh is not installed"
ASSETS="$(gh release view "$TAG" --repo "$RELEASE_REPO" --json assets --jq '.assets[].name')" \
    || die "can't read release ${TAG} on ${RELEASE_REPO}"
grep -qx "latest.json" <<<"$ASSETS" || die "${TAG} has no latest.json"
UPLOADED=0
if grep -qx "$WIN_ASSET" <<<"$ASSETS"; then
    # A re-run after a partial one: go on only if it is these exact bytes.
    pre="$(mktemp -d "${TMPDIR:-/tmp}/k2-win-pre.XXXXXX")"
    gh release download "$TAG" --repo "$RELEASE_REPO" --pattern "$WIN_ASSET" --dir "$pre"
    [ "$(shasum -a 256 "$pre/$WIN_ASSET" | awk '{print $1}')" = "$HAVE" ] \
        || { rm -rf "$pre"; die "${TAG} already has a DIFFERENT $WIN_ASSET; this script never replaces a published setup"; }
    rm -rf "$pre"
    UPLOADED=1
    say "${TAG} already has these exact setup bytes (a re-run); not uploading them again"
fi

# ── One approval for the two Windows names (a RE-APPROVAL of the base) ──
WIN_NAMES=(setup.exe "$WIN_ASSET")
names_sorted="$(printf '%s\n' "${WIN_NAMES[@]}" | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//')"
st="$(k2u_k2sign status "$VERSION")" || die "k2sign status failed (above)"
[ "$(printf '%s' "$st" | k2sa_status_field approval)" = on ] || die "the signer's approval policy is off; use release.sh's rules, not this script"
sbase="$(printf '%s' "$st" | k2sa_status_field session.base_commit)"
[ -n "$sbase" ] || die "${VERSION} has no signing session; it wasn't released through k2sign"
[ "$sbase" = "$BASE" ] || die "${VERSION}'s session is for base ${sbase:0:12}, ${TAG}^ is ${BASE:0:12}"
if [ "$(printf '%s' "$st" | k2sa_status_field session.assets)" = "$names_sorted" ] \
    && [ "$(printf '%s' "$st" | k2sa_status_field session.windows)" = true ] \
    && [ "$(printf '%s' "$st" | k2sa_status_field session.valid)" = true ]; then
    say "using the Windows session already approved for ${VERSION} (a re-run)"
else
    work="$(mktemp -d "${TMPDIR:-/tmp}/k2-win-approve.XXXXXX")"
    trap 'rm -rf "$work"' EXIT
    cf="$work/challenge.json"; of="$work/approval.der"
    k2u_k2sign challenge "$VERSION" "$BASE" "${WIN_NAMES[@]}" --windows --reapprove > "$cf" \
        || die "the signer refused to issue a challenge (above)"
    nonce="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["nonce"])' "$cf")"
    printf '\n\033[1;33m══ Approval needed (Touch ID or your login password): RE-approve K2 %s (base %s) for the Windows setup ONLY (no independent check) ══\033[0m\n' \
        "$VERSION" "${BASE:0:7}" >&2
    /usr/bin/osascript -e "display notification \"Touch ID or password: approve the K2 $VERSION Windows setup\" with title \"K2 release\"" >/dev/null 2>&1 || true
    k2sa_run_helper "$cf" "$of" "$VERSION" || die "no approval; nothing was signed or uploaded"
    k2u_k2sign approve "$nonce" < "$of" >/dev/null || die "the signer did not accept the approval (above); nothing was signed"
    rm -rf "$work"; trap - EXIT
    say "Windows signing session open for ${VERSION}"
fi

# ── Last word before touching the live release ──
printf '\nThis uploads %s to the LIVE release %s and adds windows-x86_64 to its latest.json.\nType the version (%s) to go on: ' "$WIN_ASSET" "$TAG" "$VERSION" >&2
read -r answer
[ "$answer" = "$VERSION" ] || die "not confirmed; nothing uploaded (the approval stays valid for the session's hours)"

# Setup first, so latest.json never points at a missing file.
if [ "$UPLOADED" = 0 ]; then
    gh release upload "$TAG" "${SETUP}#Windows installer (Alpha, x64)" "$SUMS" --repo "$RELEASE_REPO"
    say "uploaded $WIN_ASSET and $(basename "$SUMS")"
fi

# Signs U1 (setup.exe, b64) and U2 (the URL name, k2_signatures) through
# k2sign, verifies both, and re-uploads latest.json with windows-x86_64.
bash "$PROJECT_DIR/scripts/windows-patch-latest-json.sh" "$VERSION" "$SETUP"

# The notes line release.sh adds when Windows ships with a release.
notes="$(mktemp "${TMPDIR:-/tmp}/k2-win-notes.XXXXXX")"
if gh release view "$TAG" --repo "$RELEASE_REPO" --json body --jq .body > "$notes"; then
    if ! grep -q "Windows (Alpha)" "$notes"; then
        {
            printf '\n\n### Windows (Alpha)\n\n`K2_%s_x64-setup.exe` is an Alpha build for Windows 10/11 (x64). It is not code-signed yet, so Windows SmartScreen will ask before it runs.\n' "$VERSION"
            if [ "${#KNOWN_ISSUES[@]}" -gt 0 ]; then
                printf '\n**Known issues**\n\n'
                printf -- '- %s\n' "${KNOWN_ISSUES[@]}"
            fi
        } >> "$notes"
        gh release edit "$TAG" --repo "$RELEASE_REPO" --notes-file "$notes" \
            || say "WARNING: could not add the Windows (Alpha) note to ${TAG}'s notes"
    fi
else
    say "WARNING: could not read ${TAG}'s notes to add the Windows (Alpha) note"
fi
rm -f "$notes"

# ── Check what clients will read ──
check="$(mktemp -d "${TMPDIR:-/tmp}/k2-win-check.XXXXXX")"
# latest.json is read through the GitHub API (the release asset itself), not
# the public download URL: GitHub's CDN can serve the copy from before the
# re-upload for a while. A few tries cover the API catching up; it stays a
# hard check (the script fails if the asset never shows windows-x86_64).
check_latest_json() {
    rm -f "$check/latest.json"
    gh release download "$TAG" --repo "$RELEASE_REPO" --pattern latest.json --dir "$check" --clobber || return 1
    python3 -I - "$check/latest.json" "$VERSION" "$WIN_ASSET" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
w = d["platforms"].get("windows-x86_64")
assert w, "no windows-x86_64 entry yet; platforms: " + ", ".join(sorted(d["platforms"]))
assert d["version"] == sys.argv[2], d["version"]
assert w["url"].endswith("/" + sys.argv[3]), w["url"]
assert w["signature"] and len(w["k2_signatures"]) == 1, w
print("latest.json platforms:", ", ".join(sorted(d["platforms"])))
PY
}
tries=5
for i in $(seq 1 "$tries"); do
    if check_latest_json; then
        break
    fi
    [ "$i" -lt "$tries" ] || die "${TAG}'s latest.json (read through the GitHub API, $tries tries) still has no good windows-x86_64 entry; clients won't see the Windows setup. Check the asset with: gh release download $TAG --repo $RELEASE_REPO --pattern latest.json"
    say "latest.json check $i/$tries failed; trying again in 10 s"
    sleep 10
done
curl -fsSL "https://github.com/${RELEASE_REPO}/releases/download/${TAG}/${WIN_ASSET}" -o "$check/$WIN_ASSET"
[ "$(shasum -a 256 "$check/$WIN_ASSET" | awk '{print $1}')" = "$HAVE" ] || die "the published $WIN_ASSET differs from the signed bytes"
rm -rf "$check"
say "OK: ${TAG} has the Windows (Alpha) setup, signed for the updater"
