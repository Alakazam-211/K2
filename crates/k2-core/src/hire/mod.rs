//! Agent hire — one shared core for every door that makes an agent
//! (prd-agent-hire-templates-v1, the 2026-10-13 ruling "build A
//! only": the inline recipe, no saved templates).
//!
//! Doors (all call this module; none carries its own hire logic):
//! - `POST /v1/w` — converge one workspace at a path (Julie 2, Scout).
//! - `POST /cli/agents/hire` — `k2 agent hire <dir>` (converge one path).
//! - `POST /v1/agents/hire` and `POST /cli/agent-instances/hire` — the
//!   inline recipe: one agent or a team, all-or-nothing, idempotent.
//!
//! Pieces:
//! - [`root`]: the owner-set folder API-made workspaces live under (T2).
//! - [`recipe`]: the request shapes and their validation (fail closed on
//!   unknown fields — a newer client can't widen anything by being
//!   half-understood).
//! - [`steps`]: the per-workspace steps shared by converge and create.
//! - [`converge`]: create-or-converge one workspace at a path.
//! - [`plan`] / [`apply`]: plan every member (paths, predicted handles,
//!   group name) before any write, then apply with an undo list.
//! - [`limits`]: an API key's hire envelope and the ceiling checks.
//! - [`ledger`]: `agent_instances`, members, `hire_events`.
//!
//! Launch and first brief need the daemon's session machinery, so the
//! daemon runs them after the durable steps commit.

pub mod apply;
pub mod converge;
pub mod deliveries;
pub mod ledger;
pub mod limits;
pub mod links;
pub mod plan;
pub mod recipe;
pub mod root;
pub mod steps;
pub mod templates;

use std::fmt;

/// A refusal with an HTTP status, a stable code and a human hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HireError {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
}

impl HireError {
    pub fn new(status: u16, code: &'static str, hint: impl Into<String>) -> Self {
        Self { status, code, hint: hint.into() }
    }
    pub fn usage(hint: impl Into<String>) -> Self {
        Self::new(400, "usage", hint)
    }
    pub fn internal(hint: impl Into<String>) -> Self {
        Self::new(500, "hire_failed", hint)
    }
    /// The JSON error body every hire door answers with.
    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": false,
            "error": { "code": self.code, "hint": self.hint },
        })
    }
}

impl fmt::Display for HireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

/// Who is asking. Decides the extra checks, never the shape of the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HireCaller {
    /// The owner daemon token.
    Owner,
    /// A Connect login (Owner or Admin role; the route floor admits no less).
    Login { username: String },
    /// An agent passport (`k2 agent hire` inside a K2 cell). Never raises an
    /// agent switch (DB access) — the daemon-side self-grant check.
    Agent { workspace_id: String },
    /// A K2 v1 API key.
    ApiKey { id: String, label: Option<String> },
}

impl HireCaller {
    /// Audit actor: `owner-token`, `user:<name>`, `agent:<ws id>`,
    /// `api-key:<id>` (HT9). Never a secret.
    pub fn actor(&self) -> String {
        match self {
            HireCaller::Owner => "owner-token".to_string(),
            HireCaller::Login { username } => format!("user:{username}"),
            HireCaller::Agent { workspace_id } => format!("agent:{workspace_id}"),
            HireCaller::ApiKey { id, .. } => format!("api-key:{id}"),
        }
    }
    pub fn api_key_id(&self) -> Option<&str> {
        match self {
            HireCaller::ApiKey { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }
    pub fn key_label(&self) -> Option<&str> {
        match self {
            HireCaller::ApiKey { label, .. } => label.as_deref(),
            _ => None,
        }
    }
    pub fn is_agent(&self) -> bool {
        matches!(self, HireCaller::Agent { .. })
    }
}

/// Unix seconds.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
