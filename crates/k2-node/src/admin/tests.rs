//! Executor unit tests (no root): temp dirs, a recording platform, real
//! signatures. prd-k2-node-admin-v1 §16 "Executor (unit, no root)".

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use k2_node_proto::admin::{self, code, AdminKeys, ApprovalSigner, Class, Command, SignedCommand};
use k2_node_proto::crypto::{self, SigningKey};
use serde_json::{json, Value};

use super::config::{self, AdminFile};
use super::exec::{ExecPaths, Executor, LocalRequest, LocalResponse, Platform};

pub struct RecPlatform {
    pub now: AtomicI64,
    pub calls: Mutex<Vec<String>>,
    pub fail_restart: Mutex<bool>,
    /// Staged builds the preflight ran (paths).
    pub preflights: Mutex<Vec<String>>,
    /// Make the preflight fail (a build the node user can't exec).
    pub fail_preflight: Mutex<bool>,
    /// `enable_job_wall` calls (the job user each named).
    pub walls: Mutex<Vec<String>>,
    /// Runs on every `restart_unit` (the owner-change tests: the node's
    /// half swaps its files as a real restart would).
    pub on_restart: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// The node pid [`RecPlatform`] reports: this test process, so the mark
/// also carries a real start time.
pub fn node_pid() -> u32 {
    std::process::id()
}

impl Platform for RecPlatform {
    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
    fn os(&self) -> &'static str {
        crate::sysinfo::os_label()
    }
    fn arch(&self) -> &'static str {
        std::env::consts::ARCH
    }
    fn restart_node(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("restart-node".into());
        if *self.fail_restart.lock().unwrap() {
            return Err("restart failed".into());
        }
        Ok(())
    }
    fn restart_unit(&self, unit: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("restart-unit {unit}"));
        if let Some(f) = self.on_restart.lock().unwrap().as_ref() {
            f();
        }
        Ok(())
    }
    fn node_pid(&self) -> Option<u32> {
        Some(node_pid())
    }
    /// Fake builds aren't executable: answer what the staged build's marker says.
    /// No real units: the machine running the tests may be a node itself.
    fn running_job_units(&self) -> Option<usize> {
        Some(0)
    }
    /// Writes jobs.toml only (no useradd/setfacl without root).
    fn enable_job_wall(
        &self,
        paths: &ExecPaths,
        _node_user: &str,
        job_user: &str,
    ) -> Result<super::jobs::WallSetup, admin::AdminRefusal> {
        let before = super::jobs::read_jobs_file(&paths.config_dir).ok().flatten();
        let f = super::jobs::JobsFile { job_user: job_user.into(), tenants: before.as_ref().map(|b| b.tenants.clone()).unwrap_or_default() };
        super::jobs::write_jobs(&paths.config_dir, &f).map_err(|e| admin::AdminRefusal::new(code::FAILED, e))?;
        self.walls.lock().unwrap().push(job_user.to_string());
        Ok(super::jobs::WallSetup { created_user: false, jobs_changed: before.as_ref() != Some(&f), home_acl: "test".into(), jobs_msg: "written".into() })
    }
    fn preflight(&self, staged: &std::path::Path, _node_user: &str) -> Result<String, String> {
        self.preflights.lock().unwrap().push(staged.display().to_string());
        if *self.fail_preflight.lock().unwrap() {
            return Err(format!("exec {}: Permission denied (os error 13)", staged.display()));
        }
        let v = super::update::installed_version(staged).ok_or("no marker")?;
        Ok(format!("k2-node {v} (protocol {})", k2_node_proto::PROTOCOL))
    }
}

pub struct Rig {
    pub exec: Executor,
    pub plat: Arc<RecPlatform>,
    pub ctl: SigningKey,
    pub owner: ApprovalSigner,
    pub node_fp: String,
    pub dir: PathBuf,
}

pub const NOW: i64 = 1_800_000_000;

impl Rig {
    pub fn new(level: &str) -> Self {
        let dir = crate::util::temp_dir("exec");
        let paths = ExecPaths {
            config_dir: dir.join("etc"),
            state_dir: dir.join("state"),
            home: dir.join("home"),
            binary: dir.join("bin/k2-node"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        super::exec::ensure_state_dir(&paths.state_dir).unwrap();
        paths.layout().ensure().unwrap();
        let node_key = crate::identity::load_or_create_key(&paths.layout().key()).unwrap();
        let ctl = SigningKey::generate().unwrap();
        let owner = ApprovalSigner::generate().unwrap();
        config::write(
            &paths.admin(),
            &AdminFile {
                remote_admin: level.into(),
                controller_fp: Some(ctl.fingerprint()),
                controller_key: Some(crypto::b64(ctl.spki_der())),
                approval_keys: vec![owner.public().encode()],
                ..Default::default()
            },
        )
        .unwrap();
        let plat = Arc::new(RecPlatform {
            now: AtomicI64::new(NOW),
            calls: Mutex::new(vec![]),
            fail_restart: Mutex::new(false),
            preflights: Mutex::new(vec![]),
            fail_preflight: Mutex::new(false),
            walls: Mutex::new(vec![]),
            on_restart: Mutex::new(None),
        });
        let mut exec = Executor::new(paths, plat.clone());
        exec.drain_settle = std::time::Duration::from_millis(60);
        exec.node_swap_wait = std::time::Duration::from_secs(2);
        Self { exec, plat, ctl, owner, node_fp: node_key.fingerprint(), dir }
    }

    pub fn cmd(&self, verb: &str, args: Value) -> Command {
        Command {
            v: admin::ADMIN_V,
            cmd_id: format!("c{}", &crypto::random_id().unwrap()[..8]),
            node_fp: self.node_fp.clone(),
            controller_fp: self.ctl.fingerprint(),
            class: Class::Operate,
            verb: verb.into(),
            args,
            plan: None,
            proposed_by: "agent:it".into(),
            approved_by: None,
            approved_via: None,
            request_id: None,
            iat: NOW,
            exp: NOW + 600,
            nonce: admin::new_nonce().unwrap(),
        }
    }

    pub fn sign(&self, c: &Command) -> SignedCommand {
        admin::sign_controller(&self.ctl, c).unwrap()
    }

    /// Controller + owner signatures.
    pub fn approve(&self, mut c: Command) -> SignedCommand {
        c.class = Class::Privileged;
        c.request_id = Some("car_1".into());
        c.approved_by = Some("owner:rosson".into());
        c.approved_via = Some("local-auth".into());
        let s = self.sign(&c);
        let body = crypto::unb64(&s.body_b64).unwrap();
        s.with_owner_sig(&self.owner.sign(&admin::owner_message(&body)))
    }

    pub fn run(&self, s: &SignedCommand) -> admin::AdminResult {
        self.exec.handle_command(s)
    }
}

fn code_of(r: &admin::AdminResult) -> &str {
    r.code.as_deref().unwrap_or("")
}

/// CE-24: `packages.allow` is operate at `policy` (a controller signature,
/// no owner approval); one validated name lands on the approved list
/// (Linux); a Mac refuses it with its own code; `off` refuses it.
#[test]
fn packages_allow_adds_one_name_on_linux() {
    let r = Rig::new("policy");
    let res = r.run(&r.sign(&r.cmd("packages.allow", json!({"package": "pkg-config"}))));
    let file = r.exec.paths.config_dir.join("k2-node-packages.toml");
    if cfg!(target_os = "linux") {
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(crate::packages::read_approved(&file).unwrap(), vec!["pkg-config"]);
        let again = r.run(&r.sign(&r.cmd("packages.allow", json!({"package": "libssl-dev"}))));
        assert_eq!(again.state, "done", "{again:?}");
        assert_eq!(crate::packages::read_approved(&file).unwrap(), vec!["libssl-dev", "pkg-config"]);
    } else {
        assert_eq!(code_of(&res), k2_node_proto::admin::PACKAGES_LINUX_ONLY, "{res:?}");
        assert!(!file.exists());
    }
    for bad in [json!({"package": "-y"}), json!({"package": "a b"}), json!({"package": "x", "more": 1}), json!({})] {
        let res = r.run(&r.sign(&r.cmd("packages.allow", bad.clone())));
        assert_eq!(code_of(&res), code::BAD_ARGS, "{bad}: {res:?}");
    }
    let off = Rig::new("off");
    let res = off.run(&off.sign(&off.cmd("packages.allow", json!({"package": "pkg-config"}))));
    assert_eq!(code_of(&res), code::REMOTE_ADMIN_OFF, "{res:?}");
}

#[test]
fn signed_operate_command_runs_and_is_audited_twice() {
    let r = Rig::new("policy");
    let res = r.run(&r.sign(&r.cmd("control.pause", json!({}))));
    assert_eq!(res.state, "done", "{res:?}");
    let c = crate::config::read_control(&r.exec.paths.layout().control()).unwrap();
    assert_eq!(c.control, k2_node_proto::frames::Control::Paused);
    assert_eq!(c.by.as_deref(), Some("controller:agent:it"));
    let lines = r.exec.audit().verify().unwrap();
    let ev: Vec<&str> = lines.iter().map(|l| l.event.as_str()).collect();
    assert_eq!(ev, vec!["exec", "result"], "audit written before and after");
    assert_eq!(res.audit_seq, Some(2));
}

#[test]
fn unsigned_wrong_key_expired_replayed_and_wrong_node_refuse() {
    let r = Rig::new("full");
    // Wrong controller key.
    let other = SigningKey::generate().unwrap();
    let c = r.cmd("control.pause", json!({}));
    let forged = admin::sign_controller(&other, &c).unwrap();
    assert_eq!(code_of(&r.run(&forged)), code::BAD_CONTROLLER_SIGNATURE);
    // Unsigned (garbage sig).
    let mut s = r.sign(&c);
    s.sig_c = crypto::b64(b"nope");
    assert_eq!(code_of(&r.run(&s)), code::BAD_CONTROLLER_SIGNATURE);
    // Expired.
    let mut old = r.cmd("control.pause", json!({}));
    old.iat = NOW - 4000;
    old.exp = NOW - 3600;
    assert_eq!(code_of(&r.run(&r.sign(&old))), code::EXPIRED);
    // Wrong node.
    let mut wn = r.cmd("control.pause", json!({}));
    wn.node_fp = "f".repeat(64);
    assert_eq!(code_of(&r.run(&r.sign(&wn))), code::WRONG_NODE);
    // Replay: the second run of the same signed command refuses.
    let s = r.sign(&r.cmd("control.drain", json!({})));
    assert_eq!(r.run(&s).state, "done");
    assert_eq!(code_of(&r.run(&s)), code::REPLAYED);
    // Nothing was paused by any refused command; the drain ran.
    assert_eq!(crate::config::read_control(&r.exec.paths.layout().control()).unwrap().control, k2_node_proto::frames::Control::Draining);
    // Every refusal is audited and the chain holds.
    let lines = r.exec.audit().verify().unwrap();
    assert_eq!(lines.iter().filter(|l| l.event == "refused").count(), 5);
}

#[test]
fn privileged_needs_owner_signature_and_full_level() {
    let r = Rig::new("full");
    let c = r.cmd("unit.restart", json!({"unit": "k2-node"}));
    // Controller-only privileged command: refused, nothing restarted.
    assert_eq!(code_of(&r.run(&r.sign(&c))), code::APPROVAL_REQUIRED);
    assert!(r.plat.calls.lock().unwrap().is_empty());
    // Owner-signed by a stranger.
    let mut c2 = r.cmd("unit.restart", json!({"unit": "k2-node"}));
    c2.class = Class::Privileged;
    c2.request_id = Some("car_1".into());
    c2.approved_by = Some("owner:x".into());
    let s = r.sign(&c2);
    let body = crypto::unb64(&s.body_b64).unwrap();
    let stranger = ApprovalSigner::generate().unwrap();
    let s = s.with_owner_sig(&stranger.sign(&admin::owner_message(&body)));
    assert_eq!(code_of(&r.run(&s)), code::BAD_APPROVAL_SIGNATURE);
    // Approved: runs.
    let ok = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
    assert_eq!(ok.state, "done", "{ok:?}");
    assert_eq!(*r.plat.calls.lock().unwrap(), vec!["restart-unit k2-node".to_string()]);
    let exec_line = r.exec.audit().verify().unwrap().into_iter().find(|l| l.event == "exec").unwrap();
    assert_eq!(exec_line.approval_key.as_deref(), Some(r.owner.public().fingerprint().as_str()));
    assert_eq!(exec_line.class.as_deref(), Some("privileged"));
    // A non-K2 unit is refused even approved.
    assert_eq!(code_of(&r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "sshd"}))))), code::BAD_ARGS);
    // At `policy` level a privileged verb refuses even when approved.
    let p = Rig::new("policy");
    assert_eq!(code_of(&p.run(&p.approve(p.cmd("unit.restart", json!({"unit": "k2-node"}))))), code::REMOTE_ADMIN_OFF);
}

#[test]
fn levels_off_allows_only_lowering_and_level_never_rises_remotely() {
    let r = Rig::new("off");
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("control.pause", json!({}))))), code::REMOTE_ADMIN_OFF);
    let f = Rig::new("full");
    assert_eq!(code_of(&f.run(&f.sign(&f.cmd("admin.level.lower", json!({"level": "full"}))))), code::HUMAN_REQUIRED);
    let res = f.run(&f.sign(&f.cmd("admin.level.lower", json!({"level": "policy"}))));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(config::read(&f.exec.paths.admin()).unwrap().unwrap().level, admin::Level::Policy);
    assert_eq!(code_of(&f.run(&f.sign(&f.cmd("admin.level.lower", json!({"level": "full"}))))), code::HUMAN_REQUIRED);
    // No admin.toml at all = off.
    std::fs::remove_file(f.exec.paths.admin()).unwrap();
    assert_eq!(code_of(&f.run(&f.sign(&f.cmd("control.pause", json!({}))))), code::REMOTE_ADMIN_OFF);
    // An unreadable admin.toml refuses everything.
    std::fs::write(f.exec.paths.admin(), "remote_admin = 7").unwrap();
    assert_eq!(code_of(&f.run(&f.sign(&f.cmd("control.pause", json!({}))))), code::NOT_CONFIGURED);
}

#[test]
fn unknown_and_node_verbs_and_bad_args_refuse() {
    let r = Rig::new("full");
    assert_eq!(code_of(&r.run(&r.approve(r.cmd("script", json!({"text": "id"}))))), code::VERB_NOT_ALLOWED);
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("health", json!({}))))), code::VERB_NOT_ALLOWED);
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("control.pause", json!({"x": 1}))))), code::BAD_ARGS);
    // node.update must come with its build.
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("node.update", json!({}))))), code::BAD_ARGS);
}

#[test]
fn policy_set_narrowing_runs_widening_needs_the_owner() {
    let r = Rig::new("policy");
    let res = r.run(&r.sign(&r.cmd("policy.set", json!({"cpu_cap_percent": 40}))));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(crate::config::read_policy_file(&r.exec.paths.layout().policy()).unwrap().cpu_cap_percent, Some(40));
    // Widening by the controller alone: held for the owner.
    let res = r.run(&r.sign(&r.cmd("policy.set", json!({"cpu_cap_percent": 90}))));
    assert_eq!(code_of(&res), code::APPROVAL_REQUIRED);
    assert_eq!(crate::config::read_policy_file(&r.exec.paths.layout().policy()).unwrap().cpu_cap_percent, Some(40));
    // A widening disguised as operate by the controller label still needs approval.
    let mut c = r.cmd("policy.set", json!({"cpu_cap_percent": 90}));
    c.class = Class::Read;
    assert_eq!(code_of(&r.run(&r.sign(&c))), code::APPROVAL_REQUIRED);
    // Owner-approved widening at `policy` level: runs.
    let res = r.run(&r.approve(r.cmd("policy.set", json!({"cpu_cap_percent": 90}))));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(res.result["widened"], json!(["cpu_cap_percent"]));
    // Bad field.
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("policy.set", json!({"cpu": 1}))))), code::BAD_ARGS);
    // policy.narrow is allowed even at `off` and persists.
    let off = Rig::new("off");
    let res = off.run(&off.sign(&off.cmd("policy.narrow", json!({"max_parallel": 1}))));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(super::policy::read_narrow(&off.exec.paths.config_dir).unwrap().max_parallel, Some(1));
    assert_eq!(code_of(&off.run(&off.sign(&off.cmd("policy.set", json!({"cpu_cap_percent": 10}))))), code::REMOTE_ADMIN_OFF);
}

#[test]
fn node_key_change_after_pin_is_wrong_node() {
    let r = Rig::new("policy");
    assert_eq!(r.run(&r.sign(&r.cmd("control.pause", json!({})))).state, "done");
    // A job (same uid as the node) swaps the node key; root keeps its pin.
    let k = SigningKey::generate().unwrap();
    crate::util::atomic_write(&r.exec.paths.layout().key(), k.to_pkcs8_pem().unwrap().as_bytes(), 0o600).unwrap();
    let mut c = r.cmd("control.resume", json!({}));
    c.node_fp = k.fingerprint();
    assert_eq!(code_of(&r.run(&r.sign(&c))), code::WRONG_NODE);
    assert_eq!(r.run(&r.sign(&r.cmd("control.resume", json!({})))).state, "done");
}

fn keys_frame(r: &Rig, keys: Vec<String>, rotate_with: Option<&ApprovalSigner>) -> AdminKeys {
    let mut k = AdminKeys { keys, node_fp: r.node_fp.clone(), iat: NOW, nonce: admin::new_nonce().unwrap(), sig_c: String::new(), sig_o: None };
    let part = k.signed_part().unwrap();
    k.sig_c = r.ctl.sign_domain(admin::DOMAIN_KEYS_CONTROLLER, &[&part]).unwrap();
    if let Some(o) = rotate_with {
        k.sig_o = Some(crypto::b64(&o.sign(&crypto::signed_bytes(admin::DOMAIN_KEYS_OWNER, &[&part]))));
    }
    k
}

#[test]
fn approval_keys_first_pin_then_rotation_needs_an_old_key() {
    let r = Rig::new("full");
    // Start with no approval keys.
    let mut f = config::read(&r.exec.paths.admin()).unwrap().unwrap().file;
    f.approval_keys.clear();
    config::write(&r.exec.paths.admin(), &f).unwrap();
    let a = ApprovalSigner::generate().unwrap();
    let res = r.exec.handle_keys(&keys_frame(&r, vec![a.public().encode()], None));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(res.result["mode"], "first_pin");
    // Rotation without an old key's signature: refused.
    let b = ApprovalSigner::generate().unwrap();
    let res = r.exec.handle_keys(&keys_frame(&r, vec![b.public().encode()], None));
    assert_eq!(code_of(&res), code::APPROVAL_REQUIRED);
    let res = r.exec.handle_keys(&keys_frame(&r, vec![b.public().encode()], Some(&b)));
    assert_eq!(code_of(&res), code::BAD_APPROVAL_SIGNATURE);
    let res = r.exec.handle_keys(&keys_frame(&r, vec![b.public().encode()], Some(&a)));
    assert_eq!(res.result["mode"], "rotate", "{res:?}");
    assert_eq!(config::read(&r.exec.paths.admin()).unwrap().unwrap().approval_keys, vec![b.public()]);
    // Replay of the same frame.
    let k = keys_frame(&r, vec![b.public().encode()], Some(&b));
    assert_eq!(r.exec.handle_keys(&k).state, "done");
    assert_eq!(code_of(&r.exec.handle_keys(&k)), code::REPLAYED);
}

#[test]
fn socket_round_trip_and_peer_check() {
    let r = Rig::new("policy");
    let sock = super::server::short_sock(&r.dir);
    let l = super::server::bind(&sock, None).unwrap();
    let me = unsafe { libc::geteuid() };
    let exec = Arc::new(r.exec);
    let e2 = exec.clone();
    let srv = std::thread::spawn(move || {
        // First conn: we are allowed; second: a rule that excludes us.
        let (s, _) = l.accept().unwrap();
        super::server::serve_conn(&e2, &super::server::PeerRule { allowed_uids: vec![me] }, s);
        let (s, _) = l.accept().unwrap();
        super::server::serve_conn(&e2, &super::server::PeerRule { allowed_uids: vec![u32::MAX - 1] }, s);
    });
    let c = Command {
        v: admin::ADMIN_V,
        cmd_id: "sock1".into(),
        node_fp: r.node_fp.clone(),
        controller_fp: r.ctl.fingerprint(),
        class: Class::Operate,
        verb: "control.pause".into(),
        args: json!({}),
        plan: None,
        proposed_by: "agent:it".into(),
        approved_by: None,
        approved_via: None,
        request_id: None,
        iat: NOW,
        exp: NOW + 60,
        nonce: admin::new_nonce().unwrap(),
    };
    let cmd = admin::sign_controller(&r.ctl, &c).unwrap();
    let t = std::time::Duration::from_secs(10);
    match super::server::request(&sock, &LocalRequest::Command { cmd: cmd.clone() }, None, t).unwrap() {
        LocalResponse::Result(res) => assert_eq!(res.state, "done", "{res:?}"),
        o => panic!("{o:?}"),
    }
    match super::server::request(&sock, &LocalRequest::Status, None, t).unwrap() {
        LocalResponse::Result(res) => assert_eq!(code_of(&res), code::PEER_NOT_ALLOWED),
        o => panic!("{o:?}"),
    }
    srv.join().unwrap();
}

// ── S5: signed self-update + rollback ────────────────────────────────

use k2_root_proto::node_build;
use k2_root_proto::testsign;

fn fake_build(version: &str) -> Vec<u8> {
    let (os, arch) = (crate::sysinfo::os_label(), std::env::consts::ARCH);
    let marker = node_build::format_marker(version, k2_node_proto::PROTOCOL, os, arch);
    if os == "macos" {
        node_build::fake_macho(arch, &marker)
    } else {
        testsign::fake_elf(arch, &marker)
    }
}

fn build_sig(bytes: &[u8], version: &str) -> String {
    let tc = node_build::trusted_comment(crate::sysinfo::os_label(), std::env::consts::ARCH, version, k2_node_proto::PROTOCOL);
    testsign::sign_with_test_key(bytes, &tc)
}

fn update_rig() -> (Rig, Vec<u8>) {
    let mut r = Rig::new("policy");
    r.exec.test_keys = true;
    // At the rollback floor (0.46.4): a manual rollback may go back to it.
    let old = fake_build(super::update::ROLLBACK_FLOOR);
    std::fs::create_dir_all(r.exec.paths.binary.parent().unwrap()).unwrap();
    std::fs::write(&r.exec.paths.binary, &old).unwrap();
    (r, old)
}

fn update_cmd(r: &Rig, bytes: &[u8], version: &str, trigger: &str) -> SignedCommand {
    r.sign(&r.cmd("node.update", json!({"version": version, "sha256": crypto::sha256_hex(bytes), "trigger": trigger})))
}

fn wait_phase(r: &Rig, phase: &str) -> super::update::UpdateRecord {
    for _ in 0..250 {
        let rec = super::update::load(&r.exec);
        if rec.phase == phase {
            return rec;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("update never reached {phase}: {:?}", super::update::load(&r.exec));
}

fn wait_calls(r: &Rig, n: usize) {
    for _ in 0..250 {
        if r.plat.calls.lock().unwrap().len() >= n {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("expected {n} platform calls: {:?}", r.plat.calls.lock().unwrap());
}

fn write_mark(r: &Rig, version: &str, at: i64) {
    let m = super::update::HealthyMark { version: version.into(), at, handshake: true };
    crate::util::atomic_write(&super::update::healthy_path(&r.exec.paths.home), &serde_json::to_vec(&m).unwrap(), 0o644).unwrap();
}

#[test]
fn good_update_swaps_restarts_and_passes_the_health_gate() {
    let (r, old) = update_rig();
    let new = fake_build("99.0.0");
    let res = r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &build_sig(&new, "99.0.0"), &new);
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(res.result["phase"], "swapped");
    assert_eq!(std::fs::read(&r.exec.paths.binary).unwrap(), new);
    assert_eq!(std::fs::read(format!("{}.prev", r.exec.paths.binary.display())).unwrap(), old);
    assert_eq!(*r.plat.calls.lock().unwrap(), vec!["restart-node".to_string()]);
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = std::fs::metadata(&r.exec.paths.binary).unwrap();
        assert_eq!(m.permissions().mode() & 0o7777, 0o755, "the swapped-in build is 0755 (Bug 1)");
        assert_eq!(m.uid(), unsafe { libc::geteuid() });
    }
    assert_eq!(r.plat.preflights.lock().unwrap().len(), 1, "the staged build ran once as the node user before the swap");
    let mark = super::update::read_update_mark(&r.exec.paths.config_dir).expect("an update restart is marked");
    assert_eq!((mark.what.as_str(), mark.node_pid, mark.keep), ("node.update", Some(node_pid()), false));
    write_mark(&r, "99.0.0", NOW + 1);
    let rec = wait_phase(&r, "healthy");
    assert_eq!((rec.from.as_str(), rec.to.as_str()), (crate::PRODUCT_VERSION, "99.0.0"));
    // T3: one step per stage, in order, every one ok, in update.json.
    let names: Vec<&str> = rec.steps.iter().map(|s| s.step.as_str()).collect();
    assert_eq!(names, vec!["received", "verify_sig", "stage", "preflight", "decide", "swap", "mark", "restart", "health_wait", "healthy"]);
    assert!(rec.steps.iter().all(|s| s.ok), "{:?}", rec.steps);
    assert_eq!(rec.steps[4].detail.as_deref(), Some("restart running=0"), "a solo node restarts");
    for _ in 0..250 {
        if super::update::read_update_mark(&r.exec.paths.config_dir).unwrap().ended {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(super::update::read_update_mark(&r.exec.paths.config_dir).unwrap().ended, "the gate's end retires the mark (it keeps nothing)");
    assert!(!r.exec.reexec.load(Ordering::SeqCst), "tests don't re-exec");
    // Manual rollback goes to the local .prev only.
    let res = r.run(&r.sign(&r.cmd("node.rollback", json!({}))));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(std::fs::read(&r.exec.paths.binary).unwrap(), old);
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("node.rollback", json!({}))))), code::NO_PREVIOUS);
}

#[test]
fn bad_builds_refuse_and_leave_the_binary() {
    let (r, old) = update_rig();
    let new = fake_build("99.0.0");
    let sig = build_sig(&new, "99.0.0");
    // Tampered bytes (the controller even signed the tampered sha).
    let mut bad = new.clone();
    bad[40] ^= 1;
    let res = r.exec.handle_update(&update_cmd(&r, &bad, "99.0.0", "explicit"), &sig, &bad);
    assert_eq!(code_of(&res), code::BAD_BUILD, "{res:?}");
    // Another key.
    let tc = node_build::trusted_comment(crate::sysinfo::os_label(), std::env::consts::ARCH, "99.0.0", k2_node_proto::PROTOCOL);
    let other = testsign::sign(&[3u8; 32], [9, 9, 9, 9, 9, 9, 9, 9], &new, &tc);
    assert_eq!(code_of(&r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &other, &new)), code::BAD_BUILD);
    // Bytes differ from what the controller authorized.
    assert_eq!(code_of(&r.exec.handle_update(&update_cmd(&r, &old, "99.0.0", "explicit"), &sig, &new)), code::BAD_BUILD);
    // A root-helper-signed build (domain separation).
    let helper_tc = k2_root_proto::sig::trusted_comment(std::env::consts::ARCH, "99.0.0", k2_node_proto::PROTOCOL);
    let hs = testsign::sign_with_test_key(&new, &helper_tc);
    assert_eq!(code_of(&r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &hs, &new)), code::BAD_BUILD);
    // Older and same versions.
    let older = fake_build("0.0.2");
    assert_eq!(code_of(&r.exec.handle_update(&update_cmd(&r, &older, "0.0.2", "explicit"), &build_sig(&older, "0.0.2"), &older)), code::DOWNGRADE);
    let same = fake_build(crate::PRODUCT_VERSION);
    let res = r.exec.handle_update(&update_cmd(&r, &same, crate::PRODUCT_VERSION, "explicit"), &build_sig(&same, crate::PRODUCT_VERSION), &same);
    assert_eq!(code_of(&res), code::SAME_VERSION);
    // A release-keys-only executor refuses the TEST-key build.
    let mut rk = Rig::new("policy");
    rk.exec.test_keys = false;
    std::fs::create_dir_all(rk.exec.paths.binary.parent().unwrap()).unwrap();
    std::fs::write(&rk.exec.paths.binary, &old).unwrap();
    assert_eq!(code_of(&rk.exec.handle_update(&update_cmd(&rk, &new, "99.0.0", "explicit"), &sig, &new)), code::BAD_BUILD);
    assert_eq!(std::fs::read(&r.exec.paths.binary).unwrap(), old, "nothing was installed");
    assert!(r.plat.calls.lock().unwrap().is_empty());
    // An unsigned (controller) update command never reaches the build check.
    let mut s = update_cmd(&r, &new, "99.0.0", "explicit");
    s.sig_c = crypto::b64(b"x");
    assert_eq!(code_of(&r.exec.handle_update(&s, &sig, &new)), code::BAD_CONTROLLER_SIGNATURE);
}

#[test]
fn unhealthy_update_rolls_back_and_the_version_is_refused_after() {
    let (r, old) = update_rig();
    let new = fake_build("99.0.0");
    let sig = build_sig(&new, "99.0.0");
    assert_eq!(r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &sig, &new).state, "done");
    // No healthy mark; time passes the gate.
    r.plat.now.store(NOW + super::update::HEALTH_GATE_SECS + 1, Ordering::SeqCst);
    let rec = wait_phase(&r, "rolled_back");
    wait_calls(&r, 2); // the record is written before the swap; the restart comes after it
    assert_eq!(rec.reason.as_deref(), Some("health_gate"));
    assert!(rec.refused.contains(&"99.0.0".to_string()));
    assert_eq!(std::fs::read(&r.exec.paths.binary).unwrap(), old);
    assert_eq!(std::fs::read(format!("{}.bad", r.exec.paths.binary.display())).unwrap(), new);
    assert_eq!(*r.plat.calls.lock().unwrap(), vec!["restart-node".to_string(), "restart-node".to_string()]);
    // T3: the rollback's steps follow the update's, in one record (the
    // restart step is saved just after the platform call returns).
    let mut rec = super::update::load(&r.exec);
    for _ in 0..250 {
        if rec.steps.len() >= 12 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        rec = super::update::load(&r.exec);
    }
    let names: Vec<&str> = rec.steps.iter().map(|s| s.step.as_str()).collect();
    assert_eq!(names, vec!["received", "verify_sig", "stage", "preflight", "decide", "swap", "mark", "restart", "health_wait", "rollback", "mark", "restart"]);
    assert_eq!(rec.steps[9].detail.as_deref(), Some("why=health_gate"));
    let res = r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &sig, &new);
    assert_eq!(code_of(&res), code::ROLLED_BACK_VERSION);
    // A healthy mark from another version never passes a gate.
    let newer = fake_build("99.0.1");
    let mut c = r.cmd("node.update", json!({"version": "99.0.1", "sha256": crypto::sha256_hex(&newer), "trigger": "explicit"}));
    c.iat = NOW + 1000;
    c.exp = NOW + 1600;
    r.plat.now.store(NOW + 1000, Ordering::SeqCst);
    assert_eq!(r.exec.handle_update(&r.sign(&c), &build_sig(&newer, "99.0.1"), &newer).state, "done");
    write_mark(&r, "0.0.1", NOW + 1500);
    r.plat.now.store(NOW + 1000 + super::update::HEALTH_GATE_SECS + 1, Ordering::SeqCst);
    let rec = wait_phase(&r, "rolled_back");
    wait_calls(&r, 4);
    assert!(rec.refused.contains(&"99.0.1".to_string()));
    // The rollback's audit line follows its restart (and the mark's retirement).
    let updates = || r.exec.audit().verify().unwrap().iter().filter(|l| l.event == "update").count();
    for _ in 0..250 {
        if updates() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(updates() >= 2, "both rollbacks are audited");
}

/// Bug 1 / T2 (no root): a staged build the node user can't run never
/// goes in: refused before the swap, `.new` removed, nothing restarted.
#[test]
fn a_build_that_fails_the_preflight_is_never_swapped_in() {
    let (r, old) = update_rig();
    *r.plat.fail_preflight.lock().unwrap() = true;
    let new = fake_build("99.0.0");
    let res = r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &build_sig(&new, "99.0.0"), &new);
    assert_eq!(code_of(&res), code::BAD_BUILD, "{res:?}");
    assert!(res.message.as_deref().unwrap_or("").contains("doesn't run as"), "{res:?}");
    assert_eq!(std::fs::read(&r.exec.paths.binary).unwrap(), old, "the running build stays");
    assert!(!std::path::Path::new(&format!("{}.new", r.exec.paths.binary.display())).exists(), ".new is removed");
    assert!(r.plat.calls.lock().unwrap().is_empty(), "nothing restarted");
    assert!(super::update::read_update_mark(&r.exec.paths.config_dir).is_none());
    assert_eq!(super::update::load(&r.exec).phase, "", "no gate started");
    // The next offer of the same build (now runnable) goes through.
    *r.plat.fail_preflight.lock().unwrap() = false;
    assert_eq!(r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &build_sig(&new, "99.0.0"), &new).state, "done");
}

/// C5: after a healthy gate a serving executor sets `reexec` and returns
/// (`serve` then drains its connections); it never exits mid-request.
#[test]
fn a_healthy_gate_asks_serve_to_reexec_instead_of_exiting() {
    let (mut r, _) = update_rig();
    r.exec.exit_after_update = true;
    let new = fake_build("99.0.0");
    assert_eq!(r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &build_sig(&new, "99.0.0"), &new).state, "done");
    assert!(!r.exec.reexec.load(Ordering::SeqCst), "not before the gate passes");
    write_mark(&r, "99.0.0", NOW + 1);
    let rec = wait_phase(&r, "healthy");
    for _ in 0..250 {
        if r.exec.reexec.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(r.exec.reexec.load(Ordering::SeqCst), "serve is told to re-exec");
    let rec2 = super::update::load(&r.exec);
    assert_eq!(rec2.steps.last().map(|s| s.step.as_str()), Some("reexec"), "{:?} / {:?}", rec.steps, rec2.steps);
    // This test process is still here: no process::exit.
}

/// A-M2 / B-L4: a manual rollback never goes below 0.46.4 (the updater
/// fix) or to a build with no marker; the gate's own rollback still
/// restores whatever ran before. B-L1: `<bin>` is replaced in one rename
/// and the build going out stays as `.bad`.
#[test]
fn manual_rollback_has_a_floor_and_swaps_atomically() {
    let (r, _) = update_rig();
    let bin = r.exec.paths.binary.clone();
    let prev = std::path::PathBuf::from(format!("{}.prev", bin.display()));
    let cur = fake_build("0.46.6");
    std::fs::write(&bin, &cur).unwrap();
    std::fs::write(&prev, fake_build("0.46.3")).unwrap();
    let res = r.run(&r.sign(&r.cmd("node.rollback", json!({}))));
    assert_eq!(code_of(&res), code::DOWNGRADE, "{res:?}");
    assert!(res.message.as_deref().unwrap_or("").contains("0.46.4"), "{res:?}");
    std::fs::write(&prev, b"no marker here").unwrap();
    assert_eq!(code_of(&r.run(&r.sign(&r.cmd("node.rollback", json!({}))))), code::DOWNGRADE);
    assert_eq!(std::fs::read(&bin).unwrap(), cur, "nothing moved");
    assert!(r.plat.calls.lock().unwrap().is_empty(), "nothing restarted");
    // At the floor: goes, atomically.
    let back = fake_build("0.46.4");
    std::fs::write(&prev, &back).unwrap();
    let res = r.run(&r.sign(&r.cmd("node.rollback", json!({}))));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(std::fs::read(&bin).unwrap(), back);
    assert_eq!(std::fs::read(format!("{}.bad", bin.display())).unwrap(), cur);
    assert!(!prev.exists());
    let m = super::update::read_update_mark(&r.exec.paths.config_dir).unwrap();
    assert!(m.ended, "the rollback's restart is over: its mark keeps nothing (B-M4)");
}

/// B-L2: the record says `swapping` before the rename; an executor that
/// died in between finishes the swap (the new build is in: restart into
/// the gate) or drops it (it isn't: nothing changed).
#[test]
fn an_interrupted_swap_is_finished_or_dropped_at_start() {
    let (r, old) = update_rig();
    let bin = r.exec.paths.binary.clone();
    let new = fake_build("99.0.0");
    let rec = super::update::UpdateRecord { phase: "swapping".into(), from: "0.46.4".into(), to: "99.0.0".into(), at: NOW, cmd_id: Some("c9".into()), ..Default::default() };
    let save = |rec: &super::update::UpdateRecord| std::fs::write(r.exec.paths.update_state(), serde_json::to_vec(rec).unwrap()).unwrap();
    // Not swapped yet: dropped, .new removed, nothing restarted.
    std::fs::write(format!("{}.new", bin.display()), &new).unwrap();
    save(&rec);
    super::update::resume_pending(&r.exec);
    let got = super::update::load(&r.exec);
    assert_eq!(got.phase, "failed", "{got:?}");
    assert!(!std::path::Path::new(&format!("{}.new", bin.display())).exists());
    assert_eq!(std::fs::read(&bin).unwrap(), old);
    assert!(r.plat.calls.lock().unwrap().is_empty());
    // Swapped already: the gate runs.
    std::fs::write(&bin, &new).unwrap();
    save(&rec);
    super::update::resume_pending(&r.exec);
    let got = super::update::load(&r.exec);
    assert_eq!(got.phase, "swapped", "{got:?}");
    assert_eq!(*r.plat.calls.lock().unwrap(), vec!["restart-node".to_string()], "the node restarts into the gate");
    write_mark(&r, "99.0.0", NOW + 1);
    wait_phase(&r, "healthy");
}

#[test]
fn auto_update_off_refuses_auto_offers_only() {
    let (r, _) = update_rig();
    let mut f = config::read(&r.exec.paths.admin()).unwrap().unwrap().file;
    f.auto_update = "off".into();
    config::write(&r.exec.paths.admin(), &f).unwrap();
    let new = fake_build("99.0.0");
    let sig = build_sig(&new, "99.0.0");
    assert_eq!(code_of(&r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "auto"), &sig, &new)), code::AUTO_UPDATE_OFF);
    assert_eq!(r.exec.handle_update(&update_cmd(&r, &new, "99.0.0", "explicit"), &sig, &new).state, "done");
}
