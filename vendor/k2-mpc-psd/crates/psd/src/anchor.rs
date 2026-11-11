//! The pack's half of an anchor's state (the kit's `anchor` module and `docs/SESSIONS.md`,
//! "Anchors"): whether a layer that still holds an anchor's key is the same layer
//! ([`judge`]), and which layers to suggest when the key is gone ([`candidate`]).
//!
//! **Kinds come in families.** A layer can change between two kinds and stay the same layer:
//! a group that becomes an artboard (or back) keeps its children, and a fill that gains or
//! loses its vector data becomes a shape (or a fill) and keeps its fill. Those pairs are one
//! family. Any other kind change is a kind change: a raster layer turned into a smart object, a
//! text layer rasterized, a layer replaced by an adjustment.
//!
//! **Judging a layer that holds the key.** Another family: `KindChanged`. Within the family, the
//! kit's identity rule: in doubt only when the layer was renamed **and** moved further than its
//! old diagonal **and** its content changed (the anchored version's keyed content hash against
//! the layer's now; unknown counts as changed). A plain rename, a plain move, a reshape or new
//! pixels are the same layer. The hint (a point in the layer's own box) is never judged off:
//! that needs content bounds, which the index doesn't hold yet.
//!
//! **Weak IDs.** A smart object's `placed_id` can be shared by several layers (copies of one
//! placed item), so it never says that a layer is the anchored one: the judgment ignores it, and
//! it only ever makes a candidate. A candidate is a suggestion the kit shows; it never moves an
//! anchor.

use k2_mpc_base::anchor::{judge_by_default, AnchorJudgment, CandidateWhy};
use k2_mpc_base::index::{Fingerprint, IndexNode};

/// The kind families: a change within one keeps the layer.
const FAMILIES: &[&[&str]] = &[&["group", "artboard"], &["fill", "shape"]];

/// Whether a layer of kind `a` can become kind `b` and stay the same layer.
pub fn same_family(a: &str, b: &str) -> bool {
    a == b || FAMILIES.iter().any(|f| f.contains(&a) && f.contains(&b))
}

/// `now` holds the anchor's key: is it the layer `was` describes? See the module docs.
pub fn judge(was: &Fingerprint, hint: Option<&str>, now: &IndexNode) -> AnchorJudgment {
    let _ = hint;
    if !same_family(&was.kind, &now.kind) {
        return AnchorJudgment::KindChanged;
    }
    // the kit's identity rule, with the kind taken as the same (a family change is not one)
    let mut within = was.clone();
    within.kind.clone_from(&now.kind);
    judge_by_default(&within, now)
}

/// The anchor's key is gone: is `node` a layer a person may re-anchor to? A layer of the same
/// family that shares a weak ID (`placed_id`) with the anchored one, or has its (non-empty)
/// name. Never more than a suggestion.
pub fn candidate(was: &Fingerprint, hint: Option<&str>, node: &IndexNode) -> Option<CandidateWhy> {
    let _ = hint;
    if !same_family(&was.kind, &node.kind) {
        return None;
    }
    if was
        .weak
        .iter()
        .any(|(k, v)| node.fingerprint.weak.get(k) == Some(v))
    {
        return Some(CandidateWhy::FingerprintMatch);
    }
    if !was.name.is_empty() && node.name == was.name {
        return Some(CandidateWhy::SameKindAndName);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_symmetric_and_closed() {
        for (a, b) in [("group", "artboard"), ("fill", "shape")] {
            assert!(same_family(a, b) && same_family(b, a));
        }
        for k in [
            "raster",
            "text",
            "smart",
            "adjustment",
            "other",
            "group",
            "fill",
        ] {
            assert!(same_family(k, k));
        }
        for (a, b) in [
            ("raster", "smart"),
            ("text", "raster"),
            ("raster", "shape"),
            ("group", "raster"),
            ("artboard", "fill"),
            ("adjustment", "fill"),
        ] {
            assert!(!same_family(a, b), "{a} to {b}");
        }
    }
}
