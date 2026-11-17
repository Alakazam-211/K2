//! A native doc's engine store: the native engine's records, exactly as the engine gave them,
//! one sequence per doc and generation.
//!
//! - [`NATIVE_RECORDS`]: `(doc, generation, index)` → the record's bytes. Record 0 is the
//!   engine's header record (`Engine::header_record`); then every record the engine returns, in
//!   the order it returns them (a change's record, an S record, ...). Never chunked, never
//!   re-encoded: each record is one row, byte for byte at its index.
//! - [`NATIVE_GENS`]: `doc` → the doc's current generation: its number, how many records it
//!   holds and their bytes (so a record source's lengths are O(1)), the apply position its
//!   checkpoint stands for, and the writer's engine config (opaque here: the native adapter's
//!   encoding of the limits and the compaction switch the records were written under, which
//!   their load must use).
//! - A doc's first generation is generation 0, written whole with its first records
//!   ([`super::DocStore::put_native_store`]); a cut writes the next one and drops the old one's
//!   rows in one transaction (`native_cut.rs`).
//! - The first native doc's transaction stamps the store format 8 with
//!   [`super::FEATURE_NATIVE`] (`features.rs`), never lowered. A store with the bit and one
//!   with engine records must be the same store: the open checks both ways
//!   ([`check_native`]).
//!
//! Both tables are in [`super::DOC_TABLES`], so a doc's delete and a store rewrite reach them as
//! every other per-doc table. Only a build with the native engine writes them. A build without
//! it opens a store holding them (its open checks the bit against their rows, `check_held`),
//! copies them byte for byte on a rewrite, and refuses each native doc on its own
//! ([`super::StoreError::NativeEngineMissing`]): it never reads, changes or deletes them. The
//! delete that removes the store's last engine store clears the bit (`features.rs`).

use redb::TableDefinition;

/// `(doc, generation, index)` → a record of the doc's engine store.
pub(super) const NATIVE_RECORDS: TableDefinition<(u64, u64, u64), &[u8]> =
    TableDefinition::new("native_records");

/// A store nonce → the doc whose engine store's record 0 holds it: every nonce this store's
/// engine stores were made with, kept after the doc's delete, so a nonce is never used twice
/// in one store. Shared (not keyed by doc).
pub(super) const NATIVE_NONCES: TableDefinition<&[u8], u64> = TableDefinition::new("native_nonces");

/// `doc` → its current generation ([`NativeGen`]).
pub(super) const NATIVE_GENS: TableDefinition<u64, &[u8]> = TableDefinition::new("native_gens");

/// The open's refusal of engine records in a store without the native bit ([`check_native`],
/// and `check_held` without the native engine).
pub(crate) const RECORDS_WITHOUT_BIT: &str = "engine records in a store without the native bit";

/// The open's check of a store's native bit against its engine stores, without the native
/// engine (which reads their records: [`check_native`]): by their rows alone, from a read
/// transaction, a store with the bit holds at least one engine store and a store holding one
/// has the bit. Refused as [`StoreError::NativeWithoutEngine`] or as damage, before anything
/// reads or writes the store. The store then opens: its native docs are refused one by one
/// ([`StoreError::NativeEngineMissing`]), their rows never read or changed.
pub(super) fn check_held(r: &redb::ReadTransaction, bit: bool) -> Result<(), super::StoreError> {
    use super::StoreError;
    use redb::ReadableTable;
    let held = match r.open_table(NATIVE_GENS) {
        Ok(t) => {
            let first = t.first()?;
            first.is_some()
        }
        Err(redb::TableError::TableDoesNotExist(_)) => false,
        Err(e) => return Err(e.into()),
    };
    if bit && super::features::native_store_refused_whole() {
        return Err(StoreError::NativeWithoutEngine);
    }
    match (bit, held) {
        (true, false) if !super::features::native_bit_unchecked() => {
            Err(StoreError::NativeWithoutEngine)
        }
        (false, true) if !super::features::native_bit_unchecked() => {
            Err(StoreError::Corrupt(RECORDS_WITHOUT_BIT))
        }
        _ => Ok(()),
    }
}
