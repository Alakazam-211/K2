//! DNS K1 — daemon-side control-plane proxy + local capability envelope.
//!
//! Agents never hold the tunnel token. Handlers resolve *who the caller is*
//! via Wave 0 [`crate::caller_workspace::resolve_caller_workspace`], gate on
//! [`k2_core::workspace::settings::dns_manage_allowed_for_path`], enforce the
//! local record-type / zone-lifecycle envelope, then proxy to the live web
//! API (`GET/POST/DELETE /api/dns/…` on k2-dev-web) with the daemon's
//! tunnel bearer (`k2c_…` from `~/.k2/tunnel.json`).
//!
//! Routes: [`routes`] via the thin [`crate::dns_routes`] shim.

pub mod proxy;
pub mod routes;

/// Record types agents (and the local envelope) may create/update.
/// NS is deliberately absent — zone apex NS is lifecycle-owned; sub-
/// delegation is human/dashboard-only. Mirrors k2-dev-web `AGENT_RECORD_TYPES`.
pub const AGENT_RECORD_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "MX", "SRV", "CAA"];

/// Teaching text when the DNS-manage toggle is off for the caller's workspace.
/// Canonical owner path (must match `cli/k2` `DNS_GATED_HINT` / `k2 dns --help`
/// exit-3 prose — GH#32). Product UI: app master under Settings → K2 Connect,
/// per-workspace under Workspaces → (workspace) → Allow DNS manage.
pub const DNS_DENIED_HINT: &str = "this agent isn't allowed to manage DNS — \
the owner can enable it in Settings → K2 Connect (Allow agents to manage DNS records) \
or Workspaces → (workspace) → Allow DNS manage";

/// Teaching text when a local envelope check rejects the request.
pub const ZONE_LIFECYCLE_HINT: &str =
    "agents cannot create or delete zones — zone lifecycle is owner-only";

/// Teaching text when a DNS write targets a zone not attached on this box.
pub const DNS_ZONE_NOT_ATTACHED_HINT: &str = "this DNS zone is not attached to this server — \
the owner can attach it in Settings → K2 Server → Domains (or `k2 domain add`)";

/// The owner name the k2.dev records API wants for `name` in zone `apex`.
///
/// `POST /api/dns/zones/{id}/records` treats every `name` as RELATIVE to
/// the zone (`@` = apex; k2-dev-web `validateRecord`). Posting an FQDN
/// such as `_imaps._tcp.example.com` lands the record at
/// `_imaps._tcp.example.com.example.com` (IT2). Rules, case-insensitive,
/// trailing dots ignored:
/// - `name == apex` → `@`
/// - `name == <x>.<apex>` → `<x>`
/// - anything else (not under the apex, or empty) → `Err`; the caller
///   must not post that row.
pub fn relative_record_name(name: &str, apex: &str) -> Result<String, String> {
    let apex_n = apex.trim().trim_end_matches('.').to_ascii_lowercase();
    if apex_n.is_empty() {
        return Err("relative record name: empty zone apex".to_string());
    }
    let name_n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if name_n.is_empty() {
        return Err(format!("relative record name: empty owner name for zone {apex_n}"));
    }
    if name_n == apex_n {
        return Ok("@".to_string());
    }
    match name_n.strip_suffix(&format!(".{apex_n}")) {
        Some(rel) if !rel.is_empty() && !rel.ends_with('.') => Ok(rel.to_string()),
        _ => Err(format!(
            "record name '{}' is not inside zone {apex_n} — refusing to post it",
            name.trim()
        )),
    }
}

/// Teaching text when NS (or any non-envelope type) is requested.
pub fn unsupported_type_hint(rtype: &str) -> String {
    format!(
        "record type '{rtype}' is not allowed for agents — allowed: {}",
        AGENT_RECORD_TYPES.join(", ")
    )
}

/// `true` iff `rtype` is in the agent envelope (case-insensitive).
pub fn record_type_allowed(rtype: &str) -> bool {
    let upper = rtype.trim().to_ascii_uppercase();
    AGENT_RECORD_TYPES.iter().any(|t| *t == upper)
}

/// Normalize a record type for the proxy body (uppercase). Returns `None`
/// when the type is outside the envelope (including empty).
pub fn normalize_record_type(rtype: &str) -> Option<String> {
    let upper = rtype.trim().to_ascii_uppercase();
    if upper.is_empty() || !record_type_allowed(&upper) {
        None
    } else {
        Some(upper)
    }
}

/// Local reject for `managed_by` rows agents must not touch.
/// Empty/`user` (or missing) is fine; anything else is frozen automation.
pub fn managed_by_touchable(managed_by: Option<&str>) -> bool {
    match managed_by.map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some("user") => true,
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_type_allowlist_rejects_ns() {
        assert!(!record_type_allowed("NS"));
        assert!(!record_type_allowed("ns"));
        assert!(normalize_record_type("NS").is_none());
        assert!(normalize_record_type("A").as_deref() == Some("A"));
        assert!(normalize_record_type("aaaa").as_deref() == Some("AAAA"));
        for t in AGENT_RECORD_TYPES {
            assert!(record_type_allowed(t), "{t}");
        }
        assert!(!record_type_allowed("SOA"));
        assert!(!record_type_allowed(""));
    }

    #[test]
    fn managed_by_only_user_is_touchable() {
        assert!(managed_by_touchable(None));
        assert!(managed_by_touchable(Some("user")));
        assert!(managed_by_touchable(Some("  user  ")));
        assert!(!managed_by_touchable(Some("k2-system")));
        assert!(!managed_by_touchable(Some("k2-mail")));
        assert!(!managed_by_touchable(Some("k2-publish")));
    }

    /// IT2: the records API is zone-relative.
    #[test]
    fn relative_record_name_strips_the_apex() {
        let ok = |n: &str, a: &str| relative_record_name(n, a).expect(n);
        assert_eq!(ok("example.com", "example.com"), "@");
        assert_eq!(ok("example.com.", "example.com"), "@");
        assert_eq!(ok("_imaps._tcp.example.com", "example.com"), "_imaps._tcp");
        assert_eq!(ok("_imaps._tcp.example.com.", "example.com."), "_imaps._tcp");
        assert_eq!(ok("AutoConfig.Example.COM", "example.com"), "autoconfig");
        assert_eq!(ok("  autodiscover.example.com  ", " EXAMPLE.com "), "autodiscover");
        assert_eq!(ok("a.b.sub.example.com", "sub.example.com"), "a.b");
    }

    #[test]
    fn relative_record_name_refuses_names_outside_the_zone() {
        for (name, apex) in [
            ("_imaps._tcp.example.net", "example.com"),
            // Suffix match must be on a label boundary.
            ("autoconfig.notexample.com", "example.com"),
            ("badexample.com", "example.com"),
            // The parent of the zone is outside it.
            ("example.com", "sub.example.com"),
            ("", "example.com"),
            (".", "example.com"),
            ("autoconfig", "example.com"),
        ] {
            let err = relative_record_name(name, apex)
                .expect_err(&format!("{name:?} in {apex} must be refused"));
            assert!(!err.is_empty());
        }
        assert!(relative_record_name("example.com", "").is_err());
        assert!(relative_record_name("x..example.com", "example.com").is_err());
    }
}
