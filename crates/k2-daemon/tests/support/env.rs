//! The runner guard for the grouped test binaries (`tests/it_*/main.rs`).
//!
//! Each group holds what used to be many test binaries, one module each.
//! Those tests were written for a process of their own: they change `$HOME`
//! and `K2_*` env, write to the shared test DB (`k2_core::db::init_for_tests`)
//! and fill process-wide statics.
//!
//! - Under cargo-nextest (CI, `k2 compute test`, the Linux gate and soak)
//!   every test runs in its own process, so a module boundary changes
//!   nothing.
//! - Under plain `cargo test` the modules of a group would share one
//!   process. A run filtered to one module (`cargo test -p k2-daemon --test
//!   it_apps -- app_grace_integration::`) holds exactly the tests the old
//!   binary held, and the filter skips `runner_guard`. A whole-binary run
//!   mixes modules, so `runner_guard` runs and fails the run with the fix
//!   in its message, whatever the other tests did.

/// Fails unless this process was started by cargo-nextest, which sets
/// `NEXTEST=1` in every test process.
pub fn require_nextest() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    let bin = env!("CARGO_CRATE_NAME");
    panic!(
        "{bin} holds many test modules that each expect a process of their own \
         ($HOME, K2_* env, the shared test DB, statics). Run it with cargo-nextest: \
         `cargo nextest run -p k2-daemon --test {bin}`. With plain cargo test, run \
         one module at a time: `cargo test -p k2-daemon --test {bin} -- <module>::`."
    );
}
