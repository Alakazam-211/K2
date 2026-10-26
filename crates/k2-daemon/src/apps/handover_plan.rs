//! The plan of an App handover (prd-app-handoff-v1 rev 2 §4.2): what
//! would move from workspace A to workspace B, and how, computed from live
//! rows. Read only.
//!
//! The plan is what the owner's card shows. Its [`Plan::hash`] covers the
//! things the card promises (labels, guests and their classes, secrets and
//! references, the database case, App keys, A's waiting requests, B's
//! switch, the options): the approval recomputes it and refuses a
//! different one (`stale`), so the owner approves exactly what they saw.
//! Volatile readings (free memory, folder-trust scans) are shown but not
//! hashed.
//!
//! Lock order: everything from the daemon DB first, then skin.db, then the
//! vault; never skin → daemon DB.

use std::collections::{BTreeMap, BTreeSet};

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::store::{self, AppRow};
use k2_core::skin::handover as sh;
use serde::Serialize;
use serde_json::{json, Value as J};

use super::routes::{db_err, ws_handle, Ws, R};

// What this build can move (the slices that lift each refusal). Until a
// part moves, a plan that needs it is refused naming what (S1: 0.46.4 never
// breaks an App silently).
/// S3: guests tied to the giving workspace (re-tie + credential retire).
pub(crate) const TIES_MOVE: bool = true;
/// S4: workspace-scope secrets and `.env.k2` references.
pub(crate) const SECRETS_MOVE: bool = false;
/// S4: a database of the giving workspace.
pub(crate) const DB_MOVE: bool = false;

pub const NEEDS_TIE_MOVE: &str = "handover_needs_tie_move";
pub const NEEDS_SECRET_MOVE: &str = "handover_needs_secret_move";
pub const NEEDS_DB_MOVE: &str = "handover_needs_db_move";
pub const DB_SHARED: &str = "handover_db_shared";
pub const DB_BUSY: &str = "handover_db_busy";
pub const VAULT_CLOSED: &str = "handover_vault_closed";

/// The owner's choices on the card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// A database A's other Apps use too: `share` gives B a grant on it
    /// (A keeps the database and its passwords). Anything else refuses.
    pub db: Option<String>,
    /// A secret name B already has: `keep` (default: the App's value as
    /// `NAME__<app>`) or `use_b` (B's own value).
    pub collisions: Option<String>,
    /// Send the shared guests owner reset links (drops their tie).
    pub reset_shared: bool,
    /// Copy the App's source folder into B (S7, opt-in).
    pub copy_source: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelItem {
    pub label: String,
    /// `app` (the App's own) | `taken` (from a legacy service) | `subdomain`.
    pub kind: &'static str,
    /// The workspace holding it now (`None`: no row).
    pub holder: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretItem {
    pub secret_id: String,
    pub name: String,
    /// `server` | `workspace` | `other`.
    pub scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_workspace: Option<String>,
    /// `grant` (granted to the service) | `ref` (an `.env.k2` reference).
    pub via: &'static str,
    /// The env key (`.env.k2` key or the grant's env name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// `untouched` | `move` (re-scoped to B) | `copy` (A uses it too).
    pub case: &'static str,
    /// B already has a secret of that name.
    pub collision: bool,
    /// Something outside this App uses it (another service, a `ws:` grant,
    /// another App's reference).
    pub used_elsewhere: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbItem {
    /// `none` | `unknown` | `sole` | `shared` | `busy` | `other`.
    pub case: &'static str,
    pub name: Option<String>,
    pub owner_workspace: Option<String>,
    /// `ref` | `default`.
    pub source: Option<String>,
    pub other_apps: Vec<String>,
    pub grantees: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyItem {
    pub id: String,
    pub label: Option<String>,
    /// Delivered through the vault (K2 can re-key it); `false` = shown once
    /// to the owner.
    pub vault: bool,
    pub rooms: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    pub code: &'static str,
    pub hint: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub app: AppRow,
    pub to: Ws,
    /// `owner_workspace` now ('' for a parked App).
    pub from: String,
    /// The room the App's guests reach now: `from`, or for a parked App
    /// the workspace it was parked from (A21).
    pub room_from: String,
    pub options: Options,
    pub brief_hash: Option<String>,
    pub labels: Vec<LabelItem>,
    pub domains: Vec<String>,
    pub guests: Vec<sh::HandoverGuest>,
    pub rooms: sh::RoomFootprint,
    pub secrets: Vec<SecretItem>,
    pub refs_unresolved: Vec<String>,
    pub db: DbItem,
    pub keys: Vec<KeyItem>,
    pub requests: Vec<J>,
    pub to_build: bool,
    pub overages: Vec<String>,
    pub folder_held: Option<bool>,
    pub link_open: bool,
    pub link_deliver: bool,
    pub rekey: J,
    pub source: Option<String>,
    pub blockers: Vec<Blocker>,
}

fn sha(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The canonical form of a JSON value (object keys sorted).
fn canonical(v: &J) -> String {
    match v {
        J::Object(m) => {
            let sorted: BTreeMap<&String, &J> = m.iter().collect();
            let parts: Vec<String> = sorted.into_iter().map(|(k, v)| format!("{}:{}", J::String(k.clone()), canonical(v))).collect();
            format!("{{{}}}", parts.join(","))
        }
        J::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

impl Plan {
    /// Count of guests in `class`.
    pub fn count(&self, class: &str) -> usize {
        self.guests.iter().filter(|g| g.class == class).count()
    }

    pub fn names(&self, class: &str) -> Vec<String> {
        self.guests.iter().filter(|g| g.class == class).map(|g| g.username.clone()).collect()
    }

    /// What the card promises (see the module doc).
    fn basis(&self) -> J {
        let mut guests: Vec<(String, &str)> = self.guests.iter().map(|g| (g.principal_id.clone(), g.class)).collect();
        guests.sort();
        let mut labels: Vec<(String, &str, Option<String>)> = self.labels.iter().map(|l| (l.label.clone(), l.kind, l.holder.clone())).collect();
        labels.sort();
        let mut secrets: Vec<(String, &str, &str, bool)> = self.secrets.iter().map(|s| (s.secret_id.clone(), s.via, s.case, s.collision)).collect();
        secrets.sort();
        let mut keys: Vec<String> = self.keys.iter().map(|k| k.id.clone()).collect();
        keys.sort();
        let mut requests: Vec<String> = self.requests.iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
        requests.sort();
        json!({
            "app": self.app.app_id,
            "serviceId": self.app.service_id,
            "from": self.from,
            "roomFrom": self.room_from,
            "to": self.to.id,
            "labels": labels,
            "domains": self.domains,
            "guests": guests,
            "sharedRoles": self.rooms.shared_roles,
            "secrets": secrets,
            "refsUnresolved": self.refs_unresolved,
            "db": [self.db.case, self.db.name.clone().unwrap_or_default().as_str()],
            "keys": keys,
            "requests": requests,
            "toBuild": self.to_build,
            "options": self.options,
            "brief": self.brief_hash,
        })
    }

    /// sha256 of the canonical basis.
    pub fn hash(&self) -> String {
        sha(&canonical(&self.basis()))
    }

    /// The card's lines (§7), each only when it applies.
    pub fn lines(&self) -> Vec<String> {
        let (a, b, app) = (ws_handle(&self.room_from), ws_handle(&self.to.id), self.app.app_id.clone());
        let a = if self.room_from.is_empty() { "the owner (parked)".to_string() } else { a };
        let mut out = vec![format!("Hand over {app} from {a} to {b}: {b} becomes the agent for {app}; {a} stops managing it the moment you approve.")];
        match self.rekey["mode"].as_str() {
            Some("none") | None => out.push("Keeps running. Nothing restarts.".into()),
            Some("bluegreen") => out.push("Keeps running. It restarts once on a fresh copy to change its keys (no outage).".into()),
            Some("pause") => out.push("Keeps running, with a few seconds' pause to change its keys (it can't run two copies).".into()),
            Some("defer") => out.push(format!("Keeps running. Its keys change at {b}'s first deploy: {a} keeps the old ones until then.")),
            Some("next_start") => out.push("It isn't running: its new keys apply at its next start.".into()),
            Some(_) => {}
        }
        let hosts: Vec<String> = self.labels.iter().map(|l| l.label.clone()).collect();
        if !hosts.is_empty() || !self.domains.is_empty() {
            out.push(format!(
                "Addresses: {}{} stay as they are. {b} holds them now.",
                hosts.join(", "),
                if self.domains.is_empty() { String::new() } else { format!(" and {} custom domain(s) ({})", self.domains.len(), self.domains.join(", ")) }
            ));
        }
        let moving = self.guests.len() - self.count(sh::CLASS_SHARED) - self.count(sh::CLASS_AGENT_LOGIN);
        if moving > 0 {
            out.push(format!(
                "Guests: {moving} guest(s) keep their access and stay signed in. From now on they talk to {b}. Their conversations with {a} stay with {a}."
            ));
        }
        let retired = self.guests.iter().filter(|g| (g.class == sh::CLASS_GIVER_TIED || g.class == sh::CLASS_APP_KEY_TIED) && g.has_password).count();
        if retired > 0 {
            out.push(format!(
                "Passwords {a} set: {retired} guest(s)' passwords were set by {a}. They stay signed in, but their passwords are cleared so {a} can't \
                 sign in as them. Each sets a new one when their sign-in ends; {b} can send the link (k2 app user reset), or you can."
            ));
        }
        let shared = self.names(sh::CLASS_SHARED);
        if !shared.is_empty() {
            out.push(format!(
                "Shared guests: {} guest(s) also use {a}'s other Apps and stay {a}'s ({}). They lose {app} until you send them a reset{}.",
                shared.len(),
                shared.join(", "),
                if self.options.reset_shared { ": owner reset links go out now" } else { "" }
            ));
        }
        let logins = self.names(sh::CLASS_AGENT_LOGIN);
        if !logins.is_empty() {
            out.push(format!("{a}'s own logins on {app} ({}) stop working.", logins.join(", ")));
        }
        if !self.rooms.shared_roles.is_empty() {
            out.push(format!(
                "Roles other Apps share ({}) still name {a}'s room: they stay as they are (edit them in Settings → Apps if {app}'s guests should reach {b} through them).",
                self.rooms.shared_roles.join(", ")
            ));
        }
        let moved: Vec<&str> = self.secrets.iter().filter(|s| s.case == "move").map(|s| s.name.as_str()).collect();
        let copied: Vec<&str> = self.secrets.iter().filter(|s| s.case == "copy").map(|s| s.name.as_str()).collect();
        if !moved.is_empty() || !copied.is_empty() {
            let mut l = format!("Secrets: {} move to {b}", moved.len() + copied.len());
            if !moved.is_empty() {
                l.push_str(&format!(": {}", moved.join(", ")));
            }
            if !copied.is_empty() {
                l.push_str(&format!(". {} are shared with {a}'s other work and are copied: {a} still knows them, so rotate them: {}", copied.len(), copied.join(", ")));
            }
            out.push(format!("{l}."));
        }
        let readable: Vec<&str> = self.secrets.iter().filter(|s| s.case != "untouched").map(|s| s.name.as_str()).collect();
        if !readable.is_empty() {
            out.push(format!("Rotate after the handover: {a} could have read {} (K2 can't take back what was seen).", readable.join(", ")));
        }
        match self.db.case {
            "sole" => out.push(format!("Database {} moves to {b}, and its passwords change.", self.db.name.clone().unwrap_or_default())),
            "shared" if self.options.db.as_deref() == Some("share") => out.push(format!(
                "Database {} is shared: {b} gets access to it, and {a} can still read and change this App's data.",
                self.db.name.clone().unwrap_or_default()
            )),
            "shared" => out.push(format!(
                "Database {} is also used by {}: share it (A keeps access to this App's data) or don't hand over yet.",
                self.db.name.clone().unwrap_or_default(),
                self.db.other_apps.iter().chain(self.db.grantees.iter()).cloned().collect::<Vec<_>>().join(", ")
            )),
            "unknown" | "other" => {
                if let Some(n) = &self.db.note {
                    out.push(format!("Database: {n}. {b} must re-home it."));
                }
            }
            _ => {}
        }
        if self.keys.iter().any(|k| k.vault) {
            out.push(format!("Keys: {app}'s keys change. The old ones stop working after the switch."));
        }
        if !self.to_build {
            out.push(format!("Turns on \"Build and run Apps\" for {b} (it's off)."));
        }
        if !self.overages.is_empty() {
            out.push(format!("{b} goes past its default budget: {}.", self.overages.join("; ")));
        }
        if !self.requests.is_empty() {
            out.push(format!("{a}'s {} waiting request(s) for {app} are cancelled.", self.requests.len()));
        }
        if self.folder_held == Some(true) {
            out.push(format!("{b}'s folder isn't trusted yet: {b} can't act until you trust it."));
        }
        if self.options.copy_source {
            if let Some(src) = &self.source {
                out.push(format!("Its source ({src}) is copied into {b}'s folder."));
            }
        }
        out.push(format!("Undo: you can undo this for {} hours (Settings → Apps, or k2 app handover undo {app}).", k2_core::deploy::handover::WINDOW_SECS / 3600));
        out
    }

    /// The plan as the routes answer it. `names`: guest usernames shown
    /// (the owner, an Owner/Admin login, the App's own agent).
    pub fn json(&self, names: bool) -> J {
        let guests: Vec<J> = self
            .guests
            .iter()
            .map(|g| {
                let mut v = json!({ "class": g.class, "hasPassword": g.has_password, "enabled": g.grant_enabled });
                if names {
                    v["username"] = json!(g.username);
                }
                if let Some(owner_made) = g.test_guest {
                    v["testGuest"] = json!(if owner_made { "owner" } else { "agent" });
                }
                v
            })
            .collect();
        let mut by_class: BTreeMap<&str, usize> = BTreeMap::new();
        for g in &self.guests {
            *by_class.entry(g.class).or_default() += 1;
        }
        json!({
            "app": self.app.app_id,
            "serviceId": self.app.service_id,
            "from": { "workspace": self.from, "handle": if self.from.is_empty() { J::Null } else { json!(ws_handle(&self.from)) }, "parked": self.from.is_empty() },
            "roomFrom": if self.room_from.is_empty() { J::Null } else { json!(ws_handle(&self.room_from)) },
            "to": to_json(&self.to),
            "keeps": {
                "state": self.app.state,
                "frontPort": self.app.front_port,
                "liveDeployment": self.app.live_deployment,
                "liveSeed": self.app.live_seed,
            },
            "labels": self.labels,
            "domains": self.domains,
            "guests": { "total": self.guests.len(), "byClass": by_class, "list": guests },
            "rooms": self.rooms,
            "secrets": self.secrets,
            "refsUnresolved": self.refs_unresolved,
            "database": self.db,
            "appKeys": self.keys,
            "requests": self.requests,
            "toSettings": { "build": self.to_build, "overages": self.overages, "folderHeld": self.folder_held },
            "link": { "open": self.link_open, "deliver": self.link_deliver },
            "rekey": self.rekey,
            "source": self.source,
            "options": self.options,
            "blockers": self.blockers,
            "lines": self.lines(),
            "planHash": self.hash(),
        })
    }
}

fn to_json(w: &Ws) -> J {
    let (name, color) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::db::schema::Project::get(&conn, &w.id).map(|p| (p.name, p.color)).unwrap_or_default()
    };
    json!({ "workspace": w.id, "handle": ws_handle(&w.id), "name": name, "color": color, "folder": w.path })
}

/// The workspace a parked App was parked from (its last `park` audit row's
/// `from`), so its guests' rooms move from there (A21).
pub(crate) fn parked_from(app_id: &str) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, app_id, 500)
        .ok()
        .and_then(|rows| rows.into_iter().find(|r| r.event == "park"))
        .and_then(|r| r.payload.get("from").and_then(J::as_str).map(str::to_string))
        .unwrap_or_default()
}

/// Compute the plan for `app` → `to`.
pub(crate) fn compute(app: &AppRow, to: &Ws, options: &Options, brief: Option<&str>) -> R<Plan> {
    let from = app.owner_workspace.clone();
    let room_from = if from.is_empty() { parked_from(&app.app_id) } else { from.clone() };
    let cfg = super::labels::load_cfg(&app.app_id)?;

    // ── daemon DB first ──────────────────────────────────────────────
    let (others_of_a, domains, requests, keys_rows) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let others: Vec<AppRow> = if room_from.is_empty() {
            Vec::new()
        } else {
            store::list_apps(&conn, Some(&room_from)).map_err(db_err)?.into_iter().filter(|a| a.app_id != app.app_id).collect()
        };
        let domains = k2_core::domains::hostnames_for_app(&conn, &app.app_id).map_err(db_err)?;
        let f = reqs::Filter { app_id: Some(&app.app_id), service_id: None, workspace: None, status: Some(reqs::STATUS_PENDING) };
        let requests: Vec<J> = reqs::list(&conn, &f, 200)
            .map_err(db_err)?
            .into_iter()
            .filter(|r| r.body.get("route").and_then(J::as_str) != Some(super::handover::APPLY_ROUTE))
            .map(|r| json!({ "id": r.id, "kind": r.kind, "summary": r.summary, "requestedBy": r.requested_by }))
            .collect();
        drop(conn);
        let keys = k2_core::api_keys::list_app_keys(Some(&app.service_id)).map_err(|e| super::routes::no("db_error", 500, e))?;
        (others, domains, requests, keys)
    };
    let mut labels = vec![LabelItem { label: app.app_id.clone(), kind: "app", holder: super::labels::holder_of(&app.app_id)? }];
    for t in &cfg.labels {
        labels.push(LabelItem { label: t.label.clone(), kind: "taken", holder: super::labels::holder_of(&t.label)? });
    }
    for d in &cfg.subdomains {
        labels.push(LabelItem { label: d.label.clone(), kind: "subdomain", holder: super::labels::holder_of(&d.label)? });
    }
    // Only labels the App's workspace (or nobody, for a parked App) holds
    // move; a label another workspace holds now is listed as it is.
    let keys: Vec<KeyItem> = keys_rows
        .into_iter()
        .filter(|k| k.revoked_at.is_none())
        .map(|k| {
            let g: J = k.grant_json.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(J::Null);
            KeyItem {
                id: k.id,
                label: k.label,
                vault: g.get("vaultSecret").and_then(J::as_str).is_some_and(|s| !s.is_empty()),
                rooms: g.get("rooms").and_then(J::as_array).map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default(),
            }
        })
        .collect();
    let to_build = super::access::level(&to.id) >= super::access::Level::Build;
    let tunnel = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::published_services::get_by_id(&conn, &app.service_id).map_err(db_err)?.is_some_and(|s| s.expose == k2_core::published_services::EXPOSE_TUNNEL)
    };
    let overages = super::limits::install_overages(&to.id, app.memory_limit_bytes, tunnel)?
        .into_iter()
        .filter(|o| !o.starts_with("no memory limit") && !o.starts_with("memory "))
        .collect();
    let db = db_plan(app, &room_from, &others_of_a);

    // ── skin.db ──────────────────────────────────────────────────────
    let other_services: Vec<String> = others_of_a.iter().map(|a| a.service_id.clone()).collect();
    let guests = sh::guests(&app.service_id, &room_from, &other_services).map_err(|e| super::routes::no("skin_error", 500, e))?;
    let rooms = sh::room_footprint(&app.service_id, &room_from).map_err(|e| super::routes::no("skin_error", 500, e))?;

    // ── vault ────────────────────────────────────────────────────────
    let (secrets, refs_unresolved, vault_ok) = secrets_plan(app, &room_from, &to.id, &others_of_a);

    let (link_open, link_deliver) = super::gateway::link_info(&app.app_id).map(|(_, d)| (true, d)).unwrap_or((false, false));
    let rekey = rekey_plan(app, &keys, &db, link_open, link_deliver);
    let folder_held = match k2_core::cli_folder_trust::gate::check(&to.path) {
        k2_core::cli_folder_trust::gate::Outcome::Held(_) | k2_core::cli_folder_trust::gate::Outcome::Denied(_) => Some(true),
        k2_core::cli_folder_trust::gate::Outcome::Granted { .. } => Some(false),
        k2_core::cli_folder_trust::gate::Outcome::Skipped(_) => None,
    };

    let mut plan = Plan {
        app: app.clone(),
        to: to.clone(),
        from,
        room_from,
        options: options.clone(),
        brief_hash: brief.map(sha),
        labels,
        domains,
        guests,
        rooms,
        secrets,
        refs_unresolved,
        db,
        keys,
        requests,
        to_build,
        overages,
        folder_held,
        link_open,
        link_deliver,
        rekey,
        source: app.folder_path.clone(),
        blockers: Vec::new(),
    };
    plan.blockers = blockers(&plan, vault_ok);
    Ok(plan)
}

/// What this build can't move yet, or what the owner must choose first.
fn blockers(p: &Plan, vault_ok: bool) -> Vec<Blocker> {
    let mut out = Vec::new();
    let (a, app) = (ws_handle(&p.room_from), p.app.app_id.as_str());
    if !TIES_MOVE {
        let mut tied = p.names(sh::CLASS_GIVER_TIED);
        tied.extend(p.names(sh::CLASS_APP_KEY_TIED));
        tied.extend(p.names(sh::CLASS_SHARED));
        if !tied.is_empty() {
            out.push(Blocker {
                code: NEEDS_TIE_MOVE,
                hint: format!(
                    "{} guest(s) of {app} are tied to {a} (its agent or the App's key set or could know their password): this K2 can't move them \
                     without signing them out. Reset them first (k2 app user reset, owner), or wait for the full handover",
                    tied.len()
                ),
                names: tied,
            });
        }
    }
    if !vault_ok {
        out.push(Blocker {
            code: VAULT_CLOSED,
            hint: "the vault isn't open: K2 can't see which secrets the App uses. Open it (k2 vault status), then ask again".into(),
            names: Vec::new(),
        });
    }
    if !SECRETS_MOVE {
        let mut names: Vec<String> = p.secrets.iter().filter(|s| s.case != "untouched").map(|s| s.name.clone()).collect();
        names.extend(p.refs_unresolved.iter().cloned());
        names.sort();
        names.dedup();
        if !names.is_empty() {
            out.push(Blocker {
                code: NEEDS_SECRET_MOVE,
                hint: format!("{app} uses {a}'s own secrets ({}): this K2 can't move them yet; they would stop resolving at its next start", names.join(", ")),
                names,
            });
        }
    }
    match p.db.case {
        "sole" if !DB_MOVE => out.push(Blocker {
            code: NEEDS_DB_MOVE,
            hint: format!(
                "{app} uses {a}'s database {}: this K2 can't move a database yet; at its next start the App would lose it",
                p.db.name.clone().unwrap_or_default()
            ),
            names: p.db.name.iter().cloned().collect(),
        }),
        "shared" if !DB_MOVE || p.options.db.as_deref() != Some("share") => out.push(Blocker {
            code: if DB_MOVE { DB_SHARED } else { NEEDS_DB_MOVE },
            hint: format!(
                "{app}'s database {} is also used by {}: {}",
                p.db.name.clone().unwrap_or_default(),
                p.db.other_apps.iter().chain(p.db.grantees.iter()).cloned().collect::<Vec<_>>().join(", "),
                if DB_MOVE { "choose share (A keeps access to this App's data; --db share) or wait for per-App databases" } else { "this K2 can't move a database yet" }
            ),
            names: p.db.other_apps.clone(),
        }),
        "busy" => out.push(Blocker {
            code: DB_BUSY,
            hint: format!("{app}'s database {} is being upgraded or dual-run: hand over after that ends", p.db.name.clone().unwrap_or_default()),
            names: p.db.name.iter().cloned().collect(),
        }),
        _ => {}
    }
    out
}

fn db_item(case: &'static str) -> DbItem {
    DbItem { case, name: None, owner_workspace: None, source: None, other_apps: Vec::new(), grantees: Vec::new(), note: None }
}

/// §4.6: which database case the App is.
fn db_plan(app: &AppRow, room_from: &str, others_of_a: &[AppRow]) -> DbItem {
    use super::pg_choice::{DbOf, Source};
    match super::pg_choice::db_of(app) {
        DbOf::None => db_item("none"),
        DbOf::Unknown(why) => DbItem { note: Some(why), ..db_item("unknown") },
        DbOf::Known(r) => {
            // The owner workspace's default database, reached only through
            // the fallback (no DATABASE_URL): the App gets no DSN, so it
            // uses no database unless its manifest asks for Postgres.
            if r.source == Source::Default && super::pg_choice::range_of(app).is_none() {
                return db_item("none");
            }
            let mut item = DbItem {
                case: "other",
                name: Some(r.name.clone()),
                owner_workspace: Some(ws_handle(&r.project_id)),
                source: Some(match r.source {
                    Source::Ref => "ref".into(),
                    Source::Default => "default".into(),
                }),
                other_apps: Vec::new(),
                grantees: Vec::new(),
                note: None,
            };
            if r.project_id != room_from || room_from.is_empty() {
                item.note = Some(format!("its database {} belongs to {}, not to the App's workspace: it stays as it is", r.name, ws_handle(&r.project_id)));
                return item;
            }
            let busy = {
                let h = k2_core::db::shared();
                let conn = h.lock();
                let moving: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sql_dual_runs WHERE db = ?1 AND state NOT IN ('switched','aborted','failed')",
                        rusqlite::params![r.name],
                        |x| x.get(0),
                    )
                    .unwrap_or(0);
                let mv: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sql_moves WHERE db = ?1 AND state IN ('queued','checking','copying','verifying','switching')",
                        rusqlite::params![r.name],
                        |x| x.get(0),
                    )
                    .unwrap_or(0);
                moving + mv > 0
            };
            if busy {
                item.case = "busy";
                return item;
            }
            let users: BTreeSet<String> = super::pg_choice::apps_using(&r.name).into_iter().map(|u| u.app).filter(|a| a != &app.app_id).collect();
            let grantees: Vec<String> = {
                let h = k2_core::db::shared();
                let conn = h.lock();
                let mut st = match conn.prepare(
                    "SELECT g.project_id FROM sql_grants g JOIN sql_databases d ON d.id = g.database_id WHERE d.name = ?1 AND d.status = 'active' ORDER BY g.project_id",
                ) {
                    Ok(s) => s,
                    Err(_) => return item,
                };
                st.query_map(rusqlite::params![r.name], |x| x.get::<_, String>(0)).map(|it| it.flatten().collect()).unwrap_or_default()
            };
            // The default fallback counts as sole only when A has no other
            // App (PRD §4.6).
            let default_shared = r.source == Source::Default && !others_of_a.is_empty();
            item.other_apps = users.into_iter().collect();
            item.grantees = grantees.iter().map(|g| ws_handle(g)).collect();
            item.case = if item.other_apps.is_empty() && grantees.is_empty() && !default_shared { "sole" } else { "shared" };
            if default_shared && item.other_apps.is_empty() {
                item.other_apps = others_of_a.iter().map(|a| a.app_id.clone()).collect();
            }
            item
        }
    }
}

/// §4.5: the App's secrets: granted to the service, and `.env.k2`
/// references in its live Seed. `(items, unresolved refs, vault open)`.
fn secrets_plan(app: &AppRow, room_from: &str, to: &str, others_of_a: &[AppRow]) -> (Vec<SecretItem>, Vec<String>, bool) {
    use k2_vault::{Actor, Consumer, ScopeKind, SecretStore, Selector};
    let mut items: Vec<SecretItem> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let refs: Vec<(String, String)> = match app.live_seed.as_deref() {
        Some(seed) => crate::vault::publish_env::refs_in(&app.service_id, &k2_core::deploy::seed::seed_path(seed)),
        None => Vec::new(),
    };
    let Ok(vault) = crate::vault::get() else {
        return (items, refs.into_iter().map(|(k, _)| k).collect(), false);
    };
    let me = Consumer::published_service(app.service_id.clone());
    // A's other Apps' references to its own secrets (by name).
    let mut a_refs: BTreeSet<String> = BTreeSet::new();
    for o in others_of_a {
        if let Some(seed) = o.live_seed.as_deref() {
            for (_, r) in crate::vault::publish_env::refs_in(&o.service_id, &k2_core::deploy::seed::seed_path(seed)) {
                if let Ok(Selector::Name { scope, name, .. }) = crate::vault::selector_for_ref(&r) {
                    if scope.kind == ScopeKind::Workspace && scope.id == room_from {
                        a_refs.insert(name);
                    }
                }
            }
        }
    }
    let b_has = |name: &str| vault.meta(&Selector::user(k2_vault::Scope::workspace(to), name), &Actor::Owner).is_ok();
    let used_elsewhere = |secret_id: &str, name: &str| -> bool {
        if a_refs.contains(name) {
            return true;
        }
        vault.grants(&Selector::Id(secret_id.to_string()), &Actor::Owner).map(|gs| gs.iter().any(|g| g.consumer != me)).unwrap_or(true)
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    if let Ok(grants) = vault.grants_to(&me) {
        for (g, name) in grants {
            let Ok(meta) = vault.meta(&Selector::Id(g.secret_id.clone()), &Actor::Owner) else { continue };
            seen.insert(g.secret_id.clone());
            let (scope, scope_ws) = match meta.scope.kind {
                ScopeKind::Server => ("server", None),
                ScopeKind::Workspace => ("workspace", Some(meta.scope.id.clone())),
                _ => ("other", None),
            };
            let mine = scope == "workspace" && scope_ws.as_deref() == Some(room_from) && !room_from.is_empty();
            let elsewhere = mine && used_elsewhere(&g.secret_id, &name);
            items.push(SecretItem {
                secret_id: g.secret_id,
                name: name.clone(),
                scope,
                scope_workspace: scope_ws.map(|w| ws_handle(&w)),
                via: "grant",
                key: g.env_name.or(g.file_name),
                case: if !mine { "untouched" } else if elsewhere { "copy" } else { "move" },
                collision: mine && b_has(&name),
                used_elsewhere: elsewhere,
            });
        }
    }
    for (key, r) in refs {
        let sel = match crate::vault::selector_for_ref(&r) {
            Ok(s) => s,
            Err(_) => {
                unresolved.push(key);
                continue;
            }
        };
        let Selector::Name { scope, name, .. } = &sel else { continue };
        if scope.kind != ScopeKind::Workspace || scope.id != room_from || room_from.is_empty() {
            continue;
        }
        let Ok(meta) = vault.meta(&sel, &Actor::Owner) else {
            unresolved.push(key);
            continue;
        };
        if !seen.insert(meta.id.clone()) {
            continue;
        }
        let elsewhere = used_elsewhere(&meta.id, name);
        items.push(SecretItem {
            secret_id: meta.id,
            name: name.clone(),
            scope: "workspace",
            scope_workspace: Some(ws_handle(room_from)),
            via: "ref",
            key: Some(key),
            case: if elsewhere { "copy" } else { "move" },
            collision: b_has(name),
            used_elsewhere: elsewhere,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name).then(a.secret_id.cmp(&b.secret_id)));
    (items, unresolved, true)
}

/// §4.10: does the handover re-key, and how.
fn rekey_plan(app: &AppRow, keys: &[KeyItem], db: &DbItem, link_open: bool, link_deliver: bool) -> J {
    let vault_keys = keys.iter().filter(|k| k.vault).count();
    let db_rotates = db.case == "sole";
    let link_restart = link_open && !link_deliver;
    let needed = vault_keys > 0 || db_rotates || link_restart;
    let running = app.state == store::STATE_RUNNING && app.live_seed.is_some();
    let mode = if !needed {
        "none"
    } else if !running {
        "next_start"
    } else if super::rehost::no_overlap_shape_live(app).is_some() {
        if app.rehost_when_no_overlap == "defer" {
            "defer"
        } else {
            "pause"
        }
    } else {
        "bluegreen"
    };
    json!({ "needed": needed, "mode": mode, "keys": vault_keys, "database": db_rotates, "link": link_restart })
}
