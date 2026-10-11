//! Helper tests over a temp root with fake processes (§14). Nothing here
//! runs sudo, executes a helper as root, writes /etc, /usr/local or
//! /var/lib, or calls the real systemctl. Run as a non-root user.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use k2_root_proto::paths::*;
use k2_root_proto::sig::{encode_payload, trusted_comment};
use k2_root_proto::testsign::{fake_elf, sign_with_test_key};
use k2_root_proto::verbs::{catalog, exit, Class};
use k2_root_proto::version::{format_marker, PROTOCOL};

use crate::ctx::{under, AuditSink, Ctx, Exec, ExecOut, ProcHandle, Procs, Random};
use crate::run;

// ── Fakes ───────────────────────────────────────────────────────────────

#[derive(Default)]
struct ExecLog {
    calls: Vec<(String, Vec<String>, Option<Vec<u8>>)>,
    /// `systemctl show … MainPID` answer per unit.
    main_pids: HashMap<String, i32>,
    /// prog → (status, stdout, stderr)
    answers: HashMap<String, ExecOut>,
    /// `systemctl --user -M` fails (no user D-Bus), like a server image.
    no_user_bus: bool,
}

#[derive(Clone, Default)]
struct FakeExec(Rc<RefCell<ExecLog>>);

impl Exec for FakeExec {
    fn run(&self, prog: &str, args: &[String], stdin: Option<&[u8]>) -> io::Result<ExecOut> {
        let mut log = self.0.borrow_mut();
        log.calls
            .push((prog.to_string(), args.to_vec(), stdin.map(|b| b.to_vec())));
        let via_runuser = prog == RUNUSER && args.iter().any(|a| a == SYSTEMCTL);
        if (prog == SYSTEMCTL || via_runuser) && args.iter().any(|a| a == "MainPID") {
            if prog == SYSTEMCTL && log.no_user_bus && args.iter().any(|a| a == "-M") {
                return Ok(ExecOut {
                    status: 1,
                    stdout: vec![],
                    stderr: b"Transport endpoint is not connected".to_vec(),
                });
            }
            let unit = args.last().cloned().unwrap_or_default();
            return Ok(match log.main_pids.get(&unit) {
                Some(p) => ExecOut {
                    status: 0,
                    stdout: format!("{p}\n").into_bytes(),
                    stderr: vec![],
                },
                None => ExecOut {
                    status: 0,
                    stdout: b"0\n".to_vec(),
                    stderr: vec![],
                },
            });
        }
        Ok(log.answers.get(prog).cloned().unwrap_or_default())
    }
}

#[derive(Clone)]
struct P {
    exe: PathBuf,
    exe_owner: u32,
    ppid: i32,
    ruid: u32,
}

#[derive(Default)]
struct ProcTable {
    parent: i32,
    procs: HashMap<i32, P>,
    dead: HashSet<i32>,
    users: HashMap<String, u32>,
}

#[derive(Clone, Default)]
struct FakeProcs(Rc<RefCell<ProcTable>>);

struct H(i32, Rc<RefCell<ProcTable>>);
impl ProcHandle for H {
    fn alive(&self) -> bool {
        !self.1.borrow().dead.contains(&self.0)
    }
}

impl Procs for FakeProcs {
    fn parent_pid(&self) -> i32 {
        self.0.borrow().parent
    }
    fn hold(&self, pid: i32) -> Option<Box<dyn ProcHandle>> {
        self.0
            .borrow()
            .procs
            .contains_key(&pid)
            .then(|| Box::new(H(pid, self.0.clone())) as Box<dyn ProcHandle>)
    }
    fn exe(&self, pid: i32) -> Option<(PathBuf, u32)> {
        self.0
            .borrow()
            .procs
            .get(&pid)
            .map(|p| (p.exe.clone(), p.exe_owner))
    }
    fn ppid_of(&self, pid: i32) -> Option<i32> {
        self.0.borrow().procs.get(&pid).map(|p| p.ppid)
    }
    fn ruid_of(&self, pid: i32) -> Option<u32> {
        self.0.borrow().procs.get(&pid).map(|p| p.ruid)
    }
    fn uid_of_user(&self, user: &str) -> Option<u32> {
        self.0.borrow().users.get(user).copied()
    }
}

struct CountRandom(RefCell<u8>);
impl Random for CountRandom {
    fn fill(&self, buf: &mut [u8]) -> io::Result<()> {
        let mut n = self.0.borrow_mut();
        for b in buf.iter_mut() {
            *n = n.wrapping_add(1);
            *b = *n;
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct MemAudit(Rc<RefCell<Vec<String>>>);
impl AuditSink for MemAudit {
    fn line(&self, json: &str) {
        self.0.borrow_mut().push(json.to_string());
    }
}

const DAEMON_PID: i32 = 4242;
const AGENT_PID: i32 = 5151;
const SUDO_PID: i32 = 6000;
const K2_UID: u32 = 1001;

struct T {
    _dir: tempfile::TempDir,
    root: PathBuf,
    ctx: Ctx,
    exec: FakeExec,
    procs: FakeProcs,
    audit: MemAudit,
}

fn mkdirs(root: &Path, dirs: &[&str]) {
    for d in dirs {
        std::fs::create_dir_all(under(root, d)).unwrap();
    }
}

/// A temp root shaped like a provisioned box: daemon.toml for user k2 /
/// k2-daemon.service, the helper installed, a daemon (pid 4242) whose
/// child is an agent (pid 5151). By default the caller is the daemon via
/// `sudo` as the parent.
fn box_root() -> T {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    mkdirs(
        &root,
        &[
            "/etc/k2-root-helper",
            "/etc/systemd/system",
            "/etc/sudoers.d",
            "/usr/local/libexec",
            "/usr/local/bin",
            "/var/lib",
            "/var/log",
            "/run",
        ],
    );
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n";
    std::fs::write(under(&root, DAEMON_TOML), toml).unwrap();
    std::fs::set_permissions(
        under(&root, DAEMON_TOML),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::fs::write(
        under(&root, HELPER_PATH),
        fake_elf("x86_64", "old helper bytes"),
    )
    .unwrap();
    std::fs::set_permissions(
        under(&root, HELPER_PATH),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    for d in ["/usr/local/libexec", "/usr/local", "/usr"] {
        std::fs::set_permissions(under(&root, d), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let exec = FakeExec::default();
    exec.0
        .borrow_mut()
        .main_pids
        .insert("k2-daemon.service".into(), DAEMON_PID);
    let procs = FakeProcs::default();
    {
        let mut t = procs.0.borrow_mut();
        t.users.insert("k2".into(), K2_UID);
        t.procs.insert(
            DAEMON_PID,
            P {
                exe: "/home/k2/.local/bin/k2-daemon".into(),
                exe_owner: K2_UID,
                ppid: 1,
                ruid: K2_UID,
            },
        );
        t.procs.insert(
            AGENT_PID,
            P {
                exe: "/usr/bin/bash".into(),
                exe_owner: 0,
                ppid: DAEMON_PID,
                ruid: K2_UID,
            },
        );
        t.procs.insert(
            SUDO_PID,
            P {
                exe: "/usr/bin/sudo".into(),
                exe_owner: 0,
                ppid: DAEMON_PID,
                ruid: K2_UID,
            },
        );
        t.parent = SUDO_PID;
    }
    let audit = MemAudit::default();
    let ctx = Ctx {
        root: root.clone(),
        real_owners: false,
        euid: 0,
        arch: "x86_64",
        self_exe: under(&root, HELPER_PATH),
        test_keys: true,
        exec: Box::new(exec.clone()),
        procs: Box::new(procs.clone()),
        rand: Box::new(CountRandom(RefCell::new(0))),
        audit: Box::new(audit.clone()),
        out: RefCell::new(Vec::new()),
        lock_timeout_ms: 300,
    };
    T {
        _dir: dir,
        root,
        ctx,
        exec,
        procs,
        audit,
    }
}

impl T {
    fn call(&self, args: &[&str]) -> crate::Outcome {
        run(&self.ctx, args, io::empty())
    }
    fn call_in(&self, args: &[&str], stdin: &[u8]) -> crate::Outcome {
        run(&self.ctx, args, stdin)
    }
    fn p(&self, abs: &str) -> PathBuf {
        under(&self.root, abs)
    }
    /// The caller is the agent (a child of the daemon) via sudo.
    fn as_agent(&self) {
        let mut t = self.procs.0.borrow_mut();
        t.procs.get_mut(&SUDO_PID).unwrap().ppid = AGENT_PID;
    }
    fn calls(&self) -> Vec<(String, Vec<String>, Option<Vec<u8>>)> {
        self.exec.0.borrow().calls.clone()
    }
    fn non_query_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|(p, a, _)| !(p == SYSTEMCTL && a.iter().any(|x| x == "MainPID")))
            .map(|(p, a, _)| format!("{p} {}", a.join(" ")))
            .collect()
    }
}

fn ok(o: &crate::Outcome) {
    assert_eq!(o.code, exit::OK, "stderr: {}", o.stderr);
}

/// A reader that fails the test if it is read at all.
struct NoRead;
impl io::Read for NoRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("stdin read for a verb that takes none");
    }
}

// ── Core rules ──────────────────────────────────────────────────────────

#[test]
fn refused_argv_exits_2_reads_no_stdin_and_is_audited() {
    let t = box_root();
    let o = run(&t.ctx, &["mail", "remove", "/var/lib/stalwart"], NoRead);
    assert_eq!(o.code, exit::ARGV);
    assert!(
        !o.stderr.contains("/var/lib"),
        "caller bytes never echoed: {}",
        o.stderr
    );
    let a = t.audit.0.borrow();
    assert!(a.last().unwrap().contains("refused-argv"));
    // Verbs without stdin never read it.
    drop(a);
    let o = run(&t.ctx, &["mail", "usage"], NoRead);
    assert_eq!(o.code, exit::OK, "{}", o.stderr);
}

#[test]
fn stdin_over_the_cap_is_refused() {
    let t = box_root();
    let big = vec![b'a'; k2_root_proto::verbs::CAP_TEMPLATE + 1];
    let o = t.call_in(&["mail", "render-unit"], &big);
    assert_eq!(o.code, exit::ARGV);
    assert_eq!(o.stderr, "stdin too large");
    assert!(!t.p(STALWART_UNIT_PATH).exists());
}

#[test]
fn only_version_runs_without_root() {
    let mut t = box_root();
    t.ctx.euid = 1000;
    ok(&t.call(&["helper", "version"]));
    let o = t.call(&["mail", "usage"]);
    assert_eq!(o.code, exit::STATE);
}

#[test]
fn version_json_and_marker() {
    let t = box_root();
    let o = t.call(&["helper", "version"]);
    ok(&o);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["name"], "k2-root-helper");
    assert_eq!(v["protocol"], PROTOCOL);
    assert_eq!(v["version"], crate::core::VERSION);
    assert_eq!(v["packageOwned"], false);
    assert_eq!(v["namespaces"]["keep"], true);
    assert_eq!(v["daemons"][0]["unit"], "k2-daemon.service");
    assert!(v["stalwartPinSha256"]
        .as_str()
        .is_some_and(|s| s.len() == 64));
    assert!(v["keyIds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == k2_root_proto::keys::TEST_KEY.key_id));
    // The compiled-in marker is exactly the proto format.
    let arch = std::env::consts::ARCH;
    assert_eq!(
        crate::BUILD_MARKER,
        format_marker(crate::core::VERSION, PROTOCOL, arch)
    );
}

#[test]
fn every_daemon_verb_refuses_a_non_daemon_caller_with_77_and_no_effect() {
    let t = box_root();
    t.as_agent();
    for v in catalog().into_iter().filter(|v| v.class() == Class::Daemon) {
        let argv = v.argv();
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let o = t.call_in(&refs, b"");
        assert_eq!(o.code, exit::NOT_DAEMON, "{argv:?}: {}", o.stderr);
        assert_eq!(o.stderr, "caller_not_daemon");
    }
    assert!(
        t.non_query_calls().is_empty(),
        "no child ran: {:?}",
        t.non_query_calls()
    );
    let audit = t.audit.0.borrow();
    assert!(audit.iter().all(|l| l.contains("caller_not_daemon")));
}

#[test]
fn every_open_verb_runs_for_an_agent() {
    let t = box_root();
    t.as_agent();
    for args in [
        &["helper", "version"][..],
        &["mail", "usage"],
        &["mail", "systemctl", "restart"],
        &["keep", "status"],
        &["keep", "gen", "get"],
    ] {
        let o = t.call(args);
        ok(&o);
    }
}

// ── Caller walk fixtures (§6.2) ─────────────────────────────────────────

fn snapshot_call(t: &T) -> i32 {
    // A daemon verb with no side effect beyond a refusal on state.
    t.call(&["keep", "gen", "bump"]).code
}

#[test]
fn caller_walk_shapes() {
    // sudo as the parent (PAM session): helper ← sudo ← daemon.
    let t = box_root();
    assert_eq!(snapshot_call(&t), exit::OK);

    // sudo exec'd directly: the helper's parent IS the daemon.
    let t = box_root();
    t.procs.0.borrow_mut().parent = DAEMON_PID;
    assert_eq!(snapshot_call(&t), exit::OK);

    // use_pty: helper ← sudo monitor ← sudo ← daemon (two hops).
    let t = box_root();
    {
        let mut p = t.procs.0.borrow_mut();
        p.procs.insert(
            6001,
            P {
                exe: "/usr/bin/sudo".into(),
                exe_owner: 0,
                ppid: SUDO_PID,
                ruid: 0,
            },
        );
        p.parent = 6001;
    }
    assert_eq!(snapshot_call(&t), exit::OK);

    // sudo-rs.
    let t = box_root();
    t.procs.0.borrow_mut().procs.get_mut(&SUDO_PID).unwrap().exe = "/usr/bin/sudo-rs".into();
    assert_eq!(snapshot_call(&t), exit::OK);

    // A "sudo" that is not root-owned is not stepped over.
    let t = box_root();
    t.procs
        .0
        .borrow_mut()
        .procs
        .get_mut(&SUDO_PID)
        .unwrap()
        .exe_owner = K2_UID;
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // Four sudo hops: more than 3 is never walked to the daemon.
    let t = box_root();
    {
        let mut p = t.procs.0.borrow_mut();
        p.procs.insert(
            6001,
            P {
                exe: "/usr/bin/sudo".into(),
                exe_owner: 0,
                ppid: SUDO_PID,
                ruid: 0,
            },
        );
        p.procs.insert(
            6002,
            P {
                exe: "/usr/bin/sudo".into(),
                exe_owner: 0,
                ppid: 6001,
                ruid: 0,
            },
        );
        p.procs.insert(
            6003,
            P {
                exe: "/usr/bin/sudo".into(),
                exe_owner: 0,
                ppid: 6002,
                ruid: 0,
            },
        );
        p.parent = 6003;
    }
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // The agent (a child of the daemon).
    let t = box_root();
    t.as_agent();
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // pid reuse: the held process died during the check.
    let t = box_root();
    t.procs.0.borrow_mut().dead.insert(DAEMON_PID);
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // Right pid, wrong uid (not a configured daemon user).
    let t = box_root();
    t.procs
        .0
        .borrow_mut()
        .procs
        .get_mut(&DAEMON_PID)
        .unwrap()
        .ruid = 0;
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // The unit isn't running (MainPID 0).
    let t = box_root();
    t.exec.0.borrow_mut().main_pids.clear();
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // Missing daemon.toml.
    let t = box_root();
    std::fs::remove_file(t.p(DAEMON_TOML)).unwrap();
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // Group-writable daemon.toml.
    let t = box_root();
    std::fs::set_permissions(t.p(DAEMON_TOML), std::fs::Permissions::from_mode(0o664)).unwrap();
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);

    // daemon.toml as a symlink.
    let t = box_root();
    std::fs::rename(t.p(DAEMON_TOML), t.p("/etc/k2-root-helper/real.toml")).unwrap();
    symlink(t.p("/etc/k2-root-helper/real.toml"), t.p(DAEMON_TOML)).unwrap();
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);
}

#[test]
fn user_scope_units_are_asked_through_the_user_manager() {
    let t = box_root();
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"user\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    assert_eq!(snapshot_call(&t), exit::OK);
    let calls = t.calls();
    let show = calls
        .iter()
        .find(|(p, a, _)| p == SYSTEMCTL && a.iter().any(|x| x == "MainPID"))
        .unwrap();
    assert_eq!(
        show.1,
        vec![
            "--user",
            "-M",
            "k2@",
            "show",
            "-p",
            "MainPID",
            "--value",
            "k2-daemon.service"
        ]
    );
}

#[test]
fn user_scope_falls_back_to_the_private_socket_without_a_user_bus() {
    let t = box_root();
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"user\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    t.exec.0.borrow_mut().no_user_bus = true;
    assert_eq!(snapshot_call(&t), exit::OK);
    let calls = t.calls();
    let fallback = calls
        .iter()
        .find(|(p, a, _)| p == RUNUSER && a.iter().any(|x| x == "MainPID"))
        .expect("runuser fallback ran");
    assert_eq!(
        fallback.1,
        vec![
            "-u",
            "k2",
            "--",
            "/usr/bin/env",
            "XDG_RUNTIME_DIR=/run/user/1001",
            "/usr/bin/systemctl",
            "--user",
            "show",
            "-p",
            "MainPID",
            "--value",
            "k2-daemon.service"
        ]
    );
    // An agent is still refused through the fallback.
    t.as_agent();
    assert_eq!(snapshot_call(&t), exit::NOT_DAEMON);
}

// ── mail ────────────────────────────────────────────────────────────────

#[test]
fn render_unit_and_dropin_write_canned_bytes() {
    let t = box_root();
    let pw = "0123456789abcdef".repeat(4);
    ok(&t.call_in(&["mail", "render-unit"], pw.as_bytes()));
    let unit = std::fs::read_to_string(t.p(STALWART_UNIT_PATH)).unwrap();
    assert_eq!(unit, k2_root_proto::templates::systemd_unit(Some(&pw)));
    let mode = std::fs::metadata(t.p(STALWART_UNIT_PATH))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    // The password never reaches the audit line.
    assert!(t.audit.0.borrow().iter().all(|l| !l.contains(&pw)));

    ok(&t.call_in(&["mail", "render-unit"], b""));
    assert_eq!(
        std::fs::read_to_string(t.p(STALWART_UNIT_PATH)).unwrap(),
        k2_root_proto::templates::systemd_unit(None)
    );

    for bad in [
        &b"xyz"[..],
        "A".repeat(64).as_bytes(),
        format!("{}\n", "a".repeat(63)).as_bytes(),
        "a".repeat(65).as_bytes(),
    ] {
        let o = t.call_in(&["mail", "render-unit"], bad);
        assert_eq!(o.code, exit::ARGV, "{bad:?}");
    }

    // drop-in needs its dir (mkdir first, as before).
    assert_ne!(t.call(&["mail", "render-dropin"]).code, exit::OK);
    ok(&t.call(&["mail", "mkdir", "dropin-dir"]));
    ok(&t.call(&["mail", "render-dropin"]));
    assert_eq!(
        std::fs::read_to_string(t.p(STALWART_DROPIN_PATH)).unwrap(),
        k2_root_proto::templates::hardening_dropin()
    );
    let mode = std::fs::metadata(t.p(STALWART_DROPIN_PATH))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644);
}

#[test]
fn render_unit_refuses_a_symlink_at_the_unit_path() {
    let t = box_root();
    std::fs::write(t.p("/etc/victim"), b"keep").unwrap();
    symlink(t.p("/etc/victim"), t.p(STALWART_UNIT_PATH)).unwrap();
    assert_ne!(t.call_in(&["mail", "render-unit"], b"").code, exit::OK);
    assert_eq!(std::fs::read(t.p("/etc/victim")).unwrap(), b"keep");
}

#[test]
fn mkdir_chown_remove_follow_no_links() {
    let t = box_root();
    ok(&t.call(&["mail", "mkdir", "data"]));
    ok(&t.call(&["mail", "mkdir", "data"])); // idempotent
    let mode = std::fs::metadata(t.p(STALWART_DATA_DIR))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);
    std::fs::write(t.p("/var/lib/stalwart/f"), b"x").unwrap();
    ok(&t.call(&["mail", "chown", "data"]));

    // A link planted inside the data dir stops chown (never followed).
    symlink("/etc/shadow", t.p("/var/lib/stalwart/link")).unwrap();
    assert_eq!(t.call(&["mail", "chown", "data"]).code, exit::STATE);
    std::fs::remove_file(t.p("/var/lib/stalwart/link")).unwrap();

    // A link AT the target: mkdir/chown/remove refuse.
    std::fs::create_dir_all(t.p("/srv/elsewhere")).unwrap();
    symlink(t.p("/srv/elsewhere"), t.p(STALWART_LOG_DIR)).unwrap();
    assert_eq!(t.call(&["mail", "mkdir", "log"]).code, exit::STATE);
    assert_eq!(t.call(&["mail", "remove", "log"]).code, exit::STATE);
    assert!(t.p("/srv/elsewhere").exists());

    // remove data: gone.
    ok(&t.call(&["mail", "remove", "data"]));
    assert!(!t.p(STALWART_DATA_DIR).exists());
    ok(&t.call(&["mail", "remove", "data"])); // missing is fine
}

#[test]
fn systemctl_vectors_are_fixed() {
    let t = box_root();
    for (w, want) in [
        ("daemon-reload", vec!["daemon-reload"]),
        ("enable-now", vec!["enable", "--now", "stalwart"]),
        ("restart", vec!["restart", "stalwart"]),
        ("stop", vec!["stop", "stalwart"]),
        ("disable-now", vec!["disable", "--now", "stalwart"]),
    ] {
        ok(&t.call(&["mail", "systemctl", w]));
        let last = t
            .calls()
            .into_iter()
            .filter(|(p, a, _)| p == SYSTEMCTL && !a.iter().any(|x| x == "MainPID"))
            .last()
            .unwrap();
        assert_eq!(last.1, want);
        assert!(last.2.is_none(), "no stdin to systemctl");
    }
}

#[test]
fn install_bin_accepts_only_pinned_tarballs() {
    let t = box_root();
    let o = t.call_in(&["mail", "install-bin"], b"not the pinned tarball");
    assert_eq!(o.code, exit::ARGV);
    assert!(o.stderr.contains("sha256 mismatch"));
    assert!(!t.p(STALWART_BIN).exists());
    // The two-entry check itself, with a table we control.
    let tarball = b"pretend tarball";
    use sha2::Digest;
    let sha: String = sha2::Sha256::digest(tarball)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let sha: &'static str = Box::leak(sha.into_boxed_str());
    let pin = k2_root_proto::stalwart::StalwartArtifact {
        arch: "x86_64",
        triple: "t",
        sha256: sha,
    };
    let other = k2_root_proto::stalwart::StalwartArtifact {
        arch: "x86_64",
        triple: "t",
        sha256: "0".repeat(64).leak(),
    };
    assert_eq!(
        crate::mail::match_archive(tarball, &pin, &other).unwrap(),
        k2_root_proto::stalwart::STALWART_PINNED_VERSION
    );
    assert_eq!(
        crate::mail::match_archive(tarball, &other, &pin).unwrap(),
        k2_root_proto::stalwart::STALWART_PREVIOUS_VERSION
    );
    assert!(crate::mail::match_archive(tarball, &other, &other).is_err());
}

#[test]
fn extract_member_is_exact_stalwart_file() {
    fn pack(files: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, data, kind) in files {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            if *kind == tar::EntryType::Symlink {
                h.set_link_name("/etc/shadow").unwrap();
                h.set_size(0);
            }
            h.set_path(name).unwrap();
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }
    let good = pack(&[
        ("README", b"x", tar::EntryType::Regular),
        ("stalwart", b"\x7fELFbin", tar::EntryType::Regular),
    ]);
    assert_eq!(
        crate::mail::extract_stalwart_member(&good).unwrap(),
        b"\x7fELFbin"
    );
    for bad in [
        pack(&[("x/stalwart", b"bin", tar::EntryType::Regular)]),
        pack(&[("stalwart", b"", tar::EntryType::Symlink)]),
        pack(&[("stalwart", b"", tar::EntryType::Regular)]),
        pack(&[("other", b"bin", tar::EntryType::Regular)]),
    ] {
        assert!(crate::mail::extract_stalwart_member(&bad).is_err());
    }
}

#[test]
fn usage_reports_layout_under_the_root() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/var/lib/stalwart/data")).unwrap();
    std::fs::write(t.p("/var/lib/stalwart/data/a.sst"), vec![0u8; 100]).unwrap();
    let o = t.call(&["mail", "usage"]);
    ok(&o);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["dataBytes"], 100);
}

// ── pg ──────────────────────────────────────────────────────────────────

#[test]
fn psql_runs_through_runuser_with_an_argv_array() {
    let t = box_root();
    t.exec.0.borrow_mut().answers.insert(
        RUNUSER.into(),
        ExecOut {
            status: 0,
            stdout: b"1\n".to_vec(),
            stderr: vec![],
        },
    );
    let o = t.call_in(
        &[
            "pg",
            "psql",
            "-d",
            "myapp_test",
            "-v",
            "ON_ERROR_STOP=1",
            "-tA",
        ],
        b"SELECT 1;",
    );
    ok(&o);
    assert_eq!(o.stdout, b"1\n");
    let (prog, args, stdin) = t
        .calls()
        .into_iter()
        .find(|(p, _, _)| p == RUNUSER)
        .unwrap();
    assert_eq!(prog, "/usr/sbin/runuser");
    assert_eq!(
        args,
        vec![
            "-u",
            "postgres",
            "--",
            "/usr/bin/psql",
            "-X",
            "-d",
            "myapp_test",
            "-v",
            "ON_ERROR_STOP=1",
            "-tA"
        ]
    );
    assert_eq!(stdin.as_deref(), Some(&b"SELECT 1;"[..]));
    assert!(t
        .calls()
        .iter()
        .all(|(p, _, _)| p != "/bin/sh" && p != "/usr/bin/su"));
    // The SQL never reaches the audit line.
    assert!(t.audit.0.borrow().iter().all(|l| !l.contains("SELECT")));
}

#[test]
fn psql_failure_relays_postgres_words() {
    let t = box_root();
    t.exec.0.borrow_mut().answers.insert(
        RUNUSER.into(),
        ExecOut {
            status: 3,
            stdout: vec![],
            stderr: b"ERROR:  role \"x\" already exists".to_vec(),
        },
    );
    let o = t.call_in(&["pg", "psql", "-d", "postgres"], b"CREATE ROLE x;");
    assert_eq!(o.code, exit::IO);
    assert!(o.stderr.contains("already exists"));
}

#[test]
fn pg_systemctl_vectors() {
    let t = box_root();
    ok(&t.call(&["pg", "systemctl", "enable", "--now", "postgresql"]));
    ok(&t.call(&["pg", "systemctl", "restart", "postgresql@16-main"]));
    ok(&t.call(&["pg", "systemctl", "daemon-reload"]));
    let calls: Vec<Vec<String>> = t
        .calls()
        .into_iter()
        .filter(|(p, a, _)| p == SYSTEMCTL && !a.iter().any(|x| x == "MainPID"))
        .map(|(_, a, _)| a)
        .collect();
    assert_eq!(
        calls,
        vec![
            vec!["enable".to_string(), "--now".into(), "postgresql".into()],
            vec!["restart".to_string(), "postgresql@16-main".into()],
            vec!["daemon-reload".to_string()],
        ]
    );
}

fn pg_cluster(t: &T, ver: &str) {
    std::fs::create_dir_all(t.p(&format!("/etc/postgresql/{ver}/main/conf.d"))).unwrap();
}

#[test]
fn ram_fence_writes_canned_files() {
    let t = box_root();
    pg_cluster(&t, "16");
    pg_cluster(&t, "15");
    let o = t.call(&["pg", "install-ram-fence"]);
    ok(&o);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["clusters"], serde_json::json!(["15", "16"]));
    for p in [PG_RAM_DROPIN_PATH, PG_RAM_TEMPLATE_DROPIN_PATH] {
        assert_eq!(
            std::fs::read_to_string(t.p(p)).unwrap(),
            k2_root_proto::templates::pg_ram_dropin()
        );
    }
    for ver in ["15", "16"] {
        let f = t.p(&format!("/etc/postgresql/{ver}/main/conf.d/99-k2-ram.conf"));
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            k2_root_proto::templates::pg_guc_conf()
        );
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
    // No /etc/postgresql at all: drop-ins only.
    let t = box_root();
    let o = t.call(&["pg", "install-ram-fence"]);
    ok(&o);
}

#[test]
fn ram_fence_refuses_planted_links_and_writes_nothing() {
    // RH9: a conf.d that is a symlink (postgres can plant one).
    let t = box_root();
    std::fs::create_dir_all(t.p("/etc/postgresql/16/main")).unwrap();
    symlink(t.p("/etc/sudoers.d"), t.p("/etc/postgresql/16/main/conf.d")).unwrap();
    assert_eq!(t.call(&["pg", "install-ram-fence"]).code, exit::STATE);
    assert!(!t.p(PG_RAM_DROPIN_PATH).exists(), "nothing written");
    assert!(std::fs::read_dir(t.p("/etc/sudoers.d"))
        .unwrap()
        .next()
        .is_none());

    // A 99-k2-ram.conf that is a symlink.
    let t = box_root();
    pg_cluster(&t, "16");
    symlink(
        t.p("/etc/sudoers.d/x"),
        t.p("/etc/postgresql/16/main/conf.d/99-k2-ram.conf"),
    )
    .unwrap();
    assert_eq!(t.call(&["pg", "install-ram-fence"]).code, exit::STATE);
    assert!(!t.p(PG_RAM_DROPIN_PATH).exists(), "nothing written");
    assert!(!t.p("/etc/sudoers.d/x").exists());

    // A link higher up (main).
    let t = box_root();
    std::fs::create_dir_all(t.p("/etc/postgresql/16")).unwrap();
    std::fs::create_dir_all(t.p("/srv/fake/conf.d")).unwrap();
    symlink(t.p("/srv/fake"), t.p("/etc/postgresql/16/main")).unwrap();
    assert_eq!(t.call(&["pg", "install-ram-fence"]).code, exit::STATE);
    assert!(!t.p("/srv/fake/conf.d/99-k2-ram.conf").exists());
}

// ── keep ────────────────────────────────────────────────────────────────

#[test]
fn keep_lifecycle_modes_and_refusals() {
    let t = box_root();
    let st = |t: &T| -> serde_json::Value {
        serde_json::from_slice(&t.call(&["keep", "status"]).stdout).unwrap()
    };
    assert_eq!(st(&t)["kek"], "absent");
    // get before init: refused.
    assert_eq!(t.call(&["keep", "kek", "get"]).code, exit::STATE);
    let o = t.call(&["keep", "kek", "init"]);
    ok(&o);
    assert!(o.stdout.is_empty(), "init prints nothing");
    assert_eq!(
        t.call(&["keep", "kek", "init"]).code,
        exit::STATE,
        "refuses when present"
    );
    let k1 = t.call(&["keep", "kek", "get"]);
    ok(&k1);
    assert_eq!(k1.stdout.len(), 32);
    let store = t.p("/var/lib/k2-root-helper/keep/k2");
    assert_eq!(
        std::fs::metadata(&store).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(store.join("kek"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(t.p(KEEP_DIR))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    // stage → abort leaves kek alone.
    let s = t.call(&["keep", "kek", "stage"]);
    ok(&s);
    assert_eq!(st(&t)["next"], "present");
    ok(&t.call(&["keep", "kek", "abort"]));
    assert_eq!(st(&t)["next"], "absent");
    assert_eq!(t.call(&["keep", "kek", "get"]).stdout, k1.stdout);
    assert_eq!(
        t.call(&["keep", "kek", "commit"]).code,
        exit::STATE,
        "nothing staged"
    );

    // stage → commit swaps.
    let s = t.call(&["keep", "kek", "stage"]);
    ok(&t.call(&["keep", "kek", "commit"]));
    assert_eq!(t.call(&["keep", "kek", "get"]).stdout, s.stdout);
    assert_eq!(std::fs::read(store.join("kek.prev")).unwrap(), k1.stdout);

    // gen.
    let g = |o: crate::Outcome| -> u64 {
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()["gen"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(g(t.call(&["keep", "gen", "get"])), 0);
    assert_eq!(g(t.call(&["keep", "gen", "bump"])), 1);
    assert_eq!(g(t.call(&["keep", "gen", "bump"])), 2);
    assert_eq!(st(&t)["gen"], 2);

    // The KEK never reaches the audit line.
    let hex: String = k1.stdout.iter().map(|b| format!("{b:02x}")).collect();
    assert!(t.audit.0.borrow().iter().all(|l| !l.contains(&hex)));
}

#[test]
fn keep_store_is_keyed_by_the_caller_walk_user() {
    let t = box_root();
    // Two daemons on one box: k2 and k2std.
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n\
                [[daemon]]\nuser = \"k2std\"\nunit = \"k2-daemon-std.service\"\nscope = \"system\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    ok(&t.call(&["keep", "kek", "init"]));
    {
        let mut p = t.procs.0.borrow_mut();
        p.users.insert("k2std".into(), 1002);
        p.procs.insert(
            7000,
            P {
                exe: "/home/k2std/.local/bin/k2-daemon".into(),
                exe_owner: 1002,
                ppid: 1,
                ruid: 1002,
            },
        );
        p.procs.get_mut(&SUDO_PID).unwrap().ppid = 7000;
    }
    t.exec
        .0
        .borrow_mut()
        .main_pids
        .insert("k2-daemon-std.service".into(), 7000);
    let st: serde_json::Value =
        serde_json::from_slice(&t.call(&["keep", "status"]).stdout).unwrap();
    assert_eq!(st["kek"], "absent", "k2std sees its own (empty) store");
    ok(&t.call(&["keep", "kek", "init"]));
    assert!(t.p("/var/lib/k2-root-helper/keep/k2/kek").exists());
    assert!(t.p("/var/lib/k2-root-helper/keep/k2std/kek").exists());
}

#[test]
fn keep_refuses_a_linked_store() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/var/lib/k2-root-helper/keep")).unwrap();
    std::fs::create_dir_all(t.p("/tmp/attacker")).unwrap();
    symlink(t.p("/tmp/attacker"), t.p("/var/lib/k2-root-helper/keep/k2")).unwrap();
    assert_eq!(t.call(&["keep", "kek", "init"]).code, exit::STATE);
    assert!(std::fs::read_dir(t.p("/tmp/attacker"))
        .unwrap()
        .next()
        .is_none());
}

// ── self-update / rollback (§9.2, §9.3) ─────────────────────────────────

fn build(version: &str, protocol: u32) -> Vec<u8> {
    fake_elf("x86_64", &format_marker(version, protocol, "x86_64"))
}

fn payload(elf: &[u8], version: &str, protocol: u32) -> Vec<u8> {
    let sig = sign_with_test_key(elf, &trusted_comment("x86_64", version, protocol));
    encode_payload(&sig, elf)
}

fn next_version() -> String {
    let v = crate::core::running_version();
    format!("{}.{}.{}", v.0, v.1, v.2 + 1)
}

#[test]
fn self_update_installs_a_signed_newer_build_and_keeps_prev() {
    let t = box_root();
    let old = std::fs::read(t.p(HELPER_PATH)).unwrap();
    let nv = next_version();
    let elf = build(&nv, PROTOCOL);
    let o = t.call_in(&["helper", "self-update"], &payload(&elf, &nv, PROTOCOL));
    ok(&o);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["result"], "updated");
    assert_eq!(v["to"], nv);
    assert_eq!(std::fs::read(t.p(HELPER_PATH)).unwrap(), elf);
    assert_eq!(
        std::fs::read(t.p("/usr/local/libexec/k2-root-helper.prev")).unwrap(),
        old,
        ".prev byte for byte"
    );
    assert_eq!(
        std::fs::metadata(t.p(HELPER_PATH))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(!t.p("/usr/local/libexec/.k2-root-helper.new").exists());
    // Never executed: no child process at all during self-update.
    assert!(t.non_query_calls().is_empty());
    let audit = t.audit.0.borrow();
    let line = audit.last().unwrap();
    assert!(
        line.contains(&nv) && line.contains("keyId") && line.contains("sha256"),
        "{line}"
    );
}

#[test]
fn self_update_refusals_change_nothing() {
    let nv = next_version();
    let good = build(&nv, PROTOCOL);
    let cur = crate::core::VERSION.to_string();
    let older = {
        let v = crate::core::running_version();
        format!("{}.{}.{}", v.0, v.1.saturating_sub(1), 0)
    };
    let cases: Vec<(&str, Box<dyn Fn(&T) -> Vec<u8>>)> = vec![
        (
            "bad signature",
            Box::new(|_t: &T| {
                let mut p = payload(&build(&next_version(), PROTOCOL), &next_version(), PROTOCOL);
                let n = p.len();
                p[n - 70] ^= 0xff;
                p
            }),
        ),
        (
            "tampered comment",
            Box::new(|_t: &T| {
                let nv = next_version();
                let elf = build(&nv, PROTOCOL);
                let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", &nv, PROTOCOL));
                let forged = sig.replace("protocol:4", "protocol:5");
                encode_payload(&forged, &elf)
            }),
        ),
        (
            "daemon asset",
            Box::new(|_t: &T| {
                let nv = next_version();
                let elf = build(&nv, PROTOCOL);
                let sig = sign_with_test_key(
                    &elf,
                    &format!("file:k2-daemon-linux-x86_64 version:{nv} protocol:4"),
                );
                encode_payload(&sig, &elf)
            }),
        ),
        (
            "wrong arch",
            Box::new(|_t: &T| {
                let nv = next_version();
                let elf = fake_elf("aarch64", &format_marker(&nv, PROTOCOL, "aarch64"));
                let sig = sign_with_test_key(&elf, &trusted_comment("aarch64", &nv, PROTOCOL));
                encode_payload(&sig, &elf)
            }),
        ),
        (
            "zero markers",
            Box::new(|_t: &T| {
                let nv = next_version();
                let elf = fake_elf("x86_64", "none");
                payload(&elf, &nv, PROTOCOL)
            }),
        ),
        (
            "two markers",
            Box::new(|_t: &T| {
                let nv = next_version();
                let mut elf = build(&nv, PROTOCOL);
                elf.extend_from_slice(format_marker(&nv, PROTOCOL, "x86_64").as_bytes());
                payload(&elf, &nv, PROTOCOL)
            }),
        ),
        (
            "marker mismatch",
            Box::new(|_t: &T| {
                let nv = next_version();
                let elf = build("99.0.0", PROTOCOL);
                payload(&elf, &nv, PROTOCOL)
            }),
        ),
        (
            "lower version",
            Box::new(move |_t: &T| payload(&build(&older, PROTOCOL), &older, PROTOCOL)),
        ),
        (
            "lower protocol",
            Box::new(|_t: &T| {
                let nv = next_version();
                payload(&build(&nv, 3), &nv, 3)
            }),
        ),
        (
            "oversize",
            Box::new(|_t: &T| vec![0u8; k2_root_proto::verbs::CAP_SELF_UPDATE + 1]),
        ),
        (
            "stage symlink",
            Box::new(|t: &T| {
                symlink(
                    t.p("/etc/victim"),
                    t.p("/usr/local/libexec/.k2-root-helper.new"),
                )
                .unwrap();
                payload(&build(&next_version(), PROTOCOL), &next_version(), PROTOCOL)
            }),
        ),
        (
            "group-writable parent",
            Box::new(|t: &T| {
                std::fs::set_permissions(
                    t.p("/usr/local/libexec"),
                    std::fs::Permissions::from_mode(0o775),
                )
                .unwrap();
                payload(&build(&next_version(), PROTOCOL), &next_version(), PROTOCOL)
            }),
        ),
        (
            "non-canonical exe",
            Box::new(|_t: &T| {
                payload(&build(&next_version(), PROTOCOL), &next_version(), PROTOCOL)
            }),
        ),
        (
            "refused version",
            Box::new(|t: &T| {
                std::fs::create_dir_all(t.p(STATE_DIR)).unwrap();
                std::fs::write(
                    t.p(STATE_JSON),
                    format!("{{\"refused\":[\"{}\"]}}", next_version()),
                )
                .unwrap();
                payload(&build(&next_version(), PROTOCOL), &next_version(), PROTOCOL)
            }),
        ),
    ];
    for (name, make) in cases {
        let mut t = box_root();
        if name == "non-canonical exe" {
            t.ctx.self_exe = t.p("/usr/lib/k2/k2-root-helper");
        }
        std::fs::write(t.p("/etc/victim"), b"keep").unwrap();
        let before = std::fs::read(t.p(HELPER_PATH)).unwrap();
        let input = make(&t);
        let o = t.call_in(&["helper", "self-update"], &input);
        assert_ne!(o.code, exit::OK, "{name}: should refuse");
        assert!(
            o.code == exit::SIGNATURE || o.code == exit::ARGV || o.code == exit::STATE,
            "{name}: code {}",
            o.code
        );
        assert_eq!(
            std::fs::read(t.p(HELPER_PATH)).unwrap(),
            before,
            "{name}: helper unchanged"
        );
        assert!(
            !t.p("/usr/local/libexec/k2-root-helper.prev").exists(),
            "{name}: no prev"
        );
        assert_eq!(
            std::fs::read(t.p("/etc/victim")).unwrap(),
            b"keep",
            "{name}"
        );
    }
    // Same version: a no-op success.
    let t = box_root();
    let o = t.call_in(
        &["helper", "self-update"],
        &payload(&build(&cur, PROTOCOL), &cur, PROTOCOL),
    );
    ok(&o);
    assert!(String::from_utf8_lossy(&o.stdout).contains("\"same\""));
    let _ = good;
}

#[test]
fn self_update_without_test_keys_trusts_nothing_until_the_ceremony() {
    let mut t = box_root();
    t.ctx.test_keys = false;
    let nv = next_version();
    let o = t.call_in(
        &["helper", "self-update"],
        &payload(&build(&nv, PROTOCOL), &nv, PROTOCOL),
    );
    assert_eq!(o.code, exit::SIGNATURE);
}

#[test]
fn self_rollback_restores_prev_and_refuses_the_bad_version_afterwards() {
    let t = box_root();
    // No .prev: refused.
    assert_eq!(t.call(&["helper", "self-rollback"]).code, exit::STATE);
    let old = std::fs::read(t.p(HELPER_PATH)).unwrap();
    let nv = next_version();
    ok(&t.call_in(
        &["helper", "self-update"],
        &payload(&build(&nv, PROTOCOL), &nv, PROTOCOL),
    ));
    // Rollback runs from the (new) current binary; in the test ctx our own
    // version is VERSION, which is what goes on the refuse list.
    let o = t.call(&["helper", "self-rollback"]);
    ok(&o);
    assert_eq!(std::fs::read(t.p(HELPER_PATH)).unwrap(), old);
    assert!(t.p("/usr/local/libexec/k2-root-helper.bad").exists());
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(t.p(STATE_JSON)).unwrap()).unwrap();
    assert!(state["refused"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == crate::core::VERSION));
    // And an agent cannot roll back.
    let t2 = box_root();
    t2.as_agent();
    assert_eq!(t2.call(&["helper", "self-rollback"]).code, exit::NOT_DAEMON);
}

#[test]
fn rollback_refuse_list_blocks_reinstall() {
    let t = box_root();
    std::fs::create_dir_all(t.p(STATE_DIR)).unwrap();
    let nv = next_version();
    std::fs::write(t.p(STATE_JSON), format!("{{\"refused\":[\"{nv}\"]}}")).unwrap();
    let o = t.call_in(
        &["helper", "self-update"],
        &payload(&build(&nv, PROTOCOL), &nv, PROTOCOL),
    );
    assert_eq!(o.code, exit::SIGNATURE);
    assert!(o.stderr.contains("rolled back"));
}

// ── legacy retire, audit, lock ──────────────────────────────────────────

#[test]
fn legacy_retire_removes_only_the_fixed_files_then_checks_sudoers() {
    let t = box_root();
    for p in [
        LEGACY_PG_HELPER,
        LEGACY_PG_SUDOERS,
        LEGACY_MAIL_HELPER,
        LEGACY_MAIL_SUDOERS,
    ] {
        std::fs::write(t.p(p), b"x").unwrap();
    }
    let o = t.call(&["helper", "legacy-retire", "pg"]);
    ok(&o);
    assert!(!t.p(LEGACY_PG_HELPER).exists() && !t.p(LEGACY_PG_SUDOERS).exists());
    assert!(
        t.p(LEGACY_MAIL_HELPER).exists(),
        "mail helper kept one train"
    );
    assert!(t
        .calls()
        .iter()
        .any(|(p, a, _)| p == VISUDO && a == &vec!["-c".to_string(), "-q".to_string()]));
    // A symlink planted at a legacy path is refused, not followed.
    std::fs::remove_file(t.p(LEGACY_MAIL_SUDOERS)).unwrap();
    symlink(t.p("/etc/victim"), t.p(LEGACY_MAIL_SUDOERS)).unwrap();
    std::fs::write(t.p("/etc/victim"), b"keep").unwrap();
    assert_eq!(
        t.call(&["helper", "legacy-retire", "mail"]).code,
        exit::STATE
    );
    assert!(t.p("/etc/victim").exists());
}

#[test]
fn lock_serializes_mutating_verbs() {
    let t = box_root();
    let held = crate::take_lock(&t.ctx).expect("first lock");
    let o = t.call(&["mail", "systemctl", "restart"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("busy"));
    // Read verbs take no lock.
    ok(&t.call(&["mail", "usage"]));
    drop(held);
    ok(&t.call(&["mail", "systemctl", "restart"]));
}

#[test]
fn audit_lines_are_json_with_class_and_caller() {
    let t = box_root();
    ok(&t.call(&["mail", "systemctl", "restart"]));
    let line = t.audit.0.borrow().last().unwrap().clone();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["ns"], "mail");
    assert_eq!(v["verb"], "mail systemctl restart");
    assert_eq!(v["class"], "open");
    assert_eq!(v["caller"]["pid"], DAEMON_PID);
    assert_eq!(v["result"], "ok");
}
