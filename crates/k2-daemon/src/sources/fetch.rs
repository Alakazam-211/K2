//! The source fetcher (contract §3.4, §10): `FetchPlan` →
//! `vault_fetch::execute` with the source's own allowlist, errors mapped to
//! fixed `Reason`s. Owner: B1. Try (B2) calls [`fetch_once`] too.

use k2_core::sources::{FetchOutcome, FetchPlan, Fetcher, Reason};

/// The daemon's [`Fetcher`].
pub struct HttpFetcher;

impl Fetcher for HttpFetcher {
    fn fetch(&self, plan: &FetchPlan) -> FetchOutcome {
        fetch_once(plan)
    }
}

/// Run one request.
///
/// C0 skeleton: makes no connection and answers `connect` (B1 replaces it).
pub fn fetch_once(plan: &FetchPlan) -> FetchOutcome {
    let _ = plan;
    FetchOutcome::Err { reason: Reason::Connect, http_status: None, retry_after_secs: None }
}
