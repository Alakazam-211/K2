//! Annotations on a blob doc (`docs/SESSIONS.md`, "Annotations"): the calls a host's views
//! use, rights, attribution, caps and events. The tables and their transactions are the
//! store's ([`crate::store`]'s `annot`); the anchors' states are [`crate::anchor`]'s, with
//! the pack's rules ([`BlobPack::judge_anchor`], [`BlobPack::anchor_candidate`]).
//!
//! Every call takes the session lock only to admit the caller (rights asked of the host, the
//! rate limit) and to copy an App viewer's handles; index reads and the store's transaction
//! run after it, so a comment never waits on a write's upload. A comment write holds the
//! session's annotation gate shared from its admission to its commit, and an erasure holds it
//! alone: nothing admitted before an erasure lands after it.
//!
//! **What a call reports is what it committed**: the store hands back the thread as its
//! transaction left it, and the event is emitted from that, so a committed change never reads
//! back as a failure (an anchor that can't be resolved just now reads `resolution: None`).

use std::collections::BTreeSet;

use super::*;
use crate::anchor::{
    planted, resolve_in, Anchor, AnchorResolution, AnchorState, AnchorTarget, LastSeen, NodeIndex,
    ANCHOR_HINT_MAX,
};
use crate::annot::rec::ThreadRec;
use crate::annot::{
    AnnotCall, AnnotEvent, AnnotOp, AnnotOutput, AnnotPurge, Attachment, Attribution, Comment,
    NewAnchor, NewComment, PendingAnchor, Reanchored, Thread, ThreadId, ThreadPage, ThreadQuery,
    ThreadStatus, ANNOT_ATTACHMENTS_MAX, ANNOT_ATTACHMENT_MAX, ANNOT_LOG_MAX, ANNOT_MENTIONS_MAX,
    ANNOT_NAME_MAX, ANNOT_PAGE_BYTES, ANNOT_PAGE_DEFAULT, ANNOT_PAGE_MAX, ANNOT_PID_MAX,
    ANNOT_RESOLVE_LOOKBACK,
};
use crate::index::{IndexView, NodeKey};
use crate::store::{AnnotFail, Committed, Needles, Observed, MIN_MATCH};

/// Who is calling, admitted.
struct Caller {
    attr: Attribution,
    kind: ParticipantKind,
    owner: bool,
    /// May read history: else no node copy from an older version is shown.
    history: bool,
    key: DocKey,
    store: Arc<DocStore>,
    /// The doc's current version.
    vid: VersionId,
}

/// One version's index and its key map.
type Indexed = (Arc<IndexView>, Arc<NodeIndex>);

/// The versions' indexes one call reads, each at most once.
#[derive(Default)]
struct Views {
    by_version: HashMap<VersionId, Option<Indexed>>,
    /// The doc's versions, newest first, purged ones left out (read on first need).
    versions: Option<Vec<VersionId>>,
}

fn fail<T>(r: std::result::Result<std::result::Result<T, AnnotFail>, StoreError>) -> Result<T> {
    match r? {
        Ok(t) => Ok(t),
        Err(AnnotFail::NoThread(thread)) => Err(SessionError::NoAnnotation {
            thread,
            comment: None,
        }),
        Err(AnnotFail::NoComment(thread, n)) => Err(SessionError::NoAnnotation {
            thread,
            comment: Some(n),
        }),
        Err(AnnotFail::Cap { what, limit }) => Err(SessionError::AnnotationCap { what, limit }),
        Err(AnnotFail::NotYours(thread, comment)) => {
            Err(SessionError::NotCommentAuthor { thread, comment })
        }
        Err(AnnotFail::VersionGone(version)) => Err(SessionError::NotIndexed {
            version,
            why: crate::index::NotIndexedWhy::Purged,
        }),
        Err(AnnotFail::Erasure(why)) => Err(SessionError::BadParticipant(why.to_owned())),
    }
}

fn bad(why: impl Into<String>) -> SessionError {
    SessionError::BadAnnotation(why.into())
}

fn printable(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// `s` cut to at most `max` bytes, at a character boundary.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut e = max;
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    s[..e].to_owned()
}

/// The attribution a comment write stores: its ids (pid, on behalf of) are identities, so
/// one over [`ANNOT_PID_MAX`] bytes is refused (`MPC-E115`, the host's to fix); its display
/// fields (name, surface) are cut to [`ANNOT_NAME_MAX`] bytes. Every stored row then reads.
fn bound_attribution(mut a: Attribution) -> Result<Attribution> {
    if planted!(AttrUnbounded) {
        return Ok(a);
    }
    let long = |s: &str| s.len() > ANNOT_PID_MAX;
    if long(&a.pid) || a.on_behalf_of.as_deref().is_some_and(long) {
        return Err(SessionError::BadParticipant(format!(
            "a participant id is at most {ANNOT_PID_MAX} bytes to comment"
        )));
    }
    a.name = cut(&a.name, ANNOT_NAME_MAX);
    a.via = cut(&a.via, ANNOT_NAME_MAX);
    Ok(a)
}

/// Mentions as given, capped before any work: at most [`ANNOT_MENTIONS_MAX`].
fn cap_mentions(pids: &[String]) -> Result<()> {
    if pids.len() > ANNOT_MENTIONS_MAX {
        return Err(bad(format!("at most {ANNOT_MENTIONS_MAX} mentions")));
    }
    Ok(())
}

/// Mentions as stored: each a pid of at most [`ANNOT_PID_MAX`] bytes, once each, in order.
fn check_mentions(pids: Vec<String>) -> Result<Vec<String>> {
    cap_mentions(&pids)?;
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut keep = vec![false; pids.len()];
    for (i, p) in pids.iter().enumerate() {
        if p.is_empty() || p.len() > ANNOT_PID_MAX || !printable(p) {
            return Err(bad(format!(
                "a mention is a participant id of 1 to {ANNOT_PID_MAX} bytes"
            )));
        }
        keep[i] = seen.insert(p.as_str());
    }
    Ok(pids
        .into_iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(p))
        .collect())
}

fn check_attachments(a: &[Attachment]) -> Result<()> {
    if a.len() > ANNOT_ATTACHMENTS_MAX {
        return Err(bad(format!("at most {ANNOT_ATTACHMENTS_MAX} attachments")));
    }
    for x in a {
        if x.target.is_empty() || x.target.len() > ANNOT_ATTACHMENT_MAX || !printable(&x.target) {
            return Err(bad(format!(
                "an attachment's target is 1 to {ANNOT_ATTACHMENT_MAX} bytes"
            )));
        }
    }
    Ok(())
}

fn ev(op: AnnotOp, by: &Attribution) -> AnnotEvent {
    AnnotEvent {
        seq: 0,
        thread: ThreadId(0),
        comment: None,
        op,
        by: by.clone(),
        to: None,
        version: None,
    }
}

// ----- what an App viewer sees (K5): pids become its handles -----

fn lens_attr(l: &mut super::super::handles::Lens<'_>, mut a: Attribution) -> Attribution {
    a.pid = l.pid(&a.pid);
    a.on_behalf_of = a.on_behalf_of.map(|p| l.pid(&p));
    a
}

fn lens_thread(l: &mut super::super::handles::Lens<'_>, mut t: Thread) -> Thread {
    t.created = lens_attr(l, t.created);
    if let ThreadStatus::Resolved { by } = t.status {
        t.status = ThreadStatus::Resolved {
            by: lens_attr(l, by),
        };
    }
    if let Some(mut p) = t.pending_anchor.take() {
        p.by = lens_attr(l, p.by);
        t.pending_anchor = Some(p);
    }
    t.comments = t
        .comments
        .into_iter()
        .map(|mut c| {
            c.by = lens_attr(l, c.by);
            c.deleted = c.deleted.map(|d| lens_attr(l, d));
            c.mentions = c.mentions.iter().map(|p| l.pid(p)).collect();
            c
        })
        .collect();
    t
}

fn lens_event(l: &mut super::super::handles::Lens<'_>, mut e: AnnotEvent) -> AnnotEvent {
    e.by = lens_attr(l, e.by);
    e
}

impl BlobSession {
    /// The annotation gate, shared: a comment write's, from admission to commit.
    fn annot_write_gate(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.annot_gate.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Admit `conn` for an annotation call needing `need`; a comment-changing call (`Comment`)
    /// also takes from the pid's edit rate bucket, and its attribution is bounded. No content
    /// changes, so a declined file doesn't refuse it.
    fn annot_caller(&self, conn: ConnId, need: Need) -> Result<Caller> {
        let now = self.now();
        let who = {
            let mut st = self.lock()?;
            let st = &mut *st;
            let i = self.admit(st, conn, need, Action::Annotate, now)?;
            if need == Need::Comment {
                let c = &self.config;
                let b = st
                    .buckets
                    .entry(st.members[i].p.pid.clone())
                    .or_insert_with(|| Bucket::full(c.edit_burst, now));
                b.take(now, c.edit_rate_per_s, c.edit_burst)
                    .map_err(|retry_after_ms| SessionError::RateLimited { retry_after_ms })?;
            }
            let m = &st.members[i];
            let attr = Attribution {
                pid: m.p.pid.clone(),
                name: m.p.name.clone(),
                via: m.p.via.clone(),
                on_behalf_of: m.p.on_behalf_of.clone(),
                at_ms: now,
            };
            Caller {
                attr,
                kind: m.p.kind,
                owner: m.rights.owner,
                history: m.rights.history,
                key: st.key.clone(),
                store: Self::store(st)?,
                vid: st.vid,
            }
        };
        let who = match need {
            Need::Comment | Need::Owner => Caller {
                attr: bound_attribution(who.attr.clone())?,
                ..who
            },
            _ => who,
        };
        #[cfg(any(test, feature = "differential"))]
        if need == Need::Comment {
            crate::differential::annotation_pause_point();
        }
        Ok(who)
    }

    /// `conn` also needs `need` (history, for an older version).
    fn annot_also(&self, conn: ConnId, need: Need) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, need, Action::Annotate, now)?;
        Ok(())
    }

    /// An App viewer's pids from its handles; anyone else's as given. Only the viewer's
    /// handle list is copied under the session lock.
    fn annot_unpid(&self, conn: ConnId, pids: Vec<String>) -> Vec<String> {
        if pids.is_empty() {
            return pids;
        }
        let seen = {
            let Ok(st) = self.lock() else {
                return pids;
            };
            let Some(v) = st
                .members
                .iter()
                .find(|m| m.conn == conn && m.p.kind == ParticipantKind::App)
                .map(|m| m.p.pid.clone())
            else {
                return pids;
            };
            st.handles.seen_by(&v)
        };
        pids.iter()
            .map(|p| super::super::handles::unpid_in(&seen, p))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn annot_emit_full(
        &self,
        who: &Caller,
        seq: u64,
        thread: ThreadId,
        comment: Option<u32>,
        op: AnnotOp,
        mentions: Vec<String>,
        anchor_key: Option<NodeKey>,
    ) {
        self.emit(AuditEvent::Annotation {
            key: who.key.clone(),
            seq,
            thread,
            comment,
            op,
            pid: who.attr.pid.clone(),
            on_behalf_of: who.attr.on_behalf_of.clone(),
            mentions,
            anchor_key,
        });
    }

    /// The event of what `done` committed (nothing when it changed nothing).
    fn annot_emit_done(&self, who: &Caller, done: &Committed, op: AnnotOp, mentions: Vec<String>) {
        if let Some(seq) = done.seq {
            let key = done.rec.anchor.key().cloned();
            self.annot_emit_full(who, seq, done.id, done.n, op, mentions, key);
        }
    }

    fn check_body(&self, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(bad("a comment needs a body"));
        }
        let max = self.config.annot_body_max_bytes;
        if body.len() as u64 > max {
            return Err(SessionError::AnnotationCap {
                what: "body bytes",
                limit: max,
            });
        }
        Ok(())
    }

    /// The anchor `a` names, made now, with its state in the version it names; an older
    /// version needs history.
    fn new_anchor(
        &self,
        conn: ConnId,
        a: &NewAnchor,
        cur: VersionId,
    ) -> Result<(Anchor, AnchorState)> {
        match a {
            NewAnchor::Doc => Ok((Anchor::doc(cur), AnchorState::Attached)),
            NewAnchor::Node { key, version, hint } => {
                if hint
                    .as_ref()
                    .is_some_and(|h| h.len() > ANCHOR_HINT_MAX || !printable(h))
                {
                    return Err(bad(format!("a hint is at most {ANCHOR_HINT_MAX} bytes")));
                }
                let k = NodeKey::parse(key)
                    .ok_or_else(|| bad(format!("{key:?} is not a node key (@n:… or @k:…)")))?;
                if version.is_some_and(|v| v != cur) {
                    self.annot_also(conn, Need::History)?;
                }
                let view = self.index_view(*version)?;
                let ix = NodeIndex::new(&view.nodes);
                let Some(n) = view.nodes.iter().find(|n| n.key == k) else {
                    let holders: Vec<&str> = view
                        .nodes
                        .iter()
                        .filter(|n| n.native.as_ref() == Some(&k))
                        .map(|n| n.key.as_str())
                        .collect();
                    if holders.len() > 1 {
                        return Err(bad(format!(
                            "{k} is held by {} nodes in version {}: name one by its key ({})",
                            holders.len(),
                            view.version,
                            holders.join(", ")
                        )));
                    }
                    return Err(bad(format!("no node {k} in version {}", view.version)));
                };
                let anchor = Anchor::on_node(n, view.version, hint.clone());
                let r = resolve_in(
                    &anchor,
                    view.version,
                    &view.nodes,
                    &ix,
                    &PackRules(&*self.pack),
                );
                Ok((anchor, r.state))
            }
        }
    }

    /// Version `at`'s index, or the newest earlier one that has one (at most
    /// [`ANNOT_RESOLVE_LOOKBACK`] versions back); `None` when there is none.
    fn view_near(
        &self,
        store: &DocStore,
        views: &mut Views,
        at: VersionId,
    ) -> Result<Option<Indexed>> {
        if let Some(Some(v)) = views.by_version.get(&at) {
            return Ok(Some(v.clone()));
        }
        if views.versions.is_none() {
            let mut vs: Vec<VersionId> = store
                .blob_versions(self.doc_id)?
                .into_iter()
                .filter(|(_, v)| !v.redacted)
                .map(|(id, _)| id)
                .collect();
            vs.reverse();
            views.versions = Some(vs);
        }
        let tried: Vec<VersionId> = views
            .versions
            .as_ref()
            .map(|vs| {
                vs.iter()
                    .copied()
                    .filter(|v| *v <= at)
                    .take(ANNOT_RESOLVE_LOOKBACK)
                    .collect()
            })
            .unwrap_or_default();
        for v in tried {
            if let Some(got) = views.by_version.get(&v) {
                if got.is_some() {
                    return Ok(got.clone());
                }
                continue;
            }
            let got = match self.index_view(Some(v)) {
                Ok(view) => {
                    let ix = NodeIndex::new(&view.nodes);
                    Some((Arc::new(view), Arc::new(ix)))
                }
                Err(SessionError::IndexUnsupported) => return Ok(None),
                Err(SessionError::IndexPending { .. } | SessionError::NotIndexed { .. }) => None,
                Err(SessionError::Store(StoreError::NoVersion(..))) => None,
                Err(e) => return Err(e),
            };
            views.by_version.insert(v, got.clone());
            if got.is_some() {
                return Ok(got);
            }
        }
        Ok(None)
    }

    /// `a` resolved at `at`; `None` when no index applies (or it can't be read just now).
    fn resolution(
        &self,
        store: &DocStore,
        views: &mut Views,
        a: &Anchor,
        at: VersionId,
    ) -> Option<AnchorResolution> {
        let rules = PackRules(&*self.pack);
        if matches!(a.target, AnchorTarget::Doc) {
            return Some(resolve_in(a, at, &[], &NodeIndex::default(), &rules));
        }
        let (view, ix) = self.view_near(store, views, at).ok().flatten()?;
        let mut r = resolve_in(a, view.version, &view.nodes, &ix, &rules);
        r.current = view.version == at;
        Some(r)
    }

    /// A thread as `who` may see it, resolved at `at`. A reader without history sees no copy
    /// of a node from an older version: not the fingerprint or `last_seen` taken there, and
    /// no resolution against an older index.
    fn thread_of(
        &self,
        who: &Caller,
        views: &mut Views,
        id: ThreadId,
        rec: ThreadRec,
        comments: Vec<Comment>,
        at: VersionId,
    ) -> Thread {
        let mut anchor = rec.anchor;
        let mut resolution = self.resolution(&who.store, views, &anchor, at);
        if !who.history && !planted!(HistoryLeak) {
            if anchor.created_version != who.vid {
                anchor.fingerprint = None;
            }
            if anchor
                .last_seen
                .as_ref()
                .is_some_and(|l| l.version != who.vid)
            {
                anchor.last_seen = None;
            }
            if resolution.as_ref().is_some_and(|r| !r.current) {
                resolution = None;
            }
        }
        Thread {
            id,
            anchor,
            resolution,
            status: rec.status,
            created: rec.created,
            comments,
            pending_anchor: rec.pending,
        }
    }

    /// The thread a write committed, as `who` sees it now.
    fn thread_from(&self, who: &Caller, done: Committed) -> Thread {
        self.thread_of(
            who,
            &mut Views::default(),
            done.id,
            done.rec,
            done.comments,
            who.vid,
        )
    }

    /// **Start a comment thread** on the whole doc or a node (`docs/SESSIONS.md`,
    /// "Annotations"). Needs [`Rights::comment`] (edit implies it); a node in an older version
    /// needs history too. The node must exist in that version's index
    /// ([`SessionError::IndexPending`] until it is recorded; [`SessionError::IndexUnsupported`]
    /// on a type with none, where only [`NewAnchor::Doc`] works), and the version must still
    /// be live when the thread commits (`MPC-E712` if a purge won the race). Makes no version
    /// and no undo step; an agent's comment on someone's behalf starts no approval. Refused:
    /// an empty body, bad mentions or attachments (`MPC-E321`); past a cap (`MPC-E907`).
    /// Emits [`AuditEvent::Annotation`] with the mentions (the host notifies them).
    pub fn comment(&self, conn: ConnId, c: NewComment) -> Result<Thread> {
        let out = self.comment_inner(conn, c);
        self.for_viewer(conn, out, lens_thread)
    }

    fn comment_inner(&self, conn: ConnId, c: NewComment) -> Result<Thread> {
        cap_mentions(&c.mentions)?;
        let _gate = self.annot_write_gate();
        let mentions = self.annot_unpid(conn, c.mentions);
        let who = self.annot_caller(conn, Need::Comment)?;
        self.check_body(&c.body)?;
        let mentions = check_mentions(mentions)?;
        check_attachments(&c.attachments)?;
        let (anchor, state) = self.new_anchor(conn, &c.anchor, who.vid)?;
        let at = anchor.created_version;
        let rec = ThreadRec {
            anchor,
            status: ThreadStatus::Open,
            created: who.attr.clone(),
            pending: None,
            last_state: Some((at, state)),
            changed: 0,
            log_rows: 0,
            purged: false,
        };
        let first = Comment {
            n: 0,
            by: who.attr.clone(),
            body: Some(c.body),
            mentions: mentions.clone(),
            attachments: c.attachments,
            deleted: None,
        };
        let done = fail(who.store.annot_create(
            self.doc_id,
            &rec,
            &first,
            &ev(AnnotOp::Created, &who.attr),
            self.config.annot_threads_max,
        ))?;
        self.annot_emit_done(&who, &done, AnnotOp::Created, mentions);
        Ok(self.thread_from(&who, done))
    }

    /// **Reply** on thread `t` (open or resolved; a reply doesn't reopen it). Needs comment.
    pub fn reply(
        &self,
        conn: ConnId,
        t: ThreadId,
        body: String,
        mentions: Vec<String>,
        attachments: Vec<Attachment>,
    ) -> Result<Thread> {
        let out = self.reply_inner(conn, t, body, mentions, attachments);
        self.for_viewer(conn, out, lens_thread)
    }

    fn reply_inner(
        &self,
        conn: ConnId,
        t: ThreadId,
        body: String,
        mentions: Vec<String>,
        attachments: Vec<Attachment>,
    ) -> Result<Thread> {
        cap_mentions(&mentions)?;
        let _gate = self.annot_write_gate();
        let mentions = self.annot_unpid(conn, mentions);
        let who = self.annot_caller(conn, Need::Comment)?;
        self.check_body(&body)?;
        let mentions = check_mentions(mentions)?;
        check_attachments(&attachments)?;
        let c = Comment {
            n: 0,
            by: who.attr.clone(),
            body: Some(body),
            mentions: mentions.clone(),
            attachments,
            deleted: None,
        };
        let done = fail(who.store.annot_reply(
            self.doc_id,
            t,
            &c,
            &ev(AnnotOp::Replied, &who.attr),
            self.config.annot_comments_max,
        ))?;
        self.annot_emit_done(&who, &done, AnnotOp::Replied, mentions);
        Ok(self.thread_from(&who, done))
    }

    /// **Resolve** thread `t` (already resolved: unchanged, no history row). Needs comment.
    pub fn resolve(&self, conn: ConnId, t: ThreadId) -> Result<Thread> {
        let out = self.set_status(conn, t, true);
        self.for_viewer(conn, out, lens_thread)
    }

    /// **Reopen** a resolved thread `t` (already open: unchanged). Needs comment.
    pub fn reopen(&self, conn: ConnId, t: ThreadId) -> Result<Thread> {
        let out = self.set_status(conn, t, false);
        self.for_viewer(conn, out, lens_thread)
    }

    fn set_status(&self, conn: ConnId, t: ThreadId, resolved: bool) -> Result<Thread> {
        let _gate = self.annot_write_gate();
        let who = self.annot_caller(conn, Need::Comment)?;
        let op = if resolved {
            AnnotOp::Resolved
        } else {
            AnnotOp::Reopened
        };
        let done = fail(who.store.annot_update(
            self.doc_id,
            t,
            Some(self.config.annot_thread_log_max),
            &mut |rec| {
                if rec.status.is_open() != resolved {
                    return Ok(None);
                }
                rec.status = if resolved {
                    ThreadStatus::Resolved {
                        by: who.attr.clone(),
                    }
                } else {
                    ThreadStatus::Open
                };
                Ok(Some(ev(op, &who.attr)))
            },
        ))?;
        self.annot_emit_done(&who, &done, op, Vec::new());
        Ok(self.thread_from(&who, done))
    }

    /// **Delete** comment `n` of thread `t`: its body (and mentions, attachments) is cleared
    /// **at once**, with no undo; who and when stay. Its author, the person it was made for,
    /// or an owner (`MPC-E117` otherwise). An already deleted one is left as it is. The
    /// cleared bytes stay in the store file's freed pages until its next rewrite (a purge
    /// makes one: [`AnnotPurge::Comment`]).
    pub fn delete_comment(&self, conn: ConnId, t: ThreadId, n: u32) -> Result<Thread> {
        let out = self.delete_inner(conn, t, n);
        self.for_viewer(conn, out, lens_thread)
    }

    fn delete_inner(&self, conn: ConnId, t: ThreadId, n: u32) -> Result<Thread> {
        let _gate = self.annot_write_gate();
        let who = self.annot_caller(conn, Need::Comment)?;
        let me = who.attr.pid.clone();
        let owner = who.owner;
        let may = move |c: &Comment| {
            planted!(AnyoneDeletes)
                || owner
                || c.by.pid == me
                || c.by.on_behalf_of.as_deref() == Some(me.as_str())
        };
        let done = match fail(who.store.annot_delete(self.doc_id, t, n, &who.attr, &may)) {
            Err(e @ SessionError::NotCommentAuthor { .. }) => {
                self.emit(AuditEvent::Refused {
                    key: who.key.clone(),
                    pid: who.attr.pid.clone(),
                    action: Action::Annotate,
                    need: Need::AuthorOrOwner,
                });
                return Err(e);
            }
            other => other?,
        };
        self.annot_emit_done(&who, &done, AnnotOp::Deleted, Vec::new());
        Ok(self.thread_from(&who, done))
    }

    /// **Re-anchor** thread `t` to `to` (a node, or the whole doc): the only call that
    /// changes where a thread points. A [`ParticipantKind::Person`]'s applies (the history
    /// row records who confirmed it, K-ANN-9) and lifts a purge's hold on the anchor's
    /// records; anyone else's (an agent's) is recorded as [`Thread::pending_anchor`], shown as
    /// a proposal, and changes nothing until a person re-anchors to it. Needs comment (and
    /// history for a node of an older version).
    pub fn reanchor(&self, conn: ConnId, t: ThreadId, to: NewAnchor) -> Result<Reanchored> {
        let out = self.reanchor_inner(conn, t, to);
        self.for_viewer(conn, out, |l, r| match r {
            Reanchored::Applied(t) => Reanchored::Applied(lens_thread(l, t)),
            Reanchored::Proposed(t) => Reanchored::Proposed(lens_thread(l, t)),
        })
    }

    fn reanchor_inner(&self, conn: ConnId, t: ThreadId, to: NewAnchor) -> Result<Reanchored> {
        let _gate = self.annot_write_gate();
        let who = self.annot_caller(conn, Need::Comment)?;
        let (anchor, state) = self.new_anchor(conn, &to, who.vid)?;
        let applies = who.kind == ParticipantKind::Person || planted!(AgentReanchorApplies);
        let target = anchor.target.clone();
        let at = anchor.created_version;
        let op = if applies {
            AnnotOp::Reanchored
        } else {
            AnnotOp::ReanchorProposed
        };
        let done = fail(who.store.annot_update(
            self.doc_id,
            t,
            Some(self.config.annot_thread_log_max),
            &mut |rec| {
                if applies {
                    rec.anchor = anchor.clone();
                    rec.pending = None;
                    rec.last_state = Some((at, state));
                    rec.purged = false;
                } else {
                    rec.pending = Some(PendingAnchor {
                        to: target.clone(),
                        at,
                        by: who.attr.clone(),
                    });
                }
                let mut e = ev(op, &who.attr);
                e.to = Some(target.clone());
                e.version = Some(at);
                Ok(Some(e))
            },
        ))?;
        if let Some(seq) = done.seq {
            let key = match &target {
                AnchorTarget::Node { key } => Some(key.clone()),
                _ => None,
            };
            self.annot_emit_full(&who, seq, t, None, op, Vec::new(), key);
        }
        let thread = self.thread_from(&who, done);
        Ok(if applies {
            Reanchored::Applied(thread)
        } else {
            Reanchored::Proposed(thread)
        })
    }

    /// One thread, its anchor resolved at `version` (`None`: the current one; an older one
    /// needs history). Needs read. What a host reads to show a thread, or a notification's
    /// body.
    pub fn thread(&self, conn: ConnId, t: ThreadId, version: Option<VersionId>) -> Result<Thread> {
        let out = self.thread_inner(conn, t, version);
        self.for_viewer(conn, out, lens_thread)
    }

    fn thread_inner(
        &self,
        conn: ConnId,
        t: ThreadId,
        version: Option<VersionId>,
    ) -> Result<Thread> {
        let who = self.annot_caller(conn, Need::Read)?;
        let at = version.unwrap_or(who.vid);
        if at != who.vid {
            self.annot_also(conn, Need::History)?;
        }
        let (rec, comments) =
            who.store
                .annot_thread(self.doc_id, t)?
                .ok_or(SessionError::NoAnnotation {
                    thread: t,
                    comment: None,
                })?;
        Ok(self.thread_of(&who, &mut Views::default(), t, rec, comments, at))
    }

    /// **The threads**, a page at a time, in id order, each anchor resolved at the query's
    /// version (an older one needs history): by status, by node, or only what changed after
    /// a change seq ([`ThreadPage::head`] of an earlier page; every change counts: a reply, an
    /// erasure's renames, a purge's clears, an anchor's new state). Needs read.
    pub fn comments(&self, conn: ConnId, q: &ThreadQuery) -> Result<ThreadPage> {
        let out = self.comments_inner(conn, q);
        self.for_viewer(conn, out, |l, mut p| {
            p.threads = p.threads.into_iter().map(|t| lens_thread(l, t)).collect();
            p
        })
    }

    fn comments_inner(&self, conn: ConnId, q: &ThreadQuery) -> Result<ThreadPage> {
        let who = self.annot_caller(conn, Need::Read)?;
        let at = q.version.unwrap_or(who.vid);
        if at != who.vid {
            self.annot_also(conn, Need::History)?;
            who.store.blob_version(self.doc_id, at)?;
        }
        let on = match &q.on {
            Some(k) => Some(
                NodeKey::parse(k)
                    .ok_or_else(|| bad(format!("{k:?} is not a node key (@n:… or @k:…)")))?,
            ),
            None => None,
        };
        let limit = q
            .limit
            .unwrap_or(ANNOT_PAGE_DEFAULT)
            .clamp(1, ANNOT_PAGE_MAX);
        let store = who.store.clone();
        // the head first: a later `since` then misses nothing that lands while this reads
        let head = store.annot_head(self.doc_id)?;
        let touched = match q.since {
            Some(s) => Some(store.annot_changed_since(self.doc_id, s)?),
            None => None,
        };
        let mut views = Views::default();
        let mut threads: Vec<Thread> = Vec::new();
        let mut bytes = 0usize;
        let mut after = q.after.map_or(0, |t| t.0);
        let mut next = None;
        'scan: loop {
            let batch = store.annot_threads(self.doc_id, after, 256)?;
            let Some(last) = batch.last() else { break };
            after = last.0 .0;
            for (id, rec) in batch {
                let keep = q.open.map_or(true, |o| rec.status.is_open() == o)
                    && on.as_ref().map_or(true, |k| rec.anchor.key() == Some(k))
                    && touched.as_ref().map_or(true, |s| s.contains(&id));
                if !keep {
                    continue;
                }
                if threads.len() >= limit || (bytes >= ANNOT_PAGE_BYTES && !threads.is_empty()) {
                    next = threads.last().map(|t| t.id);
                    break 'scan;
                }
                let Some((rec, comments)) = store.annot_thread(self.doc_id, id)? else {
                    continue;
                };
                bytes += comments
                    .iter()
                    .map(|c| c.body.as_ref().map_or(0, String::len))
                    .sum::<usize>();
                threads.push(self.thread_of(&who, &mut views, id, rec, comments, at));
            }
        }
        Ok(ThreadPage {
            threads,
            next,
            head,
        })
    }

    /// **The annotations' history** after `since` (of thread `t` only, when named, through
    /// its own index), oldest first, at most `limit` (and [`ANNOT_LOG_MAX`]): who did what,
    /// when. Never a body. Needs read and history.
    pub fn annotation_log(
        &self,
        conn: ConnId,
        t: Option<ThreadId>,
        since: u64,
        limit: usize,
    ) -> Result<Vec<AnnotEvent>> {
        let out = self.annotation_log_inner(conn, t, since, limit);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|e| lens_event(l, e)).collect()
        })
    }

    fn annotation_log_inner(
        &self,
        conn: ConnId,
        t: Option<ThreadId>,
        since: u64,
        limit: usize,
    ) -> Result<Vec<AnnotEvent>> {
        let who = self.annot_caller(conn, Need::History)?;
        if let Some(t) = t {
            if who.store.annot_thread(self.doc_id, t)?.is_none() {
                return Err(SessionError::NoAnnotation {
                    thread: t,
                    comment: None,
                });
            }
        }
        Ok(who
            .store
            .annot_events(self.doc_id, t, since, limit.clamp(1, ANNOT_LOG_MAX))?)
    }

    /// **Purge annotations** (owner, K-PURGE-3): clear what `what` names, then rewrite the
    /// store file, so no byte is left. Bodies, and with [`AnnotPurge::Thread`],
    /// [`AnnotPurge::Text`] and [`AnnotPurge::All`] the anchors' copies of node names and
    /// hints (those anchors then read `Suspect(HintsPurged)` while their key is found, and get
    /// no new copy until a re-anchor). The threads, who and when stay; the history gets a
    /// `Purged` row per thread. Audited as [`AuditEvent::Purged`]. A text is at least
    /// [`MIN_MATCH`] bytes.
    pub fn purge_annotations(&self, conn: ConnId, what: AnnotPurge) -> Result<PurgeOutcome> {
        let who = self.annot_caller(conn, Need::Owner)?;
        let needles = match &what {
            AnnotPurge::Text(t) if t.len() < MIN_MATCH => {
                return Err(bad(format!(
                    "a purge by text needs at least {MIN_MATCH} bytes"
                )))
            }
            AnnotPurge::Text(t) => Some(Needles::new([t.as_bytes()], false)),
            _ => None,
        };
        who.store.check_rewritable()?;
        let c = fail(
            who.store
                .annot_purge(self.doc_id, &what, needles.as_ref(), &who.attr),
        )?;
        let counts = PurgeCounts {
            comments: c.comments,
            anchors: c.anchors,
            ..PurgeCounts::default()
        };
        let scope = match &what {
            AnnotPurge::Comment(t, n) => PurgedScope::Annotations {
                thread: Some(*t),
                comment: Some(*n),
            },
            AnnotPurge::Thread(t) => PurgedScope::Annotations {
                thread: Some(*t),
                comment: None,
            },
            AnnotPurge::Text(t) => PurgedScope::AnnotationText { bytes: t.len() },
            AnnotPurge::All => PurgedScope::Annotations {
                thread: None,
                comment: None,
            },
        };
        Ok(self.annot_purged(&who, scope, counts))
    }

    /// The store rewrite, the audit event and the outcome of an annotation purge or erasure.
    fn annot_purged(&self, who: &Caller, scope: PurgedScope, counts: PurgeCounts) -> PurgeOutcome {
        let mut not_reached = Vec::new();
        let rewrite = match who.store.rewrite_file() {
            Ok(r) => Some(r),
            Err(_) => {
                not_reached.push(NotReached::StoreFile);
                None
            }
        };
        not_reached.push(NotReached::SentNotifications);
        not_reached.extend(NotReached::ALWAYS);
        self.emit(AuditEvent::Purged {
            key: who.key.clone(),
            by: who.attr.pid.clone(),
            scope,
            counts,
        });
        PurgeOutcome::of_blobs(counts, not_reached, rewrite)
    }

    /// **Participant erasure** on a blob doc (owner, K-PURGE-7): `pid` becomes `pseudonym` in
    /// every attribution the doc holds (its versions and approvals, and every thread,
    /// comment, history row and mention), the bodies the erased participant wrote, or an agent
    /// wrote on their behalf, are cleared, their connections are removed (`Left { Kicked }`),
    /// and the store file is rewritten. Comment writes admitted before it finish first; none
    /// lands after it. Refused as [`crate::session::Session::purge`]'s erasure is, and more
    /// (`MPC-E115`): an empty pid or pseudonym, the same one, a reserved one (`kit:`), the
    /// owner erasing themselves, a pseudonym over [`ANNOT_PID_MAX`] bytes, and a pseudonym
    /// that is already a participant's (present, or in the doc's records). The pseudonym is
    /// the host's choice; the audit event carries only it.
    pub fn erase_participant(
        &self,
        conn: ConnId,
        pid: &str,
        pseudonym: &str,
    ) -> Result<PurgeOutcome> {
        let _gate = if planted!(NoAnnotGate) {
            None
        } else {
            Some(self.annot_gate.write().unwrap_or_else(|p| p.into_inner()))
        };
        let who = self.annot_caller(conn, Need::Owner)?;
        let refuse = |m: &str| Err(SessionError::BadParticipant(m.into()));
        if pid.is_empty() || pseudonym.is_empty() || pid == pseudonym {
            return refuse("erasure needs a pid and a different, non-empty pseudonym");
        }
        if !planted!(EraseSkipsChecks) {
            let reserved = super::super::participant::RESERVED_PID_PREFIX;
            if pid.starts_with(reserved) || pseudonym.starts_with(reserved) {
                return refuse("the kit's own pids can't be erased or used as pseudonyms");
            }
            if pid == who.attr.pid {
                return refuse("an owner can't erase themselves");
            }
            if pseudonym.len() > ANNOT_PID_MAX || !printable(pseudonym) {
                return refuse("a pseudonym is a printable id of at most 256 bytes");
            }
            let present = self.lock()?.members.iter().any(|m| m.p.pid == pseudonym);
            if present {
                return refuse("the pseudonym is a participant's id");
            }
        }
        who.store.check_rewritable()?;
        let c = fail(
            who.store
                .blob_erase_participant(self.doc_id, pid, pseudonym),
        )?;
        {
            let mut st = self.lock()?;
            let st = &mut *st;
            self.remove_pid(st, pid, LeaveReason::Kicked);
            st.handles.forget(pid);
            if let Ok(store) = Self::store(st) {
                if let Ok(v) = store.blob_version(self.doc_id, st.vid) {
                    st.cur = v;
                }
            }
        }
        let counts = PurgeCounts {
            attributions: c.attributions,
            comments: c.comments,
            ..PurgeCounts::default()
        };
        Ok(self.annot_purged(
            &who,
            PurgedScope::Participant {
                pseudonym: pseudonym.to_owned(),
            },
            counts,
        ))
    }

    /// **Run an annotation verb** (`argv` as the agent typed it after `<host> doc <path>`:
    /// `comment`, `reply`, `resolve`, `resolve --reopen`, `comment --delete`, `comments`,
    /// `comments --log`, `reanchor`; [`crate::parity::ANNOTATION_VERBS`]). A thin parser over
    /// the typed calls above, which it makes with the same rights and effects: a person's
    /// action and an agent's verb are the same call. A usage error is exit 2
    /// ([`crate::tools::ToolRunError::exit`]). [`BlobSession::run_tool`] hands these verbs
    /// here too.
    pub fn run_annotation(
        &self,
        conn: ConnId,
        argv: &[String],
    ) -> std::result::Result<AnnotOutput, crate::tools::ToolRunError> {
        Ok(match AnnotCall::parse(argv)? {
            AnnotCall::Comment(c) => AnnotOutput::Thread(self.comment(conn, c)?),
            AnnotCall::Reply {
                thread,
                body,
                mentions,
                attachments,
            } => AnnotOutput::Thread(self.reply(conn, thread, body, mentions, attachments)?),
            AnnotCall::Resolve(t) => AnnotOutput::Thread(self.resolve(conn, t)?),
            AnnotCall::Reopen(t) => AnnotOutput::Thread(self.reopen(conn, t)?),
            AnnotCall::Delete { thread, n } => {
                AnnotOutput::Thread(self.delete_comment(conn, thread, n)?)
            }
            AnnotCall::Comments(q) => AnnotOutput::Page(self.comments(conn, &q)?),
            AnnotCall::Log {
                thread,
                since,
                limit,
            } => AnnotOutput::Log(self.annotation_log(conn, thread, since, limit)?),
            AnnotCall::Reanchor { thread, to } => {
                AnnotOutput::Reanchored(self.reanchor(conn, thread, to)?)
            }
        })
    }

    /// After an index job recorded a version (ready): resolve each node-anchored thread
    /// against it, off any transaction (one key map for every anchor), then apply the
    /// changed records (`last_seen`, the state) in one short transaction that skips any
    /// thread changed meanwhile and any version purged meanwhile; emit
    /// [`AuditEvent::AnchorState`] for each applied state change. Records only: no thread is
    /// moved, and an anchor a purge cleared gets no `last_seen` back until a re-anchor.
    /// `view` is read only when the doc has a node anchor.
    pub(super) fn observe_anchors(
        store: &DocStore,
        host: &dyn Host,
        pack: &dyn BlobPack,
        doc: DocId,
        key: &DocKey,
        view: impl FnOnce() -> Option<IndexView>,
    ) {
        let Ok(threads) = store.annot_node_threads(doc) else {
            return;
        };
        if threads.is_empty() {
            return;
        }
        let Some(view) = view() else {
            return;
        };
        let v = view.version;
        let ix = NodeIndex::new(&view.nodes);
        let rules = PackRules(pack);
        let mut updates: Vec<Observed> = Vec::new();
        let mut changes: Vec<crate::annot::StateChange> = Vec::new();
        for (id, rec) in threads {
            let r = resolve_in(&rec.anchor, v, &view.nodes, &ix, &rules);
            let mut new = rec.clone();
            let mut changed = false;
            if planted!(AutoReanchor) {
                if let Some(c) = r.candidates.first() {
                    new.anchor.target = AnchorTarget::Node { key: c.key.clone() };
                    changed = true;
                }
            }
            let may_see = !new.purged || planted!(LastSeenReturns);
            let newer_seen = new
                .anchor
                .last_seen
                .as_ref()
                .map_or(true, |l| l.version <= v);
            if may_see && newer_seen {
                if let Some(n) = r.node.as_ref().and_then(|k| ix_node(&view, &ix, k)) {
                    let seen = LastSeen::of(n, v);
                    if new.anchor.last_seen.as_ref() != Some(&seen) {
                        new.anchor.last_seen = Some(seen);
                        changed = true;
                    }
                }
            }
            if new.last_state.map_or(true, |(lv, _)| lv <= v)
                && new.last_state != Some((v, r.state))
            {
                let was = new.last_state.map(|(_, s)| s);
                new.last_state = Some((v, r.state));
                changed = true;
                if was != Some(r.state) {
                    changes.push((id, was, r.state));
                }
            }
            if changed {
                updates.push(Observed {
                    id,
                    expected: rec.changed,
                    rec: new,
                });
            }
        }
        let Ok(done) = store.annot_observe_apply(doc, v, &updates) else {
            return;
        };
        for (thread, was, now) in changes {
            if done.contains(&thread) {
                host.on_event(&AuditEvent::AnchorState {
                    key: key.clone(),
                    thread,
                    version: v,
                    was,
                    now,
                });
            }
        }
    }
}

/// The node of `view` with key `k`, through its key map.
fn ix_node<'a>(
    view: &'a IndexView,
    ix: &NodeIndex,
    k: &NodeKey,
) -> Option<&'a crate::index::IndexNode> {
    ix.position(k).and_then(|i| view.nodes.get(i))
}
