//! Tier 0: multiplayer basics for **any** file (images, PDFs, office files, binaries) with
//! no type pack. A blob doc is a pointer to a blob ([`crate::blob`]); its versions are the
//! pointer's history. People and agents get presence, a lease, `write --base` with no
//! last-writer-wins, versions and restore. Desktop apps ignore leases, so a save that lands
//! under a lease (or over a version the file didn't have yet) is merged by a pack that can
//! (tier 0.5), and otherwise kept: the save becomes current, and what it replaced stays a
//! version **and** is written to a conflict copy (beside the file, or in the folder the host
//! chooses, [`Host::conflict_dir`]). Never lost.
//!
//! The contract is in `docs/SESSIONS.md`, "Tier 0".

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::config::Config;
use super::disk::{self, DiskCheck, DiskState, FileStat, FlushOutcome, Probe};
use super::error::{Result, SessionError};
use super::host::{Action, AuditEvent, Host, LeaseEnd, LeaveReason, Need, PurgedScope};
use super::live::{probe_error, Joined, NotReached, PurgeOutcome};
use super::pack::Declined;
use super::participant::{
    ConnId, Participant, ParticipantKind, RosterEntry, Stay, EXTERNAL_PID, IMPORT_PID,
};
use super::presence::{self, Presence};
use super::ratelimit::Bucket;

mod promote;
use crate::blob::{BlobError, BlobId, BlobInfo, BlobReader, BlobStore};
use crate::store::{
    Approval, ApprovalState, BlobReason, BlobSync, BlobVersion, DocId, DocKey, DocMeta, DocStore,
    PurgeCounts, StoreError, VersionId, BLOB_DOC_PACK,
};
pub use promote::{Promoted, Promotion};

/// The participant a pack's merge of a conflicting save is attributed to (tier 0.5).
pub const MERGE_PID: &str = "kit:merge";

/// Longest view region ([`BlobSession::set_region`]), in bytes.
pub const REGION_MAX_BYTES: usize = 256;

/// What a pack's three-way byte merge produced ([`BlobPack::merge3_bytes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobMerge {
    /// The merged file.
    Merged(Vec<u8>),
    /// The changes overlap: keep both (the conflict copy).
    Conflict,
    /// This pack can't merge these bytes.
    Unsupported,
}

/// What a file type adds to tier 0. Every method has a default, so tier 0 for any file is
/// [`AnyFile`]; a later pack (tier 0.5) supplies [`BlobPack::merge3_bytes`] to turn
/// conflicting saves into merges.
pub trait BlobPack: Send + Sync {
    /// Whether [`BlobPack::merge3_bytes`] can do anything (the kit then reads base, ours and
    /// theirs into memory, up to [`Config::blob_merge_max_bytes`] each). Default: no.
    fn can_merge(&self) -> bool {
        false
    }

    /// Merge `ours` (the doc's current version) and `theirs` (what another program saved)
    /// from `base` (what the file held before). Called with the session locked: bounded, no
    /// I/O. Default: [`BlobMerge::Unsupported`].
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        let _ = (base, ours, theirs);
        BlobMerge::Unsupported
    }

    /// The conflict copy's file name (in the conflict-copy folder: the file's own, or
    /// [`Host::conflict_dir`]'s) for the file `file_name`, made at `at_ms` (host clock, ms
    /// since the Unix epoch), holding `who`'s content. Default [`conflict_copy_name`]. A name
    /// with a path separator, or `.`/`..`, falls back to the default; an existing file is
    /// never overwritten (the kit adds ` 2`, ` 3`...).
    fn conflict_copy_name(&self, file_name: &str, at_ms: u64, who: &str) -> String {
        conflict_copy_name(file_name, at_ms, who)
    }
}

/// Tier 0 for any file: no merge, the default conflict-copy name.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyFile;

impl BlobPack for AnyFile {}

/// `name (conflict <who> YYYY-MM-DD HHMMSS).ext`, in UTC (K-A11Y-5). `who` is made safe for a
/// file name (no separators or control characters, at most 64 characters; `someone` when
/// empty).
pub fn conflict_copy_name(file_name: &str, at_ms: u64, who: &str) -> String {
    let (stem, ext) = split_ext(file_name);
    let secs = at_ms / 1000;
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let t = secs % 86_400;
    let who: String = who
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(64)
        .collect();
    let who = match who.trim() {
        "" => "someone",
        w => w,
    };
    format!(
        "{stem} (conflict {who} {y:04}-{mo:02}-{d:02} {:02}{:02}{:02}){ext}",
        t / 3600,
        t / 60 % 60,
        t % 60
    )
}

/// `("report", ".pdf")`; a leading dot is part of the stem (`.bashrc` has no extension).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant's algorithm).
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The doc's lease, as [`BlobSession::lease`] and the status show it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LeaseInfo {
    /// The holder (a pid: any of its connections may write, renew or release).
    pub pid: String,
    pub name: String,
    pub acquired_ms: u64,
    pub expires_ms: u64,
    /// Taken automatically for the length of one write.
    pub auto: bool,
}

/// Returned by [`BlobSession::read`]. Leases never block reads.
#[non_exhaustive]
pub struct BlobRead {
    pub version: VersionId,
    pub info: BlobVersion,
    /// The bytes, verified as they stream: only a clean end means they are good.
    pub reader: BlobReader,
}

/// Returned by [`BlobSession::write`] and [`BlobSession::restore`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobWriteResult {
    /// The current version after the call.
    pub version: VersionId,
    pub blob: BlobId,
    pub size: u64,
    /// False when the bytes equal the current version's (nothing new was recorded).
    pub changed: bool,
    /// Whether the file was written back (false without a file, when it is missing, or when
    /// the write-back failed: it retries on `tick`).
    pub written: bool,
}

/// Returned by [`BlobSession::set_approval`]: the version's approval record and, when it was
/// declined, the way back (K-HIST-19 for tier 0). A declined version stays: blobs have no
/// `revert`, so the person reverses it **explicitly** with `restore --version restore_to`;
/// the kit never restores on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobApproval {
    /// The version the approval is for.
    pub version: VersionId,
    pub approval: Approval,
    /// Declined: the version before it, whose content `restore` makes current again.
    /// `None` when approved, or when that version was purged (its content is gone).
    pub restore_to: Option<VersionId>,
    /// Declined: the versions written after it, oldest first (empty when approved). Blobs
    /// don't merge, so restoring `restore_to` also replaces **their** content as the current
    /// one; they stay versions (restorable), but say so before restoring.
    pub later: Vec<VersionId>,
}

/// One author of a blob doc's history, in [`BlobBlame::authors`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobAuthor {
    /// A pid, the kit's own (`kit:import`, `kit:external`), or an erased participant's
    /// pseudonym.
    pub pid: String,
    /// How many versions they set.
    pub versions: usize,
    /// When they last set one (host clock, ms).
    pub last_ms: u64,
}

/// Returned by [`BlobSession::blame`]: tier 0's blame is the pointer's history (the blob
/// is one opaque unit, K-HIST-11's "whole blob" grain). Metadata only, no content.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobBlame {
    /// The current version.
    pub version: VersionId,
    /// Who set it, when, through what, why, for whom (`author`, `at_ms`, `via`, `reason`,
    /// `on_behalf_of`, `restored_from`...).
    pub current: BlobVersion,
    /// How many versions the history holds (purged ones included: their entries stay).
    pub versions: usize,
    /// Everyone who set a version, most recent first.
    pub authors: Vec<BlobAuthor>,
    /// The approval state of the current version, when an agent wrote it on someone's
    /// behalf (as text blame's [`super::BlameLine::approval`]).
    pub approval: Option<ApprovalState>,
}

/// What a blob version was, in [`BlobLogEntry::kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobLogKind {
    /// The doc's first content (`kit:import`).
    Import,
    /// A participant's `write --base`.
    Write,
    /// A participant restored version [`BlobLogEntry::of`] (a forward change).
    Restore,
    /// Another program saved the file (`kit:external`).
    External,
    /// Another program's save that conflicted (under a lease, or over a version the file
    /// didn't have yet): it became current, and what it replaced (the previous version) was
    /// kept, also as [`BlobLogEntry::conflict_copy`]. A conflict whose copy couldn't be
    /// written logs as [`BlobLogKind::External`] (the store records the copy, not the
    /// conflict).
    ConflictKept,
    /// Tier 0.5: a pack merged a conflicting save (`kit:merge`; the save is the entry
    /// before it).
    Merged,
}

/// One entry of [`BlobSession::log`]: one version. Field names follow the text log's
/// (`ChangeInfo`, `VersionInfo`) where they mean the same. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobLogEntry {
    /// The version.
    pub id: VersionId,
    pub kind: BlobLogKind,
    /// For a restore, the version restored.
    pub of: Option<u64>,
    /// Who set it: a pid, or the kit's own (`kit:import`, `kit:external`, `kit:merge`).
    pub author: String,
    /// Their display name then.
    pub author_name: String,
    /// Surface: `"cli"`, `"tab"`, `"disk"`...
    pub via: String,
    /// When (host clock, ms).
    pub at_ms: u64,
    /// The person an agent wrote it for, as recorded on the version.
    pub on_behalf_of: Option<String>,
    /// Its name, if it was named.
    pub name: Option<String>,
    /// Purged: the entry stays, its content is gone.
    pub redacted: bool,
    /// Its size in bytes.
    pub size: u64,
    /// The file's modification time, when the content came from disk.
    pub mtime_ms: Option<u64>,
    /// The conflict copy this save made (resolved against the file's folder; the record as
    /// stored when the session has no file), whether or not it still exists.
    pub conflict_copy: Option<PathBuf>,
    /// The approval state of a version an agent wrote on someone's behalf (`log --pending`
    /// lists the `Pending` ones; [`BlobSession::set_approval`] settles them).
    pub approval: Option<ApprovalState>,
}

/// Returned by [`BlobSession::status`]. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobStatus {
    pub key: DocKey,
    pub version: VersionId,
    /// The current pointer: blob, size, mtime and who set it.
    pub current: BlobVersion,
    pub participants: usize,
    pub connections: usize,
    pub lease: Option<LeaseInfo>,
    /// `None` without a file.
    pub disk: Option<DiskState>,
    /// The doc's conflict copies that exist now (recorded by the kit; oldest first).
    pub conflict_copies: Vec<ConflictCopy>,
}

/// A conflict copy the kit wrote and recorded ([`BlobStatus::conflict_copies`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConflictCopy {
    /// Where it is now: the record resolved against the file's folder.
    pub path: PathBuf,
    /// The outside save that made it (the version that records it).
    pub version: VersionId,
    /// The version whose content it holds (what that save replaced).
    pub kept: VersionId,
    /// When it was written (host clock, ms).
    pub at_ms: u64,
}

/// A recorded conflict copy (from the versions), cached per session.
#[derive(Debug, Clone)]
struct CopyRec {
    version: VersionId,
    kept: VersionId,
    at_ms: u64,
    /// As recorded: a path relative to the file's folder (a plain file name when beside it,
    /// as every copy written by v0.3.x and earlier is).
    rec: String,
}

/// One connection to a blob doc: the shared presence core plus an opaque view region.
struct BMember {
    pres: Presence,
    region: Option<String>,
}

impl std::ops::Deref for BMember {
    type Target = Presence;
    fn deref(&self) -> &Presence {
        &self.pres
    }
}

impl std::ops::DerefMut for BMember {
    fn deref_mut(&mut self) -> &mut Presence {
        &mut self.pres
    }
}

struct BDisk {
    path: PathBuf,
    /// Canonical; the file must resolve inside it.
    root: PathBuf,
    /// What the file holds (as the kit last saw or wrote it).
    synced: BlobId,
    /// Recent own writes: a file holding one of them is an echo, never an outside edit.
    /// Cleared by every outside save taken in (v0.9.5: a write of ours from before a
    /// person's save, back again, is that person's save).
    own: VecDeque<BlobId>,
    /// The file as last seen, with git's racy mark (`disk::Seen`).
    stat: Option<disk::Seen>,
    missing: bool,
    /// Why the file can't be a blob doc now (too large); writes are refused.
    declined: Option<String>,
    dirty_since: Option<u64>,
    last_check_ms: u64,
}

struct BState {
    key: DocKey,
    /// Every App viewer's handles in this doc (K5).
    handles: super::handles::Handles,
    store: Option<Arc<DocStore>>,
    closed: Option<SessionError>,
    vid: VersionId,
    cur: BlobVersion,
    members: Vec<BMember>,
    joins: u64,
    lease: Option<LeaseInfo>,
    buckets: HashMap<String, Bucket>,
    disk: Option<BDisk>,
    /// The doc's recorded conflict copies (loaded from the versions on first use).
    copies: Option<Vec<CopyRec>>,
    /// Copies a person edited (their bytes no longer match their version), by path, at the
    /// stat they had: never deleted, and not re-hashed until they change.
    copies_edited: HashMap<PathBuf, disk::Seen>,
    /// When the conflict-copy cleanup last ran (host clock).
    last_cleanup_ms: Option<u64>,
}

const OWN_WRITES_KEPT: usize = 8;

/// `target` relative to `base` (both absolute and canonical): `..` up to their common
/// prefix, then down.
fn relative_path(base: &Path, target: &Path) -> PathBuf {
    let b: Vec<Component<'_>> = base.components().collect();
    let t: Vec<Component<'_>> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    out
}

/// A recorded copy's path: `rec` joined to the file's folder `base`, `..` resolved lexically
/// (`base` is canonical). An absolute `rec` stands on its own.
pub(super) fn resolve_copy(base: &Path, rec: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for c in Path::new(rec).components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A doc's blob versions as log entries, oldest first (a blob doc's `log`; a promoted
/// doc's tier-0 history). `base`: the file's folder, against which conflict copies resolve.
pub(super) fn blob_log(
    store: &DocStore,
    doc_id: DocId,
    base: Option<&Path>,
) -> Result<Vec<BlobLogEntry>> {
    let approvals: HashMap<VersionId, ApprovalState> = store
        .blob_approvals(doc_id)?
        .into_iter()
        .map(|(v, a)| (v, a.state))
        .collect();
    Ok(store
        .blob_versions(doc_id)?
        .into_iter()
        .map(|(id, v)| {
            let kind = match v.reason {
                BlobReason::Import => BlobLogKind::Import,
                BlobReason::Write => BlobLogKind::Write,
                BlobReason::Restore => BlobLogKind::Restore,
                BlobReason::External if v.conflict_copy.is_some() => BlobLogKind::ConflictKept,
                BlobReason::External => BlobLogKind::External,
                BlobReason::Merged => BlobLogKind::Merged,
            };
            let conflict_copy = v.conflict_copy.as_deref().map(|c| match base {
                Some(b) => resolve_copy(b, c),
                None => PathBuf::from(c),
            });
            BlobLogEntry {
                id,
                kind,
                of: v.restored_from,
                author: v.author,
                author_name: v.author_name,
                via: v.via,
                at_ms: v.at_ms,
                on_behalf_of: v.on_behalf_of,
                name: v.name,
                redacted: v.redacted,
                size: v.size,
                mtime_ms: v.mtime_ms,
                conflict_copy,
                approval: approvals.get(&id).copied(),
            }
        })
        .collect())
}

/// A loaded blob doc and the people and agents in it (tier 0). Thread-safe; every call
/// takes the session's lock, except a write's upload into the blob store. Get one from
/// [`super::SessionManager::open_blob`] or [`super::SessionManager::open_blob_file`].
///
/// Presence works as for text sessions (one roster entry per pid, connections, holds, away,
/// idle expiry, rights asked on every call); there are no carets, only an optional opaque
/// view region. The lease holder shows as an editor.
pub struct BlobSession {
    host: Arc<dyn Host>,
    config: Arc<Config>,
    blobs: Arc<BlobStore>,
    pack: Arc<dyn BlobPack>,
    doc_id: DocId,
    state: Mutex<BState>,
    last_used_ms: AtomicU64,
}

/// Copy a verified read to `out`, to its clean end (where the blob's check completes).
fn copy_verified(mut r: BlobRead, out: &mut dyn io::Write) -> Result<super::FileCopy> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut n: u64 = 0;
    loop {
        let k = match r.reader.read(&mut buf) {
            Ok(k) => k,
            Err(e) => return Err(r.reader.error(e).into()),
        };
        if k == 0 {
            break;
        }
        out.write_all(&buf[..k])
            .map_err(|e| BlobError::Io(format!("writing the copy: {e}")))?;
        n += k as u64;
    }
    out.flush()
        .map_err(|e| BlobError::Io(format!("writing the copy: {e}")))?;
    Ok(super::FileCopy {
        version: r.version,
        bytes: n,
        codec: super::FileCodec::Exact,
    })
}

fn blob_io(e: BlobError) -> io::Error {
    io::Error::other(e.to_string())
}

impl BlobSession {
    // ----- loading (crate-private; see SessionManager) -----

    #[allow(clippy::too_many_arguments)]
    fn build(
        host: Arc<dyn Host>,
        config: Arc<Config>,
        blobs: Arc<BlobStore>,
        pack: Arc<dyn BlobPack>,
        store: Arc<DocStore>,
        doc_id: DocId,
        key: DocKey,
        (vid, cur): (VersionId, BlobVersion),
    ) -> Self {
        let now = host.now_ms();
        Self {
            host,
            config,
            blobs,
            pack,
            doc_id,
            last_used_ms: AtomicU64::new(now),
            state: Mutex::new(BState {
                key,
                store: Some(store),
                closed: None,
                vid,
                cur,
                members: Vec::new(),
                joins: 0,
                lease: None,
                buckets: HashMap::new(),
                disk: None,
                copies: None,
                copies_edited: HashMap::new(),
                handles: Default::default(),
                last_cleanup_ms: None,
            }),
        }
    }

    pub(crate) fn load(
        host: Arc<dyn Host>,
        config: Arc<Config>,
        blobs: Arc<BlobStore>,
        pack: Arc<dyn BlobPack>,
        store: Arc<DocStore>,
        doc_id: DocId,
    ) -> Result<Self> {
        let meta = store.meta(doc_id)?;
        if meta.pack != BLOB_DOC_PACK {
            return Err(SessionError::PackMismatch {
                stored: meta.pack,
                pack: BLOB_DOC_PACK.to_owned(),
            });
        }
        let current = store.blob_current(doc_id)?;
        Ok(Self::build(
            host, config, blobs, pack, store, doc_id, meta.key, current,
        ))
    }

    /// Create a blob doc at `key` whose first version is the stored blob `info` (by
    /// [`IMPORT_PID`]), with the file's sync state when it came from a file, in one
    /// transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import(
        host: Arc<dyn Host>,
        config: Arc<Config>,
        blobs: Arc<BlobStore>,
        pack: Arc<dyn BlobPack>,
        store: Arc<DocStore>,
        key: DocKey,
        info: BlobInfo,
        from_file: Option<FileStat>,
    ) -> Result<Self> {
        let now = host.now_ms();
        let meta = DocMeta {
            key: key.clone(),
            pack: BLOB_DOC_PACK.to_owned(),
            engine: "none".to_owned(),
            engine_format: 0,
            codec: Vec::new(),
            created_ms: now,
            synced_hash: None,
            synced_seq: 0,
            declined: None,
        };
        let via = if from_file.is_some() {
            "disk"
        } else {
            "import"
        };
        let mut first = BlobVersion::new(
            info.id,
            info.size,
            now,
            IMPORT_PID,
            "import",
            via,
            BlobReason::Import,
        );
        first.mtime_ms = from_file.and_then(|s| s.mtime_ms());
        let sync = from_file.map(|s| BlobSync {
            blob: info.id,
            stat: disk::Seen::now(s).persisted(),
        });
        let (doc_id, vid) = store.create_blob_doc(&meta, &first, sync.as_ref())?;
        Ok(Self::build(
            host,
            config,
            blobs,
            pack,
            store,
            doc_id,
            key,
            (vid, first),
        ))
    }

    // ----- plumbing -----

    fn lock(&self) -> Result<MutexGuard<'_, BState>> {
        let g = self.lock_any()?;
        if let Some(e) = &g.closed {
            return Err(e.clone());
        }
        Ok(g)
    }

    fn lock_any(&self) -> Result<MutexGuard<'_, BState>> {
        self.state
            .lock()
            .map_err(|_| SessionError::Broken("session lock poisoned by a panic".into()))
    }

    fn now(&self) -> u64 {
        let now = self.host.now_ms();
        self.last_used_ms.fetch_max(now, Ordering::Relaxed);
        now
    }

    fn store(st: &BState) -> Result<Arc<DocStore>> {
        st.store.clone().ok_or(SessionError::Unloaded)
    }

    fn emit(&self, ev: AuditEvent) {
        self.host.on_event(&ev);
    }

    fn member(st: &BState, conn: ConnId) -> Result<usize> {
        st.members
            .iter()
            .position(|m| m.conn == conn)
            .ok_or(SessionError::NotJoined { conn })
    }

    fn remove_member(&self, st: &mut BState, i: usize, reason: LeaveReason) {
        let m = st.members.remove(i);
        self.emit(AuditEvent::Left {
            key: st.key.clone(),
            pid: m.pres.p.pid,
            conn: m.pres.conn,
            reason,
        });
    }

    /// Remove every connection of `pid` (kicked, or lost read): their lease ends too.
    fn remove_pid(&self, st: &mut BState, pid: &str, reason: LeaveReason) -> usize {
        let mut n = 0;
        while let Some(i) = st.members.iter().position(|m| m.p.pid == pid) {
            self.remove_member(st, i, reason);
            n += 1;
        }
        if st.lease.as_ref().is_some_and(|l| l.pid == pid) {
            self.end_lease(st, LeaseEnd::HolderRemoved);
        }
        n
    }

    fn admit(
        &self,
        st: &mut BState,
        conn: ConnId,
        need: Need,
        action: Action,
        now: u64,
    ) -> Result<usize> {
        let i = Self::member(st, conn)?;
        let rights = self.host.rights(&st.members[i].p, &st.key);
        let pid = st.members[i].p.pid.clone();
        for m in st.members.iter_mut().filter(|m| m.p.pid == pid) {
            m.rights = rights;
        }
        if let Some(n) = presence::refusal(rights, need) {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid: pid.clone(),
                action,
                need: n,
            });
            if n == Need::Read {
                self.remove_pid(st, &pid, LeaveReason::RightsLost);
            }
            return Err(SessionError::Forbidden { need: n });
        }
        st.members[i].last_active_ms = now;
        Ok(i)
    }

    /// Admit a write: edit rights, not declined, under the pid's rate limit.
    fn admit_write(
        &self,
        st: &mut BState,
        conn: ConnId,
        action: Action,
        now: u64,
    ) -> Result<usize> {
        let i = self.admit(st, conn, Need::Edit, action, now)?;
        if let Some(reason) = st.disk.as_ref().and_then(|d| d.declined.clone()) {
            return Err(SessionError::Declined(Declined {
                code: "declined",
                message: reason,
            }));
        }
        let c = &self.config;
        let b = st
            .buckets
            .entry(st.members[i].p.pid.clone())
            .or_insert_with(|| Bucket::full(c.edit_burst, now));
        b.take(now, c.edit_rate_per_s, c.edit_burst)
            .map_err(|retry_after_ms| SessionError::RateLimited { retry_after_ms })?;
        Ok(i)
    }

    // ----- presence (as text sessions; see presence.rs) -----

    /// Open a new connection for `p`. Needs read. `Joined::client` is 0: a blob doc has no
    /// CRDT clients. `Joined::version` is the current version.
    pub fn join(&self, p: Participant, stay: Stay) -> Result<Joined> {
        self.join_impl(p, stay, false)
    }

    /// Like [`BlobSession::join`], reusing the pid's shared connection with the same `via`
    /// (one-shot verbs).
    pub fn join_shared(&self, p: Participant, stay: Stay) -> Result<Joined> {
        self.join_impl(p, stay, true)
    }

    fn join_impl(&self, p: Participant, stay: Stay, shared: bool) -> Result<Joined> {
        presence::check_participant(&p)?;
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let rights = self.host.rights(&p, &st.key);
        if !rights.read {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid: p.pid.clone(),
                action: Action::Join,
                need: Need::Read,
            });
            self.remove_pid(st, &p.pid, LeaveReason::RightsLost);
            return Err(SessionError::Forbidden { need: Need::Read });
        }
        for m in st.members.iter_mut().filter(|m| m.p.pid == p.pid) {
            m.rights = rights;
        }
        if shared {
            if let Some(m) = st.members.iter_mut().find(|m| m.shares_with(&p)) {
                m.p = p;
                m.last_active_ms = now;
                if stay == Stay::Held {
                    m.stay = Stay::Held;
                }
                return Ok(Joined {
                    conn: m.conn,
                    client: 0,
                    version: st.vid,
                });
            }
        }
        st.joins += 1;
        let pres = Presence::new(p, shared, rights, stay, st.joins, now);
        let conn = pres.conn;
        self.emit(AuditEvent::Joined {
            key: st.key.clone(),
            pid: pres.p.pid.clone(),
            conn,
            client: 0,
        });
        if st.disk.is_some() {
            let _ = self.disk_check(st, now, false);
        }
        st.members.push(BMember { pres, region: None });
        Ok(Joined {
            conn,
            client: 0,
            version: st.vid,
        })
    }

    /// Close one connection. The pid's lease (if any) stays until it expires or is
    /// released: a command-line agent leases in one call and writes in the next.
    pub fn leave(&self, conn: ConnId) -> Result<bool> {
        self.now();
        let mut st = self.lock()?;
        match Self::member(&st, conn) {
            Ok(i) => {
                self.remove_member(&mut st, i, LeaveReason::Left);
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Remove every connection of `pid` (the host's say-so); their lease ends.
    pub fn kick(&self, pid: &str) -> Result<usize> {
        let mut st = self.lock()?;
        Ok(self.remove_pid(&mut st, pid, LeaveReason::Kicked))
    }

    /// Keep a connection present. Needs read.
    pub fn touch(&self, conn: ConnId) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Touch, now)?;
        Ok(())
    }

    pub fn set_stay(&self, conn: ConnId, stay: Stay) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Touch, now)?;
        st.members[i].stay = stay;
        Ok(())
    }

    pub fn set_visible(&self, conn: ConnId, visible: bool) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::View, now)?;
        st.members[i].set_visible(visible, now);
        Ok(())
    }

    /// Set (or clear) what this connection is looking at: an opaque string the host's
    /// viewer chooses (`"page 3"`, `"slide 7"`), at most [`REGION_MAX_BYTES`]. Needs read.
    pub fn set_region(&self, conn: ConnId, region: Option<String>) -> Result<()> {
        if region.as_ref().is_some_and(|r| r.len() > REGION_MAX_BYTES) {
            return Err(SessionError::BadPosition(format!(
                "a view region is at most {REGION_MAX_BYTES} bytes"
            )));
        }
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::View, now)?;
        st.members[i].region = region;
        Ok(())
    }

    /// `out` as `conn` may see it: an App viewer's (K5) goes through its handles (`f` for the
    /// value; a refusal naming someone else, as `Leased`'s holder, by handle). Anyone
    /// else's is unchanged.
    fn for_viewer<T>(
        &self,
        conn: ConnId,
        out: Result<T>,
        f: impl FnOnce(&mut super::handles::Lens<'_>, T) -> T,
    ) -> Result<T> {
        let Ok(mut st) = self.lock() else {
            return out;
        };
        let Some(v) = st
            .members
            .iter()
            .find(|m| m.conn == conn && m.p.kind == ParticipantKind::App)
            .map(|m| m.p.pid.clone())
        else {
            return out;
        };
        let mut lens = st.handles.lens(&v);
        match out {
            Ok(t) => Ok(f(&mut lens, t)),
            Err(e) => Err(lens.error(e)),
        }
    }

    /// Who is here, as for text docs: one entry per pid, editors (the lease holder among
    /// them) first, then viewers, then away. `region` and `holds_lease` are filled; there
    /// are no carets. Alone, it lists only the caller. Needs read.
    pub fn roster(&self, conn: ConnId) -> Result<Vec<RosterEntry>> {
        let out = self.roster_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|e| l.entry(e)).collect()
        })
    }

    fn roster_inner(&self, conn: ConnId) -> Result<Vec<RosterEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Roster, now)?;
        self.expire_lease(&mut st, now);
        let holder = st.lease.as_ref().map(|l| l.pid.clone());
        let is_holder = |pid: &str| holder.as_deref() == Some(pid);
        let mut rows = presence::fold_roster(&st.members, now, &self.config, is_holder);
        for (conns, entry, _) in &mut rows {
            entry.region = conns.iter().find_map(|m| m.region.clone());
            entry.holds_lease = is_holder(&entry.participant.pid);
        }
        Ok(presence::sort_roster(rows))
    }

    /// Close idle connections without a hold. Returns those that left.
    pub fn expire_idle(&self) -> Result<Vec<(ConnId, String)>> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let mut gone = Vec::new();
        let mut i = 0;
        while i < st.members.len() {
            if st.members[i].is_idle(now, &self.config) {
                gone.push((st.members[i].conn, st.members[i].p.pid.clone()));
                self.remove_member(&mut st, i, LeaveReason::Idle);
            } else {
                i += 1;
            }
        }
        Ok(gone)
    }

    /// Re-resolve every participant's rights, once per pid; those without read are removed
    /// (and their lease ends). Returns the pids whose rights changed.
    pub fn recheck_rights(&self) -> Result<Vec<super::RightsChange>> {
        let mut st = self.lock()?;
        let st = &mut *st;
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        let mut i = 0;
        while i < st.members.len() {
            let pid = st.members[i].p.pid.clone();
            if seen.contains(&pid) {
                i += 1;
                continue;
            }
            seen.push(pid.clone());
            let r = self.host.rights(&st.members[i].p, &st.key);
            if r != st.members[i].rights {
                out.push(super::RightsChange {
                    pid: pid.clone(),
                    rights: r,
                });
            }
            for m in st.members.iter_mut().filter(|m| m.p.pid == pid) {
                m.rights = r;
            }
            if !r.read {
                self.remove_pid(st, &pid, LeaveReason::RightsLost);
            } else {
                i += 1;
            }
        }
        Ok(out)
    }

    // ----- the lease -----

    fn end_lease(&self, st: &mut BState, how: LeaseEnd) {
        if let Some(l) = st.lease.take() {
            self.emit(AuditEvent::LeaseEnded {
                key: st.key.clone(),
                pid: l.pid,
                how,
            });
        }
    }

    /// Drop the lease if it ran out on the host's clock. Returns its holder if it did.
    fn expire_lease(&self, st: &mut BState, now: u64) -> Option<String> {
        if st.lease.as_ref().is_some_and(|l| now >= l.expires_ms) {
            let pid = st.lease.as_ref().map(|l| l.pid.clone());
            self.end_lease(st, LeaseEnd::Expired);
            return pid;
        }
        None
    }

    /// `Leased` if someone other than `pid` holds the lease now.
    fn check_lease(&self, st: &mut BState, pid: &str, now: u64) -> Result<()> {
        self.expire_lease(st, now);
        match &st.lease {
            Some(l) if l.pid != pid => Err(SessionError::Leased {
                holder: l.pid.clone(),
                name: l.name.clone(),
                expires_ms: l.expires_ms,
            }),
            _ => Ok(()),
        }
    }

    fn ttl(&self, ttl_ms: Option<u64>) -> u64 {
        ttl_ms
            .unwrap_or(self.config.lease_default_ms)
            .clamp(1, self.config.lease_max_ms.max(1))
    }

    fn grant(&self, st: &mut BState, i: usize, ttl: u64, now: u64, auto: bool) -> LeaseInfo {
        let m = &mut st.members[i];
        if !auto {
            m.last_cursor_ms = Some(now);
        }
        let (pid, name) = (m.p.pid.clone(), m.p.name.clone());
        let acquired_ms = match &st.lease {
            Some(l) if l.pid == pid && !l.auto => l.acquired_ms,
            _ => now,
        };
        let l = LeaseInfo {
            pid,
            name,
            acquired_ms,
            expires_ms: now.saturating_add(ttl),
            auto,
        };
        st.lease = Some(l.clone());
        self.emit(AuditEvent::LeaseAcquired {
            key: st.key.clone(),
            pid: l.pid.clone(),
            expires_ms: l.expires_ms,
            auto,
        });
        l
    }

    /// Take the doc's lease for `ttl_ms` ([`Config::lease_default_ms`] when `None`, at most
    /// [`Config::lease_max_ms`]), or extend it if the caller's pid already holds it. Needs
    /// edit. Someone else holding it gives [`SessionError::Leased`] (exit 5) with the holder
    /// and expiry. Leases never block reads.
    pub fn acquire_lease(&self, conn: ConnId, ttl_ms: Option<u64>) -> Result<LeaseInfo> {
        let out = self.acquire_lease_inner(conn, ttl_ms);
        self.for_viewer(conn, out, |l, v| l.lease(v))
    }

    fn acquire_lease_inner(&self, conn: ConnId, ttl_ms: Option<u64>) -> Result<LeaseInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Edit, Action::Lease, now)?;
        let pid = st.members[i].p.pid.clone();
        self.check_lease(&mut st, &pid, now)?;
        let ttl = self.ttl(ttl_ms);
        Ok(self.grant(&mut st, i, ttl, now, false))
    }

    /// Extend the caller's lease. [`SessionError::NoLease`] when nobody holds it,
    /// [`SessionError::Leased`] when someone else does.
    pub fn renew_lease(&self, conn: ConnId, ttl_ms: Option<u64>) -> Result<LeaseInfo> {
        let out = self.renew_lease_inner(conn, ttl_ms);
        self.for_viewer(conn, out, |l, v| l.lease(v))
    }

    fn renew_lease_inner(&self, conn: ConnId, ttl_ms: Option<u64>) -> Result<LeaseInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Edit, Action::Lease, now)?;
        let pid = st.members[i].p.pid.clone();
        self.check_lease(&mut st, &pid, now)?;
        if st.lease.is_none() {
            return Err(SessionError::NoLease);
        }
        let ttl = self.ttl(ttl_ms);
        Ok(self.grant(&mut st, i, ttl, now, false))
    }

    /// Release the caller's lease. Needs read (someone who lost edit can still let go).
    /// Returns false when the caller's pid didn't hold it.
    pub fn release_lease(&self, conn: ConnId) -> Result<bool> {
        let out = self.release_lease_inner(conn);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn release_lease_inner(&self, conn: ConnId) -> Result<bool> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Lease, now)?;
        self.expire_lease(&mut st, now);
        if st
            .lease
            .as_ref()
            .is_some_and(|l| l.pid == st.members[i].p.pid)
        {
            self.end_lease(&mut st, LeaseEnd::Released);
            return Ok(true);
        }
        Ok(false)
    }

    /// Break whoever's lease (an owner's override; the holder's next write names a base
    /// and is checked as usual). Needs [`super::Rights::owner`]; audited as
    /// [`AuditEvent::LeaseBroken`]. Returns the lease that was broken, if one was held.
    pub fn break_lease(&self, conn: ConnId) -> Result<Option<LeaseInfo>> {
        let out = self.break_lease_inner(conn);
        self.for_viewer(conn, out, |l, v| v.map(|x| l.lease(x)))
    }

    fn break_lease_inner(&self, conn: ConnId) -> Result<Option<LeaseInfo>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Owner, Action::BreakLease, now)?;
        self.expire_lease(&mut st, now);
        let Some(l) = st.lease.clone() else {
            return Ok(None);
        };
        self.emit(AuditEvent::LeaseBroken {
            key: st.key.clone(),
            by: st.members[i].p.pid.clone(),
            holder: l.pid.clone(),
            expires_ms: l.expires_ms,
        });
        self.end_lease(&mut st, LeaseEnd::Broken);
        Ok(Some(l))
    }

    /// The lease, if one is held now. Metadata; no connection needed.
    pub fn lease(&self) -> Result<Option<LeaseInfo>> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.expire_lease(&mut st, now);
        Ok(st.lease.clone())
    }

    // ----- reading -----

    /// The current content, streaming and verified. Needs read; never blocked by a lease.
    pub fn read(&self, conn: ConnId) -> Result<BlobRead> {
        let out = self.read_inner(conn);
        self.for_viewer(conn, out, |l, mut r| {
            r.info = l.blob_version(r.info);
            r
        })
    }

    fn read_inner(&self, conn: ConnId) -> Result<BlobRead> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
        let reader = self.blobs.reader(&st.key.space, &st.cur.blob)?;
        Ok(BlobRead {
            version: st.vid,
            info: st.cur.clone(),
            reader,
        })
    }

    /// Every version, oldest first (the pointer's history). Needs history.
    pub fn versions(&self, conn: ConnId) -> Result<Vec<(VersionId, BlobVersion)>> {
        let out = self.versions_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter()
                .map(|(id, b)| (id, l.blob_version(b)))
                .collect()
        })
    }

    fn versions_inner(&self, conn: ConnId) -> Result<Vec<(VersionId, BlobVersion)>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::History, Action::Versions, now)?;
        Ok(Self::store(&st)?.blob_versions(self.doc_id)?)
    }

    /// The history as a log (`log` for any file): one entry per version, **oldest first**:
    /// who and when, what it was (a write, a restore, an outside save, a conflicting save
    /// kept with a copy, a merge...), for whom, its name, whether it was purged, and its
    /// conflict copy. Built from the stored versions. Needs history, as
    /// [`BlobSession::versions`].
    pub fn log(&self, conn: ConnId) -> Result<Vec<BlobLogEntry>> {
        let out = self.log_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|e| l.blob_log_entry(e)).collect()
        })
    }

    fn log_inner(&self, conn: ConnId) -> Result<Vec<BlobLogEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::History, Action::History, now)?;
        let base = Self::copy_base(&st);
        let store = Self::store(&st)?;
        blob_log(&store, self.doc_id, base.as_deref())
    }

    /// **Blame** for any file (tier 0, P22): the current version's setter and a summary of
    /// the version history (how many versions, who set them, most recent first). Needs read
    /// and history (K-PERM-9).
    pub fn blame(&self, conn: ConnId) -> Result<BlobBlame> {
        let out = self.blame_inner(conn);
        self.for_viewer(conn, out, |l, b| l.blob_blame(b))
    }

    fn blame_inner(&self, conn: ConnId) -> Result<BlobBlame> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Blame, now)?;
        self.admit(&mut st, conn, Need::History, Action::Blame, now)?;
        let store = Self::store(&st)?;
        let all = store.blob_versions(self.doc_id)?;
        let approval = store.blob_approval(self.doc_id, st.vid)?.map(|a| a.state);
        let mut authors: Vec<BlobAuthor> = Vec::new();
        for (_, v) in all.iter().rev() {
            match authors.iter_mut().find(|a| a.pid == v.author) {
                Some(a) => a.versions += 1,
                None => authors.push(BlobAuthor {
                    pid: v.author.clone(),
                    versions: 1,
                    last_ms: v.at_ms,
                }),
            }
        }
        Ok(BlobBlame {
            version: st.vid,
            current: st.cur.clone(),
            versions: all.len(),
            authors,
            approval,
        })
    }

    /// One version's content, streaming and verified. Needs history.
    pub fn read_version(&self, conn: ConnId, vid: VersionId) -> Result<BlobRead> {
        let out = self.read_version_inner(conn, vid);
        self.for_viewer(conn, out, |l, mut r| {
            r.info = l.blob_version(r.info);
            r
        })
    }

    fn read_version_inner(&self, conn: ConnId, vid: VersionId) -> Result<BlobRead> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::History, Action::Versions, now)?;
        let info = Self::store(&st)?.blob_version(self.doc_id, vid)?;
        if info.redacted {
            return Err(StoreError::PurgedVersion(self.doc_id, vid).into());
        }
        let reader = self.blobs.reader(&st.key.space, &info.blob)?;
        Ok(BlobRead {
            version: vid,
            info,
            reader,
        })
    }

    /// **The file at version `vid`, copied to `out`** as it streams from the blob store, in
    /// bounded memory (exports and derivatives of big files). Needs history, as
    /// [`BlobSession::read_version`]. The bytes are verified as they stream and the check
    /// completes at the clean end: only `Ok` means the copy is good. On an error (a damaged
    /// blob: [`BlobError::Damaged`], exit 1; a failed write: [`BlobError::Io`]) the caller
    /// **must discard what was written** ([`super::FileCopy`]). The session lock is not held
    /// while copying.
    pub fn file_at_to(
        &self,
        conn: ConnId,
        vid: VersionId,
        out: &mut dyn io::Write,
    ) -> Result<super::FileCopy> {
        copy_verified(self.read_version(conn, vid)?, out)
    }

    /// The current file, copied to `out` as [`BlobSession::file_at_to`] copies a version.
    /// Needs read.
    pub fn file_now_to(&self, conn: ConnId, out: &mut dyn io::Write) -> Result<super::FileCopy> {
        copy_verified(self.read(conn)?, out)
    }

    /// Name (or rename) a version (`version --name`). Named versions are never removed by
    /// retention. Needs edit.
    pub fn name_version(&self, conn: ConnId, vid: VersionId, name: &str) -> Result<()> {
        let out = self.name_version_inner(conn, vid, name);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn name_version_inner(&self, conn: ConnId, vid: VersionId, name: &str) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Edit, Action::Versions, now)?;
        Ok(Self::store(&st)?.name_blob_version(self.doc_id, vid, name)?)
    }

    /// Metadata only; no connection needed.
    pub fn status(&self) -> Result<BlobStatus> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.expire_lease(&mut st, now);
        let conflict_copies = self.existing_copies(&mut st)?;
        Ok(BlobStatus {
            key: st.key.clone(),
            version: st.vid,
            current: st.cur.clone(),
            participants: presence::participants(&st.members),
            connections: st.members.len(),
            lease: st.lease.clone(),
            disk: Self::disk_state(&st),
            conflict_copies,
        })
    }

    pub fn key(&self) -> Result<DocKey> {
        Ok(self.lock()?.key.clone())
    }

    // ----- writing -----

    /// `write --base V`: replace the content with what `src` yields, streaming. Needs edit,
    /// and:
    /// - **the lease**: someone else holding it gives [`SessionError::Leased`] (exit 5),
    ///   nothing written. With no lease held, the write takes one for its own length
    ///   ([`Config::lease_auto_ms`] at most), so a second writer is told at once instead of
    ///   after uploading;
    /// - **a current base**: `base` must be the current version, else
    ///   [`SessionError::Stale`] `{ expected: base, actual }` (re-read and retry). Never
    ///   last-writer-wins. Checked before the upload and again at commit.
    ///
    /// The upload runs without the session lock. The new version is durable before this
    /// returns, then written to the file (atomic; echo-suppressed). Equal bytes record
    /// nothing (`changed: false`).
    pub fn write(
        &self,
        conn: ConnId,
        src: &mut dyn Read,
        base: VersionId,
    ) -> Result<BlobWriteResult> {
        let out = self.write_inner(conn, src, base);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn write_inner(
        &self,
        conn: ConnId,
        src: &mut dyn Read,
        base: VersionId,
    ) -> Result<BlobWriteResult> {
        let now = self.now();
        let (space, pid, auto) = {
            let mut st = self.lock()?;
            let i = self.admit_write(&mut st, conn, Action::WriteBase, now)?;
            let pid = st.members[i].p.pid.clone();
            self.check_lease(&mut st, &pid, now)?;
            if base != st.vid {
                return Err(SessionError::Stale {
                    expected: base,
                    actual: st.vid,
                });
            }
            let auto = st.lease.is_none();
            if auto {
                self.grant(&mut st, i, self.config.lease_auto_ms.max(1), now, true);
            }
            (st.key.space.clone(), pid, auto)
        };
        let put = self.blobs.put(&space, src);
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let out = self.commit_write(st, conn, put, base, now);
        if auto && st.lease.as_ref().is_some_and(|l| l.auto && l.pid == pid) {
            self.end_lease(st, LeaseEnd::Released);
        }
        out
    }

    /// [`BlobSession::write`] from memory.
    pub fn write_bytes(
        &self,
        conn: ConnId,
        bytes: &[u8],
        base: VersionId,
    ) -> Result<BlobWriteResult> {
        let out = self.write_bytes_inner(conn, bytes, base);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn write_bytes_inner(
        &self,
        conn: ConnId,
        bytes: &[u8],
        base: VersionId,
    ) -> Result<BlobWriteResult> {
        self.write(conn, &mut &bytes[..], base)
    }

    fn commit_write(
        &self,
        st: &mut BState,
        conn: ConnId,
        put: std::result::Result<BlobInfo, BlobError>,
        base: VersionId,
        now: u64,
    ) -> Result<BlobWriteResult> {
        let info = put?;
        let i = self.admit(st, conn, Need::Edit, Action::WriteBase, now)?;
        let pid = st.members[i].p.pid.clone();
        self.check_lease(st, &pid, now)?;
        if base != st.vid {
            return Err(SessionError::Stale {
                expected: base,
                actual: st.vid,
            });
        }
        if info.id == st.cur.blob {
            return Ok(BlobWriteResult {
                version: st.vid,
                blob: info.id,
                size: info.size,
                changed: false,
                written: false,
            });
        }
        let p = &st.members[i].p;
        let mut v = BlobVersion::new(
            info.id,
            info.size,
            now,
            p.pid.clone(),
            p.name.clone(),
            p.via.clone(),
            BlobReason::Write,
        );
        v.on_behalf_of = p.on_behalf_of.clone();
        self.commit_own(st, i, v, now)
    }

    /// Record a participant's version, then write it back.
    fn commit_own(
        &self,
        st: &mut BState,
        i: usize,
        v: BlobVersion,
        now: u64,
    ) -> Result<BlobWriteResult> {
        let store = Self::store(st)?;
        let vid = store.commit_blob_versions(
            self.doc_id,
            &st.key.space,
            st.vid,
            std::slice::from_ref(&v),
            None,
        )?[0];
        st.vid = vid;
        st.cur = v;
        st.members[i].last_edit_ms = Some(now);
        Self::mark_dirty(st, now);
        // a failed write-back is reported as an event; tick retries
        let written = self.write_back(st, now).unwrap_or_default();
        Ok(BlobWriteResult {
            version: vid,
            blob: st.cur.blob,
            size: st.cur.size,
            changed: true,
            written,
        })
    }

    /// `restore --version V`: make version `vid`'s content current again, as a **new**
    /// version (a forward change; history is never rewritten), attributed to the caller with
    /// [`BlobReason::Restore`]. Needs edit and history; the lease rule applies as for a
    /// write (deliberate, so no base).
    pub fn restore(&self, conn: ConnId, vid: VersionId) -> Result<BlobWriteResult> {
        let out = self.restore_inner(conn, vid);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn restore_inner(&self, conn: ConnId, vid: VersionId) -> Result<BlobWriteResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::History, Action::Restore, now)?;
        let i = self.admit_write(st, conn, Action::Restore, now)?;
        let pid = st.members[i].p.pid.clone();
        self.check_lease(st, &pid, now)?;
        let old = Self::store(st)?.blob_version(self.doc_id, vid)?;
        if old.redacted {
            return Err(StoreError::PurgedVersion(self.doc_id, vid).into());
        }
        if old.blob == st.cur.blob {
            return Ok(BlobWriteResult {
                version: st.vid,
                blob: old.blob,
                size: old.size,
                changed: false,
                written: false,
            });
        }
        if self.blobs.info(&st.key.space, &old.blob)?.is_none() {
            return Err(BlobError::NotFound(old.blob.to_hex()).into());
        }
        let p = &st.members[i].p;
        let mut v = BlobVersion::new(
            old.blob,
            old.size,
            now,
            p.pid.clone(),
            p.name.clone(),
            p.via.clone(),
            BlobReason::Restore,
        );
        v.on_behalf_of = p.on_behalf_of.clone();
        v.restored_from = Some(vid);
        self.commit_own(st, i, v, now)
    }

    // ----- approvals of on-behalf writes (K-ID-10, K-ID-11, K-T0-9) -----

    /// Settle the approval of version `vid`, written (or restored) by an assistant agent on
    /// behalf of a person: `Approved` or `Declined`, once. The rules are the text doc's
    /// ([`super::Session::set_approval`]): only that person may do it; any other connection
    /// (the agent itself, another person, an owner) gets [`SessionError::Forbidden`] with
    /// [`Need::Approver`] and an [`AuditEvent::Refused`]. Needs read; the lease doesn't
    /// apply (nothing is written but the record). The version itself is never held or
    /// undone by this.
    ///
    /// A **declined** version stays (K-HIST-19 for tier 0): [`BlobApproval::restore_to`] is
    /// the version before it, which the person restores explicitly if they want it back,
    /// and [`BlobApproval::later`] lists the versions written since (a restore replaces
    /// their content too: blobs don't merge). The kit never restores on its own.
    ///
    /// Errors: [`SessionError::NoApproval`] (`seq` is the version) when `vid` wasn't written
    /// on anyone's behalf or doesn't exist; [`SessionError::BadApproval`] for `Pending`, or
    /// for a version already settled the other way. Settling it again the same way returns
    /// the record unchanged. Emits [`AuditEvent::Approval`] (`seq` is the version).
    pub fn set_approval(
        &self,
        conn: ConnId,
        vid: VersionId,
        state: ApprovalState,
    ) -> Result<BlobApproval> {
        let out = self.set_approval_inner(conn, vid, state);
        self.for_viewer(conn, out, |l, a| l.blob_approval(a))
    }

    fn set_approval_inner(
        &self,
        conn: ConnId,
        vid: VersionId,
        state: ApprovalState,
    ) -> Result<BlobApproval> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Approve, now)?;
        let pid = st.members[i].p.pid.clone();
        let store = Self::store(&st)?;
        let Some(cur) = store.blob_approval(self.doc_id, vid)? else {
            return Err(SessionError::NoApproval { seq: vid });
        };
        if cur.on_behalf_of != pid {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid,
                action: Action::Approve,
                need: Need::Approver,
            });
            return Err(SessionError::Forbidden {
                need: Need::Approver,
            });
        }
        if state == ApprovalState::Pending {
            return Err(SessionError::BadApproval(
                "an approval can only be settled as approved or declined".into(),
            ));
        }
        match cur.state {
            ApprovalState::Pending => {}
            s if s == state => return self.blob_approval_out(&store, vid, cur),
            s => {
                return Err(SessionError::BadApproval(format!(
                    "version {vid} was already settled as {s:?}"
                )))
            }
        }
        let new = cur.settled(state, pid.clone(), now);
        store.set_blob_approval(self.doc_id, vid, &new)?;
        self.emit(AuditEvent::Approval {
            key: st.key.clone(),
            seq: vid,
            pid,
            state,
        });
        self.blob_approval_out(&store, vid, new)
    }

    /// [`BlobApproval`] for `a`: a declined version's way back.
    fn blob_approval_out(
        &self,
        store: &DocStore,
        vid: VersionId,
        a: Approval,
    ) -> Result<BlobApproval> {
        let (mut restore_to, mut later) = (None, Vec::new());
        if a.state == ApprovalState::Declined {
            let all = store.blob_versions(self.doc_id)?;
            restore_to = all
                .iter()
                .rfind(|(id, _)| *id < vid)
                .filter(|(_, v)| !v.redacted)
                .map(|(id, _)| *id);
            later = all
                .iter()
                .filter(|(id, _)| *id > vid)
                .map(|(id, _)| *id)
                .collect();
        }
        Ok(BlobApproval {
            version: vid,
            approval: a,
            restore_to,
            later,
        })
    }

    /// The approval record of version `vid` (`None` when it wasn't written on anyone's
    /// behalf). Needs read; and [`super::Rights::history`] unless the caller is the agent
    /// that wrote it or the person it was written for (the text doc's rule,
    /// [`super::Session::approval`]).
    pub fn approval(&self, conn: ConnId, vid: VersionId) -> Result<Option<Approval>> {
        let out = self.approval_inner(conn, vid);
        self.for_viewer(conn, out, |l, v| v.map(|a| l.approval(a)))
    }

    fn approval_inner(&self, conn: ConnId, vid: VersionId) -> Result<Option<Approval>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::ReadApproval, now)?;
        let pid = st.members[i].p.pid.clone();
        let a = Self::store(&st)?.blob_approval(self.doc_id, vid)?;
        if a.as_ref()
            .is_some_and(|a| a.author == pid || a.on_behalf_of == pid)
        {
            return Ok(a);
        }
        self.admit(&mut st, conn, Need::History, Action::ReadApproval, now)?;
        Ok(a)
    }

    /// Approval records of this doc, oldest version first: the tab's "waiting for my
    /// approval" list is the `Pending` ones whose `on_behalf_of` is me (`log --pending
    /// --on-behalf-of me` on the agent side). With [`super::Rights::history`], all of them;
    /// without, only those where the caller is the agent or the person. Needs read.
    pub fn approvals(&self, conn: ConnId) -> Result<Vec<(VersionId, Approval)>> {
        let out = self.approvals_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|(id, a)| (id, l.approval(a))).collect()
        })
    }

    fn approvals_inner(&self, conn: ConnId) -> Result<Vec<(VersionId, Approval)>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::ReadApproval, now)?;
        let (pid, history) = (st.members[i].p.pid.clone(), st.members[i].rights.history);
        let mut all = Self::store(&st)?.blob_approvals(self.doc_id)?;
        if !history {
            all.retain(|(_, a)| a.author == pid || a.on_behalf_of == pid);
        }
        Ok(all)
    }

    // ----- erasure (K-PURGE, K-T0-9) -----

    /// **Purge** blob version `version` (`Some`; the current one is refused with
    /// [`StoreError::CurrentVersion`]: write or restore another first), or (`None`) the
    /// doc's whole history: every version but the current. Owner only (refused with
    /// [`Need::Owner`]). Each purged version keeps its entry (who, when, why, size) with
    /// [`BlobVersion::redacted`] set and the zero id (its keyed id is a digest of the
    /// content: it goes too); reading or restoring it fails with
    /// [`StoreError::PurgedVersion`].
    ///
    /// Then, at once (no GC grace): each released blob nothing else references is removed,
    /// index and file; one still referenced (the same bytes in another version or doc, or a
    /// pack's retain) stays and is counted as [`NotReached::SharedBlobs`]. The doc's
    /// recorded **conflict copies** (beside its file) are scanned: one whose bytes are a
    /// purged blob (compared by keyed id, never a plain hash) is deleted; others are left and
    /// counted ([`NotReached::ConflictCopies`]); copies in the host's folder
    /// ([`Host::conflict_dir`]) are found by their record too. The file is written back first, so it holds
    /// the current content; the store file is rewritten after. Audited as
    /// [`AuditEvent::Purged`]. The approval record of a purged on-behalf version is reached
    /// and kept, as its entry is: it is metadata only (agent, person, state, settler, time;
    /// never content or a blob id), counted in [`PurgeCounts::approvals`].
    pub fn purge(&self, conn: ConnId, version: Option<VersionId>) -> Result<PurgeOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Owner, Action::Purge, now)?;
        let by = st.members[i].p.pid.clone();
        let store = Self::store(st)?;
        // a store holding a newer kit's data can't be rewritten: refuse before anything
        // changes (v0.9.5)
        store.check_rewritable()?;
        let all = store.blob_versions(self.doc_id)?;
        let vids: Vec<VersionId> = match version {
            Some(v) if v == st.vid => return Err(StoreError::CurrentVersion(self.doc_id, v).into()),
            Some(v) => {
                if !all.iter().any(|(id, _)| *id == v) {
                    return Err(StoreError::NoVersion(self.doc_id, v).into());
                }
                vec![v]
            }
            None => all
                .iter()
                .filter(|(id, v)| *id != st.vid && !v.redacted)
                .map(|(id, _)| *id)
                .collect(),
        };
        let mut not_reached = Vec::new();
        // the file holds the current content first (an outside save is recorded, not lost)
        if st.disk.is_some() {
            let ok = self.write_back(st, now).is_ok()
                && matches!(Self::disk_state(st), Some(DiskState::Clean));
            if !ok {
                not_reached.push(NotReached::File);
            }
        }
        let space = st.key.space.clone();
        let (released, approvals) = store.blob_purge_versions(self.doc_id, &vids, now)?;
        let purged: Vec<BlobId> = released.iter().map(|r| r.1).collect();
        let free: Vec<(u64, BlobId, u64)> = released
            .iter()
            .filter(|r| r.3 == 0)
            .map(|r| (r.0, r.1, r.2))
            .collect();
        let shared = released.len() as u64 - free.len() as u64;
        if shared > 0 {
            not_reached.push(NotReached::SharedBlobs(shared));
        }
        let (blobs_removed, _) = self.blobs.remove_unreferenced_now(&free)?;
        // conflict copies the kit wrote for this doc (recorded on the versions; beside the
        // file, or in the host's folder)
        let (mut copies_removed, mut copies_left) = (0, 0);
        if let Some(base) = Self::copy_base(st) {
            for p in self.copy_paths(st, &base)? {
                let Ok(mut f) = File::open(&p) else { continue };
                let id = self.blobs.id_of(&space, &mut f).ok().map(|x| x.0);
                drop(f);
                match id {
                    Some(id) if purged.contains(&id) => {
                        if fs::remove_file(&p).is_ok() {
                            copies_removed += 1;
                        } else {
                            copies_left += 1;
                        }
                    }
                    Some(id) if id == st.cur.blob => {}
                    _ => copies_left += 1,
                }
            }
        } else if all.iter().any(|(_, v)| v.conflict_copy.is_some()) {
            not_reached.push(NotReached::File);
        }
        if copies_left > 0 {
            not_reached.push(NotReached::ConflictCopies(copies_left));
        }
        if let Some(d) = st.disk.as_mut() {
            d.own.retain(|b| !purged.contains(b));
        }
        let counts = PurgeCounts {
            blob_versions: vids.len() as u64,
            blobs_removed,
            conflict_copies: copies_removed,
            approvals,
            ..PurgeCounts::default()
        };
        let rewrite = match store.rewrite_file() {
            Ok(r) => Some(r),
            Err(_) => {
                not_reached.push(NotReached::StoreFile);
                None
            }
        };
        not_reached.extend(NotReached::ALWAYS);
        self.emit(AuditEvent::Purged {
            key: st.key.clone(),
            by,
            scope: PurgedScope::BlobVersions { version },
            counts,
        });
        Ok(PurgeOutcome::of_blobs(counts, not_reached, rewrite))
    }

    /// The doc's recorded conflict copies that exist now (beside its file, or in the host's
    /// folder; for a byte scan, K-T0-9). Needs history.
    pub fn conflict_copies(&self, conn: ConnId) -> Result<Vec<PathBuf>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::History, Action::Versions, now)?;
        Ok(self
            .existing_copies(&mut st)?
            .into_iter()
            .map(|c| c.path)
            .collect())
    }

    // ----- conflict copies -----

    /// The folder recorded copies are relative to: the file's real folder.
    fn copy_base(st: &BState) -> Option<PathBuf> {
        let d = st.disk.as_ref()?;
        match disk::probe(&d.path, &d.root) {
            Ok(Probe::File { real, .. }) => real.parent().map(Path::to_owned),
            _ => d.path.parent().map(Path::to_owned),
        }
    }

    /// The recorded copies (from the versions, once per session; then kept up to date).
    fn copy_records(&self, st: &mut BState) -> Result<Vec<CopyRec>> {
        if st.copies.is_none() {
            let recs = Self::store(st)?
                .blob_versions(self.doc_id)?
                .into_iter()
                .filter_map(|(vid, v)| {
                    v.conflict_copy.map(|rec| CopyRec {
                        version: vid,
                        kept: vid.saturating_sub(1),
                        at_ms: v.at_ms,
                        rec,
                    })
                })
                .collect();
            st.copies = Some(recs);
        }
        Ok(st.copies.clone().unwrap_or_default())
    }

    /// Every recorded copy's path (existing or not).
    fn copy_paths(&self, st: &mut BState, base: &Path) -> Result<Vec<PathBuf>> {
        Ok(self
            .copy_records(st)?
            .iter()
            .map(|r| resolve_copy(base, &r.rec))
            .collect())
    }

    /// The recorded copies that exist now as regular files, oldest first.
    fn existing_copies(&self, st: &mut BState) -> Result<Vec<ConflictCopy>> {
        let Some(base) = Self::copy_base(st) else {
            return Ok(Vec::new());
        };
        Ok(self
            .copy_records(st)?
            .into_iter()
            .map(|r| ConflictCopy {
                path: resolve_copy(&base, &r.rec),
                version: r.version,
                kept: r.kept,
                at_ms: r.at_ms,
            })
            .filter(|c| fs::symlink_metadata(&c.path).is_ok_and(|m| m.is_file()))
            .collect())
    }

    /// K-A11Y-5's cleanup (on `tick`, at most every [`Config::history_tick_ms`]): delete a
    /// recorded copy older than [`Config::conflict_copy_keep_ms`] when it is still a regular
    /// file holding exactly its version's bytes (compared by keyed id). Kept: a copy whose
    /// version (the content, or the save that made it) is named; a copy a person edited;
    /// one whose version was purged (the purge removed it if it still matched). Returns the
    /// paths removed.
    fn cleanup_copies(&self, st: &mut BState, now: u64) -> Result<Vec<PathBuf>> {
        if st
            .last_cleanup_ms
            .is_some_and(|t| now.saturating_sub(t) < self.config.history_tick_ms)
        {
            return Ok(Vec::new());
        }
        st.last_cleanup_ms = Some(now);
        let Some(base) = Self::copy_base(st) else {
            return Ok(Vec::new());
        };
        let store = Self::store(st)?;
        let space = st.key.space.clone();
        let mut removed = Vec::new();
        for r in self.copy_records(st)? {
            if now.saturating_sub(r.at_ms) < self.config.conflict_copy_keep_ms {
                continue;
            }
            let path = resolve_copy(&base, &r.rec);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let stat = FileStat::of(&meta);
            if st
                .copies_edited
                .get(&path)
                .is_some_and(|s| s.vouches_for(stat))
            {
                continue;
            }
            let (Ok(kept), Ok(saved)) = (
                store.blob_version(self.doc_id, r.kept),
                store.blob_version(self.doc_id, r.version),
            ) else {
                continue;
            };
            if kept.name.is_some() || saved.name.is_some() || kept.redacted {
                continue;
            }
            if meta.len() != kept.size {
                st.copies_edited.insert(path, disk::Seen::now(stat));
                continue;
            }
            let hashed = File::open(&path).and_then(|mut f| {
                let id = self.blobs.id_of(&space, &mut f).map_err(blob_io)?;
                Ok((id.0, FileStat::of(&f.metadata()?)))
            });
            match hashed {
                // unchanged while it was hashed: delete it
                Ok((id, after))
                    if id == kept.blob && after == stat && fs::remove_file(&path).is_ok() =>
                {
                    st.copies_edited.remove(&path);
                    removed.push(path);
                }
                Ok((id, _)) if id != kept.blob => {
                    st.copies_edited.insert(path, disk::Seen::now(stat));
                }
                _ => {}
            }
        }
        Ok(removed)
    }

    // ----- for the manager -----

    pub(crate) fn last_used_ms(&self) -> u64 {
        self.last_used_ms.load(Ordering::Relaxed)
    }

    /// Connections plus a live lease (a held lease keeps the session loaded), or `None`
    /// once closed.
    pub(crate) fn pool_info(&self) -> Option<usize> {
        let now = self.host.now_ms();
        let st = self.lock().ok()?;
        let leased = st.lease.as_ref().is_some_and(|l| now < l.expires_ms);
        Some(st.members.len() + usize::from(leased))
    }

    pub(crate) fn rekey(&self, new: DocKey) -> Result<()> {
        let mut st = self.lock()?;
        if new.space != st.key.space {
            return Err(SessionError::BadPath(
                "a blob doc can't move to another space (its blobs are keyed by space)".into(),
            ));
        }
        Self::store(&st)?.rekey(self.doc_id, new.clone())?;
        st.key = new;
        Ok(())
    }

    /// Close this handle because its doc is being deleted: no write-back. Emits
    /// [`AuditEvent::DocDeleted`] with the connections that were in it (any lease ends with
    /// it); later calls fail with [`SessionError::Deleted`].
    pub(crate) fn close_deleted(&self) {
        let Ok(mut st) = self.lock_any() else {
            return;
        };
        if st.closed.is_some() {
            return;
        }
        let connections = st
            .members
            .iter()
            .map(|m| (m.conn, m.p.pid.clone()))
            .collect();
        self.emit(AuditEvent::DocDeleted {
            key: st.key.clone(),
            connections,
        });
        st.closed = Some(SessionError::Deleted);
        st.store = None;
        st.members.clear();
        st.lease = None;
    }

    /// The host closes this session ([`super::SessionManager::close`]), members or not: the
    /// audit names the connections, then it closes (a pending write-back written, any lease
    /// ended); later calls fail with [`SessionError::Closed`].
    pub(crate) fn close_by_host(&self, reason: &'static str) {
        {
            let Ok(st) = self.lock_any() else {
                return;
            };
            if st.closed.is_some() {
                return;
            }
            let connections = st
                .members
                .iter()
                .map(|m| (m.conn, m.p.pid.clone()))
                .collect();
            self.emit(AuditEvent::SessionClosed {
                key: st.key.clone(),
                connections,
                reason,
            });
        }
        self.close(SessionError::Closed { reason });
    }

    pub(crate) fn close(&self, reason: SessionError) {
        let Ok(mut st) = self.lock_any() else {
            return;
        };
        if st.closed.is_some() {
            return;
        }
        if st.disk.as_ref().is_some_and(|d| d.dirty_since.is_some()) {
            let now = self.host.now_ms();
            let _ = self.write_back(&mut st, now);
        }
        st.closed = Some(reason);
        st.store = None;
        st.members.clear();
        st.lease = None;
    }

    /// The host's clock tick: lease expiry, the stat backup while anyone is present, a
    /// write-back that failed before, and the conflict-copy cleanup.
    pub(crate) fn tick(&self) -> Result<BlobTick> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        let mut out = BlobTick {
            expired: self.expire_lease(st, now),
            ..BlobTick::default()
        };
        if st.disk.is_none() {
            return Ok(out);
        }
        let d = st.disk.as_ref().expect("checked above");
        if !st.members.is_empty() && now.saturating_sub(d.last_check_ms) >= self.config.disk_poll_ms
        {
            out.check = Some(self.disk_check(st, now, false)?);
        }
        let d = st.disk.as_ref().expect("checked above");
        let due = d.dirty_since.is_some() && !d.missing && d.declined.is_none();
        out.written = due && self.write_back(st, now)?;
        out.copies_removed = self.cleanup_copies(st, now)?;
        Ok(out)
    }
}

/// What [`BlobSession::tick`] did.
#[derive(Debug, Default)]
pub(crate) struct BlobTick {
    /// What the stat backup found, when it ran.
    pub check: Option<DiskCheck>,
    pub written: bool,
    /// The pid whose lease ran out.
    pub expired: Option<String>,
    /// Conflict copies the cleanup deleted.
    pub copies_removed: Vec<PathBuf>,
}

// ----- disk sync -----

impl BlobSession {
    /// The host's watcher saw the file change: hash it and record an outside save, if it is
    /// one (see [`BlobSession::check_disk`] for what happens then). Never writes the file,
    /// except a conflict copy beside it.
    pub fn file_changed(&self) -> Result<DiskCheck> {
        let r = self.file_changed_in();
        self.shown(r)
    }

    fn file_changed_in(&self) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.disk_check(&mut st, now, true)
    }

    /// The stat backup: like [`BlobSession::file_changed`], but reads the file only if its
    /// size, mtime or inode changed, or if the kit's last look was inside the file's mtime
    /// tick (git's racy rule). An outside save becomes a version by
    /// [`EXTERNAL_PID`] (`via: "disk"`). When it lands while the lease is held, or while the
    /// doc has a version the file doesn't have yet, it is a **conflict**: a pack that can
    /// merge merges it ([`DiskCheck::Merged`]); otherwise the save stays current (the person
    /// just saved it, and their app shows it), what was current before stays a version, and
    /// a conflict copy of it is written beside the file ([`DiskCheck::Conflict`],
    /// [`AuditEvent::BlobConflict`]).
    pub fn check_disk(&self) -> Result<DiskCheck> {
        let r = self.check_disk_in();
        self.shown(r)
    }

    fn check_disk_in(&self) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.disk_check(&mut st, now, false)
    }

    /// The host paired a rename of the file (same root). Then checks it.
    pub fn file_moved(&self, new_path: &Path) -> Result<DiskCheck> {
        let r = self.file_moved_in(new_path);
        self.shown(r)
    }

    fn file_moved_in(&self, new_path: &Path) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        let Some(d) = st.disk.as_mut() else {
            return Err(SessionError::BadPath("this session has no file".into()));
        };
        match disk::probe(new_path, &d.root) {
            Ok(Probe::File { .. }) => {}
            Ok(p) => return Err(probe_error(new_path, &p)),
            Err(e) => return Err(SessionError::Disk(format!("{}: {e}", new_path.display()))),
        }
        d.path = new_path.to_owned();
        d.stat = None;
        self.disk_check(st, now, true)
    }

    /// Write the current version to the file now if it doesn't hold it. Needs read.
    pub fn flush(&self, conn: ConnId) -> Result<FlushOutcome> {
        let r = self.flush_in(conn);
        self.shown(r)
    }

    fn flush_in(&self, conn: ConnId) -> Result<FlushOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Flush, now)?;
        let written = self.write_back(&mut st, now)?;
        Ok(FlushOutcome {
            written,
            hash: st.cur.blob.to_hex(),
            disk: Self::disk_state(&st),
        })
    }

    /// `r` as the host may show it: an error's paths made safe (K7): inside this file's
    /// root, relative; elsewhere, `<outside>`.
    fn shown<T>(&self, r: Result<T>) -> Result<T> {
        r.map_err(|e| {
            let root = self
                .lock_any()
                .ok()
                .and_then(|st| st.disk.as_ref().map(|d| d.root.clone()));
            match root {
                Some(root) => e.redacted(&root),
                None => e,
            }
        })
    }

    /// The identity (device and inode) of the file this session syncs with (K9, K-DISK-14):
    /// a host keys sessions by it as well as by the path. `None` without a file, when the
    /// file is gone, or where the platform has none.
    pub fn file_identity(&self) -> Result<Option<super::disk::FileIdentity>> {
        let path = self.lock()?.disk.as_ref().map(|d| d.path.clone());
        Ok(path.and_then(|p| super::disk::file_identity(&p).ok().flatten()))
    }

    pub fn file_path(&self) -> Result<Option<PathBuf>> {
        Ok(self.lock()?.disk.as_ref().map(|d| d.path.clone()))
    }

    /// Start syncing with the file at `path` (inside canonical `root`): settle a write-back
    /// a crash interrupted, then reconcile like an outside save.
    pub(crate) fn attach_file(&self, path: &Path, root: &Path) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        if let Some(d) = &st.disk {
            if d.path == path && d.root == root {
                return self.disk_check(st, now, false);
            }
            return Err(SessionError::BadPath(format!(
                "doc is open at {}; report a rename with file_moved",
                d.path.display()
            )));
        }
        let store = Self::store(st)?;
        let (synced, pending) = store.blob_sync_states(self.doc_id)?;
        let synced = synced.unwrap_or(BlobSync {
            blob: st.cur.blob,
            stat: None,
        });
        let mut d = BDisk {
            path: path.to_owned(),
            root: root.to_owned(),
            synced: synced.blob,
            own: VecDeque::new(),
            stat: synced
                .stat
                .as_deref()
                .and_then(FileStat::from_bytes)
                .map(disk::Seen::loaded),
            missing: false,
            declined: None,
            dirty_since: None,
            last_check_ms: now,
        };
        // a write-back a crash interrupted: did it land?
        if let Some(p) = pending {
            let landed = match disk::probe(path, root) {
                Ok(Probe::File { real, .. }) => {
                    disk::clean_temps(&real);
                    File::open(&real)
                        .ok()
                        .and_then(|mut f| {
                            let stat = FileStat::of(&f.metadata().ok()?);
                            let (id, _) = self.blobs.id_of(&st.key.space, &mut f).ok()?;
                            Some((id, stat))
                        })
                        .filter(|(id, _)| *id == p.blob)
                }
                _ => None,
            };
            match landed {
                Some((id, stat)) => {
                    store.set_blob_synced(
                        self.doc_id,
                        &BlobSync {
                            blob: id,
                            stat: disk::Seen::now(stat).persisted(),
                        },
                    )?;
                    d.synced = id;
                    d.stat = Some(disk::Seen::now(stat));
                }
                None => store.clear_blob_pending(self.doc_id)?,
            }
        } else if let Ok(Probe::File { real, .. }) = disk::probe(path, root) {
            disk::clean_temps(&real);
        }
        st.disk = Some(d);
        let out = self.disk_check(st, now, false)?;
        // a version the file never got (a crash before write-back): write it soon
        if st.disk.as_ref().is_some_and(|d| d.synced != st.cur.blob) {
            Self::mark_dirty(st, now);
        }
        Ok(out)
    }

    fn mark_dirty(st: &mut BState, now: u64) {
        if let Some(d) = st.disk.as_mut() {
            d.dirty_since.get_or_insert(now);
        }
    }

    fn disk_state(st: &BState) -> Option<DiskState> {
        let d = st.disk.as_ref()?;
        Some(if let Some(r) = &d.declined {
            DiskState::Declined(r.clone())
        } else if d.missing {
            DiskState::Missing
        } else if let Some(since_ms) = d.dirty_since {
            DiskState::Dirty { since_ms }
        } else {
            DiskState::Clean
        })
    }

    fn disk_error(&self, st: &BState, what: &str, e: impl std::fmt::Display) -> SessionError {
        let error = format!("{what}: {e}");
        self.emit(AuditEvent::DiskError {
            key: st.key.clone(),
            error: error.clone(),
        });
        SessionError::Disk(error)
    }

    fn set_missing(&self, st: &mut BState) -> DiskCheck {
        let d = st.disk.as_mut().expect("has disk");
        if !d.missing {
            d.missing = true;
            d.stat = None;
            let unflushed = d.dirty_since.is_some();
            self.emit(AuditEvent::FileMissing {
                key: st.key.clone(),
                unflushed,
            });
        }
        DiskCheck::Missing
    }

    /// Record that the file holds `blob` (at `stat`).
    fn set_synced(&self, st: &mut BState, blob: BlobId, stat: FileStat) -> Result<()> {
        Self::store(st)?.set_blob_synced(
            self.doc_id,
            &BlobSync {
                blob,
                stat: disk::Seen::now(stat).persisted(),
            },
        )?;
        let d = st.disk.as_mut().expect("has disk");
        d.synced = blob;
        d.stat = Some(disk::Seen::now(stat));
        Ok(())
    }

    /// Look at the file; hash it when forced or when its stat changed; record an outside
    /// save. Never writes the file (a conflict copy is a new file beside it).
    fn disk_check(&self, st: &mut BState, now: u64, force: bool) -> Result<DiskCheck> {
        let Some(d) = st.disk.as_mut() else {
            return Ok(DiskCheck::Unchanged);
        };
        d.last_check_ms = now;
        let (path, root) = (d.path.clone(), d.root.clone());
        let (real, stat) = match disk::probe(&path, &root) {
            Ok(Probe::File { real, stat, .. }) => (real, stat),
            Ok(_) => return Ok(self.set_missing(st)),
            Err(e) => return Err(self.disk_error(st, &path.display().to_string(), e)),
        };
        if !force && !d.missing && d.stat.is_some_and(|s| s.vouches_for(stat)) {
            return Ok(DiskCheck::Unchanged);
        }
        let max = self.blobs.config().max_blob_bytes;
        let hashed = File::open(&real).and_then(|mut f| {
            let stat = FileStat::of(&f.metadata()?);
            if f.metadata()?.len() > max {
                return Ok(None);
            }
            let id = self.blobs.id_of(&st.key.space, &mut f).map_err(blob_io)?;
            Ok(Some((id.0, stat)))
        });
        let (id, stat) = match hashed {
            Ok(Some(v)) => v,
            Ok(None) => {
                let d = st.disk.as_mut().expect("has disk");
                d.stat = Some(disk::Seen::now(stat));
                d.missing = false;
                if d.declined.is_none() {
                    d.declined = Some(format!("the file is over the {max}-byte blob limit"));
                    self.emit(AuditEvent::FileDeclined {
                        key: st.key.clone(),
                        code: "too_large",
                    });
                }
                return Ok(DiskCheck::Declined(Declined {
                    code: "too_large",
                    message: format!("the file is over the {max}-byte blob limit"),
                }));
            }
            Err(e) => return Err(self.disk_error(st, &real.display().to_string(), e)),
        };
        let d = st.disk.as_mut().expect("has disk");
        let was_missing = std::mem::replace(&mut d.missing, false);
        let was_declined = d.declined.take().is_some();
        d.stat = Some(disk::Seen::now(stat));
        if was_missing || was_declined {
            self.emit(AuditEvent::FileRestored {
                key: st.key.clone(),
            });
        }
        let d = st.disk.as_mut().expect("has disk");
        if id == d.synced || id == st.cur.blob {
            // the file holds what the kit knows (a touch, or our write landed unrecorded):
            // record its stat, so the next open needn't hash it
            self.set_synced(st, id, stat)?;
            if id == st.cur.blob {
                st.disk.as_mut().expect("has disk").dirty_since = None;
            }
            return Ok(DiskCheck::Unchanged);
        }
        if d.own.contains(&id) {
            // an older write of ours is back (an app saved a stale buffer, or a late echo):
            // its content is a version already; the file holds it now, and the next
            // write-back puts the current version back. Only writes made since the last
            // outside save count (the set is cleared by each: v0.9.5), and every suppressed
            // echo is reported
            self.set_synced(st, id, stat)?;
            Self::mark_dirty(st, now);
            self.emit(AuditEvent::EchoSuppressed {
                key: st.key.clone(),
            });
            return Ok(DiskCheck::OwnWrite);
        }
        let out = self.ingest(st, &real, now)?;
        // a person saved since the kit's recent writes: one of them coming back now is their
        // save, not an echo (v0.9.5)
        if let Some(d) = st.disk.as_mut() {
            d.own.clear();
        }
        Ok(out)
    }

    /// Record the file's new content (another program saved it).
    fn ingest(&self, st: &mut BState, real: &Path, now: u64) -> Result<DiskCheck> {
        let stored = File::open(real)
            .map_err(|e| BlobError::Io(e.to_string()))
            .and_then(|mut f| {
                let stat = FileStat::of(&f.metadata().map_err(|e| BlobError::Io(e.to_string()))?);
                let info = self.blobs.put(&st.key.space, &mut f)?;
                Ok((info, stat))
            });
        let (info, stat) = match stored {
            Ok(v) => v,
            Err(e) => return Err(self.disk_error(st, &real.display().to_string(), e)),
        };
        let store = Self::store(st)?;
        self.expire_lease(st, now);
        let holder = st.lease.clone();
        let d = st.disk.as_ref().expect("has disk");
        let (base, file_name) = (
            d.synced,
            real.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_owned(),
        );
        let conflict = holder.is_some() || st.cur.blob != base;
        let mut theirs = BlobVersion::new(
            info.id,
            info.size,
            now,
            EXTERNAL_PID,
            "external edit",
            "disk",
            BlobReason::External,
        );
        theirs.mtime_ms = stat.mtime_ms();
        let sync = BlobSync {
            blob: info.id,
            stat: disk::Seen::now(stat).persisted(),
        };
        if !conflict {
            let vid = store.commit_blob_versions(
                self.doc_id,
                &st.key.space,
                st.vid,
                std::slice::from_ref(&theirs),
                Some(&sync),
            )?[0];
            st.vid = vid;
            st.cur = theirs;
            let d = st.disk.as_mut().expect("has disk");
            d.synced = info.id;
            d.stat = Some(disk::Seen::now(stat));
            d.dirty_since = None;
            self.emit(AuditEvent::ExternalEdit {
                key: st.key.clone(),
                seq: vid,
            });
            return Ok(DiskCheck::Ingested { version: vid });
        }
        let (kept, ours) = (st.vid, st.cur.clone());
        if let Some(out) = self.try_merge(st, &store, base, &ours, &theirs, &sync, stat, now)? {
            return Ok(out);
        }
        // keep both: the save is current; what it replaced is a version and a copy
        let who = match &holder {
            Some(l) if ours.author.starts_with(super::RESERVED_PID_PREFIX) => l.name.clone(),
            _ => ours.author_name.clone(),
        };
        let copy = self.write_conflict_copy(st, real, &file_name, &ours, &who, now);
        theirs.conflict_copy = copy.clone();
        let at_ms = theirs.at_ms;
        let vid = store.commit_blob_versions(
            self.doc_id,
            &st.key.space,
            st.vid,
            std::slice::from_ref(&theirs),
            Some(&sync),
        )?[0];
        st.vid = vid;
        st.cur = theirs;
        if let (Some(recs), Some(rec)) = (st.copies.as_mut(), copy.clone()) {
            recs.push(CopyRec {
                version: vid,
                kept,
                at_ms,
                rec,
            });
        }
        let d = st.disk.as_mut().expect("has disk");
        d.synced = info.id;
        d.stat = Some(disk::Seen::now(stat));
        d.dirty_since = None;
        self.emit(AuditEvent::ExternalEdit {
            key: st.key.clone(),
            seq: vid,
        });
        self.emit(AuditEvent::BlobConflict {
            key: st.key.clone(),
            current: vid,
            kept,
            copy: copy.clone(),
            lease_holder: holder.map(|l| l.pid),
        });
        let base = real.parent().map(Path::to_owned);
        Ok(DiskCheck::Conflict {
            version: vid,
            kept,
            copy: copy.and_then(|c| base.map(|b| resolve_copy(&b, &c))),
        })
    }

    /// Tier 0.5: let the pack merge a conflicting save. `None`: no merge (tier 0, too big,
    /// or the pack reported a conflict).
    #[allow(clippy::too_many_arguments)]
    fn try_merge(
        &self,
        st: &mut BState,
        store: &DocStore,
        base: BlobId,
        ours: &BlobVersion,
        theirs: &BlobVersion,
        sync: &BlobSync,
        stat: FileStat,
        now: u64,
    ) -> Result<Option<DiskCheck>> {
        if !self.pack.can_merge() {
            return Ok(None);
        }
        let space = st.key.space.clone();
        let max = self.config.blob_merge_max_bytes;
        let read = |id: &BlobId| self.blobs.read_to_vec(&space, id, max);
        let (Ok(b), Ok(o), Ok(t)) = (read(&base), read(&ours.blob), read(&theirs.blob)) else {
            return Ok(None);
        };
        let BlobMerge::Merged(m) = self.pack.merge3_bytes(&b, &o, &t) else {
            return Ok(None);
        };
        let info = self.blobs.put_bytes(&space, &m)?;
        let merged = BlobVersion::new(
            info.id,
            info.size,
            now,
            MERGE_PID,
            "merge",
            "disk",
            BlobReason::Merged,
        );
        // the file holds theirs until the merge is written back
        let vids = store.commit_blob_versions(
            self.doc_id,
            &space,
            st.vid,
            &[theirs.clone(), merged.clone()],
            Some(sync),
        )?;
        st.vid = vids[1];
        st.cur = merged;
        let d = st.disk.as_mut().expect("has disk");
        d.synced = theirs.blob;
        d.stat = Some(disk::Seen::now(stat));
        self.emit(AuditEvent::ExternalEdit {
            key: st.key.clone(),
            seq: vids[0],
        });
        self.emit(AuditEvent::BlobMerged {
            key: st.key.clone(),
            version: vids[1],
        });
        Self::mark_dirty(st, now);
        let _ = self.write_back(st, now);
        Ok(Some(DiskCheck::Merged { version: vids[1] }))
    }

    /// Write `ours` as a conflict copy (atomic, never over an existing file), beside the file
    /// or in [`Host::conflict_dir`]'s folder (created if missing). Returns its record: its
    /// path relative to the file's folder (a plain file name when beside it), or `None`
    /// (reported as a disk error) if that failed: the content is a version either way.
    fn write_conflict_copy(
        &self,
        st: &BState,
        real: &Path,
        file_name: &str,
        ours: &BlobVersion,
        who: &str,
        now: u64,
    ) -> Option<String> {
        let base = real.parent()?;
        let dir = match self.host.conflict_dir(real) {
            None => base.to_owned(),
            Some(d) => {
                let made = fs::create_dir_all(&d).and_then(|()| fs::canonicalize(&d));
                match made {
                    Ok(d) => d,
                    Err(e) => {
                        let what = format!("the conflict-copy folder {}", d.display());
                        self.disk_error(st, &what, e);
                        return None;
                    }
                }
            }
        };
        let mut name = self.pack.conflict_copy_name(file_name, now, who);
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            name = conflict_copy_name(file_name, now, who);
        }
        let (stem, ext) = split_ext(&name);
        let mut chosen = None;
        for n in 1..=99 {
            let candidate = if n == 1 {
                name.clone()
            } else {
                format!("{stem} {n}{ext}")
            };
            if fs::symlink_metadata(dir.join(&candidate)).is_err() {
                chosen = Some(candidate);
                break;
            }
        }
        let chosen = chosen?;
        let target = dir.join(&chosen);
        let Some(rec) = relative_path(base, &target).to_str().map(str::to_owned) else {
            self.disk_error(st, "the conflict copy's path", "not UTF-8: not written");
            return None;
        };
        let space = st.key.space.clone();
        let written = disk::atomic_write_with(&target, |f| {
            self.blobs.copy_to(&space, &ours.blob, f).map_err(blob_io)?;
            Ok(())
        });
        match written {
            Ok(_) => Some(rec),
            Err(e) => {
                self.disk_error(st, "writing the conflict copy", e);
                None
            }
        }
    }

    /// Write the current version to the file if it doesn't hold it. Checks the file first
    /// (by stat; an outside save is recorded before anything is written), then writes
    /// atomically from the blob store (verified; a damaged blob is never written), recording
    /// the write in flight before the rename so a crash is recognised on the next open.
    fn write_back(&self, st: &mut BState, now: u64) -> Result<bool> {
        if st.disk.as_ref().map_or(true, |d| d.declined.is_some()) {
            return Ok(false);
        }
        let stale = match self.disk_check(st, now, false)? {
            DiskCheck::Missing | DiskCheck::Declined(_) => return Ok(false),
            DiskCheck::OwnWrite => true,
            _ => false,
        };
        let d = st.disk.as_mut().expect("has disk");
        if d.synced == st.cur.blob && !stale {
            d.dirty_since = None;
            return Ok(false);
        }
        let real = match disk::probe(&d.path, &d.root) {
            Ok(Probe::File { real, .. }) => real,
            Ok(_) => {
                self.set_missing(st);
                return Ok(false);
            }
            Err(e) => {
                let p = d.path.display().to_string();
                return Err(self.disk_error(st, &p, e));
            }
        };
        let store = Self::store(st)?;
        let blob = st.cur.blob;
        store.set_blob_pending(self.doc_id, &BlobSync { blob, stat: None })?;
        let space = st.key.space.clone();
        let written = disk::atomic_write_with(&real, |f| {
            self.blobs.copy_to(&space, &blob, f).map_err(blob_io)?;
            Ok(())
        });
        let stat = match written {
            Ok((stat, dir_sync)) => {
                if let Some(e) = dir_sync {
                    self.disk_error(st, "fsync of the file's folder", e);
                }
                stat
            }
            Err(e) => {
                let _ = store.clear_blob_pending(self.doc_id);
                return Err(self.disk_error(st, &real.display().to_string(), e));
            }
        };
        // the file is in place: if recording it fails, the next open hashes the file,
        // finds the write in flight and promotes it
        if let Err(e) = self.set_synced(st, blob, stat) {
            self.disk_error(st, "recording the write-back", e);
        }
        let d = st.disk.as_mut().expect("has disk");
        d.synced = blob;
        d.stat = Some(disk::Seen::now(stat));
        d.own.push_back(blob);
        while d.own.len() > OWN_WRITES_KEPT {
            d.own.pop_front();
        }
        d.dirty_since = None;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_copy_names() {
        // 2026-10-27 14:30:05 UTC
        let at = 1_793_111_405_000;
        assert_eq!(
            conflict_copy_name("report.pdf", at, "Alice"),
            "report (conflict Alice 2026-10-27 143005).pdf"
        );
        assert_eq!(
            conflict_copy_name("Makefile", at, "a/b:c"),
            "Makefile (conflict a_b_c 2026-10-27 143005)"
        );
        assert_eq!(
            conflict_copy_name(".bashrc", at, "  "),
            ".bashrc (conflict someone 2026-10-27 143005)"
        );
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
