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
//!   and admins included — are never provisioned by role: a Connect login
//!   (`k2:connect:<id>`) becomes a member only through a `connect` chat
//!   grant, written by the owner or by that login's own first chat-pass
//!   mint ([`connect_chat_gate`]). A login linked to the owner is the
//!   owner person: it is `k2:owner` in chat, never a `k2:connect` member
//!   (one person, one member). Provisioning makes an install member
//!   only; App Space membership (K31) will come only from that App's
//!   `kind=app` grants (Space adds stay explicit, rev 134/147).
//! - **HC2:** `skipped` lives on the batch Space/channel member-add
//!   responses (not on provision). The seam is here for K5/K31: a member
//!   recorded `held` is not re-added by reconcile until its grant changes.
//! - **A8:** provision is declarative (absent clears). Profile drift is
//!   repaired with `update`; a re-provision of an existing member first
//!   copies back the fields K2 does not own from Sew's member list.
//! - **KM13:** the `actor` stored on a row and sent as `Sew-Actor` is
//!   attribution only ("reported by K2"), never proof a person acted.
//! - Workspace names/handles, the owner name and project deletes live in
//!   the main DB, so they cannot share the skin.db transaction: they call
//!   [`kick_reconcile`] and the sweep fixes the drift.

pub mod app_space;
pub mod avatar;
pub mod chat;
pub mod features;
pub mod home;
pub mod members;
pub mod outbox;
pub mod reconcile;
pub mod sew_wire;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use rusqlite::{Connection, OptionalExtension};
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
    /// S28's `POST /mgmt/v1/sessions/revoke` (S27-A12, wired in 0.46.1):
    /// queued by [`enqueue_session_revoke`] on its own subject key
    /// (`revoke:…`), so it never waits behind (or blocks) a member's
    /// provisioning queue.
    SewSessionRevoke,
    /// K31: add one App user to that App's Sew Space
    /// (`POST /mgmt/v1/spaces/members/add`, subject `space:<id>:<ext>`).
    SpaceMemberAdd,
    /// K31: remove one person from an App's Sew Space.
    SpaceMemberRemove,
    /// Sew for Apps A3: push the person's K2 picture
    /// (`POST /mgmt/v1/principals/avatar`, raw JPEG; [`avatar`]).
    AvatarSet,
    /// Sew for Apps A3: clear it (`POST /mgmt/v1/principals/avatar/clear`).
    AvatarClear,
    /// Decision 13: the install's home client (`POST /mgmt/v1/home-client`;
    /// [`home`]).
    HomeClient,
    /// Decision 13: one App's `hidden_from_home` and label
    /// (`POST /mgmt/v1/apps/{app_id}/settings`).
    AppSettings,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Provision => "provision",
            Op::Update => "update",
            Op::Deprovision => "deprovision",
            Op::OwnerSet => "owner_set",
            Op::SewSessionRevoke => "sew_session_revoke",
            Op::SpaceMemberAdd => "space_member_add",
            Op::SpaceMemberRemove => "space_member_remove",
            Op::AvatarSet => "avatar_set",
            Op::AvatarClear => "avatar_clear",
            Op::HomeClient => "home_client",
            Op::AppSettings => "app_settings",
        }
    }

    pub fn parse(raw: &str) -> Option<Op> {
        Some(match raw {
            "provision" => Op::Provision,
            "update" => Op::Update,
            "deprovision" => Op::Deprovision,
            "owner_set" => Op::OwnerSet,
            "sew_session_revoke" => Op::SewSessionRevoke,
            "space_member_add" => Op::SpaceMemberAdd,
            "space_member_remove" => Op::SpaceMemberRemove,
            "avatar_set" => Op::AvatarSet,
            "avatar_clear" => Op::AvatarClear,
            "home_client" => Op::HomeClient,
            "app_settings" => Op::AppSettings,
            _ => return None,
        })
    }

    /// The ops the worker delivers. S28's session revoke joined in 0.46.1
    /// (plan B7: "S28 adds its op to `Op::DELIVERED`"); K31's App Space
    /// membership ops ride the same worker.
    pub const DELIVERED: [Op; 11] = [
        Op::Provision,
        Op::Update,
        Op::Deprovision,
        Op::OwnerSet,
        Op::SewSessionRevoke,
        Op::SpaceMemberAdd,
        Op::SpaceMemberRemove,
        Op::AvatarSet,
        Op::AvatarClear,
        Op::HomeClient,
        Op::AppSettings,
    ];

    /// K31's App Space membership ops.
    pub fn is_space_op(self) -> bool {
        matches!(self, Op::SpaceMemberAdd | Op::SpaceMemberRemove)
    }

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
    /// A K2 login session that could hold a Sew session ended (S28 §12).
    SessionRevoke,
    /// A Connect login was removed (its `connect` chat grant goes).
    ConnectUserDelete,
    /// A Connect login's chat-pass mint found no delivered member.
    PassMint,
    /// A Connect login was linked to the owner: it is `k2:owner` in chat
    /// now, so its own `k2:connect` member goes.
    OwnerLinked,
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
            Cause::SessionRevoke => "session_revoke",
            Cause::ConnectUserDelete => "connect_user_delete",
            Cause::PassMint => "pass_mint",
            Cause::OwnerLinked => "owner_linked",
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
    GrantCreated {
        grant: &'a SkinGrant,
    },
    GrantUpdated {
        old: &'a SkinGrant,
        new: &'a SkinGrant,
    },
    GrantDeleted {
        grant: &'a SkinGrant,
    },
    GrantEnabled {
        grant: &'a SkinGrant,
    },
    GrantDisabled {
        grant: &'a SkinGrant,
    },
    /// Email or full name changed.
    PrincipalChanged {
        principal: &'a SkinPrincipal,
    },
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
    /// The owner person's displayName (0.46.0 person names): the linked
    /// account's "First Last", else its username, else `Owner`
    /// ([`crate::person_name::effective`] for `owner`). The retired
    /// "Your name" setting is never read. `None` (tests' `Default`) →
    /// `Owner`.
    pub owner_name: Option<String>,
    /// The owner's primary linked K2 login
    /// ([`crate::person_name::owner_login`]): the `k2:owner` Sew username
    /// ([`chat::owner_username`]). `None` (nothing linked) → `owner`.
    pub owner_login: Option<String>,
    /// Connect logins by K7 id (`connect` chat subjects). Every account
    /// is loaded (a handful per box); absent = removed.
    pub connect: HashMap<String, ConnectFacts>,
    /// The Connect store could not be read: reconcile must not treat a
    /// `connect` grant as orphaned.
    pub connect_unreadable: bool,
}

/// One Connect login as a `k2:connect:<id>` member needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectFacts {
    pub id: String,
    pub username: String,
    /// The login's "First Last", else its username.
    pub display_name: String,
    /// Linked to the owner (`ownerAccounts`): this login IS the owner
    /// person, so in chat it is `k2:owner` (its passes name `k2:owner`)
    /// and never a separate `k2:connect` member. One person, one member.
    pub owner_linked: bool,
}

/// Every Connect login with an id, keyed by id, with its owner link
/// ([`crate::notify_prefs::People::is_linked`]).
pub fn connect_facts() -> Result<HashMap<String, ConnectFacts>, String> {
    let people = crate::notify_prefs::People {
        users: Vec::new(),
        linked: crate::app_settings::load().owner_accounts,
    };
    Ok(crate::connect_users::chat_member_facts()?
        .into_iter()
        .map(|(id, username, display_name)| {
            let owner_linked = people.is_linked(&username);
            (id.clone(), ConnectFacts { id, username, display_name, owner_linked })
        })
        .collect())
}

/// Refusal for a `connect` chat grant aimed at an owner-linked login.
pub const OWNER_CONNECT_IS_OWNER: &str =
    "the owner's Connect login is the owner in chat (k2:owner); it gets no separate chat account";

impl HookCtx {
    /// Load facts for `workspace_ids` (unknown ids are simply absent), the
    /// owner person's displayName and linked login. Call OUTSIDE any
    /// skin.db closure.
    pub fn load(actor: &str, workspace_ids: &[String]) -> HookCtx {
        let mut workspaces = HashMap::new();
        for id in workspace_ids {
            if let Some(f) = workspace_facts(id) {
                workspaces.insert(id.clone(), f);
            }
        }
        let (connect, connect_unreadable) = match connect_facts() {
            Ok(m) => (m, false),
            Err(e) => {
                crate::log_debug!("[grant-hook] Connect users unreadable: {e}");
                (HashMap::new(), true)
            }
        };
        HookCtx {
            actor: actor.to_string(),
            workspaces,
            owner_name: Some(
                crate::person_name::load_effective(crate::notify_prefs::OWNER).display_name,
            ),
            owner_login: crate::person_name::owner_login_now(),
            connect,
            connect_unreadable,
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
        handle: handle
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty()),
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
    fn plan(
        &self,
        conn: &Connection,
        ev: &HookEvent<'_>,
        ctx: &HookCtx,
    ) -> Result<Vec<NewOp>, String>;
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
    // S31: a chat grant change may start or stop an agent's delivery hub.
    crate::sew_chat::hub::kick();
}

/// K31: after a chat grant change, ask for a sweep only when some Space
/// is bound to an App (the sweep plans App Space membership). Never
/// creates `skin.db`.
pub fn kick_app_spaces() {
    if !skin_store_exists() {
        return;
    }
    let bound = crate::skin::with_skin_conn(|conn| {
        conn.query_row("SELECT EXISTS(SELECT 1 FROM tool_app_spaces)", [], |r| r.get::<_, bool>(0))
            .map_err(|e| e.to_string())
    })
    .unwrap_or(false);
    if bound {
        kick_reconcile();
    }
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

pub(crate) fn skin_store_exists() -> bool {
    crate::paths::k2_home().join("skin.db").exists()
}

// ── S28's session revokes on this outbox ─────────────────────────────

/// The outbox subject of a session revoke: `revoke:k2_sid:<sid>` or
/// `revoke:<external id>`. Never a member's own key, so a revoke is its
/// own queue: it is not held behind a blocked provision of the same
/// person, and it never holds their provisioning up.
pub fn revoke_subject_key(op: &crate::sew_revoke::SewSessionRevoke) -> String {
    use crate::sew_revoke::SewSessionRevoke as R;
    match op {
        R::K2Sid(sid) => format!("revoke:k2_sid:{sid}"),
        R::ExternalId(ext) => format!("revoke:{ext}"),
    }
}

/// Queue one `sew_session_revoke` on the chat outbox, inside the caller's
/// open `skin.db` transaction (S28-A3: the row commits or rolls back with
/// the revoke). `Ok(false)` when Sew was never set up on this box (no
/// chat target row): there is no Sew session to end. A paused target
/// (`enabled = 0`) still queues; the worker delivers once it is back on.
pub fn enqueue_session_revoke(
    conn: &Connection,
    op: &crate::sew_revoke::SewSessionRevoke,
) -> Result<bool, String> {
    if outbox::get_target(conn, CHAT_KIND)?.is_none() {
        return Ok(false);
    }
    let row = NewOp {
        tool: CHAT_KIND,
        op: Op::SewSessionRevoke,
        subject_key: revoke_subject_key(op),
        payload: op.payload(),
        grant_id: None,
        grant_rev: None,
        cause: Cause::SessionRevoke,
        after_subject: None,
    };
    outbox::enqueue(conn, &[row], ACTOR_SYSTEM, outbox::now_secs())?;
    kick();
    Ok(true)
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
            .execute(
                "DELETE FROM grants WHERE kind = ?1",
                rusqlite::params![kind],
            )
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

// ── Connect logins (`k2:connect:<id>`, SW2) ──────────────────────────

/// A Connect login's `connect` chat grant, if any.
pub fn connect_chat_grant(conn: &Connection, user_id: &str) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!(
            "{} WHERE kind = ?1 AND subject_kind = 'connect' AND subject_id = ?2",
            crate::skin::GRANT_SELECT
        ),
        rusqlite::params![CHAT_KIND, user_id],
        crate::skin::map_grant_row,
    )
    .optional()
    .map_err(|e| format!("connect chat grant lookup: {e}"))
}

/// A Connect login was removed: its chat grant goes, which enqueues the
/// `deprovision` of its `k2:connect:<id>` member (as a removed App user's
/// does). `Ok(false)` when it had none (or Sew was never set up).
pub fn connect_user_removed(user_id: &str) -> Result<bool, String> {
    if !skin_store_exists() {
        return Ok(false);
    }
    let Some(g) = crate::skin::with_skin_conn(|c| connect_chat_grant(c, user_id))? else {
        return Ok(false);
    };
    Ok(crate::skin::delete_grant_by_id_with_cascade(&g.id, ACTOR_SYSTEM)?.is_some())
}

/// May a Connect login's chat pass be minted now? Sew never invents
/// members: a pass whose `sub` has no member gets 403 `not_provisioned`
/// from Sew, so K2 checks first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectChatGate {
    /// The `k2:connect:<id>` member was delivered: mint.
    Ready,
    /// Its provision is queued or in flight (this call may have queued
    /// it): answer "retry shortly", never mint.
    Pending,
    /// Its provision is blocked or was skipped (owner action needed).
    Blocked(String),
    /// The owner switched this login's chat grant off.
    Disabled,
    /// Chat (the Sew target) is not enabled on this box.
    Off,
}

/// The chat-pass gate for a Connect login (S28 §5, SW2).
///
/// Rev 134 lets every Connect login (any role) mint, so the login's own
/// first mint is its chat grant: with none, this writes a `connect` chat
/// grant (scope `member`, `created_by user:<name>`), whose hook queues the
/// provision. Rev 147 holds: nobody becomes a member by role, only by a
/// grant (the owner's, or the person signing in to chat). An enabled grant
/// whose member was never delivered (or whose rows were pruned, or whose
/// last op was a deprovision) gets a fresh provision row (`pass_mint`);
/// the upsert is idempotent and A8 keeps Sew-owned fields.
pub fn connect_chat_gate(user_id: &str, username: &str) -> Result<ConnectChatGate, String> {
    enum Step {
        Off,
        NoGrant,
        Grant(SkinGrant),
    }
    if !skin_store_exists() {
        return Ok(ConnectChatGate::Off);
    }
    // An owner-linked login mints as `k2:owner` and never reaches here;
    // it must never get a `connect` grant or member if it does.
    if connect_facts()?.get(user_id).is_some_and(|c| c.owner_linked) {
        return Err(OWNER_CONNECT_IS_OWNER.to_string());
    }
    let step = crate::skin::with_skin_conn(|conn| {
        if !outbox::target_enabled(conn, CHAT_KIND)? {
            return Ok(Step::Off);
        }
        Ok(match connect_chat_grant(conn, user_id)? {
            Some(g) => Step::Grant(g),
            None => Step::NoGrant,
        })
    })?;
    let actor = format!("user:{username}");
    let grant = match step {
        Step::Off => return Ok(ConnectChatGate::Off),
        Step::NoGrant => {
            let w = SkinGrantWrite {
                subject_kind: "connect".into(),
                subject_id: user_id.into(),
                kind: CHAT_KIND.into(),
                target_id: String::new(),
                role: None,
                scope: Some("member".into()),
                created_by: Some(actor),
            };
            return match crate::skin::create_grant(&w) {
                Ok(_) => Ok(ConnectChatGate::Pending),
                // A concurrent mint wrote it first: its row is queued.
                Err(e) if e.starts_with("grant already exists") => Ok(ConnectChatGate::Pending),
                Err(e) => Err(e),
            };
        }
        Step::Grant(g) if !g.enabled => return Ok(ConnectChatGate::Disabled),
        Step::Grant(g) => g,
    };
    let subject = sew_wire::ExternalId::K2Connect(user_id.to_string()).to_string();
    let ctx = HookCtx::load(&actor, &[]);
    let now = outbox::now_secs();
    let gate = crate::skin::with_skin_conn(|conn| {
        let open: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT state, last_error FROM tool_outbox
                 WHERE tool = ?1 AND subject_key = ?2 AND state IN ('pending','blocked')
                 ORDER BY CASE state WHEN 'blocked' THEN 0 ELSE 1 END, seq LIMIT 1",
                rusqlite::params![CHAT_KIND, subject],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("connect chat gate: {e}"))?;
        match open {
            Some((s, e)) if s == outbox::STATE_BLOCKED => {
                return Ok(ConnectChatGate::Blocked(
                    e.unwrap_or_else(|| "chat provisioning is blocked".into()),
                ))
            }
            Some(_) => return Ok(ConnectChatGate::Pending),
            None => {}
        }
        let newest: Option<(String, String)> = conn
            .query_row(
                "SELECT op, state FROM tool_outbox
                 WHERE tool = ?1 AND subject_key = ?2 AND state != 'superseded'
                 ORDER BY seq DESC LIMIT 1",
                rusqlite::params![CHAT_KIND, subject],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("connect chat gate: {e}"))?;
        match newest.as_ref().map(|(o, s)| (o.as_str(), s.as_str())) {
            Some(("provision" | "update", outbox::STATE_DONE)) => {
                return Ok(ConnectChatGate::Ready)
            }
            Some((_, s)) if s != outbox::STATE_DONE => {
                return Ok(ConnectChatGate::Blocked(format!(
                    "chat provisioning was {s}; ask the owner to retry it"
                )))
            }
            // No row (pruned) or a delivered deprovision: queue one.
            _ => {}
        }
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("connect chat gate begin: {e}"))?;
        let op = chat::provision_op(&tx, &grant, &ctx, Cause::PassMint)?;
        outbox::enqueue(&tx, &[op], &ctx.actor, now)?;
        tx.commit()
            .map_err(|e| format!("connect chat gate commit: {e}"))?;
        Ok(ConnectChatGate::Pending)
    })?;
    if gate == ConnectChatGate::Pending {
        kick();
    }
    Ok(gate)
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
pub fn hook_states(
    grants: &[SkinGrant],
) -> Result<HashMap<String, (String, Option<String>)>, String> {
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
        // A6: Sew's list is canonical for member ids (the identity map).
        if kind == CHAT_KIND {
            members::reconcile(&tx, actual, now)?;
        }
        let built = reconcile::build_desired(&tx, &ctx)?;
        let mut orphan_ops = Vec::new();
        for (g, cause) in &built.orphans {
            tx.execute("DELETE FROM grants WHERE id = ?1", rusqlite::params![g.id])
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
        // K31: App Space membership follows App grants (rev 147). After
        // the member ops above, so a fresh member's provision queues first.
        if kind == CHAT_KIND {
            let (space_ops, space) = app_space::plan(&tx, &busy, now)?;
            if !space_ops.is_empty() || space.held_forgotten > 0 {
                crate::log_debug!(
                    "[grant-hook] app spaces: {}",
                    serde_json::to_value(&space).unwrap_or_default()
                );
            }
            ops.extend(space_ops);
            // A3: pictures Sew doesn't show yet (after the member ops, on
            // each member's own queue).
            ops.extend(avatar::plan(&tx, &busy)?);
        }
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

// ── Decision 13: home client and per-App settings ────────────────────

/// The sweep's settings step: record the feature flags Sew reported
/// (`features` from its status; never its version), then queue the
/// settings it can take and doesn't have yet. Returns the rows queued.
pub fn apply_settings(kind: &str, sew_features: &[String]) -> Result<usize, String> {
    if kind != CHAT_KIND {
        return Ok(0);
    }
    let now = outbox::now_secs();
    let n = crate::skin::with_skin_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("settings begin: {e}"))?;
        if features::note(&tx, kind, sew_features, now)? {
            crate::log_debug!("[grant-hook] Sew features: {}", sew_features.join(","));
        }
        let busy = outbox::busy_subjects(&tx, kind)?;
        let ops = home::plan(&tx, &busy)?;
        let n = outbox::enqueue(&tx, &ops, ACTOR_RECONCILE, now)?.len();
        tx.commit().map_err(|e| format!("settings commit: {e}"))?;
        Ok(n)
    })?;
    if n > 0 {
        kick();
    }
    Ok(n)
}

/// Did the box's Sew report `flag` in its last status ([`features`])?
/// `false` before any sweep, or on a box without Sew.
pub fn sew_has(flag: &str) -> Result<bool, String> {
    if !skin_store_exists() {
        return Ok(false);
    }
    crate::skin::with_skin_conn(|conn| features::has(conn, CHAT_KIND, flag))
}

/// Every flag the box's Sew last reported (empty before any sweep).
pub fn sew_features() -> Result<Vec<String>, String> {
    if !skin_store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(|conn| features::list(conn, CHAT_KIND))
}

/// Tests: as if Sew's status had reported `flags`.
#[cfg(any(test, feature = "test-util"))]
pub fn note_features_for_tests(flags: &[&str]) -> Result<(), String> {
    let f: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
    crate::skin::with_skin_conn(|conn| features::note(conn, CHAT_KIND, &f, outbox::now_secs()).map(|_| ()))
}

/// Record the signalling origins of `apps` (`(service id, wss origin or
/// None, label)`, computed by the daemon from the App's bindings) before
/// the sweep plans settings. Returns how many changed.
pub fn note_app_origins(apps: &[(String, Option<String>, String)]) -> Result<usize, String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        let mut n = 0;
        for (app, origin, name) in apps {
            if home::set_app_origin(conn, app, origin.as_deref(), name, ACTOR_RECONCILE, now)? {
                n += 1;
            }
        }
        Ok(n)
    })
}

/// The Apps the sweep keeps an origin for: those bound to a Sew Space,
/// those with the owner's guests-chat grant, and those with settings.
pub fn chat_apps() -> Result<Vec<String>, String> {
    if !skin_store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(|conn| {
        let mut out: Vec<String> = app_space::bindings(conn)?.into_iter().map(|b| b.app_id).collect();
        let mut st = conn.prepare("SELECT service_id FROM app_chat_grants").map_err(|e| format!("chat apps: {e}"))?;
        let g = st
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| format!("chat apps: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("chat apps: {e}"))?;
        out.extend(g);
        out.extend(home::apps(conn)?);
        out.sort();
        out.dedup();
        Ok(out)
    })
}

/// The owner's settings, as stored (`GET /cli/sew/home`).
pub fn home_settings() -> Result<Vec<home::Setting>, String> {
    if !skin_store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(home::list)
}

/// Set (`Some`) or clear the home client; the next sweep delivers it.
pub fn set_home_client(app_id: Option<&str>, actor: &str) -> Result<bool, String> {
    let now = outbox::now_secs();
    let changed = crate::skin::with_skin_conn(|conn| home::set_home_client(conn, app_id, actor, now))?;
    if changed {
        kick_reconcile();
    }
    Ok(changed)
}

/// Set an App's `hidden_from_home` (and its label); the next sweep
/// delivers it.
pub fn set_app_home(app_id: &str, hidden: bool, display_name: &str, actor: &str) -> Result<bool, String> {
    let now = outbox::now_secs();
    let changed = crate::skin::with_skin_conn(|conn| home::set_app(conn, app_id, hidden, display_name, actor, now))?;
    if changed {
        kick_reconcile();
    }
    Ok(changed)
}

// ── K31: App Space bindings ──────────────────────────────────────────

/// One binding with its ledger counts (`GET /cli/sew/app-spaces`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSpaceView {
    #[serde(flatten)]
    pub binding: app_space::Binding,
    pub members: app_space::Counts,
}

pub fn app_spaces() -> Result<Vec<AppSpaceView>, String> {
    crate::skin::with_skin_conn(|conn| {
        app_space::bindings(conn)?
            .into_iter()
            .map(|b| {
                let members = app_space::counts(conn, &b.space_id)?;
                Ok(AppSpaceView { binding: b, members })
            })
            .collect()
    })
}

/// Record `app` ↔ `space` (Sew's side is bound first, by the caller) and
/// plan the App's users into it. Returns whether the row is new and the
/// plan summary.
pub fn app_space_bind(
    app_id: &str,
    space_id: &str,
    actor: &str,
) -> Result<(bool, app_space::Summary), String> {
    let now = outbox::now_secs();
    let r = crate::skin::with_skin_conn(|conn| {
        let created = app_space::bind(conn, app_id, space_id, actor, now)?;
        let s = app_space::reconcile(conn, actor, now)?;
        Ok((created, s))
    })?;
    kick();
    Ok(r)
}

/// Forget `app` ↔ `space`. Members stay unless `remove_members` (then only
/// the members K2 added automatically leave; S32 Q3, Rosson 2026-10-27).
pub fn app_space_unbind(
    app_id: &str,
    space_id: &str,
    actor: &str,
    remove_members: bool,
) -> Result<(bool, app_space::Summary), String> {
    let now = outbox::now_secs();
    let r = crate::skin::with_skin_conn(|conn| {
        let gone = app_space::unbind(conn, app_id, space_id, remove_members, actor, now)?;
        let s = app_space::reconcile(conn, actor, now)?;
        Ok((gone, s))
    })?;
    kick();
    Ok(r)
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
            crate::log_debug!(
                "[grant-hook] Sew install_id changed {} → {install_id}",
                old.unwrap_or_default()
            );
            let ops: Vec<NewOp> = p
                .plan(&tx, &HookEvent::TargetEnabled, &ctx)?
                .into_iter()
                .map(|mut o| {
                    o.cause = Cause::InstallChanged;
                    o
                })
                .collect();
            outbox::enqueue(&tx, &ops, ACTOR_RECONCILE, now)?;
            // A3: the new install has no picture K2 pushed; A6: every
            // member id changed.
            avatar::forget_all(&tx)?;
            members::forget_all(&tx)?;
            // Decision 13: the new install owes every setting again.
            home::forget_sent(&tx)?;
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
    crate::skin::with_skin_conn(|conn| {
        outbox::mark_done(conn, id, status as i64, Some(result), now)?;
        // A3: a delivered picture is what Sew shows now.
        avatar::after_done(conn, id, now)?;
        // A6: a delivered provision/deprovision moves the identity map on.
        members::after_done(conn, id, result, now)?;
        // Decision 13: Sew has that settings rev now.
        home::after_done(conn, id)?;
        // K31: a delivered Space op moves the membership ledger on.
        app_space::after_done(conn, id, now)
    })
}

/// A3: an `avatar_set` row whose picture changed before it went: finished
/// unsent (`superseded`; the newer row behind it carries the picture).
pub fn record_superseded(id: &str) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| {
        conn.execute(
            "UPDATE tool_outbox SET state = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![outbox::STATE_SUPERSEDED, now, id],
        )
        .map(|_| ())
        .map_err(|e| format!("tool_outbox supersede: {e}"))
    })
}

/// A3: the picture an `avatar_set` row sends, read at send time (`None` =
/// superseded: the picture changed or went since the row was queued).
pub fn avatar_bytes(payload_raw: &str) -> Result<Option<Vec<u8>>, String> {
    let v: serde_json::Value = serde_json::from_str(payload_raw).map_err(|e| format!("avatar row payload: {e}"))?;
    crate::skin::with_skin_conn(|conn| avatar::picture_for_send(conn, &v))
}

/// A8: store the filled provision body (or just the mark, when nothing
/// changed) before the first send.
pub fn record_filled(id: &str, payload: Option<&serde_json::Value>) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| outbox::mark_filled(conn, id, payload, now))
}

/// HC2: a member-add listed the subject as `skipped` (K5/K31 seam).
pub fn record_held(id: &str, status: u16, result: &str, why: &str) -> Result<(), String> {
    let now = outbox::now_secs();
    crate::log_debug!("[grant-hook] row {id} held: {why}");
    crate::skin::with_skin_conn(|conn| {
        outbox::mark_held(conn, id, status as i64, Some(result), why, now)?;
        app_space::after_held(conn, id, why, now)
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
        outbox::mark_retry(
            conn,
            id,
            status.map(i64::from),
            error,
            now + delay_secs.max(0),
            now,
        )
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
    crate::skin::with_skin_conn(|conn| {
        outbox::set_target_reconcile(conn, kind, None, Some(error), now)
    })
}

pub fn prune() -> Result<usize, String> {
    if !skin_store_exists() {
        return Ok(0);
    }
    let now = outbox::now_secs();
    crate::skin::with_skin_conn(|conn| outbox::prune(conn, now))
}

/// A6: one mapped member by external id (`k2:principal:<id>` …).
pub fn chat_member(ext: &str) -> Result<Option<members::MemberRow>, String> {
    if !skin_store_exists() {
        return Ok(None);
    }
    crate::skin::with_skin_conn(|conn| members::by_external_id(conn, ext))
}

/// Tests: map `ext` ↔ `member_id` directly (as a delivered provision would).
#[cfg(any(test, feature = "test-util"))]
pub fn map_member_for_tests(ext: &str, member_id: &str, kind: &str, username: &str, state: &str) -> Result<(), String> {
    crate::skin::with_skin_conn(|conn| members::upsert(conn, ext, member_id, kind, Some(username), state, outbox::now_secs()))
}

/// Tests: make `ext` a tombstone directly.
#[cfg(any(test, feature = "test-util"))]
pub fn tombstone_member_for_tests(ext: &str) -> Result<(), String> {
    crate::skin::with_skin_conn(|conn| members::tombstone(conn, ext, outbox::now_secs()))
}

/// Tests: every pending row due now.
#[cfg(any(test, feature = "test-util"))]
pub fn force_due_for_tests() -> Result<usize, String> {
    crate::skin::with_skin_conn(outbox::force_due_for_tests)
}

#[cfg(test)]
mod tests;
