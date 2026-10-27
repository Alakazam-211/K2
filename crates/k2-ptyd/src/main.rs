//! `k2-ptyd` binary.
//!
//! - `k2-ptyd --launch`: double-fork + setsid, start the server detached,
//!   exit 0 once it listens (3 if one already owns this K2 home).
//! - `k2-ptyd --serve`: run the server in the foreground (tests, debugging).
//! - `k2-ptyd --version`.
//! - `k2-ptyd --selftest --state-format N`: print `ok <version> N` and exit
//!   0 when this build can take over a running ptyd's state (PRD §7.10).
//! - `k2-ptyd --resume-state-fd N --listen-fd M`: the new image after a
//!   self-upgrade exec (never run by hand).
//!
//! The K2 home comes from `K2_HOME` (the daemon always sets it).

#[cfg(unix)]
fn main() {
    use k2_ptyd::launch;
    use k2_ptyd::paths::Paths;
    use k2_ptyd::server::{self, orphan_from_env, Config, EXITED_TTL, IDLE_DEFAULT};
    use std::time::Duration;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    if mode == "--version" {
        println!("k2-ptyd {} proto {}", k2_ptyd::proto::PTYD_VERSION, k2_ptyd::proto::PROTO_MAX);
        return;
    }
    if mode == "--selftest" {
        let format = (args.get(1).map(String::as_str) == Some("--state-format")).then(|| args.get(2).map(String::as_str)).flatten();
        match server::selftest_reply(format) {
            Ok(line) => {
                println!("{line}");
                return;
            }
            Err(e) => {
                eprintln!("k2-ptyd: {e}");
                std::process::exit(launch::EXIT_FAILED);
            }
        }
    }
    // Security review F5: harden before anything else — before the env,
    // the config, the log and, for `--resume-state-fd` after an exec
    // self-upgrade, before the state fd (key, rings) and the inherited
    // master fds are touched. `execve` made this image dumpable again.
    server::harden_process();
    let Some(home) = std::env::var_os("K2_HOME").filter(|h| !h.is_empty()) else {
        eprintln!("k2-ptyd: K2_HOME is required");
        std::process::exit(launch::EXIT_FAILED);
    };
    let home = std::path::PathBuf::from(home);
    if !home.is_absolute() {
        eprintln!("k2-ptyd: K2_HOME must be absolute");
        std::process::exit(launch::EXIT_FAILED);
    }
    let paths = Paths::from_home(&home);
    // Test knob (debug builds only): shorter idle exit.
    let idle = if cfg!(debug_assertions) {
        std::env::var("K2_PTYD_TEST_IDLE_SECS").ok().and_then(|s| s.parse().ok()).map(Duration::from_secs).unwrap_or(IDLE_DEFAULT)
    } else {
        IDLE_DEFAULT
    };
    let cfg = Config {
        paths,
        orphan: orphan_from_env(std::env::var("K2_PTYD_ORPHAN_SECS").ok().as_deref()),
        idle,
        exited_ttl: EXITED_TTL,
        ready_fd: None,
    };
    let code = match mode {
        "--launch" => launch::launcher(cfg),
        "--serve" => exit_code(server::serve(cfg)),
        "--resume-state-fd" => {
            let state_fd = args.get(1).and_then(|s| s.parse().ok());
            let listen_fd = (args.get(2).map(String::as_str) == Some("--listen-fd")).then(|| args.get(3).and_then(|s| s.parse().ok())).flatten();
            match (state_fd, listen_fd) {
                (Some(state), Some(listen)) => exit_code(server::resume(cfg, state, listen)),
                _ => {
                    eprintln!("usage: k2-ptyd --resume-state-fd N --listen-fd M");
                    2
                }
            }
        }
        _ => {
            eprintln!("usage: k2-ptyd --launch | --serve | --version | --selftest --state-format N");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(unix)]
fn exit_code(r: std::io::Result<k2_ptyd::server::Exit>) -> i32 {
    use k2_ptyd::launch;
    use k2_ptyd::server::Exit;
    match r {
        Ok(Exit::Clean) => launch::EXIT_OK,
        Ok(Exit::AlreadyRunning(pid)) => {
            eprintln!("k2-ptyd: already running (pid {})", pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()));
            launch::EXIT_ALREADY_RUNNING
        }
        Err(e) => {
            eprintln!("k2-ptyd: {e}");
            launch::EXIT_FAILED
        }
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("k2-ptyd: Windows support lands in S8");
    std::process::exit(1);
}
