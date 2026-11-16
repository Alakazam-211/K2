//! The packs: each format's reader as the kit's [`BlobPack`], indexing one version of a file
//! into the kit's structure index ([`IndexDraft`]) from the model ([`index_document`]).
//!
//! **What a node is.** Depth first, parent before children:
//!
//! | Node | Kind | Children |
//! |---|---|---|
//! | a timeline | `sequence` | its tracks, then its own markers |
//! | a track | `track` | its items but gaps (in the model, not the index), then its markers |
//! | an item | `clip`, `transition`, `title`, `caption`, `generator`, `nest`, `other` | its markers, then a nest's tracks |
//! | a marker | `marker` | none |
//!
//! **Keys.** A node's key is the model's ([`crate::model`]): the format's own ID, or derived
//! from what the node is. A derived key the document repeats, or one over the kit's length
//! limit, is left out: the kit mints a key for it and carries it across versions itself.
//!
//! **What a node holds.** Times as exact seconds (`1001/30000s`) in `attrs`, and the start's
//! timecode when the timeline has a rate and the start is on its grid: drop-frame
//! (`HH:MM:SS;FF`) when the timeline is ([`crate::model::drop_frame_at`]: a file's drop-frame
//! statement counts only at `30000/1001` and `60000/1001`), else non-drop. An item's media is its
//! name and file name only, never a path. `bounds` are frames at the timeline's rate (else
//! milliseconds, floored), across, and the track's place, down. A title's or caption's text,
//! and a marker's note, are the node's text. `visible` is an item's enabled flag and a track's
//! unmuted one. The record digest is the model's fingerprint; the content digest is its
//! carried pieces' digest, when it has any. Weak IDs for anchors: an item's media key and
//! source in.
//!
//! Agents never write these files ([`AgentWrites::Refused`]): the shared timeline they will
//! edit through verbs is later work.

use std::collections::BTreeMap;
use std::io::Read;

use k2_mpc_base::index::{
    cut_text, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs, NodeKey,
    NodeRect, INDEX_ATTR_VALUE_MAX, INDEX_NAME_MAX, INDEX_SOURCE_MAX, INDEX_TEXT_MAX,
};
use k2_mpc_base::session::{AgentWrites, BlobPack};

use crate::error::{ReadError, ReadLimits};
use crate::model::{Document, Item, ItemKind, Key, KeyOrigin, Marker, Media, Timeline, Track};
use crate::time::{Rate, Snap, Time, Timecode};

/// The reader's output format, for every pack: bump when what a file indexes to changes, and
/// the kit recomputes.
pub const FORMAT: u32 = 1;

/// The index verbs' names. `export` stays `None`: a timeline has no per-node export yet.
pub const VERBS: IndexVerbs =
    IndexVerbs::new("clip list", "clip show", "clip diff", "clip blame", None);

/// Reads OTIO files.
#[derive(Debug, Clone, Copy, Default)]
pub struct OtioPack;

/// Reads FCPXML files.
#[derive(Debug, Clone, Copy, Default)]
pub struct FcpxmlPack;

/// Reads PRPROJ files.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrprojPack;

type Reader = fn(&[u8], &ReadLimits) -> Result<Document, ReadError>;

/// Reads the whole stream (bounded by the reader's size limit) and indexes it.
fn index_with(
    read: Reader,
    src: &mut dyn Read,
    limits: &IndexLimits,
) -> Result<IndexDraft, IndexRefused> {
    let read_limits = ReadLimits::default();
    let mut bytes = Vec::new();
    src.take(read_limits.max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(IndexRefused::Read)?;
    if bytes.len() as u64 > read_limits.max_bytes {
        return Err(IndexRefused::Unreadable {
            why: "over the reader's size limit".into(),
        });
    }
    let doc = read(&bytes, &read_limits).map_err(refusal)?;
    index_document(&doc, limits)
}

/// A reader's refusal as the kit's: an unknown version or object type is a file this pack
/// doesn't index; anything else is unreadable, with fixed text that quotes nothing from it.
pub fn refusal(e: ReadError) -> IndexRefused {
    match e {
        ReadError::Io(e) => IndexRefused::Read(e),
        ReadError::UnknownVersion { .. } | ReadError::Unknown { .. } => IndexRefused::Unsupported,
        e => IndexRefused::Unreadable {
            why: e.reason().into(),
        },
    }
}

macro_rules! pack {
    ($ty:ty, $name:literal, $read:path) => {
        impl BlobPack for $ty {
            fn index_spec(&self) -> Option<IndexSpec> {
                Some(IndexSpec::new($name, FORMAT, &VERBS))
            }

            fn index(
                &self,
                src: &mut dyn Read,
                _size: u64,
                limits: &IndexLimits,
            ) -> Result<IndexDraft, IndexRefused> {
                index_with($read, src, limits)
            }

            /// Agents don't write the app's file: they will edit the shared timeline by verbs.
            fn agent_writes(&self) -> AgentWrites {
                AgentWrites::Refused
            }
        }
    };
}

pack!(OtioPack, "otio", crate::otio::read_otio);
pack!(FcpxmlPack, "fcpxml", crate::fcpxml::read_fcpxml);
pack!(PrprojPack, "prproj", crate::prproj::read_prproj);

/// `s` cut to `max` bytes at a character boundary.
fn cut(s: &str, max: usize) -> (String, bool) {
    let (kept, was_cut) = cut_text(s, u32::try_from(max).unwrap_or(u32::MAX));
    (kept.to_owned(), was_cut)
}

/// The key the index gets: the model's, unless it repeats or is too long (the kit mints those).
fn index_key(k: &Key) -> Option<String> {
    match k.origin() {
        KeyOrigin::Native | KeyOrigin::Derived => {
            NodeKey::parse(k.as_str()).map(|_| k.as_str().to_owned())
        }
        _ => None,
    }
}

/// Counts the index nodes a document makes.
fn count(doc: &Document) -> u64 {
    fn tracks(ts: &[Track]) -> u64 {
        ts.iter()
            .map(|t| {
                1 + t.markers.len() as u64
                    + t.items
                        .iter()
                        .filter(|i| i.kind != ItemKind::Gap)
                        .map(|i| 1 + i.markers.len() as u64 + tracks(&i.tracks))
                        .sum::<u64>()
            })
            .sum()
    }
    doc.timelines
        .iter()
        .map(|t| 1 + t.markers.len() as u64 + tracks(&t.tracks))
        .sum()
}

/// What placing a node needs from its timeline.
struct Frame<'d> {
    rate: Option<Rate>,
    tc_start: Time,
    drop: bool,
    media: &'d [Media],
}

impl Frame<'_> {
    /// A time across, for bounds: frames at the rate, else milliseconds; floored.
    fn across(&self, t: Time) -> i64 {
        match self.rate {
            Some(r) => t.to_frames(r, Snap::Floor).unwrap_or(i64::MAX),
            None => t
                .checked_scale(1000, 1)
                .map(|ms| ms.num().div_euclid(ms.den() as i64))
                .unwrap_or(i64::MAX),
        }
    }

    /// The timecode of record time `t`, when the timeline has a rate and `t` is on its grid.
    fn timecode(&self, t: Time) -> Option<String> {
        let r = self.rate?;
        let f = self
            .tc_start
            .checked_add(t)
            .ok()?
            .to_frames(r, Snap::Refuse)
            .ok()?;
        Timecode::from_frames(f, r, self.drop)
            .ok()
            .map(|tc| tc.to_string())
    }
}

struct Builder<'d> {
    draft: IndexDraft,
    limits: IndexLimits,
    frame: Frame<'d>,
}

impl Builder<'_> {
    fn text_max(&self) -> usize {
        self.limits.max_text_bytes.min(INDEX_TEXT_MAX) as usize
    }

    /// Pushes `node` under `parent` at `depth`, refusing past the depth limit; its index.
    fn push(
        &mut self,
        mut node: DraftNode,
        parent: Option<u32>,
        depth: u32,
    ) -> Result<u32, IndexRefused> {
        if depth > u32::from(self.limits.max_depth) {
            return Err(IndexRefused::TooDeep {
                depth,
                limit: self.limits.max_depth,
            });
        }
        node.parent = parent;
        let me = self.draft.nodes.len() as u32;
        self.draft.nodes.push(node);
        Ok(me)
    }

    fn node(
        &self,
        kind: &str,
        name: &str,
        key: &Key,
        record: k2_mpc_base::index::Digest,
    ) -> DraftNode {
        let (n, cut_name) = cut(name, INDEX_NAME_MAX);
        let mut d = DraftNode::new(kind, n, record);
        d.key = index_key(key);
        if cut_name {
            d.flags.insert("name_truncated".into());
        }
        d
    }

    fn text(&self, d: &mut DraftNode, text: Option<&str>) {
        if let Some(t) = text {
            let (kept, was_cut) = cut(t, self.text_max());
            d.text = Some(kept);
            if was_cut {
                d.flags.insert("text_truncated".into());
            }
        }
    }

    fn timeline(&mut self, t: &Timeline) -> Result<(), IndexRefused> {
        let mut d = self.node("sequence", &t.name, &t.key, t.fingerprint());
        d.content = t.payload();
        let mut a = BTreeMap::new();
        if let Some(r) = t.rate {
            a.insert("rate".into(), format!("{}/{}", r.num(), r.den()));
        }
        a.insert("tc_start".into(), t.tc_start.to_string());
        a.insert("drop_frame".into(), t.drop_frame.to_string());
        if let Some(w) = t.width {
            a.insert("width".into(), w.to_string());
        }
        if let Some(h) = t.height {
            a.insert("height".into(), h.to_string());
        }
        if let Some(r) = t.audio_rate {
            a.insert("audio_rate".into(), r.to_string());
        }
        // the end is the latest end of any item; a duration the file states is carried as
        // `duration`, never used for it (`duration` is the end when the file states none)
        let end = t.extent().ok();
        if let Some(e) = end {
            a.insert("end".into(), e.to_string());
            a.insert("end_from".into(), "items".into());
        }
        if let Some(du) = t.duration.or(end) {
            a.insert("duration".into(), du.to_string());
        }
        a.insert("compound".into(), t.compound.to_string());
        // the tracks' order, whatever the format: a later track composites over an earlier one
        a.insert("stacking".into(), "last_on_top".into());
        a.insert(
            "bounds_unit".into(),
            if t.rate.is_some() { "frames" } else { "ms" }.into(),
        );
        d.attrs = a;
        let me = self.push(d, None, 0)?;
        self.tracks(&t.tracks, me, 1)?;
        self.markers(&t.markers, me, 1)
    }

    fn tracks(&mut self, ts: &[Track], parent: u32, depth: u32) -> Result<(), IndexRefused> {
        for (n, t) in ts.iter().enumerate() {
            let mut d = self.node("track", &t.name, &t.key, t.fingerprint());
            d.content = t.payload();
            d.visible = !t.muted;
            d.attrs.insert("track_kind".into(), t.kind.name().into());
            d.attrs.insert("muted".into(), t.muted.to_string());
            d.attrs.insert("locked".into(), t.locked.to_string());
            let me = self.push(d, Some(parent), depth)?;
            for i in t.items.iter().filter(|i| i.kind != ItemKind::Gap) {
                self.item(i, me, depth + 1, n as i64)?;
            }
            self.markers(&t.markers, me, depth + 1)?;
        }
        Ok(())
    }

    fn item(&mut self, i: &Item, parent: u32, depth: u32, row: i64) -> Result<(), IndexRefused> {
        let mut d = self.node(i.kind.name(), &i.name, &i.key, i.fingerprint());
        d.content = i.payload();
        d.visible = i.enabled;
        self.text(&mut d, i.text.as_deref());
        let end = i.end().ok();
        let mut a = BTreeMap::new();
        a.insert("start".into(), i.start.to_string());
        a.insert("duration".into(), i.duration.to_string());
        if let Some(e) = end {
            a.insert("end".into(), e.to_string());
        }
        if let Some(tc) = self.frame.timecode(i.start) {
            a.insert("start_tc".into(), tc);
        }
        a.insert("enabled".into(), i.enabled.to_string());
        if let Some(s) = i.source_in {
            a.insert("source_in".into(), s.to_string());
            d.weak.insert("source_in".into(), s.to_string());
        }
        if let Some(mk) = &i.media {
            d.weak
                .insert("media".into(), cut(mk.as_str(), INDEX_ATTR_VALUE_MAX).0);
            if let Some(m) = self.frame.media.iter().find(|m| &m.key == mk) {
                a.insert("media".into(), cut(&m.name, INDEX_ATTR_VALUE_MAX).0);
                a.insert(
                    "media_file".into(),
                    cut(m.file_name(), INDEX_ATTR_VALUE_MAX).0,
                );
                a.insert("media_kind".into(), m.kind.name().into());
            }
        }
        if let Some(t) = &i.transition {
            a.insert("transition".into(), cut(&t.kind, INDEX_ATTR_VALUE_MAX).0);
            if let Some(o) = t.in_offset {
                a.insert("cut_offset".into(), o.to_string());
            }
        }
        if !i.effects.is_empty() {
            a.insert(
                "effects".into(),
                cut(&i.effects.join(", "), INDEX_ATTR_VALUE_MAX).0,
            );
        }
        d.attrs = a;
        let left = self.frame.across(i.start);
        let right = end.map_or(i64::MAX, |e| self.frame.across(e));
        d.bounds = Some(NodeRect {
            left,
            top: row,
            right,
            bottom: row.saturating_add(1),
        });
        let me = self.push(d, Some(parent), depth)?;
        self.markers(&i.markers, me, depth + 1)?;
        self.tracks(&i.tracks, me, depth + 1)
    }

    fn markers(&mut self, ms: &[Marker], parent: u32, depth: u32) -> Result<(), IndexRefused> {
        for m in ms {
            let mut d = self.node("marker", &m.name, &m.key, m.fingerprint());
            self.text(&mut d, m.note.as_deref());
            let mut a = BTreeMap::new();
            a.insert("marker_kind".into(), m.kind.name().into());
            a.insert("at".into(), m.at.to_string());
            a.insert("duration".into(), m.duration.to_string());
            if let Some(t) = m.media_at {
                a.insert("media_at".into(), t.to_string());
            }
            if let Some(c) = &m.color {
                a.insert("color".into(), cut(c, INDEX_ATTR_VALUE_MAX).0);
            }
            if let Some(tc) = self.frame.timecode(m.at) {
                a.insert("at_tc".into(), tc);
            }
            d.attrs = a;
            self.push(d, Some(parent), depth)?;
        }
        Ok(())
    }
}

/// Indexes a document read by any translator, under `limits`.
pub fn index_document(doc: &Document, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let n = count(doc);
    if n > u64::from(limits.max_nodes) {
        return Err(IndexRefused::TooManyNodes {
            count: n,
            limit: limits.max_nodes,
        });
    }
    let source = cut(
        &format!("{}-{}", doc.format.name(), doc.version),
        INDEX_SOURCE_MAX,
    )
    .0;
    let mut draft = IndexDraft::new(source);
    draft
        .attrs
        .insert("timelines".into(), doc.timelines.len().to_string());
    draft
        .attrs
        .insert("media".into(), doc.media.len().to_string());
    let mut b = Builder {
        draft,
        limits: *limits,
        frame: Frame {
            rate: None,
            tc_start: Time::ZERO,
            drop: false,
            media: &doc.media,
        },
    };
    for t in &doc.timelines {
        b.frame.rate = t.rate;
        b.frame.tc_start = t.tc_start;
        b.frame.drop = t.drop_frame;
        b.timeline(t)?;
    }
    Ok(b.draft)
}
