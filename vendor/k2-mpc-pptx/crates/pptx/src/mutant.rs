//! Planted bugs, for the tests that prove the checks can fail (`tests/mutants.rs`). Without
//! the `mutants` feature, [`active`] is a constant `false` and nothing here exists in a build.
//!
//! The switch is the package layer's (`k2-mpc-opc`, whose `mutants` feature this crate's
//! turns on), so a mutant planted here reaches the package reader, writer and merge too.

#[cfg(feature = "mutants")]
pub use k2_mpc_opc::mutant::with;

#[allow(unused_imports)]
pub(crate) use k2_mpc_opc::mutant::active;
