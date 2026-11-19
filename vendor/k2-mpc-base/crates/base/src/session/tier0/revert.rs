//! Reverting one version of a blob doc, and a participant's own undo and redo
//! (`docs/SESSIONS.md`, "Revert, undo and redo on a blob doc").
//!
//! A revert of version K is the file type's own merge run so that K's change is taken away
//! from head: `merge3_known` with base K, ours head and theirs K-1 ([`SaveBase::Revert`]).
//! Before it, the kit's own check: what K changed (`index(K-1)` to `index(K)`) against what
//! changed since (`index(K)` to head), by node and field, as `--base` reads it; an overlap
//! refuses (`MPC-E731`), naming who changed it. The merged bytes then go through the verb
//! path's checks: read with the file type's index, the change from head inside what K changed
//! (`MPC-E726`), claims under [`ClaimPolicy::Respect`], the dry-run contract, and one commit
//! with the automatic lease around it only.
//!
//! `undo` and `redo` are reverts of the caller's own versions, read from `blob_verbs`: no new
//! table.

use std::collections::BTreeSet;

use super::base::overlap;
use super::node_claims::reach_of;
use super::struct_index::Target;
use super::verbs::{
    counts_of_diff, kit_flags, tool_output, units_of_diff, wants_dry_run, wants_json, with_notes,
    KitFlags,
};
use super::*;
use crate::errors::Value;
use crate::index::read::{self as iread, NodeChange};
use crate::index::{IndexView, NodeKey};
use crate::session::{BaseConflict, ClaimPolicy};
use crate::store::{Revert, RevertStep};
use crate::tools::result::BlobResult;
use crate::tools::{exit, ChangedUnit, ToolError, ToolOutput, ToolRunError};

/// Whether the verb path's fault `$m` ([`crate::differential::Tier0VerbMutant`]) is planted on
/// this thread (test-only; always false in a host's build).
macro_rules! planted {
    ($m:ident) => {
        plant_on!(
            crate::differential::tier0_verb_mutant()
                == Some(crate::differential::Tier0VerbMutant::$m)
        )
    };
}

/// What [`BlobSession::run_revert`] takes away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RevertWhat {
    /// Version K's change (`revert <version>`).
    Version(VersionId),
    /// The caller's own latest change not yet undone.
    Undo,
    /// The caller's own latest undo, while no change of theirs came after it.
    Redo,
}

impl RevertWhat {
    fn step(self) -> RevertStep {
        match self {
            RevertWhat::Version(_) => RevertStep::Revert,
            RevertWhat::Undo => RevertStep::Undo,
            RevertWhat::Redo => RevertStep::Redo,
        }
    }
}

/// The result word of each step.
fn word(step: RevertStep) -> &'static str {
    match step {
        RevertStep::Revert => "reverted",
        RevertStep::Undo => "undone",
        RevertStep::Redo => "redone",
    }
}

/// What a revert computed before the commit.
struct Planned {
    /// The version taken away.
    k: VersionId,
    bytes: Vec<u8>,
    head: IndexView,
    after: IndexView,
    diff: Vec<NodeChange>,
    draft: crate::index::IndexDraft,
    reach: BTreeSet<NodeKey>,
    digest: String,
}

/// The caller's undo and redo targets, from their own versions' records ([`own_stacks`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct Stacks {
    undo: Vec<VersionId>,
    redo: Vec<VersionId>,
}

/// The caller's (`pid`'s) undo and redo stacks over `versions` (oldest first): their own
/// kit-made versions, at most `depth` of them back. A verb's, a batch's, a revert's or a redo's
/// version goes on the undo stack and clears redo; an undo takes its version off the undo stack
/// and goes on redo; a redo takes its undo off redo. Any other own version (a whole-file write,
/// a restore) clears redo (the planted fault [`crate::differential::Tier0VerbMutant::RedoNotCleared`]
/// keeps it). `any`: every author's (the planted fault `UndoLatestAny`).
fn own_stacks(versions: &[(VersionId, BlobVersion)], pid: &str, depth: usize, any: bool) -> Stacks {
    let mine = |v: &BlobVersion| any || v.author == pid;
    let kit_made = |v: &BlobVersion| v.verb.is_some();
    // the window: the newest `depth` own kit-made versions, and what came between them
    let start = {
        let mut n = 0usize;
        let mut at = 0usize;
        for (i, (_, v)) in versions.iter().enumerate().rev() {
            if mine(v) && kit_made(v) {
                n += 1;
                at = i;
                if n == depth {
                    break;
                }
            }
        }
        at
    };
    let keep_redo = plant_on!(
        crate::differential::tier0_verb_mutant()
            == Some(crate::differential::Tier0VerbMutant::RedoNotCleared)
    );
    let mut s = Stacks::default();
    for (vid, v) in &versions[start..] {
        if !mine(v) || v.reason == BlobReason::Import {
            continue;
        }
        match (v.verb.as_deref(), v.revert) {
            (Some(_), Some(r)) if r.step == RevertStep::Undo => {
                s.undo.retain(|x| *x != r.version);
                s.redo.push(*vid);
            }
            (Some(_), Some(r)) if r.step == RevertStep::Redo => {
                s.redo.retain(|x| *x != r.version);
                s.undo.push(*vid);
            }
            (Some(_), _) => {
                s.undo.push(*vid);
                if !keep_redo {
                    s.redo.clear();
                }
            }
            (None, _) => {
                if !keep_redo {
                    s.redo.clear();
                }
            }
        }
    }
    s
}

/// `sha256:<hex>` of `bytes`.
fn digest_of(bytes: &[u8]) -> String {
    let d = crate::index::Digest::of(bytes);
    let mut s = String::with_capacity(7 + 64);
    s.push_str("sha256:");
    for b in d.as_bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

impl BlobSession {
    /// **Revert one version, or undo or redo the caller's own** (`docs/SESSIONS.md`, "Revert,
    /// undo and redo on a blob doc"). `args` are the call's flags (`--if-version V`,
    /// `--dry-run`, `--expect DIGEST`, `--json`; `--base` is refused: the revert's own check is
    /// per node).
    ///
    /// - [`RevertWhat::Version`]`(K)`: K's change taken away from head through the file type's
    ///   merge; only K's author, the person it was made for, or an owner (`MPC-E105`).
    /// - [`RevertWhat::Undo`]: the caller's own latest verb, batch, revert or redo version not
    ///   undone yet; [`RevertWhat::Redo`]: their latest undo, while no change of theirs came
    ///   after it. Nothing to undo or redo: exit 1, nothing written, a note.
    ///
    /// The version made is the caller's, with the verb `revert`, `undo` or `redo` and the
    /// version it took away ([`BlobVersion::revert`]); its result's `data` is `{reverts: K}`.
    pub fn run_revert(
        &self,
        conn: ConnId,
        what: RevertWhat,
        args: &[String],
        opts: &crate::tools::ToolOptions,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let as_data = opts.result_json || wants_json(args);
        let out = match self.revert_in(conn, what, args, opts, as_data) {
            Err(ToolRunError::Session(e)) => {
                let e = match self.for_viewer(conn, Err::<(), SessionError>(e), |_, v| v) {
                    Err(e) => e,
                    Ok(()) => unreachable!("an error stays one"),
                };
                Err(e.into())
            }
            other => other,
        };
        if !as_data {
            return out;
        }
        let call = crate::tools::result::Call {
            verb: what.step().verb().to_owned(),
            kind: Some("edit"),
            args,
            pack: None,
            dry_run: wants_dry_run(args),
        };
        Ok(crate::tools::with_result(&call, out, || {
            self.status().map(|s| s.version).unwrap_or_default()
        }))
    }

    fn revert_in(
        &self,
        conn: ConnId,
        what: RevertWhat,
        args: &[String],
        opts: &crate::tools::ToolOptions,
        as_data: bool,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let step = what.step();
        let name = step.verb();
        crate::parity::Registry::for_blob_pack(&*self.pack)
            .map_err(|e| ToolError::usage(format!("{name}: {e}")))?;
        let f = kit_flags(args)?;
        if f.base.is_some() {
            return Err(ToolError::usage(format!(
                "{name}: --base isn't for a revert: it checks each node against what changed \
                 since the version it takes away; guard it with --if-version instead"
            ))
            .into());
        }
        if let Some(a) = f.rest.first() {
            return Err(ToolError::usage(format!(
                "{name}: {a}: a revert takes --if-version V, --dry-run, --expect DIGEST and \
                 --json"
            ))
            .into());
        }
        if let RevertWhat::Version(k) = what {
            if k <= 1 {
                return Err(ToolError::usage(format!(
                    "{name}: version {k} has no version before it to go back to (version 1 is \
                     the first)"
                ))
                .into());
            }
        }
        let spec = self
            .pack
            .index_spec()
            .ok_or(SessionError::IndexUnsupported)?;
        let dry = f.dry_run;
        let now = self.now();
        // 1. admit, under the lock, and find what to take away
        let (pid, agent, path, mut t, mut cheap, k) = {
            let mut st = self.lock()?;
            let st = &mut *st;
            let i = if dry {
                let i = self.admit(st, conn, Need::Edit, Action::Revert, now)?;
                if let Some(reason) = st.disk.as_ref().and_then(|d| d.declined.clone()) {
                    return Err(SessionError::Declined(Declined {
                        code: "declined",
                        message: reason,
                    })
                    .into());
                }
                i
            } else {
                self.admit_write(st, conn, Action::Revert, now)?
            };
            self.check_agent_verb(st, i)?;
            let pid = st.members[i].p.pid.clone();
            self.check_lease(st, &pid, now)?;
            Self::check_verb_guards(st, &f)?;
            let store = Self::store(st)?;
            let k = match what {
                RevertWhat::Version(k) => {
                    if k > st.vid {
                        return Err(
                            SessionError::Store(StoreError::NoVersion(self.doc_id, k)).into()
                        );
                    }
                    let rec = store
                        .blob_version(self.doc_id, k)
                        .map_err(SessionError::from)?;
                    let allowed = rec.author == pid
                        || rec.on_behalf_of.as_deref() == Some(pid.as_str())
                        || st.members[i].rights.owner
                        || planted!(RevertIgnoresAuthor);
                    if !allowed {
                        self.emit(AuditEvent::Refused {
                            key: st.key.clone(),
                            pid,
                            action: Action::Revert,
                            need: Need::AuthorOrOwner,
                        });
                        return Err(SessionError::Forbidden {
                            need: Need::AuthorOrOwner,
                        }
                        .into());
                    }
                    k
                }
                RevertWhat::Undo | RevertWhat::Redo => {
                    let versions = store
                        .blob_versions(self.doc_id)
                        .map_err(SessionError::from)?;
                    let s = own_stacks(
                        &versions,
                        &pid,
                        self.config.undo_depth.max(1),
                        planted!(UndoLatestAny),
                    );
                    let top = if what == RevertWhat::Undo {
                        s.undo.last()
                    } else {
                        s.redo.last()
                    };
                    match top {
                        Some(k) => *k,
                        None => {
                            let version = st.vid;
                            return Ok(self.nothing_to(step, version, as_data));
                        }
                    }
                }
            };
            (
                pid,
                st.members[i].p.kind == ParticipantKind::Agent,
                st.key.path.clone(),
                self.target_of(st, None)?,
                Self::cheap_set(st, Some(i)),
                k,
            )
        };
        if !self.pack.can_merge() {
            return Err(SessionError::RevertUnsupported {
                version: k,
                why: "this file type has no merge".to_owned(),
            }
            .into());
        }
        let mut seed = None;
        let out = self.revert_planned(
            conn, step, k, &f, opts, as_data, &spec, &pid, agent, &path, &mut t, &mut cheap,
            &mut seed,
        );
        if let Some((version, draft)) = seed {
            self.seed_index(version, draft);
        }
        out
    }

    /// Exit 1, nothing written: no own change to undo or redo.
    fn nothing_to(&self, step: RevertStep, version: VersionId, as_data: bool) -> ToolOutput {
        let what = if step == RevertStep::Redo {
            "redo"
        } else {
            "undo"
        };
        let text = format!("nothing to {what}: no change of yours to take away");
        let o = tool_output(
            format!("{text}\n").into_bytes(),
            format!("nothing to {what} · v{version}"),
            exit::NO,
            version,
        );
        let rd = crate::tools::ResultData {
            capture: crate::tools::Capture {
                base: Some(version),
                ..Default::default()
            },
            blob: Some(Box::default()),
            ..Default::default()
        };
        let note = (
            Value::obj([
                ("kind", Value::from(format!("nothing_to_{what}").as_str())),
                ("text", Value::from(text.as_str())),
            ]),
            text,
        );
        with_notes(o, rd, vec![note], as_data)
    }

    /// Steps 2 to 5: plan, check, then decide under the lock; once more on a moved head.
    #[allow(clippy::too_many_arguments)]
    fn revert_planned(
        &self,
        conn: ConnId,
        step: RevertStep,
        k: VersionId,
        f: &KitFlags,
        opts: &crate::tools::ToolOptions,
        as_data: bool,
        spec: &IndexSpec,
        pid: &str,
        agent: bool,
        path: &str,
        t: &mut Target,
        cheap: &mut CheapSet,
        seed: &mut Option<(VersionId, crate::index::IndexDraft)>,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let dry = f.dry_run;
        let mut replanned = false;
        loop {
            // 2 and 3: off the lock
            let p = self.revert_plan(k, t, spec)?;
            if let Some(x) = &f.expect {
                if *x != p.digest {
                    return Err(SessionError::NotAsPreviewed {
                        expected: x.clone(),
                        actual: p.digest.clone(),
                    }
                    .into());
                }
            }
            let put = self
                .blobs
                .put_version(
                    &t.space,
                    Some(self.doc_id),
                    &mut &p.bytes[..],
                    LockWait::Try,
                    cheap,
                    &PackHints(&*self.pack),
                )
                .map_err(SessionError::from)?;
            // 4 and 5: under the lock
            let now = self.now();
            let mut guard = match self.lock() {
                Ok(g) => g,
                Err(e) => {
                    self.blobs.finish(&t.space, put);
                    return Err(e.into());
                }
            };
            let st = &mut *guard;
            let decided = (|| -> Result<Option<usize>> {
                let i = self.admit(st, conn, Need::Edit, Action::Revert, now)?;
                self.check_lease(st, pid, now)?;
                if st.vid != t.version {
                    return Ok(None);
                }
                if agent
                    && opts.policy == ClaimPolicy::Respect
                    && !planted!(ClaimUnchecked)
                    && !planted!(RevertSkipsClaims)
                {
                    if let Some(e) = self.node_claim_against(st, pid, &p.reach, now) {
                        return Err(e);
                    }
                }
                Ok(Some(i))
            })();
            let i = match decided {
                Err(e) => {
                    self.blobs.finish(&t.space, put);
                    return Err(e.into());
                }
                Ok(None) => {
                    let actual = st.vid;
                    self.blobs.finish(&t.space, put);
                    if replanned || f.if_version.is_some() {
                        return Err(SessionError::Stale {
                            expected: f.if_version.unwrap_or(t.version),
                            actual,
                        }
                        .into());
                    }
                    replanned = true;
                    *t = self.target_of(st, None)?;
                    let i = Self::member(st, conn)?;
                    *cheap = Self::cheap_set(st, Some(i));
                    continue;
                }
                Ok(Some(i)) => i,
            };
            let info = put.info();
            if info.id == st.cur.blob {
                self.blobs.finish(&t.space, put);
                return Ok(self.revert_output(step, &p, t, path, None, true, as_data));
            }
            if dry {
                self.blobs.finish(&t.space, put);
                return Ok(self.revert_output(step, &p, t, path, None, false, as_data));
            }
            let m = &st.members[i].p;
            let mut v = BlobVersion::new(
                info.id,
                info.size,
                now,
                m.pid.clone(),
                m.name.clone(),
                m.via.clone(),
                BlobReason::Write,
            );
            v.on_behalf_of = m.on_behalf_of.clone();
            v.turn = opts.turn;
            v.verb = Some(step.verb().to_owned());
            v.revert = Some(Revert::new(k, step));
            // the automatic lease, around the commit only
            let auto = st.lease.is_none();
            if auto {
                self.grant(st, i, self.config.lease_auto_ms.max(1), now, true);
            }
            let committed = self.commit_own(st, i, v, now, put.chunked());
            if auto && st.lease.as_ref().is_some_and(|l| l.auto && l.pid == pid) {
                self.end_lease(st, LeaseEnd::Released);
            }
            self.blobs.finish(&t.space, put);
            let r = committed?;
            let out = self.revert_output(step, &p, t, path, Some(r.version), false, as_data);
            if r.changed {
                *seed = Some((r.version, p.draft));
            }
            return Ok(out);
        }
    }

    /// The bytes of version `t`, read whole: bounded by the merge bound.
    fn revert_bytes(&self, t: &Target) -> Result<Vec<u8>> {
        let limit = self.config.blob_merge_max_bytes;
        if t.ver.size > limit {
            return Err(SessionError::VerbTooLarge {
                what: "a version a revert reads",
                size: t.ver.size,
                limit,
            });
        }
        let mut r = self.blobs.reader_of(&t.space, self.doc_id, &t.ver.blob)?;
        let mut out = Vec::new();
        if let Err(e) = r.read_to_end(&mut out) {
            return Err(r.error(e).into());
        }
        Ok(out)
    }

    /// K's change taken away from head `t`: the pairing of every outside save since K-1, the
    /// kit's own check (what K changed against what changed since), the file type's merge, and
    /// the output read with its index (a change from head outside what K changed refuses).
    fn revert_plan(&self, k: VersionId, t: &Target, spec: &IndexSpec) -> Result<Planned> {
        // the version K came after (past lost versions, `store/seq_floor.rs`)
        let prev = self.version_before(&t.store, k)?;
        let before = self.index_target(Some(prev))?;
        let at_k = self.index_target(Some(k))?;
        let head = self.load_view(t, spec)?;
        let view_b = self.load_view(&before, spec)?;
        let view_k = self.load_view(&at_k, spec)?;
        let d_k = iread::diff(&view_b, &view_k);
        let changed: BTreeSet<NodeKey> = d_k.iter().map(|c| c.key.clone()).collect();
        let addresses =
            || -> Vec<String> { changed.iter().map(|k| k.as_str().to_owned()).collect() };
        // the outside saves since K-1: only a writer the pack proves keeps keys may lie between
        if !planted!(PairingAssumed) {
            let versions = t.store.blob_versions(self.doc_id)?;
            let crossings = self.crossings(spec, &t.store, &t.space, &versions, prev, t.version)?;
            if let Some(c) = crossings
                .iter()
                .find(|c| c.pairing != Some(SavePairing::Proven))
            {
                return Err(SessionError::BaseAcrossUnprovenSave {
                    save: c.save,
                    base: prev,
                    head: t.version,
                    addresses: addresses(),
                });
            }
        }
        // the kit's own check: K's change against what changed since, node by node
        let overwrite = planted!(RevertOverwrites);
        if !overwrite && t.version > k {
            let since = iread::diff(&view_k, &head);
            let mut found: Vec<(NodeKey, String)> = Vec::new();
            for c in &d_k {
                if let Some(m) = since.iter().find(|m| m.key == c.key) {
                    if let Some(field) = overlap(c, m) {
                        found.push((c.key.clone(), field));
                    }
                }
            }
            if !found.is_empty() {
                let conflicts: Vec<BaseConflict> = self.who_changed(k, t, spec, &found)?;
                return Err(SessionError::RevertConflict {
                    version: k,
                    head: t.version,
                    addresses: conflicts.iter().map(|c| c.address.clone()).collect(),
                    conflicts,
                });
            }
        }
        // the file type's merge: base K, ours head, theirs K-1
        let (b, h, then) = (
            self.revert_bytes(&at_k)?,
            self.revert_bytes(t)?,
            self.revert_bytes(&before)?,
        );
        let merged = if planted!(RevertTakesWholeVersion) {
            BlobMerge::Merged(then.clone())
        } else {
            self.pack.merge3_known(&SaveMerge {
                base: &b,
                ours: &h,
                theirs: &then,
                base_is: SaveBase::Revert,
            })
        };
        let bytes = match merged {
            BlobMerge::Merged(out) => out,
            BlobMerge::Conflict if overwrite => then,
            BlobMerge::Conflict => {
                return Err(SessionError::RevertConflict {
                    version: k,
                    head: t.version,
                    addresses: addresses(),
                    conflicts: Vec::new(),
                })
            }
            BlobMerge::Unsupported => {
                return Err(SessionError::RevertUnsupported {
                    version: k,
                    why: "the file type's merge can't take these versions".to_owned(),
                })
            }
        };
        // the output, read with the file type's own index
        let draft = self
            .pack
            .index(
                &mut &bytes[..],
                bytes.len() as u64,
                &super::struct_index::limits_of(&self.config),
            )
            .map_err(|e| SessionError::OutputUnreadable { why: e.to_string() })?;
        let after = self.output_view(t, spec, &draft, t.next)?;
        let diff = iread::diff(&head, &after);
        let strayed: Vec<String> = diff
            .iter()
            .filter(|c| !changed.contains(&c.key))
            .map(|c| c.key.as_str().to_owned())
            .collect();
        if !strayed.is_empty() && !planted!(StrayUnchecked) {
            return Err(SessionError::Strayed { keys: strayed });
        }
        let keys: BTreeSet<NodeKey> = diff.iter().map(|c| c.key.clone()).collect();
        let reach = if planted!(ReachIgnored) {
            keys
        } else {
            reach_of(
                &*self.pack,
                &[&head, &after],
                &keys,
                !planted!(ReachAtHoldTime),
            )
        };
        let digest = digest_of(&bytes);
        Ok(Planned {
            k,
            bytes,
            head,
            after,
            diff,
            draft,
            reach,
            digest,
        })
    }

    /// A revert's output (a dry run's, `after` none; `same`: the merge gave head's bytes, no
    /// version).
    #[allow(clippy::too_many_arguments)]
    fn revert_output(
        &self,
        step: RevertStep,
        p: &Planned,
        t: &Target,
        path: &str,
        after: Option<VersionId>,
        same: bool,
        as_data: bool,
    ) -> ToolOutput {
        let verb = step.verb();
        let data = Value::obj([("reverts", Value::from(p.k))]);
        if same {
            let o = tool_output(
                Vec::new(),
                format!("no change to {path} ({verb} {}) · v{}", p.k, t.version),
                exit::OK,
                t.version,
            );
            let rd = crate::tools::ResultData {
                capture: crate::tools::Capture {
                    base: Some(t.version),
                    ..Default::default()
                },
                data: Some(data),
                blob: Some(Box::default()),
                ..Default::default()
            };
            return with_notes(o, rd, Vec::new(), as_data);
        }
        let (units, more): (Vec<ChangedUnit>, usize) = units_of_diff(&p.diff, &p.head, &p.after);
        let changed = Value::obj([
            ("counts", counts_of_diff(&p.diff, &p.head, &p.after)),
            (
                "units",
                Value::List(units.iter().map(ChangedUnit::to_value).collect()),
            ),
            ("units_more", Value::from(more)),
            ("lines", Value::Null),
            (
                "bytes",
                Value::obj([
                    ("size_before", Value::from(t.ver.size)),
                    ("size_after", Value::from(p.bytes.len())),
                    ("spans", Value::Null),
                    ("removed", Value::Null),
                    ("inserted", Value::Null),
                    ("after_digest", Value::from(p.digest.as_str())),
                ]),
            ),
            ("word", Value::from(word(step))),
        ]);
        let summary = match after {
            None => format!(
                "would {verb} version {} of {path} · v{} (dry run, nothing applied)",
                p.k, t.version
            ),
            Some(v) => format!("{} version {} of {path} · v{v}", word(step), p.k),
        };
        let o = tool_output(
            format!("{} version {}\n", word(step), p.k).into_bytes(),
            summary,
            exit::OK,
            after.unwrap_or(t.version),
        );
        let rd = crate::tools::ResultData {
            capture: crate::tools::Capture {
                base: Some(t.version),
                units,
                more,
            },
            data: Some(data),
            blob: Some(Box::new(BlobResult {
                after,
                changed: Some(changed),
                view: self.view_of_change(None, &p.diff, &p.head, &p.after),
            })),
            ..Default::default()
        };
        with_notes(o, rd, Vec::new(), as_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(author: &str, verb: Option<&str>, revert: Option<Revert>) -> BlobVersion {
        let mut v = BlobVersion::new(
            crate::blob::BlobId::from_slice(&[7u8; 32]).unwrap(),
            1,
            0,
            author,
            author,
            "cli",
            BlobReason::Write,
        );
        v.verb = verb.map(str::to_owned);
        v.revert = revert;
        v
    }

    /// The stacks read a participant's own versions only, and a new change clears redo.
    #[test]
    fn own_stacks_follow_undo_and_redo() {
        let vs = vec![
            (2, ver("bot", Some("part set"), None)),
            (3, ver("cy", Some("part set"), None)),
            (4, ver("bot", Some("part set"), None)),
            (
                5,
                ver("bot", Some("undo"), Some(Revert::new(4, RevertStep::Undo))),
            ),
        ];
        assert_eq!(
            own_stacks(&vs, "bot", 100, false),
            Stacks {
                undo: vec![2],
                redo: vec![5]
            }
        );
        let mut more = vs.clone();
        more.push((
            6,
            ver("bot", Some("redo"), Some(Revert::new(5, RevertStep::Redo))),
        ));
        assert_eq!(
            own_stacks(&more, "bot", 100, false),
            Stacks {
                undo: vec![2, 6],
                redo: vec![]
            }
        );
        let mut cleared = vs;
        cleared.push((6, ver("bot", None, None)));
        assert_eq!(
            own_stacks(&cleared, "bot", 100, false).redo,
            Vec::<u64>::new()
        );
        assert_eq!(own_stacks(&cleared, "cy", 100, false).undo, vec![3]);
    }
}
