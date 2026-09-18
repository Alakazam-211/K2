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

/// Normalize + validate a role. Empty/missing → `other`.
pub fn normalize_role(raw: Option<&str>) -> Result<String, String> {
    let s = raw.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(ROLE_OTHER);
    let lower = s.to_ascii_lowercase();
    if ROLES.iter().any(|r| *r == lower) {
        Ok(lower)
    } else {
        Err(format!(
            "role must be one of {} (got '{s}')",
            ROLES.join("|")
        ))
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DomainBinding {
    pub apex: String,
    pub zone_id: Option<String>,
    pub dns_write: bool,
    pub created_at: i64,
}

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
        Ok(Self {
            apex: row.get(0)?,
            zone_id: row.get(1)?,
            dns_write: dns_write != 0,
            created_at: row.get(3)?,
        })
    }
}

pub fn list_bindings(conn: &Connection) -> rusqlite::Result<Vec<DomainBinding>> {
    let mut stmt = conn.prepare(
        "SELECT apex, zone_id, dns_write, created_at FROM domain_bindings ORDER BY apex",
    )?;
    let rows = stmt.query_map([], DomainBinding::from_row)?;
    rows.collect()
}

pub fn get_binding(conn: &Connection, apex: &str) -> rusqlite::Result<Option<DomainBinding>> {
    conn.query_row(
        "SELECT apex, zone_id, dns_write, created_at FROM domain_bindings WHERE apex = ?1",
        params![apex],
        DomainBinding::from_row,
    )
    .optional()
}

pub fn get_binding_by_zone_id(conn: &Connection, zone_id: &str) -> rusqlite::Result<Option<DomainBinding>> {
    let z = zone_id.trim();
    if z.is_empty() {
        return Ok(None);
    }
    conn.query_row(
        "SELECT apex, zone_id, dns_write, created_at FROM domain_bindings WHERE zone_id = ?1",
        params![z],
        DomainBinding::from_row,
    )
    .optional()
}

/// Insert or refresh a binding. Idempotent on apex.
pub fn upsert_binding(
    conn: &Connection,
    apex: &str,
    zone_id: Option<&str>,
    dns_write: bool,
) -> rusqlite::Result<DomainBinding> {
    let now = now_unix();
    conn.execute(
        "INSERT INTO domain_bindings (apex, zone_id, dns_write, created_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(apex) DO UPDATE SET zone_id = excluded.zone_id, dns_write = excluded.dns_write",
        params![apex, zone_id, if dns_write { 1 } else { 0 }, now],
    )?;
    Ok(get_binding(conn, apex)?.expect("just upserted"))
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
    fn role_normalize() {
        assert_eq!(normalize_role(None).unwrap(), ROLE_OTHER);
        assert_eq!(normalize_role(Some("MAIL")).unwrap(), ROLE_MAIL);
        assert!(normalize_role(Some("wildcard")).is_err());
    }
}
