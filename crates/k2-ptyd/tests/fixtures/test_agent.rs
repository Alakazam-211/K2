//! Fake agent for k2-ptyd tests (PRD §12 harness). Modes:
//! - `probe <out.json>`: record what the child sees (session, ctty, fds,
//!   signal dispositions + mask, cwd, env, IUTF8), print PROBE_DONE, sleep.
//! - `burst <mib>`: write <mib> MiB of `a..z` (byte i = b'a' + i % 26), exit 0.
//! - `seq`: print `seq=<n> ` every 10 ms forever.
//! - `echo`: for each input line print `got:<line>`.
//! - `hup-ignore`: ignore SIGHUP, print READY, sleep forever.
//! - `sleep`: print READY, sleep forever.
//! - `exit <code>`: print BYE, exit with <code>.

//!
//! `#![no_main]`: Rust's normal startup ignores SIGPIPE and installs
//! SIGSEGV/SIGBUS handlers before `main`, which would hide what ptyd handed
//! the child. With a C `main` the probe sees the inherited dispositions.

#![no_main]

use std::io::{BufRead, Write};

#[no_mangle]
pub extern "C" fn main(_argc: libc::c_int, _argv: *const *const libc::c_char) -> libc::c_int {
    run();
    0
}

fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let mut out = std::io::stdout();
    match mode {
        "probe" => {
            let path = &args[1];
            std::fs::write(path, probe()).expect("write probe");
            println!("PROBE_DONE");
            out.flush().unwrap();
            sleep_forever();
        }
        "burst" => {
            let mib: usize = args[1].parse().unwrap();
            let mut written = 0usize;
            let total = mib << 20;
            while written < total {
                let n = (total - written).min(1 << 20);
                let start = written % 26;
                let piece: Vec<u8> = (0..n).map(|i| b'a' + ((start + i) % 26) as u8).collect();
                out.write_all(&piece).unwrap();
                written += n;
            }
            out.flush().unwrap();
        }
        "seq" => {
            let mut n = 0u64;
            loop {
                write!(out, "seq={n} ").unwrap();
                out.flush().unwrap();
                n += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        "echo" => {
            println!("READY");
            out.flush().unwrap();
            for line in std::io::stdin().lock().lines() {
                let line = line.unwrap();
                println!("got:{line}");
                out.flush().unwrap();
            }
        }
        "hup-ignore" => {
            // SAFETY: setting a disposition.
            unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
            println!("READY");
            out.flush().unwrap();
            sleep_forever();
        }
        "survive" => survive(&args[1], args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50)),
        "sleep" => {
            println!("READY");
            out.flush().unwrap();
            sleep_forever();
        }
        "exit" => {
            println!("BYE");
            out.flush().unwrap();
            std::process::exit(args[1].parse().unwrap());
        }
        other => {
            eprintln!("unknown mode {other:?}");
            std::process::exit(2);
        }
    }
}

static SIGHUPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

extern "C" fn on_hup(_: libc::c_int) {
    SIGHUPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Daemon survival harness agent (PRD §12, T1). In `<dir>`:
/// - `pid`: this process's pid;
/// - stdout: `seq=<n>` lines every `<ms>` ms, forever;
/// - `hook.log`: one line per second, `<unix_ms> <http status|ERR:...>` for
///   `GET /cli/whoami` over `$K2_HOOK_SOCK` with `$K2_HOOK_TOKEN`;
/// - `stdin`: every byte received on stdin (raw mode, no echo) — the agent
///   never sends a terminal query, so any byte here is unsolicited (P75)
///   unless the test typed it;
/// - `sighup`: the number of SIGHUPs received (written when non-zero).
fn survive(dir: &str, every_ms: u64) {
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: plain libc calls on our own process / tty.
    unsafe {
        libc::signal(libc::SIGHUP, on_hup as extern "C" fn(libc::c_int) as libc::sighandler_t);
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
    // The vault's per-session secrets dir this agent was handed (if any).
    std::fs::write(dir.join("secrets_dir"), std::env::var("K2_SECRETS_DIR").unwrap_or_default()).unwrap();
    std::fs::write(dir.join("pid"), std::process::id().to_string()).unwrap();
    {
        let dir = dir.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("stdin")).unwrap();
            let mut buf = [0u8; 256];
            loop {
                match std::io::stdin().read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        f.write_all(&buf[..n]).unwrap();
                        f.flush().unwrap();
                    }
                }
            }
        });
    }
    {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("hook.log")).unwrap();
            loop {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
                let r = hook_call();
                writeln!(log, "{now} {r}").unwrap();
                log.flush().unwrap();
                let hups = SIGHUPS.load(std::sync::atomic::Ordering::SeqCst);
                if hups > 0 {
                    std::fs::write(dir.join("sighup"), hups.to_string()).unwrap();
                }
                // `thread-req` = "<addr>\n<text>": post it to that Thread as
                // this agent (its passport), log `<text> <status>`.
                if let Ok(req) = std::fs::read_to_string(dir.join("thread-req")) {
                    let _ = std::fs::remove_file(dir.join("thread-req"));
                    if let Some((addr, text)) = req.split_once('\n') {
                        let form = format!("addr={}&text={}&via=thread", form_enc(addr.trim()), form_enc(text.trim()));
                        let r = cell_request("POST", "/cli/thread/post", Some(&form));
                        let mut tl = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("thread.log")).unwrap();
                        writeln!(tl, "{} {r}", text.trim()).unwrap();
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }
    let mut out = std::io::stdout();
    let mut n = 0u64;
    loop {
        // CRLF: raw mode turned output post-processing off.
        if write!(out, "seq={n}\r\n").and_then(|_| out.flush()).is_err() {
            sleep_forever();
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(every_ms));
    }
}

/// `GET /cli/whoami` over the per-cell socket; the status code or `ERR:…`.
fn hook_call() -> String {
    cell_request("GET", "/cli/whoami", None)
}

/// One request over `$K2_HOOK_SOCK` with `$K2_HOOK_TOKEN` (form body for a
/// POST); the status code or `ERR:…`.
fn cell_request(method: &str, path: &str, form: Option<&str>) -> String {
    use std::io::Read;
    let (Ok(sock), Ok(token)) = (std::env::var("K2_HOOK_SOCK"), std::env::var("K2_HOOK_TOKEN")) else {
        return "ERR:no_passport".into();
    };
    let mut s = match std::os::unix::net::UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(e) => return format!("ERR:connect:{}", e.kind()),
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    let body = form.unwrap_or("");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: k2\r\nAuthorization: Bearer {token}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if let Err(e) = s.write_all(head.as_bytes()) {
        return format!("ERR:write:{}", e.kind());
    }
    let mut raw = Vec::new();
    if let Err(e) = s.read_to_end(&mut raw) {
        if raw.is_empty() {
            return format!("ERR:read:{}", e.kind());
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text.split_whitespace().nth(1).unwrap_or("ERR:empty").to_string();
    if method == "POST" && !status.starts_with('2') {
        // Failed writes carry the daemon's reason (one line) for the log.
        let body: String = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("").chars().take(200).collect();
        return format!("{status} {}", body.replace('\n', " "));
    }
    status
}

fn form_enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn sleep_forever() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn probe() -> String {
    // SAFETY: plain libc queries about this process.
    unsafe {
        let pid = libc::getpid();
        let sid = libc::getsid(0);
        let pgid = libc::getpgrp();
        let tpgid = libc::tcgetpgrp(0);
        let isatty = libc::isatty(0) == 1;
        let mut fds = Vec::new();
        for fd in 0..4096 {
            if libc::fcntl(fd, libc::F_GETFD) != -1 {
                fds.push(fd);
            }
        }
        let mut ignored = Vec::new();
        let mut handled = Vec::new();
        let last = if cfg!(target_os = "linux") { 64 } else { 31 };
        for sig in 1..=last {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig, std::ptr::null(), &mut old) == 0 {
                if old.sa_sigaction == libc::SIG_IGN {
                    ignored.push(sig);
                } else if old.sa_sigaction != libc::SIG_DFL {
                    handled.push(sig);
                }
            }
        }
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, std::ptr::null(), &mut mask);
        let mut blocked = Vec::new();
        for sig in 1..=last {
            if libc::sigismember(&mask, sig) == 1 {
                blocked.push(sig);
            }
        }
        let mut t: libc::termios = std::mem::zeroed();
        libc::tcgetattr(0, &mut t);
        let iutf8 = t.c_iflag & libc::IUTF8 != 0;
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.sort();
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let v = serde_json::json!({
            "pid": pid, "sid": sid, "pgid": pgid, "tpgid": tpgid, "isatty": isatty,
            "fds": fds, "ignored": ignored, "handled": handled, "blocked": blocked,
            "iutf8": iutf8, "env": env, "cwd": cwd,
        });
        v.to_string()
    }
}
