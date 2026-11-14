//! simhost's session rules, with a recording `simctl` and a helper played
//! in memory (no Mac needed): what reaches the Mac's tools, what is refused
//! before it does, uploads, helpers and the one-frame-in-flight gate.

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_node_proto::crypto;
use k2_node_proto::sim::ctl::{DiskOp, Event, FromNode, ReplyBody, Request};
use k2_node_proto::sim::codes;
use serde_json::{json, Value as J};

use super::*;
use crate::sim::exec::{Exec, ExecError, Out};
use crate::sim::helperbin::{HelperPipes, HelperSpawner};
use crate::sim::SimRoot;

pub const KEY: &str = "0123456789abcdef01234567";
pub const UDID: &str = "AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB";

/// A Mac's tools, recorded: `list -j devices` shows UDID booted.
#[derive(Default)]
pub struct FakeMac {
    pub calls: Mutex<Vec<(String, Vec<String>)>>,
    pub booted: Mutex<bool>,
}

impl Exec for FakeMac {
    fn run(&self, program: &str, args: &[String], _t: Duration) -> Result<Out, ExecError> {
        self.calls.lock().unwrap().push((program.to_string(), args.to_vec()));
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        if a.len() >= 6 && a[0] == "simctl" && a[3..] == ["list", "-j", "devices"] {
            let state = if *self.booted.lock().unwrap() { "Booted" } else { "Shutdown" };
            let name = format!("K2-{KEY}--iPhone-17");
            return Ok(Out { status: 0, stdout: json!({"devices": {"r": [{"udid": UDID, "name": name, "state": state}]}}).to_string(), stderr: String::new() });
        }
        Ok(Out { status: 0, stdout: "ok\n".into(), stderr: String::new() })
    }
}

/// A helper played by a thread: `ready` first, one reply per line, and
/// three frames at once after `frames on`. A `button` named `die` makes
/// it exit (an op a server may send, so the line gets through).
#[derive(Default)]
pub struct FakeHelpers {
    pub lines: Arc<Mutex<Vec<J>>>,
}

fn framed(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 1) as u32).to_be_bytes().to_vec();
    v.push(kind);
    v.extend_from_slice(body);
    v
}

impl HelperSpawner for FakeHelpers {
    fn spawn(&self, _set: &std::path::Path, _udid: &str) -> Result<HelperPipes, String> {
        let (in_rx, in_tx) = std::io::pipe().unwrap();
        let (out_rx, mut out_tx) = std::io::pipe().unwrap();
        let (fr_rx, mut fr_tx) = std::io::pipe().unwrap();
        let lines = self.lines.clone();
        out_tx.write_all(&framed(b'J', br#"{"event":"ready","version":4,"accepts":[3,4]}"#)).unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(in_rx).lines() {
                let Ok(l) = l else { break };
                let v: J = serde_json::from_str(&l).unwrap();
                lines.lock().unwrap().push(v.clone());
                if v["op"] == "button" && v["name"] == "die" {
                    break;
                }
                if v["op"] == "frames" && v["on"] == true {
                    for n in 1..=3u8 {
                        fr_tx.write_all(&framed(b'F', &[0, 0, 0, n])).unwrap();
                    }
                }
                let _ = out_tx.write_all(&framed(b'J', json!({"id": v["id"], "ok": true}).to_string().as_bytes()));
            }
        });
        Ok(HelperPipes { stdin: Box::new(in_tx), stdout: Box::new(out_rx), frames: Box::new(fr_rx), kill: Box::new(|| {}) })
    }
}

pub struct Rig {
    pub root: std::path::PathBuf,
    pub mac: Arc<FakeMac>,
    pub helpers: Arc<FakeHelpers>,
    pub sent: Arc<Mutex<Vec<(u8, Vec<u8>)>>>,
    pub s: Arc<HostSession>,
}

pub fn rig(slot: u8) -> Rig {
    let root = crate::util::temp_dir("simhostsess");
    let mac = Arc::new(FakeMac { booted: Mutex::new(true), ..Default::default() });
    let helpers = Arc::new(FakeHelpers::default());
    let ctx = Arc::new(HostCtx { root: SimRoot::new(&root), exec: mac.clone(), helpers: helpers.clone(), devicetype_roots: Mutex::new(vec![]) });
    let sent: Arc<Mutex<Vec<(u8, Vec<u8>)>>> = Arc::default();
    let s2 = sent.clone();
    let s = HostSession::new(ctx, slot, KEY, Arc::new(move |c, b: &[u8]| {
        s2.lock().unwrap().push((c, b.to_vec()));
        true
    }))
    .unwrap();
    Rig { root, mac, helpers, sent, s }
}

impl Rig {
    pub fn req(&self, r: &Request) {
        self.s.on_frame(0, &serde_json::to_vec(r).unwrap()).unwrap();
    }

    /// The reply to `id`, waiting up to 10 s.
    pub fn reply(&self, id: u64) -> ReplyBody {
        let t0 = Instant::now();
        loop {
            for (c, b) in self.sent.lock().unwrap().iter() {
                if *c == 0 {
                    if let Ok(FromNode::Reply(r)) = serde_json::from_slice::<FromNode>(b) {
                        if r.id == id {
                            return r.body;
                        }
                    }
                }
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "no reply to {id}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn on(&self, channel: u8) -> Vec<Vec<u8>> {
        self.sent.lock().unwrap().iter().filter(|(c, _)| *c == channel).map(|(_, b)| b.clone()).collect()
    }

    pub fn set(&self, slot: u8) -> String {
        self.root.join("sets").join(slot.to_string()).to_string_lossy().into_owned()
    }

    pub fn ws_tmp(&self) -> String {
        format!("{}/tmp/0/{KEY}", self.root.display())
    }
}

fn simctl(set: &str, rest: &[&str]) -> Vec<String> {
    let mut v = vec!["simctl".to_string(), "--set".into(), set.into()];
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}

fn wait(cond: impl Fn() -> bool, what: &str) {
    let t0 = Instant::now();
    while !cond() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn allowed_runs_reach_simctl_and_refused_ones_never_do() {
    let r = rig(0);
    let set = r.set(0);
    r.req(&Request::Run { id: 1, program: "xcrun".into(), args: simctl(&set, &["boot", UDID]), timeout_ms: 1000 });
    assert_eq!(r.reply(1), ReplyBody::Run { status: 0, stdout: "ok\n".into(), stderr: String::new() });
    let bad = [
        simctl(&set, &["spawn", UDID, "sh", "-c", "id"]),
        simctl(&r.set(1), &["list", "-j", "devices"]),
        vec!["clang".into(), "-o".into(), "/tmp/x".into()],
        simctl(&set, &["openurl", UDID, "file:///var/k2node/"]),
        simctl(&set, &["install", UDID, "/var/k2node/state/x.app"]),
    ];
    for (i, args) in bad.iter().enumerate() {
        let id = 10 + i as u64;
        r.req(&Request::Run { id, program: "xcrun".into(), args: args.clone(), timeout_ms: 1000 });
        match r.reply(id) {
            ReplyBody::Refused { code, .. } => assert_eq!(code, codes::NODE_REFUSED, "{args:?}"),
            other => panic!("{args:?} must be refused, got {other:?}"),
        }
    }
    r.req(&Request::Run { id: 20, program: "/bin/sh".into(), args: vec!["-c".into(), "id".into()], timeout_ms: 1000 });
    assert!(matches!(r.reply(20), ReplyBody::Refused { .. }));
    let calls = r.mac.calls.lock().unwrap().clone();
    assert_eq!(calls, vec![("xcrun".to_string(), simctl(&set, &["boot", UDID]))], "only the allowed call reached the Mac");
}

#[test]
fn disk_ops_stay_in_the_workspace_folder() {
    let r = rig(0);
    let ws = r.ws_tmp();
    r.req(&Request::Disk { id: 1, op: DiskOp::EnsureDir, path: ws.clone() });
    assert_eq!(r.reply(1), ReplyBody::Ok);
    std::fs::write(format!("{ws}/.shot-1.png"), b"png").unwrap();
    r.req(&Request::Disk { id: 2, op: DiskOp::Read, path: format!("{ws}/.shot-1.png") });
    assert_eq!(r.reply(2), ReplyBody::Bytes { data: crypto::b64(b"png") });
    r.req(&Request::Disk { id: 3, op: DiskOp::Read, path: format!("{}/run/simhost.sock", r.root.display()) });
    assert!(matches!(r.reply(3), ReplyBody::Refused { .. }));
    r.req(&Request::Disk { id: 4, op: DiskOp::RemoveDirAll, path: format!("{}/tmp/0", r.root.display()) });
    assert!(matches!(r.reply(4), ReplyBody::Refused { .. }), "never the slot's whole folder");
    std::os::unix::fs::symlink("/etc/hosts", format!("{ws}/link.png")).unwrap();
    r.req(&Request::Disk { id: 5, op: DiskOp::Read, path: format!("{ws}/link.png") });
    assert!(matches!(r.reply(5), ReplyBody::Failed { .. }), "never read through a link");
}

fn upload(r: &Rig, first_id: u64, bytes: &[u8]) -> ReplyBody {
    let sha = crypto::sha256_hex(bytes);
    let chunks: Vec<&[u8]> = bytes.chunks(1000).collect();
    let mut off = 0u64;
    for (i, c) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let id = first_id + i as u64;
        r.req(&Request::Upload { id, sha256: sha.clone(), total: bytes.len() as u64, offset: off, data: crypto::b64(c), last });
        off += c.len() as u64;
        let b = r.reply(id);
        if last || b != ReplyBody::Ok {
            return b;
        }
    }
    unreachable!()
}

#[test]
fn uploads_unpack_under_the_workspace_and_hostile_tars_are_refused() {
    let r = rig(0);
    let src = crate::util::temp_dir("simapp").join("K2Fixture.app");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("Info.plist"), "<plist/>").unwrap();
    std::fs::write(src.join("K2Fixture"), vec![7u8; 3000]).unwrap();
    let bytes = k2_node_proto::sim::pack::pack_app(&src).unwrap();
    let ReplyBody::Path { path } = upload(&r, 1, &bytes) else { panic!("no path") };
    let want = format!("{}/uploads/0/{KEY}/{}/K2Fixture.app", r.root.display(), crypto::sha256_hex(&bytes));
    assert_eq!(path, want);
    assert_eq!(std::fs::read(format!("{path}/K2Fixture")).unwrap(), vec![7u8; 3000]);
    // Uploading the same app again replaces it.
    assert!(matches!(upload(&r, 100, &bytes), ReplyBody::Path { .. }));

    let mut h = tar::Header::new_old();
    let name = "../escape";
    h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(1);
    h.set_mode(0o644);
    h.set_cksum();
    let mut b = tar::Builder::new(Vec::new());
    b.append(&h, &b"x"[..]).unwrap();
    let evil = b.into_inner().unwrap();
    assert!(matches!(upload(&r, 200, &evil), ReplyBody::Failed { .. }));
    assert!(!r.root.join("uploads/0").join(KEY).join("escape").exists() && !r.root.join("uploads/0/escape").exists());
    // A wrong checksum, a sha that isn't hex, and chunks out of order.
    r.req(&Request::Upload { id: 300, sha256: "0".repeat(64), total: 3, offset: 0, data: crypto::b64(b"abc"), last: true });
    assert!(matches!(r.reply(300), ReplyBody::Failed { .. }));
    r.req(&Request::Upload { id: 301, sha256: "../../x".into(), total: 3, offset: 0, data: crypto::b64(b"abc"), last: true });
    assert!(matches!(r.reply(301), ReplyBody::Failed { .. }));
    r.req(&Request::Upload { id: 302, sha256: "0".repeat(64), total: 6, offset: 3, data: crypto::b64(b"abc"), last: false });
    assert!(matches!(r.reply(302), ReplyBody::Failed { .. }));
}

#[test]
fn helpers_start_only_for_a_booted_device_and_their_lines_are_policed() {
    let r = rig(0);
    *r.mac.booted.lock().unwrap() = false;
    r.req(&Request::HelperStart { id: 1, channel: 1, udid: UDID.into() });
    assert!(matches!(r.reply(1), ReplyBody::Refused { .. }), "not booted in this set");
    r.req(&Request::HelperStart { id: 2, channel: 1, udid: "booted".into() });
    assert!(matches!(r.reply(2), ReplyBody::Refused { .. }));
    *r.mac.booted.lock().unwrap() = true;
    r.req(&Request::HelperStart { id: 3, channel: 1, udid: UDID.into() });
    assert_eq!(r.reply(3), ReplyBody::Ok);
    assert!(r.on(1).iter().any(|m| m.windows(5).any(|w| w == b"ready")), "ready goes to the server");
    r.req(&Request::HelperStart { id: 4, channel: 1, udid: UDID.into() });
    assert!(matches!(r.reply(4), ReplyBody::Refused { .. }), "channel taken");

    r.s.on_frame(1, br#"{"id":7,"op":"tap","x":10,"y":20}"#).unwrap();
    let image_ok = format!(r#"{{"id":8,"op":"image","path":"{}/.shot-8.png"}}"#, r.ws_tmp());
    r.s.on_frame(1, image_ok.as_bytes()).unwrap();
    r.s.on_frame(1, br#"{"id":9,"op":"image","path":"/var/k2node/state/x.png"}"#).unwrap();
    r.s.on_frame(1, br#"{"id":10,"op":"quit"}"#).unwrap();
    wait(|| r.helpers.lines.lock().unwrap().len() >= 2, "two lines at the helper");
    let lines = r.helpers.lines.lock().unwrap().clone();
    assert_eq!(lines.iter().map(|l| l["op"].as_str().unwrap().to_string()).collect::<Vec<_>>(), vec!["tap", "image"], "refused lines never reach it");
    let refused: Vec<J> = r.on(1).iter().filter_map(|m| serde_json::from_slice::<J>(&m[5..]).ok()).filter(|v| v["ok"] == false).collect();
    assert_eq!(refused.iter().map(|v| v["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![9, 10]);
    assert!(refused.iter().all(|v| v["code"] == codes::NODE_REFUSED));
}

#[test]
fn frames_go_one_at_a_time_and_the_newest_wins() {
    let r = rig(0);
    r.req(&Request::HelperStart { id: 1, channel: 2, udid: UDID.into() });
    assert_eq!(r.reply(1), ReplyBody::Ok);
    r.s.on_frame(2, br#"{"id":5,"op":"frames","on":true,"fps":8}"#).unwrap();
    let frames = || r.on(2).into_iter().filter(|m| m[4] == b'F').map(|m| m[8]).collect::<Vec<u8>>();
    wait(|| !frames().is_empty(), "the first frame");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(frames(), vec![1], "one in flight until the server acks");
    r.req(&Request::Ack { channel: 2 });
    assert_eq!(frames(), vec![1, 3], "the newest waiting frame goes; the older one is dropped");
    r.req(&Request::Ack { channel: 2 });
    assert_eq!(frames(), vec![1, 3]);
}

#[test]
fn a_helper_that_exits_ends_its_channel() {
    let r = rig(0);
    r.req(&Request::HelperStart { id: 1, channel: 3, udid: UDID.into() });
    assert_eq!(r.reply(1), ReplyBody::Ok);
    r.s.on_frame(3, br#"{"id":2,"op":"button","name":"die"}"#).unwrap();
    wait(
        || r.on(0).iter().any(|m| matches!(serde_json::from_slice::<FromNode>(m), Ok(FromNode::Event(Event::HelperGone { channel: 3, .. })))),
        "helper_gone",
    );
    r.req(&Request::HelperStart { id: 4, channel: 3, udid: UDID.into() });
    assert_eq!(r.reply(4), ReplyBody::Ok, "the channel is free again");
}

#[test]
fn the_gate_keeps_one_frame_in_flight() {
    let mut g = Gate::default();
    assert_eq!(g.offer(vec![1]), Some(vec![1]));
    assert_eq!(g.offer(vec![2]), None);
    assert_eq!(g.offer(vec![3]), None);
    assert_eq!(g.ack(), Some(vec![3]));
    assert_eq!(g.ack(), None);
    assert_eq!(g.offer(vec![4]), Some(vec![4]));
}

#[test]
fn a_bad_session_or_control_message_is_refused() {
    let root = crate::util::temp_dir("simhostbad");
    let ctx = Arc::new(HostCtx { root: SimRoot::new(&root), exec: Arc::new(FakeMac::default()), helpers: Arc::new(FakeHelpers::default()), devicetype_roots: Mutex::new(vec![]) });
    assert!(HostSession::new(ctx.clone(), 0, "web", Arc::new(|_, _: &[u8]| true)).is_err(), "never a handle");
    assert!(HostSession::new(ctx.clone(), 9, KEY, Arc::new(|_, _: &[u8]| true)).is_err());
    let s = HostSession::new(ctx, 0, KEY, Arc::new(|_, _: &[u8]| true)).unwrap();
    assert!(s.on_frame(0, b"not json").is_err());
}
