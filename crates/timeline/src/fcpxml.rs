//! Reads FCPXML (`.fcpxml`, or the `Info.fcpxml` inside a `.fcpxmld` bundle) into the model,
//! read-only. Our own types, written from the format's public reference; the document is
//! validated against our own schema ([`SCHEMA`]), never against a DTD.
//!
//! **Versions.** `1.8` to `1.13`; any other is [`ReadError::UnknownVersion`]. A document type
//! with its own declarations (an internal subset) is refused before parsing.
//!
//! **Schema.** Every element the reader interprets has a rule: the attributes it needs, and
//! the children it may hold. A child outside its parent's rule is [`ReadError::Unknown`]; a
//! needed attribute missing is [`ReadError::Structure`]. Elements the model doesn't interpret
//! (effect parameters, adjustments, metadata, notes, keyword collections) are allowed where the
//! rule says and carried opaque: the digest of their bytes as written.
//!
//! **Timelines.** Each `project`'s sequence, then each compound clip (`media` holding a
//! `sequence`) as a compound timeline. A sequence's rate comes from its `format`'s
//! `frameDuration`; `tcStart` is its first frame's timecode, `tcFormat="DF"` drop-frame at a
//! rate with a drop-frame form (`30000/1001`, `60000/1001`); at any other rate (25 fps) it is
//! non-drop ([`crate::model::drop_frame_at`]).
//!
//! **Lanes are tracks.** The primary storyline is lane 0; a connected clip's lane is its
//! parent's plus its own `lane`; a secondary storyline (`spine` with a `lane`) puts its items
//! on its lane. One track per lane (`d-lane:<n>`), the lowest lane first, so a later track
//! composites over an earlier one, as in OTIO; a lane at or above the primary storyline is
//! video, below it audio.
//!
//! **Placement.** An element's record start is its parent's record start plus its `offset`
//! less its parent's `start` (a sequence's start is its `tcStart`; a secondary storyline's is
//! zero). Markers are placed the same way and keep their `start` as media time. A
//! transition's cut is the next element's offset in its storyline.
//!
//! **What an item is.** `asset-clip`, `video`, `audio`, `clip`, `sync-clip`, `mc-clip`: a clip
//! (a `clip`'s media is the first media its content uses); `ref-clip`: a nest of the compound
//! timeline it names; `gap`; `title` (its text); `caption` (its text); `transition`;
//! `audition`: its first (active) pick, the others carried; `live-drawing`: other. A clip's
//! content (lane-less inner story elements) is carried.
//!
//! **Keys.** Projects, compound clips and assets are keyed by their `uid` when they have one;
//! everything else is derived ([`crate::model`]'s table). Story elements have no IDs.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::collections::HashMap;

use roxmltree::Node;

use crate::build::{place, Builder};
use crate::error::{quote, ReadError, ReadLimits};
use crate::model::{
    key_token, time_token, Document, Format, Item, ItemKind, Key, Marker, MarkerKind, MediaKind,
    Opaque, Timeline, Track, TrackKind, TransitionInfo,
};
use crate::time::{Rate, Time};
use crate::xml::PlainAttr;

/// The FCPXML versions this reader knows.
pub const VERSIONS: &[&str] = &["1.8", "1.9", "1.10", "1.11", "1.12", "1.13"];

/// The story elements: what a storyline holds.
const STORY: &[&str] = &[
    "asset-clip",
    "clip",
    "ref-clip",
    "sync-clip",
    "mc-clip",
    "audition",
    "gap",
    "title",
    "audio",
    "video",
    "caption",
    "transition",
    "live-drawing",
];

/// The marker family: anchored to a story element, in its media time.
const MARKERS: &[&str] = &[
    "marker",
    "chapter-marker",
    "keyword",
    "rating",
    "analysis-marker",
];

/// What a story element may hold besides story elements, storylines and markers: carried.
/// Names starting `adjust-` or `filter-` are carried too.
const CARRIED: &[&str] = &[
    "note",
    "metadata",
    "conform-rate",
    "timeMap",
    "text",
    "text-style-def",
    "param",
    "audio-channel-source",
    "audio-role-source",
    "mc-source",
    "hidden-clip-marker",
    "object-tracker",
];

/// One element's rule: the attributes it needs and the children it may hold.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// The element.
    pub name: &'static str,
    /// Attributes it must have.
    pub required: &'static [&'static str],
    /// Children it may hold, besides those `story` allows.
    pub children: &'static [&'static str],
    /// Whether it may hold story elements, storylines, markers and carried elements (the
    /// story element rule).
    pub story: bool,
}

const fn rule(
    name: &'static str,
    required: &'static [&'static str],
    children: &'static [&'static str],
    story: bool,
) -> Rule {
    Rule {
        name,
        required,
        children,
        story,
    }
}

/// Our schema: every element the reader interprets.
pub const SCHEMA: &[Rule] = &[
    rule(
        "fcpxml",
        &["version"],
        &["import-options", "resources", "library", "event", "project"],
        false,
    ),
    rule(
        "resources",
        &[],
        &[
            "format",
            "asset",
            "media",
            "effect",
            "locator",
            "object-tracker",
        ],
        false,
    ),
    rule("format", &["id"], &[], false),
    rule(
        "asset",
        &["id"],
        &["media-rep", "metadata", "bookmark"],
        false,
    ),
    rule(
        "media",
        &["id"],
        &["sequence", "multicam", "metadata"],
        false,
    ),
    rule("effect", &["id"], &[], false),
    rule(
        "library",
        &[],
        &["event", "smart-collection", "metadata"],
        false,
    ),
    rule(
        "event",
        &[],
        &[
            "project",
            "keyword-collection",
            "collection-folder",
            "smart-collection",
            "metadata",
        ],
        false,
    ),
    rule("project", &[], &["sequence"], false),
    rule("sequence", &[], &["note", "spine", "metadata"], false),
    rule("spine", &[], &[], false),
    rule("asset-clip", &["ref", "duration"], &[], true),
    rule("clip", &["duration"], &[], true),
    rule("ref-clip", &["ref", "duration"], &[], true),
    rule("sync-clip", &["duration"], &[], true),
    rule("mc-clip", &["ref", "duration"], &[], true),
    rule("audition", &[], &[], true),
    rule("gap", &["duration"], &[], true),
    rule("title", &["ref", "duration"], &[], true),
    rule("audio", &["ref", "duration"], &[], true),
    rule("video", &["ref", "duration"], &[], true),
    rule("caption", &["duration"], &[], true),
    rule("transition", &["duration"], &[], true),
    rule("live-drawing", &["duration"], &[], true),
    rule("marker", &["start", "value"], &[], false),
    rule("chapter-marker", &["start", "value"], &[], false),
    rule("keyword", &["value"], &[], false),
    rule("rating", &["value"], &[], false),
    rule("analysis-marker", &[], &[], false),
];

/// Checks `node` against its rule: its needed attributes, and each element child allowed. A
/// spine holds story elements only; the root and an event may also hold story elements
/// (browser clips).
fn validate(node: Node<'_, '_>) -> Result<(), ReadError> {
    let name = node.tag_name().name();
    let rule = SCHEMA
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| ReadError::Unknown {
            what: "FCPXML element",
            found: quote(name),
        })?;
    for a in rule.required {
        if node.plain(a).is_none() {
            return Err(ReadError::Structure {
                why: "an element without an attribute it needs",
            });
        }
    }
    for c in node.children().filter(Node::is_element) {
        let cn = c.tag_name().name();
        let ok = rule.children.contains(&cn)
            || (matches!(name, "spine" | "fcpxml" | "event") && STORY.contains(&cn))
            || (rule.story
                && (STORY.contains(&cn)
                    || cn == "spine"
                    || MARKERS.contains(&cn)
                    || CARRIED.contains(&cn)
                    || cn.starts_with("adjust-")
                    || cn.starts_with("filter-")));
        if !ok {
            return Err(ReadError::Unknown {
                what: "FCPXML element",
                found: quote(cn),
            });
        }
    }
    Ok(())
}

/// Reads an FCPXML document.
pub fn read_fcpxml(bytes: &[u8], limits: &ReadLimits) -> Result<Document, ReadError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ReadError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    let text = crate::xml::text(bytes)?;
    let xml = crate::xml::parse(text, limits)?;
    let root = xml.root_element();
    if root.tag_name().name() != "fcpxml" {
        return Err(ReadError::Unknown {
            what: "root element",
            found: quote(root.tag_name().name()),
        });
    }
    let version = root.plain("version").unwrap_or("");
    if !VERSIONS.contains(&version) {
        return Err(ReadError::UnknownVersion {
            what: "FCPXML".into(),
            found: quote(version),
        });
    }
    validate(root)?;
    let mut r = Reader {
        b: Builder::new(limits),
        text,
        resources: HashMap::new(),
        compounds: HashMap::new(),
    };
    let mut opaque = Vec::new();
    // resources first: what the story refers to
    let mut compound_nodes = Vec::new();
    for res in root.children().filter(|c| c.has_tag_name("resources")) {
        validate(res)?;
        for c in res.children().filter(Node::is_element) {
            let id = c.plain("id").unwrap_or("");
            match c.tag_name().name() {
                "locator" | "object-tracker" => opaque.push(r.carry("fcpxml:resource", c)),
                _ => {
                    validate(c)?;
                    if r.resources.insert(id.to_owned(), c).is_some() {
                        return Err(ReadError::Structure {
                            why: "two resources share an id",
                        });
                    }
                }
            }
            if c.has_tag_name("media") {
                if let Some(seq) = c.children().find(|s| s.has_tag_name("sequence")) {
                    let n = compound_nodes.len() as u64 + 1;
                    let key = match c.plain("uid") {
                        Some(uid) => r.b.book.key(None, "compound", uid, true),
                        None => r.b.book.key(None, "d-compound", &n.to_string(), false),
                    };
                    r.compounds.insert(id.to_owned(), key.clone());
                    compound_nodes.push((c, seq, key));
                }
            }
        }
    }
    let mut projects = Vec::new();
    collect_projects(root, &mut projects, &mut opaque, &r, 0)?;
    let mut timelines = Vec::new();
    for (n, p) in projects.iter().enumerate() {
        r.b.count()?;
        let key = match p.plain("uid") {
            Some(uid) => r.b.book.key(None, "project", uid, true),
            None => r.b.book.key(None, "d-seq", &(n + 1).to_string(), false),
        };
        validate(*p)?;
        let seq =
            p.children()
                .find(|s| s.has_tag_name("sequence"))
                .ok_or(ReadError::Structure {
                    why: "a project without a sequence",
                })?;
        timelines.push(r.sequence(seq, key, p.plain("name").unwrap_or(""), false)?);
    }
    for (m, seq, key) in compound_nodes {
        r.b.count()?;
        timelines.push(r.sequence(seq, key, m.plain("name").unwrap_or(""), true)?);
    }
    Ok(Document {
        format: Format::Fcpxml,
        version: version.to_owned(),
        timelines,
        media: r.b.media,
        opaque,
    })
}

/// The projects under the root, a library and its events, in document order; what else they
/// hold is carried.
fn collect_projects<'a, 'i>(
    node: Node<'a, 'i>,
    out: &mut Vec<Node<'a, 'i>>,
    opaque: &mut Vec<Opaque>,
    r: &Reader<'a, 'i>,
    depth: u32,
) -> Result<(), ReadError> {
    r.b.depth(depth)?;
    for c in node.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "resources" => {}
            "project" => out.push(c),
            "library" | "event" => {
                validate(c)?;
                collect_projects(c, out, opaque, r, depth + 1)?;
            }
            name => opaque.push(r.carry(&format!("fcpxml:{name}"), c)),
        }
    }
    Ok(())
}

/// A story element's attribute as an exact time.
fn time_attr(node: Node<'_, '_>, name: &str) -> Result<Option<Time>, ReadError> {
    match node.plain(name) {
        None => Ok(None),
        Some(v) => Ok(Some(Time::parse_seconds(v)?)),
    }
}

fn lane_of(node: Node<'_, '_>) -> Result<Option<i32>, ReadError> {
    match node.plain("lane") {
        None => Ok(None),
        Some(v) => v
            .parse::<i32>()
            .map(Some)
            .map_err(|_| ReadError::Structure {
                why: "a lane that is not a whole number",
            }),
    }
}

/// `48k` as 48000, `44.1k` as 44100.
fn audio_rate(v: &str) -> Option<u32> {
    let k = v.strip_suffix('k')?;
    let (w, f) = k.split_once('.').unwrap_or((k, ""));
    if f.len() > 3 {
        return None;
    }
    let w: u32 = w.parse().ok()?;
    let f: u32 = if f.is_empty() {
        0
    } else {
        format!("{f:0<3}").parse().ok()?
    };
    w.checked_mul(1000)?.checked_add(f)
}

struct Reader<'a, 'i> {
    b: Builder,
    text: &'a str,
    resources: HashMap<String, Node<'a, 'i>>,
    compounds: HashMap<String, Key>,
}

/// A lane's track, being filled.
struct Lane {
    track: Track,
}

impl<'a, 'i> Reader<'a, 'i> {
    fn carry(&self, what: &str, node: Node<'a, 'i>) -> Opaque {
        Opaque::of(what, self.text[node.range()].as_bytes())
    }

    fn sequence(
        &mut self,
        seq: Node<'a, 'i>,
        key: Key,
        name: &str,
        compound: bool,
    ) -> Result<Timeline, ReadError> {
        validate(seq)?;
        let mut tl = Timeline::new(key, name);
        tl.compound = compound;
        if let Some(f) = seq.plain("format") {
            let fmt = self.resources.get(f).copied().ok_or(ReadError::Structure {
                why: "a sequence's format names no resource",
            })?;
            if let Some(fd) = time_attr(fmt, "frameDuration")? {
                tl.rate = Some(Rate::from_frame_duration(fd)?);
            }
            tl.width = fmt.plain("width").and_then(|w| w.parse().ok());
            tl.height = fmt.plain("height").and_then(|h| h.parse().ok());
        }
        tl.tc_start = time_attr(seq, "tcStart")?.unwrap_or(Time::ZERO);
        tl.drop_frame = crate::model::drop_frame_at(tl.rate, seq.plain("tcFormat") == Some("DF"));
        tl.duration = time_attr(seq, "duration")?;
        tl.audio_rate = seq.plain("audioRate").and_then(audio_rate);
        for c in seq.children().filter(Node::is_element) {
            if matches!(c.tag_name().name(), "note" | "metadata") {
                tl.opaque
                    .push(self.carry(&format!("fcpxml:{}", c.tag_name().name()), c));
            }
        }
        let spine =
            seq.children()
                .find(|s| s.has_tag_name("spine"))
                .ok_or(ReadError::Structure {
                    why: "a sequence without a spine",
                })?;
        let mut lanes: BTreeMap<i32, Lane> = BTreeMap::new();
        let scope = tl.key.clone();
        self.spine(spine, Time::ZERO, tl.tc_start, 0, &scope, &mut lanes, 1)?;
        // the lowest lane first: a later track composites over an earlier one
        for (_, mut lane) in lanes {
            lane.track.items.sort_by_key(|a| a.start);
            tl.tracks.push(lane.track);
        }
        Ok(tl)
    }

    fn lane<'l>(
        &mut self,
        lanes: &'l mut BTreeMap<i32, Lane>,
        scope: &Key,
        lane: i32,
    ) -> Result<&'l mut Lane, ReadError> {
        if let Entry::Vacant(slot) = lanes.entry(lane) {
            self.b.count()?;
            let key = self
                .b
                .book
                .key(Some(scope), "d-lane", &lane.to_string(), false);
            let kind = if lane >= 0 {
                TrackKind::Video
            } else {
                TrackKind::Audio
            };
            slot.insert(Lane {
                track: Track::new(key, format!("lane {lane}"), kind),
            });
        }
        Ok(lanes.get_mut(&lane).expect("inserted"))
    }

    /// A storyline: its story elements, placed from `record` (the record time of the
    /// storyline's local `zero`), on `lane`.
    #[allow(clippy::too_many_arguments)]
    fn spine(
        &mut self,
        spine: Node<'a, 'i>,
        record: Time,
        zero: Time,
        lane: i32,
        scope: &Key,
        lanes: &mut BTreeMap<i32, Lane>,
        depth: u32,
    ) -> Result<(), ReadError> {
        self.b.depth(depth)?;
        validate(spine)?;
        let kids: Vec<Node<'a, 'i>> = spine.children().filter(Node::is_element).collect();
        for (n, c) in kids.iter().enumerate() {
            // a transition's cut: the next non-transition element's offset
            let cut = if c.has_tag_name("transition") {
                match kids[n + 1..].iter().find(|k| !k.has_tag_name("transition")) {
                    Some(next) => Some(time_attr(*next, "offset")?.unwrap_or(Time::ZERO)),
                    None => None,
                }
            } else {
                None
            };
            self.story(*c, record, zero, lane, scope, lanes, depth + 1, cut)?;
        }
        Ok(())
    }

    /// One story element at `lane`, placed from its parent's `record` and `parent_start`.
    #[allow(clippy::too_many_arguments)]
    fn story(
        &mut self,
        node: Node<'a, 'i>,
        record: Time,
        parent_start: Time,
        lane: i32,
        scope: &Key,
        lanes: &mut BTreeMap<i32, Lane>,
        depth: u32,
        cut: Option<Time>,
    ) -> Result<(), ReadError> {
        self.b.depth(depth)?;
        self.b.count()?;
        validate(node)?;
        let name = node.tag_name().name();
        let offset = time_attr(node, "offset")?.unwrap_or(Time::ZERO);
        let at = place(record, offset, parent_start)?;
        if name == "audition" {
            // the first pick is the active one: it stands in the audition's place
            let picks: Vec<Node<'a, 'i>> = node
                .children()
                .filter(|c| STORY.contains(&c.tag_name().name()))
                .collect();
            let active = *picks.first().ok_or(ReadError::Structure {
                why: "an audition without a pick",
            })?;
            let before = lanes.get(&lane).map_or(0, |l| l.track.items.len());
            let pick_offset = time_attr(active, "offset")?.unwrap_or(Time::ZERO);
            self.story(active, at, pick_offset, lane, scope, lanes, depth + 1, None)?;
            let rest: Vec<Opaque> = picks[1..]
                .iter()
                .map(|p| self.carry("fcpxml:audition-pick", *p))
                .collect();
            if let Some(it) = self.lane(lanes, scope, lane)?.track.items.get_mut(before) {
                it.opaque.extend(rest);
            }
            return Ok(());
        }
        let start = time_attr(node, "start")?.unwrap_or(Time::ZERO);
        let duration = time_attr(node, "duration")?.ok_or(ReadError::Structure {
            why: "a story element without a duration",
        })?;
        if duration.is_negative() {
            return Err(ReadError::Structure {
                why: "a negative duration",
            });
        }
        let kind = match name {
            "gap" => ItemKind::Gap,
            "title" => ItemKind::Title,
            "caption" => ItemKind::Caption,
            "transition" => ItemKind::Transition,
            "ref-clip" => ItemKind::Nest,
            "live-drawing" => ItemKind::Other,
            _ => ItemKind::Clip,
        };
        if at.is_negative() && kind != ItemKind::Transition {
            return Err(ReadError::Structure {
                why: "an element placed before its timeline starts",
            });
        }
        let media = self.media_of(node)?;
        let label = node.plain("name").unwrap_or("");
        let what = match &media {
            Some(k) => self
                .b
                .media
                .iter()
                .find(|m| &m.key == k)
                .map(|m| m.name.clone())
                .unwrap_or_default(),
            None => String::new(),
        };
        let what = if what.is_empty() {
            label
        } else {
            what.as_str()
        };
        let track_key = self.lane(lanes, scope, lane)?.track.key.clone();
        let key = if kind == ItemKind::Transition {
            self.b.book.key(
                Some(&track_key),
                "d-tr",
                &format!("@{}", time_token(at)),
                false,
            )
        } else {
            let k = format!("d-{}", kind.name());
            self.b.book.key(
                Some(&track_key),
                &k,
                &format!("{}@{}", key_token(what), time_token(start)),
                false,
            )
        };
        let mut it = Item::new(key, kind, label, at, duration);
        if kind != ItemKind::Transition {
            it.source_in = Some(start);
        }
        it.media = media;
        it.enabled = node.plain("enabled") != Some("0");
        if kind == ItemKind::Transition {
            let in_offset = cut
                .map(|c| c.checked_sub(offset))
                .transpose()?
                .filter(|o| !o.is_negative() && *o <= duration);
            it.transition = Some(TransitionInfo::new(label, in_offset));
        }
        if kind == ItemKind::Title {
            if let Some(fx) = node
                .plain("ref")
                .and_then(|r| self.resources.get(r))
                .and_then(|e| e.plain("name"))
            {
                it.effects.push(fx.to_owned());
            }
        }
        if matches!(kind, ItemKind::Title | ItemKind::Caption) {
            let mut text = String::new();
            for t in node.children().filter(|c| c.has_tag_name("text")) {
                for s in t.descendants().filter(Node::is_text) {
                    text.push_str(s.text().unwrap_or(""));
                }
            }
            if !text.is_empty() {
                it.text = Some(text);
            }
        }
        // children: markers, connected items, storylines, content and carried pieces
        let mut connected = Vec::new();
        for c in node.children().filter(Node::is_element) {
            let cn = c.tag_name().name();
            if MARKERS.contains(&cn) {
                it.markers.push(self.marker(c, &it.key, at, start)?);
            } else if (STORY.contains(&cn) || cn == "spine") && lane_of(c)?.is_some() {
                connected.push(c);
            } else if STORY.contains(&cn) || cn == "spine" {
                it.opaque
                    .push(self.carry(&format!("fcpxml:content:{cn}"), c));
            } else {
                if cn.starts_with("filter-") {
                    let fx = c
                        .plain("name")
                        .or_else(|| {
                            c.plain("ref")
                                .and_then(|r| self.resources.get(r))
                                .and_then(|e| e.plain("name"))
                        })
                        .unwrap_or(cn);
                    it.effects.push(fx.to_owned());
                }
                it.opaque.push(self.carry(&format!("fcpxml:{cn}"), c));
            }
        }
        self.lane(lanes, scope, lane)?.track.items.push(it);
        for c in connected {
            let child_lane =
                lane.checked_add(lane_of(c)?.unwrap_or(0))
                    .ok_or(ReadError::Structure {
                        why: "a lane out of range",
                    })?;
            if c.has_tag_name("spine") {
                let off = time_attr(c, "offset")?.unwrap_or(Time::ZERO);
                let srec = place(at, off, start)?;
                self.spine(c, srec, Time::ZERO, child_lane, scope, lanes, depth + 1)?;
            } else {
                self.story(c, at, start, child_lane, scope, lanes, depth + 1, None)?;
            }
        }
        Ok(())
    }

    fn marker(
        &mut self,
        c: Node<'a, 'i>,
        owner: &Key,
        record: Time,
        start: Time,
    ) -> Result<Marker, ReadError> {
        self.b.count()?;
        validate(c)?;
        let s = time_attr(c, "start")?.unwrap_or(start);
        let kind = match c.tag_name().name() {
            "marker" => match c.plain("completed") {
                Some("0") => MarkerKind::Todo,
                Some(_) => MarkerKind::Done,
                None => MarkerKind::Marker,
            },
            "chapter-marker" => MarkerKind::Chapter,
            "keyword" => MarkerKind::Keyword,
            "rating" => MarkerKind::Rating,
            _ => MarkerKind::Other,
        };
        let name = c.plain("value").unwrap_or("");
        let key = self.b.book.key(
            Some(owner),
            "d-mk",
            &format!("{}@{}", key_token(name), time_token(s)),
            false,
        );
        let mut m = Marker::new(key, kind, name, place(record, s, start)?);
        m.duration = time_attr(c, "duration")?.unwrap_or(Time::ZERO);
        if m.duration.is_negative() {
            return Err(ReadError::Structure {
                why: "a negative duration",
            });
        }
        m.media_at = Some(s);
        m.note = c.plain("note").map(str::to_owned);
        Ok(m)
    }

    /// The media an element uses: its `ref`'s asset or compound clip, or for a clip-like
    /// element without a `ref`, the first its content uses.
    fn media_of(&mut self, node: Node<'a, 'i>) -> Result<Option<Key>, ReadError> {
        let name = node.tag_name().name();
        if matches!(
            name,
            "gap" | "transition" | "title" | "caption" | "live-drawing"
        ) {
            return Ok(None);
        }
        if let Some(r) = node.plain("ref") {
            return self.resource_media(r).map(Some);
        }
        // a clip's content: lane-less story elements, depth first (a lane-less gap's
        // connected audio is the content of a detached clip)
        let mut stack: Vec<Node<'a, 'i>> = node
            .children()
            .filter(|c| STORY.contains(&c.tag_name().name()) && c.plain("lane").is_none())
            .collect();
        stack.reverse();
        let mut seen = 0u32;
        while let Some(c) = stack.pop() {
            seen += 1;
            if seen > 10_000 {
                break;
            }
            if let Some(r) = c.plain("ref") {
                if !c.has_tag_name("title") {
                    return self.resource_media(r).map(Some);
                }
            }
            let mut kids: Vec<Node<'a, 'i>> = c
                .children()
                .filter(|k| STORY.contains(&k.tag_name().name()))
                .collect();
            kids.reverse();
            stack.extend(kids);
        }
        Ok(None)
    }

    /// The media entry for the resource `id`: an asset, a compound clip, or a multicam.
    fn resource_media(&mut self, id: &str) -> Result<Key, ReadError> {
        let res = self
            .resources
            .get(id)
            .copied()
            .ok_or(ReadError::Structure {
                why: "a reference to no resource",
            })?;
        let name = res.plain("name").unwrap_or("").to_owned();
        let uid = res.plain("uid");
        match res.tag_name().name() {
            "asset" => {
                let src = res.plain("src").map(str::to_owned).or_else(|| {
                    res.children()
                        .filter(|c| c.has_tag_name("media-rep"))
                        .find(|c| c.plain("kind").is_none_or(|k| k == "original-media"))
                        .and_then(|c| c.plain("src"))
                        .map(str::to_owned)
                });
                let start = time_attr(res, "start")?;
                let duration = time_attr(res, "duration")?;
                let rate = match res.plain("format").and_then(|f| self.resources.get(f)) {
                    Some(f) => match time_attr(*f, "frameDuration")? {
                        Some(fd) => Some(Rate::from_frame_duration(fd)?),
                        None => None,
                    },
                    None => None,
                };
                let carried: Vec<Opaque> = res
                    .children()
                    .filter(Node::is_element)
                    .map(|c| self.carry(&format!("fcpxml:{}", c.tag_name().name()), c))
                    .collect();
                self.b
                    .media(format!("asset\0{id}"), uid, &name, MediaKind::File, |m| {
                        m.path = src;
                        m.start = start;
                        m.duration = duration;
                        m.rate = rate;
                        m.opaque = carried;
                    })
            }
            "media" => {
                let seq = self.compounds.get(id).cloned();
                let kind = if seq.is_some() {
                    MediaKind::Sequence
                } else {
                    MediaKind::Other
                };
                let carried = seq.is_none().then(|| self.carry("fcpxml:media", res));
                self.b.media(format!("media\0{id}"), uid, &name, kind, |m| {
                    m.sequence = seq;
                    m.opaque.extend(carried);
                })
            }
            _ => Err(ReadError::Structure {
                why: "a reference to a resource that is not media",
            }),
        }
    }
}
