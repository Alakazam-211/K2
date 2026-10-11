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
        Ok(())
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
        let plat = Arc::new(RecPlatform { now: AtomicI64::new(NOW), calls: Mutex::new(vec![]), fail_restart: Mutex::new(false) });
        let exec = Executor::new(paths, plat.clone());
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
