//! A native doc's disk replica is text only: the texts the file holds at the sync point, kept in
//! memory and stored as a kit-neutral encoding ([`encode_texts`]) in the synced state, never
//! engine bytes. An outside save is merged from those texts as on Yrs (the merge only ever read
//! the replica's text); a write-back stores the doc's texts, with no engine encode.
//!
//! A Yrs text doc's replica is text only in memory too (v0.23), while the synced state the
//! store keeps stays the engine's encoding, as before: the one path that needs its elements (an
//! outside save the doc doesn't hold, stored as a client of its own) decodes it from the store
//! on demand ([`super::Session::synced_doc_from_store`]), and so do the sync audit's element
//! check and a purge's re-encode.

use super::State;
use crate::doc::{Doc, DocError, Edit};
use crate::schema::{Kind, Schema};
use crate::session::error::{Result, SessionError};
use crate::text::diff::Hunk;
use crate::{SharedText, SharedTexts};

/// The text-only disk replica's stored form: this tag, then the root count, then each text
/// root's name and text, each length-prefixed (little-endian u32 for names and the count, u64
/// for texts). Kit-neutral: no engine reads or writes it.
const TEXT_REPLICA_TAG: &[u8; 8] = b"MPCTEXT1";

/// The stored form of a text-only disk replica holding `texts`.
pub(super) fn encode_texts(texts: &[(&'static str, SharedText)]) -> Vec<u8> {
    let len: usize = texts.iter().map(|(r, t)| 12 + r.len() + t.len()).sum();
    let mut out = Vec::with_capacity(12 + len);
    out.extend_from_slice(TEXT_REPLICA_TAG);
    out.extend_from_slice(&(texts.len() as u32).to_le_bytes());
    for (r, t) in texts {
        out.extend_from_slice(&(r.len() as u32).to_le_bytes());
        out.extend_from_slice(r.as_bytes());
        out.extend_from_slice(&(t.len() as u64).to_le_bytes());
        out.extend_from_slice(t.as_bytes());
    }
    out
}

/// Whether `bytes` are a text-only replica's stored form (by its tag).
#[cfg(test)]
pub(crate) fn is_text_replica(bytes: &[u8]) -> bool {
    bytes.starts_with(TEXT_REPLICA_TAG)
}

/// A text-only replica's texts from its stored form: every text root of `schema`, in schema
/// order, each once. Anything else is [`DocError::Load`] (a stored state the kit never wrote).
pub(super) fn decode_texts(schema: &Schema, bytes: &[u8]) -> Result<SharedTexts> {
    let bad = |what: &str| -> SessionError {
        DocError::Load(format!("the stored text replica is unreadable: {what}")).into()
    };
    let rest = bytes
        .strip_prefix(TEXT_REPLICA_TAG.as_slice())
        .ok_or_else(|| bad("not a text replica"))?;
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = rest.get(at..at.checked_add(n)?)?;
        at += n;
        Some(s)
    };
    let u32_of = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize;
    let count = take(4).map(u32_of).ok_or_else(|| bad("its count"))?;
    let roots: Vec<&'static str> = schema
        .roots
        .iter()
        .filter(|(_, k)| *k == Kind::Text)
        .map(|(r, _)| *r)
        .collect();
    if count != roots.len() {
        return Err(bad("its root count isn't the schema's"));
    }
    let mut out = Vec::with_capacity(count);
    for root in roots {
        let n = take(4).map(u32_of).ok_or_else(|| bad("a root name"))?;
        let name = take(n).ok_or_else(|| bad("a root name"))?;
        if name != root.as_bytes() {
            return Err(bad("a root isn't the schema's"));
        }
        let n = take(8)
            .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
            .ok_or_else(|| bad("a text's length"))?;
        let n = usize::try_from(n).map_err(|_| bad("a text's length"))?;
        let text = take(n).ok_or_else(|| bad("a text"))?;
        let text = std::str::from_utf8(text).map_err(|_| bad("a text isn't UTF-8"))?;
        out.push((root, SharedText::new(text.to_owned())));
    }
    if at != rest.len() {
        return Err(bad("bytes after the last root"));
    }
    Ok(out)
}

impl super::Session {
    /// The disk replica's state as the synced state stores it, for the doc as it is now: a Yrs
    /// doc's encoded state; a native doc's texts in the text replica's form ([`encode_texts`]),
    /// never engine bytes, and no engine encode.
    pub(super) fn replica_state(&self, st: &State) -> std::result::Result<Vec<u8>, DocError> {
        if st.doc.is_native() && !crate::beta_plant::planted("replica-stores-engine-state") {
            return Ok(encode_texts(&self.doc_texts(&st.doc)?));
        }
        st.doc.encode_state()
    }

    /// Every text root's text as `doc` holds it now, in schema order (read from the doc: an
    /// outside save's merge reads them before the session's cached texts follow).
    pub(super) fn doc_texts(&self, doc: &Doc) -> std::result::Result<SharedTexts, DocError> {
        self.schema
            .roots
            .iter()
            .filter(|(_, k)| *k == Kind::Text)
            .map(|(r, _)| Ok((*r, SharedText::new(doc.text(r)?))))
            .collect()
    }

    /// Whether a doc's disk replica keeps its texts only: every native doc's, and a Yrs text
    /// doc's (v0.23; a structured doc's outside edits are aligned on the replica's keyed state,
    /// so it keeps a decoded doc). Taken once, when the file is attached
    /// ([`super::Session::attach_file`]); the test-only switch
    /// ([`crate::differential::with_decoded_replica`]) keeps a Yrs text doc's decoded, as
    /// before, for the rows that compare the two.
    pub(super) fn texts_only_replica(&self, st: &State) -> bool {
        if st.doc.is_native() {
            return true;
        }
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::decoded_replica_on() {
            return false;
        }
        !self.structured
    }

    /// A disk replica from its stored state, on the doc's kind of replica: a native doc's texts
    /// ([`decode_texts`]), a Yrs doc's state (decoded on first need: to its texts only when
    /// `texts_only`).
    pub(super) fn replica_from_state(
        &self,
        st: &State,
        state: Vec<u8>,
        texts_only: bool,
    ) -> Result<super::Replica> {
        if st.doc.is_native() {
            return Ok(super::Replica::Text(decode_texts(&self.schema, &state)?));
        }
        Ok(super::Replica::Stored {
            state,
            schema: self.schema,
            checks: (
                self.config.edit_check_every,
                self.config.edit_check_scale_units,
                self.config.position_check_every,
            ),
            texts_only,
        })
    }

    /// A disk replica from its stored state, decoded now (a stored state that doesn't decode
    /// is an error): a native doc's texts, a Yrs text doc's texts (`texts_only`: the decoded
    /// doc dropped), else the decoded doc.
    pub(super) fn replica_loaded(
        &self,
        st: &State,
        state: &[u8],
        texts_only: bool,
    ) -> Result<super::Replica> {
        if st.doc.is_native() {
            return Ok(super::Replica::Text(decode_texts(&self.schema, state)?));
        }
        let doc = Doc::load(self.schema, state)?;
        if texts_only {
            return Ok(super::Replica::Text(self.doc_texts(&doc)?));
        }
        Ok(super::Replica::Doc(self.checked(doc)))
    }

    /// A Yrs text doc's synced state as the store keeps it, decoded on demand (v0.23: its
    /// replica holds texts only): what an outside save the doc doesn't hold is made on, the
    /// sync audit's element check, a purge's re-encode. Bounded and checked: the stored row
    /// must be the one this session last synced (its length the recorded `replica_bytes`) and
    /// hold the replica's texts; anything else is refused ([`DocError::Load`]), never merged
    /// or stored on.
    pub(super) fn synced_doc_from_store(&self, st: &mut State) -> Result<Doc> {
        let row = self.store(st)?.sync_states(self.doc_id)?.0;
        let d = st
            .disk
            .as_mut()
            .ok_or_else(|| SessionError::Disk("no disk replica: the doc has no file".into()))?;
        self.synced_doc_of(d, row)
    }

    /// [`Self::synced_doc_from_store`] on a row already read (`row`, the store's synced state).
    pub(super) fn synced_doc_of(
        &self,
        d: &mut super::Disk,
        row: Option<crate::store::SyncState>,
    ) -> Result<Doc> {
        let refuse = |why: &str| -> SessionError {
            DocError::Load(format!("the stored synced state can't be used: {why}")).into()
        };
        let row = row.ok_or_else(|| refuse("none is stored"))?;
        if row.state.len() as u64 != d.replica_bytes {
            return Err(refuse("it isn't the state this session synced"));
        }
        let doc = self.checked(Doc::load(self.schema, &row.state)?);
        if let Some(texts) = d.replica.texts()? {
            if self.doc_texts(&doc)?.as_slice() != texts {
                return Err(refuse("its texts aren't the disk replica's"));
            }
        }
        Ok(doc)
    }

    /// The synced state a Yrs text doc stores after an outside save its doc doesn't hold
    /// (`file_edits`, the save on the replica's text): the stored synced state decoded
    /// ([`Self::synced_doc_from_store`]), the save made on it as a client of its own (one the
    /// doc never holds), encoded. The bytes a decoded replica gave before v0.23.
    pub(super) fn synced_state_with_save(
        &self,
        st: &mut State,
        file_edits: &[Edit],
    ) -> Result<Vec<u8>> {
        let mut r = if crate::beta_plant::planted("yrs-replica-state-from-texts") {
            // the plant: the stored state made from the replica's texts on fresh elements,
            // not from the synced state (stale texts where the decoded state was needed)
            let texts = st
                .disk
                .as_mut()
                .and_then(|d| d.replica.texts().ok().flatten().map(<[_]>::to_vec))
                .unwrap_or_default();
            let mut r = self.checked(Doc::new(self.schema)?);
            let edits: Vec<Edit> = texts
                .into_iter()
                .filter(|(_, t)| !t.is_empty())
                .map(|(root, t)| Edit::Text {
                    root: root.to_owned(),
                    hunks: vec![Hunk {
                        start: 0,
                        end: 0,
                        insert: t.as_str().to_owned(),
                    }],
                })
                .collect();
            if !edits.is_empty() {
                r.edit_as(super::mint_client_id(), &edits)?;
            }
            r
        } else {
            self.synced_doc_from_store(st)?
        };
        if !file_edits.is_empty() {
            let own = loop {
                let c = super::mint_client_id();
                if st.doc.clock_of(c) == 0 && r.clock_of(c) == 0 {
                    break c;
                }
            };
            r.edit_as(own, file_edits)?;
        }
        Ok(r.encode_state()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Schema = Schema {
        name: "t",
        roots: &[("a", Kind::Text), ("b", Kind::Text)],
    };

    /// The text replica's form round-trips, and anything the kit didn't write is refused: the
    /// wrong tag, a root count or name that isn't the schema's, a short or long body, text that
    /// isn't UTF-8.
    #[test]
    fn the_text_replica_round_trips_and_refuses_what_it_never_wrote() {
        let t = |s: &str| SharedText::new(s.to_owned());
        let texts = vec![("a", t("one\ntwo é\n")), ("b", t(""))];
        let b = encode_texts(&texts);
        assert!(is_text_replica(&b));
        assert_eq!(decode_texts(&S, &b).unwrap(), texts);
        let mut bad = b.clone();
        bad[0] ^= 1;
        assert!(decode_texts(&S, &bad).is_err(), "the tag");
        assert!(decode_texts(&S, &b[..b.len() - 1]).is_err(), "short");
        let mut long = b.clone();
        long.push(0);
        assert!(decode_texts(&S, &long).is_err(), "long");
        let one = encode_texts(&[("a", t("x"))]);
        assert!(decode_texts(&S, &one).is_err(), "the root count");
        let renamed = encode_texts(&[("a", t("x")), ("c", t(""))]);
        assert!(decode_texts(&S, &renamed).is_err(), "a root's name");
        let mut not_utf8 = encode_texts(&[("a", t("x")), ("b", t(""))]);
        // "x" in root a: after the tag, the count, a's name length and name, its text length
        let i = 8 + 4 + 4 + 1 + 8;
        assert_eq!(not_utf8[i], b'x');
        not_utf8[i] = 0xff;
        assert!(decode_texts(&S, &not_utf8).is_err(), "UTF-8");
    }
}
