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

/// The repo releases come from (`owner/name`), as a literal: every
/// release URL is built from this. The repo is moving from Alakazam-211/K2
/// to AlakazamLabs/K5 (a transfer and rename, which GitHub redirects). This
/// stays the old name until the move; on move day it becomes the repo's new
/// name, and this line is the whole change on the Rust side. Code that
/// compares a repo name accepts every name in [`RELEASE_REPO_NAMES`], so a
/// build made before the move accepts the new name and one made after
/// still accepts the old.
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

/// Every name the release repo answers to, `owner/name`: its name before
/// the move and its name after. A check that compares a name (is this our
/// repo, our patched wry) accepts exactly these, never any other; code
/// that builds a URL uses [`RELEASE_REPO`], which is one of them.
pub const RELEASE_REPO_NAMES: &[&str] = &["Alakazam-211/K2", "AlakazamLabs/K5"];

/// `url` is the release repo's git URL as Cargo writes it, under one of
/// [`RELEASE_REPO_NAMES`]: `https://github.com/<owner>/<name>`, optionally
/// with `git+` before it, `.git` after the name, and a `?query` or
/// `#fragment` (`git+https://github.com/Alakazam-211/K2?rev=abc#abc`).
/// The `owner/name` must be the whole path, compared exactly, case and all,
/// as the release trust checks compare it (k2sign's attestation identity,
/// the renderer's `isReleaseUrl`): not `evil/Alakazam-211/K2`, not
/// `Alakazam-211/K2/tree/x`, not `alakazam-211/k2`, not another host.
pub fn is_release_repo_url(url: &str) -> bool {
    let url = url.strip_prefix("git+").unwrap_or(url);
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    let path = rest.find(['?', '#']).map_or(rest, |i| &rest[..i]);
    let path = path.strip_suffix(".git").unwrap_or(path);
    RELEASE_REPO_NAMES.contains(&path)
}

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

/// Where a daemon's text template puts the command's name: a hint sent as
/// a template beside the hint (`hintTemplate`, white-label C5) holds this,
/// so a CLI of another brand can put its own name there.
pub const CLI_SLOT: &str = "{cli}";

/// Where a daemon's text template puts the product's name.
pub const PRODUCT_SLOT: &str = "{product}";

/// A text template with `cli` and `product` put in for [`CLI_SLOT`] and
/// [`PRODUCT_SLOT`]: the command's name first, then the product's (the
/// shell CLI's `k2_hint_fill` does the same, in that order).
pub fn fill_names_with(template: &str, cli: &str, product: &str) -> String {
    template.replace(CLI_SLOT, cli).replace(PRODUCT_SLOT, product)
}

/// [`fill_names_with`] with this build's names.
pub fn fill_names(template: &str) -> String {
    fill_names_with(template, CLI_NAME, PRODUCT_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_name;

    /// A template takes any brand's names, the command's first; a text with
    /// no slot is as it was.
    #[test]
    fn a_template_takes_the_names_it_is_given() {
        let t = "run {cli} compute allow-nodes --add n1 from a terminal outside {product} ({cli} help)";
        assert_eq!(
            fill_names_with(t, "zz", "Globex Desk"),
            "run zz compute allow-nodes --add n1 from a terminal outside Globex Desk (zz help)"
        );
        assert_eq!(fill_names(t), format!("run {CLI_NAME} compute allow-nodes --add n1 from a terminal outside {PRODUCT_NAME} ({CLI_NAME} help)"));
        assert_eq!(fill_names("no names {here}"), "no names {here}");
    }

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
        assert_eq!(RELEASE_REPO_NAMES, &["Alakazam-211/K2", "AlakazamLabs/K5"]);
        assert!(RELEASE_REPO_NAMES.contains(&RELEASE_REPO), "the repo releases come from is one of its names");
    }

    /// Both of the repo's names count as the repo, as Cargo writes its git
    /// URL; a third name, a longer name, another owner with the same repo
    /// name, a path before or after the name, another case and another
    /// host do not.
    #[test]
    fn brand_release_repo_url_is_exactly_one_of_its_names() {
        for yes in [
            "https://github.com/Alakazam-211/K2",
            "https://github.com/AlakazamLabs/K5",
            "https://github.com/Alakazam-211/K2.git",
            "git+https://github.com/Alakazam-211/K2?rev=abc#abc",
            "git+https://github.com/AlakazamLabs/K5.git?rev=abc#abc",
            "git+https://github.com/AlakazamLabs/K5#abc",
        ] {
            assert!(is_release_repo_url(yes), "{yes}");
        }
        for no in [
            // A path before or after the name.
            "https://github.com/evil/AlakazamLabs/K5",
            "git+https://github.com/evil/Alakazam-211/K2?rev=abc#abc",
            "https://github.com/AlakazamLabs/K5/tree/main",
            "https://github.com/AlakazamLabs/K5/",
            "https://github.com/AlakazamLabs/K5.git/x",
            // Another name, or a longer one.
            "https://github.com/Alakazam-211/K2SO",
            "https://github.com/AlakazamLabs/K2",
            "https://github.com/Alakazam-211/K5",
            "https://github.com/AlakazamLabs/K5-fork",
            "https://github.com/AlakazamLabs/K5.git.git",
            // Another owner with the same repo name, or a name inside a word.
            "https://github.com/someone/K2",
            "https://github.com/xAlakazam-211/K2",
            // Case is compared exactly.
            "https://github.com/alakazamlabs/k5",
            "https://github.com/ALAKAZAM-211/K2",
            "https://GitHub.com/AlakazamLabs/K5",
            // Another host or scheme, or no URL.
            "https://evil.example/AlakazamLabs/K5",
            "https://github.com.evil.example/AlakazamLabs/K5",
            "https://github.com@evil.example/AlakazamLabs/K5",
            "https://evil.example/https://github.com/AlakazamLabs/K5",
            "http://github.com/AlakazamLabs/K5",
            "ssh://git@github.com/AlakazamLabs/K5",
            "git+git+https://github.com/AlakazamLabs/K5",
            "AlakazamLabs/K5",
            "",
        ] {
            assert!(!is_release_repo_url(no), "{no}");
        }
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

    /// Every release URL's shape, spelled out around the repo's name:
    /// installed copies fetch these.
    #[test]
    fn brand_release_urls_are_what_they_were() {
        let r = RELEASE_REPO;
        assert_eq!(RELEASE_REPO_URL, format!("https://github.com/{r}"));
        assert_eq!(RELEASE_DOWNLOAD_BASE, format!("https://github.com/{r}/releases/download"));
        assert_eq!(RELEASES_URL_PREFIX, format!("https://github.com/{r}/releases/"));
        assert_eq!(RELEASES_LATEST_API, format!("https://api.github.com/repos/{r}/releases/latest"));
        assert_eq!(APP_FEED_URL, format!("https://github.com/{r}/releases/latest/download/latest.json"));
        assert_eq!(DAEMON_FEED_URL, format!("https://github.com/{r}/releases/latest/download/daemon-latest.json"));
        assert_eq!(release_tag_url("0.46.14"), format!("https://github.com/{r}/releases/tag/v0.46.14"));
        assert_eq!(release_tag_url("v0.46.14"), format!("https://github.com/{r}/releases/tag/v0.46.14"));
    }

    /// The desktop app's feed is the same URL its Tauri config names.
    #[test]
    fn brand_app_feed_matches_the_tauri_config() {
        let conf = include_str!("../../../src-tauri/tauri.conf.json");
        assert!(conf.contains(&format!("\"{APP_FEED_URL}\"")), "tauri.conf.json names another feed");
    }
}
