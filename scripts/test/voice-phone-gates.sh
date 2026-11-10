#!/usr/bin/env bash
# Voice calls from a paired phone (gap-voice-calls-mobile-v1 §11 row 4)
# gates, meant for a Linux compute node:
#   k2 compute run --sha <sha> --os linux --cpus 4 --needs rust@stable,cmake@3 \
#     --mem 24G -- scripts/test/voice-phone-gates.sh
#
# Warnings fail the build. Every K2_* variable is dropped so nothing reaches
# a real ~/.k2; SHELL is set (the node's job user has a nologin shell).
set -euo pipefail
export SHELL=/bin/bash
for v in $(compgen -e); do
  case "$v" in K2_* | K2SO_*) unset "$v" ;; esac
done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export RUSTFLAGS="-D warnings"
cd "$(dirname "$0")/../.."

cargo check --locked -p k2-daemon -p k2-core --tests --message-format short
cargo run --locked -q -p k2-core --bin contract-gen -- --check
cargo test --locked -p k2-daemon --lib voice::
cargo test --locked -p k2-daemon --lib routes::frame_walk
# Modules of the grouped test binaries, one at a time (each expects a
# process of its own; see crates/k2-daemon/tests/support/env.rs).
for m in voice_admin_space_integration voice_occupancy_fake_sew voice_contract_fake_sew; do
  cargo test --locked -p k2-daemon --test it_chat -- "$m::"
done
cargo test --locked -p k2-daemon --test it_contract -- contract_routes::
cargo build --locked -p k2-ptyd --bins
cargo test --locked -p k2-daemon --test permission_off_golden
echo "GATE voice-phone: OK"
