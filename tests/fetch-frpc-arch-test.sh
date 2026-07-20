#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
SIDECARS=(
    "$ROOT/src-tauri/binaries/frpc-x86_64-apple-darwin"
    "$ROOT/src-tauri/binaries/frpc-aarch64-apple-darwin"
)

for sidecar in "${SIDECARS[@]}"; do
    if [ -e "$sidecar" ]; then
        cp -p "$sidecar" "$TMP/original-$(basename "$sidecar")"
    fi
done

cleanup() {
    for sidecar in "${SIDECARS[@]}"; do
        backup="$TMP/original-$(basename "$sidecar")"
        if [ -e "$backup" ]; then
            cp -p "$backup" "$sidecar"
        else
            rm -f "$sidecar"
        fi
    done
    rm -rf "$TMP"
}
trap cleanup EXIT
mkdir -p "$TMP/bin"

cat > "$TMP/bin/uname" <<'SH'
#!/bin/sh
case "$1" in
    -s) printf '%s\n' "$TEST_UNAME_S" ;;
    -m) printf '%s\n' "$TEST_UNAME_M" ;;
    *) exit 2 ;;
esac
SH
cat > "$TMP/frpc" <<'SH'
#!/bin/sh
[ "${1:-}" = "--version" ] && printf '0.61.1\n'
SH
chmod +x "$TMP/bin/uname" "$TMP/frpc"

for arch in x86_64 arm64; do
    case "$arch" in
        x86_64) triple="x86_64-apple-darwin" ;;
        arm64)  triple="aarch64-apple-darwin" ;;
    esac
    TEST_UNAME_S=Darwin TEST_UNAME_M="$arch" PATH="$TMP/bin:$PATH" \
        FRPC_SRC="$TMP/frpc" "$ROOT/scripts/fetch-frpc.sh" >/dev/null
    cmp "$TMP/frpc" "$ROOT/src-tauri/binaries/frpc-$triple"
done

echo "fetch-frpc macOS architecture detection: PASS"
