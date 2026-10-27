//! TEST ONLY (`--features test-keys`): sign helper builds with the TEST key
//! for the Linux integration run (tests/linux/root_helper_integration.sh).
//!
//!   k2-root-helper-testsign sig <elf> <version> <protocol> <arch>   → minisign text
//!   k2-root-helper-testsign payload <elf> <version> <protocol> <arch> → self-update stdin
//!   k2-root-helper-testsign payload-raw <elf> <sigfile>              → self-update stdin
//!   k2-root-helper-testsign engine-sig <elf> <version> <protocol> <arch>     → k2-app-engine .sig
//!   k2-root-helper-testsign engine-payload <elf> <version> <protocol> <arch> → install-engine stdin
//!   k2-root-helper-testsign verify <pubkey-b64> <elf> <sigfile> <arch>
//!       (verify any minisign signature — e.g. one made by the real
//!        `minisign -S -t` CLI — through the same check the helper runs)

fn main() {
    #[cfg(unix)]
    imp::main();
    #[cfg(not(unix))]
    {
        eprintln!("k2-root-helper-testsign: server-only feature (Linux boxes); not shipped on personal Windows machines");
        std::process::exit(2);
    }
}

// server-only feature (Linux boxes); not shipped on personal Windows machines
#[cfg(unix)]
#[path = "unix/testsign.rs"]
mod imp;
