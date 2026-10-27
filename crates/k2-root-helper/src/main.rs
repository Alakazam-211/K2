//! `/usr/local/libexec/k2-root-helper`. Not setuid; the K2 daemon user runs
//! it only through `sudo -n` (one sudoers line). Everything is in the
//! library so it is unit-tested over a temp root.

fn main() {
    #[cfg(unix)]
    imp::main();
    #[cfg(not(unix))]
    {
        eprintln!("k2-root-helper: server-only feature (Linux boxes); not shipped on personal Windows machines");
        std::process::exit(2);
    }
}

// server-only feature (Linux boxes); not shipped on personal Windows machines
#[cfg(unix)]
#[path = "main_unix.rs"]
mod imp;
