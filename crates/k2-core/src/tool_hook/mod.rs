//! Generic grant hook (Sew S27): when a K2 grant for a hooked tool is
//! created, changed or deleted, the daemon makes the tool match.
//!
//! **Decisions live here, IO lives in the daemon.** Everything in this
//! module is sync and runs over the `skin.db` connection, so every writer
//! of `skin.db` (all of them go through [`crate::skin`]) produces the
//! outbox row in the SAME transaction as the grant write. The daemon's
//! `tool_hook_worker` is the only reader that talks to the tool (Sew's
//! mgmt socket), with retries and a stable `Idempotency-Key`.
//!
//! | Module | Holds |
//! |---|---|
//! | [`sew_wire`] | Sew mgmt wire types (TEMP mirror of `sew-proto`, K1 swaps it) |
//! | [`outbox`] | `tool_outbox` / `tool_targets` DDL and row ops |
//! | [`chat`] | the `chat` provisioner: scope grammar, validation, op planning |
//! | [`reconcile`] | the pure K2-grants ⇄ Sew-members diff |
//!
//! Rules this module keeps:
//! - **Rev 147:** a Sew account comes only from a `chat` grant (or the box
//!   owner `k2:owner` when the target is enabled). Connect users — owners
//!   and admins included — are never provisioned by role. App Space
//!   membership (K31) will come only from that App's `kind=app` grants.
//! - **HC2:** a subject Sew answers as `skipped` (banned) is recorded as
//!   `held` and is not re-added by reconcile until its grant changes.
//! - **KM13:** the `actor` stored on a row and sent as `Sew-Actor` is
//!   attribution only ("reported by K2"), never proof a person acted.
//! - Workspace names/handles, the owner name and project deletes live in
//!   the main DB, so they cannot share the skin.db transaction: they call
//!   [`kick_reconcile`] and the sweep fixes the drift.

pub mod chat;
pub mod outbox;
pub mod reconcile;
pub mod sew_wire;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use rusqlite::Connection;
use serde::Serialize;

use crate::skin::{SkinGrant, SkinGrantWrite, SkinPrincipal};

/// The grant kind the Sew provisioner owns.
pub const CHAT_KIND: &str = "chat";
/// One Sew install per box: every chat grant targets `sew`.
pub const CHAT_TARGET_ID: &str = "sew";
/// Sew's mgmt socket (C18). Part of the target config.
pub const DEFAULT_MGMT_SOCKET: &str = "/run/sew/mgmt.sock";
/// Sew's mgmt token file (C18, G13). K2 reads it; never copies it.
pub const DEFAULT_MGMT_TOKEN_FILE: &str = "/etc/sew/mgmt.token";

/// Actor on rows the daemon writes on its own.
pub const ACTOR_RECONCILE: &str = "system:reconcile";
pub const ACTOR_BOOT: &str = "system:boot";
/// Actor for a core write whose caller did not name one.
pub const ACTOR_SYSTEM: &str = "system:k2";

/// Error when a chat grant is made before Sew is enabled (§15).
pub const CHAT_NOT_ENABLED: &str = "chat is not enabled on this box: enable Sew first";

/// The op column. Stays unconstrained TEXT in SQL (S27-A12): a newer
/// daemon's op must never make an older worker block a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Provision,
    Update,
    Deprovision,
    OwnerSet,
    /// Reserved for S28 (`POST /mgmt/v1/sessions/revoke`). S27 never
    /// enqueues it and the S27 worker never delivers it (nor blocks it).
    SewSessionRevoke,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Provision => "provision",
            Op::Update => "update",
            Op::Deprovision => "deprovision",
            Op::OwnerSet => "owner_set",
            Op::SewSessionRevoke => "sew_session_revoke",
        }
    }

    pub fn parse(raw: &str) -> Option<Op> {
        Some(match raw {
            "provision" => Op::Provision,
            "update" => Op::Update,
            "deprovision" => Op::Deprovision,
            "owner_set" => Op::OwnerSet,
            "sew_session_revoke" => Op::SewSessionRevoke,
            _ => return None,
        })
    }

    /// The ops the S27 worker delivers.
    pub const DELIVERED: [Op; 4] = [Op::Provision, Op::Update, Op::Deprovision, Op::OwnerSet];

    pub fn delivered_by_s27(self) -> bool {
        Self::DELIVERED.contains(&self)
    }
}

/// Why a row was enqueued (`cause` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    GrantCreate,
    GrantUpdate,
    GrantDelete,
    GrantEnable,
    GrantDisable,
    PrincipalUpdate,
    PrincipalDelete,
    WorkspaceRemoved,
    TargetEnable,
    InstallChanged,
    Reconcile,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::GrantCreate => "grant_create",
            Cause::GrantUpdate => "grant_update",
            Cause::GrantDelete => "grant_delete",
            Cause::GrantEnable => "grant_enable",
            Cause::GrantDisable => "grant_disable",
            Cause::PrincipalUpdate => "principal_update",
            Cause::PrincipalDelete => "principal_delete",
            Cause::WorkspaceRemoved => "workspace_removed",
            Cause::TargetEnable => "target_enable",
            Cause::InstallChanged => "install_changed",
            Cause::Reconcile => "reconcile",
        }
    }
}

/// One op to enqueue.
#[derive(Debug, Clone, PartialEq)]
pub struct NewOp {
    pub tool: &'static str,
    pub op: Op,
    /// External id, e.g. `k2:principal:<uuid>`.
    pub subject_key: String,
    /// The exact JSON body sent to the tool. Serialized once; retries
    /// resend these bytes under the same `Idempotency-Key`.
    pub payload: serde_json::Value,
    pub grant_id: Option<String>,
    /// [`grant_rev`] of the grant this op serves (HC2 "until the grant
    /// changes"). `owner` for the box owner.
    pub grant_rev: Option<String>,
    pub cause: Cause,
    /// Wait until this subject's queue is empty (assistant → person).
    pub after_subject: Option<String>,
}

/// A grant's revision for HC2: any change of id, scope or enabled flag
/// is "the grant changed".
pub fn grant_rev(g: &SkinGrant) -> String {
    format!(
        "{}:{}:{}",
        g.id,
        g.scope.as_deref().unwrap_or(""),
        if g.enabled { 1 } else { 0 }
    )
}

/// The rev of the box owner's implicit chat grant.
pub const OWNER_GRANT_REV: &str = "owner";

/// What happened, for [`ToolProvisioner::plan`].
#[derive(Debug, Clone, Copy)]
pub enum HookEvent<'a> {
    GrantCreated { grant: &'a SkinGrant },
    GrantUpdated { old: &'a SkinGrant, new: &'a SkinGrant },
    GrantDeleted { grant: &'a SkinGrant },
    GrantEnabled { grant: &'a SkinGrant },
    GrantDisabled { grant: &'a SkinGrant },
    /// Email or full name changed.
    PrincipalChanged { principal: &'a SkinPrincipal },
    TargetEnabled,
}

/// A workspace as the hook sees it (read from the main DB + AGENT.md
/// BEFORE the skin.db lock is taken: the skin mutex is not reentrant and
/// the main DB has its own lock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFacts {
    pub id: String,
    pub handle: Option<String>,
    pub name: String,
    /// What K2 shows (AGENT.md `display_name`, else `name`, else
    /// `projects.name`).
    pub display_name: String,
}

/// Facts a hook decision may need from outside skin.db.
#[derive(Debug, Clone, Default)]
pub struct HookCtx {
    /// `owner-token` | `user:<name>` | `agent:<handle>` | `system:*`.
    pub actor: String,
    pub workspaces: HashMap<String, WorkspaceFacts>,
    pub owner_display_name: Option<String>,
}

impl HookCtx {
    /// Load facts for `workspace_ids` (unknown ids are simply absent) and
    /// the owner display name. Call OUTSIDE any skin.db closure.
    pub fn load(actor: &str, workspace_ids: &[String]) -> HookCtx {
        let mut workspaces = HashMap::new();
        for id in workspace_ids {
            if let Some(f) = workspace_facts(id) {
                workspaces.insert(id.clone(), f);
            }
        }
        HookCtx {
            actor: actor.to_string(),
            workspaces,
            owner_display_name: crate::app_settings::load()
                .owner_display_name
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }
}

/// One workspace row from the main DB, or `None` when it is gone.
pub fn workspace_facts(id: &str) -> Option<WorkspaceFacts> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String, String, Option<String>)> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT id, name, path, handle FROM projects WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .ok()
        .flatten()
    };
    let (id, name, path, handle) = row?;
    let display_name = crate::workspace::display::agent_display_name(&path);
    Some(WorkspaceFacts {
        id,
        handle: handle.map(|h| h.trim().to_string()).filter(|h| !h.is_empty()),
        name,
        display_name,
    })
}

/// A tool provisioner, keyed by grant kind. Every method runs inside the
/// skin.db transaction of the write and must use only `conn` (never a
/// public `skin::*` function: `with_conn` is not reentrant).
pub trait ToolProvisioner: Send + Sync {
    /// The grant kind it owns (`chat`).
    fn kind(&self) -> &'static str;
    /// The only target id this kind accepts (filled in when empty).
    fn default_target(&self) -> &'static str;
    /// Validate and canonicalize a create before it is stored.
    fn validate(
        &self,
        conn: &Connection,
        w: &SkinGrantWrite,
        ctx: &HookCtx,
    ) -> Result<SkinGrantWrite, String>;
    /// Validate a scope change; returns the canonical stored scope.
    fn validate_update(
        &self,
        conn: &Connection,
        old: &SkinGrant,
        scope: &str,
        ctx: &HookCtx,
    ) -> Result<String, String>;
    /// Refuse a disable that would strand dependent grants.
    fn validate_disable(&self, conn: &Connection, grant: &SkinGrant) -> Result<(), String>;
    /// Ops to enqueue for an event, in order.
    fn plan(&self, conn: &Connection, ev: &HookEvent<'_>, ctx: &HookCtx)
        -> Result<Vec<NewOp>, String>;
    /// Extra grant rows to delete with this event (assistant cascade).
    fn cascade(&self, conn: &Connection, ev: &HookEvent<'_>) -> Result<Vec<SkinGrant>, String>;
}

static CHAT: chat::ChatProvisioner = chat::ChatProvisioner;
static REGISTRY: [&'static dyn ToolProvisioner; 1] = [&CHAT];

/// v1: `[chat]`. app / mailbox / database / cli have no provisioner and
/// behave exactly as before.
pub fn registry() -> &'static [&'static dyn ToolProvisioner] {
    &REGISTRY
}

pub fn provisioner_for(kind: &str) -> Option<&'static dyn ToolProvisioner> {
    let k = kind.trim().to_ascii_lowercase();
    registry().iter().copied().find(|p| p.kind() == k)
}

// ── wake-ups ─────────────────────────────────────────────────────────

fn notify() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

static RECONCILE_WANTED: AtomicBool = AtomicBool::new(false);

/// Wake the delivery worker (best effort; a no-op permit when nothing
/// waits). Called after every commit that enqueued ops.
pub fn kick() {
    notify().notify_one();
}

/// Ask for a reconcile sweep soon (debounced by the worker). For writes
/// outside skin.db: workspace handle / display name, owner name, project
/// delete, App user changes. Non-blocking; harmless with no worker.
pub fn kick_reconcile() {
    RECONCILE_WANTED.store(true, Ordering::SeqCst);
    notify().notify_one();
}

/// Worker side: wait for a kick.
pub async fn wait_for_kick() {
    notify().notified().await;
}

/// Worker side: consume a pending reconcile request.
pub fn take_reconcile_request() -> bool {
    RECONCILE_WANTED.swap(false, Ordering::SeqCst)
}

/// Peek without consuming (tests).
pub fn reconcile_requested() -> bool {
    RECONCILE_WANTED.load(Ordering::SeqCst)
}

// ── Sew-Actor ────────────────────────────────────────────────────────

/// The `Sew-Actor` header value for a row's actor: attribution only
/// (KM13 §19a — Sew labels it "reported by K2"; it is never person
/// proof). Bytes outside `[A-Za-z0-9_.:@-]` become `_`; capped at 128.
pub fn sew_actor_header(actor: &str) -> String {
    let mut s: String = actor
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if s.is_empty() {
        s = ACTOR_SYSTEM.to_string();
    }
    s
}

// ── the service API (daemon routes + worker) ─────────────────────────

/// Whether `set_target` may enable a target off Linux (tests only).
static ALLOW_OFF_LINUX: AtomicBool = AtomicBool::new(false);

/// Tests: let `set_target(enabled=true)` run on macOS. Never in prod.
#[cfg(any(test, feature = "test-util"))]
pub fn allow_target_off_linux_for_tests(on: bool) {
    ALLOW_OFF_LINUX.store(on, Ordering::SeqCst);
}

fn enabling_allowed_here() -> Result<(), String> {
    if cfg!(target_os = "linux") || ALLOW_OFF_LINUX.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err("Sew runs on Linux only".to_string())
    }
}

/// Upsert a tool target (S29 calls this at the end of enable; the route
/// `POST /cli/skin/grant-hooks/target` too). On the transition to
/// enabled: enqueue `provision k2:owner` + `owner_set k2:owner` and ask
/// for a reconcile. Never a Connect user (rev 147).
pub fn set_target(
    kind: &str,
    enabled: bool,
    socket: Option<&str>,
    token_file: Option<&str>,
    actor: &str,
) -> Result<outbox::ToolTarget, String> {
    let Some(p) = provisioner_for(kind) else {
        return Err(format!("no grant hook for kind '{kind}'"));
    };
    if enabled {
        enabling_allowed_here()?;
    }
    let ctx = HookCtx::load(actor, &[]);
    let now = outbox::now_secs();
    let (target, enqueued) = crate::skin::with_skin_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("grant hook target begin: {e}"))?;
        let (target, transitioned) =
            outbox::upsert_target(&tx, p.kind(), enabled, socket, token_file, now)?;
        let mut n = 0;
        if transitioned {
            let ops = p.plan(&tx, &HookEvent::TargetEnabled, &ctx)?;
            n = outbox::enqueue(&tx, &ops, actor, now)?.len();
        }
        tx.commit()
            .map_err(|e| format!("grant hook target commit: {e}"))?;
        Ok((target, n))
    })?;
    crate::log_debug!(
        "[grant-hook] target {} enabled={} actor={} enqueued={enqueued}",
        target.kind,
        target.enabled,
        sew_actor_header(actor)
    );
    if enabled {
        kick_reconcile();
    }
    kick();
    Ok(target)
}

pub fn get_target(kind: &str) -> Result<Option<outbox::ToolTarget>, String> {
    crate::skin::with_skin_conn(|conn| outbox::get_target(conn, kind))
}

/// Every target. Empty (without creating `skin.db`) on a box that has no
/// skin store yet, so the worker's idle ticks never create the file.
pub fn list_targets() -> Result<Vec<outbox::ToolTarget>, String> {
    if !skin_store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(outbox::list_targets)
}

fn skin_store_exists() -> bool {
    crate::paths::k2_home().join("skin.db").exists()
}

/// S29 (S27-A10): delete every grant of `kind` and mark its pending /
/// blocked outbox rows `skipped`, with no tool call. For uninstall.
pub fn drop_target_grants(kind: &str) -> Result<DropReport, String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("grant hook drop begin: {e}"))?;
        let grants = tx
            .execute("DELETE FROM grants WHERE kind = ?1", rusqlite::params![kind])
            .map_err(|e| format!("grant hook drop grants: {e}"))?;
        let rows = outbox::skip_open_rows(&tx, kind, now, "target dropped")?;
        tx.commit()
            .map_err(|e| format!("grant hook drop commit: {e}"))?;
        Ok(DropReport {
            grants_deleted: grants,
            rows_skipped: rows,
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropReport {
    pub grants_deleted: usize,
    pub rows_skipped: usize,
}

/// S29 waits on this after enable (S27-A10).
pub fn owner_state(kind: &str) -> Result<outbox::OwnerState, String> {
    crate::skin::with_skin_conn(|conn| outbox::owner_state(conn, kind))
}

/// `GET /cli/skin/grant-hooks`.
pub fn status() -> Result<serde_json::Value, String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        let targets = outbox::list_targets(conn)?;
        let summary = outbox::summary(conn, now)?;
        let blocked = outbox::rows_in_state(conn, outbox::STATE_BLOCKED, 100, true)?;
        let held = outbox::rows_in_state(conn, outbox::STATE_HELD, 100, false)?;
        Ok(serde_json::json!({
            "targets": targets,
            "outbox": summary,
            "blocked": blocked,
            "held": held,
            "actorNote": "actor is reported by K2; it is not proof a person acted",
        }))
    })
}

/// `GET /cli/skin/grant-hooks/log`.
pub fn log(limit: usize, subject: Option<&str>) -> Result<Vec<outbox::OutboxRow>, String> {
    crate::skin::with_skin_conn(|conn| outbox::list(conn, limit, subject))
}

/// `POST /cli/skin/grant-hooks/retry` (S27-A11).
pub fn retry(id: &str) -> Result<outbox::OutboxRow, String> {
    let now = outbox::now_secs();
    let r = crate::skin::with_skin_conn(|conn| outbox::retry(conn, id, now))?;
    kick();
    Ok(r)
}

/// `POST /cli/skin/grant-hooks/skip`.
pub fn skip(id: &str) -> Result<outbox::OutboxRow, String> {
    let now = outbox::now_secs();
    let r = crate::skin::with_skin_conn(|conn| outbox::skip(conn, id, now))?;
    kick_reconcile();
    Ok(r)
}

/// Hook state per grant id for `GET /cli/skin/grants` (from the newest
/// outbox row of the grant's subject).
pub fn hook_states(grants: &[SkinGrant]) -> Result<HashMap<String, (String, Option<String>)>, String> {
    let hooked: Vec<&SkinGrant> = grants
        .iter()
        .filter(|g| provisioner_for(&g.kind).is_some())
        .collect();
    if hooked.is_empty() {
        return Ok(HashMap::new());
    }
    crate::skin::with_skin_conn(|conn| {
        let mut out = HashMap::new();
        for g in hooked {
            let Ok(ext) = chat::external_id(&g.subject_kind, &g.subject_id) else {
                continue;
            };
            let state = outbox::subject_hook_state(conn, &g.kind, &ext.to_string())?;
            out.insert(g.id.clone(), state);
        }
        Ok(out)
    })
}

/// Everything a reconcile pass needs from skin.db, read under one lock.
pub fn reconcile_inputs(kind: &str) -> Result<Vec<String>, String> {
    crate::skin::with_skin_conn(|conn| reconcile::workspace_ids_with_grants(conn, kind))
}

/// Apply one reconcile pass for `kind` against the tool's member list:
/// delete grants whose subject is gone, enqueue the planned ops (cause
/// `reconcile`, actor `system:reconcile`), all in one transaction.
pub fn apply_reconcile(
    kind: &str,
    actual: &[sew_wire::MgmtPrincipal],
) -> Result<reconcile::Summary, String> {
    let ws_ids = reconcile_inputs(kind)?;
    let ctx = HookCtx::load(ACTOR_RECONCILE, &ws_ids);
    let now = outbox::now_secs();
    let summary = crate::skin::with_skin_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("reconcile begin: {e}"))?;
        let built = reconcile::build_desired(&tx, &ctx)?;
        let mut orphan_ops = Vec::new();
        for (g, cause) in &built.orphans {
            tx.execute(
                "DELETE FROM grants WHERE id = ?1",
                rusqlite::params![g.id],
            )
            .map_err(|e| format!("reconcile orphan delete: {e}"))?;
            orphan_ops.push(chat::deprovision_op(g, *cause)?);
        }
        let mut busy = outbox::busy_subjects(&tx, kind)?;
        // The orphan deprovisions above are this sweep's own open rows.
        busy.extend(orphan_ops.iter().map(|o| o.subject_key.clone()));
        let held = outbox::held_subjects(&tx, kind)?;
        let (planned, mut summary) = reconcile::plan(&built.desired, actual, &busy, &held);
        summary.orphans_removed = built.orphans.len();
        let mut ops = orphan_ops;
        ops.extend(planned.into_iter().map(|p| p.into_new_op(kind)));
        let ids = outbox::enqueue(&tx, &ops, ACTOR_RECONCILE, now)?;
        let summary_json = serde_json::to_value(&summary).unwrap_or_default();
        outbox::set_target_reconcile(&tx, kind, Some(&summary_json), None, now)?;
        tx.commit().map_err(|e| format!("reconcile commit: {e}"))?;
        crate::log_debug!(
            "[grant-hook] reconcile {kind}: enqueued={} {}",
            ids.len(),
            summary_json
        );
        Ok(summary)
    })?;
    kick();
    Ok(summary)
}

/// A new Sew install id: store it and re-provision the owner (the new
/// install is empty, so the first `owner/set` is allowed, §16.3).
pub fn note_install_id(kind: &str, install_id: &str) -> Result<bool, String> {
    let Some(p) = provisioner_for(kind) else {
        return Err(format!("no grant hook for kind '{kind}'"));
    };
    let ctx = HookCtx::load(ACTOR_RECONCILE, &[]);
    let now = outbox::now_secs();
    let changed = crate::skin::with_skin_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("install id begin: {e}"))?;
        let old = outbox::get_target(&tx, kind)?.and_then(|t| t.install_id);
        outbox::set_target_install_id(&tx, kind, install_id, now)?;
        let changed = matches!(old.as_deref(), Some(o) if o != install_id);
        if changed {
            crate::log_debug!("[grant-hook] Sew install_id changed {} → {install_id}", old.unwrap_or_default());
            let ops: Vec<NewOp> = p
                .plan(&tx, &HookEvent::TargetEnabled, &ctx)?
                .into_iter()
                .map(|mut o| {
                    o.cause = Cause::InstallChanged;
                    o
                })
                .collect();
            outbox::enqueue(&tx, &ops, ACTOR_RECONCILE, now)?;
        }
        tx.commit().map_err(|e| format!("install id commit: {e}"))?;
        Ok(changed)
    })?;
    if changed {
        kick();
    }
    Ok(changed)
}

// ── worker API (the daemon's delivery worker records outcomes here) ──

/// Due rows (head of each subject's queue) for one tool.
pub fn due(kind: &str, limit: usize) -> Result<Vec<outbox::OutboxRow>, String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| outbox::due_heads(conn, kind, now, limit))
}

pub fn record_done(id: &str, status: u16, result: &str) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| outbox::mark_done(conn, id, status as i64, Some(result), now))
}

/// HC2: the tool listed the subject as `skipped`.
pub fn record_held(id: &str, status: u16, result: &str, why: &str) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::log_debug!("[grant-hook] row {id} held: {why}");
    crate::skin::with_skin_conn(|conn| {
        outbox::mark_held(conn, id, status as i64, Some(result), why, now)
    })
}

pub fn record_retry(
    id: &str,
    status: Option<u16>,
    error: &str,
    delay_secs: i64,
) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        outbox::mark_retry(conn, id, status.map(i64::from), error, now + delay_secs.max(0), now)
    })
}

pub fn record_blocked(id: &str, status: Option<u16>, error: &str) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::log_debug!("[grant-hook] row {id} BLOCKED: {error}");
    crate::skin::with_skin_conn(|conn| {
        outbox::mark_blocked(conn, id, status.map(i64::from), error, now)
    })
}

pub fn record_username_retry(
    id: &str,
    payload: &serde_json::Value,
    idem_key: &str,
    status: u16,
) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        outbox::rewrite_payload(conn, id, payload, idem_key, status as i64, now)
    })
}

pub fn set_health(kind: &str, healthy: bool, error: Option<&str>) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        let was = outbox::get_target(conn, kind)?.map(|t| (t.healthy, t.last_error));
        if let Some((was_ok, was_err)) = was {
            if was_ok != healthy || (!healthy && was_err.as_deref() != error) {
                crate::log_debug!(
                    "[grant-hook] target {kind} healthy={healthy} {}",
                    error.unwrap_or("")
                );
            }
        }
        outbox::set_target_health(conn, kind, healthy, error, now)
    })
}

pub fn set_reconcile_error(kind: &str, error: &str) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| outbox::set_target_reconcile(conn, kind, None, Some(error), now))
}

pub fn prune() -> Result<usize, String> {
    if !skin_store_exists() {
        return Ok(0);
    }
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| outbox::prune(conn, now))
}

/// Tests: every pending row due now.
#[cfg(any(test, feature = "test-util"))]
pub fn force_due_for_tests() -> Result<usize, String> {
    crate::skin::with_skin_conn(outbox::force_due_for_tests)
}

#[cfg(test)]
mod tests;
