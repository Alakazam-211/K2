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
