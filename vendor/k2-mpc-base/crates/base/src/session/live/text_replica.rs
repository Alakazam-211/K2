//! A native doc's disk replica is text only: the texts the file holds at the sync point, kept in
//! memory and stored as a kit-neutral encoding ([`encode_texts`]) in the synced state, never
//! engine bytes. An outside save is merged from those texts as on Yrs (the merge only ever read
//! the replica's text); a write-back stores the doc's texts, with no engine encode.

use super::State;
use crate::doc::{Doc, DocError};
use crate::schema::{Kind, Schema};
use crate::session::error::{Result, SessionError};

/// The text-only disk replica's stored form: this tag, then the root count, then each text
/// root's name and text, each length-prefixed (little-endian u32 for names and the count, u64
/// for texts). Kit-neutral: no engine reads or writes it.
const TEXT_REPLICA_TAG: &[u8; 8] = b"MPCTEXT1";

/// The stored form of a text-only disk replica holding `texts`.
pub(super) fn encode_texts(texts: &[(&'static str, String)]) -> Vec<u8> {
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
pub(super) fn decode_texts(schema: &Schema, bytes: &[u8]) -> Result<Vec<(&'static str, String)>> {
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
        out.push((root, text.to_owned()));
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
    pub(super) fn doc_texts(
        &self,
        doc: &Doc,
    ) -> std::result::Result<Vec<(&'static str, String)>, DocError> {
        self.schema
            .roots
            .iter()
            .filter(|(_, k)| *k == Kind::Text)
            .map(|(r, _)| Ok((*r, doc.text(r)?)))
            .collect()
    }

    /// A disk replica from its stored state, on the doc's kind of replica: a native doc's texts
    /// ([`decode_texts`]), a Yrs doc's state (decoded on first need).
    pub(super) fn replica_from_state(&self, st: &State, state: Vec<u8>) -> Result<super::Replica> {
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
        })
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
        let texts = vec![("a", "one\ntwo é\n".to_owned()), ("b", String::new())];
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
        let one = encode_texts(&[("a", "x".to_owned())]);
        assert!(decode_texts(&S, &one).is_err(), "the root count");
        let renamed = encode_texts(&[("a", "x".to_owned()), ("c", String::new())]);
        assert!(decode_texts(&S, &renamed).is_err(), "a root's name");
        let mut not_utf8 = encode_texts(&[("a", "x".to_owned()), ("b", String::new())]);
        // "x" in root a: after the tag, the count, a's name length and name, its text length
        let i = 8 + 4 + 4 + 1 + 8;
        assert_eq!(not_utf8[i], b'x');
        not_utf8[i] = 0xff;
        assert!(decode_texts(&S, &not_utf8).is_err(), "UTF-8");
    }
}
