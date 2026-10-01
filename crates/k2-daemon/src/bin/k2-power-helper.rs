//! `/Library/PrivilegedHelperTools/dev.k2.power-helper` — the macOS root
//! door for heartbeat wake events (S2) and the lid-closed keep-awake (S6).
//!
//! Allowlist and IO live in `k2_daemon::power::helper`. Unknown argv
//! exits non-zero before anything changes. Installed by one admin dialog
//! the first time "Wake this computer for heartbeats" is turned on; run
//! by the daemon with `sudo -n`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match k2_daemon::power::helper::parse_argv(&refs)
        .and_then(k2_daemon::power::helper::execute)
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    };
    std::process::exit(code);
}
