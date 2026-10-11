#!/usr/bin/env bash
# install-mail-helper.sh — kept one release train for old hints and
# runbooks. The hosted-mail root helper is now k2-root-helper; this runs
# install-root-helper.sh with the same arguments (prd-k2-root-helper-v1
# §11.1, RH23). From the script's own directory when it is there, else the
# copy attached to the same release as --version.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
if [ -n "$HERE" ] && [ -f "$HERE/install-root-helper.sh" ]; then
	exec bash "$HERE/install-root-helper.sh" "$@"
fi
VERSION=""
prev=""
for a in "$@"; do
	case "$a" in --version=*) VERSION="${a#--version=}" ;; esac
	[ "$prev" = "--version" ] && VERSION="$a"
	prev="$a"
done
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "[install-mail-helper] pass --version <x.y.z> (this name now installs k2-root-helper)" >&2; exit 1; }
BASE="${K2_RELEASE_BASE:-https://github.com/Alakazam-211/K2/releases/download}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/k2-install-wrapper.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL --retry 3 --retry-delay 2 -o "$TMP/install-root-helper.sh" "$BASE/v$VERSION/install-root-helper.sh" \
	|| { echo "[install-mail-helper] could not download install-root-helper.sh for v$VERSION" >&2; exit 1; }
bash "$TMP/install-root-helper.sh" "$@"
