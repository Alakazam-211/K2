# The 32-participant soak (crates/base/tests/soak.rs): the full run, in release, SOAK_SECS
# (default 600) with SOAK_SEED (default 1), then the CI variant. Ships: Cargo.toml Cargo.lock
# crates/base. Needs Rust. Prints a progress line every SOAK_PROGRESS_SECS (default 30).
timed build cargo test -p k2-mpc-base --release --test soak --no-run
timed soak_32 env SOAK_SECS=${SOAK_SECS:-600} SOAK_SEED=${SOAK_SEED:-1} \
  cargo test -p k2-mpc-base --release --test soak -- --ignored --nocapture soak_32
timed soak_short cargo test -p k2-mpc-base --release --test soak -- --nocapture soak_short
# the D1 replay regression (v0.9.5): seed 3's one known D1 violation and nothing else
timed soak_replay cargo test -p k2-mpc-base --release --test soak -- --ignored --nocapture soak_replay_seed_3_known_d1
