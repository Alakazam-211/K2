#!/usr/bin/env bash
# The xy build: the CI brand (brand/xy) built and judged, so a build for a
# brand other than ours can prove no other name is left in what it prints.
#
#   bash scripts/brand/xy-build.sh
#
# 0. The name policy on brand/xy (scripts/brand.sh lint).
# 1. cargo check of the workspace's crates (rust-check's list, and the k5
#    binary) with K2_BRAND_DIR=brand/xy, every target, -D warnings.
# 2. cargo build of the k5 binary for xy: its pages and its --schema are
#    rewritten to the command xy when building (crates/k5-cli/build.rs),
#    and it carries the xy brand mark (scripts/brand/check-brand-mark.py).
# 3. What that binary prints for every help and study form
#    (scripts/brand/collect-outputs.sh), read by the three output judges
#    (scripts/brand/output-judges.py).
# 4. A bite: the same corpus with `Run k2 msg k2 hi` planted fails judges a
#    and b, and with `XY` planted fails judge c.
#
# The target folder (K2's ruling): the xy build gets a folder of its own,
# made for this run under the node's temp directory ($TMPDIR), never the
# shared cache and never a folder in $CARGO_TARGET_DIR. It starts cold, so
# the free space there is checked first: under XY_MIN_FREE_GB (default 40)
# nothing is built and the run fails with a line asking to resubmit on a
# node with more free disk. The folder is removed on every way out, with an
# M line that says so.
#
# XY_BUILD_OUT=<folder>: also write the review files there before the
# judges run (the corpus as corpus.tgz, the debt rows the judges would
# ask for as xy-debt.txt, the forms that fell through).
# Runs cargo and the CLI: on K2 Compute (Linux) only.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
step() { echo ""; echo "── xy build: $* ──"; }

export RUSTFLAGS="${RUSTFLAGS:--D warnings}"
export K2_BRAND_DIR=brand/xy
XY_MIN_FREE_GB="${XY_MIN_FREE_GB:-40}"

step "the xy brand's names pass the name policy"
bash scripts/brand.sh lint brand/xy

# Free disk for the xy folder, before anything builds.
XY_PARENT="${TMPDIR:-/tmp}"
free_gb="$(df -Pk "$XY_PARENT" | awk 'NR == 2 { print int($4 / 1048576) }')"
echo "M xy target: ${free_gb:-unknown} GB free under $XY_PARENT (want $XY_MIN_FREE_GB)"
if [ -z "$free_gb" ] || [ "$free_gb" -lt "$XY_MIN_FREE_GB" ]; then
    echo "M xy build=1 failed: [disk: ${free_gb:-unknown} GB free under $XY_PARENT, want $XY_MIN_FREE_GB; nothing was built; resubmit on a node with more free disk, never in the shared folder]"
    exit 1
fi
XY_TARGET="$(mktemp -d "$XY_PARENT/k2-xy-build.XXXXXX")"
W="$(mktemp -d "$XY_PARENT/k2-xy-corpus.XXXXXX")"
cleanup() {
    rm -rf "$XY_TARGET" "$W"
    echo "M xy target removed: $([ -e "$XY_TARGET" ] && echo no || echo yes)"
}
trap cleanup EXIT
export CARGO_TARGET_DIR="$XY_TARGET"

step "cargo check, K2_BRAND_DIR=brand/xy"
cargo check --locked -p k2-core -p k2-client -p k2-daemon -p k2-node-proto -p k2-node \
    -p k2-root-proto -p k2-root-helper -p k2-app-engine -p k2-ptyd -p k2-release-keys -p k2-export \
    -p k2-deskd-proto -p k2-deskd -p k2-deckrender -p k2-scenerender -p k2-brand -p k5-cli --all-targets

step "cargo build of the k5 binary, K2_BRAND_DIR=brand/xy"
cargo build --locked -p k5-cli --bin k5-cli
BIN="$XY_TARGET/debug/k5-cli"
step "the xy binary carries the xy brand mark, and only it"
python3 scripts/brand/check-brand-mark.py --brand xy "$BIN"

step "what the xy binary prints"
bash scripts/brand/collect-outputs.sh "$BIN" brand/xy "$W/corpus"
if [ -n "${XY_BUILD_OUT:-}" ]; then
    mkdir -p "$XY_BUILD_OUT"
    tar -C "$W" -czf "$XY_BUILD_OUT/corpus.tgz" corpus
    cp "$W/fell-through.txt" "$XY_BUILD_OUT/"
    python3 scripts/brand/output-judges.py --brand brand/xy --write-debt "$W/corpus" > "$XY_BUILD_OUT/xy-debt.txt" 2>/dev/null || true
    echo "review files in $XY_BUILD_OUT: corpus.tgz, fell-through.txt, xy-debt.txt ($(wc -l < "$XY_BUILD_OUT/xy-debt.txt" | tr -d ' ') rows)"
fi

step "the output judges"
python3 scripts/brand/output-judges.py --brand brand/xy "$W/corpus"

step "the bite: planted names fail the judges"
for plant in 'Run k2 msg k2 hi' 'XY keeps your agents'; do
    rm -rf "$W/bite"
    cp -R "$W/corpus" "$W/bite"
    first="$(find "$W/bite" -name '*.out' | sort | head -n 1)"
    printf '%s\n' "$plant" >> "$first"
    rc=0
    python3 scripts/brand/output-judges.py --brand brand/xy "$W/bite" > /dev/null 2> "$W/bite.err" || rc=$?
    case "$plant" in
        Run*) want="FAILED (a, b)" ;;
        *) want="FAILED (c)" ;;
    esac
    if [ "$rc" != 1 ] || ! grep -qF "$want" "$W/bite.err"; then
        echo "xy build: the planted line '$plant' did not fail as it should (rc $rc):" >&2
        cat "$W/bite.err" >&2
        exit 1
    fi
    echo "planted '$plant': $want, as it should"
done
echo ""
echo "xy build: passed"
