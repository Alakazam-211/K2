//! Who an approval is for. A person is a Connect login, kept by its stable
//! id (`ConnectUser.id`, SA-12), never by its name. Agents, App guests,
//! Garden widgets and K2 terminal cells are never asked.
//!
//! "The person the agent works for" (Rosson's question 3) is this server's
//! owner: the logins the owner linked (`ownerAccounts`). With no `--to`,
//! an agent asks the owner's primary linked login.

use k2_core::connect_users::Role;
use k2_core::notify_prefs::{People, OWNER};

use crate::cli_response::CliResponse;
use crate::permission::answer::err;
use k2_core::brand::CLI_NAME;

/// A person who can be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Person {
    /// `ConnectUser.id`.
    pub id: String,
    /// The login name now.
    pub login: String,
    /// The owner person (a login the owner linked): the person every agent
    /// on this server works for.
    pub works_for: bool,
}

/// The login's facts: `(id, role, disabled)`.
fn facts(login: &str) -> Result<Option<(String, Role, bool)>, String> {
    let all = k2_core::connect_users::admin_space_facts()?;
    Ok(all
        .into_iter()
        .find(|(_, name, _, _)| name.eq_ignore_ascii_case(login))
        .map(|(id, _, role, disabled)| (id, role, disabled)))
}

fn works_for(login: &str) -> bool {
    match People::load() {
        Ok(p) => p.person_key(login) == OWNER,
        Err(e) => {
            k2_core::log_debug!("[approve] people load failed: {e}");
            false
        }
    }
}

/// The person `--to` names (`None` or `owner`: the owner's primary linked
/// login). Refused: no such login, a disabled one, a Voice & Chat only one
/// (they can't answer), or an owner with no linked login.
pub(crate) fn resolve(to: Option<&str>) -> Result<Person, CliResponse> {
    let named = to.map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(OWNER));
    let login = match named {
        Some(l) => l.to_ascii_lowercase(),
        None => k2_core::person_name::owner_login_now().ok_or_else(|| {
            err(
                404,
                "approval_no_person",
                "this server's owner has no login linked, so there is nobody to ask by default; name a person with --to <login>",
            )
        })?,
    };
    let Some((id, role, disabled)) = facts(&login).map_err(|e| err(500, "approval_store", e))? else {
        return Err(err(404, "approval_no_person", format!("no login named {login} on this server ({CLI_NAME} people lists them)")));
    };
    if disabled {
        return Err(err(409, "approval_no_person", format!("{login}'s login is turned off")));
    }
    if role < Role::Member {
        return Err(err(409, "approval_no_person", format!("{login} has a Voice & Chat only login and can't approve")));
    }
    if id.is_empty() {
        return Err(err(500, "approval_no_person", format!("{login}'s login has no id yet; try again in a moment")));
    }
    let works_for = works_for(&login);
    Ok(Person { id, login, works_for })
}

/// The stable id of a signed-in login (`None`: unknown or turned off).
pub(crate) fn id_of_login(login: &str) -> Option<String> {
    match facts(login) {
        Ok(Some((id, _, false))) if !id.is_empty() => Some(id),
        Ok(_) => None,
        Err(e) => {
            k2_core::log_debug!("[approve] login facts for {login}: {e}");
            None
        }
    }
}

/// The person key a login's sockets and pushes carry (`owner` for a login
/// the owner linked).
pub(crate) fn person_key(login: &str) -> String {
    crate::connect_device_routes::person_for_actor(&format!("user:{login}"))
}
