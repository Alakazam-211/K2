#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/k2-headless-intel-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
mkdir "$TMP/bin"

cat > "$TMP/bin/uname" <<'EOF'
#!/bin/sh
[ "$1" = -s ] && echo Darwin || echo "${FAKE_ARCH:-x86_64}"
EOF
cat > "$TMP/bin/sysctl" <<'EOF'
#!/bin/sh
case "$*" in
  *sysctl.proc_translated*) echo "${FAKE_TRANSLATED:-0}" ;;
  *hw.optional.arm64*) echo "${FAKE_ARM64_HW:-0}" ;;
  *) exit 1 ;;
esac
EOF
cat > "$TMP/bin/git" <<'EOF'
#!/bin/sh
case "$*" in
  *'diff '*'--quiet'*) exit 0 ;;
  *'cat-file -t v0.41.3'*) echo tag ;;
  *'rev-parse HEAD'*) echo 655fe8bbbc5bbd259cabb77ea714f3bf092db2c3 ;;
  *'rev-parse v0.41.3^{}'*) echo 655fe8bbbc5bbd259cabb77ea714f3bf092db2c3 ;;
  *'hash-object cli/k2'*) echo cli-blob ;;
  *'rev-parse v0.41.3:cli/k2'*)
    [ "${FAKE_CLI_MISMATCH:-0}" = 1 ] && echo other-blob || echo cli-blob ;;
  *) echo "unexpected fake git args: $*" >&2; exit 2 ;;
esac
EOF
cat > "$TMP/bin/lipo" <<'EOF'
#!/bin/sh
echo "${FAKE_LIPO_ARCHS:-x86_64}"
EOF
cat > "$TMP/bin/file" <<'EOF'
#!/bin/sh
echo "$1: Mach-O 64-bit executable x86_64"
EOF
cat > "$TMP/bin/codesign" <<'EOF'
#!/bin/sh
case "$1" in
  --force) for target; do :; done; printf '\nAPPLE-SIGNED\n' >> "$target" ;;
  --verify) : ;;
  *) exit 2 ;;
esac
EOF
cat > "$TMP/bin/bunx" <<'EOF'
#!/bin/sh
target="$4"
[ "${FAKE_BUNX_FAIL:-0}" != 1 ] || exit 9
grep -q 'APPLE-SIGNED' "$target" || { echo 'daemon was not Apple-signed first' >&2; exit 8; }
digest=$(shasum -a 256 "$target" | awk '{print $1}')
printf 'untrusted comment: test signature\n%s\n' "$digest" > "$target.sig"
EOF
chmod +x "$TMP/bin/"*

daemon="$TMP/k2-daemon"
printf 'Mach-O fixture with version 0.41.3\n' > "$daemon"
chmod +x "$daemon"
out="$TMP/out"

PATH="$TMP/bin:$PATH" \
APPLE_SIGNING_IDENTITY='Developer ID Application: Test' \
TAURI_SIGNING_PRIVATE_KEY='test-key' \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD='test-password' \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$out" \
    --base-url http://127.0.0.1:9876

test -f "$out/k2-daemon-macos-x86_64"
test -f "$out/k2-daemon-macos-x86_64.sig"
test -f "$out/cli/k2"
test -f "$out/daemon-latest.json"
test -f "$out/SHA256SUMS"
grep -q 'APPLE-SIGNED' "$out/k2-daemon-macos-x86_64"
grep -q '"macos-x86_64"' "$out/daemon-latest.json"
grep -q 'http://127.0.0.1:9876/k2-daemon-macos-x86_64' "$out/daemon-latest.json"
python3 - "$out/daemon-latest.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
artifact = document["artifacts"]["macos-x86_64"]
assert document["version"] == "0.41.3"
assert artifact["url"] == "http://127.0.0.1:9876/k2-daemon-macos-x86_64"
assert artifact["sig"] == "http://127.0.0.1:9876/k2-daemon-macos-x86_64.sig"
assert len(artifact["sha256"]) == 64
PY
(cd "$out" && shasum -a 256 -c SHA256SUMS)

if PATH="$TMP/bin:$PATH" \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/bad" --base-url http://example.com; then
    echo "non-loopback HTTP must fail" >&2
    exit 1
fi

reject_url() {
    label="$1"
    url="$2"
    if PATH="$TMP/bin:$PATH" \
        APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
        "$ROOT/scripts/package-headless-macos-intel.sh" \
        --daemon "$daemon" --out-dir "$TMP/rejected-$label" --base-url "$url"; then
        echo "$label URL must fail" >&2
        exit 1
    fi
    test ! -e "$TMP/rejected-$label"
}

reject_url credentials 'https://user:pass@example.com/release'
reject_url query 'https://example.com/release?channel=stable'
reject_url fragment 'https://example.com/release#stable'
reject_url quote 'https://example.com/a"b'
reject_url backslash 'https://example.com/a\b'
reject_url traversal 'https://example.com/a/../b'
reject_url encoded-control 'https://example.com/a%0Ab'
reject_url literal-control $'https://example.com/a\nb'

if PATH="$TMP/bin:$PATH" FAKE_ARCH=arm64 \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/wrong-arch" \
    --base-url https://example.com; then
    echo "non-Intel host must fail" >&2
    exit 1
fi

if PATH="$TMP/bin:$PATH" FAKE_TRANSLATED=1 \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/translated" \
    --base-url https://example.com; then
    echo "Rosetta process must fail" >&2
    exit 1
fi

if PATH="$TMP/bin:$PATH" FAKE_LIPO_ARCHS='x86_64 arm64' \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/fat" \
    --base-url https://example.com; then
    echo "fat binary must fail" >&2
    exit 1
fi

if PATH="$TMP/bin:$PATH" FAKE_CLI_MISMATCH=1 \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/dirty-cli" \
    --base-url https://example.com; then
    echo "non-tag CLI bytes must fail" >&2
    exit 1
fi

if PATH="$TMP/bin:$PATH" FAKE_BUNX_FAIL=1 \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/signer-fail" \
    --base-url https://example.com; then
    echo "signer failure must fail" >&2
    exit 1
fi
test ! -e "$TMP/signer-fail"

if PATH="$TMP/bin:$PATH" \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/spoofed-loopback" \
    --base-url 'http://127.0.0.1:80@evil.example/release'; then
    echo "spoofed loopback URL must fail" >&2
    exit 1
fi

# Native hardware and fresh-output guards also fail before publishing.
if PATH="$TMP/bin:$PATH" FAKE_ARM64_HW=1 \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$TMP/arm-hardware" \
    --base-url https://example.com; then
    echo "Apple Silicon hardware must fail" >&2
    exit 1
fi
test ! -e "$TMP/arm-hardware"

before="$(shasum -a 256 "$out/SHA256SUMS")"
if PATH="$TMP/bin:$PATH" \
    APPLE_SIGNING_IDENTITY=x TAURI_SIGNING_PRIVATE_KEY=x \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x \
    "$ROOT/scripts/package-headless-macos-intel.sh" \
    --daemon "$daemon" --out-dir "$out" \
    --base-url https://example.com; then
    echo "existing output must fail" >&2
    exit 1
fi
[ "$before" = "$(shasum -a 256 "$out/SHA256SUMS")" ]
(cd "$out" && shasum -a 256 -c SHA256SUMS)

echo "OK: native Intel package is signed before hashing and emits bounded updater metadata"
