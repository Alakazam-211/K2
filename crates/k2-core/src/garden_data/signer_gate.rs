//! The signer gate (prd-garden-access-v1 §1.6, GA1, G15, G22; slice A3).
//!
//! Garden access is always on in a running daemon (0.46.3,
//! `zen::garden_access::enabled`); the switch survives only for unit tests
//! of the retired path.
//!
//! A Garden sees everything the person sees (Rosson rev 4), so the one
//! thing that decides whether a placement's code runs with the person's
//! rights is **who signed its live version**:
//!
//! - K2's built-ins (`k2:<name>`) always pass;
//! - a version stamped by the person passes;
//! - a version stamped by an agent passes only while that agent is on the
//!   list of **the Garden the placement is in**, read at call time (so an
//!   Allow gains it everything with no re-sign, and a Remove takes it away
//!   on the next call);
//! - anything else (unsigned, a raw disk edit, a reset to an unsigned
//!   hash, an unlisted agent's stamp, a gone workspace's stamp) fails.
//!
//! "Its live version" is the bundle hash, and from 0.46.5 (S0) that hash
//! covers the manifest's run fields too (`zen::widgets::version_digest`:
//! caps, reads, sources, libraries as written, entry, colorScheme). So a
//! raw disk edit of `manifest.json` alone (a cap, a neighbour read, a CDN
//! file) is a new, unsigned version and fails here, exactly like an edit
//! to the code; a locked Garden's frozen copy is re-checked against its
//! saved manifest on every read (`zen::lock::frozen_widget`).
//!
//! A version that fails keeps **Always** only ([`ALWAYS_CAPS`]): its own
//! `store:*` keyspace, `agents:read` and `presence:read`. Every other call
//! answers 403 [`NEEDS_EDITOR`]. Use is never gated by the *agent's* list
//! (G15a): a Thread post from a passing widget reaches any agent.
//!
//! Honest bar (as `garden_data`): a guardrail against mistakes and injected
//! prompts, not a wall against a same-user process.

use rusqlite::Connection;

use super::{authors, DataError, MadeBy};

/// The caps a version keeps when it fails the gate (§1.6 "Always").
pub const ALWAYS_CAPS: &[&str] = &["agents:read", "presence:read", "store:read", "store:write"];

/// 403 code: this version isn't signed by the person, K2 or an agent on
/// this Garden's list.
pub const NEEDS_EDITOR: &str = "needs_editor";

/// The plain words for [`NEEDS_EDITOR`].
pub fn needs_editor_message(widget: &str) -> String {
    format!(
        "Someone who edits this Garden has to sign this version of {widget} (k2 zen widget sign {widget}, or Sign in its History); an agent not on the Garden's list asks the person with k2 zen widget sign {widget} --ask. Until then it runs without your data."
    )
}

/// Does the live version `(widget, hash)` placed in Garden `garden` pass
/// the gate now? Read on `conn` (callers hold the shared lock).
pub fn passes(conn: &Connection, garden: &str, widget: &str, hash: &str) -> Result<bool, DataError> {
    if authors::is_builtin(widget) {
        return Ok(true);
    }
    let folder = widget.split('@').next().unwrap_or(widget);
    match authors::maker_of(conn, folder, hash)? {
        MadeBy::Owner | MadeBy::K2 => Ok(true),
        MadeBy::Agent { project_id } => crate::zen::garden_access::is_listed(conn, garden, &project_id)
            .map_err(|e| DataError::Unavailable(format!("garden access list: {e:?}"))),
        MadeBy::Unclaimed => Ok(false),
    }
}

/// `caps` cut down to [`ALWAYS_CAPS`] (order kept).
pub fn always_only(caps: &[String]) -> Vec<String> {
    caps.iter().filter(|c| ALWAYS_CAPS.contains(&c.as_str())).cloned().collect()
}

/// The caps a placement runs with: `caps` (its effective caps) when the
/// switch is off or the gate passes; else [`always_only`]. The bool is
/// whether the gate passed (always `true` with the switch off). Opens the
/// shared DB only with the switch on. A store error fails closed.
pub fn gate_caps(garden: &str, widget: &str, hash: &str, caps: Vec<String>) -> (Vec<String>, bool) {
    if !crate::zen::garden_access::enabled() {
        return (caps, true);
    }
    let ok = {
        let db = crate::db::shared();
        let conn = db.lock();
        match passes(&conn, garden, widget, hash) {
            Ok(b) => b,
            Err(e) => {
                crate::log_debug!("[garden_data/signer_gate] {garden}/{widget}: {e}; failing closed");
                false
            }
        }
    };
    if ok {
        (caps, true)
    } else {
        (always_only(&caps), false)
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::garden_data::authors::{CallerKind, StampSource};
    use crate::zen::garden_access::{self as ga, Actor, Change, Via};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::db::run_migrations(&conn).expect("migrations");
        for id in ["w-listed", "w-other"] {
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

    const G: &str = "g-aaaaaaaa";

    #[test]
    fn the_person_k2_and_a_listed_agent_pass_and_nothing_else_does() {
        // `ga::add` writes Access Audit lines under $HOME: a temp HOME (and its
        // env lock) keeps them out of a parallel test's audit file.
        let _home = crate::test_env::TempHome::new();
        let conn = db();
        let owner = Actor::Owner;
        ga::add(&conn, G, "w-listed", &Change::new(Via::Settings, &owner, 1)).expect("list");
        // K2's built-ins always pass, with no row.
        assert!(passes(&conn, G, "k2:diary@1", &h('a')).expect("builtin"));
        // Unsigned fails.
        assert!(!passes(&conn, G, "board", &h('a')).expect("unsigned"));
        // The person's stamp passes.
        authors::stamp(&conn, "board", &h('a'), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 2).expect("stamp");
        assert!(passes(&conn, G, "board", &h('a')).expect("owner"));
        // A listed agent's stamp passes on that Garden only.
        authors::stamp(&conn, "board", &h('b'), &agent("w-listed"), StampSource::Sign, CallerKind::Agent, 3).expect("stamp");
        assert!(passes(&conn, G, "board", &h('b')).expect("listed"));
        assert!(!passes(&conn, "g-bbbbbbbb", "board", &h('b')).expect("other garden"));
        // An unlisted agent's stamp fails.
        authors::stamp(&conn, "board", &h('c'), &agent("w-other"), StampSource::Sign, CallerKind::Agent, 4).expect("stamp");
        assert!(!passes(&conn, G, "board", &h('c')).expect("unlisted"));
        // Allow gains it with no re-sign; Remove takes it away.
        ga::add(&conn, G, "w-other", &Change::new(Via::Card, &owner, 5)).expect("allow");
        assert!(passes(&conn, G, "board", &h('c')).expect("allowed"));
        ga::remove(&conn, G, "w-other", &Change::new(Via::Settings, &owner, 6)).expect("remove");
        assert!(!passes(&conn, G, "board", &h('c')).expect("removed"));
    }

    #[test]
    fn always_only_keeps_own_store_agents_and_presence() {
        let caps: Vec<String> = ["agents:read", "thread:read", "thread:post", "files:read", "store:read", "store:write", "presence:read", "guests:rooms"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(always_only(&caps), vec!["agents:read", "store:read", "store:write", "presence:read"]);
    }
}
