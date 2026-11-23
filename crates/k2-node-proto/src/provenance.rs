//! Build statements (prd-compute-build-attestations-v1 §4–§5, slice S2):
//! one in-toto Statement v1 per job, with a SLSA Provenance v1 predicate,
//! in a DSSE envelope signed with the node key. Human-readable spec and
//! golden vectors: `docs/provenance-v1.md`,
//! `tests/release/fixtures/provenance-v1.json`.
//!
//! **Who writes it.** The node agent, after the job's processes are gone
//! and its artifacts hashed, in the same step that signs the receipt. The
//! job never sees the key or the statement.
//!
//! **Made from the receipt.** [`Statement::for_receipt`] reads the signed
//! receipt's own body and takes every fact the receipt has from there
//! (artifacts, commit and tree, ids, times, secret names, tools, log,
//! isolation): the statement can't say something its receipt doesn't. The
//! plan adds what the receipt only hashes (argv, env, needs, globs), and is
//! refused unless its digest is the receipt's `plan_digest` and its argv
//! hashes to the receipt's `argv_sha256`. [`check_against_receipt`] is the
//! same rule from the verifier's side.
//!
//! **Signing.** [`crate::crypto::SigningKey::sign_statement`] is the only way
//! to sign one: it takes a typed [`Statement`], fixes the payload type to
//! [`PAYLOAD_TYPE`] and builds the DSSE PAE itself (BA-29). There is no
//! function that signs caller-chosen bytes under a caller-chosen type.
//!
//! **Formats.** Lists that are part of the statement are always present
//! (`[]` when empty); optional scalars are absent when unset, as the SLSA
//! examples are. A verifier parses with unknown fields refused and refuses a
//! payload that is not its own canonical form ([`crate::canonical`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ci::{Isolation, Wall};
use crate::crypto;
use crate::frames::{Plan, Receipt, SignedReceipt};
use crate::share::Network;
use crate::tiers::Tier;

// ── constants ────────────────────────────────────────────────────────

/// Offer feature (protocol 10, BA-7): the node signs a build statement for
/// a job whose plan has `provenance: 1` and that kept at least one
/// artifact, and sends it as `ReceiptFrame.provenance`.
pub const FEATURE_PROVENANCE_V1: &str = "provenance-v1";
/// `Plan.provenance`: the only value this build knows.
pub const PROVENANCE_V1: u32 = 1;

/// in-toto Statement v1.
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
/// SLSA Provenance v1.
pub const PREDICATE_TYPE: &str = "https://slsa.dev/provenance/v1";
/// The DSSE payload type of an in-toto statement. Fixed: nothing else is
/// ever signed as a DSSE envelope by a node key.
pub const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
/// What a K2 compute job is (`buildDefinition.buildType`). A new field is a
/// new version (`…/job/v2`).
pub const BUILD_TYPE: &str = "https://k2.dev/compute/buildtypes/job/v1";
/// `runDetails.builder.id` for a node that holds its owner's roster:
/// this, then the roster key's fingerprint (64 hex).
pub const BUILDER_ID_OWNER: &str = "https://k2.dev/compute/builder/v1/owner/";
/// `runDetails.builder.id` for a node that holds no roster yet: this, then
/// its owner server's tunnel-key fingerprint (64 hex). A verifier that pins
/// a roster key never accepts this form.
pub const BUILDER_ID_OWNER_SERVER: &str = "https://k2.dev/compute/builder/v1/owner-server/";

/// Largest statement payload. A bigger one (a thousand long artifact names
/// beside a large env) is not written: the receipt alone goes, and the
/// job's notes say why. Keeps the receipt frame under
/// [`crate::frames::MAX_FRAME_BYTES`].
pub const MAX_PAYLOAD_BYTES: usize = 384 * 1024;

/// The DSSE pre-authentication encoding's fixed prefix.
pub const PAE_PREFIX: &str = "DSSEv1";

// ── the statement ────────────────────────────────────────────────────

/// An in-toto ResourceDescriptor. Every one carries a `uri` or a `digest`
/// (BA-30); [`Statement::check`] refuses one with neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    fn named(name: impl Into<String>) -> Self {
        Self { name: Some(name.into()), uri: None, digest: BTreeMap::new(), annotations: BTreeMap::new() }
    }
    fn at(uri: impl Into<String>) -> Self {
        Self { name: None, uri: Some(uri.into()), digest: BTreeMap::new(), annotations: BTreeMap::new() }
    }
    fn uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }
    fn digest(mut self, alg: &str, hex: impl Into<String>) -> Self {
        self.digest.insert(alg.to_string(), hex.into());
        self
    }
    fn note(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.annotations.insert(k.to_string(), v.into());
        self
    }
    /// The entry's `sha256` digest (64 hex), if it has one.
    pub fn sha256(&self) -> Option<&str> {
        self.digest.get("sha256").map(String::as_str)
    }
}

/// The statement: one per job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
pub struct Provenance {
    pub build_definition: BuildDefinition,
    pub run_details: RunDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BuildDefinition {
    pub build_type: String,
    pub external_parameters: ExternalParameters,
    pub internal_parameters: InternalParameters,
    pub resolved_dependencies: Vec<ResourceDescriptor>,
}

/// What the job asked for (the plan, which the node enforces). `env` holds
/// values: nothing secret may travel there (BA-39); secrets come through
/// `secretEnv`, names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExternalParameters {
    /// The main repo and the commit the node checked out; absent for a job
    /// with no source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResourceDescriptor>,
    pub argv: Vec<String>,
    /// Relative to the source root (or the job dir); absent = the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub secret_env: Vec<String>,
    pub needs: Vec<String>,
    pub packages: Vec<String>,
    /// The artifact globs.
    pub artifacts: Vec<String>,
    pub dirty: bool,
}

/// How the node ran it (plan ids, receipt v2's facts, the node's network
/// rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
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
    /// `warm`, `cold`, `private` or `gen` (the receipt's `src.warmth`);
    /// absent for a job with no source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmth: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
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
pub struct RunDetails {
    pub builder: Builder,
    pub metadata: Metadata,
    pub byproducts: Vec<ResourceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Builder {
    /// One id per owner (BA-30): [`BUILDER_ID_OWNER`] + the roster key's
    /// fingerprint. The node's own key is in `builderDependencies`.
    pub id: String,
    pub version: BTreeMap<String, String>,
    pub builder_dependencies: Vec<ResourceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Metadata {
    /// `<job>/<attempt>/<generation>`.
    pub invocation_id: String,
    /// RFC 3339 UTC, seconds, by the node's clock.
    pub started_on: String,
    pub finished_on: String,
}

// ── names inside the statement ───────────────────────────────────────

/// `builderDependencies` / `byproducts` / `resolvedDependencies` names.
pub mod names {
    pub const NODE_STATEMENT: &str = "k2-node-statement";
    pub const NODE_KEY: &str = "k2-node-key";
    pub const LOG: &str = "log";
    pub const RECEIPT: &str = "k2-receipt";
    pub const DIRTY: &str = "dirty-patch";
    pub const TOOLCHAIN_PREFIX: &str = "toolchain:";
    pub const CACHE_PREFIX: &str = "cache:";
    pub const JOB_CACHE_PREFIX: &str = "job-cache:";
    pub const WITH_PREFIX: &str = "with:";
}

// ── what the node adds ───────────────────────────────────────────────

/// Whose fleet the node says it belongs to (`builder.id`). The roster is
/// what proves it; this is the node's claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// The roster key's fingerprint (the node holds its owner's roster).
    RosterKey(String),
    /// The owner server's tunnel-key fingerprint (no roster held yet).
    Server(String),
}

impl Owner {
    pub fn builder_id(&self) -> String {
        match self {
            Owner::RosterKey(fp) => format!("{BUILDER_ID_OWNER}{fp}"),
            Owner::Server(fp) => format!("{BUILDER_ID_OWNER_SERVER}{fp}"),
        }
    }
}

/// Facts only the node has, for a statement. For a receipt v2 job, every
/// fact the receipt's `run` carries (isolation, wall, slot, lane, version,
/// commit, epoch, statement) is taken from the receipt instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFacts {
    pub owner: Owner,
    /// The network the job ran under (recorded when it started).
    pub network: Network,
    pub isolation: Isolation,
    pub wall: Wall,
    pub tenant_slot: u32,
    pub lane: Option<String>,
    pub k2node_version: String,
    pub k2node_commit: Option<String>,
    pub protocol: u32,
    pub key_epoch: u32,
    /// [`crate::ci::digest`] of the self-statement in force, if any.
    pub statement_digest: Option<String>,
    /// The node's name in its owner's roster, if it holds one.
    pub node_name: Option<String>,
}

// ── building ─────────────────────────────────────────────────────────

impl Statement {
    /// The statement for one ended job, from its **signed** receipt (the
    /// exact body the node signed), its plan and the node's facts.
    /// `Ok(None)`: the job kept no artifact, so there is no statement
    /// (BA-30). `Err`: the inputs disagree (a plan that isn't the
    /// receipt's, a receipt the node couldn't have made) or a value can't
    /// be written; the node then signs the receipt alone.
    pub fn for_receipt(signed: &SignedReceipt, plan: &Plan, f: &NodeFacts) -> Result<Option<Statement>, String> {
        let r: Receipt = serde_json::from_str(&signed.body).map_err(|e| format!("provenance: the receipt body: {e}"))?;
        if r.artifacts.is_empty() {
            return Ok(None);
        }
        r.check_version()?;
        if plan.digest()? != r.plan_digest {
            return Err("provenance: the plan is not the one the receipt names (plan_digest)".into());
        }
        if crate::frames::argv_sha256(&plan.argv) != r.argv_sha256 {
            return Err("provenance: the plan's argv is not the receipt's (argv_sha256)".into());
        }
        if plan.job_id != r.job_id || plan.workspace_id != r.workspace_id {
            return Err("provenance: the plan's job or workspace is not the receipt's".into());
        }
        if plan.secret_env != r.secret_env {
            return Err("provenance: the plan's secret names are not the receipt's".into());
        }
        if let Some(k) = plan.env.keys().find(|k| !r.env_names.contains(k)) {
            return Err(format!("provenance: the plan's env {k} is not among the receipt's env names"));
        }
        if plan.src.is_some() != r.src.is_some() {
            return Err("provenance: the plan has a source exactly when the receipt names a checkout".into());
        }
        let started = r.started_at.ok_or("provenance: the job kept artifacts but never started")?;

        let subject = r
            .artifacts
            .iter()
            .map(|a| ResourceDescriptor::named(a.name.clone()).digest("sha256", a.sha256.clone()))
            .collect();

        // Isolation and the rest: the receipt's v2 facts when it has them.
        let (isolation, wall, slot, lane, version_text, commit, epoch, st_digest) = match &r.run {
            Some(run) => (
                run.isolation,
                run.wall,
                run.tenant_slot,
                run.lane.clone(),
                run.k2node_version.clone(),
                run.k2node_commit.clone(),
                run.key_epoch,
                Some(run.statement_digest.clone()),
            ),
            None => (
                f.isolation,
                f.wall,
                f.tenant_slot,
                f.lane.clone(),
                f.k2node_version.clone(),
                f.k2node_commit.clone(),
                f.key_epoch,
                f.statement_digest.clone(),
            ),
        };

        let mut deps = Vec::new();
        let mut source = None;
        if let (Some(src), Some(ran)) = (&plan.src, &r.src) {
            let uri = source_uri(src.remote_url.as_deref(), &src.project_key);
            source = Some(ResourceDescriptor::at(uri.clone()).digest("gitCommit", ran.commit.clone()));
            deps.push(ResourceDescriptor::at(format!("{uri}@{}", ran.commit)).digest("gitCommit", ran.commit.clone()).digest("gitTree", ran.tree.clone()));
            for w in &plan.with {
                let wuri = source_uri(w.remote_url.as_deref(), &w.project_key);
                deps.push(ResourceDescriptor::named(format!("{}{}", names::WITH_PREFIX, w.path)).uri(format!("{wuri}@{}", w.commit)).digest("gitCommit", w.commit.clone()));
            }
            if let Some(d) = &ran.dirty_sha256 {
                deps.push(ResourceDescriptor::named(names::DIRTY).digest("sha256", d.clone()));
            }
        }
        for (tool, v) in &r.tools_used {
            deps.push(
                ResourceDescriptor::named(format!("{}{tool}", names::TOOLCHAIN_PREFIX))
                    .uri(format!("pkg:generic/{}@{}", purl_escape(tool), purl_escape(v)))
                    .note("version", v.clone())
                    .note("observedBy", "node-probe"),
            );
        }
        if let Some(Some(c)) = &r.cache {
            let hex = c.gen_digest.strip_prefix("sha256:").ok_or("provenance: cache.gen_digest is not sha256:")?;
            deps.push(
                ResourceDescriptor::named(format!("{}{}", names::CACHE_PREFIX, c.lineage))
                    .digest("sha256", hex)
                    .note("seq", c.seq)
                    .note("producerReceipt", c.producer.receipt_digest.clone()),
            );
        }
        if let Some(name) = &plan.cache {
            deps.push(
                ResourceDescriptor::named(format!("{}{name}", names::JOB_CACHE_PREFIX))
                    .uri(format!("k2-job-cache:{name}"))
                    .note("mutable", true),
            );
        }

        let mut version = BTreeMap::new();
        version.insert("k2-node".to_string(), version_text);
        if let Some(c) = &commit {
            version.insert("k2-node-commit".to_string(), c.clone());
        }
        version.insert("protocol".to_string(), f.protocol.to_string());
        let mut bdeps = Vec::new();
        if let Some(d) = &st_digest {
            let hex = d.strip_prefix("sha256:").ok_or("provenance: statement digest is not sha256:")?;
            bdeps.push(ResourceDescriptor::named(names::NODE_STATEMENT).digest("sha256", hex));
        }
        let mut key = ResourceDescriptor::named(names::NODE_KEY).digest("sha256", r.node_fp.clone()).note("keyEpoch", epoch);
        if let Some(n) = &f.node_name {
            key = key.note("node", n.clone());
        }
        bdeps.push(key);

        let mut byproducts = Vec::new();
        if crate::ci::is_fp(&r.log.sha256) {
            byproducts.push(ResourceDescriptor::named(names::LOG).digest("sha256", r.log.sha256.clone()).note("bytes", r.log.bytes).note("truncated", r.log.truncated));
        }
        byproducts.push(ResourceDescriptor::named(names::RECEIPT).digest("sha256", crypto::sha256_hex(signed.body.as_bytes())).note("v", r.v));

        let st = Statement {
            type_: STATEMENT_TYPE.into(),
            subject,
            predicate_type: PREDICATE_TYPE.into(),
            predicate: Provenance {
                build_definition: BuildDefinition {
                    build_type: BUILD_TYPE.into(),
                    external_parameters: ExternalParameters {
                        source,
                        argv: plan.argv.clone(),
                        cwd: plan.cwd.clone(),
                        env: plan.env.clone(),
                        secret_env: r.secret_env.clone(),
                        needs: plan.needs.clone(),
                        packages: plan.packages.clone(),
                        artifacts: plan.artifacts.clone(),
                        dirty: r.src.as_ref().is_some_and(|s| s.dirty_sha256.is_some()),
                    },
                    internal_parameters: InternalParameters {
                        plan_digest: r.plan_digest.clone(),
                        job_id: r.job_id.clone(),
                        attempt: r.attempt,
                        generation: r.generation,
                        controller_fp: r.controller_fp.clone(),
                        workspace_id: r.workspace_id.clone(),
                        tier: plan.tier_or_class(),
                        lane,
                        network: f.network,
                        limits: Limits {
                            max_secs: plan.limits.max_secs,
                            cpu_millis: plan.limits.cpu_millis,
                            mem_bytes: plan.limits.mem_bytes,
                            disk_bytes: plan.limits.disk_bytes,
                        },
                        exclusive: plan.exclusive,
                        isolation,
                        wall,
                        tenant_slot: slot,
                        warmth: r.src.as_ref().and_then(|s| s.warmth.clone()),
                    },
                    resolved_dependencies: deps,
                },
                run_details: RunDetails {
                    builder: Builder { id: f.owner.builder_id(), version, builder_dependencies: bdeps },
                    metadata: Metadata {
                        invocation_id: format!("{}/{}/{}", r.job_id, r.attempt, r.generation),
                        started_on: rfc3339(started)?,
                        finished_on: rfc3339(r.ended_at)?,
                    },
                    byproducts,
                },
            },
        };
        st.check()?;
        Ok(Some(st))
    }
}

// ── shape rules ──────────────────────────────────────────────────────

impl Statement {
    /// The rules every statement keeps (writer and reader alike): the fixed
    /// types, at least one subject, subjects named with a sha256 only, every
    /// ResourceDescriptor with a `uri` or a `digest` (BA-30), known digest
    /// algorithms well formed, no floats, the builder id per owner, one
    /// node key and one receipt named.
    pub fn check(&self) -> Result<(), String> {
        if self.type_ != STATEMENT_TYPE {
            return Err(format!("provenance: _type is not {STATEMENT_TYPE}"));
        }
        if self.predicate_type != PREDICATE_TYPE {
            return Err(format!("provenance: predicateType is not {PREDICATE_TYPE}"));
        }
        let bd = &self.predicate.build_definition;
        if bd.build_type != BUILD_TYPE {
            return Err(format!("provenance: buildType is not {BUILD_TYPE}"));
        }
        if self.subject.is_empty() {
            return Err("provenance: a statement names at least one subject (no artifacts, no statement)".into());
        }
        if self.subject.len() > crate::results::MAX_FILES {
            return Err(format!("provenance: over {} subjects", crate::results::MAX_FILES));
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.subject {
            let name = s.name.as_deref().filter(|n| !n.is_empty()).ok_or("provenance: a subject without a name")?;
            if !seen.insert(name) {
                return Err(format!("provenance: subject {name:?} twice"));
            }
            if s.uri.is_some() || !s.annotations.is_empty() || s.digest.len() != 1 || s.sha256().is_none() {
                return Err(format!("provenance: subject {name:?} carries exactly a name and a sha256"));
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
        let id_ok = [BUILDER_ID_OWNER, BUILDER_ID_OWNER_SERVER]
            .iter()
            .any(|p| rd.builder.id.strip_prefix(p).is_some_and(crate::ci::is_fp));
        if !id_ok {
            return Err("provenance: builder.id is not an owner's (…/owner/<64 hex> or …/owner-server/<64 hex>)".into());
        }
        let count = |list: &[ResourceDescriptor], name: &str| list.iter().filter(|d| d.name.as_deref() == Some(name)).count();
        if count(&rd.builder.builder_dependencies, names::NODE_KEY) != 1 {
            return Err("provenance: builderDependencies names the node key exactly once".into());
        }
        if count(&rd.byproducts, names::RECEIPT) != 1 {
            return Err("provenance: byproducts names the receipt exactly once".into());
        }
        for t in [&rd.metadata.started_on, &rd.metadata.finished_on] {
            parse_rfc3339(t).ok_or_else(|| format!("provenance: {t:?} is not RFC 3339 UTC seconds"))?;
        }
        Ok(())
    }
}

fn check_rd(what: &str, d: &ResourceDescriptor) -> Result<(), String> {
    if d.uri.is_none() && d.digest.is_empty() {
        return Err(format!("provenance: a {what} entry ({:?}) has neither uri nor digest", d.name));
    }
    if d.uri.as_deref().is_some_and(|u| u.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err(format!("provenance: a {what} entry has a bad uri"));
    }
    for (alg, v) in &d.digest {
        let ok = match alg.as_str() {
            "sha256" => crate::ci::is_fp(v),
            "gitCommit" | "gitTree" => v.len() == 40 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            _ => false,
        };
        if !ok {
            return Err(format!("provenance: a {what} entry has a bad {alg} digest"));
        }
    }
    for v in d.annotations.values() {
        no_floats(v)?;
    }
    Ok(())
}

fn no_floats(v: &Value) -> Result<(), String> {
    match v {
        Value::Number(n) if !(n.is_i64() || n.is_u64()) => Err("provenance: a float in an annotation".into()),
        Value::Array(a) => a.iter().try_for_each(no_floats),
        Value::Object(o) => o.values().try_for_each(no_floats),
        _ => Ok(()),
    }
}

// ── the payload and the DSSE envelope ────────────────────────────────

/// The bytes a node signs for `st`: its canonical JSON, after its shape
/// check. Refused when it holds a `0x00` byte (JSON escapes every control
/// character, so a real statement never does; refusing keeps a PAE from
/// ever parsing as a K2 domain message, BA-29) or is over
/// [`MAX_PAYLOAD_BYTES`].
pub fn payload(st: &Statement) -> Result<Vec<u8>, String> {
    st.check()?;
    let bytes = crate::canonical::to_vec(st)?;
    check_payload_bytes(&bytes)?;
    Ok(bytes)
}

/// The byte rules of a payload: no zero byte, at most
/// [`MAX_PAYLOAD_BYTES`].
fn check_payload_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.contains(&0) {
        return Err("provenance: the payload holds a zero byte".into());
    }
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(format!("provenance: the statement is {} bytes, over {MAX_PAYLOAD_BYTES}", bytes.len()));
    }
    Ok(())
}

/// DSSE's pre-authentication encoding:
/// `"DSSEv1" SP len(type) SP type SP len(payload) SP payload`, lengths in
/// ASCII decimal bytes. What the signature covers.
pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let head = format!("{PAE_PREFIX} {} {payload_type} {} ", payload_type.len(), payload.len());
    let mut out = Vec::with_capacity(head.len() + payload.len());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// A DSSE envelope, exactly `{payloadType, payload, signatures: [{keyid,
/// sig}]}`; `payload` and `sig` standard base64 with padding (BA-30).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dsse {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DsseSignature {
    /// The node key's fingerprint (SHA-256 of its SPKI DER, 64 hex).
    pub keyid: String,
    /// ECDSA P-256 SHA-256, ASN.1 DER, over [`pae`].
    pub sig: String,
}

impl Dsse {
    /// The one signature's key id. Never trust it before [`Dsse::open`].
    pub fn keyid(&self) -> Option<&str> {
        match self.signatures.as_slice() {
            [s] => Some(s.keyid.as_str()),
            _ => None,
        }
    }

    /// Verify with the node key `spki_der`, then parse. Refuses another
    /// payload type, not exactly one signature, a keyid that isn't this
    /// key's fingerprint, a bad signature, unknown fields, a payload that
    /// is not its own canonical form, and a statement that breaks
    /// [`Statement::check`].
    pub fn open(&self, spki_der: &[u8]) -> Result<Statement, String> {
        if self.payload_type != PAYLOAD_TYPE {
            return Err(format!("provenance: payloadType is not {PAYLOAD_TYPE}"));
        }
        let [sig] = self.signatures.as_slice() else {
            return Err("provenance: the envelope carries exactly one signature".into());
        };
        if sig.keyid != crypto::fingerprint_of_spki_der(spki_der) {
            return Err("provenance: the signature's keyid is not this node key's fingerprint".into());
        }
        if self.payload.len() > MAX_PAYLOAD_BYTES.div_ceil(3) * 4 {
            return Err(format!("provenance: the payload is over {MAX_PAYLOAD_BYTES} bytes"));
        }
        let body = crypto::unb64(&self.payload).map_err(|e| format!("provenance: payload {e}"))?;
        if crypto::b64(&body) != self.payload {
            return Err("provenance: payload is not standard padded base64".into());
        }
        if !crypto::verify_dsse(spki_der, PAYLOAD_TYPE, &body, &sig.sig) {
            return Err("provenance: the signature does not verify with that node key".into());
        }
        Self::parse(&body)
    }

    /// Parse without verifying: only to see which key to verify with.
    /// Never trust the result.
    pub fn peek(&self) -> Result<Statement, String> {
        let body = crypto::unb64(&self.payload).map_err(|e| format!("provenance: payload {e}"))?;
        Self::parse(&body)
    }

    fn parse(body: &[u8]) -> Result<Statement, String> {
        check_payload_bytes(body)?;
        let st: Statement = serde_json::from_slice(body).map_err(|e| format!("provenance: {e}"))?;
        if crate::canonical::to_vec(&st)? != body {
            return Err("provenance: the payload is not its canonical form".into());
        }
        st.check()?;
        Ok(st)
    }

    /// The `.intoto.jsonl` line: the envelope as one JSON line, then `\n`.
    pub fn line(&self) -> Result<String, String> {
        Ok(format!("{}\n", serde_json::to_string(self).map_err(|e| format!("provenance: {e}"))?))
    }

    /// The whole check a receipt's reader runs: the envelope verifies with
    /// `node_spki_der`, and the statement agrees with the receipt
    /// ([`check_against_receipt`], which also ties the key to the receipt's
    /// `node_fp`). The receipt's own signature is the caller's check (it
    /// pinned the key).
    pub fn open_for_receipt(&self, node_spki_der: &[u8], receipt: &SignedReceipt) -> Result<Statement, String> {
        let st = self.open(node_spki_der)?;
        // The key that signed is the node key the statement names (and,
        // below, the receipt's `node_fp`).
        let fp = crypto::fingerprint_of_spki_der(node_spki_der);
        let named = st.predicate.run_details.builder.builder_dependencies.iter().find(|d| d.name.as_deref() == Some(names::NODE_KEY));
        if named.and_then(|d| d.sha256()) != Some(fp.as_str()) {
            return Err("provenance: the statement names another node key than the one that signed it".into());
        }
        check_against_receipt(&st, receipt)?;
        Ok(st)
    }
}

// ── agreement with the receipt ───────────────────────────────────────

/// A statement and the receipt it names say the same thing: the same
/// files (names and digests, in order), job, attempt, plan, controller,
/// workspace, node key, commit and tree, dirty patch, warmth, secret
/// names, argv, times, tools, log, receipt digest, cache generation, and
/// (receipt v2) the same isolation, wall, slot, lane, node version, epoch
/// and self-statement; every plain env name is one the receipt lists. The
/// receipt body is read as given (its signature is the caller's check).
pub fn check_against_receipt(st: &Statement, signed: &SignedReceipt) -> Result<(), String> {
    let r: Receipt = serde_json::from_str(&signed.body).map_err(|e| format!("provenance: the receipt body: {e}"))?;
    let bad = |what: &str| Err(format!("provenance: the statement's {what} is not the receipt's"));
    let bd = &st.predicate.build_definition;
    let ep = &bd.external_parameters;
    let ip = &bd.internal_parameters;
    let rd = &st.predicate.run_details;

    let subjects: Vec<(Option<&str>, Option<&str>)> = st.subject.iter().map(|s| (s.name.as_deref(), s.sha256())).collect();
    let artifacts: Vec<(Option<&str>, Option<&str>)> = r.artifacts.iter().map(|a| (Some(a.name.as_str()), Some(a.sha256.as_str()))).collect();
    if subjects != artifacts {
        return bad("subjects (artifacts)");
    }
    if (ip.plan_digest.as_str(), ip.job_id.as_str(), ip.attempt, ip.generation) != (r.plan_digest.as_str(), r.job_id.as_str(), r.attempt, r.generation) {
        return bad("plan, job or attempt");
    }
    if rd.metadata.invocation_id != format!("{}/{}/{}", r.job_id, r.attempt, r.generation) {
        return bad("invocationId");
    }
    if ip.controller_fp != r.controller_fp || ip.workspace_id != r.workspace_id {
        return bad("controller or workspace");
    }
    if ep.secret_env != r.secret_env {
        return bad("secretEnv");
    }
    if crate::frames::argv_sha256(&ep.argv) != r.argv_sha256 {
        return bad("argv");
    }
    if let Some(k) = ep.env.keys().find(|k| !r.env_names.contains(k)) {
        return Err(format!("provenance: env {k} is not among the receipt's env names"));
    }
    match &r.src {
        Some(src) => {
            let commit_ok = ep.source.as_ref().and_then(|s| s.digest.get("gitCommit")) == Some(&src.commit);
            let git = bd.resolved_dependencies.first().filter(|d| d.name.is_none());
            let git_ok = git.is_some_and(|d| d.digest.get("gitCommit") == Some(&src.commit) && d.digest.get("gitTree") == Some(&src.tree));
            if !commit_ok || !git_ok {
                return bad("commit or tree");
            }
            let dirty = bd.resolved_dependencies.iter().find(|d| d.name.as_deref() == Some(names::DIRTY)).and_then(|d| d.sha256());
            if ep.dirty != src.dirty_sha256.is_some() || dirty != src.dirty_sha256.as_deref() {
                return bad("dirty patch");
            }
            if ip.warmth != src.warmth {
                return bad("warmth");
            }
        }
        None => {
            if ep.source.is_some() || ep.dirty || ip.warmth.is_some() || bd.resolved_dependencies.iter().any(|d| d.name.is_none() || d.name.as_deref() == Some(names::DIRTY)) {
                return bad("source (the receipt names no checkout)");
            }
        }
    }
    let started = r.started_at.ok_or("provenance: the receipt says the job never started")?;
    if rd.metadata.started_on != rfc3339(started)? || rd.metadata.finished_on != rfc3339(r.ended_at)? {
        return bad("times");
    }
    let key = rd.builder.builder_dependencies.iter().find(|d| d.name.as_deref() == Some(names::NODE_KEY));
    if key.and_then(|k| k.sha256()) != Some(r.node_fp.as_str()) {
        return bad("node key");
    }
    let receipt = rd.byproducts.iter().find(|d| d.name.as_deref() == Some(names::RECEIPT));
    if receipt.and_then(|d| d.sha256()) != Some(crypto::sha256_hex(signed.body.as_bytes()).as_str())
        || receipt.and_then(|d| d.annotations.get("v")) != Some(&Value::from(r.v))
    {
        return bad("receipt digest");
    }
    let log = rd.byproducts.iter().find(|d| d.name.as_deref() == Some(names::LOG));
    let log_ok = match log {
        Some(l) => {
            l.sha256() == Some(r.log.sha256.as_str())
                && l.annotations.get("bytes") == Some(&Value::from(r.log.bytes))
                && l.annotations.get("truncated") == Some(&Value::from(r.log.truncated))
        }
        None => !crate::ci::is_fp(&r.log.sha256),
    };
    if !log_ok {
        return bad("log");
    }
    let tools: BTreeMap<String, String> = bd
        .resolved_dependencies
        .iter()
        .filter_map(|d| {
            let tool = d.name.as_deref()?.strip_prefix(names::TOOLCHAIN_PREFIX)?;
            Some((tool.to_string(), d.annotations.get("version")?.as_str()?.to_string()))
        })
        .collect();
    if tools != r.tools_used {
        return bad("toolchains (tools_used)");
    }
    let caches: Vec<&ResourceDescriptor> = bd.resolved_dependencies.iter().filter(|d| d.name.as_deref().is_some_and(|n| n.starts_with(names::CACHE_PREFIX))).collect();
    let cache_ok = match (&r.cache, caches.as_slice()) {
        (Some(Some(c)), [d]) => {
            d.name.as_deref() == Some(format!("{}{}", names::CACHE_PREFIX, c.lineage).as_str())
                && d.sha256().map(|h| format!("sha256:{h}")).as_deref() == Some(c.gen_digest.as_str())
        }
        (Some(Some(_)), _) => false,
        (_, []) => true,
        _ => false,
    };
    if !cache_ok {
        return bad("cache generation");
    }
    if let Some(run) = &r.run {
        if (ip.isolation, ip.wall, ip.tenant_slot, &ip.lane) != (run.isolation, run.wall, run.tenant_slot, &run.lane) {
            return bad("isolation, wall, slot or lane");
        }
        if rd.builder.version.get("k2-node") != Some(&run.k2node_version) || rd.builder.version.get("k2-node-commit") != run.k2node_commit.as_ref() {
            return bad("k2-node version");
        }
        let epoch = key.and_then(|k| k.annotations.get("keyEpoch"));
        let st_digest = rd
            .builder
            .builder_dependencies
            .iter()
            .find(|d| d.name.as_deref() == Some(names::NODE_STATEMENT))
            .and_then(|d| d.sha256())
            .map(|h| format!("sha256:{h}"));
        if epoch != Some(&Value::from(run.key_epoch)) || st_digest.as_deref() != Some(run.statement_digest.as_str()) {
            return bad("key epoch or self-statement");
        }
    }
    Ok(())
}

// ── whose fleet ──────────────────────────────────────────────────────

impl Statement {
    /// The owner `builder.id` names: the node's claim, which only the
    /// owner's roster proves. `None` for an id outside both forms
    /// ([`Statement::check`] refuses those).
    pub fn builder_owner(&self) -> Option<Owner> {
        let id = &self.predicate.run_details.builder.id;
        if let Some(fp) = id.strip_prefix(BUILDER_ID_OWNER).filter(|f| crate::ci::is_fp(f)) {
            return Some(Owner::RosterKey(fp.to_string()));
        }
        id.strip_prefix(BUILDER_ID_OWNER_SERVER).filter(|f| crate::ci::is_fp(f)).map(|fp| Owner::Server(fp.to_string()))
    }

    /// The rule for a verifier that pins an owner's roster key
    /// (`k2 compute verify --owner-key`, k2-signer, kessel-attest): the
    /// statement must name exactly that key (`…/owner/<fp>`). The
    /// `…/owner-server/<fp>` form is refused: a node that held no roster
    /// when it signed names no roster key, so nothing ties it to the
    /// pinned owner. Whether the node key is active in that owner's roster
    /// is the verifier's next check.
    pub fn check_builder_owner(&self, roster_key_fp: &str) -> Result<(), String> {
        match self.builder_owner() {
            Some(Owner::RosterKey(fp)) if fp == roster_key_fp => Ok(()),
            Some(Owner::RosterKey(fp)) => Err(format!("provenance: builder.id names roster key {fp}, not the pinned owner key {roster_key_fp}")),
            Some(Owner::Server(fp)) => Err(format!(
                "provenance: builder.id is the owner-server form ({fp}): the node held no roster from its owner when it signed, so it can't be checked against a pinned roster key"
            )),
            None => Err("provenance: builder.id is not an owner's".into()),
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────

/// `source.uri` from the plan's remote (BA-14): `git+<remote>` with any
/// user name and password taken out; an scp-style `user@host:path` becomes
/// `git+ssh://host/path`; a bare path `git+file://<path>`; no remote
/// (bundles only) `k2-project:<project_key>`.
pub fn source_uri(remote: Option<&str>, project_key: &str) -> String {
    let Some(remote) = remote.map(str::trim).filter(|r| !r.is_empty()) else {
        return format!("k2-project:{project_key}");
    };
    let remote = remote.strip_prefix("git+").unwrap_or(remote);
    if let Some(i) = remote.find("://") {
        let (scheme, rest) = (&remote[..i], &remote[i + 3..]);
        let auth_end = rest.find('/').unwrap_or(rest.len());
        let host_path = match rest[..auth_end].rfind('@') {
            Some(at) => &rest[at + 1..],
            None => rest,
        };
        return format!("git+{scheme}://{host_path}");
    }
    // scp-like: [user@]host:path (no slash before the colon).
    if let Some(colon) = remote.find(':') {
        if !remote[..colon].contains('/') {
            let host = remote[..colon].rsplit('@').next().unwrap_or("");
            let path = remote[colon + 1..].trim_start_matches('/');
            return format!("git+ssh://{host}/{path}");
        }
    }
    format!("git+file://{remote}")
}

/// Percent-encode a package URL part: everything but ASCII letters, digits
/// and `.-_~`.
pub fn purl_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Unix seconds as RFC 3339 UTC (`2027-01-15T08:00:00Z`). Years 0001–9999.
pub fn rfc3339(secs: i64) -> Result<String, String> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if !(1..=9999).contains(&y) {
        return Err(format!("provenance: time {secs} is outside years 1-9999"));
    }
    Ok(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60))
}

/// The inverse of [`rfc3339`] (exactly that form), for checks.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r).filter(|t| t.bytes().all(|c| c.is_ascii_digit()))?.parse().ok() };
    let (y, m, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    // Days from civil (H. Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let t = days * 86_400 + h * 3600 + mi * 60 + sec;
    (rfc3339(t).ok()? == s).then_some(t)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::crypto::SigningKey;
    use serde_json::json;

    /// Every signing domain in the crate (what a node or controller key
    /// signs under). A new domain goes here.
    pub(crate) const DOMAINS: &[&str] = &[
        crate::admin::DOMAIN_CMD_CONTROLLER,
        crate::admin::DOMAIN_CMD_OWNER,
        crate::admin::DOMAIN_PLAN_OWNER,
        crate::admin::DOMAIN_KEYS_CONTROLLER,
        crate::admin::DOMAIN_KEYS_OWNER,
        crate::ci::DOMAIN_NODE_STATEMENT,
        crate::ci::DOMAIN_NODE_ROTATION,
        crate::ci::DOMAIN_NODE_ROSTER,
        crate::ci::DOMAIN_NODE_SEAT,
        crate::ci::DOMAIN_NODE_ATTEST,
        crate::ci::DOMAIN_SIGNER_SAS,
        crate::gitcred::DOMAIN,
        crate::secret::DOMAIN,
        crate::frames::DOMAIN_ATTACH_CONTROLLER,
        crate::frames::DOMAIN_ATTACH_NODE,
        crate::frames::DOMAIN_SESSION,
        crate::frames::DOMAIN_FRAME_C2N,
        crate::frames::DOMAIN_FRAME_N2C,
        crate::frames::DOMAIN_ENROLL_CONTROLLER,
        crate::frames::DOMAIN_ENROLL_NODE,
        crate::frames::DOMAIN_RECEIPT,
        crate::pairing::SAS_DOMAIN_COMPUTE,
    ];

    const COMMIT: &str = "1111111111111111111111111111111111111111";
    const TREE: &str = "2222222222222222222222222222222222222222";

    fn plan_json() -> Value {
        json!({
            "job_id": "job-1", "node_id": "test-node-01", "workspace_id": "ws-1", "workspace_label": "acme",
            "requested_by": "agent", "argv": ["bash", "scripts/ci/build.sh", "x86_64"],
            "env": {"BUILD_COMMIT": COMMIT}, "cwd": null,
            "limits": {"max_secs": 7200, "cpu_millis": 4000, "mem_bytes": 8589934592u64, "disk_bytes": 107374182400u64, "log_cap_bytes": 67108864},
            "src": {"project_key": "pk", "remote_url": "https://ci:s3cr3t@git.example.test/acme/app.git", "commit": COMMIT, "dirty": null, "slots": 1},
            "exclusive": false, "created_at": 1800000700, "needs": ["rust@1.99.1"], "artifacts": ["dist/*"],
            "secret_env": ["BUILD_API_KEY"], "tier": "ci", "provenance": 1
        })
    }

    fn plan() -> Plan {
        serde_json::from_value(plan_json()).unwrap()
    }

    fn receipt_json(key: &SigningKey, p: &Plan) -> Value {
        json!({
            "v": 1, "job_id": "job-1", "attempt": 1, "generation": 1, "plan_digest": p.digest().unwrap(),
            "node_fp": key.fingerprint(), "controller_fp": "c".repeat(64), "workspace_id": "ws-1",
            "argv_sha256": crate::frames::argv_sha256(&p.argv),
            "env_names": ["BUILD_COMMIT", "K2_JOB_ID"],
            "src": {"commit": COMMIT, "tree": TREE, "dirty_sha256": null, "slot": null, "warmth": "private"},
            "tools": {}, "boot_id": "boot-1", "started_at": 1800000731, "ended_at": 1800002260,
            "state": "done", "exit": {"code": 0, "signal": null},
            "log": {"sha256": "3".repeat(64), "bytes": 812345, "truncated": false},
            "artifacts": [
                {"name": "dist/acme-linux-x86_64", "sha256": "4".repeat(64), "bytes": 1000},
                {"name": "dist/acme-linux-x86_64.sha256", "sha256": "5".repeat(64), "bytes": 90}
            ],
            "secret_env": ["BUILD_API_KEY"],
            "tools_used": {"cargo": "1.99.1", "rust-toolchain": "1.99.1-x86_64-unknown-linux-gnu", "rustc": "1.99.1"}
        })
    }

    fn sign_receipt(key: &SigningKey, v: Value) -> SignedReceipt {
        let r: Receipt = serde_json::from_value(v).unwrap();
        SignedReceipt::sign(key, &r).unwrap()
    }

    fn facts() -> NodeFacts {
        NodeFacts {
            owner: Owner::RosterKey("e".repeat(64)),
            network: Network::None,
            isolation: Isolation::Exclusive,
            wall: Wall::JobUser,
            tenant_slot: 1,
            lane: Some("ci-standard".into()),
            k2node_version: "0.46.13".into(),
            k2node_commit: Some("6".repeat(40)),
            protocol: 10,
            key_epoch: 3,
            statement_digest: Some(format!("sha256:{}", "7".repeat(64))),
            node_name: Some("test-node-01".into()),
        }
    }

    pub(crate) fn built(key: &SigningKey) -> (Plan, SignedReceipt, Statement) {
        let p = plan();
        let sr = sign_receipt(key, receipt_json(key, &p));
        let st = Statement::for_receipt(&sr, &p, &facts()).unwrap().expect("artifacts: a statement");
        (p, sr, st)
    }

    #[test]
    fn pae_is_the_dsse_spec_encoding() {
        // The DSSE spec's own example (protocol.md).
        assert_eq!(pae("http://example.com/HelloWorld", b"hello world"), b"DSSEv1 29 http://example.com/HelloWorld 11 hello world".to_vec());
        assert_eq!(pae(PAYLOAD_TYPE, b""), b"DSSEv1 28 application/vnd.in-toto+json 0 ".to_vec());
    }

    /// BA-12 / BA-29, both directions: no K2 domain message is a PAE, and
    /// no PAE of a statement is a K2 domain message.
    #[test]
    fn a_pae_and_a_k2_domain_message_never_collide() {
        let key = SigningKey::generate().unwrap();
        let (_, _, st) = built(&key);
        let p = pae(PAYLOAD_TYPE, &payload(&st).unwrap());
        assert!(!p.contains(&0), "a PAE of a statement holds no zero byte");
        for d in DOMAINS {
            assert!(!d.contains(' ') && !d.as_bytes().contains(&0), "{d}: a domain has no space or zero byte");
            for parts in [&[][..], &[&b""[..]][..], &[&b"DSSEv1 "[..], &b"x"[..]][..]] {
                let m = crate::crypto::signed_bytes(d, parts);
                assert!(!m.starts_with(PAE_PREFIX.as_bytes()) || m[PAE_PREFIX.len()] != b' ', "{d}: a domain message starts like a PAE");
            }
            let mut head = d.as_bytes().to_vec();
            head.push(0);
            assert!(!p.starts_with(&head), "{d}: a PAE parses as this domain's message");
        }
        // And the one place a zero byte could come from is refused.
        assert!(check_payload_bytes(b"{\"a\":\"\0\"}").unwrap_err().contains("zero byte"));
        let mut q = plan_json();
        q["env"]["NUL"] = json!("a\u{0}b");
        let q: Plan = serde_json::from_value(q).unwrap();
        let mut rv = receipt_json(&key, &q);
        rv["env_names"] = json!(["BUILD_COMMIT", "K2_JOB_ID", "NUL"]);
        let sr = sign_receipt(&key, rv);
        let st = Statement::for_receipt(&sr, &q, &facts()).unwrap().unwrap();
        let bytes = payload(&st).unwrap();
        assert!(!bytes.contains(&0) && String::from_utf8(bytes).unwrap().contains("a\\u0000b"), "JSON escapes it");
    }

    #[test]
    fn signed_by_the_node_key_and_checked_against_its_receipt() {
        let key = SigningKey::generate().unwrap();
        let (_, sr, st) = built(&key);
        let env = key.sign_statement(&st).unwrap();
        assert_eq!(env.payload_type, PAYLOAD_TYPE);
        assert_eq!(env.keyid(), Some(key.fingerprint().as_str()));
        assert_eq!(env.open(key.spki_der()).unwrap(), st);
        assert_eq!(env.open_for_receipt(key.spki_der(), &sr).unwrap(), st);
        // The exact envelope shape (BA-30).
        let v = serde_json::to_value(&env).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["payload", "payloadType", "signatures"]);
        let sig_keys: Vec<&String> = v["signatures"][0].as_object().unwrap().keys().collect();
        assert_eq!(sig_keys, vec!["keyid", "sig"]);
        assert_eq!(env.payload.len() % 4, 0, "padded base64");
        assert!(env.line().unwrap().ends_with("}\n") && !env.line().unwrap().trim_end().contains('\n'), "one JSON line");
        // Another key, a forged keyid, a changed payload, another type, two
        // signatures: all refused.
        let other = SigningKey::generate().unwrap();
        assert!(env.open(other.spki_der()).unwrap_err().contains("keyid"));
        let mut forged = env.clone();
        forged.signatures[0].keyid = other.fingerprint();
        assert!(forged.open(other.spki_der()).unwrap_err().contains("does not verify"));
        let mut st2 = st.clone();
        st2.subject[0].digest.insert("sha256".into(), "9".repeat(64));
        let mut changed = env.clone();
        changed.payload = crypto::b64(&payload(&st2).unwrap());
        assert!(changed.open(key.spki_der()).unwrap_err().contains("does not verify"));
        let mut typed = env.clone();
        typed.payload_type = "application/json".into();
        assert!(typed.open(key.spki_der()).is_err());
        let mut two = env.clone();
        two.signatures.push(two.signatures[0].clone());
        assert!(two.open(key.spki_der()).unwrap_err().contains("exactly one"));
        let unpadded = Dsse { payload: env.payload.trim_end_matches('=').to_string(), ..env.clone() };
        if unpadded.payload != env.payload {
            assert!(unpadded.open(key.spki_der()).is_err());
        }
    }

    #[test]
    fn the_statement_is_the_receipts_facts() {
        let key = SigningKey::generate().unwrap();
        let (p, sr, st) = built(&key);
        let r: Receipt = serde_json::from_str(&sr.body).unwrap();
        let names: Vec<_> = st.subject.iter().map(|s| (s.name.clone().unwrap(), s.sha256().unwrap().to_string())).collect();
        assert_eq!(names, r.artifacts.iter().map(|a| (a.name.clone(), a.sha256.clone())).collect::<Vec<_>>());
        let bd = &st.predicate.build_definition;
        let src = bd.external_parameters.source.as_ref().unwrap();
        assert_eq!(src.uri.as_deref(), Some("git+https://git.example.test/acme/app.git"), "the password never reaches a statement");
        assert_eq!(src.digest.get("gitCommit").map(String::as_str), Some(COMMIT));
        assert_eq!(bd.resolved_dependencies[0].digest.get("gitTree").map(String::as_str), Some(TREE));
        assert_eq!(bd.external_parameters.argv, p.argv);
        assert_eq!(bd.external_parameters.secret_env, vec!["BUILD_API_KEY"]);
        let ip = &bd.internal_parameters;
        assert_eq!((ip.network, ip.isolation, ip.wall, ip.tenant_slot), (Network::None, Isolation::Exclusive, Wall::JobUser, 1));
        assert_eq!(ip.warmth.as_deref(), Some("private"));
        // Toolchains carry a uri (BA-30).
        let rustc = bd.resolved_dependencies.iter().find(|d| d.name.as_deref() == Some("toolchain:rustc")).unwrap();
        assert_eq!(rustc.uri.as_deref(), Some("pkg:generic/rustc@1.99.1"));
        let rd = &st.predicate.run_details;
        assert_eq!(rd.builder.id, format!("{BUILDER_ID_OWNER}{}", "e".repeat(64)), "one id per owner");
        let nk = rd.builder.builder_dependencies.iter().find(|d| d.name.as_deref() == Some(names::NODE_KEY)).unwrap();
        assert_eq!(nk.sha256(), Some(key.fingerprint().as_str()), "the node goes in builderDependencies");
        assert_eq!(rd.metadata.started_on, "2027-01-15T08:12:11Z");
        assert_eq!(rd.metadata.invocation_id, "job-1/1/1");
        let rc = rd.byproducts.iter().find(|d| d.name.as_deref() == Some(names::RECEIPT)).unwrap();
        assert_eq!(rc.sha256(), Some(crypto::sha256_hex(sr.body.as_bytes()).as_str()));
        check_against_receipt(&st, &sr).unwrap();
    }

    #[test]
    fn no_artifacts_no_statement() {
        let key = SigningKey::generate().unwrap();
        let p = plan();
        let mut rv = receipt_json(&key, &p);
        rv.as_object_mut().unwrap().remove("artifacts");
        let sr = sign_receipt(&key, rv);
        assert_eq!(Statement::for_receipt(&sr, &p, &facts()).unwrap(), None);
        // And a statement with no subject never passes its own check.
        let (_, _, mut st) = built(&key);
        st.subject.clear();
        assert!(st.check().unwrap_err().contains("at least one subject"));
        assert!(key.sign_statement(&st).is_err(), "nothing to sign");
    }

    /// Receipt and statement can't disagree: the builder takes the
    /// receipt's own facts and refuses a plan that isn't the receipt's; a
    /// statement made for one receipt fails against any other.
    #[test]
    fn a_receipt_and_its_statement_cannot_disagree() {
        let key = SigningKey::generate().unwrap();
        let (p, sr, st) = built(&key);
        // A plan that isn't the receipt's: refused.
        let mut q = p.clone();
        q.env.insert("EXTRA".into(), "1".into());
        assert!(Statement::for_receipt(&sr, &q, &facts()).unwrap_err().contains("plan_digest"));
        // Each change to the receipt: the old statement fails against it,
        // a statement built for it passes.
        type Edit = Box<dyn Fn(&mut Value)>;
        let edits: Vec<(&str, Edit)> = vec![
            ("one artifact digest", Box::new(|v| v["artifacts"][1]["sha256"] = json!("8".repeat(64)))),
            ("an artifact name", Box::new(|v| v["artifacts"][0]["name"] = json!("dist/other"))),
            ("one artifact fewer", Box::new(|v| { v["artifacts"].as_array_mut().unwrap().pop(); })),
            ("commit", Box::new(|v| v["src"]["tree"] = json!("9".repeat(40)))),
            ("dirty patch", Box::new(|v| v["src"]["dirty_sha256"] = json!("a".repeat(64)))),
            ("warmth", Box::new(|v| v["src"]["warmth"] = json!("warm"))),
            ("node key", Box::new(|v| v["node_fp"] = json!("b".repeat(64)))),
            ("end time", Box::new(|v| v["ended_at"] = json!(1800002261))),
            ("attempt", Box::new(|v| v["attempt"] = json!(2))),
            ("controller", Box::new(|v| v["controller_fp"] = json!("d".repeat(64)))),
            ("tools", Box::new(|v| v["tools_used"]["rustc"] = json!("1.99.2"))),
            ("log", Box::new(|v| v["log"]["sha256"] = json!("f".repeat(64)))),
            ("an env name", Box::new(|v| v["env_names"] = json!(["K2_JOB_ID"]))),
        ];
        for (what, edit) in edits {
            let mut v = receipt_json(&key, &p);
            edit(&mut v);
            let other = sign_receipt(&key, v);
            assert!(check_against_receipt(&st, &other).is_err(), "{what}: the old statement still agrees");
            match Statement::for_receipt(&other, &p, &facts()) {
                Ok(Some(s2)) => check_against_receipt(&s2, &other).unwrap_or_else(|e| panic!("{what}: built for it, yet {e}")),
                Ok(None) => panic!("{what}: no statement"),
                Err(e) => assert!(what == "an env name", "{what}: {e}"),
            }
        }
        // Receipt v2: its run facts win over the node's, and must agree.
        let mut v = receipt_json(&key, &p);
        v["v"] = json!(2);
        v["run"] = json!({"isolation": "job_uid", "uid_index": 3, "wall": "job_user", "tenant_slot": 2, "k2node_version": "0.46.12",
            "k2node_commit": null, "key_epoch": 1, "statement_digest": format!("sha256:{}", "a".repeat(64)), "roster_seq_seen": 4, "lane": null});
        let v2 = sign_receipt(&key, v);
        let s2 = Statement::for_receipt(&v2, &p, &facts()).unwrap().unwrap();
        let ip = &s2.predicate.build_definition.internal_parameters;
        assert_eq!((ip.isolation, ip.tenant_slot, ip.lane.clone()), (Isolation::JobUid, 2, None), "the receipt's facts, not the node's");
        assert_eq!(s2.predicate.run_details.builder.version.get("k2-node").map(String::as_str), Some("0.46.12"));
        check_against_receipt(&s2, &v2).unwrap();
        assert!(check_against_receipt(&st, &v2).is_err());
        let mut lied = s2.clone();
        lied.predicate.build_definition.internal_parameters.isolation = Isolation::Exclusive;
        assert!(check_against_receipt(&lied, &v2).unwrap_err().contains("isolation"));
    }

    /// A statement (and its receipt) naming another node key than the one
    /// that signed the envelope is refused, even though each verifies.
    #[test]
    fn the_signing_key_is_the_named_node_key() {
        let key = SigningKey::generate().unwrap();
        let other = SigningKey::generate().unwrap();
        let p = plan();
        // A receipt that says it is `other`'s, with a statement that agrees,
        // both signed by `key`.
        let sr = sign_receipt(&key, receipt_json(&other, &p));
        let st = Statement::for_receipt(&sr, &p, &facts()).unwrap().unwrap();
        check_against_receipt(&st, &sr).unwrap();
        let env = key.sign_statement(&st).unwrap();
        env.open(key.spki_der()).unwrap();
        assert!(env.open_for_receipt(key.spki_der(), &sr).unwrap_err().contains("another node key"));
    }

    /// What a job writes reaches the statement only as data: its outputs'
    /// names and digests (subjects) and its env values, escaped. A name or
    /// value shaped like JSON stays a string.
    #[test]
    fn job_written_text_stays_inside_its_strings() {
        let key = SigningKey::generate().unwrap();
        let mut pj = plan_json();
        pj["env"]["INJECT"] = json!(r#"x"},"subject":[{"name":"evil","digest":{"sha256":"00"}}],"a":{"b":"#);
        let p: Plan = serde_json::from_value(pj).unwrap();
        let mut rv = receipt_json(&key, &p);
        rv["env_names"] = json!(["BUILD_COMMIT", "INJECT", "K2_JOB_ID"]);
        rv["artifacts"][0]["name"] = json!("dist/\"},{\"name\":\"evil\"\n.bin");
        let sr = sign_receipt(&key, rv);
        let st = Statement::for_receipt(&sr, &p, &facts()).unwrap().unwrap();
        let env = key.sign_statement(&st).unwrap();
        let back = env.open_for_receipt(key.spki_der(), &sr).unwrap();
        assert_eq!(back.subject.len(), 2);
        assert_eq!(back.subject[0].name.as_deref(), Some("dist/\"},{\"name\":\"evil\"\n.bin"));
        assert!(back.predicate.build_definition.external_parameters.env["INJECT"].starts_with("x\"}"));
    }

    #[test]
    fn shape_rules_refuse_what_stock_tools_would() {
        let key = SigningKey::generate().unwrap();
        let (_, _, st) = built(&key);
        st.check().unwrap();
        // A toolchain entry with neither uri nor digest (BA-30).
        let mut s = st.clone();
        let t = s.predicate.build_definition.resolved_dependencies.iter_mut().find(|d| d.name.as_deref() == Some("toolchain:rustc")).unwrap();
        t.uri = None;
        assert!(s.check().unwrap_err().contains("neither uri nor digest"));
        // A per-node builder id.
        let mut s = st.clone();
        s.predicate.run_details.builder.id = "https://k2.dev/compute/builder/v1/node/test-node-01".into();
        assert!(s.check().unwrap_err().contains("builder.id"));
        // A float, a bad digest, a subject with a uri, a second node key.
        let mut s = st.clone();
        s.predicate.run_details.byproducts[0].annotations.insert("x".into(), json!(1.5));
        assert!(s.check().unwrap_err().contains("float"));
        let mut s = st.clone();
        s.subject[0].digest.insert("sha256".into(), "ABC".into());
        assert!(s.check().is_err());
        let mut s = st.clone();
        s.subject[0].uri = Some("file:x".into());
        assert!(s.check().is_err());
        let mut s = st.clone();
        let k = s.predicate.run_details.builder.builder_dependencies.last().unwrap().clone();
        s.predicate.run_details.builder.builder_dependencies.push(k);
        assert!(s.check().unwrap_err().contains("node key exactly once"));
        // Unknown fields and a non-canonical payload are refused at parse.
        let mut v = serde_json::to_value(&st).unwrap();
        v["extra"] = json!(1);
        assert!(Dsse::parse(&crate::canonical::to_vec(&v).unwrap()).is_err());
        let loose = serde_json::to_vec_pretty(&st).unwrap();
        assert!(Dsse::parse(&loose).unwrap_err().contains("canonical"));
    }

    #[test]
    fn source_uris() {
        assert_eq!(source_uri(Some("https://u:tok@git.example.test/a/b.git"), "pk"), "git+https://git.example.test/a/b.git");
        assert_eq!(source_uri(Some("https://git.example.test/a/b"), "pk"), "git+https://git.example.test/a/b");
        assert_eq!(source_uri(Some("git@git.example.test:a/b.git"), "pk"), "git+ssh://git.example.test/a/b.git");
        assert_eq!(source_uri(Some("ssh://git@git.example.test/a/b.git"), "pk"), "git+ssh://git.example.test/a/b.git");
        assert_eq!(source_uri(Some("git+https://git.example.test/a/b"), "pk"), "git+https://git.example.test/a/b");
        assert_eq!(source_uri(Some("/srv/git/app.git"), "pk"), "git+file:///srv/git/app.git");
        assert_eq!(source_uri(None, "pk"), "k2-project:pk");
        assert_eq!(source_uri(Some("  "), "pk"), "k2-project:pk");
        assert_eq!(purl_escape("1.99.1-x86_64"), "1.99.1-x86_64");
        assert_eq!(purl_escape("cargo,clippy rustc+x"), "cargo%2Cclippy%20rustc%2Bx");
    }

    /// A verifier pinning a roster key takes only `…/owner/<that fp>`; the
    /// owner-server form (a node that held no roster) and another owner's
    /// key are refused by name.
    #[test]
    fn a_pinned_roster_key_refuses_the_owner_server_form() {
        let key = SigningKey::generate().unwrap();
        let (p, sr, st) = built(&key);
        assert_eq!(st.builder_owner(), Some(Owner::RosterKey("e".repeat(64))));
        st.check_builder_owner(&"e".repeat(64)).unwrap();
        let e = st.check_builder_owner(&"f".repeat(64)).unwrap_err();
        assert!(e.contains("not the pinned owner key"), "{e}");
        let server = NodeFacts { owner: Owner::Server("d".repeat(64)), ..facts() };
        let st2 = Statement::for_receipt(&sr, &p, &server).unwrap().unwrap();
        assert_eq!(st2.builder_owner(), Some(Owner::Server("d".repeat(64))));
        let e = st2.check_builder_owner(&"d".repeat(64)).unwrap_err();
        assert!(e.contains("owner-server form"), "even the same 64 hex: {e}");
        let mut odd = st;
        odd.predicate.run_details.builder.id = "https://example.test/builder".into();
        assert_eq!(odd.builder_owner(), None);
        assert!(odd.check_builder_owner(&"e".repeat(64)).is_err());
    }

    #[test]
    fn rfc3339_vectors() {
        for (t, s) in [
            (0, "1970-01-01T00:00:00Z"),
            (1_800_000_000, "2027-01-15T08:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (4_102_444_799, "2099-12-31T23:59:59Z"),
            (-1, "1969-12-31T23:59:59Z"),
        ] {
            assert_eq!(rfc3339(t).unwrap(), s);
            assert_eq!(parse_rfc3339(s), Some(t));
        }
        assert_eq!(parse_rfc3339("2027-02-30T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2027-01-15T08:00:00+00:00"), None);
        assert!(rfc3339(i64::MAX / 2).is_err());
    }
}
