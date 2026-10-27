//! Host-wide custom-domain inventory (`domain_bindings` / `domain_names`).
//!
//! Three objects (prd-custom-domains-and-certs-v1 D1): zone ≠ binding ≠
//! termination+cert. This module is the binding: an apex attached to
//! THIS daemon, plus hostnames under it. Attach is not NS transfer
//! (`dns_write` is only true after control-plane bind succeeds).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::mail_domain::normalize_mail_domain;

pub const ROLE_MAIL: &str = "mail";
pub const ROLE_PUBLISH: &str = "publish";
pub const ROLE_DIRECT: &str = "direct";
pub const ROLE_OTHER: &str = "other";

pub const ROLES: &[&str] = &[ROLE_MAIL, ROLE_PUBLISH, ROLE_DIRECT, ROLE_OTHER];

/// A hostname bound to one App (or published `--cmd` service) by its
/// name: role `app:<name>` (0.46.1). The App learns it as a public origin
/// (`K2_APP_ORIGIN` / `K2_APP_ORIGINS`) on its next deploy or restart.
pub const ROLE_APP_PREFIX: &str = "app:";

/// The App a role binds its hostname to (`app:hearth` → `hearth`).
pub fn app_of_role(role: &str) -> Option<&str> {
    role.strip_prefix(ROLE_APP_PREFIX).filter(|n| !n.is_empty())
}

/// Normalize + validate a role. Empty/missing → `other`.
pub fn normalize_role(raw: Option<&str>) -> Result<String, String> {
    let s = raw.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(ROLE_OTHER);
    let lower = s.to_ascii_lowercase();
    if ROLES.iter().any(|r| *r == lower) {
        return Ok(lower);
    }
    if let Some(name) = lower.strip_prefix(ROLE_APP_PREFIX) {
        if crate::published_services::is_valid_name(name) {
            return Ok(lower);
        }
        return Err(format!("role app:<name> needs an App or published service name (got '{s}')"));
    }
    Err(format!(
        "role must be one of {}|app:<name> (got '{s}')",
        ROLES.join("|")
    ))
}

/// The custom hostnames bound to App `name` (role `app:<name>`) whose apex
/// is still attached here. Sorted; a dangling apex is treated as missing.
pub fn hostnames_for_app(conn: &Connection, name: &str) -> rusqlite::Result<Vec<String>> {
    let role = format!("{ROLE_APP_PREFIX}{}", name.to_ascii_lowercase());
    let mut stmt = conn.prepare(
        "SELECT n.hostname FROM domain_names n JOIN domain_bindings b ON b.apex = n.apex \
         WHERE n.role = ?1 ORDER BY n.hostname",
    )?;
    let rows = stmt.query_map(params![role], |row| row.get::<_, String>(0))?;
    rows.collect()
}

/// True iff `hostname` is the apex or a label under it.
pub fn hostname_under_apex(hostname: &str, apex: &str) -> bool {
    hostname == apex || hostname.ends_with(&format!(".{apex}"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Control-plane zone status: records are live (NS delegated to k2.dev).
pub const ZONE_STATUS_ACTIVE: &str = "active";
/// Control-plane zone status: zone exists in the k2.dev account but the
/// registrar nameservers do not point at k2.dev yet. No record writes.
pub const ZONE_STATUS_PENDING_NS: &str = "pending_ns";
/// Control-plane zone status: the zone is winding down (plan ended).
/// Records still serve, but no writes.
pub const ZONE_STATUS_WINDDOWN: &str = "winddown";
/// Control-plane zone status: suspended by k2.dev. No writes.
pub const ZONE_STATUS_SUSPENDED: &str = "suspended";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DomainBinding {
    pub apex: String,
    pub zone_id: Option<String>,
    pub dns_write: bool,
    pub created_at: i64,
    /// `active` | `pending_ns` from k2.dev bind (A8.1). `None` for BYO
    /// rows and rows bound before A8.1 (0127).
    #[serde(default)]
    pub status: Option<String>,
    /// Nameservers k2.dev wants at the registrar (A8.1).
    #[serde(default)]
    pub nameservers: Vec<String>,
    /// k2.dev auto-added this zone to the account on bind (`created:true`).
    #[serde(default)]
    pub auto_created: bool,
}

impl DomainBinding {
    /// True when the zone is in the k2.dev account but its nameservers do
    /// not point at k2.dev yet (record writes are refused).
    pub fn is_pending_ns(&self) -> bool {
        self.status.as_deref() == Some(ZONE_STATUS_PENDING_NS)
    }

    /// True when k2.dev holds the zone read-only (`winddown` / `suspended`).
    pub fn is_readonly(&self) -> bool {
        matches!(
            self.status.as_deref(),
            Some(ZONE_STATUS_WINDDOWN) | Some(ZONE_STATUS_SUSPENDED)
        )
    }
}

/// Everything a successful control-plane bind says about the zone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoundZone {
    pub zone_id: Option<String>,
    pub status: Option<String>,
    pub nameservers: Vec<String>,
    pub dns_write: bool,
    pub auto_created: bool,
}

const BINDING_COLS: &str =
    "apex, zone_id, dns_write, created_at, status, nameservers, auto_created";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DomainName {
    pub hostname: String,
    pub apex: String,
    pub role: String,
}

impl DomainBinding {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let dns_write: i64 = row.get(2)?;
        let ns_json: Option<String> = row.get(5)?;
        let nameservers = ns_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default();
        let auto_created: i64 = row.get(6)?;
        Ok(Self {
            apex: row.get(0)?,
            zone_id: row.get(1)?,
            dns_write: dns_write != 0,
            created_at: row.get(3)?,
            status: row.get(4)?,
            nameservers,
            auto_created: auto_created != 0,
        })
    }
}

pub fn list_bindings(conn: &Connection) -> rusqlite::Result<Vec<DomainBinding>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {BINDING_COLS} FROM domain_bindings ORDER BY apex"
    ))?;
    let rows = stmt.query_map([], DomainBinding::from_row)?;
    rows.collect()
}

pub fn get_binding(conn: &Connection, apex: &str) -> rusqlite::Result<Option<DomainBinding>> {
    conn.query_row(
        &format!("SELECT {BINDING_COLS} FROM domain_bindings WHERE apex = ?1"),
        params![apex],
        DomainBinding::from_row,
    )
    .optional()
}

/// The zone a DNS name lives in, out of `apexes`: the name itself, else the
/// LONGEST apex it is a dot-suffix of (`notify.brand.com` under
/// `brand.com`, never under `rand.com`). Returns `(apex, sub-labels)`,
/// sub-labels empty for the apex itself (Email Notifications E11: a
/// sending subdomain's DKIM and autoconfig rows go in its parent's zone).
pub fn zone_for_name<'a>(name: &str, apexes: impl IntoIterator<Item = &'a str>) -> Option<(&'a str, String)> {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if n.is_empty() {
        return None;
    }
    let mut best: Option<(&'a str, String)> = None;
    for apex in apexes {
        let a = apex.trim().trim_end_matches('.').to_ascii_lowercase();
        if a.is_empty() {
            continue;
        }
        let sub = if n == a {
            String::new()
        } else if let Some(prefix) = n.strip_suffix(&format!(".{a}")) {
            prefix.to_string()
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| a.len() > b.trim_end_matches('.').len()) {
            best = Some((apex, sub));
        }
    }
    best
}

/// [`zone_for_name`] over this box's domain bindings: the binding a mail
/// domain's records go in, and the sub-labels to prefix record names with.
pub fn binding_for_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<(DomainBinding, String)>> {
    let all = list_bindings(conn)?;
    let Some((apex, sub)) = zone_for_name(name, all.iter().map(|b| b.apex.as_str())) else { return Ok(None) };
    let apex = apex.to_string();
    Ok(all.into_iter().find(|b| b.apex == apex).map(|b| (b, sub)))
}

pub fn get_binding_by_zone_id(conn: &Connection, zone_id: &str) -> rusqlite::Result<Option<DomainBinding>> {
    let z = zone_id.trim();
    if z.is_empty() {
        return Ok(None);
    }
    conn.query_row(
        &format!("SELECT {BINDING_COLS} FROM domain_bindings WHERE zone_id = ?1"),
        params![z],
        DomainBinding::from_row,
    )
    .optional()
}

/// Insert or refresh a BYO / legacy binding (no control-plane zone
/// status). Idempotent on apex. Clears any A8.1 status so a row that
/// k2.dev no longer owns stops reading as pending/active.
pub fn upsert_binding(
    conn: &Connection,
    apex: &str,
    zone_id: Option<&str>,
    dns_write: bool,
) -> rusqlite::Result<DomainBinding> {
    upsert_binding_zone(
        conn,
        apex,
        &BoundZone {
            zone_id: zone_id.map(str::to_string),
            status: None,
            nameservers: Vec::new(),
            dns_write,
            auto_created: false,
        },
    )
}

/// Insert or refresh a binding with everything k2.dev bind returned
/// (A8.1: status, nameservers, created). Idempotent on apex.
/// `auto_created` is sticky: a later re-bind that omits `created` keeps
/// the note that k2.dev added the zone.
pub fn upsert_binding_zone(
    conn: &Connection,
    apex: &str,
    zone: &BoundZone,
) -> rusqlite::Result<DomainBinding> {
    let now = now_unix();
    let ns_json: Option<String> = if zone.nameservers.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&zone.nameservers).unwrap_or_else(|_| "[]".into()))
    };
    let keep_created = zone.status.is_some();
    conn.execute(
        "INSERT INTO domain_bindings \
           (apex, zone_id, dns_write, created_at, status, nameservers, auto_created) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(apex) DO UPDATE SET \
           zone_id = excluded.zone_id, \
           dns_write = excluded.dns_write, \
           status = excluded.status, \
           nameservers = excluded.nameservers, \
           auto_created = CASE WHEN ?8 THEN MAX(domain_bindings.auto_created, excluded.auto_created) \
                               ELSE excluded.auto_created END",
        params![
            apex,
            zone.zone_id.as_deref(),
            if zone.dns_write { 1 } else { 0 },
            now,
            zone.status.as_deref(),
            ns_json,
            if zone.auto_created { 1 } else { 0 },
            keep_created,
        ],
    )?;
    Ok(get_binding(conn, apex)?.expect("just upserted"))
}

/// Store a fresh control-plane status on a bound row (prd-dns-pending-and-
/// cutover-safety-v1 P1/P2: the box's own re-check). Touches only `status`
/// and `dns_write`; nameservers, `auto_created` and `created_at` stay. The
/// row must still carry `zone_id` (a row re-bound to another zone in the
/// meantime is left alone). Returns true when the row changed.
pub fn set_zone_status(
    conn: &Connection,
    apex: &str,
    zone_id: &str,
    status: &str,
    dns_write: bool,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE domain_bindings SET status = ?3, dns_write = ?4 \
         WHERE apex = ?1 AND zone_id = ?2 \
           AND (status IS NOT ?3 OR dns_write != ?4)",
        params![apex, zone_id, status, if dns_write { 1 } else { 0 }],
    )?;
    Ok(n > 0)
}

/// Remove apex and cascade hostnames. Returns whether a row existed.
pub fn remove_binding(conn: &Connection, apex: &str) -> rusqlite::Result<bool> {
    conn.execute(
        "DELETE FROM domain_names WHERE apex = ?1",
        params![apex],
    )?;
    let n = conn.execute(
        "DELETE FROM domain_bindings WHERE apex = ?1",
        params![apex],
    )?;
    Ok(n > 0)
}

pub fn list_names(conn: &Connection) -> rusqlite::Result<Vec<DomainName>> {
    let mut stmt = conn.prepare(
        "SELECT hostname, apex, role FROM domain_names ORDER BY apex, hostname",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DomainName {
            hostname: row.get(0)?,
            apex: row.get(1)?,
            role: row.get(2)?,
        })
    })?;
    rows.collect()
}

pub fn list_names_for_apex(conn: &Connection, apex: &str) -> rusqlite::Result<Vec<DomainName>> {
    let mut stmt = conn.prepare(
        "SELECT hostname, apex, role FROM domain_names WHERE apex = ?1 ORDER BY hostname",
    )?;
    let rows = stmt.query_map(params![apex], |row| {
        Ok(DomainName {
            hostname: row.get(0)?,
            apex: row.get(1)?,
            role: row.get(2)?,
        })
    })?;
    rows.collect()
}

pub fn get_name(conn: &Connection, hostname: &str) -> rusqlite::Result<Option<DomainName>> {
    conn.query_row(
        "SELECT hostname, apex, role FROM domain_names WHERE hostname = ?1",
        params![hostname],
        |row| {
            Ok(DomainName {
                hostname: row.get(0)?,
                apex: row.get(1)?,
                role: row.get(2)?,
            })
        },
    )
    .optional()
}

pub fn upsert_name(conn: &Connection, hostname: &str, apex: &str, role: &str) -> rusqlite::Result<DomainName> {
    conn.execute(
        "INSERT INTO domain_names (hostname, apex, role) VALUES (?1, ?2, ?3) \
         ON CONFLICT(hostname) DO UPDATE SET apex = excluded.apex, role = excluded.role",
        params![hostname, apex, role],
    )?;
    Ok(get_name(conn, hostname)?.expect("just upserted"))
}

pub fn remove_name(conn: &Connection, hostname: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM domain_names WHERE hostname = ?1",
        params![hostname],
    )?;
    Ok(n > 0)
}

/// True when this box may write the zone (`dns_write=1` and matching zone id).
pub fn zone_attached_for_write(conn: &Connection, zone_id: &str) -> rusqlite::Result<bool> {
    Ok(get_binding_by_zone_id(conn, zone_id)?
        .map(|b| b.dns_write)
        .unwrap_or(false))
}

/// True when this box may write the apex (`dns_write=1`).
pub fn apex_attached_for_write(conn: &Connection, apex: &str) -> rusqlite::Result<bool> {
    Ok(get_binding(conn, apex)?.map(|b| b.dns_write).unwrap_or(false))
}

/// Normalize a user-supplied apex (IDNA punycode).
pub fn normalize_apex(input: &str) -> Result<String, String> {
    normalize_mail_domain(input)
}

/// Normalize a hostname FQDN (same IDNA helper as apex).
pub fn normalize_hostname(input: &str) -> Result<String, String> {
    normalize_mail_domain(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        crate::db::isolated_test_connection()
    }

    /// Email Notifications E11: a sending subdomain's records go in the
    /// longest bound zone it sits under, named relative to it.
    #[test]
    fn a_subdomain_finds_its_longest_bound_zone() {
        let apexes = ["brand.com", "notify.brand.com.example", "rand.com", "deep.brand.com"];
        assert_eq!(zone_for_name("notify.brand.com", apexes), Some(("brand.com", "notify".to_string())));
        assert_eq!(zone_for_name("x.deep.brand.com.", apexes), Some(("deep.brand.com", "x".to_string())));
        assert_eq!(zone_for_name("Brand.COM", apexes), Some(("brand.com", String::new())));
        assert_eq!(zone_for_name("abrand.com", apexes), None, "a label boundary, not a string suffix");
        assert_eq!(zone_for_name("", apexes), None);
        let conn = fresh();
        upsert_binding(&conn, "brand.com", Some("z1"), true).unwrap();
        let (b, sub) = binding_for_name(&conn, "notify.brand.com").unwrap().expect("bound");
        assert_eq!((b.apex.as_str(), sub.as_str()), ("brand.com", "notify"));
        assert!(binding_for_name(&conn, "brand.org").unwrap().is_none());
    }

    #[test]
    fn migration_creates_tables() {
        let conn = fresh();
        for table in ["domain_bindings", "domain_names"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn hostname_under_apex_is_strict() {
        assert!(hostname_under_apex("example.com", "example.com"));
        assert!(hostname_under_apex("mail.example.com", "example.com"));
        assert!(hostname_under_apex("a.b.example.com", "example.com"));
        assert!(!hostname_under_apex("notexample.com", "example.com"));
        assert!(!hostname_under_apex("example.com.evil.com", "example.com"));
        assert!(!hostname_under_apex("mail.other.com", "example.com"));
    }

    #[test]
    fn upsert_is_idempotent_and_remove_cascades() {
        let conn = fresh();
        let b = upsert_binding(&conn, "example.com", Some("z1"), true).unwrap();
        assert_eq!(b.apex, "example.com");
        assert_eq!(b.zone_id.as_deref(), Some("z1"));
        assert!(b.dns_write);
        upsert_binding(&conn, "example.com", Some("z1"), true).unwrap();
        assert_eq!(list_bindings(&conn).unwrap().len(), 1);

        upsert_name(&conn, "mail.example.com", "example.com", ROLE_MAIL).unwrap();
        upsert_name(&conn, "example.com", "example.com", ROLE_OTHER).unwrap();
        assert_eq!(list_names_for_apex(&conn, "example.com").unwrap().len(), 2);

        assert!(remove_binding(&conn, "example.com").unwrap());
        assert!(list_bindings(&conn).unwrap().is_empty());
        assert!(list_names(&conn).unwrap().is_empty());
    }

    #[test]
    fn zone_write_gate() {
        let conn = fresh();
        upsert_binding(&conn, "byo.example", None, false).unwrap();
        upsert_binding(&conn, "hosted.example", Some("zone-hosted"), true).unwrap();
        assert!(!apex_attached_for_write(&conn, "byo.example").unwrap());
        assert!(apex_attached_for_write(&conn, "hosted.example").unwrap());
        assert!(!zone_attached_for_write(&conn, "zone-hosted-nope").unwrap());
        assert!(zone_attached_for_write(&conn, "zone-hosted").unwrap());
        assert!(!apex_attached_for_write(&conn, "missing.example").unwrap());
    }

    #[test]
    fn set_zone_status_touches_only_status_and_write() {
        let conn = fresh();
        let b = upsert_binding_zone(
            &conn,
            "szs.example",
            &BoundZone {
                zone_id: Some("z-szs".into()),
                status: Some(ZONE_STATUS_PENDING_NS.into()),
                nameservers: vec!["ns1.k2.dev".into()],
                dns_write: false,
                auto_created: true,
            },
        )
        .unwrap();
        assert!(b.is_pending_ns());
        assert!(!b.is_readonly());
        // Wrong zone id: untouched.
        assert!(!set_zone_status(&conn, "szs.example", "z-other", ZONE_STATUS_ACTIVE, true).unwrap());
        assert!(get_binding(&conn, "szs.example").unwrap().unwrap().is_pending_ns());
        // Flip.
        assert!(set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_ACTIVE, true).unwrap());
        let after = get_binding(&conn, "szs.example").unwrap().unwrap();
        assert_eq!(after.status.as_deref(), Some(ZONE_STATUS_ACTIVE));
        assert!(after.dns_write);
        assert_eq!(after.nameservers, vec!["ns1.k2.dev".to_string()]);
        assert!(after.auto_created);
        assert_eq!(after.created_at, b.created_at);
        // Same values again: no change reported.
        assert!(!set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_ACTIVE, true).unwrap());
        // Read-only states.
        assert!(set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_WINDDOWN, false).unwrap());
        let ro = get_binding(&conn, "szs.example").unwrap().unwrap();
        assert!(ro.is_readonly());
        assert!(!ro.dns_write);
        assert!(set_zone_status(&conn, "szs.example", "z-szs", ZONE_STATUS_SUSPENDED, false).unwrap());
        assert!(get_binding(&conn, "szs.example").unwrap().unwrap().is_readonly());
        // Missing row: nothing.
        assert!(!set_zone_status(&conn, "nope.example", "z-szs", ZONE_STATUS_ACTIVE, true).unwrap());
    }

    #[test]
    fn role_normalize() {
        assert_eq!(normalize_role(None).unwrap(), ROLE_OTHER);
        assert_eq!(normalize_role(Some("MAIL")).unwrap(), ROLE_MAIL);
        assert!(normalize_role(Some("wildcard")).is_err());
        assert_eq!(normalize_role(Some("App:Hearth")).unwrap(), "app:hearth");
        assert_eq!(app_of_role("app:hearth"), Some("hearth"));
        assert_eq!(app_of_role(ROLE_PUBLISH), None);
        let e = normalize_role(Some("app:")).unwrap_err();
        assert!(e.contains("app:<name>"), "{e}");
        assert!(normalize_role(Some("app:no_underscores")).is_err());
    }

    #[test]
    fn hostnames_for_app_are_its_bound_names_on_attached_apexes() {
        let conn = fresh();
        upsert_binding(&conn, "example.com", None, false).unwrap();
        upsert_name(&conn, "hearth.example.com", "example.com", "app:hearth").unwrap();
        upsert_name(&conn, "chat.example.com", "example.com", "app:hearth").unwrap();
        upsert_name(&conn, "other.example.com", "example.com", "app:other").unwrap();
        upsert_name(&conn, "www.example.com", "example.com", ROLE_PUBLISH).unwrap();
        // A dangling apex (no binding row) counts as missing.
        upsert_name(&conn, "hearth.gone.test", "gone.test", "app:hearth").unwrap();
        assert_eq!(
            hostnames_for_app(&conn, "hearth").unwrap(),
            vec!["chat.example.com".to_string(), "hearth.example.com".to_string()]
        );
        assert_eq!(hostnames_for_app(&conn, "nobody").unwrap(), Vec::<String>::new());
    }
}
