//! `k2-node simhost`: the Simulator service, run as `_k2sim` by the
//! LaunchDaemon `dev.k2.simhost` (§3.3, IR-9, IR-10).
//!
//! It listens on `/var/k2sim/run/simhost.sock` (0660; the folder is
//! `_k2sim:k2simctl` 0750) and answers only its peer uid (`_k2node`), from
//! the kernel's peer credentials. Each connection is one [`Hello`]:
//! facts, the Simulator user's memory, a reset, a workspace's shutdown or
//! erase, or a session ([`super::hostsession`]).
//!
//! At start, and on k2-node's `reset` (IR-7), it shuts every device down:
//! no session survives either process. Every hour it deletes devices and
//! uploads unused for 14 days (the local lane's default) and day-old temp
//! files.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use k2_node_proto::share::{Slot, MAX_SLOTS};
use k2_node_proto::sim::valid_workspace_key;

use super::exec::Exec;
use super::helperbin::HelperSpawner;
use super::hostsession::{list_devices, HostCtx, HostSession};
use super::local::{self, Answer, HostFacts, Hello};
use super::SimRoot;

/// Devices and uploads unused this long are deleted.
pub const KEEP_SECS: u64 = 14 * 24 * 3600;
/// Screenshots and other temp files.
pub const TMP_KEEP_SECS: u64 = 24 * 3600;

pub struct Host {
    pub ctx: Arc<HostCtx>,
    peer_uid: u32,
    /// Builds the helper for an Xcode (facts say whether it builds).
    build_helper: Box<dyn Fn(&str) -> Result<PathBuf, String> + Send + Sync>,
}

fn mkdir(p: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(mode).create(p)
}

impl Host {
    pub fn new(root: SimRoot, exec: Arc<dyn Exec>, helpers: Arc<dyn HelperSpawner>, peer_uid: u32, build_helper: Box<dyn Fn(&str) -> Result<PathBuf, String> + Send + Sync>) -> Arc<Self> {
        Arc::new(Self { ctx: Arc::new(HostCtx { root, exec, helpers, devicetype_roots: Mutex::new(vec![]) }), peer_uid, build_helper })
    }

    fn root(&self) -> &SimRoot {
        &self.ctx.root
    }

    /// Bind the socket (a stale one from an earlier run is replaced; any
    /// other file there is an error) and make it 0660.
    pub fn bind(&self, socket: &Path) -> Result<UnixListener, String> {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        for d in [self.root().sets(), self.root().bin(), self.root().cache_tmp(), self.root().log_dir()] {
            mkdir(&d, 0o700).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        for d in ["tmp", "uploads"] {
            mkdir(&self.root().root.join(d), 0o700).map_err(|e| format!("{d}: {e}"))?;
        }
        match std::fs::symlink_metadata(socket) {
            Ok(m) if m.file_type().is_socket() => std::fs::remove_file(socket).map_err(|e| format!("{}: {e}", socket.display()))?,
            Ok(_) => return Err(format!("{} exists and isn't a socket", socket.display())),
            Err(_) => {}
        }
        let l = UnixListener::bind(socket).map_err(|e| format!("bind {}: {e}", socket.display()))?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660)).map_err(|e| format!("chmod {}: {e}", socket.display()))?;
        Ok(l)
    }

    /// Accept forever, one thread per connection.
    pub fn serve(self: &Arc<Self>, l: UnixListener) {
        for c in l.incoming() {
            let Ok(c) = c else { continue };
            let me = self.clone();
            std::thread::spawn(move || me.handle(c));
        }
    }

    pub fn handle(&self, mut c: UnixStream) {
        match local::peer_uid(&c) {
            Ok(u) if u == self.peer_uid => {}
            _ => {
                let _ = local::write_line(&mut c, &Answer::err("simhost answers only the node"));
                return;
            }
        }
        let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
        let hello: Hello = match local::read_line(&mut c) {
            Ok(h) => h,
            Err(_) => {
                let _ = local::write_line(&mut c, &Answer::err("a hello must be one JSON line"));
                return;
            }
        };
        let t0 = std::time::Instant::now();
        if let Hello::Session { slot, workspace_key, .. } = &hello {
            self.session(c, *slot, workspace_key);
        } else {
            let a = self.answer(&hello);
            let _ = local::write_line(&mut c, &a);
        }
        // Op names and durations only (§3.3); the 5 s memory poll isn't news.
        if !matches!(hello, Hello::Rss) {
            crate::nlog!("simhost: {} {} ms", hello.op(), t0.elapsed().as_millis());
        }
    }

    fn check(slot: Slot, key: &str) -> Result<(), String> {
        if slot >= MAX_SLOTS || !valid_workspace_key(key) {
            return Err("bad slot or workspace key".into());
        }
        Ok(())
    }

    /// Every op but a session.
    pub fn answer(&self, h: &Hello) -> Answer {
        let count = |n: u32| Answer { count: Some(n), ..Answer::ok() };
        match h {
            Hello::Facts => Answer { facts: Some(self.facts()), ..Answer::ok() },
            Hello::Rss => Answer { rss: Some(self.rss()), ..Answer::ok() },
            Hello::Reset => count(self.reset()),
            Hello::Shutdown { slot, workspace_key } => match Self::check(*slot, workspace_key) {
                Ok(()) => count(self.workspace_devices(*slot, workspace_key, false)),
                Err(e) => Answer::err(e),
            },
            Hello::Erase { slot, workspace_key } => match Self::check(*slot, workspace_key) {
                Ok(()) => {
                    let n = self.workspace_devices(*slot, workspace_key, true);
                    let _ = std::fs::remove_dir_all(self.root().tmp(*slot).join(workspace_key));
                    let _ = std::fs::remove_dir_all(self.root().uploads(*slot).join(workspace_key));
                    count(n)
                }
                Err(e) => Answer::err(e),
            },
            Hello::Session { .. } => Answer::err("a session isn't a one-shot op"),
        }
    }

    fn session(&self, mut c: UnixStream, slot: Slot, key: &str) {
        let Ok(w) = c.try_clone() else { return };
        let w = Mutex::new(w);
        let send: super::hostsession::Sender = Arc::new(move |ch, b: &[u8]| local::write_frame(&mut *w.lock().unwrap(), ch, b).is_ok());
        let s = match HostSession::new(self.ctx.clone(), slot, key, send) {
            Ok(s) => s,
            Err(e) => {
                let _ = local::write_line(&mut c, &Answer::err(e));
                return;
            }
        };
        if local::write_line(&mut c, &Answer::ok()).is_err() {
            return;
        }
        let _ = c.set_read_timeout(None);
        while let Ok(Some((ch, payload))) = local::read_frame(&mut c) {
            if s.on_frame(ch, &payload).is_err() {
                break;
            }
        }
        s.end();
    }

    /// The Mac's facts now (and the device type roots the policy uses).
    pub fn facts(&self) -> HostFacts {
        let f = super::probe::probe(&*self.ctx.exec, &self.root().root, |x| (self.build_helper)(x));
        *self.ctx.devicetype_roots.lock().unwrap() = f.devicetype_roots.clone();
        f
    }

    /// RSS of every process of this user (the Simulator's included).
    pub fn rss(&self) -> u64 {
        let uid = unsafe { libc::getuid() };
        match self.ctx.exec.run("/bin/ps", &["-axo".into(), "uid=,rss=".into()], Duration::from_secs(10)) {
            Ok(o) if o.status == 0 => super::probe::rss_of(&o.stdout, uid),
            _ => 0,
        }
    }

    fn set_dirs(&self) -> Vec<(Slot, PathBuf)> {
        (0..MAX_SLOTS).map(|s| (s, self.root().set(s))).filter(|(_, p)| p.is_dir()).collect()
    }

    fn simctl(&self, set: &Path, verb: &str, udid: &str) -> bool {
        let args = vec!["simctl".into(), "--set".into(), set.to_string_lossy().into_owned(), verb.into(), udid.into()];
        self.ctx.exec.run("xcrun", &args, Duration::from_secs(120)).is_ok_and(|o| o.status == 0)
    }

    /// Shut every booted device down, in every slot's set.
    pub fn reset(&self) -> u32 {
        let mut n = 0;
        for (_, set) in self.set_dirs() {
            for (udid, _, state) in list_devices(&*self.ctx.exec, &set.to_string_lossy()).unwrap_or_default() {
                if state == "Booted" && self.simctl(&set, "shutdown", &udid) {
                    n += 1;
                }
            }
        }
        n
    }

    /// Shut down (and with `delete`, delete) one workspace's devices.
    pub fn workspace_devices(&self, slot: Slot, key: &str, delete: bool) -> u32 {
        let set = self.root().set(slot);
        if !set.is_dir() {
            return 0;
        }
        let prefix = format!("K2-{key}--");
        let mut n = 0;
        for (udid, name, state) in list_devices(&*self.ctx.exec, &set.to_string_lossy()).unwrap_or_default() {
            if !name.starts_with(&prefix) {
                continue;
            }
            if state == "Booted" {
                self.simctl(&set, "shutdown", &udid);
            }
            if !delete || self.simctl(&set, "delete", &udid) {
                n += 1;
            }
        }
        n
    }

    /// Devices and uploads unused for [`KEEP_SECS`], temp files after a day.
    pub fn janitor(&self, now: SystemTime) {
        let older = |p: &Path, secs: u64| std::fs::symlink_metadata(p).and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).is_ok_and(|d| d.as_secs() >= secs));
        for (_, set) in self.set_dirs() {
            for (udid, _, state) in list_devices(&*self.ctx.exec, &set.to_string_lossy()).unwrap_or_default() {
                if state != "Booted" && older(&set.join(&udid).join("device.plist"), KEEP_SECS) {
                    self.simctl(&set, "delete", &udid);
                }
            }
        }
        for slot in 0..MAX_SLOTS {
            for (base, keep) in [(self.root().uploads(slot), KEEP_SECS), (self.root().tmp(slot), TMP_KEEP_SECS)] {
                let Ok(keys) = std::fs::read_dir(&base) else { continue };
                for k in keys.flatten() {
                    let Ok(items) = std::fs::read_dir(k.path()) else { continue };
                    for it in items.flatten() {
                        if older(&it.path(), keep) {
                            let p = it.path();
                            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                        }
                    }
                }
            }
        }
    }
}

/// `k2-node simhost [--root /var/k2sim] [--peer-user _k2node]`: the
/// service's main (blocking forever).
pub fn main(root: &Path, peer_user: &str) -> Result<(), String> {
    let peer_uid = uid_of(peer_user).ok_or_else(|| format!("no user {peer_user} on this Mac"))?;
    let root = SimRoot::new(root);
    let exec: Arc<dyn Exec> = Arc::new(super::exec::SystemExec { home: root.root.clone(), tmp: root.cache_tmp() });
    let helpers = Arc::new(super::helperbin::SystemHelpers { bin_dir: root.bin(), home: root.root.clone(), tmp: root.cache_tmp(), exec: exec.clone() });
    let bin_dir = root.bin();
    let x2 = exec.clone();
    let host = Host::new(root.clone(), exec, helpers, peer_uid, Box::new(move |xcode| super::helperbin::ensure_built(&bin_dir, &*x2, xcode)));
    let l = host.bind(&root.socket())?;
    let n = host.reset();
    crate::nlog!("simhost: listening on {}; shut down {n} device(s) left booted", root.socket().display());
    let h2 = host.clone();
    std::thread::spawn(move || loop {
        let _ = h2.facts();
        h2.janitor(SystemTime::now());
        std::thread::sleep(Duration::from_secs(3600));
    });
    host.serve(l);
    Ok(())
}

/// A user's uid by name.
pub fn uid_of(user: &str) -> Option<u32> {
    let c = std::ffi::CString::new(user).ok()?;
    // SAFETY: getpwnam returns a pointer into static storage or null; we
    // copy the uid out at once.
    let p = unsafe { libc::getpwnam(c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).pw_uid })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::hostsession::tests::{FakeHelpers, FakeMac, KEY, UDID};

    fn host(peer: u32) -> (Arc<Host>, Arc<FakeMac>, PathBuf) {
        let d = crate::util::temp_dir("simhost");
        let mac = Arc::new(FakeMac { booted: Mutex::new(true), ..Default::default() });
        let h = Host::new(SimRoot::new(&d), mac.clone(), Arc::new(FakeHelpers::default()), peer, Box::new(|_| Ok(PathBuf::from("/bin/true"))));
        (h, mac, d)
    }

    fn me() -> u32 {
        unsafe { libc::getuid() }
    }

    /// A socket name short enough for any OS (a job's temp dir is too long).
    fn short_sock(tag: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/k2sh-{tag}-{}-{}.sock", std::process::id(), &k2_node_proto::crypto::random_id().unwrap()[..8]))
    }

    #[test]
    fn only_the_node_user_is_answered() {
        let (h, _, d) = host(me().wrapping_add(1));
        let sock = short_sock("peer");
        let l = h.bind(&sock).unwrap();
        let h2 = h.clone();
        std::thread::spawn(move || h2.serve(l));
        let e = local::call(&sock, &Hello::Rss, Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("only the node"), "{e}");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o660);
        assert!(h.bind(&d.join("sets")).unwrap_err().contains("isn't a socket"), "never replaces a real file");
    }

    #[test]
    fn a_session_runs_over_the_socket_and_bad_hellos_are_refused() {
        let (h, mac, d) = host(me());
        let sock = short_sock("sess");
        let l = h.bind(&sock).unwrap();
        let h2 = h.clone();
        std::thread::spawn(move || h2.serve(l));
        assert!(local::call(&sock, &Hello::Shutdown { slot: 0, workspace_key: "web".into() }, Duration::from_secs(5)).is_err());
        assert!(local::open_session(&sock, 0, "../x", &"a".repeat(32)).is_err());
        let mut s = local::open_session(&sock, 0, KEY, &"a".repeat(32)).unwrap();
        let set = d.join("sets/0").to_string_lossy().into_owned();
        let req = k2_node_proto::sim::ctl::Request::Run { id: 1, program: "xcrun".into(), args: vec!["simctl".into(), "--set".into(), set.clone(), "boot".into(), UDID.into()], timeout_ms: 1000 };
        local::write_frame(&mut s, 0, &serde_json::to_vec(&req).unwrap()).unwrap();
        let (ch, body) = local::read_frame(&mut s).unwrap().unwrap();
        assert_eq!(ch, 0);
        let r: k2_node_proto::sim::ctl::FromNode = serde_json::from_slice(&body).unwrap();
        assert!(matches!(r, k2_node_proto::sim::ctl::FromNode::Reply(k2_node_proto::sim::ctl::Reply { id: 1, body: k2_node_proto::sim::ctl::ReplyBody::Run { status: 0, .. } })));
        assert!(mac.calls.lock().unwrap().iter().any(|(_, a)| a.get(3).map(String::as_str) == Some("boot")));
    }

    #[test]
    fn reset_shutdown_and_erase_act_on_the_right_devices() {
        let (h, mac, d) = host(me());
        std::fs::create_dir_all(d.join("sets/0")).unwrap();
        std::fs::create_dir_all(d.join("tmp/0").join(KEY)).unwrap();
        std::fs::create_dir_all(d.join("uploads/0").join(KEY).join("abc")).unwrap();
        assert_eq!(h.answer(&Hello::Reset).count, Some(1), "the booted device in slot 0's set");
        assert_eq!(h.answer(&Hello::Shutdown { slot: 0, workspace_key: KEY.into() }).count, Some(1));
        assert_eq!(h.answer(&Hello::Shutdown { slot: 0, workspace_key: "ffffffffffffffffffffffff".into() }).count, Some(0), "another workspace's devices are left alone");
        let a = h.answer(&Hello::Erase { slot: 0, workspace_key: KEY.into() });
        assert_eq!(a.count, Some(1));
        assert!(!d.join("tmp/0").join(KEY).exists() && !d.join("uploads/0").join(KEY).exists());
        let verbs: Vec<String> = mac.calls.lock().unwrap().iter().filter_map(|(_, a)| a.get(3).cloned()).filter(|v| v != "list").collect();
        assert_eq!(verbs, vec!["shutdown", "shutdown", "shutdown", "delete"]);
        assert!(!h.answer(&Hello::Erase { slot: 9, workspace_key: KEY.into() }).ok);
    }

    #[test]
    fn the_janitor_keeps_recent_things_and_deletes_old_ones() {
        let (h, mac, d) = host(me());
        *mac.booted.lock().unwrap() = false;
        let dev = d.join("sets/0").join(UDID);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("device.plist"), "x").unwrap();
        let up = d.join("uploads/0").join(KEY).join("sha");
        std::fs::create_dir_all(&up).unwrap();
        let shot = d.join("tmp/0").join(KEY).join(".shot.png");
        std::fs::create_dir_all(shot.parent().unwrap()).unwrap();
        std::fs::write(&shot, "png").unwrap();
        h.janitor(SystemTime::now());
        assert!(up.exists() && shot.exists());
        assert!(!mac.calls.lock().unwrap().iter().any(|(_, a)| a.get(3).map(String::as_str) == Some("delete")));
        h.janitor(SystemTime::now() + Duration::from_secs(KEEP_SECS + 60));
        assert!(!up.exists() && !shot.exists());
        assert!(mac.calls.lock().unwrap().iter().any(|(_, a)| a.get(3).map(String::as_str) == Some("delete") && a.get(4).map(String::as_str) == Some(UDID)));
    }

    #[test]
    fn the_users_uid_is_found_by_name() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(uid_of("no-such-user-k2"), None);
    }
}
