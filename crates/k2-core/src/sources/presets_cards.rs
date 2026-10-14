//! Card presets (contract §11.4): `k2-fleet`, `k2-fleet-summary`,
//! `github-latest-release`, `vercel-deployments`, `stripe-past-due`.
//! Owner: B3. `presets::find` (B1) searches here after the base presets.

use std::collections::BTreeMap;

use super::spec::{SourceSpec, SpecError};

pub const NAMES: &[&str] =
    &["k2-fleet", "k2-fleet-summary", "github-latest-release", "vercel-deployments", "stripe-past-due"];

/// The spec a card preset makes from its `presetArgs`; `None` when `name`
/// is not a card preset. `name`/`label` are filled by the caller.
///
/// C0 skeleton: no card presets yet (B3 replaces it).
pub fn find(name: &str, args: &BTreeMap<String, String>) -> Option<Result<SourceSpec, SpecError>> {
    let _ = (name, args);
    None
}
