//! The build statement: an in-toto Statement v1 whose predicate is SLSA
//! Provenance v1 with the `…/compute/buildtypes/job/v1` build type. The
//! format is frozen (a new field is a new build type), so the types here
//! refuse unknown fields and unknown values.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::{is_fp, is_git_hex, parse_rfc3339};

/// in-toto Statement v1.
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
/// SLSA Provenance v1.
pub const PREDICATE_TYPE: &str = "https://slsa.dev/provenance/v1";
/// The build type this crate reads.
pub const BUILD_TYPE: &str = "https://k2.dev/compute/buildtypes/job/v1";
/// `builder.id` of a node that held its owner's roster: this, then the
/// roster key's fingerprint.
pub const BUILDER_ID_OWNER: &str = "https://k2.dev/compute/builder/v1/owner/";
/// `builder.id` of a node that held no roster when it signed: this, then
/// its owner server's fingerprint. It names no roster key, so a verifier
/// that pins one refuses it.
pub const BUILDER_ID_OWNER_SERVER: &str = "https://k2.dev/compute/builder/v1/owner-server/";
/// Most subjects in one statement.
pub const MAX_SUBJECTS: usize = 1000;

/// Names used inside the statement.
pub mod names {
    /// `builderDependencies`: the node's self-statement.
    pub const NODE_STATEMENT: &str = "k2-node-statement";
    /// `builderDependencies`: the node key (its digest is the fingerprint).
    pub const NODE_KEY: &str = "k2-node-key";
    /// `byproducts`: the job's log.
    pub const LOG: &str = "log";
    /// `byproducts`: the job's signed receipt.
    pub const RECEIPT: &str = "k2-receipt";
    /// `resolvedDependencies`: an uncommitted patch applied to the source.
    pub const DIRTY: &str = "dirty-patch";
}

/// An in-toto ResourceDescriptor. Every one has a `uri` or a `digest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct ResourceDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digest: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, Value>,
}

impl ResourceDescriptor {
    /// The `sha256` digest, if any.
    pub fn sha256(&self) -> Option<&str> {
        self.digest.get("sha256").map(String::as_str)
    }
}

/// The statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct Statement {
    #[serde(rename = "_type")]
    pub type_: String,
    pub subject: Vec<ResourceDescriptor>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: Provenance,
}

/// SLSA Provenance v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Provenance {
    pub build_definition: BuildDefinition,
    pub run_details: RunDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct BuildDefinition {
    pub build_type: String,
    pub external_parameters: ExternalParameters,
    pub internal_parameters: InternalParameters,
    pub resolved_dependencies: Vec<ResourceDescriptor>,
}

/// What the job asked for, as the node enforced it. `env` holds values;
/// `secretEnv` holds names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct ExternalParameters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResourceDescriptor>,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub secret_env: Vec<String>,
    pub needs: Vec<String>,
    pub packages: Vec<String>,
    pub artifacts: Vec<String>,
    pub dirty: bool,
}

/// How the node ran it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct InternalParameters {
    pub plan_digest: String,
    pub job_id: String,
    pub attempt: u32,
    pub generation: u32,
    pub controller_fp: String,
    pub workspace_id: String,
    pub tier: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    pub network: Network,
    pub limits: Limits,
    pub exclusive: bool,
    pub isolation: Isolation,
    pub wall: Wall,
    pub tenant_slot: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmth: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Limits {
    pub max_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_bytes: Option<u64>,
    pub disk_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct RunDetails {
    pub builder: Builder,
    pub metadata: Metadata,
    pub byproducts: Vec<ResourceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Builder {
    pub id: String,
    pub version: BTreeMap<String, String>,
    pub builder_dependencies: Vec<ResourceDescriptor>,
}

/// Times are the node's own clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Metadata {
    pub invocation_id: String,
    pub started_on: String,
    pub finished_on: String,
}

/// The job's scheduling tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Tier {
    Batch,
    Normal,
    Priority,
    Ci,
}

/// The network the job ran under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub enum Network {
    None,
    NoLan,
    On,
}

/// How the job was kept apart from other jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// A user id held by this job alone.
    JobUid,
    /// Nothing else ran on the machine meanwhile.
    Exclusive,
    /// One user id per tenant, shared with its other jobs.
    TenantUid,
    /// One user id for every tenant.
    SharedUid,
}

impl Isolation {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::JobUid => "job_uid",
            Isolation::Exclusive => "exclusive",
            Isolation::TenantUid => "tenant_uid",
            Isolation::SharedUid => "shared_uid",
        }
    }

    /// From the wire name.
    pub fn parse(s: &str) -> Option<Self> {
        [Isolation::JobUid, Isolation::Exclusive, Isolation::TenantUid, Isolation::SharedUid].into_iter().find(|i| i.as_str() == s)
    }
}

/// The wall jobs ran behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wall {
    /// Linux: a separate user per job.
    JobUser,
    /// macOS sandbox.
    Seatbelt,
    /// No wall.
    None,
}

impl Statement {
    /// The shape rules every reader keeps: the fixed types, 1 to
    /// [`MAX_SUBJECTS`] subjects each with a unique name and exactly one
    /// sha256, every ResourceDescriptor with a `uri` or a `digest`, known
    /// digests well formed, no floats in annotations, `builder.id` in one of
    /// the two owner forms, the node key and the receipt named once each,
    /// and times in `YYYY-MM-DDTHH:MM:SSZ`.
    pub fn check(&self) -> Result<(), String> {
        if self.type_ != STATEMENT_TYPE {
            return Err(format!("_type is not {STATEMENT_TYPE}"));
        }
        if self.predicate_type != PREDICATE_TYPE {
            return Err(format!("predicateType is not {PREDICATE_TYPE}"));
        }
        let bd = &self.predicate.build_definition;
        if bd.build_type != BUILD_TYPE {
            return Err(format!("buildType is {:?}; this verifier reads {BUILD_TYPE}", bd.build_type));
        }
        if self.subject.is_empty() {
            return Err("the statement names no subject".into());
        }
        if self.subject.len() > MAX_SUBJECTS {
            return Err(format!("over {MAX_SUBJECTS} subjects"));
        }
        let mut seen = BTreeSet::new();
        for s in &self.subject {
            let name = s.name.as_deref().filter(|n| !n.is_empty()).ok_or("a subject without a name")?;
            if !seen.insert(name) {
                return Err(format!("subject {name:?} twice"));
            }
            if s.uri.is_some() || !s.annotations.is_empty() || s.digest.len() != 1 || s.sha256().is_none() {
                return Err(format!("subject {name:?} must carry exactly a name and a sha256"));
            }
            check_rd("subject", s)?;
        }
        if let Some(src) = &bd.external_parameters.source {
            check_rd("externalParameters.source", src)?;
        }
        for d in &bd.resolved_dependencies {
            check_rd("resolvedDependencies", d)?;
        }
        let rd = &self.predicate.run_details;
        for d in &rd.builder.builder_dependencies {
            check_rd("builderDependencies", d)?;
        }
        for d in &rd.byproducts {
            check_rd("byproducts", d)?;
        }
        if self.builder_owner().is_none() {
            return Err("builder.id is not an owner's (…/owner/<64 hex> or …/owner-server/<64 hex>)".into());
        }
        let count = |list: &[ResourceDescriptor], name: &str| list.iter().filter(|d| d.name.as_deref() == Some(name)).count();
        if count(&rd.builder.builder_dependencies, names::NODE_KEY) != 1 {
            return Err("builderDependencies must name the node key exactly once".into());
        }
        if count(&rd.byproducts, names::RECEIPT) != 1 {
            return Err("byproducts must name the receipt exactly once".into());
        }
        for t in [&rd.metadata.started_on, &rd.metadata.finished_on] {
            parse_rfc3339(t).ok_or_else(|| format!("{t:?} is not a time in the form YYYY-MM-DDTHH:MM:SSZ"))?;
        }
        Ok(())
    }

    /// Whose fleet `builder.id` names (the node's claim; the roster proves it).
    pub fn builder_owner(&self) -> Option<Owner> {
        let id = &self.predicate.run_details.builder.id;
        if let Some(fp) = id.strip_prefix(BUILDER_ID_OWNER).filter(|f| is_fp(f)) {
            return Some(Owner::RosterKey(fp.to_string()));
        }
        id.strip_prefix(BUILDER_ID_OWNER_SERVER).filter(|f| is_fp(f)).map(|fp| Owner::Server(fp.to_string()))
    }

    /// The node key the statement names (its fingerprint).
    pub fn node_key_fp(&self) -> Option<&str> {
        self.builder_dep(names::NODE_KEY).and_then(ResourceDescriptor::sha256)
    }

    /// A `builderDependencies` entry by name.
    pub fn builder_dep(&self, name: &str) -> Option<&ResourceDescriptor> {
        self.predicate.run_details.builder.builder_dependencies.iter().find(|d| d.name.as_deref() == Some(name))
    }

    /// The source commit, if the job had source.
    pub fn commit(&self) -> Option<&str> {
        self.predicate.build_definition.external_parameters.source.as_ref()?.digest.get("gitCommit").map(String::as_str)
    }

    /// The source uri, if the job had source.
    pub fn source_uri(&self) -> Option<&str> {
        self.predicate.build_definition.external_parameters.source.as_ref()?.uri.as_deref()
    }

    /// The node's name, as the node wrote it (the roster's is the one to trust).
    pub fn node_name(&self) -> Option<&str> {
        self.builder_dep(names::NODE_KEY)?.annotations.get("node")?.as_str()
    }

    /// The node key's epoch, as the statement says.
    pub fn key_epoch(&self) -> Option<u64> {
        self.builder_dep(names::NODE_KEY)?.annotations.get("keyEpoch")?.as_u64()
    }

    /// Start time, unix seconds (node clock). [`Statement::check`] proved it parses.
    pub fn started(&self) -> i64 {
        parse_rfc3339(&self.predicate.run_details.metadata.started_on).unwrap_or(i64::MAX)
    }

    /// End time, unix seconds (node clock).
    pub fn finished(&self) -> i64 {
        parse_rfc3339(&self.predicate.run_details.metadata.finished_on).unwrap_or(i64::MAX)
    }
}

/// The owner a statement's `builder.id` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// `…/owner/<roster key fingerprint>`.
    RosterKey(String),
    /// `…/owner-server/<server fingerprint>`: no roster key named.
    Server(String),
}

fn check_rd(what: &str, d: &ResourceDescriptor) -> Result<(), String> {
    if d.uri.is_none() && d.digest.is_empty() {
        return Err(format!("a {what} entry ({:?}) has neither uri nor digest", d.name));
    }
    if d.uri.as_deref().is_some_and(|u| u.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err(format!("a {what} entry has a bad uri"));
    }
    for (alg, v) in &d.digest {
        let ok = match alg.as_str() {
            "sha256" => is_fp(v),
            "gitCommit" | "gitTree" => is_git_hex(v),
            _ => false,
        };
        if !ok {
            return Err(format!("a {what} entry has a bad {alg} digest"));
        }
    }
    for v in d.annotations.values() {
        no_floats(v)?;
    }
    Ok(())
}

fn no_floats(v: &Value) -> Result<(), String> {
    match v {
        Value::Number(n) if !(n.is_i64() || n.is_u64()) => Err("a float in an annotation".into()),
        Value::Array(a) => a.iter().try_for_each(no_floats),
        Value::Object(o) => o.values().try_for_each(no_floats),
        _ => Ok(()),
    }
}
