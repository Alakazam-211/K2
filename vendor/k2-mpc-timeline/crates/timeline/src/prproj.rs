//! Reads PRPROJ project files into the model, read-only. Learnt from the files' own structure
//! and from permitted sample files; no other code is used.
//!
//! **Container.** A gzip stream around one XML document, or the XML itself. The decompressed
//! document is held to [`ReadLimits::max_bytes`].
//!
//! **Objects.** The project file's root element, at document version 3 (any other version is
//! refused), holds a flat table of objects. Each has `ObjectID` (an integer the file renumbers
//! on every save) or `ObjectUID` (a GUID that survives saves), a `ClassID` and a `Version`, and
//! refers to others
//! with `ObjectRef` and `ObjectURef`. The reader knows the classes in [`CLASSES`], each at one
//! version: an object of a known class at another version, or under another class ID, is
//! refused ([`ReadError::UnknownVersion`], [`ReadError::Unknown`]); a track item of a class it
//! doesn't know is refused too. Classes the reader never reaches (settings, view state, bins)
//! are not read.
//!
//! **Timelines.** Every `Sequence`: its name, its markers, and its track groups (video, audio,
//! captions) in file order, each group's tracks in their own index order (`V1` first), so a
//! later track composites over an earlier one, as in OTIO. The video group's `FrameRate` is the
//! ticks in a frame, and `FrameRect` the frame size; the audio group's `FrameRate` the ticks in
//! a sample. Times are ticks, 254,016,000,000 a second. No start timecode and no timecode
//! setting is read: a timeline starts at zero, non-drop (`drop_frame` false).
//!
//! **Tracks and items.** A track's `ClipItems` and `TransitionItems`. A clip item's
//! `TrackItem` `Start` (zero when absent) and `End` place it; its sub-clip's clip gives its
//! `InPoint`, its `ClipID`, its markers (the source's, in media time) and its source: a media
//! file (a clip) or another sequence (a nest). `IsMuted` on a clip item disables it, on a
//! track mutes it. A caption item is a caption. A transition's cut is where its outgoing clip
//! ends and its incoming clip starts; at the head of a clip with no outgoing one, its start;
//! at a tail, its end.
//!
//! **Keys.** Native wherever the file has a GUID: `@seq:` (sequence `ObjectUID`), `@track:`
//! (track `ObjectUID`), `@clip:` (`ClipID`), `@mk:` (marker ID; a clip's markers scoped under
//! the clip, since every use of a source shows them), `@media:` (media `ObjectUID`).
//! Transitions are derived ([`crate::model`]'s table).
//!
//! **Carried.** A clip item's component chain (its effects and their parameters) and a caption
//! item's text blocks: the objects reachable from them, digested in reading order.

use std::collections::{HashMap, HashSet};
use std::io::Read;

use roxmltree::Node;

use crate::build::{place, Builder};
use crate::error::{quote, ReadError, ReadLimits};
use crate::json::{self, Json};
use crate::model::{
    time_token, Document, Format, Item, ItemKind, Key, Marker, MarkerKind, MediaKind, Opaque,
    Timeline, Track, TrackKind, TransitionInfo,
};
use crate::time::{Rate, Time};
use crate::xml::PlainAttr;

/// Ticks in a second.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// The document version this reader knows.
pub const DOCUMENT_VERSION: &str = "3";

/// The classes this reader reads: element name, class ID, and the one version it knows.
pub const CLASSES: &[(&str, &str, &str)] = &[
    ("Sequence", "6a15d903-8739-11d5-af2d-9b7855ad8974", "12"),
    (
        "VideoTrackGroup",
        "9e9abf7a-0918-49c2-91ae-991b5dde77bb",
        "13",
    ),
    (
        "AudioTrackGroup",
        "9b9238b9-53a8-4cc3-b03f-b36246d052e6",
        "6",
    ),
    (
        "DataTrackGroup",
        "b714b71d-6838-48dd-9b77-db19088ced7e",
        "1",
    ),
    (
        "VideoClipTrack",
        "f68dcd81-8805-11d5-af2d-9bfa89d4ddd4",
        "1",
    ),
    (
        "AudioClipTrack",
        "097f6203-99ae-11d5-84f2-8cf14bde7040",
        "7",
    ),
    (
        "CaptionDataClipTrack",
        "b9d20db2-f229-482d-87fd-10f1fa157107",
        "1",
    ),
    (
        "VideoClipTrackItem",
        "368b0406-29e3-4923-9fcd-094fbf9a1089",
        "8",
    ),
    (
        "AudioClipTrackItem",
        "064ec682-9ba6-11d5-af2d-9ca32c7d6164",
        "11",
    ),
    (
        "CaptionDataClipTrackItem",
        "541ba122-fe61-4350-abe5-040ed395006e",
        "3",
    ),
    (
        "VideoTransitionTrackItem",
        "3eeaed31-f78e-4144-b8e8-077656517181",
        "6",
    ),
    (
        "AudioTransitionTrackItem",
        "23f687b6-9c5a-42f4-bea1-e7b3e28e7082",
        "4",
    ),
    ("SubClip", "e0c58dc9-dbdd-4166-aef7-5db7e3f22e84", "6"),
    ("VideoClip", "9308dbef-2440-4acb-9ab2-953b9a4e82ec", "11"),
    ("AudioClip", "b8830d03-de02-41ee-84ec-fe566dc70cd9", "8"),
    (
        "TranscriptClip",
        "9e0179bb-153c-4884-b34b-eb7082f34384",
        "2",
    ),
    ("Markers", "bee50706-b524-416c-9f03-b596ce5f6866", "4"),
    ("Marker", "a45508e0-3ff7-4d04-90a7-2e0dfff4c910", "3"),
    ("Media", "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff", "30"),
    (
        "VideoMediaSource",
        "e64ddf74-8fac-4682-8aa8-0e0ca2248949",
        "2",
    ),
    (
        "AudioMediaSource",
        "f588da05-fc2a-4fbc-9383-74d653b379e3",
        "2",
    ),
    (
        "DataMediaSource",
        "ff36343e-4ece-4d37-ab61-e99b758f9d30",
        "1",
    ),
    (
        "VideoSequenceSource",
        "4752dfa9-7a7e-4a3b-a25b-cafde1a8d036",
        "3",
    ),
    (
        "AudioSequenceSource",
        "e8d4cc83-38cb-491f-9d94-e5f7e3b205ee",
        "7",
    ),
];

/// The objects a carried closure follows at most.
const CLOSURE_MAX: usize = 10_000;

/// Reads a PRPROJ file (gzip or plain XML).
pub fn read_prproj(bytes: &[u8], limits: &ReadLimits) -> Result<Document, ReadError> {
    let xml_bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        let mut z = flate2::read::GzDecoder::new(bytes).take(limits.max_bytes.saturating_add(1));
        z.read_to_end(&mut out).map_err(|_| ReadError::Syntax {
            format: "gzip",
            at: 0,
            why: "a damaged compressed stream",
        })?;
        if out.len() as u64 > limits.max_bytes {
            return Err(ReadError::TooLarge {
                limit: limits.max_bytes,
            });
        }
        std::borrow::Cow::Owned(out)
    } else {
        if bytes.len() as u64 > limits.max_bytes {
            return Err(ReadError::TooLarge {
                limit: limits.max_bytes,
            });
        }
        std::borrow::Cow::Borrowed(bytes)
    };
    let text = crate::xml::text(&xml_bytes)?;
    let xml = crate::xml::parse(text, limits)?;
    let root = xml.root_element();
    if root.tag_name().name() != "PremiereData" {
        return Err(ReadError::Unknown {
            what: "root element",
            found: quote(root.tag_name().name()),
        });
    }
    let version = root.plain("Version").unwrap_or("");
    if version != DOCUMENT_VERSION {
        return Err(ReadError::UnknownVersion {
            what: "PRPROJ document".into(),
            found: quote(version),
        });
    }
    let mut r = Reader {
        b: Builder::new(limits),
        text,
        by_id: HashMap::new(),
        by_uid: HashMap::new(),
        seq_keys: HashMap::new(),
    };
    let mut sequences = Vec::new();
    for o in root.children().filter(Node::is_element) {
        if let Some(id) = o.plain("ObjectID") {
            if r.by_id.insert(id, o).is_some() {
                return Err(ReadError::Structure {
                    why: "two objects share an ID",
                });
            }
        }
        if let Some(uid) = o.plain("ObjectUID") {
            if r.by_uid.insert(uid, o).is_some() {
                return Err(ReadError::Structure {
                    why: "two objects share an ID",
                });
            }
        }
        if o.has_tag_name("Sequence") {
            let uid = o.plain("ObjectUID").ok_or(ReadError::Structure {
                why: "a sequence without an ObjectUID",
            })?;
            let key = r.b.book.key(None, "seq", uid, true);
            r.seq_keys.insert(uid, key);
            sequences.push(o);
        }
    }
    let mut timelines = Vec::new();
    for s in sequences {
        timelines.push(r.sequence(s)?);
    }
    Ok(Document {
        format: Format::Prproj,
        version: version.to_owned(),
        timelines,
        media: r.b.media,
        opaque: Vec::new(),
    })
}

/// A child element's text.
fn child_text<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|c| c.has_tag_name(name))
        .and_then(|c| c.text())
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(name))
}

/// The first descendant element called `name` (the node itself excluded).
fn find<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.descendants().skip(1).find(|c| c.has_tag_name(name))
}

/// Ticks as a time.
fn ticks(s: &str) -> Result<Time, ReadError> {
    let t: i64 = s.trim().parse().map_err(|_| ReadError::Structure {
        why: "a tick count that is not a whole number",
    })?;
    Ok(Time::new(t, TICKS_PER_SECOND as u64)?)
}

fn opt_ticks(node: Node<'_, '_>, name: &str) -> Result<Option<Time>, ReadError> {
    child_text(node, name).map(ticks).transpose()
}

fn is_true(node: Node<'_, '_>, name: &str) -> bool {
    child_text(node, name).is_some_and(|v| v.trim() == "true")
}

fn missing(why: &'static str) -> ReadError {
    ReadError::Structure { why }
}

struct Reader<'a, 'i> {
    b: Builder,
    text: &'a str,
    by_id: HashMap<&'a str, Node<'a, 'i>>,
    by_uid: HashMap<&'a str, Node<'a, 'i>>,
    seq_keys: HashMap<&'a str, Key>,
}

impl<'a, 'i> Reader<'a, 'i> {
    /// The object a reference element (`<X ObjectRef="n"/>` or `ObjectURef`) names, checked
    /// to be one of `classes`.
    fn deref(&self, r: Node<'a, 'i>, classes: &[&str]) -> Result<Node<'a, 'i>, ReadError> {
        let o = if let Some(id) = r.plain("ObjectRef") {
            self.by_id.get(id)
        } else if let Some(uid) = r.plain("ObjectURef") {
            self.by_uid.get(uid)
        } else {
            None
        }
        .copied()
        .ok_or(missing("a reference to no object"))?;
        check_class(o, classes)?;
        Ok(o)
    }

    /// The object `parent`'s child `name` refers to.
    fn deref_child(
        &self,
        parent: Node<'a, 'i>,
        name: &str,
        classes: &[&str],
    ) -> Result<Option<Node<'a, 'i>>, ReadError> {
        match child(parent, name) {
            Some(r) => self.deref(r, classes).map(Some),
            None => Ok(None),
        }
    }

    /// The objects reachable from `start` by references, digested in reading order: a carried
    /// piece.
    fn closure(&self, what: &str, start: Node<'a, 'i>) -> Opaque {
        let mut seen: HashSet<std::ops::Range<usize>> = HashSet::new();
        let mut queue = std::collections::VecDeque::from([start]);
        let mut bytes = Vec::new();
        while let Some(o) = queue.pop_front() {
            if seen.len() >= CLOSURE_MAX || !seen.insert(o.range()) {
                continue;
            }
            bytes.extend_from_slice(self.text[o.range()].as_bytes());
            bytes.push(0);
            for d in o.descendants() {
                let next = d
                    .plain("ObjectRef")
                    .and_then(|id| self.by_id.get(id))
                    .or_else(|| d.plain("ObjectURef").and_then(|u| self.by_uid.get(u)));
                if let Some(n) = next {
                    queue.push_back(*n);
                }
            }
        }
        Opaque::of(what, &bytes)
    }

    fn sequence(&mut self, s: Node<'a, 'i>) -> Result<Timeline, ReadError> {
        check_class(s, &["Sequence"])?;
        self.b.count()?;
        let uid = s.plain("ObjectUID").unwrap_or("");
        let key = self.seq_keys[uid].clone();
        let mut tl = Timeline::new(key.clone(), child_text(s, "Name").unwrap_or(""));
        if let Some(owner) = child(s, "MarkerOwner") {
            if let Some(ms) = self.deref_child(owner, "Markers", &["Markers"])? {
                tl.markers = self.markers(ms, &key, Time::ZERO, Time::ZERO, false)?;
            }
        }
        let groups = child(s, "TrackGroups").ok_or(missing("a sequence without track groups"))?;
        for g in groups.children().filter(|c| c.has_tag_name("TrackGroup")) {
            let second = child(g, "Second").ok_or(missing("a track group without its object"))?;
            let group = self.deref(
                second,
                &["VideoTrackGroup", "AudioTrackGroup", "DataTrackGroup"],
            )?;
            let inner =
                child(group, "TrackGroup").ok_or(missing("a track group without its tracks"))?;
            let frame = opt_ticks(inner, "FrameRate")?;
            let kind = match group.tag_name().name() {
                "VideoTrackGroup" => {
                    if let Some(f) = frame {
                        tl.rate = Some(Rate::from_frame_duration(f)?);
                    }
                    if let Some(rect) = child_text(group, "FrameRect") {
                        let v: Vec<u32> = rect
                            .split(',')
                            .filter_map(|x| x.trim().parse().ok())
                            .collect();
                        if let &[l, t, r, b] = v.as_slice() {
                            tl.width = r.checked_sub(l);
                            tl.height = b.checked_sub(t);
                        }
                    }
                    TrackKind::Video
                }
                "AudioTrackGroup" => {
                    if let Some(f) = frame.filter(|f| f.num() > 0) {
                        // ticks per sample: samples per second when whole
                        let rate = Time::new(f.den() as i64, f.num() as u64)?;
                        if rate.den() == 1 {
                            tl.audio_rate = u32::try_from(rate.num()).ok();
                        }
                    }
                    TrackKind::Audio
                }
                _ => TrackKind::Caption,
            };
            if let Some(tracks) = child(inner, "Tracks") {
                let mut group = Vec::new();
                for t in tracks.children().filter(|c| c.has_tag_name("Track")) {
                    let obj = self.deref(
                        t,
                        &["VideoClipTrack", "AudioClipTrack", "CaptionDataClipTrack"],
                    )?;
                    group.push(self.track(obj, kind)?);
                }
                // by their own index, the lowest first (stable: ties keep the group's order)
                group.sort_by_key(|(index, _)| *index);
                tl.tracks.extend(group.into_iter().map(|(_, t)| t));
            }
        }
        Ok(tl)
    }

    /// A track and its own index (`V1` is 0).
    fn track(&mut self, obj: Node<'a, 'i>, kind: TrackKind) -> Result<(u32, Track), ReadError> {
        self.b.count()?;
        let uid = obj
            .plain("ObjectUID")
            .ok_or(missing("a track without an ObjectUID"))?;
        let key = self.b.book.key(None, "track", uid, true);
        let ct = find(obj, "ClipTrack").ok_or(missing("a track without its clip track"))?;
        let head = ct
            .children()
            .find(|c| c.has_tag_name("Track"))
            .ok_or(missing("a track without its header"))?;
        let index: u32 = child_text(head, "Index")
            .and_then(|i| i.trim().parse().ok())
            .unwrap_or(0);
        let name = match child_text(head, "Name") {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ => format!("{}{}", kind.letter(), index + 1),
        };
        let mut tr = Track::new(key.clone(), name, kind);
        tr.muted = is_true(head, "IsMuted");
        tr.locked = is_true(head, "IsLocked");
        // clips first: transitions find their cut among them
        if let Some(items) = child(ct, "ClipItems").and_then(|c| child(c, "TrackItems")) {
            for ti in items.children().filter(|c| c.has_tag_name("TrackItem")) {
                let o = self.deref(
                    ti,
                    &[
                        "VideoClipTrackItem",
                        "AudioClipTrackItem",
                        "CaptionDataClipTrackItem",
                    ],
                )?;
                let it = self.clip_item(o, &key)?;
                tr.items.push(it);
            }
        }
        let mut transitions = Vec::new();
        if let Some(items) = child(ct, "TransitionItems").and_then(|c| child(c, "TrackItems")) {
            for ti in items.children().filter(|c| c.has_tag_name("TrackItem")) {
                let o = self.deref(
                    ti,
                    &["VideoTransitionTrackItem", "AudioTransitionTrackItem"],
                )?;
                transitions.push(self.transition(o, &key, &tr.items)?);
            }
        }
        tr.items.extend(transitions);
        tr.items.sort_by_key(|a| a.start);
        Ok((index, tr))
    }

    /// `TrackItem` `Start` (zero when absent) and `End`, as start and duration.
    fn span(node: Node<'a, 'i>) -> Result<(Time, Time), ReadError> {
        let ti = child(node, "TrackItem").ok_or(missing("a track item without its times"))?;
        let start = opt_ticks(ti, "Start")?.unwrap_or(Time::ZERO);
        let end = opt_ticks(ti, "End")?.ok_or(missing("a track item without an end"))?;
        let dur = end.checked_sub(start)?;
        if dur.is_negative() || start.is_negative() {
            return Err(missing("a track item that ends before it starts"));
        }
        Ok((start, dur))
    }

    fn clip_item(&mut self, o: Node<'a, 'i>, track: &Key) -> Result<Item, ReadError> {
        self.b.count()?;
        let cti = find(o, "ClipTrackItem").ok_or(missing("a clip item without its body"))?;
        let (start, dur) = Self::span(cti)?;
        let sub = self
            .deref_child(cti, "SubClip", &["SubClip"])?
            .ok_or(missing("a clip item without a sub-clip"))?;
        let name = child_text(sub, "Name").unwrap_or("");
        let clip = self
            .deref_child(sub, "Clip", &["VideoClip", "AudioClip", "TranscriptClip"])?
            .ok_or(missing("a sub-clip without a clip"))?;
        let body = find(clip, "Clip").ok_or(missing("a clip without its body"))?;
        let source_in = opt_ticks(body, "InPoint")?.unwrap_or(Time::ZERO);
        let caption = o.has_tag_name("CaptionDataClipTrackItem");
        let mut kind = if caption {
            ItemKind::Caption
        } else {
            ItemKind::Clip
        };
        let mut media = None;
        if let Some(src) = self.deref_child(
            body,
            "Source",
            &[
                "VideoMediaSource",
                "AudioMediaSource",
                "DataMediaSource",
                "VideoSequenceSource",
                "AudioSequenceSource",
            ],
        )? {
            if let Some(ms) = find(src, "MediaSource") {
                if let Some(m) = self.deref_child(ms, "Media", &["Media"])? {
                    media = Some(self.media(m)?);
                }
            } else if let Some(ss) = find(src, "SequenceSource") {
                let r = child(ss, "Sequence")
                    .ok_or(missing("a sequence source without its sequence"))?;
                let seq = self.deref(r, &["Sequence"])?;
                let uid = seq.plain("ObjectUID").unwrap_or("");
                let seq_key = self.seq_keys.get(uid).cloned();
                let seq_name = child_text(seq, "Name").unwrap_or("").to_owned();
                kind = ItemKind::Nest;
                media = Some(self.b.media(
                    format!("seq\0{uid}"),
                    Some(uid),
                    &seq_name,
                    MediaKind::Sequence,
                    |m| m.sequence = seq_key,
                )?);
            }
        }
        let key = match child_text(body, "ClipID") {
            Some(id) => self.b.book.key(None, "clip", id.trim(), true),
            None => self.b.book.key(
                Some(track),
                &format!("d-{}", kind.name()),
                &format!(
                    "{}@{}",
                    crate::model::key_token(name),
                    time_token(source_in)
                ),
                false,
            ),
        };
        let mut it = Item::new(key.clone(), kind, name, start, dur);
        it.source_in = Some(source_in);
        it.media = media;
        it.enabled = !is_true(cti, "IsMuted");
        if let Some(owner) = child(body, "MarkerOwner") {
            if let Some(ms) = self.deref_child(owner, "Markers", &["Markers"])? {
                it.markers = self.markers(ms, &key, start, source_in, true)?;
            }
        }
        if let Some(c) = find(cti, "Components") {
            if let Some(chain) = c
                .plain("ObjectRef")
                .and_then(|id| self.by_id.get(id))
                .copied()
            {
                it.opaque.push(self.closure("prproj:components", chain));
            }
        }
        if caption {
            if let Some(bv) = child(o, "BlockVector") {
                it.opaque.push(self.closure("prproj:caption", bv));
            }
        }
        Ok(it)
    }

    fn transition(
        &mut self,
        o: Node<'a, 'i>,
        track: &Key,
        clips: &[Item],
    ) -> Result<Item, ReadError> {
        self.b.count()?;
        let tti = find(o, "TransitionTrackItem").ok_or(missing("a transition without its body"))?;
        let (start, dur) = Self::span(tti)?;
        let name = child_text(tti, "DisplayName")
            .or_else(|| child_text(tti, "MatchName"))
            .unwrap_or("");
        let outgoing = child_text(tti, "HasOutgoingClip").map(|v| v.trim() == "true");
        let incoming = child_text(tti, "HasIncomingClip").map(|v| v.trim() == "true");
        let end = start.checked_add(dur)?;
        let in_offset = match (outgoing, incoming) {
            (Some(false), _) => Some(Time::ZERO),
            (_, Some(false)) => Some(dur),
            _ => {
                // the boundary inside it where one clip ends and another starts
                let mut cut = None;
                for c in clips {
                    let ce = c.end()?;
                    if ce > start && ce < end && clips.iter().any(|d| d.start == ce) {
                        cut = Some(ce.checked_sub(start)?);
                        break;
                    }
                }
                cut
            }
        };
        let key = self.b.book.key(
            Some(track),
            "d-tr",
            &format!("@{}", time_token(start)),
            false,
        );
        let mut it = Item::new(key, ItemKind::Transition, name, start, dur);
        it.transition = Some(TransitionInfo::new(
            child_text(tti, "MatchName").unwrap_or(name),
            in_offset,
        ));
        Ok(it)
    }

    fn media(&mut self, m: Node<'a, 'i>) -> Result<Key, ReadError> {
        let uid = m
            .plain("ObjectUID")
            .ok_or(missing("media without an ObjectUID"))?;
        let path = child_text(m, "ActualMediaFilePath")
            .or_else(|| child_text(m, "FilePath"))
            .map(str::to_owned);
        let title = child_text(m, "Title").unwrap_or("").to_owned();
        let name = if title.is_empty() {
            path.as_deref()
                .and_then(|p| p.rsplit(['/', '\\']).next())
                .unwrap_or("")
                .to_owned()
        } else {
            title
        };
        self.b.media(
            format!("media\0{uid}"),
            Some(uid),
            &name,
            MediaKind::File,
            |e| {
                e.path = path;
            },
        )
    }

    /// A `Markers` object's markers: `at = start + (marker - origin)`; `on_item` scopes the keys
    /// under `owner` and keeps the marker's time as media time.
    fn markers(
        &mut self,
        ms: Node<'a, 'i>,
        owner: &Key,
        start: Time,
        origin: Time,
        on_item: bool,
    ) -> Result<Vec<Marker>, ReadError> {
        let mut out = Vec::new();
        let Some(list) = child(ms, "Markers") else {
            return Ok(out);
        };
        for e in list.children().filter(|c| c.has_tag_name("Marker")) {
            self.b.count()?;
            let second = child(e, "Second").ok_or(missing("a marker entry without its object"))?;
            let mo = self.deref(second, &["Marker"])?;
            let dva = child_text(mo, "DVAMarker").ok_or(missing("a marker without its data"))?;
            let v = json::parse(dva.as_bytes(), 32, 10_000)?;
            let d = v
                .get("DVAMarker")
                .ok_or(missing("a marker without its data"))?;
            let tick = |k: &str| -> Result<Option<Time>, ReadError> {
                match d.get(k).and_then(|t| t.get("ticks")) {
                    None => Ok(None),
                    Some(Json::Num(n)) => {
                        let t = n
                            .as_i64()
                            .ok_or(missing("a tick count that is not a whole number"))?;
                        Ok(Some(Time::new(t, TICKS_PER_SECOND as u64)?))
                    }
                    Some(Json::Str(s)) => ticks(s).map(Some),
                    Some(_) => Err(missing("a tick count that is not a whole number")),
                }
            };
            let at = tick("mStartTime")?.unwrap_or(Time::ZERO);
            let duration = tick("mDuration")?.unwrap_or(Time::ZERO);
            if duration.is_negative() {
                return Err(missing("a negative duration"));
            }
            let id = d
                .get("mMarkerID")
                .and_then(Json::as_str)
                .or_else(|| child_text(e, "First"))
                .ok_or(missing("a marker without an ID"))?
                .trim()
                .to_owned();
            let key = if on_item {
                self.b.book.key(Some(owner), "mk", &id, true)
            } else {
                self.b.book.key(None, "mk", &id, true)
            };
            let kind = match d.get("mType").and_then(Json::as_str) {
                Some("Comment") | None => MarkerKind::Marker,
                Some("Chapter") => MarkerKind::Chapter,
                Some(_) => MarkerKind::Other,
            };
            let mut m = Marker::new(
                key,
                kind,
                d.get("mName").and_then(Json::as_str).unwrap_or(""),
                place(start, at, origin)?,
            );
            m.duration = duration;
            m.note = d
                .get("mComment")
                .and_then(Json::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            if on_item {
                m.media_at = Some(at);
            }
            out.push(m);
        }
        Ok(out)
    }
}

/// `o` must be one of `classes`, at the class ID and version [`CLASSES`] gives.
fn check_class(o: Node<'_, '_>, classes: &[&str]) -> Result<(), ReadError> {
    let tag = o.tag_name().name();
    if !classes.contains(&tag) {
        return Err(ReadError::Unknown {
            what: "PRPROJ class",
            found: quote(tag),
        });
    }
    let (_, id, ver) =
        CLASSES
            .iter()
            .find(|(t, _, _)| *t == tag)
            .ok_or_else(|| ReadError::Unknown {
                what: "PRPROJ class",
                found: quote(tag),
            })?;
    if o.plain("ClassID") != Some(*id) {
        return Err(ReadError::Unknown {
            what: "PRPROJ class ID",
            found: quote(o.plain("ClassID").unwrap_or("")),
        });
    }
    let found = o.plain("Version").unwrap_or("");
    if found != *ver {
        return Err(ReadError::UnknownVersion {
            what: format!("PRPROJ class {}", quote(tag)),
            found: quote(found),
        });
    }
    Ok(())
}
