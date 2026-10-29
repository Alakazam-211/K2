//! Node admin wire (`prd-k2-node-admin-v1` §7). **The single source of
//! truth** for the signed admin command, its checks and refusal codes;
//! the controller (k2-daemon) and the node's root executor
//! (`k2-node admin serve`) both use this module and nothing else.
//! Human-readable copy: `.k2/prds/node-admin-wire-v1.md`.
//!
//! ```text
//! SignedCommand (JSON, ≤ 1 MiB)
//!   { "body_b64": base64(canonical JSON of Command),
//!     "sig_c":    base64(P-256 DER sig, controller tunnel key,
//!                        over signed_bytes(DOMAIN_CMD_CONTROLLER, [body])),
//!     "sig_o":    base64(Ed25519 sig, owner approval key,          (privileged only)
//!                        over signed_bytes(DOMAIN_CMD_OWNER, [body])
//!                        or, for a plan step, signed_bytes(DOMAIN_PLAN_OWNER, [plan])),
//!     "plan_b64": base64(canonical JSON of Plan) }                   (plan steps only)
//! ```
//!
//! **Verify before parse.** [`open`] parses only the outer envelope (whose
//! body is an opaque string), checks `sig_c` over the raw body bytes, and
//! only then parses the body. Every refusal is a stable [`code`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto;

/// Version of the command body (`v`).
pub const ADMIN_V: u32 = 1;

pub const DOMAIN_CMD_CONTROLLER: &str = "k2-node-admin-v1/controller";
pub const DOMAIN_CMD_OWNER: &str = "k2-node-admin-v1/owner";
pub const DOMAIN_PLAN_OWNER: &str = "k2-node-admin-v1/owner-plan";
/// Approval key rotation (`admin_keys` frame).
pub const DOMAIN_KEYS_CONTROLLER: &str = "k2-node-admin-v1/keys-controller";
pub const DOMAIN_KEYS_OWNER: &str = "k2-node-admin-v1/keys-owner";

/// Largest signed command (envelope JSON) the executor reads.
pub const MAX_COMMAND_BYTES: usize = 1 << 20;
/// `exp - iat` cap (Q15: 1 h).
pub const MAX_LIFETIME_SECS: i64 = 3600;
/// Clock skew allowance either way (§7.3).
pub const CLOCK_SKEW_SECS: i64 = 300;
/// Most steps in one plan.
pub const MAX_PLAN_STEPS: usize = 32;

/// Offer `features` a protocol-2 node advertises (§7.1). The controller
/// must not send admin frames to a node without `admin` (an older node
/// closes the session on an unknown frame).
pub const FEATURE_ADMIN: &str = "admin";
pub const FEATURE_HEALTH: &str = "health";
/// The executor takes `wall.enable {restart}` at `policy` and restarts
/// k2-node only through a settling drain that never kills a job (0.46.4).
/// The controller branches on this, never on a version string.
pub const FEATURE_WALL_RESTART: &str = "wall-restart";
pub const FEATURE_SELF_UPDATE: &str = "self-update";

/// Stable refusal codes. Teaching JSON on the controller keys off these.
pub mod code {
    pub const TOO_LARGE: &str = "too_large";
    pub const MALFORMED: &str = "malformed";
    pub const BAD_CONTROLLER_SIGNATURE: &str = "bad_controller_signature";
    pub const WRONG_NODE: &str = "wrong_node";
    pub const WRONG_CONTROLLER: &str = "wrong_controller";
    pub const EXPIRED: &str = "approval_expired";
    pub const NOT_YET_VALID: &str = "not_yet_valid";
    pub const LIFETIME_TOO_LONG: &str = "lifetime_too_long";
    pub const REPLAYED: &str = "replayed";
    pub const APPROVAL_REQUIRED: &str = "approval_required";
    pub const BAD_APPROVAL_SIGNATURE: &str = "bad_approval_signature";
    pub const PLAN_MISMATCH: &str = "plan_mismatch";
    pub const REMOTE_ADMIN_OFF: &str = "remote_admin_off";
    pub const VERB_NOT_ALLOWED: &str = "verb_not_allowed";
    pub const BAD_ARGS: &str = "bad_args";
    /// A key that is still read but no longer set: `gate_reserved_slots`
    /// since 0.46.5 (prd-compute-job-tiers-v1 §3.7). A usage error (exit
    /// 2), like `bad_args`.
    pub const DEPRECATED_KEY: &str = "deprecated_key";
    pub const PEER_NOT_ALLOWED: &str = "peer_not_allowed";
    pub const EXECUTOR_UNREACHABLE: &str = "executor_unreachable";
    pub const NOT_CONFIGURED: &str = "not_configured";
    pub const HUMAN_REQUIRED: &str = "human_required";
    pub const FAILED: &str = "failed";
    // self-update
    pub const BAD_BUILD: &str = "bad_build";
    pub const DOWNGRADE: &str = "downgrade";
    pub const SAME_VERSION: &str = "same_version";
    pub const ROLLED_BACK_VERSION: &str = "rolled_back_version";
    pub const AUTO_UPDATE_OFF: &str = "auto_update_off";
    pub const NO_PREVIOUS: &str = "no_previous";
    pub const BUSY: &str = "busy";
}

/// A refusal: stable code + short sentence (never echoes caller bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminRefusal {
    pub code: String,
    pub message: String,
}

impl AdminRefusal {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

impl std::fmt::Display for AdminRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

fn refuse(code: &str, msg: &str) -> AdminRefusal {
    AdminRefusal::new(code, msg)
}

// ── classes, levels, the verb catalog ────────────────────────────────

/// §7.2. The executor computes the class itself; the controller's label
/// in the body is only checked to be no *lower* than the real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Read,
    Operate,
    Privileged,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Read => "read",
            Class::Operate => "operate",
            Class::Privileged => "privileged",
        }
    }
}

/// The machine owner's remote-admin consent (`admin.toml`, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Off,
    Policy,
    Full,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Policy => "policy",
            Level::Full => "full",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Level::Off),
            "policy" => Some(Level::Policy),
            "full" => Some(Level::Full),
            _ => None,
        }
    }
}

/// Where a verb runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// The unprivileged node answers itself (read verbs, job housekeeping).
    Node,
    /// The root executor.
    Executor,
}

/// One verb of the v1 catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerbSpec {
    pub name: &'static str,
    /// The class when nothing widens. `policy.set` is re-classed by the
    /// executor from the node's own file (narrowing = operate).
    pub base: Class,
    /// Lowest `remote_admin` level that allows it.
    pub min_level: Level,
    pub runs: Runs,
}

/// The v1 catalog (§7.5, §7.6). Anything else is `verb_not_allowed`.
/// S7/S8 verbs (power.*, pkg.*, script, services) join this table when
/// they're built; until then a node refuses them by name.
pub const VERBS: &[VerbSpec] = &[
    // read: the node answers
    VerbSpec { name: "health", base: Class::Read, min_level: Level::Off, runs: Runs::Node },
    VerbSpec { name: "policy.show", base: Class::Read, min_level: Level::Off, runs: Runs::Node },
    VerbSpec { name: "ps", base: Class::Read, min_level: Level::Off, runs: Runs::Node },
    // operate, allowed even at `off` (narrowing / lowering only)
    VerbSpec { name: "policy.narrow", base: Class::Operate, min_level: Level::Off, runs: Runs::Executor },
    VerbSpec { name: "admin.level.lower", base: Class::Operate, min_level: Level::Off, runs: Runs::Executor },
    // `policy` level
    VerbSpec { name: "policy.set", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "control.pause", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "control.drain", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "control.resume", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "node.update", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "node.rollback", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "gc", base: Class::Operate, min_level: Level::Policy, runs: Runs::Node },
    VerbSpec { name: "jobs.kill", base: Class::Operate, min_level: Level::Policy, runs: Runs::Node },
    // `full`, owner-approved
    VerbSpec { name: "unit.restart", base: Class::Privileged, min_level: Level::Full, runs: Runs::Executor },
    // Make people's home folders on the node private (chmod o-rwx): root
    // on the human's files, so always the owner's own approval (0.46.3).
    VerbSpec { name: "homes.private", base: Class::Privileged, min_level: Level::Full, runs: Runs::Executor },
    // Linux: create the `k2job` user and turn the job wall on (what setup
    // does at the machine), for nodes updated remotely (0.46.3). Operate at
    // `policy` since 0.46.4 (Rosson, Q5): it only walls jobs in further (a
    // fixed user, no argument but `restart`), so an operator turns it on
    // without a per-node approval. A 0.46.3 executor still classes it
    // privileged and answers `approval_required`; the controller holds it.
    VerbSpec { name: "wall.enable", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    // Node sharing (protocol 3, prd-compute-shared-queue-v1 §10.2;
    // arguments: `crate::share::check_verb_args`). The node lists its
    // servers itself; the rest write root-owned `servers.toml`.
    VerbSpec { name: "servers.list", base: Class::Read, min_level: Level::Off, runs: Runs::Node },
    // Runs `tenant.prepare`; always the owner's approval. `full` like every
    // privileged verb (the PRD's table said `policy`; the catalog rule wins:
    // at `policy` a node adds servers at the machine only).
    VerbSpec { name: "servers.add", base: Class::Privileged, min_level: Level::Full, runs: Runs::Executor },
    // Operate when it only narrows; the executor re-classes a widening
    // (`share::ShareLimits::widens`) as privileged, like `policy.set`.
    VerbSpec { name: "servers.limits", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "servers.pause", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "servers.resume", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    VerbSpec { name: "servers.revoke", base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
    // Move the owner server to another of the owner's own servers on the
    // node (prd-macos-tenant-compute-v1 §6): root admin changes hands, so
    // always the owner's approval and `full` (at `policy`: at the machine).
    VerbSpec { name: "servers.owner", base: Class::Privileged, min_level: Level::Full, runs: Runs::Executor },
    // 0.46.7 (CE-24, the `compute.package-allow` card): add one system
    // package to the node's approved list (`/etc/k2-node-packages.toml`),
    // so a job's `--needs sys:<name>` installs it. Operate at `policy`:
    // the owner or an Admin taps the card (Rosson 2026-10-29: a click, not
    // Touch ID). Linux only; argument `{package}` checked with
    // `needs::valid_package`. An older executor answers `verb_not_allowed`.
    VerbSpec { name: VERB_PACKAGES_ALLOW, base: Class::Operate, min_level: Level::Policy, runs: Runs::Executor },
];

/// The CE-24 verb.
pub const VERB_PACKAGES_ALLOW: &str = "packages.allow";
/// A `packages.allow` on a Mac: nothing to approve there (the job's owner
/// installs it with brew at the machine).
pub const PACKAGES_LINUX_ONLY: &str = "packages_linux_only";

/// Verbs that restart `k2-node`. On a node with two or more active
/// servers they drain first: no new starts, running jobs finish (up to
/// 30 min), then the swap (prd-compute-shared-queue-v1 §8.2, V55). The
/// optional argument [`ARG_NOW`] (`true`) skips the wait. Solo nodes never
/// drain, as today, except for `servers.owner`, which always drains first
/// (it moves the job folders; `now` stops running jobs instead).
pub const DRAIN_VERBS: &[&str] = &["node.update", "node.rollback", "unit.restart", "servers.owner"];
/// `"now": true` on a [`DRAIN_VERBS`] command: don't wait for the drain.
pub const ARG_NOW: &str = "now";
/// The longest a drain waits before the swap goes ahead anyway.
pub const DRAIN_MAX_SECS: i64 = 30 * 60;

/// The `now` argument of a drain verb: absent = `false` (drain first).
pub fn now_arg(args: &Value) -> Result<bool, AdminRefusal> {
    match args.get(ARG_NOW) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(AdminRefusal::new(code::BAD_ARGS, "now is true or false")),
    }
}

pub fn verb(name: &str) -> Option<&'static VerbSpec> {
    VERBS.iter().find(|v| v.name == name)
}

// ── the command body ─────────────────────────────────────────────────

/// A plan step's reference to its plan (§7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRef {
    pub plan_id: String,
    /// 0-based.
    pub step: u32,
    pub of: u32,
    /// `sha256:<hex>` of the canonical [`Plan`] bytes (`plan_b64`).
    pub digest: String,
}

/// What the owner approves once for several steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub v: u32,
    pub plan_id: String,
    pub node_fp: String,
    pub controller_fp: String,
    pub request_id: String,
    pub steps: Vec<PlanStep>,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    pub verb: String,
    pub args: Value,
}

/// The signed command body (§7.3). Strict: unknown fields refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub v: u32,
    pub cmd_id: String,
    pub node_fp: String,
    pub controller_fp: String,
    pub class: Class,
    pub verb: String,
    /// A JSON object; never floats.
    pub args: Value,
    #[serde(default)]
    pub plan: Option<PlanRef>,
    pub proposed_by: String,
    #[serde(default)]
    pub approved_by: Option<String>,
    /// `local-auth` | `connect-password` | `vault-password`.
    #[serde(default)]
    pub approved_via: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    pub iat: i64,
    pub exp: i64,
    /// base64url (no pad) of 16 random bytes; single use per node.
    pub nonce: String,
}

impl Command {
    /// Canonical bytes (what both signatures cover).
    pub fn canonical(&self) -> Result<Vec<u8>, String> {
        crate::canonical::to_vec(self)
    }

    /// `sha256:<hex>` of the canonical body (the `digest` in results and audit).
    pub fn digest(&self) -> Result<String, String> {
        Ok(format!("sha256:{}", crypto::sha256_hex(&self.canonical()?)))
    }
}

impl Plan {
    pub fn canonical(&self) -> Result<Vec<u8>, String> {
        crate::canonical::to_vec(self)
    }
}

/// A fresh nonce: base64url, no padding, 16 random bytes.
pub fn new_nonce() -> Result<String, String> {
    Ok(b64url(&crypto::random_bytes(16)?))
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn nonce_ok(n: &str) -> bool {
    use base64::Engine;
    n.len() == 22 && base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(n).is_ok_and(|b| b.len() == 16)
}

// ── the envelope ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCommand {
    pub body_b64: String,
    pub sig_c: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig_o: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_b64: Option<String>,
}

/// The bytes the owner's approval key signs for a single command.
pub fn owner_message(body: &[u8]) -> Vec<u8> {
    crypto::signed_bytes(DOMAIN_CMD_OWNER, &[body])
}

/// The bytes the owner's approval key signs for a plan.
pub fn owner_plan_message(plan: &[u8]) -> Vec<u8> {
    crypto::signed_bytes(DOMAIN_PLAN_OWNER, &[plan])
}

/// Controller side: sign a command with the tunnel key. `sig_o` is added
/// by [`SignedCommand::with_owner_sig`] after the owner confirm.
pub fn sign_controller(key: &crypto::SigningKey, cmd: &Command) -> Result<SignedCommand, String> {
    let body = cmd.canonical()?;
    let sig_c = key.sign_domain(DOMAIN_CMD_CONTROLLER, &[&body])?;
    Ok(SignedCommand { body_b64: crypto::b64(&body), sig_c, sig_o: None, plan_b64: None })
}

impl SignedCommand {
    /// Attach the owner's raw 64-byte Ed25519 signature (from the vault's
    /// `engine_sign("compute","node-admin", owner_message(body))`).
    pub fn with_owner_sig(mut self, sig: &[u8; 64]) -> Self {
        self.sig_o = Some(crypto::b64(sig));
        self
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("encode signed command: {e}"))
    }
}

/// An owner approval public key: `ed25519:<base64 of 32 raw bytes>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalKey(pub [u8; 32]);

impl ApprovalKey {
    pub fn parse(s: &str) -> Option<Self> {
        let b = crypto::unb64(s.strip_prefix("ed25519:")?).ok()?;
        Some(Self(b.try_into().ok()?))
    }
    pub fn encode(&self) -> String {
        format!("ed25519:{}", crypto::b64(&self.0))
    }
    /// SHA-256 hex of the raw key (what health reports).
    pub fn fingerprint(&self) -> String {
        crypto::sha256_hex(&self.0)
    }
    pub fn verify(&self, msg: &[u8], sig: &[u8]) -> bool {
        use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
        UnparsedPublicKey::new(&ED25519, &self.0).verify(msg, sig).is_ok()
    }
}

/// A signer for approval keys (tests, and any controller path that
/// holds a seed instead of the vault).
pub struct ApprovalSigner(aws_lc_rs::signature::Ed25519KeyPair);

impl ApprovalSigner {
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self, String> {
        aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(seed)
            .map(Self)
            .map_err(|_| "bad ed25519 seed".to_string())
    }
    pub fn generate() -> Result<Self, String> {
        let seed: [u8; 32] = crypto::random_bytes(32)?.try_into().map_err(|_| "rng".to_string())?;
        Self::from_seed(&seed)
    }
    pub fn public(&self) -> ApprovalKey {
        use aws_lc_rs::signature::KeyPair;
        let mut k = [0u8; 32];
        k.copy_from_slice(self.0.public_key().as_ref());
        ApprovalKey(k)
    }
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let mut out = [0u8; 64];
        out.copy_from_slice(self.0.sign(msg).as_ref());
        out
    }
}

/// A command whose controller signature checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub cmd: Command,
    /// The exact signed body bytes.
    pub body: Vec<u8>,
    pub sig_o: Option<Vec<u8>>,
    pub plan: Option<Vec<u8>>,
}

impl Opened {
    pub fn digest(&self) -> String {
        format!("sha256:{}", crypto::sha256_hex(&self.body))
    }
}

/// §7.3 steps 1–2 (size) and 2 (controller signature), then parse the
/// body. Nothing in `body_b64` is parsed before `sig_c` verifies.
pub fn open(raw: &[u8], controller_spki_der: &[u8]) -> Result<Opened, AdminRefusal> {
    if raw.len() > MAX_COMMAND_BYTES {
        return Err(refuse(code::TOO_LARGE, "signed command over 1 MiB"));
    }
    let env: SignedCommand =
        serde_json::from_slice(raw).map_err(|_| refuse(code::MALFORMED, "signed command envelope does not parse"))?;
    let body = crypto::unb64(&env.body_b64).map_err(|_| refuse(code::MALFORMED, "body is not base64"))?;
    if !crypto::verify_domain(controller_spki_der, DOMAIN_CMD_CONTROLLER, &[&body], &env.sig_c) {
        return Err(refuse(code::BAD_CONTROLLER_SIGNATURE, "controller signature does not verify"));
    }
    let cmd: Command = serde_json::from_slice(&body).map_err(|_| refuse(code::MALFORMED, "command body does not parse"))?;
    // The signed bytes must be exactly the canonical form: no second
    // reading of the same signature (duplicate keys, whitespace, order).
    if cmd.canonical().map_err(|_| refuse(code::MALFORMED, "command body is not canonical"))? != body {
        return Err(refuse(code::MALFORMED, "command body is not canonical JSON"));
    }
    if cmd.v != ADMIN_V {
        return Err(refuse(code::MALFORMED, "unknown command version"));
    }
    if !cmd.args.is_object() || has_float(&cmd.args) {
        return Err(refuse(code::BAD_ARGS, "args must be a JSON object without floats"));
    }
    if !nonce_ok(&cmd.nonce) {
        return Err(refuse(code::MALFORMED, "nonce must be 16 bytes base64url"));
    }
    if cmd.cmd_id.is_empty() || cmd.cmd_id.len() > 64 || !cmd.cmd_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(refuse(code::MALFORMED, "cmd_id must be 1-64 of [A-Za-z0-9_-]"));
    }
    let sig_o = match &env.sig_o {
        None => None,
        Some(s) => Some(crypto::unb64(s).map_err(|_| refuse(code::MALFORMED, "sig_o is not base64"))?),
    };
    let plan = match &env.plan_b64 {
        None => None,
        Some(s) => Some(crypto::unb64(s).map_err(|_| refuse(code::MALFORMED, "plan is not base64"))?),
    };
    Ok(Opened { cmd, body, sig_o, plan })
}

fn has_float(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_f64(),
        Value::Array(a) => a.iter().any(has_float),
        Value::Object(m) => m.values().any(has_float),
        _ => false,
    }
}

/// §7.3 step 3 (minus the nonce store, which is the executor's): the
/// command is for this node and this controller, and alive now.
pub fn check_target_and_time(cmd: &Command, node_fp: &str, controller_fp: &str, now: i64) -> Result<(), AdminRefusal> {
    if !crypto::ct_eq(cmd.node_fp.as_bytes(), node_fp.as_bytes()) {
        return Err(refuse(code::WRONG_NODE, "command is for another node"));
    }
    if !crypto::ct_eq(cmd.controller_fp.as_bytes(), controller_fp.as_bytes()) {
        return Err(refuse(code::WRONG_CONTROLLER, "command names another controller"));
    }
    if cmd.exp <= cmd.iat || cmd.exp - cmd.iat > MAX_LIFETIME_SECS {
        return Err(refuse(code::LIFETIME_TOO_LONG, "command lifetime must be 1 s to 1 h"));
    }
    if cmd.iat > now + CLOCK_SKEW_SECS {
        return Err(refuse(code::NOT_YET_VALID, "command issued in the future (check clocks)"));
    }
    if now > cmd.exp + CLOCK_SKEW_SECS {
        return Err(refuse(code::EXPIRED, "command expired"));
    }
    Ok(())
}

/// §7.3 step 4 for a privileged command: `sig_o` by one of `keys` over
/// the body (or over the plan, whose step must be this exact command).
/// Returns the fingerprint of the key that verified.
pub fn verify_owner(o: &Opened, keys: &[ApprovalKey], now: i64) -> Result<String, AdminRefusal> {
    let sig = o.sig_o.as_ref().ok_or_else(|| refuse(code::APPROVAL_REQUIRED, "owner approval required"))?;
    if o.cmd.request_id.as_deref().unwrap_or("").is_empty() || o.cmd.approved_by.as_deref().unwrap_or("").is_empty() {
        return Err(refuse(code::APPROVAL_REQUIRED, "request_id and approved_by are required"));
    }
    if keys.is_empty() {
        return Err(refuse(code::NOT_CONFIGURED, "no approval key is pinned on this node"));
    }
    let msg = match (&o.cmd.plan, &o.plan) {
        (None, None) => owner_message(&o.body),
        (Some(r), Some(pb)) => {
            check_plan(&o.cmd, r, pb, now)?;
            owner_plan_message(pb)
        }
        _ => return Err(refuse(code::PLAN_MISMATCH, "plan reference and plan body must come together")),
    };
    for k in keys {
        if k.verify(&msg, sig) {
            return Ok(k.fingerprint());
        }
    }
    Err(refuse(code::BAD_APPROVAL_SIGNATURE, "owner approval signature does not verify"))
}

fn check_plan(cmd: &Command, r: &PlanRef, plan_bytes: &[u8], now: i64) -> Result<(), AdminRefusal> {
    let mismatch = |m: &str| refuse(code::PLAN_MISMATCH, m);
    let digest = format!("sha256:{}", crypto::sha256_hex(plan_bytes));
    if !crypto::ct_eq(digest.as_bytes(), r.digest.as_bytes()) {
        return Err(mismatch("plan digest does not match"));
    }
    let p: Plan = serde_json::from_slice(plan_bytes).map_err(|_| mismatch("plan does not parse"))?;
    if p.canonical().map_err(|_| mismatch("plan not canonical"))? != plan_bytes || p.v != ADMIN_V {
        return Err(mismatch("plan is not canonical v1"));
    }
    if p.plan_id != r.plan_id
        || p.node_fp != cmd.node_fp
        || p.controller_fp != cmd.controller_fp
        || Some(&p.request_id) != cmd.request_id.as_ref()
        || p.steps.is_empty()
        || p.steps.len() > MAX_PLAN_STEPS
        || r.of as usize != p.steps.len()
        || r.step >= r.of
    {
        return Err(mismatch("plan header does not match the command"));
    }
    if p.exp <= p.iat || p.exp - p.iat > MAX_LIFETIME_SECS || now > p.exp + CLOCK_SKEW_SECS {
        return Err(refuse(code::EXPIRED, "plan expired"));
    }
    let step = &p.steps[r.step as usize];
    if step.verb != cmd.verb || step.args != cmd.args {
        return Err(mismatch("step differs from the approved plan"));
    }
    Ok(())
}

// ── approval key rotation (§7.4) ─────────────────────────────────────

/// `admin_keys` frame body: the new pinned list, signed by the controller
/// key and (to rotate) an already-pinned approval key. First pin on a node
/// that has none needs only the controller signature (trust on first
/// use, §7.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminKeys {
    /// `ed25519:<b64>` keys, 1..=4.
    pub keys: Vec<String>,
    pub node_fp: String,
    pub iat: i64,
    pub nonce: String,
    /// P-256 over signed_bytes(DOMAIN_KEYS_CONTROLLER, [canonical(keys,node_fp,iat,nonce)]).
    pub sig_c: String,
    /// Ed25519 by a currently pinned key over signed_bytes(DOMAIN_KEYS_OWNER, [same bytes]).
    #[serde(default)]
    pub sig_o: Option<String>,
}

impl AdminKeys {
    pub fn signed_part(&self) -> Result<Vec<u8>, String> {
        crate::canonical::to_vec(&serde_json::json!({
            "keys": self.keys, "node_fp": self.node_fp, "iat": self.iat, "nonce": self.nonce,
        }))
    }
}

// ── results and update states (n→c) ──────────────────────────────────

/// `admin_result` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminResult {
    pub cmd_id: String,
    /// `done` | `refused` | `failed` | `accepted` (long-running: update).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default)]
    pub stdout_tail: String,
    #[serde(default)]
    pub stderr_tail: String,
    /// The node-side audit line number, when the executor wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_seq: Option<u64>,
    /// `sha256:<hex>` of the signed body (empty when it never parsed).
    pub digest: String,
    /// Verb-specific JSON (policy, ps rows, gc counts, …).
    #[serde(default)]
    pub result: Value,
}

impl AdminResult {
    pub fn refused(cmd_id: &str, digest: &str, r: &AdminRefusal) -> Self {
        Self {
            cmd_id: cmd_id.to_string(),
            state: "refused".into(),
            code: Some(r.code.clone()),
            message: Some(r.message.clone()),
            exit: None,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            audit_seq: None,
            digest: digest.to_string(),
            result: Value::Null,
        }
    }
    pub fn done(cmd_id: &str, digest: &str, result: Value) -> Self {
        Self {
            cmd_id: cmd_id.to_string(),
            state: "done".into(),
            code: None,
            message: None,
            exit: Some(0),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            audit_seq: None,
            digest: digest.to_string(),
            result,
        }
    }
}

/// Self-update states (`update_state`, §7.1, §8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Staged,
    Swapped,
    Healthy,
    RolledBack,
    Refused,
    Failed,
}

/// One step of a self-update as the executor ran it (received, verify,
/// stage, preflight, decide, mark, swap, restart, health wait, healthy or
/// rollback, re-exec). Kept in the executor's `update.json` and reported
/// in [`crate::frames::UpdateState::steps`] (0.46.4; optional on the wire).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStep {
    pub step: String,
    pub ok: bool,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::SigningKey;

    fn cmd(node: &str, ctl: &str, verb: &str, args: Value, now: i64) -> Command {
        Command {
            v: ADMIN_V,
            cmd_id: "c1".into(),
            node_fp: node.into(),
            controller_fp: ctl.into(),
            class: Class::Operate,
            verb: verb.into(),
            args,
            plan: None,
            proposed_by: "agent:it".into(),
            approved_by: None,
            approved_via: None,
            request_id: None,
            iat: now,
            exp: now + 600,
            nonce: new_nonce().unwrap(),
        }
    }

    #[test]
    fn signed_command_round_trips_and_tampers_fail() {
        let k = SigningKey::generate().unwrap();
        let c = cmd("nfp", &k.fingerprint(), "policy.set", serde_json::json!({"mem_cap_gb": 6}), 1000);
        let s = sign_controller(&k, &c).unwrap();
        let raw = s.to_json().unwrap();
        let o = open(raw.as_bytes(), k.spki_der()).unwrap();
        assert_eq!(o.cmd, c);
        assert_eq!(o.digest(), c.digest().unwrap());
        // Wrong key.
        let other = SigningKey::generate().unwrap();
        assert_eq!(open(raw.as_bytes(), other.spki_der()).unwrap_err().code, code::BAD_CONTROLLER_SIGNATURE);
        // A changed body (re-encoded) fails the signature.
        let mut c2 = c.clone();
        c2.args = serde_json::json!({"mem_cap_gb": 60});
        let forged = SignedCommand { body_b64: crypto::b64(&c2.canonical().unwrap()), ..s.clone() };
        assert_eq!(open(forged.to_json().unwrap().as_bytes(), k.spki_der()).unwrap_err().code, code::BAD_CONTROLLER_SIGNATURE);
        // Oversize and garbage.
        assert_eq!(open(&vec![b'{'; MAX_COMMAND_BYTES + 1], k.spki_der()).unwrap_err().code, code::TOO_LARGE);
        assert_eq!(open(b"not json", k.spki_der()).unwrap_err().code, code::MALFORMED);
        assert_eq!(open(br#"{"body_b64":"x","sig_c":"y","extra":1}"#, k.spki_der()).unwrap_err().code, code::MALFORMED);
    }

    /// A body that would crash/abuse the parser is never parsed unless it
    /// carries a valid controller signature.
    #[test]
    fn verify_happens_before_parse() {
        let k = SigningKey::generate().unwrap();
        let garbage = b"{\"v\":1,\"args\":[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[";
        let env = SignedCommand { body_b64: crypto::b64(garbage), sig_c: "AAAA".into(), sig_o: None, plan_b64: None };
        assert_eq!(open(env.to_json().unwrap().as_bytes(), k.spki_der()).unwrap_err().code, code::BAD_CONTROLLER_SIGNATURE);
        // Signed but not canonical: refused as malformed.
        let body = br#"{ "v":1 }"#;
        let sig_c = k.sign_domain(DOMAIN_CMD_CONTROLLER, &[body]).unwrap();
        let env = SignedCommand { body_b64: crypto::b64(body), sig_c, sig_o: None, plan_b64: None };
        assert_eq!(open(env.to_json().unwrap().as_bytes(), k.spki_der()).unwrap_err().code, code::MALFORMED);
    }

    #[test]
    fn floats_and_bad_nonces_refuse() {
        let k = SigningKey::generate().unwrap();
        let mut c = cmd("n", &k.fingerprint(), "policy.set", serde_json::json!({"x": 1.5}), 1);
        let s = sign_controller(&k, &c).unwrap();
        assert_eq!(open(s.to_json().unwrap().as_bytes(), k.spki_der()).unwrap_err().code, code::BAD_ARGS);
        c.args = serde_json::json!({});
        c.nonce = "short".into();
        let s = sign_controller(&k, &c).unwrap();
        assert_eq!(open(s.to_json().unwrap().as_bytes(), k.spki_der()).unwrap_err().code, code::MALFORMED);
    }

    #[test]
    fn target_and_time_checks() {
        let c = cmd("node", "ctl", "health", serde_json::json!({}), 10_000);
        assert!(check_target_and_time(&c, "node", "ctl", 10_000).is_ok());
        assert_eq!(check_target_and_time(&c, "other", "ctl", 10_000).unwrap_err().code, code::WRONG_NODE);
        assert_eq!(check_target_and_time(&c, "node", "x", 10_000).unwrap_err().code, code::WRONG_CONTROLLER);
        assert_eq!(check_target_and_time(&c, "node", "ctl", 10_600 + CLOCK_SKEW_SECS + 1).unwrap_err().code, code::EXPIRED);
        assert!(check_target_and_time(&c, "node", "ctl", 10_600 + CLOCK_SKEW_SECS).is_ok(), "skew allowance");
        assert_eq!(check_target_and_time(&c, "node", "ctl", 10_000 - CLOCK_SKEW_SECS - 1).unwrap_err().code, code::NOT_YET_VALID);
        let mut long = c.clone();
        long.exp = long.iat + MAX_LIFETIME_SECS + 1;
        assert_eq!(check_target_and_time(&long, "node", "ctl", 10_000).unwrap_err().code, code::LIFETIME_TOO_LONG);
    }

    #[test]
    fn owner_signature_single_and_plan() {
        let k = SigningKey::generate().unwrap();
        let owner = ApprovalSigner::generate().unwrap();
        let stranger = ApprovalSigner::generate().unwrap();
        let mut c = cmd("n", &k.fingerprint(), "unit.restart", serde_json::json!({"label": "k2-node"}), 100);
        c.class = Class::Privileged;
        c.request_id = Some("car_1".into());
        c.approved_by = Some("owner:rosson".into());
        let s = sign_controller(&k, &c).unwrap();
        let body = crypto::unb64(&s.body_b64).unwrap();
        // No sig_o.
        let o = open(s.to_json().unwrap().as_bytes(), k.spki_der()).unwrap();
        assert_eq!(verify_owner(&o, &[owner.public()], 100).unwrap_err().code, code::APPROVAL_REQUIRED);
        // Right key.
        let good = s.clone().with_owner_sig(&owner.sign(&owner_message(&body)));
        let o = open(good.to_json().unwrap().as_bytes(), k.spki_der()).unwrap();
        assert_eq!(verify_owner(&o, &[stranger.public(), owner.public()], 100).unwrap(), owner.public().fingerprint());
        // Wrong key, wrong domain, no pinned keys.
        assert_eq!(verify_owner(&o, &[stranger.public()], 100).unwrap_err().code, code::BAD_APPROVAL_SIGNATURE);
        assert_eq!(verify_owner(&o, &[], 100).unwrap_err().code, code::NOT_CONFIGURED);
        let wrong_dom = s.clone().with_owner_sig(&owner.sign(&owner_plan_message(&body)));
        let o2 = open(wrong_dom.to_json().unwrap().as_bytes(), k.spki_der()).unwrap();
        assert_eq!(verify_owner(&o2, &[owner.public()], 100).unwrap_err().code, code::BAD_APPROVAL_SIGNATURE);
        // Missing request id.
        let mut c3 = c.clone();
        c3.request_id = None;
        let s3 = sign_controller(&k, &c3).unwrap();
        let b3 = crypto::unb64(&s3.body_b64).unwrap();
        let s3 = s3.with_owner_sig(&owner.sign(&owner_message(&b3)));
        let o3 = open(s3.to_json().unwrap().as_bytes(), k.spki_der()).unwrap();
        assert_eq!(verify_owner(&o3, &[owner.public()], 100).unwrap_err().code, code::APPROVAL_REQUIRED);

        // A plan of two steps, approved once.
        let plan = Plan {
            v: ADMIN_V,
            plan_id: "p1".into(),
            node_fp: "n".into(),
            controller_fp: k.fingerprint(),
            request_id: "car_1".into(),
            steps: vec![
                PlanStep { verb: "unit.restart".into(), args: serde_json::json!({"label": "k2-node"}) },
                PlanStep { verb: "policy.set".into(), args: serde_json::json!({"mem_cap_gb": 10}) },
            ],
            iat: 100,
            exp: 700,
        };
        let pb = plan.canonical().unwrap();
        let osig = owner.sign(&owner_plan_message(&pb));
        let step = |i: u32, verb: &str, args: Value| {
            let mut s = c.clone();
            s.verb = verb.into();
            s.args = args;
            s.plan = Some(PlanRef { plan_id: "p1".into(), step: i, of: 2, digest: format!("sha256:{}", crypto::sha256_hex(&pb)) });
            let mut sc = sign_controller(&k, &s).unwrap().with_owner_sig(&osig);
            sc.plan_b64 = Some(crypto::b64(&pb));
            open(sc.to_json().unwrap().as_bytes(), k.spki_der()).unwrap()
        };
        assert!(verify_owner(&step(1, "policy.set", serde_json::json!({"mem_cap_gb": 10})), &[owner.public()], 100).is_ok());
        assert_eq!(
            verify_owner(&step(1, "policy.set", serde_json::json!({"mem_cap_gb": 99})), &[owner.public()], 100).unwrap_err().code,
            code::PLAN_MISMATCH,
            "a step that differs from the approved plan"
        );
        assert_eq!(verify_owner(&step(0, "policy.set", serde_json::json!({"mem_cap_gb": 10})), &[owner.public()], 100).unwrap_err().code, code::PLAN_MISMATCH);
        assert_eq!(verify_owner(&step(0, "unit.restart", serde_json::json!({"label": "k2-node"})), &[owner.public()], 800 + CLOCK_SKEW_SECS).unwrap_err().code, code::EXPIRED);
    }

    #[test]
    fn approval_keys_parse_strictly() {
        let s = ApprovalSigner::from_seed(&[7u8; 32]).unwrap();
        let enc = s.public().encode();
        assert_eq!(ApprovalKey::parse(&enc), Some(s.public()));
        assert!(ApprovalKey::parse("ed25519:AAAA").is_none());
        assert!(ApprovalKey::parse(&enc.replace("ed25519:", "p256:")).is_none());
        assert_eq!(s.public().fingerprint().len(), 64);
    }

    /// Pinned vector: the exact bytes the owner signs for a fixed body.
    #[test]
    fn owner_message_layout_is_pinned() {
        let m = owner_message(b"{}");
        let mut want = DOMAIN_CMD_OWNER.as_bytes().to_vec();
        want.push(0);
        want.extend_from_slice(&[0, 0, 0, 2, b'{', b'}']);
        assert_eq!(m, want);
    }

    /// Fuzz smoke (no cargo-fuzz in the workspace): random bytes and
    /// single-byte mutations of a valid envelope never panic and never
    /// open with a broken signature.
    #[test]
    fn open_survives_random_and_mutated_input() {
        let k = SigningKey::generate().unwrap();
        let c = cmd("n", &k.fingerprint(), "policy.set", serde_json::json!({"mem_cap_gb": 6, "x": [1, {"y": "z"}]}), 1);
        let good = sign_controller(&k, &c).unwrap().to_json().unwrap().into_bytes();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for i in 0..20_000 {
            let input: Vec<u8> = if i % 2 == 0 {
                let n = (rnd() % 600) as usize;
                (0..n).map(|_| rnd() as u8).collect()
            } else {
                let mut m = good.clone();
                let at = (rnd() as usize) % m.len();
                m[at] = rnd() as u8;
                m
            };
            if let Ok(o) = open(&input, k.spki_der()) {
                assert_eq!(o.body, c.canonical().unwrap(), "only the signed body ever opens");
            }
        }
    }

    #[test]
    fn drain_verbs_take_an_optional_now() {
        for v in DRAIN_VERBS {
            assert!(verb(v).is_some(), "{v} is in the catalog");
        }
        assert!(!now_arg(&serde_json::json!({})).unwrap(), "absent = drain first");
        assert!(now_arg(&serde_json::json!({"now": true})).unwrap());
        assert!(!now_arg(&serde_json::json!({"now": false})).unwrap());
        assert_eq!(now_arg(&serde_json::json!({"now": "yes"})).unwrap_err().code, code::BAD_ARGS);
    }

    #[test]
    fn catalog_is_consistent() {
        let mut names: Vec<&str> = VERBS.iter().map(|v| v.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), VERBS.len(), "duplicate verb");
        for v in VERBS {
            if v.base == Class::Read {
                assert_eq!(v.runs, Runs::Node, "{}: read verbs never reach root", v.name);
            }
            if v.base == Class::Privileged {
                assert_eq!(v.min_level, Level::Full, "{}", v.name);
            }
        }
        assert!(verb("script").is_none(), "S7 verbs are refused by name until built");
    }
}
