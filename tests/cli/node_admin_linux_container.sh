#!/usr/bin/env bash
# Driver for the node admin Linux end-to-end: build the test-keys
# k2-node builds, then run tests/cli/node_admin_linux.sh as root inside a
# THROWAWAY podman container (`--rm`). For the build box only
# (root@37.27.67.180, a scratch checkout) — never a customer box.
#
#   bash tests/cli/node_admin_linux_container.sh <scratch-out-dir>
#
# Builds (test-keys only; never release): 0.0.1 (installed), 0.0.2 (good
# update), 0.0.3 (K2_NODE_TEST_BROKEN=1: exits at start) + the
# k2-node-testsign controller/owner stand-in.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:?scratch output dir}"
IMAGE="${K2_E2E_IMAGE:-docker.io/library/ubuntu:24.04}"
mkdir -p "$OUT/builds"
export CARGO_TARGET_DIR="$OUT/target"
cd "$ROOT"
build() { # <version> [broken]
    K2_NODE_TEST_VERSION="$1" K2_NODE_TEST_BROKEN="${2:-0}" \
        cargo build -q -p k2-node --features test-keys --bin k2-node --example k2-node-testsign
    cp "$CARGO_TARGET_DIR/debug/k2-node" "$OUT/builds/k2-node-$1"
}
build 0.0.1
build 0.0.2
build 0.0.3 1
cp "$CARGO_TARGET_DIR/debug/examples/k2-node-testsign" "$OUT/builds/"
cp "$ROOT/tests/cli/node_admin_linux.sh" "$OUT/builds/"
chmod -R a+rX "$OUT/builds"
"$OUT/builds/k2-node-0.0.2" version | grep -q "0.0.2" || { echo "build version override failed" >&2; exit 1; }
podman run --rm --name "k2-node-admin-e2e-$$" -v "$OUT/builds:/builds:ro" "$IMAGE" bash -c '
    set -e
    export DEBIAN_FRONTEND=noninteractive
    apt-get -qq update >/dev/null && apt-get -qq install -y python3 procps >/dev/null
    K2_DISPOSABLE_BOX=1 bash /builds/node_admin_linux.sh /builds
'
