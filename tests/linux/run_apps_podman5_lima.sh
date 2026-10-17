#!/usr/bin/env bash
# K2 installs Podman 5 (0.46.1 lane P) on a throwaway Lima VM, from a dev
# Mac:
#
#   bash tests/linux/run_apps_podman5_lima.sh [ubuntu-24.04|debian-12] [--keep]
#
# 1. a fresh VM k2-podman5-<template>-<stamp> (Lima template; it mounts
#    only this run's work folder, never $HOME);
# 2. builds this checkout's daemon (debug), test-keys root helper + signer
#    and engine inside it (its own target dir), never on the Mac;
# 3. fetches the pinned podman-static tarball for the VM's arch and checks
#    it against k2-root-proto's pin;
# 4. runs tests/linux/apps_e2e_podman5.sh as root in a fresh boot of it;
# 5. deletes the VM (--keep leaves it for a look; delete it yourself).
#
# Never touches k2-dev or any VM it didn't make, the real ~/.k2, the
# keychain or the production daemon. Logs: ./target-it-logs/podman5-<stamp>/.
set -euo pipefail

TEMPLATE="${1:-ubuntu-24.04}"
KEEP=0
[ "${2:-}" = "--keep" ] && KEEP=1
case "$TEMPLATE" in ubuntu-24.04 | debian-12) ;; *) echo "template: ubuntu-24.04 or debian-12" >&2; exit 2 ;; esac
command -v limactl >/dev/null || { echo "limactl is required (brew install lima)" >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="$(date +%Y%m%d%H%M%S)"
VM="k2-podman5-${TEMPLATE%%.*}-$STAMP"
VM="${VM//./}"
LOGS="$ROOT/target-it-logs/podman5-$STAMP"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-podman5-XXXXXX")"
mkdir -p "$LOGS" "$WORK/it"
cleanup() {
	if [ "$KEEP" = 0 ]; then limactl delete -f "$VM" >/dev/null 2>&1 || true; rm -rf "$WORK"; else echo "kept VM $VM and $WORK"; fi
}
trap cleanup EXIT

git -C "$ROOT" archive -o "$WORK/src.tar" HEAD
echo "== VM $VM ($TEMPLATE) =="
limactl create --name "$VM" --tty=false --cpus 6 --memory 8 --disk 40 \
	--set ".mounts=[{\"location\":\"$WORK\",\"writable\":true}]" "template:$TEMPLATE" >"$LOGS/create.log" 2>&1
limactl start "$VM" >>"$LOGS/create.log" 2>&1

cat >"$WORK/build.sh" <<'BUILD'
#!/bin/bash
set -euo pipefail
W="$(cd "$(dirname "$0")" && pwd)"
export DEBIAN_FRONTEND=noninteractive
sudo apt-get update -q >/dev/null
sudo apt-get install -y -q --no-install-recommends build-essential pkg-config libssl-dev libdbus-1-dev cmake clang git python3 curl ca-certificates file >/dev/null
[ -x "$HOME/.cargo/bin/cargo" ] || curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal >/dev/null
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/k2target"
rm -rf "$HOME/k2src" && mkdir -p "$HOME/k2src" && tar -xf "$W/src.tar" -C "$HOME/k2src" && cd "$HOME/k2src"
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
cargo build -j 6 -p k2-daemon --bin k2-daemon 2>&1 | tail -n 1
cargo build --release -j 6 -p k2-root-helper --features test-keys --bin k2-root-helper --example k2-root-helper-testsign 2>&1 | tail -n 1
cargo build --release -j 6 -p k2-app-engine 2>&1 | tail -n 1
IT="$W/it"
mkdir -p "$IT/bin" "$IT/cli" "$IT/scripts" "$IT/tests" "$IT/podman"
cp "$CARGO_TARGET_DIR/debug/k2-daemon" "$IT/bin/k2-daemon"
cp "$CARGO_TARGET_DIR/release/k2-root-helper" "$IT/bin/k2-root-helper-cur"
cp "$CARGO_TARGET_DIR/release/examples/k2-root-helper-testsign" "$IT/bin/testsign"
cp "$CARGO_TARGET_DIR/release/k2-app-engine" "$IT/bin/k2-app-engine"
cp cli/k2 "$IT/cli/k2"
cp scripts/install-root-helper.sh "$IT/scripts/"
cp -r tests/linux/guarded-fixture tests/linux/engine_client.py tests/linux/apps_e2e_podman5.sh "$IT/tests/"
# The pinned tarball and its sha256, straight from k2-root-proto.
case "$(uname -m)" in x86_64) A=amd64 ;; aarch64) A=arm64 ;; esac
P=crates/k2-root-proto/src/podman.rs
V="$(sed -n 's/^pub const BUNDLE_VERSION: &str = "\(.*\)";/\1/p' "$P")"
SHA="$(grep -A3 "asset: \"podman-linux-$A.tar.gz\"" "$P" | sed -n 's/.*sha256: "\([0-9a-f]*\)".*/\1/p')"
curl -sSfL -o "$IT/podman/podman-linux-$A.tar.gz" "https://github.com/mgoltzsche/podman-static/releases/download/v$V/podman-linux-$A.tar.gz"
echo "$SHA  $IT/podman/podman-linux-$A.tar.gz" | sha256sum -c -
file "$IT/bin/"*
BUILD
echo "== build in the VM =="
limactl shell "$VM" -- bash "$WORK/build.sh" 2>&1 | tee "$LOGS/build.log"
echo "== fresh boot, then the e2e as root =="
limactl stop "$VM" >/dev/null 2>&1 && limactl start "$VM" >/dev/null 2>&1
set +e
limactl shell "$VM" -- sudo bash "$WORK/it/tests/apps_e2e_podman5.sh" "$WORK/it" 2>&1 | tee "$LOGS/e2e.log"
rc=${PIPESTATUS[0]}
set -e
echo "e2e exit $rc (logs: $LOGS)"
exit "$rc"
