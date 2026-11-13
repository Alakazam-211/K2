//! The carrier pack's byte-exact row (the pack contract: `docs/PACK_TEMPLATE.md`, "A format
//! the pack models only in part"), as the kit runs it for any pack that keeps a carrier
//! ([`super::Pack::wants_carrier`]): an untouched doc writes back byte for byte. A pack calls
//! [`carrier_judge`] over its corpus from its own tests (feature `test-support`); a pack that
//! doesn't keep a carrier gets [`CarrierVerdict::NotACarrierPack`] and nothing is checked.
//!
//! Per file, three legs, every failure reported (not just the first):
//! 1. **the hook**: `encode_from(file, decode(file))` is the file, byte for byte (for a pack that
//!    pairs, `encode_from_paired` with the pairing of `decode(file)`);
//! 2. **through the kit**: the file opened live renders as itself (`file_now`), and a flush leaves
//!    it as it was;
//! 3. **after a restart**: a new manager on the same store opens it again, renders it as itself,
//!    and a flush leaves it as it was.
//!
//! The kit leans on this row: opening an untouched carrier file renders nothing, because the
//! doc as imported is taken to render as the file (`docs/SESSIONS.md`, "The carrier").

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::{
    AuditEvent, Config, Host, Pack, Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use crate::doc::Neutral;
use crate::store::{DocKey, DocStore};

/// What [`carrier_judge`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarrierVerdict {
    /// The pack doesn't keep a carrier: the row doesn't apply, nothing was checked.
    NotACarrierPack,
    /// Every file wrote back byte for byte, all three legs.
    ByteExact { files: usize },
}

struct JudgeHost;

impl Host for JudgeHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        // 2026-10-27 14:30:00 UTC: a fixed clock, so nothing falls due
        1_793_111_400_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// A scratch folder for one file's legs, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The byte-exact row over `corpus` (each file's name, with its extension, and its bytes) for
/// `pack`: [`CarrierVerdict::NotACarrierPack`] when it doesn't keep a carrier; otherwise every
/// failure, one line each, naming the file and the leg.
pub fn carrier_judge(
    pack: Arc<dyn Pack>,
    corpus: &[(&str, &[u8])],
) -> Result<CarrierVerdict, Vec<String>> {
    if !pack.wants_carrier() {
        return Ok(CarrierVerdict::NotACarrierPack);
    }
    if corpus.is_empty() {
        return Err(vec!["carrier row: an empty corpus checks nothing".into()]);
    }
    let mut fails = Vec::new();
    for (name, bytes) in corpus {
        if let Err(e) = the_hook(pack.as_ref(), bytes) {
            fails.push(format!("carrier row: {name}: the hook: {e}"));
        }
        if let Err(e) = through_the_kit(&pack, name, bytes) {
            fails.push(format!("carrier row: {name}: {e}"));
        }
    }
    match fails.is_empty() {
        true => Ok(CarrierVerdict::ByteExact {
            files: corpus.len(),
        }),
        false => Err(fails),
    }
}

/// Leg 1: `encode_from` of the file's own content, in schema order, is the file.
fn the_hook(pack: &dyn Pack, bytes: &[u8]) -> Result<(), String> {
    let decoded = pack
        .decode(bytes)
        .map_err(|d| format!("decode declined: {} ({})", d.message, d.code))?;
    let schema = pack.schema();
    let mut roots: Vec<(String, Neutral)> = decoded.roots;
    roots.sort_by_key(|(r, _)| {
        schema
            .roots
            .iter()
            .position(|(n, _)| n == r)
            .unwrap_or(usize::MAX)
    });
    // a pack that pairs: the file's own keys, as its decode minted them
    let out = match pack.wants_pairing() {
        true => {
            let pairing = pack
                .pairing(&roots, &decoded.codec)
                .map_err(|e| format!("pairing failed: {}", e.0))?;
            pack.encode_from_paired(bytes, &pairing, &roots, &decoded.codec)
                .map_err(|e| format!("encode_from_paired failed: {e}"))?
        }
        false => pack
            .encode_from(bytes, &roots, &decoded.codec)
            .map_err(|e| format!("encode_from failed: {}", e.0))?,
    };
    if out != bytes {
        return Err(first_difference(&out, bytes));
    }
    Ok(())
}

/// Legs 2 and 3: open live, render, flush; again after a restart.
fn through_the_kit(pack: &Arc<dyn Pack>, name: &str, bytes: &[u8]) -> Result<(), String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-carrier-judge-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let scratch = Scratch(dir);
    let ws = scratch.0.join("ws");
    std::fs::create_dir_all(&ws).map_err(|e| format!("scratch folder: {e}"))?;
    let ws = std::fs::canonicalize(&ws).map_err(|e| format!("scratch folder: {e}"))?;
    let file_name = Path::new(name)
        .file_name()
        .ok_or_else(|| format!("{name:?} names no file"))?;
    let path = ws.join(file_name);
    std::fs::write(&path, bytes).map_err(|e| format!("writing the file: {e}"))?;
    let store = Arc::new(
        DocStore::open(scratch.0.join("docs.redb")).map_err(|e| format!("the store: {e}"))?,
    );
    let host: Arc<dyn Host> = Arc::new(JudgeHost);
    // no size limit: the host's for this type wins over the pack's own default
    let config = Config {
        keyed_max_entries: usize::MAX,
        keyed_max_entries_by_pack: [(pack.schema().name.to_owned(), usize::MAX)].into(),
        grid_max_cells: usize::MAX,
        ..Config::default()
    };
    let key = DocKey {
        space: "ws".into(),
        path: file_name.to_string_lossy().into_owned(),
    };
    for leg in ["opened live", "after a restart"] {
        let mgr = SessionManager::new(store.clone(), host.clone(), config.clone());
        let s = mgr
            .open_file(pack.clone(), &key, &path, &ws)
            .map_err(|e| format!("{leg}: didn't open live: {e}"))?;
        let who = Participant::new("judge", "judge", "#123456", ParticipantKind::Person, "test");
        let c = s
            .join(who, Stay::Held)
            .map_err(|e| format!("{leg}: join: {e}"))?
            .conn;
        let now = s.file_now(c).map_err(|e| format!("{leg}: file_now: {e}"))?;
        if now.bytes != bytes {
            return Err(format!(
                "{leg}: the doc doesn't render as its file: {}",
                first_difference(&now.bytes, bytes)
            ));
        }
        s.flush(c).map_err(|e| format!("{leg}: flush: {e}"))?;
        let after = std::fs::read(&path).map_err(|e| format!("{leg}: reading the file: {e}"))?;
        if after != bytes {
            return Err(format!(
                "{leg}: the write-back changed the file: {}",
                first_difference(&after, bytes)
            ));
        }
        drop(s);
        mgr.close(&key, "carrier judge")
            .map_err(|e| format!("{leg}: close: {e}"))?;
    }
    Ok(())
}

/// Where `got` first differs from `want` (lengths, then the offset).
fn first_difference(got: &[u8], want: &[u8]) -> String {
    let at = got
        .iter()
        .zip(want)
        .position(|(a, b)| a != b)
        .unwrap_or(got.len().min(want.len()));
    format!(
        "{} bytes, not the file's {}; first difference at byte {at}",
        got.len(),
        want.len()
    )
}
