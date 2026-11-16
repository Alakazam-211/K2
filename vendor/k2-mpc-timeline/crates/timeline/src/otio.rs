//! Reads OTIO JSON (`.otio`) into the model, read-only. Written from the OTIO file format's
//! public description (Apache-2.0); no OTIO code is used.
//!
//! **What it reads.** The root is a `Timeline.1`, or a `SerializableCollection.1` of them
//! (nested collections too). A timeline's `tracks` is a `Stack.1` of `Track.1` (the older
//! name `Sequence.1` too); a track's children are `Clip.1`/`Clip.2`, `Gap.1`,
//! `Transition.1`, and `Stack.1` or `Track.1` (a nest). A stack may also hold a clip, a gap
//! or a nest directly: a layer, read as a track of its own (`O1`, `O2`…) holding that one
//! item from the stack's zero. Markers are `Marker.1` to `Marker.3`;
//! media references `ExternalReference.1`, `MissingReference.1`, `GeneratorReference.1` and
//! `ImageSequenceReference.1`. Times are `RationalTime.1` and `TimeRange.1`.
//!
//! **Refusals.** An object type it doesn't know where it needs to place something (a track's
//! child, a stack's child, the root) is [`ReadError::Unknown`]; a known type at an unknown
//! version is [`ReadError::UnknownVersion`]; a missing field, a negative duration or a
//! transition before its track starts is [`ReadError::Structure`]; a time or rate it can't
//! hold exactly is [`ReadError::Time`]. Inside `effects` and `metadata` anything is carried.
//!
//! **Timecode.** A timeline's `global_start_time` is its first frame's timecode. OTIO's schema
//! has no drop-frame flag (a time is a value and a rate). A timeline is drop-frame only when its
//! `metadata` holds the drop-frame (DF/NDF) entry [`CONVERTER_DF_KEY`] as exactly `DF` and its
//! rate is `30000/1001` or `60000/1001` ([`drop_frame_entry`]); anything else there is ignored,
//! never refused: non-drop. The entry is only read: nothing here writes it.
//!
//! **Placement.** A track's children follow each other; a transition takes no time and sits
//! across the cut, `in_offset` before it and `out_offset` after. A clip's markers are in its
//! media time and placed by its source range. A nest's tracks are in the nest's own time.
//!
//! **Carried.** Each object's non-empty `metadata`, each effect, a track's `source_range`,
//! the inactive media references of a `Clip.2`, and what a collection holds besides
//! timelines: canonical JSON, digested ([`crate::model::Opaque`]).
//!
//! **Keys.** OTIO has no object IDs: every key is derived ([`crate::model`]'s table).

use crate::build::{place, Builder};
use crate::error::{quote, ReadError, ReadLimits};
use crate::json::{self, Json};
use crate::model::{
    key_token, time_token, Document, Format, Item, ItemKind, Key, Marker, MarkerKind, MediaKind,
    Opaque, Timeline, Track, TrackKind, TransitionInfo,
};
use crate::time::{Rate, Time};

/// A third-party converter's key.
pub const CONVERTER_DF_KEY: &str = "fcp_xml/timecode/displayformat";

/// Whether a timeline at `rate` is drop-frame, from the drop-frame (DF/NDF) entry its
/// `metadata` may hold under [`CONVERTER_DF_KEY`] (its parts split at `/`), as `entry` found it:
/// only exactly `DF`, and only at `30000/1001` or `60000/1001` ([`crate::model::drop_frame_at`]).
/// `NDF`, any other value or none, and `DF` at any other rate are non-drop, never an error.
pub fn drop_frame_entry(entry: Option<&str>, rate: Option<Rate>) -> bool {
    crate::model::drop_frame_at(rate, entry == Some("DF"))
}

/// Reads an OTIO JSON document.
pub fn read_otio(bytes: &[u8], limits: &ReadLimits) -> Result<Document, ReadError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ReadError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    let root = json::parse(bytes, limits.max_depth, limits.max_items)?;
    let (name, ver) = schema(&root)?;
    let mut r = Reader {
        b: Builder::new(limits),
        first_rate: None,
    };
    let mut timelines = Vec::new();
    let mut opaque = Vec::new();
    match name {
        "Timeline" => {
            known(name, ver, &[1])?;
            timelines.push(r.timeline(&root, 1)?);
        }
        "SerializableCollection" => {
            known(name, ver, &[1])?;
            r.collection(&root, &mut timelines, &mut opaque, 0)?;
        }
        _ => {
            return Err(ReadError::Unknown {
                what: "OTIO root schema",
                found: quote(name),
            })
        }
    }
    Ok(Document {
        format: Format::Otio,
        version: format!("{name}.{ver}"),
        timelines,
        media: r.b.media,
        opaque,
    })
}

/// A `TimeRange.1`: start, duration, and the duration's rate.
type Span = (Time, Time, Rate);

/// The schema name and version of an OTIO object.
fn schema(v: &Json) -> Result<(&str, u32), ReadError> {
    let s = v
        .get("OTIO_SCHEMA")
        .and_then(Json::as_str)
        .ok_or(ReadError::Structure {
            why: "an object without an OTIO_SCHEMA",
        })?;
    let (name, ver) = s.rsplit_once('.').ok_or(ReadError::Structure {
        why: "an OTIO_SCHEMA without a version",
    })?;
    let ver = ver.parse::<u32>().map_err(|_| ReadError::UnknownVersion {
        what: format!("OTIO schema {}", quote(name)),
        found: quote(ver),
    })?;
    Ok((name, ver))
}

fn known(name: &str, ver: u32, versions: &[u32]) -> Result<(), ReadError> {
    if versions.contains(&ver) {
        Ok(())
    } else {
        Err(ReadError::UnknownVersion {
            what: format!("OTIO schema {}", quote(name)),
            found: ver.to_string(),
        })
    }
}

/// `v` must be the object type `name` at one of `versions`.
fn expect(v: &Json, name: &'static str, versions: &[u32]) -> Result<(), ReadError> {
    let (n, ver) = schema(v)?;
    if n != name {
        return Err(ReadError::Unknown {
            what: "OTIO schema",
            found: quote(n),
        });
    }
    known(n, ver, versions)
}

fn missing(why: &'static str) -> ReadError {
    ReadError::Structure { why }
}

fn name_of(v: &Json) -> &str {
    v.get("name").and_then(Json::as_str).unwrap_or("")
}

/// A `RationalTime.1` as an exact time, with its rate.
fn rational_time(v: &Json) -> Result<(Time, Rate), ReadError> {
    expect(v, "RationalTime", &[1])?;
    let num = |k: &str| match v.get(k) {
        Some(Json::Num(n)) => Ok(n),
        _ => Err(missing("a RationalTime without a numeric value and rate")),
    };
    let rate = Rate::from_f64(num("rate")?.as_f64())?;
    let (n, d) = num("value")?.ratio().ok_or(missing(
        "a time value that is not a finite number this reader holds exactly",
    ))?;
    // value / rate = n * rate.den / (d * rate.num)
    let t = Time::from_i128(
        n.checked_mul(i128::from(rate.den()))
            .ok_or(crate::time::TimeError::Overflow)?,
        d.checked_mul(i128::from(rate.num()))
            .ok_or(crate::time::TimeError::Overflow)?,
    )?;
    Ok((t, rate))
}

/// A `TimeRange.1`: start, duration, and the duration's rate.
fn time_range(v: &Json) -> Result<Span, ReadError> {
    expect(v, "TimeRange", &[1])?;
    let (start, _) = rational_time(
        v.get("start_time")
            .ok_or(missing("a TimeRange without a start"))?,
    )?;
    let (dur, rate) = rational_time(
        v.get("duration")
            .ok_or(missing("a TimeRange without a duration"))?,
    )?;
    if dur.is_negative() {
        return Err(missing("a negative duration"));
    }
    Ok((start, dur, rate))
}

/// A field holding a `TimeRange.1` or null.
fn opt_range(v: &Json, key: &str) -> Result<Option<Span>, ReadError> {
    match v.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(r) => time_range(r).map(Some),
    }
}

/// `v`'s canonical JSON as a carried piece.
fn carry(what: &str, v: &Json) -> Opaque {
    let mut b = Vec::new();
    v.canonical(&mut b);
    Opaque::of(what, &b)
}

/// The object's `metadata`, carried when it isn't empty.
fn metadata(v: &Json, out: &mut Vec<Opaque>) {
    if let Some(m) = v.get("metadata") {
        if !m.is_empty_obj() {
            out.push(carry("otio:metadata", m));
        }
    }
}

/// The object's `effects`: their names, each carried.
fn effects(v: &Json, out: &mut Vec<Opaque>) -> Result<Vec<String>, ReadError> {
    let mut names = Vec::new();
    match v.get("effects") {
        None | Some(Json::Null) => {}
        Some(Json::Arr(a)) => {
            for e in a {
                let (s, _) = schema(e)?;
                let n = e
                    .get("effect_name")
                    .and_then(Json::as_str)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| {
                        let n = name_of(e);
                        if n.is_empty() {
                            s
                        } else {
                            n
                        }
                    });
                names.push(n.to_owned());
                out.push(carry("otio:effect", e));
            }
        }
        Some(_) => return Err(missing("effects that are not a list")),
    }
    Ok(names)
}

fn children(v: &Json) -> Result<&[Json], ReadError> {
    match v.get("children") {
        None | Some(Json::Null) => Ok(&[]),
        Some(c) => c.as_arr().ok_or(missing("children that are not a list")),
    }
}

struct Reader {
    b: Builder,
    /// The first rate a track's item used, for a timeline that states none.
    first_rate: Option<Rate>,
}

impl Reader {
    fn collection(
        &mut self,
        v: &Json,
        timelines: &mut Vec<Timeline>,
        opaque: &mut Vec<Opaque>,
        depth: u32,
    ) -> Result<(), ReadError> {
        self.b.depth(depth)?;
        metadata(v, opaque);
        for c in children(v)? {
            let (n, ver) = schema(c)?;
            match n {
                "Timeline" => {
                    known(n, ver, &[1])?;
                    let ordinal = timelines.len() as u64 + 1;
                    let t = self.timeline(c, ordinal)?;
                    timelines.push(t);
                }
                "SerializableCollection" => {
                    known(n, ver, &[1])?;
                    self.collection(c, timelines, opaque, depth + 1)?;
                }
                "Clip"
                | "Gap"
                | "Stack"
                | "Track"
                | "Sequence"
                | "Transition"
                | "Marker"
                | "ExternalReference"
                | "MissingReference"
                | "GeneratorReference"
                | "ImageSequenceReference"
                | "Effect" => opaque.push(carry("otio:collection-item", c)),
                _ => {
                    return Err(ReadError::Unknown {
                        what: "OTIO schema",
                        found: quote(n),
                    })
                }
            }
        }
        Ok(())
    }

    fn timeline(&mut self, v: &Json, ordinal: u64) -> Result<Timeline, ReadError> {
        self.b.count()?;
        let key = self.b.book.key(None, "d-seq", &ordinal.to_string(), false);
        let mut opaque = Vec::new();
        metadata(v, &mut opaque);
        let (tc_start, tc_rate) = match v.get("global_start_time") {
            None | Some(Json::Null) => (Time::ZERO, None),
            Some(t) => {
                let (t, r) = rational_time(t)?;
                (t, Some(r))
            }
        };
        self.first_rate = None;
        let stack = v
            .get("tracks")
            .ok_or(missing("a timeline without tracks"))?;
        expect(stack, "Stack", &[1])?;
        metadata(stack, &mut opaque);
        effects(stack, &mut opaque)?;
        let origin = match opt_range(stack, "source_range")? {
            Some((s, _, _)) => {
                opaque.push(carry(
                    "otio:stack-source-range",
                    stack.get("source_range").unwrap_or(&Json::Null),
                ));
                s
            }
            None => Time::ZERO,
        };
        let tracks = self.stack_tracks(stack, &key, 1)?;
        let markers = self.markers(stack, &key, Time::ZERO, origin, false)?;
        let rate = tc_rate.or(self.first_rate);
        let entry = v
            .get("metadata")
            .and_then(|m| CONVERTER_DF_KEY.split('/').try_fold(m, |n, k| n.get(k)))
            .and_then(Json::as_str);
        Ok(Timeline {
            key,
            name: name_of(v).to_owned(),
            rate,
            tc_start,
            drop_frame: drop_frame_entry(entry, rate),
            width: None,
            height: None,
            audio_rate: None,
            duration: None,
            compound: false,
            tracks,
            markers,
            opaque,
        })
    }

    /// A `Track.1` (or `Sequence.1`) under `scope`; `counts` numbers tracks per kind.
    fn track(
        &mut self,
        v: &Json,
        scope: &Key,
        counts: &mut [u32; 4],
        depth: u32,
    ) -> Result<Track, ReadError> {
        self.b.depth(depth)?;
        self.b.count()?;
        let (n, ver) = schema(v)?;
        known(n, ver, &[1])?;
        let kind = match v.get("kind").and_then(Json::as_str) {
            Some("Video") => TrackKind::Video,
            Some("Audio") => TrackKind::Audio,
            _ => TrackKind::Other,
        };
        let slot = match kind {
            TrackKind::Video => 0,
            TrackKind::Audio => 1,
            TrackKind::Caption => 2,
            _ => 3,
        };
        counts[slot] += 1;
        let label = format!("{}{}", kind.letter(), counts[slot]);
        let key = self.b.book.key(Some(scope), "d-trk", &label, false);
        let name = match name_of(v) {
            "" => label,
            n => n.to_owned(),
        };
        let mut opaque = Vec::new();
        metadata(v, &mut opaque);
        effects(v, &mut opaque)?;
        if let Some(r) = v.get("source_range").filter(|r| !r.is_null()) {
            time_range(r)?;
            opaque.push(carry("otio:track-source-range", r));
        }
        let items = self.items(children(v)?, &key, depth)?;
        let markers = self.markers(v, &key, Time::ZERO, Time::ZERO, false)?;
        Ok(Track {
            key,
            name,
            kind,
            muted: !v.get("enabled").and_then(Json::as_bool).unwrap_or(true),
            locked: false,
            items,
            markers,
            opaque,
        })
    }

    /// A track's children, one after another from zero (a transition takes no time and sits
    /// across its cut), sorted by start.
    fn items(&mut self, children: &[Json], key: &Key, depth: u32) -> Result<Vec<Item>, ReadError> {
        let mut items = Vec::new();
        let mut pos = Time::ZERO;
        for c in children {
            self.b.count()?;
            let (cn, cv) = schema(c)?;
            match cn {
                "Clip" => {
                    known(cn, cv, &[1, 2])?;
                    let it = self.clip(c, key, pos)?;
                    pos = pos.checked_add(it.duration)?;
                    items.push(it);
                }
                "Gap" => {
                    known(cn, cv, &[1])?;
                    let (src, dur, rate) = opt_range(c, "source_range")?
                        .ok_or(missing("a gap without a source range"))?;
                    self.first_rate.get_or_insert(rate);
                    let gk = self.b.book.key(
                        Some(key),
                        "d-gap",
                        &format!("@{}", time_token(pos)),
                        false,
                    );
                    let mut it = Item::new(gk, ItemKind::Gap, name_of(c), pos, dur);
                    it.source_in = Some(src);
                    it.effects = effects(c, &mut it.opaque)?;
                    metadata(c, &mut it.opaque);
                    it.markers = self.markers(c, &it.key, pos, src, true)?;
                    it.enabled = c.get("enabled").and_then(Json::as_bool).unwrap_or(true);
                    pos = pos.checked_add(dur)?;
                    items.push(it);
                }
                "Transition" => {
                    known(cn, cv, &[1])?;
                    let off = |k: &str| -> Result<Time, ReadError> {
                        let (t, _) = rational_time(
                            c.get(k)
                                .ok_or(missing("a transition without its offsets"))?,
                        )?;
                        if t.is_negative() {
                            return Err(missing("a transition with a negative offset"));
                        }
                        Ok(t)
                    };
                    let (inn, out) = (off("in_offset")?, off("out_offset")?);
                    let start = pos.checked_sub(inn)?;
                    let tk = self.b.book.key(
                        Some(key),
                        "d-tr",
                        &format!("@{}", time_token(start)),
                        false,
                    );
                    let mut it = Item::new(
                        tk,
                        ItemKind::Transition,
                        name_of(c),
                        start,
                        inn.checked_add(out)?,
                    );
                    it.transition = Some(TransitionInfo {
                        kind: c
                            .get("transition_type")
                            .and_then(Json::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        in_offset: Some(inn),
                    });
                    metadata(c, &mut it.opaque);
                    if let Some(p) = c.get("parameters").filter(|p| !p.is_empty_obj()) {
                        it.opaque.push(carry("otio:transition-parameters", p));
                    }
                    items.push(it);
                }
                "Stack" | "Track" | "Sequence" => {
                    known(cn, cv, &[1])?;
                    let it = self.nest(c, cn, key, pos, depth)?;
                    pos = pos.checked_add(it.duration)?;
                    items.push(it);
                }
                _ => {
                    return Err(ReadError::Unknown {
                        what: "OTIO schema",
                        found: quote(cn),
                    })
                }
            }
        }
        // a transition on the first cut may start before the track does: kept, as written
        items.sort_by_key(|a| a.start);
        Ok(items)
    }

    /// A stack's children: its tracks, and any other child as a layer, a track of its own
    /// holding that one item from the stack's zero.
    fn stack_tracks(
        &mut self,
        stack: &Json,
        scope: &Key,
        depth: u32,
    ) -> Result<Vec<Track>, ReadError> {
        let mut tracks = Vec::new();
        let mut counts = [0u32; 4];
        for c in children(stack)? {
            let (n, _) = schema(c)?;
            match n {
                "Track" | "Sequence" => tracks.push(self.track(c, scope, &mut counts, depth)?),
                "Transition" => return Err(missing("a transition outside a track")),
                _ => {
                    self.b.depth(depth)?;
                    self.b.count()?;
                    counts[3] += 1;
                    let label = format!("{}{}", TrackKind::Other.letter(), counts[3]);
                    let key = self.b.book.key(Some(scope), "d-trk", &label, false);
                    let items = self.items(std::slice::from_ref(c), &key, depth)?;
                    let mut t = Track::new(key, label, TrackKind::Other);
                    t.items = items;
                    tracks.push(t);
                }
            }
        }
        Ok(tracks)
    }

    /// A media reference: its media key, and its available range.
    fn media(&mut self, r: &Json) -> Result<(Key, Option<Span>), ReadError> {
        let (n, ver) = schema(r)?;
        let range = opt_range(r, "available_range")?;
        let (kind, location) = match n {
            "ExternalReference" => {
                known(n, ver, &[1])?;
                (
                    MediaKind::File,
                    r.get("target_url")
                        .and_then(Json::as_str)
                        .map(str::to_owned),
                )
            }
            "ImageSequenceReference" => {
                known(n, ver, &[1])?;
                let base = r
                    .get("target_url_base")
                    .and_then(Json::as_str)
                    .unwrap_or("");
                let prefix = r.get("name_prefix").and_then(Json::as_str).unwrap_or("");
                (MediaKind::File, Some(format!("{base}{prefix}")))
            }
            "MissingReference" => {
                known(n, ver, &[1])?;
                (MediaKind::Missing, None)
            }
            "GeneratorReference" => {
                known(n, ver, &[1])?;
                (MediaKind::Generator, None)
            }
            _ => (MediaKind::Other, None),
        };
        let mut name = name_of(r).to_owned();
        if name.is_empty() {
            name = match (&location, kind) {
                (Some(l), _) => l.rsplit(['/', '\\']).next().unwrap_or("").to_owned(),
                (None, MediaKind::Generator) => r
                    .get("generator_kind")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_owned(),
                _ => String::new(),
            };
        }
        let mut canon = Vec::new();
        r.canonical(&mut canon);
        // the same location is the same media; a reference without one is itself
        let identity = match &location {
            Some(l) => format!("{}\0{l}", kind.name()),
            None => format!("{}\0{}", kind.name(), String::from_utf8_lossy(&canon)),
        };
        let meta = r
            .get("metadata")
            .filter(|m| !m.is_empty_obj())
            .map(|m| carry("otio:metadata", m));
        let params = r
            .get("parameters")
            .filter(|p| !p.is_empty_obj())
            .map(|p| carry("otio:generator-parameters", p));
        let unknown = (kind == MediaKind::Other).then(|| carry("otio:media-reference", r));
        let key = self.b.media(identity, None, &name, kind, |m| {
            m.path = location;
            if let Some((s, d, rate)) = range {
                m.start = Some(s);
                m.duration = Some(d);
                m.rate = Some(rate);
            }
            m.opaque.extend(meta);
            m.opaque.extend(params);
            m.opaque.extend(unknown);
        })?;
        Ok((key, range))
    }

    fn clip(&mut self, c: &Json, scope: &Key, pos: Time) -> Result<Item, ReadError> {
        let (_, ver) = schema(c)?;
        let mut opaque = Vec::new();
        let reference = if ver == 1 {
            c.get("media_reference").filter(|r| !r.is_null())
        } else {
            let active = c
                .get("active_media_reference_key")
                .and_then(Json::as_str)
                .unwrap_or("DEFAULT_MEDIA");
            match c.get("media_references") {
                Some(Json::Obj(kv)) => {
                    for (k, r) in kv {
                        if k != active {
                            opaque.push(carry("otio:inactive-media-reference", r));
                        }
                    }
                    kv.iter()
                        .find(|(k, _)| k == active)
                        .map(|(_, r)| r)
                        .filter(|r| !r.is_null())
                }
                None | Some(Json::Null) => None,
                Some(_) => return Err(missing("media references that are not an object")),
            }
        };
        let media = match reference {
            Some(r) => Some(self.media(r)?),
            None => None,
        };
        let range = opt_range(c, "source_range")?;
        let available = media.as_ref().and_then(|(_, a)| *a);
        let (src, dur, rate) = range
            .or(available)
            .ok_or(missing("a clip without a duration"))?;
        self.first_rate.get_or_insert(rate);
        let what = match &media {
            Some((k, _)) => {
                let m = self
                    .b
                    .media
                    .iter()
                    .find(|m| &m.key == k)
                    .map(|m| m.name.clone());
                m.unwrap_or_default()
            }
            None => String::new(),
        };
        let what = if what.is_empty() {
            name_of(c).to_owned()
        } else {
            what
        };
        let ck = self.b.book.key(
            Some(scope),
            "d-clip",
            &format!("{}@{}", key_token(&what), time_token(src)),
            false,
        );
        let mut it = Item::new(ck, ItemKind::Clip, name_of(c), pos, dur);
        it.source_in = Some(src);
        it.media = media.map(|(k, _)| k);
        if let Some(k) = &it.media {
            if self
                .b
                .media
                .iter()
                .any(|m| &m.key == k && m.kind == MediaKind::Generator)
            {
                it.kind = ItemKind::Generator;
            }
        }
        it.enabled = c.get("enabled").and_then(Json::as_bool).unwrap_or(true);
        it.effects = effects(c, &mut opaque)?;
        metadata(c, &mut opaque);
        it.opaque = opaque;
        it.markers = self.markers(c, &it.key, pos, src, true)?;
        Ok(it)
    }

    /// A `Stack.1` or `Track.1` inside a track: a nest of tracks in its own time.
    fn nest(
        &mut self,
        c: &Json,
        schema_name: &str,
        scope: &Key,
        pos: Time,
        depth: u32,
    ) -> Result<Item, ReadError> {
        let range = opt_range(c, "source_range")?;
        let src = range.map_or(Time::ZERO, |(s, _, _)| s);
        let nk = self.b.book.key(
            Some(scope),
            "d-nest",
            &format!("{}@{}", key_token(name_of(c)), time_token(src)),
            false,
        );
        let tracks = if schema_name == "Stack" {
            self.stack_tracks(c, &nk, depth + 1)?
        } else {
            let mut counts = [0u32; 4];
            vec![self.track(c, &nk, &mut counts, depth + 1)?]
        };
        let dur = match range {
            Some((_, d, _)) => d,
            None => {
                let mut end = Time::ZERO;
                for t in &tracks {
                    for i in &t.items {
                        end = end.max(i.end()?);
                    }
                }
                end
            }
        };
        let mut it = Item::new(nk, ItemKind::Nest, name_of(c), pos, dur);
        it.source_in = Some(src);
        it.tracks = tracks;
        it.enabled = c.get("enabled").and_then(Json::as_bool).unwrap_or(true);
        if schema_name == "Stack" {
            it.effects = effects(c, &mut it.opaque)?;
            metadata(c, &mut it.opaque);
            it.markers = self.markers(c, &it.key, pos, src, true)?;
        }
        Ok(it)
    }

    /// `v`'s markers, in its own time: `at = start + (marked - origin)`; `on_item` keeps the
    /// marked time as the media time.
    fn markers(
        &mut self,
        v: &Json,
        owner: &Key,
        start: Time,
        origin: Time,
        on_item: bool,
    ) -> Result<Vec<Marker>, ReadError> {
        let list = match v.get("markers") {
            None | Some(Json::Null) => return Ok(Vec::new()),
            Some(Json::Arr(a)) => a,
            Some(_) => return Err(missing("markers that are not a list")),
        };
        let mut out = Vec::new();
        for m in list {
            self.b.count()?;
            let (n, ver) = schema(m)?;
            if n != "Marker" {
                return Err(ReadError::Unknown {
                    what: "OTIO schema",
                    found: quote(n),
                });
            }
            known(n, ver, &[1, 2, 3])?;
            let range = if ver == 1 {
                m.get("range")
            } else {
                m.get("marked_range")
            };
            let (s, d, _) = time_range(range.ok_or(missing("a marker without a range"))?)?;
            let at = place(start, s, origin)?;
            let mk = self.b.book.key(
                Some(owner),
                "d-mk",
                &format!("{}@{}", key_token(name_of(m)), time_token(s)),
                false,
            );
            let mut note = m.get("comment").and_then(Json::as_str).map(str::to_owned);
            if note.as_deref() == Some("") {
                note = None;
            }
            out.push(Marker {
                key: mk,
                kind: MarkerKind::Marker,
                name: name_of(m).to_owned(),
                note,
                at,
                duration: d,
                media_at: on_item.then_some(s),
                color: m
                    .get("color")
                    .and_then(Json::as_str)
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned),
            });
        }
        Ok(out)
    }
}
