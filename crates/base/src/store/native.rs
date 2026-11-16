//! A native doc's engine store: the native engine's records, exactly as the engine gave them,
//! one sequence per doc and generation.
//!
//! - [`NATIVE_RECORDS`]: `(doc, generation, index)` → the record's bytes.
//! - [`NATIVE_GENS`]: `doc` → the doc's current generation.
//! - [`NATIVE_NONCES`]: every store nonce the store's engine stores were made with, kept after a
//!   doc's delete.
//!
//! Only a build with the native engine's store writes them. This build defines the tables
//! (their keys and values as bytes) only to keep them: its open checks the store's native bit
//! against them by their rows ([`check_held`]), a rewrite copies them through byte for byte
//! (they are in [`super::DOC_TABLES`] and [`super::SHARED_TABLES`]), and each native doc is
//! refused on its own ([`super::StoreError::NativeEngineMissing`]): their rows are never read,
//! changed or deleted here.

use redb::TableDefinition;

/// `(doc, generation, index)` → a record of the doc's engine store.
pub(super) const NATIVE_RECORDS: TableDefinition<(u64, u64, u64), &[u8]> =
    TableDefinition::new("native_records");

/// A store nonce → the doc whose engine store's record 0 holds it: every nonce this store's
/// engine stores were made with, kept after the doc's delete, so a nonce is never used twice
/// in one store. Shared (not keyed by doc).
pub(super) const NATIVE_NONCES: TableDefinition<&[u8], u64> = TableDefinition::new("native_nonces");

/// `doc` → its current generation (the native engine's store encodes it).
pub(super) const NATIVE_GENS: TableDefinition<u64, &[u8]> = TableDefinition::new("native_gens");

/// The open's refusal of engine records in a store without the native bit ([`check_held`]).
pub(crate) const RECORDS_WITHOUT_BIT: &str = "engine records in a store without the native bit";

/// The open's check of a store's native bit against its engine stores, without the native
/// engine (which reads their records): by their rows alone, from a read transaction, a store
/// with the bit holds at least one engine store and a store holding one has the bit. Refused as
/// [`super::StoreError::NativeWithoutEngine`] or as damage, before anything reads or writes the
/// store. The store then opens: its native docs are refused one by one
/// ([`super::StoreError::NativeEngineMissing`]), their rows never read or changed.
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
