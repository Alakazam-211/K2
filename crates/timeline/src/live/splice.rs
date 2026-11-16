//! The live content written back into the OTIO file ([`encode_from_paired`]): the carrier the
//! kit hands over is read with every object's byte range, the carrier's pairing puts the live
//! keys on its tracks, items, markers and media in the order the read yields them, that keyed
//! carrier is diffed against the live content by key, and only what differs is rewritten.
//!
//! - **Every value comes from the carrier.** The pairing holds keys only; a pairing whose count
//!   differs from the carrier's units is [`SpliceError::PairingMismatch`] (the kit's
//!   `MPC-E521`), and nothing is spliced.
//! - **What is rewritten:** a clip's `source_range` start and duration values, its `name` and
//!   `enabled`; the children of a track whose items moved, were added or removed (each item's
//!   own bytes kept, its gaps recomputed from the absolute starts, a gap whose neighbours and
//!   length didn't change kept byte for byte, one whose length did resized in place); a markers
//!   array whose markers changed; a track's `name`; and **our stamp** in each touched item's
//!   `metadata` (`"mpc": {"key": …, "doc": …}`), so the next outside save pairs it by key.
//! - **Times** are written at the item's own rate with **the rate token copied byte for byte**
//!   (`29.97` stays `29.97`) and values in the file's number style (`86.0` where it writes
//!   `86.0`). A value no finite decimal holds is refused.
//! - **Untouched is byte for byte:** with no difference, the carrier is returned as it is.
//! - **Read back once:** the result must read as the live content under the new pairing, else
//!   it is refused and the file is left as it was. The carrier and the result are read by the
//!   live reader alone ([`model::read_live`]): the tier-0 reader read the file when it opened.
//! - Anything else that differs (a track added, a media changed, a transition or nest edited) is
//!   a [`PackError`]: never written by guess.

use std::collections::{BTreeMap, HashMap, HashSet};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::session::{PackError, Pairing, SpliceError};

use super::model::{
    self, active_ref, f, t, Child, Keys, LiveLimits, LiveTimeline, Store, ITEMS, MARKERS, MEDIA,
    STAMP, TIMELINE, TRACKS,
};
use super::ticks;
use crate::json::{quote_str, Node};
use crate::model::{Item, ItemKind, KeyOrigin, Step, Track, TrackKind, TransitionInfo};
use crate::mutant;
use crate::time::Rate;

fn err(m: impl Into<String>) -> SpliceError {
    SpliceError::Pack(PackError(m.into()))
}

/// One replacement of the file's bytes `start..end` (an insertion when they are equal).
type Rep = (usize, usize, String);

/// The keys the carrier's units take: the pairing's, checked against the carrier's counts.
fn paired_keys(lt: &LiveTimeline, pairing: &Pairing) -> Result<Keys, SpliceError> {
    let get = |root: &str, want: usize| -> Result<Vec<Key>, SpliceError> {
        let k = pairing
            .keys(root)
            .ok_or_else(|| SpliceError::PairingMismatch(format!("no keys for `{root}`")))?;
        if k.len() != want {
            return Err(SpliceError::PairingMismatch(format!(
                "{} keys of `{root}` for the file's {want}",
                k.len()
            )));
        }
        Ok(k.to_vec())
    };
    let keys = Keys {
        tracks: get(TRACKS, lt.tracks.len())?,
        items: get(ITEMS, lt.items.len())?,
        markers: get(MARKERS, lt.markers.len())?,
        media: get(MEDIA, lt.media.len())?,
    };
    if mutant::active("PairByPosition") {
        // planted: the pairing ignored, the keys an import would mint
        return Ok(lt.minted().0);
    }
    Ok(keys)
}

/// The pairing of `content` as the file it renders to holds it: tracks in their order; each
/// track's items in time order; markers by owner in that order (each item's, then the track's,
/// then the timeline's); media in the order the items first use them.
pub fn pairing_of(content: &[(String, Neutral)]) -> Pairing {
    let s = Store::of_content(content);
    let mut items = Vec::new();
    let mut markers = Vec::new();
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    let mut by_owner: HashMap<&str, Vec<&Key>> = HashMap::new();
    for (k, _) in s.markers {
        by_owner
            .entry(s.marker(k, f::OWNER).unwrap_or(""))
            .or_default()
            .push(k);
    }
    let owned = |owner: &str, out: &mut Vec<Key>| {
        out.extend(
            by_owner
                .get(owner)
                .into_iter()
                .flatten()
                .map(|k| (*k).clone()),
        );
    };
    for (tk, _) in s.tracks {
        for ik in s.track_items(tk) {
            items.push(ik.clone());
            owned(ik.as_str(), &mut markers);
            if let Some(m) = s.item(ik, f::MEDIA) {
                if seen.insert(m) {
                    if let Ok(k) = Key::new(m) {
                        media.push(k);
                    }
                }
            }
        }
        owned(tk.as_str(), &mut markers);
    }
    owned("", &mut markers);
    Pairing {
        roots: vec![
            (
                TRACKS.to_owned(),
                s.tracks.iter().map(|(k, _)| k.clone()).collect(),
            ),
            (ITEMS.to_owned(), items),
            (MARKERS.to_owned(), markers),
            (MEDIA.to_owned(), media),
        ],
    }
}

/// The marks and doc id `content` holds (store-only: no file has them).
fn store_only(content: &[(String, Neutral)]) -> (model::Marks, String) {
    let s = Store::of_content(content);
    let n = |k: &str| s.tl(k).and_then(|v| v.parse().ok()).unwrap_or(0);
    (
        [n(t::MARK_T), n(t::MARK_I), n(t::MARK_K), n(t::MARK_M)],
        s.tl(t::DOC).unwrap_or("").to_owned(),
    )
}

/// Why two contents differ (lists compared by key, not order; the timeline's store-only and
/// derived fields left out), or `Ok`.
pub fn same_content(a: &[(String, Neutral)], b: &[(String, Neutral)]) -> Result<(), String> {
    let skip_tl = [t::DOC, t::MARK_T, t::MARK_I, t::MARK_K, t::MARK_M, t::RATE];
    let norm = |c: &[(String, Neutral)]| -> BTreeMap<String, BTreeMap<String, Value>> {
        c.iter()
            .map(|(r, n)| {
                let m: BTreeMap<String, Value> = match n {
                    Neutral::List(l) => l
                        .iter()
                        .map(|(k, v)| (k.as_str().to_owned(), v.clone()))
                        .collect(),
                    Neutral::Map(m) if r == TIMELINE => m
                        .iter()
                        .filter(|(k, _)| !skip_tl.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    Neutral::Map(m) => m.clone(),
                    _ => BTreeMap::new(),
                };
                (r.clone(), m)
            })
            .collect()
    };
    let (x, y) = (norm(a), norm(b));
    for (root, m) in &x {
        let other = y.get(root).cloned().unwrap_or_default();
        for (k, v) in m {
            if other.get(k) != Some(v) {
                return Err(format!(
                    "`{root}` {}: {:?} and {:?}",
                    k.replace('\x1f', " "),
                    v.as_str().unwrap_or(""),
                    other.get(k).and_then(Value::as_str)
                ));
            }
        }
        if let Some(k) = other.keys().find(|k| !m.contains_key(*k)) {
            return Err(format!(
                "`{root}` {}: only one side holds it",
                k.replace('\x1f', " ")
            ));
        }
    }
    Ok(())
}

/// Field by field, how the unit `k`'s fields in the map `m` differ from `fields` (the file's):
/// the names of the fields that differ, `*` when the map holds a field the file doesn't.
fn differing(
    fields: &[(&'static str, String)],
    m: &BTreeMap<String, Value>,
    k: &Key,
) -> Vec<String> {
    let start = format!("{}\x1f", k.as_str());
    let mut out = Vec::new();
    let mut seen = 0;
    for (key, v) in m.range(start.clone()..) {
        let Some(fld) = key.strip_prefix(&start) else {
            break;
        };
        seen += 1;
        match fields.iter().find(|(x, _)| *x == fld) {
            Some((_, x)) if Some(x.as_str()) == v.as_str() => {}
            Some(_) => out.push(fld.to_owned()),
            None => out.push("*".to_owned()),
        }
    }
    if seen != fields.len() {
        for (x, _) in fields {
            if !m.contains_key(&format!("{start}{x}")) {
                out.push((*x).to_owned());
            }
        }
    }
    out
}

/// What differs between the carrier (under its keys) and the live content, by key: the file
/// units whose fields or kind changed, and whether any unit was added or removed or the
/// timeline's own fields or the media changed.
pub struct Diff<'k> {
    /// File items whose fields or kind differ.
    pub items: HashSet<&'k Key>,
    /// File markers whose fields differ.
    pub markers: HashSet<&'k Key>,
    /// File tracks whose fields differ.
    pub tracks: HashSet<&'k Key>,
    /// A unit added or removed, a media changed, or a timeline field changed.
    pub other: bool,
}

impl<'k> Diff<'k> {
    /// The carrier `lt` under `keys` against the live store `new`.
    pub fn of(lt: &LiveTimeline, keys: &'k Keys, new: &Store<'_>) -> Diff<'k> {
        let kinds = |l: &[(Key, Value)]| -> HashMap<Key, Option<String>> {
            l.iter()
                .map(|(k, v)| (k.clone(), v.as_str().map(str::to_owned)))
                .collect()
        };
        let (ni, nk, nt, nm) = (
            kinds(new.items),
            kinds(new.markers),
            kinds(new.tracks),
            kinds(new.media),
        );
        let mut d = Diff {
            items: HashSet::new(),
            markers: HashSet::new(),
            tracks: HashSet::new(),
            other: ni.len() != keys.items.len()
                || nk.len() != keys.markers.len()
                || nt.len() != keys.tracks.len()
                || nm.len() != keys.media.len(),
        };
        for (n, k) in keys.items.iter().enumerate() {
            match ni.get(k) {
                None => d.other = true,
                Some(kind) => {
                    let diffs = differing(&lt.item_fields(n, keys), new.ip, k);
                    let stale = mutant::active("DiffStaleUnit") && diffs == ["name"];
                    if (kind.as_deref() != Some(lt.items[n].kind) || !diffs.is_empty()) && !stale {
                        d.items.insert(k);
                    }
                }
            }
        }
        for (n, k) in keys.markers.iter().enumerate() {
            match nk.get(k) {
                None => d.other = true,
                Some(_) => {
                    if !differing(&lt.marker_fields(n, keys), new.kp, k).is_empty() {
                        d.markers.insert(k);
                    }
                }
            }
        }
        for (n, k) in keys.tracks.iter().enumerate() {
            match nt.get(k) {
                Some(kind) if kind.as_deref() == Some(lt.tracks[n].kind) => {
                    if !differing(&lt.track_fields(n), new.tp, k).is_empty() {
                        d.tracks.insert(k);
                    }
                }
                _ => d.other = true,
            }
        }
        for (n, k) in keys.media.iter().enumerate() {
            match nm.get(k) {
                Some(kind) if kind.as_deref() == Some(lt.media[n].kind) => {
                    d.other |= !differing(&lt.media_fields(n), new.mp, k).is_empty();
                }
                _ => d.other = true,
            }
        }
        d.other |= lt
            .timeline_fields()
            .iter()
            .any(|(x, v)| *x != t::RATE && new.tl(x) != Some(v.as_str()));
        d
    }

    /// Nothing differs: the carrier is the file, as it is.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.markers.is_empty() && self.tracks.is_empty() && !self.other
    }
}

/// The live content spliced into `carrier`, whose units carry `pairing`'s keys.
pub fn encode_from_paired(
    carrier: &[u8],
    pairing: &Pairing,
    content: &[(String, Neutral)],
    limits: &LiveLimits,
) -> Result<Vec<u8>, SpliceError> {
    let lt = model::read_live(carrier, limits)
        .map_err(|d| err(format!("the carrier doesn't read live: {d}")))?;
    let keys = paired_keys(&lt, pairing)?;
    let (_, doc) = store_only(content);
    if mutant::active("EncodeReserialises") {
        // planted: the whole file written again from what was read
        let mut out = Vec::new();
        lt.root.to_json().canonical(&mut out);
        return Ok(out);
    }
    let new = Store::of_content(content);
    let diff = Diff::of(&lt, &keys, &new);
    if diff.is_empty() {
        return Ok(carrier.to_vec());
    }
    let sp = Splice::new(carrier, &lt, &keys, &diff, new, doc)?;
    let (reps, rewritten) = sp.reps()?;
    let out = apply(carrier, 0, &reps).map_err(err)?;
    if mutant::active("ReadBackSkipped") {
        // planted: the result written unread
        return Ok(out);
    }
    // read back once: the whole result parsed and walked, its units keyed by the content's own
    // pairing, and every unit the splice touched compared with the live content
    let back = model::read_live(&out, limits)
        .map_err(|d| err(format!("the written file doesn't read live: {d}")))?;
    let bk = paired_keys(&back, &pairing_of(content)).map_err(|e| {
        err(format!(
            "the written file doesn't hold the live content's units: {e}"
        ))
    })?;
    read_back(&back, &bk, &new, &keys, &diff, &rewritten).map_err(|d| {
        err(format!(
            "the written file doesn't read back as the live content: {d}"
        ))
    })?;
    Ok(out)
}

/// The touched units of the written file `back` (under `bk`) against the live store: every item
/// the diff found changed or new, every item on a track whose children were rewritten (their
/// gaps place them), every marker changed, new or owned by such an item, every track changed or
/// rewritten, every media, and the timeline's fields. Untouched units' bytes are the carrier's.
fn read_back(
    back: &LiveTimeline,
    bk: &Keys,
    new: &Store<'_>,
    keys: &Keys,
    diff: &Diff<'_>,
    rewritten: &HashSet<Key>,
) -> Result<(), String> {
    let at = |l: &[Key]| -> HashMap<Key, usize> {
        l.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect()
    };
    let (bi, bm, bt) = (at(&bk.items), at(&bk.markers), at(&bk.tracks));
    let was: HashSet<&Key> = keys.items.iter().collect();
    let was_mk: HashSet<&Key> = keys.markers.iter().collect();
    let show =
        |root: &str, k: &Key, d: &[String]| format!("`{root}` {}: {}", k.as_str(), d.join(", "));
    let mut items_touched: HashSet<&Key> = HashSet::new();
    for (k, kind) in new.items {
        let on_rewritten = new
            .item(k, f::TRACK)
            .is_some_and(|tk| rewritten.iter().any(|r| r.as_str() == tk));
        if !was.contains(k) || diff.items.contains(k) || on_rewritten {
            items_touched.insert(k);
            let n = *bi
                .get(k)
                .ok_or_else(|| format!("item {} is missing", k.as_str()))?;
            if kind.as_str() != Some(back.items[n].kind) {
                return Err(format!("item {}: its kind", k.as_str()));
            }
            let d = differing(&back.item_fields(n, bk), new.ip, k);
            if !d.is_empty() {
                return Err(show(super::ITEM_PROPS, k, &d));
            }
        }
    }
    for (k, _) in new.markers {
        let owner = new.marker(k, f::OWNER).unwrap_or("");
        let owned = items_touched.iter().any(|i| i.as_str() == owner);
        if !was_mk.contains(k) || diff.markers.contains(k) || owned {
            let n = *bm
                .get(k)
                .ok_or_else(|| format!("marker {} is missing", k.as_str()))?;
            let d = differing(&back.marker_fields(n, bk), new.kp, k);
            if !d.is_empty() {
                return Err(show(super::MARKER_PROPS, k, &d));
            }
        }
    }
    for (k, _) in new.tracks {
        if diff.tracks.contains(k) || rewritten.contains(k) {
            let n = *bt
                .get(k)
                .ok_or_else(|| format!("track {} is missing", k.as_str()))?;
            let d = differing(&back.track_fields(n), new.tp, k);
            if !d.is_empty() {
                return Err(show(super::TRACK_PROPS, k, &d));
            }
        }
    }
    for (n, k) in bk.media.iter().enumerate() {
        let d = differing(&back.media_fields(n), new.mp, k);
        if !d.is_empty() {
            return Err(show(super::MEDIA_PROPS, k, &d));
        }
    }
    for (x, v) in back.timeline_fields() {
        if x != t::RATE && new.tl(x) != Some(v.as_str()) {
            return Err(format!("`{TIMELINE}` {x}"));
        }
    }
    Ok(())
}

/// `reps` applied to `bytes`, which start at file offset `base`.
fn apply(bytes: &[u8], base: usize, reps: &[Rep]) -> Result<Vec<u8>, String> {
    let mut sorted: Vec<&Rep> = reps.iter().collect();
    sorted.sort_by_key(|r| (r.0, r.1));
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = base;
    for (s, e, text) in sorted {
        if *s < at || *e < *s || *e > base + bytes.len() {
            return Err("two changes to the file overlap".into());
        }
        out.extend_from_slice(&bytes[at - base..s - base]);
        out.extend_from_slice(text.as_bytes());
        at = *e;
    }
    out.extend_from_slice(&bytes[at - base..]);
    Ok(out)
}

fn slice(b: &[u8], n: &Node) -> String {
    String::from_utf8_lossy(&b[n.start..n.end]).into_owned()
}

/// Set `key` of the object `obj` to the JSON text `value`: its value replaced, or a new entry
/// after the object's `{`, in the object's own spacing.
fn set_entry(b: &[u8], obj: &Node, key: &str, value: String, reps: &mut Vec<Rep>) {
    let entries = obj.entries().unwrap_or(&[]);
    if let Some((_, _, v)) = entries.iter().find(|(k, _, _)| k == key) {
        reps.push((v.start, v.end, value));
        return;
    }
    let text = match entries.first() {
        Some((_, at, _)) => {
            let ws = String::from_utf8_lossy(&b[obj.start + 1..*at]);
            format!("{ws}{}: {value},", quote_str(key))
        }
        None => format!("{}: {value}", quote_str(key)),
    };
    reps.push((obj.start + 1, obj.start + 1, text));
}

/// An array of `elems`, in the spacing of `arr` (the array it replaces) when there is one.
fn array(b: &[u8], arr: Option<&Node>, elems: &[String]) -> String {
    if elems.is_empty() {
        return "[]".into();
    }
    let items = arr.and_then(Node::as_arr).unwrap_or(&[]);
    let (lead, sep, trail) = match (arr, items) {
        (Some(a), [one]) => {
            let lead = String::from_utf8_lossy(&b[a.start + 1..one.start]).into_owned();
            let trail = String::from_utf8_lossy(&b[one.end..a.end - 1]).into_owned();
            (lead.clone(), format!(",{lead}"), trail)
        }
        (Some(a), [first, second, ..]) => (
            String::from_utf8_lossy(&b[a.start + 1..first.start]).into_owned(),
            String::from_utf8_lossy(&b[first.end..second.start]).into_owned(),
            String::from_utf8_lossy(&b[items[items.len() - 1].end..a.end - 1]).into_owned(),
        ),
        _ => (String::new(), ", ".to_owned(), String::new()),
    };
    format!("[{lead}{}{trail}]", elems.join(&sep))
}

/// A rate token as the file wrote it, and the rate it names.
fn rate_of(tok: &str) -> Option<Rate> {
    Rate::from_f64(tok.parse::<f64>().ok()?).ok()
}

/// A RationalTime's value set to `ticks`, at its own rate token.
fn set_time(rt: &Node, value_ticks: i64, reps: &mut Vec<Rep>) -> Result<(), String> {
    let v = rt.get("value").ok_or("a time without a value")?;
    let r = rt.get("rate").ok_or("a time without a rate")?;
    let tok = r.as_num().ok_or("a rate that isn't a number")?.text();
    let rate = rate_of(tok).ok_or("a rate it can't name")?;
    let like = v.as_num().map_or("0", |n| n.text());
    let text = ticks::value_text(value_ticks, rate, like)
        .ok_or("a time between the frames its rate can write")?;
    reps.push((v.start, v.end, text));
    if mutant::active("RateTokenRewritten") {
        // planted: the rate written as the exact number it names, not as the file wrote it
        reps.push((r.start, r.end, format!("{}", rate.as_f64())));
    }
    Ok(())
}

/// A `TimeRange.1` at the rate token `tok`, values in the style of `like`.
fn time_range(start: i64, dur: i64, tok: &str, like: &str) -> Result<String, String> {
    let rate = rate_of(tok).ok_or("a rate it can't name")?;
    let v = |x: i64| {
        ticks::value_text(x, rate, like).ok_or("a time between the frames its rate can write")
    };
    Ok(format!(
        "{{\"OTIO_SCHEMA\": \"TimeRange.1\", \"duration\": {{\"OTIO_SCHEMA\": \"RationalTime.1\", \
         \"rate\": {tok}, \"value\": {}}}, \"start_time\": {{\"OTIO_SCHEMA\": \"RationalTime.1\", \
         \"rate\": {tok}, \"value\": {}}}}}",
        v(dur)?,
        v(start)?
    ))
}

/// The rate token and value style of a clip's duration: its source range's, else its media's
/// available range's.
fn clip_rate(c: &Node) -> Option<(String, String)> {
    let r = c.get("source_range").filter(|r| !r.is_null()).or_else(|| {
        active_ref(c)?
            .get("available_range")
            .filter(|r| !r.is_null())
    })?;
    let d = r.get("duration")?;
    Some((
        d.get("rate")?.as_num()?.text().to_owned(),
        d.get("value")?.as_num()?.text().to_owned(),
    ))
}

fn tok(kind: &str) -> String {
    format!("\"OTIO_SCHEMA\": \"{kind}\"")
}

struct Splice<'a> {
    b: &'a [u8],
    lt: &'a LiveTimeline,
    keys: &'a Keys,
    diff: &'a Diff<'a>,
    new: Store<'a>,
    /// The live items' kinds and keys, looked up once.
    kinds: HashMap<&'a Key, &'a str>,
    new_items: HashSet<&'a Key>,
    doc: String,
    /// The file items' keys, by index.
    ikeys: &'a [Key],
    item_at: HashMap<&'a Key, usize>,
    marker_at: HashMap<&'a Key, usize>,
    media_at: HashMap<&'a Key, usize>,
    /// Each item's rate token and value style (carrier items; new clips take their media's).
    rates: HashMap<Key, (String, String)>,
    /// Each owner's markers, in order: the file's, and the live content's.
    old_owned: HashMap<String, Vec<Key>>,
    new_owned: HashMap<String, Vec<Key>>,
}

impl<'a> Splice<'a> {
    fn new(
        b: &'a [u8],
        lt: &'a LiveTimeline,
        keys: &'a Keys,
        diff: &'a Diff<'a>,
        new: Store<'a>,
        doc: String,
    ) -> Result<Self, SpliceError> {
        // the tracks, their kinds and their state are fixed in live v1; so are the media
        let same_tracks = new.tracks.len() == keys.tracks.len()
            && new
                .tracks
                .iter()
                .zip(&keys.tracks)
                .zip(&lt.tracks)
                .all(|(((k, v), x), tr)| k == x && v.as_str() == Some(tr.kind));
        if !same_tracks {
            return Err(err(
                "the live tracks differ from the file's: live v1 adds, removes and reorders none",
            ));
        }
        let file_media: HashSet<&Key> = keys.media.iter().collect();
        for (k, _) in new.media {
            if !file_media.contains(k) {
                return Err(err("a media the file doesn't hold: live v1 adds none"));
            }
        }
        for (n, k) in keys.tracks.iter().enumerate() {
            if diff.tracks.contains(k) {
                for (fld, v) in lt.track_fields(n) {
                    if [f::MUTED, f::ENABLED, f::LOCKED, f::EDIT].contains(&fld)
                        && new.track(k, fld) != Some(v.as_str())
                    {
                        return Err(err(format!(
                            "a track's `{fld}` changed: live v1 writes only names"
                        )));
                    }
                }
            }
        }
        let mut rates = HashMap::new();
        for (i, it) in lt.items.iter().enumerate() {
            if let Some(r) = clip_rate(lt.node(&it.path)) {
                rates.insert(keys.items[i].clone(), r);
            }
        }
        let mut old_owned: HashMap<String, Vec<Key>> = HashMap::new();
        for (m, k) in keys.markers.iter().enumerate() {
            old_owned
                .entry(lt.owner_key(m, keys).to_owned())
                .or_default()
                .push(k.clone());
        }
        let mut sp = Splice {
            b,
            lt,
            keys,
            diff,
            kinds: new
                .items
                .iter()
                .filter_map(|(k, v)| Some((k, v.as_str()?)))
                .collect(),
            new_items: new.items.iter().map(|(k, _)| k).collect(),
            new,
            doc,
            ikeys: &keys.items,
            item_at: keys.items.iter().enumerate().map(|(i, k)| (k, i)).collect(),
            marker_at: keys
                .markers
                .iter()
                .enumerate()
                .map(|(i, k)| (k, i))
                .collect(),
            media_at: keys.media.iter().enumerate().map(|(i, k)| (k, i)).collect(),
            rates,
            old_owned,
            new_owned: Self::by_owner(&new),
        };
        // new clips: their media's rate, from the first file clip that uses it
        for (k, _) in new.items {
            if sp.item_at.contains_key(k) {
                continue;
            }
            let m = new.item(k, f::MEDIA).unwrap_or("");
            let src = sp.source_clip(m).ok_or_else(|| {
                err("a new clip whose media no clip of the file uses: nothing to copy its reference from")
            })?;
            if let Some(r) = clip_rate(lt.node(&lt.items[src].path)) {
                sp.rates.insert(k.clone(), r);
            }
        }
        Ok(sp)
    }

    /// The first file clip whose media is `media` (a key).
    fn source_clip(&self, media: &str) -> Option<usize> {
        let m = self
            .media_at
            .iter()
            .find(|(k, _)| k.as_str() == media)
            .map(|(_, i)| *i)?;
        self.lt
            .items
            .iter()
            .position(|it| it.media == Some(m) && matches!(it.kind, "clip" | "generator"))
    }

    /// The replacements, and the tracks whose children were rewritten.
    fn reps(&self) -> Result<(Vec<Rep>, HashSet<Key>), SpliceError> {
        let mut reps = Vec::new();
        let mut rewritten = HashSet::new();
        // items deleted with markers still owned, or fields nothing writes
        for (k, kind) in self.new.items {
            if let Some(&i) = self.item_at.get(k) {
                let it = &self.lt.items[i];
                if kind.as_str() != Some(it.kind) {
                    return Err(err("an item's kind changed"));
                }
                if !it.editable && self.item_changed(k) {
                    return Err(err(format!(
                        "a {} changed: live v1 writes clips only",
                        it.kind
                    )));
                }
                if self.diff.items.contains(k) {
                    for (fld, v) in self.lt.item_fields(i, self.keys) {
                        if [f::MEDIA, f::TR, f::EDIT, f::RETIMED].contains(&fld)
                            && self.new.item(k, fld) != Some(v.as_str())
                        {
                            return Err(err(format!("an item's `{fld}` changed")));
                        }
                    }
                }
            } else if !matches!(kind.as_str(), Some("clip" | "generator")) {
                return Err(err("a new item that isn't a clip"));
            }
        }
        for (i, k) in self.ikeys.iter().enumerate() {
            if !self.new_items.contains(k) {
                let it = &self.lt.items[i];
                if !it.editable {
                    return Err(err(format!("a {} was removed", it.kind)));
                }
            }
        }
        for (k, _) in self.new.markers {
            let owner = self.new.marker(k, f::OWNER).unwrap_or("");
            if let Some(&m) = self.marker_at.get(k) {
                if self.lt.owner_key(m, self.keys) != owner {
                    return Err(err("a marker changed its owner"));
                }
            }
            let live = owner.is_empty()
                || self.kinds.keys().any(|x| x.as_str() == owner)
                || self.keys.tracks.iter().any(|x| x.as_str() == owner);
            if !live {
                return Err(err("a marker whose owner is gone"));
            }
        }
        for (ti, (tk, _)) in self.new.tracks.iter().enumerate() {
            if self.track(ti, tk, &mut reps)? {
                rewritten.insert(tk.clone());
            }
        }
        // the timeline's own markers
        if self.markers_changed("") {
            let stack = self.lt.node(&self.lt.stack);
            let arr = self.markers_array("", stack.get("markers"))?;
            set_entry(self.b, stack, "markers", arr, &mut reps);
        }
        Ok((reps, rewritten))
    }

    fn item_changed(&self, k: &Key) -> bool {
        !self.item_at.contains_key(k)
            || self.diff.items.contains(k)
            || self.markers_changed(k.as_str())
    }

    /// Each owner's marker keys, in list order.
    fn by_owner(s: &Store<'_>) -> HashMap<String, Vec<Key>> {
        let mut m: HashMap<String, Vec<Key>> = HashMap::new();
        for (k, _) in s.markers {
            let owner = s.marker(k, f::OWNER).unwrap_or("");
            m.entry(owner.to_owned()).or_default().push(k.clone());
        }
        m
    }

    /// The marker keys of `owner`, in list order, on one side.
    fn owned<'m>(m: &'m HashMap<String, Vec<Key>>, owner: &str) -> &'m [Key] {
        m.get(owner).map_or(&[], Vec::as_slice)
    }

    fn markers_changed(&self, owner: &str) -> bool {
        let (a, b) = (
            Self::owned(&self.old_owned, owner),
            Self::owned(&self.new_owned, owner),
        );
        a != b || b.iter().any(|k| self.diff.markers.contains(k))
    }

    /// The rate token and style for a new marker of `owner` (an item's, else the first file
    /// clip's).
    fn marker_rate(&self, owner: &str) -> Result<(String, String), String> {
        if let Ok(k) = Key::new(owner) {
            if let Some(r) = self.rates.get(&k) {
                return Ok(r.clone());
            }
        }
        self.lt
            .items
            .iter()
            .find_map(|it| clip_rate(self.lt.node(&it.path)))
            .ok_or_else(|| "no rate in the file to write a marker's time at".to_owned())
    }

    /// The markers array of `owner` as the live content holds it.
    fn markers_array(&self, owner: &str, arr: Option<&Node>) -> Result<String, SpliceError> {
        let mut elems = Vec::new();
        for k in Self::owned(&self.new_owned, owner) {
            let at = ticks::parse(self.new.marker(k, f::AT).unwrap_or(""))
                .ok_or_else(|| err("a marker's time"))?;
            let dur = ticks::parse(self.new.marker(k, f::DUR).unwrap_or(""))
                .ok_or_else(|| err("a marker's duration"))?;
            let name = self.new.marker(k, f::NAME).unwrap_or("");
            let note = self.new.marker(k, f::NOTE).unwrap_or("");
            let color = self.new.marker(k, f::COLOR).unwrap_or("");
            match self.marker_at.get(&k) {
                Some(&m) => {
                    let mk = &self.lt.markers[m];
                    let n = self.lt.node(&mk.path);
                    let mut reps = Vec::new();
                    if mk.name != name {
                        set_entry(self.b, n, "name", quote_str(name), &mut reps);
                    }
                    if mk.note != note {
                        set_entry(self.b, n, "comment", quote_str(note), &mut reps);
                    }
                    if mk.color != color {
                        set_entry(self.b, n, "color", quote_str(color), &mut reps);
                    }
                    if mk.at != at || mk.dur != dur {
                        let r = n
                            .get("marked_range")
                            .or_else(|| n.get("range"))
                            .ok_or_else(|| err("a marker without a range"))?;
                        let (s, d) = (r.get("start_time"), r.get("duration"));
                        let (s, d) = s.zip(d).ok_or_else(|| err("a marker's range"))?;
                        if mk.at != at {
                            set_time(s, at, &mut reps).map_err(err)?;
                        }
                        if mk.dur != dur {
                            set_time(d, dur, &mut reps).map_err(err)?;
                        }
                    }
                    let bytes = apply(&self.b[n.start..n.end], n.start, &reps).map_err(err)?;
                    elems.push(String::from_utf8_lossy(&bytes).into_owned());
                }
                None => {
                    let (rtok, like) = self.marker_rate(owner).map_err(err)?;
                    elems.push(format!(
                        "{{{}, \"metadata\": {{}}, \"name\": {}, \"color\": {}, \"marked_range\": {}, \"comment\": {}}}",
                        tok("Marker.2"),
                        quote_str(name),
                        quote_str(color),
                        time_range(at, dur, &rtok, &like).map_err(err)?,
                        quote_str(note)
                    ));
                }
            }
        }
        Ok(array(self.b, arr, &elems))
    }

    /// An item's bytes as the live content holds it: a file item with its changes, or a new
    /// clip.
    fn item_reps(&self, k: &Key, i: usize, reps: &mut Vec<Rep>) -> Result<(), SpliceError> {
        let it = &self.lt.items[i];
        let n = self.lt.node(&it.path);
        let name = self.new.item(k, f::NAME).unwrap_or("");
        if it.name != name {
            set_entry(self.b, n, "name", quote_str(name), reps);
        }
        let off = self.new.item(k, f::OFF) == Some("1");
        if it.off != off {
            set_entry(self.b, n, "enabled", (!off).to_string(), reps);
        }
        let (_, dur) = self.new.geom(k).ok_or_else(|| err("an item's geometry"))?;
        let src = self.new.item(k, f::SRC).and_then(ticks::parse);
        if it.dur != dur || it.src != src {
            let src = src.ok_or_else(|| err("a clip's source time"))?;
            match n.get("source_range").filter(|r| !r.is_null()) {
                Some(r) => {
                    let (s, d) = r
                        .get("start_time")
                        .zip(r.get("duration"))
                        .ok_or_else(|| err("a clip's range"))?;
                    if it.src != Some(src) {
                        set_time(s, src, reps).map_err(err)?;
                    }
                    if it.dur != dur {
                        set_time(d, dur, reps).map_err(err)?;
                    }
                }
                None => {
                    let (rtok, like) = clip_rate(n).ok_or_else(|| err("a clip with no rate"))?;
                    set_entry(
                        self.b,
                        n,
                        "source_range",
                        time_range(src, dur, &rtok, &like).map_err(err)?,
                        reps,
                    );
                }
            }
        }
        if self.markers_changed(k.as_str()) {
            let arr = self.markers_array(k.as_str(), n.get("markers"))?;
            set_entry(self.b, n, "markers", arr, reps);
        }
        if self.item_changed(k) && !mutant::active("KeyNotInMetadata") {
            self.stamp(n, k, reps);
        }
        Ok(())
    }

    /// Our stamp, `"mpc": {"key": …, "doc": …}`, in the object's metadata.
    fn stamp(&self, n: &Node, k: &Key, reps: &mut Vec<Rep>) {
        let s = format!(
            "{{\"key\": {}, \"doc\": {}}}",
            quote_str(k.as_str()),
            quote_str(&self.doc)
        );
        match n.get("metadata").filter(|m| m.entries().is_some()) {
            Some(m) => set_entry(self.b, m, STAMP, s, reps),
            None => set_entry(
                self.b,
                n,
                "metadata",
                format!("{{{}: {s}}}", quote_str(STAMP)),
                reps,
            ),
        }
    }

    /// The bytes of item `k`, its changes applied.
    fn item_bytes(&self, k: &Key) -> Result<String, SpliceError> {
        let Some(&i) = self.item_at.get(k) else {
            return self.new_clip(k);
        };
        let n = self.lt.node(&self.lt.items[i].path);
        let mut reps = Vec::new();
        self.item_reps(k, i, &mut reps)?;
        let out = apply(&self.b[n.start..n.end], n.start, &reps).map_err(err)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// A new clip: its media reference copied from the first file clip that uses its media.
    fn new_clip(&self, k: &Key) -> Result<String, SpliceError> {
        let m = self.new.item(k, f::MEDIA).unwrap_or("");
        let src_clip = self
            .source_clip(m)
            .ok_or_else(|| err("a new clip's media"))?;
        let c = self.lt.node(&self.lt.items[src_clip].path);
        let r = active_ref(c).ok_or_else(|| err("a new clip's media reference"))?;
        let (rtok, like) = self
            .rates
            .get(k)
            .cloned()
            .ok_or_else(|| err("a new clip with no rate"))?;
        let (_, dur) = self.new.geom(k).ok_or_else(|| err("a clip's geometry"))?;
        let src = self
            .new
            .item(k, f::SRC)
            .and_then(ticks::parse)
            .ok_or_else(|| err("a clip's source time"))?;
        let ver2 = c
            .get("OTIO_SCHEMA")
            .and_then(Node::as_str)
            .is_some_and(|s| s == "Clip.2");
        let reference = slice(self.b, r);
        let refs = if ver2 {
            format!(
                "\"media_references\": {{\"DEFAULT_MEDIA\": {reference}}}, \"active_media_reference_key\": \"DEFAULT_MEDIA\""
            )
        } else {
            format!("\"media_reference\": {reference}")
        };
        let stamp = if mutant::active("KeyNotInMetadata") {
            "{}".to_owned()
        } else {
            format!(
                "{{{}: {{\"key\": {}, \"doc\": {}}}}}",
                quote_str(STAMP),
                quote_str(k.as_str()),
                quote_str(&self.doc)
            )
        };
        Ok(format!(
            "{{{}, \"metadata\": {stamp}, \"name\": {}, \"source_range\": {}, \"effects\": [], \"markers\": {}, \"enabled\": {}, {refs}}}",
            tok(if ver2 { "Clip.2" } else { "Clip.1" }),
            quote_str(self.new.item(k, f::NAME).unwrap_or("")),
            time_range(src, dur, &rtok, &like).map_err(err)?,
            self.markers_array(k.as_str(), None)?,
            self.new.item(k, f::OFF) != Some("1"),
        ))
    }

    /// Track `tk`: its name, its markers, and its children rewritten when its items' order or
    /// gaps changed (else each item changed in place).
    fn track(&self, ti: usize, tk: &Key, reps: &mut Vec<Rep>) -> Result<bool, SpliceError> {
        let lt_track = &self.lt.tracks[ti];
        let node = self.lt.node(&lt_track.path);
        if Some(lt_track.name.as_str()) != self.new.track(tk, f::NAME) {
            set_entry(
                self.b,
                node,
                "name",
                quote_str(self.new.track(tk, f::NAME).unwrap_or("")),
                reps,
            );
        }
        if self.markers_changed(tk.as_str()) {
            let arr = self.markers_array(tk.as_str(), node.get("markers"))?;
            set_entry(self.b, node, "markers", arr, reps);
        }
        // the track's steps as the live content places them
        let order: Vec<&Key> = self.new.track_items(tk);
        let mut model_items = Vec::with_capacity(order.len());
        for k in &order {
            let (start, dur) = self.new.geom(k).ok_or_else(|| err("an item's geometry"))?;
            let tm = |x: i64| ticks::time(x).map_err(|_| err("a time out of range"));
            let transition = self.kinds.get(k).copied() == Some("transition");
            let mut it = Item::new(
                crate::model::Key::from_parts(k.as_str(), KeyOrigin::Native),
                if transition {
                    ItemKind::Transition
                } else {
                    ItemKind::Clip
                },
                "",
                tm(start)?,
                tm(dur)?,
            );
            if transition {
                let (inn, _) = self
                    .new
                    .item(k, f::TR)
                    .and_then(ticks::parse_pair)
                    .ok_or_else(|| err("a transition's offsets"))?;
                it.transition = Some(TransitionInfo::new("", Some(tm(inn)?)));
            }
            model_items.push(it);
        }
        let mut mt = Track::new(
            crate::model::Key::from_parts(tk.as_str(), KeyOrigin::Native),
            "",
            TrackKind::Other,
        );
        mt.items = model_items;
        let steps = mt.to_steps().map_err(|f| {
            err(format!(
                "two items overlap on a track, or a transition has no cut: {f:?}"
            ))
        })?;
        #[derive(PartialEq, Debug, Clone)]
        enum Tok {
            Item(Key),
            Gap(i64),
        }
        let mut new_toks = Vec::new();
        let mut end: i64 = 0;
        for s in &steps {
            match s {
                Step::Gap(d) => {
                    let d = ticks::of(*d).map_err(|_| err("a gap off the tick grid"))?;
                    new_toks.push(Tok::Gap(d));
                    end += d;
                }
                Step::Item(n) => {
                    new_toks.push(Tok::Item(order[*n].clone()));
                    end += self.new.geom(order[*n]).map_or(0, |g| g.1);
                }
                Step::Transition { index, .. } => new_toks.push(Tok::Item(order[*index].clone())),
            }
        }
        let mut old_toks = Vec::new();
        let mut old_end: i64 = 0;
        let keys_of = |i: usize| -> Key { self.ikeys[i].clone() };
        for c in &lt_track.children {
            match c {
                Child::Item(i) => {
                    old_toks.push(Tok::Item(keys_of(*i)));
                    if self.lt.items[*i].kind != "transition" {
                        old_end += self.lt.items[*i].dur;
                    }
                }
                Child::Gap { dur, .. } => {
                    old_toks.push(Tok::Gap(*dur));
                    old_end += dur;
                }
            }
        }
        // a trailing gap keeps the track's length where the items still fit in it
        if matches!(lt_track.children.last(), Some(Child::Gap { .. })) && end < old_end {
            new_toks.push(Tok::Gap(old_end - end));
        }
        if new_toks == old_toks {
            for k in &order {
                if let Some(&i) = self.item_at.get(*k) {
                    self.item_reps(k, i, reps)?;
                }
            }
            return Ok(false);
        }
        if !lt_track.editable {
            return Err(err(
                "a track whose gaps hold more than a duration: live v1 doesn't rebuild them",
            ));
        }
        // the old gaps by their neighbours
        let mut old_gaps: HashMap<(Option<Key>, Option<Key>), &Node> = HashMap::new();
        for (j, c) in lt_track.children.iter().enumerate() {
            if let Child::Gap { path, .. } = c {
                let node = self.lt.node(path);
                let prev = old_toks[..j].iter().rev().find_map(|t| match t {
                    Tok::Item(k) => Some(k.clone()),
                    Tok::Gap(_) => None,
                });
                let next = old_toks[j + 1..].iter().find_map(|t| match t {
                    Tok::Item(k) => Some(k.clone()),
                    Tok::Gap(_) => None,
                });
                old_gaps.insert((prev, next), node);
            }
        }
        let mut elems = Vec::new();
        for (j, tk2) in new_toks.iter().enumerate() {
            match tk2 {
                Tok::Item(k) => elems.push(self.item_bytes(k)?),
                Tok::Gap(d) => {
                    let prev = new_toks[..j].iter().rev().find_map(|t| match t {
                        Tok::Item(k) => Some(k.clone()),
                        Tok::Gap(_) => None,
                    });
                    let next = new_toks[j + 1..].iter().find_map(|t| match t {
                        Tok::Item(k) => Some(k.clone()),
                        Tok::Gap(_) => None,
                    });
                    let was = old_gaps.get(&(prev.clone(), next.clone()));
                    if mutant::active("GapNotRebuilt") {
                        // planted: the old gaps kept as they were, none made or resized
                        if let Some(g) = was {
                            elems.push(slice(self.b, g));
                        }
                        continue;
                    }
                    elems.push(self.gap(*d, was.copied(), prev.as_ref(), next.as_ref())?);
                }
            }
        }
        let arr = node.get("children");
        let text = array(self.b, arr, &elems);
        match arr {
            Some(a) => reps.push((a.start, a.end, text)),
            None => set_entry(self.b, node, "children", text, reps),
        }
        Ok(true)
    }

    /// A gap of `d` ticks: the old one between the same neighbours kept or resized, else a new
    /// one at a neighbour's rate.
    fn gap(
        &self,
        d: i64,
        was: Option<&Node>,
        prev: Option<&Key>,
        next: Option<&Key>,
    ) -> Result<String, SpliceError> {
        if let Some(g) = was {
            let r = g.get("source_range").and_then(|r| r.get("duration"));
            let old = r.and_then(|r| model::rational(r).ok()).map(|(v, _)| v);
            if old == Some(d) {
                return Ok(slice(self.b, g));
            }
            if let Some(rt) = r {
                let mut reps = Vec::new();
                if set_time(rt, d, &mut reps).is_ok() {
                    let out = apply(&self.b[g.start..g.end], g.start, &reps).map_err(err)?;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
            }
        }
        for k in [next, prev].into_iter().flatten() {
            if let Some((rtok, like)) = self.rates.get(k) {
                if let Ok(tr) = time_range(0, d, rtok, like) {
                    return Ok(format!(
                        "{{{}, \"metadata\": {{}}, \"name\": \"\", \"source_range\": {tr}, \"effects\": [], \"markers\": [], \"enabled\": true}}",
                        tok("Gap.1")
                    ));
                }
            }
        }
        Err(err("a gap no neighbour's rate writes exactly"))
    }
}
