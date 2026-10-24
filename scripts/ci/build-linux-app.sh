#!/usr/bin/env bash
# build-linux-app.sh — the Linux desktop app bundles (x86_64 deb, rpm,
# AppImage), built in the current checkout. The same steps app-linux.yml
# runs, as one script a K2 Compute node runs for release.sh
# (plan-ci-release-on-k2-compute-v1 §2 job L3).
#
#   scripts/ci/build-linux-app.sh [--out DIR]
#
# Writes into DIR (default dist/):
#   K2_<v>_amd64.deb  K2-<v>-1.x86_64.rpm  K2_<v>_amd64.AppImage
#   app-linux.sums    "<sha256>  <name>" lines (the SHA256SUMS lines)
#   build-info.json   version, commit, tree, toolchain, every file's hash
#
# The AppImage is over 100 MB: fetch it with `k2 compute fetch
# --over-relay` (the relay cap, G1). Unsigned; nothing here signs.
# Needs the GTK/WebKit dev packages (G6; the node's owner approves them:
# sudo k2-node packages allow …) and bun. Never reads ~/.k2.
set -euo pipefail

OUT="dist"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:-}"; shift 2 ;;
        *) echo "usage: scripts/ci/build-linux-app.sh [--out DIR]" >&2; exit 2 ;;
    esac
done
[ -n "$OUT" ] || { echo "usage: scripts/ci/build-linux-app.sh [--out DIR]" >&2; exit 2; }

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
if [ "$(uname -s)" != "Linux" ] || [ "$(uname -m)" != "x86_64" ]; then
    echo "FATAL: the Linux app bundles build on x86_64 Linux (this is $(uname -s) $(uname -m))" >&2
    exit 1
fi
for tool in bun cargo pkg-config; do
    command -v "$tool" >/dev/null || { echo "FATAL: $tool is not installed (--needs bun@1.2,rust@stable,sys:pkg-config)" >&2; exit 1; }
done
pkg-config --exists webkit2gtk-4.1 || { echo "FATAL: webkit2gtk-4.1 dev package missing (sys:libwebkit2gtk-4.1-dev; owner: sudo k2-node packages allow …)" >&2; exit 1; }

VERSION="$(python3 -c "import json; print(json.load(open('src-tauri/tauri.conf.json'))['version'])")"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
B="$TARGET_DIR/release/bundle"
rm -rf "$B/deb" "$B/rpm" "$B/appimage"

bun install --frozen-lockfile
# linuxdeploy without FUSE (job users have no /dev/fuse).
APPIMAGE_EXTRACT_AND_RUN=1 bunx tauri build --bundles deb,rpm,appimage

rm -rf "$OUT"
mkdir -p "$OUT"
FILES=("K2_${VERSION}_amd64.deb" "K2-${VERSION}-1.x86_64.rpm" "K2_${VERSION}_amd64.AppImage")
for f in "$B/deb/${FILES[0]}" "$B/rpm/${FILES[1]}" "$B/appimage/${FILES[2]}"; do
    if [ ! -f "$f" ]; then
        echo "FATAL: missing bundle $f" >&2
        ls -la "$B"/deb "$B"/rpm "$B"/appimage >&2 || true
        exit 1
    fi
    cp "$f" "$OUT/"
done
(cd "$OUT" && sha256sum "${FILES[@]}" > app-linux.sums)
test "$(grep -c . "$OUT/app-linux.sums")" = 3

K2_BI_OUT="$OUT" K2_BI_VERSION="$VERSION" \
K2_BI_COMMIT="$(git rev-parse HEAD 2>/dev/null || echo unknown)" \
K2_BI_TREE="$(git rev-parse 'HEAD^{tree}' 2>/dev/null || echo unknown)" \
K2_BI_RUSTC="$(rustc -vV)" K2_BI_BUN="$(bun --version)" \
K2_BI_FILES="${FILES[*]}" \
python3 - <<'PY'
import hashlib, json, os
out = os.environ["K2_BI_OUT"]
files = {}
for name in os.environ["K2_BI_FILES"].split():
    p = os.path.join(out, name)
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    files[name] = {"sha256": h.hexdigest(), "bytes": os.path.getsize(p)}
info = {
    "kind": "linux-app",
    "version": os.environ["K2_BI_VERSION"],
    "arch": "x86_64",
    "commit": os.environ["K2_BI_COMMIT"],
    "tree": os.environ["K2_BI_TREE"],
    "rustc": os.environ["K2_BI_RUSTC"],
    "bun": os.environ["K2_BI_BUN"],
    "files": files,
}
json.dump(info, open(os.path.join(out, "build-info.json"), "w"), indent=2, sort_keys=True)
print(json.dumps({k: info[k] for k in ("version", "commit")}))
PY
ls -la "$OUT"
echo "build-linux-app: done (unsigned)."
