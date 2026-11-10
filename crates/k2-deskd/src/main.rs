//! `k2-deskd`: see the crate docs. No arguments change what it does:
//! `--version` prints the build and protocol (the release AMFI launch check
//! runs it) and `--tcc-probe` prints this process's two TCC flags (deskd's
//! own live re-check uses it; see `mac::tcc`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("k2-deskd {} (protocol {})", k2_deskd_proto::VERSION, k2_deskd_proto::PROTOCOL_VERSION);
            return;
        }
        #[cfg(target_os = "macos")]
        Some("--tcc-probe") => {
            println!("{}", k2_deskd::mac::tcc::probe_line());
            return;
        }
        Some(other) => {
            eprintln!("k2-deskd takes no arguments (got {other:?}); the daemon starts it");
            std::process::exit(2);
        }
        None => {}
    }
    #[cfg(target_os = "macos")]
    k2_deskd::mac::run();
    #[cfg(target_os = "linux")]
    k2_deskd::linux::run();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("k2-deskd runs on macOS and Linux only in v1");
        std::process::exit(2);
    }
}
