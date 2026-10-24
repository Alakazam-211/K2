#!/usr/bin/env bash
# build-linux-bins.sh — the Linux release binaries for one arch, built
# natively in the current checkout. One script for every builder: a K2
# Compute node (release.sh's compute path, plan-ci-release-on-k2-compute-v1
# §2 job L1/L2), and later daemon-binaries.yml, so the two can't drift.
#
#   scripts/ci/build-linux-bins.sh <x86_64|aarch64> [--out DIR]
#
# Writes into DIR (default dist/), nothing else leaves the checkout:
#   k2-daemon-linux-<arch>        (embeds k2-ptyd, P84)
#   k2-node-linux-<arch>
#   k2-root-helper-linux-<arch>
#   k2-app-engine-linux-<arch>
#   <each>.sha256                 ("<sha256>  <name>", sha256sum format)
#   build-info.json               version, commit, tree, toolchain, glibc,
#                                 OAuth mode, every file's sha256 + bytes
#
# UNSIGNED, always: the updater key and the root-helper key never leave
# z3mbpZ (release.sh signs after it fetched and checked these).
#
# Native only. aarch64 is built on an aarch64 machine (the ARM Linux VM
# on a Mac mini, G10), never cross-compiled here: a mismatch is refused.
#
# Job inputs are plain env names: K2 Compute refuses any K2_* name in
# --env (env_k2_reserved), so a node job passes BUILD_* and this script
# maps them.
#
# Mail OAuth (the Gmail/Microsoft client values the daemon compiles in):
#   K2_GMAIL_CLIENT_ID / K2_GMAIL_CLIENT_SECRET set (or, from a compute
#     job, BUILD_GMAIL_CLIENT_ID / BUILD_GMAIL_CLIENT_SECRET /
#     BUILD_MICROSOFT_CLIENT_ID) → baked and checked in the binary (as
#     daemon-binaries.yml does).
#   BUILD_OAUTH=placeholder → built WITHOUT them, recorded as
#     "oauth": "placeholder" in build-info.json. Shadow builds only: such a
#     daemon must never ship (release.sh refuses it), and its hash can't
#     match the GitHub-built one. Whether the values may travel to nodes
#     as plain --env is Rosson's call (plan §9 Q4).
#   Neither → refused (the same FATAL as every release build).
#
# Optional:
#   BUILD_REMAP=1   --remap-path-prefix for the checkout and cargo home, so
#                   two nodes (different job dirs) can build the same bytes
#                   (G12; needs the same pinned rustc on both).
#
# Never reads ~/.k2, never talks to a daemon, never signs.
set -euo pipefail

usage() { echo "usage: scripts/ci/build-linux-bins.sh <x86_64|aarch64> [--out DIR]" >&2; exit 2; }

ARCH="${1:-}"
shift || true
OUT="dist"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:-}"; shift 2 ;;
        *) usage ;;
    esac
done
case "$ARCH" in
    x86_64|aarch64) ;;
    *) usage ;;
esac
[ -n "$OUT" ] || usage

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# The checkout's commit/tree. A compute job's checkout belongs to the node
# user, so plain git refuses it ("dubious ownership"); safe.directory for
# this one call. No git at all → what the submitter said (BUILD_COMMIT /
# BUILD_TREE); the signed receipt stays the authority either way.
k2_ci_rev() {
    git -c safe.directory='*' rev-parse "$1" 2>/dev/null || printf '%s\n' "$2"
}

if [ "$(uname -s)" != "Linux" ]; then
    echo "FATAL: build-linux-bins.sh builds Linux binaries on Linux (this is $(uname -s))" >&2
    exit 1
fi
HOST_ARCH="$(uname -m)"
[ "$HOST_ARCH" = "arm64" ] && HOST_ARCH="aarch64"
if [ "$HOST_ARCH" != "$ARCH" ]; then
    echo "FATAL: asked for $ARCH on a $HOST_ARCH machine. Build each arch natively (aarch64: the ARM Linux VM node)." >&2
    exit 1
fi
TRIPLE="${ARCH}-unknown-linux-gnu"

# shellcheck source=scripts/require-mail-oauth-build-env.sh
source "$ROOT/scripts/require-mail-oauth-build-env.sh"
# shellcheck source=scripts/linux-ptyd-embed.sh
source "$ROOT/scripts/linux-ptyd-embed.sh"

# From a compute job: BUILD_* → the K2_* names option_env! reads.
for v in GMAIL_CLIENT_ID GMAIL_CLIENT_SECRET MICROSOFT_CLIENT_ID; do
    src="BUILD_$v" dst="K2_$v"
    if [ -n "${!src:-}" ] && [ -z "${!dst:-}" ]; then
        export "$dst=${!src}"
    fi
done
OAUTH_MODE="baked"
if [ "${BUILD_OAUTH:-}" = "placeholder" ]; then
    if [ -n "${K2_GMAIL_CLIENT_ID:-}" ]; then
        echo "FATAL: BUILD_OAUTH=placeholder but a Gmail client id is set; pick one" >&2
        exit 1
    fi
    OAUTH_MODE="placeholder"
    echo "  ⚠ Mail OAuth: PLACEHOLDER build (BUILD_OAUTH=placeholder). Shadow only; never ship this daemon."
else
    require_mail_oauth_build_env
fi

if [ "${BUILD_REMAP:-0}" = "1" ]; then
    CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
    export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT=/k2 --remap-path-prefix=$CARGO_HOME_DIR=/cargo"
    echo "  path remap on: $ROOT → /k2, $CARGO_HOME_DIR → /cargo"
fi

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
REL="$TARGET_DIR/$TRIPLE/release"
DAEMON="k2-daemon-linux-$ARCH"
NODE="k2-node-linux-$ARCH"
HELPER="k2-root-helper-linux-$ARCH"
ENGINE="k2-app-engine-linux-$ARCH"

echo "Building k2-ptyd for the daemon to embed ($TRIPLE)..."
k2_ptyd_build_for_embed "$TRIPLE"
echo "Building k2-daemon + k2-node ($TRIPLE)..."
cargo build --release --target "$TRIPLE" --bin k2-daemon --bin k2-node
echo "Building k2-root-helper ($TRIPLE)..."
cargo build --release --target "$TRIPLE" -p k2-root-helper
echo "Building k2-app-engine ($TRIPLE)..."
cargo build --release --target "$TRIPLE" -p k2-app-engine

rm -rf "$OUT"
mkdir -p "$OUT"
cp "$REL/k2-daemon" "$OUT/$DAEMON"
cp "$REL/k2-node" "$OUT/$NODE"
cp "$REL/k2-root-helper" "$OUT/$HELPER"
cp "$REL/k2-app-engine" "$OUT/$ENGINE"

# The byte checks daemon-binaries.yml runs, before anything leaves the node.
# release.sh runs them again on what it fetched (scripts/lib/compute-release.sh).
assert_daemon_embeds_ptyd "$OUT/$DAEMON"
for pair in "$HELPER:K2_ROOT_HELPER_TEST_KEYS_ENABLED" "$NODE:K2_NODE_TEST_KEYS_ENABLED" "$ENGINE:K2_APP_ENGINE_TEST_KEYS_ENABLED"; do
    f="${pair%%:*}" marker="${pair#*:}"
    if grep -a -q "$marker" "$OUT/$f"; then
        echo "FATAL: $f was built with test-keys" >&2
        exit 1
    fi
done
test "$(grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]* proto=[0-9]* arch=[a-z0-9_]*;' "$OUT/$HELPER" | wc -l)" = 1 \
    || { echo "FATAL: $HELPER has no single build marker" >&2; exit 1; }
test "$(grep -a -o 'K2_NODE_BUILD v=[0-9.]* proto=[0-9]* os=linux arch=[a-z0-9_]*;' "$OUT/$NODE" | wc -l)" = 1 \
    || { echo "FATAL: $NODE has no single build marker" >&2; exit 1; }
test "$(grep -a -o 'K2_APP_ENGINE_BUILD v=[0-9.]* proto=[0-9]* arch=[a-z0-9_]*;' "$OUT/$ENGINE" | wc -l)" = 1 \
    || { echo "FATAL: $ENGINE has no single build marker" >&2; exit 1; }
if [ "$OAUTH_MODE" = "baked" ]; then
    assert_daemon_oauth_not_placeholder "$OUT/$DAEMON"
fi
file "$OUT/$DAEMON" "$OUT/$NODE" "$OUT/$HELPER" "$OUT/$ENGINE"

for f in "$DAEMON" "$NODE" "$HELPER" "$ENGINE"; do
    (cd "$OUT" && sha256sum "$f" > "$f.sha256")
done

VERSION="$(grep -m1 '^version = ' crates/k2-daemon/Cargo.toml | sed 's/version = "\(.*\)"/\1/')"
PTYD_SHA="$(_k2_sha256_of "$K2_PTYD_EMBED")"
K2_BI_OUT="$OUT" K2_BI_ARCH="$ARCH" K2_BI_VERSION="$VERSION" K2_BI_OAUTH="$OAUTH_MODE" \
K2_BI_COMMIT="$(k2_ci_rev HEAD "${BUILD_COMMIT:-unknown}")" \
K2_BI_TREE="$(k2_ci_rev 'HEAD^{tree}' "${BUILD_TREE:-unknown}")" \
K2_BI_RUSTC="$(rustc -vV)" K2_BI_CARGO="$(cargo -V)" \
K2_BI_GLIBC="$(ldd --version 2>/dev/null | head -1 || echo unknown)" \
K2_BI_REMAP="${BUILD_REMAP:-0}" K2_BI_PTYD="$PTYD_SHA" \
K2_BI_FILES="$DAEMON $NODE $HELPER $ENGINE" \
python3 - <<'PY'
import hashlib, json, os
out = os.environ["K2_BI_OUT"]
files = {}
for name in os.environ["K2_BI_FILES"].split():
    p = os.path.join(out, name)
    files[name] = {"sha256": hashlib.sha256(open(p, "rb").read()).hexdigest(), "bytes": os.path.getsize(p)}
info = {
    "kind": "linux-bins",
    "version": os.environ["K2_BI_VERSION"],
    "arch": os.environ["K2_BI_ARCH"],
    "commit": os.environ["K2_BI_COMMIT"],
    "tree": os.environ["K2_BI_TREE"],
    "oauth": os.environ["K2_BI_OAUTH"],
    "remap": os.environ["K2_BI_REMAP"] == "1",
    "ptyd_sha256": os.environ["K2_BI_PTYD"],
    "rustc": os.environ["K2_BI_RUSTC"],
    "cargo": os.environ["K2_BI_CARGO"],
    "glibc": os.environ["K2_BI_GLIBC"],
    "files": files,
}
json.dump(info, open(os.path.join(out, "build-info.json"), "w"), indent=2, sort_keys=True)
print(json.dumps({k: info[k] for k in ("version", "arch", "commit", "oauth")}))
PY
ls -la "$OUT"
echo "build-linux-bins: $ARCH done (unsigned; sign on z3mbpZ)."
