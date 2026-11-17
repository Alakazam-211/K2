//! Reads 2D drawings in ASCII DXF for the multiplayer kit, byte-exact.
//!
//! - [`token`]: our own tokenizer. Every group code and value keeps its exact byte range, its
//!   line endings and its code padding; a byte-order mark and bytes after the end marker are
//!   carried as they are. Writing the tokens back gives the input's bytes.
//! - [`structure`]: the file's structure over the tokens: header variables, tables and their
//!   entries, blocks, entities with their followers, and objects, each by its pair range.
//! - [`convert`]: that structure as the format-neutral drawing model
//!   ([`k2_mpc_cad_model::Doc`]), each entity's facts as separate fields.
//! - [`merge`]: the three-way merge by unit and field, for a person's app saving the file while
//!   the doc has moved on.
//! - [`pack`]: the kit's tier-0 blob pack, with the model's structure index.
//! - [`verbs`]: the read verbs, each a pure function of one version's bytes with typed data.
//! - [`edit`]: the edit verbs, each a pure function of one version's bytes to a plan of byte
//!   ranges the kit applies, checked by the pack's own read of its output before it returns.
//!
//! Nothing is guessed: a file that isn't code/value pairs, or is binary DXF, is refused with a
//! typed [`CadError`] naming the byte offset; a well-formed but unusual file is read as it is.
#![forbid(unsafe_code)]

pub mod codes;
pub mod convert;
pub mod draw;
pub mod edit;
pub mod error;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
pub mod merge;
pub mod mleader;
pub mod pack;
pub mod structure;
#[cfg(feature = "testgen")]
pub mod testgen;
pub mod token;
pub mod verbs;

pub use error::{CadError, Code};
pub use k2_mpc_cad_model::{handle, mutant, Doc};
pub use pack::DxfPack;

/// The resources a read may use; over any, a read is refused with [`error::OVER_BOUND`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The file's size.
    pub max_bytes: u64,
    /// One line (a long multiline text chunk, a binary chunk).
    pub max_line: usize,
    /// Code/value pairs in the file.
    pub max_pairs: u64,
    /// Pairs in one entity, its followers included.
    pub max_entity_pairs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 1 << 30,
            max_line: 1 << 20,
            max_pairs: 200_000_000,
            max_entity_pairs: 1 << 20,
        }
    }
}

/// The drawing model of `bytes`.
pub fn read_doc(bytes: &[u8], limits: &Limits) -> Result<Doc, CadError> {
    let t = token::tokenize(bytes, limits)?;
    let s = structure::walk(t, limits)?;
    Ok(convert::doc(&s))
}

/// The structure probe of the drawing `bytes`, named `name`: the model's probe
/// ([`k2_mpc_cad_model::probe::probe`]), then `check`'s findings by code and count (`check
/// <code> <count>`), never a value. With the model, for the set's probe.
pub fn probe(name: &str, bytes: &[u8]) -> Result<(String, Doc), CadError> {
    let limits = Limits::default();
    let t = token::tokenize(bytes, &limits)?;
    let s = structure::walk(t, &limits)?;
    let doc = convert::doc(&s);
    let mut out = k2_mpc_cad_model::probe::probe(name, &doc);
    let c = verbs::check(&s, &doc, &[])?;
    if let Some(k2_mpc_base::errors::Value::Obj(by)) = c.data.get("by_code") {
        for (code, n) in by {
            out.push_str(&format!("check {code} {}\n", n.to_json()));
        }
    }
    Ok((out, doc))
}
