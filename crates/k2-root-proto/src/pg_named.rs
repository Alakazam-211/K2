//! Named Postgres clusters (protocol 15, `namespaces.pgnamed`;
//! prd-named-pg-clusters-v1, 0.46.11).
//!
//! Beside each major's `<M>/main`, the root helper can make a cluster with
//! a name of its own (`17/feedback`) for one database that needs a cluster
//! to itself (WAL archiving and point-in-time restore rewind a whole
//! cluster). The rules, shared by the daemon and the helper:
//!
//! - [`ClusterName`]: `^[a-z][a-z0-9_]{1,23}$`, never `main`. One name
//!   space per box: the helper refuses a name `pg_lsclusters` shows on any
//!   major, or whose data or config dir exists under any major.
//! - Ports stay in K2's band ([`crate::pg_major::BAND`]), never
//!   [`crate::pg_major::PRIMARY_PORT`] (the box's first `<M>/main`).
//! - The helper owns the list of the clusters it made, [`NAMED_STATE`]
//!   (root 0600), written only right after its own `pg_createcluster`.
//!   Every named verb checks the pair is in that list and matches what
//!   runs (data dir, port; `ensure-cluster` and `clusters` also the
//!   cluster's `system_identifier`). A cluster made by hand (`17/k2db` on
//!   akzm) can never enter it, so no verb ever touches one.
//! - `<M>` alone keeps meaning `<M>/main` on every existing verb.

use serde::{Deserialize, Serialize};

use crate::pg_major::PgMajor;

/// The helper's list of the named clusters it created.
pub const NAMED_STATE: &str = "/var/lib/k2-root-helper/pg-named.json";
/// Largest list the helper reads.
pub const NAMED_STATE_CAP: usize = 64 * 1024;
/// Most named clusters on one box (the band holds 18 ports in all).
pub const MAX_NAMED: usize = 16;
/// WAL spools of named clusters with archiving: `<root>/<M>-<name>/`.
pub const WAL_SPOOL_ROOT: &str = "/var/lib/postgresql/k2-wal";
/// The archive settings of a named cluster with `--wal-archive`.
pub const WAL_ARCHIVE_CONF_FILE: &str = "97-k2-wal-archive.conf";
/// How often a quiet cluster still closes a WAL segment for the archive.
pub const ARCHIVE_TIMEOUT_SECS: u32 = 300;
/// The doctor warns when a spool holds more than this (K2 Backups ships
/// and prunes it).
pub const WAL_SPOOL_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// A named cluster's name: `^[a-z][a-z0-9_]{1,23}$`, not `main`. No `-`:
/// the systemd instance is `postgresql@<M>-<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ClusterName(String);

impl ClusterName {
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let ok = (2..=24).contains(&b.len())
            && b[0].is_ascii_lowercase()
            && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
            && s != crate::pg_major::CLUSTER_NAME;
        ok.then(|| Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `<M>-<name>` on helper argv (the named form of a cluster token, as
/// Debian spells the instance: `postgresql@17-feedback`). Helper tokens
/// never carry a `/`, and a name never carries a `-`, so the split is
/// unambiguous. A bare `<M>` is not one: that is `<M>/main`, [`PgMajor`].
/// People and the daemon write `17/feedback` ([`label`]).
pub fn parse_ref(t: &str) -> Option<(PgMajor, ClusterName)> {
    let (m, n) = t.split_once('-')?;
    Some((PgMajor::parse(m)?, ClusterName::parse(n)?))
}

pub fn ref_token(m: PgMajor, n: &ClusterName) -> String {
    format!("{}-{}", m.get(), n.as_str())
}

/// How people read a cluster: `17/feedback`.
pub fn label(m: u32, n: &str) -> String {
    format!("{m}/{n}")
}

/// `/var/lib/postgresql/<M>/<name>`.
pub fn data_dir(m: u32, n: &str) -> String {
    format!("/var/lib/postgresql/{m}/{n}")
}
/// `/etc/postgresql/<M>/<name>`.
pub fn etc_dir(m: u32, n: &str) -> String {
    format!("/etc/postgresql/{m}/{n}")
}
/// `postgresql@<M>-<name>.service`.
pub fn unit(m: u32, n: &str) -> String {
    format!("postgresql@{m}-{n}.service")
}
/// The shared-slice drop-in of a named cluster's own instance.
pub fn slice_dropin(m: u32, n: &str) -> String {
    format!("/etc/systemd/system/postgresql@{m}-{n}.service.d/k2-slice.conf")
}
/// `<WAL_SPOOL_ROOT>/<M>-<name>`.
pub fn wal_spool(m: u32, n: &str) -> String {
    format!("{WAL_SPOOL_ROOT}/{m}-{n}")
}

/// `conf.d/97-k2-wal-archive.conf`: archive every finished WAL segment
/// into the cluster's spool (never overwriting a file already there). No
/// caller text reaches the command: the path is built from validated
/// tokens. K2 Backups ships the spool off the box and prunes it.
pub fn wal_archive_conf(m: u32, n: &str) -> String {
    let d = wal_spool(m, n);
    format!(
        "# K2 WAL archive for {m}/{n}. Written by k2-root-helper `pg ensure-cluster --wal-archive`.\n\
         # K2 Backups ships {d} off the box and prunes it.\n\
         archive_mode = on\n\
         archive_command = 'test ! -f {d}/%f && cp %p {d}/%f'\n\
         archive_timeout = {ARCHIVE_TIMEOUT_SECS}\n"
    )
}

/// One named cluster the helper made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamedRec {
    pub major: u32,
    pub name: String,
    pub port: u16,
    pub data_dir: String,
    /// `pg_control_system().system_identifier` right after creation.
    pub system_id: String,
    #[serde(default)]
    pub wal_archive: bool,
    pub created_at: i64,
}

impl NamedRec {
    /// The record is one the helper could have written: valid tokens, the
    /// K2 layout, a band port other than 5432 and a numeric system id.
    pub fn valid(&self) -> bool {
        PgMajor::new(self.major).is_some()
            && ClusterName::parse(&self.name).is_some()
            && self.data_dir == data_dir(self.major, &self.name)
            && crate::pg_major::in_band(self.port)
            && self.port != crate::pg_major::PRIMARY_PORT
            && !self.system_id.is_empty()
            && self.system_id.len() <= 20
            && self.system_id.bytes().all(|b| b.is_ascii_digit())
    }
}

/// [`NAMED_STATE`]'s shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedState {
    #[serde(default)]
    pub clusters: Vec<NamedRec>,
}

impl NamedState {
    /// The valid record for `(major, name)`.
    pub fn get(&self, major: u32, name: &str) -> Option<&NamedRec> {
        self.clusters.iter().find(|r| r.major == major && r.name == name && r.valid())
    }
}

/// A `system_identifier` as psql answers it (`-tA`): digits only.
pub fn system_id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["feedback", "fb", "k2_feedback", "a1", "x".repeat(24).as_str()] {
            assert!(ClusterName::parse(ok).is_some(), "{ok}");
        }
        for bad in ["", "f", "main", "Feedback", "1fb", "_fb", "fb-x", "fb/x", "fb x", "x".repeat(25).as_str(), "fé"] {
            assert!(ClusterName::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn refs() {
        let (m, n) = parse_ref("17-feedback").expect("ref");
        assert_eq!((m.get(), n.as_str()), (17, "feedback"));
        assert_eq!(ref_token(m, &n), "17-feedback");
        assert_eq!(label(17, "feedback"), "17/feedback");
        for bad in ["17", "17-", "-feedback", "17-main", "017-feedback", "15-feedback", "17-feed-back", "17-Feedback", "17/feedback"] {
            assert!(parse_ref(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn layout() {
        assert_eq!(data_dir(17, "feedback"), "/var/lib/postgresql/17/feedback");
        assert_eq!(etc_dir(17, "feedback"), "/etc/postgresql/17/feedback");
        assert_eq!(unit(17, "feedback"), "postgresql@17-feedback.service");
        assert_eq!(slice_dropin(17, "feedback"), "/etc/systemd/system/postgresql@17-feedback.service.d/k2-slice.conf");
        let c = wal_archive_conf(17, "feedback");
        assert!(c.contains("archive_mode = on\n"));
        assert!(c.contains("archive_command = 'test ! -f /var/lib/postgresql/k2-wal/17-feedback/%f && cp %p /var/lib/postgresql/k2-wal/17-feedback/%f'\n"), "{c}");
        assert!(c.contains("archive_timeout = 300\n"));
    }

    #[test]
    fn records_validate() {
        let r = NamedRec {
            major: 17,
            name: "feedback".into(),
            port: 5434,
            data_dir: "/var/lib/postgresql/17/feedback".into(),
            system_id: "7301234567890123456".into(),
            wal_archive: true,
            created_at: 1,
        };
        assert!(r.valid());
        let s = NamedState { clusters: vec![r.clone()] };
        assert!(s.get(17, "feedback").is_some());
        assert!(s.get(17, "k2db").is_none());
        assert!(s.get(16, "feedback").is_none());
        for bad in [
            NamedRec { port: 5432, ..r.clone() },
            NamedRec { port: 5460, ..r.clone() },
            NamedRec { name: "main".into(), data_dir: "/var/lib/postgresql/17/main".into(), ..r.clone() },
            NamedRec { data_dir: "/srv/pg".into(), ..r.clone() },
            NamedRec { system_id: "".into(), ..r.clone() },
            NamedRec { system_id: "12x".into(), ..r.clone() },
            NamedRec { major: 15, data_dir: "/var/lib/postgresql/15/feedback".into(), ..r.clone() },
        ] {
            assert!(!bad.valid(), "{bad:?}");
            assert!(NamedState { clusters: vec![bad] }.get(17, "feedback").is_none());
        }
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<NamedState>(&j).unwrap(), s);
        assert!(serde_json::from_str::<NamedState>(r#"{"clusters":[],"x":1}"#).is_err());
    }
}
