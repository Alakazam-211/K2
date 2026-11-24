//! The helper behind `k2 vault run` (prd-vault-for-agents-v1 A1):
//! `k2-daemon --vault-run --secret NAME … [--env-file F] [--scope S]
//! [--no-mask] -- cmd [args…]`. It never boots the daemon.
//!
//! Only names are on this process's argv. The helper asks the daemon for
//! the values over the session's cell socket (policy, audit and per-run
//! files are the daemon's), sets them on one child, and masks the child's
//! stdout and stderr, because bash can't mask a stream safely.
//!
//! The session's passport reaches the helper on an inherited file
//! descriptor (`--token-fd N`, a pipe the CLI writes), never in its
//! environment. The child gets the secrets, not the agent's identity: the
//! helper clears every K2 credential variable from the child's environment
//! and sets `K2_NO_OWNER_FALLBACK=1`, so a `k2` the child runs refuses
//! rather than read the owner token from disk.

use std::ffi::OsString;

/// The argv mode (`daemon_main` dispatches on it before anything else).
pub const ARG: &str = "--vault-run";

#[cfg(unix)]
pub use imp::run_from_args;
#[cfg(not(unix))]
use k2_core::brand::CLI_NAME;

/// Windows isn't a 0.46.11 gate; ship it when its CI lane passes.
#[cfg(not(unix))]
pub fn run_from_args(_args: &[OsString]) -> i32 {
    eprintln!("run_not_on_windows: {CLI_NAME} vault run needs macOS or Linux for now; use {CLI_NAME} vault env-file or {CLI_NAME} vault file");
    1
}

#[cfg(unix)]
mod imp {
    use super::{OsString, ARG};
    use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
    use k2_core::cli_name;
    use std::ffi::OsStr;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use k2_vault::redact::{wipe, Redactor};
    use serde::Deserialize;
    use serde_json::{json, Value};

    const USAGE: &str = concat!("usage: ", cli_name!(), " vault run [--secret NAME[=ENV_NAME]]... [--env-file .env.k2] [--scope workspace|server|session] [--no-mask] -- cmd [args…]");
    /// The credential variables the child never gets, by name. Any other
    /// `K2_`, `K5_` or `K2SO_` variable whose name holds TOKEN or PASSPORT
    /// goes too ([`private_var`]).
    const CLEARED: &[&str] = &[
        "K2_HOOK_TOKEN",
        "K5_HOOK_TOKEN",
        "K2SO_HOOK_TOKEN",
        "K2_HOOK_TOKEN_FILE",
        "K5_HOOK_TOKEN_FILE",
        "K2_REMOTE_TOKEN",
        "K2_CAPABILITY_TOKEN",
        "K2_OWNER_TOKEN",
    ];
    /// The no-owner guard under its other names: only the `K2_` one is set,
    /// so a `K5_` or `K2SO_` copy can't override it.
    const GUARD_ALIASES: &[&str] = &["K5_NO_OWNER_FALLBACK", "K2SO_NO_OWNER_FALLBACK"];
    const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
    const MAX_ANSWER: usize = 64 * 1024 * 1024;
    /// How long output is still read after the child has exited, for a
    /// background process that kept the pipe open.
    const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);

    #[derive(Debug, Default)]
    struct Args {
        secrets: Vec<(String, Option<String>)>,
        env_file: Option<PathBuf>,
        scope: Option<String>,
        no_mask: bool,
        token_fd: Option<i32>,
        cmd: Vec<OsString>,
    }

    fn parse(argv: &[OsString]) -> Result<Args, String> {
        let mut a = Args::default();
        let mut it = argv.iter().skip(1);
        if it.next().map(OsString::as_os_str) != Some(OsStr::new(ARG)) {
            return Err(USAGE.to_string());
        }
        let mut dashes = false;
        while let Some(flag) = it.next() {
            if flag == "--" {
                dashes = true;
                break;
            }
            let mut value = |name: &str| -> Result<String, String> {
                it.next()
                    .and_then(|v| v.to_str())
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| format!("usage: {name} needs a value\n{USAGE}"))
            };
            match flag.to_str() {
                Some("--secret") => {
                    let v = value("--secret")?;
                    a.secrets.push(match v.split_once('=') {
                        Some((n, e)) => (n.to_string(), Some(e.to_string())),
                        None => (v, None),
                    });
                }
                Some("--env-file") => a.env_file = Some(PathBuf::from(value("--env-file")?)),
                Some("--scope") => a.scope = Some(value("--scope")?),
                Some("--no-mask") => a.no_mask = true,
                Some("--token-fd") => {
                    let v = value("--token-fd")?;
                    let fd = v.parse::<i32>().ok().filter(|n| *n > 2);
                    a.token_fd = Some(fd.ok_or_else(|| format!("usage: --token-fd takes a file descriptor above 2\n{USAGE}"))?);
                }
                _ => return Err(format!("usage: unknown flag ({CLI_NAME} vault run --help)\n{USAGE}")),
            }
        }
        if !dashes {
            return Err(format!("usage: put -- before the command\n{USAGE}"));
        }
        a.cmd = it.cloned().collect();
        if a.cmd.is_empty() {
            return Err(format!("usage: no command after --\n{USAGE}"));
        }
        if a.secrets.is_empty() && a.env_file.is_none() {
            return Err(format!("usage: name at least one secret\n{USAGE}"));
        }
        Ok(a)
    }

    /// `(status, body)` from a one-shot HTTP request on the cell socket.
    fn post(sock: &Path, token: &str, path: &str, body: &str) -> std::io::Result<(u16, Vec<u8>)> {
        let mut s = UnixStream::connect(sock)?;
        s.set_read_timeout(Some(SOCKET_TIMEOUT))?;
        s.set_write_timeout(Some(SOCKET_TIMEOUT))?;
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes())?;
        let mut raw = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        let mut want: Option<usize> = None;
        loop {
            if want.is_none() {
                if let Some(he) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..he]).to_ascii_lowercase();
                    if let Some(n) = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()) {
                        want = Some(he + 4 + n);
                    }
                }
            }
            if want.is_some_and(|w| raw.len() >= w) || raw.len() > MAX_ANSWER {
                break;
            }
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    wipe(&mut raw);
                    wipe(&mut chunk);
                    return Err(e);
                }
            }
        }
        wipe(&mut chunk);
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not an HTTP answer");
        let he = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(bad)?;
        let status = std::str::from_utf8(&raw[..he])
            .ok()
            .and_then(|h| h.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or_else(bad)?;
        let end = want.unwrap_or(raw.len()).min(raw.len());
        let body = raw[he + 4..end].to_vec();
        wipe(&mut raw);
        Ok((status, body))
    }

    /// The vault's exit codes (`_vault_exit_for` in the CLI).
    fn exit_for(status: u16) -> i32 {
        match status {
            400 | 413 | 422 => 2,
            401 | 403 => 3,
            404 => 4,
            _ => 1,
        }
    }

    /// A refusal, printed as the CLI prints one: `code: hint`, then the
    /// permission card's closing line when the daemon sent one.
    fn print_refusal(body: &[u8]) {
        let Ok(v) = serde_json::from_slice::<Value>(body) else {
            eprintln!("{}", String::from_utf8_lossy(&body[..body.len().min(300)]));
            return;
        };
        let e = &v["error"];
        match e.as_object() {
            Some(_) => eprintln!("{}: {}", e["code"].as_str().unwrap_or("error"), e["hint"].as_str().unwrap_or("")),
            None => eprintln!("{}", e.as_str().unwrap_or("the daemon refused")),
        }
        let p = &v["permission"];
        if let Some(cmd) = p["command"].as_str().filter(|c| !c.is_empty()) {
            eprintln!("Ask with: {cmd}");
        } else if p["requestable"] == json!(false) {
            if let Some(h) = p["hint"].as_str().filter(|h| !h.is_empty()) {
                eprintln!("{h}");
            }
            eprintln!("It can't be requested.");
            // Only while the daemon says agents may report (0.46.18).
            if p["feedback"] == json!(true) {
                eprintln!("{}", k2_core::permission::request::CLOSING_FEEDBACK);
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Answer {
        run_id: String,
        run_dir: Option<String>,
        items: Vec<Item>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Item {
        name: String,
        env: String,
        how: String,
        value_b64: Option<String>,
        mask_b64: Option<String>,
        path: Option<String>,
    }

    fn decode(v: Option<String>) -> Option<Vec<u8>> {
        let s = v?;
        let d = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).ok();
        let mut bytes = s.into_bytes();
        wipe(&mut bytes);
        d
    }

    fn file_name(cmd: &OsStr) -> String {
        Path::new(cmd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn remove_run_dir(answer_dir: Option<&str>, run_id: &str) {
        // Only ever the folder this run was given.
        if let Some(d) = answer_dir.map(Path::new).filter(|d| d.is_absolute() && d.ends_with(Path::new("run").join(run_id))) {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    // ── signals ─────────────────────────────────────────────────────────

    const FORWARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    /// Where a forwarded signal goes: `-pgid`, the child's process group, or
    /// the child's pid while it shares our terminal. 0 until it exists.
    static SIGNAL_TARGET: AtomicI32 = AtomicI32::new(0);

    extern "C" fn forward(sig: libc::c_int) {
        let target = SIGNAL_TARGET.load(Ordering::SeqCst);
        if target != 0 {
            // SAFETY: kill(2) is async-signal-safe.
            unsafe { libc::kill(target, sig) };
        }
    }

    /// A signal the terminal already sent to the child too: stay alive for
    /// the child's exit code. A handler, not SIG_IGN, which exec would pass
    /// on to the child.
    extern "C" fn stay_alive(_sig: libc::c_int) {}

    fn signal_set() -> libc::sigset_t {
        // SAFETY: the set is initialised by sigemptyset before any use.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for s in FORWARDED {
                libc::sigaddset(&mut set, s);
            }
            set
        }
    }

    /// Held until the child exists, so a signal that lands early is
    /// delivered to it rather than killing us with its files on disk.
    fn block_signals() {
        let set = signal_set();
        // SAFETY: valid set; a null old-set is allowed.
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    }

    fn unblock_signals() {
        let set = signal_set();
        // SAFETY: valid set; a null old-set is allowed.
        unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut()) };
    }

    fn install_handlers(shares_terminal: bool) {
        for s in FORWARDED {
            let handler: extern "C" fn(libc::c_int) = if shares_terminal && s != libc::SIGTERM { stay_alive } else { forward };
            // SAFETY: the struct is zeroed then filled; the handler only
            // calls kill(2).
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = handler as usize;
                sa.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(s, &sa, std::ptr::null_mut());
            }
        }
    }

    fn foreground_tty(fd: libc::c_int) -> bool {
        // SAFETY: isatty, tcgetpgrp and getpgrp take plain values.
        unsafe { libc::isatty(fd) == 1 && libc::tcgetpgrp(fd) == libc::getpgrp() }
    }

    // ── masking ─────────────────────────────────────────────────────────

    /// Copies one of the child's pipes to ours through the mask. A value
    /// split across two reads is still masked (the stream holds a tail).
    /// When our side closes, the pipe closes too, so the child gets its
    /// SIGPIPE as it would have.
    fn pump<R: Read + AsRawFd, W: Write>(mut src: R, mut dst: W, mask: &Redactor, exited: &OnceLock<Instant>) {
        let mut stream = mask.stream();
        let mut buf = vec![0u8; 64 * 1024];
        let fd = src.as_raw_fd();
        loop {
            if exited.get().is_some_and(|t| t.elapsed() >= DRAIN_AFTER_EXIT) {
                break;
            }
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for the length given.
            let r = unsafe { libc::poll(&mut p, 1, 50) };
            if r == 0 || (r < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted) {
                continue;
            }
            if r < 0 {
                break;
            }
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let out = stream.push(&buf[..n]);
            wipe(&mut buf[..n]);
            if dst.write_all(&out).and_then(|()| dst.flush()).is_err() {
                return;
            }
        }
        let _ = dst.write_all(&stream.finish());
        let _ = dst.flush();
    }

    // ── the run ─────────────────────────────────────────────────────────

    pub fn run_from_args(argv: &[OsString]) -> i32 {
        let args = match parse(argv) {
            Ok(a) => a,
            Err(msg) => {
                eprintln!("{msg}");
                return 2;
            }
        };
        let env_str = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        // The passport: from --token-fd (the CLI's pipe), else, from an
        // older CLI, $K2_HOOK_TOKEN. Either way the child never gets it.
        let token = match args.token_fd {
            Some(fd) => match read_token_fd(fd) {
                Ok(t) => Some(t).filter(|t| !t.is_empty()),
                Err(e) => {
                    eprintln!("run_failed: can't read the passport on fd {fd} ({})", e.kind());
                    return 1;
                }
            },
            None => env_str("K2_HOOK_TOKEN"),
        };
        let (Some(sock), Some(token)) = (env_str("K2_HOOK_SOCK"), token) else {
            eprintln!("use_run_as_agent: {CLI_NAME} vault run runs inside a {PRODUCT_NAME} session (it needs $K2_HOOK_SOCK and the session's passport); people use {CLI_NAME} vault reveal");
            return 3;
        };
        block_signals();
        match request(&args, Path::new(&sock), &token) {
            Ok(code) => code,
            Err(code) => code,
        }
    }

    /// The passport on an inherited descriptor, read to its end and closed,
    /// so the child never inherits it. One trailing newline is dropped.
    fn read_token_fd(fd: i32) -> std::io::Result<String> {
        use std::os::fd::FromRawFd;
        // SAFETY: the CLI opened this descriptor for us (a pipe on fd 3) and
        // nothing else in this process owns it; the File closes it on drop.
        let f = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut buf = Vec::new();
        // `take` owns the File; it closes the descriptor at the end of this
        // statement, before any child is started.
        f.take(64 * 1024).read_to_end(&mut buf)?;
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        String::from_utf8(buf).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))
    }

    /// A K2 credential variable the child never gets: one of [`CLEARED`],
    /// or a `K2_`, `K5_` or `K2SO_` name holding TOKEN or PASSPORT.
    pub(super) fn private_var(name: &OsStr) -> bool {
        let Some(n) = name.to_str() else { return false };
        if CLEARED.contains(&n) {
            return true;
        }
        let k2 = n.starts_with("K2_") || n.starts_with("K5_") || n.starts_with("K2SO_");
        k2 && (n.contains("TOKEN") || n.contains("PASSPORT"))
    }

    fn request(args: &Args, sock: &Path, token: &str) -> Result<i32, i32> {
        let env_file = match &args.env_file {
            None => None,
            Some(p) => {
                let cwd = std::env::current_dir().map_err(|e| {
                    eprintln!("{CLI_NAME} vault run: can't read the current folder ({})", e.kind());
                    1
                })?;
                let abs = cwd.join(p);
                Some(abs.to_str().map(str::to_string).ok_or_else(|| {
                    eprintln!("usage: the --env-file path isn't UTF-8");
                    2
                })?)
            }
        };
        let secrets: Vec<Value> = args
            .secrets
            .iter()
            .map(|(n, e)| match e {
                Some(e) => json!({ "name": n, "env": e }),
                None => json!({ "name": n }),
            })
            .collect();
        let cmd = file_name(&args.cmd[0]);
        let mut body = json!({ "secrets": secrets, "cmd": cmd, "noMask": args.no_mask });
        if let Some(f) = env_file {
            body["envFile"] = json!(f);
        }
        if let Some(s) = &args.scope {
            body["scope"] = json!(s);
        }
        let (status, mut raw) = post(sock, token, "/cli/vault/run", &body.to_string()).map_err(|e| {
            eprintln!("run_unavailable: can't reach the {PRODUCT_NAME} daemon on $K2_HOOK_SOCK ({}); is this session still running?", e.kind());
            1
        })?;
        if status != 200 {
            print_refusal(&raw);
            wipe(&mut raw);
            return Err(exit_for(status));
        }
        let parsed = serde_json::from_slice::<Answer>(&raw);
        wipe(&mut raw);
        let answer = parsed.map_err(|_| {
            eprintln!("run_failed: the daemon's answer wasn't understood; is the {CLI_NAME} CLI the same version as the daemon?");
            1
        })?;
        Ok(launch(args, answer))
    }

    fn launch(args: &Args, answer: Answer) -> i32 {
        let Answer { run_id, run_dir, items } = answer;
        let fail = |code: i32, msg: String| -> i32 {
            remove_run_dir(run_dir.as_deref(), &run_id);
            eprintln!("{msg}");
            code
        };

        let mut child_env: Vec<(String, Vec<u8>)> = Vec::with_capacity(items.len());
        // (secret name, value) for the mask: this run's values only.
        let mut masked: Vec<(String, Vec<u8>)> = Vec::new();
        for it in items {
            match it.how.as_str() {
                "env" => {
                    let Some(v) = decode(it.value_b64) else { return fail(1, format!("run_failed: bad value for {}", it.name)) };
                    masked.push((it.name, v.clone()));
                    child_env.push((it.env, v));
                }
                "file" => {
                    let Some(path) = it.path else { return fail(1, format!("run_failed: no file for {}", it.name)) };
                    if let Some(v) = decode(it.mask_b64) {
                        masked.push((it.name, v));
                    }
                    child_env.push((it.env, path.into_bytes()));
                }
                "plain" => {
                    let Some(v) = decode(it.value_b64) else { return fail(1, format!("run_failed: bad value for {}", it.name)) };
                    child_env.push((it.env, v));
                }
                // A newer daemon's delivery this helper can't honour.
                _ => return fail(1, format!("run_failed: this helper can't deliver {} that way; update {PRODUCT_NAME}", it.name)),
            }
        }

        // Sharing the foreground terminal, the child must stay in our process
        // group or it stops the moment it reads the terminal; the terminal's
        // own ^C and ^Z then reach us both, as they would any command.
        // --no-mask gives the child the terminal on stdout and stderr too.
        let shares_terminal = foreground_tty(0) || (args.no_mask && (foreground_tty(1) || foreground_tty(2)));

        // `k2 vault run` runs inside the caller's terminal session (the CLI
        // started this helper there, not the daemon): its program may read
        // the terminal, so it stays in that session.
        let mut cmd = k2_core::bg_proc::command_keeping_session(&args.cmd[0]);
        cmd.args(&args.cmd[1..]);
        // The child gets the secrets, not the agent's identity: no K2
        // credential variable, and the no-owner guard, so a k2 it runs
        // refuses rather than read the owner token from disk.
        for (k, _) in std::env::vars_os() {
            if private_var(&k) || GUARD_ALIASES.iter().any(|a| k.as_os_str() == OsStr::new(a)) {
                cmd.env_remove(&k);
            }
        }
        cmd.env(k2_core::desktop::virtual_desktop::NO_OWNER_FALLBACK, "1");
        for (k, v) in &child_env {
            cmd.env(k, OsStr::from_bytes(v));
        }
        if !args.no_mask {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        if !shares_terminal {
            cmd.process_group(0);
        }
        // std passes the signal mask on to a child, so without this the
        // command would start with INT, TERM and HUP blocked for good.
        // SAFETY: runs between fork and exec; signal_set and pthread_sigmask
        // don't allocate or take locks.
        unsafe {
            cmd.pre_exec(|| {
                let set = signal_set();
                match libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut()) {
                    0 => Ok(()),
                    e => Err(std::io::Error::from_raw_os_error(e)),
                }
            });
        }
        install_handlers(shares_terminal);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let (code, why) = match e.kind() {
                    std::io::ErrorKind::NotFound => (127, "command not found"),
                    std::io::ErrorKind::PermissionDenied => (126, "permission denied"),
                    _ => (126, "could not start it"),
                };
                return fail(code, format!("{CLI_NAME} vault run: {}: {why}", args.cmd[0].to_string_lossy()));
            }
        };
        // Copies std made for the child are not reachable; ours are wiped.
        drop(cmd);
        for (_, v) in &mut child_env {
            wipe(v);
        }

        let pid = child.id() as libc::c_int;
        SIGNAL_TARGET.store(if shares_terminal { pid } else { -pid }, Ordering::SeqCst);
        unblock_signals();

        let mask = if args.no_mask { Redactor::empty() } else { Redactor::new(masked.iter().map(|(n, v)| (n.as_str(), v.as_slice()))) };
        for (_, v) in &mut masked {
            wipe(v);
        }
        let out = child.stdout.take();
        let err = child.stderr.take();
        let exited: OnceLock<Instant> = OnceLock::new();
        let status = std::thread::scope(|s| {
            if let Some(o) = out {
                s.spawn(|| pump(o, std::io::stdout(), &mask, &exited));
            }
            if let Some(e) = err {
                s.spawn(|| pump(e, std::io::stderr(), &mask, &exited));
            }
            let st = child.wait();
            let _ = exited.set(Instant::now());
            st
        });
        SIGNAL_TARGET.store(0, Ordering::SeqCst);
        drop(mask);
        remove_run_dir(run_dir.as_deref(), &run_id);
        match status {
            Ok(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
            Err(e) => {
                eprintln!("{CLI_NAME} vault run: lost the child ({})", e.kind());
                1
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn argv(a: &[&str]) -> Vec<OsString> {
            let mut v = vec![OsString::from("k2-daemon"), OsString::from(ARG)];
            v.extend(a.iter().map(OsString::from));
            v
        }

        #[test]
        fn flags_stop_at_the_dashes() {
            let a = parse(&argv(&["--secret", "A", "--secret", "B=MY_B", "--no-mask", "--", "grep", "--secret", "x", "--help"])).expect("parse");
            assert_eq!(a.secrets, vec![("A".to_string(), None), ("B".to_string(), Some("MY_B".to_string()))]);
            assert!(a.no_mask);
            assert_eq!(a.cmd, vec![OsString::from("grep"), "--secret".into(), "x".into(), "--help".into()]);
        }

        #[test]
        fn a_bad_command_line_is_usage() {
            for bad in [&["--secret", "A"][..], &["--secret", "A", "--"][..], &["--", "ls"][..], &["--secret", "A", "--bogus", "--", "ls"][..]] {
                let e = parse(&argv(bad)).expect_err("refused");
                assert!(e.contains("usage:"), "{e}");
            }
        }

        #[test]
        fn a_bad_flag_is_not_echoed() {
            // Someone typing a value where a flag goes must not see it logged.
            let e = parse(&argv(&["sk-not-a-flag", "--secret", "A", "--", "ls"])).expect_err("refused");
            assert!(!e.contains("sk-not-a-flag"), "{e}");
        }

        #[test]
        fn the_token_fd_is_a_flag_before_the_dashes() {
            let a = parse(&argv(&["--token-fd", "3", "--secret", "A", "--", "env"])).expect("parse");
            assert_eq!(a.token_fd, Some(3));
            for bad in ["0", "2", "x", "-1"] {
                let e = parse(&argv(&["--token-fd", bad, "--secret", "A", "--", "env"])).expect_err("refused");
                assert!(e.contains("--token-fd"), "{e}");
            }
        }

        #[test]
        fn credential_variables_are_private() {
            for n in ["K2_HOOK_TOKEN", "K5_HOOK_TOKEN", "K2SO_HOOK_TOKEN", "K2_HOOK_TOKEN_FILE", "K2_OWNER_TOKEN", "K2_SOME_PASSPORT", "K5_NEW_TOKEN_X"] {
                assert!(private_var(OsStr::new(n)), "{n}");
            }
            for n in ["K2_HOOK_SOCK", "K2_SECRETS_DIR", "K2_PROJECT_PATH", "GITHUB_TOKEN", "MY_K2_TOKEN", "PATH"] {
                assert!(!private_var(OsStr::new(n)), "{n}");
            }
        }

        #[test]
        fn exit_codes_match_the_cli() {
            assert_eq!((exit_for(400), exit_for(403), exit_for(404), exit_for(500)), (2, 3, 4, 1));
        }
    }
}
