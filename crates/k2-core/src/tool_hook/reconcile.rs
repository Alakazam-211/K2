//! Reconcile: K2 chat grants ⇄ Sew members (S27 §16, S27-A5).
//!
//! [`build_desired`] reads what K2 wants (skin.db + pre-loaded workspace
//! facts); [`plan`] is a pure diff against Sew's `MgmtPrincipal` list.
//!
//! The desired set is EXACTLY: `k2:owner` (the box owner, implicit while
//! the target is enabled) plus one member per ENABLED chat grant whose
//! subject still exists. Nothing else — in particular no Connect user of
//! any role (rev 147: App Space membership only from App grants; Connect
//! owners/admins are never provisioned by role).

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};
use serde::Serialize;

use super::chat;
use super::sew_wire::{
    AgentKind, ExternalId, InstallRole, MemberKind, MgmtPrincipal, ProfilePatch, ProvisionRequest,
};
use super::{grant_rev, Cause, HookCtx, NewOp, Op, CHAT_KIND, OWNER_GRANT_REV};
use crate::skin::SkinGrant;

/// One member K2 wants in Sew.
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    pub request: ProvisionRequest,
    pub grant: Option<SkinGrant>,
    /// HC2 rev (`owner` for the box owner).
    pub grant_rev: String,
}

impl Desired {
    fn ext(&self) -> &ExternalId {
        &self.request.external_id
    }
}

/// What reconcile plans, before it becomes an outbox row.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedOp {
    pub op: Op,
    pub subject: ExternalId,
    pub payload: serde_json::Value,
    pub grant_id: Option<String>,
    pub grant_rev: Option<String>,
    pub after_subject: Option<String>,
}

impl PlannedOp {
    pub fn into_new_op(self, _kind: &str) -> NewOp {
        NewOp {
            tool: CHAT_KIND,
            op: self.op,
            subject_key: self.subject.to_string(),
            payload: self.payload,
            grant_id: self.grant_id,
            grant_rev: self.grant_rev,
            cause: Cause::Reconcile,
            after_subject: self.after_subject,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub provisioned: usize,
    pub updated: usize,
    pub deprovisioned: usize,
    pub skipped_busy: usize,
    /// HC2: subjects Sew answered `skipped`, left alone until the grant changes.
    pub skipped_held: usize,
    pub ignored_local: usize,
    /// External ids K2 does not own (e.g. a future `k2:connect:*`).
    pub ignored_other: usize,
    pub orphans_removed: usize,
}

/// The workspace ids that hold chat grants (to pre-load their facts
/// outside the skin.db lock).
pub fn workspace_ids_with_grants(conn: &Connection, kind: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT subject_id FROM grants WHERE kind = ?1 AND subject_kind = 'workspace'",
        )
        .map_err(|e| format!("reconcile grants: {e}"))?;
    let rows = stmt
        .query_map(params![kind], |r| r.get::<_, String>(0))
        .map_err(|e| format!("reconcile grants: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("reconcile grants row: {e}"))?);
    }
    Ok(out)
}

pub struct DesiredBuild {
    pub desired: Vec<Desired>,
    /// Chat grants whose subject is gone: reconcile deletes them and
    /// enqueues `deprovision` (§14.4).
    pub orphans: Vec<(SkinGrant, Cause)>,
}

/// K2's desired chat members. Runs inside the reconcile transaction;
/// workspace facts come from `ctx` (loaded before the lock).
pub fn build_desired(conn: &Connection, ctx: &HookCtx) -> Result<DesiredBuild, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{} WHERE kind = ?1 ORDER BY created_at, id",
            crate::skin::GRANT_SELECT
        ))
        .map_err(|e| format!("reconcile grants: {e}"))?;
    let rows = stmt
        .query_map(params![CHAT_KIND], crate::skin::map_grant_row)
        .map_err(|e| format!("reconcile grants: {e}"))?;
    let mut grants = Vec::new();
    for r in rows {
        grants.push(r.map_err(|e| format!("reconcile grant row: {e}"))?);
    }
    let mut desired = vec![Desired {
        request: chat::owner_provision_request(ctx),
        grant: None,
        grant_rev: OWNER_GRANT_REV.to_string(),
    }];
    let mut orphans = Vec::new();
    for g in grants {
        let gone = match g.subject_kind.as_str() {
            "principal" => chat::principal_by_id(conn, &g.subject_id)?
                .is_none()
                .then_some(Cause::PrincipalDelete),
            "workspace" => {
                (!ctx.workspaces.contains_key(&g.subject_id)).then_some(Cause::WorkspaceRemoved)
            }
            _ => None,
        };
        if let Some(cause) = gone {
            orphans.push((g, cause));
            continue;
        }
        if !g.enabled {
            continue;
        }
        match chat::provision_request(conn, &g, ctx) {
            Ok((request, _)) => desired.push(Desired {
                grant_rev: grant_rev(&g),
                request,
                grant: Some(g),
            }),
            Err(e) => crate::log_debug!("[grant-hook] reconcile skips grant {}: {e}", g.id),
        }
    }
    Ok(DesiredBuild { desired, orphans })
}

fn provision(d: &Desired) -> PlannedOp {
    PlannedOp {
        op: Op::Provision,
        subject: d.ext().clone(),
        payload: serde_json::to_value(&d.request).unwrap_or_default(),
        grant_id: d.grant.as_ref().map(|g| g.id.clone()),
        grant_rev: Some(d.grant_rev.clone()),
        after_subject: d.request.assistant_of.as_ref().map(ExternalId::to_string),
    }
}

fn deprovision(ext: &ExternalId, grant_rev: Option<String>) -> PlannedOp {
    PlannedOp {
        op: Op::Deprovision,
        subject: ext.clone(),
        payload: serde_json::json!({ "external_id": ext.to_string() }),
        grant_id: None,
        grant_rev,
        after_subject: None,
    }
}

/// Sew's handle is right when it is the base or the deterministic suffix.
fn username_ok(want: &str, have: &str, ext: &ExternalId) -> bool {
    have == want || have == chat::suffixed_username(want, &ext.to_string())
}

/// The pure diff (§16.2 step 5, S27-A5, HC2):
///
/// | Sew vs desired | op |
/// |---|---|
/// | desired, absent | provision |
/// | desired, deactivated | provision (reactivates) |
/// | desired, active, agent kind differs | deprovision, then provision |
/// | desired, active, guest or assistant_of differs | provision (upsert) |
/// | desired, active, display name / username differs | update |
/// | active `k2:principal`/`k2:workspace`, not desired | deprovision |
/// | `k2:owner` | never deprovision, never owner_set |
/// | `local:` | ignored, counted |
/// | busy (open outbox row) | skipped, counted |
/// | held (HC2) under the same grant rev | skipped, counted |
///
/// Guest = `install_role == guest`; `admin`/`owner` are not drift back
/// to member. Email is not on Sew's list, so it is not diffed.
pub fn plan(
    desired: &[Desired],
    actual: &[MgmtPrincipal],
    busy: &HashSet<String>,
    held: &HashMap<String, Option<String>>,
) -> (Vec<PlannedOp>, Summary) {
    let mut out = Vec::new();
    let mut s = Summary::default();
    let by_ext: HashMap<String, &MgmtPrincipal> = actual
        .iter()
        .map(|m| (m.external_id.to_string(), m))
        .collect();
    let mut wanted: HashSet<String> = HashSet::new();

    for d in desired {
        let key = d.ext().to_string();
        wanted.insert(key.clone());
        if busy.contains(&key) {
            s.skipped_busy += 1;
            continue;
        }
        if let Some(rev) = held.get(&key) {
            if rev.as_deref() == Some(d.grant_rev.as_str()) {
                s.skipped_held += 1;
                continue;
            }
        }
        let Some(m) = by_ext.get(&key) else {
            out.push(provision(d));
            s.provisioned += 1;
            continue;
        };
        if m.deactivated {
            out.push(provision(d));
            s.provisioned += 1;
            continue;
        }
        let want_agent_kind: Option<AgentKind> = d.request.agent_kind;
        if m.kind != d.request.kind
            || (d.request.kind == MemberKind::Agent && m.agent_kind != want_agent_kind)
        {
            out.push(deprovision(d.ext(), Some(d.grant_rev.clone())));
            out.push(provision(d));
            s.deprovisioned += 1;
            s.provisioned += 1;
            continue;
        }
        let is_guest = m.install_role == InstallRole::Guest;
        if is_guest != d.request.guest || m.assistant_of != d.request.assistant_of {
            out.push(provision(d));
            s.provisioned += 1;
            continue;
        }
        let want_user = &d.request.profile.username;
        let want_name = &d.request.profile.display_name;
        let user_ok = username_ok(want_user, &m.username, d.ext());
        if !user_ok || &m.display_name != want_name {
            let patch = ProfilePatch {
                display_name: (&m.display_name != want_name).then(|| want_name.clone()),
                username: (!user_ok).then(|| want_user.clone()),
                ..Default::default()
            };
            out.push(PlannedOp {
                op: Op::Update,
                subject: d.ext().clone(),
                payload: serde_json::json!({
                    "external_id": d.ext().to_string(),
                    "profile": patch,
                }),
                grant_id: d.grant.as_ref().map(|g| g.id.clone()),
                grant_rev: Some(d.grant_rev.clone()),
                after_subject: None,
            });
            s.updated += 1;
        }
    }

    for m in actual {
        let key = m.external_id.to_string();
        if wanted.contains(&key) {
            continue;
        }
        match &m.external_id {
            ExternalId::K2Owner => {}
            ExternalId::Local(_) => s.ignored_local += 1,
            ExternalId::Other(_) => s.ignored_other += 1,
            ExternalId::K2Principal(_) | ExternalId::K2Workspace(_) => {
                if m.deactivated {
                    continue;
                }
                if busy.contains(&key) {
                    s.skipped_busy += 1;
                    continue;
                }
                out.push(deprovision(&m.external_id, None));
                s.deprovisioned += 1;
            }
        }
    }
    (out, s)
}
