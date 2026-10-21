//! The person layer of the spawn decision (pure over the facts; unit
//! tested). The daemon gathers the facts in three steps (main DB → skin.db
//! → main DB) so neither database lock is held while the other is read.
//!
//! Fresh spawn: **chat pin > person pin > workspace pin > server default**.
//! The person pin applies only when
//! - the workspace is marked as one person's own agent (`person_agents`);
//! - the session key is the agent's own (its pinned chat, canonical,
//!   heartbeats), never an extra tab (`tab-*`: the owner's tabs use the
//!   server default, so the owner never spends a person's plan);
//! - there is no chat pin on that session (an explicit pick wins);
//! - the person chose an item for that tool (the person pin).
//!
//! Then the item must be usable ([`check_usable`]); if not, the App's
//! fallback setting decides: the company plan, or no spawn.
//!
//! Resume: a conversation recorded on a person item runs on it again only
//! while it is usable; otherwise the same fallback rule applies (history is
//! carried into the home it runs in next).

use rusqlite::Connection;

use super::{app_settings, get, person_agent, use_for, AppSettings, Item, PersonAgent};
use crate::llm_accounts::{pins, state, Tool, WalletError};
use crate::skin::wallet_access::Person;

/// Why a person's item can't run their agent right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// The App user no longer exists.
    PrincipalGone,
    /// The principal is an agent's own login, not a person.
    NotAPerson,
    /// The workspace is no longer marked as this person's agent.
    NotTheirAgent,
    /// The person no longer holds the room through the marked App.
    AccessLost,
    /// Another App user holds the room too (single-person rule).
    SharedRoom,
    /// The App's wallet is off.
    WalletOff,
    /// The person lost App access; the item waits 30 days.
    Dormant,
    /// The item was removed.
    Missing,
    /// Not signed in (or the sign-in expired / was revoked).
    NeedsLogin,
}

impl Unusable {
    pub fn code(&self) -> &'static str {
        match self {
            Unusable::PrincipalGone => "person_gone",
            Unusable::NotAPerson => "not_a_person",
            Unusable::NotTheirAgent => "not_their_agent",
            Unusable::AccessLost => "access_lost",
            Unusable::SharedRoom => "shared_room",
            Unusable::WalletOff => "wallet_off",
            Unusable::Dormant => "dormant",
            Unusable::Missing => "item_missing",
            Unusable::NeedsLogin => "needs_login",
        }
    }

    pub fn sentence(&self) -> &'static str {
        match self {
            Unusable::PrincipalGone => "that App user no longer exists",
            Unusable::NotAPerson => "that login is an agent's, not a person's",
            Unusable::NotTheirAgent => "this agent is no longer their personal agent",
            Unusable::AccessLost => "they no longer have this agent's room in the App",
            Unusable::SharedRoom => "another App user also has this agent's room, and a personal subscription only runs an agent nobody else uses",
            Unusable::WalletOff => "the App's wallet is off",
            Unusable::Dormant => "they lost App access",
            Unusable::Missing => "the sign-in was removed",
            Unusable::NeedsLogin => "it needs a new sign-in",
        }
    }
}

/// What skin.db says, read live at the spawn.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub person: Option<Person>,
    /// Other people (not the item's owner) who hold the room.
    pub others_in_room: Vec<String>,
}

/// Can `item` run `mark`'s agent right now?
pub fn check_usable(
    item: Option<&Item>,
    mark: Option<&PersonAgent>,
    facts: &Facts,
    settings: &AppSettings,
) -> Result<(), Unusable> {
    let Some(item) = item else { return Err(Unusable::Missing) };
    let Some(mark) = mark.filter(|m| m.principal_id == item.principal_id) else {
        return Err(Unusable::NotTheirAgent);
    };
    let Some(person) = facts.person.as_ref() else { return Err(Unusable::PrincipalGone) };
    if person.principal_id != item.principal_id {
        return Err(Unusable::PrincipalGone);
    }
    if person.is_agent_login {
        return Err(Unusable::NotAPerson);
    }
    if !settings.enabled {
        return Err(Unusable::WalletOff);
    }
    if item.dormant_since.is_some() {
        return Err(Unusable::Dormant);
    }
    if !person.holds_room_via(&mark.service_id, &mark.project_id) {
        return Err(Unusable::AccessLost);
    }
    if facts.others_in_room.iter().any(|p| p != &item.principal_id) {
        return Err(Unusable::SharedRoom);
    }
    if item.state != state::SIGNED_IN {
        return Err(Unusable::NeedsLogin);
    }
    Ok(())
}

/// A spawn the person layer has a say in.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub tool: Tool,
    pub mark: Option<PersonAgent>,
    /// The person item in play (the person pin, or the resumed one).
    pub item_id: String,
    pub item: Option<Item>,
    /// The principal whose item it is (for the skin read).
    pub principal_id: String,
    /// The App whose settings apply: the mark's, else the item's own.
    pub settings: AppSettings,
    /// A resume of a conversation recorded on a person item.
    pub resume: bool,
}

/// Phase A: does the person layer apply to this spawn? `None` = the
/// existing rules decide, untouched.
pub fn candidate(
    conn: &Connection,
    tool: Tool,
    session_key: &str,
    project_id: Option<&str>,
    conversation_id: Option<&str>,
    is_resume: bool,
) -> Result<Option<Candidate>, WalletError> {
    let mark = match project_id.filter(|p| !p.is_empty()) {
        Some(pid) => person_agent(conn, pid)?,
        None => None,
    };
    if is_resume {
        let Some(Some(id)) = pins::recorded_account(conn, tool, session_key, conversation_id)? else {
            return Ok(None);
        };
        if !super::is_item_id(&id) {
            return Ok(None);
        }
        // An explicit chat pick wins over a recorded person item on resume
        // too (the existing rules carry the history).
        if pins::chat_pin(conn, tool, session_key, conversation_id)?.is_some() {
            return Ok(None);
        }
        let item = get(conn, &id)?;
        let principal_id = item
            .as_ref()
            .map(|i| i.principal_id.clone())
            .or_else(|| mark.as_ref().map(|m| m.principal_id.clone()))
            .unwrap_or_default();
        let svc = mark
            .as_ref()
            .map(|m| m.service_id.clone())
            .or_else(|| item.as_ref().and_then(|i| i.added_via.clone()))
            .unwrap_or_default();
        return Ok(Some(Candidate {
            tool,
            mark,
            item_id: id,
            item,
            principal_id,
            settings: app_settings(conn, &svc)?,
            resume: true,
        }));
    }
    let Some(mark) = mark else { return Ok(None) };
    if session_key.starts_with("tab-") {
        return Ok(None);
    }
    if pins::chat_pin(conn, tool, session_key, conversation_id)?.is_some() {
        return Ok(None);
    }
    let Some(id) = use_for(conn, &mark.principal_id, tool.as_str())? else {
        return Ok(None);
    };
    let item = get(conn, &id)?;
    let settings = app_settings(conn, &mark.service_id)?;
    Ok(Some(Candidate {
        tool,
        principal_id: mark.principal_id.clone(),
        mark: Some(mark),
        item_id: id,
        item,
        settings,
        resume: false,
    }))
}

/// Phase C: what to do.
#[derive(Debug, Clone)]
pub enum Plan {
    /// Run on the person's item.
    Person(Item),
    /// Not usable; the App's fallback is on: the existing rules (company
    /// plan).
    Fallback(Unusable),
    /// Not usable; fallback off: no spawn.
    Refuse(Unusable),
}

pub fn plan(c: &Candidate, facts: &Facts) -> Plan {
    match check_usable(c.item.as_ref(), c.mark.as_ref(), facts, &c.settings) {
        Ok(()) => Plan::Person(c.item.clone().expect("usable means present")),
        Err(why) if c.settings.fallback => Plan::Fallback(why),
        Err(why) => Plan::Refuse(why),
    }
}

/// The refusal a `Refuse` plan returns (409 `person_login_needed`).
pub fn refusal(c: &Candidate, facts: &Facts, why: &Unusable) -> WalletError {
    let who = facts.person.as_ref().map(|p| p.first_name()).unwrap_or_else(|| "this person".into());
    let label = c.item.as_ref().map(|i| format!(" ({})", i.label)).unwrap_or_default();
    let next = match why {
        Unusable::NeedsLogin => format!(" {who} signs in again from the App."),
        _ => String::new(),
    };
    WalletError::Conflict(
        "person_login_needed",
        format!(
            "This is {who}'s personal agent and runs only on their own {} subscription{label}, which can't be used: {}.{next} The App's \"fall back to the company plan\" setting is off.",
            c.tool.display(),
            why.sentence()
        ),
    )
}
