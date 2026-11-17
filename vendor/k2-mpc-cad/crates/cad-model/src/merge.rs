//! What a drawing pack's three-way merge reports, format-neutral: the merged file, or the
//! conflicts that kept it from merging (each naming the unit, the field and both sides), or why
//! it couldn't read the files well enough to merge them at all. A merge never takes one side's
//! value over the other's: two changes to the same field are a conflict, held for a person.

/// One field two sides changed differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    /// The unit: `@h:<HANDLE>`, `$VAR` (a header variable), or `name:<NAME>`.
    pub address: String,
    /// The field: `geom`, `layer`, `color`, `rgb`, `ltype`, `lw`, `alpha`, `text`, `content`,
    /// or `whole` (a unit merged whole: deleted on one side and changed on the other, added
    /// twice, or a unit with no fields of its own).
    pub field: String,
    /// What ours made of it, short (`deleted`, or its values).
    pub ours: String,
    /// What theirs made of it, short.
    pub theirs: String,
}

/// A three-way merge's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The merged file: theirs, with ours' changes spliced in; `applied` counts the units of
    /// ours applied.
    Merged { bytes: Vec<u8>, applied: usize },
    /// Fields both sides changed differently: nothing merged.
    Conflict(Vec<MergeConflict>),
    /// The files can't be merged by unit (unreadable, no handles, a change outside the units
    /// the merge reads): `why` in words, never file content.
    Unsupported(String),
}

/// The longest side summary a conflict keeps, in bytes.
pub const SUMMARY_MAX: usize = 120;

/// `s` cut to [`SUMMARY_MAX`] bytes on a character boundary.
pub fn summary(s: &str) -> String {
    let mut end = s.len().min(SUMMARY_MAX);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}
