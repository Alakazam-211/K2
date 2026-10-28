//! Endgame Stage B (0.46.6): the home database moves from `k2so.db` to
//! `k2.db`, once, at boot, before anything opens it.
//!
//! ## What it does
//!
//! [`switch_home_db`] runs in the daemon after the singleton lock and before
//! [`super::init_database`] opens the file. It leaves `~/.k2` in one shape:
//!
//! - `k2.db` is the real database;
//! - `k2so.db` is a **relative symlink** to `k2.db` (unix), so old daemons
//!   and outside readers that still name `k2so.db` reach the same file;
//! - `db-renamed.json` records when and how it happened.
//!
//! ## Why the symlink is safe
//!
//! SQLite's unix VFS resolves every symlink in the path before it names the
//! `-wal` / `-shm` files (`appendOnePathElement`, bundled 3.45 and the
//! macOS / Linux system builds): a connection opened as `k2so.db` uses
//! `k2.db-wal` and `k2.db-shm`, so it shares the WAL index with the daemon
//! and never sees a torn or stale page. [`tests::symlink_reader_shares_wal`]
//! proves it for the bundled build. A **hard link would not be safe** (it
//! keeps its own name, so its own WAL), so none is ever made. Windows gets
//! no compat link: every Windows daemon (0.40.9x on) already prefers `k2.db`.
//!
//! ## The rename
//!
//! 1. Open `k2so.db` and switch it to `journal_mode=DELETE`. That folds the
//!    WAL into the main file and deletes `-wal` / `-shm`, and SQLite only
//!    allows it when this is the **only** connection: anyone else holding the
//!    file (a draining predecessor, a stray tool) makes it fail, and the
//!    switch is deferred to a later boot with nothing changed.
//! 2. `BEGIN EXCLUSIVE`: no other process can read or write while the file
//!    moves (in DELETE mode this is a file lock; no journal is created until
//!    a page is written, and nothing is).
//! 3. `rename(k2so.db, k2.db)` (atomic, same directory), fsync the directory,
//!    then the `k2so.db -> k2.db` symlink, fsync again, then the marker.
//!    The next open sets WAL again ([`super::open_with_resilience`]).
//!
//! Every crash point leaves a state the next boot finishes: before the
//! rename nothing moved; after it `k2.db` is real and alone, so the link and
//! marker are added; after the link only the marker is missing.
//!
//! ## Two real files
//!
//! The stub `k2.db` left by Stage A (0.40.66) sits beside a live `k2so.db`
//! on several Macs. Today's pick ([`super::resolve_home_db_path`]) decides
//! which file is in use; the **other** one is moved to `db-quarantine/`
//! (never deleted) when it holds no live workspace. When both hold live
//! workspaces the switch **refuses**: nothing moves, the daemon keeps
//! today's pick, and `db-rename-blocked.json` says what to do.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// The database's name from 0.46.6 on.
pub const NEW_NAME: &str = "k2.db";
/// The legacy name; after the switch a symlink to [`NEW_NAME`] (unix).
pub const OLD_NAME: &str = "k2so.db";
/// Written once the home is on `k2.db`.
pub const MARKER: &str = "db-renamed.json";
/// Written while both files hold live workspaces; removed once resolved.
pub const BLOCKED_NOTE: &str = "db-rename-blocked.json";
/// Where a stale file is moved (never deleted).
pub const QUARANTINE_DIR: &str = "db-quarantine";

const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// What the boot switch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Neither file existed: a fresh install, created as `k2.db`.
    Fresh,
    /// The home was already on `k2.db` (maybe finishing a crashed switch).
    AlreadyDone,
    /// `k2so.db` became `k2.db` this boot.
    Renamed,
    /// `k2.db` was already the live file; the stale `k2so.db` was
    /// quarantined and replaced by the link.
    KeptK2Db,
    /// Nothing changed this boot; today's pick is used. The reason says why.
    Deferred { reason: String },
    /// Both files hold live workspaces: refused, nothing moved.
    Blocked { k2_projects: i64, k2so_projects: i64 },
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Fresh => "fresh",
            Outcome::AlreadyDone => "already",
            Outcome::Renamed => "renamed",
            Outcome::KeptK2Db => "kept-k2.db",
            Outcome::Deferred { .. } => "deferred",
            Outcome::Blocked { .. } => "blocked",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwitchReport {
    /// The file to open.
    pub path: PathBuf,
    pub outcome: Outcome,
    /// Files moved into `db-quarantine/` this boot.
    pub quarantined: Vec<PathBuf>,
    /// Warnings worth a log line.
    pub notes: Vec<String>,
}

impl SwitchReport {
    /// One log line for the daemon.
    pub fn log_line(&self) -> String {
        let mut s = format!(
            "[db] home switch: {} → {}",
            self.outcome.label(),
            self.path.display()
        );
        match &self.outcome {
            Outcome::Deferred { reason } => s.push_str(&format!(" (deferred: {reason})")),
            Outcome::Blocked { k2_projects, k2so_projects } => s.push_str(&format!(
                " (REFUSED: k2.db has {k2_projects} workspace(s), k2so.db has {k2so_projects}; \
                 see {BLOCKED_NOTE})"
            )),
            _ => {}
        }
        for q in &self.quarantined {
            s.push_str(&format!("; quarantined {}", q.display()));
        }
        for n in &self.notes {
            s.push_str(&format!("; {n}"));
        }
        s
    }
}

/// How to run the switch.
#[derive(Debug, Clone, Default)]
pub struct SwitchOpts {
    /// `Some(reason)`: change nothing this boot (a hand-off successor whose
    /// predecessor still holds the DB open; `K2_ALLOW_MULTI_DAEMON`).
    pub defer: Option<String>,
    /// Who did it, for the marker (`k2-daemon 0.46.6`).
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Absent,
    Real,
    /// A symlink; `true` when it points at `k2.db` in the same directory.
    Link { to_new: bool },
    /// A directory, a socket, …
    Other,
}

fn kind(path: &Path) -> Kind {
    match std::fs::symlink_metadata(path) {
        Err(_) => Kind::Absent,
        Ok(m) if m.file_type().is_symlink() => {
            let to_new = std::fs::read_link(path)
                .map(|t| t == Path::new(NEW_NAME) || t == path.with_file_name(NEW_NAME))
                .unwrap_or(false);
            Kind::Link { to_new }
        }
        Ok(m) if m.is_file() => Kind::Real,
        Ok(_) => Kind::Other,
    }
}

/// What a probe of a candidate database found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    /// Zero bytes, or not an SQLite file at all.
    NotSqlite,
    /// SQLite, but no `projects` table (`sqlite3 ~/.k2/k2.db` and quit).
    NoProjects,
    /// Live (non-sentinel) workspace rows.
    Count(i64),
    /// Could not tell (locked, corrupt, unreadable): never moved.
    Unknown(String),
}

fn probe(path: &Path) -> Probe {
    use std::io::Read;
    let mut head = [0u8; 16];
    let n = match std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) {
        Ok(n) => n,
        Err(e) => return Probe::Unknown(format!("read {}: {e}", path.display())),
    };
    if n == 0 {
        // An empty file SQLite would treat as a new DB; its -wal could
        // still hold pages, so only a lone empty file is NotSqlite.
        let wal_len = std::fs::metadata(crate::private_home::sibling(path, "-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
        return if wal_len == 0 {
            Probe::NotSqlite
        } else {
            Probe::Unknown("empty file with a non-empty -wal".into())
        };
    }
    if n < 16 || &head != b"SQLite format 3\0" {
        return Probe::NotSqlite;
    }
    let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => return Probe::Unknown(format!("open {}: {e}", path.display())),
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(2000));
    let has: Result<i64, _> = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
        [],
        |r| r.get(0),
    );
    match has {
        Err(e) => return Probe::Unknown(format!("read {}: {e}", path.display())),
        Ok(0) => return Probe::NoProjects,
        Ok(_) => {}
    }
    match conn.query_row(
        "SELECT count(*) FROM projects WHERE id NOT IN ('_orphan', '_broadcast')",
        [],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(c) => Probe::Count(c),
        Err(e) => Probe::Unknown(format!("count {}: {e}", path.display())),
    }
}

/// Test hook: `K2_TEST_DB_SWITCH_CRASH=<point>` aborts the process at that
/// point (debug builds only; compiled out of release builds).
fn crash_point(_point: &str) {
    #[cfg(debug_assertions)]
    if std::env::var("K2_TEST_DB_SWITCH_CRASH").ok().as_deref() == Some(_point) {
        eprintln!("[db] K2_TEST_DB_SWITCH_CRASH: aborting at {_point}");
        std::process::abort();
    }
}

fn fsync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn utc_stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Write `name` in `dir` as owner-only JSON, atomically.
fn write_json(dir: &Path, name: &str, v: &serde_json::Value) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let body = serde_json::to_vec_pretty(v).map_err(std::io::Error::other)?;
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&body)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(name))?;
    fsync_dir(dir);
    Ok(())
}

/// Move `file` and its `-wal` / `-shm` / `-journal` into
/// `db-quarantine/<name>.<stamp>` (kept as a set, so the copy still opens).
fn quarantine(dir: &Path, file: &Path) -> std::io::Result<PathBuf> {
    let qdir = dir.join(QUARANTINE_DIR);
    std::fs::create_dir_all(&qdir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&qdir, std::fs::Permissions::from_mode(0o700))?;
    }
    let name = file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "db".into());
    let stamp = utc_stamp();
    let mut dest = qdir.join(format!("{name}.{stamp}"));
    let mut i = 1;
    while std::fs::symlink_metadata(&dest).is_ok() {
        dest = qdir.join(format!("{name}.{stamp}-{i}"));
        i += 1;
    }
    std::fs::rename(file, &dest)?;
    for s in SIDECARS {
        let from = crate::private_home::sibling(file, s);
        if std::fs::symlink_metadata(&from).is_ok() {
            std::fs::rename(&from, crate::private_home::sibling(&dest, s))?;
        }
    }
    fsync_dir(&qdir);
    fsync_dir(dir);
    Ok(dest)
}

/// Move side files named for `k2.db` that have no `k2.db` beside them (a
/// quarantined stub's leftovers, a tool's stray WAL): SQLite would read a
/// stray `k2.db-wal` as the renamed file's own WAL.
fn clear_orphan_sidecars(dir: &Path, rep: &mut SwitchReport) -> std::io::Result<()> {
    let new = dir.join(NEW_NAME);
    for s in SIDECARS {
        let side = crate::private_home::sibling(&new, s);
        if std::fs::symlink_metadata(&side).is_ok() {
            let dest = quarantine(dir, &side)?;
            rep.quarantined.push(dest);
        }
    }
    Ok(())
}

/// `k2so.db -> k2.db` (relative). Unix only.
fn ensure_link(dir: &Path, rep: &mut SwitchReport) {
    let old = dir.join(OLD_NAME);
    match kind(&old) {
        Kind::Absent => {
            #[cfg(unix)]
            {
                if let Err(e) = std::os::unix::fs::symlink(NEW_NAME, &old) {
                    rep.notes.push(format!("compat link {OLD_NAME} -> {NEW_NAME} not made: {e}"));
                } else {
                    fsync_dir(dir);
                }
            }
        }
        Kind::Link { to_new: true } => {}
        Kind::Link { to_new: false } => rep
            .notes
            .push(format!("{OLD_NAME} is a symlink that does not point at {NEW_NAME}; left alone")),
        Kind::Real | Kind::Other => rep
            .notes
            .push(format!("{OLD_NAME} exists and is not a link; left alone")),
    }
}

fn ensure_marker(dir: &Path, by: &str, rep: &mut SwitchReport, force: bool) {
    if !force && dir.join(MARKER).exists() {
        return;
    }
    let link = match kind(&dir.join(OLD_NAME)) {
        Kind::Link { to_new: true } => format!("{OLD_NAME} -> {NEW_NAME}"),
        _ => "none".to_string(),
    };
    let v = serde_json::json!({
        "version": 1,
        "from": OLD_NAME,
        "to": NEW_NAME,
        "outcome": rep.outcome.label(),
        "at": chrono::Utc::now().to_rfc3339(),
        "by": by,
        "link": link,
        "quarantined": rep.quarantined.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    });
    if let Err(e) = write_json(dir, MARKER, &v) {
        rep.notes.push(format!("{MARKER} not written: {e}"));
    }
    // A switch that went through clears an earlier refusal.
    let _ = std::fs::remove_file(dir.join(BLOCKED_NOTE));
}

#[derive(Debug)]
enum RenameErr {
    /// Someone else has the file open, or it is mid-write: try a later boot.
    Busy(String),
    Io(String),
}

/// Steps 1–3 of the module docs. `old` is a real file, `new` is absent and
/// has no side files.
fn rename_legacy(dir: &Path, rep: &mut SwitchReport) -> Result<(), RenameErr> {
    let old = dir.join(OLD_NAME);
    let new = dir.join(NEW_NAME);
    let conn = Connection::open_with_flags(&old, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| RenameErr::Busy(format!("open {OLD_NAME}: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_millis(2000))
        .map_err(|e| RenameErr::Io(format!("busy_timeout: {e}")))?;
    // Only the sole connection may leave WAL mode.
    let mode: String = conn
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .map_err(|e| RenameErr::Busy(format!("{OLD_NAME} is in use (journal_mode): {e}")))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(RenameErr::Busy(format!(
            "{OLD_NAME} is in use by another connection (journal_mode stayed {mode})"
        )));
    }
    conn.execute_batch("BEGIN EXCLUSIVE")
        .map_err(|e| RenameErr::Busy(format!("{OLD_NAME} is in use (exclusive lock): {e}")))?;
    let wal = crate::private_home::sibling(&old, "-wal");
    let wal_len = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    if wal_len > 0 {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(RenameErr::Busy(format!(
            "{OLD_NAME}-wal still holds {wal_len} bytes after the checkpoint"
        )));
    }
    let journal = crate::private_home::sibling(&old, "-journal");
    if std::fs::metadata(&journal).map(|m| m.len() > 0).unwrap_or(false) {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(RenameErr::Busy(format!("{OLD_NAME}-journal is not empty")));
    }
    crash_point("before_rename");
    std::fs::rename(&old, &new).map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK");
        RenameErr::Io(format!("rename {OLD_NAME} -> {NEW_NAME}: {e}"))
    })?;
    fsync_dir(dir);
    crash_point("after_rename");
    // Empty leftovers under the old name (SQLite removes them on the mode
    // change; a zero-length one can remain if another reader made it).
    for s in SIDECARS {
        let side = crate::private_home::sibling(&old, s);
        if std::fs::metadata(&side).map(|m| m.len() == 0).unwrap_or(false) {
            let _ = std::fs::remove_file(&side);
        }
    }
    ensure_link(dir, rep);
    crash_point("after_link");
    let _ = conn.execute_batch("ROLLBACK");
    drop(conn);
    Ok(())
}

/// The boot switch. Never deletes a database; on any doubt it changes
/// nothing and returns today's pick.
pub fn switch_home_db(dir: &Path, opts: &SwitchOpts) -> SwitchReport {
    let new = dir.join(NEW_NAME);
    let old = dir.join(OLD_NAME);
    let mut rep = SwitchReport {
        path: super::resolve_home_db_path_quiet(dir),
        outcome: Outcome::AlreadyDone,
        quarantined: Vec::new(),
        notes: Vec::new(),
    };
    if let Some(reason) = &opts.defer {
        rep.outcome = Outcome::Deferred { reason: reason.clone() };
        return rep;
    }
    let defer = |mut rep: SwitchReport, reason: String| {
        rep.path = super::resolve_home_db_path(dir);
        rep.outcome = Outcome::Deferred { reason };
        rep
    };
    match (kind(&new), kind(&old)) {
        // Fresh install (or a link left with no file behind it).
        (Kind::Absent, Kind::Absent) | (Kind::Absent, Kind::Link { to_new: true }) => {
            if let Err(e) = clear_orphan_sidecars(dir, &mut rep) {
                return defer(rep, format!("stray {NEW_NAME} side files not moved: {e}"));
            }
            rep.path = new;
            rep.outcome = Outcome::Fresh;
            ensure_link(dir, &mut rep);
            ensure_marker(dir, &opts.by, &mut rep, true);
            rep
        }
        // Already switched (finishes a crash after the rename or the link).
        (Kind::Real, Kind::Absent) | (Kind::Real, Kind::Link { .. }) => {
            rep.path = new;
            rep.outcome = Outcome::AlreadyDone;
            ensure_link(dir, &mut rep);
            ensure_marker(dir, &opts.by, &mut rep, false);
            rep
        }
        // The legacy file alone: rename it.
        (Kind::Absent, Kind::Real) => do_rename(dir, opts, rep),
        // The Stage A stub beside a live file (or two live files).
        (Kind::Real, Kind::Real) => {
            let pick = super::pick_dual_real_db(&new, &old);
            let other = if pick == new { old.clone() } else { new.clone() };
            let (pick_probe, other_probe) = (probe(&pick), probe(&other));
            match &other_probe {
                Probe::Count(n) if *n > 0 => {
                    let (k2, k2so) = if pick == new {
                        (count_of(&pick_probe), *n)
                    } else {
                        (*n, count_of(&pick_probe))
                    };
                    rep.path = pick.clone();
                    rep.outcome = Outcome::Blocked { k2_projects: k2, k2so_projects: k2so };
                    write_blocked(dir, &pick, k2, k2so, &mut rep);
                    return rep;
                }
                Probe::Unknown(why) => {
                    return defer(rep, format!("cannot read {}: {why}", other.display()));
                }
                _ => {}
            }
            if let Probe::Unknown(why) = &pick_probe {
                return defer(rep, format!("cannot read {}: {why}", pick.display()));
            }
            match quarantine(dir, &other) {
                Ok(dest) => rep.quarantined.push(dest),
                Err(e) => return defer(rep, format!("quarantine {}: {e}", other.display())),
            }
            if pick == new {
                rep.path = new;
                rep.outcome = Outcome::KeptK2Db;
                ensure_link(dir, &mut rep);
                ensure_marker(dir, &opts.by, &mut rep, true);
                rep
            } else {
                do_rename(dir, opts, rep)
            }
        }
        (n, o) => defer(
            rep,
            format!("unexpected shape: {NEW_NAME} is {n:?}, {OLD_NAME} is {o:?}; nothing moved"),
        ),
    }
}

fn count_of(p: &Probe) -> i64 {
    match p {
        Probe::Count(n) => *n,
        _ => 0,
    }
}

fn do_rename(dir: &Path, opts: &SwitchOpts, mut rep: SwitchReport) -> SwitchReport {
    if let Err(e) = clear_orphan_sidecars(dir, &mut rep) {
        rep.path = super::resolve_home_db_path(dir);
        rep.outcome = Outcome::Deferred {
            reason: format!("stray {NEW_NAME} side files not moved: {e}"),
        };
        return rep;
    }
    match rename_legacy(dir, &mut rep) {
        Ok(()) => {
            rep.path = dir.join(NEW_NAME);
            rep.outcome = Outcome::Renamed;
            ensure_marker(dir, &opts.by, &mut rep, true);
        }
        Err(RenameErr::Busy(reason)) | Err(RenameErr::Io(reason)) => {
            rep.path = super::resolve_home_db_path(dir);
            rep.outcome = Outcome::Deferred { reason };
        }
    }
    rep
}

/// The owner-facing text for a refused switch.
pub fn blocked_message(dir: &Path, using: &Path, k2: i64, k2so: i64) -> String {
    format!(
        "K2 found two databases that both hold workspaces: {} ({k2so} workspace(s)) and {} \
         ({k2} workspace(s)). It did not rename or move either one, and it is still using {} \
         (the same file as before). To finish the move to k2.db: stop K2, move the copy you \
         don't want out of {} (keep it as a backup; don't delete it), then start K2 again.",
        dir.join(OLD_NAME).display(),
        dir.join(NEW_NAME).display(),
        using.display(),
        dir.display(),
    )
}

fn write_blocked(dir: &Path, using: &Path, k2: i64, k2so: i64, rep: &mut SwitchReport) {
    let prev: Option<serde_json::Value> = std::fs::read(dir.join(BLOCKED_NOTE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let notice_filed = prev
        .as_ref()
        .and_then(|v| v.get("noticeFiled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let first_at = prev
        .as_ref()
        .and_then(|v| v.get("firstAt"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!(chrono::Utc::now().to_rfc3339()));
    let v = serde_json::json!({
        "version": 1,
        "firstAt": first_at,
        "lastAt": chrono::Utc::now().to_rfc3339(),
        "using": using.display().to_string(),
        "k2dbWorkspaces": k2,
        "k2sodbWorkspaces": k2so,
        "noticeFiled": notice_filed,
        "message": blocked_message(dir, using, k2, k2so),
    });
    if let Err(e) = write_json(dir, BLOCKED_NOTE, &v) {
        rep.notes.push(format!("{BLOCKED_NOTE} not written: {e}"));
    }
}

/// True once per refusal: the caller files an owner notice, then calls
/// [`mark_blocked_notice_filed`].
pub fn blocked_notice_due(dir: &Path) -> bool {
    std::fs::read(dir.join(BLOCKED_NOTE))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .map(|v| !v.get("noticeFiled").and_then(|f| f.as_bool()).unwrap_or(false))
        .unwrap_or(false)
}

pub fn mark_blocked_notice_filed(dir: &Path) {
    let Some(mut v) = std::fs::read(dir.join(BLOCKED_NOTE))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
    else {
        return;
    };
    v["noticeFiled"] = serde_json::json!(true);
    let _ = write_json(dir, BLOCKED_NOTE, &v);
}

#[cfg(test)]
mod tests;
