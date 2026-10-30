//! [`EngineDoc`] on Yrs (wire-compatible with Yjs).
//!
//! - One Yrs root per schema root (text roots only, today).
//! - Offsets are UTF-16 inside Yrs (`OffsetKind::Utf16`), matching Yjs and CodeMirror.
//!   Hunks arrive in UTF-8 byte offsets and are converted here.
//! - Wire updates are Yjs v1; stored state is v2 (10-30% smaller in the D0 spike).
//! - Outside bytes get a cheap shape check first, then decode/apply under `catch_unwind`.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use yrs::block::ClientID;
use yrs::branch::BranchID;
use yrs::types::text::ChangeKind as YChangeKind;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, PositionMismatch, ReadTxn, Snapshot,
    StateVector, Text, TextRef, Transact, Update,
};
#[cfg(feature = "conformance")]
use yrs::{Assoc, IndexedSequence, StickyIndex};

use super::{BlameRun, EngineCheck, EngineDoc, SESSION_CLIENT};
use crate::doc::{ClientId, DocError, Edit, UpdateInfo, MAX_CLIENT_ID};
use crate::schema::{Kind, Schema};
use crate::text::diff::Hunk;

pub(crate) struct YrsDoc {
    // handles drop before their Doc (fields drop in declaration order; rule 8)
    texts: HashMap<&'static str, TextRef>,
    doc: Doc,
    /// An apply left the doc in a state that can't be trusted: updates left pending (the
    /// engine can't encode those reliably), or a failed or panicked apply that may have
    /// half-integrated. Nothing is encoded or edited from a
    /// poisoned doc; its owner reloads it from the store.
    poisoned: bool,
    /// Each text root's text, as last read or written (v0.9.2). Reading it out of Yrs walks
    /// every item of the root, deleted ones included, so it grows with the doc's history,
    /// not its text; an edit made here knows its result. Emptied by every apply.
    text_cache: Mutex<HashMap<&'static str, String>>,
    /// The sampled runtime check of the edit path and the text cache (v0.9.3).
    checks: Checks,
    /// What the engine's position index reported through its audit hook (v0.9.7: a shadow
    /// check found its marker path and the full walk disagree; the walk was used). Shared
    /// with the hook, which runs inside a transaction and only pushes here.
    positions: Arc<Mutex<Vec<EngineCheck>>>,
}

/// A throwaway replica and its handle on one text root ([`YrsDoc::replica`]).
struct Replica {
    // handles drop before their Doc (fields drop in declaration order; rule 8)
    text: TextRef,
    doc: Doc,
}

/// At most this many position index reports wait to be taken (a broken index reports on
/// every sampled lookup; the rest are counted by the engine, not kept).
const POSITION_REPORTS_KEPT: usize = 64;

/// Point `doc`'s position index audit hook at `sink` and set its shadow rate (one marker
/// lookup in `every` also walks and compares; 0 never). Before any transaction on `doc`:
/// both take its write lock.
fn watch_positions(doc: &Doc, sink: &Arc<Mutex<Vec<EngineCheck>>>, every: u32) {
    let sink = Arc::clone(sink);
    doc.set_position_audit_hook(Some(Box::new(move |m: &PositionMismatch| {
        let root = match &m.branch {
            BranchID::Root(name) => Some(name.to_string()),
            BranchID::Nested(_) => None,
        };
        let mut found = sink.lock().unwrap_or_else(|p| p.into_inner());
        if found.len() < POSITION_REPORTS_KEPT {
            found.push(EngineCheck::Position {
                root,
                index: m.index,
                marker: m.marker.0,
            });
        }
    })));
    doc.set_position_shadow_rate(every);
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::position_index_off() {
        doc.set_position_index(false);
    }
}

/// The canary (v0.9.3): every `every`th edit is also made the old way (a throwaway replica
/// as the client, from the state before it) and the two updates compared byte for byte, and
/// the text cache is compared with a fresh read. A mismatch is never served: the doc goes
/// back to the state before the edit and takes the throwaway's update, then makes every
/// later edit that way (`fallback`), or reads every text afresh (`cache_off`), until it is
/// reloaded. What it found waits in `found` for the owner to report.
///
/// The interval adapts to the doc's size (v0.9.7): a sampled edit costs O(the doc's history),
/// so past `scale` units of history the interval grows in proportion to it (`every x units /
/// scale`, never under `every`), keeping the canary's amortised cost per edit bounded. Below
/// `scale` a doc is checked one edit in `every`, as before. Setting the rate (as the session
/// does after every load) places the next sample from the doc's history alone: `every` edits
/// away below the scale; past it at a phase within the interval, `1 + mix(units) mod
/// interval` edits away ([`canary_phase`]). A reload starts the edit count afresh, so a fixed first gap would set the rate
/// by how often the doc is reloaded: `every` brings each check after a reload closer, and a
/// full interval never fires when reloads come more often than that. With the phase spread
/// over the interval, each reload samples with a probability of about (edits until the next
/// reload) / interval, and the long-run rate stays about one edit in `interval`.
#[derive(Debug, Default)]
struct Checks {
    /// The base interval, the floor. 0: never.
    every: u32,
    /// The history (UTF-16 units, [`history_units`]) past which the interval grows with it.
    /// 0: a fixed rate.
    scale: u64,
    /// The edit count of the next sample.
    next: u64,
    /// The position index's shadow rate (v0.9.7; 0: never).
    position_every: u32,
    edits: u64,
    fallback: bool,
    cache_off: bool,
    found: Vec<EngineCheck>,
    /// Whether the edit in progress is a sampled one.
    now: bool,
}

impl Checks {
    fn new() -> Self {
        let mut c = Self {
            every: crate::doc::DEFAULT_EDIT_CHECK_EVERY,
            scale: crate::doc::DEFAULT_EDIT_CHECK_SCALE_UNITS,
            position_every: crate::doc::DEFAULT_POSITION_CHECK_EVERY,
            ..Self::default()
        };
        // an empty doc: the first sample at edit `every`
        c.restart(0);
        c
    }

    /// Count an edit; whether it is a sampled one. A sampled edit places the next sample
    /// from the doc's history, `units` (read only then: O(clients)).
    fn sample(&mut self, units: impl FnOnce() -> u64) -> bool {
        self.edits += 1;
        if self.every == 0 || self.edits < self.next {
            return false;
        }
        self.next = self.edits.saturating_add(self.interval(units()));
        true
    }

    /// The edits until the next sample on a doc with `units` of history: `every`, or
    /// `every x units / scale` past the scale (saturating).
    fn interval(&self, units: u64) -> u64 {
        let every = u64::from(self.every);
        if self.scale == 0 {
            return every;
        }
        let grown = u128::from(every) * u128::from(units) / u128::from(self.scale);
        every.max(u64::try_from(grown).unwrap_or(u64::MAX))
    }

    /// The next sample on a doc with `units` of history when the rate is set (after every
    /// load): `every` edits from now below the scale (the first sample at edit `every`), and
    /// past it at a phase within the interval derived from the history ([`canary_phase`]).
    fn restart(&mut self, units: u64) {
        let interval = self.interval(units);
        let first = if self.every != 0 && self.scale != 0 && units > self.scale {
            canary_phase(units, interval)
        } else {
            interval
        };
        self.next = self.edits.saturating_add(first);
    }
}

/// The canary's first gap after a reload past the scale: `1 + mix(units) mod interval`, in
/// 1..=interval. Deterministic, and spread evenly over the interval: the history is mixed
/// first (SplitMix64's finaliser) because `units mod interval` alone is not. The interval is
/// `every x units / scale`, so `units mod interval` is under `scale / every` (under 50 edits
/// at the defaults) whatever the interval, which would check soon after every reload.
fn canary_phase(units: u64, interval: u64) -> u64 {
    let mut z = units.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    1 + z % interval.max(1)
}

/// The doc's history in UTF-16 units: the sum of its state vector's clocks (every unit ever
/// inserted counts, deleted or not). O(clients).
fn history_units(doc: &Doc) -> u64 {
    doc.transact()
        .state_vector()
        .iter()
        .map(|(_, clock)| u64::from(*clock))
        .sum()
}

/// Every Yrs doc the kit makes. **GC stays on** (`skip_gc: false`, K-PURGE-11, E-ERASE-10):
/// deleted text leaves the doc's state when the deleting transaction commits (its items
/// keep their IDs and lengths, not their content), so a re-encoded state never carries
/// deleted content. Purge depends on it; `gc_is_on` fails if it is turned off.
fn new_doc(client: u64) -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(client),
        offset_kind: OffsetKind::Utf16,
        skip_gc: false,
        ..Options::default()
    })
}

/// Root text `name` of `doc`: the only way this module takes a shared-type handle
/// (ARCHITECTURE rule 8, v0.9.7). A root's handle stays valid for the doc's life; a nested
/// type's handle can outlive its type after a remote delete, so the kit never holds one, and
/// never across an apply. Checked in debug builds and by
/// `tests::the_engine_holds_only_root_handles`.
fn root_text(doc: &Doc, name: &str) -> TextRef {
    let t = doc.get_or_insert_text(name);
    debug_assert!(
        matches!(
            AsRef::<yrs::branch::Branch>::as_ref(&t).id(),
            BranchID::Root(_)
        ),
        "the kit holds only root handles"
    );
    t
}

fn guarded<T>(f: impl FnOnce() -> Result<T, DocError>) -> Result<T, DocError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(Err(DocError::Panic))
}

/// The one way the kit decodes update bytes that come from outside it (v0.9.5): v1 only (the
/// fork bounds what decoding allocates), then refused (hardening) when any block carries
/// content the kit never uses, subdocuments ([`DocError::Unaccepted`]). Decode-only: nothing is
/// applied before the check, and the caller applies the update returned (decoded once).
fn decode_accepted(update: &[u8]) -> Result<Update, DocError> {
    let u = guarded(|| Update::decode_v1(update).map_err(|e| DocError::Decode(e.to_string())))?;
    if u.has_subdoc_content() {
        return Err(DocError::Unaccepted);
    }
    Ok(u)
}

/// The store-load scan (hardening, v0.9.5): a stored state or log entry holding content the
/// kit never uses (subdocuments) is never loaded as live ([`DocError::Quarantined`]).
/// Expected never to fire: the kit never writes such content, and refuses it at every ingress.
fn stored_accepted(u: &Update) -> Result<(), DocError> {
    if u.has_subdoc_content() {
        return Err(DocError::Quarantined);
    }
    Ok(())
}

fn roots(doc: &Doc, schema: &Schema) -> HashMap<&'static str, TextRef> {
    schema
        .roots
        .iter()
        .filter(|(_, k)| *k == Kind::Text)
        .map(|(n, _)| (*n, root_text(doc, n)))
        .collect()
}

/// Read one lib0 varuint (7 bits per byte, low groups first, at most 53 bits).
fn read_varuint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    for shift in (0..56).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (v <= MAX_CLIENT_ID).then_some(v);
        }
    }
    None
}

/// Shape check of an encoded state vector before decoding. Keeps work on hostile input
/// bounded by its size: a real vector has at least 2 bytes per entry and is consumed exactly.
fn state_vector_well_formed(b: &[u8]) -> bool {
    let mut at = 0;
    let Some(n) = read_varuint(b, &mut at) else {
        return false;
    };
    if n > (b.len() as u64) / 2 {
        return false;
    }
    for _ in 0..n {
        if read_varuint(b, &mut at).is_none() || read_varuint(b, &mut at).is_none() {
            return false;
        }
    }
    at == b.len()
}

/// Shape check of a client's v1 update before decoding. Keeps work on hostile input bounded
/// by its size (no outsized reservations): the client count can't exceed the bytes left divided
/// by 3 (each client section is at least its block count, client ID and clock), and the
/// first client's block count can't exceed the bytes left. More than one client is allowed
/// here; [`EngineDoc::check_update`] decides what other clients' blocks may be.
/// A client update's bytes: the cheap shape check, then [`decode_accepted`].
fn decode_client(update: &[u8]) -> Result<Update, DocError> {
    if !client_update_well_formed(update) {
        return Err(DocError::Decode("malformed update".into()));
    }
    decode_accepted(update)
}

fn client_update_well_formed(b: &[u8]) -> bool {
    let mut at = 0;
    let Some(clients) = read_varuint(b, &mut at) else {
        return false;
    };
    if clients > ((b.len() - at) / 3) as u64 {
        return false;
    }
    if clients >= 1 {
        let Some(blocks) = read_varuint(b, &mut at) else {
            return false;
        };
        if blocks > (b.len() - at) as u64 {
            return false;
        }
    }
    true
}

/// The update must continue each client's history, not skip ahead: for every client in
/// it, its lowest clock is within what the doc (`have`) already holds for that client.
fn continues_each_client(have: &StateVector, u: &Update) -> Result<(), DocError> {
    for (client, from) in u.state_vector_lower().iter() {
        let h = have.get(client);
        if *from > h {
            return Err(DocError::MissingDeps {
                client: client.get(),
                from: *from,
                have: h,
            });
        }
    }
    Ok(())
}

/// A text root's hunks with their UTF-16 (offset, length), ready to apply.
pub(crate) type TextPlan<'a> = (&'a str, &'a [Hunk], Vec<(u32, u32)>);

/// UTF-16 offset and length of each hunk, in one forward pass over `text`.
fn to_utf16(text: &str, hunks: &[Hunk]) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(hunks.len());
    let (mut at_byte, mut at16) = (0usize, 0u32);
    let advance = |to: usize, at_byte: &mut usize, at16: &mut u32| {
        *at16 += text[*at_byte..to].encode_utf16().count() as u32;
        *at_byte = to;
    };
    for h in hunks {
        advance(h.start, &mut at_byte, &mut at16);
        let s16 = at16;
        advance(h.end, &mut at_byte, &mut at16);
        out.push((s16, at16 - s16));
    }
    out
}

/// The first engine clock of each hunk's inserted text when `client`, whose clock is
/// `clock_before`, applies `edits` (canonical hunks) through [`EngineDoc::edit_as`]: per
/// edit, per hunk **in hunk order**, the clock its insertion starts at (for a hunk that
/// inserts nothing, the clock it would have started at). Clocks count UTF-16 code units.
///
/// This is this adapter's own order, and only its: `edit_as` applies the roots in edit order
/// and each root's hunks **back to front**, and every non-empty insertion takes the next
/// consecutive clocks (deletions take none). The kit names the elements a change inserts by
/// it (undo spec U23: provenance of restored text), so a test cross-checks it against
/// [`EngineDoc::blame_runs`].
pub(crate) fn insert_clocks(clock_before: u32, edits: &[Edit]) -> Vec<Vec<u32>> {
    let mut next = clock_before;
    edits
        .iter()
        .map(|e| match e {
            Edit::Text { hunks, .. } => {
                let mut out = vec![0u32; hunks.len()];
                for (k, h) in hunks.iter().enumerate().rev() {
                    out[k] = next;
                    next += h.insert.encode_utf16().count() as u32;
                }
                out
            }
        })
        .collect()
}

impl YrsDoc {
    /// Drop the cached texts (the doc is about to change).
    fn forget_texts(&mut self) {
        self.text_cache
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    fn has_pending(&self) -> bool {
        let txn = self.doc.transact();
        let s = txn.store();
        s.pending_update().is_some() || s.pending_ds().is_some()
    }

    /// Refuse to read engine state out of a poisoned (or pending) doc.
    fn usable(&self) -> Result<(), DocError> {
        if self.poisoned || self.has_pending() {
            Err(DocError::Pending)
        } else {
            Ok(())
        }
    }

    /// A throwaway replica of this doc (its full state applied to a fresh Yrs doc), with
    /// the replica's handle on text root `root`. Nothing about it outlives the caller.
    fn replica(&self, client: u64, root: &str) -> Result<Replica, DocError> {
        let replica = new_doc(client);
        let t = root_text(&replica, root);
        let full = guarded(|| {
            Ok(self
                .doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default()))
        })?;
        guarded(|| {
            let u = Update::decode_v1(&full).map_err(|e| DocError::Apply(e.to_string()))?;
            replica
                .transact_mut()
                .apply_update(u)
                .map_err(|e| DocError::Apply(e.to_string()))
        })?;
        Ok(Replica {
            text: t,
            doc: replica,
        })
    }

    /// Make the change `plans` as `client` on this doc and return its update: on return the
    /// doc holds the change. Today's path is the only one in a build without the test-only
    /// `differential` hooks ([`crate::differential`]), which may choose another per thread.
    fn make_change(&mut self, client: ClientId, plans: &[TextPlan]) -> Result<Vec<u8>, DocError> {
        #[cfg(any(test, feature = "differential"))]
        {
            crate::differential::make(self, client, plans)
        }
        #[cfg(not(any(test, feature = "differential")))]
        {
            self.checked_change(client, plans)
        }
    }

    /// The edit path (v0.9.3): the change made **on this doc, as `client`**
    /// (`transact_mut_as`, the k2-yrs patch), with the sampled canary ([`Checks`]). The caller
    /// holds the doc exclusively (`&mut`; a session's lock), so no one else writes as `client`
    /// meanwhile, and the doc holds everything `client` ever wrote: it is the only writer of
    /// every client ID it edits as (`session::one_writer_tests`).
    pub(crate) fn checked_change(
        &mut self,
        client: ClientId,
        plans: &[TextPlan],
    ) -> Result<Vec<u8>, DocError> {
        let sampled = self.checks.now;
        if self.checks.fallback {
            let update = self.throwaway_update(client, plans)?;
            self.apply_trusted(&update)?;
            return Ok(update);
        }
        if !sampled {
            return self.main_doc_update(client, plans);
        }
        prof!("engine.canary");
        // the canary: the same edit the old way, from the state before it
        let before = self.encode_v2()?;
        let want = self.throwaway_update(client, plans)?;
        let got = self.main_doc_update(client, plans)?;
        if got == want {
            return Ok(got);
        }
        // never serve the fast result: back to the state before, the throwaway's update on it
        self.reset_to(&before)?;
        self.apply_trusted(&want)?;
        self.checks.fallback = true;
        self.checks.found.push(EngineCheck::EditPath { client });
        Ok(want)
    }

    /// The change made on this doc as `client`, in one transaction (`transact_mut_as`): each
    /// root's hunks back to front, so earlier offsets stay valid. O(the change) plus Yrs's
    /// walk to each position. A panic inside poisons the doc (it may have half-applied).
    fn main_doc_update(
        &mut self,
        client: ClientId,
        plans: &[TextPlan],
    ) -> Result<Vec<u8>, DocError> {
        self.forget_texts();
        #[cfg(any(test, feature = "differential"))]
        let planted = crate::differential::planted_divergence();
        #[cfg(not(any(test, feature = "differential")))]
        let planted = false;
        let (doc, texts) = (&self.doc, &self.texts);
        let made = guarded(|| {
            let mut txn = {
                prof!("engine.txn_begin");
                doc.transact_mut_as(ClientID::new(client))
            };
            prof!("engine.txn_ops");
            for (root, hunks, pos) in plans {
                let t = &texts[root];
                if planted {
                    // test builds only: a fast path that diverges (front to back)
                    let mut delta: i64 = 0;
                    for (h, &(s16, len16)) in hunks.iter().zip(pos.iter()) {
                        let at = (i64::from(s16) + delta) as u32;
                        if len16 > 0 {
                            t.remove_range(&mut txn, at, len16);
                        }
                        let ins = h.insert.encode_utf16().count() as i64;
                        if ins > 0 {
                            t.insert(&mut txn, at, &h.insert);
                        }
                        delta += ins - i64::from(len16);
                    }
                    continue;
                }
                for (h, &(s16, len16)) in hunks.iter().zip(pos.iter()).rev() {
                    if len16 > 0 {
                        // upstream deletes what it can past the end, panics, and the
                        // transaction commits as it unwinds: never ask for that
                        debug_assert!(u64::from(s16) + u64::from(len16) <= u64::from(t.len(&txn)));
                        t.remove_range(&mut txn, s16, len16);
                    }
                    if !h.insert.is_empty() {
                        t.insert(&mut txn, s16, &h.insert);
                    }
                }
            }
            let update = {
                prof!("engine.encode_update");
                txn.encode_update_v1()
            };
            {
                prof!("engine.txn_commit");
                drop(txn);
            }
            Ok(update)
        });
        if made.is_err() {
            self.poisoned = true;
        }
        made
    }

    /// Replace this doc's state with `state` (v2, from [`YrsDoc::encode_v2`] of this doc):
    /// the canary's way back. The checks' state stays.
    fn reset_to(&mut self, state: &[u8]) -> Result<(), DocError> {
        let doc = new_doc(SESSION_CLIENT);
        watch_positions(&doc, &self.positions, self.checks.position_every);
        let texts: HashMap<&'static str, TextRef> = self
            .texts
            .keys()
            .map(|n| (*n, root_text(&doc, n)))
            .collect();
        guarded(|| {
            let u = Update::decode_v2(state).map_err(|e| DocError::Load(e.to_string()))?;
            doc.transact_mut()
                .apply_update(u)
                .map_err(|e| DocError::Load(e.to_string()))
        })?;
        self.doc = doc;
        self.texts = texts;
        self.poisoned = false;
        self.forget_texts();
        Ok(())
    }

    /// The canary on the text cache: each cached text against a fresh read. A mismatch is
    /// replaced by the fresh text, reported, and the cache is off until reload.
    fn check_text_cache(&mut self) {
        if self.checks.cache_off {
            // nothing cached is read any more
            self.forget_texts();
            return;
        }
        let mut stale = Vec::new();
        {
            let cache = self.text_cache.get_mut().unwrap_or_else(|p| p.into_inner());
            for (name, cached) in cache.iter() {
                let fresh = self.texts[name].get_string(&self.doc.transact());
                if *cached != fresh {
                    stale.push(*name);
                }
            }
        }
        if !stale.is_empty() {
            self.forget_texts();
            self.checks.cache_off = true;
            for root in stale {
                self.checks.found.push(EngineCheck::TextCache { root });
            }
        }
    }

    /// How often the canary samples an edit, at the least (0: never). The next sample is
    /// `every` edits from now below the scale, and past it at a phase within the interval
    /// derived from the doc's history (`Checks::restart`): the session sets the rate after
    /// every load, and the long-run rate survives reloads.
    pub(crate) fn set_check_every(&mut self, every: u32) {
        self.checks.every = every;
        self.checks.restart(history_units(&self.doc));
    }

    /// The history (UTF-16 units) past which the canary's interval grows with the doc's size
    /// (0: a fixed rate). The next sample is placed as for [`YrsDoc::set_check_every`].
    pub(crate) fn set_check_scale(&mut self, units: u64) {
        self.checks.scale = units;
        self.checks.restart(history_units(&self.doc));
    }

    /// The doc's history in UTF-16 units (the sum of its state vector's clocks), the
    /// canary's measure of its size (test-only: the soak prints it, v0.9.7).
    #[cfg(any(test, feature = "differential"))]
    pub(crate) fn history_units_for_tests(&self) -> u64 {
        history_units(&self.doc)
    }

    /// The position index's counters (k2.3): lookups, marker hits, walks, shadow checks,
    /// mismatches, stale markers.
    #[cfg(any(test, feature = "differential"))]
    pub(crate) fn position_stats(&self) -> [u64; 6] {
        let s = self.doc.position_index_stats();
        [
            s.lookups,
            s.marker_hits,
            s.walks,
            s.shadow_checks,
            s.mismatches,
            s.stale_markers,
        ]
    }

    /// The position index's shadow rate: one marker lookup in `every` also walks the text
    /// from its start and compares (0: never).
    pub(crate) fn set_position_every(&mut self, every: u32) {
        self.checks.position_every = every;
        self.doc.set_position_shadow_rate(every);
    }

    /// Test-only: plant a bug in this doc's position index (until it is reset or reloaded).
    #[cfg(feature = "planted-position-bugs")]
    pub(crate) fn plant_position_bug(&self, bug: yrs::PlantedPositionBug) {
        self.doc.plant_position_bug(Some(bug));
    }

    /// What the canary and the position index found since the last call.
    pub(crate) fn take_checks(&mut self) -> Vec<EngineCheck> {
        let mut found = std::mem::take(&mut self.checks.found);
        found.append(&mut self.positions.lock().unwrap_or_else(|p| p.into_inner()));
        found
    }

    /// Whether the canary switched this doc to the throwaway path (until reload).
    pub(crate) fn on_fallback(&self) -> bool {
        self.checks.fallback
    }

    /// Today's edit path: the change made on a throwaway replica of this doc (its full
    /// current state, applied to a fresh Yrs doc with `client`'s ID), edited, encoded and
    /// dropped. Nothing about the replica outlives the call. The doc itself doesn't change:
    /// the caller applies the update.
    pub(crate) fn throwaway_update(
        &self,
        client: ClientId,
        plans: &[TextPlan],
    ) -> Result<Vec<u8>, DocError> {
        // The replica lives only inside this function: built from the session, edited as
        // `client`, encoded, dropped. Nothing about it outlives the call.
        let replica = new_doc(client);
        let rtexts: HashMap<&str, TextRef> = self
            .texts
            .keys()
            .map(|n| (*n, root_text(&replica, n)))
            .collect();
        let full = guarded(|| {
            Ok(self
                .doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default()))
        })?;
        guarded(|| {
            let u = Update::decode_v1(&full).map_err(|e| DocError::Apply(e.to_string()))?;
            replica
                .transact_mut()
                .apply_update(u)
                .map_err(|e| DocError::Apply(e.to_string()))
        })?;
        let mut txn = replica.transact_mut();
        for (root, hunks, pos) in plans {
            let t = &rtexts[root];
            // Back to front, so earlier offsets stay valid.
            for (h, &(s16, len16)) in hunks.iter().zip(pos.iter()).rev() {
                if len16 > 0 {
                    debug_assert!(u64::from(s16) + u64::from(len16) <= u64::from(t.len(&txn)));
                    t.remove_range(&mut txn, s16, len16);
                }
                if !h.insert.is_empty() {
                    t.insert(&mut txn, s16, &h.insert);
                }
            }
        }
        Ok(txn.encode_update_v1())
    }

    /// The full state, v2, under `catch_unwind` even when the doc looks usable.
    fn encode_v2(&self) -> Result<Vec<u8>, DocError> {
        guarded(|| {
            Ok(self
                .doc
                .transact()
                .encode_state_as_update_v2(&StateVector::default()))
        })
    }
}

/// Test-only engine pieces of the differential hooks ([`crate::differential`]).
#[cfg(any(test, feature = "differential"))]
impl YrsDoc {
    /// A separate doc with this doc's state (its v2 encoding, applied to a fresh doc with the
    /// same roots): the reference side of a shadowed edit.
    pub(crate) fn twin(&self) -> Result<YrsDoc, DocError> {
        self.usable()?;
        let state = self.encode_v2()?;
        let doc = new_doc(SESSION_CLIENT);
        let positions = Arc::new(Mutex::new(Vec::new()));
        let checks = Checks::new();
        watch_positions(&doc, &positions, checks.position_every);
        let texts: HashMap<&'static str, TextRef> = self
            .texts
            .keys()
            .map(|n| (*n, root_text(&doc, n)))
            .collect();
        guarded(|| {
            let u = Update::decode_v2(&state).map_err(|e| DocError::Load(e.to_string()))?;
            doc.transact_mut()
                .apply_update(u)
                .map_err(|e| DocError::Load(e.to_string()))
        })?;
        Ok(YrsDoc {
            doc,
            texts,
            poisoned: false,
            text_cache: Mutex::new(HashMap::new()),
            checks,
            positions,
        })
    }

    /// The full state, v2 (`None` on a poisoned doc), and the state vector, sorted.
    pub(crate) fn snapshot_for_tests(&self) -> (Option<Vec<u8>>, Vec<(u64, u32)>) {
        let state = if self.usable().is_ok() {
            self.encode_v2().ok()
        } else {
            None
        };
        let mut sv: Vec<(u64, u32)> = self
            .doc
            .transact()
            .state_vector()
            .iter()
            .map(|(c, k)| (c.get(), *k))
            .collect();
        sv.sort_unstable();
        (state, sv)
    }

    /// Each root's text read from the engine (not the cache), in root-name order.
    pub(crate) fn texts_for_tests(&self) -> Vec<(&'static str, String)> {
        let mut v: Vec<(&'static str, String)> = self
            .texts
            .iter()
            .map(|(n, t)| (*n, t.get_string(&self.doc.transact())))
            .collect();
        v.sort_unstable();
        v
    }

    /// A wrong cached text for `root` (the canary's test).
    pub(crate) fn corrupt_text_cache(&mut self, root: &str, text: &str) {
        if let Some((name, _)) = self.texts.get_key_value(root) {
            let name = *name;
            self.text_cache
                .get_mut()
                .unwrap_or_else(|p| p.into_inner())
                .insert(name, text.to_owned());
        }
    }

    /// Mutant edit path: the throwaway replica, with each root's hunks applied **front to
    /// back** (offsets shifted as it goes). The same text, but a change's insertions take
    /// their clocks in another order, so its update differs whenever it inserts at two
    /// places. The doc doesn't change (the caller applies the update).
    pub(crate) fn mutant_front_to_back(
        &self,
        client: ClientId,
        plans: &[TextPlan],
    ) -> Result<Vec<u8>, DocError> {
        let replica = self.replica(client, plans.first().map_or("", |p| p.0))?.doc;
        let rtexts: HashMap<&str, TextRef> = self
            .texts
            .keys()
            .map(|n| (*n, root_text(&replica, n)))
            .collect();
        guarded(|| {
            let mut txn = replica.transact_mut();
            for (root, hunks, pos) in plans {
                let t = &rtexts[root];
                let mut delta: i64 = 0;
                for (h, &(s16, len16)) in hunks.iter().zip(pos.iter()) {
                    let at = (i64::from(s16) + delta) as u32;
                    if len16 > 0 {
                        t.remove_range(&mut txn, at, len16);
                    }
                    let ins = h.insert.encode_utf16().count() as i64;
                    if ins > 0 {
                        t.insert(&mut txn, at, &h.insert);
                    }
                    delta += ins - i64::from(len16);
                }
            }
            Ok(txn.encode_update_v1())
        })
    }

    /// Mutant edit path: `client` edits on `kept`, a replica of this doc taken at its first
    /// edit and never synced after (a stale base: what a kept replica that missed updates
    /// would do). Positions are this doc's, applied to the old replica.
    pub(crate) fn mutant_stale(
        &self,
        client: ClientId,
        plans: &[TextPlan],
        kept: &mut HashMap<ClientId, Doc>,
    ) -> Result<Vec<u8>, DocError> {
        let replica = match kept.entry(client) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(self.replica(client, plans.first().map_or("", |p| p.0))?.doc)
            }
        };
        let replica = &*replica;
        let rtexts: HashMap<&str, TextRef> = plans
            .iter()
            .map(|p| (p.0, root_text(replica, p.0)))
            .collect();
        guarded(|| {
            let mut txn = replica.transact_mut();
            for (root, hunks, pos) in plans {
                let t = &rtexts[root];
                for (h, &(s16, len16)) in hunks.iter().zip(pos.iter()).rev() {
                    let len = t.len(&txn);
                    let s = s16.min(len);
                    let l = len16.min(len - s);
                    if l > 0 {
                        t.remove_range(&mut txn, s, l);
                    }
                    if !h.insert.is_empty() {
                        t.insert(&mut txn, s, &h.insert);
                    }
                }
            }
            Ok(txn.encode_update_v1())
        })
    }
}

impl YrsDoc {
    /// Check (as [`EngineDoc::check_update`]) and apply an update from a connection that owns
    /// client `allowed`, decoded once ([`decode_accepted`]): nothing is applied unless every
    /// check passed.
    #[cfg_attr(not(feature = "untrusted-wire"), allow(dead_code))]
    pub(crate) fn apply_client(
        &mut self,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<UpdateInfo, DocError> {
        if self.poisoned {
            return Err(DocError::Pending);
        }
        let u = decode_client(update)?;
        let info = self.check_decoded(&u, allowed)?;
        self.apply_decoded(u)?;
        Ok(info)
    }

    /// The client-ID and continuity checks of [`EngineDoc::check_update`] on a decoded update.
    fn check_decoded(&self, u: &Update, allowed: ClientId) -> Result<UpdateInfo, DocError> {
        guarded(|| {
            // Each client's upper clock over every content block (items and GC; skips carry
            // nothing). Not `Update::state_vector`: that counts only runs starting at clock
            // 0, so it says nothing about a held block from the middle of a history.
            let mut upper: HashMap<ClientId, u32> = HashMap::new();
            for (c, ranges) in u.insertions(true).iter() {
                let end = ranges.iter().map(|r| r.end).max().unwrap_or(0);
                let e = upper.entry(c.get()).or_insert(0);
                *e = (*e).max(end);
            }
            // The upper state vector too, in case it names a client the set doesn't.
            for (c, clock) in u.state_vector().iter() {
                let e = upper.entry(c.get()).or_insert(0);
                *e = (*e).max(*clock);
            }
            let mut clients: Vec<(ClientId, u32)> = upper.into_iter().collect();
            clients.sort_unstable();
            let have = self.doc.transact().state_vector();
            let mut inserts = Vec::new();
            for &(c, up) in &clients {
                // a client listed with no content range is not "known"
                let known = up != 0 && up <= have.get(&ClientID::new(c));
                if c != allowed && !known {
                    return Err(DocError::ForeignClient { allowed, found: c });
                }
                if !known {
                    inserts.push(c);
                }
            }
            continues_each_client(&have, u)?;
            Ok(UpdateInfo {
                delete_only: inserts.is_empty(),
                inserts_from: inserts,
            })
        })
    }

    /// Apply a decoded update. Anything after decoding that goes wrong (an apply error, a
    /// panic, or unresolved dependencies left pending) **poisons** the doc.
    fn apply_decoded(&mut self, u: Update) -> Result<(), DocError> {
        if self.poisoned {
            return Err(DocError::Pending);
        }
        self.forget_texts();
        let applied = guarded(|| {
            self.doc
                .transact_mut()
                .apply_update(u)
                .map_err(|e| DocError::Apply(e.to_string()))
        });
        if let Err(e) = applied {
            self.poisoned = true;
            return Err(e);
        }
        if self.has_pending() {
            self.poisoned = true;
            return Err(DocError::Pending);
        }
        Ok(())
    }

    /// Whether this doc and `other` hold the same inserts: their state vectors are equal
    /// (compared as maps, not as encoded bytes, whose order is a hash map's). O(clients).
    pub(crate) fn same_state_vector(&self, other: &YrsDoc) -> bool {
        self.doc.transact().state_vector() == other.doc.transact().state_vector()
    }

    /// The delete set as merged clock ranges per client, sorted: equal for equal deletions
    /// however each doc split its blocks.
    fn deleted_ranges(&self) -> Vec<(u64, Vec<(u32, u32)>)> {
        let ds = self.doc.transact().snapshot().delete_set;
        let mut out: Vec<(u64, Vec<(u32, u32)>)> = ds
            .iter()
            .map(|(client, ranges)| {
                let mut v: Vec<(u32, u32)> = ranges.iter().map(|r| (r.start, r.end)).collect();
                v.sort_unstable();
                let mut merged: Vec<(u32, u32)> = Vec::with_capacity(v.len());
                for (s, e) in v {
                    match merged.last_mut() {
                        Some(last) if s <= last.1 => last.1 = last.1.max(e),
                        _ => merged.push((s, e)),
                    }
                }
                (client.get(), merged)
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// Whether this doc and `other` hold the same content: the same inserts (state
    /// vectors), the same deletions (delete sets, as merged ranges) and the same texts. Not
    /// their encodings: upstream can split identical content into blocks differently on two
    /// replicas, so equal docs may encode differently. O(doc).
    pub(crate) fn same_content(&self, other: &YrsDoc) -> bool {
        self.same_state_vector(other)
            && self.deleted_ranges() == other.deleted_ranges()
            && self.texts.keys().all(|r| self.text(r) == other.text(r))
    }

    /// Whether this doc holds updates it couldn't integrate (missing dependencies: yrs's
    /// pending structs or delete set) or is poisoned. Never true on a correct stream: an
    /// apply that leaves anything pending poisons the doc and fails. K-AUD-1 (v0.9.5).
    pub(crate) fn unresolved(&self) -> bool {
        self.poisoned || self.has_pending()
    }
}

#[cfg(feature = "conformance")]
impl YrsDoc {
    /// The state vector as sorted `(client, clock)` pairs: equal for equal states (the
    /// encoded vector's order is a hash map's).
    pub(crate) fn state_clocks(&self) -> Vec<(ClientId, u32)> {
        let mut v: Vec<(ClientId, u32)> = self
            .doc
            .transact()
            .state_vector()
            .iter()
            .map(|(c, k)| (c.get(), *k))
            .collect();
        v.sort_unstable();
        v
    }

    /// What this doc has past `clocks` (a state this doc has seen, as from
    /// [`YrsDoc::state_clocks`]), as an update: the inserts past each client's clock, plus
    /// the doc's whole delete set (yrs doesn't version deletes).
    pub(crate) fn events_since(&self, clocks: &[(ClientId, u32)]) -> Result<Vec<u8>, DocError> {
        self.usable()?;
        let mut sv = StateVector::default();
        for &(c, k) in clocks {
            sv.set_max(ClientID::new(c), k);
        }
        guarded(|| Ok(self.doc.transact().encode_state_as_update_v1(&sv)))
    }

    /// Deliver an update another of this kit's docs made in this process: refused with
    /// [`DocError::MissingDeps`] (nothing changes) when it doesn't continue each client's
    /// history ([`continues_each_client`], the check client updates get), else applied as
    /// trusted bytes ([`EngineDoc::apply_trusted`], which poisons the doc if dependencies are
    /// still left unresolved).
    pub(crate) fn deliver_local(&mut self, update: &[u8]) -> Result<(), DocError> {
        if self.poisoned {
            return Err(DocError::Pending);
        }
        let u = decode_accepted(update)?;
        continues_each_client(&self.doc.transact().state_vector(), &u)?;
        self.apply_decoded(u)
    }

    /// A relative position in text root `root` at UTF-16 offset `at16`, stuck to the
    /// character after it (`right`) or before it. `None` past the end, or at the end when
    /// stuck right (no character there).
    pub(crate) fn sticky(&self, root: &str, at16: u32, right: bool) -> Option<StickyIndex> {
        let assoc = if right { Assoc::After } else { Assoc::Before };
        let txn = self.doc.transact();
        self.texts[root].sticky_index(&txn, at16, assoc)
    }

    /// Where `pos` is now, as a UTF-16 offset of its text root; `None` when its root or
    /// character isn't in this doc.
    pub(crate) fn resolve_sticky(&self, pos: &StickyIndex) -> Option<u32> {
        let txn = self.doc.transact();
        pos.get_offset(&txn).map(|o| o.index)
    }
}

impl EngineDoc for YrsDoc {
    fn new(schema: &Schema) -> Self {
        let doc = new_doc(SESSION_CLIENT);
        let positions = Arc::new(Mutex::new(Vec::new()));
        let checks = Checks::new();
        watch_positions(&doc, &positions, checks.position_every);
        let texts = roots(&doc, schema);
        Self {
            doc,
            texts,
            poisoned: false,
            text_cache: Mutex::new(HashMap::new()),
            checks,
            positions,
        }
    }

    fn load(schema: &Schema, state: &[u8]) -> Result<Self, DocError> {
        let e = Self::new(schema);
        guarded(|| {
            let u = Update::decode_v2(state).map_err(|err| DocError::Load(err.to_string()))?;
            stored_accepted(&u)?;
            e.doc
                .transact_mut()
                .apply_update(u)
                .map_err(|err| DocError::Load(err.to_string()))
        })?;
        if e.has_pending() {
            return Err(DocError::Load(
                "stored state has unresolved dependencies".into(),
            ));
        }
        Ok(e)
    }

    fn text(&self, root: &str) -> String {
        let (name, t) = self.texts.get_key_value(root).expect("a schema root");
        if self.checks.cache_off {
            return t.get_string(&self.doc.transact());
        }
        let mut cache = self.text_cache.lock().unwrap_or_else(|p| p.into_inner());
        cache
            .entry(*name)
            .or_insert_with(|| t.get_string(&self.doc.transact()))
            .clone()
    }

    fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    fn encode_state(&self) -> Result<Vec<u8>, DocError> {
        self.usable()?;
        self.encode_v2()
    }

    fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError> {
        if !state_vector_well_formed(remote_sv) {
            return Err(DocError::Decode("malformed state vector".into()));
        }
        self.usable()?;
        guarded(|| {
            let sv =
                StateVector::decode_v1(remote_sv).map_err(|e| DocError::Decode(e.to_string()))?;
            Ok(self.doc.transact().encode_state_as_update_v1(&sv))
        })
    }

    /// The client-ID rule for an update from the connection that owns client `allowed`:
    ///
    /// - **New content** (blocks past what this doc has) may come only from `allowed`.
    /// - Blocks from **other clients** are accepted only when **already known**: for each
    ///   such client, the update's upper clock is at most this doc's state for it (every
    ///   block lies within what the doc has). Real Yjs clients send these: a client that
    ///   went offline holding another client's update as pending includes it in its resync.
    ///   Applying them is a no-op (blocks are identified by client and clock, and yrs skips
    ///   what it already has), so even forged content under a known ID changes nothing.
    /// - A block from another client one clock past this doc's state is `ForeignClient`.
    /// - The update must not skip ahead of the doc for any client (`MissingDeps`).
    ///
    /// `inserts_from` lists only clients with genuinely new insertions (at most `allowed`).
    fn check_update(&self, update: &[u8], allowed: ClientId) -> Result<UpdateInfo, DocError> {
        let u = decode_client(update)?;
        self.check_decoded(&u, allowed)
    }

    /// Apply a kit-made update. A decode error changes nothing. Anything after decoding
    /// that goes wrong (an apply error, a panic, or unresolved dependencies left pending)
    /// **poisons** the doc: from then on it refuses to encode or edit (`Pending`), and its
    /// owner must reload it from the store.
    fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError> {
        if self.poisoned {
            return Err(DocError::Pending);
        }
        let u = guarded(|| Update::decode_v1(update).map_err(|e| DocError::Decode(e.to_string())))?;
        // the store-load scan: the store's log is replayed through here
        stored_accepted(&u)?;
        self.apply_decoded(u)
    }

    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Vec<u8>, DocError> {
        self.usable()?;
        prof!("engine.edit_as");
        // the canary (v0.9.3): a sampled edit first checks the text cache its positions are
        // computed against, and is then made both ways (`checked_change`)
        self.checks.now = self.checks.sample(|| history_units(&self.doc));
        if self.checks.now {
            self.check_text_cache();
        }
        // Positions are computed against the session's current text, before the replica exists.
        let mut before: Vec<String> = Vec::with_capacity(edits.len());
        let plans: Vec<TextPlan> = edits
            .iter()
            .map(|e| match e {
                Edit::Text { root, hunks } => {
                    prof!("engine.positions");
                    let cur = self.text(root);
                    let pos = to_utf16(&cur, hunks);
                    before.push(cur);
                    (root.as_str(), hunks.as_slice(), pos)
                }
            })
            .collect();

        let update = {
            prof!("engine.make_change");
            self.make_change(client, &plans)?
        };
        prof!("engine.text_cache");
        // the texts it made are known: no walk of the doc to read them again (a root edited
        // twice in one call is read afresh)
        let cache_on = !self.checks.cache_off;
        let mut cache = self.text_cache.lock().unwrap_or_else(|p| p.into_inner());
        for (k, (root, hunks, _)) in plans.iter().enumerate() {
            if cache_on && plans.iter().filter(|p| p.0 == *root).count() == 1 {
                if let Some((name, _)) = self.texts.get_key_value(*root) {
                    let after = crate::text::diff::apply(&before[k], hunks);
                    debug_assert_eq!(
                        after,
                        self.texts[*root].get_string(&self.doc.transact()),
                        "the cached text is the doc's"
                    );
                    cache.insert(*name, after);
                }
            }
        }
        drop(cache);
        if self.checks.now {
            self.check_text_cache();
        }
        Ok(update)
    }

    /// `client`'s entry of the state vector (0 for a client the doc has never seen), read
    /// alone (v0.9.8: building the whole vector was O(clients), and the session reads it at
    /// least twice per edit).
    fn clock_of(&self, client: ClientId) -> u32 {
        self.doc.transact().client_state(&ClientID::new(client))
    }

    /// `diff_range(now, empty)` (proven in D0, Q5): with the empty snapshot as the low end,
    /// every live item is "added", so each comes back as its own run with its ID, GC on.
    /// It **splits blocks** (it takes `&mut` of a transaction), so it runs on a throwaway
    /// replica, never on this doc: the cost is a full encode, decode and walk, O(doc).
    fn blame_runs(&self, root: &str) -> Result<Vec<BlameRun>, DocError> {
        self.usable()?;
        let r = self.replica(SESSION_CLIENT, root)?;
        let (replica, t) = (&r.doc, &r.text);
        guarded(|| {
            let mut txn = replica.transact_mut();
            let now = txn.snapshot();
            let diffs = t.diff_range(&mut txn, Some(&now), Some(&Snapshot::default()), |c| c);
            let mut out: Vec<BlameRun> = Vec::with_capacity(diffs.len());
            // the clock just past the last run (its first clock plus its UTF-16 length)
            let mut end = 0u32;
            for d in diffs {
                let (Out::Any(Any::String(s)), Some(y)) = (&d.insert, &d.ychange) else {
                    return Err(DocError::Apply("blame: a run that is not text".into()));
                };
                if y.kind != YChangeKind::Added {
                    return Err(DocError::Apply("blame: a run that is not live".into()));
                }
                let len16 = s.encode_utf16().count() as u32;
                let (client, clock) = (y.id.client.get(), y.id.clock);
                // adjacent items of one client with consecutive clocks are one run
                match out.last_mut() {
                    Some(last) if last.client == client && clock == end => {
                        last.bytes += s.len();
                    }
                    _ => out.push(BlameRun {
                        bytes: s.len(),
                        client,
                        clock,
                    }),
                }
                end = clock + len16;
            }
            drop(txn);
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;

    const S: Schema = Schema {
        name: "t",
        roots: &[("source", Kind::Text)],
    };

    /// A remote client (Yjs stand-in) synced from the doc.
    fn client_doc(id: u64, from: &Doc) -> Client {
        let d = new_doc(id);
        let t = root_text(&d, "source");
        let sv = d.transact().state_vector().encode_v1();
        let u = from.encode_diff(&sv).unwrap();
        d.transact_mut()
            .apply_update(Update::decode_v1(&u).unwrap())
            .unwrap();
        Client { text: t, doc: d }
    }

    /// A remote client's doc and its text handle.
    struct Client {
        // handles drop before their Doc (fields drop in declaration order; rule 8)
        text: TextRef,
        doc: yrs::Doc,
    }

    /// Whether any 8-byte window of `secret` appears in `hay` (the purge scan's rule).
    fn holds_any_window(hay: &[u8], secret: &[u8]) -> bool {
        secret.windows(8).any(|w| hay.windows(8).any(|h| h == w))
    }

    /// E-ERASE-10 / K-PURGE-11: with GC on, a deleted secret is absent from the encoded
    /// state (and from a doc loaded from it). The control runs the same steps on a Yrs doc
    /// with `skip_gc: true` and must find the secret, so the check can fail.
    #[test]
    fn gc_is_on() {
        let secret = crate::hash::hex(&crate::hash::random32().unwrap());
        let mut d = Doc::new(S).unwrap();
        d.replace_text_as(500, "source", &format!("a {secret} b\n"))
            .unwrap();
        d.replace_text_as(501, "source", "a  b\n").unwrap();
        let state = d.encode_state().unwrap();
        assert!(!holds_any_window(&state, secret.as_bytes()));
        let again = Doc::load(S, &state).unwrap();
        assert_eq!(again.text("source").unwrap(), "a  b\n");
        assert!(!holds_any_window(
            &again.encode_state().unwrap(),
            secret.as_bytes()
        ));
        // the mutant: GC off keeps the deleted content in the state
        let m = yrs::Doc::with_options(Options {
            client_id: ClientID::new(500),
            offset_kind: OffsetKind::Utf16,
            skip_gc: true,
            ..Options::default()
        });
        let t = root_text(&m, "source");
        t.insert(&mut m.transact_mut(), 0, &format!("a {secret} b\n"));
        let n = secret.len() as u32;
        t.remove_range(&mut m.transact_mut(), 2, n);
        let kept = m
            .transact()
            .encode_state_as_update_v2(&StateVector::default());
        assert!(
            holds_any_window(&kept, secret.as_bytes()),
            "the skip_gc control must keep the secret"
        );
    }

    /// The canary's adaptive interval (v0.9.7): below the scale, one edit in `every` is
    /// sampled, the first at edit `every`, as at a fixed rate; past it, the interval grows in
    /// proportion to the history (`every x units / scale`), so at about 10x the scale it is
    /// about 10x. Setting the rate again, as a reload does, puts the next sample at the
    /// history's phase within the interval ([`canary_phase`]), on the same doc and on one
    /// loaded afresh. Scale 0 is a fixed rate, and setting the scale restarts
    /// the count.
    #[test]
    fn the_canary_interval_grows_with_the_history_past_the_scale() {
        const EVERY: u64 = 2;
        const SCALE: u64 = 50;
        let (append, run) = (canary_append, canary_run);
        let mut y = YrsDoc::new(&S);
        y.set_check_every(EVERY as u32);
        y.set_check_scale(SCALE);

        // under the scale: the first sample at edit 2, then every 2 edits
        let small = run(&mut y, 40);
        let at: Vec<u64> = small.iter().map(|s| s.0).collect();
        assert_eq!(at, (1..=20).map(|k| k * EVERY).collect::<Vec<_>>());
        assert!(small.iter().all(|s| s.1 < SCALE));
        assert!(!y.on_fallback() && y.take_checks().is_empty());

        // about 10x the scale: one unsampled edit (41) takes the history to 500 units
        assert_eq!(append(&mut y, &"y".repeat(460)), None);
        assert_eq!(history_units(&y.doc), 10 * SCALE);
        let large = run(&mut y, 200);
        assert_eq!(large[0], (42, 10 * SCALE), "the sample due at edit 42");
        for w in large.windows(2) {
            let ((a, units), (b, _)) = (w[0], w[1]);
            assert_eq!(b - a, EVERY.max(EVERY * units / SCALE), "{w:?}");
        }
        // the history went from 10x to 14x the scale: gaps of 20 to 28 edits, not 2
        assert!(large
            .windows(2)
            .all(|w| (20..=28).contains(&(w[1].0 - w[0].0))));
        assert!(
            (7..=11).contains(&large.len()),
            "{} samples in 200 edits (100 at the fixed rate)",
            large.len()
        );
        assert!(!y.on_fallback() && y.take_checks().is_empty());

        // setting the rate again, as the session does after a load: the next sample is at the
        // history's phase within the interval (14x the scale: an interval of 28 edits)
        assert_eq!(append(&mut y, &"z".repeat(10)), None);
        let units = history_units(&y.doc);
        assert_eq!(units, 710, "the history after 250 more edits");
        let interval = EVERY * units / SCALE;
        assert_eq!(interval, 28);
        y.set_check_every(EVERY as u32);
        let from = y.checks.edits;
        let after: Vec<u64> = run(&mut y, interval).iter().map(|s| s.0 - from).collect();
        assert_eq!(
            after,
            vec![canary_phase(units, interval)],
            "the first sample after setting the rate"
        );
        // a doc loaded afresh from the state, its rate set as the session sets it: its edit
        // count starts at 0, and its first sample is at the phase, not at edit `every`
        let state = y.encode_state().unwrap();
        let mut l = YrsDoc::load(&S, &state).unwrap();
        l.set_check_every(EVERY as u32);
        l.set_check_scale(SCALE);
        let units = history_units(&l.doc);
        assert_eq!(units, 710 + interval);
        let interval = EVERY * units / SCALE;
        let first: Vec<u64> = run(&mut l, interval).iter().map(|s| s.0).collect();
        assert_eq!(
            first,
            vec![canary_phase(units, interval)],
            "the first sample after a load"
        );
        assert!(!l.on_fallback() && l.take_checks().is_empty());

        // scale 0: the fixed rate again, from now (the next sample `every` edits on)
        y.set_check_scale(0);
        let from = y.checks.edits;
        let fixed: Vec<u64> = run(&mut y, 20).iter().map(|s| s.0 - from).collect();
        assert_eq!(fixed, (1..=10).map(|k| k * EVERY).collect::<Vec<_>>());
    }

    /// The canary's long-run rate survives frequent reloads (v0.9.7): 200 reloads of a doc
    /// past the scale, each followed by fewer edits than the interval (as in the soak, where
    /// a large doc was reloaded every ~1,700 edits). A first sample a full interval in would
    /// never fire; one at `every` would fire after every reload. At the history's phase the
    /// samples come at about one edit in `interval` over the run.
    #[test]
    fn the_canary_keeps_its_rate_across_reloads_shorter_than_the_interval() {
        // the phase covers the interval (`units mod interval` alone stays under scale/every)
        let interval = 1_000;
        let phases: Vec<u64> = (50_000..51_000)
            .map(|u| canary_phase(u, interval))
            .collect();
        assert!(phases.iter().all(|p| (1..=interval).contains(p)));
        let low = phases.iter().filter(|p| **p <= interval / 2).count();
        assert!(
            (400..=600).contains(&low),
            "{low} of 1,000 phases in the first half"
        );
        assert!(phases.iter().any(|p| *p > 900) && phases.iter().any(|p| *p <= 100));

        const EVERY: u32 = 2;
        const SCALE: u64 = 50;
        let mut y = YrsDoc::new(&S);
        // 20x the scale: an interval of 40 edits, growing to about 50 over the run
        assert_eq!(canary_append(&mut y, &"a".repeat(1000)), None);
        let (mut sampled, mut edits, mut expected) = (0u64, 0u64, 0f64);
        for reload in 0..200u64 {
            let state = y.encode_state().unwrap();
            y = YrsDoc::load(&S, &state).unwrap();
            y.set_check_every(EVERY);
            y.set_check_scale(SCALE);
            let interval = y.checks.interval(history_units(&y.doc));
            // 7 to 17 one-unit edits before the next reload, always under the interval
            let gap = 7 + reload * 5 % 11;
            assert!(gap < interval);
            sampled += canary_run(&mut y, gap).len() as u64;
            edits += gap;
            expected += gap as f64 / interval as f64;
            assert!(!y.on_fallback() && y.take_checks().is_empty());
        }
        let ratio = sampled as f64 / expected;
        assert!(
            (0.7..=1.3).contains(&ratio),
            "{sampled} samples in {edits} edits over 200 reloads; about {expected:.0} at one in \
             `interval`"
        );
    }

    /// Append `s` at the end of `source`; `Some(history before the edit)` when the canary
    /// sampled the edit.
    fn canary_append(y: &mut YrsDoc, s: &str) -> Option<u64> {
        let units = history_units(&y.doc);
        let at = y.text("source").len();
        let e = [Edit::Text {
            root: "source".into(),
            hunks: vec![Hunk {
                start: at,
                end: at,
                insert: s.into(),
            }],
        }];
        y.edit_as(7, &e).unwrap();
        y.checks.now.then_some(units)
    }

    /// `n` one-unit edits: the sampled edits' numbers and their history before them.
    fn canary_run(y: &mut YrsDoc, n: u64) -> Vec<(u64, u64)> {
        (0..n)
            .filter_map(|_| canary_append(y, "x").map(|u| (y.checks.edits, u)))
            .collect()
    }

    #[test]
    fn clock_of_is_the_state_vector_entry() {
        let sv_entry = |y: &YrsDoc, c: u64| y.doc.transact().state_vector().get(&ClientID::new(c));
        let check = |y: &YrsDoc, clients: &[u64]| {
            for &c in clients {
                assert_eq!(y.clock_of(c), sv_entry(y, c), "client {c}");
            }
        };
        let mut y = YrsDoc::new(&S);
        // a doc with nothing yet, and clients it has never seen
        check(&y, &[2, 7, 1_000, MAX_CLIENT_ID]);
        assert_eq!(y.clock_of(7), 0);
        // edits (inserts and deletes) by many clients, one of them many times
        for i in 0..200u64 {
            let c = 2 + i % 37;
            let text = y.text("source");
            // ASCII text: every byte offset is a character boundary
            let at = (i as usize * 7) % (text.len() + 1);
            let mut hunks = vec![crate::text::diff::Hunk {
                start: at,
                end: at,
                insert: format!("c{c}-{i}\n"),
            }];
            if i % 5 == 4 && text.len() > 4 {
                hunks = vec![crate::text::diff::Hunk {
                    start: 0,
                    end: 4,
                    insert: String::new(),
                }];
            }
            let e = vec![Edit::Text {
                root: "source".into(),
                hunks,
            }];
            y.edit_as(c, &e).unwrap();
        }
        let seen: Vec<u64> = (2..2 + 37).collect();
        check(&y, &seen);
        check(&y, &[1, 40, 999, MAX_CLIENT_ID]);
        assert!(seen.iter().all(|&c| y.clock_of(c) > 0));
        // the same after a load from the state (GC'd deletes, merged blocks)
        let state = y.encode_state().unwrap();
        let l = YrsDoc::load(&S, &state).unwrap();
        check(&l, &seen);
        check(&l, &[1, 40, 999, MAX_CLIENT_ID]);
        for &c in &seen {
            assert_eq!(l.clock_of(c), y.clock_of(c), "client {c} after a load");
        }
    }

    #[test]
    fn edit_as_attributes_and_returns_update() {
        let mut e = Doc::new(S).unwrap();
        let u = e
            .replace_text_as(500, "source", "# hi 🎉\n")
            .unwrap()
            .unwrap();
        assert_eq!(e.text("source").unwrap(), "# hi 🎉\n");
        let fresh = Doc::new(S).unwrap();
        assert_eq!(
            fresh.check_client_update(&u, 500).unwrap().inserts_from,
            vec![500]
        );
        assert!(matches!(
            fresh.check_client_update(&u, 501),
            Err(DocError::ForeignClient { found: 500, .. })
        ));
        assert_eq!(e.replace_text_as(500, "source", "# hi 🎉\n").unwrap(), None);
    }

    #[test]
    fn edits_land_at_the_right_utf16_offsets() {
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(10, "source", "日本🎉 abc\n🎉x\n")
            .unwrap();
        e.replace_text_as(11, "source", "日本🎉 aXc\n🎉xy\n")
            .unwrap();
        assert_eq!(e.text("source").unwrap(), "日本🎉 aXc\n🎉xy\n");
    }

    #[test]
    fn session_client_and_bad_ids_are_refused() {
        let mut e = Doc::new(S).unwrap();
        for id in [SESSION_CLIENT, 0, 1 << 53] {
            assert!(
                matches!(
                    e.replace_text_as(id, "source", "x"),
                    Err(DocError::BadClient(_))
                ),
                "{id}"
            );
        }
    }

    #[test]
    fn client_updates_are_checked() {
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(7, "source", "owned by 7\n").unwrap();
        let cct = client_doc(100, &e);
        let (c, ct) = (&cct.doc, &cct.text);
        let ins = {
            let mut t = c.transact_mut();
            ct.insert(&mut t, 0, "hi ");
            t.encode_update_v1()
        };
        assert_eq!(
            e.apply_client_update(&ins, 100).unwrap().inserts_from,
            vec![100]
        );
        let del = {
            let mut t = c.transact_mut();
            ct.remove_range(&mut t, 3, 5);
            t.encode_update_v1()
        };
        assert!(e.apply_client_update(&del, 100).unwrap().delete_only);
        assert_eq!(e.text("source").unwrap(), ct.get_string(&c.transact()));
        let fft = client_doc(7, &e);
        let (f, ft) = (&fft.doc, &fft.text);
        let forged = {
            let mut t = f.transact_mut();
            ft.insert(&mut t, 0, "forged ");
            t.encode_update_v1()
        };
        assert!(matches!(
            e.check_client_update(&forged, 100),
            Err(DocError::ForeignClient { found: 7, .. })
        ));
        let ggt = client_doc(200, &e);
        let (g, gt) = (&ggt.doc, &ggt.text);
        {
            let mut t = g.transact_mut();
            gt.insert(&mut t, 0, "one ");
        }
        let second = {
            let mut t = g.transact_mut();
            gt.insert(&mut t, 0, "two ");
            t.encode_update_v1()
        };
        assert!(matches!(
            e.check_client_update(&second, 200),
            Err(DocError::MissingDeps { client: 200, .. })
        ));
    }

    /// An update from yrs client `c` (a transaction inserting `text` at 0).
    fn typed(c: &Client, text: &str) -> Vec<u8> {
        let mut t = c.doc.transact_mut();
        c.text.insert(&mut t, 0, text);
        t.encode_update_v1()
    }

    fn apply_raw(c: &Client, u: &[u8]) {
        c.doc
            .transact_mut()
            .apply_update(Update::decode_v1(u).unwrap())
            .unwrap();
    }

    /// What a reconnecting client sends: everything it has that the session lacks.
    fn resync(c: &Client, session: &Doc) -> Vec<u8> {
        let sv = StateVector::decode_v1(&session.state_vector()).unwrap();
        c.doc.transact().encode_state_as_update_v1(&sv)
    }

    /// v0.9.5 hardening: the store-load scan. A stored state, or a log entry replayed from
    /// the store, holding subdocument content is never loaded (quarantined); ordinary ones
    /// load. (The in-process delivery between the kit's docs, `deliver_local`, exists only
    /// with the test-only `conformance` feature and refuses it as client updates are
    /// refused.) Built through Yrs's normal API.
    #[test]
    fn stored_subdocument_content_is_quarantined() {
        use yrs::Map;
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(7, "source", "base\n").unwrap();
        let c = client_doc(100, &e);
        let sv = StateVector::decode_v1(&e.state_vector()).unwrap();
        c.doc
            .get_or_insert_map("m")
            .insert(&mut c.doc.transact_mut(), "sub", yrs::Doc::new());
        let u = c.doc.transact().encode_state_as_update_v1(&sv);
        let state = c
            .doc
            .transact()
            .encode_state_as_update_v2(&StateVector::default());
        assert_eq!(Doc::load(S, &state).err(), Some(DocError::Quarantined));
        assert!(Doc::load(S, &e.encode_state().unwrap()).is_ok());
        let mut f = Doc::load(S, &e.encode_state().unwrap()).unwrap();
        let before = f.state_vector();
        assert_eq!(f.apply_trusted(&u), Err(DocError::Quarantined));
        assert_eq!(f.state_vector(), before, "nothing applied");
        assert!(f.encode_state().is_ok(), "not poisoned");
    }

    #[test]
    fn resyncs_may_carry_known_foreign_blocks_only() {
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(7, "source", "base\n").unwrap();
        let a = client_doc(100, &e);
        let b = client_doc(200, &e);
        // b types twice; the session gets both, a gets only the second (the first was
        // lost when a dropped), so a holds it as pending
        let b1 = typed(&b, "b1 ");
        let b2 = typed(&b, "b2 ");
        e.apply_client_update(&b1, 200).unwrap();
        e.apply_client_update(&b2, 200).unwrap();
        apply_raw(&a, &b2);
        assert!(a.doc.transact().store().pending_update().is_some());
        // a types offline, then resyncs: its own content plus b's held, known blocks
        typed(&a, "a1 ");
        let r = resync(&a, &e);
        let before = e.text("source").unwrap();
        let info = e.check_client_update(&r, 100).unwrap();
        assert_eq!(info.inserts_from, vec![100], "only genuinely new content");
        assert!(!info.delete_only);
        e.apply_client_update(&r, 100).unwrap();
        let after = e.text("source").unwrap();
        assert_eq!(
            after.len(),
            before.len() + "a1 ".len(),
            "nothing but a's content"
        );
        assert!(after.contains("a1 ") && after.contains("b1 ") && after.contains("b2 "));
        assert_eq!(after.matches("b2 ").count(), 1);
        // a resync carrying a foreign block one clock past the session's state is refused
        let b3 = typed(&b, "b3 "); // never sent to the session
        apply_raw(&a, &b3);
        typed(&a, "a2 ");
        let r = resync(&a, &e);
        assert_eq!(
            e.check_client_update(&r, 100),
            Err(DocError::ForeignClient {
                allowed: 100,
                found: 200
            })
        );
        // and forging as a known client past its state is refused as before
        let f = client_doc(7, &e);
        assert!(matches!(
            e.check_client_update(&typed(&f, "forged "), 100),
            Err(DocError::ForeignClient { found: 7, .. })
        ));
        // a resync of only known content inserts nothing
        let c = client_doc(300, &e);
        let r = c
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let info = e.check_client_update(&r, 300).unwrap();
        assert!(info.inserts_from.is_empty() && info.delete_only);
        let t = e.text("source").unwrap();
        e.apply_client_update(&r, 300).unwrap();
        assert_eq!(e.text("source").unwrap(), t, "known blocks are a no-op");
    }

    #[test]
    fn garbage_never_panics_and_stays_cheap() {
        use rand::{rngs::SmallRng, Rng, SeedableRng};
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(9, "source", "seed text\n").unwrap();
        let mut rng = SmallRng::seed_from_u64(42);
        let valid = e.encode_diff(&StateVector::default().encode_v1()).unwrap();
        let mut worst = std::time::Duration::ZERO;
        for i in 0..5000 {
            let mut b: Vec<u8> = if i % 2 == 0 {
                (0..rng.gen_range(0..64)).map(|_| rng.gen()).collect()
            } else {
                valid.clone()
            };
            if !b.is_empty() && i % 2 == 1 {
                let k = rng.gen_range(0..b.len());
                b[k] = rng.gen();
            }
            let t = std::time::Instant::now();
            let _ = e.check_client_update(&b, 9);
            let _ = e.encode_diff(&b);
            let _ = Doc::load(S, &b);
            worst = worst.max(t.elapsed());
        }
        assert_eq!(e.text("source").unwrap(), "seed text\n");
        // Bounded work on hostile input. This catches outsized decoder pre-allocations (0.7-11 s
        // per input before the shape checks), not the machine's speed: 500 ms leaves room for a
        // debug build on a loaded box. The precise "rejected in O(1)" bound is in `shape_checks`.
        assert!(
            worst < std::time::Duration::from_millis(500),
            "slowest garbage input took {worst:?}"
        );
    }

    #[test]
    fn shape_checks() {
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(9, "source", "x").unwrap();
        assert!(state_vector_well_formed(&e.state_vector()));
        assert!(state_vector_well_formed(
            &StateVector::default().encode_v1()
        ));
        // counts far larger than the bytes could hold
        let sv = [255, 255, 255, 255, 15, 1, 1];
        assert!(!state_vector_well_formed(&sv));
        assert!(matches!(e.encode_diff(&sv), Err(DocError::Decode(_))));
        // more clients than 3 bytes each could hold
        assert!(!client_update_well_formed(&[3, 0, 0, 0, 0, 0, 0, 0]));
        assert!(client_update_well_formed(&[2, 0, 0, 0, 0, 0, 0]));
        let upd = [255, 255, 255, 255, 15, 0, 1, 0, 0, 0];
        let t = std::time::Instant::now();
        assert!(matches!(
            e.check_client_update(&upd, 9),
            Err(DocError::Decode(_))
        ));
        assert!(t.elapsed() < std::time::Duration::from_millis(5));
    }

    /// Two changes by client 77; the second depends on the first.
    fn dependent_pair() -> (Vec<u8>, Vec<u8>) {
        let mut a = Doc::new(S).unwrap();
        let u1 = a.replace_text_as(77, "source", "one\n").unwrap().unwrap();
        let u2 = a
            .replace_text_as(77, "source", "one\ntwo\n")
            .unwrap()
            .unwrap();
        (u1, u2)
    }

    #[test]
    fn a_pending_update_poisons_the_doc_and_nothing_panics() {
        let (_, u2) = dependent_pair();
        let mut d = Doc::new(S).unwrap();
        d.replace_text_as(9, "source", "x\n").unwrap();
        assert_eq!(d.apply_trusted(&u2), Err(DocError::Pending));
        // at the engine: the update stays pending, and the raw encode is contained
        let mut y = YrsDoc::new(&S);
        assert_eq!(y.apply_trusted(&u2), Err(DocError::Pending));
        assert!(y.has_pending() && y.poisoned);
        let raw = y.encode_v2();
        assert!(
            matches!(raw, Err(DocError::Panic) | Ok(_)),
            "never unwinds: {raw:?}"
        );
        assert_eq!(y.encode_state(), Err(DocError::Pending));
        // and every way out refuses
        assert_eq!(d.encode_state(), Err(DocError::Pending));
        #[cfg(feature = "untrusted-wire")]
        assert_eq!(
            d.encode_diff(&StateVector::default().encode_v1()),
            Err(DocError::Pending)
        );
        assert_eq!(
            d.replace_text_as(9, "source", "y\n"),
            Err(DocError::Pending)
        );
        assert_eq!(d.apply_trusted(&u2), Err(DocError::Pending));
        // reading the text still works (it is what the doc shows)
        assert_eq!(d.text("source").unwrap(), "x\n");
    }

    #[test]
    fn a_decode_error_does_not_poison() {
        let mut d = Doc::new(S).unwrap();
        d.replace_text_as(9, "source", "x\n").unwrap();
        assert!(matches!(
            d.apply_trusted(&[1, 2, 3]),
            Err(DocError::Decode(_))
        ));
        assert!(d.encode_state().is_ok());
        let (u1, u2) = dependent_pair();
        d.apply_trusted(&u1).unwrap();
        d.apply_trusted(&u2).unwrap();
        let t = d.text("source").unwrap();
        assert!(
            t.contains("one\ntwo\n") && t.contains("x\n") && t.len() == 10,
            "{t:?}"
        );
        assert!(d.encode_state().is_ok());
    }

    /// D0's Q5 on the kit's doc: every live character's inserting client and clock, as
    /// runs covering the text (UTF-16 clocks, emoji included), one client's consecutive
    /// clocks merged across changes; and the call leaves the doc alone (throwaway replica):
    /// its state vector and encoded state are byte-identical after.
    #[test]
    fn blame_runs_cover_the_text_and_leave_the_doc_alone() {
        let run = |bytes, client, clock| BlameRun {
            bytes,
            client,
            clock,
        };
        let mut d = Doc::new(S).unwrap();
        d.replace_text_as(500, "source", "aaaa\nbbbb\n").unwrap();
        // 600 replaces the third `a` and appends a line, in one change (applied back to
        // front: the appended line takes 600's clocks 0..4, the `X` clock 4)
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        d.edit_as(
            600,
            &[Edit::Text {
                root: "source".into(),
                hunks: vec![h(2, 3, "X"), h(10, 10, "🎉y\n")],
            }],
        )
        .unwrap();
        d.replace_text_as(500, "source", "aaXa\nbbbb\n🎉y\nzz")
            .unwrap();
        let (sv, state) = (d.state_vector(), d.encode_state().unwrap());
        let runs = d.blame_runs("source").unwrap();
        assert_eq!(
            runs,
            vec![
                run(2, 500, 0),
                run(1, 600, 4),
                run(7, 500, 3),
                run(6, 600, 0), // 🎉 is 4 bytes, 2 clocks
                run(2, 500, 10),
            ]
        );
        assert_eq!(d.state_vector(), sv);
        assert_eq!(d.encode_state().unwrap(), state);
        assert_eq!(d.blame_runs("source").unwrap(), runs, "repeatable");
        // one client typing on: one run across its changes
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(7, "source", "ab").unwrap();
        e.replace_text_as(7, "source", "abc").unwrap();
        assert_eq!(e.blame_runs("source").unwrap(), vec![run(3, 7, 0)]);
        assert_eq!(e.clock_of(7), 3);
        assert_eq!(e.clock_of(8), 0);
        // an empty root has no runs; a missing one is refused
        assert!(Doc::new(S)
            .unwrap()
            .blame_runs("source")
            .unwrap()
            .is_empty());
        assert!(matches!(e.blame_runs("nope"), Err(DocError::NoRoot { .. })));
    }

    /// `insert_clocks` is the order `edit_as` really uses: every inserted character's
    /// (client, clock), read back through `blame_runs`, is the one it predicts, across
    /// roots, several hunks per root, deletions and multi-unit characters.
    #[test]
    fn insert_clocks_match_blame_runs() {
        const TWO: Schema = Schema {
            name: "t2",
            roots: &[("a", Kind::Text), ("b", Kind::Text)],
        };
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let mut d = Doc::new(TWO).unwrap();
        d.replace_text_as(500, "a", "one two three\n").unwrap();
        d.replace_text_as(500, "b", "αβγ\n").unwrap();
        // client 600 already has clocks: its next change starts past them
        d.replace_text_as(600, "b", "αβγ\nz").unwrap();
        let edits = vec![
            Edit::Text {
                root: "b".into(),
                hunks: vec![h(0, 2, "🎉"), h(4, 4, "xy"), h(7, 8, "")],
            },
            Edit::Text {
                root: "a".into(),
                hunks: vec![h(0, 0, "A"), h(4, 7, "TWO🎉"), h(14, 14, "end")],
            },
        ];
        let before: Vec<String> = ["a", "b"].iter().map(|r| d.text(r).unwrap()).collect();
        let clock = d.clock_of(600);
        let want = insert_clocks(clock, &edits);
        d.edit_as(600, &edits).unwrap();
        let mut total = 0;
        for (e, starts) in edits.iter().zip(&want) {
            let Edit::Text { root, hunks } = e;
            let text = d.text(root).unwrap();
            let runs = d.blame_runs(root).unwrap();
            // each live byte offset's (client, clock)
            let mut at: Vec<(usize, u64, u32)> = Vec::new();
            let mut b = 0;
            for r in &runs {
                let mut c = r.clock;
                for (i, ch) in text[b..b + r.bytes].char_indices() {
                    at.push((b + i, r.client, c));
                    c += ch.len_utf16() as u32;
                }
                b += r.bytes;
            }
            let mut delta: isize = 0;
            for (hk, &c0) in hunks.iter().zip(starts) {
                let s = (hk.start as isize + delta) as usize;
                let mut c = c0;
                for (i, ch) in hk.insert.char_indices() {
                    let got = at.iter().find(|x| x.0 == s + i).unwrap();
                    assert_eq!((got.1, got.2), (600, c), "{root}: {hk:?} char {i}");
                    c += ch.len_utf16() as u32;
                }
                total += hk.insert.encode_utf16().count() as u32;
                delta += hk.insert.len() as isize - (hk.end - hk.start) as isize;
            }
        }
        assert_eq!(d.clock_of(600), clock + total);
        assert_ne!(before[0], d.text("a").unwrap());
    }

    #[test]
    fn store_round_trip() {
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(3, "source", "a\nb\n").unwrap();
        e.replace_text_as(4, "source", "a\nB\nc\n").unwrap();
        let l = Doc::load(S, &e.encode_state().unwrap()).unwrap();
        assert_eq!(l.text("source").unwrap(), "a\nB\nc\n");
        assert_eq!(l.state_vector(), e.state_vector());
    }
}

/// ARCHITECTURE rule 8 (v0.9.7): the kit holds only root shared-type handles. The engine
/// module takes handles in one place, `root_text` (which asserts a root in debug builds),
/// names no nested type, and reads no nested type out of a value; every doc it makes keeps
/// root handles only. A future change that starts holding a nested handle fails here.
#[cfg(test)]
mod root_handles {
    use super::*;

    const S: Schema = Schema {
        name: "t",
        roots: &[("a", Kind::Text), ("b", Kind::Text)],
    };

    fn is_root(t: &TextRef) -> bool {
        matches!(
            AsRef::<yrs::branch::Branch>::as_ref(t).id(),
            BranchID::Root(_)
        )
    }

    #[test]
    fn the_engine_holds_only_root_handles() {
        let src = include_str!("yrs.rs");
        // the module's code, without its tests and comments
        let code: Vec<&str> = src
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        let takes = code.iter().filter(|l| l.contains("get_or_insert_")).count();
        assert_eq!(takes, 1, "only root_text takes a shared-type handle");
        for nested in [
            "MapRef",
            "ArrayRef",
            "XmlFragmentRef",
            "XmlElementRef",
            "XmlTextRef",
            "Out::YText",
            "Out::YMap",
            "Out::YArray",
            "Out::YXml",
            "Out::UndefinedRef",
            "BranchPtr",
        ] {
            assert!(
                !code.iter().any(|l| l.contains(nested)),
                "the engine module names {nested}: the kit holds only root handles"
            );
        }
        // handles drop before their Doc: in every struct of this file (tests included), a
        // field holding a handle is declared before the field holding its Doc (fields drop
        // in declaration order), and no tuple type pairs a Doc with a handle after it
        let mut structs = 0;
        for block in src.split("struct ").skip(1) {
            let Some(body) = block.split_once('{').map(|(_, b)| b) else {
                continue;
            };
            let body = body.split('}').next().unwrap_or("");
            let fields: Vec<&str> = body
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(':'))
                .collect();
            let doc = fields
                .iter()
                .position(|l| l.ends_with(": Doc,") || l.ends_with(": yrs::Doc,"));
            let handle = fields.iter().rposition(|l| l.contains("TextRef"));
            if let (Some(d), Some(h)) = (doc, handle) {
                structs += 1;
                assert!(
                    h < d,
                    "a handle field is declared after its Doc: {fields:?} (handles drop first)"
                );
            }
        }
        assert!(
            structs >= 3,
            "the scan found the Doc + handle structs ({structs})"
        );
        let compact: String = src.split_whitespace().collect();
        // (the needles are built here, so this test's own source doesn't match them)
        for doc in ["Doc", "yrs::Doc"] {
            let pair = format!("({doc},{})", "TextRef");
            assert!(
                !compact.contains(pair.as_str()),
                "a tuple drops its Doc before its handle: {pair}"
            );
        }
        // every doc the module makes keeps root handles only
        let d = YrsDoc::new(&S);
        assert!(d.texts.values().all(is_root));
        let state = d.encode_v2().unwrap();
        let l = YrsDoc::load(&S, &state).unwrap();
        assert!(l.texts.values().all(is_root));
        let t = l.twin().unwrap();
        assert!(t.texts.values().all(is_root));
    }
}
