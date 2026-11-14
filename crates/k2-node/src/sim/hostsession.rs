//! One sim session inside simhost: the plain stream messages k2-node
//! relays, handled under the session's server slot and workspace.
//!
//! - Channel 0 ([`k2_node_proto::sim::ctl`]): `run` only what the policy
//!   allows (IR-5), disk ops only in the workspace's own folder, uploads
//!   unpacked safely under `<uploads>/<workspace>/<sha256>` (IR-3),
//!   `helper_start` only for a device booted in this slot's set.
//! - Channels 1…n: one helper each. Request lines are parsed and policed,
//!   never passed through as text (`image` only into the workspace's
//!   folder); the helper's replies go back unchanged; its frames go
//!   newest-wins with one in flight until the server acks (§3.4).
//!
//! Nothing here logs an argument, a typed text or a file's contents.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use k2_node_proto::crypto;
use k2_node_proto::share::Slot;
use k2_node_proto::sim::ctl::{self, DiskOp, Event, FromNode, Reply, ReplyBody, Request};
use k2_node_proto::sim::policy::{self, SlotPaths};
use k2_node_proto::sim::{self as psim, codes};
use serde_json::{json, Value as J};

use super::exec::{Exec, ExecError};
use super::helperbin::{HelperPipes, HelperSpawner};
use super::SimRoot;

/// How long a helper may take to say `ready`.
pub const HELPER_READY: Duration = Duration::from_secs(60);
/// The longest one `run` may take, whatever the server asks.
pub const RUN_MAX: Duration = Duration::from_secs(600);
/// Largest upload accepted (the pack's cap plus tar headers).
pub const UPLOAD_MAX: u64 = psim::pack::MAX_BYTES + (64 << 20);

/// What every session of one simhost shares.
pub struct HostCtx {
    pub root: SimRoot,
    pub exec: Arc<dyn Exec>,
    pub helpers: Arc<dyn HelperSpawner>,
    /// Where device type bundles live (the last probe's).
    pub devicetype_roots: Mutex<Vec<String>>,
}

impl HostCtx {
    pub fn slot_paths(&self, slot: Slot) -> SlotPaths {
        self.root.slot_paths(slot, &self.devicetype_roots.lock().unwrap())
    }
}

/// Sends one framed message to k2-node; `false` = the connection is gone.
pub type Sender = Arc<dyn Fn(u8, &[u8]) -> bool + Send + Sync>;

/// One frame in flight; the newest one waits (§3.4).
#[derive(Debug, Default)]
pub struct Gate {
    in_flight: bool,
    latest: Option<Vec<u8>>,
}

impl Gate {
    /// A new frame from the helper: send it now, or keep it (replacing an
    /// older waiting one).
    pub fn offer(&mut self, frame: Vec<u8>) -> Option<Vec<u8>> {
        if self.in_flight {
            self.latest = Some(frame);
            None
        } else {
            self.in_flight = true;
            Some(frame)
        }
    }

    /// The server took the last one: the waiting frame, if any, goes now.
    pub fn ack(&mut self) -> Option<Vec<u8>> {
        match self.latest.take() {
            Some(f) => Some(f),
            None => {
                self.in_flight = false;
                None
            }
        }
    }
}

struct HelperRun {
    stdin: Mutex<Box<dyn Write + Send>>,
    gate: Mutex<Gate>,
    kill: Mutex<Box<dyn FnMut() + Send>>,
    dead: AtomicBool,
}

impl HelperRun {
    fn stop(&self) {
        self.dead.store(true, Ordering::SeqCst);
        let mut k = self.kill.lock().unwrap();
        (k.as_mut())();
    }
}

/// One framed helper message, raw (`u32 BE len ‖ kind ‖ payload`), as the
/// server reads it.
pub fn read_raw(r: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n + 64 > super::local::MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("a {n}-byte helper message")));
    }
    let mut out = len.to_vec();
    out.resize(4 + n, 0);
    r.read_exact(&mut out[4..])?;
    Ok(Some(out))
}

fn is_ready(raw: &[u8]) -> bool {
    raw.get(4) == Some(&b'J') && serde_json::from_slice::<J>(&raw[5..]).ok().and_then(|v| v.get("event").and_then(J::as_str).map(|e| e == "ready")).unwrap_or(false)
}

/// A helper's JSON message, framed.
pub fn helper_json(v: &J) -> Vec<u8> {
    let body = v.to_string();
    let mut out = ((body.len() + 1) as u32).to_be_bytes().to_vec();
    out.push(b'J');
    out.extend_from_slice(body.as_bytes());
    out
}

/// `simctl --set <set> list -j devices` → (udid, name, state).
pub fn list_devices(exec: &dyn Exec, set: &str) -> Result<Vec<(String, String, String)>, String> {
    let o = exec
        .run("xcrun", &["simctl".into(), "--set".into(), set.into(), "list".into(), "-j".into(), "devices".into()], Duration::from_secs(60))
        .map_err(|e| format!("simctl list: {e}"))?;
    if o.status != 0 {
        return Err(format!("simctl list exited {}", o.status));
    }
    parse_devices(&o.stdout)
}

pub fn parse_devices(json: &str) -> Result<Vec<(String, String, String)>, String> {
    let v: J = serde_json::from_str(json).map_err(|e| format!("simctl list devices: {e}"))?;
    let map = v.get("devices").and_then(J::as_object).ok_or("simctl list devices: no devices map")?;
    let s = |d: &J, k: &str| d.get(k).and_then(J::as_str).unwrap_or_default().to_string();
    Ok(map.values().filter_map(J::as_array).flatten().map(|d| (s(d, "udid"), s(d, "name"), s(d, "state"))).collect())
}

pub struct HostSession {
    ctx: Arc<HostCtx>,
    slot: Slot,
    key: String,
    paths: SlotPaths,
    send: Sender,
    helpers: Mutex<HashMap<u8, Arc<HelperRun>>>,
    upload: Mutex<Vec<u8>>,
}

impl HostSession {
    pub fn new(ctx: Arc<HostCtx>, slot: Slot, workspace_key: &str, send: Sender) -> Result<Arc<Self>, String> {
        if !psim::valid_workspace_key(workspace_key) || slot >= k2_node_proto::share::MAX_SLOTS {
            return Err("bad session (slot or workspace key)".into());
        }
        let paths = ctx.slot_paths(slot);
        for d in [&paths.set, &paths.tmp, &paths.uploads] {
            mkdir(std::path::Path::new(d)).map_err(|e| format!("{d}: {e}"))?;
        }
        Ok(Arc::new(Self { ctx, slot, key: workspace_key.to_string(), paths, send, helpers: Mutex::new(HashMap::new()), upload: Mutex::new(Vec::new()) }))
    }

    pub fn slot(&self) -> Slot {
        self.slot
    }

    fn reply(&self, id: u64, body: ReplyBody) {
        let v = serde_json::to_vec(&FromNode::Reply(Reply { id, body })).expect("a reply serializes");
        (self.send)(ctl::CONTROL, &v);
    }

    fn refused(r: policy::Refusal) -> ReplyBody {
        ReplyBody::Refused { code: r.code.into(), message: r.message }
    }

    /// One message from k2-node. `Err` ends the session (a control message
    /// that doesn't parse).
    pub fn on_frame(self: &Arc<Self>, channel: u8, payload: &[u8]) -> Result<(), String> {
        if channel != ctl::CONTROL {
            self.on_helper_line(channel, payload);
            return Ok(());
        }
        let req: Request = serde_json::from_slice(payload).map_err(|_| "a control message doesn't parse".to_string())?;
        match req {
            Request::Run { id, program, args, timeout_ms } => {
                let me = self.clone();
                std::thread::spawn(move || {
                    let body = me.run(&program, &args, timeout_ms);
                    me.reply(id, body);
                });
            }
            Request::Disk { id, op, path } => {
                let body = match policy::check_disk(&self.paths, &self.key, op, &path) {
                    Err(r) => Self::refused(r),
                    Ok(()) => disk(op, &path),
                };
                self.reply(id, body);
            }
            Request::Upload { id, sha256, total, offset, data, last } => {
                let body = self.upload(&sha256, total, offset, &data, last);
                self.reply(id, body);
            }
            Request::HelperStart { id, channel, udid } => {
                let me = self.clone();
                std::thread::spawn(move || {
                    let body = me.helper_start(channel, &udid);
                    me.reply(id, body);
                });
            }
            Request::HelperStop { id, channel } => {
                if let Some(h) = self.helpers.lock().unwrap().remove(&channel) {
                    h.stop();
                }
                self.reply(id, ReplyBody::Ok);
            }
            Request::Ack { channel } => {
                let h = self.helpers.lock().unwrap().get(&channel).cloned();
                if let Some(h) = h {
                    let next = h.gate.lock().unwrap().ack();
                    if let Some(f) = next {
                        (self.send)(channel, &f);
                    }
                }
            }
            Request::Bye => {}
        }
        Ok(())
    }

    fn run(&self, program: &str, args: &[String], timeout_ms: u64) -> ReplyBody {
        if let Err(r) = policy::check_run(&self.paths, &self.key, program, args) {
            return Self::refused(r);
        }
        let t = Duration::from_millis(timeout_ms.max(1000)).min(RUN_MAX);
        match self.ctx.exec.run(program, args, t) {
            Ok(o) => ReplyBody::Run { status: o.status, stdout: o.stdout, stderr: o.stderr },
            Err(ExecError::Timeout) => ReplyBody::Failed { code: "timeout".into(), message: format!("{program} took longer than {} s", t.as_secs()) },
            Err(ExecError::Failed(e)) => ReplyBody::Failed { code: "node_failed".into(), message: e },
        }
    }

    fn upload(&self, sha: &str, total: u64, offset: u64, data: &str, last: bool) -> ReplyBody {
        let fail = |m: &str| ReplyBody::Failed { code: "upload".into(), message: m.into() };
        let mut buf = self.upload.lock().unwrap();
        let hex = sha.len() == 64 && sha.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        if !hex || total > UPLOAD_MAX || offset != buf.len() as u64 {
            buf.clear();
            return fail(if !hex { "the upload's checksum isn't a sha256" } else if total > UPLOAD_MAX { "the app is over 1 GB" } else { "the upload is out of order" });
        }
        let Ok(chunk) = crypto::unb64(data) else {
            buf.clear();
            return fail("an upload chunk isn't base64");
        };
        if buf.len() as u64 + chunk.len() as u64 > total {
            buf.clear();
            return fail("the upload is longer than it said");
        }
        buf.extend_from_slice(&chunk);
        if !last {
            return ReplyBody::Ok;
        }
        let bytes = std::mem::take(&mut *buf);
        drop(buf);
        if bytes.len() as u64 != total || crypto::sha256_hex(&bytes) != sha {
            return fail("the upload's checksum doesn't match");
        }
        let dir = format!("{}/{}/{sha}", self.paths.uploads.trim_end_matches('/'), self.key);
        let dest = std::path::Path::new(&dir);
        let _ = std::fs::remove_dir_all(dest);
        match psim::pack::unpack_app(&bytes, dest) {
            Ok(top) => match top.file_name() {
                Some(n) => ReplyBody::Path { path: format!("{dir}/{}", n.to_string_lossy()) },
                None => fail("the upload has no app folder"),
            },
            Err(e) => {
                let _ = std::fs::remove_dir_all(dest);
                fail(&e.0)
            }
        }
    }

    fn helper_start(self: &Arc<Self>, channel: u8, udid: &str) -> ReplyBody {
        let no = |m: &str| ReplyBody::Refused { code: codes::NODE_REFUSED.into(), message: m.into() };
        if channel == ctl::CONTROL || self.helpers.lock().unwrap().contains_key(&channel) {
            return no("that helper channel is taken");
        }
        if !psim::valid_udid(udid) {
            return no("not a Simulator device id");
        }
        let booted = match list_devices(&*self.ctx.exec, &self.paths.set) {
            Ok(list) => list.iter().any(|(u, _, s)| u.eq_ignore_ascii_case(udid) && s == "Booted"),
            Err(e) => return ReplyBody::Failed { code: "node_failed".into(), message: e },
        };
        if !booted {
            return no("that device isn't booted in this server's set");
        }
        let pipes: HelperPipes = match self.ctx.helpers.spawn(std::path::Path::new(&self.paths.set), udid) {
            Ok(p) => p,
            Err(e) => return ReplyBody::Failed { code: "helper_build_failed".into(), message: e },
        };
        let HelperPipes { stdin, mut stdout, mut frames, kill } = pipes;
        let run = Arc::new(HelperRun { stdin: Mutex::new(stdin), gate: Mutex::new(Gate::default()), kill: Mutex::new(kill), dead: AtomicBool::new(false) });
        self.helpers.lock().unwrap().insert(channel, run.clone());
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        // Replies and events: straight through. The helper's end ends the channel.
        let (me, r) = (self.clone(), run.clone());
        std::thread::spawn(move || {
            let mut told = false;
            let why = loop {
                match read_raw(&mut stdout) {
                    Ok(Some(raw)) => {
                        if !(me.send)(channel, &raw) {
                            break "the connection closed";
                        }
                        if !told && is_ready(&raw) {
                            told = true;
                            let _ = ready_tx.send(());
                        }
                    }
                    Ok(None) => break "the helper exited",
                    Err(_) => break "the helper's output broke",
                }
            };
            r.stop();
            let mine = {
                let mut hs = me.helpers.lock().unwrap();
                let same = hs.get(&channel).is_some_and(|h| Arc::ptr_eq(h, &r));
                if same {
                    hs.remove(&channel);
                }
                same
            };
            if mine {
                let ev = serde_json::to_vec(&FromNode::Event(Event::HelperGone { channel, reason: why.into() })).expect("an event serializes");
                (me.send)(ctl::CONTROL, &ev);
            }
        });
        // Frames: newest wins, one in flight.
        let (me, r) = (self.clone(), run.clone());
        std::thread::spawn(move || {
            while let Ok(Some(raw)) = read_raw(&mut frames) {
                if r.dead.load(Ordering::SeqCst) {
                    break;
                }
                let now = r.gate.lock().unwrap().offer(raw);
                if let Some(f) = now {
                    if !(me.send)(channel, &f) {
                        break;
                    }
                }
            }
        });
        match ready_rx.recv_timeout(HELPER_READY) {
            Ok(()) => ReplyBody::Ok,
            Err(_) => {
                if let Some(h) = self.helpers.lock().unwrap().remove(&channel) {
                    h.stop();
                }
                ReplyBody::Failed { code: "helper_timeout".into(), message: "the Simulator helper didn't start".into() }
            }
        }
    }

    fn on_helper_line(&self, channel: u8, line: &[u8]) {
        let Some(h) = self.helpers.lock().unwrap().get(&channel).cloned() else { return };
        match policy::check_helper_line(&self.paths, &self.key, line) {
            Ok(v) => {
                let mut b = serde_json::to_vec(&v).expect("a parsed line serializes");
                b.push(b'\n');
                let mut w = h.stdin.lock().unwrap();
                let _ = w.write_all(&b).and_then(|_| w.flush());
            }
            Err(r) => {
                let id = serde_json::from_slice::<J>(line).ok().and_then(|v| v.get("id").cloned()).unwrap_or(J::Null);
                (self.send)(channel, &helper_json(&json!({"id": id, "ok": false, "code": r.code, "error": r.message})));
            }
        }
    }

    /// The connection to k2-node ended: stop this session's helpers (the
    /// devices stay up for the grace; a redial starts new helpers).
    pub fn end(&self) {
        for (_, h) in self.helpers.lock().unwrap().drain() {
            h.stop();
        }
    }
}

fn mkdir(p: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p)
}

fn disk(op: DiskOp, path: &str) -> ReplyBody {
    let p = std::path::Path::new(path);
    let fail = |e: std::io::Error| ReplyBody::Failed { code: "io".into(), message: e.kind().to_string() };
    match op {
        DiskOp::EnsureDir => mkdir(p).map(|_| ReplyBody::Ok).unwrap_or_else(fail),
        // Never through a link: the lane asks about its own files.
        DiskOp::IsDir => ReplyBody::Bool { value: std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) },
        DiskOp::IsFile => ReplyBody::Bool { value: std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) },
        DiskOp::Read => match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_file() && m.len() <= ctl::MAX_READ as u64 => std::fs::read(p).map(|b| ReplyBody::Bytes { data: crypto::b64(&b) }).unwrap_or_else(fail),
            Ok(m) if m.is_file() => ReplyBody::Failed { code: "io".into(), message: "the file is too large to send".into() },
            Ok(_) => ReplyBody::Failed { code: "io".into(), message: "not a file".into() },
            Err(e) => fail(e),
        },
        DiskOp::RemoveFile => std::fs::remove_file(p).map(|_| ReplyBody::Ok).unwrap_or_else(fail),
        DiskOp::RemoveDirAll => match std::fs::remove_dir_all(p) {
            Ok(()) => ReplyBody::Ok,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReplyBody::Ok,
            Err(e) => fail(e),
        },
    }
}

#[cfg(test)]
#[path = "hostsession_tests.rs"]
pub(crate) mod tests;
