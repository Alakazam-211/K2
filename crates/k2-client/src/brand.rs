//! The product's names, one definition each, ahead of a rename.
//!
//! The command's name, its aliases, the product's name and the brand's id
//! come from the brand file, `brand/<id>/brand.toml` (`brand/k2` unless
//! `K2_BRAND_DIR` names another folder), through the `k2-brand` crate,
//! and are re-exported here. The rest is still defined here.
//!
//! What people and agents read as the product's name, the command's name,
//! the public website, the support address and the repo releases come
//! from. Code that prints one of these, or builds a URL from one, uses
//! this module, so a later rename is a change here and not a search across
//! the tree. `k2_core::brand` re-exports it; the renderer's copy is
//! `src/renderer/lib/brand.ts` (its test reads this file and the default
//! brand file, and fails when they drift). `scripts/brand-gate.sh` keeps
//! new hard-coded product names out of the rest of the Rust and renderer
//! code.
//!
//! These are names for words and links only. Names other programs hold
//! (bundle ids, launchd labels, unit names, token prefixes, headers,
//! cookies, crypto tags, asset names an installed copy checks) are not
//! here and do not follow these: they have their own homes and rules.
//!
//! The text forms are macros as well as consts, so a `const` sentence can
//! be built with `concat!` (`concat!("run ", cli_name!(), " app list")`)
//! and still be one `&'static str`. Each const is defined from its macro.

// From the brand file: the command's name and the product's name as
// literals for `concat!` (`cli_name!()`, `product_name!()`, at this
// crate's root), as consts, the command's aliases with their modes, and the
// brand's id.
use k2_brand::product_name;
pub use k2_brand::{AliasMode, BRAND_ID, CLI_ALIASES, CLI_ALIAS_MODES, CLI_NAME, PRODUCT_NAME};

/// The repo releases come from (`owner/name`), as a literal.
#[macro_export]
macro_rules! release_repo {
    () => {
        "Alakazam-211/K2"
    };
}

/// The company that makes it.
pub const COMPANY_NAME: &str = "Alakazam Labs";

/// The product with its maker: "K2 by Alakazam Labs".
pub const PRODUCT_LOCKUP: &str = concat!(product_name!(), " by Alakazam Labs");

/// The public website.
pub const PUBLIC_SITE: &str = "https://k2.dev";

/// The zone boxes are named under (`<label>.k2.dev`): the hosts module's
/// primary zone, under the name this module gives it.
pub const INFRA_ZONE: &str = crate::hosts::PRIMARY_ZONE;

/// The zones this product serves boxes under today, primary first. A
/// client asks the daemon for this list (`/boot-status` `hostedZones`)
/// instead of carrying its own, so a zone that is not public yet is in no
/// client binary. Narrower than `hosts::ZONES`, which also holds zones
/// kept as ours for the checks that refuse our own names, before anything
/// is served under them.
pub const HOSTED_ZONES: &[&str] = &[crate::hosts::PRIMARY_ZONE];

/// Where people write for help.
pub const SUPPORT_EMAIL: &str = "support@k2.dev";

/// The repo releases come from, `owner/name`.
pub const RELEASE_REPO: &str = release_repo!();

/// The repo's page.
pub const RELEASE_REPO_URL: &str = concat!("https://github.com/", release_repo!());

/// Where a release's files are downloaded from: `<this>/v<version>/<asset>`.
pub const RELEASE_DOWNLOAD_BASE: &str = concat!("https://github.com/", release_repo!(), "/releases/download");

/// The prefix every release page and release file URL starts with.
pub const RELEASES_URL_PREFIX: &str = concat!("https://github.com/", release_repo!(), "/releases/");

/// GitHub's API answer for the newest release.
pub const RELEASES_LATEST_API: &str = concat!("https://api.github.com/repos/", release_repo!(), "/releases/latest");

/// The desktop app's update feed (also `plugins.updater.endpoints` in
/// `src-tauri/tauri.conf.json`).
pub const APP_FEED_URL: &str = concat!("https://github.com/", release_repo!(), "/releases/latest/download/latest.json");

/// The headless daemon's update feed.
pub const DAEMON_FEED_URL: &str =
    concat!("https://github.com/", release_repo!(), "/releases/latest/download/daemon-latest.json");

/// The page of one release: `…/releases/tag/v<version>`. A leading `v` on
/// `version` is not doubled; nothing else is trimmed.
pub fn release_tag_url(version: &str) -> String {
    format!("{RELEASES_URL_PREFIX}tag/v{}", version.strip_prefix('v').unwrap_or(version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_name;

    /// The values, spelled out: a change to one is a rename people see.
    #[test]
    fn brand_names_are_what_they_were() {
        assert_eq!(PRODUCT_NAME, "K2");
        assert_eq!(COMPANY_NAME, "Alakazam Labs");
        assert_eq!(PRODUCT_LOCKUP, "K2 by Alakazam Labs");
        assert_eq!(CLI_NAME, "k2");
        assert_eq!(CLI_ALIASES, &["k2so"]);
        assert_eq!(PUBLIC_SITE, "https://k2.dev");
        assert_eq!(INFRA_ZONE, "k2.dev");
        assert_eq!(SUPPORT_EMAIL, "support@k2.dev");
        assert_eq!(RELEASE_REPO, "Alakazam-211/K2");
    }

    #[test]
    fn brand_macros_and_consts_agree() {
        assert_eq!(product_name!(), PRODUCT_NAME);
        assert_eq!(cli_name!(), CLI_NAME);
        assert_eq!(release_repo!(), RELEASE_REPO);
        assert_eq!(PRODUCT_LOCKUP, format!("{PRODUCT_NAME} by {COMPANY_NAME}"));
    }

    /// Boxes are served under k2.dev alone; k5.dev is a zone of ours for
    /// refusals only, and is not hosted.
    #[test]
    fn brand_hosted_zones_are_the_primary_zone_alone() {
        assert_eq!(HOSTED_ZONES, &["k2.dev"]);
        assert_eq!(HOSTED_ZONES[0], crate::hosts::PRIMARY_ZONE);
        assert!(HOSTED_ZONES.iter().all(|z| crate::hosts::ZONES.contains(z)), "every hosted zone is a zone of ours");
        assert!(!HOSTED_ZONES.contains(&"k5.dev"));
    }

    /// Every release URL, spelled out: installed copies fetch these.
    #[test]
    fn brand_release_urls_are_what_they_were() {
        assert_eq!(RELEASE_REPO_URL, "https://github.com/Alakazam-211/K2");
        assert_eq!(RELEASE_DOWNLOAD_BASE, "https://github.com/Alakazam-211/K2/releases/download");
        assert_eq!(RELEASES_URL_PREFIX, "https://github.com/Alakazam-211/K2/releases/");
        assert_eq!(RELEASES_LATEST_API, "https://api.github.com/repos/Alakazam-211/K2/releases/latest");
        assert_eq!(APP_FEED_URL, "https://github.com/Alakazam-211/K2/releases/latest/download/latest.json");
        assert_eq!(DAEMON_FEED_URL, "https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json");
        assert_eq!(release_tag_url("0.46.14"), "https://github.com/Alakazam-211/K2/releases/tag/v0.46.14");
        assert_eq!(release_tag_url("v0.46.14"), "https://github.com/Alakazam-211/K2/releases/tag/v0.46.14");
    }

    /// The desktop app's feed is the same URL its Tauri config names.
    #[test]
    fn brand_app_feed_matches_the_tauri_config() {
        let conf = include_str!("../../../src-tauri/tauri.conf.json");
        assert!(conf.contains(&format!("\"{APP_FEED_URL}\"")), "tauri.conf.json names another feed");
    }
}
