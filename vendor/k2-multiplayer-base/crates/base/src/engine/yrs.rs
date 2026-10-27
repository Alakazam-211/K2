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
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, Text, TextRef, Transact, Update,
};

use super::{EngineDoc, SESSION_CLIENT};
use crate::doc::{ClientId, DocError, Edit, UpdateInfo, MAX_CLIENT_ID};
use crate::schema::{Kind, Schema};
use crate::text::diff::Hunk;

pub(crate) struct YrsDoc {
    doc: Doc,
    texts: HashMap<&'static str, TextRef>,
}

fn new_doc(client: u64) -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(client),
        offset_kind: OffsetKind::Utf16,
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
/// by its size: a client's own update carries at most one client, and that client's block
/// count can't exceed the bytes left.
fn client_update_well_formed(b: &[u8]) -> bool {
    let mut at = 0;
    let Some(clients) = read_varuint(b, &mut at) else {
        return false;
    };
    if clients > 1 {
        return false;
    }
    if clients == 1 {
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
}

impl EngineDoc for YrsDoc {
    fn new(schema: &Schema) -> Self {
        let doc = new_doc(SESSION_CLIENT);
        let texts = roots(&doc, schema);
        Self { doc, texts }
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
        Ok(e)
    }

    fn text(&self, root: &str) -> String {
        self.texts[root].get_string(&self.doc.transact())
    }

    fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    fn encode_state(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v2(&StateVector::default())
    }

    fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError> {
        if !state_vector_well_formed(remote_sv) {
            return Err(DocError::Decode("malformed state vector".into()));
        }
        guarded(|| {
            let sv =
                StateVector::decode_v1(remote_sv).map_err(|e| DocError::Decode(e.to_string()))?;
            Ok(self.doc.transact().encode_state_as_update_v1(&sv))
        })
    }

    fn check_update(&self, update: &[u8], allowed: ClientId) -> Result<UpdateInfo, DocError> {
        if !client_update_well_formed(update) {
            return Err(DocError::Decode(
                "malformed update, or more than one client in a client update".into(),
            ));
        }
        guarded(|| {
            let u = Update::decode_v1(update).map_err(|e| DocError::Decode(e.to_string()))?;
            let mut inserts: Vec<ClientId> =
                u.insertions(true).client_ids().map(|c| c.get()).collect();
            // Upper state vector too: catches content blocks the insertion set might not list.
            inserts.extend(u.state_vector().iter().map(|(c, _)| c.get()));
            inserts.sort_unstable();
            inserts.dedup();
            if let Some(&found) = inserts.iter().find(|&&c| c != allowed) {
                return Err(DocError::ForeignClient { allowed, found });
            }
            // The update must continue this client's history, not skip ahead.
            let have = self.doc.transact().state_vector();
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

    fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError> {
        guarded(|| {
            let u = Update::decode_v1(update).map_err(|e| DocError::Decode(e.to_string()))?;
            self.doc
                .transact_mut()
                .apply_update(u)
                .map_err(|e| DocError::Apply(e.to_string()))
        })?;
        if self.has_pending() {
            return Err(DocError::Pending);
        }
        Ok(())
    }

    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Vec<u8>, DocError> {
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
            let full = self
                .doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
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
        // Bounded work on hostile input: nothing tiny may take long (debug-build allowance).
        assert!(
            worst < std::time::Duration::from_millis(50),
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
        let upd = [255, 255, 255, 255, 15, 0, 1, 0, 0, 0];
        let t = std::time::Instant::now();
        assert!(matches!(
            e.check_client_update(&upd, 9),
            Err(DocError::Decode(_))
        ));
        assert!(t.elapsed() < std::time::Duration::from_millis(5));
    }

    #[test]
    fn store_round_trip() {
        let mut e = Doc::new(S).unwrap();
        e.replace_text_as(3, "source", "a\nb\n").unwrap();
        e.replace_text_as(4, "source", "a\nB\nc\n").unwrap();
        let l = Doc::load(S, &e.encode_state()).unwrap();
        assert_eq!(l.text("source").unwrap(), "a\nB\nc\n");
        assert_eq!(l.state_vector(), e.state_vector());
    }
}
