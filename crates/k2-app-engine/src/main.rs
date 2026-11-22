//! `k2-app-engine serve | version | process-run <app-id>`.

fn main() {
    // The brand mark: static data a release reads (scripts/brand/check-brand-mark.py).
    k2_brand::mark::keep();
    #[cfg(unix)]
    imp::main();
    #[cfg(not(unix))]
    {
        eprintln!("k2-app-engine: server-only feature (Linux boxes); not shipped on personal Windows machines");
        std::process::exit(2);
    }
}

// server-only feature (Linux boxes); not shipped on personal Windows machines
#[cfg(unix)]
#[path = "main_unix.rs"]
mod imp;
