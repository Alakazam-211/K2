//! Store format 8: the required-features word (`docs/design/version-storage-r2.md`, section 10).
//!
//! - **Format 8** means the store's `meta` holds a word of required features under
//!   [`REQUIRED_FEATURES`]. From 8 on, a new on-disk capability is a feature bit, not a format
//!   number: two independent capabilities can't be ordered on one number.
//! - **The features**: [`FEATURE_CHUNKED`] (the store holds chunked versions),
//!   [`FEATURE_NATIVE`] (the store holds engine stores: a container-level guard, never a claim
//!   about the engine's own header bits, which the kit never writes, reads or mirrors) and
//!   [`FEATURE_SEQ_FLOOR`] (a recovery after a lost group of grouped commits recorded lost
//!   versions, `seq_floor.rs`: a kit without the floor would issue them again).
//! - **At open, before anything else reads or writes the store**, a kit refuses a word naming a
//!   bit it doesn't know, by name ([`StoreError::UnknownFeatures`]); a format-8 store without
//!   its word, or a word in a store below format 8, is damage ([`StoreError::Corrupt`]).
//! - **The native bit** and the store's engine stores are one fact, checked at open both ways:
//!   a store with the bit and no engine store is refused ([`StoreError::NativeWithoutEngine`]),
//!   and engine records in a store without the bit are damage. With the native engine the
//!   check also reads each engine store's record 0 (`native.rs`); without it, their rows alone.
//!   **A build without the native engine opens a store holding native docs**: it refuses those
//!   docs one by one ([`StoreError::NativeEngineMissing`], `MPC-E169`), never the store, so the
//!   store's other docs work, and a rewrite copies the engine store's tables through
//!   (`native.rs`).
//! - **Format 7 was never written** (it was declared for the native engine's stores, and no
//!   shipped host turned that engine on): a format-7 store is refused at open, before anything
//!   reads it ([`StoreError::NeverWritten`]).
//!
//! A store's first chunked version stamps format 8 with [`FEATURE_CHUNKED`], lazily, in its own
//! transaction (`chunks.rs`), and the stamp is never lowered. The native bit is set by a
//! store's first engine store (`native.rs`) and cleared by the transaction that removes its
//! last one ([`clear_native_if_none`]): the format stays 8. Kits v0.14.0 to v0.18.0 read up to
//! format 7 and refuse a format-8 store at open ([`StoreError::Newer`]), its bytes unchanged.

use redb::Database;

use super::{DocStore, StoreError, META, STORE_FORMAT_READ};

/// The first store format whose `meta` holds a required-features word.
pub const STORE_FORMAT_FEATURES: u64 = 8;

/// A format no kit writes: declared for the native engine's stores and retired unwritten. A
/// store stamped with it is refused at open ([`StoreError::NeverWritten`]).
pub const STORE_FORMAT_NEVER_WRITTEN: u64 = 7;

/// `meta` key of a format-8 store: its required-features word.
pub(super) const REQUIRED_FEATURES: &str = "required_features";

/// Required feature: the store holds chunked versions.
pub const FEATURE_CHUNKED: u64 = 1 << 0;

/// Required feature: the store holds engine stores (the native engine's).
pub const FEATURE_NATIVE: u64 = 1 << 1;

/// Required feature: a recovery recorded versions a lost group of grouped commits may have
/// had (`seq_floor.rs`): a kit that doesn't keep the seq floor would issue them again, so it
/// refuses the store at open.
pub const FEATURE_SEQ_FLOOR: u64 = 1 << 2;

/// Every feature bit this build knows.
const FEATURES_KNOWN: u64 = FEATURE_CHUNKED | FEATURE_NATIVE | FEATURE_SEQ_FLOOR;

/// The names of the bits set in `bits`, lowest first: a known feature by its name, any other
/// as `bit N`.
pub(crate) fn feature_names(bits: u64) -> Vec<String> {
    (0..u64::BITS)
        .filter(|i| bits & (1 << i) != 0)
        .map(|i| match 1u64 << i {
            FEATURE_CHUNKED => "chunked".to_owned(),
            FEATURE_NATIVE => "native".to_owned(),
            FEATURE_SEQ_FLOOR => "seq floor".to_owned(),
            _ => format!("bit {i}"),
        })
        .collect()
}

/// [`StoreError::UnknownFeatures`]'s message: the store's format and each unknown bit by name.
pub(super) fn unknown_features_message(found: u64, unknown: u64) -> String {
    let names = feature_names(unknown);
    let noun = if names.len() == 1 {
        "feature"
    } else {
        "features"
    };
    format!(
        "store is format {found} and requires {noun} {}, which this build doesn't know: use \
         the newer build",
        names.join(", ")
    )
}

/// What a store's `meta` says it is, checked before anything else reads or writes it, from a
/// read transaction: its format (`None`: a new store), refused when it is one no kit writes
/// ([`STORE_FORMAT_NEVER_WRITTEN`]), above what this build reads, or (format 8) when its
/// features word is missing, names a bit this build doesn't know, or sets the native bit.
pub(super) fn check_stamp(db: &Database) -> Result<Option<u64>, StoreError> {
    let r = db.begin_read()?;
    let meta = match r.open_table(META) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let format = meta.get("store_format")?.map(|v| v.value());
    let word = meta.get(REQUIRED_FEATURES)?.map(|v| v.value());
    let Some(f) = format else {
        if word.is_some() {
            return Err(StoreError::Corrupt(
                "a features word in a store without a format",
            ));
        }
        return Ok(None);
    };
    if f == STORE_FORMAT_NEVER_WRITTEN && !planted(plant::FormatPlant::Format7ReadOnly) {
        return Err(StoreError::NeverWritten { found: f });
    }
    if f > STORE_FORMAT_READ {
        return Err(StoreError::Newer { found: f });
    }
    let checked = match (f >= STORE_FORMAT_FEATURES, word) {
        (false, None) => Ok(Some(f)),
        (false, Some(_)) => Err(StoreError::Corrupt("a features word below format 8")),
        (true, None) => Err(StoreError::Corrupt("format 8 without its features word")),
        (true, Some(w)) => {
            let unknown = w & !FEATURES_KNOWN;
            if unknown != 0 && !planted(plant::FormatPlant::UnknownFeatureIgnored) {
                return Err(StoreError::UnknownFeatures { found: f, unknown });
            }
            // the bit against the engine stores: with the native engine, their records too;
            // without it, their rows alone (its native docs are refused one by one, later)
            super::native::check_held(&r, w & FEATURE_NATIVE != 0)?;
            Ok(Some(f))
        }
    }?;
    // below format 8 there is no bit: a store holding engine records is damage
    if f < STORE_FORMAT_FEATURES {
        super::native::check_held(&r, false)?;
    }
    Ok(checked)
}

/// Raise the store to format 8 with feature `bit` in `w`'s transaction, never lowering the
/// format or clearing a bit.
pub(super) fn stamp_feature(w: &redb::WriteTransaction, bit: u64) -> Result<(), StoreError> {
    use redb::ReadableTable;
    let mut meta = w.open_table(META)?;
    let f = meta.get("store_format")?.map_or(0, |v| v.value());
    if f < STORE_FORMAT_FEATURES {
        meta.insert("store_format", STORE_FORMAT_FEATURES)?;
    }
    let word = meta.get(REQUIRED_FEATURES)?.map_or(0, |v| v.value());
    if word & bit == 0 {
        meta.insert(REQUIRED_FEATURES, word | bit)?;
    }
    Ok(())
}

/// Clear the native bit in `w`'s transaction when the store holds no engine store any more:
/// the transaction that removes a store's last engine store (its last native doc's delete, or
/// a space's) clears it, so no store is left with the bit and no engine store, which every
/// build refuses at open. The format stays: it is never lowered, and a format-8 store's word
/// may be 0. Called after the removal, in the same transaction.
pub(super) fn clear_native_if_none(w: &redb::WriteTransaction) -> Result<(), StoreError> {
    use redb::ReadableTable;
    let mut meta = w.open_table(META)?;
    let word = meta.get(REQUIRED_FEATURES)?.map_or(0, |v| v.value());
    if word & FEATURE_NATIVE == 0 || planted(plant::FormatPlant::NativeBitKept) {
        return Ok(());
    }
    let held = w.open_table(super::native::NATIVE_GENS)?.first()?.is_some();
    if !held {
        meta.insert(REQUIRED_FEATURES, word & !FEATURE_NATIVE)?;
    }
    Ok(())
}

/// The plant `native-unchecked`: the open doesn't check the native bit (test builds only).
pub(super) fn native_bit_unchecked() -> bool {
    planted(plant::FormatPlant::NativeUnchecked)
}

/// The plant `native-store-refused-whole` (test builds only): without the native engine, a
/// store with the native bit is refused whole at open, as before its native docs were refused
/// one by one.
pub(super) fn native_store_refused_whole() -> bool {
    planted(plant::FormatPlant::NativeStoreRefusedWhole)
}

/// The plant `native-tables-dropped` (test builds only): a rewrite leaves out the engine
/// store's tables, as one that drops the tables it doesn't know would.
pub(super) fn native_tables_dropped() -> bool {
    planted(plant::FormatPlant::NativeTablesDropped)
}

impl DocStore {
    /// The store's required-features word (0 below format 8; [`FEATURE_CHUNKED`],
    /// [`FEATURE_NATIVE`]).
    pub fn required_features(&self) -> Result<u64, StoreError> {
        let r = self.begin_read()?;
        let w = r
            .open_table(META)?
            .get(REQUIRED_FEATURES)?
            .map_or(0, |v| v.value());
        Ok(w)
    }
}

/// Whether `p` is planted (test builds only).
#[cfg(test)]
fn planted(p: plant::FormatPlant) -> bool {
    plant::planted(p)
}

/// Never planted outside test builds.
#[cfg(not(test))]
fn planted(_: plant::FormatPlant) -> bool {
    false
}

/// Planted faults of the open's format checks (test builds only), each caught by its row in
/// `features_tests.rs`: installed per thread, for one call, by the row from `MPC_FORMAT_PLANT`.
pub(crate) mod plant {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum FormatPlant {
        /// A format-7 store opens as a format this build reads but doesn't write, as kits
        /// v0.10.1 to v0.18.0 opened it (`format-7-read-only`).
        Format7ReadOnly,
        /// A features word's unknown bits are ignored (`unknown-feature-ignored`).
        UnknownFeatureIgnored,
        /// The native bit is not checked (`native-unchecked`).
        NativeUnchecked,
        /// Without the native engine, a store with the native bit is refused whole at open
        /// (`native-store-refused-whole`).
        NativeStoreRefusedWhole,
        /// A rewrite leaves out the engine store's tables (`native-tables-dropped`).
        NativeTablesDropped,
        /// The native bit is kept when the store's last engine store goes (`native-bit-kept`).
        NativeBitKept,
    }

    #[cfg(test)]
    pub(crate) use on::{planted, under};

    #[cfg(test)]
    mod on {
        use super::FormatPlant;
        use std::cell::Cell;

        impl FormatPlant {
            fn named(name: &str) -> Option<Self> {
                Some(match name {
                    "format-7-read-only" => FormatPlant::Format7ReadOnly,
                    "unknown-feature-ignored" => FormatPlant::UnknownFeatureIgnored,
                    "native-unchecked" => FormatPlant::NativeUnchecked,
                    "native-store-refused-whole" => FormatPlant::NativeStoreRefusedWhole,
                    "native-tables-dropped" => FormatPlant::NativeTablesDropped,
                    "native-bit-kept" => FormatPlant::NativeBitKept,
                    _ => return None,
                })
            }
        }

        thread_local! {
            static PLANT: Cell<Option<FormatPlant>> = const { Cell::new(None) };
        }

        /// Whether `p` is planted on this thread.
        pub(crate) fn planted(p: FormatPlant) -> bool {
            PLANT.with(|c| c.get() == Some(p))
        }

        /// Run `f` with `p` planted on this thread when `MPC_FORMAT_PLANT` names it (printed
        /// either way, since a job's record shows the variable's name, not its value); any
        /// other value is refused, so a misspelt plant can't pass as a clean run.
        pub(crate) fn under<R>(p: FormatPlant, f: impl FnOnce() -> R) -> R {
            let v = std::env::var("MPC_FORMAT_PLANT");
            println!("MPC_FORMAT_PLANT={}", v.as_deref().unwrap_or("(unset)"));
            let on = match v.as_deref() {
                Ok(name) => match FormatPlant::named(name) {
                    Some(named) => named == p,
                    None => unreachable!("MPC_FORMAT_PLANT={name}: no such plant"),
                },
                Err(_) => false,
            };
            if !on {
                return f();
            }
            PLANT.with(|c| c.set(Some(p)));
            let r = f();
            PLANT.with(|c| c.set(None));
            r
        }
    }
}
