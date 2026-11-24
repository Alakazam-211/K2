//! Postgres majors on a K2 box, in one place (prd-pg17-and-db-upgrade-v1
//! rev 2 §3–§4). The daemon and the root helper both read this table:
//! which majors a database may live on, when each one reaches end of
//! life, the Debian cluster layout of a major, and K2's port band.
//!
//! - [`SUPPORTED`]: majors a database may be created on or moved to.
//! - [`PREVIEW`]: majors open to the owner only, behind `--preview`
//!   (empty in 0.46.3: 18 waits for a `pg_query` with the 18 grammar).
//! - [`LAYOUT`]: majors the cluster layout, the records and the helper's
//!   `add-major` allow-list accept. 18 is here, so turning it on later is
//!   a constant change plus drills, not a new layout.
//! - [`NEW_INSTALL`]: the major a new box is born on, and the default for
//!   new databases on every box from 0.46.3 (`sql_server.default_major`
//!   NULL means this).
//!
//! Nothing derives a port from a major: the first cluster keeps
//! [`PRIMARY_PORT`], each further one takes the first free port in
//! [`BAND`], and K2 records it (`sql_clusters`).

/// Majors a database may be created on or moved to.
pub const SUPPORTED: &[u32] = &[16, 17];
/// Owner-only preview majors (`--preview`). Empty: no 18 preview in 0.46.3.
pub const PREVIEW: &[u32] = &[];
/// Majors the layout, the records and `add-major` accept.
pub const LAYOUT: &[u32] = &[16, 17, 18];
/// What new boxes are born on and what `k2 db create` uses by default.
pub const NEW_INSTALL: u32 = 17;
/// Community end-of-life dates (year, month, day). A major past its date
/// is refused for new databases and as a move target.
pub const EOL: &[(u32, (i32, u32, u32))] = &[
    (16, (2028, 11, 9)),
    (17, (2029, 11, 8)),
    (18, (2030, 11, 14)),
];

/// The cluster name K2 creates and accepts (`<major>/main`). Other names
/// are refused (§10).
pub const CLUSTER_NAME: &str = "main";
/// The box's first cluster keeps this port; nothing that works today moves.
pub const PRIMARY_PORT: u16 = 5432;
/// K2's Postgres port band (inclusive). Every K2 cluster listens inside
/// it; the whole band is refused to App box services and admitted (on
/// loopback, by role and password) to App UIDs.
pub const BAND: (u16, u16) = (5432, 5449);

/// Why a major (or a move) is refused. `code()` is the CLI/JSON reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not in [`LAYOUT`]: K2 knows nothing about it.
    Unknown(u32),
    /// Known to the layout but neither supported nor (with the flag)
    /// preview.
    Unsupported(u32),
    /// A preview major asked for without `--preview` (owner only).
    PreviewOnly(u32),
    /// Past its community end-of-life date.
    Eol(u32),
    /// A move that isn't upward (same major or a downgrade).
    NotUpward { from: u32, to: u32 },
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unknown(_) => "unknown_major",
            Self::Unsupported(_) => "unsupported_major",
            Self::PreviewOnly(_) => "preview_major",
            Self::Eol(_) => "major_eol",
            Self::NotUpward { .. } => "not_upward",
        }
    }

    pub fn hint(&self) -> String {
        let sup = SUPPORTED.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        match self {
            Self::Unknown(m) => format!("Postgres {m} is not a major K2 knows (supported: {sup})"),
            Self::Unsupported(m) => format!("Postgres {m} is not supported for databases yet (supported: {sup})"),
            Self::PreviewOnly(m) => format!("Postgres {m} is a preview: the owner may use it with --preview"),
            Self::Eol(m) => format!(
                "Postgres {m} is past its end of life ({}); pick a newer major (supported: {sup})",
                eol(*m).map(|(y, mo, d)| format!("{y:04}-{mo:02}-{d:02}")).unwrap_or_default()
            ),
            Self::NotUpward { from, to } => {
                format!("a database moves upward only: {from} → {to} is not a move K2 makes")
            }
        }
    }
}

/// The end-of-life date of a known major.
pub fn eol(major: u32) -> Option<(i32, u32, u32)> {
    EOL.iter().find(|(m, _)| *m == major).map(|(_, d)| *d)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Today, in days since the epoch (UTC).
pub fn today_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

/// Is `major` past its end of life on `today` (days since the epoch)?
/// Unknown majors count as past it (fail closed).
pub fn past_eol(major: u32, today: i64) -> bool {
    match eol(major) {
        Some((y, m, d)) => today > days_from_civil(y, m, d),
        None => true,
    }
}

/// May a database be created on (or moved to) `major` on `today`?
/// `preview` is the owner's `--preview` flag.
pub fn check_target(major: u32, preview: bool, today: i64) -> Result<(), Refusal> {
    if !LAYOUT.contains(&major) {
        return Err(Refusal::Unknown(major));
    }
    if !SUPPORTED.contains(&major) {
        if !PREVIEW.contains(&major) {
            return Err(Refusal::Unsupported(major));
        }
        if !preview {
            return Err(Refusal::PreviewOnly(major));
        }
    }
    if past_eol(major, today) {
        return Err(Refusal::Eol(major));
    }
    Ok(())
}

/// A move `from → to`: upward only, to a major [`check_target`] accepts.
/// The source only has to be one K2 knows (a database on an EOL major is
/// exactly what should move).
pub fn check_move(from: u32, to: u32, preview: bool, today: i64) -> Result<(), Refusal> {
    if !LAYOUT.contains(&from) {
        return Err(Refusal::Unknown(from));
    }
    if to <= from {
        return Err(Refusal::NotUpward { from, to });
    }
    check_target(to, preview, today)
}

/// The supported majors above `from` a database could move to today.
pub fn upgrades_from(from: u32, today: i64) -> Vec<u32> {
    SUPPORTED
        .iter()
        .copied()
        .filter(|to| check_move(from, *to, false, today).is_ok())
        .collect()
}

/// Strict major parse: 2 digits, no sign, no spaces, inside [`LAYOUT`]'s
/// shape (10–99). Anything else is `None`.
pub fn parse_major(s: &str) -> Option<u32> {
    if s.len() != 2 || !s.bytes().all(|b| b.is_ascii_digit()) || s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

/// Major of a `server_version_num` (`160004` → 16, `170002` → 17).
pub fn major_from_version_num(n: u64) -> Option<u32> {
    if (100_000..1_000_000).contains(&n) {
        Some((n / 10_000) as u32)
    } else {
        None
    }
}

pub fn in_band(port: u16) -> bool {
    (BAND.0..=BAND.1).contains(&port)
}

/// The first port in [`BAND`] not in `taken` (recorded clusters, every
/// `postgresql.conf`, `ss -ltn`). `None` when the band is full: refuse.
pub fn next_port(taken: &[u16]) -> Option<u16> {
    (BAND.0..=BAND.1).find(|p| !taken.contains(p))
}

/// Debian layout of `<major>/main`.
pub fn data_dir(major: u32) -> String {
    format!("/var/lib/postgresql/{major}/{CLUSTER_NAME}")
}
pub fn conf_dir(major: u32) -> String {
    format!("/etc/postgresql/{major}/{CLUSTER_NAME}")
}
pub fn unit(major: u32) -> String {
    format!("postgresql@{major}-{CLUSTER_NAME}.service")
}
pub fn bin_dir(major: u32) -> String {
    format!("/usr/lib/postgresql/{major}/bin")
}

// ── The root helper's side (protocol 9, `namespaces.pgmajor`, S2) ───────
//
// Argv tokens, fixed paths, the canned apt and cluster files, and the
// parsers the helper and the move machine share. Nothing here runs a
// program.

/// A major token on helper argv: exactly one of [`LAYOUT`], as
/// written (`17`, never `017` or `17.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PgMajor(u32);

impl PgMajor {
    pub fn parse(t: &str) -> Option<Self> {
        LAYOUT
            .iter()
            .copied()
            .find(|m| m.to_string() == t)
            .map(Self)
    }
    pub fn new(m: u32) -> Option<Self> {
        LAYOUT.contains(&m).then_some(Self(m))
    }
    pub fn get(self) -> u32 {
        self.0
    }
    pub fn token(self) -> String {
        self.0.to_string()
    }
    /// `/usr/lib/postgresql/<M>/bin/<prog>`.
    pub fn bin(self, prog: &str) -> String {
        format!("{PG_LIB}/{}/bin/{prog}", self.0)
    }
    /// `postgresql@<M>-main`.
    pub fn unit(self) -> String {
        format!("postgresql@{}-main", self.0)
    }
    pub fn data_dir(self) -> String {
        format!("/var/lib/postgresql/{}/main", self.0)
    }
    pub fn etc_dir(self) -> String {
        format!("/etc/postgresql/{}/main", self.0)
    }
}

/// A port token for `ensure-cluster`: canonical decimal inside the band.
pub fn port_token(t: &str) -> Option<u16> {
    let p: u16 = t.parse().ok()?;
    (p.to_string() == t && in_band(p)).then_some(p)
}

/// A move's tag: `YYYYMMDDTHHMMSSZ` (UTC), part of the dump's file name.
pub fn tag_ok(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 16
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z'
}

/// A role K2 may copy between clusters: `[a-z0-9_]{1,63}`, never
/// `postgres` and never a `pg_` built-in.
pub fn role_name_ok(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 63
        && r != "postgres"
        && !r.starts_with("pg_")
        && r.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A stored password the helper may carry into `CREATE ROLE … PASSWORD
/// '<v>'`: a SCRAM verifier (`SCRAM-SHA-256$<iter>:<salt>$<key>:<key>`,
/// base64 parts) or a legacy `md5` + 32 hex. Nothing else can reach the
/// SQL literal.
pub fn verifier_ok(v: &str) -> bool {
    if let Some(hex) = v.strip_prefix("md5") {
        return hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    let Some(rest) = v.strip_prefix("SCRAM-SHA-256$") else {
        return false;
    };
    let b64 = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    };
    let Some((iter_salt, keys)) = rest.split_once('$') else {
        return false;
    };
    let Some((iter, salt)) = iter_salt.split_once(':') else {
        return false;
    };
    let Some((stored, server)) = keys.split_once(':') else {
        return false;
    };
    !iter.is_empty() && iter.len() <= 7 && iter.bytes().all(|b| b.is_ascii_digit()) && b64(salt) && b64(stored) && b64(server)
}

// ── Fixed paths (protocol 9) ────────────────────────────────────────────

pub const PG_LIB: &str = "/usr/lib/postgresql";
pub const PG_LSCLUSTERS: &str = "/usr/bin/pg_lsclusters";
pub const PG_CREATECLUSTER: &str = "/usr/bin/pg_createcluster";
pub const SS: &str = "/usr/bin/ss";
pub const APT_MARK: &str = "/usr/bin/apt-mark";
pub const DPKG_QUERY: &str = "/usr/bin/dpkg-query";
pub const ENV: &str = "/usr/bin/env";
pub const RM: &str = "/usr/bin/rm";
pub const MKDIR: &str = "/usr/bin/mkdir";
pub const STAT: &str = "/usr/bin/stat";
pub const OS_RELEASE: &str = "/etc/os-release";
pub const OS_RELEASE_VENDOR: &str = "/usr/lib/os-release";
/// The compiled-in PGDG key lands here (`Signed-By:` of K2's source).
pub const PGDG_KEYRING: &str = "/etc/apt/keyrings/k2-pgdg.asc";
/// postgresql-common's own copy of the PGDG key; ours must match it.
pub const PGDG_COMMON_KEY: &str = "/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc";
pub const PGDG_SOURCES: &str = "/etc/apt/sources.list.d/k2-pgdg.sources";
/// Sources someone else wrote for PGDG (postgresql-common's script, a
/// hand install): K2 doesn't add a second one (apt refuses two Signed-By
/// values for one URI), its pins still apply.
pub const PGDG_FOREIGN_SOURCES: &[&str] = &[
    "/etc/apt/sources.list.d/pgdg.list",
    "/etc/apt/sources.list.d/pgdg.sources",
    "/etc/apt/sources.list.d/apt_postgresql_org.list",
];
pub const PGDG_PREF: &str = "/etc/apt/preferences.d/k2-pgdg.pref";
/// `create_main_cluster = false`, written before any install so apt never
/// makes and starts a surprise cluster (V9, V22).
pub const CREATECLUSTER_DROPIN: &str = "/etc/postgresql-common/createcluster.d/50-k2.conf";
/// Per K2-created cluster, beside the RAM fence's `99-k2-ram.conf`.
pub const CLUSTER_CONF_FILE: &str = "98-k2-cluster.conf";
/// Every `postgresql@` instance runs in this slice; the cap is for all
/// clusters together (V11, V47).
pub const SLICE_UNIT_PATH: &str = "/etc/systemd/system/k2-postgres.slice";
/// The slice drop-in goes on each K2-created cluster's own instance
/// (`postgresql@<M>-main.service.d`), never on the template: clusters K2
/// doesn't own keep their own slice (review #1 L3).
pub fn instance_slice_dropin(major: u32) -> String {
    format!("/etc/systemd/system/postgresql@{major}-main.service.d/k2-slice.conf")
}
/// unattended-upgrades also takes PGDG's updates (the pins still limit
/// what PGDG may supply), so libpq5 and the PGDG servers keep getting
/// security fixes (review #1 M2).
pub const UNATTENDED_DROPIN: &str = "/etc/apt/apt.conf.d/51k2-pgdg-unattended";

pub fn unattended_dropin() -> &'static str {
    "// K2: security and minor updates from apt.postgresql.org for the Postgres\n\
     // majors K2 installed (pinned by /etc/apt/preferences.d/k2-pgdg.pref).\n\
     // Written by k2-root-helper `pg add-major`.\n\
     Unattended-Upgrade::Origins-Pattern {\n\
     \t\"origin=apt.postgresql.org,codename=${distro_codename}-pgdg\";\n\
     };\n"
}

/// Every psql the helper runs as postgres gets this in its environment:
/// a client option beats `ALTER DATABASE/ROLE … SET search_path`, so a
/// tenant's schema, operator or overload never resolves ahead of
/// pg_catalog in a superuser session (review #2 F1, CVE-2018-1058 class).
/// `role=none` likewise beats `ALTER DATABASE … SET role`, so the session
/// stays postgres (re-check #2 L-a: a tenant role would break the helper
/// install, the seal and the move's own reads).
pub const SAFE_PGOPTIONS: &str = "PGOPTIONS=-c search_path=pg_catalog,pg_temp -c role=none";

/// The oldest source server a move dumps (re-check #1 R2): the
/// CVE-2024-7348 protection (`restrict_nonsystem_relation_kind`) needs
/// the server at 16.4 / 17.0 or later, not only the client tools.
pub fn min_server_num(major: u32) -> u32 {
    match major {
        16 => 160_004,
        m => m * 10_000,
    }
}

/// How many characters of a database name a rollback copy keeps
/// (`<first 46>__k2rb_<10-digit ts>` fits 63).
pub const ROLLBACK_BASE_MAX: usize = 63 - 17;

/// The migrator role Postgres actually has for `db`: `<db>_migrator`, cut
/// at 63 bytes like every identifier.
pub fn migrator_of(db: &str) -> String {
    let mut m = format!("{db}_migrator");
    m.truncate(63);
    m
}

/// The oldest minor of each major's `pg_dump`/`pg_restore` the helper
/// runs (CVE-2024-7348, CVE-2025-8714/8715).
pub fn min_tool_minor(major: u32) -> u32 {
    match major {
        16 => 10,
        17 => 6,
        _ => 0,
    }
}

/// `(major, minor)` from `pg_dump (PostgreSQL) 17.11 (Ubuntu …)`.
pub fn parse_tool_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.split("(PostgreSQL)").nth(1)?.trim();
    let v = rest.split_whitespace().next()?;
    let mut it = v.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().map(|m| m.chars().take_while(char::is_ascii_digit).collect::<String>()).and_then(|m| m.parse().ok())?;
    Some((major, minor))
}

/// `move-restore` stdin: the database owner's login, so the data and
/// post-data sections load as the owner, never as the superuser (review
/// #1 H1 / #2 F2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveLogin {
    pub role: String,
    pub password: String,
}

impl MoveLogin {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !role_name_ok(&self.role) {
            return Err("move-restore: the owner is not a K2 role name");
        }
        if self.password.is_empty() || self.password.len() > 512 || self.password.chars().any(|c| c.is_control()) {
            return Err("move-restore: the owner's password is empty, too long or has control characters");
        }
        Ok(())
    }

    /// One `.pgpass` line for this login on loopback (`:` and `\` escaped).
    pub fn passfile_line(&self, port: u16, db: &str) -> String {
        let esc = |s: &str| s.replace('\\', "\\\\").replace(':', "\\:");
        format!("127.0.0.1:{port}:{}:{}:{}\n", esc(db), esc(&self.role), esc(&self.password))
    }
}

/// Is `datname` a database K2 made, by its owner (review #1 L1)? K2's
/// databases are owned by `<name>_migrator`; a rolled-back copy keeps its
/// owner under `<name>__k2rb_<unix secs>`.
pub fn k2_stamp_ok(datname: &str, owner: &str) -> bool {
    let base = match datname.split_once("__k2rb_") {
        Some((b, ts)) if !ts.is_empty() && ts.bytes().all(|c| c.is_ascii_digit()) => b,
        Some(_) => return false,
        None => return !datname.is_empty() && owner != datname && owner == migrator_of(datname),
    };
    if base.is_empty() {
        return false;
    }
    if base.len() < ROLLBACK_BASE_MAX {
        return owner == migrator_of(base);
    }
    // A rollback copy of a long name keeps only its first 46 characters
    // (re-check #1 R3): the owner is `migrator_of(<base><rest>)` for the
    // original name's unknown rest. So the owner starts with the base, and
    // what follows is name characters ending in `_migrator`, or (cut at
    // 63) in a non-empty prefix of it.
    let Some(tail) = owner.strip_prefix(base) else { return false };
    let name_chars = |s: &str| s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
    if !name_chars(tail) || owner.len() > 63 {
        return false;
    }
    if tail.ends_with("_migrator") {
        return true;
    }
    owner.len() == 63 && (1.."_migrator".len()).any(|k| tail.ends_with(&"_migrator"[..k]))
}

/// Every in-band port Sew's config names (any `*port = N`), so K2 never
/// takes it even while Sew is stopped (review #1 L6).
pub fn sew_ports(toml_text: &str) -> Vec<u16> {
    toml_text
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            if !k.trim().ends_with("port") {
                return None;
            }
            v.trim().trim_matches('"').parse::<u16>().ok()
        })
        .filter(|p| in_band(*p))
        .collect()
}

/// `dpkg-query -W -f '${Package} ${Version} ${Status}\n'` → installed
/// package → version.
pub fn installed_versions(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter(|l| l.ends_with("install ok installed"))
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.to_string(), w.next()?.to_string()))
        })
        .collect()
}

/// Packages whose installed version an install changed or removed
/// (`libpq5` may move: PGDG's 17 needs `libpq5 >= 17`).
pub fn changed_packages(
    before: &std::collections::BTreeMap<String, String>,
    after: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    before
        .iter()
        .filter(|(p, _)| p.as_str() != "libpq5")
        .filter(|(p, v)| after.get(*p) != Some(*v))
        .map(|(p, v)| format!("{p} {v} → {}", after.get(p).map(String::as_str).unwrap_or("removed")))
        .collect()
}
/// Move dumps (postgres-owned, 0700), kept with the source for the
/// rollback window.
pub const MOVES_DIR: &str = "/var/lib/postgresql/k2-moves";
pub const PG_SOCKET_DIR: &str = "/var/run/postgresql";
/// Sew's private Postgres (`sew-db.service`): never a K2 cluster, never
/// counted, never touched (Sew 2026-10-20).
pub const SEW_DB_DATA: &str = "/var/lib/sew/db";
pub const SEW_DB_UNITS: &[&str] = &[
    "/etc/systemd/system/sew-db.service",
    "/usr/lib/systemd/system/sew-db.service",
    "/lib/systemd/system/sew-db.service",
];
pub const SEW_TOML: &str = "/etc/sew/sew.toml";

/// `/var/lib/postgresql/k2-moves/<db>-<tag>.dump`.
pub fn dump_path(db: &str, tag: &str) -> String {
    format!("{MOVES_DIR}/{db}-{tag}.dump")
}

// ── apt: PGDG with a pinned key ─────────────────────────────────────────

/// apt.postgresql.org's signing key (fpr `B97B0AFC…ACCC4CF8`), compiled in.
/// Byte-identical to postgresql-common 257's copy and to
/// `https://www.postgresql.org/media/keys/ACCC4CF8.asc` (checked 2026-10-20).
pub const PGDG_KEY_ASC: &str = include_str!("pgdg-ACCC4CF8.asc");
pub const PGDG_FPR: &str = "B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8";
pub const PGDG_URI: &str = "https://apt.postgresql.org/pub/repos/apt";
pub const PGDG_ORIGIN: &str = "apt.postgresql.org";

/// Distro codenames K2 adds PGDG for, with the distro's own Postgres major.
pub const CODENAMES: &[(&str, u32)] = &[("jammy", 14), ("noble", 16), ("bookworm", 15), ("trixie", 17)];

pub fn distro_major(codename: &str) -> Option<u32> {
    CODENAMES.iter().find(|(c, _)| *c == codename).map(|(_, m)| *m)
}

/// `VERSION_CODENAME=` from `/etc/os-release` (quotes stripped).
pub fn os_release_codename(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        l.strip_prefix("VERSION_CODENAME=")
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|v| !v.is_empty())
    })
}

/// The deb822 source for `codename` (allow-listed), signed by K2's keyring.
pub fn pgdg_sources(codename: &str) -> Option<String> {
    distro_major(codename)?;
    Some(format!(
        "# K2: apt.postgresql.org for the Postgres majors K2 runs side by side.\n\
         # Written by k2-root-helper `pg add-major`; pinned by {PGDG_PREF}.\n\
         Types: deb\n\
         URIs: {PGDG_URI}\n\
         Suites: {codename}-pgdg\n\
         Components: main\n\
         Signed-By: {PGDG_KEYRING}\n"
    ))
}

/// The package names `add-major` installs for `major`.
pub fn packages(major: u32) -> [String; 2] {
    [format!("postgresql-{major}"), format!("postgresql-client-{major}")]
}

/// The pins (V21, drill on Ubuntu 24.04 2026-10-20):
/// - everything from PGDG at 100: it never replaces the distro's Postgres,
///   `postgresql-common` or anything else on `apt upgrade`;
/// - at 500: the server and client of each installable major the distro
///   doesn't ship, and `libpq5` (PGDG's 17 needs `libpq5 (>= 17~~)`;
///   libpq is backward compatible, so the distro's 16 keeps working);
/// - the metas at -1: never a surprise "latest" major.
pub fn pgdg_pref(codename: &str) -> Option<String> {
    let distro = distro_major(codename)?;
    let mut five: Vec<String> = vec!["libpq5".into()];
    for m in LAYOUT.iter().copied().filter(|m| *m != distro) {
        five.extend(packages(m));
    }
    Some(format!(
        "# K2: PGDG may only provide the Postgres majors K2 asked for.\n\
         # Written by k2-root-helper `pg add-major`.\n\
         Package: *\n\
         Pin: origin {PGDG_ORIGIN}\n\
         Pin-Priority: 100\n\
         \n\
         Package: {}\n\
         Pin: origin {PGDG_ORIGIN}\n\
         Pin-Priority: 500\n\
         \n\
         Package: postgresql postgresql-client postgresql-contrib postgresql-all postgresql-doc\n\
         Pin: origin {PGDG_ORIGIN}\n\
         Pin-Priority: -1\n",
        five.join(" ")
    ))
}

pub fn createcluster_dropin() -> &'static str {
    "# K2: never create (and start) a cluster when a postgresql-<n> package\n\
     # is installed; K2 creates clusters itself (`pg ensure-cluster`).\n\
     create_main_cluster = false\n"
}

/// `conf.d/98-k2-cluster.conf` of a K2-created cluster (§4).
pub fn cluster_conf() -> &'static str {
    "# K2 cluster settings. Written by k2-root-helper `pg ensure-cluster`.\n\
     listen_addresses = 'localhost'\n\
     password_encryption = 'scram-sha-256'\n\
     wal_level = logical\n"
}

/// The shared slice: every K2 cluster together under one cap (V47).
pub fn slice_unit() -> &'static str {
    "# K2: all Postgres clusters together. Written by k2-root-helper.\n\
     [Unit]\n\
     Description=K2 Postgres clusters (shared memory cap)\n\
     Before=slices.target\n\
     \n\
     [Slice]\n\
     MemoryAccounting=yes\n\
     MemoryHigh=25%\n\
     MemoryMax=40%\n\
     CPUWeight=50\n"
}

pub fn slice_dropin() -> &'static str {
    "# K2: every postgresql@ instance in the shared slice.\n\
     [Service]\n\
     Slice=k2-postgres.slice\n"
}

// ── Parsers ─────────────────────────────────────────────────────────────

/// One `pg_lsclusters -h` row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRow {
    pub major: u32,
    pub name: String,
    pub port: u16,
    /// `online`, `down`, `online,recovery`, …
    pub status: String,
    pub owner: String,
    pub data_dir: String,
}

impl ClusterRow {
    pub fn online(&self) -> bool {
        self.status.split(',').next() == Some("online")
    }
}

/// `pg_lsclusters -h`: `Ver Cluster Port Status Owner DataDir LogFile`.
/// Rows that don't parse are skipped; a data dir under Sew's tree is
/// never a cluster (Sew 2026-10-20), nor is anything outside
/// `/var/lib/postgresql`.
pub fn parse_lsclusters(text: &str) -> Vec<ClusterRow> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 6 {
                return None;
            }
            let major: u32 = f[0].parse().ok()?;
            let port: u16 = f[2].parse().ok()?;
            let data_dir = f[5].to_string();
            if data_dir.starts_with("/var/lib/sew/") || data_dir == SEW_DB_DATA {
                return None;
            }
            Some(ClusterRow {
                major,
                name: f[1].to_string(),
                port,
                status: f[3].to_string(),
                owner: f[4].to_string(),
                data_dir,
            })
        })
        .collect()
}

/// Is this row a cluster K2 may use (`<M>/main` under the Debian layout)?
/// Non-`main` clusters and data dirs outside `/var/lib/postgresql` are
/// always refused (§10).
pub fn k2_layout(row: &ClusterRow) -> bool {
    row.name == "main" && row.data_dir == format!("/var/lib/postgresql/{}/main", row.major)
}

/// Every `:<port>` some socket listens on, from `ss -ltnH`.
pub fn listening_ports(ss: &str) -> Vec<u16> {
    ss.lines()
        .filter_map(|l| {
            let local = l.split_whitespace().nth(3)?;
            local.rsplit_once(':')?.1.parse().ok()
        })
        .collect()
}

/// `pg_major = <n>` in Sew's `/etc/sew/sew.toml` (any table).
pub fn sew_pg_major(toml_text: &str) -> Option<u32> {
    toml_text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("pg_major")?.trim_start();
        let v = rest.strip_prefix('=')?.trim().trim_matches('"');
        v.parse().ok()
    })
}

/// An `apt-get install -s` answer K2 refuses (`Some(why)`): any removal,
/// and any version change of a Postgres server package already installed
/// (a downgrade, or PGDG replacing the distro's build). Sew's own
/// Postgres runs the installed `postgresql-<pg_major>` binaries, so the
/// refusal names Sew when it pins that major.
pub fn apt_sim_refusal(sim: &str, sew_major: Option<u32>) -> Option<String> {
    for line in sim.lines() {
        let mut w = line.split_whitespace();
        let (Some(op), Some(pkg)) = (w.next(), w.next()) else { continue };
        let name = pkg.split(':').next().unwrap_or(pkg);
        // The major a package belongs to: postgresql-16, postgresql-client-16,
        // postgresql-16-pgvector…
        let server_major = name
            .strip_prefix("postgresql-client-")
            .or_else(|| name.strip_prefix("postgresql-"))
            .and_then(|m| m.split('-').next())
            .and_then(|m| m.parse::<u32>().ok());
        // Every Postgres package an install must leave as it is (review #1
        // M1): servers, clients, extensions, postgresql-common.
        let guarded = server_major.is_some() || matches!(name, "postgresql-common" | "postgresql-client-common");
        let sew_note = |m: Option<u32>| match (m, sew_major) {
            (Some(a), Some(b)) if a == b => format!(" (Sew's own Postgres runs postgresql-{a})"),
            _ => String::new(),
        };
        match op {
            "Remv" | "Purg" => {
                return Some(format!(
                    "apt_would_remove: the install would remove {name}{}; K2 never removes packages (nothing changed)",
                    sew_note(server_major)
                ))
            }
            // `Inst postgresql-16 [16.15-…] (16.16-…pgdg…)`: an installed
            // version in brackets = a version change of an installed package.
            "Inst" if guarded && w.next().is_some_and(|t| t.starts_with('[')) => {
                return Some(format!(
                    "apt_would_change_installed: the install would replace the installed {name}{}; K2 never changes an installed Postgres package (nothing changed)",
                    sew_note(server_major)
                ))
            }
            _ => {}
        }
    }
    None
}

/// `copy-roles` stdin: the roles to recreate in the target cluster with
/// the source's stored password and attributes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRolesRequest {
    pub roles: Vec<String>,
}

pub const MAX_COPY_ROLES: usize = 256;

impl CopyRolesRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.roles.is_empty() || self.roles.len() > MAX_COPY_ROLES {
            return Err("copy-roles: 1 to 256 roles");
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.roles {
            if !role_name_ok(r) {
                return Err("copy-roles: a role name is not a K2 role name");
            }
            if !seen.insert(r.as_str()) {
                return Err("copy-roles: a role is listed twice");
            }
        }
        Ok(())
    }
}

/// One role as `pg_authid` holds it (the `copy-roles` read).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct AuthRow {
    pub rolname: String,
    pub rolsuper: bool,
    pub rolinherit: bool,
    pub rolcreaterole: bool,
    pub rolcreatedb: bool,
    pub rolcanlogin: bool,
    pub rolreplication: bool,
    pub rolbypassrls: bool,
    pub rolconnlimit: i32,
    pub rolpassword: Option<String>,
}

impl AuthRow {
    /// Same attributes and stored password (what "already there" means).
    pub fn same_as(&self, o: &AuthRow) -> bool {
        self == o
    }

    /// `CREATE ROLE` for the target. `None` when the row must not be
    /// copied (superuser, replication, BYPASSRLS, an unsafe stored
    /// password, a non-K2 name).
    pub fn create_sql(&self) -> Option<String> {
        if !role_name_ok(&self.rolname) || self.rolsuper || self.rolreplication || self.rolbypassrls {
            return None;
        }
        let yes = |b: bool, y: &'static str, n: &'static str| if b { y } else { n };
        let mut s = format!(
            "CREATE ROLE \"{}\" {} {} {} {} NOSUPERUSER NOREPLICATION NOBYPASSRLS CONNECTION LIMIT {}",
            self.rolname,
            yes(self.rolcanlogin, "LOGIN", "NOLOGIN"),
            yes(self.rolinherit, "INHERIT", "NOINHERIT"),
            yes(self.rolcreaterole, "CREATEROLE", "NOCREATEROLE"),
            yes(self.rolcreatedb, "CREATEDB", "NOCREATEDB"),
            self.rolconnlimit,
        );
        match &self.rolpassword {
            Some(v) if verifier_ok(v) => {
                s.push_str(" PASSWORD '");
                s.push_str(v);
                s.push('\'');
            }
            Some(_) => return None,
            None => s.push_str(" PASSWORD NULL"),
        }
        s.push_str(";\n");
        Some(s)
    }
}

/// The `pg_authid` read for `roles` (validated names only): one JSON
/// array on one line.
pub fn authid_select_sql(roles: &[String]) -> String {
    let list: Vec<String> = roles.iter().map(|r| format!("'{r}'")).collect();
    format!(
        "SELECT coalesce(json_agg(json_build_object(\
         'rolname', rolname, 'rolsuper', rolsuper, 'rolinherit', rolinherit, \
         'rolcreaterole', rolcreaterole, 'rolcreatedb', rolcreatedb, \
         'rolcanlogin', rolcanlogin, 'rolreplication', rolreplication, \
         'rolbypassrls', rolbypassrls, 'rolconnlimit', rolconnlimit, \
         'rolpassword', rolpassword) ORDER BY rolname), '[]'::json) \
         FROM pg_authid WHERE rolname IN ({});\n",
        list.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> i64 {
        days_from_civil(y, m, d)
    }

    #[test]
    fn civil_days_match_known_dates() {
        assert_eq!(day(1970, 1, 1), 0);
        assert_eq!(day(2000, 3, 1), 11_017);
        assert_eq!(day(2026, 10, 20), 20_746);
    }

    #[test]
    fn policy_table_is_the_0463_one() {
        assert_eq!(SUPPORTED, &[16, 17]);
        assert!(PREVIEW.is_empty(), "no 18 preview in 0.46.3");
        assert_eq!(NEW_INSTALL, 17);
        assert!(SUPPORTED.contains(&NEW_INSTALL));
        assert!(LAYOUT.contains(&18), "18 is accepted by the layout");
        for m in LAYOUT {
            assert!(eol(*m).is_some(), "every layout major has an EOL date: {m}");
        }
    }

    #[test]
    fn targets_on_2026_10_20() {
        let t = day(2026, 10, 20);
        assert_eq!(check_target(16, false, t), Ok(()));
        assert_eq!(check_target(17, false, t), Ok(()));
        assert_eq!(check_target(18, false, t), Err(Refusal::Unsupported(18)));
        assert_eq!(check_target(18, true, t), Err(Refusal::Unsupported(18)), "no preview list in 0.46.3");
        assert_eq!(check_target(15, false, t), Err(Refusal::Unknown(15)));
        assert_eq!(check_target(19, false, t), Err(Refusal::Unknown(19)));
        assert_eq!(check_target(0, false, t), Err(Refusal::Unknown(0)));
    }

    #[test]
    fn eol_refuses_after_the_date_not_on_it() {
        assert_eq!(check_target(16, false, day(2028, 11, 9)), Ok(()));
        assert_eq!(check_target(16, false, day(2028, 11, 10)), Err(Refusal::Eol(16)));
        assert_eq!(Refusal::Eol(16).code(), "major_eol");
        assert!(Refusal::Eol(16).hint().contains("2028-11-09"), "{}", Refusal::Eol(16).hint());
        assert!(past_eol(99, 0), "unknown majors fail closed");
    }

    #[test]
    fn moves_go_upward_only() {
        let t = day(2026, 10, 20);
        assert_eq!(check_move(16, 17, false, t), Ok(()));
        assert_eq!(check_move(17, 16, false, t), Err(Refusal::NotUpward { from: 17, to: 16 }));
        assert_eq!(check_move(17, 17, false, t), Err(Refusal::NotUpward { from: 17, to: 17 }));
        assert_eq!(check_move(16, 18, false, t), Err(Refusal::Unsupported(18)));
        assert_eq!(check_move(14, 17, false, t), Err(Refusal::Unknown(14)));
        assert_eq!(upgrades_from(16, t), vec![17]);
        assert!(upgrades_from(17, t).is_empty());
        // A source past its EOL may still move (that is the point).
        assert_eq!(check_move(16, 17, false, day(2029, 1, 1)), Ok(()));
    }

    #[test]
    fn parse_major_is_strict() {
        assert_eq!(parse_major("16"), Some(16));
        assert_eq!(parse_major("17"), Some(17));
        for bad in ["", "1", "017", "+17", " 17", "17 ", "1a", "170", "07", "-1"] {
            assert_eq!(parse_major(bad), None, "{bad:?}");
        }
        assert_eq!(major_from_version_num(160_004), Some(16));
        assert_eq!(major_from_version_num(170_002), Some(17));
        assert_eq!(major_from_version_num(90_624), None);
    }

    #[test]
    fn port_band() {
        assert!(in_band(5432) && in_band(5433) && in_band(5449));
        assert!(!in_band(5431) && !in_band(5450) && !in_band(0));
        assert_eq!(next_port(&[]), Some(5432));
        assert_eq!(next_port(&[5432]), Some(5433));
        assert_eq!(next_port(&[5432, 5433, 5435]), Some(5434));
        let full: Vec<u16> = (5432..=5449).collect();
        assert_eq!(next_port(&full), None, "a full band refuses");
        assert_eq!(BAND.1 - BAND.0 + 1, 18);
    }

    #[test]
    fn layout_paths() {
        assert_eq!(data_dir(17), "/var/lib/postgresql/17/main");
        assert_eq!(conf_dir(17), "/etc/postgresql/17/main");
        assert_eq!(unit(16), "postgresql@16-main.service");
        assert_eq!(bin_dir(17), "/usr/lib/postgresql/17/bin");
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(PgMajor::parse("17").map(PgMajor::get), Some(17));
        for bad in ["", "017", "17.1", "15", "19", "-17", "17 ", "1e1"] {
            assert!(PgMajor::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(PgMajor::parse("17").unwrap().bin("pg_dump"), "/usr/lib/postgresql/17/bin/pg_dump");
        assert_eq!(PgMajor::parse("16").unwrap().unit(), "postgresql@16-main");
        assert_eq!(port_token("5433"), Some(5433));
        assert_eq!(port_token("5449"), Some(5449));
        for bad in ["5431", "5450", "05433", "22", "5433;", ""] {
            assert!(port_token(bad).is_none(), "{bad:?}");
        }
        assert!(tag_ok("20261020T101500Z"));
        for bad in ["2026-10-20", "20261020T101500", "20261020t101500z", "20261020T1015000Z", "../../etc/xZ"] {
            assert!(!tag_ok(bad), "{bad:?}");
        }
        for ok in ["ws_jobs_agent", "jobs_migrator", "a", &"a".repeat(63)] {
            assert!(role_name_ok(ok), "{ok}");
        }
        for bad in ["postgres", "pg_read_all_data", "Jobs", "a-b", "a\"b", "", &"a".repeat(64)] {
            assert!(!role_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn verifiers() {
        let v = "SCRAM-SHA-256$4096:c2FsdHNhbHQ=$c3RvcmVka2V5c3RvcmVka2V5:c2VydmVya2V5c2VydmVya2V5";
        assert!(verifier_ok(v));
        assert!(verifier_ok("md50123456789abcdef0123456789abcdef"));
        for bad in [
            "",
            "hunter2",
            "SCRAM-SHA-256$4096:salt$a:b'; DROP ROLE x; --",
            "SCRAM-SHA-256$4096:salt$a",
            "SCRAM-SHA-256$x:salt$a:b",
            "md5xyz",
            "SCRAM-SHA-256$4096:sa lt$a:b",
        ] {
            assert!(!verifier_ok(bad), "{bad:?}");
        }
    }

    fn row(name: &str) -> AuthRow {
        AuthRow {
            rolname: name.into(),
            rolsuper: false,
            rolinherit: false,
            rolcreaterole: false,
            rolcreatedb: false,
            rolcanlogin: true,
            rolreplication: false,
            rolbypassrls: false,
            rolconnlimit: -1,
            rolpassword: Some("SCRAM-SHA-256$4096:c2FsdA==$a2V5:c2Vy".into()),
        }
    }

    #[test]
    fn create_role_sql_keeps_the_verifier_and_refuses_dangerous_rows() {
        let s = row("jobs_agent").create_sql().unwrap();
        assert_eq!(
            s,
            "CREATE ROLE \"jobs_agent\" LOGIN NOINHERIT NOCREATEROLE NOCREATEDB NOSUPERUSER NOREPLICATION NOBYPASSRLS CONNECTION LIMIT -1 PASSWORD 'SCRAM-SHA-256$4096:c2FsdA==$a2V5:c2Vy';\n"
        );
        let mut nl = row("jobs_bind");
        nl.rolcanlogin = false;
        nl.rolpassword = None;
        assert!(nl.create_sql().unwrap().contains("NOLOGIN") && nl.create_sql().unwrap().ends_with("PASSWORD NULL;\n"));
        for f in [
            |r: &mut AuthRow| r.rolsuper = true,
            |r: &mut AuthRow| r.rolreplication = true,
            |r: &mut AuthRow| r.rolbypassrls = true,
            |r: &mut AuthRow| r.rolpassword = Some("plain".into()),
            |r: &mut AuthRow| r.rolname = "postgres".into(),
        ] {
            let mut r = row("x");
            f(&mut r);
            assert!(r.create_sql().is_none(), "{r:?}");
        }
        let req = CopyRolesRequest { roles: vec!["a".into(), "a".into()] };
        assert!(req.validate().is_err());
        let req = CopyRolesRequest { roles: vec!["a".into(), "pg_monitor".into()] };
        assert!(req.validate().is_err());
        assert!(CopyRolesRequest { roles: vec![] }.validate().is_err());
        assert!(CopyRolesRequest { roles: vec!["jobs_agent".into(), "ws_x_agent".into()] }.validate().is_ok());
        assert!(serde_json::from_str::<CopyRolesRequest>(r#"{"roles":["a"],"sql":"x"}"#).is_err());
        let sel = authid_select_sql(&["a".into(), "b_c".into()]);
        assert!(sel.contains("WHERE rolname IN ('a', 'b_c')"));
    }

    #[test]
    fn apt_files() {
        assert!(PGDG_KEY_ASC.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"));
        assert!(PGDG_KEY_ASC.trim_end().ends_with("-----END PGP PUBLIC KEY BLOCK-----"));
        let src = pgdg_sources("noble").unwrap();
        assert!(src.contains("\nSuites: noble-pgdg\n"));
        assert!(src.contains("\nSigned-By: /etc/apt/keyrings/k2-pgdg.asc\n"));
        assert!(src.contains("\nURIs: https://apt.postgresql.org/pub/repos/apt\n"));
        assert!(pgdg_sources("sid").is_none());
        assert!(pgdg_sources("noble\nURIs: http://evil").is_none());
        let pref = pgdg_pref("noble").unwrap();
        assert!(pref.contains("Package: *\nPin: origin apt.postgresql.org\nPin-Priority: 100\n"));
        assert!(
            pref.contains("Package: libpq5 postgresql-17 postgresql-client-17 postgresql-18 postgresql-client-18\nPin: origin apt.postgresql.org\nPin-Priority: 500\n"),
            "noble's own 16 never comes from PGDG: {pref}"
        );
        assert!(!pref.contains("postgresql-16"), "{pref}");
        assert!(pref.contains("Package: postgresql postgresql-client postgresql-contrib postgresql-all postgresql-doc\nPin: origin apt.postgresql.org\nPin-Priority: -1\n"));
        assert!(!pref.contains("postgresql-common"), "postgresql-common stays the distro's");
        let trixie = pgdg_pref("trixie").unwrap();
        assert!(trixie.contains("postgresql-16 postgresql-client-16") && !trixie.contains("postgresql-17 "));
        assert_eq!(os_release_codename("NAME=\"Ubuntu\"\nVERSION_CODENAME=noble\n"), Some("noble".into()));
        assert_eq!(os_release_codename("VERSION_CODENAME=\"bookworm\"\n"), Some("bookworm".into()));
        assert_eq!(os_release_codename("ID=arch\n"), None);
        assert!(createcluster_dropin().contains("\ncreate_main_cluster = false\n"));
        assert!(cluster_conf().contains("listen_addresses = 'localhost'"));
        assert!(cluster_conf().contains("wal_level = logical"));
        assert!(cluster_conf().contains("password_encryption = 'scram-sha-256'"));
        assert!(slice_unit().contains("\nMemoryHigh=25%\nMemoryMax=40%\n"));
        assert!(slice_dropin().contains("\nSlice=k2-postgres.slice\n"));
    }

    #[test]
    fn lsclusters_skips_sew_and_foreign_layouts() {
        let t = "16  main    5432 online postgres /var/lib/postgresql/16/main /var/log/postgresql/postgresql-16-main.log\n\
                 17  main    5433 down   postgres /var/lib/postgresql/17/main /var/log/postgresql/postgresql-17-main.log\n\
                 16  sew     5499 online sew-db   /var/lib/sew/db /var/log/x.log\n\
                 16  other   5434 online postgres /srv/pg /var/log/y.log\n\
                 garbage\n";
        let rows = parse_lsclusters(t);
        assert_eq!(rows.len(), 3, "Sew's data dir is never a cluster: {rows:?}");
        assert!(rows.iter().all(|r| !r.data_dir.starts_with("/var/lib/sew")));
        assert!(rows[0].online() && !rows[1].online());
        assert_eq!(rows[1].port, 5433);
        assert!(k2_layout(&rows[0]) && k2_layout(&rows[1]));
        assert!(!k2_layout(&rows[2]), "non-main / outside /var/lib/postgresql is refused");
        let ss = "LISTEN 0 200 127.0.0.1:5432 0.0.0.0:*\nLISTEN 0 4096 [::1]:5433 [::]:*\nLISTEN 0 4096 *:22 *:*\n";
        assert_eq!(listening_ports(ss), vec![5432, 5433, 22]);
        assert_eq!(sew_pg_major("[db]\npg_major = 16\n"), Some(16));
        assert_eq!(sew_pg_major("pg_major=\"17\"\n"), Some(17));
        assert_eq!(sew_pg_major("[db]\nport = 1\n"), None);
    }

    /// Sew 2026-10-20 (1): no K2 install path may remove or change the
    /// installed Postgres server Sew runs.
    #[test]
    fn apt_sim_never_removes_or_changes_an_installed_server() {
        let ok = "Inst libpq5 [16.15-0ubuntu0.24.04.1] (18.6-1.pgdg24.04+2 PostgreSQL)\nInst postgresql-17 (17.11-1.pgdg24.04+2 PostgreSQL)\nConf postgresql-17\n";
        assert_eq!(apt_sim_refusal(ok, Some(16)), None);
        let remove = "Remv postgresql-16 [16.15-0ubuntu0.24.04.1]\nInst postgresql-17 (17.11)\n";
        let why = apt_sim_refusal(remove, Some(16)).unwrap();
        assert!(why.starts_with("apt_would_remove") && why.contains("Sew"), "{why}");
        let why = apt_sim_refusal(remove, None).unwrap();
        assert!(!why.contains("Sew"));
        let change = "Inst postgresql-16 [16.15-0ubuntu0.24.04.1] (16.11-1.pgdg24.04+1 PostgreSQL)\n";
        let why = apt_sim_refusal(change, Some(16)).unwrap();
        assert!(why.starts_with("apt_would_change_installed") && why.contains("Sew"), "{why}");
        assert!(apt_sim_refusal("Purg postgresql-client-16\n", None).is_some());
    }

    #[test]
    fn review_fixes_pure_parts() {
        assert_eq!(SAFE_PGOPTIONS, "PGOPTIONS=-c search_path=pg_catalog,pg_temp -c role=none");
        assert_eq!((min_server_num(16), min_server_num(17), min_server_num(18)), (160_004, 170_000, 180_000));
        assert_eq!(instance_slice_dropin(17), "/etc/systemd/system/postgresql@17-main.service.d/k2-slice.conf");
        assert!(unattended_dropin().contains("\"origin=apt.postgresql.org,codename=${distro_codename}-pgdg\";"));
        assert_eq!(parse_tool_version("pg_dump (PostgreSQL) 17.11 (Ubuntu 17.11-1.pgdg24.04+2)"), Some((17, 11)));
        assert_eq!(parse_tool_version("pg_restore (PostgreSQL) 16.15 (Ubuntu 16.15-0ubuntu0.24.04.1)\n"), Some((16, 15)));
        assert_eq!(parse_tool_version("pg_dump (PostgreSQL) 18.0"), Some((18, 0)));
        assert_eq!(parse_tool_version("nonsense"), None);
        assert_eq!((min_tool_minor(16), min_tool_minor(17), min_tool_minor(18)), (10, 6, 0));
        let l = MoveLogin { role: "ws_x_migrator".into(), password: "a:b\\c".into() };
        assert!(l.validate().is_ok());
        assert_eq!(l.passfile_line(5433, "ws_x"), "127.0.0.1:5433:ws_x:ws_x_migrator:a\\:b\\\\c\n");
        for bad in [
            MoveLogin { role: "postgres".into(), password: "x".into() },
            MoveLogin { role: "ws_x_migrator".into(), password: "".into() },
            MoveLogin { role: "ws_x_migrator".into(), password: "a\nb".into() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<MoveLogin>(r#"{"role":"a","password":"b","x":1}"#).is_err());
        assert!(k2_stamp_ok("ws_jobs", "ws_jobs_migrator"));
        assert!(k2_stamp_ok("ws_jobs__k2rb_1792541220", "ws_jobs_migrator"));
        assert!(!k2_stamp_ok("ws_jobs", "postgres"));
        assert!(!k2_stamp_ok("customer_db", "alice"));
        assert!(!k2_stamp_ok("ws_jobs__k2rb_x", "ws_jobs_migrator"));
        assert_eq!(sew_ports("[db]\nport = 5440\n[server]\nhttp_port = 8470\ndb_port=\"5433\"\n"), vec![5440, 5433]);
        let before = installed_versions("postgresql-16 16.15-0u install ok installed\nlibpq5 16.15 install ok installed\npostgresql-common 257 install ok installed\nfoo 1 deinstall ok config-files\n");
        assert_eq!(before.len(), 3);
        let mut after = before.clone();
        after.insert("libpq5".into(), "18.6".into());
        after.insert("postgresql-17".into(), "17.11".into());
        assert!(changed_packages(&before, &after).is_empty(), "libpq5 may move; new packages are not changes");
        after.remove("postgresql-16");
        after.insert("postgresql-common".into(), "282".into());
        let c = changed_packages(&before, &after);
        assert_eq!(c, vec!["postgresql-16 16.15-0u → removed".to_string(), "postgresql-common 257 → 282".to_string()]);
    }

    /// Review #1 M1: clients, extensions and postgresql-common are guarded too.
    #[test]
    fn apt_sim_guards_every_postgres_package() {
        for line in [
            "Inst postgresql-client-16 [16.15-0ubuntu0.24.04.1] (16.11-1.pgdg24.04+1)",
            "Inst postgresql-16-pgvector [0.6.0-1] (0.8.0-1.pgdg24.04+1)",
            "Inst postgresql-common [257build1.1] (282.pgdg24.04+1)",
            "Inst postgresql-client-common [257build1.1] (282.pgdg24.04+1)",
        ] {
            let why = apt_sim_refusal(line, Some(16)).unwrap_or_else(|| panic!("{line}"));
            assert!(why.starts_with("apt_would_change_installed"), "{why}");
        }
        assert!(apt_sim_refusal("Inst libpq5 [16.15] (18.6-1.pgdg24.04+2)", Some(16)).is_none());
        assert!(apt_sim_refusal("Inst postgresql-client-17 (17.11-1.pgdg24.04+2)", Some(16)).is_none());
    }

    /// Re-check #1 R3: a rollback copy keeps the first 46 characters, so a
    /// long name's copy must still carry the K2 stamp (close, drop, roll
    /// back). Every length from 1 to 62, a 60-character name spelled out.
    #[test]
    fn the_stamp_holds_for_long_names_and_their_rollback_copies() {
        let ts = "1792560309";
        let cut = |s: String| -> String { s.chars().take(63).collect() };
        for len in 1..=62usize {
            let db: String = "ws_".chars().chain(std::iter::repeat('a')).take(len).collect();
            let owner = migrator_of(&db);
            assert!(owner.len() <= 63);
            assert!(k2_stamp_ok(&db, &owner), "{len}: {db} / {owner}");
            let base: String = db.chars().take(ROLLBACK_BASE_MAX).collect();
            let copy = format!("{base}__k2rb_{ts}");
            assert!(copy.len() <= 63, "{copy}");
            assert!(k2_stamp_ok(&copy, &owner), "{len}: {copy} / {owner}");
            assert!(!k2_stamp_ok(&copy, "postgres"));
            assert!(!k2_stamp_ok(&copy, &migrator_of(&format!("x{db}"))), "{len}");
            if len < 62 {
                // (At 62 the agent and migrator names both cut to `<db>_`.)
                assert!(!k2_stamp_ok(&copy, &cut(format!("{db}_agent"))), "{len}");
                assert!(!k2_stamp_ok(&db, &cut(format!("{db}_agent"))), "{len}");
            }
        }
        // 63: the role would be the name itself; never K2's.
        let db63: String = "ws_".chars().chain(std::iter::repeat('a')).take(63).collect();
        assert!(!k2_stamp_ok(&db63, &migrator_of(&db63)));
        let db60 = format!("ws_{}", "k".repeat(57));
        assert_eq!(db60.len(), 60);
        assert_eq!(migrator_of(&db60), format!("{db60}_mi"));
        let copy = format!("{}__k2rb_{ts}", &db60[..ROLLBACK_BASE_MAX]);
        assert_eq!(copy.len(), 63);
        assert!(k2_stamp_ok(&db60, &migrator_of(&db60)));
        assert!(k2_stamp_ok(&copy, &migrator_of(&db60)));
        assert!(
            !k2_stamp_ok(&copy, &format!("{}zzzzzzzzzzzzzzzzz", &db60[..ROLLBACK_BASE_MAX])),
            "a 63-character role that doesn't end in a prefix of _migrator"
        );
        // 54 characters: the longest whose migrator isn't cut.
        let db54 = format!("ws_{}", "q".repeat(51));
        let copy = format!("{}__k2rb_{ts}", &db54[..ROLLBACK_BASE_MAX]);
        assert!(k2_stamp_ok(&copy, &format!("{db54}_migrator")));
        assert!(!k2_stamp_ok(&copy, &format!("{db54}_migratorx")));
    }

    /// Re-check #1 I3: the bundled PGDG key is the one with the pinned
    /// fingerprint (v4: SHA-1 over 0x99, the 2-byte length and the primary
    /// public-key packet).
    #[test]
    fn the_pgdg_key_file_has_the_pinned_fingerprint() {
        use base64::Engine;
        use sha1::Digest;
        let mut b64 = String::new();
        let mut in_body = false;
        for line in PGDG_KEY_ASC.lines() {
            let line = line.trim_end();
            if line.starts_with("-----BEGIN") {
                continue;
            }
            if !in_body {
                if line.is_empty() {
                    in_body = true;
                }
                continue;
            }
            if line.starts_with('=') || line.starts_with("-----END") {
                break;
            }
            b64.push_str(line);
        }
        let raw = base64::engine::general_purpose::STANDARD.decode(b64).expect("armored base64");
        let body: &[u8] = match raw[0] {
            // old-format packet, tag 6, 1/2/4-byte length
            0x98 => &raw[2..2 + raw[1] as usize],
            0x99 => &raw[3..3 + u16::from_be_bytes([raw[1], raw[2]]) as usize],
            0x9a => &raw[5..5 + u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]) as usize],
            // new-format packet, tag 6
            0xc6 => {
                let l0 = raw[1] as usize;
                if l0 < 192 {
                    &raw[2..2 + l0]
                } else if l0 < 224 {
                    &raw[3..3 + ((l0 - 192) << 8) + raw[2] as usize + 192]
                } else {
                    &raw[6..6 + u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]) as usize]
                }
            }
            other => panic!("the first packet is not a public key: {other:#x}"),
        };
        assert_eq!(body[0], 4, "a v4 key");
        let mut h = sha1::Sha1::new();
        h.update([0x99]);
        h.update((body.len() as u16).to_be_bytes());
        h.update(body);
        let fpr: String = h.finalize().iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(fpr, PGDG_FPR);
    }
}
