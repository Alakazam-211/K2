//! Exec self-upgrade, Unix (PRD §7.10, P63, P64).
//!
//! On `upgrade{exe}` ptyd:
//! 1. runs `exe --selftest --state-format 1` (10 s bound; it must exit 0
//!    and print `ok <version> 1`) and refuses a downgrade;
//! 2. takes any input already queued on attached data streams, then
//!    serializes its full state — every session with its master fd, child
//!    pid, state, ring at exact offsets and pending input, exited-unacked
//!    records, held fds by name, the boot id and the key — to a memfd
//!    (Linux) or an unlinked 0600 file in `<K2_HOME>/run/ptyd/` (macOS, or
//!    a Linux kernel without memfd);
//! 3. answers `ok`, closes every client connection, and clears
//!    `FD_CLOEXEC` on the masters, the held fds, the `ctl.sock` listener,
//!    the lock file and the state fd;
//! 4. `execve(exe, ["k2-ptyd","--resume-state-fd",N,"--listen-fd",M])`
//!    with its own allowlisted environment (P5).
//!
//! The pid, the fds and the parent/child links are kept, so `waitpid`
//! still works; the new image ([`resume`]) sets CLOEXEC back on everything
//! it inherited, re-hardens itself (exec resets dumpability) and reaps any
//! child that exited during the gap. The boot id and the key are the same,
//! so the daemon reconnects (the listener never closed) to "the same ptyd".
//!
//! A failure before `ok` answers `upgrade_failed` and changes nothing. An
//! `execve` failure after `ok` restores CLOEXEC and keeps serving the old
//! image: clients reconnect and see the old version (P64).

use std::ffi::CString;
use std::io::{BufReader, BufWriter, Read, Seek};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::*;
use crate::proto::parse_version;

/// The state layout this build writes and reads.
pub const STATE_FORMAT: u32 = 1;
const SELFTEST_TIMEOUT: Duration = Duration::from_secs(10);
const OK_FLUSH: Duration = Duration::from_secs(2);
const MAGIC: &[u8; 8] = b"K2PTYDS1";
/// Header JSON cap (metas are ≤ 64 KiB each).
const MAX_HEADER: u64 = 256 << 20;

/// `k2-ptyd --selftest --state-format N`: the line a healthy new image
/// prints, or why it can't take over.
pub fn selftest_reply(format: Option<&str>) -> Result<String, String> {
    match format.map(|f| f.parse::<u32>()) {
        Some(Ok(STATE_FORMAT)) => Ok(format!("ok {} {}", PTYD_VERSION, STATE_FORMAT)),
        Some(Ok(f)) => Err(format!("state format {f} is not supported (this build reads {STATE_FORMAT})")),
        _ => Err("usage: k2-ptyd --selftest --state-format N".into()),
    }
}

#[derive(Serialize, Deserialize)]
struct State {
    format: u32,
    from_version: String,
    boot_id: String,
    /// hex; the key file is left as it is.
    key: String,
    lock_fd: i32,
    sessions: Vec<SessionRec>,
    held: Vec<HeldRec>,
}

#[derive(Serialize, Deserialize)]
struct SessionRec {
    sid: String,
    pid: i32,
    master_fd: Option<i32>,
    ring_cap: usize,
    ring_start: u64,
    ring_len: usize,
    input_len: usize,
    exited: Option<ExitRec>,
    cols: u16,
    rows: u16,
    started_at: u64,
    meta: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct ExitRec {
    code: Option<i32>,
    signal: Option<i32>,
    at: u64,
    /// How long ago it exited (the TTL clock, P25).
    age_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct HeldRec {
    name: String,
    fd: i32,
    unlink: Option<String>,
}

struct Prepared {
    exe: CString,
    argv: Vec<CString>,
    envp: Vec<CString>,
    state: OwnedFd,
    to_version: String,
}

impl Server {
    pub(super) fn do_upgrade(&mut self, id: u64, rid: Option<u64>, exe: String) {
        let fail = |s: &mut Server, why: String| {
            s.log.line(&format!("upgrade_failed: {why}"));
            s.reply(id, rid, err("upgrade_failed", &why));
        };
        let path = Path::new(&exe);
        if !path.is_absolute() {
            return fail(self, "exe must be an absolute path".into());
        }
        if self.shutting_down.is_some() {
            return fail(self, "busy: shutting down".into());
        }
        if self.sessions.values().any(|s| s.kill_deadline.is_some()) {
            return fail(self, "busy: a kill is in progress".into());
        }
        let to_version = match selftest(path, SELFTEST_TIMEOUT) {
            Ok(v) => v,
            Err(e) => return fail(self, format!("selftest: {e}")),
        };
        match (parse_version(&to_version), parse_version(PTYD_VERSION)) {
            (Some(new), Some(cur)) if new < cur => return fail(self, format!("downgrade {PTYD_VERSION} -> {to_version} refused")),
            (Some(_), Some(_)) => {}
            _ => return fail(self, format!("unparsable version {to_version:?}")),
        }
        // Input the daemon already wrote to a data stream is part of the
        // state (it would otherwise die with the socket).
        let data: Vec<u64> = self.conns.iter().filter(|(_, c)| matches!(c.kind, Kind::Data { sid: Some(_) })).map(|(i, _)| *i).collect();
        for c in data {
            self.conn_readable(c);
        }
        let prep = match self.prepare_exec(&exe, to_version) {
            Ok(p) => p,
            Err(e) => return fail(self, e),
        };
        self.log.line(&format!("upgrade: v{} -> v{} sessions={} held={}", PTYD_VERSION, prep.to_version, self.sessions.len(), self.held.len()));
        self.reply(id, rid, Reply::Ok);
        self.flush_blocking(id, OK_FLUSH);
        let ids: Vec<u64> = self.conns.keys().copied().collect();
        for c in ids {
            self.drop_conn(c);
        }
        let inherit = self.inherited_fds(prep.state.as_raw_fd());
        for fd in &inherit {
            let _ = set_cloexec(*fd, false);
        }
        let e = exec(&prep);
        // Still here: the exec failed. Keep serving the old image (P64).
        for fd in &inherit {
            let _ = set_cloexec(*fd, true);
        }
        drop(prep);
        self.log.line(&format!("upgrade_failed: exec: {}; still serving v{}", e.kind(), PTYD_VERSION));
    }

    fn prepare_exec(&mut self, exe: &str, to_version: String) -> Result<Prepared, String> {
        use std::os::unix::ffi::OsStrExt;
        let state = self.write_state().map_err(|e| format!("state: {e}"))?;
        let c = |s: &str| CString::new(s).map_err(|_| "NUL byte in the exe path".to_string());
        let argv = vec![
            c("k2-ptyd")?,
            c("--resume-state-fd")?,
            c(&state.as_raw_fd().to_string())?,
            c("--listen-fd")?,
            c(&self.listener.as_raw_fd().to_string())?,
        ];
        // P5: the new image gets the allowlist only (also drops test knobs).
        let envp = std::env::vars_os()
            .filter(|(k, _)| k.to_str().is_some_and(crate::env_allowed))
            .filter_map(|(k, v)| {
                let mut b = k.as_bytes().to_vec();
                b.push(b'=');
                b.extend_from_slice(v.as_bytes());
                CString::new(b).ok()
            })
            .collect();
        Ok(Prepared { exe: c(exe)?, argv, envp, state, to_version })
    }

    /// Every fd the new image takes over.
    fn inherited_fds(&self, state: RawFd) -> Vec<RawFd> {
        let mut v = vec![state, self.listener.as_raw_fd(), self._lock.as_raw_fd()];
        v.extend(self.sessions.values().filter_map(|s| s.master.as_ref().map(|m| m.as_raw_fd())));
        v.extend(self.held.values().map(|h| h.fd.as_raw_fd()));
        v
    }

    /// Write one connection's queued bytes, waiting up to `within`.
    fn flush_blocking(&mut self, id: u64, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            self.conn_writable(id);
            let Some(c) = self.conns.get(&id) else { return };
            if c.wbuf.is_empty() || c.closing {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            let mut p = libc::pollfd { fd: c.sock.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
            // SAFETY: one valid pollfd.
            unsafe { libc::poll(&mut p, 1, left.as_millis().clamp(1, 1000) as i32) };
        }
    }

    fn write_state(&self) -> io::Result<OwnedFd> {
        let mut f = std::fs::File::from(state_file(&self.cfg.paths.dir)?);
        let mut sids: Vec<&String> = self.sessions.keys().collect();
        sids.sort();
        let sessions = sids
            .iter()
            .map(|sid| {
                let s = &self.sessions[*sid];
                SessionRec {
                    sid: (*sid).clone(),
                    pid: s.pid,
                    master_fd: s.master.as_ref().map(|m| m.as_raw_fd()),
                    ring_cap: s.ring.cap(),
                    ring_start: s.ring.start(),
                    ring_len: s.ring.len(),
                    input_len: s.input.len(),
                    exited: match s.state {
                        SState::Running => None,
                        SState::Exited { code, signal, at, when } => {
                            Some(ExitRec { code, signal, at, age_ms: when.elapsed().as_millis() as u64 })
                        }
                    },
                    cols: s.cols,
                    rows: s.rows,
                    started_at: s.started_at,
                    meta: s.meta.clone(),
                }
            })
            .collect();
        let held = self
            .held
            .iter()
            .map(|(n, h)| HeldRec { name: n.clone(), fd: h.fd.as_raw_fd(), unlink: h.unlink.as_ref().map(|p| p.to_string_lossy().into_owned()) })
            .collect();
        let st = State {
            format: STATE_FORMAT,
            from_version: PTYD_VERSION.into(),
            boot_id: self.boot_id.clone(),
            key: auth::hex(&self.key),
            lock_fd: self._lock.as_raw_fd(),
            sessions,
            held,
        };
        let hdr = serde_json::to_vec(&st).map_err(io::Error::other)?;
        {
            let mut w = BufWriter::with_capacity(1 << 20, &mut f);
            w.write_all(MAGIC)?;
            w.write_all(&(hdr.len() as u64).to_be_bytes())?;
            w.write_all(&hdr)?;
            for sid in &sids {
                let s = &self.sessions[*sid];
                let (a, b) = s.ring.slices_from(s.ring.start());
                w.write_all(a)?;
                w.write_all(b)?;
                let (a, b) = s.input.as_slices();
                w.write_all(a)?;
                w.write_all(b)?;
            }
            w.flush()?;
        }
        Ok(OwnedFd::from(f))
    }
}

/// A memfd (Linux), else an unlinked 0600 file in the 0700 ptyd dir.
/// CLOEXEC until the exec.
fn state_file(dir: &Path) -> io::Result<OwnedFd> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: memfd_create with a static name.
        let fd = unsafe { libc::memfd_create(c"k2-ptyd-state".as_ptr(), libc::MFD_CLOEXEC) };
        if fd >= 0 {
            // SAFETY: fresh fd.
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        // ENOSYS (kernel < 3.17) or a seccomp filter: the file fallback.
    }
    use std::os::unix::fs::OpenOptionsExt;
    let name = dir.join(format!(".upgrade-state-{}-{}", std::process::id(), auth::hex(&auth::random_bytes::<6>()?)));
    let f = std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&name)?;
    std::fs::remove_file(&name)?;
    Ok(OwnedFd::from(f))
}

fn exec(p: &Prepared) -> io::Error {
    // Test knob (debug builds only): exercise the exec-failure path (P64).
    if cfg!(debug_assertions) && std::env::var_os("K2_PTYD_TEST_FAIL_EXEC").is_some() {
        return io::Error::from_raw_os_error(libc::ENOEXEC);
    }
    let mut argv: Vec<*const libc::c_char> = p.argv.iter().map(|c| c.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const libc::c_char> = p.envp.iter().map(|c| c.as_ptr()).collect();
    envp.push(std::ptr::null());
    // SAFETY: NUL-terminated arrays of valid C strings that outlive the call.
    unsafe { libc::execve(p.exe.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
    io::Error::last_os_error()
}

/// Run `exe --selftest --state-format 1` with a bound. `Ok(version)`.
fn selftest(exe: &Path, within: Duration) -> Result<String, String> {
    let mut child = Command::new(exe)
        .args(["--selftest", "--state-format", &STATE_FORMAT.to_string()])
        .env_clear()
        .envs(std::env::vars_os().filter(|(k, _)| k.to_str().is_some_and(crate::env_allowed)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("did not start: {}", e.kind()))?;
    let mut out = child.stdout.take().ok_or("no stdout")?;
    set_nonblocking(out.as_raw_fd(), true).map_err(|e| e.kind().to_string())?;
    let mut buf = Vec::new();
    let drain = |out: &mut std::process::ChildStdout, buf: &mut Vec<u8>| {
        let mut chunk = [0u8; 512];
        loop {
            match out.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) if buf.len() < 4096 => buf.extend_from_slice(&chunk[..n]),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    };
    let deadline = Instant::now() + within;
    let status = loop {
        drain(&mut out, &mut buf);
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => return Err(format!("wait: {}", e.kind())),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {} s", within.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    drain(&mut out, &mut buf);
    if !status.success() {
        return Err(format!("exited {status}"));
    }
    let text = String::from_utf8_lossy(&buf);
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["ok", v, f] if *f == STATE_FORMAT.to_string() => Ok((*v).to_string()),
        _ => Err(format!("unexpected answer {:?}", text.chars().take(80).collect::<String>())),
    }
}

// ---------------------------------------------------------------- resume

/// Take ownership of an fd the previous image left open; CLOEXEC again
/// (S-2: nothing ptyd holds may reach an agent).
fn take_inherited(fd: RawFd) -> io::Result<OwnedFd> {
    if fd < 3 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("inherited fd {fd} is a std fd")));
    }
    // SAFETY: probing an fd number.
    cvt(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
    set_cloexec(fd, true)?;
    // SAFETY: the previous image handed this fd to us; nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

type Payload = Vec<(Vec<u8>, Vec<u8>)>;

fn read_state(fd: OwnedFd) -> io::Result<(State, Payload)> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
    let mut f = std::fs::File::from(fd);
    f.seek(io::SeekFrom::Start(0))?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not a k2-ptyd state".into()));
    }
    let mut len = [0u8; 8];
    r.read_exact(&mut len)?;
    let len = u64::from_be_bytes(len);
    if len > MAX_HEADER {
        return Err(bad(format!("header of {len} bytes")));
    }
    let mut hdr = vec![0u8; len as usize];
    r.read_exact(&mut hdr)?;
    let st: State = serde_json::from_slice(&hdr).map_err(|e| bad(e.to_string()))?;
    if st.format != STATE_FORMAT {
        return Err(bad(format!("state format {} (this build reads {STATE_FORMAT})", st.format)));
    }
    let mut payload = Vec::with_capacity(st.sessions.len());
    for s in &st.sessions {
        if s.ring_len > s.ring_cap || s.ring_cap > crate::ring::MAX_RING || s.input_len > CTRL_WBUF_MAX {
            return Err(bad(format!("session {} sizes", s.sid)));
        }
        let mut ring = vec![0u8; s.ring_len];
        r.read_exact(&mut ring)?;
        let mut input = vec![0u8; s.input_len];
        r.read_exact(&mut input)?;
        payload.push((ring, input));
    }
    Ok((st, payload))
}

/// `k2-ptyd --resume-state-fd N --listen-fd M`: the new image after an
/// upgrade exec. Same pid, same children, same boot id and key.
pub fn resume(cfg: Config, state_fd: RawFd, listen_fd: RawFd) -> io::Result<Exit> {
    let mut log = Log::open(&cfg.paths.log);
    match resume_server(cfg, state_fd, listen_fd) {
        Ok(mut s) => {
            // Children that exited while the image was being replaced.
            let running: Vec<String> = s.sessions.iter().filter(|(_, x)| x.running()).map(|(k, _)| k.clone()).collect();
            for sid in running {
                s.try_reap(&sid);
            }
            let r = s.run();
            s.cleanup();
            r.map(|()| Exit::Clean)
        }
        Err(e) => {
            log.line(&format!("resume failed: {e}"));
            Err(e)
        }
    }
}

fn resume_server(cfg: Config, state_fd: RawFd, listen_fd: RawFd) -> io::Result<Server> {
    let (st, payload) = read_state(take_inherited(state_fd)?)?;
    let listener = take_inherited(listen_fd)?;
    let lock = std::fs::File::from(take_inherited(st.lock_fd)?);
    let key: [u8; auth::KEY_LEN] = auth::unhex(&st.key)
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "key"))?;
    let mut sessions = HashMap::new();
    for (rec, (ring, input)) in st.sessions.into_iter().zip(payload) {
        let master = rec.master_fd.map(take_inherited).transpose()?;
        let state = match rec.exited {
            None => SState::Running,
            Some(e) => SState::Exited {
                code: e.code,
                signal: e.signal,
                at: e.at,
                when: Instant::now().checked_sub(Duration::from_millis(e.age_ms)).unwrap_or_else(Instant::now),
            },
        };
        sessions.insert(
            rec.sid,
            Session {
                pid: rec.pid,
                master,
                ring: Ring::restore(rec.ring_cap, rec.ring_start, ring),
                state,
                cols: rec.cols,
                rows: rec.rows,
                started_at: rec.started_at,
                meta: rec.meta,
                input: VecDeque::from(input),
                stream: None,
                sent: 0,
                kill_deadline: None,
                kill_waiters: Vec::new(),
            },
        );
    }
    let mut held = HashMap::new();
    for h in st.held {
        held.insert(h.name, Held { fd: take_inherited(h.fd)?, unlink: h.unlink.map(PathBuf::from) });
    }
    let sig_r = install_signals()?;
    // exec reset dumpability (S-3): harden the new image again.
    harden_process();
    let mut log = Log::open(&cfg.paths.log);
    log.line(&format!(
        "resumed v{} from v{} sessions={} held={}",
        PTYD_VERSION,
        st.from_version,
        sessions.len(),
        held.len()
    ));
    let idle_since = sessions.is_empty().then(Instant::now);
    Ok(Server {
        cfg,
        log,
        key,
        boot_id: st.boot_id,
        listener,
        sig_r,
        conns: HashMap::new(),
        next_conn: 1,
        sessions,
        owner: None,
        owner_since: None,
        ownerless_since: Some(Instant::now()),
        idle_since,
        held,
        shutting_down: None,
        exit_now: false,
        orphan_warned: false,
        _lock: lock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_reply_accepts_only_this_format() {
        assert_eq!(selftest_reply(Some("1")).unwrap(), format!("ok {PTYD_VERSION} 1"));
        assert!(selftest_reply(Some("2")).is_err());
        assert!(selftest_reply(Some("x")).is_err());
        assert!(selftest_reply(None).is_err());
    }
}
