//! The live store's roots, keys and fields, and an OTIO file read into them ([`read`]), with
//! the byte range of every object the write-back splices ([`LiveTimeline`]).
//!
//! **Roots.** `tracks`, `items`, `markers` and `media` are MovableLists with minted keys
//! (`t<n>`, `i<n>`, `k<n>`, `m<n>`), each with a composite field map (`<key>\x1f<field>`); the
//! `timeline` map holds the timeline's own fields and the key marks. **Existence lives in the
//! lists. The items list's order means nothing:** a track's items in time order are its items
//! sorted by start, which every read does. Gaps are not stored: they are the time between items.
//!
//! **Time is ticks** ([`super::ticks`]): record times from the timeline's zero, as decimal
//! strings. An item's `geom` is one field, `start,dur`.
//!
//! **One timeline per live doc.** A collection declines (`live_many_timelines`); so does a
//! timeline whose stack holds something other than tracks, or trims itself (`live_structure`).

use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::index::Digest;
use k2_mpc_base::session::Declined;
use k2_mpc_base::table::CELL_SEP;

use super::ticks::{self, TickError};
use crate::error::ReadLimits;
use crate::json::{self, Node, NodeValue};
use crate::model::key_token;
use crate::time::{Rate, Time};

/// The tracks, in the file's stacking order: a later track composites over an earlier one
/// (OTIO's meaning; [`t::STACKING`] says so in the store).
pub const TRACKS: &str = "tracks";
/// The tracks' fields.
pub const TRACK_PROPS: &str = "track_props";
/// The items: clips, generators, transitions and nests. Insertion order, no meaning.
pub const ITEMS: &str = "items";
/// The items' fields.
pub const ITEM_PROPS: &str = "item_props";
/// The markers.
pub const MARKERS: &str = "markers";
/// The markers' fields.
pub const MARKER_PROPS: &str = "marker_props";
/// The media the items use.
pub const MEDIA: &str = "media";
/// The media's fields.
pub const MEDIA_PROPS: &str = "media_props";
/// The timeline's own fields and the key marks.
pub const TIMELINE: &str = "timeline";

/// Field names of the composite maps.
pub mod f {
    /// An item's track (the track's key).
    pub const TRACK: &str = "track";
    /// An item's record start and duration, ticks: `start,dur`.
    pub const GEOM: &str = "geom";
    /// An item's media time at its first frame, ticks.
    pub const SRC: &str = "src";
    /// A name.
    pub const NAME: &str = "name";
    /// An item's media (the media's key).
    pub const MEDIA: &str = "media";
    /// `1` when an item is disabled.
    pub const OFF: &str = "off";
    /// A transition's offsets around its cut, ticks: `in,out`.
    pub const TR: &str = "tr";
    /// What the file calls a track (its label, which `track list` prints; an outside save
    /// pairs tracks by name and kind, [`crate::live::pair_tracks`]) or a media (a digest of its
    /// location, for pairing an outside save); never a path. Items and markers have none stored: theirs is
    /// worked out from their live fields when a save is paired.
    pub const NID: &str = "nid";
    /// What may be written live: `1` everything, `0` nothing.
    pub const EDIT: &str = "edit";
    /// `1` when an item plays at another speed or holds a frame (its own time effect: a time
    /// warp, a freeze frame), else `0`; apart from [`EDIT`] (a transition or a nest is `edit`
    /// `0` and not retimed; a nest's insides and an effect the store doesn't keep never make it
    /// retimed). Read-only.
    pub const RETIMED: &str = "retimed";
    /// `1` when a track is disabled in the file: a muted audio track (on any track, the same as
    /// [`ENABLED`] `0`). Read-only.
    pub const MUTED: &str = "muted";
    /// `1` when a track takes part, `0` when the file disables it: a hidden video track, a
    /// muted audio track. Read-only: no verb writes it, and the write-back refuses a change.
    pub const ENABLED: &str = "enabled";
    /// `1` when a track is locked.
    pub const LOCKED: &str = "locked";
    /// A marker's owner: an item's or a track's key; empty for the timeline.
    pub const OWNER: &str = "owner";
    /// A marker's time, ticks: media time on an item, record time otherwise.
    pub const AT: &str = "at";
    /// A duration, ticks.
    pub const DUR: &str = "dur";
    /// A marker's comment.
    pub const NOTE: &str = "note";
    /// A marker's colour, as the file names it.
    pub const COLOR: &str = "color";
    /// A media's file name (the last component of its location, never the path).
    pub const FILE: &str = "file";
    /// A media's first frame, ticks: its available range's start, so a clip's place in its
    /// media is its `src` less this, exactly. None when it has no available range
    /// ([`ORIGIN`] is then [`ORIGIN_UNKNOWN`]).
    pub const START: &str = "start";
    /// Where a media's time starts: [`ORIGIN_RANGE`] (its [`START`] is its available range's)
    /// or [`ORIGIN_UNKNOWN`] (it has no available range, and no start).
    pub const ORIGIN: &str = "origin";
    /// [`ORIGIN`]'s value for a media with an available range.
    pub const ORIGIN_RANGE: &str = "range";
    /// [`ORIGIN`]'s value for a media with none.
    pub const ORIGIN_UNKNOWN: &str = "unknown";
    /// A media's rate, or the rate a clip's times are written at: `num/den`.
    pub const RATE: &str = "rate";
}

/// Keys of the `timeline` map.
pub mod t {
    /// The timeline's name.
    pub const NAME: &str = "name";
    /// Its rate, `num/den`, when the file says.
    pub const RATE: &str = "rate";
    /// The timecode of its first frame, ticks.
    pub const TC: &str = "tc";
    /// `1` when its timecode is drop-frame (`HH:MM:SS;FF` at `30000/1001` and `60000/1001`),
    /// else `0`. Read from the drop-frame (DF/NDF) entry of the timeline's `metadata`
    /// ([`crate::otio::drop_frame_entry`]: exactly `DF`, at `30000/1001` or `60000/1001`); a
    /// file without it, or with anything else there, stores `0`. Read-only: no verb writes it,
    /// the write-back refuses a change, and the entry in the file is never written.
    pub const DROP_FRAME: &str = "drop_frame";
    /// The format (`otio`).
    pub const FORMAT: &str = "format";
    /// The file's own version (`Timeline.1`).
    pub const VERSION: &str = "version";
    /// This doc's id, in our stamp on the items the write-back touches.
    pub const DOC: &str = "doc";
    /// The highest track key ever minted.
    pub const MARK_T: &str = "mark/t";
    /// The highest item key ever minted.
    pub const MARK_I: &str = "mark/i";
    /// The highest marker key ever minted.
    pub const MARK_K: &str = "mark/k";
    /// The highest media key ever minted.
    pub const MARK_M: &str = "mark/m";
    /// How the tracks stack: always [`LAST_ON_TOP`] (a later track composites over an earlier
    /// one, OTIO's meaning), so a player needn't know the format.
    pub const STACKING: &str = "stacking";
    /// [`STACKING`]'s value: the tracks' order, the bottom first.
    pub const LAST_ON_TOP: &str = "last_on_top";
    /// What the timeline's end is: always [`ITEM_ENDS`]. A duration the file stores (a track's
    /// own range) stays in the file and is never the end.
    pub const END_FROM: &str = "end_from";
    /// [`END_FROM`]'s value: the end is the latest end (record start + duration) of any item on
    /// any track, as `timeline info` prints it.
    pub const ITEM_ENDS: &str = "items";
}

/// The key of a composite map: `<elem>\x1f<field>`.
pub fn prop(k: &Key, fld: &str) -> String {
    format!("{}{CELL_SEP}{fld}", k.as_str())
}

/// The minted key `<prefix><n>`.
pub fn key(prefix: char, n: u64) -> Key {
    Key::new(format!("{prefix}{n}")).expect("a minted key")
}

/// `n` of a minted key `<prefix><n>` (no leading zeros).
pub fn key_num(k: &str, prefix: char) -> Option<u64> {
    let d = k.strip_prefix(prefix)?;
    if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) || (d.len() > 1 && d.starts_with('0'))
    {
        return None;
    }
    d.parse().ok()
}

/// The live tier's bounds. A file over them declines (`live_too_large`) and stays at tier 0.
#[derive(Debug, Clone)]
pub struct LiveLimits {
    /// The file, in bytes.
    pub max_file_bytes: u64,
    /// Items (clips, transitions, nests).
    pub max_items: usize,
    /// Markers.
    pub max_markers: usize,
    /// Tracks.
    pub max_tracks: usize,
    /// The reader's own bounds.
    pub read: ReadLimits,
}

impl Default for LiveLimits {
    fn default() -> Self {
        LiveLimits {
            max_file_bytes: 32 << 20,
            max_items: 2_500,
            max_markers: 2_000,
            max_tracks: 200,
            read: ReadLimits::default(),
        }
    }
}

/// `1` or `0`.
fn flag(b: bool) -> String {
    (if b { "1" } else { "0" }).to_owned()
}

/// Where a node sits in the file's tree: the place of each entry and element from the root.
pub type Path = Vec<usize>;

/// `path` and then the place of `key`'s entry in the object at `path`.
fn entry_path(n: &Node, path: &[usize], key: &str) -> Option<Path> {
    let i = n.entries()?.iter().position(|(k, _, _)| k == key)?;
    let mut p = path.to_vec();
    p.push(i);
    Some(p)
}

/// `path` and then element `j`.
fn elem_path(path: &[usize], j: usize) -> Path {
    let mut p = path.to_vec();
    p.push(j);
    p
}

/// A decline with its code and fixed text (nothing quoted from the file).
pub fn declined(code: &'static str, message: impl Into<String>) -> Declined {
    Declined {
        code,
        message: message.into(),
    }
}

fn tick(t: Time) -> Result<i64, Declined> {
    ticks::of(t).map_err(|e| match e {
        TickError::OffTick => declined(
            "live_time_off_tick",
            "a time in the file isn't a whole number of ticks (1/254,016,000,000 s)",
        ),
        TickError::Range => declined("live_time_range", "a time in the file is out of range"),
    })
}

fn structure(why: &str) -> Declined {
    declined(
        "live_structure",
        format!("the timeline can't open live: {why}"),
    )
}

// ---------- what the file holds, with its ranges ----------

/// A track's child, in the file's order.
#[derive(Debug, Clone)]
pub enum Child {
    /// An item: its index in [`LiveTimeline::items`].
    Item(usize),
    /// A gap: its duration in ticks and its object.
    Gap {
        /// Its duration, ticks.
        dur: i64,
        /// Where the gap object is.
        path: Path,
    },
}

/// The tracks' labels by the format's convention, counted in stacking order (the bottom
/// first): a track's kind's letter (`V` video, `A` audio, `O` any other) and its place among
/// the tracks of its kind, from 1 (`V1`, `A2`). The read labels the file's tracks with it (each
/// track's `nid`, which `track list` prints) and the view labels
/// the store's tracks with it ([`super::view::KIND_LABEL`]), so the two agree.
#[derive(Debug, Clone, Default)]
pub struct TrackLabels {
    video: u32,
    audio: u32,
    other: u32,
}

impl TrackLabels {
    /// The label of the next track up the stack, of the store's kind `kind` (`video`, `audio`,
    /// anything else counted as other).
    pub fn next(&mut self, kind: &str) -> String {
        let (n, letter) = match kind {
            "video" => (&mut self.video, 'V'),
            "audio" => (&mut self.audio, 'A'),
            _ => (&mut self.other, 'O'),
        };
        *n += 1;
        format!("{letter}{n}")
    }
}

/// One track as the file holds it.
#[derive(Debug, Clone)]
pub struct LTrack {
    /// Its kind (`video`, `audio`, `other`).
    pub kind: &'static str,
    /// Its name as the file writes it.
    pub name: String,
    /// Its label: the kind's letter and its place among tracks of that kind (`V1`,
    /// [`TrackLabels`]).
    pub label: String,
    /// Disabled in the file.
    pub muted: bool,
    /// Every gap is plain (a duration and nothing else): the write-back may rebuild them.
    pub editable: bool,
    /// Its children, in order.
    pub children: Vec<Child>,
    /// Where the track object is.
    pub path: Path,
}

/// One item as the file holds it.
#[derive(Debug, Clone)]
pub struct LItem {
    /// Its track's index.
    pub track: usize,
    /// `clip`, `generator`, `transition` or `nest`.
    pub kind: &'static str,
    /// Record start, ticks.
    pub start: i64,
    /// Record duration, ticks.
    pub dur: i64,
    /// Media time at its first frame, ticks (none for a transition).
    pub src: Option<i64>,
    /// Its name.
    pub name: String,
    /// Its media's index.
    pub media: Option<usize>,
    /// Disabled.
    pub off: bool,
    /// A transition's offsets, ticks.
    pub tr: Option<(i64, i64)>,
    /// The rate its times are written at (its duration's), when the write-back writes them.
    pub rate: Option<Rate>,
    /// Every live verb may write it.
    pub editable: bool,
    /// It plays at another speed or holds a frame (its own time effect).
    pub retimed: bool,
    /// Our stamp in its metadata, `(key, doc)`, when it has one.
    pub stamp: Option<(String, String)>,
    /// Where the object is (a clip, a transition, a stack or track).
    pub path: Path,
}

/// What owns a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// An item, by index.
    Item(usize),
    /// A track, by index.
    Track(usize),
    /// The timeline (its stack).
    Timeline,
}

/// One marker as the file holds it.
#[derive(Debug, Clone)]
pub struct LMarker {
    /// Its owner.
    pub owner: Owner,
    /// Its time, ticks: media time on an item, record time otherwise.
    pub at: i64,
    /// Its duration, ticks.
    pub dur: i64,
    /// Its name.
    pub name: String,
    /// Its comment.
    pub note: String,
    /// Its colour.
    pub color: String,
    /// Where the marker object is.
    pub path: Path,
}

/// One media reference as the items use it.
#[derive(Debug, Clone)]
pub struct LMedia {
    /// `file`, `missing`, `generator`, `other`.
    pub kind: &'static str,
    /// Its name.
    pub name: String,
    /// The last component of its location, else its name.
    pub file: String,
    /// Its available range, ticks, with its rate.
    pub range: Option<(i64, i64, Rate)>,
    /// A digest of what it is (its location), for pairing: never the path.
    pub nid: String,
}

/// An OTIO timeline as the live store reads it, with what the write-back needs of the file.
#[derive(Debug, Clone)]
pub struct LiveTimeline {
    /// Its name.
    pub name: String,
    /// Its rate, from its start time or its first item.
    pub rate: Option<Rate>,
    /// The timecode of its first frame, ticks.
    pub tc: i64,
    /// Its timecode is drop-frame ([`t::DROP_FRAME`]).
    pub drop_frame: bool,
    /// The root's schema (`Timeline.1`).
    pub version: String,
    /// A digest of the file as first read: the doc id an import mints.
    pub doc_id: String,
    /// Tracks, in order.
    pub tracks: Vec<LTrack>,
    /// Items, in the order the file holds them (track by track).
    pub items: Vec<LItem>,
    /// Markers, in the order the file holds them (each track's items' and the track's own,
    /// track by track, then the timeline's).
    pub markers: Vec<LMarker>,
    /// Media, in the order the items first use them.
    pub media: Vec<LMedia>,
    /// Where the timeline's stack object is.
    pub stack: Path,
    /// The root object.
    pub root: Node,
}

fn get<'n>(n: &'n Node, k: &str) -> Option<&'n Node> {
    n.get(k).filter(|v| !v.is_null())
}

fn schema(n: &Node) -> (&str, &str) {
    let s = n.get("OTIO_SCHEMA").and_then(Node::as_str).unwrap_or("");
    s.rsplit_once('.').unwrap_or((s, ""))
}

fn name_of(n: &Node) -> String {
    n.get("name")
        .and_then(Node::as_str)
        .unwrap_or("")
        .to_owned()
}

/// A `RationalTime.1` as ticks, with its rate.
pub fn rational(n: &Node) -> Result<(i64, Rate), Declined> {
    let bad = || structure("a time it can't read");
    let rate = Rate::from_f64(
        n.get("rate")
            .and_then(Node::as_num)
            .ok_or_else(bad)?
            .as_f64(),
    )
    .map_err(|_| bad())?;
    let (num, den) = n
        .get("value")
        .and_then(Node::as_num)
        .and_then(|v| v.ratio())
        .ok_or_else(bad)?;
    let t = Time::from_i128(
        num.checked_mul(i128::from(rate.den())).ok_or_else(bad)?,
        den.checked_mul(i128::from(rate.num())).ok_or_else(bad)?,
    )
    .map_err(|_| declined("live_time_range", "a time in the file is out of range"))?;
    Ok((tick(t)?, rate))
}

/// A `TimeRange.1`: start and duration in ticks, and the duration's rate.
pub fn range(n: &Node) -> Result<(i64, i64, Rate), Declined> {
    let bad = || structure("a time range it can't read");
    let (s, _) = rational(n.get("start_time").ok_or_else(bad)?)?;
    let (d, r) = rational(n.get("duration").ok_or_else(bad)?)?;
    Ok((s, d, r))
}

/// The active media reference of a clip.
pub fn active_ref(c: &Node) -> Option<&Node> {
    if schema(c).1 == "1" {
        return get(c, "media_reference");
    }
    let active = c
        .get("active_media_reference_key")
        .and_then(Node::as_str)
        .unwrap_or("DEFAULT_MEDIA");
    get(get(c, "media_references")?, active)
}

/// Our stamp in an object's metadata: `metadata.mpc.key` and `metadata.mpc.doc`.
pub const STAMP: &str = "mpc";

fn stamp_of(n: &Node) -> Option<(String, String)> {
    let s = get(get(n, "metadata")?, STAMP)?;
    Some((
        s.get("key")?.as_str()?.to_owned(),
        s.get("doc")?.as_str()?.to_owned(),
    ))
}

/// Whether an effect list holds a time effect (the clip plays at another speed or holds).
fn retimed(n: &Node) -> bool {
    get(n, "effects").and_then(Node::as_arr).is_some_and(|a| {
        a.iter().any(|e| {
            let s = schema(e).0;
            s.contains("TimeWarp") || s.contains("FreezeFrame") || s.contains("TimeEffect")
        })
    })
}

fn hex(d: &Digest, n: usize) -> String {
    d.as_bytes()
        .iter()
        .take(n / 2)
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct Walk<'l> {
    limits: &'l LiveLimits,
    out_items: Vec<LItem>,
    out_markers: Vec<LMarker>,
    media: Vec<LMedia>,
    media_by: HashMap<String, usize>,
    first_rate: Option<Rate>,
}

impl Walk<'_> {
    fn media(&mut self, r: &Node) -> Result<usize, Declined> {
        let (n, _) = schema(r);
        let (kind, location) = match n {
            "ExternalReference" => (
                "file",
                r.get("target_url")
                    .and_then(Node::as_str)
                    .map(str::to_owned),
            ),
            "ImageSequenceReference" => (
                "file",
                Some(format!(
                    "{}{}",
                    r.get("target_url_base")
                        .and_then(Node::as_str)
                        .unwrap_or(""),
                    r.get("name_prefix").and_then(Node::as_str).unwrap_or("")
                )),
            ),
            "MissingReference" => ("missing", None),
            "GeneratorReference" => ("generator", None),
            _ => ("other", None),
        };
        let identity = match &location {
            Some(l) => format!("{kind}\0{l}"),
            None => {
                let mut canon = Vec::new();
                r.to_json().canonical(&mut canon);
                format!("{kind}\0{}", String::from_utf8_lossy(&canon))
            }
        };
        if let Some(&i) = self.media_by.get(&identity) {
            return Ok(i);
        }
        let mut name = name_of(r);
        if name.is_empty() {
            name = match (&location, kind) {
                (Some(l), _) => l.rsplit(['/', '\\']).next().unwrap_or("").to_owned(),
                (None, "generator") => r
                    .get("generator_kind")
                    .and_then(Node::as_str)
                    .unwrap_or("")
                    .to_owned(),
                _ => String::new(),
            };
        }
        let file = match &location {
            Some(l) => l.rsplit(['/', '\\']).next().unwrap_or("").to_owned(),
            None => name.clone(),
        };
        let range = match get(r, "available_range") {
            Some(a) => Some(range(a)?),
            None => None,
        };
        let nid = format!("m:{}", hex(&Digest::of(identity.as_bytes()), 16));
        let i = self.media.len();
        self.media.push(LMedia {
            kind,
            name,
            file,
            range,
            nid,
        });
        self.media_by.insert(identity, i);
        Ok(i)
    }

    fn markers(&mut self, n: &Node, at: &[usize], owner: Owner) -> Result<(), Declined> {
        let Some(list) = get(n, "markers").and_then(Node::as_arr) else {
            return Ok(());
        };
        let list_at = entry_path(n, at, "markers").unwrap_or_default();
        for (j, m) in list.iter().enumerate() {
            let (_, ver) = schema(m);
            let r = if ver == "1" {
                m.get("range")
            } else {
                m.get("marked_range")
            }
            .ok_or_else(|| structure("a marker without a range"))?;
            let (at, dur, _) = range(r)?;
            self.out_markers.push(LMarker {
                owner,
                at,
                dur,
                name: name_of(m),
                note: m
                    .get("comment")
                    .and_then(Node::as_str)
                    .unwrap_or("")
                    .to_owned(),
                color: m
                    .get("color")
                    .and_then(Node::as_str)
                    .unwrap_or("")
                    .to_owned(),
                path: elem_path(&list_at, j),
            });
        }
        Ok(())
    }

    fn track(
        &mut self,
        ti: usize,
        n: &Node,
        at: &[usize],
        label: String,
        kind: &'static str,
    ) -> Result<LTrack, Declined> {
        let mut pos: i64 = 0;
        let mut children = Vec::new();
        let mut editable = true;
        let kids = get(n, "children").and_then(Node::as_arr).unwrap_or(&[]);
        let kids_at = entry_path(n, at, "children").unwrap_or_default();
        for (j, c) in kids.iter().enumerate() {
            let c_at = elem_path(&kids_at, j);
            let (cn, _) = schema(c);
            match cn {
                "Gap" => {
                    let (_, dur, rate) = range(
                        get(c, "source_range").ok_or_else(|| structure("a gap without a range"))?,
                    )?;
                    self.first_rate.get_or_insert(rate);
                    let plain = name_of(c).is_empty()
                        && get(c, "markers")
                            .and_then(Node::as_arr)
                            .is_none_or(<[Node]>::is_empty)
                        && get(c, "effects")
                            .and_then(Node::as_arr)
                            .is_none_or(<[Node]>::is_empty)
                        && get(c, "metadata")
                            .and_then(Node::entries)
                            .is_none_or(<[_]>::is_empty)
                        && c.get("enabled").and_then(Node::as_bool).unwrap_or(true);
                    editable &= plain;
                    pos = pos.checked_add(dur).ok_or_else(tick_range)?;
                    children.push(Child::Gap { dur, path: c_at });
                }
                "Transition" => {
                    let off = |k: &str| -> Result<i64, Declined> {
                        Ok(rational(
                            c.get(k)
                                .ok_or_else(|| structure("a transition without its offsets"))?,
                        )?
                        .0)
                    };
                    let (inn, out) = (off("in_offset")?, off("out_offset")?);
                    let idx = self.push_item(LItem {
                        track: ti,
                        kind: "transition",
                        start: pos.checked_sub(inn).ok_or_else(tick_range)?,
                        dur: inn.checked_add(out).ok_or_else(tick_range)?,
                        src: None,
                        name: name_of(c),
                        media: None,
                        off: false,
                        tr: Some((inn, out)),
                        rate: None,
                        editable: false,
                        retimed: false,
                        stamp: stamp_of(c),
                        path: c_at.clone(),
                    })?;
                    children.push(Child::Item(idx));
                }
                "Clip" => {
                    let media = match active_ref(c) {
                        Some(r) => Some(self.media(r)?),
                        None => None,
                    };
                    let (src, dur, rate) = match get(c, "source_range") {
                        Some(r) => range(r)?,
                        None => media
                            .and_then(|m| self.media[m].range)
                            .ok_or_else(|| structure("a clip without a duration"))?,
                    };
                    self.first_rate.get_or_insert(rate);
                    let generator = media.is_some_and(|m| self.media[m].kind == "generator");
                    let idx = self.push_item(LItem {
                        track: ti,
                        kind: if generator { "generator" } else { "clip" },
                        start: pos,
                        dur,
                        src: Some(src),
                        name: name_of(c),
                        media,
                        off: !c.get("enabled").and_then(Node::as_bool).unwrap_or(true),
                        tr: None,
                        rate: Some(rate),
                        editable: !retimed(c),
                        retimed: retimed(c),
                        stamp: stamp_of(c),
                        path: c_at.clone(),
                    })?;
                    self.markers(c, &c_at, Owner::Item(idx))?;
                    pos = pos.checked_add(dur).ok_or_else(tick_range)?;
                    children.push(Child::Item(idx));
                }
                "Stack" | "Track" | "Sequence" => {
                    let r = match get(c, "source_range") {
                        Some(r) => Some(range(r)?),
                        None => None,
                    };
                    let dur = match r {
                        Some((_, d, _)) => d,
                        None => tick(nest_extent(c)?)?,
                    };
                    let idx = self.push_item(LItem {
                        track: ti,
                        kind: "nest",
                        start: pos,
                        dur,
                        src: Some(r.map_or(0, |(s, _, _)| s)),
                        name: name_of(c),
                        media: None,
                        off: !c.get("enabled").and_then(Node::as_bool).unwrap_or(true),
                        tr: None,
                        rate: r.map(|(_, _, x)| x),
                        editable: false,
                        retimed: retimed(c),
                        stamp: stamp_of(c),
                        path: c_at.clone(),
                    })?;
                    if cn == "Stack" {
                        self.markers(c, &c_at, Owner::Item(idx))?;
                    }
                    pos = pos.checked_add(dur).ok_or_else(tick_range)?;
                    children.push(Child::Item(idx));
                }
                _ => return Err(structure("a track holds an object it doesn't know")),
            }
        }
        if pos > ticks::RECORD_MAX {
            return Err(declined(
                "live_time_range",
                "a track is longer than a live timeline holds exactly (2^53 ticks, about 9.85 hours)",
            ));
        }
        self.markers(n, at, Owner::Track(ti))?;
        Ok(LTrack {
            kind,
            name: name_of(n),
            label,
            muted: !n.get("enabled").and_then(Node::as_bool).unwrap_or(true),
            editable,
            children,
            path: at.to_vec(),
        })
    }

    fn push_item(&mut self, it: LItem) -> Result<usize, Declined> {
        if self.out_items.len() >= self.limits.max_items {
            return Err(declined(
                "live_too_large",
                format!("more than {} items", self.limits.max_items),
            ));
        }
        self.out_items.push(it);
        Ok(self.out_items.len() - 1)
    }
}

fn tick_range() -> Declined {
    declined("live_time_range", "a time in the file is out of range")
}

/// A nest's length when it states none: the end of its longest track (a stack) or the sum
/// of its children (a track), as the reader places them.
fn nest_extent(c: &Node) -> Result<Time, Declined> {
    let doc = crate::otio::read_otio(
        format!(
            "{{\"OTIO_SCHEMA\":\"Timeline.1\",\"name\":\"\",\"tracks\":{{\"OTIO_SCHEMA\":\"Stack.1\",\"children\":[{}]}}}}",
            wrap_track(c)
        )
        .as_bytes(),
        &ReadLimits::default(),
    )
    .map_err(|_| structure("a nest it can't place"))?;
    doc.timelines
        .first()
        .map(|t| t.extent())
        .transpose()
        .map_err(|_| tick_range())
        .map(|t| t.unwrap_or(Time::ZERO))
}

fn wrap_track(c: &Node) -> String {
    let mut b = Vec::new();
    c.to_json().canonical(&mut b);
    let inner = String::from_utf8_lossy(&b).into_owned();
    format!("{{\"OTIO_SCHEMA\":\"Track.1\",\"children\":[{inner}]}}")
}

/// Reads an OTIO file as a live timeline, or why it can't be one (typed; nothing quoted from
/// the file). The tier-0 reader reads it first, so whatever it refuses declines here too.
pub fn read(bytes: &[u8], limits: &LiveLimits) -> Result<LiveTimeline, Declined> {
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(declined(
            "live_too_large",
            format!("the file is over {} bytes", limits.max_file_bytes),
        ));
    }
    let doc = crate::otio::read_otio(bytes, &limits.read).map_err(|e| {
        use crate::error::ReadError as E;
        match e {
            E::TooLarge { .. } | E::Limit { .. } => {
                declined("live_too_large", "the file is over the reader's bounds")
            }
            E::Unknown { .. } | E::UnknownVersion { .. } => declined(
                "unsupported",
                "the file holds an object type or version it doesn't know",
            ),
            _ => declined("unreadable", "the file isn't an OTIO timeline it can read"),
        }
    })?;
    if doc.timelines.len() != 1 || doc.version != "Timeline.1" {
        return Err(declined(
            "live_many_timelines",
            "a live doc holds one timeline: this file is a collection",
        ));
    }
    read_live(bytes, limits)
}

/// [`read`] without the tier-0 reader's pass: for bytes the live pack read before or wrote
/// itself (the carrier, a write-back's result). Only the live reader runs: the spanned JSON
/// reader and the walk, with every one of their refusals.
pub fn read_live(bytes: &[u8], limits: &LiveLimits) -> Result<LiveTimeline, Declined> {
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(declined(
            "live_too_large",
            format!("the file is over {} bytes", limits.max_file_bytes),
        ));
    }
    let root = json::parse_spanned(bytes, limits.read.max_depth, limits.read.max_items)
        .map_err(|_| declined("unreadable", "the file isn't an OTIO timeline it can read"))?;
    if root.get("OTIO_SCHEMA").and_then(Node::as_str) != Some("Timeline.1") {
        return Err(declined(
            "live_many_timelines",
            "a live doc holds one timeline: this file isn't one",
        ));
    }
    let stack = root
        .get("tracks")
        .ok_or_else(|| structure("a timeline without tracks"))?;
    let stack_at = entry_path(&root, &[], "tracks").unwrap_or_default();
    if get(stack, "source_range").is_some() {
        return Err(structure("its stack trims itself (a source range)"));
    }
    let (tc, tc_rate) = match get(&root, "global_start_time") {
        Some(t) => {
            let (v, r) = rational(t)?;
            (v, Some(r))
        }
        None => (0, None),
    };
    let mut w = Walk {
        limits,
        out_items: Vec::new(),
        out_markers: Vec::new(),
        media: Vec::new(),
        media_by: HashMap::new(),
        first_rate: None,
    };
    let mut tracks = Vec::new();
    let mut labels = TrackLabels::default();
    let tracks_at = entry_path(stack, &stack_at, "children").unwrap_or_default();
    for (j, c) in get(stack, "children")
        .and_then(Node::as_arr)
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        let (n, _) = schema(c);
        if n != "Track" && n != "Sequence" {
            return Err(structure("its stack holds a layer that isn't a track"));
        }
        if tracks.len() >= limits.max_tracks {
            return Err(declined(
                "live_too_large",
                format!("more than {} tracks", limits.max_tracks),
            ));
        }
        let kind = match c.get("kind").and_then(Node::as_str) {
            Some("Video") => "video",
            Some("Audio") => "audio",
            _ => "other",
        };
        let ti = tracks.len();
        let t = w.track(ti, c, &elem_path(&tracks_at, j), labels.next(kind), kind)?;
        tracks.push(t);
    }
    w.markers(stack, &stack_at, Owner::Timeline)?;
    if w.out_markers.len() > limits.max_markers {
        return Err(declined(
            "live_too_large",
            format!("more than {} markers", limits.max_markers),
        ));
    }
    let rate = tc_rate.or(w.first_rate);
    let entry = root
        .get("metadata")
        .and_then(|m| {
            crate::otio::CONVERTER_DF_KEY
                .split('/')
                .try_fold(m, |n, k| n.get(k))
        })
        .and_then(Node::as_str);
    Ok(LiveTimeline {
        name: name_of(&root),
        rate,
        tc,
        drop_frame: crate::otio::drop_frame_entry(entry, rate),
        version: "Timeline.1".into(),
        doc_id: hex(&Digest::of(bytes), 16),
        tracks,
        items: w.out_items,
        markers: w.out_markers,
        media: w.media,
        stack: stack_at,
        root,
    })
}

// ---------- the keyed content ----------

/// The keys of a timeline's units, in its decode order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keys {
    /// Tracks.
    pub tracks: Vec<Key>,
    /// Items.
    pub items: Vec<Key>,
    /// Markers.
    pub markers: Vec<Key>,
    /// Media.
    pub media: Vec<Key>,
}

/// The highest minted number per root: tracks, items, markers, media.
pub type Marks = [u64; 4];

/// The derived identity of an item, for pairing an outside save: what it is, its media's (or
/// its own) name, and its source in. It survives a move and a rename of the item.
pub fn item_nid(kind: &str, what: &str, src: Option<i64>, start: i64) -> String {
    match kind {
        "transition" => format!("tr@{start}"),
        _ => format!("{kind}:{}@{}", key_token(what), src.unwrap_or(0)),
    }
}

impl LiveTimeline {
    /// The keys an import mints: in decode order, from 1.
    pub fn minted(&self) -> (Keys, Marks) {
        let n = |c: char, len: usize| (1..=len as u64).map(|i| key(c, i)).collect::<Vec<_>>();
        (
            Keys {
                tracks: n('t', self.tracks.len()),
                items: n('i', self.items.len()),
                markers: n('k', self.markers.len()),
                media: n('m', self.media.len()),
            },
            [
                self.tracks.len() as u64,
                self.items.len() as u64,
                self.markers.len() as u64,
                self.media.len() as u64,
            ],
        )
    }

    /// An item's derived identity ([`item_nid`]).
    pub fn nid(&self, i: usize) -> String {
        let it = &self.items[i];
        let what = match it.media {
            Some(m) if !self.media[m].name.is_empty() => self.media[m].name.as_str(),
            _ => it.name.as_str(),
        };
        item_nid(it.kind, what, it.src, it.start)
    }

    /// A marker's derived identity: its owner's, its name and its time.
    pub fn marker_nid(&self, m: usize, keys: &Keys) -> String {
        let mk = &self.markers[m];
        let owner = match mk.owner {
            Owner::Item(i) => keys.items[i].as_str().to_owned(),
            Owner::Track(t) => keys.tracks[t].as_str().to_owned(),
            Owner::Timeline => String::new(),
        };
        format!("{owner}/{}@{}", key_token(&mk.name), mk.at)
    }

    /// The doc content under `keys`, with the marks `marks` and the doc id `doc`.
    /// Track `t`'s fields, as the store holds them.
    pub fn track_fields(&self, t: usize) -> Vec<(&'static str, String)> {
        let tr = &self.tracks[t];
        vec![
            (f::NAME, tr.name.clone()),
            (f::MUTED, flag(tr.muted)),
            (f::ENABLED, flag(!tr.muted)),
            (f::LOCKED, flag(false)),
            (f::EDIT, flag(tr.editable)),
            (f::NID, tr.label.clone()),
        ]
    }

    /// Item `i`'s fields, as the store holds them under `keys`.
    pub fn item_fields(&self, i: usize, keys: &Keys) -> Vec<(&'static str, String)> {
        let it = &self.items[i];
        let mut v = vec![
            (f::TRACK, keys.tracks[it.track].as_str().to_owned()),
            (f::GEOM, ticks::pair_text(it.start, it.dur)),
            (f::NAME, it.name.clone()),
            (f::OFF, flag(it.off)),
            (f::EDIT, flag(it.editable)),
            (f::RETIMED, flag(it.retimed)),
        ];
        if let Some(src) = it.src {
            v.push((f::SRC, ticks::text(src)));
        }
        if let Some(m) = it.media {
            v.push((f::MEDIA, keys.media[m].as_str().to_owned()));
        }
        if let Some(r) = it.rate {
            v.push((f::RATE, ticks::rate_text(r)));
        }
        if let Some((a, b)) = it.tr {
            v.push((f::TR, ticks::pair_text(a, b)));
        }
        v
    }

    /// Marker `m`'s owner key, under `keys` (empty for the timeline).
    pub fn owner_key<'k>(&self, m: usize, keys: &'k Keys) -> &'k str {
        match self.markers[m].owner {
            Owner::Item(i) => keys.items[i].as_str(),
            Owner::Track(t) => keys.tracks[t].as_str(),
            Owner::Timeline => "",
        }
    }

    /// Marker `m`'s fields, as the store holds them under `keys`.
    pub fn marker_fields(&self, m: usize, keys: &Keys) -> Vec<(&'static str, String)> {
        let mk = &self.markers[m];
        vec![
            (f::OWNER, self.owner_key(m, keys).to_owned()),
            (f::AT, ticks::text(mk.at)),
            (f::DUR, ticks::text(mk.dur)),
            (f::NAME, mk.name.clone()),
            (f::NOTE, mk.note.clone()),
            (f::COLOR, mk.color.clone()),
        ]
    }

    /// Media `m`'s fields, as the store holds them.
    pub fn media_fields(&self, m: usize) -> Vec<(&'static str, String)> {
        let md = &self.media[m];
        let mut v = vec![
            (f::NAME, md.name.clone()),
            (f::FILE, md.file.clone()),
            (f::NID, md.nid.clone()),
        ];
        match md.range {
            Some((st, d, r)) => {
                v.push((f::START, ticks::text(st)));
                v.push((f::DUR, ticks::text(d)));
                v.push((f::RATE, ticks::rate_text(r)));
                v.push((f::ORIGIN, f::ORIGIN_RANGE.to_owned()));
            }
            None => v.push((f::ORIGIN, f::ORIGIN_UNKNOWN.to_owned())),
        }
        v
    }

    /// The `timeline` map's fields the file holds, and what the store states of it (how its
    /// tracks stack, what its end is); not the store-only marks and doc id.
    pub fn timeline_fields(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            (t::NAME, self.name.clone()),
            (t::TC, ticks::text(self.tc)),
            (t::DROP_FRAME, flag(self.drop_frame)),
            (t::FORMAT, "otio".to_owned()),
            (t::VERSION, self.version.clone()),
            (t::STACKING, t::LAST_ON_TOP.to_owned()),
            (t::END_FROM, t::ITEM_ENDS.to_owned()),
        ];
        if let Some(r) = self.rate {
            v.push((t::RATE, ticks::rate_text(r)));
        }
        v
    }

    /// The doc content under `keys`, with the marks `marks` and the doc id `doc`.
    pub fn to_content(&self, keys: &Keys, marks: Marks, doc: &str) -> Vec<(String, Neutral)> {
        let s = |v: &str| Value::str(v);
        let fill = |m: &mut BTreeMap<String, Value>, k: &Key, fields: Vec<(&str, String)>| {
            for (fld, v) in fields {
                m.insert(prop(k, fld), Value::str(v));
            }
        };
        let mut tl = BTreeMap::new();
        for (k, v) in self.timeline_fields() {
            tl.insert(k.to_owned(), Value::str(v));
        }
        tl.insert(t::DOC.to_owned(), s(doc));
        for (k, v) in [t::MARK_T, t::MARK_I, t::MARK_K, t::MARK_M]
            .iter()
            .zip(marks)
        {
            tl.insert((*k).to_owned(), s(&v.to_string()));
        }
        let mut tracks = Vec::new();
        let mut tp = BTreeMap::new();
        for (n, (tr, k)) in self.tracks.iter().zip(&keys.tracks).enumerate() {
            tracks.push((k.clone(), s(tr.kind)));
            fill(&mut tp, k, self.track_fields(n));
        }
        let mut items = Vec::new();
        let mut ip = BTreeMap::new();
        for (n, (it, k)) in self.items.iter().zip(&keys.items).enumerate() {
            items.push((k.clone(), s(it.kind)));
            fill(&mut ip, k, self.item_fields(n, keys));
        }
        let mut markers = Vec::new();
        let mut kp = BTreeMap::new();
        for (n, k) in keys.markers.iter().enumerate().take(self.markers.len()) {
            markers.push((k.clone(), s("marker")));
            fill(&mut kp, k, self.marker_fields(n, keys));
        }
        let mut media = Vec::new();
        let mut mp = BTreeMap::new();
        for (n, (md, k)) in self.media.iter().zip(&keys.media).enumerate() {
            media.push((k.clone(), s(md.kind)));
            fill(&mut mp, k, self.media_fields(n));
        }
        vec![
            (TRACKS.to_owned(), Neutral::List(tracks)),
            (ITEMS.to_owned(), Neutral::List(items)),
            (MARKERS.to_owned(), Neutral::List(markers)),
            (MEDIA.to_owned(), Neutral::List(media)),
            (TRACK_PROPS.to_owned(), Neutral::Map(tp)),
            (ITEM_PROPS.to_owned(), Neutral::Map(ip)),
            (MARKER_PROPS.to_owned(), Neutral::Map(kp)),
            (MEDIA_PROPS.to_owned(), Neutral::Map(mp)),
            (TIMELINE.to_owned(), Neutral::Map(tl)),
        ]
    }

    /// The node at `path` (entry and element places from the root): a unit's object.
    pub fn node(&self, path: &[usize]) -> &Node {
        let mut n = &self.root;
        for &i in path {
            n = match &n.v {
                NodeValue::Obj(kv) => &kv[i].2,
                NodeValue::Arr(a) => &a[i],
                _ => return n,
            };
        }
        n
    }
}

// ---------- the store, as the hooks and verbs read it ----------

static EMPTY_MAP: BTreeMap<String, Value> = BTreeMap::new();

/// The live store's roots, borrowed from a [`k2_mpc_base::session::View`] or from content.
#[derive(Debug, Clone, Copy)]
pub struct Store<'a> {
    /// Tracks.
    pub tracks: &'a [(Key, Value)],
    /// Items.
    pub items: &'a [(Key, Value)],
    /// Markers.
    pub markers: &'a [(Key, Value)],
    /// Media.
    pub media: &'a [(Key, Value)],
    /// Track fields.
    pub tp: &'a BTreeMap<String, Value>,
    /// Item fields.
    pub ip: &'a BTreeMap<String, Value>,
    /// Marker fields.
    pub kp: &'a BTreeMap<String, Value>,
    /// Media fields.
    pub mp: &'a BTreeMap<String, Value>,
    /// The timeline's fields.
    pub tl: &'a BTreeMap<String, Value>,
}

impl<'a> Store<'a> {
    /// From a view of the live doc.
    pub fn of_view(v: &'a k2_mpc_base::session::View) -> Store<'a> {
        Store {
            tracks: v.list(TRACKS).unwrap_or(&[]),
            items: v.list(ITEMS).unwrap_or(&[]),
            markers: v.list(MARKERS).unwrap_or(&[]),
            media: v.list(MEDIA).unwrap_or(&[]),
            tp: v.map(TRACK_PROPS).unwrap_or(&EMPTY_MAP),
            ip: v.map(ITEM_PROPS).unwrap_or(&EMPTY_MAP),
            kp: v.map(MARKER_PROPS).unwrap_or(&EMPTY_MAP),
            mp: v.map(MEDIA_PROPS).unwrap_or(&EMPTY_MAP),
            tl: v.map(TIMELINE).unwrap_or(&EMPTY_MAP),
        }
    }

    /// From doc content.
    pub fn of_content(c: &'a [(String, Neutral)]) -> Store<'a> {
        let list = |r: &str| -> &'a [(Key, Value)] {
            c.iter()
                .find_map(|(n, v)| match v {
                    Neutral::List(l) if n == r => Some(l.as_slice()),
                    _ => None,
                })
                .unwrap_or(&[])
        };
        let map = |r: &str| -> &'a BTreeMap<String, Value> {
            c.iter()
                .find_map(|(n, v)| match v {
                    Neutral::Map(m) if n == r => Some(m),
                    _ => None,
                })
                .unwrap_or(&EMPTY_MAP)
        };
        Store {
            tracks: list(TRACKS),
            items: list(ITEMS),
            markers: list(MARKERS),
            media: list(MEDIA),
            tp: map(TRACK_PROPS),
            ip: map(ITEM_PROPS),
            kp: map(MARKER_PROPS),
            mp: map(MEDIA_PROPS),
            tl: map(TIMELINE),
        }
    }

    /// A field of a composite map.
    pub fn field(m: &'a BTreeMap<String, Value>, k: &Key, fld: &str) -> Option<&'a str> {
        m.get(&prop(k, fld)).and_then(Value::as_str)
    }

    /// An item's field.
    pub fn item(&self, k: &Key, fld: &str) -> Option<&'a str> {
        Self::field(self.ip, k, fld)
    }

    /// A track's field.
    pub fn track(&self, k: &Key, fld: &str) -> Option<&'a str> {
        Self::field(self.tp, k, fld)
    }

    /// A marker's field.
    pub fn marker(&self, k: &Key, fld: &str) -> Option<&'a str> {
        Self::field(self.kp, k, fld)
    }

    /// A media's field.
    pub fn media_field(&self, k: &Key, fld: &str) -> Option<&'a str> {
        Self::field(self.mp, k, fld)
    }

    /// The timeline's field.
    pub fn tl(&self, k: &str) -> Option<&'a str> {
        self.tl.get(k).and_then(Value::as_str)
    }

    /// An item's derived identity ([`item_nid`]), worked out from its live fields: its kind,
    /// its media's name (else its own), its source in, and for a transition its start. Never
    /// stored, so no edit can leave it stale.
    pub fn item_identity(&self, k: &Key) -> Option<String> {
        let kind = self.kind(k)?;
        let src = self.item(k, f::SRC).and_then(ticks::parse);
        let (start, _) = self.geom(k)?;
        let media_name = self
            .item(k, f::MEDIA)
            .and_then(|m| Key::new(m).ok())
            .and_then(|m| self.media_field(&m, f::NAME))
            .filter(|n| !n.is_empty());
        let what = media_name.unwrap_or_else(|| self.item(k, f::NAME).unwrap_or(""));
        Some(item_nid(kind, what, src, start))
    }

    /// A marker's derived identity, worked out from its live fields: its owner, its name and
    /// its time (as [`LiveTimeline::marker_nid`] gives a file's).
    pub fn marker_identity(&self, k: &Key) -> Option<String> {
        let owner = self.marker(k, f::OWNER)?;
        let at = self.marker(k, f::AT).and_then(ticks::parse)?;
        let name = self.marker(k, f::NAME).unwrap_or("");
        Some(format!("{owner}/{}@{at}", key_token(name)))
    }

    /// The timeline's rate, when it has one.
    pub fn rate(&self) -> Option<Rate> {
        self.tl(t::RATE).and_then(ticks::parse_rate)
    }

    /// An item's record start and duration.
    pub fn geom(&self, k: &Key) -> Option<(i64, i64)> {
        self.item(k, f::GEOM).and_then(ticks::parse_pair)
    }

    /// An item's kind (its list value).
    pub fn kind(&self, k: &Key) -> Option<&'a str> {
        self.items
            .iter()
            .find(|(x, _)| x == k)
            .and_then(|(_, v)| v.as_str())
    }

    /// The items of track `track`, in time order: transitions by their cut before the item
    /// that starts there, then by start, ties in list order (the order the file holds them).
    pub fn track_items(&self, track: &Key) -> Vec<&'a Key> {
        let mut v: Vec<(i64, u8, usize, &Key)> = Vec::new();
        for (n, (k, kind)) in self.items.iter().enumerate() {
            if self.item(k, f::TRACK) != Some(track.as_str()) {
                continue;
            }
            let (start, _) = self.geom(k).unwrap_or((0, 0));
            let at = match (
                kind.as_str(),
                self.item(k, f::TR).and_then(ticks::parse_pair),
            ) {
                (Some("transition"), Some((inn, _))) => (start.saturating_add(inn), 0),
                _ => (start, 1),
            };
            v.push((at.0, at.1, n, k));
        }
        if crate::mutant::active("ListOrderTrusted") {
            // planted: the list's order taken as time order
            return v.into_iter().map(|(_, _, _, k)| k).collect();
        }
        v.sort_by_key(|&(at, item, n, _)| (at, item, n));
        v.into_iter().map(|(_, _, _, k)| k).collect()
    }
}
