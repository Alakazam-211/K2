//! The annotation rows as stored (`store::annot`'s tables): one thread row, one row per
//! comment (the only copy of its body), one history row per op (never a body). Each starts
//! with its format byte; every read is bounded (a damaged row reads as `Corrupt`, an error the
//! caller gets back).

use std::collections::BTreeMap;

use super::{
    AnnotEvent, AnnotOp, AttachKind, Attachment, Attribution, Comment, PendingAnchor, ThreadId,
    ThreadStatus,
};
use crate::anchor::{Anchor, AnchorState, AnchorTarget, LastSeen, ANCHOR_HINT_MAX};
use crate::index::{
    Fingerprint, NodeHash, NodeKey, NodeRect, INDEX_ATTRS_MAX, INDEX_ATTR_KEY_MAX,
    INDEX_ATTR_VALUE_MAX, INDEX_KEY_MAX, INDEX_KIND_MAX, INDEX_NAME_MAX,
};
use crate::store::enc::{R, W};
use crate::store::StoreError;

const THREAD_V: u8 = 1;
const COMMENT_V: u8 = 1;
const EVENT_V: u8 = 1;
const WHAT_THREAD: &str = "annotation thread";
const WHAT_COMMENT: &str = "annotation comment";
const WHAT_EVENT: &str = "annotation history";

/// A thread row: everything but its comments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadRec {
    pub anchor: Anchor,
    pub status: ThreadStatus,
    pub created: Attribution,
    pub pending: Option<PendingAnchor>,
    /// The anchor's state at the newest version an index commit resolved it against.
    pub last_state: Option<(u64, AnchorState)>,
    /// The doc's change seq when this thread last changed (its row, a comment, its anchor's
    /// records): what `comments --since` compares.
    pub changed: u64,
    /// History rows of this thread (bounded: `Config::annot_thread_log_max`).
    pub log_rows: u32,
    /// A purge took the anchor's copies of node content: an index record doesn't put a
    /// `last_seen` back until a re-anchor.
    pub purged: bool,
}

fn w_attr(w: &mut W, a: &Attribution) {
    w.str(&a.pid)
        .str(&a.name)
        .str(&a.via)
        .opt_str(a.on_behalf_of.as_deref())
        .u64(a.at_ms);
}

/// An attribution as read back: never refused for its length (the session bounds what it
/// writes, `annot::bound_attribution`; a row an earlier build wrote longer still reads, so it
/// can be purged). Each field is bounded by the row itself.
fn r_attr(r: &mut R<'_>) -> Result<Attribution, StoreError> {
    Ok(Attribution {
        pid: r.str()?,
        name: r.str()?,
        via: r.str()?,
        on_behalf_of: r.opt_str()?,
        at_ms: r.u64()?,
    })
}

fn w_target(w: &mut W, t: &AnchorTarget) {
    match t {
        AnchorTarget::Doc => {
            w.u8(0);
        }
        AnchorTarget::Node { key } => {
            w.u8(1).str(key.as_str());
        }
    }
}

fn r_target(r: &mut R<'_>, what: &'static str) -> Result<AnchorTarget, StoreError> {
    Ok(match r.u8()? {
        0 => AnchorTarget::Doc,
        1 => AnchorTarget::Node {
            key: NodeKey(r.str_max(INDEX_KEY_MAX)?),
        },
        _ => return Err(StoreError::Corrupt(what)),
    })
}

fn w_rect(w: &mut W, b: Option<NodeRect>) {
    match b {
        None => {
            w.u8(0);
        }
        Some(b) => {
            w.u8(1)
                .u64(b.left as u64)
                .u64(b.top as u64)
                .u64(b.right as u64)
                .u64(b.bottom as u64);
        }
    }
}

fn r_rect(r: &mut R<'_>, what: &'static str) -> Result<Option<NodeRect>, StoreError> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(NodeRect {
            left: r.u64()? as i64,
            top: r.u64()? as i64,
            right: r.u64()? as i64,
            bottom: r.u64()? as i64,
        }),
        _ => return Err(StoreError::Corrupt(what)),
    })
}

fn flag(r: &mut R<'_>, what: &'static str) -> Result<bool, StoreError> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StoreError::Corrupt(what)),
    }
}

pub(crate) fn enc_thread(t: &ThreadRec) -> Vec<u8> {
    let mut w = W::default();
    w.u8(THREAD_V).u64(t.anchor.created_version);
    w_target(&mut w, &t.anchor.target);
    w.opt_str(t.anchor.hint.as_deref());
    match &t.anchor.fingerprint {
        None => {
            w.u8(0);
        }
        Some(fp) => {
            w.u8(1).str(&fp.kind).str(&fp.name);
            w_rect(&mut w, fp.bounds);
            w.opt_bytes(fp.content.as_ref().map(|h| h.as_bytes().as_slice()));
            w.u32(fp.weak.len() as u32);
            for (k, v) in &fp.weak {
                w.str(k).str(v);
            }
        }
    }
    match &t.anchor.last_seen {
        None => {
            w.u8(0);
        }
        Some(l) => {
            w.u8(1).u64(l.version).str(&l.name);
            w_rect(&mut w, l.bounds);
        }
    }
    match &t.status {
        ThreadStatus::Open => {
            w.u8(0);
        }
        ThreadStatus::Resolved { by } => {
            w.u8(1);
            w_attr(&mut w, by);
        }
    }
    w_attr(&mut w, &t.created);
    match &t.pending {
        None => {
            w.u8(0);
        }
        Some(p) => {
            w.u8(1);
            w_target(&mut w, &p.to);
            w.u64(p.at);
            w_attr(&mut w, &p.by);
        }
    }
    match t.last_state {
        None => {
            w.u8(0);
        }
        Some((v, s)) => {
            w.u8(1).u64(v).u8(s.code());
        }
    }
    w.u64(t.changed).u32(t.log_rows).u8(u8::from(t.purged));
    w.0
}

pub(crate) fn dec_thread(b: &[u8]) -> Result<ThreadRec, StoreError> {
    let what = WHAT_THREAD;
    let mut r = R::new(b, what);
    if r.u8()? != THREAD_V {
        return Err(StoreError::Corrupt(what));
    }
    let created_version = r.u64()?;
    let target = r_target(&mut r, what)?;
    let hint = r.opt_str_max(ANCHOR_HINT_MAX)?;
    let fingerprint = if flag(&mut r, what)? {
        let kind = r.str_max(INDEX_KIND_MAX)?;
        let name = r.str_max(INDEX_NAME_MAX)?;
        let bounds = r_rect(&mut r, what)?;
        let content = match r.opt_bytes()? {
            None => None,
            Some(b) => Some(NodeHash::from_raw(
                b.try_into().map_err(|_| StoreError::Corrupt(what))?,
            )),
        };
        let n = r.u32()? as usize;
        if n > INDEX_ATTRS_MAX {
            return Err(StoreError::Corrupt(what));
        }
        let mut weak = BTreeMap::new();
        for _ in 0..n {
            let k = r.str_max(INDEX_ATTR_KEY_MAX)?;
            let v = r.str_max(INDEX_ATTR_VALUE_MAX)?;
            weak.insert(k, v);
        }
        Some(Fingerprint {
            kind,
            name,
            bounds,
            content,
            weak,
        })
    } else {
        None
    };
    let last_seen = if flag(&mut r, what)? {
        let version = r.u64()?;
        let name = r.str_max(INDEX_NAME_MAX)?;
        let bounds = r_rect(&mut r, what)?;
        Some(LastSeen::new(version, name, bounds))
    } else {
        None
    };
    let status = if flag(&mut r, what)? {
        ThreadStatus::Resolved {
            by: r_attr(&mut r)?,
        }
    } else {
        ThreadStatus::Open
    };
    let created = r_attr(&mut r)?;
    let pending = if flag(&mut r, what)? {
        let to = r_target(&mut r, what)?;
        let at = r.u64()?;
        let by = r_attr(&mut r)?;
        Some(PendingAnchor { to, at, by })
    } else {
        None
    };
    let last_state = if flag(&mut r, what)? {
        let v = r.u64()?;
        let s = AnchorState::from_code(r.u8()?).ok_or(StoreError::Corrupt(what))?;
        Some((v, s))
    } else {
        None
    };
    let changed = r.u64()?;
    let log_rows = r.u32()?;
    let purged = flag(&mut r, what)?;
    r.end()?;
    Ok(ThreadRec {
        anchor: Anchor {
            created_version,
            target,
            hint,
            fingerprint,
            last_seen,
        },
        status,
        created,
        pending,
        last_state,
        changed,
        log_rows,
        purged,
    })
}

pub(crate) fn enc_comment(c: &Comment) -> Vec<u8> {
    let mut w = W::default();
    w.u8(COMMENT_V);
    w_attr(&mut w, &c.by);
    w.opt_str(c.body.as_deref()).strs(&c.mentions);
    w.u32(c.attachments.len() as u32);
    for a in &c.attachments {
        w.u8(match a.kind {
            AttachKind::Proposal => 1,
        })
        .str(&a.target);
    }
    match &c.deleted {
        None => {
            w.u8(0);
        }
        Some(d) => {
            w.u8(1);
            w_attr(&mut w, d);
        }
    }
    w.0
}

pub(crate) fn dec_comment(n: u32, b: &[u8]) -> Result<Comment, StoreError> {
    let what = WHAT_COMMENT;
    let mut r = R::new(b, what);
    if r.u8()? != COMMENT_V {
        return Err(StoreError::Corrupt(what));
    }
    let by = r_attr(&mut r)?;
    let body = r.opt_str()?;
    let mentions = r.strs()?;
    let k = r.u32()? as usize;
    if k > super::ANNOT_ATTACHMENTS_MAX {
        return Err(StoreError::Corrupt(what));
    }
    let mut attachments = Vec::with_capacity(k);
    for _ in 0..k {
        let kind = match r.u8()? {
            1 => AttachKind::Proposal,
            _ => return Err(StoreError::Corrupt(what)),
        };
        attachments.push(Attachment {
            kind,
            target: r.str_max(super::ANNOT_ATTACHMENT_MAX)?,
        });
    }
    let deleted = if flag(&mut r, what)? {
        Some(r_attr(&mut r)?)
    } else {
        None
    };
    r.end()?;
    Ok(Comment {
        n,
        by,
        body,
        mentions,
        attachments,
        deleted,
    })
}

pub(crate) fn enc_event(e: &AnnotEvent) -> Vec<u8> {
    let mut w = W::default();
    w.u8(EVENT_V).u8(e.op.code()).u64(e.thread.0);
    match e.comment {
        None => {
            w.u8(0);
        }
        Some(n) => {
            w.u8(1).u32(n);
        }
    }
    w_attr(&mut w, &e.by);
    match &e.to {
        None => {
            w.u8(0);
        }
        Some(t) => {
            w.u8(1);
            w_target(&mut w, t);
        }
    }
    match e.version {
        None => {
            w.u8(0);
        }
        Some(v) => {
            w.u8(1).u64(v);
        }
    }
    w.0
}

pub(crate) fn dec_event(seq: u64, b: &[u8]) -> Result<AnnotEvent, StoreError> {
    let what = WHAT_EVENT;
    let mut r = R::new(b, what);
    if r.u8()? != EVENT_V {
        return Err(StoreError::Corrupt(what));
    }
    let op = AnnotOp::from_code(r.u8()?).ok_or(StoreError::Corrupt(what))?;
    let thread = ThreadId(r.u64()?);
    let comment = if flag(&mut r, what)? {
        Some(r.u32()?)
    } else {
        None
    };
    let by = r_attr(&mut r)?;
    let to = if flag(&mut r, what)? {
        Some(r_target(&mut r, what)?)
    } else {
        None
    };
    let version = if flag(&mut r, what)? {
        Some(r.u64()?)
    } else {
        None
    };
    r.end()?;
    Ok(AnnotEvent {
        seq,
        thread,
        comment,
        op,
        by,
        to,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by(pid: &str) -> Attribution {
        Attribution {
            pid: pid.into(),
            name: "Ann".into(),
            via: "tab".into(),
            on_behalf_of: Some("bob".into()),
            at_ms: 7,
        }
    }

    /// A row an earlier build wrote with a long attribution still reads (so it can be
    /// purged): the decoder never refuses a field for its length.
    #[test]
    fn a_long_attribution_reads_back() {
        let mut a = by("p");
        a.name = "n".repeat(10_000);
        a.pid = "q".repeat(5_000);
        let c = Comment {
            n: 0,
            by: a,
            body: Some("x".into()),
            mentions: Vec::new(),
            attachments: Vec::new(),
            deleted: None,
        };
        assert_eq!(dec_comment(0, &enc_comment(&c)).unwrap(), c);
    }

    #[test]
    fn rows_round_trip_and_refuse_damage() {
        let mut weak = BTreeMap::new();
        weak.insert("placed".to_owned(), "P1".to_owned());
        let t = ThreadRec {
            anchor: Anchor {
                created_version: 3,
                target: AnchorTarget::Node {
                    key: NodeKey("@n:4".into()),
                },
                hint: Some("0.5,0.5".into()),
                fingerprint: Some(Fingerprint {
                    kind: "raster".into(),
                    name: "Logo".into(),
                    bounds: Some(NodeRect {
                        left: -4,
                        top: 0,
                        right: 9,
                        bottom: i64::MAX,
                    }),
                    content: Some(NodeHash::from_raw([9; 32])),
                    weak,
                }),
                last_seen: Some(LastSeen::new(4, "Logo", None)),
            },
            status: ThreadStatus::Resolved { by: by("a") },
            created: by("c"),
            pending: Some(PendingAnchor {
                to: AnchorTarget::Doc,
                at: 9,
                by: by("agent"),
            }),
            last_state: Some((4, AnchorState::Suspect(crate::anchor::AnchorDoubt::WeakKey))),
            changed: 9,
            log_rows: 3,
            purged: true,
        };
        let b = enc_thread(&t);
        assert_eq!(dec_thread(&b).unwrap(), t);
        let c = Comment {
            n: 2,
            by: by("a"),
            body: Some("hello".into()),
            mentions: vec!["bob".into()],
            attachments: vec![Attachment::proposal("out/logo-v2.psd")],
            deleted: Some(by("d")),
        };
        assert_eq!(dec_comment(2, &enc_comment(&c)).unwrap(), c);
        let e = AnnotEvent {
            seq: 5,
            thread: ThreadId(1),
            comment: Some(0),
            op: AnnotOp::ReanchorProposed,
            by: by("x"),
            to: Some(AnchorTarget::Node {
                key: NodeKey("@k:r2.0".into()),
            }),
            version: Some(2),
        };
        assert_eq!(dec_event(5, &enc_event(&e)).unwrap(), e);
        for cut in 0..b.len() {
            assert!(
                dec_thread(&b[..cut]).is_err(),
                "a cut row at {cut} is refused"
            );
        }
        let mut long = b.clone();
        long.push(0);
        assert!(dec_thread(&long).is_err());
        let mut bad = b;
        bad[0] = 9;
        assert!(dec_thread(&bad).is_err());
    }
}
