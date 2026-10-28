//! [`EngineDoc`] on Yrs (wire-compatible with Yjs).
//!
//! - One Yrs root per schema root (text roots only, today).
//! - Offsets are UTF-16 inside Yrs (`OffsetKind::Utf16`), matching Yjs and CodeMirror.
//!   Hunks arrive in UTF-8 byte offsets and are converted here.
//! - Wire updates are Yjs v1; stored state is v2 (10-30% smaller in the D0 spike).
//! - Outside bytes get a cheap shape check first, then decode/apply under `catch_unwind`.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use yrs::block::ClientID;
use yrs::types::text::ChangeKind as YChangeKind;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, ReadTxn, Snapshot, StateVector, Text, TextRef,
    Transact, Update,
};

use super::{BlameRun, EngineDoc, SESSION_CLIENT};
use crate::doc::{ClientId, DocError, Edit, UpdateInfo, MAX_CLIENT_ID};
use crate::schema::{Kind, Schema};
use crate::text::diff::Hunk;

pub(crate) struct YrsDoc {
    doc: Doc,
    texts: HashMap<&'static str, TextRef>,
    /// An apply left the doc in a state that can't be trusted: pending updates (yrs 0.28
    /// panics encoding those, transaction.rs:408, an unwrap on a merge error), or a failed or
    /// panicked apply that may have half-integrated. Nothing is encoded or edited from a
    /// poisoned doc; its owner reloads it from the store.
    poisoned: bool,
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

fn guarded<T>(f: impl FnOnce() -> Result<T, DocError>) -> Result<T, DocError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(Err(DocError::Panic))
}

fn roots(doc: &Doc, schema: &Schema) -> HashMap<&'static str, TextRef> {
    schema
        .roots
        .iter()
        .filter(|(_, k)| *k == Kind::Text)
        .map(|(n, _)| (*n, doc.get_or_insert_text(*n)))
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
/// by its size (no reservation bombs): the client count can't exceed the bytes left divided
/// by 3 (each client section is at least its block count, client ID and clock), and the
/// first client's block count can't exceed the bytes left. More than one client is allowed
/// here; [`EngineDoc::check_update`] decides what other clients' blocks may be.
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

/// A text root's hunks with their UTF-16 (offset, length), ready to apply.
type TextPlan<'a> = (&'a str, &'a [Hunk], Vec<(u32, u32)>);

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

impl YrsDoc {
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
    fn replica(&self, client: u64, root: &str) -> Result<(Doc, TextRef), DocError> {
        let replica = new_doc(client);
        let t = replica.get_or_insert_text(root);
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
        Ok((replica, t))
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

impl EngineDoc for YrsDoc {
    fn new(schema: &Schema) -> Self {
        let doc = new_doc(SESSION_CLIENT);
        let texts = roots(&doc, schema);
        Self {
            doc,
            texts,
            poisoned: false,
        }
    }

    fn load(schema: &Schema, state: &[u8]) -> Result<Self, DocError> {
        let e = Self::new(schema);
        guarded(|| {
            let u = Update::decode_v2(state).map_err(|err| DocError::Load(err.to_string()))?;
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
        self.texts[root].get_string(&self.doc.transact())
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
        if !client_update_well_formed(update) {
            return Err(DocError::Decode("malformed update".into()));
        }
        guarded(|| {
            let u = Update::decode_v1(update).map_err(|e| DocError::Decode(e.to_string()))?;
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
            // The update must continue each client's history, not skip ahead.
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
            Ok(UpdateInfo {
                delete_only: inserts.is_empty(),
                inserts_from: inserts,
            })
        })
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

    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Vec<u8>, DocError> {
        self.usable()?;
        // Positions are computed against the session's current text, before the replica exists.
        let plans: Vec<TextPlan> = edits
            .iter()
            .map(|e| match e {
                Edit::Text { root, hunks } => {
                    let cur = self.text(root);
                    (root.as_str(), hunks.as_slice(), to_utf16(&cur, hunks))
                }
            })
            .collect();

        // The replica lives only inside this block: built from the session, edited as
        // `client`, encoded, dropped. Nothing about it outlives the call.
        let update = {
            let replica = new_doc(client);
            let rtexts: HashMap<&str, TextRef> = self
                .texts
                .keys()
                .map(|n| (*n, replica.get_or_insert_text(*n)))
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
            for (root, hunks, pos) in &plans {
                let t = &rtexts[root];
                // Back to front, so earlier offsets stay valid.
                for (h, &(s16, len16)) in hunks.iter().zip(pos.iter()).rev() {
                    if len16 > 0 {
                        t.remove_range(&mut txn, s16, len16);
                    }
                    if !h.insert.is_empty() {
                        t.insert(&mut txn, s16, &h.insert);
                    }
                }
            }
            txn.encode_update_v1()
        };
        self.apply_trusted(&update)?;
        Ok(update)
    }

    fn clock_of(&self, client: ClientId) -> u32 {
        self.doc
            .transact()
            .state_vector()
            .get(&ClientID::new(client))
    }

    /// `diff_range(now, empty)` (proven in D0, Q5): with the empty snapshot as the low end,
    /// every live item is "added", so each comes back as its own run with its ID, GC on.
    /// It **splits blocks** (it takes `&mut` of a transaction), so it runs on a throwaway
    /// replica, never on this doc: the cost is a full encode, decode and walk, O(doc).
    fn blame_runs(&self, root: &str) -> Result<Vec<BlameRun>, DocError> {
        self.usable()?;
        let (replica, t) = self.replica(SESSION_CLIENT, root)?;
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
    fn client_doc(id: u64, from: &Doc) -> (yrs::Doc, TextRef) {
        let d = new_doc(id);
        let t = d.get_or_insert_text("source");
        let sv = d.transact().state_vector().encode_v1();
        let u = from.encode_diff(&sv).unwrap();
        d.transact_mut()
            .apply_update(Update::decode_v1(&u).unwrap())
            .unwrap();
        (d, t)
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
        let t = m.get_or_insert_text("source");
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
        let (c, ct) = client_doc(100, &e);
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
        let (f, ft) = client_doc(7, &e);
        let forged = {
            let mut t = f.transact_mut();
            ft.insert(&mut t, 0, "forged ");
            t.encode_update_v1()
        };
        assert!(matches!(
            e.check_client_update(&forged, 100),
            Err(DocError::ForeignClient { found: 7, .. })
        ));
        let (g, gt) = client_doc(200, &e);
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
    fn typed(c: &(yrs::Doc, TextRef), text: &str) -> Vec<u8> {
        let mut t = c.0.transact_mut();
        c.1.insert(&mut t, 0, text);
        t.encode_update_v1()
    }

    fn apply_raw(c: &(yrs::Doc, TextRef), u: &[u8]) {
        c.0.transact_mut()
            .apply_update(Update::decode_v1(u).unwrap())
            .unwrap();
    }

    /// What a reconnecting client sends: everything it has that the session lacks.
    fn resync(c: &(yrs::Doc, TextRef), session: &Doc) -> Vec<u8> {
        let sv = StateVector::decode_v1(&session.state_vector()).unwrap();
        c.0.transact().encode_state_as_update_v1(&sv)
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
        assert!(a.0.transact().store().pending_update().is_some());
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
        let r =
            c.0.transact()
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
        // Bounded work on hostile input. This catches the decode bombs (0.7-11 s per input
        // before the shape checks), not the machine's speed: 500 ms leaves room for a debug
        // build on a loaded box. The precise "rejected in O(1)" bound is in `shape_checks`.
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
        // at the engine: yrs keeps the pending update, and the raw encode (what panicked in
        // yrs 0.28, transaction.rs:408) is contained
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
