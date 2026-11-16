//! Rows of store format 8 (`features.rs`): the required-features word, format 7 refused as
//! never written, and the native bit. Each store is made by this build and then stamped by hand
//! through redb itself (this build stamps format 8 only with a store's first chunked version,
//! `tests/chunked_versions.rs`). Every refusal leaves the file's bytes as they were.
//!
//! Plants (`MPC_FORMAT_PLANT`), each caught by its row after a passing control step:
//! `format-7-read-only`, `unknown-feature-ignored`, `native-unchecked`.

use super::features::plant::{under, FormatPlant};
use super::features::REQUIRED_FEATURES;
use super::*;
use crate::errors::Refusal;
use crate::tools::exit;

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "k2mb-features-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
    fn db(&self) -> PathBuf {
        self.0.join("docs.redb")
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.db()).unwrap()
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws-1".into(),
        path: p.into(),
    }
}

/// A store this build made, holding one doc (`a.md`), closed.
fn made(tag: &str) -> TmpDir {
    let t = TmpDir::new(tag);
    let s = DocStore::open(t.db()).unwrap();
    s.create(&DocMeta {
        key: key("a.md"),
        pack: "markdown".into(),
        engine: "yrs-0.28".into(),
        engine_format: 2,
        codec: vec![1, 0],
        created_ms: 1_000,
        synced_hash: None,
        synced_seq: 0,
        declined: None,
    })
    .unwrap();
    t
}

/// Stamp the closed store at `t` format `format`, with features word `word` (`None`: no word,
/// and any word there removed), and a pending rewrite when `pending`: as another build would
/// leave it.
fn stamp(t: &TmpDir, format: u64, word: Option<u64>, pending: bool) {
    let db = Database::open(t.db()).unwrap();
    let w = db.begin_write().unwrap();
    {
        let mut m = w.open_table(META).unwrap();
        m.insert("store_format", format).unwrap();
        match word {
            Some(v) => {
                m.insert(REQUIRED_FEATURES, v).unwrap();
            }
            None => {
                m.remove(REQUIRED_FEATURES).unwrap();
            }
        }
        if pending {
            m.insert(REWRITE_PENDING, 1).unwrap();
        } else {
            m.remove(REWRITE_PENDING).unwrap();
        }
    }
    w.commit().unwrap();
}

/// The control step: the store stamped format 6 opens here, stays 6 and holds its doc.
fn control_format_6_opens(t: &TmpDir) {
    stamp(t, 6, None, false);
    let s = DocStore::open(t.db()).expect("control: a format-6 store opens");
    assert_eq!(s.format().unwrap(), 6, "control: a format-6 store stays 6");
    assert!(s.find(&key("a.md")).unwrap().is_some(), "control: its doc");
    println!("control: a format-6 store opens");
}

/// Open the store at `t`, expecting `want`, and check its bytes are unchanged.
fn refused(t: &TmpDir, want: &StoreError, what: &str) -> StoreError {
    let before = t.bytes();
    let e = DocStore::open(t.db()).err();
    assert_eq!(e.as_ref(), Some(want), "row: {what}");
    assert!(
        t.bytes() == before,
        "row: {what}: the refused store's bytes are unchanged"
    );
    e.unwrap()
}

/// Format 7 was never written: a store stamped 7 is refused at open as a format no kit writes
/// (`MPC-E503`, exit 6), its bytes unchanged, with a pending rewrite too (refused before the
/// rewrite is looked at). `MPC_FORMAT_PLANT=format-7-read-only` (format 7 opens read-only, as
/// kits v0.10.1 to v0.18.0 opened it) fails at the row's first step.
#[test]
fn format_7_is_refused_as_never_written() {
    under(FormatPlant::Format7ReadOnly, || {
        let t = made("f7");
        control_format_6_opens(&t);
        stamp(&t, 7, None, false);
        let want = StoreError::NeverWritten { found: 7 };
        let e = refused(&t, &want, "format 7 is refused at open as never written");
        assert_eq!(e.to_string(), "store is format 7, which no kit writes");
        assert_eq!(
            (e.report().id(), e.report().exit()),
            ("MPC-E503", exit::STORE)
        );
        stamp(&t, 7, None, true);
        refused(
            &t,
            &want,
            "format 7 with a pending rewrite is refused as never written",
        );
        stamp(&t, 7, Some(FEATURE_CHUNKED), false);
        refused(
            &t,
            &want,
            "format 7 with a features word is refused as never written",
        );
    });
}

/// A format-8 store whose word names only features this build knows opens as any store this
/// build writes: its stamp and word stay, its doc is there, and a rewrite (and so a purge)
/// copies it with its meta; a rewrite another build left pending is finished at open.
#[test]
fn format_8_with_known_features_opens_and_is_rewritten() {
    let t = made("f8-known");
    stamp(&t, 8, Some(FEATURE_CHUNKED), false);
    {
        let s = DocStore::open(t.db()).expect("row: a format-8 store naming chunked opens");
        assert_eq!(s.format().unwrap(), 8, "row: the stamp stays 8");
        assert_eq!(s.required_features().unwrap(), FEATURE_CHUNKED);
        assert!(
            s.find(&key("a.md")).unwrap().is_some(),
            "row: its doc is there"
        );
        s.rewrite_file()
            .expect("row: a format-8 store is rewritten here");
        assert_eq!(s.format().unwrap(), 8, "row: the rewrite keeps the stamp");
        assert_eq!(
            s.required_features().unwrap(),
            FEATURE_CHUNKED,
            "row: the rewrite keeps the features word"
        );
        assert!(s.find(&key("a.md")).unwrap().is_some());
    }
    stamp(&t, 8, Some(FEATURE_CHUNKED), true);
    let s = DocStore::open(t.db())
        .expect("row: a rewrite another build left pending on format 8 is finished at open");
    assert_eq!(
        (s.format().unwrap(), s.required_features().unwrap()),
        (8, FEATURE_CHUNKED),
        "row: the finished rewrite keeps the stamp and the word"
    );
    drop(s);
    // and a store below format 8 has no word
    let t6 = made("f8-known-6");
    let s = DocStore::open(t6.db()).unwrap();
    assert_eq!(s.required_features().unwrap(), 0);
}

/// A word naming a bit this build doesn't know is refused at open by name (`MPC-E502`, exit
/// 6), alone or beside known ones, the bytes unchanged. `MPC_FORMAT_PLANT=
/// unknown-feature-ignored` fails at the row's first step.
#[test]
fn format_8_unknown_feature_is_refused_by_name() {
    under(FormatPlant::UnknownFeatureIgnored, || {
        let t = made("f8-unknown");
        control_format_6_opens(&t);
        stamp(&t, 8, Some(FEATURE_CHUNKED | (1 << 5)), false);
        let e = refused(
            &t,
            &StoreError::UnknownFeatures {
                found: 8,
                unknown: 1 << 5,
            },
            "an unknown feature bit beside chunked is refused",
        );
        assert_eq!(
            e.to_string(),
            "store is format 8 and requires feature bit 5, which this build doesn't know: \
             use the newer build"
        );
        let r = e.report();
        assert_eq!((r.id(), r.exit()), ("MPC-E502", exit::STORE));
        let names = r.details().iter().find(|(k, _)| *k == "features");
        assert_eq!(
            names.map(|(_, v)| v.clone()),
            Some(crate::errors::Value::List(vec!["bit 5".into()])),
            "row: the details name the bit"
        );
        stamp(&t, 8, Some((1 << 2) | (1 << 63)), false);
        let e = refused(
            &t,
            &StoreError::UnknownFeatures {
                found: 8,
                unknown: (1 << 2) | (1 << 63),
            },
            "unknown bits alone are refused",
        );
        assert_eq!(
            e.to_string(),
            "store is format 8 and requires features bit 2, bit 63, which this build doesn't \
             know: use the newer build"
        );
    });
}

/// The native bit: this kit stores no engine records, so a store that sets it holds no engine
/// store and is refused at open, alone or with chunked, the bytes unchanged. A rollback onto
/// this kit is the likely reader, so it says to use the newer build (`MPC-E502`, exit 6), the
/// bit named in `details.features`. `MPC_FORMAT_PLANT=native-unchecked` fails at the row's
/// first step.
#[test]
fn format_8_native_without_an_engine_store_is_refused() {
    under(FormatPlant::NativeUnchecked, || {
        let t = made("f8-native");
        control_format_6_opens(&t);
        stamp(&t, 8, Some(FEATURE_NATIVE), false);
        let e = refused(
            &t,
            &StoreError::NativeWithoutEngine,
            "the native bit with no engine store is refused",
        );
        assert_eq!(
            e.to_string(),
            "store sets the native feature but holds no engine store: use the newer build"
        );
        let r = e.report();
        assert_eq!(
            (r.id(), r.exit()),
            ("MPC-E502", exit::STORE),
            "row: the native refusal is store_newer"
        );
        let names = r.details().iter().find(|(k, _)| *k == "features");
        assert_eq!(
            names.map(|(_, v)| v.clone()),
            Some(crate::errors::Value::List(vec!["native".into()])),
            "row: the details name the bit"
        );
        stamp(&t, 8, Some(FEATURE_CHUNKED | FEATURE_NATIVE), false);
        refused(
            &t,
            &StoreError::NativeWithoutEngine,
            "chunked and native with no engine store is refused",
        );
    });
}

/// The word is format 8's, and format 8's only: a format-8 store without it, and a word in a
/// store below format 8, are damage, refused at open with the bytes unchanged. Above what this
/// build reads, the format refuses first.
#[test]
fn format_8_features_word_is_checked_against_the_format() {
    let t = made("f8-word");
    control_format_6_opens(&t);
    stamp(&t, 8, None, false);
    refused(
        &t,
        &StoreError::Corrupt("format 8 without its features word"),
        "format 8 without its word is damage",
    );
    stamp(&t, 6, Some(FEATURE_CHUNKED), false);
    refused(
        &t,
        &StoreError::Corrupt("a features word below format 8"),
        "a word below format 8 is damage",
    );
    stamp(&t, 9, Some(1 << 7), false);
    let e = refused(
        &t,
        &StoreError::Newer { found: 9 },
        "a format above what this build reads refuses before its word is read",
    );
    assert_eq!(e.to_string(), "store is format 9; this build reads up to 8");
}

/// The names of a word's bits, lowest first.
#[test]
fn feature_names_name_known_bits_and_number_the_rest() {
    assert_eq!(feature_names(0), Vec::<String>::new());
    assert_eq!(
        feature_names(FEATURE_CHUNKED | FEATURE_NATIVE | (1 << 9)),
        vec!["chunked", "native", "bit 9"]
    );
    assert_eq!(
        (FEATURE_CHUNKED, FEATURE_NATIVE, STORE_FORMAT_FEATURES),
        (1, 2, 8)
    );
    assert_eq!(STORE_FORMAT_NEVER_WRITTEN, 7);
    assert_eq!(
        (
            STORE_FORMAT_BASE,
            STORE_FORMAT_STRUCTURED,
            STORE_FORMAT,
            STORE_FORMAT_CHUNKED,
            STORE_FORMAT_READ
        ),
        (5, 6, 6, 8, 8)
    );
}

/// The format constants keep their meaning: `STORE_FORMAT` is the format a store with no
/// opt-in feature is stamped (6), a store's first chunked version stamps `STORE_FORMAT_CHUNKED`
/// (8), and this build reads up to `STORE_FORMAT_READ` (8): every message and detail that says
/// what this build reads or rewrites names 8.
#[test]
fn the_format_constants_keep_their_meaning() {
    assert_eq!(
        STORE_FORMAT, 6,
        "row: STORE_FORMAT is the stamp of a store with no opt-in feature"
    );
    assert_eq!(
        STORE_FORMAT_CHUNKED, 8,
        "row: a store's first chunked version stamps 8"
    );
    assert_eq!(STORE_FORMAT_READ, 8, "row: this build reads up to 8");
    let max = |e: &StoreError| {
        e.report()
            .details()
            .iter()
            .find(|(k, _)| *k == "max")
            .map(|(_, v)| v.clone())
    };
    let newer = StoreError::Newer { found: 9 };
    assert_eq!(
        newer.to_string(),
        "store is format 9; this build reads up to 8",
        "row: the refusal of a newer store names what this build reads"
    );
    assert_eq!(
        max(&newer),
        Some(crate::errors::Value::from(8u64)),
        "row: its details name 8"
    );
    let rewrite = StoreError::RewriteNewer { found: 9 };
    assert!(
        rewrite.to_string().contains("only up to format 8"),
        "row: the rewrite refusal names what this build reads: {rewrite}"
    );
    assert_eq!(
        max(&rewrite),
        Some(crate::errors::Value::from(8u64)),
        "row: its details name 8"
    );
}
