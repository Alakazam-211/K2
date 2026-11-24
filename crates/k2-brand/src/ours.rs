//! Which brand is ours: the one rule, shared.
//!
//! A brand is ours when its id is one of [`OUR_IDS`] and its folder is
//! this repo's own `brand/<id>` (the two paths compared once made
//! canonical). A copy of `brand/k2` in another folder is not ours, and
//! neither is any other id. A brand that isn't ours sets its own values
//! for the fields that name us or reach us (`brand_file.rs`).
//!
//! No dependencies: crates/k2-brand's build script and tests use this
//! file through `brand_file.rs`. scripts/brand/lint.py (`is_ours`) keeps
//! the same rule.

use std::path::Path;

/// The ids of our own brands.
pub const OUR_IDS: &[&str] = &["k2", "k5"];

/// Whether the brand `id`, read from the folder `folder`, is ours: `id` is
/// one of [`OUR_IDS`] and `folder` is `<root>/brand/<id>`, where `root` is
/// the repo's root. A path that can't be made canonical is not ours.
pub fn is_ours(id: &str, folder: &Path, root: &Path) -> bool {
    if !OUR_IDS.contains(&id) {
        return false;
    }
    match (
        folder.canonicalize(),
        root.join("brand").join(id).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
