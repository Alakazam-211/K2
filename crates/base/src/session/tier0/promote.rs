//! Tier 0 → text promotion (v0.9.2): a blob doc whose content now reads as text becomes a
//! text doc of a [`Pack`], **keeping its key, its doc id and its history**. Driven by
//! [`crate::session::SessionManager::promote_blob_file`] / `promote_blob`; the contract is in
//! `docs/SESSIONS.md`, "Tier 0 → text".
//!
//! Under the blob session's lock: the lease must be free; the file is reconciled (an
//! outside save becomes a version first) and written back; then every blob version is read
//! and decoded by the pack, oldest first, and the text history is built through the kit's
//! own engine calls (never crafted bytes): each version that decodes becomes a text
//! version, through one change by its author at its time when its text differs from the
//! one before. One store transaction ([`crate::store::DocStore::promote_blob_doc`]) switches
//! the doc; then the blobs only the moved versions held are removed at once, and the
//! session closes.

use std::sync::Arc;

use super::*;
use crate::doc::{ClientId, Doc, Edit, Neutral};
use crate::engine::{ENGINE_FORMAT, ENGINE_NAME};
use crate::hash::file_hash;
use crate::session::live::{fresh_client, ordered_roots, record_root};
use crate::session::pack::Pack;
use crate::session::summary;
use crate::store::{
    fresh_key, ChangeInfo, ChangeKind, ClockSpan, LogEntry, NewChange, PromoteRows, RevertRecord,
    Seq, SyncState, Version, VersionReason,
};
use crate::text::{diff, lines};

/// What one tier-0 version became in a promotion ([`Promotion::versions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Promoted {
    /// It decoded as text: it is text version `version` of the doc, at log position `seq`.
    /// `change` is the log position of the change that made its text (by its author, at
    /// its time, `via` and `on_behalf_of` as recorded), or `None` when its text equals the
    /// version's before it (the bytes differed only in line endings or a BOM, say): then
    /// there is no change, only the version.
    Text {
        version: VersionId,
        seq: Seq,
        change: Option<Seq>,
        /// The blob version also stays in the doc's tier-0 history: it was written on
        /// someone's behalf but made no text change, so no change carries its approval.
        tier0_kept: bool,
    },
    /// It doesn't read as text (`code`: the pack's refusal, such as `mixed_line_endings`,
    /// `binary`, `not_utf8`, `too_large`; or `unreadable` when its blob couldn't be read):
    /// it stays in the doc's **tier-0 history**, bytes, entry and approval as they were.
    NonText { code: &'static str },
    /// It was purged before: it stays purged in the tier-0 history (its entry only);
    /// nothing of it is imported.
    Purged,
}

/// Returned by [`crate::session::SessionManager::promote_blob_file`] and `promote_blob`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Promotion {
    pub key: DocKey,
    /// Every tier-0 version, oldest first, and what it became.
    pub versions: Vec<(VersionId, Promoted)>,
    /// Log changes the text history was built from (the import and each later change).
    pub changes: u64,
    /// Blobs removed at once: the moved versions' content, now in the text history, that
    /// nothing else used (another version or doc with the same bytes keeps it).
    pub blobs_removed: u64,
    /// The connections that were in the blob session (gone with it): tell their tabs.
    pub connections: Vec<(ConnId, String)>,
}

impl Promotion {
    /// The versions kept as tier-0 history (non-text, purged, or kept for an approval).
    pub fn tier0_kept(&self) -> usize {
        self.versions
            .iter()
            .filter(|(_, p)| {
                !matches!(
                    p,
                    Promoted::Text {
                        tier0_kept: false,
                        ..
                    }
                )
            })
            .count()
    }
}

/// One blob version that decoded.
struct Decoded {
    vid: VersionId,
    v: BlobVersion,
    roots: Vec<(String, Neutral)>,
    codec: Vec<u8>,
}

impl BlobSession {
    /// Promote this blob doc to a text doc of `pack` (see the module docs). On success the
    /// session is closed ([`SessionError::Unloaded`] from then on) and
    /// [`AuditEvent::Promoted`] is emitted; on any error nothing changed.
    ///
    /// Errors: [`SessionError::Leased`] while anyone holds the lease;
    /// [`SessionError::Declined`] when the current version doesn't read as text (the pack's
    /// reason; nothing changes: the doc stays tier 0); [`SessionError::Disk`] when the file
    /// can't be brought to the current version first; a store error (nothing written).
    pub(crate) fn promote(&self, pack: &Arc<dyn Pack>) -> Result<Promotion> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.expire_lease(st, now);
        if let Some(l) = &st.lease {
            return Err(SessionError::Leased {
                holder: l.pid.clone(),
                name: l.name.clone(),
                expires_ms: l.expires_ms,
            });
        }
        // the file holds the current version (an outside save is a version first)
        if st.disk.is_some() {
            self.disk_check(st, now, true)?;
            if st.disk.as_ref().is_some_and(|d| d.dirty_since.is_some()) {
                self.write_back(st, now)?;
            }
            match Self::disk_state(st) {
                None | Some(DiskState::Clean | DiskState::Missing) => {}
                Some(DiskState::Dirty { .. }) => {
                    return Err(SessionError::Disk(
                        "the file doesn't hold the current version yet; retry".into(),
                    ))
                }
                Some(DiskState::Declined(r)) => {
                    return Err(SessionError::Declined(Declined {
                        code: "too_large",
                        message: r,
                    }))
                }
            }
        }
        let store = Self::store(st)?;
        let space = st.key.space.clone();
        let max = self.config.disk_read_max_bytes;
        let schema = pack.schema();
        let approvals: HashMap<VersionId, Approval> =
            store.blob_approvals(self.doc_id)?.into_iter().collect();

        // read and decode, oldest first (one blob in memory at a time, plus the current's)
        let mut outcome: Vec<(VersionId, Promoted)> = Vec::new();
        let mut decoded: Vec<Decoded> = Vec::new();
        let mut current_bytes = Vec::new();
        for (vid, v) in store.blob_versions(self.doc_id)? {
            let current = vid == st.vid;
            if v.redacted {
                outcome.push((vid, Promoted::Purged));
                continue;
            }
            let bytes = if v.size > max {
                Err(Declined {
                    code: "too_large",
                    message: format!("{} bytes; text reads up to {max}", v.size),
                })
            } else {
                match self.blobs.read_to_vec(&space, &v.blob, max) {
                    Ok(b) => Ok(b),
                    Err(e) if current => return Err(e.into()),
                    Err(e) => Err(Declined {
                        code: "unreadable",
                        message: e.to_string(),
                    }),
                }
            };
            let content = bytes.and_then(|b| {
                let c = pack.decode(&b)?;
                if current {
                    current_bytes = b;
                }
                Ok(c)
            });
            match content {
                Ok(c) => {
                    let roots = ordered_roots(&schema, c.roots)?;
                    decoded.push(Decoded {
                        vid,
                        v,
                        roots,
                        codec: c.codec,
                    });
                }
                Err(why) if current => return Err(SessionError::Declined(why)),
                Err(why) => outcome.push((vid, Promoted::NonText { code: why.code })),
            }
        }

        // the text history, built through the engine as any change is
        let mut doc = Doc::new(schema)?;
        doc.set_edit_check_every(self.config.edit_check_every);
        let mut clients: Vec<(ClientId, String)> = Vec::new();
        let mut entries: Vec<LogEntry> = Vec::new();
        let mut changes: Vec<NewChange> = Vec::new();
        let mut carried: Vec<(usize, Approval)> = Vec::new();
        let mut versions: Vec<Version> = Vec::new();
        let mut moved: Vec<VersionId> = Vec::new();
        let mut text_vid: HashMap<VersionId, VersionId> = HashMap::new();
        let mut codec = Vec::new();
        let mut seq: Seq = 0;
        for d in decoded {
            let Decoded { vid, v, roots, .. } = &d;
            codec = d.codec.clone();
            let client = match clients.iter().find(|(_, p)| *p == v.author) {
                Some((c, _)) => *c,
                None => {
                    let c = fresh_client(&store, self.doc_id, |c| {
                        clients.iter().any(|(x, _)| *x == c)
                    })?;
                    clients.push((c, v.author.clone()));
                    c
                }
            };
            let first = versions.is_empty();
            let before: Vec<(String, String)> = roots
                .iter()
                .map(|(r, _)| Ok((r.clone(), doc.text(r)?)))
                .collect::<Result<_>>()?;
            let mut edits = Vec::new();
            for ((root, Neutral::Text(t)), (_, old)) in roots.iter().zip(&before) {
                let hunks = lines::align_insertions(old, diff::hunks(old, t));
                if !hunks.is_empty() {
                    edits.push(Edit::Text {
                        root: root.clone(),
                        hunks,
                    });
                }
            }
            let clock_before = doc.clock_of(client);
            let change = match doc.edit_as(client, &edits)? {
                None => None,
                Some(update) => {
                    seq += 1;
                    let mut kind = match v.reason {
                        _ if first => ChangeKind::Import,
                        BlobReason::External => ChangeKind::External,
                        BlobReason::Restore => ChangeKind::Restore,
                        _ => ChangeKind::Edit,
                    };
                    let of = v.restored_from.and_then(|r| text_vid.get(&r).copied());
                    if kind == ChangeKind::Restore && of.is_none() {
                        // what it restored isn't text history: an edit to that text
                        kind = ChangeKind::Edit;
                    }
                    let mut info = ChangeInfo::new(kind, &v.author, &v.via, v.at_ms);
                    info.of = of.filter(|_| kind == ChangeKind::Restore);
                    info.on_behalf_of = v.on_behalf_of.clone();
                    let mut record = Vec::new();
                    for ((root, Neutral::Text(t)), (_, old)) in roots.iter().zip(&before) {
                        let new = doc.text(root)?;
                        if new != *t {
                            return Err(SessionError::Broken(format!(
                                "promotion: root {root:?} of version {vid} didn't apply"
                            )));
                        }
                        if *old == new {
                            continue;
                        }
                        let hunks = diff::hunks(old, &new);
                        let (a, r) = summary::line_counts(old, &new, &hunks);
                        info.lines_added += a as u64;
                        info.lines_removed += r as u64;
                        // a diff of the texts, as an outside edit's record: no roots
                        record.push(record_root(root, old, new.is_empty(), &hunks, None));
                    }
                    changes.push(NewChange {
                        info,
                        record: RevertRecord {
                            author: v.author.clone(),
                            on_behalf_of: v.on_behalf_of.clone(),
                            at_ms: v.at_ms,
                            roots: record,
                        },
                        span: Some(ClockSpan {
                            client,
                            start: clock_before,
                            end: doc.clock_of(client),
                        }),
                        provenance: Vec::new(),
                    });
                    entries.push(LogEntry {
                        author: v.author.clone(),
                        client,
                        at_ms: v.at_ms,
                        via: v.via.clone(),
                        update,
                        on_behalf_of: v.on_behalf_of.clone(),
                    });
                    if let (Some(_), Some(a)) = (&v.on_behalf_of, approvals.get(vid)) {
                        carried.push((entries.len() - 1, a.clone()));
                    }
                    Some(seq)
                }
            };
            let reason = match v.reason {
                _ if first => VersionReason::Import,
                BlobReason::External | BlobReason::Merged => VersionReason::External,
                _ => VersionReason::Idle,
            };
            versions.push(Version {
                seq,
                at_ms: v.at_ms,
                reason,
                name: v.name.clone(),
                authors: vec![v.author.clone()],
                content: roots.clone(),
            });
            let version = versions.len() as VersionId;
            text_vid.insert(*vid, version);
            // its approval needs a change to live on; without one the blob version stays
            let tier0_kept = v.on_behalf_of.is_some() && change.is_none();
            if !tier0_kept {
                moved.push(*vid);
            }
            outcome.push((
                *vid,
                Promoted::Text {
                    version,
                    seq,
                    change,
                    tier0_kept,
                },
            ));
        }
        outcome.sort_by_key(|(vid, _)| *vid);

        // the switch: one transaction
        let old = store.meta(self.doc_id)?;
        let hash_key = fresh_key()?;
        let hash = file_hash(&hash_key, &current_bytes);
        let meta = DocMeta {
            key: st.key.clone(),
            pack: schema.name.to_owned(),
            engine: ENGINE_NAME.to_owned(),
            engine_format: ENGINE_FORMAT,
            codec,
            created_ms: old.created_ms,
            synced_hash: Some(hash.clone()),
            synced_seq: seq,
            declined: None,
        };
        let sync = SyncState {
            hash,
            seq,
            state: doc.encode_state()?,
        };
        let released = store.promote_blob_doc(
            self.doc_id,
            &PromoteRows {
                expect_last: st.vid,
                meta: &meta,
                clients: &clients,
                entries: &entries,
                changes: &changes,
                approvals: &carried,
                versions: &versions,
                sync: &sync,
                doc_key: &hash_key,
                moved: &moved,
                now,
            },
        )?;
        // the moved versions' bytes now live in the text history: a blob nothing else
        // uses goes now, not after the GC grace (a later purge of the text must not miss it)
        let free: Vec<(u64, BlobId, u64)> = released
            .iter()
            .filter(|r| r.3 == 0)
            .map(|r| (r.0, r.1, r.2))
            .collect();
        // committed: a failure here leaves them to the GC (index rows unreferenced)
        let blobs_removed = self.blobs.remove_unreferenced_now(&free).map_or(0, |r| r.0);
        let connections: Vec<(ConnId, String)> = st
            .members
            .iter()
            .map(|m| (m.conn, m.p.pid.clone()))
            .collect();
        let out = Promotion {
            key: st.key.clone(),
            versions: outcome,
            changes: entries.len() as u64,
            blobs_removed,
            connections: connections.clone(),
        };
        self.emit(AuditEvent::Promoted {
            key: st.key.clone(),
            connections,
            text_versions: versions.len() as u64,
            changes: out.changes,
            tier0_kept: out.tier0_kept() as u64,
        });
        st.closed = Some(SessionError::Unloaded);
        st.store = None;
        st.members.clear();
        st.lease = None;
        Ok(out)
    }
}
