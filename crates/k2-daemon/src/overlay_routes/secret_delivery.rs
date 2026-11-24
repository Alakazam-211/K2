//! How a secret card's value reaches the agent when the agent asks
//! (`k2 thread secret`): the delivery the card proposes, from what the agent
//! typed (`--delivery`, `--for`, `--inject-*`).
//!
//! - `--inject-*` says how K2 sends the secret in its own calls, so it is
//!   fetch (or proxy) only; with no `--delivery` it means fetch (SC-A5).
//! - Not every secret is for a site (a computer's admin password, an ssh
//!   passphrase). With no site and no delivery the agent reads it as a file
//!   (`k2 vault file NAME`); `--for` with no delivery stays fetch.
//! - Fetch or proxy with no site would put up a card the person can't save,
//!   so it is refused when the agent asks, while the agent can still fix it.
use k2_core::{cli_name, product_name};

/// The refusal for a card K2 would send to a site it was never told.
pub(super) const CARD_NEEDS_SITE: &str = concat!(product_name!(), " sends this secret to a site (fetch), and the card names none: add --for HOST (like api.example.com), or use --delivery file (or leave --delivery and --inject-* out) to read it yourself with ", cli_name!(), " vault file NAME");

/// The card's delivery: `delivery` is the agent's `--delivery` (checked to
/// be a known word already), `has_inject` whether it named a sending, and
/// `hosts` its `--for` sites, normalised.
pub(super) fn card_delivery(delivery: Option<String>, has_inject: bool, hosts: &[String]) -> Result<Option<String>, k2_vault::VaultError> {
    let delivery = match (has_inject, delivery) {
        (true, None) => Some("fetch".to_string()),
        (true, Some(d)) if d != "fetch" && d != "proxy" => {
            return Err(k2_vault::VaultError::bad_request(
                "inject_needs_fetch",
                concat!("--inject-* says how ", product_name!(), " sends the secret in its own calls, so it needs --delivery fetch (the default with --for)"),
            ))
        }
        (_, d) => d,
    };
    match delivery {
        None if hosts.is_empty() => Ok(Some("file".to_string())),
        Some(d) if (d == "fetch" || d == "proxy") && hosts.is_empty() => Err(k2_vault::VaultError::bad_request("card_needs_site", CARD_NEEDS_SITE)),
        d => Ok(d),
    }
}
