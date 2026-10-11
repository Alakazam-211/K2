//! The root executor's decision core (prd-k2-node-admin-v1 §7.3): every
//! check, in order, then the verb. No I/O beyond the config dir, the
//! state dir and the [`Platform`]; the socket lives in `server`.
//!
//! Order (verify before parse; any doubt refuses):
//! 1. (server) peer is the node user or root; message ≤ 1 MiB.
//! 2. `sig_c` over the raw body against root's pinned controller key.
//! 3. body parses strictly; this node, this controller; `iat`/`exp`
//!    alive (±5 min); nonce unseen.
//! 4. the executor classifies the verb from the node's own state; a
//!    privileged one needs `sig_o` by a pinned approval key.
//! 5. `remote_admin` allows it; args validate.
//! 6. audit `exec` + record the nonce **before** running; run; audit
//!    `result`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use k2_node_proto::admin::{
    self, code, AdminKeys, AdminRefusal, AdminResult, ApprovalKey, Class, Level, Opened, Runs, SignedCommand,
    CLOCK_SKEW_SECS, MAX_LIFETIME_SECS,
};
use k2_node_proto::crypto;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::config::{self, AdminConfig, AdminFile};
use super::store::{Audit, AuditEntry, NonceStore};

/// Where the executor reads and writes. Fixed by the root-owned service
/// unit's argv; nothing a socket peer sends can change them.
#[derive(Debug, Clone)]
pub struct ExecPaths {
    /// `/etc/k2-node` (admin.toml, policy.toml, control.toml, narrow.toml).
    pub config_dir: PathBuf,
    /// Root-only state (0700).
    pub state_dir: PathBuf,
    /// The node home (read: the node key; write: nothing but `run/`).
    pub home: PathBuf,
    /// The installed `k2-node` binary (self-update swaps it).
    pub binary: PathBuf,
}

impl ExecPaths {
    pub fn admin(&self) -> PathBuf {
        self.config_dir.join(config::ADMIN_FILE)
    }
    pub fn nonces(&self) -> PathBuf {
        self.state_dir.join("nonces.json")
    }
    pub fn audit(&self) -> PathBuf {
        self.state_dir.join("audit.log")
    }
    pub fn node_fp_pin(&self) -> PathBuf {
        self.state_dir.join("node_fp")
    }
    pub fn update_state(&self) -> PathBuf {
        self.state_dir.join("update.json")
    }
    pub fn layout(&self) -> crate::paths::Layout {
        crate::paths::Layout::new(&self.home, &self.config_dir)
    }
}

/// The OS glue (service manager). Swapped for a recorder in tests.
pub trait Platform: Send + Sync {
    fn now(&self) -> i64;
    fn os(&self) -> &'static str;
    fn arch(&self) -> &'static str;
    /// Restart the node service (`k2-node.service` / `system/dev.k2.node`).
    fn restart_node(&self) -> Result<(), String>;
    /// Restart a K2-owned unit by its short name (`k2-node`, `k2-node-packages`).
    fn restart_unit(&self, unit: &str) -> Result<(), String>;
}

/// systemd on Linux, launchd on macOS. Absolute program paths, clean env.
pub struct SystemPlatform;

fn run_fixed(program: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{program} {}: exit {:?}: {}", args.join(" "), out.status.code(), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// K2-owned units a remote `unit.restart` may name.
pub const K2_UNITS: &[&str] = &["k2-node", "k2-node-packages"];

impl Platform for SystemPlatform {
    fn now(&self) -> i64 {
        crate::util::now()
    }
    fn os(&self) -> &'static str {
        crate::sysinfo::os_label()
    }
    fn arch(&self) -> &'static str {
        std::env::consts::ARCH
    }
    fn restart_node(&self) -> Result<(), String> {
        self.restart_unit("k2-node")
    }
    fn restart_unit(&self, unit: &str) -> Result<(), String> {
        if !K2_UNITS.contains(&unit) {
            return Err(format!("{unit} is not a K2 unit"));
        }
        if cfg!(target_os = "macos") {
            match unit {
                "k2-node" => run_fixed("/bin/launchctl", &["kickstart", "-k", "system/dev.k2.node"]),
                _ => Err(format!("{unit} has no launchd job on macOS")),
            }
        } else {
            let u = match unit {
                "k2-node" => "k2-node.service",
                _ => "k2-node-packages.path",
            };
            run_fixed("/usr/bin/systemctl", &["restart", u]).or_else(|_| run_fixed("/bin/systemctl", &["restart", u]))
        }
    }
}

/// Test-keys builds only: a hook program replaces the service manager
/// (`<hook> restart-node` / `<hook> restart-unit <name>`), so the Linux
/// container end-to-end can run without systemd. A release build has no
/// such code path.
#[cfg(feature = "test-keys")]
pub struct HookPlatform(pub PathBuf);

#[cfg(feature = "test-keys")]
impl Platform for HookPlatform {
    fn now(&self) -> i64 {
        crate::util::now()
    }
    fn os(&self) -> &'static str {
        crate::sysinfo::os_label()
    }
    fn arch(&self) -> &'static str {
        std::env::consts::ARCH
    }
    fn restart_node(&self) -> Result<(), String> {
        run_fixed(&self.0.display().to_string(), &["restart-node"])
    }
    fn restart_unit(&self, unit: &str) -> Result<(), String> {
        run_fixed(&self.0.display().to_string(), &["restart-unit", unit])
    }
}

/// What the node may ask over the local socket. Header JSON; an `update`
/// is followed by one binary frame of `size` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LocalRequest {
    Command { cmd: SignedCommand },
    Update { cmd: SignedCommand, sig: String, size: u64 },
    Keys { keys: AdminKeys },
    Status,
}

/// The executor's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalResponse {
    Result(AdminResult),
    Status(ExecStatus),
}

/// `status` (unsigned, read-only; feeds health §10.1 "executor").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecStatus {
    pub version: String,
    pub protocol: u32,
    pub remote_admin: String,
    pub controller_fp: Option<String>,
    pub approval_key_fps: Vec<String>,
    pub auto_update: String,
    pub node_fp: Option<String>,
    pub configured: bool,
    pub config_error: Option<String>,
    pub last_update: Option<Value>,
}

/// A checked command, ready to run.
pub struct Checked {
    pub opened: Opened,
    pub class: Class,
    pub approval_key: Option<String>,
    pub cfg: AdminConfig,
    pub node_fp: String,
}

pub struct Executor {
    pub paths: ExecPaths,
    pub platform: Arc<dyn Platform>,
    /// Also trust the TEST minisign key (test-keys builds only).
    pub test_keys: bool,
    /// One root action at a time (the server is single-threaded; the
    /// update health gate runs in the background and holds this).
    pub busy: Arc<Mutex<()>>,
    /// After a healthy update (or a rollback) exit so the service manager
    /// starts the new binary. True in `admin serve`; false in tests.
    pub exit_after_update: bool,
}

type Refused = (String, String, AdminRefusal);

fn refused(o: Option<&Opened>, r: AdminRefusal) -> Refused {
    match o {
        Some(o) => (o.cmd.cmd_id.clone(), o.digest(), r),
        None => (String::new(), String::new(), r),
    }
}

impl Executor {
    pub fn new(paths: ExecPaths, platform: Arc<dyn Platform>) -> Self {
        Self { paths, platform, test_keys: cfg!(feature = "test-keys"), busy: Arc::new(Mutex::new(())), exit_after_update: false }
    }

    pub fn audit(&self) -> Audit {
        Audit::new(&self.paths.audit())
    }

    pub fn now(&self) -> i64 {
        self.platform.now()
    }

    /// The node fingerprint, pinned in root state on first use (the node
    /// key file is the node user's; root never trusts a later change).
    pub fn node_fp(&self) -> Result<String, AdminRefusal> {
        let pin = self.paths.node_fp_pin();
        if let Ok(s) = std::fs::read_to_string(&pin) {
            let s = s.trim().to_string();
            if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(s);
            }
            return Err(AdminRefusal::new(code::NOT_CONFIGURED, "node fingerprint pin is unreadable"));
        }
        let pem = std::fs::read_to_string(self.paths.layout().key())
            .map_err(|_| AdminRefusal::new(code::NOT_CONFIGURED, "no node key yet (enroll the node first)"))?;
        let key = crypto::SigningKey::from_pkcs8_pem(&pem).map_err(|_| AdminRefusal::new(code::NOT_CONFIGURED, "node key unreadable"))?;
        let fp = key.fingerprint();
        crate::util::atomic_write(&pin, fp.as_bytes(), 0o600).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
        Ok(fp)
    }

    fn config(&self) -> Result<AdminConfig, AdminRefusal> {
        match config::read(&self.paths.admin()) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err(AdminRefusal::new(code::REMOTE_ADMIN_OFF, "remote admin is not set up on this node (sudo k2-node setup --remote-admin …)")),
            Err(e) => Err(AdminRefusal::new(code::NOT_CONFIGURED, format!("admin.toml: {e}"))),
        }
    }

    /// §7.3 steps 2–5. Never runs anything.
    pub fn check(&self, raw: &[u8]) -> Result<Checked, Refused> {
        let cfg = self.config().map_err(|r| refused(None, r))?;
        let ctl = cfg
            .controller
            .clone()
            .ok_or_else(|| refused(None, AdminRefusal::new(code::NOT_CONFIGURED, "no controller key pinned (sudo k2-node admin pin)")))?;
        let o = admin::open(raw, &ctl.spki_der).map_err(|r| refused(None, r))?;
        let node_fp = self.node_fp().map_err(|r| refused(Some(&o), r))?;
        let now = self.now();
        admin::check_target_and_time(&o.cmd, &node_fp, &ctl.fp, now).map_err(|r| refused(Some(&o), r))?;
        let nonces = NonceStore::open(&self.paths.nonces()).map_err(|e| refused(Some(&o), AdminRefusal::new(code::FAILED, e)))?;
        if nonces.seen(&o.cmd.nonce) {
            return Err(refused(Some(&o), AdminRefusal::new(code::REPLAYED, "this command already ran")));
        }
        let spec = admin::verb(&o.cmd.verb)
            .ok_or_else(|| refused(Some(&o), AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not on this node's list")))?;
        if spec.runs != Runs::Executor {
            return Err(refused(Some(&o), AdminRefusal::new(code::VERB_NOT_ALLOWED, "the node answers this verb itself")));
        }
        let computed = self.classify(&o.cmd.verb, &o.cmd.args).map_err(|r| refused(Some(&o), r))?;
        let class = computed.max(o.cmd.class);
        let approval_key = if class == Class::Privileged {
            Some(admin::verify_owner(&o, &cfg.approval_keys, now).map_err(|r| refused(Some(&o), r))?)
        } else {
            None
        };
        if cfg.level < spec.min_level {
            return Err(refused(
                Some(&o),
                AdminRefusal::new(code::REMOTE_ADMIN_OFF, format!("remote admin here is {}; {} needs {}", cfg.level.as_str(), spec.name, spec.min_level.as_str())),
            ));
        }
        Ok(Checked { opened: o, class, approval_key, cfg, node_fp })
    }

    /// The executor's own class for a verb (never the controller's label).
    pub fn classify(&self, verb: &str, args: &Value) -> Result<Class, AdminRefusal> {
        let spec = admin::verb(verb).ok_or_else(|| AdminRefusal::new(code::VERB_NOT_ALLOWED, "unknown verb"))?;
        match verb {
            "policy.set" => super::policy::classify_set(&self.paths.layout(), args),
            _ => Ok(spec.base),
        }
    }

    /// Audit a refusal (best effort: a refusal is never upgraded to an
    /// allow because the audit failed) and build the result.
    pub fn refuse_result(&self, (cmd_id, digest, r): Refused) -> AdminResult {
        let seq = self
            .audit()
            .append(
                AuditEntry {
                    event: "refused".into(),
                    cmd_id: (!cmd_id.is_empty()).then(|| cmd_id.clone()),
                    digest: (!digest.is_empty()).then(|| digest.clone()),
                    code: Some(r.code.clone()),
                    ..Default::default()
                },
                self.now(),
            )
            .ok();
        let mut res = AdminResult::refused(&cmd_id, &digest, &r);
        res.audit_seq = seq;
        res
    }

    /// Steps 6: audit, nonce, run, audit.
    fn run_checked(&self, c: Checked, payload: Option<(&str, &[u8])>) -> AdminResult {
        let o = &c.opened;
        let digest = o.digest();
        let base = AuditEntry {
            cmd_id: Some(o.cmd.cmd_id.clone()),
            verb: Some(o.cmd.verb.clone()),
            class: Some(c.class.as_str().into()),
            digest: Some(digest.clone()),
            proposed_by: Some(o.cmd.proposed_by.clone()),
            approved_by: o.cmd.approved_by.clone(),
            approval_key: c.approval_key.clone(),
            ..Default::default()
        };
        let now = self.now();
        let audit = self.audit();
        let exec_seq = match audit.append(AuditEntry { event: "exec".into(), detail: json!({"args": o.cmd.args}), ..base.clone() }, now) {
            Ok(s) => s,
            Err(e) => return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::FAILED, format!("audit write failed, not running: {e}"))),
        };
        let mut nonces = match NonceStore::open(&self.paths.nonces()) {
            Ok(n) => n,
            Err(e) => return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::FAILED, e)),
        };
        if nonces.seen(&o.cmd.nonce) {
            return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::REPLAYED, "this command already ran"));
        }
        if let Err(e) = nonces.insert(&o.cmd.nonce, o.cmd.exp, now) {
            return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::FAILED, format!("nonce store: {e}")));
        }
        let out = self.dispatch(&c, payload);
        let (mut res, code_s) = match out {
            Ok(v) => (AdminResult::done(&o.cmd.cmd_id, &digest, v), None),
            Err(r) => {
                let mut x = AdminResult::refused(&o.cmd.cmd_id, &digest, &r);
                if r.code == code::FAILED {
                    x.state = "failed".into();
                }
                let c = r.code.clone();
                (x, Some(c))
            }
        };
        let seq = audit
            .append(
                AuditEntry { event: "result".into(), code: code_s, detail: json!({"state": res.state, "exec_seq": exec_seq}), ..base },
                self.now(),
            )
            .unwrap_or(exec_seq);
        res.audit_seq = Some(seq);
        res
    }

    /// One `command` request (operate or privileged).
    pub fn handle_command(&self, cmd: &SignedCommand) -> AdminResult {
        let raw = match cmd.to_json() {
            Ok(r) => r,
            Err(e) => return AdminResult::refused("", "", &AdminRefusal::new(code::MALFORMED, e)),
        };
        let _g = self.busy.lock().unwrap_or_else(|p| p.into_inner());
        match self.check(raw.as_bytes()) {
            Ok(c) => {
                if c.opened.cmd.verb == "node.update" {
                    return self.refuse_result(refused(Some(&c.opened), AdminRefusal::new(code::BAD_ARGS, "node.update comes with its build (op update)")));
                }
                self.run_checked(c, None)
            }
            Err(r) => self.refuse_result(r),
        }
    }

    /// One `update` request: the signed `node.update` command plus the
    /// build bytes (already read into memory by the server).
    pub fn handle_update(&self, cmd: &SignedCommand, sig: &str, bytes: &[u8]) -> AdminResult {
        let raw = match cmd.to_json() {
            Ok(r) => r,
            Err(e) => return AdminResult::refused("", "", &AdminRefusal::new(code::MALFORMED, e)),
        };
        let guard = match self.busy.try_lock() {
            Ok(g) => g,
            Err(_) => return AdminResult::refused("", "", &AdminRefusal::new(code::BUSY, "another root action (or an update health check) is running")),
        };
        let r = match self.check(raw.as_bytes()) {
            Ok(c) if c.opened.cmd.verb == "node.update" => self.run_checked(c, Some((sig, bytes))),
            Ok(c) => self.refuse_result(refused(Some(&c.opened), AdminRefusal::new(code::BAD_ARGS, "op update carries only node.update"))),
            Err(r) => self.refuse_result(r),
        };
        drop(guard);
        r
    }

    /// The verbs (S1: level, control, unit; S3 policy; S5 update/rollback).
    fn dispatch(&self, c: &Checked, payload: Option<(&str, &[u8])>) -> Result<Value, AdminRefusal> {
        let args = &c.opened.cmd.args;
        let by = format!("controller:{}", c.opened.cmd.proposed_by);
        match c.opened.cmd.verb.as_str() {
            "admin.level.lower" => self.level_lower(c, args),
            "control.pause" | "control.drain" | "control.resume" => {
                no_args(args)?;
                let to = match c.opened.cmd.verb.as_str() {
                    "control.pause" => k2_node_proto::frames::Control::Paused,
                    "control.drain" => k2_node_proto::frames::Control::Draining,
                    _ => k2_node_proto::frames::Control::Active,
                };
                crate::config::write_control(&self.paths.layout().control(), to, &by, self.now()).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
                Ok(json!({"control": to.as_str()}))
            }
            "unit.restart" => {
                let unit = str_arg(args, "unit")?;
                only_keys(args, &["unit"])?;
                if !K2_UNITS.contains(&unit.as_str()) {
                    return Err(AdminRefusal::new(code::BAD_ARGS, "unit must be a K2 unit (k2-node, k2-node-packages)"));
                }
                self.platform.restart_unit(&unit).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
                Ok(json!({"restarted": unit}))
            }
            "policy.set" => super::policy::apply_set(&self.paths.layout(), args),
            "policy.narrow" => super::policy::apply_narrow(&self.paths.config_dir, args),
            "node.update" => {
                let (sig, bytes) = payload.ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "node.update needs its build"))?;
                super::update::apply(self, c, sig, bytes)
            }
            "node.rollback" => {
                no_args_except(args, &["reason"])?;
                super::update::rollback(self, "manual")
            }
            _ => Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not built on this node yet")),
        }
    }

    fn level_lower(&self, _c: &Checked, args: &Value) -> Result<Value, AdminRefusal> {
        only_keys(args, &["level"])?;
        let to = Level::parse(&str_arg(args, "level")?).ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "level must be off|policy|full"))?;
        let cfg = self.config()?;
        if to >= cfg.level {
            return Err(AdminRefusal::new(code::HUMAN_REQUIRED, "the level can only be lowered remotely; raise it at the machine (sudo k2-node admin level …)"));
        }
        let mut f: AdminFile = cfg.file.clone();
        f.remote_admin = to.as_str().into();
        config::write(&self.paths.admin(), &f).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
        Ok(json!({"remote_admin": to.as_str(), "was": cfg.level.as_str()}))
    }

    /// `admin_keys` (§7.4): first pin with the controller signature (trust
    /// on first use); rotation also needs a currently pinned key.
    pub fn handle_keys(&self, k: &AdminKeys) -> AdminResult {
        let _g = self.busy.lock().unwrap_or_else(|p| p.into_inner());
        let r = (|| -> Result<Value, AdminRefusal> {
            let cfg = self.config()?;
            let ctl = cfg.controller.clone().ok_or_else(|| AdminRefusal::new(code::NOT_CONFIGURED, "no controller key pinned"))?;
            let part = k.signed_part().map_err(|e| AdminRefusal::new(code::MALFORMED, e))?;
            if !crypto::verify_domain(&ctl.spki_der, admin::DOMAIN_KEYS_CONTROLLER, &[&part], &k.sig_c) {
                return Err(AdminRefusal::new(code::BAD_CONTROLLER_SIGNATURE, "controller signature does not verify"));
            }
            let node_fp = self.node_fp()?;
            if !crypto::ct_eq(k.node_fp.as_bytes(), node_fp.as_bytes()) {
                return Err(AdminRefusal::new(code::WRONG_NODE, "keys are for another node"));
            }
            let now = self.now();
            if k.iat > now + CLOCK_SKEW_SECS || now > k.iat + MAX_LIFETIME_SECS + CLOCK_SKEW_SECS {
                return Err(AdminRefusal::new(code::EXPIRED, "key update expired"));
            }
            let mut nonces = NonceStore::open(&self.paths.nonces()).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
            if nonces.seen(&k.nonce) {
                return Err(AdminRefusal::new(code::REPLAYED, "this key update already ran"));
            }
            let mut new = Vec::new();
            for s in &k.keys {
                new.push(ApprovalKey::parse(s).ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "approval key is not ed25519:<b64>"))?);
            }
            if new.is_empty() || new.len() > 4 {
                return Err(AdminRefusal::new(code::BAD_ARGS, "1 to 4 approval keys"));
            }
            let mode = if cfg.approval_keys.is_empty() {
                "first_pin"
            } else {
                let sig = k
                    .sig_o
                    .as_ref()
                    .and_then(|s| crypto::unb64(s).ok())
                    .ok_or_else(|| AdminRefusal::new(code::APPROVAL_REQUIRED, "rotation needs a signature by a pinned approval key"))?;
                let msg = crypto::signed_bytes(admin::DOMAIN_KEYS_OWNER, &[&part]);
                if !cfg.approval_keys.iter().any(|a| a.verify(&msg, &sig)) {
                    return Err(AdminRefusal::new(code::BAD_APPROVAL_SIGNATURE, "rotation not signed by a pinned approval key"));
                }
                "rotate"
            };
            nonces.insert(&k.nonce, k.iat + MAX_LIFETIME_SECS, now).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
            let mut f = cfg.file.clone();
            f.approval_keys = new.iter().map(|a| a.encode()).collect();
            config::write(&self.paths.admin(), &f).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
            let fps: Vec<String> = new.iter().map(|a| a.fingerprint()).collect();
            let _ = self.audit().append(AuditEntry { event: "keys".into(), detail: json!({"mode": mode, "fps": fps}), ..Default::default() }, now);
            Ok(json!({"mode": mode, "approval_key_fps": fps}))
        })();
        match r {
            Ok(v) => AdminResult::done("admin_keys", "", v),
            Err(e) => self.refuse_result((String::from("admin_keys"), String::new(), e)),
        }
    }

    pub fn status(&self) -> ExecStatus {
        let (cfg, err) = match config::read(&self.paths.admin()) {
            Ok(c) => (c, None),
            Err(e) => (None, Some(e)),
        };
        let last_update = std::fs::read(self.paths.update_state()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        ExecStatus {
            version: crate::PRODUCT_VERSION.to_string(),
            protocol: k2_node_proto::PROTOCOL,
            remote_admin: cfg.as_ref().map(|c| c.level.as_str()).unwrap_or("off").to_string(),
            controller_fp: cfg.as_ref().and_then(|c| c.controller.as_ref().map(|x| x.fp.clone())),
            approval_key_fps: cfg.as_ref().map(|c| c.approval_keys.iter().map(|k| k.fingerprint()).collect()).unwrap_or_default(),
            auto_update: cfg.as_ref().map(|c| if c.auto_update { "follow" } else { "off" }).unwrap_or("follow").to_string(),
            node_fp: std::fs::read_to_string(self.paths.node_fp_pin()).ok().map(|s| s.trim().to_string()),
            configured: cfg.as_ref().is_some_and(|c| c.controller.is_some()),
            config_error: err,
            last_update,
        }
    }

    pub fn handle(&self, req: &LocalRequest, payload: Option<&[u8]>) -> LocalResponse {
        match req {
            LocalRequest::Status => LocalResponse::Status(self.status()),
            LocalRequest::Command { cmd } => LocalResponse::Result(self.handle_command(cmd)),
            LocalRequest::Keys { keys } => LocalResponse::Result(self.handle_keys(keys)),
            LocalRequest::Update { cmd, sig, .. } => LocalResponse::Result(self.handle_update(cmd, sig, payload.unwrap_or(&[]))),
        }
    }
}

// ── arg helpers (strict: unknown keys refuse) ────────────────────────

pub fn only_keys(args: &Value, allowed: &[&str]) -> Result<(), AdminRefusal> {
    let m = args.as_object().ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "args must be an object"))?;
    if let Some(k) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(AdminRefusal::new(code::BAD_ARGS, format!("unknown arg {k:?}")));
    }
    Ok(())
}

pub fn no_args(args: &Value) -> Result<(), AdminRefusal> {
    only_keys(args, &[])
}

pub fn no_args_except(args: &Value, allowed: &[&str]) -> Result<(), AdminRefusal> {
    only_keys(args, allowed)
}

pub fn str_arg(args: &Value, k: &str) -> Result<String, AdminRefusal> {
    args.get(k)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, format!("{k} (string) is required")))
}

/// Create the state dir root-only (0700) if missing.
pub fn ensure_state_dir(p: &Path) -> Result<(), String> {
    crate::util::mkdir_mode(p, 0o700)
}
