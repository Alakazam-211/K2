#!/bin/bash
# k2-ptyd compatibility CI (prd-k2-ptyd-v1 P66, §12 item 15): the daemon at
# HEAD against the k2-ptyd binaries of the previous two releases.
#
#   scripts/ptyd-survival/compat.sh [REV...]     (default: v0.46.3 v0.46.4)
#
# REV is a tag or a commit (K2 compute jobs have the history but no tags:
# pass the tags' commits, e.g. `$(git rev-parse 'v0.46.3^{commit}')`).
#
# For each REV, that release's k2-ptyd is built from its own source (the
# crate + Cargo.lock from `git archive`, built standalone in a temp dir; the
# repo is only read), then the HEAD daemon's T1 survival harness runs:
#
#  1. K2_PTYD_BIN=<old k2-ptyd>: every ptyd launch is the old binary. The
#     restart, crash, adoption, heartbeat and stop cases must all pass.
#  2. The S7 upgrade case with the OLD ptyd already running (agents in it)
#     and HEAD's k2-ptyd as the daemon's binary, minimum version raised:
#     the daemon asks for an in-place upgrade; a 0.1.x ptyd answers
#     `unsupported`, and the daemon must keep it and adopt as before.
#
# Any handshake refusal, survival failure, unknown REV or build failure
# fails the run (exit != 0). Linux and macOS. Never touches the real ~/.k2,
# the keychain or a running daemon (the harness uses scratch homes).
#
# Honours CARGO_TARGET_DIR (compute nodes set one).
set -euo pipefail
cd "$(dirname "$0")/../.."
for v in $(env | grep -E '^(K2_|K2SO_)' | cut -d= -f1); do unset "$v"; done
# Compute jobs start with an empty env; the daemon's shell sessions need one.
export SHELL="${SHELL:-/bin/bash}"
# A compute job's checkout belongs to another uid: let git read it.
g() { git -c safe.directory="$PWD" "$@"; }

revs=("$@")
[ ${#revs[@]} -gt 0 ] || revs=(v0.46.3 v0.46.4)
target="${CARGO_TARGET_DIR:-$PWD/target}"
work="$(mktemp -d "${TMPDIR:-/tmp}/k2-ptyd-compat-XXXXXX")"
trap 'rm -rf "$work"' EXIT

echo "== HEAD $(g rev-parse --short HEAD): building k2-ptyd and the k2-daemon survival harness"
cargo build -p k2-ptyd --bins
head_ptyd="$work/k2-ptyd-HEAD"
cp "$target/debug/k2-ptyd" "$head_ptyd"
cargo test -p k2-daemon --test ptyd_survival_integration --no-run

upgrade_test=a_restart_upgrades_an_old_ptyd_in_place_or_keeps_one_that_cannot
results=()
for rev in "${revs[@]}"; do
    commit=$(g rev-parse -q --verify "$rev^{commit}") || { echo "FAIL: $rev is not in this repo (git fetch --tags)" >&2; exit 1; }
    tag="${commit:0:10}"
    src="$work/src-$tag"
    mkdir -p "$src"
    g archive "$commit" crates/k2-ptyd Cargo.lock | tar -x -C "$src"
    crate="$src/crates/k2-ptyd"
    # Standalone: detach from the workspace, keep the release's dependency pins.
    printf '\n[workspace]\n' >> "$crate/Cargo.toml"
    cp "$src/Cargo.lock" "$crate/Cargo.lock"
    CARGO_TARGET_DIR="$work/target-$tag" cargo build --manifest-path "$crate/Cargo.toml" --bin k2-ptyd
    old="$work/k2-ptyd-$tag"
    cp "$work/target-$tag/debug/k2-ptyd" "$old"
    echo "== $rev ($tag): $("$old" --version) vs HEAD $("$head_ptyd" --version)"

    echo "-- $rev 1/2: HEAD daemon, every ptyd launch is $rev's"
    K2_PTYD_BIN="$old" cargo test -p k2-daemon --test ptyd_survival_integration -- --skip "$upgrade_test" 2>&1 | tee "$work/survival-$tag.log"
    grep -q "test result: ok. 2 passed" "$work/survival-$tag.log" || { echo "FAIL: $rev: expected the 2 survival cases to pass" >&2; exit 1; }
    echo "-- $rev 2/2: $rev's ptyd already running, HEAD's k2-ptyd as the daemon's binary, upgrade asked"
    K2_PTYD_BIN="$head_ptyd" K2_PTYD_PRELAUNCH_BIN="$old" \
        cargo test -p k2-daemon --test ptyd_survival_integration -- "$upgrade_test" --exact 2>&1 | tee "$work/upgrade-$tag.log"
    grep -q "test result: ok. 1 passed" "$work/upgrade-$tag.log" || { echo "FAIL: $rev: the upgrade case did not pass" >&2; exit 1; }
    results+=("ok: HEAD daemon + k2-ptyd from $rev ($tag, $("$old" --version))")
done

echo "== P66 compatibility ($(uname -s) $(uname -m)):"
printf '%s\n' "${results[@]}"
