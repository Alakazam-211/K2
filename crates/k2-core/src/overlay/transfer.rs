//! A clone carries its Threads (prd-clone-carries-threads-and-vault-v1 §7,
//! slice B; vs-live CL5–CL11, CL18, CL30, CL31).
//!
//! **Export** (source daemon): every Thread conversation the workspace owns
//! (its pinned Chat, its sidecars, older keys) as `threads/threads.json`
//! (`k2-threads-export/1`). Thread collection only: Chatter, ChatterLog and
//! collected `gb:` conversations never travel. `inject` is dropped, text is
//! re-run through the redactor (a belt), card expiry rows stay behind.
//! Other people's posts travel too (Rosson R2) and become historical names.
//!
//! **Import** (destination daemon, after the clone's identity is applied):
//! each conversation lands under the clone's pin (`canonical`), its own
//! sidecar key when the transcript travelled (`sidecar`, handle + old names
//! recreated), or `import:<uuid>` (`pending_canonical` until the clone has
//! a pin; `detached` read-only history otherwise). Every line gets a new id
//! and seq, an [`ImportedMeta`] with the originals, and an imported author
//! so nobody here sees the source owner's posts as "You". Open cards are
//! closed. A divider line ("Imported from `sales` on rosson.k2.dev, …")
//! sits above each imported history; the Thread continues live below it.
//! Re-runs are no-ops (redb `import_ids`, keyed per line). Nothing here
//! wakes an agent, injects into a PTY, pushes or federates.
//!
//! **Settle** ([`settle_pending`]): when the clone first gets a pin, the
//! pending conversation moves onto it ([`super::move_conversation`]).

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::doc::{ImportedMeta, OverlayDoc, PostAuthor, AUTHOR_GUEST, AUTHOR_IMPORTED, AUTHOR_OWNER, AUTHOR_USER};
use super::{catalog, store, WRITE};

/// `threads.json` format id. A later format is refused, never guessed at.
pub const EXPORT_FORMAT: &str = "k2-threads-export/1";
/// Where the export sits inside a clone bundle.
pub const THREADS_ARCHIVE_PATH: &str = "threads/threads.json";
/// Most lines one export carries (oldest dropped first, reported).
pub const MAX_EXPORT_DOCS: usize = 50_000;
/// Largest export (JSON bytes); also the unpack read cap.
pub const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;
/// Conversation keys K2 mints for imports that have no pin / no sidecar.
pub const IMPORT_KEY_PREFIX: &str = "import:";
/// Error text for a write to imported history. Routes map it to 409
/// `imported_read_only` (CL31).
pub const IMPORTED_READ_ONLY: &str = "imported Thread lines are read-only";
/// Note on a card that was still open on the source.
pub const CARRIED_OVER_NOTE: &str = "carried over: answer on the source";

pub const ROLE_CANONICAL: &str = "canonical";
pub const ROLE_PENDING: &str = "pending_canonical";
pub const ROLE_SIDECAR: &str = "sidecar";
pub const ROLE_DETACHED: &str = "detached";

/// Whether a clone brings its Threads. Wire: `"all"` | `"none"` |
/// `{"sinceDays": N}`. Absent on a request = [`ThreadsChoice::All`]
/// ("Bring the Thread" is ticked by default, D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThreadsChoice {
    #[default]
    All,
    None,
    SinceDays(u32),
}

impl ThreadsChoice {
    pub fn includes_threads(&self) -> bool {
        !matches!(self, ThreadsChoice::None)
    }

    pub fn since_days(&self) -> Option<u32> {
        match self {
            ThreadsChoice::SinceDays(d) => Some(*d),
            _ => None,
        }
    }
}

impl Serialize for ThreadsChoice {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ThreadsChoice::All => s.serialize_str("all"),
            ThreadsChoice::None => s.serialize_str("none"),
            ThreadsChoice::SinceDays(d) => {
                use serde::ser::SerializeMap;
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("sinceDays", d)?;
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ThreadsChoice {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Since {
            #[serde(rename = "sinceDays")]
            since_days: u32,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Word(String),
            Bool(bool),
            Since(Since),
        }
        match Wire::deserialize(d)? {
            Wire::Word(w) => match w.trim().to_ascii_lowercase().as_str() {
                "all" => Ok(ThreadsChoice::All),
                "none" => Ok(ThreadsChoice::None),
                other => Err(serde::de::Error::custom(format!(
                    "threads must be \"all\", \"none\" or {{\"sinceDays\": N}}, got \"{other}\""
                ))),
            },
            Wire::Bool(true) => Ok(ThreadsChoice::All),
            Wire::Bool(false) => Ok(ThreadsChoice::None),
            Wire::Since(Since { since_days: 0 }) => {
                Err(serde::de::Error::custom("threads.sinceDays must be at least 1"))
            }
            Wire::Since(Since { since_days }) => Ok(ThreadsChoice::SinceDays(since_days)),
        }
    }
}

// ── export file ───────────────────────────────────────────────────────

/// `threads/threads.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsExport {
    pub format: String,
    pub clone_id: String,
    pub source_host: String,
    pub source_workspace_id: String,
    pub source_handle: String,
    /// RFC 3339.
    pub exported_at: String,
    pub limits: ExportLimits,
    pub conversations: Vec<ExportedConversation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLimits {
    pub max_docs: usize,
    pub max_bytes: usize,
    pub since_days: Option<u32>,
    /// Lines left out by the caps (oldest first).
    pub dropped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedConversation {
    /// `canonical` | `sidecar` | `detached`.
    pub role: String,
    pub source_conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// A sidecar's `ws/<ordinal>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u32>,
    /// A sidecar's current handle (Chats name slug).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A sidecar's older names (0135 aliases).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub docs: Vec<ExportedDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedDoc {
    pub seq: i64,
    pub doc: OverlayDoc,
}

/// The manifest's optional `threads` summary (names only, counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsSummary {
    pub conversations: usize,
    pub docs: usize,
    pub bytes: usize,
    pub dropped: usize,
}

impl ThreadsExport {
    pub fn summary(&self, bytes: usize) -> ThreadsSummary {
        ThreadsSummary {
            conversations: self.conversations.len(),
            docs: self.conversations.iter().map(|c| c.docs.len()).sum(),
            bytes,
            dropped: self.limits.dropped,
        }
    }

    /// Distinct people who posted (labels as the source shows them: the
    /// owner's display name, Connect logins, `guest <name>`). For the
    /// dialog's "Includes messages from" line.
    pub fn participants(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in &self.conversations {
            for d in &c.docs {
                if !is_person_post(&d.doc) {
                    continue;
                }
                let label = match &d.doc.author {
                    Some(a) if a.kind == AUTHOR_GUEST => format!("guest {}", a.login),
                    Some(a) if a.kind == AUTHOR_OWNER => "owner".to_string(),
                    Some(a) => a.login.clone(),
                    None => d.doc.from.clone(),
                };
                if !label.trim().is_empty() && !out.contains(&label) {
                    out.push(label);
                }
            }
        }
        out
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| format!("threads export json: {e}"))
    }

    /// Parse `threads.json`; a later or unknown format is refused.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_EXPORT_BYTES {
            return Err(format!("threads export is {} bytes, over the {MAX_EXPORT_BYTES} cap", bytes.len()));
        }
        let v: ThreadsExport = serde_json::from_slice(bytes).map_err(|e| format!("threads export parse: {e}"))?;
        if v.format != EXPORT_FORMAT {
            return Err(format!("threads export format '{}' is not {EXPORT_FORMAT}", v.format));
        }
        Ok(v)
    }
}

/// How to build an export.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Manifest `clone_id` (or an export id for `k2 thread export`).
    pub clone_id: String,
    /// This server's name as the destination will show it.
    pub source_host: String,
    pub since_days: Option<u32>,
    pub max_docs: usize,
    pub max_bytes: usize,
    /// Unix seconds (the `sinceDays` cut).
    pub now: i64,
}

impl ExportOptions {
    pub fn new(clone_id: impl Into<String>, source_host: impl Into<String>, choice: ThreadsChoice) -> Self {
        Self {
            clone_id: clone_id.into(),
            source_host: source_host.into(),
            since_days: choice.since_days(),
            max_docs: MAX_EXPORT_DOCS,
            max_bytes: MAX_EXPORT_BYTES,
            now: super::now_secs(),
        }
    }
}

/// This server as an import divider names it: its K2 Connect host
/// (`rosson.k2.dev`), else the machine's host name, else "this computer".
pub fn default_source_host() -> String {
    let sub = crate::federation::address_notice::local_address().subdomain;
    let sub = sub.trim().trim_end_matches(".k2.dev");
    if !sub.is_empty() {
        return format!("{sub}.k2.dev");
    }
    machine_host_name().unwrap_or_else(|| "this computer".to_string())
}

#[cfg(unix)]
fn machine_host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for buf.len() bytes; gethostname NUL-terminates
    // on success when the name fits.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let s = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(unix))]
fn machine_host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().filter(|s| !s.trim().is_empty())
}

fn is_person_post(doc: &OverlayDoc) -> bool {
    doc.author.is_some() || doc.via.as_deref() == Some("compose")
}

/// Is `conversation_id` a key K2 minted for an import?
pub fn is_import_key(conversation_id: &str) -> bool {
    conversation_id.trim().starts_with(IMPORT_KEY_PREFIX)
}

/// Every Thread conversation `project_id` owns, as a `k2-threads-export/1`
/// file. Thread collection only (never Chatter / ChatterLog / `gb:`).
pub fn export_project_threads(
    conn: &Connection,
    project_id: &str,
    opts: &ExportOptions,
) -> Result<ThreadsExport, String> {
    let handle = crate::workspace_session_handles::workspace_address_name(conn, project_id)?;
    let pin_row = crate::db::schema::WorkspaceSession::get(conn, project_id).map_err(|e| format!("pin lookup: {e}"))?;
    let harness = pin_row.as_ref().map(|r| r.harness.clone()).filter(|h| !h.trim().is_empty());
    let pin = pin_row.and_then(|r| r.session_id).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    // With no pin yet, a pending import is this workspace's main Thread.
    let pending: Option<String> = if pin.is_none() { pending_canonical_for(conn, project_id)? } else { None };

    let mut keys: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT conversation_id FROM overlay_conversations WHERE project_id = ?1 ORDER BY conversation_id")
            .map_err(|e| format!("export catalog: {e}"))?;
        let rows = stmt
            .query_map(params![project_id], |r| r.get::<_, String>(0))
            .map_err(|e| format!("export catalog: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("export catalog row: {e}"))?
    };
    // The pinned Chat first, then the rest in a stable order.
    if let Some(p) = pin.as_ref().or(pending.as_ref()) {
        if let Some(i) = keys.iter().position(|k| k == p) {
            let k = keys.remove(i);
            keys.insert(0, k);
        }
    }
    let handles: HashMap<String, u32> = {
        let mut stmt = conn
            .prepare("SELECT conversation_key, ordinal FROM workspace_session_handles WHERE project_id = ?1")
            .map_err(|e| format!("export handles: {e}"))?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32)))
            .map_err(|e| format!("export handles: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("export handles row: {e}"))?
    };
    let cut = opts.since_days.map(|d| opts.now - i64::from(d) * 86_400);

    let mut convs: Vec<ExportedConversation> = Vec::new();
    for key in keys {
        if key.starts_with("gb:") {
            continue;
        }
        let is_main = pin.as_deref() == Some(key.as_str()) || pending.as_deref() == Some(key.as_str());
        let (role, ordinal, name, aliases) = if is_main {
            (ROLE_CANONICAL, None, None, Vec::new())
        } else if let Some(ord) = handles.get(&key) {
            let current = crate::workspace_session_handles::current_handle_for(conn, project_id, &key)?;
            let past = crate::workspace_session_handles::past_handles_for(conn, project_id, &key)?;
            let aliases: Vec<String> = past
                .into_iter()
                .filter(|h| !crate::workspace_session_handles::is_ordinal_token(h))
                .collect();
            (ROLE_SIDECAR, Some(*ord), current.filter(|c| c != &ord.to_string()), aliases)
        } else {
            (ROLE_DETACHED, None, None, Vec::new())
        };
        let mut docs = Vec::new();
        for item in store::read_thread(&key, 0)? {
            let doc = item.doc;
            if doc.kind == "chatter" || doc.external.is_some() {
                continue;
            }
            if cut.is_some_and(|c| doc.created_at < c) {
                continue;
            }
            docs.push(ExportedDoc { seq: item.seq, doc: clean_for_export(doc) });
        }
        if docs.is_empty() {
            continue;
        }
        let session_id = if is_import_key(&key) { None } else { Some(key.clone()) };
        convs.push(ExportedConversation {
            role: role.to_string(),
            source_conversation_id: key,
            session_id,
            harness: if is_main { harness.clone() } else { None },
            ordinal,
            name,
            aliases,
            docs,
        });
    }

    let dropped = apply_caps(&mut convs, opts.max_docs, opts.max_bytes)?;
    Ok(ThreadsExport {
        format: EXPORT_FORMAT.to_string(),
        clone_id: opts.clone_id.clone(),
        source_host: opts.source_host.clone(),
        source_workspace_id: project_id.to_string(),
        source_handle: handle,
        exported_at: chrono::Utc::now().to_rfc3339(),
        limits: ExportLimits {
            max_docs: opts.max_docs,
            max_bytes: opts.max_bytes,
            since_days: opts.since_days,
            dropped,
        },
        conversations: convs,
    })
}

/// Belt (CL9): no `inject`, text through the vault redactor.
fn clean_for_export(mut doc: OverlayDoc) -> OverlayDoc {
    doc.inject = None;
    doc.body = crate::redaction::redact_opt(doc.body.as_deref());
    if let Some(c) = doc.choice.as_mut() {
        c.prompt = crate::redaction::redact(&c.prompt);
        if let Some(a) = c.answer.as_deref() {
            c.answer = Some(crate::redaction::redact(a));
        }
    }
    if let Some(s) = doc.secret.as_mut() {
        s.prompt = crate::redaction::redact_opt(s.prompt.as_deref());
    }
    doc
}

/// Drop the oldest lines (across all conversations) until both caps hold.
/// Returns how many were dropped; empty conversations go too.
fn apply_caps(convs: &mut Vec<ExportedConversation>, max_docs: usize, max_bytes: usize) -> Result<usize, String> {
    // Envelope slack for the file's own fields.
    const SLACK: usize = 64 * 1024;
    let budget = max_bytes.saturating_sub(SLACK);
    let mut rows: Vec<(i64, usize, i64, usize)> = Vec::new(); // (created_at, conv, seq, bytes)
    let mut total = 0usize;
    for (ci, c) in convs.iter().enumerate() {
        for d in &c.docs {
            let n = serde_json::to_vec(d).map_err(|e| format!("threads export size: {e}"))?.len() + 1;
            total += n;
            rows.push((d.doc.created_at, ci, d.seq, n));
        }
    }
    let mut count = rows.len();
    if count <= max_docs && total <= budget {
        return Ok(0);
    }
    rows.sort();
    let mut drop: HashSet<(usize, i64)> = HashSet::new();
    for (_, ci, seq, n) in rows {
        if count <= max_docs && total <= budget {
            break;
        }
        drop.insert((ci, seq));
        count -= 1;
        total -= n;
    }
    for (ci, c) in convs.iter_mut().enumerate() {
        c.docs.retain(|d| !drop.contains(&(ci, d.seq)));
    }
    convs.retain(|c| !c.docs.is_empty());
    Ok(drop.len())
}

// ── import ────────────────────────────────────────────────────────────

/// Where an import lands and what travelled with it.
#[derive(Debug, Clone, Default)]
pub struct ImportContext {
    /// The clone's project on this server.
    pub project_id: String,
    /// The clone's handle here (`sales-2`). None = read it from the project.
    pub clone_handle: Option<String>,
    /// Provider conversation / session ids whose transcript files travelled
    /// (a sidecar keeps its address only then; else it is detached).
    pub travelled_sessions: HashSet<String>,
    /// Vault secret names the clone copied (slice A): their `k2v://ws/<src>/NAME`
    /// refs on secret cards point at the clone. Others stay as text.
    pub copied_secret_names: HashSet<String>,
}

/// One imported conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedConversation {
    pub local_conversation_id: String,
    pub role: String,
    pub source_conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub imported: usize,
    pub skipped: usize,
}

/// What [`import_threads`] did. Names and counts only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub clone_id: String,
    pub source_host: String,
    pub source_handle: String,
    pub conversations: Vec<ImportedConversation>,
    /// Lines written (dividers not counted).
    pub docs_imported: usize,
    /// Lines already there (re-run).
    pub docs_skipped: usize,
    /// The main Thread waits for the clone's first pin.
    pub pending: bool,
    pub detached: usize,
}

/// Import `export` into `ctx.project_id`. Idempotent per line. Run after
/// the clone's identity (pin, chat names) is applied.
pub fn import_threads(conn: &Connection, export: &ThreadsExport, ctx: &ImportContext) -> Result<ImportReport, String> {
    if export.format != EXPORT_FORMAT {
        return Err(format!("threads export format '{}' is not {EXPORT_FORMAT}", export.format));
    }
    let project_id = ctx.project_id.trim();
    if project_id.is_empty() {
        return Err("threads import: project_id required".to_string());
    }
    let clone_handle = match ctx.clone_handle.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => h.to_string(),
        None => crate::workspace_session_handles::workspace_address_name(conn, project_id)?,
    };
    let now = super::now_secs();
    let mut report = ImportReport {
        clone_id: export.clone_id.clone(),
        source_host: export.source_host.clone(),
        source_handle: export.source_handle.clone(),
        ..Default::default()
    };
    let rw = Rewrite { export, clone_handle: &clone_handle, copied: &ctx.copied_secret_names, now };

    for conv in &export.conversations {
        if conv.docs.is_empty() {
            continue;
        }
        let existing = import_row(conn, project_id, &export.clone_id, &conv.source_conversation_id)?;
        let (local, role) = place_conversation(conn, project_id, conv, existing.as_ref(), ctx)?;
        let entries: Vec<(String, OverlayDoc)> = conv
            .docs
            .iter()
            .map(|d| (import_key(project_id, &export.source_host, &d.doc.id), rw.doc(&d.doc)))
            .collect();
        let divider = rw.divider(conv, project_id);

        let write = {
            let _guard = WRITE.lock();
            conn.execute_batch("SAVEPOINT overlay_import").map_err(|e| format!("overlay import savepoint: {e}"))?;
            let res = (|| -> Result<store::ImportWrite, String> {
                if let Some((owner, _, _)) = catalog::get(conn, &local)? {
                    if owner != project_id {
                        return Err(format!("conversation {local} belongs to another workspace"));
                    }
                }
                catalog::ensure_conversation(conn, &local, project_id)?;
                let start_after = catalog::get(conn, &local)?.map(|r| r.1).unwrap_or(0);
                store::import_docs(&local, start_after, &entries, divider, |w| {
                    conn.execute(
                        "UPDATE overlay_conversations SET last_thread_seq = MAX(last_thread_seq, ?2) WHERE conversation_id = ?1",
                        params![local, w.last_seq],
                    )
                    .map_err(|e| format!("overlay import catalog: {e}"))?;
                    upsert_import_row(conn, project_id, &local, role, export, conv, w)
                })
            })();
            match res {
                Ok(w) => {
                    conn.execute_batch("RELEASE overlay_import").map_err(|e| format!("overlay import release: {e}"))?;
                    w
                }
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK TO overlay_import; RELEASE overlay_import");
                    return Err(e);
                }
            }
        };
        let lines = lines_written(&write, entries.len());
        report.docs_imported += lines;
        report.docs_skipped += write.skipped;
        report.pending |= role == ROLE_PENDING;
        report.detached += usize::from(role == ROLE_DETACHED);
        report.conversations.push(ImportedConversation {
            local_conversation_id: local,
            role: role.to_string(),
            source_conversation_id: conv.source_conversation_id.clone(),
            name: conv.name.clone(),
            imported: lines,
            skipped: write.skipped,
        });
    }
    // A pin may already be known (the clone launched in between): settle now.
    if report.pending {
        for s in settle_pending(conn, project_id)? {
            for c in report.conversations.iter_mut().filter(|c| c.local_conversation_id == s.from) {
                c.local_conversation_id = s.pin.clone();
                c.role = ROLE_CANONICAL.to_string();
            }
        }
        report.pending = report.conversations.iter().any(|c| c.role == ROLE_PENDING);
    }
    Ok(report)
}

/// Lines written minus the divider (written first, only with ≥1 new line).
fn lines_written(w: &store::ImportWrite, entries: usize) -> usize {
    let new_lines = entries - w.skipped;
    debug_assert!(w.written == new_lines || w.written == new_lines + 1);
    new_lines.min(w.written)
}

/// `{project_id}/{source_host}/{source_doc_id}`.
pub fn import_key(project_id: &str, source_host: &str, source_doc_id: &str) -> String {
    format!("{project_id}/{source_host}/{source_doc_id}")
}

/// Where a source conversation lands here, and its role (CL5, CL11, CL29).
fn place_conversation(
    conn: &Connection,
    project_id: &str,
    conv: &ExportedConversation,
    existing: Option<&ImportRow>,
    ctx: &ImportContext,
) -> Result<(String, &'static str), String> {
    // A re-run lands where the first run did (a settled pending row now
    // names the pin).
    if let Some(row) = existing {
        let role = match row.role.as_str() {
            ROLE_CANONICAL => ROLE_CANONICAL,
            ROLE_PENDING => ROLE_PENDING,
            ROLE_SIDECAR => ROLE_SIDECAR,
            _ => ROLE_DETACHED,
        };
        return Ok((row.local_conversation_id.clone(), role));
    }
    let owned_elsewhere = |key: &str| -> Result<bool, String> {
        Ok(catalog::get(conn, key)?.is_some_and(|(owner, _, _)| owner != project_id))
    };
    match conv.role.as_str() {
        ROLE_CANONICAL => {
            let pin = crate::db::schema::WorkspaceSession::get(conn, project_id)
                .map_err(|e| format!("pin lookup: {e}"))?
                .and_then(|r| r.session_id)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && !is_import_key(s));
            match pin {
                Some(pin) if !owned_elsewhere(&pin)? => Ok((pin, ROLE_CANONICAL)),
                _ => Ok((new_import_key(), ROLE_PENDING)),
            }
        }
        ROLE_SIDECAR => {
            let key = conv.source_conversation_id.trim();
            let travelled = ctx.travelled_sessions.contains(key)
                || conv.session_id.as_deref().is_some_and(|s| ctx.travelled_sessions.contains(s));
            let taken_here = crate::db::schema::WorkspaceSession::get(conn, project_id)
                .map_err(|e| format!("pin lookup: {e}"))?
                .and_then(|r| r.session_id)
                .is_some_and(|s| s.trim() == key);
            if !travelled || key.is_empty() || is_import_key(key) || taken_here || owned_elsewhere(key)? {
                return Ok((new_import_key(), ROLE_DETACHED));
            }
            restore_sidecar_handle(conn, project_id, key, conv)?;
            Ok((key.to_string(), ROLE_SIDECAR))
        }
        _ => Ok((new_import_key(), ROLE_DETACHED)),
    }
}

fn new_import_key() -> String {
    format!("{IMPORT_KEY_PREFIX}{}", uuid::Uuid::new_v4())
}

/// Recreate a sidecar's handle row (its source ordinal when free) and its
/// older names (aliases not already reserved to another chat here).
fn restore_sidecar_handle(conn: &Connection, project_id: &str, key: &str, conv: &ExportedConversation) -> Result<(), String> {
    use crate::workspace_session_handles as h;
    if h::get(conn, project_id, key)?.is_none() {
        let placed = match conv.ordinal {
            Some(ord) if ord > 0 && h::get_by_ordinal(conn, project_id, ord)?.is_none() => conn
                .execute(
                    "INSERT INTO workspace_session_handles (project_id, conversation_key, ordinal) VALUES (?1, ?2, ?3)",
                    params![project_id, key, ord as i64],
                )
                .map(|_| true)
                .map_err(|e| format!("restore sidecar handle: {e}"))?,
            _ => false,
        };
        if !placed {
            h::allocate_ordinal(conn, project_id, key)?;
        }
    }
    let mut names: Vec<&str> = conv.aliases.iter().map(String::as_str).collect();
    // Its current name answers through `chat_session_names` when that pin
    // travelled; keep it reserved to this chat either way.
    if let Some(n) = conv.name.as_deref() {
        names.push(n);
    }
    for slug in names {
        let slug = slug.trim();
        if slug.is_empty() || h::is_ordinal_token(slug) {
            continue;
        }
        match h::alias_owner(conn, project_id, slug)? {
            Some(owner) if owner != key => continue,
            _ => h::retire_slug(conn, project_id, slug, key)?,
        }
    }
    Ok(())
}

/// The field rewrites of §7.3 step 4.
struct Rewrite<'a> {
    export: &'a ThreadsExport,
    clone_handle: &'a str,
    copied: &'a HashSet<String>,
    now: i64,
}

impl Rewrite<'_> {
    fn meta(&self, source_doc_id: &str) -> ImportedMeta {
        ImportedMeta {
            clone_id: self.export.clone_id.clone(),
            source_host: self.export.source_host.clone(),
            source_handle: self.export.source_handle.clone(),
            source_doc_id: source_doc_id.to_string(),
            imported_at: self.now,
            ..Default::default()
        }
    }

    /// `sales` → `sales-2`, `sales/reviewer` → `sales-2/reviewer`.
    fn address(&self, addr: &str) -> Option<String> {
        let src = self.export.source_handle.as_str();
        if src.is_empty() || src == self.clone_handle {
            return None;
        }
        if addr == src {
            return Some(self.clone_handle.to_string());
        }
        addr.strip_prefix(src)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(|rest| format!("{}/{rest}", self.clone_handle))
    }

    /// The historical name a person's post shows here.
    fn person(&self, author: Option<&PostAuthor>, from: &str) -> PostAuthor {
        let host = self.export.source_host.as_str();
        let from = from.trim();
        match author {
            Some(a) if a.kind == AUTHOR_IMPORTED => a.clone(),
            Some(a) if a.kind == AUTHOR_OWNER => {
                let name = a
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("owner"))
                    .or_else(|| (!from.is_empty() && !from.eq_ignore_ascii_case("owner")).then_some(from));
                let label = match name {
                    Some(n) => n.to_string(),
                    None => format!("owner of {host}"),
                };
                PostAuthor::imported(&a.login, &label)
            }
            Some(a) if a.kind == AUTHOR_USER => PostAuthor::imported(&a.login, &format!("{} (Connect, {host})", a.login)),
            Some(a) if a.kind == AUTHOR_GUEST => PostAuthor::imported(&a.login, &format!("guest {} (App)", a.login)),
            Some(a) => PostAuthor::imported(&a.login, &format!("{} ({})", a.login, a.kind)),
            // Pre-author era compose post (CL7): the owner, by name.
            None => {
                let login = if from.is_empty() { "someone" } else { from };
                PostAuthor::imported(login, login)
            }
        }
    }

    fn doc(&self, src: &OverlayDoc) -> OverlayDoc {
        let mut doc = src.clone();
        let mut meta = self.meta(&src.id);
        doc.id = uuid::Uuid::new_v4().to_string();
        doc.inject = None;
        if is_person_post(src) {
            let who = self.person(src.author.as_ref(), &src.from);
            meta.author = src.author.clone();
            meta.from = Some(src.from.clone());
            doc.from = who.name.clone().unwrap_or_else(|| who.login.clone());
            doc.author = Some(who);
            if src.via.as_deref() == Some("compose") {
                // Out of compose history and the "via=compose is yours" rule.
                meta.via = src.via.clone();
                doc.via = Some("import".to_string());
            }
        } else if let Some(f) = self.address(&src.from) {
            meta.from = Some(src.from.clone());
            doc.from = f;
        }
        if let Some(t) = src.to.as_deref().and_then(|t| self.address(t)) {
            meta.to = src.to.clone();
            doc.to = Some(t);
        }
        if let Some(c) = doc.choice.as_mut() {
            if let Some(by) = c.answered_by.take() {
                meta.answered_by = Some(by.clone());
                c.answered_by = Some(self.person(Some(&by), by.name.as_deref().unwrap_or(&by.login)));
            }
            if c.status == "pending" {
                c.status = "voided".to_string();
                c.expires_at = None;
                meta.note = Some(CARRIED_OVER_NOTE.to_string());
            }
        }
        if let Some(s) = doc.secret.as_mut() {
            if let Some(by) = s.answered_by.take() {
                meta.answered_by = Some(by.clone());
                s.answered_by = Some(self.person(Some(&by), by.name.as_deref().unwrap_or(&by.login)));
            }
            if s.status == "pending" {
                s.status = "voided".to_string();
                s.expires_at = None;
                meta.note = Some(CARRIED_OVER_NOTE.to_string());
            }
            if let Some(r) = s.reference.as_deref() {
                s.reference = Some(self.secret_ref(r, &s.name));
            }
        }
        doc.imported = Some(meta);
        doc
    }

    /// `k2v://ws/<source>/NAME[@v]` → the clone's when NAME was copied.
    fn secret_ref(&self, r: &str, name: &str) -> String {
        if !self.copied.contains(name) {
            return r.to_string();
        }
        let Some(rest) = r.trim().strip_prefix("k2v://ws/") else {
            return r.to_string();
        };
        let Some((ws, tail)) = rest.split_once('/') else {
            return r.to_string();
        };
        let src = &self.export;
        if ws == src.source_handle || ws == src.source_workspace_id {
            let bare = tail.split('@').next().unwrap_or(tail);
            return format!("k2v://ws/{}/{bare}", self.clone_handle);
        }
        r.to_string()
    }

    /// "Imported from `sales` on rosson.k2.dev, 2026-10-16 (412 messages)",
    /// first above this conversation's imported lines (CL8: a plain text
    /// line from K2 on older viewers).
    fn divider(&self, conv: &ExportedConversation, project_id: &str) -> Option<(String, OverlayDoc)> {
        let first = conv.docs.iter().map(|d| d.doc.created_at).min()?;
        let n = conv.docs.len();
        let date = chrono::DateTime::from_timestamp(self.now, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let what = match conv.name.as_deref() {
            Some(name) if conv.role == ROLE_SIDECAR => format!("`{}/{name}`", self.export.source_handle),
            _ => format!("`{}`", self.export.source_handle),
        };
        let body = format!(
            "Imported from {what} on {}, {date} ({n} message{})",
            self.export.source_host,
            if n == 1 { "" } else { "s" }
        );
        let mut meta = self.meta("");
        meta.divider = true;
        let doc = OverlayDoc {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "text".to_string(),
            from: "K2".to_string(),
            to: Some(self.clone_handle.to_string()),
            created_at: first,
            body: Some(body),
            via: Some("import".to_string()),
            inject: None,
            choice: None,
            secret: None,
            widget: None,
            design: None,
            author: None,
            external: None,
            imported: Some(meta),
        };
        let key = import_key(
            project_id,
            &self.export.source_host,
            &format!("divider:{}:{}", self.export.clone_id, conv.source_conversation_id),
        );
        Some((key, doc))
    }
}

// ── overlay_imports rows ──────────────────────────────────────────────

/// One `overlay_imports` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRow {
    pub id: String,
    pub project_id: String,
    pub local_conversation_id: String,
    pub role: String,
    pub clone_id: String,
    pub source_host: String,
    pub source_handle: String,
    pub source_conversation_id: String,
    pub source_session_id: Option<String>,
    pub harness: Option<String>,
    pub name: Option<String>,
    pub doc_count: i64,
    pub skipped_count: i64,
    pub imported_at: i64,
    pub settled_at: Option<i64>,
}

const ROW_COLS: &str = "id, project_id, local_conversation_id, role, clone_id, source_host, source_handle, \
     source_conversation_id, source_session_id, harness, name, doc_count, skipped_count, imported_at, settled_at";

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<ImportRow> {
    Ok(ImportRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        local_conversation_id: r.get(2)?,
        role: r.get(3)?,
        clone_id: r.get(4)?,
        source_host: r.get(5)?,
        source_handle: r.get(6)?,
        source_conversation_id: r.get(7)?,
        source_session_id: r.get(8)?,
        harness: r.get(9)?,
        name: r.get(10)?,
        doc_count: r.get(11)?,
        skipped_count: r.get(12)?,
        imported_at: r.get(13)?,
        settled_at: r.get(14)?,
    })
}

fn import_row(conn: &Connection, project_id: &str, clone_id: &str, source_conversation_id: &str) -> Result<Option<ImportRow>, String> {
    conn.query_row(
        &format!(
            "SELECT {ROW_COLS} FROM overlay_imports WHERE project_id = ?1 AND clone_id = ?2 AND source_conversation_id = ?3"
        ),
        params![project_id, clone_id, source_conversation_id],
        row_of,
    )
    .optional()
    .map_err(|e| format!("overlay_imports get: {e}"))
}

fn upsert_import_row(
    conn: &Connection,
    project_id: &str,
    local: &str,
    role: &str,
    export: &ThreadsExport,
    conv: &ExportedConversation,
    w: store::ImportWrite,
) -> Result<(), String> {
    let lines = lines_written(&w, conv.docs.len()) as i64;
    conn.execute(
        "INSERT INTO overlay_imports (id, project_id, local_conversation_id, role, clone_id, source_host, \
         source_handle, source_conversation_id, source_session_id, harness, name, doc_count, skipped_count) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
         ON CONFLICT(project_id, clone_id, source_conversation_id) DO UPDATE SET \
           doc_count = doc_count + excluded.doc_count, skipped_count = excluded.skipped_count",
        params![
            uuid::Uuid::new_v4().to_string(),
            project_id,
            local,
            role,
            export.clone_id,
            export.source_host,
            export.source_handle,
            conv.source_conversation_id,
            conv.session_id,
            conv.harness,
            conv.name,
            lines,
            w.skipped as i64,
        ],
    )
    .map_err(|e| format!("overlay_imports upsert: {e}"))?;
    Ok(())
}

/// Every import row of a workspace (`k2 thread imports <ws>`), newest first.
pub fn list_imports(conn: &Connection, project_id: &str) -> Result<Vec<ImportRow>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ROW_COLS} FROM overlay_imports WHERE project_id = ?1 ORDER BY imported_at DESC, id"
        ))
        .map_err(|e| format!("overlay_imports list: {e}"))?;
    let rows = stmt.query_map(params![project_id], row_of).map_err(|e| format!("overlay_imports list: {e}"))?;
    rows.collect::<Result<_, _>>().map_err(|e| format!("overlay_imports row: {e}"))
}

/// The workspace an `import:<id>` key belongs to (CL30 addressing).
pub fn project_for_import_key(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    if !is_import_key(key) {
        return Ok(None);
    }
    conn.query_row(
        "SELECT project_id FROM overlay_imports WHERE local_conversation_id = ?1 LIMIT 1",
        params![key.trim()],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("overlay_imports by key: {e}"))
}

/// The pending main Thread of a workspace with no pin yet (newest), for a
/// read-only resolve instead of a 404 (CL5).
pub fn pending_canonical_for(conn: &Connection, project_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT local_conversation_id FROM overlay_imports WHERE project_id = ?1 AND role = 'pending_canonical' \
         ORDER BY imported_at DESC, id LIMIT 1",
        params![project_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("overlay_imports pending: {e}"))
}

/// Is this conversation (or this line in it) imported history that must
/// not be written (CL31)? `import:` keys are read-only whole; elsewhere
/// only lines with `imported`.
pub fn is_read_only(conversation_id: &str, doc_id: Option<&str>) -> Result<bool, String> {
    if is_import_key(conversation_id) {
        return Ok(true);
    }
    match doc_id.map(str::trim).filter(|d| !d.is_empty()) {
        Some(id) => Ok(store::get_doc(id)?.is_some_and(|d| d.imported.is_some())),
        None => Ok(false),
    }
}

// ── pending settle (CL5) ──────────────────────────────────────────────

/// A pending import that moved onto the clone's pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettledImport {
    pub project_id: String,
    pub from: String,
    pub pin: String,
    pub clone_id: String,
    pub source_host: String,
    pub source_handle: String,
}

static SETTLE_LISTENER: std::sync::OnceLock<fn(&SettledImport)> = std::sync::OnceLock::new();

/// Called after every settle (first call wins). The vault slice posts the
/// deferred "Leave blank" secret cards here (CL18).
pub fn set_settle_listener(f: fn(&SettledImport)) {
    let _ = SETTLE_LISTENER.set(f);
}

/// Move a workspace's `pending_canonical` imports onto its pin, once it has
/// one that no other workspace owns. Called from the canonical resolve,
/// `persist_session_identity` and a boot sweep. Hold the shared DB lock
/// (`conn`); takes the overlay write lock inside (DB → WRITE, like every
/// other overlay writer).
pub fn settle_pending(conn: &Connection, project_id: &str) -> Result<Vec<SettledImport>, String> {
    let rows: Vec<(String, String, String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, local_conversation_id, clone_id, source_host, source_handle FROM overlay_imports \
                 WHERE project_id = ?1 AND role = 'pending_canonical' ORDER BY imported_at, id",
            )
            .map_err(|e| format!("overlay_imports pending: {e}"))?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .map_err(|e| format!("overlay_imports pending: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("overlay_imports pending row: {e}"))?
    };
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let Some(pin) = crate::db::schema::WorkspaceSession::get(conn, project_id)
        .map_err(|e| format!("pin lookup: {e}"))?
        .and_then(|r| r.session_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !is_import_key(s))
    else {
        return Ok(Vec::new());
    };
    if catalog::get(conn, &pin)?.is_some_and(|(owner, _, _)| owner != project_id) {
        // Same-box hazard (CL11): never attach to another workspace's Thread.
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (id, from, clone_id, source_host, source_handle) in rows {
        super::move_conversation(conn, &from, &pin)?;
        conn.execute(
            "UPDATE overlay_imports SET role = 'canonical', local_conversation_id = ?2, settled_at = unixepoch() WHERE id = ?1",
            params![id, pin],
        )
        .map_err(|e| format!("overlay_imports settle: {e}"))?;
        let s = SettledImport { project_id: project_id.to_string(), from, pin: pin.clone(), clone_id, source_host, source_handle };
        if let Some(f) = SETTLE_LISTENER.get() {
            f(&s);
        }
        out.push(s);
    }
    Ok(out)
}

/// Boot sweep: settle every workspace with a pending import.
pub fn settle_all_pending(conn: &Connection) -> Result<Vec<SettledImport>, String> {
    let projects: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT project_id FROM overlay_imports WHERE role = 'pending_canonical'")
            .map_err(|e| format!("overlay_imports sweep: {e}"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("overlay_imports sweep: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("overlay_imports sweep row: {e}"))?
    };
    let mut out = Vec::new();
    for p in projects {
        out.extend(settle_pending(conn, &p)?);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
