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
pub const DNS_DENIED_HINT: &str = "this agent isn't allowed to manage DNS — \
the owner can enable it in Settings → Agents / Projects (or flip dnsManageEnabled)";

/// Teaching text when a local envelope check rejects the request.
pub const ZONE_LIFECYCLE_HINT: &str =
    "agents cannot create or delete zones — zone lifecycle is owner-only";

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
}
