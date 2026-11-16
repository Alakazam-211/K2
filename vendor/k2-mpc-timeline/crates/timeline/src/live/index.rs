//! The verbs' index of a live timeline ([`TrackIndex`]): each track's live items in time order,
//! with each item's kind, place and rate, kept by the kit beside the live doc as the pack's
//! verb state (`Pack::verb_state_build`, `Pack::verb_state_change`) and updated from every
//! change in O(log n) per op, so a verb costs what it touches, not the whole timeline.
//!
//! - **Order** on a track is `(at, transition first, key)`: `at` is an item's start, or a
//!   transition's cut; a transition sorts before the item that starts at its cut. Items that
//!   tie on both (zero-length items at one start) are put in list order when read
//!   ([`TrackIndex::ordered`]), as the store's own reads put them.
//! - **Kept from the change's ops:** an item a list op left live is put again from the doc after
//!   the change; one it left deleted or dropped is taken out; a live item whose `track`,
//!   `geom`, `tr`, `rate` or `media` was set is put again. Every key the items list ever held
//!   (live or deleted) is kept by number, so a mint goes past them.
//! - **The kit checks it** in test builds: after every change and at every hand-out it is
//!   compared with a fresh build ([`TrackIndex::build`]).

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::elem::{Applied, Key, Value};
use k2_mpc_base::session::View;

use super::model::{f, key_num, prop, ITEMS, ITEM_PROPS};
use super::ticks;
use crate::time::Rate;

/// One item as the index holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its key.
    pub key: Key,
    /// Its track's key.
    pub track: String,
    /// Its kind (`clip`, `generator`, `transition`, `nest`).
    pub kind: String,
    /// Its record start and duration, ticks.
    pub start: i64,
    /// Its record duration, ticks.
    pub dur: i64,
    /// Where it sorts: its start, or a transition's cut.
    pub at: i64,
    /// The rate its times are written at.
    pub rate: Option<Rate>,
    /// Its media's key.
    pub media: Option<String>,
}

impl Entry {
    /// Whether it is a transition.
    pub fn is_transition(&self) -> bool {
        self.kind == "transition"
    }

    /// Its record end.
    pub fn end(&self) -> i64 {
        self.start.saturating_add(self.dur)
    }

    fn order(&self) -> Slot {
        (
            self.at,
            u8::from(!self.is_transition()),
            self.key.as_str().to_owned(),
        )
    }
}

/// Where an item sorts on its track: `(at, 0 for a transition else 1, key)`. The empty key
/// sorts before every key, so `(at, 0, "")` starts a range at `at`.
type Slot = (i64, u8, String);

/// Each track's live items in time order, each item's entry, the items using each media, and
/// every key number the items list ever held.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TrackIndex {
    by_track: BTreeMap<String, BTreeSet<Slot>>,
    of_item: BTreeMap<String, Entry>,
    by_media: BTreeMap<String, BTreeSet<Key>>,
    numbers: BTreeSet<u64>,
    /// Every live item's record end, counted: the last is the timeline's length.
    ends: BTreeMap<i64, usize>,
}

fn field<'v>(m: Option<&'v BTreeMap<String, Value>>, k: &Key, fld: &str) -> Option<&'v str> {
    m?.get(&prop(k, fld)).and_then(Value::as_str)
}

impl TrackIndex {
    /// The index of the doc `view` holds.
    pub fn build(view: &View) -> TrackIndex {
        let mut ix = TrackIndex::default();
        for (k, v) in view.list(ITEMS).unwrap_or(&[]) {
            ix.put(view, k, v.as_str().unwrap_or(""));
        }
        for k in view
            .list(ITEMS)
            .unwrap_or(&[])
            .iter()
            .map(|(k, _)| k)
            .chain(view.hidden(ITEMS).into_iter().flat_map(|h| h.keys()))
        {
            if let Some(n) = key_num(k.as_str(), 'i') {
                ix.numbers.insert(n);
            }
        }
        ix
    }

    /// Item `k` of kind `kind` put in from the doc `view`.
    fn put(&mut self, view: &View, k: &Key, kind: &str) {
        let m = view.map(ITEM_PROPS);
        let (start, dur) = field(m, k, f::GEOM)
            .and_then(ticks::parse_pair)
            .unwrap_or((0, 0));
        let at = match (kind, field(m, k, f::TR).and_then(ticks::parse_pair)) {
            ("transition", Some((inn, _))) => start.saturating_add(inn),
            _ => start,
        };
        let e = Entry {
            key: k.clone(),
            track: field(m, k, f::TRACK).unwrap_or("").to_owned(),
            kind: kind.to_owned(),
            start,
            dur,
            at,
            rate: field(m, k, f::RATE).and_then(ticks::parse_rate),
            media: field(m, k, f::MEDIA).map(str::to_owned),
        };
        self.by_track
            .entry(e.track.clone())
            .or_default()
            .insert(e.order());
        if let Some(md) = &e.media {
            self.by_media
                .entry(md.clone())
                .or_default()
                .insert(k.clone());
        }
        *self.ends.entry(e.end()).or_default() += 1;
        self.of_item.insert(k.as_str().to_owned(), e);
    }

    /// Item `k` taken out; its entry, if it was in.
    fn take(&mut self, k: &Key) -> Option<Entry> {
        let e = self.of_item.remove(k.as_str())?;
        if let Some(n) = self.ends.get_mut(&e.end()) {
            *n -= 1;
            if *n == 0 {
                self.ends.remove(&e.end());
            }
        }
        if let Some(set) = self.by_track.get_mut(&e.track) {
            set.remove(&e.order());
            if set.is_empty() {
                self.by_track.remove(&e.track);
            }
        }
        if let Some(md) = &e.media {
            if let Some(set) = self.by_media.get_mut(md) {
                set.remove(k);
                if set.is_empty() {
                    self.by_media.remove(md);
                }
            }
        }
        Some(e)
    }

    /// One change to the doc, now `view`: kept, as a fresh build would give it.
    pub fn change(&mut self, applied: &[Applied], view: &View) {
        for a in applied {
            match a {
                Applied::List { root, ops } if root == ITEMS => {
                    for o in ops {
                        let was = self.take(&o.key);
                        match o.kind {
                            k2_mpc_base::elem::ListOpKind::Drop => {
                                if let Some(n) = key_num(o.key.as_str(), 'i') {
                                    self.numbers.remove(&n);
                                }
                            }
                            _ => {
                                if let Some(n) = key_num(o.key.as_str(), 'i') {
                                    self.numbers.insert(n);
                                }
                            }
                        }
                        if o.index_after.is_some() {
                            let kind = o
                                .value_after
                                .as_ref()
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .or_else(|| was.map(|e| e.kind))
                                .unwrap_or_default();
                            self.put(view, &o.key, &kind);
                        }
                    }
                }
                Applied::Map { root, ops } if root == ITEM_PROPS => {
                    for o in ops {
                        let Some((item, fld)) = o.key.split_once('\x1f') else {
                            continue;
                        };
                        if ![f::TRACK, f::GEOM, f::TR, f::RATE, f::MEDIA].contains(&fld) {
                            continue;
                        }
                        let Ok(k) = Key::new(item) else { continue };
                        if let Some(e) = self.take(&k) {
                            self.put(view, &k, &e.kind);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Item `k`'s entry, when it is live.
    pub fn get(&self, k: &Key) -> Option<&Entry> {
        self.of_item.get(k.as_str())
    }

    /// The latest record end of any live item (a transition's too): the timeline's length,
    /// `None` when there are no items.
    pub fn max_end(&self) -> Option<i64> {
        self.ends.last_key_value().map(|(e, _)| *e)
    }

    /// How many live items there are.
    pub fn len(&self) -> usize {
        self.of_item.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.of_item.is_empty()
    }

    /// The highest key number the items list ever held (live or deleted), 0 for none.
    pub fn max_number(&self) -> u64 {
        self.numbers.last().copied().unwrap_or(0)
    }

    /// The highest key number among the live items, 0 for none.
    pub fn max_live_number(&self) -> u64 {
        self.of_item
            .keys()
            .filter_map(|k| key_num(k, 'i'))
            .max()
            .unwrap_or(0)
    }

    /// The live items using media `m`.
    pub fn media_users(&self, m: &str) -> impl Iterator<Item = &Key> {
        self.by_media.get(m).into_iter().flatten()
    }

    /// The items of `track` in time order, ties put in the order of the doc's list `view`.
    pub fn ordered<'a>(&'a self, track: &str, view: &View) -> Vec<&'a Key> {
        let Some(set) = self.by_track.get(track) else {
            return Vec::new();
        };
        let mut out: Vec<&Slot> = set.iter().collect();
        // ties on (at, transition first) go in list order, as the store's own reads put them
        let tied = out.windows(2).any(|w| (w[0].0, w[0].1) == (w[1].0, w[1].1));
        if tied {
            let pos: BTreeMap<&str, usize> = view
                .list(ITEMS)
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(i, (k, _))| (k.as_str(), i))
                .collect();
            out.sort_by_key(|(at, t, k)| {
                (*at, *t, pos.get(k.as_str()).copied().unwrap_or(usize::MAX))
            });
        }
        out.into_iter()
            .filter_map(|(_, _, k)| self.of_item.get(k).map(|e| &e.key))
            .collect()
    }

    /// The live transition on `track` whose cut is `at`, if any.
    pub fn transition_at(&self, track: &str, at: i64) -> Option<&Key> {
        let (_, t, k) = self
            .by_track
            .get(track)?
            .range((at, 0, String::new())..)
            .next()
            .filter(|(a, t, _)| *a == at && *t == 0)?;
        let _ = t;
        self.of_item.get(k).map(|e| &e.key)
    }

    /// The live items of `track` (but `skip`, transitions aside) that sort before `before`,
    /// going back.
    fn back_from<'a>(
        &'a self,
        track: &str,
        before: i64,
        skip: Option<String>,
    ) -> impl Iterator<Item = &'a Entry> {
        self.by_track
            .get(track)
            .into_iter()
            .flat_map(move |s| s.range(..(before, 0, String::new())).rev())
            .filter(|(_, t, _)| *t == 1)
            .filter_map(|(_, _, k)| self.of_item.get(k))
            .filter(move |e| skip.as_deref() != Some(e.key.as_str()))
    }

    /// The live items of `track` (but `skip`, transitions aside) from `from` on.
    fn forward_from<'a>(
        &'a self,
        track: &str,
        from: i64,
        skip: Option<String>,
    ) -> impl Iterator<Item = &'a Entry> {
        self.by_track
            .get(track)
            .into_iter()
            .flat_map(move |s| s.range((from, 0, String::new())..))
            .filter(|(_, t, _)| *t == 1)
            .filter_map(|(_, _, k)| self.of_item.get(k))
            .filter(move |e| skip.as_deref() != Some(e.key.as_str()))
    }

    /// The first item of `track` (but `skip`, transitions aside) that meets `start..end`.
    pub fn overlap<'a>(
        &'a self,
        track: &str,
        skip: Option<&Key>,
        start: i64,
        end: i64,
    ) -> Option<&'a Key> {
        let skip = skip.map(|k| k.as_str().to_owned());
        // items on a track don't overlap one another, so their ends rise with their starts:
        // going back from `end`, the first that ends at or before `start` ends every search
        for e in self.back_from(track, end, skip) {
            if e.end() > start && e.start < end {
                return Some(&e.key);
            }
            if e.end() <= start && e.dur > 0 {
                break;
            }
        }
        None
    }

    /// The items of `track` (but `skip`, transitions aside) around `start..end`: the last
    /// ending at or before `start`, with its end, and the first starting at or after `end`,
    /// with its start.
    #[allow(clippy::type_complexity)]
    pub fn neighbours<'a>(
        &'a self,
        track: &str,
        skip: Option<&Key>,
        start: i64,
        end: i64,
    ) -> (Option<(&'a Key, i64)>, Option<(&'a Key, i64)>) {
        let skip = skip.map(|k| k.as_str().to_owned());
        let prev = self
            .back_from(track, start.saturating_add(1), skip.clone())
            .find(|e| e.end() <= start)
            .map(|e| (&e.key, e.end()));
        let next = self
            .forward_from(track, end, skip)
            .next()
            .map(|e| (&e.key, e.start));
        (prev, next)
    }

    /// The item of `track` (transitions aside) under record time `at`.
    pub fn at(&self, track: &str, at: i64) -> Option<&Key> {
        self.back_from(track, at.saturating_add(1), None)
            .next()
            .filter(|e| e.start <= at && at < e.end())
            .map(|e| &e.key)
    }
}
