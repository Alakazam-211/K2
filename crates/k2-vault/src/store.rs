//! The `SecretStore` trait (§5.2) and its request/response types.
//!
//! `LocalVault` is the built-in implementation. Outside vaults (1Password,
//! OpenBao, AWS) arrive later as `Provider`s resolved *through* a local
//! row, so policy, grants and audit stay in one place.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::hosts::Origin;
use crate::inject::Inject;
use crate::model::{Consumer, Delivery, Kind, Namespace, Scope, SecretStatus};
use crate::policy::{Actor, HumanProof};
use crate::secret::SecretBytes;

/// Everything about a secret except its value.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretMeta {
    pub id: String,
    pub namespace: Namespace,
    pub scope: Scope,
    pub name: String,
    pub kind: Kind,
    pub provider: String,
    pub hosts: Vec<String>,
    pub inject: Option<Inject>,
    pub default_delivery: Delivery,
    pub delivery_ceiling: Delivery,
    pub origin: String,
    pub created_by: String,
    pub human_given: bool,
    pub latest_version: u32,
    pub status: SecretStatus,
    pub rotate_every_days: Option<u32>,
    pub description: String,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl SecretMeta {
    /// Can the consumers of this secret read its value by default?
    pub fn agent_can_see(&self) -> bool {
        self.default_delivery.readable()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Id(String),
    Name { namespace: Namespace, scope: Scope, name: String },
}

impl Selector {
    pub fn user(scope: Scope, name: impl Into<String>) -> Selector {
        Selector::Name { namespace: Namespace::User, scope, name: name.into() }
    }
    pub fn system(scope: Scope, name: impl Into<String>) -> Selector {
        Selector::Name { namespace: Namespace::System, scope, name: name.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutMode {
    /// Fail if the name exists.
    Create,
    /// Fail if it doesn't.
    Rotate,
    /// Create or add a version.
    Upsert,
}

#[derive(Debug, Clone)]
pub struct PutRequest {
    pub namespace: Namespace,
    pub scope: Scope,
    pub name: String,
    pub kind: Kind,
    pub hosts: Vec<String>,
    /// `None`: the preset for a recognised key format, else `Bearer {}`
    /// for key kinds.
    pub inject: Option<Inject>,
    pub default_delivery: Option<Delivery>,
    pub delivery_ceiling: Option<Delivery>,
    pub origin: String,
    pub description: String,
    pub rotate_every_days: Option<u32>,
    /// RFC 3339. Required for session scope.
    pub expires_at: Option<String>,
    pub mode: PutMode,
    /// VS9: when the latest version already holds this exact value, add no
    /// version (safe retries during adoption).
    pub idempotent: bool,
    /// Treat the value as human-given even if the actor is a subsystem
    /// (adoption of things people typed before 0.45.2).
    pub human_given_override: Option<bool>,
    /// A copy another vault already accepted (carry, clone, handoff): the
    /// ssh_key shape check (item 3) is skipped, so a value that vault took
    /// still lands; callers warn when it doesn't read as a key. Agent
    /// delivery still needs a key the agent serves.
    pub landed_copy: bool,
}

impl PutRequest {
    pub fn new(namespace: Namespace, scope: Scope, name: impl Into<String>, kind: Kind) -> PutRequest {
        PutRequest {
            namespace,
            scope,
            name: name.into(),
            kind,
            hosts: vec![],
            inject: None,
            default_delivery: None,
            delivery_ceiling: None,
            origin: String::new(),
            description: String::new(),
            rotate_every_days: None,
            expires_at: None,
            mode: PutMode::Upsert,
            idempotent: false,
            human_given_override: None,
            landed_copy: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PutOutcome {
    pub meta: SecretMeta,
    pub created: bool,
    pub new_version: bool,
    /// Leading/trailing whitespace was trimmed from a pasted key (VS5).
    pub trimmed: bool,
    /// An agent replaced a person's value: the owner gets a notice with
    /// undo (VS11).
    pub owner_notice: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyPatch {
    /// Replace the host list.
    pub hosts: Option<Vec<String>>,
    #[serde(default)]
    pub add_hosts: Vec<String>,
    #[serde(default)]
    pub remove_hosts: Vec<String>,
    pub default_delivery: Option<Delivery>,
    pub delivery_ceiling: Option<Delivery>,
    pub inject: Option<Inject>,
    pub description: Option<String>,
    /// `Some(0)` clears the reminder.
    pub rotate_every_days: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct GrantRequest {
    pub selector: Selector,
    pub consumer: Consumer,
    pub delivery: Delivery,
    pub env_name: Option<String>,
    pub file_name: Option<String>,
    pub hosts_narrowed: Option<Vec<String>>,
    pub expires_at: Option<String>,
    pub version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub id: String,
    pub secret_id: String,
    pub version_selector: String,
    pub consumer: Consumer,
    pub delivery: Delivery,
    pub env_name: Option<String>,
    pub file_name: Option<String>,
    pub hosts_narrowed: Option<Vec<String>>,
    pub granted_by: String,
    pub granted_at: String,
    pub expires_at: Option<String>,
}

/// Who is asking for a value at use time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requester {
    /// An agent or shell session of a workspace.
    Session { workspace_id: String, session_id: Option<String> },
    /// A published service (V8).
    PublishedService { id: String, workspace_id: String },
    /// An engine adapter (`stalwart`, `postgres`, `sew`, …).
    Engine(String),
    /// A daemon subsystem (`mail`, `db`, `tunnel`, …). System namespace.
    System(String),
    /// A Garden v3 data source (contract-sources-v1 §10.3): resolves only
    /// through an explicit `ConsumerKind::Daemon` grant with consumer id
    /// `source:<id>`, for `fetch`, to the source's exact origin.
    Source { id: String },
    /// One run of a `k2 job` (prd-vault-gaps-v1 item 5). Only grants to
    /// `job:<workspace id>/<job id>` reach it: no implicit workspace
    /// grants (Q13, least privilege for unattended runs).
    Job { workspace_id: String, job_id: String, name: String },
    /// One attempt of a compute job, released to a node
    /// (prd-compute-secret-env-v1 SE10). Only explicit `compute:` grants
    /// reach it: `<ws>/<node_id>`, or `<ws>/*` when `owned_node` (this
    /// server owns the node; never for a node where it is a tenant, Q2).
    /// No implicit workspace grant ever matches.
    Compute { workspace_id: String, node_id: String, job_id: String, generation: u32, owned_node: bool },
}

impl Requester {
    pub fn label(&self) -> String {
        match self {
            Requester::Session { workspace_id, session_id: Some(s) } => format!("session:{workspace_id}/{s}"),
            Requester::Session { workspace_id, session_id: None } => format!("workspace:{workspace_id}"),
            Requester::PublishedService { id, .. } => format!("published_service:{id}"),
            Requester::Engine(e) => format!("engine:{e}"),
            Requester::System(s) => format!("daemon:{s}"),
            Requester::Source { id } => format!("source:{id}"),
            Requester::Job { workspace_id, name, .. } => format!("job:{workspace_id}/{name}"),
            Requester::Compute { workspace_id, node_id, .. } => format!("compute:{workspace_id}/{node_id}"),
        }
    }
}

/// Why a value leaves the store.
#[derive(Debug, Clone, Copy)]
pub enum Use<'a> {
    Fetch(&'a Origin),
    Proxy(&'a Origin),
    File,
    Env,
    Api,
    System,
    /// One ssh-agent signature (item 3); the host-key fingerprint of the
    /// connection's session bind, when the client sent one (audit only).
    Agent(Option<&'a str>),
}

impl Use<'_> {
    pub fn needs(&self) -> Delivery {
        match self {
            Use::Fetch(_) => Delivery::Fetch,
            Use::Proxy(_) => Delivery::Proxy,
            Use::File => Delivery::File,
            Use::Env => Delivery::Env,
            Use::Api => Delivery::Api,
            Use::System => Delivery::None,
            Use::Agent(_) => Delivery::Agent,
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            Use::Fetch(_) => "fetch",
            Use::Proxy(_) => "proxy",
            Use::File | Use::Env => "inject",
            Use::Api => "resolve",
            Use::System => "resolve",
            Use::Agent(_) => "sign",
        }
    }
}

/// A value plus what the consumer may do with it.
pub struct Resolved {
    pub value: SecretBytes,
    pub meta: SecretMeta,
    pub version: u32,
    pub delivery: Delivery,
    pub env_name: Option<String>,
    pub file_name: Option<String>,
    pub inject: Inject,
}

/// One env/file delivery for a spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnItem {
    pub secret_id: String,
    pub name: String,
    pub delivery: Delivery,
    pub env_name: Option<String>,
    pub file_name: Option<String>,
}

/// One proxy delivery for a spawn (§9.3): the session gets a placeholder
/// and a loopback base URL, never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyItem {
    pub secret_id: String,
    pub name: String,
    /// The secret's sites, narrowed by the grant when it narrows them.
    pub hosts: Vec<String>,
    /// The grant's env name (generic providers name their pair after it).
    pub env_name: Option<String>,
}

/// A live proxy placeholder: whose session it belongs to and what it may
/// reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub secret_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    pub seq: u64,
    pub ts: String,
    pub secret_id: Option<String>,
    pub version: Option<i64>,
    pub action: String,
    pub actor: String,
    pub consumer: Option<String>,
    pub target_host: Option<String>,
    pub outcome: String,
    pub count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub secret_id: Option<String>,
    pub since: Option<String>,
    /// Only rows whose secret is in this workspace (agents see their own).
    pub workspace_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub include_system: bool,
    pub scope: Option<Scope>,
    pub include_destroyed: bool,
}

/// What every consumer in the daemon talks to.
pub trait SecretStore: Send + Sync {
    fn status(&self) -> crate::vault::VaultStatus;
    fn put(&self, req: PutRequest, value: SecretBytes, actor: &Actor) -> Result<PutOutcome>;
    fn rotate(&self, sel: &Selector, value: SecretBytes, actor: &Actor) -> Result<PutOutcome>;
    fn meta(&self, sel: &Selector, actor: &Actor) -> Result<SecretMeta>;
    fn list(&self, filter: &ListFilter, actor: &Actor) -> Result<Vec<SecretMeta>>;
    fn set_policy(&self, sel: &Selector, p: PolicyPatch, actor: &Actor) -> Result<SecretMeta>;
    fn grant(&self, g: GrantRequest, actor: &Actor) -> Result<Grant>;
    fn revoke(&self, grant_id: &str, actor: &Actor) -> Result<()>;
    fn disable(&self, sel: &Selector, actor: &Actor) -> Result<()>;
    fn enable(&self, sel: &Selector, actor: &Actor) -> Result<()>;
    fn destroy(&self, sel: &Selector, actor: &Actor) -> Result<()>;
    /// The only way a value leaves the store for use.
    fn resolve(&self, sel: &Selector, use_: Use<'_>, who: &Requester) -> Result<Resolved>;
    /// Print a `user` value to a person (owner confirm).
    fn reveal(&self, sel: &Selector, actor: &Actor, proof: &HumanProof) -> Result<SecretBytes>;
    fn audit(&self, q: &AuditQuery, actor: &Actor) -> Result<Vec<AuditRow>>;
}
