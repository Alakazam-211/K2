//! Fake agent for k2-ptyd tests (PRD §12 harness). Modes:
//! - `probe <out.json>`: record what the child sees (session, ctty, fds,
//!   signal dispositions + mask, cwd, env, IUTF8), print PROBE_DONE, sleep.
//! - `burst <mib>`: write <mib> MiB of `a..z` (byte i = b'a' + i % 26), exit 0.
//! - `seq`: print `seq=<n> ` every 10 ms forever.
//! - `echo`: for each input line print `got:<line>`.
//! - `hup-ignore`: ignore SIGHUP, print READY, sleep forever.
//! - `sleep`: print READY, sleep forever.
//! - `exit <code>`: print BYE, exit with <code>.

use std::io::{BufRead, Write};

fn main() {
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
