//! K2 as the multiplayer kit's [`Host`]: rights per participant, the clock,
//! and the audit sink (Markdown multiplayer v1, K2).
//!
//! **Rights come from the pid alone.** The kit asks [`Host::rights`] at join,
//! on every call (read), on every edit (edit) and on history reads
//! (history), with the session locked. K2 stamps every participant itself
//! ([`super::caller`]), so the pid names who it is, and this module
//! re-derives that principal's rights from K2's current state on each call:
//! a removed login, a removed connection or a revoked share takes effect on
//! the next call, and within the 2 s upkeep for open connections.
//!
//! | pid | Who | Rights |
//! |---|---|---|
//! | `person:owner` | the owner token, a shell tab (via `terminal`), a login the owner linked | all, any workspace |
//! | `person:<login>` | a Connect login (every role is Member or above) | all, any workspace |
//! | `agent:<project id>` | an agent's passport (Loopback / its cell socket) | its own workspace: all. Another workspace: a connection **and** a share (MI25), see [`agent_rights`] |
//!
//! Rights are `{read, edit, history}` (kit amendment 1). History is never
//! implied by read or edit (K-PERM-9): people have it, an agent has it in
//! its own workspace, and elsewhere only where a share grants it. Acting on
//! someone's behalf never widens an agent's rights (K-PERM-10): the kit asks
//! about the agent's own pid.
//!
//! **Called under a session lock**: never call into the kit from here, and
//! take K2's DB lock only briefly (the DB lock is never held while calling
//! the kit anywhere in `docs`, so the order is always session, then DB).
//!
//! Audit: refusals, removals and approvals go to the Access Audit
//! (`auth_audit`) as metadata only (doc key, pid, action); joins and leaves
//! to the debug log.

use std::sync::Weak;
use std::time::{SystemTime, UNIX_EPOCH};

use k2_core::log_debug;
use k2_mpc_base::session::{
    AuditEvent, ClaimEnd, Host, LeaveReason, Participant, PurgedScope, ReservationEnd, Rights,
};
use k2_mpc_base::store::{DocKey, DocStore, PurgeCounts};

/// pid prefix for people.
pub const PERSON_PREFIX: &str = "person:";
/// pid prefix for agents (followed by the workspace's project id).
pub const AGENT_PREFIX: &str = "agent:";

/// Access Audit event names (Settings → Access Audit).
pub const AUDIT_REFUSED: &str = "doc-refused";
pub const AUDIT_REMOVED: &str = "doc-removed";
pub const AUDIT_APPROVAL: &str = "doc-approval";
/// The file under a live doc changed outside K2, went missing, stopped
/// decoding, came back, or failed to read/write.
pub const AUDIT_FILE: &str = "doc-file";
/// The audit `user` for file events (the kit's outside-edit participant).
const EXTERNAL: &str = k2_mpc_base::session::EXTERNAL_PID;
/// Tier 0: an owner broke someone's lease.
pub const AUDIT_LEASE_BROKEN: &str = "doc-lease-broken";
/// Tier 0: another program saved a file under someone's lease, over a
/// version the file never got, or (kit v0.14.0) over a version the kit
/// wrote (an agent's write, a restore, a merge): the save is current, the
/// replaced content is a version and a conflict copy.
pub const AUDIT_CONFLICT: &str = "doc-conflict";
/// A purge (owner only; not on `k2 doc`): scope and counts, never content.
pub const AUDIT_PURGED: &str = "doc-purged";
/// A removed workspace's tier-0 blob key was destroyed with its blobs.
pub const AUDIT_SPACE_KEY_PURGED: &str = "doc-space-key-purged";
/// An outside save may have removed or replaced text someone wrote through
/// the kit (kit v0.9.7.2): line numbers only, never the text.
pub const AUDIT_OUTSIDE_OVERRIDE: &str = "doc-outside-override";
/// K2 closed a doc's live session (a multiplayer core switched off): the
/// connections that were in it, as a count.
pub const AUDIT_SESSION_CLOSED: &str = "doc-session-closed";

thread_local! {
    /// While [`collect_disk_errors`] runs on this thread: the kit's
    /// `DiskError`s, by doc.
    static DISK_ERRORS: std::cell::RefCell<Option<Vec<(DocKey, String)>>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` and collect the `AuditEvent::DiskError`s the kit reports while it
/// runs. The kit calls [`Host::on_event`] on the calling thread, so what is
/// collected is what `f`'s own kit calls reported (a host close's
/// write-backs), never another thread's.
pub fn collect_disk_errors<R>(f: impl FnOnce() -> R) -> (R, Vec<(DocKey, String)>) {
    let outer = DISK_ERRORS.with(|c| c.borrow_mut().replace(Vec::new()));
    let r = f();
    let got = DISK_ERRORS.with(|c| std::mem::replace(&mut *c.borrow_mut(), outer)).unwrap_or_default();
    (r, got)
}

/// What one principal may do with one doc (K2's copy of the kit's `Rights`,
/// with a wire shape). `owner` (break a tier-0 lease, purge, erase a
/// participant) is never implied by the others. `comment` is the explicit
/// comment grant of a read-only share: edit implies commenting (Rosson's
/// ruling), so editors need nothing extra, and a read-only share comments
/// only when it was granted (`k2 doc share --comment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocRights {
    pub read: bool,
    pub edit: bool,
    pub history: bool,
    pub owner: bool,
    pub comment: bool,
}

impl DocRights {
    pub const NONE: DocRights = DocRights { read: false, edit: false, history: false, owner: false, comment: false };
    pub const FULL: DocRights = DocRights { read: true, edit: true, history: true, owner: false, comment: false };
    /// The box owner and Admin logins: everything, and the doc owner's acts.
    pub const OWNER: DocRights = DocRights { read: true, edit: true, history: true, owner: true, comment: false };

    /// The kit's view. Nothing without read; edit, history, owner and the
    /// comment grant only with it (`Rights::with_comment`; edit implies it).
    pub fn kit(self) -> Rights {
        let base = match (self.read, self.edit) {
            (true, true) => Rights::editor(),
            (true, false) => Rights::read_only(),
            _ => return Rights::none(),
        };
        base.with_history(self.history).with_owner(self.owner).with_comment(self.comment)
    }

    /// Whether these rights may comment: read, and edit or the comment grant.
    pub fn may_comment(self) -> bool {
        self.kit().may_comment()
    }

    /// Wire shape: `{read, edit, history, comment}` (`comment`: may comment).
    pub fn json(self) -> serde_json::Value {
        serde_json::json!({ "read": self.read, "edit": self.edit, "history": self.history, "comment": self.may_comment() })
    }
}

/// Why a cross-workspace agent has no rights (for the refusal it gets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossRefusal {
    /// No comms connection between the two workspaces (taught as
    /// `k2 connections add`, `walls.rs`).
    NotConnected,
    /// Connected, but nobody shared this file or a folder above it (MI25).
    NotShared,
}

/// A parsed pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// `person:<person key>`: `owner` or a Connect login.
    Person(String),
    /// `agent:<project id>`.
    Agent(String),
}

impl Principal {
    pub fn parse(pid: &str) -> Option<Principal> {
        if let Some(k) = pid.strip_prefix(PERSON_PREFIX).filter(|k| !k.is_empty()) {
            return Some(Principal::Person(k.to_string()));
        }
        if let Some(id) = pid.strip_prefix(AGENT_PREFIX).filter(|k| !k.is_empty()) {
            return Some(Principal::Agent(id.to_string()));
        }
        None
    }

    pub fn pid(&self) -> String {
        match self {
            Principal::Person(k) => format!("{PERSON_PREFIX}{k}"),
            Principal::Agent(id) => format!("{AGENT_PREFIX}{id}"),
        }
    }
}

/// Whether `person_key` is a person on this box now: the owner, or a
/// Connect login that still exists.
pub fn person_exists(person_key: &str) -> bool {
    person_key == k2_core::notify_prefs::OWNER || k2_core::connect_users::role_for_user(person_key).is_some()
}

/// A person's rights on any doc on this box: the owner (and the logins the
/// owner linked, which resolve to `owner`) and every existing login may do
/// everything (every role is Member or above, and Member writes files
/// today: `/cli/fs/write-file` is a Member route). A login that no longer
/// exists gets nothing. The doc owner's acts (breaking a tier-0 lease) are
/// the box owner's and Admin logins' (PRD Q21: the workspace owner and
/// admins); agents never have them.
pub fn person_rights(person_key: &str) -> DocRights {
    if person_key == k2_core::notify_prefs::OWNER {
        return DocRights::OWNER;
    }
    match k2_core::connect_users::role_for_user(person_key) {
        Some(role) if role >= k2_core::connect_users::Role::Admin => DocRights::OWNER,
        Some(_) => DocRights::FULL,
        None => DocRights::NONE,
    }
}

/// An agent's rights on a doc in workspace `space`. Its own workspace: all.
/// Another workspace: a comms connection is required but not enough; a
/// person must have shared the file or a folder above it (MI25, K-PERM-4):
/// read, plus write, history and comment only where a share grants them.
/// `store` holds the shares (`None` once released: nothing is shared then).
pub fn agent_rights(agent_project: &str, space: &str, rel: &str, store: Option<&DocStore>) -> Result<DocRights, CrossRefusal> {
    if agent_project == space {
        return Ok(DocRights::FULL);
    }
    if !k2_core::connections::are_local_peers(agent_project, space) {
        return Err(CrossRefusal::NotConnected);
    }
    let granted = store.and_then(|st| match super::shares::grant_all(st, space, agent_project, rel) {
        Ok(g) => g,
        Err(e) => {
            log_debug!("[docs] WARN: share lookup failed (refusing): {e}");
            None
        }
    });
    match granted {
        Some(g) => Ok(DocRights { read: true, edit: g.write, history: g.history, owner: false, comment: g.comment }),
        None => Err(CrossRefusal::NotShared),
    }
}

/// Rights for `pid` on `key`: the whole rule, used by the kit and by K2's
/// own pre-checks.
pub fn rights_for(pid: &str, key: &DocKey, store: Option<&DocStore>) -> DocRights {
    match Principal::parse(pid) {
        Some(Principal::Person(k)) => person_rights(&k),
        Some(Principal::Agent(id)) => agent_rights(&id, &key.space, &key.path, store).unwrap_or(DocRights::NONE),
        None => DocRights::NONE,
    }
}

/// The host. Holds only a weak handle on the store (for shares), so it never
/// keeps the file open past a release.
#[derive(Debug, Default)]
pub struct K2Host {
    store: Weak<DocStore>,
}

impl K2Host {
    pub fn new(store: Weak<DocStore>) -> Self {
        K2Host { store }
    }
}

impl Host for K2Host {
    fn rights(&self, who: &Participant, key: &DocKey) -> Rights {
        let store = self.store.upgrade();
        rights_for(&who.pid, key, store.as_deref()).kit()
    }

    fn now_ms(&self) -> u64 {
        now_ms()
    }

    /// Tier 0's conflict copies go under the workspace's `.k2/conflicts/`
    /// (K-A11Y-5: out of the way, and out of Git: K2 writes a `*`
    /// `.gitignore` there the first time).
    fn conflict_dir(&self, file: &std::path::Path) -> Option<std::path::PathBuf> {
        conflict_dir_for(file)
    }

    /// Kit v0.14.0: the copy of an agent's replaced write is named after
    /// the agent (`notes (Ada's version).md`); anything else keeps the
    /// kit's name ([`super::conflicts::copy_name_for`]).
    fn conflict_copy_name(&self, file_name: &str, _at_ms: u64, who: &str) -> Option<String> {
        super::conflicts::copy_name_for(file_name, who, super::conflicts::is_agent_name)
    }

    fn on_event(&self, ev: &AuditEvent) {
        match ev {
            AuditEvent::Refused { key, pid, action, need } => audit(
                AUDIT_REFUSED,
                pid,
                format!("{} action={action:?} need={need:?}", key_label(key)),
            ),
            AuditEvent::Approval { key, seq, pid, state } => audit(
                AUDIT_APPROVAL,
                pid,
                format!("{} change={seq} state={state:?}", key_label(key)),
            ),
            // Disk sync (kit C4): what happened to the file, metadata only.
            AuditEvent::ExternalEdit { key, seq } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} external_edit change={seq}", key_label(key)))
            }
            AuditEvent::FileMissing { key, unflushed } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} missing unflushed={unflushed}", key_label(key)))
            }
            AuditEvent::FileDeclined { key, code } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} declined code={code}", key_label(key)))
            }
            AuditEvent::FileRestored { key } => audit(AUDIT_FILE, EXTERNAL, format!("{} restored", key_label(key))),
            // Kit v0.9.5: the file came back holding one of the kit's own
            // recent writes (a stale editor buffer), so it wasn't an edit.
            AuditEvent::EchoSuppressed { key } => {
                log_debug!("[docs] {}: a save of the kit's own earlier write was not taken as an edit", key_label(key))
            }
            // Kit v0.9.3's sampled edit check: never served, but a kit bug
            // to report (the doc takes the slow path until reloaded).
            AuditEvent::EditPathMismatch { key, client, change } => log_debug!(
                "[docs] WARN: kit edit check failed on {} (client {client}, change {change}); slow path until reloaded, report it to K2 Cores",
                key_label(key)
            ),
            AuditEvent::TextCacheMismatch { key, root, change } => log_debug!(
                "[docs] WARN: kit text cache check failed on {} (root {root}, change {change}); fresh reads until reloaded, report it to K2 Cores",
                key_label(key)
            ),
            AuditEvent::DiskError { key, error } => {
                DISK_ERRORS.with(|c| {
                    if let Some(v) = c.borrow_mut().as_mut() {
                        v.push((key.clone(), error.clone()));
                    }
                });
                log_debug!("[docs] WARN: disk error on {}: {error}", key_label(key));
                audit(AUDIT_FILE, EXTERNAL, format!("{} disk_error", key_label(key)))
            }
            AuditEvent::Left { key, pid, conn, reason } => match reason {
                LeaveReason::Kicked | LeaveReason::RightsLost => audit(
                    AUDIT_REMOVED,
                    pid,
                    format!("{} {conn} reason={reason:?}", key_label(key)),
                ),
                LeaveReason::Left | LeaveReason::Idle => {
                    log_debug!("[docs] {pid} left {} ({conn}, {reason:?})", key_label(key))
                }
            },
            // Purge stays off `k2 doc` (owner-only, a later surface); when it
            // runs, the audit gets what was reached (scope, counts), never
            // content and never a content hash (K-PURGE-1).
            AuditEvent::Purged { key, by, scope, counts } => audit(AUDIT_PURGED, by, purged_outcome(key, scope, counts)),
            // Tier 0 (kit amendment 3): leases are presence (debug log);
            // a break and a conflicting save are audited, metadata only.
            AuditEvent::LeaseAcquired { key, pid, expires_ms, auto } => {
                log_debug!("[docs] {pid} holds the lease on {} until {expires_ms} (auto={auto})", key_label(key))
            }
            AuditEvent::LeaseEnded { key, pid, how } => {
                log_debug!("[docs] {pid}'s lease on {} ended ({how:?})", key_label(key))
            }
            AuditEvent::LeaseBroken { key, by, holder, expires_ms } => audit(
                AUDIT_LEASE_BROKEN,
                by,
                format!("{} holder={holder} expires_ms={expires_ms}", key_label(key)),
            ),
            AuditEvent::BlobConflict { key, current, kept, copy, lease_holder } => {
                audit(
                    AUDIT_CONFLICT,
                    EXTERNAL,
                    format!(
                        "{} current={current} kept={kept} copy={} lease_holder={}",
                        key_label(key),
                        copy.as_deref().unwrap_or("-"),
                        lease_holder.as_deref().unwrap_or("-")
                    ),
                );
                // The kit emits it once per copy: the person is told once
                // (a tab notice and a Thread line) and the agents get an
                // inbox note (K-T0-3), off the session lock.
                super::conflicts::notify(super::conflicts::Conflict {
                    key: key.clone(),
                    current: *current,
                    kept: *kept,
                    copy: copy.clone(),
                    lease_holder: lease_holder.clone(),
                });
            }
            AuditEvent::BlobMerged { key, version } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} merged version={version}", key_label(key)))
            }
            // Agent presence (kit amendments 13/14): when a claim (or its
            // hand-off) ends with someone waiting, that agent is told the
            // lines are free and the version to re-read at. Off the lock.
            // Claims by node on a blob doc (kit A5): the roster's claims
            // changed; open tabs refresh the layer panel. Nobody waits in
            // line for a node claim (another's claim refuses, exit 5).
            AuditEvent::ClaimTaken { key, lines, .. } if lines.root == k2_mpc_base::session::NODE_CLAIM_ROOT => {
                super::indexer::changed(key, super::indexer::Change::ClaimTaken, None, None, None)
            }
            AuditEvent::ClaimEnded { key, pid, how, lines, version, .. } if lines.root == k2_mpc_base::session::NODE_CLAIM_ROOT => {
                log_debug!("[docs] {pid}'s node claim on {} ended ({})", key_label(key), how.as_str());
                super::indexer::changed(key, super::indexer::Change::ClaimEnded, Some(*version), None, None)
            }
            AuditEvent::ClaimEnded { key, pid, how, lines, version, next_waiter, .. } => {
                log_debug!("[docs] {pid}'s claim on {} lines {}-{} ended ({})", key_label(key), lines.first, lines.last, how.as_str());
                if let Some((_, waiter)) = next_waiter.as_ref().filter(|_| !matches!(how, ClaimEnd::Reset)) {
                    super::claims::notify_free(key.clone(), waiter.clone(), pid.clone(), (lines.first, lines.last), *version);
                }
            }
            AuditEvent::ReservationEnded { key, pid, how, lines, version, next_waiter, .. } => {
                log_debug!("[docs] {}'s hand-off on {} ended ({how:?})", pid, key_label(key));
                if let Some((_, waiter)) =
                    next_waiter.as_ref().filter(|_| !matches!(how, ReservationEnd::Claimed | ReservationEnd::Reset))
                {
                    super::claims::notify_free(key.clone(), waiter.clone(), pid.clone(), (lines.first, lines.last), *version);
                }
            }
            // The structure index (kit A1): a version wants indexing. The
            // session is locked: only queue the job (`indexer`).
            AuditEvent::IndexWanted { key, version } => super::indexer::want(key, *version),
            AuditEvent::IndexReady { key, version, nodes } => {
                log_debug!("[docs] {} version {version} indexed ({nodes} nodes)", key_label(key));
                super::indexer::changed(key, super::indexer::Change::IndexReady, Some(*version), None, None)
            }
            AuditEvent::IndexNotIndexed { key, version } => {
                log_debug!("[docs] {} version {version} has no index (k2 doc status says why)", key_label(key));
                super::indexer::changed(key, super::indexer::Change::IndexNotIndexed, Some(*version), None, None)
            }
            // Comments (kit A4): never a body. The people and agents a
            // comment mentions are told off the lock (the body is read with
            // `thread()` there); open tabs catch up with `comments --since`.
            AuditEvent::Annotation { key, seq, thread, comment, op, pid, on_behalf_of, mentions, .. } => {
                super::indexer::changed(key, super::indexer::Change::Annotation, None, Some(*seq), Some(thread.to_string()));
                if !mentions.is_empty() {
                    super::annotations::notify_mentions(super::annotations::Mention {
                        key: key.clone(),
                        thread: *thread,
                        comment: *comment,
                        op: op.name(),
                        by: pid.clone(),
                        on_behalf_of: on_behalf_of.clone(),
                        mentions: mentions.clone(),
                    });
                }
            }
            AuditEvent::AnchorState { key, thread, version, now, .. } => {
                log_debug!("[docs] {} {thread}: anchor {} at version {version}", key_label(key), now.name());
                super::indexer::changed(key, super::indexer::Change::AnchorState, Some(*version), None, Some(thread.to_string()))
            }
            AuditEvent::RecheckFailed { key, error } => {
                log_debug!("[docs] WARN: rights re-check failed on {}: {error}", key_label(key))
            }
            // Kit v0.9.7.2: an outside save may have removed someone's text.
            // The note, with the text, goes to its author; the audit gets
            // line numbers only.
            AuditEvent::OutsideOverride { key, change, rule, region, by, version } => audit(
                AUDIT_OUTSIDE_OVERRIDE,
                EXTERNAL,
                format!(
                    "{} change={change} rule={rule} region={region} author={} version={}",
                    key_label(key),
                    by.as_deref().unwrap_or("-"),
                    version.map_or_else(|| "-".to_string(), |v| v.to_string())
                ),
            ),
            // Kit v0.9.7.2: K2 closed the session (`SessionManager::close_where`:
            // a multiplayer core switched off).
            AuditEvent::SessionClosed { key, connections, reason } => audit(
                AUDIT_SESSION_CLOSED,
                "k2",
                format!("{} reason={reason} connections={}", key_label(key), connections.len()),
            ),
            AuditEvent::DocDeleted { key, connections } => {
                audit(AUDIT_FILE, "k2", format!("{} removed connections={}", key_label(key), connections.len()))
            }
            AuditEvent::Joined { key, pid, conn, .. } => {
                log_debug!("[docs] {pid} joined {} ({conn})", key_label(key))
            }
            AuditEvent::CompactFailed { key, error } => {
                log_debug!("[docs] WARN: compaction failed for {}: {error}", key_label(key))
            }
            AuditEvent::Recovered { key, error } => {
                log_debug!("[docs] WARN: store write failed for {}; session reloaded: {error}", key_label(key))
            }
            // Kit v0.10.0.3: the first sync after a reopen found the stored
            // disk replica (the merge base for the next outside save) stale
            // and reloaded it from the doc. Should never fire: a kit bug to
            // report. Byte lengths only, never content.
            AuditEvent::DiskReplicaRepaired { key, doc_len, replica_len } => {
                log_disk_replica_repaired(key, *doc_len, *replica_len)
            }
            // The write-back's sampled check (every
            // `Config::writeback_check_every` write-backs) found the disk
            // replica different from the live doc; repaired on the spot, the
            // file was right either way. Should never fire: a kit bug to
            // report (K2 Cores, 11-01). Metadata only.
            AuditEvent::DiskReplicaMismatch { key, version } => log_disk_replica_mismatch(key, *version),
            // `AuditEvent` is `#[non_exhaustive]` (the kit adds events in
            // patch releases), so a catch-all is required; it logs the event
            // rather than dropping it.
            other => log_debug!("[docs] event {other:?}"),
        }
    }
}

/// The warn line for [`AuditEvent::DiskReplicaRepaired`]. Stable prefix
/// `[docs] WARN: disk replica repaired` (grep for it). `key` is the doc's
/// `<project id>:<workspace-relative path>` ([`key_label`]), never an
/// absolute path; the lengths are bytes; never content.
pub fn disk_replica_repaired_line(key: &DocKey, doc_len: u64, replica_len: u64) -> String {
    format!(
        "[docs] WARN: disk replica repaired key={} doc_len={doc_len} replica_len={replica_len} (kit bug: report it to K2 Cores)",
        key_label(key)
    )
}

/// The warn line for [`AuditEvent::DiskReplicaMismatch`]. Stable prefix
/// `[docs] WARN: disk replica mismatch` (grep for it). `key` as in
/// [`disk_replica_repaired_line`]; `version` is the doc version checked.
pub fn disk_replica_mismatch_line(key: &DocKey, version: u64) -> String {
    format!(
        "[docs] WARN: disk replica mismatch key={} version={version} (kit bug: report it to K2 Cores)",
        key_label(key)
    )
}

#[cfg(test)]
thread_local! {
    /// Tests: the disk-replica-repaired and -mismatch lines logged on this thread.
    static REPAIRED_LINES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn log_disk_replica_repaired(key: &DocKey, doc_len: u64, replica_len: u64) {
    let line = disk_replica_repaired_line(key, doc_len, replica_len);
    log_debug!("{line}");
    #[cfg(test)]
    REPAIRED_LINES.with(|v| v.borrow_mut().push(line));
}

fn log_disk_replica_mismatch(key: &DocKey, version: u64) {
    let line = disk_replica_mismatch_line(key, version);
    log_debug!("{line}");
    #[cfg(test)]
    REPAIRED_LINES.with(|v| v.borrow_mut().push(line));
}

/// `<workspace>/.k2/conflicts` for a file: the nearest folder above it that
/// has a `.k2` folder (a workspace root). `None` (beside the file) when
/// there is none. Creates the folder and its `.gitignore` once.
pub fn conflict_dir_for(file: &std::path::Path) -> Option<std::path::PathBuf> {
    let root = file.ancestors().skip(1).find(|d| d.join(".k2").is_dir())?;
    let dir = root.join(".k2").join("conflicts");
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&ignore, "*\n")) {
            log_debug!("[docs] WARN: conflict folder {}: {e}", dir.display());
        }
    }
    Some(dir)
}

/// Milliseconds since the epoch (the kit's clock and the share stamps).
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `<space>:<path>`: metadata only.
pub fn key_label(key: &DocKey) -> String {
    format!("{}:{}", key.space, key.path)
}

/// One Access Audit line (metadata only, never doc content).
pub fn audit(event: &str, pid: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, pid, outcome, "-", "-", "cli"));
}

/// A purge's Access Audit outcome: the doc, what was targeted and what
/// changed, as numbers. A purged text appears only as its byte length; an
/// erased participant only as its pseudonym. Never content, never a hash.
pub fn purged_outcome(key: &DocKey, scope: &PurgedScope, c: &PurgeCounts) -> String {
    let scope = match scope {
        PurgedScope::History => "history".to_string(),
        PurgedScope::Changes { from, to } => format!("changes:{from}-{to}"),
        PurgedScope::Versions { from, to } => format!("versions:{from}-{to}"),
        PurgedScope::Text { bytes } => format!("text:{bytes}B"),
        PurgedScope::Participant { pseudonym } => format!("participant:{pseudonym}"),
        PurgedScope::BlobVersions { version: Some(v) } => format!("blob_version:{v}"),
        PurgedScope::BlobVersions { version: None } => "blob_versions:all_but_current".to_string(),
        _ => "other".to_string(),
    };
    format!(
        "{} scope={scope} log_entries={} versions={} revert_hunks={} bytes={} attributions={} undo_batches={} read_bases={} blob_versions={} blobs_removed={} conflict_copies={}",
        key_label(key),
        c.log_entries,
        c.versions,
        c.revert_hunks,
        c.bytes,
        c.attributions,
        c.undo_batches,
        c.read_bases,
        c.blob_versions,
        c.blobs_removed,
        c.conflict_copies,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_round_trip_and_reject_junk() {
        for p in [Principal::Person("owner".into()), Principal::Agent("abc-123".into())] {
            assert_eq!(Principal::parse(&p.pid()), Some(p));
        }
        for junk in ["", "person:", "agent:", "kit:import", "owner", "user:bob"] {
            assert_eq!(Principal::parse(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn unknown_pids_get_nothing() {
        let key = DocKey { space: "ws".into(), path: "a.md".into() };
        assert_eq!(rights_for("user:bob", &key, None), DocRights::NONE);
        assert_eq!(rights_for("kit:import", &key, None), DocRights::NONE);
    }

    #[test]
    fn the_owner_has_every_right_and_an_agent_has_its_own_workspace() {
        let key = DocKey { space: "ws-a".into(), path: "a.md".into() };
        assert_eq!(rights_for("person:owner", &key, None), DocRights::OWNER);
        assert_eq!(rights_for("agent:ws-a", &key, None), DocRights::FULL);
    }

    #[test]
    fn only_the_box_owner_holds_the_doc_owners_acts_never_an_agent() {
        let key = DocKey { space: "ws-a".into(), path: "a.png".into() };
        assert!(rights_for("person:owner", &key, None).owner);
        assert!(!rights_for("agent:ws-a", &key, None).owner, "an agent in its own workspace is not the doc owner");
        assert!(DocRights::OWNER.kit().owner, "owner reaches the kit");
        assert!(!DocRights::FULL.kit().owner, "owner is never implied");
    }

    #[test]
    fn kit_rights_follow_read_edit_and_history() {
        assert_eq!(DocRights::FULL.kit(), Rights::full());
        let r = |read, edit, history, owner| DocRights { read, edit, history, owner, comment: false };
        assert_eq!(r(true, false, true, false).kit(), Rights::read_only().with_history(true));
        assert_eq!(r(true, true, false, false).kit(), Rights::editor());
        assert_eq!(r(false, true, true, true).kit(), Rights::none());
    }

    #[test]
    fn a_purge_is_audited_as_numbers_never_content() {
        let key = DocKey { space: "ws".into(), path: "notes/a.md".into() };
        let mut c = PurgeCounts::default();
        c.bytes = 42;
        c.blob_versions = 2;
        let line = purged_outcome(&key, &PurgedScope::Text { bytes: 42 }, &c);
        assert_eq!(
            line,
            "ws:notes/a.md scope=text:42B log_entries=0 versions=0 revert_hunks=0 bytes=42 attributions=0 undo_batches=0 read_bases=0 blob_versions=2 blobs_removed=0 conflict_copies=0"
        );
        let blob = purged_outcome(&key, &PurgedScope::BlobVersions { version: None }, &c);
        assert!(blob.contains("scope=blob_versions:all_but_current"), "{blob}");
    }

    #[test]
    fn a_disk_replica_repair_is_logged_as_a_warn_line_with_lengths_only() {
        REPAIRED_LINES.with(|v| v.borrow_mut().clear());
        let key = DocKey { space: "proj-123".into(), path: "notes/a.md".into() };
        let ev = AuditEvent::DiskReplicaRepaired { key, doc_len: 120, replica_len: 87 };
        K2Host::default().on_event(&ev);
        let got = REPAIRED_LINES.with(|v| v.borrow().clone());
        assert_eq!(
            got,
            vec![
                "[docs] WARN: disk replica repaired key=proj-123:notes/a.md doc_len=120 replica_len=87 (kit bug: report it to K2 Cores)"
                    .to_string()
            ],
            "the event must reach its own arm (not the catch-all) and log exactly one line"
        );
    }

    #[test]
    fn a_disk_replica_mismatch_is_logged_as_a_warn_line_with_metadata_only() {
        REPAIRED_LINES.with(|v| v.borrow_mut().clear());
        let key = DocKey { space: "proj-123".into(), path: "notes/a.md".into() };
        let ev = AuditEvent::DiskReplicaMismatch { key, version: 42 };
        K2Host::default().on_event(&ev);
        let got = REPAIRED_LINES.with(|v| v.borrow().clone());
        assert_eq!(
            got,
            vec![
                "[docs] WARN: disk replica mismatch key=proj-123:notes/a.md version=42 (kit bug: report it to K2 Cores)"
                    .to_string()
            ],
            "the event must reach its own arm (not the catch-all) and log exactly one line"
        );
    }
}
