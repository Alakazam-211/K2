//! The owner's decide token, minted by this server: every approval happens
//! inside K2, with nothing to set up on the Postal website. The plane's
//! `POST /decide-token` takes the box's Connect credential and answers the
//! `k2c_pd_` token for the box's label, once; it is kept where owner-link
//! keeps it (the vault's `system` scope, [`k2_core::postal::keys`]), never
//! logged and never readable by an agent.
//!
//! **When it is minted, and only then:**
//! - the owner turns Postal on (off to on: `k2 postal enable`, its card,
//!   or the Settings switch);
//! - a person accepts a friend request and no token is here (one try).
//!
//! The plane holds one token at a time: `409 already_minted` means it has
//! one. With ours stored, that is ours and it stays. With none stored (it
//! was lost), the old one is revoked and a new one minted, in those two
//! moments only, never on a timer. `429 rate_limited` and `401` are
//! answered as they are, never retried; a 5xx is tried again twice, after
//! [`RETRY_PAUSES`], on the owner's switch. Turning Postal off revokes the
//! token on the plane and removes it from the vault. `k2 postal owner-link`
//! stays the way to link approvals by hand.
//!
//! Every mint tells the owner: one line on the owner's Thread and an audit
//! row, with the label and never the token.
//!
//! Known limit: agents on one server run as the same OS user, so a file
//! read by one agent's process isn't kept from another's; the token in the
//! vault is no more exposed than other system secrets. Per-agent OS
//! isolation closes that later.

use std::time::Duration;

use serde_json::{json, Value};

use super::off_async;
use super::plane::{CallError, Mint, Plane};

/// The waits before the second and third try after a 5xx (the switch only).
pub const RETRY_PAUSES: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

/// Why a mint is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The owner turned Postal on.
    SwitchOn,
    /// A person accepted a friend request and no token was here.
    Accept,
}

impl Why {
    fn words(self) -> &'static str {
        match self {
            Why::SwitchOn => "Postal turned on",
            Why::Accept => "a friend request accepted",
        }
    }
}

/// What came of asking for the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    /// A new token, stored for `label`.
    Minted { label: String },
    /// The plane has one and this server holds it: kept.
    Kept { label: String },
    /// The plane had one this server lost: revoked, and a new one stored.
    Replaced { label: String },
}

/// Is this the plane's 5xx (tried again on the switch)? Not a rate limit,
/// not a transport failure (each of those already took its timeout).
fn worth_another_try(e: &CallError) -> bool {
    matches!(e, CallError::Plane(k2_core::postal::sdk::plane::PlaneError::Api { status, .. }) if *status >= 500)
}

fn mint(net: &Plane, why: Why) -> Result<Mint, CallError> {
    let pauses: &[Duration] = if why == Why::SwitchOn { &RETRY_PAUSES } else { &[] };
    let mut tries = pauses.iter();
    loop {
        match net.decide_token_mint() {
            Err(e) if worth_another_try(&e) => match tries.next() {
                Some(d) => std::thread::sleep(*d),
                None => return Err(e),
            },
            r => return r,
        }
    }
}

fn store(label: &str, token: &str) -> Result<(), CallError> {
    k2_core::postal::keys::set_decide_token(label, token).map_err(CallError::NoConnect)
}

/// Ask the plane for the owner's decide token and keep it (see the module
/// notes for when). `actor` is who asked, for the audit row.
pub fn ensure(net: &Plane, why: Why, actor: &str) -> Result<Got, CallError> {
    let label = net.connect_label()?;
    let have = k2_core::postal::keys::decide_token(&label).map_err(CallError::NoConnect)?.is_some();
    let got = match mint(net, why)? {
        Mint::Minted { token, label } => {
            store(&label, &token)?;
            Got::Minted { label }
        }
        Mint::AlreadyMinted if have => return Ok(Got::Kept { label }),
        Mint::AlreadyMinted => {
            net.decide_token_revoke()?;
            match mint(net, why)? {
                Mint::Minted { token, label } => {
                    store(&label, &token)?;
                    Got::Replaced { label }
                }
                Mint::AlreadyMinted => {
                    return Err(CallError::NoConnect(
                        "the Postal network still holds an approval token for this server after revoking it: try again in a minute".into(),
                    ))
                }
            }
        }
    };
    tell_owner(&got, why, actor);
    Ok(got)
}

/// A person's Accept: when no token is here for the handle's label, ask
/// for one, once.
pub fn ensure_for_accept(net: &Plane, label: &str, actor: &str) -> Result<(), CallError> {
    if k2_core::postal::keys::decide_token(label).map_err(CallError::NoConnect)?.is_some() {
        return Ok(());
    }
    off_async(|| ensure(net, Why::Accept, actor)).map(|_| ())
}

/// The answer's `decideToken` for the owner's switch-off.
pub fn on_switch_off(actor: &str) -> Value {
    off_async(|| match super::net() {
        Some(net) => revoke(&net, actor),
        None => json!({ "state": "failed", "code": "postal_unreachable", "hint": "this server has no Postal network client" }),
    })
}

/// The owner turned Postal off: revoke the token on the plane, then forget
/// it here (also when the plane can't be reached: the next switch-on finds
/// the plane's old one and replaces it).
pub fn revoke(net: &Plane, actor: &str) -> Value {
    let label = match net.connect_label() {
        Ok(l) => l,
        Err(e) => return json!({ "state": "failed", "code": e.code(), "hint": e.hint() }),
    };
    let on_plane = net.decide_token_revoke();
    let forgot = k2_core::postal::keys::clear_decide_token(&label);
    let outcome = match (&on_plane, &forgot) {
        (Ok(n), Ok(())) => format!("label={label} revoked ({n} on the network) and removed"),
        (Err(e), Ok(())) => format!("label={label} removed here; not revoked on the network: {}", e.code()),
        (_, Err(e)) => format!("label={label} not removed here: {e}"),
    };
    crate::hire_routes::audit("postal-decide-token", actor, &outcome);
    match (on_plane, forgot) {
        (Ok(n), Ok(())) => json!({ "state": "revoked", "label": label, "revoked": n }),
        (Err(e), Ok(())) => json!({ "state": "removed", "label": label, "code": e.code(), "hint": e.hint() }),
        (_, Err(e)) => json!({ "state": "failed", "label": label, "code": "postal_vault", "hint": e }),
    }
}

/// The answer's `decideToken` for the owner's switch-on.
pub fn on_switch_on(actor: &str) -> Value {
    off_async(|| switch_on(actor))
}

fn switch_on(actor: &str) -> Value {
    let Some(net) = super::net() else {
        return json!({ "state": "failed", "code": "postal_unreachable", "hint": "this server has no Postal network client" });
    };
    match ensure(&net, Why::SwitchOn, actor) {
        Ok(Got::Minted { label }) => json!({ "state": "minted", "label": label, "hint": "Postal approvals are linked for this server" }),
        Ok(Got::Replaced { label }) => json!({ "state": "replaced", "label": label, "hint": "Postal approvals are linked again for this server (the old token was revoked)" }),
        Ok(Got::Kept { label }) => json!({ "state": "kept", "label": label, "hint": "this server already holds its Postal approval token; it stays" }),
        Err(e) => json!({
            "state": "failed",
            "code": e.code(),
            "hint": format!("{} (the owner can also link approvals with {} postal owner-link)", e.hint(), k2_core::cli_name!()),
        }),
    }
}

/// The owner's Thread line and the audit row for a mint.
fn tell_owner(got: &Got, why: Why, actor: &str) {
    let (label, how) = match got {
        Got::Minted { label } => (label, "minted"),
        Got::Replaced { label } => (label, "replaced"),
        Got::Kept { .. } => return,
    };
    crate::hire_routes::audit("postal-decide-token", actor, &format!("label={label} {how} ({})", why.words()));
    let title = format!("Postal approvals linked for {label}");
    let body = format!(
        "{p} got this server's Postal approval token for {label} from the Postal network ({w}), so people here can accept friend requests in {p}. It is kept in {p}'s vault; agents can't read it. Turning Postal off revokes it. Agents on this server aren't isolated from each other yet.",
        p = k2_core::product_name!(),
        w = why.words(),
    );
    file_owner_notice(&title, &body);
}

fn file_owner_notice(title: &str, body: &str) {
    #[cfg(not(test))]
    if let Err(e) = crate::feedback_routes::file_owners_only_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "postal", title, body) {
        k2_core::log_debug!("[postal] owner notice '{title}': {e}");
    }
    #[cfg(test)]
    OWNER_NOTICES.lock().unwrap_or_else(|p| p.into_inner()).push(format!("{title}\n{body}"));
}

/// Tests: the owner notices filed.
#[cfg(test)]
pub static OWNER_NOTICES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
#[path = "decide_token_tests.rs"]
mod tests;
