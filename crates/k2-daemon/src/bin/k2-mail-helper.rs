//! `/usr/local/libexec/k2-mail-helper` — the only root door for hostmail.
//!
//! Allowlist and IO live in `k2_daemon::mail::helper`. Unknown argv exits
//! non-zero before any filesystem change. Stdin is never written to the log.

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let cmd = k2_daemon::mail::helper::parse_argv(&refs)?;
    let stdin = if cmd.reads_stdin() {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)
            .map_err(|_| "mail helper: read stdin failed".to_string())?;
        buf
    } else {
        Vec::new()
    };
    k2_daemon::mail::helper::execute(cmd, &stdin)
}
