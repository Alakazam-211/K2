//! The native engine Beta's planted faults (test builds only), each caught by its row in
//! `session/native_beta_tests.rs`: named by `MPC_BETA_PLANT` at run time (unset: none). A plant
//! that fires says so on stderr once, so a planted run's log shows the plant was in place.
//! Never in a build: outside test builds nothing is planted.

/// Whether the plant `name` is installed (`MPC_BETA_PLANT=name`).
#[cfg(test)]
pub(crate) fn planted(name: &str) -> bool {
    static NOTED: std::sync::Once = std::sync::Once::new();
    let on = std::env::var("MPC_BETA_PLANT").as_deref() == Ok(name);
    if on {
        NOTED.call_once(|| eprintln!("mpc-beta-plant: plant installed and reached: {name}"));
    }
    on
}

/// Outside test builds: nothing is planted.
#[cfg(not(test))]
#[inline]
pub(crate) fn planted(_: &str) -> bool {
    false
}
