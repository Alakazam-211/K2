//! Entry points for the fuzz targets (`fuzz/` at the repo root; feature `fuzzing`, off by
//! default, never enabled by a host). Not a stable API.
//!
//! The store's own record decoders: each record kind either refuses arbitrary bytes with a
//! typed error, or decodes them to a record whose encoding decodes again to the same
//! encoding (a fixpoint; for canonical records, the input bytes themselves). Engine bytes a
//! record carries (a snapshot's state, a log entry's update) are never decoded here.

/// Every record kind [`record`] takes.
pub fn records() -> &'static [&'static str] {
    crate::store::FUZZ_RECORDS
}

/// Decode `bytes` as record kind `name` (one of [`records`]) and check the oracle; panics
/// on a violation (a finding). Returns whether the bytes decoded.
pub fn record(name: &str, bytes: &[u8]) -> bool {
    if name == "file stat" {
        return crate::session::fuzz_file_stat(bytes);
    }
    crate::store::fuzz_record(name, bytes).unwrap_or_else(|| panic!("unknown record kind {name:?}"))
}
