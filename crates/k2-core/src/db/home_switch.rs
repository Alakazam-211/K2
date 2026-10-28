//! Endgame Stage B (0.46.6, `prd-k2-db-rename-v1`): the home database
//! moves from `k2so.db` to `k2.db`, once, at boot, before anything opens it.
//!
//! ## What it does
//!
//! [`switch_home_db`] runs in the daemon after the singleton lock and before
//! [`super::init_home_database`] opens the file. It leaves `~/.k2` in one
//! shape:
//!
//! - `k2.db` is the real database;
//! - `k2so.db` is a **relative symlink** to `k2.db` (unix), so old daemons
//!   and outside readers that still name `k2so.db` reach the same file;
//! - `db-renamed.json` records when and how it happened.
//!
//! ## Why the symlink is safe
//!
//! SQLite's unix VFS resolves every symlink in the path before it names the
//! `-wal` / `-shm` / `-journal` files (`appendOnePathElement`; bundled 3.45,
//! the macOS and Linux system builds, 3.10.0 on): a connection opened as
//! `k2so.db` uses `k2.db-wal` and `k2.db-shm`, so it shares the WAL index
//! with the daemon and never sees a torn or stale page.
//! [`tests::symlink_reader_shares_wal`] proves it for the bundled build. A
//! **hard link is never left behind** (it keeps its own name, so its own
//! WAL). Windows gets no rename in 0.46.6 (DB-A14).
//!
//! ## The rename (old file alone)
//!
//! 1. Open `k2so.db` and switch it to `journal_mode=DELETE`. That folds the
//!    WAL into the main file and deletes `-wal` / `-shm`, and SQLite only
//!    allows it when this is the **only** connection: anyone else holding the
//!    file (a draining predecessor, a stray tool) makes it fail, and the
//!    switch is deferred to a later boot with nothing changed.
//! 2. `BEGIN EXCLUSIVE`: no other process can read or write while the file
//!    moves (in DELETE mode this is a file lock; no journal is created until
//!    a page is written, and nothing is).
//! 3. `link(k2so.db, k2.db)`: a second name for the same inode, then fsync.
//! 4. A tmp symlink `.k2so.db.link-tmp -> k2.db`, then `rename` over
//!    `k2so.db` (atomic): at every instant `k2so.db` names the database, so
//!    no reader can mint an empty file there (DB-A1). fsync again.
//! 5. `ROLLBACK`, close, write the marker. The next open sets WAL again.
//!
//! A filesystem without hard links falls back to `rename` + the link (the
//! branch's first shape); a file a reader made in that short gap is healed
//! the same boot (DB-A2).
//!
//! Every crash point leaves a state the next boot finishes: before step 3
//! nothing moved; after it both names are the same inode, which is finished
//! (WAL folded under the old name first, then the link); after step 4 only
//! the marker is missing.
//!
//! ## Two real files
//!
//! The stub `k2.db` left by Stage A (0.40.66) sits beside a live `k2so.db`
//! on several Macs. Today's pick ([`super::resolve_home_db_path`]) decides
//! which file is in use; the **other** one is moved to `db-quarantine/`
//! (never deleted) when it holds no live workspace. When both hold live
//! workspaces the switch **refuses**: nothing moves, the daemon keeps
//! today's pick, and `db-rename-blocked.json` says what to do. The owner's
//! way out is `k2 db home --keep k2.db|k2so.db` (or the Thread card): it
//! writes `db-switch-choice.json`, pinned to the two files' inodes, and the
//! next boot keeps that file and quarantines the other (DB-A6).
//!
//! ## Off switches
//!
//! `~/.k2/db-switch-hold` (any content) defers the switch on that box
//! (DB-A5). A hand-off successor and `K2_ALLOW_MULTI_DAEMON` defer too.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rusqlite::{Connection, OpenFlags};

/// The database's name from 0.46.6 on.
pub const NEW_NAME: &str = "k2.db";
/// The legacy name; after the switch a symlink to [`NEW_NAME`] (unix).
pub const OLD_NAME: &str = "k2so.db";
/// Written once the home is on `k2.db`.
pub const MARKER: &str = "db-renamed.json";
/// Written while the switch refuses (two live files, a hot old-name
/// journal); removed once resolved.
pub const BLOCKED_NOTE: &str = "db-rename-blocked.json";
/// Where a stale file is moved (never deleted).
pub const QUARANTINE_DIR: &str = "db-quarantine";
/// The owner's hold: while it exists the switch changes nothing (DB-A5).
pub const HOLD_FILE: &str = "db-switch-hold";
/// The owner's choice between two live files (DB-A6).
pub const CHOICE_FILE: &str = "db-switch-choice.json";
/// The tmp name the compat link is made under before its atomic rename.
const LINK_TMP: &str = ".k2so.db.link-tmp";

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
    /// A non-empty `k2so.db-journal` sits beside the link (DB-A15): an
    /// outside writer crashed mid-write under the old name. The daemon
    /// must not open the database until a person looks.
    HotJournal { journal: PathBuf, bytes: u64 },
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
            Outcome::HotJournal { .. } => "hot-journal",
        }
    }

    /// The why, for boot-status and the log (`None` when nothing to say).
    pub fn reason(&self) -> Option<String> {
        match self {
            Outcome::Deferred { reason } => Some(reason.clone()),
            Outcome::Blocked { k2_projects, k2so_projects } => Some(format!(
                "both files hold workspaces (k2.db {k2_projects}, k2so.db {k2so_projects}); \
                 k2 db home --keep k2.db|k2so.db"
            )),
            Outcome::HotJournal { journal, bytes } => Some(format!(
                "{} holds {bytes} bytes of an unfinished write; K2 won't open the database",
                journal.display()
            )),
            _ => None,
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
    /// The owner's choice this boot applied (DB-A6), as recorded.
    pub choice: Option<serde_json::Value>,
}

impl SwitchReport {
    fn new(path: PathBuf) -> Self {
        Self { path, outcome: Outcome::AlreadyDone, quarantined: Vec::new(), notes: Vec::new(), choice: None }
    }

    /// One log line for the daemon.
    pub fn log_line(&self) -> String {
        let mut s = format!("[db] home switch: {} → {}", self.outcome.label(), self.path.display());
        if let Some(r) = self.outcome.reason() {
            s.push_str(&format!(" ({r})"));
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

// ── this boot's report (boot-status `homeDb`, DB-A4) ──────────────────

static LAST: OnceLock<(PathBuf, SwitchReport)> = OnceLock::new();

/// Remember this boot's report (first call wins; one boot per process).
pub fn remember(dir: &Path, rep: &SwitchReport) {
    let _ = LAST.set((dir.to_path_buf(), rep.clone()));
}

/// This boot's report, once the database was initialized.
pub fn last_report() -> Option<&'static SwitchReport> {
    LAST.get().map(|(_, r)| r)
}

/// boot-status `homeDb` (DB-A4): `{file, outcome, reason, markerAt}`, or
/// `null` before the database was initialized (tests). `file` is the file
/// name the daemon opened (`k2.db` / `k2so.db`), never a full path.
pub fn status_json() -> serde_json::Value {
    let Some((dir, rep)) = LAST.get() else { return serde_json::Value::Null };
    status_json_for(dir, rep)
}

pub fn status_json_for(dir: &Path, rep: &SwitchReport) -> serde_json::Value {
    let marker_at = read_json(&dir.join(MARKER)).and_then(|v| v.get("at").cloned());
    serde_json::json!({
        "file": rep.path.file_name().map(|s| s.to_string_lossy().to_string()),
        "outcome": rep.outcome.label(),
        "reason": rep.outcome.reason(),
        "markerAt": marker_at,
    })
}

// ── file shapes ───────────────────────────────────────────────────────

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

/// `(dev, inode)` of a path, not following a final symlink.
#[cfg(unix)]
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}
#[cfg(not(unix))]
fn file_id(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Both names are the same file: a switch that crashed between its hard
/// link and its link swap.
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((file_id(a), file_id(b)), (Some(x), Some(y)) if x == y)
}

fn len_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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

impl Probe {
    fn live(&self) -> bool {
        matches!(self, Probe::Count(n) if *n > 0)
    }
    fn count(&self) -> i64 {
        match self {
            Probe::Count(n) => *n,
            _ => 0,
        }
    }
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
        return if len_of(&crate::private_home::sibling(path, "-wal")) == 0 {
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

// ── small helpers ─────────────────────────────────────────────────────

/// Test hook: `K2_TEST_DB_SWITCH_CRASH=<point>` aborts the process at that
/// point (debug builds only; compiled out of release builds). Points:
/// `before_rename`, `after_rename` (both names are the database), and
/// `after_link` (the link is in place, no marker yet).
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

fn read_json(path: &Path) -> Option<serde_json::Value> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
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

#[cfg(unix)]
fn chmod_600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn chmod_600(_path: &Path) {}

/// Move `file` and its `-wal` / `-shm` / `-journal` into
/// `db-quarantine/<stamp>-<name>` (kept as a set, so the copy still opens;
/// the name still ends in `.db`, so the permission sweep sees it, and each
/// moved file is 0600: DB-A7). Never deletes anything.
fn quarantine(dir: &Path, file: &Path) -> std::io::Result<PathBuf> {
    let qdir = dir.join(QUARANTINE_DIR);
    std::fs::create_dir_all(&qdir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&qdir, std::fs::Permissions::from_mode(0o700))?;
    }
    let name = file.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "db".into());
    let stamp = utc_stamp();
    let mut dest = qdir.join(format!("{stamp}-{name}"));
    let mut i = 1;
    while std::fs::symlink_metadata(&dest).is_ok() {
        dest = qdir.join(format!("{stamp}-{i}-{name}"));
        i += 1;
    }
    std::fs::rename(file, &dest)?;
    chmod_600(&dest);
    for s in SIDECARS {
        let from = crate::private_home::sibling(file, s);
        if std::fs::symlink_metadata(&from).is_ok() {
            let to = crate::private_home::sibling(&dest, s);
            std::fs::rename(&from, &to)?;
            chmod_600(&to);
        }
    }
    fsync_dir(&qdir);
    fsync_dir(dir);
    Ok(dest)
}

/// Move side files that belong to no database here: `k2.db-*` with no
/// `k2.db` beside them (SQLite would read a stray `k2.db-wal` as the
/// renamed file's own WAL), and `k2so.db-wal` / `-shm` / an empty
/// `-journal` beside the link or nothing (DB-A3: SQLite ignores them, people
/// and tools don't). A non-empty `k2so.db-journal` is never moved here: see
/// [`hot_old_journal`].
fn clear_orphan_sidecars(dir: &Path, rep: &mut SwitchReport) -> std::io::Result<()> {
    let new = dir.join(NEW_NAME);
    if kind(&new) == Kind::Absent {
        for s in SIDECARS {
            let side = crate::private_home::sibling(&new, s);
            if std::fs::symlink_metadata(&side).is_ok() {
                rep.quarantined.push(quarantine(dir, &side)?);
            }
        }
    }
    let old = dir.join(OLD_NAME);
    if matches!(kind(&old), Kind::Absent | Kind::Link { .. }) {
        for s in SIDECARS {
            let side = crate::private_home::sibling(&old, s);
            if std::fs::symlink_metadata(&side).is_err() {
                continue;
            }
            if s == "-journal" && len_of(&side) > 0 {
                continue;
            }
            rep.quarantined.push(quarantine(dir, &side)?);
        }
    }
    Ok(())
}

/// DB-A15: a non-empty `k2so.db-journal` beside the link (or beside
/// nothing) while `k2.db` is the real file. Only an outside writer that
/// slipped in under the old name and crashed mid-write leaves one; SQLite
/// opening `k2.db` would not see it.
fn hot_old_journal(dir: &Path) -> Option<(PathBuf, u64)> {
    let old = dir.join(OLD_NAME);
    if !matches!(kind(&old), Kind::Absent | Kind::Link { .. }) {
        return None;
    }
    let j = crate::private_home::sibling(&old, "-journal");
    let n = len_of(&j);
    (kind(&j) == Kind::Real && n > 0).then_some((j, n))
}

/// Make `k2so.db -> k2.db` (relative) without a moment where `k2so.db`
/// names nothing (DB-A1): the link is made under a tmp name and renamed
/// into place. It replaces only what is safe to replace: nothing, a link,
/// or a second name of `k2.db` itself. Unix only.
#[cfg(unix)]
fn swap_in_link(dir: &Path) -> std::io::Result<()> {
    let old = dir.join(OLD_NAME);
    let tmp = dir.join(LINK_TMP);
    if matches!(kind(&tmp), Kind::Link { .. }) {
        std::fs::remove_file(&tmp)?;
    }
    std::os::unix::fs::symlink(NEW_NAME, &tmp)?;
    if let Err(e) = std::fs::rename(&tmp, &old) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    fsync_dir(dir);
    Ok(())
}

/// Leave `k2so.db` as the compat link (unix), healing what the switch may
/// find there: a stray file nobody's database (DB-A2: a reader that minted
/// an empty `k2so.db`) goes to quarantine first; a live one stays and is
/// noted (the next boot's two-file rule decides).
fn ensure_link(dir: &Path, rep: &mut SwitchReport) {
    #[cfg(unix)]
    {
        let old = dir.join(OLD_NAME);
        let new = dir.join(NEW_NAME);
        match kind(&old) {
            Kind::Link { to_new: true } => return,
            Kind::Link { to_new: false } => {
                rep.notes.push(format!("{OLD_NAME} is a symlink that does not point at {NEW_NAME}; left alone"));
                return;
            }
            Kind::Other => {
                rep.notes.push(format!("{OLD_NAME} is not a file or a link; left alone"));
                return;
            }
            Kind::Real if same_file(&old, &new) => {}
            Kind::Real => match probe(&old) {
                Probe::NotSqlite | Probe::NoProjects | Probe::Count(0) => match quarantine(dir, &old) {
                    Ok(dest) => rep.quarantined.push(dest),
                    Err(e) => {
                        rep.notes.push(format!("stray {OLD_NAME} not quarantined: {e}"));
                        return;
                    }
                },
                p => {
                    rep.notes.push(format!("{OLD_NAME} is a real file ({p:?}); left alone for the next boot"));
                    return;
                }
            },
            Kind::Absent => {}
        }
        if let Err(e) = swap_in_link(dir) {
            rep.notes.push(format!("compat link {OLD_NAME} -> {NEW_NAME} not made: {e}"));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, rep);
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
    let mut v = serde_json::json!({
        "version": 1,
        "from": OLD_NAME,
        "to": NEW_NAME,
        "outcome": rep.outcome.label(),
        "at": chrono::Utc::now().to_rfc3339(),
        "by": by,
        "link": link,
        "quarantined": rep.quarantined.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    });
    if let Some(c) = &rep.choice {
        v["choice"] = c.clone();
    }
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

/// Steps 1–2: open `path` (an existing file, never created), fold its WAL
/// in (only the sole connection may leave WAL mode) and take the exclusive
/// lock. The side files under `path`'s name must be empty afterwards.
fn fold_and_lock(path: &Path) -> Result<Connection, RenameErr> {
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| RenameErr::Busy(format!("open {name}: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_millis(2000))
        .map_err(|e| RenameErr::Io(format!("busy_timeout: {e}")))?;
    let mode: String = conn
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .map_err(|e| RenameErr::Busy(format!("{name} is in use (journal_mode): {e}")))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(RenameErr::Busy(format!("{name} is in use by another connection (journal_mode stayed {mode})")));
    }
    conn.execute_batch("BEGIN EXCLUSIVE")
        .map_err(|e| RenameErr::Busy(format!("{name} is in use (exclusive lock): {e}")))?;
    for s in ["-wal", "-journal"] {
        let n = len_of(&crate::private_home::sibling(path, s));
        if n > 0 {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(RenameErr::Busy(format!("{name}{s} still holds {n} bytes after the checkpoint")));
        }
    }
    Ok(conn)
}

/// Empty leftovers under the old name (SQLite removes them on the mode
/// change; a zero-length one can remain if another reader made it).
fn drop_empty_old_sidecars(dir: &Path) {
    let old = dir.join(OLD_NAME);
    for s in SIDECARS {
        let side = crate::private_home::sibling(&old, s);
        if kind(&side) == Kind::Real && len_of(&side) == 0 {
            let _ = std::fs::remove_file(&side);
        }
    }
}

/// The rename: `k2so.db` real, `k2.db` absent with no side files.
fn rename_legacy(dir: &Path, rep: &mut SwitchReport) -> Result<(), RenameErr> {
    let old = dir.join(OLD_NAME);
    let new = dir.join(NEW_NAME);
    let conn = fold_and_lock(&old)?;
    crash_point("before_rename");
    // Step 3: a second name for the same inode (no gap where `k2so.db` is
    // missing). A filesystem without hard links gets a plain rename.
    let hard = cfg!(unix) && std::fs::hard_link(&old, &new).is_ok();
    if !hard {
        if let Err(e) = std::fs::rename(&old, &new) {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(RenameErr::Io(format!("rename {OLD_NAME} -> {NEW_NAME}: {e}")));
        }
        rep.notes.push("no hard links here: renamed, then linked".into());
    }
    fsync_dir(dir);
    crash_point("after_rename");
    drop_empty_old_sidecars(dir);
    // Step 4: the link replaces the old name atomically.
    ensure_link(dir, rep);
    crash_point("after_link");
    let _ = conn.execute_batch("ROLLBACK");
    drop(conn);
    if hard && kind(&old) != (Kind::Link { to_new: true }) {
        // The swap failed and both names still point at one inode. Leaving
        // that would give an old reader its own WAL: put the single name
        // back and try again next boot.
        let _ = std::fs::remove_file(&new);
        return Err(RenameErr::Io("the compat link could not replace k2so.db".into()));
    }
    Ok(())
}

/// A crash between the hard link and the link swap left both names on
/// one inode. A downgraded daemon may have opened it as `k2so.db` since
/// (its WAL then sits under that name), so fold it in under that name
/// first, then swap the link in.
fn finish_hard_link(dir: &Path, rep: &mut SwitchReport) -> Result<(), RenameErr> {
    let old = dir.join(OLD_NAME);
    let new = dir.join(NEW_NAME);
    for s in ["-wal", "-journal"] {
        let n = len_of(&crate::private_home::sibling(&new, s));
        if n > 0 {
            return Err(RenameErr::Busy(format!("{NEW_NAME}{s} holds {n} bytes while both names are one file")));
        }
    }
    let conn = fold_and_lock(&old)?;
    drop_empty_old_sidecars(dir);
    ensure_link(dir, rep);
    let _ = conn.execute_batch("ROLLBACK");
    drop(conn);
    if kind(&old) != (Kind::Link { to_new: true }) {
        return Err(RenameErr::Io("the compat link could not replace k2so.db".into()));
    }
    Ok(())
}

// ── the owner's choice (DB-A6) ────────────────────────────────────────

/// Record the owner's choice between two live files: `keep` is `k2.db` or
/// `k2so.db`. Pinned to both files' `(dev, inode)` so it can only ever
/// apply to the two files the owner saw. Takes effect at the next boot.
pub fn write_keep_choice(dir: &Path, keep: &str, by: &str) -> Result<serde_json::Value, String> {
    let keep = keep.trim();
    if keep != NEW_NAME && keep != OLD_NAME {
        return Err(format!("--keep must be {NEW_NAME} or {OLD_NAME}, not '{keep}'"));
    }
    let new = dir.join(NEW_NAME);
    let old = dir.join(OLD_NAME);
    if kind(&new) != Kind::Real || kind(&old) != Kind::Real || same_file(&new, &old) {
        return Err(format!(
            "there is nothing to choose: {} and {} are not two separate files (k2 db home shows what K2 uses)",
            new.display(),
            old.display()
        ));
    }
    let id = |p: &Path| file_id(p).map(|(d, i)| format!("{d}:{i}"));
    let (pn, po) = (probe(&new), probe(&old));
    let other = if keep == NEW_NAME { OLD_NAME } else { NEW_NAME };
    let v = serde_json::json!({
        "version": 1,
        "keep": keep,
        "by": by,
        "at": chrono::Utc::now().to_rfc3339(),
        "k2db": { "id": id(&new), "bytes": len_of(&new), "workspaces": pn.count() },
        "k2sodb": { "id": id(&old), "bytes": len_of(&old), "workspaces": po.count() },
    });
    write_json(dir, CHOICE_FILE, &v).map_err(|e| format!("{CHOICE_FILE} not written: {e}"))?;
    Ok(serde_json::json!({
        "ok": true,
        "keep": keep,
        "moveAside": other,
        "quarantine": dir.join(QUARANTINE_DIR).display().to_string(),
        "when": "the next time K2 starts",
        "message": format!(
            "At its next start K2 keeps {keep} and moves {other} into {} (it is never deleted). Restart K2 to finish.",
            dir.join(QUARANTINE_DIR).display()
        ),
    }))
}

/// The choice on disk, if it still names exactly these two files.
fn current_choice(dir: &Path, rep: &mut SwitchReport) -> Option<(String, serde_json::Value)> {
    let path = dir.join(CHOICE_FILE);
    let v = read_json(&path)?;
    let keep = v.get("keep").and_then(|k| k.as_str()).unwrap_or("").to_string();
    let id = |p: &Path| file_id(p).map(|(d, i)| format!("{d}:{i}"));
    let matches = (keep == NEW_NAME || keep == OLD_NAME)
        && v["k2db"]["id"].as_str().map(str::to_string) == id(&dir.join(NEW_NAME))
        && v["k2sodb"]["id"].as_str().map(str::to_string) == id(&dir.join(OLD_NAME))
        && id(&dir.join(NEW_NAME)).is_some();
    if !matches {
        rep.notes.push(format!("{CHOICE_FILE} names other files than these; ignored and removed"));
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some((keep, v))
}

// ── the switch ────────────────────────────────────────────────────────

/// The boot switch. Never deletes a database; on any doubt it changes
/// nothing and returns today's pick.
pub fn switch_home_db(dir: &Path, opts: &SwitchOpts) -> SwitchReport {
    let new = dir.join(NEW_NAME);
    let old = dir.join(OLD_NAME);
    let mut rep = SwitchReport::new(super::resolve_home_db_path_quiet(dir));
    let defer = |mut rep: SwitchReport, reason: String| {
        rep.path = super::resolve_home_db_path(dir);
        rep.outcome = Outcome::Deferred { reason };
        rep
    };
    // DB-A15 first: no defer or hold may open k2.db over an unfinished
    // write made under the old name.
    if kind(&new) == Kind::Real {
        if let Some((journal, bytes)) = hot_old_journal(dir) {
            return hot(dir, rep, journal, bytes);
        }
    }
    if let Some(reason) = &opts.defer {
        return defer(rep, reason.clone());
    }
    if cfg!(windows) {
        return defer(rep, "Windows keeps today's file in 0.46.6".into());
    }
    if dir.join(HOLD_FILE).exists() {
        return defer(rep, format!("held by the owner ({})", dir.join(HOLD_FILE).display()));
    }
    let (kn, ko) = (kind(&new), kind(&old));
    if kn == Kind::Real && ko == Kind::Real && same_file(&new, &old) {
        // Crash after the hard link (or a downgrade ran on it since).
        return match finish_hard_link(dir, &mut rep) {
            Ok(()) => finish_done(dir, opts, rep),
            Err(RenameErr::Busy(r)) | Err(RenameErr::Io(r)) => defer(rep, r),
        };
    }
    match (kn, ko) {
        // Fresh install (or a link left with no file behind it).
        (Kind::Absent, Kind::Absent) | (Kind::Absent, Kind::Link { to_new: true }) => {
            if let Some((journal, bytes)) = hot_old_journal(dir) {
                return hot(dir, rep, journal, bytes);
            }
            if let Err(e) = clear_orphan_sidecars(dir, &mut rep) {
                return defer(rep, format!("stray side files not moved: {e}"));
            }
            rep.path = new;
            rep.outcome = Outcome::Fresh;
            ensure_link(dir, &mut rep);
            ensure_marker(dir, &opts.by, &mut rep, true);
            rep
        }
        // Already switched (finishes a crash after the link).
        (Kind::Real, Kind::Absent) | (Kind::Real, Kind::Link { .. }) => finish_done(dir, opts, rep),
        // The legacy file alone: rename it.
        (Kind::Absent, Kind::Real) => do_rename(dir, opts, rep),
        // The Stage A stub beside a live file (or two live files).
        (Kind::Real, Kind::Real) => dual(dir, opts, rep),
        (n, o) => defer(rep, format!("unexpected shape: {NEW_NAME} is {n:?}, {OLD_NAME} is {o:?}; nothing moved")),
    }
}

/// `k2.db` is the real file: check the old name's journal, tidy strays,
/// make sure of the link and the marker.
fn finish_done(dir: &Path, opts: &SwitchOpts, mut rep: SwitchReport) -> SwitchReport {
    rep.path = dir.join(NEW_NAME);
    if let Some((journal, bytes)) = hot_old_journal(dir) {
        return hot(dir, rep, journal, bytes);
    }
    if let Err(e) = clear_orphan_sidecars(dir, &mut rep) {
        rep.notes.push(format!("stray side files not moved: {e}"));
    }
    if rep.outcome != Outcome::Renamed && rep.outcome != Outcome::KeptK2Db {
        rep.outcome = Outcome::AlreadyDone;
    }
    ensure_link(dir, &mut rep);
    let force = rep.outcome != Outcome::AlreadyDone;
    ensure_marker(dir, &opts.by, &mut rep, force);
    rep
}

fn hot(dir: &Path, mut rep: SwitchReport, journal: PathBuf, bytes: u64) -> SwitchReport {
    rep.path = dir.join(NEW_NAME);
    rep.outcome = Outcome::HotJournal { journal: journal.clone(), bytes };
    let v = serde_json::json!({
        "version": 1,
        "kind": "hot-journal",
        "lastAt": chrono::Utc::now().to_rfc3339(),
        "journal": journal.display().to_string(),
        "bytes": bytes,
        "message": hot_journal_message(dir, &journal, bytes),
    });
    if let Err(e) = write_json(dir, BLOCKED_NOTE, &v) {
        rep.notes.push(format!("{BLOCKED_NOTE} not written: {e}"));
    }
    rep
}

/// The owner-facing text for DB-A15.
pub fn hot_journal_message(dir: &Path, journal: &Path, bytes: u64) -> String {
    format!(
        "K2 did not start: {} holds {bytes} bytes of a write that never finished, made by a program that \
         opened K2's database under its old name ({}). Opening {} now could lose or garble that write. \
         Stop anything that uses {} and ask for help before you start K2 again; don't delete or move the \
         journal file.",
        journal.display(),
        dir.join(OLD_NAME).display(),
        dir.join(NEW_NAME).display(),
        dir.display(),
    )
}

/// Two real, different files.
fn dual(dir: &Path, opts: &SwitchOpts, mut rep: SwitchReport) -> SwitchReport {
    let new = dir.join(NEW_NAME);
    let old = dir.join(OLD_NAME);
    let today = super::pick_dual_real_db(&new, &old);
    let choice = current_choice(dir, &mut rep);
    let pick = match &choice {
        Some((keep, _)) => dir.join(keep),
        None => today.clone(),
    };
    let other = if pick == new { old.clone() } else { new.clone() };
    let (pick_probe, other_probe) = (probe(&pick), probe(&other));
    let defer = |mut rep: SwitchReport, reason: String| {
        rep.path = super::resolve_home_db_path(dir);
        rep.outcome = Outcome::Deferred { reason };
        rep
    };
    if let Probe::Unknown(why) = &pick_probe {
        return defer(rep, format!("cannot read {}: {why}", pick.display()));
    }
    match (&choice, &other_probe) {
        // The owner chose: the other file goes aside even when live.
        (Some((keep, v)), _) => {
            rep.notes.push(format!("the owner chose to keep {keep}"));
            let mut c = v.clone();
            c["appliedAt"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
            rep.choice = Some(c);
        }
        (None, p) if p.live() => {
            let (k2, k2so) = if pick == new { (pick_probe.count(), p.count()) } else { (p.count(), pick_probe.count()) };
            rep.path = today.clone();
            rep.outcome = Outcome::Blocked { k2_projects: k2, k2so_projects: k2so };
            write_blocked(dir, &today, k2, k2so, &mut rep);
            return rep;
        }
        (None, Probe::Unknown(why)) => {
            return defer(rep, format!("cannot read {}: {why}", other.display()));
        }
        (None, _) => {}
    }
    match quarantine(dir, &other) {
        Ok(dest) => rep.quarantined.push(dest),
        Err(e) => return defer(rep, format!("quarantine {}: {e}", other.display())),
    }
    let _ = std::fs::remove_file(dir.join(CHOICE_FILE));
    if pick == new {
        rep.outcome = Outcome::KeptK2Db;
        finish_done(dir, opts, rep)
    } else {
        do_rename(dir, opts, rep)
    }
}

fn do_rename(dir: &Path, opts: &SwitchOpts, mut rep: SwitchReport) -> SwitchReport {
    if let Err(e) = clear_orphan_sidecars(dir, &mut rep) {
        rep.path = super::resolve_home_db_path(dir);
        rep.outcome = Outcome::Deferred { reason: format!("stray side files not moved: {e}") };
        return rep;
    }
    match rename_legacy(dir, &mut rep) {
        Ok(()) => {
            rep.outcome = Outcome::Renamed;
            finish_done(dir, opts, rep)
        }
        Err(RenameErr::Busy(reason)) | Err(RenameErr::Io(reason)) => {
            rep.path = super::resolve_home_db_path(dir);
            rep.outcome = Outcome::Deferred { reason };
            rep
        }
    }
}

/// The owner-facing text for a refused switch.
pub fn blocked_message(dir: &Path, using: &Path, k2: i64, k2so: i64) -> String {
    format!(
        "K2 found two databases that both hold workspaces: {} ({k2so} workspace(s)) and {} \
         ({k2} workspace(s)). It did not rename or move either one, and it is still using {} \
         (the same file as before). To finish the move to k2.db, choose the copy to keep: \
         k2 db home --keep k2so.db (or --keep k2.db), then restart K2. The other copy is moved \
         into {} and never deleted.",
        dir.join(OLD_NAME).display(),
        dir.join(NEW_NAME).display(),
        using.display(),
        dir.join(QUARANTINE_DIR).display(),
    )
}

fn write_blocked(dir: &Path, using: &Path, k2: i64, k2so: i64, rep: &mut SwitchReport) {
    let prev = read_json(&dir.join(BLOCKED_NOTE)).filter(|v| v.get("kind").and_then(|k| k.as_str()) != Some("hot-journal"));
    let notice_filed = prev.as_ref().and_then(|v| v.get("noticeFiled")).and_then(|v| v.as_bool()).unwrap_or(false);
    let first_at = prev
        .as_ref()
        .and_then(|v| v.get("firstAt"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!(chrono::Utc::now().to_rfc3339()));
    let v = serde_json::json!({
        "version": 1,
        "kind": "two-live-files",
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
    read_json(&dir.join(BLOCKED_NOTE))
        .map(|v| {
            v.get("kind").and_then(|k| k.as_str()) == Some("two-live-files")
                && !v.get("noticeFiled").and_then(|f| f.as_bool()).unwrap_or(false)
        })
        .unwrap_or(false)
}

pub fn mark_blocked_notice_filed(dir: &Path) {
    let Some(mut v) = read_json(&dir.join(BLOCKED_NOTE)) else { return };
    v["noticeFiled"] = serde_json::json!(true);
    let _ = write_json(dir, BLOCKED_NOTE, &v);
}

/// `GET /cli/db-home`: this boot's result plus what is on disk now (the
/// refusal note, a pending choice, the hold).
pub fn home_json(dir: &Path) -> serde_json::Value {
    let mut v = status_json();
    if v.is_null() {
        v = serde_json::json!({});
    }
    v["dir"] = serde_json::json!(dir.display().to_string());
    v["blocked"] = read_json(&dir.join(BLOCKED_NOTE)).unwrap_or(serde_json::Value::Null);
    v["choice"] = read_json(&dir.join(CHOICE_FILE)).unwrap_or(serde_json::Value::Null);
    v["held"] = serde_json::json!(dir.join(HOLD_FILE).exists());
    v["marker"] = read_json(&dir.join(MARKER)).unwrap_or(serde_json::Value::Null);
    v
}

#[cfg(test)]
mod tests;
