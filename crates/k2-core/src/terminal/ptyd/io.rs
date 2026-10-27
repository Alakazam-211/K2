//! The K2-owned IO loop for ptyd sessions (S0 decision A1).
//!
//! In-process sessions keep alacritty's `EventLoop`. A ptyd session's
//! "PTY" is a Unix-socket byte stream from ptyd plus control requests on
//! the owner connection, so this loop is small: read → parse under the
//! Term lock → `Wakeup`; queue input; honour the DEC 2026 sync timeout; on
//! the child's exit send `ChildExit` + `Term::exit` + `Wakeup` exactly as
//! alacritty's loop does (`event_loop.rs:258-271`).
//!
//! Owning the vte `Processor` is the point: when the loop is stopped
//! ([`PtydIo::stop`]) it can `stop_sync` and report the exact replay offset
//! (`Shadow::partial_start`), which alacritty's private `State` makes
//! impossible (`ptyd-s0-results.md` §6).

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::Term;
use k2_ptyd::client::DataStream;
use k2_ptyd::proto::Request;

use super::client::{Owner, Reconnect, SessionEvent};
use super::shadow::Shadow;

/// How long a stream that hit EOF during a ptyd self-upgrade waits for the
/// owner to reconnect (the reader retries for 20 s).
const UPGRADE_WAIT: Duration = Duration::from_secs(25);

pub enum IoMsg {
    Input(Cow<'static, [u8]>),
    /// Stop the loop without touching the child (detach).
    Stop,
}

/// Cheap, cloneable handle for queueing input (also used by the
/// query-reply sink).
#[derive(Clone)]
pub struct IoSender {
    tx: mpsc::Sender<IoMsg>,
    wake: Arc<OwnedFd>,
}

impl IoSender {
    pub fn send(&self, msg: IoMsg) {
        if self.tx.send(msg).is_ok() {
            let b = 1u8;
            // SAFETY: one byte to our non-blocking wake pipe; EAGAIN means a
            // wake is already pending, which is fine.
            unsafe { libc::write(self.wake.as_raw_fd(), &b as *const u8 as *const libc::c_void, 1) };
        }
    }
}

/// What the loop reports when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoEnd {
    /// Total stream bytes read (absolute offset of the next unread byte).
    pub reader_pos: u64,
    /// First byte NOT on the grid after `stop_sync` — the snapshot's
    /// replay offset (S4).
    pub safe_offset: u64,
    /// The child exited (vs. the loop being stopped).
    pub exited: bool,
}

pub struct PtydIo {
    sender: IoSender,
    join: Mutex<Option<JoinHandle<IoEnd>>>,
    reader_pos: Arc<AtomicU64>,
    /// Adoption: the live offset (`attached.end`); 0 for a fresh spawn.
    replay_until: u64,
}

impl PtydIo {
    /// The reader passed the adoption replay (or there was none, or the
    /// loop already ended).
    pub fn replayed(&self) -> bool {
        if self.reader_pos() >= self.replay_until {
            return true;
        }
        match self.join.lock() {
            Ok(g) => g.as_ref().is_none_or(|h| h.is_finished()),
            Err(p) => p.into_inner().as_ref().is_none_or(|h| h.is_finished()),
        }
    }

    pub fn sender(&self) -> IoSender {
        self.sender.clone()
    }

    pub fn write(&self, bytes: Cow<'static, [u8]>) {
        self.sender.send(IoMsg::Input(bytes));
    }

    /// Bytes read so far (diagnostics/tests).
    pub fn reader_pos(&self) -> u64 {
        self.reader_pos.load(Ordering::Acquire)
    }

    /// Stop the loop (if still running) and join it. Idempotent; returns
    /// the loop's end report the first time.
    pub fn stop(&self) -> Option<IoEnd> {
        self.sender.send(IoMsg::Stop);
        let h = match self.join.lock() {
            Ok(mut g) => g.take(),
            Err(p) => p.into_inner().take(),
        }?;
        if h.thread().id() == std::thread::current().id() {
            return None; // never join ourselves
        }
        h.join().ok()
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut p = [0 as RawFd; 2];
    // SAFETY: pipe fills two fds.
    if unsafe { libc::pipe(p.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in p {
        // SAFETY: fcntl on our fresh fds.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            let fl = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
    }
    // SAFETY: both fresh and owned here.
    Ok(unsafe { (OwnedFd::from_raw_fd(p[0]), OwnedFd::from_raw_fd(p[1])) })
}

/// Adoption replay (PRD §7.7, P75): every stream byte below `until` was
/// already answered by the previous daemon, so it goes through the Term
/// with query replies muted. `on_live` runs exactly once, when the reader
/// reaches `until` (before the first live byte is parsed); it unmutes.
pub struct Replay {
    pub until: u64,
    pub on_live: Box<dyn FnOnce() + Send>,
}

/// Start the loop for one session. `install_sink` receives the input
/// sender BEFORE the thread starts, so the terminal-query reply sink exists
/// before the first byte is parsed (same guarantee as the in-process path).
pub fn start<L>(
    sid: String,
    stream: DataStream,
    term: Arc<FairMutex<Term<L>>>,
    listener: L,
    owner: Arc<Owner>,
    events: mpsc::Receiver<SessionEvent>,
    install_sink: impl FnOnce(IoSender),
) -> io::Result<PtydIo>
where
    L: EventListener + Send + 'static,
{
    start_with_replay(sid, stream, term, listener, owner, events, install_sink, None)
}

/// [`start`] for an adopted session: bytes below `replay.until` are a
/// muted replay (P75).
#[allow(clippy::too_many_arguments)]
pub fn start_with_replay<L>(
    sid: String,
    stream: DataStream,
    term: Arc<FairMutex<Term<L>>>,
    listener: L,
    owner: Arc<Owner>,
    events: mpsc::Receiver<SessionEvent>,
    install_sink: impl FnOnce(IoSender),
    replay: Option<Replay>,
) -> io::Result<PtydIo>
where
    L: EventListener + Send + 'static,
{
    let (wake_r, wake_w) = pipe()?;
    let (tx, rx) = mpsc::channel();
    let sender = IoSender { tx, wake: Arc::new(wake_w) };
    install_sink(sender.clone());
    let reader_pos = Arc::new(AtomicU64::new(stream.start));
    let pos = Arc::clone(&reader_pos);
    let replay_until = replay.as_ref().map(|r| r.until).unwrap_or(0);
    let gen = owner.generation();
    let join = std::thread::Builder::new().name(format!("ptyd-io {sid}")).spawn(move || {
        let mut l = Loop {
            sid,
            term,
            listener,
            gen,
            owner,
            events,
            rx,
            wake_r,
            parser: Processor::new(),
            shadow: Shadow::at(stream.start),
            wq: VecDeque::new(),
            pos,
            replay,
            ckpt_at: Instant::now(),
            ckpt_pos: stream.start,
        };
        l.run(stream)
    })?;
    Ok(PtydIo { sender, join: Mutex::new(Some(join)), reader_pos, replay_until })
}

struct Loop<L: EventListener> {
    sid: String,
    term: Arc<FairMutex<Term<L>>>,
    listener: L,
    owner: Arc<Owner>,
    /// The owner generation this stream was attached under (S7: a newer one
    /// means ptyd upgraded in place and the stream must be reattached).
    gen: u64,
    events: mpsc::Receiver<SessionEvent>,
    rx: mpsc::Receiver<IoMsg>,
    wake_r: OwnedFd,
    parser: Processor,
    shadow: Shadow,
    wq: VecDeque<u8>,
    pos: Arc<AtomicU64>,
    replay: Option<Replay>,
    /// S4: the last screen checkpoint (time, stream offset).
    ckpt_at: Instant,
    ckpt_pos: u64,
}

/// S4: while output keeps changing, save the screen this often, so a
/// daemon crash (no detach) still restores it.
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(30);

enum Step {
    Continue,
    Stopped,
    Eof,
}

const READ_CHUNK: usize = 64 * 1024;
const MAX_READ_PER_WAKE: usize = 1 << 20;

impl<L: EventListener> Loop<L> {
    fn run(&mut self, stream: DataStream) -> IoEnd {
        let (pending, mut sock) = stream.into_parts();
        if sock.set_nonblocking(true).is_err() {
            return self.finish_exit(SessionEvent::OwnerLost);
        }
        // Nothing to replay (empty backlog): live from the first byte.
        if self.replay.as_ref().is_some_and(|r| r.until <= self.shadow.pos()) {
            self.go_live();
        }
        if !pending.is_empty() {
            self.process(&pending);
        }
        loop {
            match self.turn(&mut sock) {
                Step::Continue => {}
                Step::Stopped => return self.finish_stop(),
                Step::Eof => {
                    // §7.10 (S7): ptyd is exec'ing a new image. The agent
                    // keeps running; once the owner reconnected (same boot
                    // id), reattach at the exact next unread byte — no
                    // loss, no duplicate. Anything else falls through.
                    if self.owner.upgrade_in_flight(self.gen) {
                        match self.owner.wait_reconnected(self.gen, UPGRADE_WAIT) {
                            Reconnect::Changed(g) => {
                                self.gen = g;
                                if self.reattach(&mut sock) {
                                    continue;
                                }
                            }
                            Reconnect::Unchanged => {}
                            Reconnect::Dead => return self.finish_exit(SessionEvent::OwnerLost),
                        }
                    }
                    // Stream EOF: the child exited (ptyd drains, then closes),
                    // an overrun closed it, or ptyd is gone. Ask the event
                    // channel which.
                    match self.events.recv_timeout(Duration::from_secs(2)) {
                        Ok(SessionEvent::Overrun { lost_bytes }) => {
                            crate::log_debug!("[ptyd] session={} overrun lost={} — reattaching", self.sid, lost_bytes);
                            if !self.reattach(&mut sock) {
                                return self.finish_exit(SessionEvent::OwnerLost);
                            }
                        }
                        Ok(ev) => return self.finish_exit(ev),
                        Err(_) => return self.finish_exit(SessionEvent::OwnerLost),
                    }
                }
            }
        }
    }

    /// Attach a new stream at the next unread offset and swap it in.
    /// `false` when ptyd refused (the session is gone). If the ring has
    /// already moved past that offset (an overrun), bytes were lost and the
    /// scanner restarts at the new start.
    fn reattach(&mut self, sock: &mut UnixStream) -> bool {
        let from = self.shadow.pos();
        let s = match self.owner.attach(&self.sid, from) {
            Ok(s) => s,
            Err(e) => {
                crate::log_debug!("[ptyd] session={} reattach at {} failed: {}", self.sid, from, e);
                return false;
            }
        };
        if s.start != from {
            crate::log_debug!("[ptyd] session={} reattached at {} (wanted {}): {} byte(s) lost", self.sid, s.start, from, s.start.saturating_sub(from));
            self.shadow = Shadow::at(s.start);
            self.pos.store(s.start, Ordering::Release);
        }
        let (p, ns) = s.into_parts();
        if ns.set_nonblocking(true).is_err() {
            return false;
        }
        *sock = ns;
        if !p.is_empty() {
            self.process(&p);
        }
        true
    }

    fn turn(&mut self, sock: &mut UnixStream) -> Step {
        self.maybe_checkpoint();
        let sync_deadline = self.parser.sync_timeout().sync_timeout();
        let mut timeout_ms: i32 = match sync_deadline {
            Some(d) => d.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32,
            None => -1,
        };
        // Wake for a due checkpoint when output changed since the last one.
        if self.shadow.pos() != self.ckpt_pos {
            let due = (self.ckpt_at + CHECKPOINT_EVERY).saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32;
            timeout_ms = if timeout_ms < 0 { due } else { timeout_ms.min(due) };
        }
        let mut fds = [
            libc::pollfd { fd: sock.as_raw_fd(), events: libc::POLLIN | if self.wq.is_empty() { 0 } else { libc::POLLOUT }, revents: 0 },
            libc::pollfd { fd: self.wake_r.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: two valid pollfds.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };
        if n < 0 {
            return if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted { Step::Continue } else { Step::Eof };
        }
        if n == 0 {
            // Synchronized-update timeout (same as alacritty's loop).
            if self.parser.sync_timeout().sync_timeout().is_some() {
                let mut t = self.term.lock();
                self.parser.stop_sync(&mut *t);
                drop(t);
                self.listener.send_event(Event::Wakeup);
            }
            return Step::Continue;
        }
        if fds[1].revents != 0 {
            let mut b = [0u8; 64];
            // SAFETY: draining our non-blocking pipe.
            while unsafe { libc::read(self.wake_r.as_raw_fd(), b.as_mut_ptr() as *mut libc::c_void, b.len()) } > 0 {}
            while let Ok(m) = self.rx.try_recv() {
                match m {
                    IoMsg::Input(bytes) => self.wq.extend(bytes.iter()),
                    IoMsg::Stop => return Step::Stopped,
                }
            }
        }
        let re = fds[0].revents;
        if re & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut buf = [0u8; READ_CHUNK];
            let mut total = 0usize;
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => {
                        if total > 0 {
                            self.wakeup_after(total);
                        }
                        return Step::Eof;
                    }
                    Ok(n) => {
                        self.feed(&buf[..n]);
                        total += n;
                        if total >= MAX_READ_PER_WAKE {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        if total > 0 {
                            self.wakeup_after(total);
                        }
                        return Step::Eof;
                    }
                }
            }
            if total > 0 {
                self.wakeup_after(total);
            }
        }
        if !self.wq.is_empty() {
            loop {
                let (a, _) = self.wq.as_slices();
                if a.is_empty() {
                    break;
                }
                match sock.write(a) {
                    Ok(n) => {
                        self.wq.drain(..n);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        // S7: input typed while ptyd upgrades goes to the
                        // reattached stream.
                        if !self.owner.upgrade_in_flight(self.gen) {
                            self.wq.clear();
                        }
                        break;
                    }
                }
            }
        }
        Step::Continue
    }

    fn feed(&mut self, bytes: &[u8]) {
        if let Some(until) = self.replay.as_ref().map(|r| r.until) {
            let pos = self.shadow.pos();
            if pos.saturating_add(bytes.len() as u64) >= until {
                // The replay ends inside (or exactly at the end of) this
                // chunk: muted part, unmute, live part (P75: muting ends
                // exactly at the live offset).
                let cut = (until.saturating_sub(pos) as usize).min(bytes.len());
                self.feed_raw(&bytes[..cut]);
                self.go_live();
                if cut < bytes.len() {
                    self.feed_raw(&bytes[cut..]);
                }
                return;
            }
        }
        self.feed_raw(bytes);
    }

    fn feed_raw(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut t = self.term.lock();
            self.parser.advance(&mut *t, bytes);
        }
        self.shadow.feed(bytes);
        self.pos.store(self.shadow.pos(), Ordering::Release);
    }

    /// S4 checkpoint: every [`CHECKPOINT_EVERY`] while output changed, and
    /// only outside a synchronized update (then every byte before
    /// `partial_start` is on the grid and none after, exactly as at
    /// detach). Never during the adoption replay. The grid is cloned under
    /// the Term lock; encoding and the file write run on another thread.
    fn maybe_checkpoint(&mut self) {
        if self.replay.is_some()
            || self.shadow.pos() == self.ckpt_pos
            || self.ckpt_at.elapsed() < CHECKPOINT_EVERY
            || self.parser.sync_timeout().sync_timeout().is_some()
        {
            return;
        }
        let offset = self.shadow.partial_start();
        let snap = super::snapshot::Snapshot::capture(&*self.term.lock(), offset);
        self.ckpt_at = Instant::now();
        self.ckpt_pos = self.shadow.pos();
        let sid = self.sid.clone();
        let _ = std::thread::Builder::new().name("ptyd-ckpt".into()).spawn(move || {
            if let Err(e) = super::snapshot::write(&sid, &snap) {
                crate::log_debug!("[ptyd] session={sid}: screen checkpoint not saved: {e}");
            }
        });
    }

    fn go_live(&mut self) {
        if let Some(r) = self.replay.take() {
            (r.on_live)();
        }
    }

    fn process(&mut self, bytes: &[u8]) {
        self.feed(bytes);
        self.wakeup_after(bytes.len());
    }

    /// alacritty: "queue terminal redraw unless all processed bytes were
    /// synchronized".
    fn wakeup_after(&self, processed: usize) {
        if self.parser.sync_bytes_count() < processed {
            self.listener.send_event(Event::Wakeup);
        }
    }

    fn finish_stop(&mut self) -> IoEnd {
        self.go_live();
        let mut t = self.term.lock();
        self.parser.stop_sync(&mut *t);
        drop(t);
        IoEnd { reader_pos: self.shadow.pos(), safe_offset: self.shadow.partial_start(), exited: false }
    }

    fn finish_exit(&mut self, ev: SessionEvent) -> IoEnd {
        use std::os::unix::process::ExitStatusExt;
        self.go_live();
        let status = match ev {
            SessionEvent::Exited { code: Some(c), .. } => Some(std::process::ExitStatus::from_raw((c & 0xff) << 8)),
            SessionEvent::Exited { signal: Some(s), .. } => Some(std::process::ExitStatus::from_raw(s & 0x7f)),
            // ptyd vanished: its agents got SIGHUP from the master closing.
            SessionEvent::OwnerLost => Some(std::process::ExitStatus::from_raw(libc::SIGHUP)),
            _ => None,
        };
        {
            let mut t = self.term.lock();
            self.parser.stop_sync(&mut *t);
        }
        if let Some(st) = status {
            self.listener.send_event(Event::ChildExit(st));
        }
        self.term.lock().exit();
        self.listener.send_event(Event::Wakeup);
        // The exit is consumed: drop ptyd's record (P25/P32) and the cell
        // listener ptyd held for it (P44; an unknown name is fine).
        let _ = self.owner.request(Request::AckExit { sid: self.sid.clone() });
        self.owner.release_fd(&super::cell_listener_name(&self.sid));
        self.owner.unwatch(&self.sid);
        super::adopt::release_conversations_of(&self.sid);
        // S-5: the session ended; its saved screen goes.
        super::snapshot::remove(&self.sid);
        IoEnd { reader_pos: self.shadow.pos(), safe_offset: self.shadow.partial_start(), exited: true }
    }
}
