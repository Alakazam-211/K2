//! The owner's decide token, minted by this server: every approval happens
//! inside K2, with nothing to set up on the Postal website. The plane's
//! `POST /decide-token` takes the box's Connect credential and answers the
//! `k2c_pd_` token for the box's label, once; it is kept where owner-link
//! keeps it (the vault's `system` scope, [`k2_core::postal::keys`]), never
//! logged and never readable by an agent.
//!
//! The rule: **if an invite can be sent, it can be accepted.** So the
//! token is asked for whenever this server would otherwise hold a Postal
//! friendship it can't finish:
//! - the owner turns Postal on (off to on: `k2 postal enable`, its card,
//!   or the Settings switch);
//! - the owner's Link now (Settings → General → Postal, `k2 postal link`);
//! - a person accepts a friend request and no token is here (one try, then
//!   the accept goes on in the same action);
//! - unattended, with no token here: the first good pass of a server that
//!   already had Postal on when it started ([`on_start`]; an upgrade has no
//!   off-to-on switch), an invite or a claim before it is recorded or a
//!   card goes up ([`ready`]), an invite before it goes out on the network
//!   (`pairing::send_unsent`), and a friend request arriving (its card then
//!   says this server can't accept yet, and that Accept links first).
//!
//! The plane holds one token at a time: `409 already_minted` means it has
//! one. With ours stored, that is ours and it stays. With none stored (it
//! was lost), the old one is revoked and a new one minted only when a
//! person asked ([`Why::by_person`]); an unattended ask answers
//! [`CallError::LinkLost`] and leaves the plane's alone. `429 rate_limited`
//! and `401` are answered as they are, never retried; a 5xx is tried again
//! twice, after [`RETRY_PAUSES`], on the owner's switch and Link now only.
//!
//! **No retry storm:** asks run one at a time, and an unattended ask after a
//! failed one waits ([`unattended_wait`]: the plane's `retry_after_s` on a
//! 429, an hour after a 401 or a missing Connect sign-in, five minutes after
//! anything else) and answers the last failure without calling the plane. A
//! person's ask waits only out a 429. Turning Postal off revokes the token
//! on the plane and removes it from the vault.
//!
//! The fix every card, Settings line and error offers is Link now (the
//! server gets the token itself, [`LINK_NOW`]), never a page on the Postal
//! website. `k2 postal owner-link` stays a by-hand CLI fallback, named only
//! after a mint failed, with why it failed ([`failed_hint`]).
//!
//! Every mint tells the owner: one line on the owner's Thread and an audit
//! row, with the label and never the token.
//!
//! Known limit: agents on one server run as the same OS user, so a file
//! read by one agent's process isn't kept from another's; the token in the
//! vault is no more exposed than other system secrets. Per-agent OS
//! isolation closes that later.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::postal::sdk::plane::{Code, PlaneError};
use k2_core::{cli_name, product_name};
use serde_json::{json, Value};

use super::off_async;
use super::plane::{CallError, Mint, Plane};

/// The waits before the second and third try after a 5xx (the owner's
/// switch and Link now only).
pub const RETRY_PAUSES: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

/// The fix, in the words every card, Settings line and error uses.
pub const LINK_NOW: &str = concat!(
    "the owner presses Link now in Settings → General → Postal, or runs ",
    cli_name!(),
    " postal link (this server gets its approval token from the Postal network with its ",
    product_name!(),
    " Connect account)"
);

/// Why a mint is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The owner turned Postal on.
    SwitchOn,
    /// A person accepted a friend request and no token was here.
    Accept,
    /// The owner's Link now.
    LinkNow,
    /// Postal was on when this server started, and no token is here.
    Start,
    /// A friend request arrived, and no token is here.
    Request,
    /// An invite, before it is recorded, its card goes up, or it goes out.
    Invite { person: bool },
    /// A claim, before it is recorded or its card goes up.
    Claim { person: bool },
}

impl Why {
    fn words(self) -> &'static str {
        match self {
            Why::SwitchOn => "Postal turned on",
            Why::Accept => "a friend request accepted",
            Why::LinkNow => "the owner's Link now",
            Why::Start => "Postal was on when this server started",
            Why::Request => "a friend request arrived",
            Why::Invite { .. } => "a friend request about to go out",
            Why::Claim { .. } => "a Postal address claimed",
        }
    }

    /// Did a person ask (a click or a command), not the server itself or an
    /// agent's call? Only a person's ask replaces a token the plane holds
    /// and this server lost, and only a person's ask skips the wait after
    /// a failure (except a 429's).
    pub fn by_person(self) -> bool {
        match self {
            Why::SwitchOn | Why::Accept | Why::LinkNow => true,
            Why::Start | Why::Request => false,
            Why::Invite { person } | Why::Claim { person } => person,
        }
    }

    /// Is a 5xx tried again (after [`RETRY_PAUSES`])?
    fn retries(self) -> bool {
        matches!(self, Why::SwitchOn | Why::LinkNow)
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
    matches!(e, CallError::Plane(PlaneError::Api { status, .. }) if *status >= 500)
}

fn rate_limited(e: &CallError) -> bool {
    matches!(e, CallError::Plane(PlaneError::Api { status, code, .. }) if *status == 429 || *code == Code::RateLimited)
}

fn mint(net: &Plane, why: Why) -> Result<Mint, CallError> {
    let pauses: &[Duration] = if why.retries() { &RETRY_PAUSES } else { &[] };
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

// ── the wait after a failure ─────────────────────────────────────────

/// The last failed ask: until when an unattended ask waits, until when
/// every ask waits (a 429), and what it answered.
struct Failed {
    unattended_until: Instant,
    everyone_until: Option<Instant>,
    err: CallError,
}

/// One ask at a time (the owner's switch and the loop's start can race).
static ASKING: Mutex<()> = Mutex::new(());
static LAST_FAILED: Mutex<Option<Failed>> = Mutex::new(None);
/// The start check ran (once per process).
static START_DONE: AtomicBool = AtomicBool::new(false);

/// How long an unattended ask waits after `e`.
pub fn unattended_wait(e: &CallError) -> Duration {
    if rate_limited(e) {
        return rate_wait(e);
    }
    match e {
        CallError::Plane(PlaneError::Api { status: 401, .. }) | CallError::NoConnect(_) | CallError::LinkLost(_) => Duration::from_secs(3600),
        CallError::Plane(PlaneError::Api { code: Code::Unauthorized, .. }) => Duration::from_secs(3600),
        _ => Duration::from_secs(300),
    }
}

/// A 429's wait: the plane's `retry_after_s`, else an hour.
fn rate_wait(e: &CallError) -> Duration {
    let secs = match e {
        CallError::Plane(p) => p.retry_after(),
        _ => None,
    };
    Duration::from_secs(secs.unwrap_or(3600).clamp(1, 86_400))
}

fn note_failed(e: &CallError) {
    let now = Instant::now();
    let f = Failed { unattended_until: now + unattended_wait(e), everyone_until: rate_limited(e).then(|| now + rate_wait(e)), err: e.clone() };
    *LAST_FAILED.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
}

fn clear_failed() {
    *LAST_FAILED.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// The last failure, while an ask by `why` still waits on it.
fn waiting(why: Why) -> Option<CallError> {
    let g = LAST_FAILED.lock().unwrap_or_else(|p| p.into_inner());
    let f = g.as_ref()?;
    let now = Instant::now();
    let until = if why.by_person() { f.everyone_until? } else { f.unattended_until };
    (now < until).then(|| f.err.clone())
}

/// Tests: forget the last failure and the start check.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    clear_failed();
    START_DONE.store(false, Ordering::SeqCst);
}

// ── asking ───────────────────────────────────────────────────────────

/// Ask the plane for the owner's decide token and keep it (see the module
/// notes for when). `actor` is who asked, for the audit row.
pub fn ensure(net: &Plane, why: Why, actor: &str) -> Result<Got, CallError> {
    let _one = ASKING.lock().unwrap_or_else(|p| p.into_inner());
    ensure_locked(net, why, actor)
}

fn ensure_locked(net: &Plane, why: Why, actor: &str) -> Result<Got, CallError> {
    if let Some(e) = waiting(why) {
        return Err(e);
    }
    let r = ask(net, why, actor);
    match &r {
        Ok(_) => clear_failed(),
        Err(e) => note_failed(e),
    }
    r
}

fn ask(net: &Plane, why: Why, actor: &str) -> Result<Got, CallError> {
    let label = net.connect_label()?;
    let have = k2_core::postal::keys::decide_token(&label).map_err(CallError::NoConnect)?.is_some();
    let got = match mint(net, why)? {
        Mint::Minted { token, label } => {
            store(&label, &token)?;
            Got::Minted { label }
        }
        Mint::AlreadyMinted if have => return Ok(Got::Kept { label }),
        // Lost here and held there: only a person's ask replaces it.
        Mint::AlreadyMinted if !why.by_person() => return Err(CallError::LinkLost(label)),
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

/// Does this server hold its decide token? No plane call: the Connect
/// label from this server's settings, and the vault.
pub fn held(net: &Plane) -> Result<bool, CallError> {
    let label = net.connect_label()?;
    Ok(k2_core::postal::keys::decide_token(&label).map_err(CallError::NoConnect)?.is_some())
}

/// Can people here accept friend requests? With no token here, ask for one
/// first, once, by `why`'s rules (an unattended ask after a failure answers
/// that failure without calling the plane). `Ok`: the token is here.
pub fn ready(net: &Plane, why: Why, actor: &str) -> Result<(), CallError> {
    off_async(|| {
        let _one = ASKING.lock().unwrap_or_else(|p| p.into_inner());
        if held(net)? {
            return Ok(());
        }
        ensure_locked(net, why, actor).map(|_| ())
    })
}

/// A person's Accept: when no token is here for the handle's label, ask
/// for one, once (the accept then goes on in the same action).
pub fn ensure_for_accept(net: &Plane, label: &str, actor: &str) -> Result<(), CallError> {
    if k2_core::postal::keys::decide_token(label).map_err(CallError::NoConnect)?.is_some() {
        return Ok(());
    }
    ready(net, Why::Accept, actor)
}

/// The server's first good Postal pass with Postal on: a server that had
/// Postal on before this release never sees an off-to-on switch, so its
/// token is asked for here, once per process (and only when none is here).
pub fn on_start(net: &Plane) {
    if START_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(e) = ready(net, Why::Start, "server") {
        k2_core::log_debug!("[postal] approval token at start: {}", e.code());
    }
}

/// The owner's Link now (`POST /cli/postal/settings {link: true}`).
pub fn link_now(actor: &str) -> Result<Got, CallError> {
    off_async(|| {
        let net = super::net().ok_or_else(|| CallError::Transport("this server has no Postal network client".into()))?;
        ensure(&net, Why::LinkNow, actor)
    })
}

/// Are approvals linked here? `None` when it can't be told (no Postal
/// network client, no Connect sign-in, the vault closed). No plane call.
pub fn linked() -> Option<bool> {
    let net = super::net()?;
    held(&net).ok()
}

// ── plain words ──────────────────────────────────────────────────────

/// Why a mint failed, in plain words, with nothing the plane wrote that
/// could carry a link (no URL ever reaches a card or an error).
pub fn plain_why(e: &CallError) -> String {
    match e {
        _ if rate_limited(e) => format!(
            "the Postal network says this server asked too often; it can ask again in about {} minutes",
            rate_wait(e).as_secs().div_ceil(60)
        ),
        CallError::Plane(PlaneError::Api { status: 401, .. }) | CallError::Plane(PlaneError::Api { code: Code::Unauthorized, .. }) => {
            format!("the Postal network didn't accept this server's {} Connect account (401)", product_name!())
        }
        CallError::Plane(PlaneError::Api { status, .. }) if *status >= 500 => format!("the Postal network had a problem (error {status})"),
        CallError::Plane(PlaneError::Api { status, .. }) => format!("the Postal network refused it ({}, error {status})", e.code()),
        CallError::Plane(_) => "the Postal network's answer wasn't understood".into(),
        CallError::Transport(_) => "the Postal network didn't answer".into(),
        CallError::NoConnect(why) => why.clone(),
        CallError::NoOwnerLink(_) => "this server has no approval token".into(),
        CallError::LinkLost(_) => "the Postal network holds an approval token for this server that this server no longer has".into(),
    }
}

/// The kinds of failure a card can name (a card keeps the kind, never free
/// text, so nobody can put words of their own on it).
pub const KINDS: &[&str] = &["rate_limited", "unauthorized", "no_connect", "network", "lost", "refused"];

/// A failure's kind (one of [`KINDS`]).
pub fn kind(e: &CallError) -> &'static str {
    match e {
        _ if rate_limited(e) => "rate_limited",
        CallError::Plane(PlaneError::Api { status: 401, .. }) | CallError::Plane(PlaneError::Api { code: Code::Unauthorized, .. }) => "unauthorized",
        CallError::NoConnect(_) => "no_connect",
        CallError::LinkLost(_) => "lost",
        CallError::Transport(_) => "network",
        CallError::Plane(PlaneError::Api { status, .. }) if *status >= 500 => "network",
        _ => "refused",
    }
}

/// A kind, in plain words.
pub fn kind_words(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "rate_limited" => "the Postal network says this server asked too often; it can ask again later",
        "unauthorized" => concat!("the Postal network didn't accept this server's ", product_name!(), " Connect account"),
        "no_connect" => concat!("this server isn't signed in to ", product_name!(), " Connect, or its vault isn't open"),
        "network" => "the Postal network didn't answer, or had a problem",
        "lost" => "the Postal network holds an approval token for this server that this server no longer has",
        "refused" => "the Postal network refused to make one",
        _ => return None,
    })
}

/// What a person reads after a mint failed: what is missing, why it
/// failed, and what to do (Link now again; the by-hand CLI fallback last).
pub fn failed_hint(e: &CallError) -> String {
    format!(
        "this server couldn't get its Postal approval token, so nobody here can accept friend requests yet: {}. Try again with Link now in Settings → General → Postal (or {c} postal link); if it keeps failing, the owner can link approvals by hand with {c} postal owner-link",
        plain_why(e),
        c = cli_name!()
    )
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
    answer(&ensure(&net, Why::SwitchOn, actor))
}

/// An answer's `decideToken`: what came of asking (never the token).
pub fn answer(r: &Result<Got, CallError>) -> Value {
    match r {
        Ok(Got::Minted { label }) => json!({ "state": "minted", "label": label, "hint": "Postal approvals are linked for this server" }),
        Ok(Got::Replaced { label }) => json!({ "state": "replaced", "label": label, "hint": "Postal approvals are linked again for this server (the old token was revoked)" }),
        Ok(Got::Kept { label }) => json!({ "state": "kept", "label": label, "hint": "this server already holds its Postal approval token; it stays" }),
        Err(e) => json!({ "state": "failed", "code": e.code(), "hint": failed_hint(e) }),
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
