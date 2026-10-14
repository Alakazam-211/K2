//! Source secrets (contract §10.3): `Requester::Source` resolve, binding a
//! vault grant to one source and origin, unbinding, denial parking, the
//! root-key hint. Owner: B1. The approve routes (B2) call [`bind`],
//! [`unbind`] and [`root_key_hint_for`].

use serde::Serialize;

use super::{not_built, SourceErr};

/// A root-level key that should be a restricted one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootKeyHint {
    /// `stripe_secret` | `supabase_service` | `cloudflare_global` |
    /// `github_classic`.
    pub code: &'static str,
    pub hint: &'static str,
}

/// The injections, redactor and (for `urlSecret`) URL one plan needs.
/// Secretless plans need nothing. A plan that names a secret resolves it
/// as `Requester::Source` (A1); until then it reads `secret_missing`.
pub fn resolve_plan(plan: &k2_core::sources::FetchPlan) -> Result<super::fetch::Resolved, k2_core::sources::Reason> {
    let secretless = plan.secret.is_none() && matches!(plan.url, k2_core::sources::UrlRef::Plain(_));
    if secretless {
        return Ok(super::fetch::Resolved {
            injections: Vec::new(),
            redactor: k2_vault::redact::Redactor::empty(),
            url: None,
        });
    }
    Err(k2_core::sources::Reason::SecretMissing)
}

/// Grant `secret_name` to source `source_id` for exactly `origin`
/// (`ConsumerKind::Daemon`, consumer `source:<id>`, delivery `fetch`).
///
/// C0 skeleton: 501 (B1 replaces it).
pub fn bind(source_id: &str, secret_name: &str, origin: &str, proof: &k2_vault::HumanProof) -> Result<(), SourceErr> {
    let _ = (source_id, secret_name, origin, proof);
    Err(not_built())
}

/// Revoke the source's grant.
///
/// C0 skeleton: 501 (B1 replaces it).
pub fn unbind(source_id: &str) -> Result<(), SourceErr> {
    let _ = source_id;
    Err(not_built())
}

/// Reads the value inside the daemon only; returns the hint, never the
/// value.
///
/// C0 skeleton: 501 (B1 replaces it).
pub fn root_key_hint_for(secret_name: &str) -> Result<Option<RootKeyHint>, SourceErr> {
    let _ = secret_name;
    Err(not_built())
}
