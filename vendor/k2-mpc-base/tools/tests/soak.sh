# The 32-participant soak (crates/base/tests/soak.rs): the full run, in release, SOAK_SECS
# (default 600) with SOAK_SEED (default 1), then the CI variant, then the seed-3 replay. A
# release soak runs this file whole (every step), not soak_32 alone. Ships: Cargo.toml Cargo.lock
# crates/base. Needs Rust. Prints a progress line every SOAK_PROGRESS_SECS (default 30).
timed build cargo test -p k2-mpc-base --release --test soak --no-run
# SOAK_MAX_SLOWDOWN (default 8 here; the test's own default is 2): the throughput guard, in the
# run's RUNREC as max_slowdown_percent
timed soak_32 env SOAK_SECS=${SOAK_SECS:-600} SOAK_SEED=${SOAK_SEED:-1} SOAK_MAX_SLOWDOWN=${SOAK_MAX_SLOWDOWN:-8} \
  cargo test -p k2-mpc-base --release --test soak -- --ignored --nocapture soak_32
timed soak_short cargo test -p k2-mpc-base --release --test soak -- --nocapture soak_short
# the D1 replay regression: seed 3, 27,000 calls, no known D1 violation (its one, at call
# 25,877, was fixed by v0.9.7's D1 amendment) and no other failure
timed soak_replay cargo test -p k2-mpc-base --release --test soak -- --ignored --nocapture soak_replay_seed_3_no_d1
