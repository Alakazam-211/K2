//! R8 upgrade (prd-env-to-vault-v1): server secrets need a grant, even
//! through `.env.k2`.
//!
//! Before 0.46.4 a published service (or App) resolved any
//! `k2v://server/NAME` in its `.env.k2` as the daemon, with no grant. Now
//! the service itself is the requester ([`super::publish_env`]), so a
//! server reference needs a grant to the service or its workspace.
//!
//! So that legitimate services keep working, this pass runs **once**, at
//! the first boot with the new rule, before any service starts:
//!
//! - A server reference in a service's current `.env.k2` that would have
//!   resolved under the old rule (secret active, not expired, ceiling allows
//!   env) **and** was really used lately — the service started in the last
//!   [`WINDOW_DAYS`] days **and** the vault audit shows the secret resolved
//!   for a published service (`daemon:publish`) in that window — gets a
//!   grant to that one service (`service:<id>`), at env delivery, under the
//!   same env name the `.env.k2` line uses. The narrowest consumer, and
//!   exactly what the service already received: nothing is widened. The
//!   grant is in the vault audit (actor `system:upgrade-r8`).
//! - Any other server reference is left refused.
//! - Agent launch-block references (an AGENT.md `launch:` env, which the
//!   agent can edit) that the new rule refuses are never granted.
//!
//! The owner gets one notice listing every grant made and every reference
//! now refused (names only, never values), with the one-line undo or fix.
//! The report is kept at `~/.k2/vault/r8-server-refs.json` (0600), which
//! also marks the pass done.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use k2_vault::{
    Actor, AuditQuery, Consumer, Delivery, GrantRequest, LocalVault, Namespace, Requester, ScopeKind, SecretStatus, SecretStore,
    Selector,
};
use serde::{Deserialize, Serialize};

/// "Resolved lately": the look-back for the upgrade grant.
pub const WINDOW_DAYS: i64 = 30;

/// The audit actor of the old `.env.k2` resolve path (`Requester::System("publish")`).
const OLD_PUBLISH_ACTOR: &str = "daemon:publish";

const ACTOR: &str = "upgrade-r8";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefLine {
    /// Service name (publish name / App id), or the workspace for a launch block.
    pub consumer: String,
    pub workspace: String,
    /// The env name in `.env.k2` or the launch block.
    pub key: String,
    /// The server secret's name (never its value).
    pub secret: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub done_at: String,
    pub window_days: i64,
    /// Granted to the service so it keeps working.
    pub granted: Vec<RefLine>,
    /// Service references now refused (not used lately, or the grant failed).
    pub refused: Vec<RefLine>,
    /// Agent launch-block references now refused (never granted).
    pub launch_refused: Vec<RefLine>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.granted.is_empty() && self.refused.is_empty() && self.launch_refused.is_empty()
    }
}

fn marker_path() -> PathBuf {
    super::vault_dir(&k2_core::paths::k2_home()).join("r8-server-refs.json")
}

/// The once-only boot step (from [`super::boot`], before services start).
pub fn upgrade_once(vault: &LocalVault) {
    let marker = marker_path();
    if marker.exists() {
        return;
    }
    let report = run(vault, chrono::Utc::now());
    match serde_json::to_vec_pretty(&report) {
        Ok(body) => {
            if let Err(e) = k2_vault::fsutil::write_private_atomic(&marker, &body) {
                // Not marked: the pass runs again next boot. It is
                // idempotent (a grant replaces the same grant), but the
                // owner would get the notice again, so say so in the log.
                k2_core::log_debug!("[vault] R8 upgrade report not saved ({}); it runs again next boot", e.hint);
            }
        }
        Err(e) => k2_core::log_debug!("[vault] R8 upgrade report: {e}"),
    }
    k2_core::log_debug!(
        "[vault] R8 upgrade: granted {} refused {} launch-block refused {}",
        report.granted.len(),
        report.refused.len(),
        report.launch_refused.len()
    );
    if !report.is_empty() {
        notify_owner(&report);
    }
}

struct Service {
    id: String,
    workspace_id: String,
    name: String,
    dirs: Vec<PathBuf>,
    recent: bool,
}

fn services(cutoff_unix: i64) -> Vec<Service> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let sql = "SELECT ps.id, ps.project_id, ps.name, ps.cmd, ps.cwd, ps.last_started_at, \
               a.live_seed, a.preview_seed, a.state, a.updated_at \
               FROM published_services ps LEFT JOIN apps a ON a.service_id = ps.id";
    let mut st = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            k2_core::log_debug!("[vault] R8 upgrade: services: {e}");
            return vec![];
        }
    };
    #[allow(clippy::type_complexity)]
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
            r.get::<_, Option<i64>>(9)?,
        ))
    });
    let Ok(rows) = rows else { return vec![] };
    let mut out = vec![];
    for (id, ws, name, cmd, cwd, started, live, preview, state, app_updated) in rows.flatten() {
        let is_app = cmd == k2_core::published_services::CMD_APP_SENTINEL;
        let (dirs, recent) = if is_app {
            // An App resolves `.env.k2` inside its Seed (the engine's workdir).
            let dirs: Vec<PathBuf> =
                [live, preview].into_iter().flatten().filter(|h| !h.is_empty()).map(|h| k2_core::deploy::seed::seed_path(&h)).collect();
            let recent = state.as_deref() == Some("running") || app_updated.is_some_and(|t| t >= cutoff_unix);
            (dirs, recent)
        } else {
            (vec![PathBuf::from(cwd)], started.is_some_and(|t| t >= cutoff_unix))
        };
        out.push(Service { id, workspace_id: ws, name, dirs, recent });
    }
    out
}

/// `(key, ref)` pairs of `k2v://server/…` references in a service's env file.
fn server_refs_in(dir: &Path, env_file: &str) -> Vec<(String, String)> {
    let Ok(p) = crate::vault_use_routes::jailed_path(dir, env_file) else { return vec![] };
    let Ok(text) = std::fs::read_to_string(&p) else { return vec![] };
    super::publish_env::parse_env_k2(&text)
        .into_iter()
        // `granted()` skips a key it can't set before resolving anything.
        .filter(|(k, _)| k2_vault::delivery::validate_env_name(k).is_ok())
        .filter(|(_, v)| k2_vault::refs::VaultRef::parse(v).is_ok_and(|r| r.scope_kind == ScopeKind::Server))
        .collect()
}

/// Would the pre-R8 rule have resolved it (any service, no grant)?
fn old_rule_resolves(vault: &LocalVault, sel: &Selector) -> Option<k2_vault::SecretMeta> {
    let m = vault.meta(sel, &Actor::System(ACTOR.into())).ok()?;
    let expired = m
        .expires_at
        .as_deref()
        .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
        .is_some_and(|x| x < chrono::Utc::now());
    (m.namespace == Namespace::User
        && m.status == SecretStatus::Active
        && !expired
        && m.delivery_ceiling.level() >= Delivery::Env.level())
    .then_some(m)
}

/// Did the old `.env.k2` path resolve this secret since `since`?
fn resolved_by_publish_since(vault: &LocalVault, secret_id: &str, since: &str) -> bool {
    let q = AuditQuery { secret_id: Some(secret_id.to_string()), since: Some(since.to_string()), workspace_id: None, limit: Some(5000) };
    match vault.audit(&q, &Actor::System(ACTOR.into())) {
        Ok(rows) => rows.iter().any(|r| r.actor == OLD_PUBLISH_ACTOR && r.action == "resolve" && r.outcome.starts_with("ok")),
        Err(e) => {
            k2_core::log_debug!("[vault] R8 upgrade: audit: {}", e.code);
            false
        }
    }
}

/// The pass itself (tests call it with their own clock).
pub fn run(vault: &LocalVault, now: chrono::DateTime<chrono::Utc>) -> Report {
    vault.flush_rollups();
    let cutoff = now - chrono::Duration::days(WINDOW_DAYS);
    let since = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut report = Report { done_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), window_days: WINDOW_DAYS, ..Default::default() };
    for svc in services(cutoff.timestamp()) {
        let env_file = super::publish_env::get(&svc.id).env_file.unwrap_or_else(|| ".env.k2".into());
        let who = Requester::PublishedService { id: svc.id.clone(), workspace_id: svc.workspace_id.clone() };
        let mut seen = BTreeSet::new();
        for dir in &svc.dirs {
            for (key, r) in server_refs_in(dir, &env_file) {
                if !seen.insert((key.clone(), r.clone())) {
                    continue;
                }
                let Ok(sel) = super::selector_for_ref(&r) else { continue };
                let Some(m) = old_rule_resolves(vault, &sel) else { continue };
                if vault.ref_would_resolve(&sel, &who).unwrap_or(false) {
                    continue; // already granted: nothing changes
                }
                let mut line = RefLine {
                    consumer: svc.name.clone(),
                    workspace: super::workspace_label(&svc.workspace_id),
                    key: key.clone(),
                    secret: m.name.clone(),
                    reason: String::new(),
                };
                if !svc.recent || !resolved_by_publish_since(vault, &m.id, &since) {
                    line.reason = format!("not used by a published service in the last {WINDOW_DAYS} days");
                    report.refused.push(line);
                    continue;
                }
                let g = GrantRequest {
                    selector: Selector::Id(m.id.clone()),
                    consumer: Consumer::published_service(svc.id.clone()),
                    delivery: Delivery::Env,
                    // The same variable the `.env.k2` line sets (validated above).
                    env_name: Some(key.clone()),
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                };
                match vault.grant(g, &Actor::System(ACTOR.into())) {
                    Ok(_) => {
                        line.reason = format!("used in the last {WINDOW_DAYS} days; granted to this service only, as env");
                        report.granted.push(line);
                    }
                    Err(e) => {
                        line.reason = format!("the grant failed ({})", e.code);
                        report.refused.push(line);
                    }
                }
            }
        }
    }
    report.launch_refused = launch_block_refusals(vault);
    report
}

/// Agent launch-block env (AGENT.md) references the new rule refuses.
/// Reported, never granted: the agent can edit that file.
fn launch_block_refusals(vault: &LocalVault) -> Vec<RefLine> {
    let projects: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut st) = conn.prepare("SELECT id, path FROM projects") else { return vec![] };
        let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else { return vec![] };
        rows.flatten().collect()
    };
    let mut out = vec![];
    for (id, path) in projects {
        // The workspace agent (any name finds it when present) plus named
        // agents under `.k2/agents/`.
        let mut names: BTreeSet<String> = BTreeSet::from(["-".to_string()]);
        if let Ok(rd) = std::fs::read_dir(k2_core::workspace::agent_identity::agents_dir(&path)) {
            names.extend(rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)));
        }
        let who = Requester::Session { workspace_id: id.clone(), session_id: None };
        let mut seen = BTreeSet::new();
        for name in names {
            let Ok(Some(profile)) = k2_core::workspace::launch_profile::load_launch_profile(&path, &name) else { continue };
            for (key, r) in profile.env.unwrap_or_default() {
                if !k2_vault::refs::VaultRef::is_ref(&r) || !seen.insert((key.clone(), r.clone())) {
                    continue;
                }
                let Ok(sel) = super::selector_for_ref(&r) else { continue };
                let Some(m) = old_rule_resolves(vault, &sel) else { continue };
                if vault.ref_would_resolve(&sel, &who).unwrap_or(false) {
                    continue;
                }
                out.push(RefLine {
                    consumer: super::workspace_label(&id),
                    workspace: super::workspace_label(&id),
                    key,
                    secret: m.name.clone(),
                    reason: "in an agent launch block (AGENT.md), which the agent can edit; needs a grant to the workspace".into(),
                });
            }
        }
    }
    out
}

fn notice_body(r: &Report) -> String {
    let mut b = String::from(
        "K2 0.46.4 closes a gap: a published service or App could read any server-wide secret through a k2v://server/… \
         line in its .env.k2, with no grant. Now a server secret needs a grant to that service or its workspace, \
         like any shared secret. Names only below; no value was read or moved.\n",
    );
    if !r.granted.is_empty() {
        b.push_str(&format!(
            "\nKept working (used in the last {} days, so K2 granted each to that one service, as env):\n",
            r.window_days
        ));
        for l in &r.granted {
            b.push_str(&format!(
                "- {} ({}): {} = {}. Undo: k2 vault revoke {} --scope server --from service:{}\n",
                l.consumer, l.workspace, l.key, l.secret, l.secret, l.consumer
            ));
        }
    }
    if !r.refused.is_empty() {
        b.push_str("\nNow refused (the service starts without it):\n");
        for l in &r.refused {
            b.push_str(&format!(
                "- {} ({}): {} = {}: {}. To allow: k2 vault grant {} --scope server --to service:{} --as env --env-name {}\n",
                l.consumer, l.workspace, l.key, l.secret, l.reason, l.secret, l.consumer, l.key
            ));
        }
    }
    if !r.launch_refused.is_empty() {
        b.push_str("\nAgent launch blocks now refused (the agent's next session starts without it):\n");
        for l in &r.launch_refused {
            b.push_str(&format!(
                "- workspace {}: {} = {}: {}. To allow: k2 vault grant {} --scope server --to ws:{} --as env\n",
                l.workspace, l.key, l.secret, l.reason, l.secret, l.workspace
            ));
        }
    }
    b
}

fn notify_owner(r: &Report) {
    let n = r.granted.len() + r.refused.len() + r.launch_refused.len();
    let title = format!("Vault: {n} server secret reference{} now need a grant", if n == 1 { "" } else { "s" });
    let body = notice_body(r);
    crate::vault::auth_audit("vault-r8-upgrade", "daemon", &format!("granted {} refused {} launch {}", r.granted.len(), r.refused.len(), r.launch_refused.len()), "-");
    #[cfg(not(test))]
    if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-vault", &title, &body) {
        k2_core::log_debug!("[vault] R8 upgrade notice: {e}");
    }
    #[cfg(test)]
    let _ = (title, body);
}

#[cfg(test)]
#[path = "server_refs_tests.rs"]
mod tests;
