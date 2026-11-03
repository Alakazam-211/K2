//! Hosted mail forwards: the routes (prd-hostmail-external-forwarding-v1
//! §3.3, §4.1; vs-live FW-A7–FW-A10, FW-A12; Rosson's rulings 2026-10-30 and
//! 2026-11-02).
//!
//! ```text
//! GET  /cli/mail/forward?address=          show one mailbox's forwards     owner, Admin, mail-manage agent
//! POST /cli/mail/forward {address,to,keep?,restartStopped?}
//!                                          set: exactly these destinations owner, Admin, mail-manage agent
//! POST /cli/mail/forward/add {address,to,keep?,restartStopped?}
//!                                          add, leaving the others alone   owner, Admin, mail-manage agent
//! POST /cli/mail/forward/rm {address,to}   remove these, leave the others  owner, Admin, mail-manage agent
//! POST /cli/mail/forward/unset {address}   remove them all                 owner, Admin, mail-manage agent
//! POST /cli/mail/forward/pause {address,to?}   pause one, or all           owner, Admin, mail-manage agent
//! POST /cli/mail/forward/resume {address,to?}  resume one, or all          owner, Admin; agents: own pauses only
//! GET  /cli/mail/forwards[?state=]         every forward on the box        owner, Admin
//! POST /cli/mail/config/forward-new-per-day {perDay}
//!                                          the box's daily cap on new outside addresses  the owner only
//! POST /cli/mail/address/person-link {address, person|null}
//!                                          which box login owns a mailbox  owner, Admin
//! GET|POST /cli/mail/self/forward[...]     the same verbs for the mailbox's own person (a box login linked
//!                                          to it, or the owner); never an agent
//! ```
//!
//! `to` is one address or a list. **The rows are the truth**
//! (`k2_core::mail_forward`); every change writes the Sieve right away
//! through `forward_sync` (and the hourly reconcile repairs anything that
//! drifted).
//!
//! **Same box vs outside** ([`classify_dest`]). A destination is same-box
//! when it is an active mailbox here and its domain's MX doesn't point
//! elsewhere. An active mailbox here whose domain's MX points elsewhere counts
//! as outside (told, guarded), though its mail still lands in that mailbox.
//! Any other address on one of this box's domains is refused: the mail server
//! delivers its own domains itself, whatever their MX says (probe X12), so that
//! mail would bounce here and never reach another provider. Anything else is
//! outside. Rows from before this that can't arrive keep running as they are
//! (bounces pause them) and are listed for the owner
//! (`forward_verify::note_cant_arrive`).
//!
//! **Opt-out** (Rosson 2026-11-02). Every forward goes active at once. An
//! outside address first gets one email saying so, with a stop link
//! ([`tells_destination`] is the one place that decides;
//! `forward_verify::tell_destination` sends it). A failed email is shown and
//! retried and doesn't block. A destination that pressed Stop stays out until
//! the owner or an Admin chooses it again (`restartStopped`). Limits, because
//! nobody has to agree first: [`store::NEW_OUTSIDE_PER_DAY`] new outside
//! addresses per box per day (the owner, and only the owner, can change that,
//! up to [`store::NEW_OUTSIDE_PER_DAY_MAX`]; an agent must not be able to
//! loosen its own limit), [`store::MAX_DESTINATIONS`] per mailbox, and the
//! owner hears about every outside change at once. Past
//! [`store::NOTICES_BURST_PER_HOUR`] notices in an hour a new outside forward
//! waits its turn (`forward_verify`, "Pacing"). Outside forwards keep a local
//! copy by default.
//!
//! **Who** (Rosson 2026-10-30): the owner, an Admin, the mailbox's person,
//! and any agent with mail-manage on ("Allow agents to manage hosted mail
//! on this host") can set, change and remove forwards, outside ones
//! included, with no per-forward owner OK. What still applies: the stop
//! link, the caps, the auto-pause, and the loop, spam and DMARC stops.
//! Resuming after an automatic pause (the destination refusing, the box
//! brake) is a person's call (owner, Admin, the mailbox's person); a stopped
//! destination can't be resumed at all, only chosen again by the owner or an
//! Admin.

use std::collections::HashMap;

use k2_core::mail_forward::{self as store, Forward};
use k2_core::db::schema::MailAddress;
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::email_notify::{self, fail, respond, Fail, R};
use crate::mail::forward_sync::{self, ChangeError};
use crate::mail::forward_verify::{self, OutsideChange, Told};
use crate::mail::hostmail_auth::authorize_mailbox;

pub const FORWARD_PATH: &str = "/cli/mail/forward";
pub const ADD_PATH: &str = "/cli/mail/forward/add";
pub const RM_PATH: &str = "/cli/mail/forward/rm";
pub const UNSET_PATH: &str = "/cli/mail/forward/unset";
pub const PAUSE_PATH: &str = "/cli/mail/forward/pause";
pub const RESUME_PATH: &str = "/cli/mail/forward/resume";
pub const LIST_PATH: &str = "/cli/mail/forwards";
pub const CAP_PATH: &str = "/cli/mail/config/forward-new-per-day";
pub const PERSON_LINK_PATH: &str = "/cli/mail/address/person-link";
pub const SELF_PATH: &str = "/cli/mail/self/forward";
pub const SELF_ADD_PATH: &str = "/cli/mail/self/forward/add";
pub const SELF_RM_PATH: &str = "/cli/mail/self/forward/rm";
pub const SELF_UNSET_PATH: &str = "/cli/mail/self/forward/unset";
pub const SELF_PAUSE_PATH: &str = "/cli/mail/self/forward/pause";
pub const SELF_RESUME_PATH: &str = "/cli/mail/self/forward/resume";

/// The person routes (the dispatcher resolves the person from the token).
pub fn is_self_route(p: &str) -> bool {
    matches!(p, SELF_PATH | SELF_ADD_PATH | SELF_RM_PATH | SELF_UNSET_PATH | SELF_PAUSE_PATH | SELF_RESUME_PATH)
}

/// How many audit rows `show` returns.
const EVENTS_SHOWN: usize = 50;

/// One forward change at a time: the box's daily count of new outside
/// addresses is checked and spent as one step, across the notice emails.
static ADDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ── the caller ──────────────────────────────────────────────────────

/// Who is asking (already through the route floors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner token or an Admin login.
    Owner,
    /// A mail-manage agent (the dispatcher refused the rest).
    Agent { workspace_uuid: String },
    /// A box login (or the owner, `owner`) acting on a mailbox linked to it.
    Person { key: String },
}

impl Caller {
    /// The request's caller on the `/cli/mail/*` admin routes.
    pub fn of_request() -> Caller {
        match crate::caller_workspace::request_principal() {
            Some(p) => Caller::Agent { workspace_uuid: p.workspace_uuid },
            None => Caller::Owner,
        }
    }

    /// The audit actor: `owner` | `agent:<workspace>` | `person:<key>`.
    pub fn actor(&self) -> String {
        match self {
            Caller::Owner => "owner".to_string(),
            Caller::Agent { workspace_uuid } => format!("agent:{}", agent_name(workspace_uuid)),
            Caller::Person { key } => format!("person:{key}"),
        }
    }

    /// Who to record as pausing a forward. An agent by its workspace's id: two
    /// workspaces can share a folder name.
    fn pauser(&self) -> String {
        match self {
            Caller::Agent { workspace_uuid } => format!("agent-ws:{workspace_uuid}"),
            other => other.actor(),
        }
    }

    fn is_agent(&self) -> bool {
        matches!(self, Caller::Agent { .. })
    }

    /// Who asked, for the confirm mail (`owner` | a person key |
    /// `agent:<name>`).
    fn asker(&self) -> String {
        match self {
            Caller::Owner => k2_core::notify_prefs::OWNER.to_string(),
            Caller::Agent { workspace_uuid } => format!("agent:{}", agent_name(workspace_uuid)),
            Caller::Person { key } => key.clone(),
        }
    }

    /// May this caller override a destination's Stop? The owner or an Admin,
    /// choosing to; never an agent, and not a mailbox's person.
    fn may_restart(&self) -> bool {
        match self {
            Caller::Owner => true,
            Caller::Person { key } => key == k2_core::notify_prefs::OWNER,
            Caller::Agent { .. } => false,
        }
    }

    /// May this caller resume a forward `paused_by` paused for `reason`? An
    /// agent resumes only its own hand pause: a person's stays a person's.
    pub fn may_resume(&self, reason: &str, paused_by: Option<&str>) -> Result<(), Fail> {
        if reason == store::P_AGENT_SEND_OFF {
            // A person may take it over; the sending switch is what an agent answers to.
            return if self.is_agent() {
                Err(fail(
                    403,
                    "agent_send_required",
                    "K2 paused this forward because agent sending is off for this mailbox's workspace; it goes back on when sending does, or the owner, an Admin or the mailbox's person resumes it",
                ))
            } else {
                Ok(())
            };
        }
        if reason == store::P_NOT_TOLD {
            return Err(fail(
                409,
                "held",
                "K2 paused this forward because it can't email the address its link to stop it; it goes back on by itself when that email goes, or remove it (k2 hostmail forward rm)",
            ));
        }
        if reason == store::P_REVOKED {
            return Err(fail(
                409,
                "destination_stopped",
                "that address pressed Stop in K2's email, so this forward can't be resumed; only the owner or an Admin can choose it again (k2 hostmail forward add <mailbox> --to <address> --restart-stopped)",
            ));
        }
        if store::auto_lifts(reason) {
            return Err(fail(
                409,
                "held",
                format!("K2 paused this forward because {}; it resumes by itself when that changes", reason_words(reason)),
            ));
        }
        match self {
            Caller::Agent { .. } if reason == store::P_MANUAL && paused_by != Some(self.pauser().as_str()) => Err(fail(
                403,
                "owner_only",
                format!(
                    "{} paused this forward, so an agent can't turn it back on; the owner, an Admin or the mailbox's person resumes it (k2 hostmail forward resume)",
                    paused_by.map_or("someone".to_string(), forward_verify::actor_words)
                ),
            )),
            Caller::Agent { .. } if reason != store::P_MANUAL => Err(fail(
                403,
                "owner_only",
                format!(
                    "K2 paused this forward because {}; the owner, an Admin or the mailbox's person resumes it (k2 hostmail forward resume)",
                    reason_words(reason)
                ),
            )),
            Caller::Person { .. } if reason == store::P_BOX_BRAKE => Err(fail(
                403,
                "owner_only",
                "every outside forward on this server is paused because too many were refused; the owner or an Admin resumes them",
            )),
            _ => Ok(()),
        }
    }
}

/// The agent's name (its workspace folder), never a path.
pub(crate) fn agent_name(workspace_uuid: &str) -> String {
    crate::workspace_msg::resolve_workspace(workspace_uuid)
        .and_then(|p| std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| workspace_uuid.chars().take(8).collect())
}

/// A pause reason in words.
pub fn reason_words(reason: &str) -> &'static str {
    match reason {
        store::P_DEST_GONE => "the destination says the address doesn't exist",
        store::P_REFUSED => "the destination refused too many forwards as spam or by policy",
        store::P_REVOKED => "the destination asked K2 to stop",
        store::P_DAILY_CAP => "it reached its daily limit",
        store::P_BOX_BRAKE => "too many forwards from this server were refused",
        store::P_RECEIVE_ONLY => "the mailbox's domain is receive-only",
        store::P_AIR_GAP => "air-gap is on",
        store::P_MANUAL => "someone paused it",
        store::P_NOT_TOLD => "K2 hasn't been able to email the address its link to stop this",
        store::P_AGENT_SEND_OFF => "agent sending is off for the workspace whose agent set this forward",
        store::P_UPGRADE => "not forwarding: the mail server needs upgrading: k2 hostmail upgrade",
        _ => "of a rule",
    }
}

// ── helpers ─────────────────────────────────────────────────────────

fn body_json(b: &[u8]) -> R<J> {
    match serde_json::from_slice::<J>(b) {
        Ok(v @ J::Object(_)) => Ok(v),
        Ok(_) => Err(fail(400, "usage", "the body is a JSON object")),
        Err(e) => Err(fail(400, "usage", format!("invalid JSON body: {e}"))),
    }
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn keep_field(v: &J) -> R<Option<bool>> {
    match v.get("keep") {
        None | Some(J::Null) => Ok(None),
        Some(J::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(fail(400, "usage", "'keep' is true or false")),
    }
}

/// `CliResponse` refusals from `hostmail_auth` → [`Fail`] (codes kept).
fn from_cli(r: CliResponse) -> Fail {
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let code = v.pointer("/error/code").and_then(J::as_str).unwrap_or("failed");
    let hint = v.pointer("/error/hint").and_then(J::as_str).unwrap_or("refused").to_string();
    let status: u16 = r.status.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(500);
    let code: &'static str = match code {
        "usage" => "usage",
        "not_found" => "not_found",
        "forbidden" => "forbidden",
        "not_ready" => "not_ready",
        "engine" => "engine",
        "owner_only" => "owner_only",
        _ => "failed",
    };
    fail(status, code, hint)
}

fn change_fail(e: ChangeError) -> Fail {
    match e {
        ChangeError::Row(store::TransitionError::NotFound) => fail(404, "not_found", "no such forward"),
        ChangeError::Row(store::TransitionError::Illegal(from, to)) => {
            fail(409, "bad_state", format!("the forward is {from}; it can't become {to}"))
        }
        ChangeError::Row(store::TransitionError::Db(e)) => email_notify::db_fail(e),
        ChangeError::Sieve(_, e) => fail(502, "engine", format!("saved, but the mail server wasn't updated yet ({e}); K2 retries within the hour")),
    }
}

/// The change happened; a deferred Sieve write is reported, not failed.
fn changed(r: Result<Forward, ChangeError>) -> R<(Forward, Option<String>)> {
    match r {
        Ok(f) => Ok((f, None)),
        Err(ChangeError::Sieve(f, e)) => Ok((*f, Some(format!("saved; the mail server will be updated within the hour ({})", email_notify::send::mask_addresses(&e))))),
        Err(e) => Err(change_fail(e)),
    }
}

fn now() -> i64 {
    email_notify::now()
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>) -> R<T> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn).map_err(email_notify::db_fail)
}

/// Is `domain` one of this box's mail domains (or its mail host)?
fn hosted_domain(domain: &str) -> bool {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if forward_sync::mail_host().is_some_and(|h| h == d) {
        return true;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM mail_domains WHERE lower(domain) = ?1", rusqlite::params![d], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .unwrap_or(true) // fail closed: an unreadable store refuses outside forwards
}

#[cfg(test)]
thread_local! {
    static TEST_MX: std::cell::RefCell<std::collections::HashMap<String, Vec<String>>> = std::cell::RefCell::new(Default::default());
}

/// Tests: `domain`'s MX hosts (default: no records).
#[cfg(test)]
pub(crate) fn set_test_mx(domain: &str, hosts: &[&str]) {
    TEST_MX.with(|m| m.borrow_mut().insert(domain.to_string(), hosts.iter().map(|h| h.to_string()).collect()));
}

/// `domain`'s MX hosts. `None`: no answer (no records, or DNS failed).
fn mx_hosts(domain: &str) -> Option<Vec<String>> {
    #[cfg(test)]
    {
        TEST_MX.with(|m| m.borrow().get(domain).cloned())
    }
    #[cfg(not(test))]
    {
        use crate::mail::dns_verify::DnsResolver;
        let resolver = crate::mail::dns_verify::SystemResolver::new().ok()?;
        let hosts = resolver.mx(domain).ok()?;
        Some(hosts.into_iter().map(|h| h.exchange).collect())
    }
}

/// Does `domain`'s MX positively point away from this box? No MX, a failed
/// lookup, air-gap or no mail host name answer no: K2 only counts a hosted
/// domain as outside on a real answer, and mail to a mailbox on this box
/// stays on this box either way.
fn mx_points_elsewhere(domain: &str) -> bool {
    if email_notify::airgap_blocks() {
        return false;
    }
    let Some(host) = forward_sync::ctx().host else { return false };
    match mx_hosts(domain) {
        Some(hosts) if !hosts.is_empty() => !hosts.iter().any(|h| crate::mail::dns_verify::host_eq(h, &host)),
        _ => false,
    }
}

/// The kind of destination `dest` is for `mailbox` (FW-A10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    Internal(String),
    External(String),
}

impl Dest {
    pub fn address(&self) -> &str {
        match self {
            Dest::Internal(d) | Dest::External(d) => d,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Dest::Internal(_) => store::KIND_INTERNAL,
            Dest::External(_) => store::KIND_EXTERNAL,
        }
    }
}

/// The one place that decides what a new destination goes through before
/// mail flows there. Today (Rosson 2026-11-02): an outside address is told,
/// with a stop link; a same-box one isn't. Nothing asks for a confirm.
pub fn tells_destination(dest: &Dest) -> bool {
    matches!(dest, Dest::External(_))
}

/// Classify and check a destination.
pub fn classify_dest(mailbox: &str, raw: &str) -> R<Dest> {
    let dest = email_notify::compose::strict_address(raw)
        .map_err(|_| fail(400, "usage", format!("'{}' isn't a valid email address", raw.chars().take(120).collect::<String>())))?;
    if dest.eq_ignore_ascii_case(mailbox) {
        return Err(fail(400, "usage", "a mailbox can't forward to itself"));
    }
    let domain = dest.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
    let minted = crate::mail::hostmail_auth::require_active_mint(&dest).is_ok();
    if !minted && hosted_domain(domain) {
        return Err(fail(
            400,
            "usage",
            format!(
                "'{dest}' is not a minted mailbox on this host, and this server hosts {domain}, so mail to it would stay on this server and never reach another provider, whatever the domain's MX says. Create the mailbox here, or forward to an address on a domain this server doesn't host"
            ),
        ));
    }
    if minted && !mx_points_elsewhere(domain) {
        return Ok(Dest::Internal(dest));
    }
    if email_notify::fence::reserved(&dest) {
        return Err(fail(409, "sender_address", "that is one of this server's notification senders; give an address a person reads"));
    }
    Ok(Dest::External(dest))
}

/// `to`: one address or a list, classified, each once (first spelling wins).
fn dests_from(row: &MailAddress, v: &J) -> R<Vec<Dest>> {
    let raw: Vec<&str> = match v.get("to") {
        Some(J::String(t)) if !t.trim().is_empty() => vec![t.trim()],
        Some(J::Array(a)) if !a.is_empty() => {
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                out.push(x.as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| fail(400, "usage", "'to' lists addresses"))?);
            }
            out
        }
        _ => return Err(fail(400, "usage", "missing 'to': the destination (a mailbox here, or an outside address)")),
    };
    let mut out: Vec<Dest> = Vec::with_capacity(raw.len());
    for t in raw {
        let d = classify_dest(&row.address, t)?;
        if !out.iter().any(|o| o.address() == d.address()) {
            out.push(d);
        }
    }
    Ok(out)
}

/// `to` as plain addresses, for the verbs on existing rows.
fn addresses_from(v: &J) -> R<Option<Vec<String>>> {
    let norm = k2_core::email_verify::normalize_address;
    match v.get("to") {
        None | Some(J::Null) => Ok(None),
        Some(J::String(t)) if !t.trim().is_empty() => Ok(Some(vec![norm(t)])),
        Some(J::Array(a)) if !a.is_empty() => {
            a.iter().map(|x| x.as_str().map(norm).ok_or_else(|| fail(400, "usage", "'to' lists addresses"))).collect::<R<Vec<_>>>().map(Some)
        }
        _ => Err(fail(400, "usage", "'to' is an address or a list of addresses")),
    }
}

// ── views ───────────────────────────────────────────────────────────

/// `show`'s and `list`'s words for a row whose mail can't arrive.
fn cant_arrive_words(f: &Forward) -> String {
    format!(
        "mail to {} can't arrive: this server hosts its domain and has no such mailbox, so it bounces here and never reaches another provider. Remove it (k2 hostmail forward rm {} --to {}) or create the mailbox",
        f.dest, f.address, f.dest
    )
}

/// One forward as the CLI and Settings see it.
pub fn forward_json(f: &Forward, now: i64) -> J {
    let host = forward_sync::ctx().host;
    let (verify, counts, cant_arrive) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let verify = f
            .verified_address_id
            .as_deref()
            .and_then(|id| k2_core::email_verify::get(&conn, id).ok().flatten())
            .map(|v| {
                let expired = v.state == k2_core::email_verify::ST_PENDING && v.expires_at <= now;
                json!({
                    "state": if expired { k2_core::email_verify::ST_EXPIRED } else { v.state.as_str() },
                    "expiresAt": (v.state == k2_core::email_verify::ST_PENDING).then_some(v.expires_at),
                    "verifiedAt": v.verified_at,
                })
            });
        let counts = store::counts(&conn, &f.id, now - store::WINDOW_SECS).unwrap_or_default();
        (verify, counts, store::dest_cant_arrive(&conn, &f.dest, host.as_deref()))
    };
    json!({
        "id": f.id,
        "address": f.address,
        "to": f.dest,
        "kind": f.dest_kind,
        "keep": f.keep,
        "state": f.state,
        "pauseReason": f.pause_reason,
        "pauseWords": f.pause_reason.as_deref().map(reason_words),
        "setBy": f.set_by,
        "createdAt": f.created_at,
        "updatedAt": f.updated_at,
        "activatedAt": f.activated_at,
        "pausedAt": f.paused_at,
        "heldBy": f.held_by.as_deref().map(forward_verify::actor_words),
        "verify": verify,
        // What K2 told an outside address (`sent` | `failed` | `due` | `stale`).
        "notice": f.notice_state,
        "noticeAt": f.notice_at,
        "noticeWords": if f.notice_failed_at.is_some() && f.notice_state.as_deref() == Some(store::N_DUE) {
            Some("not on: K2 hasn't been able to email this address its stop link, and it stays off until that email goes; K2 keeps trying".to_string())
        } else {
            f.notice_state.as_deref().and_then(notice_words)
        },
        "cantArrive": cant_arrive.then(|| cant_arrive_words(f)),
        "last24h": {
            "forwarded": counts.queued,
            "delivered": counts.delivered,
            "refused": counts.refused(),
            "refusedNoSuchUser": counts.refused_user,
            "refusedPolicy": counts.refused_policy,
        },
    })
}

/// `show`'s words for what K2 told an outside address, when they matter.
fn notice_words(state: &str) -> Option<String> {
    match state {
        store::N_FAILED => Some("K2 couldn't email this address its stop link yet, so it can't stop this itself; K2 keeps trying, and pauses the forward after a day".to_string()),
        store::N_DUE => Some("not on yet: K2 is pacing its emails, so it will email this address and then turn this on, within minutes or a few hours if many are waiting".to_string()),
        store::N_STALE => Some(format!(
            "not on: it was waiting for a confirm and was set more than {} days ago, so K2 didn't turn it on. It sends nothing. To turn it on, set it again",
            store::LEGACY_ON_WINDOW_SECS / 86_400
        )),
        _ => None,
    }
}

fn show(row: &MailAddress) -> R<J> {
    let now = now();
    let (live, events) = db(|c| Ok((store::live_rows(c, &row.id)?, store::events_for_address(c, &row.id, EVENTS_SHOWN)?)))?;
    let first_active = live.iter().find(|f| f.is_active());
    let fwd = first_active.or(live.first()).map(|f| forward_json(f, now));
    Ok(json!({
        "ok": true,
        "address": row.address,
        // The pre-0.46.7 shape, kept for older CLIs: only an active
        // forward is a forward.
        "to": first_active.map(|f| f.dest.clone()),
        "keep": live.iter().any(|f| f.is_active() && f.keep),
        // The shape before several destinations: the first one.
        "forward": fwd,
        "forwards": live.iter().map(|f| forward_json(f, now)).collect::<Vec<_>>(),
        "maxDestinations": store::MAX_DESTINATIONS,
        "events": events,
    }))
}

// ── the verbs (shared by the admin and person routes) ───────────────

/// `set` (exactly these destinations) or `add` (these as well).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Set,
    Add,
}

/// Can this live row stand for `dest`, as it is? An outside row waiting on an
/// old confirm or stopped can't, and a same-box row only while active.
fn row_stands(f: &Forward, dest: &Dest) -> bool {
    if f.dest != dest.address() || f.dest_kind != dest.kind() {
        return false;
    }
    if f.is_external() {
        // A row waiting its turn stands too: its notice is on the way.
        (f.state != store::ST_PENDING_VERIFY || f.notice_state.as_deref() == Some(store::N_DUE)) && f.pause_reason.as_deref() != Some(store::P_REVOKED)
    } else {
        f.is_active()
    }
}

/// An agent's outside forward on its own mailbox would send its mail past its
/// sending rules, so it needs agent sending on. Fail closed: no workspace on
/// record reads as off.
fn agent_own_mailbox_limit(row: &MailAddress, caller: &Caller) -> R<()> {
    let Caller::Agent { workspace_uuid } = caller else { return Ok(()) };
    if row.owner_project_id != *workspace_uuid {
        return Ok(());
    }
    let path: Option<String> = db(|c| {
        use rusqlite::OptionalExtension;
        c.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![workspace_uuid], |r| r.get(0))
            .optional()
            .map_err(|e| format!("mail forwards: workspace: {e}"))
    })?;
    let mode = path.map_or_else(|| "off".to_string(), |p| k2_core::workspace::settings::mail_agent_send_for_path(&p));
    if mode == "on" {
        return Ok(());
    }
    Err(fail(
        403,
        "agent_send_required",
        format!(
            "this is your own mailbox, and forwarding it outside would send its mail past your sending rules (agent sending is {mode} here). The owner or an Admin sets the forward, or turns sending on: k2 mail sending <workspace> on"
        ),
    ))
}

fn refuse_outside_now(row: &MailAddress) -> R<()> {
    if email_notify::airgap_blocks() {
        return Err(fail(409, "air_gap", "air-gap is on: nothing leaves this server, so it can't forward outside"));
    }
    if !forward_sync::outside_forward_supported() {
        return Err(fail(409, "upgrade_required", forward_sync::UPGRADE_HINT));
    }
    let mb = forward_sync::mailbox(&row.id).map_err(email_notify::db_fail)?.ok_or_else(|| fail(404, "not_found", "no such mailbox"))?;
    if mb.send_mode.as_deref() == Some("receive-only") {
        return Err(fail(
            409,
            "receive_only",
            format!("{}'s domain is receive-only, so its mail can't be forwarded outside (Settings → Email → Sending)", row.address),
        ));
    }
    Ok(())
}

fn set_or_add(row: &MailAddress, caller: &Caller, v: &J, mode: Mode) -> R<J> {
    let dests = dests_from(row, v)?;
    let keep = keep_field(v)?;
    let restart = v.get("restartStopped").and_then(J::as_bool).unwrap_or(false);
    let actor = caller.actor();
    let _adds = ADDS.lock().unwrap_or_else(|e| e.into_inner());
    let now = now();
    let live = db(|c| store::live_rows(c, &row.id))?;
    let outside: Vec<&str> = dests.iter().filter(|d| tells_destination(d)).map(Dest::address).collect();

    // Every refusal comes first: nothing has changed, and nobody was mailed.
    if !outside.is_empty() {
        refuse_outside_now(row)?;
    }
    let stopped: Vec<String> = db(|c| Ok(outside.iter().filter(|d| k2_core::email_verify::is_stopped(c, &row.address, d)).map(|d| d.to_string()).collect()))?;
    if let Some(d) = stopped.first() {
        if !restart {
            return Err(fail(
                409,
                "destination_stopped",
                if caller.may_restart() {
                    format!("{d} pressed Stop in K2's email, so K2 won't forward to it. To override that, add --restart-stopped; ask them first")
                } else {
                    format!("{d} pressed Stop in K2's email, so K2 won't forward to it. Only the owner or an Admin can choose it again")
                },
            ));
        }
        if !caller.may_restart() {
            return Err(fail(403, "owner_only", format!("{d} pressed Stop in K2's email; only the owner or an Admin can choose it again")));
        }
    }
    // Agents' own limits (review findings 6 and 9).
    if caller.is_agent() {
        if !outside.is_empty() {
            agent_own_mailbox_limit(row, caller)?;
        }
        // Same-box forwards too: a person's pause holds either way.
        for d in dests.iter().map(|d| d.address()) {
            let recent = db(|c| store::recent_pause_agents_keep(c, &row.address, d, now - store::WINDOW_SECS))?;
            if let Some(reason) = recent {
                let who = if reason == store::P_MANUAL {
                    "a person paused it".to_string()
                } else {
                    format!("K2 paused it because {}", reason_words(&reason))
                };
                return Err(fail(
                    403,
                    "owner_only",
                    format!(
                        "forwarding from {} to {d} was paused in the last 24 hours ({who}), so an agent can't start it again, removed or not. The owner, an Admin or the mailbox's person can",
                        row.address
                    ),
                ));
            }
        }
    }
    let stays = |f: &Forward| dests.iter().any(|d| d.address() == f.dest);
    let others_left = if mode == Mode::Add { live.iter().filter(|f| !stays(f)).count() } else { 0 };
    if others_left + dests.len() > store::MAX_DESTINATIONS {
        return Err(fail(
            409,
            "too_many_destinations",
            match mode {
                Mode::Add => format!(
                    "a mailbox forwards to at most {} addresses, and this would make {} for {}. Nothing changed. Remove one first: k2 hostmail forward rm {} --to <address>",
                    store::MAX_DESTINATIONS,
                    others_left + dests.len(),
                    row.address,
                    row.address
                ),
                Mode::Set => format!(
                    "a mailbox forwards to at most {} addresses, and this list has {} for {}. Nothing changed. Give at most {}",
                    store::MAX_DESTINATIONS,
                    dests.len(),
                    row.address,
                    store::MAX_DESTINATIONS
                ),
            },
        ));
    }
    let fresh: Vec<&Dest> = dests.iter().filter(|d| !live.iter().any(|f| row_stands(f, d))).collect();
    // A same-box loop would rely on the mail server's own stops (review finding 15).
    for d in fresh.iter().filter(|d| !tells_destination(d)) {
        if db(|c| store::same_box_loop(c, &row.address, d.address()))? {
            return Err(fail(409, "forward_loop", format!("{} already forwards back to {}, directly or through other mailboxes here, so this would loop. Nothing changed", d.address(), row.address)));
        }
    }
    // An opt-out with no stop link isn't an opt-out (review finding 2), and
    // nobody gets the box's whole cap in notices (finding 14).
    for d in fresh.iter().filter(|d| tells_destination(d)) {
        forward_verify::can_tell(row.stalwart_account_id.as_deref(), d.address())?;
        // Sent, and on its way from rows waiting their turn. A person's explicit
        // restart is exempt: an agent spending the limit doesn't lock them out.
        let told = db(|c| Ok(store::notices_to_dest_since(c, d.address(), now - store::WINDOW_SECS)? + store::due_to_dest(c, d.address())?))?;
        if told >= store::NOTICES_PER_DEST_PER_DAY && !stopped.iter().any(|s| s == d.address()) {
            let mut f = fail(
                429,
                "notice_limit",
                format!("K2 has emailed {} about forwards {told} times in the last 24 hours, the most it will. Nothing changed; try tomorrow", d.address()),
            );
            f.retry_after_ms = Some(3_600_000);
            return Err(f);
        }
    }
    let fresh_outside = fresh.iter().filter(|d| tells_destination(d)).count() as i64;
    if fresh_outside > 0 {
        let (used, cap) = db(|c| Ok((store::new_outside_since(c, now - store::WINDOW_SECS)?, store::new_outside_cap(c)?)))?;
        if used + fresh_outside > cap {
            let mut f = fail(
                429,
                "new_outside_limit",
                format!(
                    "this server adds at most {cap} new outside addresses a day: {used} went on in the last 24 hours, and this would add {fresh_outside}. Nothing changed. Add fewer, try later, or forward to a mailbox on this server. Only the owner can raise the limit, up to {}: k2 hostmail config --forward-new-per-day <n>",
                    store::NEW_OUTSIDE_PER_DAY_MAX
                ),
            );
            f.retry_after_ms = Some(3_600_000);
            return Err(f);
        }
    }

    // `set` declares the whole list, with each kind's default; `add` joins
    // what's there, keeping its setting unless this is the first outside one.
    let keep = keep.unwrap_or_else(|| match mode {
        Mode::Set => !outside.is_empty(),
        Mode::Add => live.iter().any(|f| f.keep) || (!outside.is_empty() && !live.iter().any(Forward::is_external)),
    });

    // Tell each new outside address BEFORE its redirect exists. Past the
    // hour's burst, or with others already waiting, it waits its turn.
    let (sent_this_hour, waiting) = db(|c| Ok((store::notices_since(c, now - 3600)?, store::due_waiting(c, now)?)))?;
    let mut room = if waiting > 0 { 0 } else { (store::NOTICES_BURST_PER_HOUR - sent_this_hour).max(0) };
    let asker = caller.asker();
    let mut told: Vec<(String, Told)> = Vec::new();
    let mut queued: Vec<String> = Vec::new();
    for d in fresh.iter().filter(|d| tells_destination(d)) {
        // A restart is a person's explicit choice, and goes now.
        if room <= 0 && !stopped.iter().any(|s| s == d.address()) {
            queued.push(d.address().to_string());
            continue;
        }
        room -= 1;
        let restart = stopped.iter().any(|s| s == d.address());
        told.push((d.address().to_string(), forward_verify::tell_destination(&row.address, row.stalwart_account_id.as_deref(), d.address(), &asker, restart, now)?));
    }

    // One change, one Sieve write.
    let mut change = OutsideChange::default();
    let r = forward_sync::change_and_sync(&row.id, |c, now| {
        let dbe = store::TransitionError::Db;
        let actor_ref = &actor;
        let change_ref = &mut change;
        let mut newest: Option<Forward> = None;
        let mut no_copy_new = false;
        // After the change an outside row (even one just retired) tells
        // `after_change` the webhook may need to follow.
        let mut retired_outside: Option<String> = None;
        if mode == Mode::Set {
            for f in live.iter().filter(|f| !dests.iter().any(|d| d.address() == f.dest)) {
                store::retire(c, f, actor_ref, now).map_err(dbe)?;
                if f.is_external() {
                    change_ref.removed.push(f.dest.clone());
                    retired_outside = Some(f.id.clone());
                }
            }
        }
        for d in fresh.iter() {
            let t = told.iter().find(|(a, _)| a == d.address()).map(|(_, t)| t);
            let waits = queued.iter().any(|q| q == d.address());
            let f = store::add(
                c,
                &store::NewForward {
                    address_id: &row.id,
                    address: &row.address,
                    dest: d.address(),
                    dest_kind: d.kind(),
                    keep,
                    set_by: actor_ref,
                    state: if waits { store::ST_PENDING_VERIFY } else { store::ST_ACTIVE },
                    verified_address_id: t.map(|t| t.verified_id.as_str()),
                },
                now,
            )
            .map_err(dbe)?;
            if let Caller::Agent { workspace_uuid } = caller {
                if d.kind() == store::KIND_EXTERNAL && row.owner_project_id == *workspace_uuid {
                    store::set_agent_ws(c, &f.id, Some(workspace_uuid)).map_err(dbe)?;
                }
            }
            if waits {
                store::set_notice(c, &f.id, store::N_DUE, now).map_err(dbe)?;
                change_ref.queued.push(f.dest.clone());
            }
            if d.kind() == store::KIND_EXTERNAL && !keep {
                no_copy_new = true;
            }
            if let Some(t) = t {
                t.record(c, &f.id, actor_ref, now).map_err(dbe)?;
                if t.via.is_none() {
                    change_ref.not_told.push(f.dest.clone());
                }
                if stopped.iter().any(|s| s == d.address()) {
                    store::record_event(c, &f.id, store::E_RESTARTED, actor_ref, &json!({}), now).map_err(dbe)?;
                    change_ref.restarted.push(f.dest.clone());
                } else {
                    change_ref.added.push(f.dest.clone());
                }
            }
            newest = Some(f);
        }
        let mut keep_moved = false;
        for f in store::live_rows(c, &row.id).map_err(dbe)? {
            keep_moved |= store::set_keep(c, &f.id, keep, actor_ref, now).map_err(dbe)? && f.is_external();
            if newest.as_ref().is_none_or(|n| !n.is_external()) {
                newest = Some(f);
            }
        }
        // A first outside add with no copy kept is a change the owner should hear.
        if keep_moved || no_copy_new {
            change_ref.keep = Some(keep);
        }
        if newest.as_ref().is_none_or(|n| !n.is_external()) {
            if let Some(id) = retired_outside {
                newest = store::get(c, &id).map_err(dbe)?;
            }
        }
        newest.ok_or(store::TransitionError::NotFound)
    });
    let (_, note) = changed(r)?;
    forward_verify::tell_owner_changed(&row.id, &row.address, &actor, &change);

    // The answer: one entry per destination asked for. The top-level fields
    // are the first one's, the shape from before several destinations.
    let rows = db(|c| store::live_rows(c, &row.id))?;
    let mut answers = Vec::with_capacity(dests.len());
    for d in &dests {
        let f = rows.iter().find(|f| f.dest == d.address()).ok_or_else(|| email_notify::db_fail(format!("the row for {} is gone", d.address())))?;
        let t = told.iter().find(|(a, _)| a == d.address()).map(|(_, t)| t);
        let message = match t {
            Some(Told { via: Some(_), .. }) => Some(format!("Forwarding to {} is on. K2 emailed {} to say so, with a link to stop it.", f.dest, f.dest)),
            Some(Told { error, .. }) => Some(format!(
                "Forwarding to {} is on, but K2 couldn't email {} its stop link yet ({}). K2 keeps trying; after a day without that email it pauses this until the email goes. forward list shows it.",
                f.dest,
                f.dest,
                error.as_deref().unwrap_or("the mail server refused")
            )),
            None if queued.iter().any(|q| q == &f.dest) => Some(format!(
                "Forwarding to {} isn't on yet. K2 is pacing its emails, so it will email {} first and then turn this on, within minutes or a few hours if many are waiting.",
                f.dest, f.dest
            )),
            None => None,
        };
        // A mailbox here whose domain's MX points elsewhere: Stalwart delivers
        // its own domains itself, whatever their MX says.
        let local = d.kind() == store::KIND_EXTERNAL && t.is_some() && hosted_domain(d.address().rsplit_once('@').map_or("", |(_, x)| x));
        answers.push(json!({
            "to": f.dest,
            "kind": f.dest_kind,
            "keep": f.keep,
            "state": f.state,
            "new": t.is_some() || fresh.iter().any(|n| n.address() == f.dest),
            "notice": t.map(Told::notice_state),
            "sentVia": t.and_then(|t| t.via),
            "message": message,
            "note": local.then(|| format!("{}'s domain's MX points elsewhere, but this server hosts that domain, so this mail goes to the mailbox on this server, not to the other provider", f.dest)),
        }));
    }
    let first = answers.first().cloned().unwrap_or(J::Null);
    let note = note.or_else(|| first["note"].as_str().map(str::to_string));
    Ok(json!({
        "ok": true,
        "address": row.address,
        "to": first["to"],
        "keep": first["keep"],
        "kind": first["kind"],
        "state": first["state"],
        "sentVia": first["sentVia"],
        "message": first["message"],
        "note": note,
        "destinations": answers,
    }))
}

fn live_or_404(row: &MailAddress) -> R<Vec<Forward>> {
    let live = db(|c| store::live_rows(c, &row.id))?;
    if live.is_empty() {
        return Err(fail(404, "not_found", format!("{} has no forward", row.address)));
    }
    Ok(live)
}

/// The rows `to` names (each must be live), or every live row.
fn targets(row: &MailAddress, v: &J) -> R<Vec<Forward>> {
    let live = live_or_404(row)?;
    let Some(want) = addresses_from(v)? else { return Ok(live) };
    let mut out = Vec::with_capacity(want.len());
    for w in &want {
        let f = live
            .iter()
            .find(|f| f.dest == *w)
            .ok_or_else(|| fail(404, "not_found", format!("{} doesn't forward to {w}", row.address)))?;
        if !out.iter().any(|o: &Forward| o.id == f.id) {
            out.push(f.clone());
        }
    }
    Ok(out)
}

/// Remove `rows` in one change.
fn remove_rows(row: &MailAddress, caller: &Caller, rows: &[Forward]) -> R<(Forward, Option<String>)> {
    let actor = caller.actor();
    let (first, actor_ref) = (rows.first().cloned().ok_or_else(|| fail(404, "not_found", "no such forward"))?, &actor);
    let r = forward_sync::change_and_sync(&row.id, |c, now| {
        let mut last = first;
        for f in rows {
            let done = store::transition(c, &f.id, store::ST_REMOVED, &store::Change { actor: actor_ref, ..Default::default() }, now)?;
            if done.is_external() || !last.is_external() {
                last = done;
            }
        }
        Ok(last)
    });
    let out = changed(r)?;
    let removed: Vec<String> = rows.iter().filter(|f| f.is_external()).map(|f| f.dest.clone()).collect();
    forward_verify::tell_owner_changed(&row.id, &row.address, &actor, &OutsideChange { removed, ..Default::default() });
    Ok(out)
}

fn rm(row: &MailAddress, caller: &Caller, v: &J) -> R<J> {
    if addresses_from(v)?.is_none() {
        return Err(fail(400, "usage", "missing 'to': which destination to remove (k2 hostmail forward unset removes them all)"));
    }
    let rows = targets(row, v)?;
    let (_, note) = remove_rows(row, caller, &rows)?;
    let left = db(|c| store::live_rows(c, &row.id))?;
    Ok(json!({
        "ok": true,
        "address": row.address,
        "removed": rows.iter().map(|f| f.dest.clone()).collect::<Vec<_>>(),
        "remaining": left.iter().map(|f| f.dest.clone()).collect::<Vec<_>>(),
        "note": note,
    }))
}

fn unset(row: &MailAddress, caller: &Caller) -> R<J> {
    let live = db(|c| store::live_rows(c, &row.id))?;
    if live.is_empty() {
        // Nothing to remove: still make sure no forward block is left.
        let note = forward_sync::sync_mailbox(&row.id, forward_sync::Why::Change).err().map(|e| email_notify::send::mask_addresses(&e));
        return Ok(json!({ "ok": true, "address": row.address, "to": J::Null, "removed": false, "note": note }));
    }
    let (f, note) = remove_rows(row, caller, &live)?;
    Ok(json!({ "ok": true, "address": row.address, "to": J::Null, "removed": true, "id": f.id, "count": live.len(), "note": note }))
}

/// One row, the shape from before several destinations; several, a list and
/// what was left alone.
fn state_answer(row: &MailAddress, rows: &[Forward], skipped: Vec<J>, note: Option<String>) -> J {
    let first = rows.first();
    json!({
        "ok": true,
        "address": row.address,
        "to": first.map(|f| f.dest.clone()),
        "state": first.map(|f| f.state.clone()),
        "pauseReason": first.and_then(|f| f.pause_reason.clone()),
        "forwards": rows.iter().map(|f| json!({ "to": f.dest, "state": f.state, "pauseReason": f.pause_reason })).collect::<Vec<_>>(),
        "skipped": skipped,
        "note": note,
    })
}

/// A pause on a row that's already paused still has to mean something. A person's
/// takes over one an agent paused, and on a row K2 holds and lifts by itself it's
/// recorded beside K2's reason: the reason stays, so an agent can't resume past it,
/// and when K2 lets go the row stays paused. An agent never overrides a person.
fn pause_again(f: Forward, caller: &Caller, now: i64) -> R<(Forward, bool)> {
    let reason = f.pause_reason.as_deref().unwrap_or("");
    let lifts = store::auto_lifts(reason) || reason == store::P_NOT_TOLD;
    let me = caller.pauser();
    let (as_hold, moves) = if lifts {
        (true, f.held_by.as_deref() != Some(me.as_str()) && (!caller.is_agent() || f.held_by.is_none()))
    } else {
        (false, !caller.is_agent() && reason == store::P_MANUAL && f.paused_by.as_deref() != Some(me.as_str()))
    };
    if !moves {
        return Ok((f, false));
    }
    let f = db(|c| store::record_pause_on_paused(c, &f.id, &caller.actor(), &me, as_hold, now))?;
    Ok((f, true))
}

fn pause(row: &MailAddress, caller: &Caller, v: &J) -> R<J> {
    let rows = targets(row, v)?;
    let mut done = Vec::with_capacity(rows.len());
    let mut skipped = Vec::new();
    let mut note = None;
    let mut told = Vec::new();
    let one = rows.len() == 1;
    for f in rows {
        if f.state == store::ST_PENDING_VERIFY && !one {
            skipped.push(json!({ "to": f.dest, "why": "it isn't on, so there's nothing to pause" }));
            continue;
        }
        let mut moved = true;
        // The row as it is now, under the lock: K2 may have lifted or set a hold
        // since it was read, and this pause mustn't be lost to that.
        let r = forward_sync::locked_change(&row.id, |at| -> R<Forward> {
            let cur = db(|c| store::get(c, &f.id)?.ok_or_else(|| "the row is gone".to_string()))?;
            if cur.state == store::ST_PAUSED {
                let (cur, m) = pause_again(cur, caller, at)?;
                moved = m;
                return Ok(cur);
            }
            let (actor, pauser) = (caller.actor(), caller.pauser());
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::transition(&conn, &cur.id, store::ST_PAUSED, &store::Change { reason: Some(store::P_MANUAL), actor: &actor, pauser: Some(&pauser), ..Default::default() }, at)
                .map_err(|e| change_fail(ChangeError::Row(e)))
        });
        let (f, n) = changed(r?)?;
        note = note.or(n);
        if moved && f.is_external() {
            told.push(f.dest.clone());
        }
        done.push(f);
    }
    forward_verify::tell_owner_changed(&row.id, &row.address, &caller.actor(), &OutsideChange { paused: told, ..Default::default() });
    Ok(state_answer(row, &done, skipped, note))
}

fn resume(row: &MailAddress, caller: &Caller, v: &J) -> R<J> {
    let rows = targets(row, v)?;
    let one = rows.len() == 1;
    let mut done = Vec::with_capacity(rows.len());
    let mut skipped = Vec::new();
    let mut refused: Option<Fail> = None;
    let mut note = None;
    let mut resumed = Vec::new();
    for f in rows {
        if f.state != store::ST_PAUSED {
            if one {
                return Err(fail(409, "bad_state", format!("the forward is {}, not paused", f.state)));
            }
            continue;
        }
        // Decided on the row as it is now, under the lock: a pause or a hold that
        // landed since it was read counts.
        let mut went_on = false;
        let r = forward_sync::locked_change(&row.id, |at| -> R<Forward> {
            let f = db(|c| store::get(c, &f.id)?.ok_or_else(|| "the row is gone".to_string()))?;
            if f.state != store::ST_PAUSED {
                return Err(fail(409, "bad_state", format!("the forward is {}, not paused", f.state)));
            }
            let reason = f.pause_reason.clone().unwrap_or_default();
            // A hold beside K2's own reason: the one who can take it back does, and
            // K2's reason still decides when it comes back on.
            let k2_decides = reason == store::P_NOT_TOLD || (store::auto_lifts(&reason) && reason != store::P_AGENT_SEND_OFF);
            if k2_decides && f.held_by.is_some() && (!caller.is_agent() || f.held_by.as_deref() == Some(caller.pauser().as_str())) {
                let f = db(|c| store::clear_hold(c, &f.id, at))?;
                note = note.take().or_else(|| Some(format!("Your pause is lifted, but K2 still holds this forward because {}; it comes back by itself when that changes", reason_words(&reason))));
                return Ok(f);
            }
            caller.may_resume(&reason, f.paused_by.as_deref())?;
            if f.is_external() {
                agent_own_mailbox_limit(row, caller)?;
            }
            // Resuming may close a loop that wasn't there when it was set.
            if !f.is_external() && db(|c| store::same_box_loop(c, &row.address, &f.dest))? {
                return Err(fail(409, "forward_loop", format!("{} forwards back to {}, directly or through other mailboxes here, so resuming this would loop. Nothing changed", f.dest, row.address)));
            }
            // A person's resume of a sending hold makes it their forward: otherwise
            // the next pass would pause it again.
            if reason == store::P_AGENT_SEND_OFF {
                db(|c| store::set_agent_ws(c, &f.id, None))?;
            }
            went_on = true;
            let actor = caller.actor();
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::transition(&conn, &f.id, store::ST_ACTIVE, &store::Change { actor: &actor, ..Default::default() }, at).map_err(|e| change_fail(ChangeError::Row(e)))
        });
        let dest = f.dest.clone();
        let r = match r {
            Ok(r) => r,
            Err(no) => {
                skipped.push(json!({ "to": dest, "why": no.hint, "code": no.code }));
                refused = refused.or(Some(no));
                continue;
            }
        };
        let (f, n) = changed(r)?;
        note = note.or(n);
        if went_on && f.is_external() {
            resumed.push(f.dest.clone());
        }
        done.push(f);
    }
    if done.is_empty() {
        return Err(refused.unwrap_or_else(|| fail(409, "bad_state", format!("no forward of {} is paused", row.address))));
    }
    forward_verify::tell_owner_changed(&row.id, &row.address, &caller.actor(), &OutsideChange { resumed, ..Default::default() });
    Ok(state_answer(row, &done, skipped, note))
}

// ── the admin routes (/cli/mail/forward*) ───────────────────────────

fn mailbox_from(v: &J) -> R<MailAddress> {
    let a = s(v, "address").ok_or_else(|| fail(400, "usage", "missing 'address': which hosted mailbox?"))?;
    authorize_mailbox(a).map_err(from_cli)
}

/// GET `/cli/mail/forward?address=`.
pub fn handle_forward_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    respond(mailbox_from(&json!({ "address": address })).and_then(|row| show(&row)))
}

/// `to` before the mailbox: a bad call is a usage error, not a 404.
fn require_to(v: &J) -> R<()> {
    if addresses_from(v)?.is_none() {
        return Err(fail(400, "usage", "missing 'to': the destination (a mailbox here, or an outside address)"));
    }
    Ok(())
}

/// POST `/cli/mail/forward` `{address, to, keep?, restartStopped?}`: exactly
/// these destinations.
pub fn handle_forward_set(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| {
        require_to(&v)?;
        let row = mailbox_from(&v)?;
        set_or_add(&row, &Caller::of_request(), &v, Mode::Set)
    }))
}

/// POST `/cli/mail/forward/add` `{address, to, keep?, restartStopped?}`:
/// these as well, the others untouched.
pub fn handle_forward_add(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| {
        require_to(&v)?;
        let row = mailbox_from(&v)?;
        set_or_add(&row, &Caller::of_request(), &v, Mode::Add)
    }))
}

/// POST `/cli/mail/forward/rm` `{address, to}`: these gone, the others untouched.
pub fn handle_forward_rm(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| rm(&mailbox_from(&v)?, &Caller::of_request(), &v)))
}

/// POST `/cli/mail/forward/unset` `{address}`.
pub fn handle_forward_unset(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| unset(&mailbox_from(&v)?, &Caller::of_request())))
}

/// POST `/cli/mail/forward/pause` `{address, to?}`.
pub fn handle_forward_pause(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| pause(&mailbox_from(&v)?, &Caller::of_request(), &v)))
}

/// POST `/cli/mail/forward/resume` `{address, to?}`.
pub fn handle_forward_resume(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| resume(&mailbox_from(&v)?, &Caller::of_request(), &v)))
}

/// POST `/cli/mail/config/forward-new-per-day` `{perDay}`: how many new
/// outside addresses this box may add a day. The owner only: the route's
/// floor refuses an Admin login, the mail surface refuses a passport (and an
/// agent gets "It can't be requested."), and this refuses a passport again.
/// An agent must never be able to loosen this limit. Lowering it counts too.
pub fn handle_forward_cap(body: &[u8]) -> CliResponse {
    if crate::caller_workspace::request_principal().is_some() {
        return email_notify::fail_response(&fail(403, "owner_only", "only the owner changes how many new outside addresses this server may add a day"));
    }
    respond(body_json(body).and_then(|v| {
        let n = v
            .get("perDay")
            .and_then(J::as_i64)
            .ok_or_else(|| fail(400, "usage", "send {perDay}: how many new outside addresses a day, a whole number"))?;
        if !(0..=store::NEW_OUTSIDE_PER_DAY_MAX).contains(&n) {
            return Err(fail(
                400,
                "usage",
                format!("--forward-new-per-day can be at most {}, and {n} is out of range. Nothing changed", store::NEW_OUTSIDE_PER_DAY_MAX),
            ));
        }
        let before = db(store::new_outside_cap)?;
        if !db(|c| store::set_new_outside_cap(c, n))? {
            return Err(fail(409, "not_ready", "this server has no mail server yet (k2 hostmail enable)"));
        }
        email_notify::audit_line("mail-forward-cap", "owner", &format!("new_outside_per_day={before}->{n}"));
        forward_verify::tell_owner(
            "The daily limit on new forward addresses changed",
            &format!("This server may now add {n} new outside forward addresses a day (it was {before}; the default is {}). This was done with the owner's credentials: if it wasn't you, change it back: k2 hostmail config --forward-new-per-day {before}.", store::NEW_OUTSIDE_PER_DAY),
            &[],
        );
        Ok(json!({
            "ok": true,
            "perDay": n,
            "default": store::NEW_OUTSIDE_PER_DAY,
            "max": store::NEW_OUTSIDE_PER_DAY_MAX,
        }))
    }))
}

/// GET `/cli/mail/forwards[?state=paused|active|pending_verify][&kind=external]`
/// — owner and Admins only.
pub fn handle_forwards_list(params: &HashMap<String, String>) -> CliResponse {
    if crate::caller_workspace::request_principal().is_some() {
        return email_notify::fail_response(&fail(403, "owner_only", "the list of every forward on this server is for the owner and Admins"));
    }
    let state = params.get("state").map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(st) = state {
        if ![store::ST_PENDING_VERIFY, store::ST_ACTIVE, store::ST_PAUSED].contains(&st) {
            return email_notify::fail_response(&fail(400, "usage", "state is pending_verify, active or paused"));
        }
    }
    let external_only = params.get("kind").is_some_and(|k| k.trim() == store::KIND_EXTERNAL);
    let now = now();
    respond(
        db(|c| Ok((store::list(c, &store::ListFilter { state, external_only, include_removed: false })?, store::webhook_error(c))))
            .map(|(rows, webhook_error)| {
                json!({
                    "ok": true,
                    "forwards": rows.iter().map(|f| forward_json(f, now)).collect::<Vec<_>>(),
                    // Without delivery outcomes the automatic pauses can't fire.
                    "outcomes": { "ok": webhook_error.is_none(), "error": webhook_error },
                })
            }),
    )
}

/// POST `/cli/mail/address/person-link` `{address, person: "<login>"|"owner"|null}`
/// — owner and Admins only (FW-A8). Linking also marks the mailbox a
/// person's (`person_mailbox`).
pub fn handle_person_link(body: &[u8]) -> CliResponse {
    if crate::caller_workspace::request_principal().is_some() {
        return email_notify::fail_response(&fail(403, "owner_only", "only the owner or an Admin links a mailbox to a person"));
    }
    respond(body_json(body).and_then(|v| {
        let row = mailbox_from(&v)?;
        let person = match v.get("person") {
            Some(J::Null) => None,
            Some(J::String(p)) if !p.trim().is_empty() => {
                let p = p.trim();
                if p == k2_core::notify_prefs::OWNER {
                    Some(p.to_string())
                } else {
                    if k2_core::connect_users::role_for_user(p).is_none() {
                        return Err(fail(404, "not_found", format!("no login '{p}' on this server (k2 users list)")));
                    }
                    // The person key (a login the owner linked is the owner).
                    let people = k2_core::notify_prefs::People::load().map_err(email_notify::db_fail)?;
                    Some(people.login_person_key(p).ok_or_else(|| fail(409, "reserved_login", format!("'{p}' can't own a mailbox")))?)
                }
            }
            _ => return Err(fail(400, "usage", "send {address, person: \"<login>\"} to link, or person: null to unlink")),
        };
        db(|c| store::set_person_principal(c, &row.id, person.as_deref()))?;
        if person.is_some() {
            crate::mail::agent_creds::set_person_flag(&row.id, true).map_err(email_notify::db_fail)?;
        }
        Ok(json!({ "ok": true, "address": row.address, "person": person }))
    }))
}

// ── the person routes (/cli/mail/self/forward*) ─────────────────────

/// The mailbox `person` may act on: `address` if linked to them, else
/// their only linked mailbox.
fn person_mailbox(person: &str, address: Option<&str>) -> R<MailAddress> {
    let mine = db(|c| store::mailboxes_of_person(c, person))?;
    let pick = match address.map(|a| a.trim().to_ascii_lowercase()) {
        Some(a) => mine.iter().find(|(_, addr)| *addr == a).ok_or_else(|| {
            fail(403, "forbidden", format!("{a} isn't linked to you; the owner or an Admin links mailboxes (k2 hostmail person link)"))
        })?,
        None => match mine.as_slice() {
            [one] => one,
            [] => {
                return Err(fail(
                    404,
                    "not_found",
                    "no mailbox is linked to you; the owner or an Admin links one (k2 hostmail person link <address> <login>)",
                ))
            }
            _ => return Err(fail(400, "usage", "more than one mailbox is linked to you: say which with 'address'")),
        },
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    crate::mail::addresses::load_address(&conn, &pick.1)
        .filter(|r| r.status == "active")
        .ok_or_else(|| fail(404, "not_found", "that mailbox is gone"))
}

/// `/cli/mail/self/forward*` for `person` (resolved from the token by the
/// dispatcher: `owner`, or a login's person key). Never an agent.
pub fn self_route(is_post: bool, path: &str, body: &[u8], person: &str, params: &HashMap<String, String>) -> CliResponse {
    if path != SELF_PATH && !is_post {
        return CliResponse::method_not_allowed();
    }
    let caller = Caller::Person { key: person.to_string() };
    if !is_post {
        let address = params.get("address").map(String::as_str).filter(|a| !a.trim().is_empty());
        return respond(person_mailbox(person, address).and_then(|row| show(&row)));
    }
    respond(body_json(body).and_then(|v| {
        let row = person_mailbox(person, s(&v, "address"))?;
        match path {
            SELF_PATH => set_or_add(&row, &caller, &v, Mode::Set),
            SELF_ADD_PATH => set_or_add(&row, &caller, &v, Mode::Add),
            SELF_RM_PATH => rm(&row, &caller, &v),
            SELF_UNSET_PATH => unset(&row, &caller),
            SELF_PAUSE_PATH => pause(&row, &caller, &v),
            SELF_RESUME_PATH => resume(&row, &caller, &v),
            _ => Err(fail(404, "not_found", "no such route")),
        }
    }))
}

#[cfg(test)]
mod tests;
