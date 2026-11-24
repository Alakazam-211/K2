//! A fake Mac compute node for tests (no Mac, no Xcode; runs on Linux CI).
//!
//! Everything on the path is real except the Mac: the attach frames
//! (`sim_open`, `sim_state`…), the stream handshake and AES-GCM in both
//! directions, the node's run policy (`k2_node_proto::sim::policy`), its
//! lease table (`…::lease`) and the `.app` unpack (`…::pack`). simctl and
//! the helper are played in memory. Every byte that crosses the "relay"
//! is recorded, so tests can check what the relay could read, and the
//! node's log keeps op names only, as the real one must.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::Frame;
use k2_node_proto::sim::ctl::{self, DiskOp, FromNode, Reply, ReplyBody, Request};
use k2_node_proto::sim::lease::{Admission, Effect, Leases};
use k2_node_proto::sim::policy::{self, SlotPaths};
use k2_node_proto::sim::stream::{NodeHandshake, Opener, Sealer, Side};
use k2_node_proto::sim::{self as psim, SimDeviceType, SimFacts, SimRuntime};
use serde_json::{json, Value as J};

use super::link::StreamConn;
use super::{IosNodeLane, NodeDirectory, NodeInfo};

pub const NODE_ID: &str = "node-rig1";
pub const NODE_NAME: &str = "rig1-mac";
pub const TYPE_17: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17";
pub const TYPE_17_PRO: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro";
pub const RUNTIME: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-5";
const DEVTYPES: &str = "/Library/Developer/CoreSimulator/Profiles/DeviceTypes";

/// The helper's framed message: `[u32 BE len][kind][payload]`.
pub fn framed(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
    v.push(kind);
    v.extend_from_slice(payload);
    v
}

fn jmsg(v: &J) -> Vec<u8> {
    framed(b'J', v.to_string().as_bytes())
}

#[derive(Default)]
struct Helper {
    frames_on: bool,
    in_flight: bool,
    seq: u64,
}

struct NodeStream {
    session: String,
    workspace_key: String,
    sealer: Mutex<Sealer>,
    opener: Mutex<Opener>,
    server: Arc<StreamConn>,
    upload: Mutex<Vec<u8>>,
    helpers: Mutex<HashMap<u8, Helper>>,
    dead: std::sync::atomic::AtomicBool,
}

/// What the fake Mac's Simulator did (assertions read it).
#[derive(Default, Debug)]
pub struct World {
    /// (udid, name, state)
    pub devices: Vec<(String, String, &'static str)>,
    pub next: u32,
    pub shutdowns: Vec<String>,
    pub installed: Vec<String>,
    pub opened: Vec<String>,
    pub taps: Vec<(f64, f64)>,
    /// The focused text field's value (the helper's `set_value` target).
    pub field: String,
    pub launched: Vec<String>,
}

pub struct FakeNode {
    pub key: SigningKey,
    controller_spki: Vec<u8>,
    root: PathBuf,
    paths: SlotPaths,
    lane: Mutex<Weak<IosNodeLane>>,
    leases: Mutex<Leases>,
    pub admission: Mutex<Admission>,
    pub world: Mutex<World>,
    streams: Mutex<HashMap<String, Arc<NodeStream>>>,
    /// Every sealed message on the relay leg: (server → node?, bytes).
    pub relay: Mutex<Vec<(bool, Vec<u8>)>>,
    /// Op names only: what the real node may log.
    pub log: Mutex<Vec<String>>,
    pub online: Mutex<bool>,
    pub hold: Mutex<Option<String>>,
    pub ready: Mutex<bool>,
    pub helper_protocol: Mutex<u64>,
    /// `k2 compute node pinned-only` on this node.
    pub pinned_only: Mutex<bool>,
    /// Frames the fake helper sends after `frames on` (then it goes still).
    pub frames_budget: Mutex<u32>,
    worker: Mutex<Option<mpsc::Sender<(Arc<NodeStream>, Vec<u8>)>>>,
}

impl FakeNode {
    pub fn new(controller: &SigningKey) -> Arc<Self> {
        let root = std::env::temp_dir().join(format!("k2-fake-mac-{}-{}", std::process::id(), crypto::random_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let paths = SlotPaths {
            set: "/var/k2sim/sets/0".into(),
            tmp: "/var/k2sim/tmp/0".into(),
            uploads: "/var/k2sim/uploads/0".into(),
            devicetype_roots: vec![DEVTYPES.into()],
        };
        let me = Arc::new(Self {
            key: SigningKey::generate().unwrap(),
            controller_spki: controller.spki_der().to_vec(),
            root,
            paths,
            lane: Mutex::new(Weak::new()),
            leases: Mutex::new(Leases::new()),
            admission: Mutex::new(Admission { enabled: true, free_mem: 8 << 30, disk_ok: true, ..Default::default() }),
            world: Mutex::new(World::default()),
            streams: Mutex::new(HashMap::new()),
            relay: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            online: Mutex::new(true),
            hold: Mutex::new(None),
            ready: Mutex::new(true),
            helper_protocol: Mutex::new(psim::HELPER_PROTOCOL),
            pinned_only: Mutex::new(false),
            frames_budget: Mutex::new(3),
            worker: Mutex::new(None),
        });
        // One worker plays the node's side of every stream, in order.
        let (tx, rx) = mpsc::channel::<(Arc<NodeStream>, Vec<u8>)>();
        let weak = Arc::downgrade(&me);
        std::thread::spawn(move || {
            while let Ok((st, bytes)) = rx.recv() {
                let Some(n) = weak.upgrade() else { break };
                n.on_stream_bytes(&st, &bytes);
            }
        });
        *me.worker.lock().unwrap() = Some(tx);
        me
    }

    pub fn attach_lane(&self, lane: &Arc<IosNodeLane>) {
        *self.lane.lock().unwrap() = Arc::downgrade(lane);
    }

    fn lane(&self) -> Option<Arc<IosNodeLane>> {
        self.lane.lock().unwrap().upgrade()
    }

    /// A node path on the fake's own disk.
    pub fn local(&self, node_path: &str) -> PathBuf {
        self.root.join(node_path.trim_start_matches('/'))
    }

    pub fn paths(&self) -> &SlotPaths {
        &self.paths
    }

    pub fn facts(&self) -> SimFacts {
        let leases = self.leases.lock().unwrap();
        let ready = *self.ready.lock().unwrap();
        let free = self.admission.lock().unwrap().free_mem.saturating_sub(leases.reserved_bytes());
        SimFacts {
            ready,
            needs: if ready { vec![] } else { vec![psim::SimNeed { what: "install an iOS Simulator runtime".into(), command: "xcodebuild -downloadPlatform iOS".into() }] },
            xcode: Some("26.6 (17F113)".into()),
            xcode_path: Some("/Applications/Xcode.app".into()),
            helper_protocol: *self.helper_protocol.lock().unwrap(),
            runtimes: vec![SimRuntime {
                identifier: RUNTIME.into(),
                name: "iOS 26.5".into(),
                version: "26.5".into(),
                device_types: vec![
                    SimDeviceType { identifier: TYPE_17.into(), name: "iPhone 17".into(), width_pt: 402, height_pt: 874, scale_milli: 3000, default: true },
                    SimDeviceType { identifier: TYPE_17_PRO.into(), name: "iPhone 17 Pro".into(), width_pt: 402, height_pt: 874, scale_milli: 3000, default: false },
                ],
            }],
            sessions: leases.sessions_for(0),
            sessions_total: leases.all().len() as u32,
            bytes: 0,
            reserve_bytes: psim::RESERVE_MEM_BYTES,
            free_bytes: free,
            paths: self.paths.clone(),
            remote_update: false,
        }
    }

    pub fn info(&self) -> NodeInfo {
        NodeInfo {
            id: NODE_ID.into(),
            name: NODE_NAME.into(),
            online: *self.online.lock().unwrap(),
            protocol: psim::PROTOCOL_SIM,
            features: vec![psim::FEATURE_SIM_IOS.into()],
            mac: true,
            sim: Some(self.facts()),
            hold: self.hold.lock().unwrap().clone(),
            pinned_only: *self.pinned_only.lock().unwrap(),
        }
    }

    // ── attach frames ────────────────────────────────────────────────

    /// A frame on the attach socket, server → node.
    pub fn on_frame(self: &Arc<Self>, f: Frame) {
        self.log.lock().unwrap().push(f.kind().to_string());
        let effects = match f {
            Frame::SimOpen(o) => {
                let adm = self.admission.lock().unwrap().clone();
                let e = self.leases.lock().unwrap().open(0, &o, &adm, 0);
                let admitted = e.iter().any(|x| x.frame.session == o.session && x.frame.state == psim::SessionState::Opening);
                self.apply(e);
                if admitted {
                    let me = self.clone();
                    let session = o.session.clone();
                    // Dial from another thread, as k2-node would.
                    std::thread::spawn(move || me.dial(&session, &o.workspace_key));
                }
                return;
            }
            Frame::SimClose(c) => self.leases.lock().unwrap().close(0, &c.session),
            Frame::SimSync(s) => self.leases.lock().unwrap().sync(0, &s.sessions),
            Frame::SimErase(_) => vec![],
            _ => vec![],
        };
        self.apply(effects);
    }

    fn apply(&self, effects: Vec<Effect>) {
        for e in effects {
            if e.shutdown {
                let prefix = format!("K2-{}--", e.workspace_key);
                let mut w = self.world.lock().unwrap();
                let ids: Vec<String> = w.devices.iter().filter(|d| d.1.starts_with(&prefix) && d.2 == "Booted").map(|d| d.0.clone()).collect();
                for d in w.devices.iter_mut().filter(|d| ids.contains(&d.0)) {
                    d.2 = "Shutdown";
                }
                w.shutdowns.extend(ids);
                drop(w);
                if let Some(st) = self.streams.lock().unwrap().remove(&e.frame.session) {
                    st.dead.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            if let Some(l) = self.lane() {
                l.on_state(NODE_ID, &e.frame);
            }
        }
    }

    /// Dial the stream for a session (the first time, or a redial).
    pub fn dial(self: &Arc<Self>, session: &str, workspace_key: &str) -> Result<(), String> {
        let lane = self.lane().ok_or("no lane")?;
        let (nh, hello) = NodeHandshake::start(&self.key, session)?;
        let (pending, challenge) = lane.stream_hello(&hello)?;
        let (proof, keys) = nh.finish(&self.key, &self.controller_spki, &challenge)?;
        let (sealer, opener) = keys.split(Side::Node);
        // The server's sends go to the node worker through the "relay".
        let slot: Arc<Mutex<Option<Arc<NodeStream>>>> = Arc::new(Mutex::new(None));
        let (slot2, me) = (slot.clone(), Arc::downgrade(self));
        let raw = Box::new(move |bytes: Vec<u8>| -> bool {
            let Some(me) = me.upgrade() else { return false };
            // The server may send the moment its side of the handshake
            // finishes, before this thread has stored the node's half (a
            // real socket buffers that write). Wait for it, briefly.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let st = loop {
                if let Some(st) = slot2.lock().unwrap().clone() {
                    break st;
                }
                if std::time::Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            };
            if st.dead.load(std::sync::atomic::Ordering::SeqCst) {
                return false;
            }
            me.relay.lock().unwrap().push((true, bytes.clone()));
            let worker = me.worker.lock().unwrap().clone();
            worker.is_some_and(|w| w.send((st, bytes)).is_ok())
        });
        let server = pending.finish(&proof, raw)?;
        let st = Arc::new(NodeStream {
            session: session.to_string(),
            workspace_key: workspace_key.to_string(),
            sealer: Mutex::new(sealer),
            opener: Mutex::new(opener),
            server,
            upload: Mutex::new(Vec::new()),
            helpers: Mutex::new(HashMap::new()),
            dead: std::sync::atomic::AtomicBool::new(false),
        });
        *slot.lock().unwrap() = Some(st.clone());
        if let Some(old) = self.streams.lock().unwrap().insert(session.to_string(), st) {
            old.dead.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let e = self.leases.lock().unwrap().stream_up(session);
        self.apply(e);
        Ok(())
    }

    /// The stream socket drops (a network blip): both ends see it.
    pub fn drop_stream(&self, session: &str) {
        if let Some(st) = self.streams.lock().unwrap().remove(session) {
            st.dead.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(l) = self.lane() {
                l.stream_closed(session, &st.server.conn_id);
            }
        }
        let e = self.leases.lock().unwrap().stream_down(session, 0);
        self.apply(e);
    }

    /// The node's clock moved on `secs` from when the stream dropped.
    pub fn tick(&self, secs: u64) {
        let e = self.leases.lock().unwrap().tick(secs);
        self.apply(e);
    }

    /// The node's live sessions.
    pub fn sessions(&self) -> Vec<String> {
        self.leases.lock().unwrap().sessions_for(0)
    }

    /// Feed raw bytes to the node's side of a session's stream (a replay).
    /// Returns the error that closed it, if any.
    pub fn inject(&self, session: &str, bytes: &[u8]) -> Result<(), String> {
        let st = self.streams.lock().unwrap().get(session).cloned().ok_or("no stream")?;
        let r = st.opener.lock().unwrap().open(bytes).map(|_| ()).map_err(|e| e.to_string());
        if r.is_err() {
            self.drop_stream(session);
        }
        r
    }

    // ── the stream, node side ────────────────────────────────────────

    fn send(&self, st: &NodeStream, channel: u8, plain: &[u8]) {
        if st.dead.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let msg = st.sealer.lock().unwrap().seal(channel, plain).expect("seal");
        self.relay.lock().unwrap().push((false, msg.clone()));
        if st.server.deliver(&msg).is_err() {
            st.dead.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn reply(&self, st: &NodeStream, id: u64, body: ReplyBody) {
        let v = serde_json::to_vec(&FromNode::Reply(Reply { id, body })).unwrap();
        self.send(st, ctl::CONTROL, &v);
    }

    fn on_stream_bytes(&self, st: &Arc<NodeStream>, bytes: &[u8]) {
        if st.dead.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let opened = st.opener.lock().unwrap().open(bytes);
        let (channel, plain) = match opened {
            Ok(x) => x,
            Err(_) => {
                self.drop_stream(&st.session);
                return;
            }
        };
        if channel == ctl::CONTROL {
            let Ok(req) = serde_json::from_slice::<Request>(&plain) else { return };
            self.log.lock().unwrap().push(req.op().to_string());
            self.on_request(st, req);
        } else {
            self.on_helper_line(st, channel, &plain);
        }
    }

    fn on_request(&self, st: &Arc<NodeStream>, req: Request) {
        let refused = |r: policy::Refusal| ReplyBody::Refused { code: r.code.into(), message: r.message };
        match req {
            Request::Run { id, program, args, .. } => {
                let body = match policy::check_run(&self.paths, &st.workspace_key, &program, &args) {
                    Err(r) => refused(r),
                    Ok(()) => self.play_run(&program, &args),
                };
                self.reply(st, id, body);
            }
            Request::Disk { id, op, path } => {
                let body = match policy::check_disk(&self.paths, &st.workspace_key, op, &path) {
                    Err(r) => refused(r),
                    Ok(()) => self.play_disk(op, &path),
                };
                self.reply(st, id, body);
            }
            Request::Upload { id, sha256, total, offset, data, last } => {
                let body = (|| {
                    let mut buf = st.upload.lock().unwrap();
                    if offset != buf.len() as u64 || total > psim::pack::MAX_BYTES * 2 {
                        buf.clear();
                        return ReplyBody::Failed { code: "upload".into(), message: "the upload is out of order".into() };
                    }
                    buf.extend(crypto::unb64(&data).unwrap_or_default());
                    if !last {
                        return ReplyBody::Ok;
                    }
                    let bytes = std::mem::take(&mut *buf);
                    if bytes.len() as u64 != total || crypto::sha256_hex(&bytes) != sha256 {
                        return ReplyBody::Failed { code: "upload".into(), message: "the upload's checksum doesn't match".into() };
                    }
                    let node_dir = format!("{}/{}", self.paths.uploads, sha256);
                    match psim::pack::unpack_app(&bytes, &self.local(&node_dir)) {
                        Ok(top) => ReplyBody::Path { path: format!("{node_dir}/{}", top.file_name().unwrap().to_string_lossy()) },
                        Err(e) => ReplyBody::Failed { code: "upload".into(), message: e.0 },
                    }
                })();
                self.reply(st, id, body);
            }
            Request::HelperStart { id, channel, udid } => {
                let listed = self.world.lock().unwrap().devices.iter().any(|d| d.0 == udid && d.2 == "Booted");
                if !psim::valid_udid(&udid) || !listed {
                    self.reply(st, id, ReplyBody::Refused { code: psim::codes::NODE_REFUSED.into(), message: "that device isn't booted in this server's set".into() });
                    return;
                }
                st.helpers.lock().unwrap().insert(channel, Helper::default());
                // The shipped helper's ready: its newest protocol and the
                // ones it accepts (the server caps a node's helper at 3).
                self.send(st, channel, &jmsg(&json!({"event": "ready", "version": psim::HELPER_PROTOCOL, "accepts": [3, psim::HELPER_PROTOCOL], "clock_ns": 1})));
                self.reply(st, id, ReplyBody::Ok);
            }
            Request::HelperStop { id, channel } => {
                st.helpers.lock().unwrap().remove(&channel);
                self.reply(st, id, ReplyBody::Ok);
            }
            Request::Ack { channel } => {
                if let Some(h) = st.helpers.lock().unwrap().get_mut(&channel) {
                    h.in_flight = false;
                }
                self.maybe_frame(st, channel);
            }
            Request::Bye => {}
        }
    }

    fn maybe_frame(&self, st: &NodeStream, channel: u8) {
        let next = {
            let mut hs = st.helpers.lock().unwrap();
            let Some(h) = hs.get_mut(&channel) else { return };
            let mut budget = self.frames_budget.lock().unwrap();
            if !h.frames_on || h.in_flight || *budget == 0 {
                return;
            }
            *budget -= 1;
            h.in_flight = true;
            h.seq += 1;
            h.seq
        };
        let mut f = Vec::new();
        f.extend_from_slice(&402u32.to_be_bytes());
        f.extend_from_slice(&874u32.to_be_bytes());
        f.extend_from_slice(&next.to_be_bytes());
        // Protocol 3's frame header: capture and emit times (helper ns).
        f.extend_from_slice(&(next * 1_000).to_be_bytes());
        f.extend_from_slice(&(next * 1_000 + 10).to_be_bytes());
        f.extend_from_slice(b"\xff\xd8fake-jpeg");
        self.send(st, channel, &framed(b'F', &f));
    }

    fn on_helper_line(&self, st: &NodeStream, channel: u8, line: &[u8]) {
        let v = match policy::check_helper_line(&self.paths, &st.workspace_key, line) {
            Ok(v) => v,
            Err(r) => {
                let id = serde_json::from_slice::<J>(line).ok().and_then(|v| v.get("id").cloned()).unwrap_or(J::Null);
                self.send(st, channel, &jmsg(&json!({"id": id, "ok": false, "code": r.code, "error": r.message})));
                return;
            }
        };
        let op = v["op"].as_str().unwrap_or("").to_string();
        self.log.lock().unwrap().push(format!("helper:{op}"));
        let id = v["id"].clone();
        let ok = |extra: J| {
            let mut r = json!({"id": id, "ok": true});
            if let (Some(o), Some(e)) = (r.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            r
        };
        let reply = match op.as_str() {
            "hello" => ok(json!({"screen": {"width": 402, "height": 874, "scale": 3}})),
            "tree" => ok(json!({"tree": {"role": "AXApplication", "label": "K2Fixture", "frame": [0, 0, 402, 874], "enabled": true, "children": [
                {"role": "AXTextField", "identifier": "email", "placeholder": "Email", "value": "", "frame": [40, 245, 321, 34], "enabled": true},
                {"role": "AXButton", "label": "Save", "identifier": "save", "frame": [182, 383, 37, 20], "enabled": true}]}})),
            "tap" => {
                self.world.lock().unwrap().taps.push((v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0)));
                ok(json!({}))
            }
            "focus" => ok(json!({"role": "AXTextField", "frame": [40, 245, 321, 34]})),
            "value" => {
                let f = self.world.lock().unwrap().field.clone();
                ok(json!({"role": "AXTextField", "value": if f.is_empty() { "Email".to_string() } else { f }, "placeholder": "Email"}))
            }
            "set_value" => {
                self.world.lock().unwrap().field = v["text"].as_str().unwrap_or("").to_string();
                ok(json!({}))
            }
            "type" => {
                let t = v["text"].as_str().unwrap_or("").to_string();
                self.world.lock().unwrap().field.push_str(&t);
                ok(json!({}))
            }
            "frames" => {
                let on = v["on"].as_bool().unwrap_or(false);
                if let Some(h) = st.helpers.lock().unwrap().get_mut(&channel) {
                    h.frames_on = on;
                }
                let r = ok(json!({}));
                self.send(st, channel, &jmsg(&r));
                if on {
                    self.maybe_frame(st, channel);
                }
                return;
            }
            "image" => {
                let p = self.local(v["path"].as_str().unwrap_or(""));
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                std::fs::write(&p, b"\x89PNG\r\n\x1a\nfake").unwrap();
                ok(json!({"width": 1206, "height": 2622}))
            }
            _ => ok(json!({})),
        };
        // An `nr` line (a touch's down and moves) gets no reply.
        if v["nr"].as_bool() == Some(true) {
            return;
        }
        self.send(st, channel, &jmsg(&reply));
    }

    fn play_disk(&self, op: DiskOp, path: &str) -> ReplyBody {
        let p = self.local(path);
        let fail = |e: std::io::Error| ReplyBody::Failed { code: "io".into(), message: e.to_string() };
        match op {
            DiskOp::EnsureDir => std::fs::create_dir_all(&p).map(|_| ReplyBody::Ok).unwrap_or_else(fail),
            DiskOp::IsDir => ReplyBody::Bool { value: p.is_dir() },
            DiskOp::IsFile => ReplyBody::Bool { value: p.is_file() },
            DiskOp::Read => std::fs::read(&p).map(|b| ReplyBody::Bytes { data: crypto::b64(&b) }).unwrap_or_else(fail),
            DiskOp::RemoveFile => std::fs::remove_file(&p).map(|_| ReplyBody::Ok).unwrap_or_else(fail),
            DiskOp::RemoveDirAll => match std::fs::remove_dir_all(&p) {
                Ok(()) => ReplyBody::Ok,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReplyBody::Ok,
                Err(e) => fail(e),
            },
        }
    }

    fn play_run(&self, program: &str, args: &[String]) -> ReplyBody {
        let ok = |s: String| ReplyBody::Run { status: 0, stdout: s, stderr: String::new() };
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        if program == "/usr/bin/plutil" {
            if a[0] == "-convert" {
                return ok(r#"{"mainScreenWidth":1206,"mainScreenHeight":2622,"mainScreenScale":3}"#.into());
            }
            let plist = self.local(a[5]);
            if !plist.is_file() {
                return ReplyBody::Run { status: 1, stdout: String::new(), stderr: "no such file".into() };
            }
            return ok(match a[1] {
                "CFBundleIdentifier" => "dev.k2.simfixture".into(),
                "CFBundleExecutable" => "K2Fixture".into(),
                "CFBundleShortVersionString" => "1.0".into(),
                _ => String::new(),
            });
        }
        if a.first() == Some(&"xcodebuild") {
            return ok("Xcode 26.6\nBuild version 17F113\n".into());
        }
        // simctl --set <set> <verb> …
        let rest = &a[3..];
        let mut w = self.world.lock().unwrap();
        let set_state = |w: &mut World, id: &str, s: &'static str| {
            for d in w.devices.iter_mut().filter(|d| d.0 == id) {
                d.2 = s;
            }
        };
        match rest {
            ["list", "-j", "runtimes"] => {
                let types: Vec<J> = [(TYPE_17, "iPhone 17"), (TYPE_17_PRO, "iPhone 17 Pro")]
                    .iter()
                    .map(|(id, n)| json!({"identifier": id, "name": n, "productFamily": "iPhone", "bundlePath": format!("{DEVTYPES}/{n}.simdevicetype")}))
                    .collect();
                ok(json!({"runtimes": [{"identifier": RUNTIME, "name": "iOS 26.5", "version": "26.5", "platform": "iOS", "isAvailable": true, "supportedDeviceTypes": types}]}).to_string())
            }
            ["list", "-j", "devices"] => {
                let list: Vec<J> = w.devices.iter().map(|(u, n, s)| json!({"udid": u, "name": n, "state": s, "isAvailable": true, "deviceTypeIdentifier": TYPE_17})).collect();
                ok(json!({"devices": {RUNTIME: list}}).to_string())
            }
            ["create", name, ..] => {
                w.next += 1;
                let u = format!("00000000-0000-0000-0000-{:012}", w.next);
                w.devices.push((u.clone(), name.to_string(), "Shutdown"));
                ok(format!("{u}\n"))
            }
            ["boot", u] => {
                set_state(&mut w, u, "Booted");
                ok(String::new())
            }
            ["shutdown", u] => {
                set_state(&mut w, u, "Shutdown");
                w.shutdowns.push(u.to_string());
                ok(String::new())
            }
            ["delete", u] => {
                w.devices.retain(|d| d.0 != *u);
                ok(String::new())
            }
            ["get_app_container", ..] => ReplyBody::Run { status: 1, stdout: String::new(), stderr: "not installed".into() },
            ["install", _, p] => {
                if !self.local(p).is_dir() {
                    return ReplyBody::Run { status: 1, stdout: String::new(), stderr: "no app there".into() };
                }
                w.installed.push(p.to_string());
                ok(String::new())
            }
            ["launch", _, b] => {
                w.launched.push(b.to_string());
                ok(format!("{b}: 4242\n"))
            }
            ["openurl", _, url] => {
                w.opened.push(url.to_string());
                ok(String::new())
            }
            ["spawn", _, "log", ..] => ok("Timestamp  Ty Process\n2026-10-28 a K2Fixture: hello\n".into()),
            _ => ok(String::new()),
        }
    }
}

/// The daemon's directory, played by the fake node and a grant list.
pub struct FakeDirectory {
    pub node: Arc<FakeNode>,
    pub controller: SigningKey,
    pub grants: Mutex<Vec<(String, String)>>,
    /// More nodes (no fake behind them), for selection tests.
    pub extra: Mutex<Vec<NodeInfo>>,
    /// Frames sent to nodes other than the fake.
    pub sent: Mutex<Vec<(String, String)>>,
    /// Workspaces whose compute node list leaves the fake node off.
    pub off_list: Mutex<Vec<String>>,
}

impl FakeDirectory {
    pub fn new() -> Arc<Self> {
        let controller = SigningKey::generate().unwrap();
        let node = FakeNode::new(&controller);
        Arc::new(Self { node, controller, grants: Mutex::new(vec![]), extra: Mutex::new(vec![]), sent: Mutex::new(vec![]), off_list: Mutex::new(vec![]) })
    }

    pub fn grant(&self, workspace_id: &str) {
        self.grants.lock().unwrap().push((NODE_ID.into(), workspace_id.into()));
    }

    /// The workspace gets a node list without the fake node on it.
    pub fn disallow(&self, workspace_id: &str) {
        self.off_list.lock().unwrap().push(workspace_id.into());
    }

    pub fn allow_again(&self, workspace_id: &str) {
        self.off_list.lock().unwrap().retain(|w| w != workspace_id);
    }
}

impl NodeDirectory for FakeDirectory {
    fn nodes(&self) -> Vec<NodeInfo> {
        let mut v = vec![self.node.info()];
        v.extend(self.extra.lock().unwrap().clone());
        v
    }
    fn granted(&self, node_id: &str, workspace_id: &str) -> bool {
        self.grants.lock().unwrap().iter().any(|(n, w)| n == node_id && w == workspace_id)
    }
    fn allowed(&self, node_id: &str, workspace_id: &str) -> bool {
        node_id != NODE_ID || !self.off_list.lock().unwrap().iter().any(|w| w == workspace_id)
    }
    fn send(&self, node_id: &str, frame: Frame) -> bool {
        if node_id != NODE_ID {
            self.sent.lock().unwrap().push((node_id.into(), frame.kind().into()));
            return true;
        }
        if !*self.node.online.lock().unwrap() {
            return false;
        }
        self.node.on_frame(frame);
        true
    }
    fn node_spki(&self, node_id: &str) -> Option<Vec<u8>> {
        (node_id == NODE_ID).then(|| self.node.key.spki_der().to_vec())
    }
    fn node_by_fp(&self, fp: &str) -> Option<String> {
        (fp == self.node.key.fingerprint()).then(|| NODE_ID.to_string())
    }
    fn controller_key(&self) -> Result<SigningKey, String> {
        SigningKey::from_pkcs8_pem(&self.controller.to_pkcs8_pem()?)
    }
}

/// Did `needle` appear in any recorded relay message?
pub fn relay_carries(node: &FakeNode, needle: &[u8]) -> bool {
    node.relay.lock().unwrap().iter().any(|(_, m)| m.windows(needle.len()).any(|w| w == needle))
}

/// A Simulator `.app` folder like the fixture's.
pub fn fixture_app(dir: &Path) -> PathBuf {
    let a = dir.join("K2Fixture.app");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::write(a.join("Info.plist"), "<plist/>").unwrap();
    std::fs::write(a.join("K2Fixture"), b"\xcf\xfa\xed\xfe").unwrap();
    a
}
