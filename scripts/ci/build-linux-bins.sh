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
# x86_64 builds natively. aarch64 builds natively on an aarch64 machine,
# or cross-compiled on an x86_64 node with zig as the C compiler and
# linker (cargo-zigbuild): we have no ARM Linux node, and zig needs no
# root. The versions and the glibc floor are pinned in
# scripts/ci/cross-pins.sh (K2 Compute installs both from --needs,
# scripts/lib/compute-release.sh); the build refuses any other version.
# zig ships its own libc headers and stubs, so nothing comes from the
# node's distro and every x86_64 node builds the same bytes. Any other
# host/target pair is refused.
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
#     match the GitHub-built one. Release builds use K2_COMPUTE_OAUTH=secret
#     (compute-release.sh, Rosson 2026-10-29): vault secrets sealed to the
#     node, never in argv, the job plan or logs.
#   Neither → refused (the same FATAL as every release build).
#   build-info.json says which ("oauth") and whether a Microsoft client id
#   was baked too ("oauth_microsoft"; never a value).
#
# Paths: always remapped, the same way on every Linux builder
# (scripts/ci/repro-build-env.sh: checkout → /k2, cargo home → /cargo,
# target dir → /target, for rustc and the C/C++ deps), so two nodes and
# GitHub can build the same bytes (G12; needs the same rustc too). Before
# 0.46.6 only the V1 job remapped (BUILD_REMAP=1, now ignored).
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
CROSS=0
if [ "$HOST_ARCH" != "$ARCH" ]; then
    if [ "$HOST_ARCH" = "x86_64" ] && [ "$ARCH" = "aarch64" ]; then
        CROSS=1
    else
        echo "FATAL: asked for $ARCH on a $HOST_ARCH machine. Build x86_64 natively; aarch64 natively or cross from x86_64 (zig)." >&2
        exit 1
    fi
fi
TRIPLE="${ARCH}-unknown-linux-gnu"
# shellcheck source=scripts/ci/cross-pins.sh
source "$ROOT/scripts/ci/cross-pins.sh"
CROSS_GLIBC="$K2_CROSS_GLIBC"
ZIG_V="" ZIGBUILD_V=""

# shellcheck source=scripts/require-mail-oauth-build-env.sh
source "$ROOT/scripts/require-mail-oauth-build-env.sh"
# shellcheck source=scripts/linux-ptyd-embed.sh
source "$ROOT/scripts/linux-ptyd-embed.sh"
# shellcheck source=scripts/ci/repro-build-env.sh
source "$ROOT/scripts/ci/repro-build-env.sh"

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

k2_repro_build_env "$ROOT"

# aarch64 from x86_64: every `cargo build` (here and in the embed helpers)
# becomes `cargo zigbuild` for <triple>.<glibc>; zig compiles the C deps
# and links. cargo-zigbuild still writes to target/<triple>/release. The
# C path maps go in CFLAGS_<triple> (cargo-zigbuild names its compiler by
# full path, which the repro wrappers don't wrap). zig and cargo-zigbuild
# must be cross-pins.sh's versions: two nodes build the same bytes only
# with the same toolchain.
if [ "$CROSS" = 1 ]; then
    command -v zig >/dev/null 2>&1 || { echo "FATAL: the aarch64 cross build needs zig (--needs zig@<version>)" >&2; exit 1; }
    command -v cargo-zigbuild >/dev/null 2>&1 || { echo "FATAL: the aarch64 cross build needs cargo-zigbuild (--needs cargo:cargo-zigbuild@<version>)" >&2; exit 1; }
    ZIG_V="$(zig version)"
    ZIGBUILD_V="$(cargo-zigbuild --version | awk '{print $2}')"
    if [ "$ZIG_V" != "$K2_CROSS_ZIG" ]; then
        echo "FATAL: zig is $ZIG_V; scripts/ci/cross-pins.sh pins $K2_CROSS_ZIG" >&2; exit 1
    fi
    if [ "$ZIGBUILD_V" != "$K2_CROSS_ZIGBUILD" ]; then
        echo "FATAL: cargo-zigbuild is $ZIGBUILD_V; scripts/ci/cross-pins.sh pins $K2_CROSS_ZIGBUILD" >&2; exit 1
    fi
    if [ ! -d "$(rustc --print sysroot)/lib/rustlib/$TRIPLE" ]; then
        echo "Adding the Rust standard library for $TRIPLE (rustup target add)..."
        rustup target add "$TRIPLE"
    fi
    cflags_var="CFLAGS_${TRIPLE//-/_}"
    cxxflags_var="CXXFLAGS_${TRIPLE//-/_}"
    export "$cflags_var=${!cflags_var:+${!cflags_var} }$K2_REPRO_CFLAGS"
    export "$cxxflags_var=${!cxxflags_var:+${!cxxflags_var} }$K2_REPRO_CFLAGS"
    # zig's own cache (its libc stubs and compiler-rt) stays inside the job.
    export ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-${TMPDIR:-/tmp}/k2-zig-cache}"
    cargo() {
        if [ "${1:-}" = "build" ]; then
            shift
            local -a a=()
            while [ $# -gt 0 ]; do
                if [ "$1" = "--target" ] && [ "${2:-}" = "$TRIPLE" ]; then
                    a+=(--target "$TRIPLE.$CROSS_GLIBC"); shift 2
                else
                    a+=("$1"); shift
                fi
            done
            command cargo zigbuild "${a[@]}"
        else
            command cargo "$@"
        fi
    }
    echo "  cross: $TRIPLE on $HOST_ARCH with zig $ZIG_V (cargo-zigbuild $ZIGBUILD_V), glibc $CROSS_GLIBC"
fi

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
REL="$TARGET_DIR/$TRIPLE/release"
DAEMON="k2-daemon-linux-$ARCH"
NODE="k2-node-linux-$ARCH"
HELPER="k2-root-helper-linux-$ARCH"
ENGINE="k2-app-engine-linux-$ARCH"

echo "Building k2-ptyd for the daemon to embed ($TRIPLE)..."
k2_ptyd_build_for_embed "$TRIPLE"
echo "Building k2-export for the daemon to embed ($TRIPLE)..."
k2_export_build_for_embed "$TRIPLE"
echo "Building k2-deskd for the daemon to embed ($TRIPLE)..."
k2_deskd_build_for_embed "$TRIPLE"
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
assert_daemon_embeds_export "$OUT/$DAEMON"
assert_daemon_embeds_deskd "$OUT/$DAEMON"
# The marker scripts/sign-daemon.sh requires exactly once (version + arch).
test "$(grep -a -o 'K2_DAEMON_BUILD v=[0-9.]* os=linux arch=[a-z0-9_]*;' "$OUT/$DAEMON" | wc -l)" = 1 \
    || { echo "FATAL: $DAEMON has no single K2_DAEMON_BUILD marker" >&2; exit 1; }
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
# V8: the commit k2-node reports in its signed self-statement.
if [ -n "${K2_NODE_BUILD_COMMIT:-}" ]; then
    test "$(grep -a -o "K2_NODE_COMMIT c=$K2_NODE_BUILD_COMMIT;" "$OUT/$NODE" | wc -l)" = 1 \
        || { echo "FATAL: $NODE doesn't carry K2_NODE_COMMIT c=$K2_NODE_BUILD_COMMIT exactly once" >&2; exit 1; }
fi
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
EXPORT_SHA="$(_k2_sha256_of "$K2_EXPORT_EMBED")"
DESKD_SHA="$(_k2_sha256_of "$K2_DESKD_EMBED")"
# Whether a Microsoft client id was baked too (never the value): a shadow
# diff against GitHub's daemon can come from that alone.
OAUTH_MS=false
if [ "$OAUTH_MODE" = "baked" ] && ! _k2_oauth_is_placeholder "${K2_MICROSOFT_CLIENT_ID:-}"; then
    OAUTH_MS=true
fi
K2_BI_OUT="$OUT" K2_BI_ARCH="$ARCH" K2_BI_VERSION="$VERSION" K2_BI_OAUTH="$OAUTH_MODE" K2_BI_OAUTH_MS="$OAUTH_MS" \
K2_BI_COMMIT="$(k2_ci_rev HEAD "${BUILD_COMMIT:-unknown}")" \
K2_BI_TREE="$(k2_ci_rev 'HEAD^{tree}' "${BUILD_TREE:-unknown}")" \
K2_BI_RUSTC="$(rustc -vV)" K2_BI_CARGO="$(cargo -V)" \
K2_BI_GLIBC="$(if [ "$CROSS" = 1 ]; then echo "cross: glibc $CROSS_GLIBC (zig $ZIG_V, cargo-zigbuild $ZIGBUILD_V)"; else ldd --version 2>/dev/null | head -1 || echo unknown; fi)" \
K2_BI_PTYD="$PTYD_SHA" K2_BI_EXPORT="$EXPORT_SHA" K2_BI_DESKD="$DESKD_SHA" \
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
    "oauth_microsoft": os.environ["K2_BI_OAUTH_MS"] == "true",
    "remap": "/k2 /cargo /target",
    "ptyd_sha256": os.environ["K2_BI_PTYD"],
    "export_sha256": os.environ["K2_BI_EXPORT"],
    "deskd_sha256": os.environ["K2_BI_DESKD"],
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
