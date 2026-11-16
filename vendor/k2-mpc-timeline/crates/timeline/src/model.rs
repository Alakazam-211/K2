//! The neutral timeline model: what every translator reads a file into.
//!
//! **Shape.** A [`Document`] holds [`Timeline`]s and the [`Media`] they use. A timeline holds
//! [`Track`]s (in stacking order: a later track composites over an earlier one, OTIO's meaning,
//! which every reader normalises to) and its own [`Marker`]s; a track holds [`Item`]s sorted by
//! start; an item may hold markers, and a nest holds its own tracks.
//!
//! **Time is absolute.** An item's `start` is its record time, from the timeline's first frame
//! (`0`), not implied by what precedes it, so a node's position is its own field. Each
//! format's sequential form (gaps between items, transitions at cuts) converts both ways
//! ([`Track::to_steps`], [`place_steps`]). A nest's tracks are in the nest's own time.
//! `tc_start` is the timecode of the first frame, kept beside.
//!
//! **What is live and what is carried.** The fields of each type are the facets the model
//! understands. Everything else the file holds for a node (effect parameters, vendor
//! metadata, unknown sub-elements) is carried opaque in `opaque`: a digest and a size per
//! piece ([`Opaque`]), never interpreted. [`Item::fingerprint`] and its siblings digest the
//! facets; [`Item::payload`] digests the opaque pieces, so a change to either shows.
//!
//! **Keys.** Every node has a [`Key`] in the kit's address grammar. A format's own unique ID
//! is a native key: `@seq:<guid>`, `@project:<uid>`, `@compound:<uid>` for timelines,
//! `@track:<guid>`, `@clip:<guid>`, `@mk:<guid>` (scoped under its clip when it is a clip's),
//! `@media:<id>`. Where a format has none the translator derives one from what the node is,
//! never from a hash ([`KeyOrigin::Derived`]):
//!
//! | Node | Derived key |
//! |---|---|
//! | timeline | `@d-seq:<n>`: its place among the file's timelines, from 1 |
//! | track | `<timeline>/d-trk:<V\|A\|C\|O><n>`: kind letter, place among tracks of that kind |
//! | lane (FCPXML) | `<timeline>/d-lane:<lane>` |
//! | item | `<track>/d-<kind>:<what>@<source in>`: the media's name (else the item's), and the media time of its first frame |
//! | transition | `<track>/d-tr:@<start>`: its record start |
//! | marker | `<owner>/d-mk:<name>@<time>`: media time on an item, else record time |
//! | media | `@d-media:<name>` |
//!
//! Names in a derived key have spaces and `/` replaced by `_` and are cut to 48 bytes; times
//! are exact (`1001_24000s` for 1001/24000 s). A derived key survives a move in record time
//! and a rename of the item (not of the media); it changes on a slip or an in-point trim. A
//! derived key the document already holds gets `~2`, `~3`… and [`KeyOrigin::Repeat`]; a key
//! over the kit's length limit is [`KeyOrigin::Long`]. The index leaves those two to the kit,
//! which mints and carries its own keys for them.

use std::collections::{HashMap, HashSet};

use k2_mpc_base::index::{Digest, DigestStream, INDEX_KEY_MAX};

use crate::time::{Rate, Time, TimeError};

/// The file format a document was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// OTIO JSON.
    Otio,
    /// FCPXML.
    Fcpxml,
    /// PRPROJ (gzip XML).
    Prproj,
}

impl Format {
    /// The format's lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            Format::Otio => "otio",
            Format::Fcpxml => "fcpxml",
            Format::Prproj => "prproj",
        }
    }
}

/// Where a key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyOrigin {
    /// The file's own ID for this node.
    Native,
    /// Derived from what the node is (the module's table).
    Derived,
    /// Derived, but the document already held that key: suffixed `~N` to stay unique.
    Repeat,
    /// Longer than the kit's key limit: kept for the model, left to the kit to key.
    Long,
}

/// A node's key, in the kit's address grammar (`@uid:…`, `@d-seq:1/d-trk:V1`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    text: String,
    origin: KeyOrigin,
}

impl Key {
    /// The key's text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Where it comes from.
    pub fn origin(&self) -> KeyOrigin {
        self.origin
    }

    /// A key with this text and origin, unchecked: for tests and tools.
    pub fn from_parts(text: impl Into<String>, origin: KeyOrigin) -> Key {
        Key {
            text: text.into(),
            origin,
        }
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Longest name piece in a derived key, in bytes.
pub const KEY_NAME_MAX: usize = 48;

/// A name as a derived key's ID piece: spaces, controls and `/` become `_`, cut to 48
/// bytes at a character boundary; empty becomes `_`.
pub fn key_token(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c.is_whitespace() || c.is_control() || c == '/' {
            '_'
        } else {
            c
        };
        if out.len() + c.len_utf8() > KEY_NAME_MAX {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// A time as a derived key's ID piece: `5s`, `1001_24000s`.
pub fn time_token(t: Time) -> String {
    t.to_string().replace('/', "_")
}

/// Whether `kind` is a word the address grammar takes as a key segment's kind.
fn kind_ok(kind: &str) -> bool {
    !kind.is_empty()
        && kind != "k"
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Whether `id` is a key segment's ID: not empty, no whitespace, no `/`.
fn id_ok(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c == '/')
}

/// The keys a document holds, so every key it hands out is unique: what a translator, or a
/// verb adding a node, makes keys with.
#[derive(Debug, Default)]
pub struct KeyBook {
    taken: HashSet<String>,
    next: HashMap<String, u64>,
}

impl KeyBook {
    /// A key `kind:id` under `scope`. A native ID the grammar can't hold (a space, a `/`) is
    /// taken as a derived key with the ID made a token ([`key_token`]); a derived ID is taken
    /// as built (callers join tokens), made a token only if the grammar can't hold it. A
    /// repeat gets `~N`.
    ///
    /// `kind` is a lower-case word of the address grammar (letters, digits, `_`, `-`; not
    /// `k`); a bad one is taken as `x`.
    pub fn key(&mut self, scope: Option<&Key>, kind: &str, id: &str, native: bool) -> Key {
        let kind = if kind_ok(kind) { kind } else { "x" };
        let (id, origin) = match (native, id_ok(id)) {
            (true, true) => (id.to_owned(), KeyOrigin::Native),
            (false, true) => (id.to_owned(), KeyOrigin::Derived),
            (_, false) => (key_token(id), KeyOrigin::Derived),
        };
        let base = match scope {
            Some(s) => format!("{}/{kind}:{id}", s.text),
            None => format!("@{kind}:{id}"),
        };
        let (text, origin) = if self.taken.contains(&base) {
            // the next suffix for this base, remembered, so many repeats stay linear
            let n = self.next.entry(base.clone()).or_insert(2);
            loop {
                let t = format!("{base}~{n}");
                *n += 1;
                if !self.taken.contains(&t) {
                    break (t, KeyOrigin::Repeat);
                }
            }
        } else {
            (base, origin)
        };
        self.taken.insert(text.clone());
        let origin = if text.len() > INDEX_KEY_MAX {
            KeyOrigin::Long
        } else {
            origin
        };
        Key { text, origin }
    }
}

/// A piece of the file carried opaque: never interpreted, only digested and sized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opaque {
    /// What it is (`otio:metadata`, `fcpxml:filter-video`, `prproj:components`).
    pub what: String,
    /// The digest of its bytes as the translator saw them (canonical for JSON).
    pub digest: Digest,
    /// Its size in bytes.
    pub bytes: u64,
}

impl Opaque {
    /// The piece `bytes`, called `what`.
    pub fn of(what: impl Into<String>, bytes: &[u8]) -> Opaque {
        Opaque {
            what: what.into(),
            digest: Digest::of(bytes),
            bytes: bytes.len() as u64,
        }
    }
}

/// One file read into the model.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Document {
    /// The format it was read from.
    pub format: Format,
    /// The file's own version (`1.10`, `3`, or the root OTIO schema `Timeline.1`).
    pub version: String,
    /// Its timelines, in file order.
    pub timelines: Vec<Timeline>,
    /// The media its items use, in first-use order.
    pub media: Vec<Media>,
    /// What the file holds outside its timelines, carried.
    pub opaque: Vec<Opaque>,
}

/// One timeline (a sequence, a project's cut, a compound clip's contents).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Timeline {
    /// Its key.
    pub key: Key,
    /// Its name.
    pub name: String,
    /// Its frame rate, when the file says.
    pub rate: Option<Rate>,
    /// The timecode of its first frame, as a time.
    pub tc_start: Time,
    /// Whether its timecode is drop-frame: only at a rate with a drop-frame form
    /// ([`drop_frame_at`]); a file that says drop-frame at any other rate is non-drop.
    pub drop_frame: bool,
    /// Frame width, when the file says.
    pub width: Option<u32>,
    /// Frame height, when the file says.
    pub height: Option<u32>,
    /// Audio sample rate, when the file says.
    pub audio_rate: Option<u32>,
    /// Its duration as the file states it, if it does.
    pub duration: Option<Time>,
    /// A compound clip's or nested sequence's contents rather than a top-level cut.
    pub compound: bool,
    /// Its tracks, in stacking order: a later track composites over an earlier one.
    pub tracks: Vec<Track>,
    /// Markers on the timeline itself.
    pub markers: Vec<Marker>,
    /// Carried pieces.
    pub opaque: Vec<Opaque>,
}

/// What a track carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TrackKind {
    /// Pictures.
    Video,
    /// Sound.
    Audio,
    /// Captions or subtitles.
    Caption,
    /// Anything else the file names.
    Other,
}

impl TrackKind {
    /// The kind's lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
            TrackKind::Caption => "caption",
            TrackKind::Other => "other",
        }
    }

    /// The letter in a derived track key and a default track name (`V`, `A`, `C`, `O`).
    pub const fn letter(self) -> char {
        match self {
            TrackKind::Video => 'V',
            TrackKind::Audio => 'A',
            TrackKind::Caption => 'C',
            TrackKind::Other => 'O',
        }
    }
}

/// One track (or lane).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Track {
    /// Its key.
    pub key: Key,
    /// Its name (the file's, else the editor convention: `V1`, `A2`, `lane -1`).
    pub name: String,
    /// What it carries.
    pub kind: TrackKind,
    /// Muted or disabled.
    pub muted: bool,
    /// Locked.
    pub locked: bool,
    /// Its items, sorted by start (file order among equal starts).
    pub items: Vec<Item>,
    /// Markers on the track itself.
    pub markers: Vec<Marker>,
    /// Carried pieces.
    pub opaque: Vec<Opaque>,
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ItemKind {
    /// A piece of media.
    Clip,
    /// Explicit empty time.
    Gap,
    /// A transition across a cut.
    Transition,
    /// A title.
    Title,
    /// A caption.
    Caption,
    /// A generated picture or sound.
    Generator,
    /// A nested timeline (a stack, a compound clip, a nested sequence).
    Nest,
    /// Something the file holds that the model doesn't name: carried.
    Other,
}

impl ItemKind {
    /// The kind's lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            ItemKind::Clip => "clip",
            ItemKind::Gap => "gap",
            ItemKind::Transition => "transition",
            ItemKind::Title => "title",
            ItemKind::Caption => "caption",
            ItemKind::Generator => "generator",
            ItemKind::Nest => "nest",
            ItemKind::Other => "other",
        }
    }
}

/// How a transition sits on its cut.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TransitionInfo {
    /// The transition's kind as the file names it.
    pub kind: String,
    /// From the transition's start to the cut, when the file says where the cut is.
    pub in_offset: Option<Time>,
}

/// One item on a track.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Item {
    /// Its key.
    pub key: Key,
    /// What it is.
    pub kind: ItemKind,
    /// Its name.
    pub name: String,
    /// Record start, from the timeline's (or nest's) first frame.
    pub start: Time,
    /// Record duration.
    pub duration: Time,
    /// The media time at its first frame, when it has media.
    pub source_in: Option<Time>,
    /// The key of its [`Media`] in the document.
    pub media: Option<Key>,
    /// Enabled (a disabled item is kept and marked).
    pub enabled: bool,
    /// A title's or caption's plain text.
    pub text: Option<String>,
    /// A transition's placement.
    pub transition: Option<TransitionInfo>,
    /// The names of its effects, in order (their parameters are carried opaque).
    pub effects: Vec<String>,
    /// Markers on the item.
    pub markers: Vec<Marker>,
    /// A nest's own tracks, in the nest's own time.
    pub tracks: Vec<Track>,
    /// Carried pieces.
    pub opaque: Vec<Opaque>,
}

impl Document {
    /// An empty document of `format`, at the file's `version`.
    pub fn new(format: Format, version: impl Into<String>) -> Self {
        Document {
            format,
            version: version.into(),
            timelines: Vec::new(),
            media: Vec::new(),
            opaque: Vec::new(),
        }
    }
}

/// Whether a timeline at `rate` whose file says drop-frame (`said`) is drop-frame: only at
/// `30000/1001` and `60000/1001`, the rates with a drop-frame form. At any other rate, or with
/// no rate, the statement changes nothing: non-drop. Every reader sets
/// [`Timeline::drop_frame`] through this, and so do the live pack's `drop_frame` and its
/// labels, so the structure index's timecodes, its `drop_frame` and the live view agree.
pub fn drop_frame_at(rate: Option<Rate>, said: bool) -> bool {
    // planted: drop-frame at any rate the file says it
    let any = crate::mutant::active("DropFrameAtAnyRate");
    said && (any || rate.is_some_and(|r| r.drop_per_minute().is_some()))
}

impl Timeline {
    /// An empty timeline with no rate, starting at zero.
    pub fn new(key: Key, name: impl Into<String>) -> Self {
        Timeline {
            key,
            name: name.into(),
            rate: None,
            tc_start: Time::ZERO,
            drop_frame: false,
            width: None,
            height: None,
            audio_rate: None,
            duration: None,
            compound: false,
            tracks: Vec::new(),
            markers: Vec::new(),
            opaque: Vec::new(),
        }
    }
}

impl Track {
    /// An empty track, unmuted and unlocked.
    pub fn new(key: Key, name: impl Into<String>, kind: TrackKind) -> Self {
        Track {
            key,
            name: name.into(),
            kind,
            muted: false,
            locked: false,
            items: Vec::new(),
            markers: Vec::new(),
            opaque: Vec::new(),
        }
    }
}

impl Marker {
    /// A point marker at record time `at`.
    pub fn new(key: Key, kind: MarkerKind, name: impl Into<String>, at: Time) -> Self {
        Marker {
            key,
            kind,
            name: name.into(),
            note: None,
            at,
            duration: Time::ZERO,
            media_at: None,
            color: None,
        }
    }
}

impl Media {
    /// A media entry with only its kind and name.
    pub fn new(key: Key, kind: MediaKind, name: impl Into<String>) -> Self {
        Media {
            key,
            kind,
            name: name.into(),
            path: None,
            start: None,
            duration: None,
            rate: None,
            sequence: None,
            opaque: Vec::new(),
        }
    }
}

impl TransitionInfo {
    /// A transition of `kind`, its cut `in_offset` after its start when known.
    pub fn new(kind: impl Into<String>, in_offset: Option<Time>) -> Self {
        TransitionInfo {
            kind: kind.into(),
            in_offset,
        }
    }
}

impl Item {
    /// An enabled item with no media, text, markers, nest or carried pieces.
    pub fn new(
        key: Key,
        kind: ItemKind,
        name: impl Into<String>,
        start: Time,
        duration: Time,
    ) -> Self {
        Item {
            key,
            kind,
            name: name.into(),
            start,
            duration,
            source_in: None,
            media: None,
            enabled: true,
            text: None,
            transition: None,
            effects: Vec::new(),
            markers: Vec::new(),
            tracks: Vec::new(),
            opaque: Vec::new(),
        }
    }

    /// Record end: `start + duration`.
    pub fn end(&self) -> Result<Time, TimeError> {
        self.start.checked_add(self.duration)
    }
}

/// What a marker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MarkerKind {
    /// A plain marker or comment.
    Marker,
    /// A to-do, not done.
    Todo,
    /// A to-do, done.
    Done,
    /// A chapter.
    Chapter,
    /// A keyword range.
    Keyword,
    /// A rating range.
    Rating,
    /// Anything else the file names (an analysis range, a web link, a segment).
    Other,
}

impl MarkerKind {
    /// The kind's lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            MarkerKind::Marker => "marker",
            MarkerKind::Todo => "todo",
            MarkerKind::Done => "done",
            MarkerKind::Chapter => "chapter",
            MarkerKind::Keyword => "keyword",
            MarkerKind::Rating => "rating",
            MarkerKind::Other => "other",
        }
    }
}

/// One marker.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Marker {
    /// Its key.
    pub key: Key,
    /// What it is.
    pub kind: MarkerKind,
    /// Its name or value.
    pub name: String,
    /// Its comment.
    pub note: Option<String>,
    /// Record time (in the owner's timeline), so it shows where it sits now.
    pub at: Time,
    /// Its length (zero for a point).
    pub duration: Time,
    /// On an item: its media time, which trims and moves don't change.
    pub media_at: Option<Time>,
    /// Its colour, as the file names it.
    pub color: Option<String>,
}

/// What a media reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaKind {
    /// A file.
    File,
    /// Another timeline in the document.
    Sequence,
    /// A generator (colour, bars, a title template).
    Generator,
    /// A reference the file marks missing.
    Missing,
    /// Anything else.
    Other,
}

impl MediaKind {
    /// The kind's lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            MediaKind::File => "file",
            MediaKind::Sequence => "sequence",
            MediaKind::Generator => "generator",
            MediaKind::Missing => "missing",
            MediaKind::Other => "other",
        }
    }
}

/// One media reference.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Media {
    /// Its key.
    pub key: Key,
    /// What it points at.
    pub kind: MediaKind,
    /// Its name.
    pub name: String,
    /// Its location as the file wrote it (a path or URL). The model keeps it for relinking;
    /// nothing the pack outputs shows more than its last component.
    pub path: Option<String>,
    /// The media time of its first frame, when the file says.
    pub start: Option<Time>,
    /// Its duration, when the file says.
    pub duration: Option<Time>,
    /// Its rate, when the file says.
    pub rate: Option<Rate>,
    /// For [`MediaKind::Sequence`]: the timeline's key.
    pub sequence: Option<Key>,
    /// Carried pieces.
    pub opaque: Vec<Opaque>,
}

impl Media {
    /// The last component of `path` (after the last `/` or `\`), else the name: what the index
    /// shows.
    pub fn file_name(&self) -> &str {
        match &self.path {
            Some(p) => p.rsplit(['/', '\\']).next().unwrap_or(""),
            None => &self.name,
        }
    }
}

// ----- digests -----

/// A canonical encoder for facet digests: each field is tagged and length-prefixed.
struct Enc(DigestStream);

impl Enc {
    fn new(domain: &str) -> Self {
        let mut d = Digest::stream();
        d.update(b"k2-mpc-timeline/");
        d.update(domain.as_bytes());
        d.update(b"/v1\0");
        Enc(d)
    }

    fn bytes(&mut self, tag: &str, v: &[u8]) -> &mut Self {
        self.0.update(tag.as_bytes());
        self.0.update(&[0]);
        self.0.update(&(v.len() as u64).to_be_bytes());
        self.0.update(v);
        self
    }

    fn str(&mut self, tag: &str, v: &str) -> &mut Self {
        self.bytes(tag, v.as_bytes())
    }

    fn opt_str(&mut self, tag: &str, v: Option<&str>) -> &mut Self {
        match v {
            Some(s) => self.bytes(tag, &[&[1u8][..], s.as_bytes()].concat()),
            None => self.bytes(tag, &[0]),
        }
    }

    fn time(&mut self, tag: &str, t: Time) -> &mut Self {
        let mut b = t.num().to_be_bytes().to_vec();
        b.extend(t.den().to_be_bytes());
        self.bytes(tag, &b)
    }

    fn opt_time(&mut self, tag: &str, t: Option<Time>) -> &mut Self {
        match t {
            Some(t) => {
                let mut b = vec![1u8];
                b.extend(t.num().to_be_bytes());
                b.extend(t.den().to_be_bytes());
                self.bytes(tag, &b)
            }
            None => self.bytes(tag, &[0]),
        }
    }

    fn opt_rate(&mut self, tag: &str, r: Option<Rate>) -> &mut Self {
        match r {
            Some(r) => {
                let mut b = vec![1u8];
                b.extend(r.num().to_be_bytes());
                b.extend(r.den().to_be_bytes());
                self.bytes(tag, &b)
            }
            None => self.bytes(tag, &[0]),
        }
    }

    fn opt_u32(&mut self, tag: &str, v: Option<u32>) -> &mut Self {
        match v {
            Some(v) => self.bytes(tag, &[&[1u8][..], &v.to_be_bytes()].concat()),
            None => self.bytes(tag, &[0]),
        }
    }

    fn flag(&mut self, tag: &str, v: bool) -> &mut Self {
        self.bytes(tag, &[u8::from(v)])
    }

    fn finish(self) -> Digest {
        self.0.finish()
    }
}

/// The digest of `opaque`'s pieces in order, or `None` when there are none.
fn payload_of(opaque: &[Opaque]) -> Option<Digest> {
    if opaque.is_empty() {
        return None;
    }
    let mut e = Enc::new("payload");
    for o in opaque {
        e.str("what", &o.what)
            .bytes("digest", o.digest.as_bytes())
            .bytes("size", &o.bytes.to_be_bytes());
    }
    Some(e.finish())
}

impl Timeline {
    /// The digest of its own facets (not its tracks, markers or carried pieces).
    pub fn fingerprint(&self) -> Digest {
        let mut e = Enc::new("timeline");
        e.str("name", &self.name)
            .opt_rate("rate", self.rate)
            .time("tc_start", self.tc_start)
            .flag("drop_frame", self.drop_frame)
            .opt_u32("width", self.width)
            .opt_u32("height", self.height)
            .opt_u32("audio_rate", self.audio_rate)
            .opt_time("duration", self.duration)
            .flag("compound", self.compound);
        e.finish()
    }

    /// The digest of its carried pieces, if it has any.
    pub fn payload(&self) -> Option<Digest> {
        payload_of(&self.opaque)
    }

    /// The end of its last item on any track (zero when empty).
    pub fn extent(&self) -> Result<Time, TimeError> {
        let mut end = Time::ZERO;
        for t in &self.tracks {
            for i in &t.items {
                end = end.max(i.end()?);
            }
        }
        Ok(end)
    }
}

impl Track {
    /// The digest of its own facets (not its items).
    pub fn fingerprint(&self) -> Digest {
        let mut e = Enc::new("track");
        e.str("name", &self.name)
            .str("kind", self.kind.name())
            .flag("muted", self.muted)
            .flag("locked", self.locked);
        e.finish()
    }

    /// The digest of its carried pieces, if it has any.
    pub fn payload(&self) -> Option<Digest> {
        payload_of(&self.opaque)
    }
}

impl Item {
    /// The digest of its own facets: kind, name, place, source, media, enabled, text,
    /// transition and effect names (not its markers, nested tracks or carried pieces).
    pub fn fingerprint(&self) -> Digest {
        let mut e = Enc::new("item");
        e.str("kind", self.kind.name())
            .str("name", &self.name)
            .time("start", self.start)
            .time("duration", self.duration)
            .opt_time("source_in", self.source_in)
            .opt_str("media", self.media.as_ref().map(Key::as_str))
            .flag("enabled", self.enabled)
            .opt_str("text", self.text.as_deref());
        match &self.transition {
            Some(t) => {
                e.str("transition", &t.kind)
                    .opt_time("in_offset", t.in_offset);
            }
            None => {
                e.bytes("transition", &[]);
            }
        }
        e.bytes("effects", &(self.effects.len() as u64).to_be_bytes());
        for f in &self.effects {
            e.str("effect", f);
        }
        e.finish()
    }

    /// The digest of its carried pieces, if it has any.
    pub fn payload(&self) -> Option<Digest> {
        payload_of(&self.opaque)
    }
}

impl Marker {
    /// The digest of its facets.
    pub fn fingerprint(&self) -> Digest {
        let mut e = Enc::new("marker");
        e.str("kind", self.kind.name())
            .str("name", &self.name)
            .opt_str("note", self.note.as_deref())
            .time("at", self.at)
            .time("duration", self.duration)
            .opt_time("media_at", self.media_at)
            .opt_str("color", self.color.as_deref());
        e.finish()
    }
}

impl Media {
    /// The digest of its facets.
    pub fn fingerprint(&self) -> Digest {
        let mut e = Enc::new("media");
        e.str("kind", self.kind.name())
            .str("name", &self.name)
            .opt_str("path", self.path.as_deref())
            .opt_time("start", self.start)
            .opt_time("duration", self.duration)
            .opt_rate("rate", self.rate)
            .opt_str("sequence", self.sequence.as_ref().map(Key::as_str));
        e.finish()
    }

    /// The digest of its carried pieces, if it has any.
    pub fn payload(&self) -> Option<Digest> {
        payload_of(&self.opaque)
    }
}

// ----- checks -----

/// Something [`Document::check`] found. Readers produce documents with none of the first
/// four; overlaps can come from the file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Finding {
    /// Two nodes hold one key.
    DuplicateKey(String),
    /// An item or marker with a negative duration, or an item other than a transition
    /// starting before zero.
    BadTime(String),
    /// An item names media the document doesn't hold.
    DanglingMedia {
        /// The item.
        item: String,
        /// The media key it names.
        media: String,
    },
    /// A track's items are not sorted by start.
    Unsorted(String),
    /// A transition's cut is outside it.
    BadTransition(String),
    /// Two items on one track cover the same time (gaps and transitions aside).
    Overlap {
        /// The track.
        track: String,
        /// The earlier item.
        first: String,
        /// The item that starts before `first` ends.
        second: String,
    },
}

impl Document {
    /// Checks the model's invariants: keys unique, times sane, media resolved, tracks sorted,
    /// transitions' cuts inside them; and reports overlaps. Empty when all hold.
    pub fn check(&self) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut keys = HashSet::new();
        let mut media = HashSet::new();
        for m in &self.media {
            if !keys.insert(m.key.as_str().to_owned()) {
                out.push(Finding::DuplicateKey(m.key.text.clone()));
            }
            media.insert(m.key.as_str());
        }
        for t in &self.timelines {
            if !keys.insert(t.key.as_str().to_owned()) {
                out.push(Finding::DuplicateKey(t.key.text.clone()));
            }
            check_markers(&t.markers, &mut keys, &mut out);
            check_tracks(&t.tracks, &media, &mut keys, &mut out);
        }
        out
    }

    /// Every node's key, in reading order (media, then each timeline depth first).
    pub fn keys(&self) -> Vec<&Key> {
        let mut out: Vec<&Key> = self.media.iter().map(|m| &m.key).collect();
        fn markers<'a>(ms: &'a [Marker], out: &mut Vec<&'a Key>) {
            out.extend(ms.iter().map(|m| &m.key));
        }
        fn tracks<'a>(ts: &'a [Track], out: &mut Vec<&'a Key>) {
            for t in ts {
                out.push(&t.key);
                for i in &t.items {
                    out.push(&i.key);
                    markers(&i.markers, out);
                    tracks(&i.tracks, out);
                }
                markers(&t.markers, out);
            }
        }
        for t in &self.timelines {
            out.push(&t.key);
            tracks(&t.tracks, &mut out);
            markers(&t.markers, &mut out);
        }
        out
    }
}

fn check_markers(ms: &[Marker], keys: &mut HashSet<String>, out: &mut Vec<Finding>) {
    for m in ms {
        if !keys.insert(m.key.text.clone()) {
            out.push(Finding::DuplicateKey(m.key.text.clone()));
        }
        if m.duration.is_negative() {
            out.push(Finding::BadTime(m.key.text.clone()));
        }
    }
}

fn check_tracks(
    tracks: &[Track],
    media: &HashSet<&str>,
    keys: &mut HashSet<String>,
    out: &mut Vec<Finding>,
) {
    for t in tracks {
        if !keys.insert(t.key.text.clone()) {
            out.push(Finding::DuplicateKey(t.key.text.clone()));
        }
        check_markers(&t.markers, keys, out);
        let mut last_start: Option<Time> = None;
        let mut last_body: Option<(&Item, Time)> = None;
        for i in &t.items {
            if !keys.insert(i.key.text.clone()) {
                out.push(Finding::DuplicateKey(i.key.text.clone()));
            }
            // a transition on a track's first cut may start before zero (its in-offset)
            if i.duration.is_negative() || (i.start.is_negative() && i.kind != ItemKind::Transition)
            {
                out.push(Finding::BadTime(i.key.text.clone()));
            }
            if let Some(m) = &i.media {
                if !media.contains(m.as_str()) {
                    out.push(Finding::DanglingMedia {
                        item: i.key.text.clone(),
                        media: m.text.clone(),
                    });
                }
            }
            if last_start.is_some_and(|s| i.start < s) {
                out.push(Finding::Unsorted(t.key.text.clone()));
            }
            last_start = Some(i.start);
            if let Some(tr) = &i.transition {
                if let Some(off) = tr.in_offset {
                    if off.is_negative() || off > i.duration {
                        out.push(Finding::BadTransition(i.key.text.clone()));
                    }
                }
            }
            check_markers(&i.markers, keys, out);
            check_tracks(&i.tracks, media, keys, out);
            if matches!(i.kind, ItemKind::Gap | ItemKind::Transition) {
                continue;
            }
            let Ok(end) = i.end() else {
                out.push(Finding::BadTime(i.key.text.clone()));
                continue;
            };
            if let Some((prev, prev_end)) = last_body {
                if i.start < prev_end {
                    out.push(Finding::Overlap {
                        track: t.key.text.clone(),
                        first: prev.key.text.clone(),
                        second: i.key.text.clone(),
                    });
                }
                if end > prev_end {
                    last_body = Some((i, end));
                }
            } else {
                last_body = Some((i, end));
            }
        }
    }
}

// ----- the sequential form -----

/// One step of a track in sequential form: what OTIO's tracks and FCPXML's spines hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Empty time no item holds.
    Gap(Time),
    /// The item at this index of the track's items: it starts where the last one ended.
    Item(usize),
    /// The transition at this index: it starts `in_offset` before the current position and
    /// takes no time of its own.
    Transition {
        /// Its index among the track's items.
        index: usize,
        /// From its start to the cut.
        in_offset: Time,
    },
}

impl Track {
    /// The track in sequential form: items in order, a [`Step::Gap`] for any empty time
    /// before or between them, transitions at their cuts. Refused when two items overlap or a
    /// transition doesn't say where its cut is.
    pub fn to_steps(&self) -> Result<Vec<Step>, Finding> {
        let mut steps = Vec::new();
        let mut pos = Time::ZERO;
        let bad_time = |i: &Item| Finding::BadTime(i.key.text.clone());
        // transitions sort among items by their cut, placed before the item that starts there
        let mut order: Vec<(Time, u8, usize)> = Vec::with_capacity(self.items.len());
        for (n, i) in self.items.iter().enumerate() {
            match &i.transition {
                Some(t) if i.kind == ItemKind::Transition => {
                    let off = t
                        .in_offset
                        .ok_or_else(|| Finding::BadTransition(i.key.text.clone()))?;
                    order.push((i.start.checked_add(off).map_err(|_| bad_time(i))?, 0, n));
                }
                _ => order.push((i.start, 1, n)),
            }
        }
        order.sort();
        let mut last: Option<usize> = None;
        for (at, is_item, n) in order {
            let i = &self.items[n];
            if is_item == 0 {
                if at > pos {
                    steps.push(Step::Gap(at.checked_sub(pos).map_err(|_| bad_time(i))?));
                    pos = at;
                }
                steps.push(Step::Transition {
                    index: n,
                    in_offset: at.checked_sub(i.start).map_err(|_| bad_time(i))?,
                });
                continue;
            }
            if i.start < pos {
                return Err(Finding::Overlap {
                    track: self.key.text.clone(),
                    first: last.map_or_else(String::new, |l| self.items[l].key.text.clone()),
                    second: i.key.text.clone(),
                });
            }
            if i.start > pos {
                steps.push(Step::Gap(
                    i.start.checked_sub(pos).map_err(|_| bad_time(i))?,
                ));
            }
            steps.push(Step::Item(n));
            pos = i.end().map_err(|_| bad_time(i))?;
            last = Some(n);
        }
        Ok(steps)
    }
}

/// Places `steps` (sequential form) into absolute starts: the start of each item they name,
/// by index into `items`. The inverse of [`Track::to_steps`].
pub fn place_steps(steps: &[Step], items: &[Item]) -> Result<Vec<(usize, Time)>, TimeError> {
    let mut pos = Time::ZERO;
    let mut out = Vec::with_capacity(steps.len());
    for s in steps {
        match s {
            Step::Gap(d) => pos = pos.checked_add(*d)?,
            Step::Item(n) => {
                out.push((*n, pos));
                pos = pos.checked_add(items[*n].duration)?;
            }
            Step::Transition { index, in_offset } => {
                out.push((*index, pos.checked_sub(*in_offset)?));
            }
        }
    }
    Ok(out)
}
