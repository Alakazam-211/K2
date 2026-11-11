//! Which blob pack a tier-0 file opens with, and the verbs it offers.
//!
//! Every other file is `AnyFile` (presence, a lease, versions, restore, and
//! comments on the whole doc). PSD/PSB files (`.psd`, `.psb`) open with
//! K2 Cores' PSD pack, which reads each version into the kit's structure
//! index (a tree of layers), so they also take the index verbs (`layer
//! list|show|diff|blame`, `outline`, `node`), claims by layer and comments
//! pinned to a layer. Agents never write them (`MPC-E715`): they propose a
//! new file and a comment.
//!
//! The verbs come from the kit: [`registry_for`] is
//! `Registry::for_blob_pack(&pack)`, so `/cli/docs/tool` routes exactly the
//! verbs the kit says run through `BlobSession::run_tool`.

use std::sync::{Arc, OnceLock};

use k2_mpc_base::parity::Registry;
use k2_mpc_base::session::{AnyFile, BlobPack};

/// The lower-case extensions the PSD pack reads.
pub const PSD_EXTENSIONS: &[&str] = &["psd", "psb"];

/// What a tier-0 file is, for its pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    /// Any other file: no structure index.
    Any,
    /// A PSD/PSB file: the PSD pack, with an index of its layers.
    Psd,
}

/// The kind of `rel` (a workspace-relative path), by its extension.
pub fn kind_of(rel: &str) -> BlobKind {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && PSD_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)) => BlobKind::Psd,
        _ => BlobKind::Any,
    }
}

/// The pack a tier-0 doc of `rel` opens with.
pub fn pack_for(rel: &str) -> Arc<dyn BlobPack> {
    match kind_of(rel) {
        BlobKind::Psd => Arc::new(k2_mpc_psd::PsdPack),
        BlobKind::Any => Arc::new(AnyFile),
    }
}

/// The verbs of a tier-0 doc of `rel`: `Registry::for_blob_pack` of its pack.
pub fn registry_for(rel: &str) -> &'static Registry {
    registry_of(kind_of(rel))
}

pub fn registry_of(kind: BlobKind) -> &'static Registry {
    static ANY: OnceLock<Registry> = OnceLock::new();
    static PSD: OnceLock<Registry> = OnceLock::new();
    match kind {
        BlobKind::Any => ANY.get_or_init(|| Registry::for_blob_pack(&AnyFile).expect("the any-file registry")),
        BlobKind::Psd => PSD.get_or_init(|| Registry::for_blob_pack(&k2_mpc_psd::PsdPack).expect("the PSD pack's registry")),
    }
}

/// Whether `rel`'s pack keeps a structure index.
pub fn indexes(rel: &str) -> bool {
    kind_of(rel) == BlobKind::Psd
}

/// The pack's name in the export worker's `index` op (`psd`).
pub fn index_pack_name(kind: BlobKind) -> Option<&'static str> {
    match kind {
        BlobKind::Psd => Some(k2_mpc_psd::pack::PACK),
        BlobKind::Any => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psd_and_psb_files_open_with_the_psd_pack_and_others_with_any_file() {
        for p in ["a.psd", "dir/B.PSD", "x/y/poster.psb", "c.Psb"] {
            assert_eq!(kind_of(p), BlobKind::Psd, "{p}");
            assert!(pack_for(p).index_spec().is_some(), "{p}: the PSD pack indexes");
        }
        for p in ["a.png", "psd", ".psd", "a.psd.zip", "dir.psd/a.png", "a.psdx"] {
            assert_eq!(kind_of(p), BlobKind::Any, "{p}");
            assert!(pack_for(p).index_spec().is_none(), "{p}");
        }
    }

    /// The kit decides the verbs: any file gets `for_tier(Blob)` plus the
    /// annotation verbs; a PSD/PSB file adds the layer verbs, `outline`,
    /// `node` and claims by node, every one through `run_tool`.
    #[test]
    fn the_registries_are_the_kits_for_each_pack() {
        let any = registry_of(BlobKind::Any);
        let psd = registry_of(BlobKind::Psd);
        for v in ["comment", "reply", "resolve", "resolve --reopen", "comment --delete", "comments", "comments --log", "reanchor"] {
            for (name, r) in [("any", any), ("psd", psd)] {
                let spec = r.get(v).unwrap_or_else(|| panic!("{v} missing from the {name} registry"));
                assert!(spec.runs_through_run_tool(), "{v} ({name}) runs through run_tool");
            }
        }
        for v in ["layer list", "layer show", "layer diff", "layer blame", "outline", "node", "claim", "claim --renew", "claim --release"] {
            let spec = psd.get(v).unwrap_or_else(|| panic!("{v} missing from the PSD registry"));
            assert!(spec.runs_through_run_tool(), "{v} runs through run_tool");
            assert!(any.get(v).is_none(), "{v} is not an any-file verb");
        }
        assert_eq!(psd.form_of(&["layer".into(), "list".into(), "--depth".into(), "1".into()]), "layer list");
        assert_eq!(psd.form_of(&["claim".into(), "--node".into(), "@lyid:4".into(), "--release".into()]), "claim --release");
    }
}
