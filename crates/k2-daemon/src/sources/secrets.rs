//! Source secrets (contract §10.3, PRD §5.2): `Requester::Source` resolve,
//! binding a vault grant to one source and one origin, unbinding, the
//! `urlSecret` URL, denial reasons, and the root-key hint. The approve
//! routes (B2) call [`bind`], [`unbind`] and [`root_key_hint_for`].
//!
//! The grant is an ordinary `ConsumerKind::Daemon` grant with consumer id
//! `source:<id>`, delivery `fetch`, hosts narrowed to the source's exact
//! origin [GV42]. The value never leaves the daemon: it goes into the
//! sealed request (vault_fetch), and the response is redacted with the
//! vault's redactor before anything parses it.

use std::sync::Arc;

use k2_core::sources::{FetchPlan, Reason, UrlRef};
use k2_vault::hosts::{Origin, OriginPattern};
use k2_vault::redact::Redactor;
use k2_vault::{Consumer, ConsumerKind, Delivery, GrantRequest, Requester, Scope, SecretStore, Selector, Use, VaultError};
use serde::Serialize;

use super::fetch::Resolved;
use super::SourceErr;
use crate::vault_fetch::Injection;

/// A root-level key that should be a restricted one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootKeyHint {
    /// `stripe_secret` | `supabase_service` | `cloudflare_global` |
    /// `github_classic`.
    pub code: &'static str,
    pub hint: &'static str,
}

/// The vault consumer for one source.
pub fn consumer(source_id: &str) -> Consumer {
    Consumer { kind: ConsumerKind::Daemon, id: format!("source:{source_id}") }
}

/// Sources name secrets on their own server, server scope.
fn selector(name: &str) -> Selector {
    Selector::user(Scope::server(), name)
}

/// A vault refusal as the fixed source reason.
pub fn denial_reason(e: &VaultError) -> Reason {
    match e.code {
        "vault_not_open" | "vault_locked" | "vault_rollback" | "vault_unavailable" => Reason::VaultUnavailable,
        "not_found" | "secret_not_found" => Reason::SecretMissing,
        _ if e.status == 404 => Reason::SecretMissing,
        _ if e.status == 503 => Reason::VaultUnavailable,
        _ => Reason::SecretDisabled,
    }
}

fn vault() -> Result<Arc<k2_vault::LocalVault>, Reason> {
    let v = crate::vault::get().map_err(|_| Reason::VaultUnavailable)?;
    if !v.status().open {
        return Err(Reason::VaultUnavailable);
    }
    Ok(v)
}

/// The injections, redactor and (for `urlSecret`) URL one plan needs.
/// Secretless plans need nothing. Errors are fixed reasons: `secret_missing`
/// (no such secret), `secret_disabled` (disabled, expired, not granted to
/// this source, or not for this origin), `vault_unavailable`.
pub fn resolve_plan(plan: &FetchPlan) -> Result<Resolved, Reason> {
    let secretless = plan.secret.is_none() && matches!(plan.url, UrlRef::Plain(_));
    if secretless {
        return Ok(Resolved { injections: Vec::new(), redactor: Arc::new(Redactor::empty()), url: None, extra_redact: None });
    }
    let source_id = plan.source_id.clone().ok_or(Reason::SecretMissing)?;
    let v = vault()?;
    if v.status().rollback {
        return Err(Reason::VaultUnavailable);
    }
    let who = Requester::Source { id: source_id };
    let (_, pinned) = Origin::parse_url(&format!("{}/", plan.origin)).map_err(|_| Reason::BlockedAddress)?;
    let mut url = None;
    let mut extra = None;
    if let UrlRef::Secret { name } = &plan.url {
        let r = v.resolve(&selector(name), Use::Fetch(&pinned), &who).map_err(|e| denial_reason(&e))?;
        let raw = r.value.expose_str().ok_or(Reason::SecretDisabled)?.trim().to_string();
        let (_, o) = Origin::parse_url(&raw).map_err(|_| Reason::SecretDisabled)?;
        if o != pinned {
            return Err(Reason::BlockedAddress);
        }
        // The whole URL, and its path and query on their own (that's where
        // webhook tokens live, and what an echo usually repeats).
        let parsed = url::Url::parse(&raw).map_err(|_| Reason::SecretDisabled)?;
        let path = parsed.path().to_string();
        let path_query = match parsed.query() {
            Some(q) => format!("{path}?{q}"),
            None => path.clone(),
        };
        let mut entries: Vec<(&str, &[u8])> = vec![(name.as_str(), raw.as_bytes())];
        if path_query.len() >= 8 {
            entries.push((name.as_str(), path_query.as_bytes()));
        }
        if path.len() >= 8 {
            entries.push((name.as_str(), path.as_bytes()));
        }
        extra = Some(Redactor::new(entries));
        url = Some(raw);
    }
    let mut injections = Vec::new();
    if let Some(name) = &plan.secret {
        let r = v.resolve(&selector(name), Use::Fetch(&pinned), &who).map_err(|e| denial_reason(&e))?;
        let (dst, fmt) = r.inject.choose(None, None).map_err(|_| Reason::SecretDisabled)?;
        let allowed = vec![OriginPattern::parse(&pinned.render(), false).map_err(|_| Reason::BlockedAddress)?];
        injections.push(Injection { name: r.meta.name.clone(), value: r.value, dst, fmt, allowed });
    }
    Ok(Resolved { injections, redactor: v.redactor(), url, extra_redact: extra })
}

/// Grant `secret_name` to source `source_id` for exactly `origin`
/// (`ConsumerKind::Daemon`, consumer `source:<id>`, delivery `fetch`).
/// Needs a person's confirm (`proof`); replaces any earlier binding.
pub fn bind(source_id: &str, secret_name: &str, origin: &str, proof: &k2_vault::HumanProof) -> Result<(), SourceErr> {
    if !matches!(proof, k2_vault::HumanProof::Confirmed { .. }) {
        return Err(SourceErr::new("vault_confirm_required", 403, "binding a secret needs a person's confirm"));
    }
    let v = crate::vault::get().map_err(|_| unavailable())?;
    if v.status().rollback {
        return Err(unavailable());
    }
    unbind(source_id)?;
    let req = GrantRequest {
        selector: selector(secret_name),
        consumer: consumer(source_id),
        delivery: Delivery::Fetch,
        env_name: None,
        file_name: None,
        hosts_narrowed: Some(vec![origin.to_string()]),
        expires_at: None,
        version: None,
    };
    v.grant(req, &k2_vault::Actor::System("sources".into())).map_err(|e| match e.code {
        "hosts_not_subset" | "host_not_allowed" | "no_allowed_hosts" => SourceErr::new(
            "secret_origin_mismatch",
            400,
            format!("{secret_name} isn't allowed to go to {origin}; add that site to the secret in Settings → Vault"),
        ),
        "not_found" | "secret_not_found" => SourceErr::new("secret_not_found", 404, format!("no server secret named {secret_name}")),
        _ if e.status == 404 => SourceErr::new("secret_not_found", 404, format!("no server secret named {secret_name}")),
        "secret_not_active" => SourceErr::new("secret_not_found", 404, format!("{secret_name} is disabled")),
        _ => SourceErr::new("vault_unavailable", 503, "the vault refused the grant"),
    })?;
    clear_denial(source_id);
    Ok(())
}

fn unavailable() -> SourceErr {
    SourceErr::new("vault_unavailable", 503, "the vault isn't open (or is waiting for the owner after a rollback)")
}

/// A bind (or the secret coming back) ends a denial park [GV68].
fn clear_denial(source_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(Some((mut st, _))) = k2_core::sources::store::load_state(&conn, source_id) {
        if st.vault_denied {
            st.vault_denied = false;
            let _ = k2_core::sources::store::save_state(&conn, &st, None);
        }
    }
}

/// Revoke every grant this source holds. Nothing bound is fine.
pub fn unbind(source_id: &str) -> Result<(), SourceErr> {
    let v = match crate::vault::get() {
        Ok(v) => v,
        // No vault on this box: nothing can be bound.
        Err(e) if e.code == "vault_not_open" => return Ok(()),
        Err(_) => return Err(unavailable()),
    };
    let grants = v.grants_to(&consumer(source_id)).map_err(|_| unavailable())?;
    for (g, _) in grants {
        v.revoke(&g.id, &k2_vault::Actor::System("sources".into())).map_err(|_| unavailable())?;
    }
    Ok(())
}

/// Reads the value inside the daemon only; returns the hint, never the
/// value [GV17].
pub fn root_key_hint_for(secret_name: &str) -> Result<Option<RootKeyHint>, SourceErr> {
    let v = crate::vault::get().map_err(|_| unavailable())?;
    let r = v
        .resolve(&selector(secret_name), Use::System, &Requester::System("sources".into()))
        .map_err(|e| match denial_reason(&e) {
            Reason::SecretMissing => SourceErr::new("secret_not_found", 404, format!("no server secret named {secret_name}")),
            Reason::VaultUnavailable => unavailable(),
            _ => SourceErr::new("secret_not_found", 404, format!("{secret_name} can't be used")),
        })?;
    let hint = r.value.expose_str().and_then(k2_core::sources::lint::root_key_hint);
    Ok(hint.map(|(code, hint)| RootKeyHint { code, hint }))
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
