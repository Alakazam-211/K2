//! Per-agent App permissions (`prd-agent-app-permissions-v1` rev 2,
//! slices A, C and E; 0.46.3, Rosson's answers locked 2026-10-20).
//!
//! **Two nested switches per agent workspace** (§2; migration 0197):
//!
//! | | Off | Switch 1 "Build and run Apps" | Switch 2 "…and manage them end to end" |
//! |---|---|---|---|
//! | Reads (own Apps) | yes | yes | yes |
//! | build, install (own workspace), deploy, rollback, stop/start/restart, exec/shell, link rotate, uninstall without purge (N2) | a held `manage` request (S42) | at once | at once |
//! | App users: guests, roles, tokens (skin hooks) | no | yes | yes |
//! | Budgets (memory, CPU weight, disk) inside the default limits (`limits.rs`: 2 GiB memory per App, CPU weight 100, 10 GiB disk across its Apps) | proposals held | **applied** | applied |
//! | Raises: budgets past the default limits (or `none`), tasks, `io`, `dbPrivate`, quota raises, egress hosts, box services, App keys, clean mode off | proposals held | proposals held | **applied by the agent itself**, no Approvals queue |
//! | Narrowing (suspend, `*/rm`, `*/revoke`, clean mode on, lowering) | yes | yes | yes |
//! | Loosening the wall (§2.2 + N3) | held for the person | held for the person | **held for the person** |
//!
//! Rosson's shape (2026-10-20): switch 1 builds and runs Apps inside the
//! default limits; switch 2 manages caps and budgets. Every memory budget
//! that grows meets box admission (`admission::check_memory`) from every
//! caller, now and when a held request is approved later.
//!
//! Switch 1 is the old "Allow this agent to manage Apps" column
//! (`agents_can_manage_skin`); switch 2 is `agents_can_manage_apps_full`
//! and only counts while switch 1 is on. New workspaces start with both
//! off (Q3 = A; the hire API and templates may preset switch 1, never
//! switch 2). Only the server's owner raises them; an Admin may only lower
//! (Q2); an agent never changes its own ([`set_switches`], the one writer).
//!
//! **Loosening the wall is always person-present** ([`is_loosening`],
//! [`LOOSENING_KINDS`], [`LOOSENING_CAPS`]): `isolation` lowered (to
//! `process`, or `vm` → `container`), `risk` high → normal, `dbRaw` on, the
//! open path (`path=open`, extra ports or UDP, broad egress, a new agent
//! `k2 publish run --cmd` service), named Linux capabilities, `--overcommit`
//! past box admission, foreign Seeds, the caps `files:agentconfig` and
//! `heartbeats:write` (N3), and purge. These are held from **every**
//! caller, the owner token included, and applied only when the owner
//! approves in person with a fresh vault confirm (`super::approvals`). The
//! "…and publish open Apps" standing approval is retired (N1): Apps already
//! on the open path keep running; further open-path moves wait for the
//! person, one request at a time.
//!
//! **Not covered by switch 2** (N4): an App's mail sends stay held for the
//! owner (MCS design) when the Mail core lands. Cores (slice B) decide
//! which caps a switch-1 agent's Apps may use; the agents holding a core's
//! *admin* switch (`db_agent_access`, `mail_manage`, `chat_manage`) are the
//! ones that provision slices (a mailbox, a Sew space, a database) for
//! App-building agents and approve reach-across requests (Rosson
//! 2026-10-20; slice K).
//!
//! **Members are read-only** (Q7, slice C): a Member login sees Apps and
//! changes nothing (`member_read_only`); the route floors of every mutating
//! `/cli/app/*` route are Admin.
//!
//! The checks are daemon-side on the passport (§9): guardrails for a
//! trusted, imperfect colleague, plus a full audit. Until isolation lands
//! an agent that reads the owner token skips the switches, but never the
//! person-present confirms.

use k2_core::app_grace::config::{Class, Op};
use k2_core::app_grace::requests as reqs;
use k2_core::connect_users::Role;
use k2_core::deploy::{self, codes};
use serde_json::{json, Value as J};

use super::routes::{actor, agent_ws, need_credential, no, ws_by_id, ws_handle, Refusal, Who, R};

/// A Member login on a mutating App route (Q7).
pub const MEMBER_READ_ONLY: &str = "member_read_only";
/// The old "…and publish open Apps" switch, asked to turn on (N1).
pub const OPEN_RETIRED: &str = "open_switch_retired";
/// The vault confirm action for `k2 app uninstall --purge` (slice E),
/// bound to the App id.
pub const PURGE_CONFIRM: &str = "app-purge";
/// `confirm_via` on a request an agent with switch 2 applied itself.
pub const SELF_VIA: &str = "switch-2";

/// Request kinds that loosen the wall: never applied by a switch, only by
/// the owner in person (§2.2, N3).
pub const LOOSENING_KINDS: &[&str] = &[reqs::KIND_DB_RAW, reqs::KIND_OVERCOMMIT, reqs::KIND_EGRESS_MODE, reqs::KIND_PUBLISH_RUN];

/// Kinds an agent with switch 2 applies itself (non-loosening widenings).
/// App mail sends (N4) must never join this list.
pub const SELF_APPLY_KINDS: &[&str] = &[reqs::KIND_EGRESS, reqs::KIND_BOX_SERVICE];

/// App caps that reach the agent itself (its config files, its wake
/// prompt): person-present even when their core is ticked (N3). They
/// belong to no core (`app_core_of_cap`), so `super::cores` never
/// pre-approves them: a deploy asking for one is held for the owner.
pub const LOOSENING_CAPS: &[&str] = &[k2_core::skin::CAP_FILES_AGENT_CONFIG, k2_core::skin::CAP_HEARTBEATS_WRITE];

// ── levels ───────────────────────────────────────────────────────────

/// Where a workspace's agent stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Both switches off: reads; every change is held.
    Off,
    /// Switch 1 "Build and run Apps".
    Build,
    /// Switches 1 and 2: "…and manage them end to end".
    Full,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Build => "build",
            Level::Full => "full",
        }
    }
}

/// The level of a workspace's agent (unknown workspace: `Off`).
pub fn level(project_id: &str) -> Level {
    match k2_core::workspace::settings::agents_app_switches(project_id) {
        (true, true) => Level::Full,
        (true, false) => Level::Build,
        _ => Level::Off,
    }
}

/// The caller's level when it is an agent passport (`None` for humans:
/// the owner token, logins, a plain K2 shell tab).
pub(crate) fn agent_level(who: &Who) -> Option<Level> {
    agent_ws(who).map(level)
}

/// The server's owner in person or a human in a plain K2 shell tab (NT5):
/// applies an owner-only widening that doesn't loosen the wall.
pub(crate) fn owner_human(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner, .. })
}

/// An agent that applies non-loosening widenings itself: switch 2 on, or
/// the replay of a `manage` request the owner approved for this exact call.
pub(crate) fn self_widens(who: &Who) -> bool {
    agent_ws(who).is_some() && (agent_level(who) == Some(Level::Full) || super::approvals::manage_approved_now())
}

// ── gates ────────────────────────────────────────────────────────────

/// A Member login changes nothing on an App (Q7).
pub(crate) fn member_read_only(who: &Who) -> R<()> {
    if matches!(who, Who::Login { role: Role::Member, .. }) {
        return Err(no(
            MEMBER_READ_ONLY,
            403,
            "Members can see Apps but not change them: ask an Admin or the owner",
        ));
    }
    Ok(())
}

/// Switch 1 for agents ("Build and run Apps"); humans pass, Members are
/// read-only. The replay of an approved `manage` request passes.
pub(crate) fn need_build(who: &Who) -> R<()> {
    need_credential(who)?;
    member_read_only(who)?;
    let Some(ws) = agent_ws(who) else { return Ok(()) };
    if super::approvals::manage_approved_now() {
        return Ok(());
    }
    if ws_by_id(ws).is_none() {
        return Err(no(codes::FORBIDDEN, 403, "your workspace is not registered"));
    }
    if level(ws) == Level::Off {
        return Err(no(
            codes::APPS_MANAGE_OFF,
            403,
            "\"Build and run Apps\" is off for this agent: ask your owner to turn it on (Workspace settings → Agent → Apps, \
             or k2 app access <workspace> --build on from a terminal outside K2)",
        ));
    }
    Ok(())
}

/// Is a config op on the person-present list (§2.2)?
pub(crate) fn is_loosening(op: &Op, class: Class) -> bool {
    match class {
        Class::PerEvent | Class::Open => true,
        Class::Owner => matches!(op, Op::Isolation(_) | Op::Risk(_)),
        Class::Narrow | Class::Budget => false,
    }
}

pub(crate) const WHY_LOOSEN: &str = "it loosens the App's wall: the owner confirms it in person (Touch ID or a password), from every caller, \
                          the owner token included";
const WHY_RAISE: &str = "a raise past what \"Build and run Apps\" covers (its default limits size memory, CPU weight and disk): an \
                         agent with \"…and manage them end to end\" makes it itself; otherwise the owner approves it";
const WHY_OWNER: &str = "the owner's (or an agent with \"…and manage them end to end\")";

/// May this caller make one config change now? `Err(reason)` = held.
///
/// `within_limits`: the caller is a switch-1 agent bound by its default
/// App limits (`limits::bound`), the op is a budget those limits size
/// (`limits::covers`) and it fits them (`limits::config_overage` found
/// nothing). Such a budget applies at once; a raise past the limits is
/// held by the caller before it gets here.
pub(crate) fn config_rule(who: &Who, op: &Op, class: Class, within_limits: bool) -> Result<(), &'static str> {
    if class == Class::Narrow {
        return Ok(());
    }
    if is_loosening(op, class) {
        return Err(WHY_LOOSEN);
    }
    match class {
        // Humans (owner, Admin, shell tab) apply budgets as before; an
        // agent with switch 2 applies its own; switch 1 applies a budget
        // inside its default limits (Rosson 2026-10-20).
        Class::Budget if agent_ws(who).is_none() || self_widens(who) || within_limits => Ok(()),
        Class::Budget => Err(WHY_RAISE),
        Class::Owner if owner_human(who) || self_widens(who) => Ok(()),
        _ => Err(WHY_OWNER),
    }
}

// ── switch 2: apply a filed widening at once ─────────────────────────

/// An agent with switch 2 applies a non-loosening request it just filed:
/// decided `approved` by the agent itself (`confirm_via = switch-2`),
/// carried out like an owner approval, audited, never on the person list.
pub(crate) fn self_apply(r: &reqs::AppRequest, who: &Who) -> R<J> {
    if !self_widens(who) || !SELF_APPLY_KINDS.contains(&r.kind.as_str()) || LOOSENING_KINDS.contains(&r.kind.as_str()) {
        return Err(no(codes::FORBIDDEN, 403, "only an agent with \"…and manage them end to end\" applies its own widenings"));
    }
    super::approvals::apply_as(r, &actor(who), SELF_VIA)
}

// ── the one writer for the switches ──────────────────────────────────

/// What a caller asks to change.
#[derive(Debug, Clone, Copy, Default)]
pub struct SwitchChange {
    pub build: Option<bool>,
    pub full: Option<bool>,
}

/// Set a workspace's App switches: the owner raises, an Admin only lowers
/// (Q2; the caller has already refused agents and Members). Switch 1 off
/// turns switch 2 off; switch 2 on needs switch 1. A change writes a
/// deploy-audit row per App the workspace maintains and leaves the agent
/// an inbox note (no wake); the route writes the Access Audit line.
pub(crate) fn set_switches(ws_id: &str, change: SwitchChange, caller_is_owner: bool, by: &str) -> R<J> {
    let ws = ws_by_id(ws_id).ok_or_else(|| no("workspace_not_found", 404, format!("no workspace '{ws_id}'")))?;
    let before = level(&ws.id);
    let (b0, f0) = (before >= Level::Build, before == Level::Full);
    let build = change.build.unwrap_or(b0);
    let mut full = change.full.unwrap_or(f0);
    // Raising anything is the owner's, checked first (an Admin asking for
    // switch 2 hears owner_only, never a hint to raise switch 1).
    let asks_raise = (change.build == Some(true) && !b0) || (change.full == Some(true) && !f0);
    if asks_raise && !caller_is_owner {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner turns an agent's App switches on (an Admin may turn them off): Settings → Workspaces → \
             <agent> → Agent, or k2 app access <ws> --build on|--manage on from a terminal outside K2",
        ));
    }
    if change.full == Some(true) && !build {
        return Err(no(
            "parent_switch_off",
            409,
            "turn on \"Build and run Apps\" first: \"…and manage them end to end\" sits under it (k2 app access <ws> --build on --manage on)",
        ));
    }
    if !build {
        full = false;
    }
    let raises = (build && !b0) || (full && !f0);
    if raises && !caller_is_owner {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner turns an agent's App switches on (an Admin may turn them off): Settings → Workspaces → \
             <agent> → Agent, or k2 app access <ws> --build on|--manage on from a terminal outside K2",
        ));
    }
    if build != b0 {
        k2_core::workspace::settings::set_agents_can_manage_skin(&ws.path, build).map_err(|e| no("workspace_not_found", 404, e))?;
    }
    if full != f0 {
        k2_core::workspace::settings::set_agents_can_manage_apps_full(&ws.id, full).map_err(|e| no("parent_switch_off", 409, e))?;
    }
    let after = level(&ws.id);
    let handle = ws_handle(&ws.id);
    if after != before {
        changed(&ws.id, &handle, before, after, by);
    }
    Ok(json!({
        "ok": true,
        "workspace": handle,
        "workspaceId": ws.id,
        "agentsCanBuildApps": after >= Level::Build,
        "agentsCanManageAppsFull": after == Level::Full,
        // The pre-0.46.3 names, for callers that read them.
        "agentsCanManageApps": after >= Level::Build,
        "agentsCanPublishOpenApps": k2_core::workspace::settings::agents_can_publish_open_apps(&ws.id),
        "level": after.as_str(),
        "was": before.as_str(),
        "changed": after != before,
    }))
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// A core's name in notes and hints.
pub(crate) fn core_name(c: &str) -> &str {
    match c {
        "db" => "Database",
        "files" => "Files",
        "thread" => "Thread",
        "mail" => "Mail",
        "sew" => "Sew",
        other => other,
    }
}

/// The audit rows, the settings event and the agent's note for a change.
fn changed(ws_id: &str, handle: &str, before: Level, after: Level, by: &str) {
    let apps = audit_apps(ws_id, by, json!({ "workspace": handle, "from": before.as_str(), "to": after.as_str() }));
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncProjects, J::Null);
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        json!({ "apps": "access", "workspace": handle, "level": after.as_str(), "appIds": apps }),
    );
    tell_agent(ws_id, handle, by);
}

/// One `access_changed` deploy-audit row per App the workspace maintains
/// (the switches and the cores share it). Returns the App ids.
pub(crate) fn audit_apps(ws_id: &str, by: &str, payload: J) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ids: Vec<String> =
        deploy::store::list_apps(&conn, Some(ws_id)).map(|a| a.into_iter().map(|r| r.app_id).collect()).unwrap_or_default();
    for app in &ids {
        if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app, "access_changed", by, None, &payload) {
            k2_core::log_debug!("[apps] audit {app} access_changed: {e}");
        }
    }
    ids
}

/// The agent's inbox note (no wake) with its App access as it is now:
/// both switches and the ticked cores (prd §5 "Audit and note").
pub(crate) fn tell_agent(ws_id: &str, handle: &str, by: &str) {
    let now = level(ws_id);
    let ticked = k2_core::workspace::settings::agents_app_cores(ws_id);
    let cores: Vec<String> = ticked.iter().map(|c| core_name(c).to_string()).collect();
    let later = ticked.iter().any(|c| !super::cores::enforced(c));
    let text = format!(
        "Your App access changed: Build and run Apps {}, manage them end to end {}, manage their users {}; cores: {} (by {by}).\n\n\
         With Build and run Apps you build, install, deploy, run and uninstall your own Apps and edit their roles and App \
         passes; with manage end to end you also raise their budgets, quotas, egress hosts, box services and keys, resume an \
         App you suspended, send push and move your own labels yourself; with manage their users you add and remove their \
         App users and test guests, set passwords, email and names, unlock them, give roles and your own room, invite \
         people and give your App the guests-rooms grant in your own room. A ticked core lets your Apps use that core \
         inside their own slice without waiting for the owner. Anything that loosens an App's wall still waits for the \
         owner in person.{} Check with k2 app access {handle}.",
        on_off(now >= Level::Build),
        on_off(now == Level::Full),
        on_off(k2_core::workspace::settings::agents_can_manage_app_users(ws_id)),
        if cores.is_empty() { "none".to_string() } else { cores.join(", ") },
        if later { " Mail and Sew take effect when App mail and Sew for Apps land." } else { "" },
    );
    super::alerts::note(ws_id, "Your App access changed", &text);
}

// ── Q4: the one-time review note ─────────────────────────────────────

/// The agents 0197 kept on switch 1 that the owner hasn't been told about
/// yet: `(workspace id, handle)`.
pub fn pending_review() -> Vec<(String, String)> {
    let ids: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut st) = conn.prepare("SELECT project_id FROM app_access_review WHERE noted_at IS NULL ORDER BY project_id") else {
            return vec![];
        };
        st.query_map([], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect()).unwrap_or_default()
    };
    ids.into_iter().filter(|id| ws_by_id(id).is_some()).map(|id| (id.clone(), ws_handle(&id))).collect()
}

/// The review list as `/cli/app/list` shows it to humans (Settings →
/// Apps reads it): every reviewed or pending row.
pub fn review_json() -> J {
    let rows: Vec<(String, Option<i64>)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut st) = conn.prepare("SELECT project_id, noted_at FROM app_access_review ORDER BY project_id") else {
            return json!([]);
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()).unwrap_or_default()
    };
    json!(rows
        .into_iter()
        .filter(|(id, _)| ws_by_id(id).is_some())
        .map(|(id, at)| json!({ "workspace": id, "handle": ws_handle(&id), "level": level(&id).as_str(), "notedAt": at }))
        .collect::<Vec<_>>())
}

/// The owner (or an Admin) reviewed this agent: it leaves the review list
/// and the Settings → Apps banner (`agent-access/set {toggle:
/// "app_review", value: false}`). Returns whether a row was removed.
pub fn review_done(project_id: &str) -> bool {
    let n = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM app_access_review WHERE project_id = ?1", rusqlite::params![project_id]).unwrap_or(0)
    };
    if n > 0 {
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "apps": "access-review", "workspace": project_id }));
    }
    n > 0
}

/// Daemon boot: tell the owner once which agents kept "Build and run
/// Apps" from the old switch (one review ticket for the owner and admins),
/// then stamp the rows. A failed note is retried at the next boot.
pub fn review_note_once() -> Option<String> {
    let pending = pending_review();
    if pending.is_empty() {
        return None;
    }
    let handles: Vec<&str> = pending.iter().map(|(_, h)| h.as_str()).collect();
    let title = "Review which agents can build and run Apps";
    let body = format!(
        "These agents could manage Apps before; they now have Build and run Apps on: {}.\n\n\
         Review each one (Settings → Workspaces → <agent> → Agent → Apps, or k2 app access <workspace> --build off \
         from a terminal outside K2). \"…and manage them end to end\" starts off for all of them, and anything that \
         loosens an App's wall still waits for you in person.",
        handles.join(", ")
    );
    let item = match crate::feedback_routes::file_owner_notice(&pending[0].0, "K2", title, &body) {
        Ok(i) => i.id,
        Err(e) => {
            k2_core::log_debug!("[apps] access review note: {e}");
            return None;
        }
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (id, _) in &pending {
            let _ = conn.execute(
                "UPDATE app_access_review SET noted_at = ?2 WHERE project_id = ?1 AND noted_at IS NULL",
                rusqlite::params![id, deploy::now()],
            );
        }
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "appsAccessReview": handles }));
    Some(item)
}

// ── slice E: purge needs the person ──────────────────────────────────

/// `uninstall --purge`: the server's owner (the owner token or an Owner
/// login) with a fresh vault confirm bound to the App (`app-purge`), every
/// time. A shell tab can't pass a confirm; agents never purge.
pub(crate) fn purge_confirm(who: &Who, app_id: &str, token: Option<&str>) -> R<()> {
    let stamp = match who {
        Who::Owner => "owner".to_string(),
        Who::Login { username, role: Role::Owner } => format!("connect:{username}"),
        _ => {
            return Err(no(
                codes::OWNER_ONLY,
                403,
                "--purge deletes the App's data: only the server's owner can, in person, from Settings or a terminal outside K2",
            ))
        }
    };
    let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
        return Err(Refusal {
            extra: Some(json!({ "action": PURGE_CONFIRM, "app": app_id })),
            ..no(
                super::egress_routes::codes::VAULT_CONFIRM_REQUIRED,
                403,
                "--purge deletes the App's data: it needs a fresh owner confirm (Touch ID, your login or the vault password). \
                 k2 app uninstall <app> --purge asks for it",
            )
        });
    };
    super::approvals::vault()?
        .confirm
        .take(token, PURGE_CONFIRM, Some(app_id), &stamp)
        .map_err(|_| no(super::egress_routes::codes::VAULT_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for purging this App, within 60 seconds"))?;
    Ok(())
}
