//! Widget authorship: the `zen_widget_authors` table (migration 0164;
//! contract §3.5; PRD §4.2, §4.3, BG3). C0 → B2.
//!
//! `(widget, live bundle hash) → maker` decides a widget's data rights.
//! The first signer of a hash wins; only the person's claim ("It's mine")
//! overwrites a stamp. Built-ins (`k2:<name>`) are K2's and need no row.
//! The newest 20 rows per widget are kept.
//!
//! One exception to "first signer wins": a row whose maker workspace is
//! gone (deleted without the B1 cleanup reaching this row) reads as
//! `Unclaimed`, and a new sign may replace it. Nobody alive loses rights
//! by that, and the hash would otherwise stay unusable until the person
//! claimed it.

use rusqlite::{params, Connection, OptionalExtension};

use super::{DataError, MadeBy};

/// Rows kept per widget.
pub const KEEP_PER_WIDGET: usize = 20;

/// Why a stamp was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampSource {
    New,
    Sign,
    Claim,
    Import,
}

impl StampSource {
    pub fn as_str(self) -> &'static str {
        match self {
            StampSource::New => "new",
            StampSource::Sign => "sign",
            StampSource::Claim => "claim",
            StampSource::Import => "import",
        }
    }
}

/// Who presented the request that stamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    Owner,
    Agent,
    Shell,
}

impl CallerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CallerKind::Owner => "owner",
            CallerKind::Agent => "agent",
            CallerKind::Shell => "shell",
        }
    }
}

/// What a stamp did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampOutcome {
    Stamped,
    /// The hash already had a maker; a sign never changes it.
    Kept { existing: MadeBy },
}

/// True for a built-in ref (`k2:diary`, `k2:diary@1`): K2 made it.
pub fn is_builtin(widget: &str) -> bool {
    widget.starts_with("k2:")
}

fn check_widget(widget: &str) -> Result<(), DataError> {
    if crate::zen::valid_theme_name(widget) {
        Ok(())
    } else {
        Err(DataError::BadRequest(format!("'{widget}' isn't a widget folder name")))
    }
}

fn check_hash(hash: &str) -> Result<(), DataError> {
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err(DataError::BadRequest("a widget hash is 64 lowercase hex characters".into()))
    }
}

fn workspace_exists(conn: &Connection, project_id: &str) -> Result<bool, DataError> {
    Ok(conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", params![project_id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// The stored row's maker, without the "workspace gone" check.
fn row_of(conn: &Connection, widget: &str, hash: &str) -> Result<Option<MadeBy>, DataError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT author_kind, author_project_id FROM zen_widget_authors WHERE widget = ?1 AND hash = ?2",
            params![widget, hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        None => None,
        Some((kind, _)) if kind == "owner" => Some(MadeBy::Owner),
        Some((_, Some(p))) => Some(MadeBy::Agent { project_id: p }),
        Some((kind, None)) => {
            return Err(DataError::Unavailable(format!("zen_widget_authors has a '{kind}' row with no maker")))
        }
    })
}

fn write_row(
    conn: &Connection,
    widget: &str,
    hash: &str,
    author: &MadeBy,
    source: StampSource,
    caller: CallerKind,
    now: i64,
) -> Result<(), DataError> {
    let (kind, project) = match author {
        MadeBy::Owner => ("owner", None),
        MadeBy::Agent { project_id } => {
            if project_id.is_empty() {
                return Err(DataError::BadRequest("an agent stamp needs its workspace".into()));
            }
            ("agent", Some(project_id.as_str()))
        }
        MadeBy::K2 | MadeBy::Unclaimed => {
            return Err(DataError::BadRequest("a stamp names the person or an agent".into()));
        }
    };
    conn.execute(
        "INSERT INTO zen_widget_authors (widget, hash, author_kind, author_project_id, source, caller_kind, signed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (widget, hash) DO UPDATE SET
           author_kind = excluded.author_kind, author_project_id = excluded.author_project_id,
           source = excluded.source, caller_kind = excluded.caller_kind, signed_at = excluded.signed_at",
        params![widget, hash, kind, project, source.as_str(), caller.as_str(), now],
    )?;
    prune(conn, widget, hash)?;
    Ok(())
}

/// Keep the newest [`KEEP_PER_WIDGET`] rows of `widget`; `keep` (the row
/// just written) always stays.
fn prune(conn: &Connection, widget: &str, keep: &str) -> Result<usize, DataError> {
    Ok(conn.execute(
        "DELETE FROM zen_widget_authors WHERE widget = ?1 AND hash NOT IN (
           SELECT hash FROM zen_widget_authors WHERE widget = ?1
           ORDER BY (hash = ?2) DESC, signed_at DESC, hash DESC LIMIT ?3)",
        params![widget, keep, KEEP_PER_WIDGET as i64],
    )?)
}

/// Stamp `(widget, hash)` with `author` unless it already has a maker.
/// `author` is `Owner` or `Agent`; `K2`/`Unclaimed` are refused. A
/// built-in is K2's: nothing is written (`Kept { K2 }`).
pub fn stamp(
    conn: &Connection,
    widget: &str,
    hash: &str,
    author: &MadeBy,
    source: StampSource,
    caller: CallerKind,
    now: i64,
) -> Result<StampOutcome, DataError> {
    if is_builtin(widget) {
        return Ok(StampOutcome::Kept { existing: MadeBy::K2 });
    }
    check_widget(widget)?;
    check_hash(hash)?;
    if row_of(conn, widget, hash)?.is_some() {
        let existing = maker_of(conn, widget, hash)?;
        if existing != MadeBy::Unclaimed {
            return Ok(StampOutcome::Kept { existing });
        }
    }
    write_row(conn, widget, hash, author, source, caller, now)?;
    Ok(StampOutcome::Stamped)
}

/// A vouching sign (prd-garden-access-v1 G9, switch on): the person, or an
/// agent listed on every Garden that places `widget`, signs `(widget,
/// hash)` and overwrites a stamp by anyone else. The caller decides who may
/// vouch; this only writes. Re-vouching by the same maker changes nothing
/// (`Kept`). A built-in is K2's: nothing is written (`Kept { K2 }`).
pub fn vouch(
    conn: &Connection,
    widget: &str,
    hash: &str,
    author: &MadeBy,
    source: StampSource,
    caller: CallerKind,
    now: i64,
) -> Result<StampOutcome, DataError> {
    if is_builtin(widget) {
        return Ok(StampOutcome::Kept { existing: MadeBy::K2 });
    }
    check_widget(widget)?;
    check_hash(hash)?;
    let existing = maker_of(conn, widget, hash)?;
    if existing == *author {
        return Ok(StampOutcome::Kept { existing });
    }
    write_row(conn, widget, hash, author, source, caller, now)?;
    Ok(StampOutcome::Stamped)
}

/// The one-time carry of 0.46.5 (S0, `zen::version_carry`): copy the
/// stamp of `(widget, old)` to `(widget, new)`: same maker, caller kind and
/// signing time, `source = import`. Nothing is written when `old` has no
/// row or `new` already has one. Returns the copied maker, as stored (a
/// gone workspace's stamp is copied too and reads `Unclaimed` as before).
pub fn carry(conn: &Connection, widget: &str, old: &str, new: &str) -> Result<Option<MadeBy>, DataError> {
    if is_builtin(widget) {
        return Ok(None);
    }
    check_widget(widget)?;
    check_hash(old)?;
    check_hash(new)?;
    if old == new || row_of(conn, widget, new)?.is_some() {
        return Ok(None);
    }
    let Some(maker) = row_of(conn, widget, old)? else { return Ok(None) };
    conn.execute(
        "INSERT INTO zen_widget_authors (widget, hash, author_kind, author_project_id, source, caller_kind, signed_at)
         SELECT widget, ?3, author_kind, author_project_id, 'import', caller_kind, signed_at
           FROM zen_widget_authors WHERE widget = ?1 AND hash = ?2",
        params![widget, old, new],
    )?;
    prune(conn, widget, new)?;
    Ok(Some(maker))
}

/// When `(widget, hash)` was signed (Unix seconds), if it has a row.
pub fn signed_at(conn: &Connection, widget: &str, hash: &str) -> Result<Option<i64>, DataError> {
    Ok(conn
        .query_row(
            "SELECT signed_at FROM zen_widget_authors WHERE widget = ?1 AND hash = ?2",
            params![widget, hash],
            |r| r.get(0),
        )
        .optional()?)
}

/// The person's "It's mine": stamp `(widget, hash)` `owner`, overwriting.
pub fn claim(conn: &Connection, widget: &str, hash: &str, now: i64) -> Result<(), DataError> {
    if is_builtin(widget) {
        return Err(DataError::BadRequest("a built-in widget is K2's; there is nothing to claim".into()));
    }
    check_widget(widget)?;
    check_hash(hash)?;
    write_row(conn, widget, hash, &MadeBy::Owner, StampSource::Claim, CallerKind::Owner, now)
}

/// The maker of `(widget, hash)`: `k2:*` → K2; no row → Unclaimed; an agent
/// whose workspace is gone → Unclaimed.
pub fn maker_of(conn: &Connection, widget: &str, hash: &str) -> Result<MadeBy, DataError> {
    if is_builtin(widget) {
        return Ok(MadeBy::K2);
    }
    Ok(match row_of(conn, widget, hash)? {
        None => MadeBy::Unclaimed,
        Some(MadeBy::Agent { project_id }) => {
            if workspace_exists(conn, &project_id)? {
                MadeBy::Agent { project_id }
            } else {
                MadeBy::Unclaimed
            }
        }
        Some(m) => m,
    })
}

/// The maker of the newest stamped version of `widget` (the most recent
/// `signed_at`), with the same "workspace gone → Unclaimed" rule as
/// [`maker_of`]. `None` when no version was ever stamped. A built-in is K2's.
pub fn newest_maker(conn: &Connection, widget: &str) -> Result<Option<MadeBy>, DataError> {
    if is_builtin(widget) {
        return Ok(Some(MadeBy::K2));
    }
    let hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM zen_widget_authors WHERE widget = ?1 ORDER BY signed_at DESC, hash DESC LIMIT 1",
            params![widget],
            |r| r.get(0),
        )
        .optional()?;
    match hash {
        None => Ok(None),
        Some(h) => maker_of(conn, widget, &h).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::db::run_migrations(&conn).expect("migrations");
        for id in ["w-builder", "w-keeper"] {
            conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)", params![id, format!("/w/{id}")])
                .expect("project row");
        }
        conn
    }

    fn h(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn agent(p: &str) -> MadeBy {
        MadeBy::Agent { project_id: p.into() }
    }

    fn rows(conn: &Connection, widget: &str) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM zen_widget_authors WHERE widget = ?1", params![widget], |r| r.get(0))
            .expect("count")
    }

    #[test]
    fn first_signer_of_a_hash_wins() {
        let conn = db();
        let got = stamp(&conn, "board", &h('a'), &agent("w-builder"), StampSource::Sign, CallerKind::Agent, 10).expect("stamp");
        assert_eq!(got, StampOutcome::Stamped);
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), agent("w-builder"));
        // The person re-validating the same code doesn't move rights.
        let again = stamp(&conn, "board", &h('a'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 11).expect("stamp");
        assert_eq!(again, StampOutcome::Kept { existing: agent("w-builder") });
        // Nor does another agent.
        let other = stamp(&conn, "board", &h('a'), &agent("w-keeper"), StampSource::Sign, CallerKind::Shell, 12).expect("stamp");
        assert_eq!(other, StampOutcome::Kept { existing: agent("w-builder") });
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), agent("w-builder"));
        // An edit is a new hash, stamped by its editor.
        stamp(&conn, "board", &h('b'), &agent("w-keeper"), StampSource::Sign, CallerKind::Agent, 13).expect("stamp");
        assert_eq!(maker_of(&conn, "board", &h('b')).expect("maker"), agent("w-keeper"));
    }

    #[test]
    fn claim_overwrites_to_owner() {
        let conn = db();
        stamp(&conn, "board", &h('a'), &agent("w-builder"), StampSource::New, CallerKind::Agent, 10).expect("stamp");
        claim(&conn, "board", &h('a'), 20).expect("claim");
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), MadeBy::Owner);
        let (source, caller): (String, String) = conn
            .query_row(
                "SELECT source, caller_kind FROM zen_widget_authors WHERE widget = 'board' AND hash = ?1",
                params![h('a')],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!((source.as_str(), caller.as_str()), ("claim", "owner"));
        // Claiming an unsigned hash stamps it too.
        claim(&conn, "board", &h('c'), 21).expect("claim unsigned");
        assert_eq!(maker_of(&conn, "board", &h('c')).expect("maker"), MadeBy::Owner);
    }

    /// G9 (garden access): a vouching sign overwrites anyone else's stamp;
    /// the same maker again changes nothing; built-ins stay K2's.
    #[test]
    fn a_vouch_overwrites_another_signer_and_keeps_its_own() {
        let conn = db();
        stamp(&conn, "board", &h('a'), &agent("w-builder"), StampSource::Sign, CallerKind::Agent, 10).expect("stamp");
        let got = vouch(&conn, "board", &h('a'), &agent("w-keeper"), StampSource::Sign, CallerKind::Agent, 11).expect("vouch");
        assert_eq!(got, StampOutcome::Stamped);
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), agent("w-keeper"));
        let again = vouch(&conn, "board", &h('a'), &agent("w-keeper"), StampSource::Sign, CallerKind::Agent, 12).expect("vouch");
        assert_eq!(again, StampOutcome::Kept { existing: agent("w-keeper") });
        let person = vouch(&conn, "board", &h('a'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 13).expect("vouch");
        assert_eq!(person, StampOutcome::Stamped);
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), MadeBy::Owner);
        let unsigned = vouch(&conn, "board", &h('b'), &agent("w-builder"), StampSource::Sign, CallerKind::Agent, 14).expect("vouch");
        assert_eq!(unsigned, StampOutcome::Stamped);
        let k2 = vouch(&conn, "k2:diary", &h('a'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 15).expect("builtin");
        assert_eq!(k2, StampOutcome::Kept { existing: MadeBy::K2 });
        assert_eq!(rows(&conn, "k2:diary"), 0);
    }

    #[test]
    fn no_row_is_unclaimed_and_builtins_are_k2() {
        let conn = db();
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), MadeBy::Unclaimed);
        assert_eq!(maker_of(&conn, "k2:diary", &h('a')).expect("maker"), MadeBy::K2);
        let got = stamp(&conn, "k2:diary", &h('a'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("stamp");
        assert_eq!(got, StampOutcome::Kept { existing: MadeBy::K2 });
        assert_eq!(rows(&conn, "k2:diary"), 0, "built-ins never get a row");
        assert!(claim(&conn, "k2:diary", &h('a'), 1).is_err());
    }

    #[test]
    fn a_gone_maker_reads_unclaimed_and_a_new_sign_replaces_it() {
        let conn = db();
        stamp(&conn, "board", &h('a'), &agent("w-keeper"), StampSource::Sign, CallerKind::Agent, 10).expect("stamp");
        conn.execute("DELETE FROM projects WHERE id = 'w-keeper'", []).expect("delete workspace");
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), MadeBy::Unclaimed);
        let got = stamp(&conn, "board", &h('a'), &agent("w-builder"), StampSource::Sign, CallerKind::Agent, 11).expect("stamp");
        assert_eq!(got, StampOutcome::Stamped);
        assert_eq!(maker_of(&conn, "board", &h('a')).expect("maker"), agent("w-builder"));
    }

    #[test]
    fn the_newest_twenty_rows_per_widget_are_kept() {
        let conn = db();
        let hashes: Vec<String> = (0..25).map(|i| format!("{i:064x}")).collect();
        for (i, hash) in hashes.iter().enumerate() {
            stamp(&conn, "board", hash, &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 100 + i as i64).expect("stamp");
        }
        stamp(&conn, "other", &h('a'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("stamp");
        assert_eq!(rows(&conn, "board"), KEEP_PER_WIDGET as i64);
        assert_eq!(rows(&conn, "other"), 1, "pruning is per widget");
        assert_eq!(maker_of(&conn, "board", &hashes[0]).expect("maker"), MadeBy::Unclaimed, "oldest pruned");
        assert_eq!(maker_of(&conn, "board", &hashes[24]).expect("maker"), MadeBy::Owner, "newest kept");
        // A row written with an older time than the rest is still kept.
        stamp(&conn, "board", &h('f'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("stamp");
        assert_eq!(maker_of(&conn, "board", &h('f')).expect("maker"), MadeBy::Owner);
        assert_eq!(rows(&conn, "board"), KEEP_PER_WIDGET as i64);
    }

    #[test]
    fn bad_input_is_refused() {
        let conn = db();
        let bad = |w: &str, hash: &str, a: &MadeBy| stamp(&conn, w, hash, a, StampSource::Sign, CallerKind::Owner, 1);
        assert!(matches!(bad("Board", &h('a'), &MadeBy::Owner), Err(DataError::BadRequest(_))));
        assert!(matches!(bad("board", "abc", &MadeBy::Owner), Err(DataError::BadRequest(_))));
        assert!(matches!(bad("board", &"A".repeat(64), &MadeBy::Owner), Err(DataError::BadRequest(_))));
        assert!(matches!(bad("board", &h('a'), &MadeBy::K2), Err(DataError::BadRequest(_))));
        assert!(matches!(bad("board", &h('a'), &MadeBy::Unclaimed), Err(DataError::BadRequest(_))));
        assert!(matches!(bad("board", &h('a'), &agent("")), Err(DataError::BadRequest(_))));
        assert_eq!(rows(&conn, "board"), 0);
    }
}
