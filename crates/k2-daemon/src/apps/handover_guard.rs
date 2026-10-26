//! The hand-over guard (prd-app-handoff-v1 S1, H19; the safety slice).
//!
//! `k2 app handover` (and `unpark`, and `release --apps handover:`) moves
//! three rows: the App's owner workspace, its service row and its own URL
//! label. That is correct only for a simple App. Until the full hand-off
//! (0.46.5) it refuses, `409 handoff_needs_v1`, naming every blocker, when
//! the App has anything the move wouldn't carry:
//!
//! - **guests tied to the old workspace** (`principal_agent_ties`, the
//!   creator stamps, an owner-marked agent login): they would stay tied to
//!   a workspace that no longer maintains the App;
//! - **extra labels or domains**: labels it took (`k2 app label take`),
//!   nested labels it serves (`k2 app domain add`), custom hostnames bound
//!   with `--role app:<id>`. Only its own label moves;
//! - **secrets the old workspace owns** (its `ws:` secrets granted to the
//!   App, or `.env.k2` references that resolve only as the old workspace),
//!   **App keys** (their room ceilings name the old workspace) and **a
//!   Postgres database** that isn't the new workspace's;
//! - **push** set up for the App;
//! - **a running App**: its link keeps posting into the old workspace's
//!   Thread until it restarts (stop it first).
//!
//! The owner proves presence with a fresh vault confirm ([`CONFIRM_ACTION`],
//! bound to the App id, or `release:<workspace id>` for a whole release):
//! the owner token alone isn't enough, since agents can read it. Blockers
//! are checked before the confirm, so a refused move never asks for one.

use k2_core::connect_users::Role;
use k2_core::deploy::{codes, store, store::AppRow};
use serde_json::{json, Value as J};

use super::routes::{no, Refusal, Who, R};

/// The vault confirm action (`vault_routes::CONFIRM_ACTIONS`). The token
/// is bound to the App id (or `release:<workspace id>`), works once,
/// within 60 seconds, for every caller (the owner token included).
pub const CONFIRM_ACTION: &str = "app-handover";
pub const NEEDS_V1: &str = "handoff_needs_v1";
pub use super::egress_routes::codes::VAULT_CONFIRM_REQUIRED;

/// One reason the move isn't safe yet.
#[derive(Debug, Clone)]
pub struct Blocker {
    pub code: &'static str,
    pub what: String,
    pub items: Vec<String>,
}

impl Blocker {
    fn json(&self) -> J {
        json!({ "code": self.code, "what": self.what, "items": self.items })
    }
}

fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, role: Role::Owner } => Some(format!("connect:{username}")),
        _ => None,
    }
}

/// The confirm id for a whole release.
pub fn release_confirm_id(ws_id: &str) -> String {
    format!("release:{ws_id}")
}

/// Take a fresh owner confirm bound to `bound` (`what` names the move in
/// the hint). No token: `403 vault_confirm_required` with the action and
/// the id the CLI and Settings mint it for.
pub(crate) fn take_confirm(who: &Who, bound: &str, token: Option<&str>, what: &str) -> R<()> {
    let Some(stamp) = confirm_stamp(who) else {
        return Err(no(codes::OWNER_ONLY, 403, format!("{what} is the server owner's, in person, from Settings or a terminal outside K2")));
    };
    let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
        return Err(Refusal {
            extra: Some(json!({ "action": CONFIRM_ACTION, "secretId": bound })),
            ..no(
                VAULT_CONFIRM_REQUIRED,
                403,
                format!("{what} needs a fresh owner confirm (Touch ID, your login or the vault password), not the owner token alone: k2 app handover asks for it"),
            )
        });
    };
    super::approvals::vault()?
        .confirm
        .take(token, CONFIRM_ACTION, Some(bound), &stamp)
        .map_err(|_| no(VAULT_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this hand-over, within 60 seconds"))?;
    Ok(())
}

/// The workspace a parked App left (its last park's `from`).
pub fn parked_from(app: &AppRow) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, &app.app_id, 200)
        .ok()
        .and_then(|rows| rows.into_iter().find(|r| r.event == "park"))
        .and_then(|r| r.payload.get("from").and_then(J::as_str).map(str::to_string))
        .unwrap_or_default()
}

fn handle(ws: &str) -> String {
    if ws.is_empty() {
        "no workspace".into()
    } else {
        super::routes::ws_handle(ws)
    }
}

/// Everything that makes handing `app` from `from` to `to` unsafe today.
/// A lookup that fails is a blocker too (fail closed), never a pass.
pub(crate) fn blockers(app: &AppRow, from: &str, to: &str) -> R<Vec<Blocker>> {
    let mut out = Vec::new();
    let svc = app.service_id.as_str();

    // A running App's link pass keeps the old workspace's room until it
    // restarts.
    if ![store::STATE_STOPPED, store::STATE_FAILED, store::STATE_PARKED].contains(&app.state.as_str()) {
        out.push(Blocker {
            code: "handoff_app_running",
            what: format!("it is {}: a running App keeps posting into {}'s Thread until it restarts; stop it first (k2 app stop {})", app.state, handle(from), app.app_id),
            items: vec![],
        });
    }

    // Guests tied to the old workspace.
    if !from.is_empty() {
        match k2_core::skin::creators::app_guests_tied_to(svc, from) {
            Ok(g) if !g.is_empty() => out.push(Blocker {
                code: "handoff_needs_tie_move",
                what: format!("{} guest{} tied to {} (added, re-keyed or marked by its agent)", g.len(), if g.len() == 1 { "" } else { "s" }, handle(from)),
                items: g,
            }),
            Ok(_) => {}
            Err(e) => out.push(Blocker { code: "handoff_needs_tie_move", what: format!("K2 couldn't read the App's guests: {e}"), items: vec![] }),
        }
    }

    // Labels and domains beyond its own label.
    let (cfg, hosts) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let cfg = k2_core::app_grace::config::load(&conn, &app.app_id).map_err(super::routes::db_err)?;
        let hosts = k2_core::domains::hostnames_for_app(&conn, &app.app_id).map_err(super::routes::db_err)?;
        (cfg, hosts)
    };
    let mut labels: Vec<String> = cfg.labels.iter().map(|l| format!("{} (taken)", l.label)).collect();
    labels.extend(cfg.subdomains.iter().map(|s| format!("{} (attached)", s.label)));
    labels.extend(hosts.iter().map(|h| format!("{h} (custom domain)")));
    if !labels.is_empty() {
        out.push(Blocker {
            code: "handoff_needs_label_move",
            what: format!("labels or domains beyond its own label {}: only that one moves", app.app_id),
            items: labels,
        });
    }

    // Secrets the old workspace owns, App keys, its database.
    let secrets = secrets_left_behind(app, from, to)?;
    if !secrets.is_empty() {
        out.push(Blocker {
            code: "handoff_needs_secret_move",
            what: format!("secrets {} owns (or that only it can hand the App): the move wouldn't carry them", handle(from)),
            items: secrets,
        });
    }
    let keys: Vec<String> = k2_core::api_keys::list_app_keys(Some(svc))
        .map_err(|e| no("db_error", 500, e))?
        .into_iter()
        .filter(|k| k.revoked_at.is_none() && k.disabled_at.is_none())
        .map(|k| match k.label.filter(|l| !l.trim().is_empty()) {
            Some(l) => format!("{l} ({})", k.id),
            None => k.id,
        })
        .collect();
    if !keys.is_empty() {
        out.push(Blocker {
            code: "handoff_needs_key_move",
            what: format!("live App keys: their rooms are {}'s; revoke them (k2 app key revoke <key-id>) and mint new ones after", handle(from)),
            items: keys,
        });
    }
    match super::pg_choice::db_of(app) {
        super::pg_choice::DbOf::None => {}
        super::pg_choice::DbOf::Known(d) if d.project_id == to && d.source == super::pg_choice::Source::Ref => {}
        super::pg_choice::DbOf::Known(d) => out.push(Blocker {
            code: "handoff_needs_db_move",
            what: format!("its Postgres database is {}'s{}", handle(&d.project_id), if d.source == super::pg_choice::Source::Default { " (the workspace's default)" } else { "" }),
            items: vec![d.name],
        }),
        super::pg_choice::DbOf::Unknown(why) => out.push(Blocker { code: "handoff_needs_db_move", what: format!("K2 can't tell which database it uses: {why}"), items: vec![] }),
    }

    // Push.
    match k2_core::skin::app_push::is_configured(svc) {
        Ok(false) => {}
        Ok(true) => out.push(Blocker { code: "handoff_needs_push_move", what: "push is set up for it (k2 app push)".into(), items: vec![] }),
        Err(e) => out.push(Blocker { code: "handoff_needs_push_move", what: format!("K2 couldn't read its push settings: {e}"), items: vec![] }),
    }
    Ok(out)
}

/// Secret names the move would strand: the old workspace's `ws:` secrets
/// granted to the App, and `.env.k2` references in its live Seed that
/// resolve as the old workspace but not as the new one.
fn secrets_left_behind(app: &AppRow, from: &str, to: &str) -> R<Vec<String>> {
    use k2_vault::{Consumer, Requester, ScopeKind, SecretStore, Selector};
    let vault = super::approvals::vault()?;
    let mut out: Vec<String> = Vec::new();
    let grants = vault.grants_to(&Consumer::published_service(app.service_id.clone())).map_err(|e| no("vault_error", 500, e.hint))?;
    for (g, name) in grants {
        let meta = vault.meta(&Selector::Id(g.secret_id.clone()), &k2_vault::Actor::Owner).map_err(|e| no("vault_error", 500, e.hint))?;
        if meta.scope.kind == ScopeKind::Workspace && !from.is_empty() && meta.scope.id == from {
            out.push(format!("{name} (granted)"));
        }
    }
    let Some(seed) = app.live_seed.as_deref() else { return Ok(out) };
    let cwd = k2_core::deploy::seed::seed_path(seed);
    let env_file = crate::vault::publish_env::get(&app.service_id).env_file.unwrap_or_else(|| ".env.k2".into());
    let Ok(p) = crate::vault_use_routes::jailed_path(&cwd, &env_file) else { return Ok(out) };
    let Ok(text) = std::fs::read_to_string(&p) else { return Ok(out) };
    let ok_for = |sel: &Selector, ws: &str| -> bool {
        let scope_ok = match sel {
            Selector::Name { scope, .. } => match scope.kind {
                ScopeKind::Server => true,
                ScopeKind::Workspace => !ws.is_empty() && scope.id == ws,
                _ => false,
            },
            _ => false,
        };
        scope_ok
            && vault
                .ref_would_resolve(sel, &Requester::PublishedService { id: app.service_id.clone(), workspace_id: ws.to_string() })
                .unwrap_or(false)
    };
    for (k, v) in crate::vault::publish_env::parse_env_k2(&text) {
        if !k2_vault::refs::VaultRef::is_ref(&v) {
            continue;
        }
        // A reference that doesn't resolve today isn't the move's doing.
        let Ok(sel) = crate::vault::selector_for_ref(&v) else { continue };
        if ok_for(&sel, from) && !ok_for(&sel, to) {
            out.push(format!("{k} (.env.k2)"));
        }
    }
    Ok(out)
}

/// `409 handoff_needs_v1`: every blocker, and the way to move the App by
/// hand until 0.46.5.
pub(crate) fn refusal(app: &AppRow, to: &str, blockers: &[Blocker]) -> Refusal {
    let to_h = handle(to);
    let id = &app.app_id;
    let manual = vec![
        format!("install a new App under a new id in {to_h} and deploy the same code (k2 app install <new-id> --workspace {to_h}; k2 app deploy <new-id> <folder>)"),
        format!("copy its data (k2 app data get {id} <path> --to <dir>, then k2 app data put <new-id> <dir>)"),
        format!("with the owner, return each label and take it for the new App (k2 app label return {id} <label>; k2 app label take <new-id> <label>)"),
        format!("repoint each domain (k2 app domain rm {id} <sub> / add <new-id> <sub>; k2 domain name add <host> --role app:<new-id>), then uninstall {id}"),
    ];
    let reasons: Vec<String> = blockers
        .iter()
        .map(|b| if b.items.is_empty() { b.what.clone() } else { format!("{}: {}", b.what, b.items.join(", ")) })
        .collect();
    let hint = format!(
        "{id} can't be handed over safely yet: {}. The full App hand-off arrives in 0.46.5; until then, move it by hand: {}.",
        reasons.join("; "),
        manual.iter().enumerate().map(|(i, s)| format!("{}) {s}", i + 1)).collect::<Vec<_>>().join("; ")
    );
    Refusal {
        extra: Some(json!({ "app": id, "blockers": blockers.iter().map(Blocker::json).collect::<Vec<_>>(), "manual": manual })),
        ..no(NEEDS_V1, 409, hint)
    }
}
