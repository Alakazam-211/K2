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
//! Three more packs index their files the same way, each behind its own
//! multiplayer core (off by default; off, their files are plain any-file
//! docs, [`kind_of`]):
//! - **the timeline core** (K2 Cores' timeline pack): `.otio`, `.fcpxml` and
//!   `.prproj` files, `clip list|show|diff|blame` over sequences, tracks,
//!   items and markers. An `.otio` file is a live keyed doc while the core is
//!   on (`super::target::Tier::Timeline`); it is indexed here only when it is
//!   a tier-0 doc (its live open was declined, or it was stored as one).
//! - **the fab core** (the fab pack): photoplot files (`feature
//!   list|show|diff|blame`) and drill files (`hole ...`), read with the
//!   pack's common-export reading (strict first). Read only.
//! - **the blend core** (the BLEND pack): `.blend` files, `object
//!   list|show|diff|blame`. A `.blend` is a live keyed doc while the core is
//!   on; it is indexed here as a tier-0 doc (a version the live tier
//!   declines, or one stored as one).
//!
//! The verbs come from the kit: [`registry_for`] is
//! `Registry::for_blob_pack(&pack)`, so `/cli/docs/tool` routes exactly the
//! verbs the kit says run through `BlobSession::run_tool`.

use std::sync::{Arc, OnceLock};

use k2_core::docs_cores;
use k2_mpc_base::parity::Registry;
use k2_mpc_base::session::{AnyFile, BlobPack};

/// The lower-case extensions the PSD pack reads.
pub const PSD_EXTENSIONS: &[&str] = &["psd", "psb"];
/// Video timelines (the timeline core): OTIO, FCPXML and PRPROJ. A
/// `.fcpxmld` bundle is a folder; its `Info.fcpxml` is an ordinary `.fcpxml`.
pub const TIMELINE_EXTENSIONS: &[&str] = &["otio", "fcpxml", "prproj"];
/// Photoplot files (the fab core): the format's own extensions and the
/// common layer names (copper, silkscreen, mask, outline).
pub const PHOTOPLOT_EXTENSIONS: &[&str] = &["gbr", "ger", "pho", "gtl", "gbl", "gto", "gbo", "gts", "gbs", "gko"];
/// Drill files (the fab core).
pub const DRILL_EXTENSIONS: &[&str] = &["drl", "xnc", "xln", "ncd"];
/// BLEND scenes (the blend core).
pub const BLEND_EXTENSIONS: &[&str] = &["blend"];

/// What a tier-0 file is, for its pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlobKind {
    /// Any other file: no structure index.
    Any,
    /// A PSD/PSB file: the PSD pack, with an index of its layers.
    Psd,
    /// An OTIO timeline (the timeline core).
    Otio,
    /// An FCPXML timeline (the timeline core).
    Fcpxml,
    /// A PRPROJ project (the timeline core).
    Prproj,
    /// A photoplot file (the fab core).
    Photoplot,
    /// A drill file (the fab core): strict XNC, else the common export.
    Drill,
    /// A BLEND scene (the blend core).
    Blend,
}

impl BlobKind {
    /// Every kind, in a fixed order.
    pub const ALL: [BlobKind; 8] =
        [BlobKind::Any, BlobKind::Psd, BlobKind::Otio, BlobKind::Fcpxml, BlobKind::Prproj, BlobKind::Photoplot, BlobKind::Drill, BlobKind::Blend];

    fn index(self) -> usize {
        BlobKind::ALL.iter().position(|k| *k == self).expect("in ALL")
    }

    /// The multiplayer core this kind's pack needs besides its file's: the
    /// timeline, fab and blend cores. `None` for PSD and any other file (the
    /// any-file core).
    pub fn core(self) -> Option<&'static str> {
        match self {
            BlobKind::Otio | BlobKind::Fcpxml | BlobKind::Prproj => Some(docs_cores::TIMELINE),
            BlobKind::Photoplot | BlobKind::Drill => Some(docs_cores::FAB),
            BlobKind::Blend => Some(docs_cores::BLEND),
            BlobKind::Any | BlobKind::Psd => None,
        }
    }

    /// A fresh pack of this kind.
    pub fn pack(self) -> Arc<dyn BlobPack> {
        match self {
            BlobKind::Any => Arc::new(AnyFile),
            BlobKind::Psd => Arc::new(k2_mpc_psd::PsdPack),
            BlobKind::Otio => Arc::new(k2_mpc_timeline::OtioPack),
            BlobKind::Fcpxml => Arc::new(k2_mpc_timeline::FcpxmlPack),
            BlobKind::Prproj => Arc::new(k2_mpc_timeline::PrprojPack),
            BlobKind::Photoplot => Arc::new(k2_mpc_fab::PhotoplotPack),
            BlobKind::Drill => Arc::new(k2_mpc_fab::CommonDrillPack),
            // Read-only for now: its index, never a merge or an agent write (`blend_read_only`).
            BlobKind::Blend => Arc::new(super::blend_read_only::BlendIndexOnly::default()),
        }
    }
}

/// The lower-case-insensitive extension of `rel`'s name, when it has a name
/// before the dot.
fn extension(rel: &str) -> Option<&str> {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    name.rsplit_once('.').and_then(|(stem, ext)| (!stem.is_empty() && !ext.is_empty()).then_some(ext))
}

fn listed(list: &[&str], ext: &str) -> bool {
    list.iter().any(|e| e.eq_ignore_ascii_case(ext))
}

/// The kind of `rel` by its name alone, whatever the cores say.
pub fn kind_by_name(rel: &str) -> BlobKind {
    let Some(ext) = extension(rel) else { return BlobKind::Any };
    if listed(PSD_EXTENSIONS, ext) {
        BlobKind::Psd
    } else if ext.eq_ignore_ascii_case("otio") {
        BlobKind::Otio
    } else if ext.eq_ignore_ascii_case("fcpxml") {
        BlobKind::Fcpxml
    } else if ext.eq_ignore_ascii_case("prproj") {
        BlobKind::Prproj
    } else if listed(PHOTOPLOT_EXTENSIONS, ext) {
        BlobKind::Photoplot
    } else if listed(DRILL_EXTENSIONS, ext) {
        BlobKind::Drill
    } else if listed(BLEND_EXTENSIONS, ext) {
        BlobKind::Blend
    } else {
        BlobKind::Any
    }
}

/// The file-type core `rel`'s name belongs to (timeline, fab or blend), on
/// or off; `None` for every other file.
pub fn core_by_name(rel: &str) -> Option<&'static str> {
    kind_by_name(rel).core()
}

/// Whether `rel` is a file the timeline, fab or blend core reads, by name:
/// never live text (line merges would break their structure), whatever the
/// text pack would guess.
pub fn is_pack_file(rel: &str) -> bool {
    core_by_name(rel).is_some()
}

/// The kind of `rel` now: [`kind_by_name`], except that a kind whose core is
/// off is any other file (no index), as before the core.
pub fn kind_of(rel: &str) -> BlobKind {
    let k = kind_by_name(rel);
    match k.core() {
        Some(core) if !super::cores::is_on(core) => BlobKind::Any,
        _ => k,
    }
}

/// The pack a tier-0 doc of `rel` opens with now.
pub fn pack_for(rel: &str) -> Arc<dyn BlobPack> {
    kind_of(rel).pack()
}

/// The verbs of a tier-0 doc of `rel`: `Registry::for_blob_pack` of its pack.
pub fn registry_for(rel: &str) -> &'static Registry {
    registry_of(kind_of(rel))
}

pub fn registry_of(kind: BlobKind) -> &'static Registry {
    static REGISTRIES: [OnceLock<Registry>; 8] = [const { OnceLock::new() }; 8];
    REGISTRIES[kind.index()].get_or_init(|| {
        Registry::for_blob_pack(&*kind.pack()).unwrap_or_else(|e| panic!("the {kind:?} pack's registry: {e:?}"))
    })
}

/// Whether `rel`'s pack keeps a structure index now.
pub fn indexes(rel: &str) -> bool {
    kind_of(rel) != BlobKind::Any
}

/// The pack's name in the export worker's `index` op (`psd`, `otio`,
/// `photoplot`, `drill-common`, `blend`...): its index spec's name, which
/// the worker's `INDEXERS` list.
pub fn index_pack_name(kind: BlobKind) -> Option<&'static str> {
    match kind {
        BlobKind::Any => None,
        BlobKind::Psd => Some(k2_mpc_psd::pack::PACK),
        BlobKind::Otio => Some("otio"),
        BlobKind::Fcpxml => Some("fcpxml"),
        BlobKind::Prproj => Some("prproj"),
        BlobKind::Photoplot => Some("photoplot"),
        BlobKind::Drill => Some("drill-common"),
        BlobKind::Blend => Some(k2_mpc_blend::pack::PACK),
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

    /// Timeline, fab and BLEND files by name, any case, with a name before
    /// the dot; near misses are any other file.
    #[test]
    fn timeline_fab_and_blend_files_are_known_by_name() {
        for (p, k) in [
            ("cut.otio", BlobKind::Otio),
            ("a/b/Cut.OTIO", BlobKind::Otio),
            ("edit/Info.fcpxml", BlobKind::Fcpxml),
            ("Show.PrProj", BlobKind::Prproj),
            ("board.gbr", BlobKind::Photoplot),
            ("board.GTL", BlobKind::Photoplot),
            ("out/board-F_Cu.ger", BlobKind::Photoplot),
            ("board.pho", BlobKind::Photoplot),
            ("board.gko", BlobKind::Photoplot),
            ("board.drl", BlobKind::Drill),
            ("board.XNC", BlobKind::Drill),
            ("board.xln", BlobKind::Drill),
            ("board.ncd", BlobKind::Drill),
            ("scene.blend", BlobKind::Blend),
            ("x/Room.BLEND", BlobKind::Blend),
        ] {
            assert_eq!(kind_by_name(p), k, "{p}");
            assert!(is_pack_file(p), "{p}");
        }
        for p in [".otio", "otio", "cut.otio.zip", "scene.blend1", "a.gbrx", "dir.drl/a.png", "a.fcpxmld", "a.xml"] {
            assert_eq!(kind_by_name(p), BlobKind::Any, "{p}");
            assert!(!is_pack_file(p), "{p}");
        }
        assert_eq!(core_by_name("cut.otio"), Some("timeline"));
        assert_eq!(core_by_name("a.prproj"), Some("timeline"));
        assert_eq!(core_by_name("a.drl"), Some("fab"));
        assert_eq!(core_by_name("a.gbr"), Some("fab"));
        assert_eq!(core_by_name("a.blend"), Some("blend"));
        assert_eq!(core_by_name("a.psd"), None, "PSD is the any-file core's");
        assert_eq!(core_by_name("a.png"), None);
    }

    /// Off (the default), a pack file is any other file: no index, the
    /// any-file registry. On, its pack and its index.
    #[test]
    fn a_pack_files_kind_follows_its_core() {
        let _l = crate::docs::cores::tests::mirror_lock();
        crate::docs::cores::tests::all_off();
        let files = [("cut.otio", "timeline", BlobKind::Otio), ("a.fcpxml", "timeline", BlobKind::Fcpxml), ("a.prproj", "timeline", BlobKind::Prproj), ("a.gbr", "fab", BlobKind::Photoplot), ("a.drl", "fab", BlobKind::Drill), ("a.blend", "blend", BlobKind::Blend)];
        for (p, _, _) in files {
            assert_eq!(kind_of(p), BlobKind::Any, "{p}: its core is off");
            assert!(!indexes(p), "{p}");
            assert!(pack_for(p).index_spec().is_none(), "{p}");
            assert!(std::ptr::eq(registry_for(p), registry_of(BlobKind::Any)), "{p}");
        }
        for (p, core, k) in files {
            crate::docs::cores::set_for_test(core, true);
            assert_eq!(kind_of(p), k, "{p}: {core} on");
            assert!(indexes(p), "{p}");
            let spec = pack_for(p).index_spec().unwrap_or_else(|| panic!("{p}: its pack indexes"));
            assert_eq!(Some(spec.pack), index_pack_name(k), "{p}: the worker's name is the index's");
            crate::docs::cores::set_for_test(core, false);
        }
        crate::docs::cores::tests::all_off();
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

    /// Each new pack's index verbs, from the kit's registry for it, every one
    /// through `run_tool`; none of them is an any-file verb.
    #[test]
    fn the_timeline_fab_and_blend_registries_have_their_index_verbs() {
        let any = registry_of(BlobKind::Any);
        for (kind, word) in [
            (BlobKind::Otio, "clip"),
            (BlobKind::Fcpxml, "clip"),
            (BlobKind::Prproj, "clip"),
            (BlobKind::Photoplot, "feature"),
            (BlobKind::Drill, "hole"),
            (BlobKind::Blend, "object"),
        ] {
            let r = registry_of(kind);
            for v in ["list", "show", "diff", "blame"] {
                let name = format!("{word} {v}");
                let spec = r.get(&name).unwrap_or_else(|| panic!("{name} missing from the {kind:?} registry"));
                assert!(spec.runs_through_run_tool(), "{name} ({kind:?}) runs through run_tool");
                assert!(any.get(&name).is_none(), "{name} is not an any-file verb");
            }
            for v in ["outline", "node", "comment", "comments"] {
                assert!(r.get(v).is_some(), "{v} missing from the {kind:?} registry");
            }
        }
    }

    /// The worker's names, one per kind with an index, all distinct.
    #[test]
    fn every_indexing_kind_has_a_distinct_worker_name() {
        let names: Vec<&str> = BlobKind::ALL.iter().filter_map(|k| index_pack_name(*k)).collect();
        assert_eq!(names, ["psd", "otio", "fcpxml", "prproj", "photoplot", "drill-common", "blend"]);
        for k in BlobKind::ALL {
            assert_eq!(index_pack_name(k).is_some(), k.pack().index_spec().is_some(), "{k:?}");
            if let (Some(n), Some(spec)) = (index_pack_name(k), k.pack().index_spec()) {
                assert_eq!(n, spec.pack, "{k:?}");
            }
        }
    }
}
