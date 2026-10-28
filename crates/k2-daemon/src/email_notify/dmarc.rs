//! E10: Email Notifications' own DMARC check (Q5: required before a
//! hosted sender is ready, **any** policy). The hosted-mail record table
//! grades only `p=none`, so a `p=reject` domain like lztek would never
//! pass it; this check doesn't use it.
//!
//! A domain passes when a parseable `v=DMARC1` record with a `p=` of
//! `none`, `quarantine` or `reject` sits at `_dmarc.<domain>` or at the
//! organizational domain's `_dmarc` (a sending subdomain inherits its
//! parent's policy, RFC 7489 §6.6.3). K2 plants `v=DMARC1; p=none` only on
//! a domain with no record anywhere, only in a zone K2 hosts, and never
//! touches an existing policy (N0: Stalwart's own suggestion for a new
//! subdomain is `p=reject`; K2 doesn't copy it).

use std::collections::HashMap;

use crate::mail::dns_verify::{join_chunks, DnsError, DnsResolver};

/// A found policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where it was found (`_dmarc.<name>`).
    pub at: String,
    /// `none` | `quarantine` | `reject`.
    pub policy: String,
    /// N7 (Rosson 2026-10-28): both `adkim=s` and `aspf=s`. A send-only
    /// apex sender can't align then (DKIM and SPF sit on the bounce
    /// subdomain), so `sender add --via` refuses it (`dmarc_strict`).
    pub strict: bool,
}

/// The result of a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dmarc {
    Found(Found),
    Missing,
}

/// Parse one TXT value: `Some(policy)` for a valid DMARC record.
pub fn parse(txt: &str) -> Option<String> {
    let mut tags = txt.split(';').map(str::trim).filter(|t| !t.is_empty());
    let first = tags.next()?;
    let (k, v) = first.split_once('=')?;
    if !k.trim().eq_ignore_ascii_case("v") || !v.trim().eq_ignore_ascii_case("DMARC1") {
        return None;
    }
    for t in tags {
        if let Some((k, v)) = t.split_once('=') {
            if k.trim().eq_ignore_ascii_case("p") {
                let p = v.trim().to_ascii_lowercase();
                return matches!(p.as_str(), "none" | "quarantine" | "reject").then_some(p);
            }
        }
    }
    None
}

/// Does a valid DMARC record set strict alignment for BOTH DKIM and SPF
/// (`adkim=s` and `aspf=s`; each defaults to relaxed)?
pub fn strict_both(txt: &str) -> bool {
    if parse(txt).is_none() {
        return false;
    }
    let tag = |name: &str| {
        txt.split(';').filter_map(|t| t.split_once('=')).find(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_ascii_lowercase())
    };
    tag("adkim").as_deref() == Some("s") && tag("aspf").as_deref() == Some("s")
}

/// Public suffixes with two labels K2 knows (enough to avoid treating
/// `co.uk` as an organization).
const TWO_LABEL_SUFFIXES: &[&str] = &["co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp", "com.br", "com.mx", "co.za", "co.in", "com.sg"];

/// The organizational domain of `domain` (the registrable part).
pub fn org_domain(domain: &str) -> String {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = d.split('.').collect();
    if labels.len() <= 2 {
        return d;
    }
    let last2 = labels[labels.len() - 2..].join(".");
    let keep = if TWO_LABEL_SUFFIXES.contains(&last2.as_str()) { 3 } else { 2 };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

fn lookup_one(r: &dyn DnsResolver, name: &str) -> Result<Option<(String, bool)>, String> {
    match r.txt(&format!("_dmarc.{name}")) {
        Ok(records) => Ok(records.iter().map(|c| join_chunks(c)).find_map(|t| parse(&t).map(|p| (p, strict_both(&t))))),
        Err(DnsError::NotFound) => Ok(None),
        Err(DnsError::Other(e)) => Err(e),
    }
}

/// Look up `domain`'s DMARC (its own, else its organizational domain's).
pub fn lookup(r: &dyn DnsResolver, domain: &str) -> Result<Dmarc, String> {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Some((p, strict)) = lookup_one(r, &d)? {
        return Ok(Dmarc::Found(Found { at: format!("_dmarc.{d}"), policy: p, strict }));
    }
    let org = org_domain(&d);
    if org != d {
        if let Some((p, strict)) = lookup_one(r, &org)? {
            return Ok(Dmarc::Found(Found { at: format!("_dmarc.{org}"), policy: p, strict }));
        }
    }
    Ok(Dmarc::Missing)
}

type Cache = HashMap<String, (Dmarc, i64)>;

/// The answer cache: process-wide in the daemon, per thread in tests (each
/// test installs its own fake resolver).
fn with_cache<T>(f: impl FnOnce(&mut Cache) -> T) -> T {
    #[cfg(test)]
    {
        thread_local! {
            static C: std::cell::RefCell<Cache> = std::cell::RefCell::new(HashMap::new());
        }
        C.with(|c| f(&mut c.borrow_mut()))
    }
    #[cfg(not(test))]
    {
        static C: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
        let m = C.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }
}

/// A found record is trusted this long; a miss is re-asked sooner.
const FOUND_TTL: i64 = 600;
const MISSING_TTL: i64 = 60;

/// The send-time check (§5.2 step 4), cached briefly. A resolver error
/// refuses (fail-closed: `dmarc_missing` with the error in words).
pub fn check(domain: &str, now: i64) -> super::R<Found> {
    let key = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let hit = with_cache(|c| c.get(&key).cloned());
    if let Some((d, at)) = hit {
        let ttl = if matches!(d, Dmarc::Found(_)) { FOUND_TTL } else { MISSING_TTL };
        if now - at < ttl {
            return found_or_missing(d, &key);
        }
    }
    let r = super::engine::dns()?;
    let d = lookup(r.as_ref(), &key).map_err(|e| super::fail(409, "dmarc_missing", format!("K2 couldn't look up _dmarc for {key} ({e}); try again shortly")))?;
    with_cache(|c| c.insert(key.clone(), (d.clone(), now)));
    found_or_missing(d, &key)
}

fn found_or_missing(d: Dmarc, domain: &str) -> super::R<Found> {
    match d {
        Dmarc::Found(f) => Ok(f),
        Dmarc::Missing => Err(super::fail(
            409,
            "dmarc_missing",
            format!(
                "{domain} has no DMARC record (nor its parent domain). Publish one before sending, for example \
                 _dmarc.{domain} TXT \"v=DMARC1; p=none\" (any policy works); k2 notifications email sender list shows the state"
            ),
        )),
    }
}

/// Forget a cached answer (after K2 planted a record, or a re-check).
pub fn forget(domain: &str) {
    let key = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    with_cache(|c| c.remove(&key));
}

/// The record K2 plants on a domain with none (E10).
pub fn planted_value(rua: Option<&str>) -> String {
    match rua {
        Some(a) => format!("v=DMARC1; p=none; rua=mailto:{a}"),
        None => "v=DMARC1; p=none".to_string(),
    }
}

/// Plant `_dmarc.<domain>` = [`planted_value`] when the domain's zone is
/// on K2 and writable. `Ok(false)` = nothing planted (not K2-hosted, read
/// only). Callers check first that no record exists anywhere.
pub fn plant(domain: &str, rua: Option<&str>) -> Result<bool, String> {
    let (zone_id, sub, writable) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::domains::binding_for_name(&conn, domain).map_err(|e| e.to_string())? {
            Some((b, sub)) => {
                let w = k2_core::domains::apex_attached_for_write(&conn, &b.apex).unwrap_or(false);
                (b.zone_id.clone(), sub, w)
            }
            None => return Ok(false),
        }
    };
    let Some(zone_id) = zone_id.filter(|z| !z.is_empty() && writable) else { return Ok(false) };
    let name = if sub.is_empty() { "_dmarc".to_string() } else { format!("_dmarc.{sub}") };
    let body = serde_json::json!({ "type": "TXT", "name": name, "content": planted_value(rua), "ttl": 3600 }).to_string();
    let resp = crate::dns::proxy::proxy_request("POST", &format!("/api/dns/zones/{zone_id}/records"), Some("k2-email-notify"), Some(&body))?;
    if resp.status != 200 && resp.status != 201 {
        return Err(format!("planting the DMARC record failed (HTTP {})", resp.status));
    }
    forget(domain);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_policy_passes_and_only_dmarc1_parses() {
        assert_eq!(parse("v=DMARC1; p=reject; rua=mailto:x@y.test").as_deref(), Some("reject"));
        assert_eq!(parse("v=DMARC1;p=quarantine").as_deref(), Some("quarantine"));
        assert_eq!(parse(" V=dmarc1 ; P=None ").as_deref(), Some("none"));
        assert_eq!(parse("v=DMARC1; rua=mailto:x@y.test"), None, "no policy");
        assert_eq!(parse("p=reject; v=DMARC1"), None, "v must come first");
        assert_eq!(parse("v=spf1 mx ~all"), None);
        assert_eq!(parse("v=DMARC1; p=maybe"), None);
    }

    #[test]
    fn strict_alignment_needs_both_tags() {
        assert!(strict_both("v=DMARC1; p=reject; adkim=s; aspf=s"));
        assert!(strict_both("v=DMARC1;p=none;ASPF=S;ADKIM=s"));
        assert!(!strict_both("v=DMARC1; p=reject; adkim=s"), "aspf defaults to relaxed");
        assert!(!strict_both("v=DMARC1; p=reject; adkim=r; aspf=s"));
        assert!(!strict_both("v=DMARC1; p=reject"));
        assert!(!strict_both("adkim=s; aspf=s"), "not a DMARC record");
    }

    #[test]
    fn org_domains() {
        assert_eq!(org_domain("notify.brand.com"), "brand.com");
        assert_eq!(org_domain("a.b.brand.com."), "brand.com");
        assert_eq!(org_domain("brand.com"), "brand.com");
        assert_eq!(org_domain("notify.brand.co.uk"), "brand.co.uk");
    }
}
