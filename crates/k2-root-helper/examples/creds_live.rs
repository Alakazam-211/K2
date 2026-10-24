//! LIVE check of the root helper's `creds` verbs against the real systemd
//! on a Linux test box, as root (`scripts/test/systemd-cred-live.sh` runs
//! it). Never on a box that matters: it writes throwaway units and edits
//! the systemd-cred allow list (restored byte for byte after).
//!
//! It calls the helper's own `creds::{put, bind, restart, unbind, remove,
//! status}` with the real production context (root `/`, real systemctl,
//! busctl and systemd-creds). Only the caller walk (sudo → the daemon's
//! main pid) is skipped: a synthetic daemon caller stands in; that walk has
//! its own unit tests. It checks:
//!  1. put seals the value into /etc/k2/credstore.encrypted (0600, never
//!     the plaintext);
//!  2. bind writes the drop-in, and systemd's real D-Bus answer (busctl)
//!     confirms K2's path; the unit reads the value from %d;
//!  3. restart queues a try-restart; a second within 30 s is refused;
//!  4. a unit that already loads the same credential ID is refused;
//!  5. another daemon can't remove it;
//!  6. a re-put (rotation) reaches the unit;
//!  7. unbind and remove take it all off; status lists no orphans of ours.
//!
//! Guard: refuses unless euid 0, Linux, and K2_CREDS_LIVE_OK=1.

use std::path::Path;
use std::process::Command;

use k2_root_helper::caller::Caller;
use k2_root_helper::creds;
use k2_root_helper::ctx::Ctx;

const ALLOW: &str = "/etc/k2-root-helper/systemd-cred-units";
const ALLOW_DIR: &str = "/etc/k2-root-helper";
const DAEMON_TOML: &str = "/etc/k2-root-helper/daemon.toml";
const STORE: &str = "/etc/k2/credstore.encrypted";

fn sh(args: &[&str]) -> (i32, String) {
    let o = Command::new(args[0]).args(&args[1..]).output().expect("run");
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
}

fn caller(user: &str) -> Caller {
    Caller { pid: 0, uid: None, user: Some(user.into()), is_daemon: true, why_not: None }
}

struct Undo {
    id: String,
    units: Vec<String>,
    allow_before: Option<Vec<u8>>,
    made_allow_dir: bool,
    made_toml: bool,
    made_store: bool,
    made_etc_k2: bool,
}

impl Drop for Undo {
    fn drop(&mut self) {
        let ctx = Ctx::production();
        let a = caller("k2probe-a");
        for u in &self.units {
            let _ = sh(&["/usr/bin/systemctl", "stop", u]);
            let _ = creds::unbind(&ctx, &a, u, &format!("{}_key", self.id));
            let _ = std::fs::remove_file(format!("/etc/systemd/system/{u}"));
            let _ = std::fs::remove_dir_all(format!("/etc/systemd/system/{u}.d"));
        }
        let _ = creds::remove(&ctx, &a, &format!("{}_key", self.id));
        // Drop this run's restart stamps from creds.json.
        if let Ok(mut st) = creds::load_state(&ctx) {
            st.restarts.retain(|u, _| !u.starts_with(&self.id));
            st.creds.retain(|c| !c.name.starts_with(&self.id));
            st.dropins.retain(|d| !d.unit.starts_with(&self.id));
            let _ = creds::save_state(&ctx, &st);
        }
        match &self.allow_before {
            Some(b) => {
                let _ = std::fs::write(ALLOW, b);
            }
            None => {
                let _ = std::fs::remove_file(ALLOW);
            }
        }
        if self.made_toml {
            let _ = std::fs::remove_file(DAEMON_TOML);
        }
        if self.made_allow_dir {
            let _ = std::fs::remove_dir(ALLOW_DIR);
        }
        if self.made_store {
            let _ = std::fs::remove_dir(STORE);
        }
        if self.made_etc_k2 {
            let _ = std::fs::remove_dir("/etc/k2");
        }
        let _ = std::fs::remove_file(format!("/run/{}.out", self.id));
        let _ = sh(&["/usr/bin/systemctl", "daemon-reload"]);
        eprintln!("cleanup done ({})", self.id);
    }
}

fn fail(msg: String) -> ! {
    // A panic, not exit(): the undo guard still runs (cleanup on failure).
    panic!("FAIL: {msg}")
}

fn main() {
    if !cfg!(target_os = "linux") || unsafe { libc::geteuid() } != 0 || std::env::var("K2_CREDS_LIVE_OK").as_deref() != Ok("1") {
        eprintln!("CANNOT RUN: needs Linux, root and K2_CREDS_LIVE_OK=1 (a test box only)");
        std::process::exit(77);
    }
    let rnd = std::fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap_or_default().replace('-', "");
    let id = format!("opsprobe{}", &rnd.trim()[..8]);
    let unit = format!("{id}.service");
    let dup = format!("{id}-dup.service");
    let cred = format!("{id}_key");
    let undo = Undo {
        id: id.clone(),
        units: vec![unit.clone(), dup.clone()],
        allow_before: std::fs::read(ALLOW).ok(),
        made_allow_dir: !Path::new(ALLOW_DIR).exists(),
        made_toml: !Path::new(DAEMON_TOML).exists(),
        made_store: !Path::new(STORE).exists(),
        made_etc_k2: !Path::new("/etc/k2").exists(),
    };
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(ALLOW_DIR).unwrap();
    std::fs::set_permissions(ALLOW_DIR, std::fs::Permissions::from_mode(0o755)).unwrap();
    if undo.made_toml {
        std::fs::write(DAEMON_TOML, "[[daemon]]\nuser = \"k2probe-a\"\nunit = \"k2test-probe-daemon.service\"\nscope = \"system\"\n").unwrap();
        std::fs::set_permissions(DAEMON_TOML, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let mut allow = undo.allow_before.clone().unwrap_or_default();
    if !allow.is_empty() && !allow.ends_with(b"\n") {
        allow.push(b'\n');
    }
    allow.extend_from_slice(format!("{unit}\n{dup}\n").as_bytes());
    std::fs::write(ALLOW, &allow).unwrap();
    std::fs::set_permissions(ALLOW, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(
        format!("/etc/systemd/system/{unit}"),
        format!("[Unit]\nDescription=k2 creds live probe\n[Service]\nType=oneshot\nExecStart=/bin/sh -c 'cat \"%d/{cred}\" > /run/{id}.out'\n"),
    )
    .unwrap();
    std::fs::write(
        format!("/etc/systemd/system/{dup}"),
        format!("[Unit]\nDescription=k2 creds live probe (same id)\n[Service]\nType=oneshot\nLoadCredential={cred}:/etc/hostname\nExecStart=/bin/true\n"),
    )
    .unwrap();
    sh(&["/usr/bin/systemctl", "daemon-reload"]);

    let ctx = Ctx::production();
    let a = caller("k2probe-a");
    let b = caller("k2probe-b");
    let v1 = format!("k2test-live-one-{id}");
    let v2 = format!("k2test-live-two-{id}");

    // 1. put
    let r = creds::put(&ctx, &a, &cred, v1.as_bytes()).unwrap_or_else(|f| fail(format!("put: {}", f.msg)));
    let path = format!("{STORE}/{cred}.cred");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| fail(format!("{path}: {e}")));
    if bytes.windows(v1.len()).any(|w| w == v1.as_bytes()) {
        fail("the plaintext is in the .cred".into());
    }
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    if mode != 0o600 {
        fail(format!(".cred mode {mode:o}"));
    }
    println!("  ok put: {path} 0600 (tpm2: {})", r["tpm2"]);

    // 2. bind (busctl post-check) and the unit reads it
    creds::bind(&ctx, &a, &unit, &cred).unwrap_or_else(|f| fail(format!("bind: {}", f.msg)));
    let (c, out) = sh(&["/usr/bin/busctl", "--json=short", "get-property", "org.freedesktop.systemd1",
        &format!("/org/freedesktop/systemd1/unit/{}", k2_root_proto::creds::bus_escape(&unit)),
        "org.freedesktop.systemd1.Service", "LoadCredentialEncrypted"]);
    println!("     busctl LoadCredentialEncrypted ({c}): {}", out.trim());
    let (c, _) = sh(&["/usr/bin/systemctl", "start", &unit]);
    let got = std::fs::read_to_string(format!("/run/{id}.out")).unwrap_or_default();
    if c != 0 || got != v1 {
        fail(format!("the unit read {got:?} (start exit {c})"));
    }
    println!("  ok bind: systemd loads K2's path (checked over D-Bus); the unit reads %d/{cred}");

    // 3. restart, rate-limited
    creds::restart(&ctx, &a, &unit).unwrap_or_else(|f| fail(format!("restart: {}", f.msg)));
    match creds::restart(&ctx, &a, &unit) {
        Err(f) if f.msg.contains("30 s") => println!("  ok restart queued; a second within 30 s refused"),
        other => fail(format!("second restart: {:?}", other.map(|_| ()).map_err(|f| f.msg))),
    }

    // 4. a unit that already loads the same ID
    match creds::bind(&ctx, &a, &dup, &cred) {
        Err(f) if f.msg.contains("already gets a credential") => println!("  ok a unit that already loads {cred} is refused"),
        other => fail(format!("bind onto a unit with the same id: {:?}", other.map(|_| ()).map_err(|f| f.msg))),
    }
    if Path::new(&format!("/etc/systemd/system/{dup}.d/k2-cred-{cred}.conf")).exists() {
        fail("a drop-in was left on the refused unit".into());
    }

    // 5. another daemon
    match creds::remove(&ctx, &b, &cred) {
        Err(f) if f.msg.contains("another K2 daemon") => println!("  ok another daemon can't remove it"),
        other => fail(format!("daemon b remove: {:?}", other.map(|_| ()).map_err(|f| f.msg))),
    }

    // 6. a re-put reaches the unit
    creds::put(&ctx, &a, &cred, v2.as_bytes()).unwrap_or_else(|f| fail(format!("re-put: {}", f.msg)));
    let (c, _) = sh(&["/usr/bin/systemctl", "start", &unit]);
    let got = std::fs::read_to_string(format!("/run/{id}.out")).unwrap_or_default();
    if c != 0 || got != v2 {
        fail(format!("after a re-put the unit read {got:?}"));
    }
    println!("  ok re-put: the unit reads the new value");

    // 7. unbind, remove, status
    creds::unbind(&ctx, &a, &unit, &cred).unwrap_or_else(|f| fail(format!("unbind: {}", f.msg)));
    creds::remove(&ctx, &a, &cred).unwrap_or_else(|f| fail(format!("remove: {}", f.msg)));
    if Path::new(&path).exists() || Path::new(&format!("/etc/systemd/system/{unit}.d/k2-cred-{cred}.conf")).exists() {
        fail("unbind/remove left a file".into());
    }
    let s = creds::status(&ctx, &a).unwrap_or_else(|f| fail(format!("status: {}", f.msg)));
    if s["orphans"].as_array().is_some_and(|o| o.iter().any(|x| x["path"].as_str().is_some_and(|p| p.contains(&id)))) {
        fail(format!("status lists an orphan of this run: {s}"));
    }
    println!("  ok unbind + remove: nothing left; status lists no orphan of this run");
    drop(undo);
    println!("PASS: creds live ({unit})");
}
