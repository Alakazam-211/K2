//! Auto-rotate ("auto Next") for LLM subscriptions.
//!
//! The wallet's Next moves a tool's server default to its next signed-in
//! subscription. Auto-rotate presses that same Next by itself as soon as
//! the live subscription reaches [`THRESHOLD_PERCENT`] of its binding usage
//! window (the top-bar chip's number: the highest of the windows that
//! drive it, e.g. Claude's 5-hour Session or Weekly).
//!
//! **Opt-in, per person, in person.** Switching accounts at a limit may
//! conflict with the provider's terms, so it is the person's own decision
//! (each person decides whether to turn token rotation on for their own
//! logins): off by default (no row), one row per tool naming the person who
//! turned it on (their wallet key: `owner-token` or `login:<name>`). Only
//! a person who may press Next (Admin and above) turns it on, with a vault
//! confirm (Touch ID or a password), so an agent's passport, and an agent
//! falling back to the owner token, can't. The daemon's loop acts only
//! while that person can still change the server default.
//!
//! The daemon owns the loop (headless): [`decide`] is its rule, pure and
//! tested here; the switch itself is the Next route's own code.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{Tool, WalletError};
use crate::product_name;

/// At or above this percent of its binding window the live subscription
/// is "at its limit".
pub const THRESHOLD_PERCENT: i64 = 99;

/// After an automatic switch, no other one for this long on that tool
/// (a stale or wrong reading can't make it flap).
pub const COOLDOWN_SECS: i64 = 10 * 60;

/// The vault confirm action that turns it on (`secretId` = the tool).
pub const CONFIRM_ACTION: &str = "llm-auto-next";

/// The words shown next to the switch (and in `k2 llm tokens auto-next`):
/// what it does, and that it may conflict with the provider's terms.
pub const TERMS_NOTE: &str = concat!("When the subscription in use reaches 99% of its limit, ", product_name!(), " switches to the next signed-in one by itself. \
Switching accounts automatically at the limit may go against the provider's terms. It's your decision; it's off unless you turn it on.");

/// One tool's setting (a row = on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub tool: String,
    /// Who turned it on (wallet key: `owner-token` or `login:<name>`).
    pub person: String,
    pub enabled_at: i64,
    /// The loop's last automatic switch (the cooldown runs from it).
    pub last_switch_at: Option<i64>,
    /// The loop's last line ("Switched to … automatically: … reached 99%",
    /// or "All your Claude subscriptions are at their limit").
    pub last_line: Option<String>,
    pub last_line_at: Option<i64>,
    /// Since when every subscription has been at its limit (one notice per
    /// episode); cleared once the live one is below the limit or a switch
    /// lands.
    pub exhausted_at: Option<i64>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Setting> {
    Ok(Setting {
        tool: r.get(0)?,
        person: r.get(1)?,
        enabled_at: r.get(2)?,
        last_switch_at: r.get(3)?,
        last_line: r.get(4)?,
        last_line_at: r.get(5)?,
        exhausted_at: r.get(6)?,
    })
}

const COLS: &str = "tool, person, enabled_at, last_switch_at, last_line, last_line_at, exhausted_at";

fn subscription_tool(tool: Tool) -> Result<(), WalletError> {
    if tool.subscription_supported() {
        Ok(())
    } else {
        Err(WalletError::Conflict(
            "no_subscriptions",
            format!("{} has API tokens only; auto-rotate moves between subscriptions", tool.display()),
        ))
    }
}

/// The tool's setting, `None` when it is off.
pub fn get(conn: &Connection, tool: Tool) -> Result<Option<Setting>, WalletError> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM llm_auto_next WHERE tool = ?1"), params![tool.as_str()], row)
        .optional()?)
}

/// Every tool that has it on.
pub fn list(conn: &Connection) -> Result<Vec<Setting>, WalletError> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM llm_auto_next ORDER BY tool"))?;
    let rows = st.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Turn it on for `tool`, as `person`. The caller checked that `person`
/// is a person who may press Next and confirmed it's them. Turning it on
/// again re-stamps the person (the newest decision is theirs).
pub fn turn_on(conn: &Connection, tool: Tool, person: &str, now: i64) -> Result<Setting, WalletError> {
    subscription_tool(tool)?;
    let person = person.trim();
    if person.is_empty() {
        return Err(WalletError::Conflict("person_required", "auto-rotate is turned on by a person".into()));
    }
    conn.execute(
        "INSERT INTO llm_auto_next (tool, person, enabled_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(tool) DO UPDATE SET person = excluded.person, enabled_at = excluded.enabled_at",
        params![tool.as_str(), person, now],
    )?;
    get(conn, tool)?.ok_or_else(|| WalletError::NotFound(tool.as_str().into()))
}

/// Turn it off. `true` when it was on.
pub fn turn_off(conn: &Connection, tool: Tool) -> Result<bool, WalletError> {
    Ok(conn.execute("DELETE FROM llm_auto_next WHERE tool = ?1", params![tool.as_str()])? > 0)
}

/// An automatic switch landed: the cooldown starts, the line is kept and
/// any "all at their limit" episode is over.
pub fn note_switch(conn: &Connection, tool: Tool, line: &str, now: i64) -> Result<(), WalletError> {
    conn.execute(
        "UPDATE llm_auto_next SET last_switch_at = ?2, last_line = ?3, last_line_at = ?2, exhausted_at = NULL WHERE tool = ?1",
        params![tool.as_str(), now, clip(line)],
    )?;
    Ok(())
}

/// Every subscription is at its limit. `true` the first time in this
/// episode (the caller tells the person once), `false` after that.
pub fn note_exhausted(conn: &Connection, tool: Tool, line: &str, now: i64) -> Result<bool, WalletError> {
    let n = conn.execute(
        "UPDATE llm_auto_next SET exhausted_at = ?2, last_line = ?3, last_line_at = ?2 WHERE tool = ?1 AND exhausted_at IS NULL",
        params![tool.as_str(), now, clip(line)],
    )?;
    Ok(n > 0)
}

/// The live subscription is below its limit again: the next time they're
/// all at their limit is a new episode.
pub fn clear_exhausted(conn: &Connection, tool: Tool) -> Result<(), WalletError> {
    conn.execute("UPDATE llm_auto_next SET exhausted_at = NULL WHERE tool = ?1 AND exhausted_at IS NOT NULL", params![tool.as_str()])?;
    Ok(())
}

fn clip(line: &str) -> String {
    line.chars().filter(|c| !c.is_control()).take(500).collect()
}

/// One token Next could move to, with what the loop knows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    /// Its binding window's percent (`None`: no reading).
    pub percent: Option<i64>,
    /// Its usage check is failing and backing off: the number can't be
    /// trusted, so it isn't a target.
    pub backing_off: bool,
    /// Signed in (a token whose sign-in expired or needs a login is no
    /// target).
    pub signed_in: bool,
}

impl Candidate {
    fn usable(&self) -> bool {
        self.signed_in && !self.backing_off && self.percent.is_none_or(|p| p < THRESHOLD_PERCENT)
    }
}

/// What the loop does for one tool now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The live subscription is below the limit (or has no reading).
    Below,
    /// At the limit, but an automatic switch ran less than
    /// [`COOLDOWN_SECS`] ago.
    Cooldown { until: i64 },
    /// Press Next, leaving out these ids (at their limit, backing off, or
    /// not signed in); Next then takes the first of the rest in its order.
    Switch { exclude: Vec<String>, to: String },
    /// At the limit and no other subscription can take over.
    AllAtLimit,
}

/// The rule: at or above [`THRESHOLD_PERCENT`], outside the cooldown, move
/// to the first usable candidate (`candidates` in Next's order).
pub fn decide(live_percent: Option<i64>, candidates: &[Candidate], last_switch_at: Option<i64>, now: i64) -> Decision {
    match live_percent {
        Some(p) if p >= THRESHOLD_PERCENT => {}
        _ => return Decision::Below,
    }
    if let Some(t) = last_switch_at {
        if now < t + COOLDOWN_SECS {
            return Decision::Cooldown { until: t + COOLDOWN_SECS };
        }
    }
    match candidates.iter().find(|c| c.usable()) {
        Some(to) => Decision::Switch {
            exclude: candidates.iter().filter(|c| !c.usable()).map(|c| c.id.clone()).collect(),
            to: to.id.clone(),
        },
        None => Decision::AllAtLimit,
    }
}

/// "Switched to <to> automatically: <from> reached 99%".
pub fn switched_line(to_label: &str, from_label: Option<&str>, percent: i64) -> String {
    match from_label {
        Some(f) => format!("Switched to {to_label} automatically: {f} reached {percent}%"),
        None => format!("Switched to {to_label} automatically: the subscription in use reached {percent}%"),
    }
}

/// "All your Claude subscriptions are at their limit".
pub fn exhausted_line(tool: Tool) -> String {
    format!("All your {} subscriptions are at their limit", tool.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn
    }

    fn cand(id: &str, percent: Option<i64>) -> Candidate {
        Candidate { id: id.into(), label: id.into(), percent, backing_off: false, signed_in: true }
    }

    #[test]
    fn auto_next_is_off_by_default_and_a_row_is_on() {
        let conn = db();
        for tool in Tool::SUBSCRIPTION {
            assert_eq!(get(&conn, tool).unwrap(), None, "{tool:?} is off until a person turns it on");
        }
        assert!(list(&conn).unwrap().is_empty());
        let s = turn_on(&conn, Tool::Claude, "owner-token", 100).unwrap();
        assert_eq!((s.tool.as_str(), s.person.as_str(), s.enabled_at), ("claude", "owner-token", 100));
        assert_eq!(get(&conn, Tool::Codex).unwrap(), None, "per tool");
        assert!(turn_off(&conn, Tool::Claude).unwrap());
        assert!(!turn_off(&conn, Tool::Claude).unwrap(), "already off");
        assert_eq!(get(&conn, Tool::Claude).unwrap(), None);
    }

    #[test]
    fn auto_next_needs_a_person_and_a_subscription_tool() {
        let conn = db();
        assert_eq!(turn_on(&conn, Tool::Claude, "  ", 1).unwrap_err().code(), "person_required");
        assert_eq!(turn_on(&conn, Tool::Gemini, "owner-token", 1).unwrap_err().code(), "no_subscriptions");
        assert!(list(&conn).unwrap().is_empty());
    }

    #[test]
    fn auto_next_at_98_percent_does_nothing() {
        let c = [cand("b", Some(10))];
        assert_eq!(decide(Some(98), &c, None, 1000), Decision::Below);
        assert_eq!(decide(None, &c, None, 1000), Decision::Below, "no reading: nothing");
    }

    #[test]
    fn auto_next_at_99_percent_switches_to_the_first_usable_one() {
        let c = [cand("b", Some(99)), cand("c", None), cand("d", Some(5))];
        assert_eq!(
            decide(Some(99), &c, None, 1000),
            Decision::Switch { exclude: vec!["b".into()], to: "c".into() },
            "b is at its limit; c has no reading yet and is next in Next's order"
        );
        assert_eq!(decide(Some(100), &[cand("b", Some(0))], None, 1000), Decision::Switch { exclude: vec![], to: "b".into() });
    }

    #[test]
    fn auto_next_skips_backing_off_and_signed_out_tokens() {
        let mut off = cand("b", Some(1));
        off.backing_off = true;
        let mut out = cand("c", Some(1));
        out.signed_in = false;
        let c = [off, out, cand("d", Some(50))];
        assert_eq!(decide(Some(99), &c, None, 1000), Decision::Switch { exclude: vec!["b".into(), "c".into()], to: "d".into() });
    }

    #[test]
    fn auto_next_does_not_flap_inside_the_cooldown() {
        let c = [cand("b", Some(1))];
        let t = 10_000;
        assert_eq!(decide(Some(99), &c, Some(t), t + 1), Decision::Cooldown { until: t + COOLDOWN_SECS });
        assert_eq!(decide(Some(99), &c, Some(t), t + COOLDOWN_SECS - 1), Decision::Cooldown { until: t + COOLDOWN_SECS });
        assert_eq!(decide(Some(99), &c, Some(t), t + COOLDOWN_SECS), Decision::Switch { exclude: vec![], to: "b".into() });
    }

    #[test]
    fn auto_next_with_every_subscription_at_its_limit_switches_nowhere() {
        assert_eq!(decide(Some(99), &[cand("b", Some(99)), cand("c", Some(100))], None, 1), Decision::AllAtLimit);
        assert_eq!(decide(Some(99), &[], None, 1), Decision::AllAtLimit, "no other subscription");
    }

    #[test]
    fn auto_next_tells_the_all_at_limit_line_once_per_episode() {
        let conn = db();
        turn_on(&conn, Tool::Claude, "owner-token", 1).unwrap();
        let line = exhausted_line(Tool::Claude);
        assert_eq!(line, "All your Claude subscriptions are at their limit");
        assert!(note_exhausted(&conn, Tool::Claude, &line, 10).unwrap(), "first time: tell");
        assert!(!note_exhausted(&conn, Tool::Claude, &line, 20).unwrap(), "same episode: quiet");
        let s = get(&conn, Tool::Claude).unwrap().unwrap();
        assert_eq!((s.exhausted_at, s.last_line.as_deref()), (Some(10), Some(line.as_str())));
        clear_exhausted(&conn, Tool::Claude).unwrap();
        assert!(note_exhausted(&conn, Tool::Claude, &line, 30).unwrap(), "a new episode tells again");
        note_switch(&conn, Tool::Claude, &switched_line("b", Some("a"), 99), 40).unwrap();
        let s = get(&conn, Tool::Claude).unwrap().unwrap();
        assert_eq!(s.exhausted_at, None, "a switch ends the episode");
        assert_eq!(s.last_switch_at, Some(40));
        assert_eq!(s.last_line.as_deref(), Some("Switched to b automatically: a reached 99%"));
        // Off for a tool with no row: nothing to note, nothing written.
        assert!(!note_exhausted(&conn, Tool::Codex, "x", 1).unwrap());
        assert_eq!(get(&conn, Tool::Codex).unwrap(), None);
    }
}
