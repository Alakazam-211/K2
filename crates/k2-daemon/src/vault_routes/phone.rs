//! The owner answers their own terminal's vault confirm from the phone
//! (0.46.17, Rosson's card #12917).
//!
//! The owner token's vault confirms (grant a value someone can read,
//! reveal, the first-setup steps, rekey and the rest) asked only this
//! computer's own prompt: Touch ID or the Mac's password, Windows Hello.
//! Over SSH, or with the owner away, nobody could answer it, and it failed
//! after two minutes. Now the terminal can also raise a card for the same
//! step on a workspace Thread, pushed to the owner's phones, and **the first
//! answer wins**:
//!
//! 1. `POST /cli/vault/confirm/start` with `phone: true` (the owner token
//!    only, on a step this path covers, permission cards on) raises the
//!    card ([`raise`]): the permission card `vault.owner-confirm`
//!    (`permission::actions::vault_owner_confirm`) on the Thread of the
//!    workspace the terminal is in (`workspacePath`) or the one it names
//!    (`thread`). It names the exact step in the daemon's own words.
//! 2. `POST /cli/vault/confirm/finish` with `phoneRequest` ([`finish`])
//!    blocks: this computer's own prompt (when `method` is one) and the
//!    card at once, until one answers, the card is denied, or
//!    [`WINDOW_SECS`] pass. The phone winning closes the computer's prompt;
//!    the computer winning withdraws the card.
//! 3. On the card, the person confirms through the existing confirm sheet
//!    (`/cli/vault/confirm/start|finish` with action [`CARD_CONFIRM`] and
//!    the request id, [`card_start`] / [`card_finish`]) and answers Allow
//!    with that token. The card's apply ([`approve`]) releases the
//!    terminal's step.
//!
//! **Never weaker than the computer.** The card is the owner's only
//! (Owner floor, and [`card_finish`] refuses anyone else), and its Allow
//! takes a fresh confirm, never a click:
//!
//! - a first-setup step (`presence`) takes only what `presence` itself takes
//!   from that answerer: an owner login's own sign-in password
//!   (`connect-password`), which `presence` already accepts as this
//!   computer's own user check for an owner login (and, as
//!   `owner-login-password`, on a Linux server); never the vault password,
//!   which `presence` refuses for these steps;
//! - every other step takes what the same confirm takes from that answerer
//!   anywhere: an owner login's password, or the vault password, which the
//!   owner token's own confirm already takes in place of Touch ID.
//!
//! [`approve`] checks the card token's method against that rule again.
//!
//! **Bound to the exact step, one use.** The card is bound to (step, the
//! daemon's form of its secret or plan, the words shown, the terminal that
//! raised it: the owner token and its vault-socket session). A finish for
//! another step, another secret or from another terminal is refused and
//! withdraws the card ([`MISMATCH`]). Each card releases one token, bound to
//! that step like any confirm token (single use, 60 seconds). A share's
//! card also binds the share itself (who gets it, how, under which name):
//! the grant route refuses the token for any other share
//! ([`check_grant`]). Each decision writes one `vault-confirm` audit line.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::brand::CLI_NAME;
use k2_core::permission::request::RequestState;
use k2_vault::{Actor, Delivery, HumanProof, LocalVault, Role, SecretStore, Selector, VaultError};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{err, presence, ConfirmBody, GrantBody, CONFIRM_ACTIONS, GRANT_READ_CONFIRM_ACTION};
use crate::vault::actor::{Channel, VaultCaller};
use crate::vault::request_peer;

/// The method a token released by the phone carries (`HumanProof`).
pub const METHOD: &str = "phone";
/// The confirm the card itself takes (`ConfirmAction::VaultOwnerConfirm`),
/// bound to the card's request id.
pub const CARD_CONFIRM: &str = "vault-owner-confirm";
/// How long the card and the terminal wait: 5 minutes.
pub const WINDOW_SECS: i64 = 300;
/// The refusal for a finish that isn't the step its card names.
pub const MISMATCH: &str = "phone_confirm_mismatch";
/// This computer's own prompt got no answer (no one at its screen, or it
/// timed out): the owner's terminal falls back to the phone card.
pub const UNANSWERED: &str = "confirm_unanswered";

/// How often the waiting terminal looks for an answer.
const TICK: Duration = Duration::from_millis(150);
/// A share's binding outlives its token by a little (tokens live 60 s).
const ISSUED_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Waiting,
    /// Allowed on the card: `method` is the card's confirm, `by` who.
    Approved { method: String, by: String },
    /// Taken by the terminal, or decided another way.
    Closed,
}

/// One terminal waiting for its phone.
#[derive(Debug, Clone)]
struct Pending {
    /// `vpc_…`: the card's `id` value.
    id: String,
    /// The card's permission request (`pr_…`), once posted.
    request: String,
    action: String,
    secret: Option<String>,
    reason: String,
    /// The words on the card (the step, in the daemon's words).
    words: String,
    /// A share's own binding (`grant-read`), checked at the grant.
    detail: Option<String>,
    /// The caller that raised it: stamp + vault-socket session.
    caller: String,
    expires_at: i64,
    state: State,
}

#[derive(Default)]
struct Registry {
    pending: Mutex<HashMap<String, Pending>>,
    /// sha256(token) → (share binding, when).
    issued: Mutex<HashMap<String, (String, Instant)>>,
}

fn reg() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Registry::default)
}

fn pending() -> std::sync::MutexGuard<'static, HashMap<String, Pending>> {
    reg().pending.lock().unwrap_or_else(|p| p.into_inner())
}

fn now() -> i64 {
    k2_core::overlay::now_secs()
}

fn token_key(t: &str) -> String {
    format!("{:x}", Sha256::digest(t.trim().as_bytes()))
}

fn caller_key(c: &VaultCaller) -> String {
    format!("{}{}", c.actor.stamp(), request_peer::bind())
}

/// The steps this path covers: every owner confirm except making a login an
/// owner or re-keying an owner login (`presence` takes no login password
/// for those), and the card's own confirm.
pub fn eligible(action: &str) -> bool {
    CONFIRM_ACTIONS.contains(&action) && action != presence::OWNER_LOGIN && action != CARD_CONFIRM
}

/// May `c` raise a phone card for `action` here? `Err` says why not.
pub fn offered(c: &VaultCaller, action: &str) -> Result<(), String> {
    if c.is_passport() || !matches!(c.actor, Actor::Owner) || c.channel == Channel::Login {
        return Err("the phone path is for the owner's own terminal (a login confirms with its own password)".into());
    }
    if !eligible(action) {
        return Err("this step is confirmed at the computer only".into());
    }
    if !k2_core::permission::gate::server_on() {
        return Err("permission cards are off on this server, so a phone can't draw the card (Settings → Permissions)".into());
    }
    Ok(())
}

/// The owner token has no check of its own here (no vault password, no
/// prompt): may the phone be its confirm? Only with an owner login to answer
/// the card (the card takes that login's own password).
pub fn only_path(c: &VaultCaller, action: &str) -> bool {
    offered(c, action).is_ok() && !presence::owner_logins().is_empty()
}

// ── 1. the terminal raises the card ─────────────────────────────────

/// What a confirm for `action` is bound to, its words, and the stamp the
/// released token is bound to: `presence`'s own binding for a first-setup
/// step, else the same as this computer's own confirm ([`super::bound_step`]).
pub(super) fn step_of(vault: &LocalVault, c: &VaultCaller, action: &str, b: &ConfirmBody) -> Result<(Option<String>, String, String), VaultError> {
    if presence::is_action(action) {
        let bound = presence::binding(action, b.secret_id.as_deref())?;
        let reason = presence::reason(action, bound.as_deref());
        return Ok((bound, reason, presence::bound_stamp(c, action)));
    }
    let (secret, reason) = super::bound_step(vault, c, action, b)?;
    Ok((secret, reason, c.actor.stamp()))
}

fn sentence(s: &str) -> String {
    let t = s.trim().trim_end_matches('.');
    let mut ch = t.chars();
    match ch.next() {
        Some(f) => format!("{}{}", f.to_uppercase(), ch.as_str()),
        None => String::new(),
    }
}

/// The card's words and values for the step, from the vault's own records
/// where it has them (a share, a reveal), and a share's own binding.
type Described = (String, Vec<(&'static str, String)>, Option<String>);

fn describe(vault: &LocalVault, c: &VaultCaller, action: &str, secret: Option<&str>, reason: &str, grant: Option<&Value>) -> Result<Described, VaultError> {
    let meta = |id: &str| vault.meta(&Selector::Id(id.to_string()), &c.actor);
    let scope_of = |m: &k2_vault::SecretMeta| super::scope_flag(&m.scope).unwrap_or_else(|| "this vault".into());
    if action == GRANT_READ_CONFIRM_ACTION {
        let g = grant.ok_or_else(|| err("grant_required", 400, "a share's phone card needs the share itself (grant)"))?;
        let gb: GrantBody = serde_json::from_value(g.clone()).map_err(|e| err("bad_grant", 400, format!("grant: {e}")))?;
        let sid = secret.ok_or_else(|| err("bad_secret_id", 400, "secretId is the secret's id"))?;
        let m = meta(sid)?;
        let sel = super::selector(c, &gb.sel)?;
        if vault.meta(&sel, &c.actor)?.id != m.id {
            return Err(err(MISMATCH, 403, "the share names another secret than secretId"));
        }
        let to = gb.to.as_deref().ok_or_else(|| err("consumer_required", 400, "who gets it? grant.to"))?;
        let consumer = super::consumer_for(c, to)?;
        let delivery = super::grant_delivery(&gb, || Ok(m.default_delivery))?;
        let detail = grant_detail(&m.id, &consumer, delivery, gb.env_name.as_deref(), gb.file_name.as_deref());
        let label = super::consumer_label(c, &consumer);
        let scope = scope_of(&m);
        let mut values = vec![("secret", m.name.clone()), ("scope", scope.clone()), ("to", label.clone()), ("as", delivery.as_str().to_string())];
        let how = match delivery {
            Delivery::Env => {
                let n = gb.env_name.clone().unwrap_or_else(|| k2_vault::delivery::default_env_name(&m.name));
                values.push(("env_name", n.clone()));
                format!("an environment variable named {n}")
            }
            Delivery::File => {
                let n = gb.file_name.clone().unwrap_or_else(|| m.name.clone());
                values.push(("file_name", n.clone()));
                format!("a file named {n}")
            }
            d => d.as_str().to_string(),
        };
        let until = match (gb.ttl.as_ref().filter(|v| !v.is_null()), gb.expires_at.as_deref()) {
            (Some(t), _) => format!(", for {}", t.as_str().map(str::to_string).unwrap_or_else(|| format!("{t} seconds"))),
            (None, Some(x)) => format!(", until {x}"),
            (None, None) => String::new(),
        };
        return Ok((format!("Share {} ({scope}) with {label} as {how}{until}", m.name), values, Some(detail)));
    }
    if action == "reveal" {
        let sid = secret.ok_or_else(|| err("bad_secret_id", 400, "secretId is the secret's id"))?;
        let m = meta(sid)?;
        let scope = scope_of(&m);
        return Ok((format!("Show the value of {} ({scope}) in your terminal", m.name), vec![("secret", m.name.clone()), ("scope", scope)], None));
    }
    Ok((sentence(reason), vec![], None))
}

/// A share's own binding: the secret, who gets it, how, and the name.
pub(super) fn grant_detail(secret_id: &str, consumer: &k2_vault::Consumer, d: Delivery, env: Option<&str>, file: Option<&str>) -> String {
    format!("{secret_id}|{:?}:{}|{}|{}|{}", consumer.kind, consumer.id, d.as_str(), env.unwrap_or(""), file.unwrap_or(""))
}

/// The workspace whose Thread takes the card, and why that one: the one
/// named (`thread`), else the one the terminal is in (its folder or a parent
/// of it), else the server's own agent's (Kessel's), the Thread this server
/// keeps for its owner.
fn thread_of(path: Option<&str>, named: Option<&str>) -> Result<(String, &'static str), VaultError> {
    let no = |m: String| err("no_thread", 409, m);
    if let Some(n) = named.map(str::trim).filter(|n| !n.is_empty()) {
        let ws = crate::vault::workspace_id(n).ok_or_else(|| no(format!("no workspace named {n} (K2_VAULT_CONFIRM_THREAD)")))?;
        return Ok((ws, "the workspace K2_VAULT_CONFIRM_THREAD names"));
    }
    if let Some(start) = path.map(str::trim).filter(|p| !p.is_empty()) {
        let mut at = Some(std::path::Path::new(start));
        while let Some(p) = at {
            if let Some(id) = crate::clone_vault::project_id_for_path(&p.display().to_string()) {
                return Ok((id, "the workspace you ran it in"));
            }
            at = p.parent();
        }
    }
    let kessel = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::orchestrator::live_kessel(&conn).ok().flatten()
    };
    match kessel {
        Some(k) if crate::overlay_routes::pinned_chat_of(&k.workspace_id).is_some() => Ok((k.workspace_id, "this server's own agent")),
        _ => Err(no(
            "you're not in a workspace and this server has no agent of its own with a Thread, so there's nowhere to put the phone card: run it from a workspace folder, or set K2_VAULT_CONFIRM_THREAD=<workspace>"
                .to_string(),
        )),
    }
}

/// Drop what nobody waits for any more (expired, or taken a while ago).
fn sweep() {
    let cut = now() - 60;
    pending().retain(|_, p| p.expires_at > cut);
    let mut issued = reg().issued.lock().unwrap_or_else(|p| p.into_inner());
    issued.retain(|_, (_, at)| at.elapsed() < ISSUED_TTL);
}

/// `/cli/vault/confirm/start` with `phone: true`: raise the card.
pub(super) fn raise(vault: &LocalVault, c: &VaultCaller, action: &str, b: &ConfirmBody, ingress: &str) -> Result<Value, VaultError> {
    offered(c, action).map_err(|why| err("phone_confirm_unavailable", 409, why))?;
    if b.proposal.as_deref().is_some_and(|p| !p.trim().is_empty()) {
        return Err(err("phone_confirm_unavailable", 409, "a clone proposal's confirm is answered at the computer"));
    }
    let (secret, reason, _) = step_of(vault, c, action, b)?;
    let (words, values, detail) = describe(vault, c, action, secret.as_deref(), &reason, b.grant.as_ref())?;
    let (ws, thread_why) = thread_of(b.workspace_path.as_deref(), b.thread.as_deref())?;
    sweep();
    let id = format!("vpc_{}", &uuid::Uuid::new_v4().simple().to_string()[..24]);
    let expires_at = now() + WINDOW_SECS;
    pending().insert(
        id.clone(),
        Pending {
            id: id.clone(),
            request: String::new(),
            action: action.to_string(),
            secret: secret.clone(),
            reason: reason.clone(),
            words: words.clone(),
            detail,
            caller: caller_key(c),
            expires_at,
            state: State::Waiting,
        },
    );
    let mut params = Map::new();
    params.insert("id".into(), json!(id));
    params.insert("step".into(), json!(words));
    for (k, v) in values {
        params.insert(k.into(), json!(v));
    }
    let asker = request_peer::asker().unwrap_or_else(|| "your terminal".into());
    let why = format!("You ran this in your own terminal ({asker}). It waits for your answer here or on the computer.");
    let stamp = c.actor.stamp();
    match crate::permission::actions::vault_owner_confirm::present(&ws, params, why, expires_at) {
        Ok((request, card, addr)) => {
            if let Some(p) = pending().get_mut(&id) {
                p.request = request.clone();
            }
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("phone card {request} for {action}"), ingress);
            Ok(json!({ "phone": { "request": request, "card": card, "addr": addr, "thread": thread_why, "expiresIn": WINDOW_SECS, "text": words } }))
        }
        Err(e) => {
            pending().remove(&id);
            Err(err("no_thread", 409, format!("the phone card could not be posted: {e}")))
        }
    }
}

// ── 2. the terminal waits ───────────────────────────────────────────

/// This computer's own answer, from the thread that shows its prompt.
pub(super) enum Local {
    Approved,
    Declined(VaultError),
    /// No answer can come from this computer (no one at its screen, timed
    /// out, or closed because the phone answered first).
    Unavailable(String),
}

/// This computer's prompt, run on its own thread; `stop` closes it.
pub(super) type LocalCheck = Box<dyn FnOnce(&AtomicBool) -> Local + Send>;

/// The computer's own checks this caller may race the phone with.
fn local_methods(vault: &LocalVault, c: &VaultCaller, action: &str) -> Vec<&'static str> {
    let all = if presence::is_action(action) { presence::methods(c, action) } else { super::methods(vault, c) };
    all.into_iter().filter(|m| matches!(*m, presence::LOCAL_AUTH | presence::WINDOWS_HELLO)).collect()
}

/// The computer's own prompt for the step, as a [`LocalCheck`].
fn local_check(action: &str, method: &str, reason: &str, secret: Option<&str>) -> LocalCheck {
    #[cfg(test)]
    {
        let _ = (action, method, reason, secret);
        let (f, closed) = test_seam::get();
        Box::new(move |stop| test_seam::run(f, &closed, stop))
    }
    #[cfg(not(test))]
    {
        if presence::fake::from_env().is_some() {
            // A test-build daemon: its env's answer, never a real prompt.
            let f = presence::fake::from_env().filter(|f| f.method.is_some());
            return Box::new(move |_| match f {
                Some(f) if f.approve => Local::Approved,
                Some(_) => Local::Declined(err("confirm_failed", 403, "not approved")),
                None => Local::Unavailable("no check on this computer".into()),
            });
        }
        let shown = if presence::is_action(action) {
            presence::prompt(action, secret)
        } else {
            format!("{} vault: {}", k2_core::brand::PRODUCT_NAME, reason.chars().filter(|ch| !ch.is_control()).take(160).collect::<String>())
        };
        let method = method.to_string();
        Box::new(move |stop| os_prompt(&method, &shown, stop))
    }
}

#[cfg(all(not(test), target_os = "macos"))]
fn os_prompt(method: &str, shown: &str, stop: &AtomicBool) -> Local {
    use crate::vault::local_auth::{ask_until, Answer};
    if method != presence::LOCAL_AUTH {
        return Local::Unavailable("this Mac's check is Touch ID or its password".into());
    }
    match ask_until(shown, stop) {
        Answer::Approved => Local::Approved,
        Answer::Declined(h) => Local::Declined(err("confirm_failed", 403, h)),
        Answer::Unavailable(h) => Local::Unavailable(h),
    }
}

#[cfg(all(not(test), windows))]
fn os_prompt(method: &str, shown: &str, _stop: &AtomicBool) -> Local {
    use crate::vault::windows_presence::{confirm, Answer};
    if method != presence::WINDOWS_HELLO {
        return Local::Unavailable("this PC's check is Windows Hello or its password".into());
    }
    // Windows' own prompt can't be closed from here: when the phone answers
    // first it stays up until the person dismisses it (its answer is unused).
    match confirm(shown) {
        Answer::Approved => Local::Approved,
        Answer::Declined(h) => Local::Declined(err("confirm_failed", 403, h)),
        Answer::Unavailable(h) => Local::Unavailable(h),
    }
}

#[cfg(all(not(test), not(target_os = "macos"), not(windows)))]
fn os_prompt(_method: &str, _shown: &str, _stop: &AtomicBool) -> Local {
    Local::Unavailable("this computer has no prompt of its own".into())
}

/// What ended the wait.
enum Won {
    Phone { method: String, by: String },
    Computer,
    ComputerNo(VaultError),
    Card(RequestState),
    Expired,
    Gone,
}

/// The card's request state, once it is decided (denied, dismissed,
/// expired, failed…); `None` while it waits or is being applied.
fn decided(request: &str) -> Option<RequestState> {
    let r = crate::permission::store::get_by_display_id(request).ok().flatten()?;
    (!matches!(r.state, RequestState::Pending | RequestState::AlreadyDone | RequestState::Applying | RequestState::Approved)).then_some(r.state)
}

/// Close the card (when it still waits) with K2's words.
fn close_card(request: &str, to: RequestState, message: &str) {
    if request.is_empty() {
        return;
    }
    if let Ok(Some(r)) = crate::permission::store::get_by_display_id(request) {
        if r.is_answerable() {
            crate::permission::answer::close_open(&[r], to, message);
        }
    }
}

/// Waiting → Closed for the terminal's own outcome; `false` when the phone
/// got there first (Approved) or it is gone.
fn claim(id: &str) -> bool {
    let mut g = pending();
    match g.get_mut(id) {
        Some(p) if p.state == State::Waiting => {
            p.state = State::Closed;
            true
        }
        _ => false,
    }
}

/// `/cli/vault/confirm/finish` with `phoneRequest`: this computer's own
/// prompt (when `method` is one) and the phone card at once; the first
/// answer wins. Returns the step's token and where it was confirmed
/// (`phone` | `computer`). Every path ends the card's wait: one use.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish(
    vault: &LocalVault,
    c: &VaultCaller,
    action: &str,
    secret: Option<&str>,
    reason: &str,
    mint_stamp: &str,
    method: &str,
    request: &str,
    ingress: &str,
) -> Result<(String, &'static str), VaultError> {
    let stamp = c.actor.stamp();
    let deny = |code: &str| crate::vault::auth_audit("vault-confirm", &stamp, &format!("deny {action} via phone (card {request}): {code}"), ingress);
    let found = pending().values().find(|p| p.request == request && !request.is_empty()).cloned();
    let Some(p) = found else {
        deny("phone_confirm_closed");
        return Err(err("phone_confirm_closed", 409, "that phone card isn't waiting (answered, withdrawn or expired): nothing was confirmed. Run the command again."));
    };
    let same = p.caller == caller_key(c) && p.action == action && p.secret.as_deref() == secret && p.reason == reason;
    let local = match method {
        METHOD => None,
        m if local_methods(vault, c, action).contains(&m) => Some(local_check(action, m, reason, secret)),
        _ => None,
    };
    let bad_method = method != METHOD && local.is_none();
    if !same || bad_method {
        pending().remove(&p.id);
        close_card(request, RequestState::Cancelled, "Withdrawn: the terminal asked for something else. Nothing was done.");
        if bad_method {
            deny("bad_method");
            return Err(err("bad_method", 400, format!("method is {METHOD} or this computer's own check ({})", local_methods(vault, c, action).join(", "))));
        }
        deny(MISMATCH);
        return Err(err(
            MISMATCH,
            403,
            "that phone card was raised for another step, another secret or another terminal: nothing was confirmed, and the card is withdrawn. Run the command again.",
        ));
    }

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Local>();
    let mut local_live = false;
    if let Some(check) = local {
        let s = stop.clone();
        local_live = std::thread::Builder::new()
            .name("vault-confirm-local".into())
            .spawn(move || {
                let _ = tx.send(check(&s));
            })
            .is_ok();
    }
    let mut local_note: Option<String> = None;
    let won = loop {
        let state = pending().get(&p.id).map(|x| (x.state.clone(), x.expires_at));
        let expires_at = match state {
            Some((State::Approved { method, by }, _)) => break Won::Phone { method, by },
            Some((State::Closed, _)) | None => break Won::Gone,
            Some((State::Waiting, at)) => at,
        };
        if let Some(s) = decided(request) {
            if claim(&p.id) {
                break Won::Card(s);
            }
            continue;
        }
        if local_live {
            match rx.try_recv() {
                Ok(Local::Approved) if claim(&p.id) => break Won::Computer,
                Ok(Local::Declined(e)) if claim(&p.id) => break Won::ComputerNo(e),
                // The phone got there first: its answer is picked up above.
                Ok(Local::Approved | Local::Declined(_)) => {}
                Ok(Local::Unavailable(why)) => {
                    local_live = false;
                    local_note = Some(why);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => local_live = false,
            }
        }
        if now() >= expires_at && claim(&p.id) {
            break Won::Expired;
        }
        std::thread::sleep(TICK);
    };
    stop.store(true, Ordering::SeqCst);
    if local_live {
        // Let the computer's prompt close (it looks every 200 ms); a prompt
        // that can't be closed from here (Windows) is left to the person.
        let _ = rx.recv_timeout(Duration::from_secs(1));
    }
    pending().remove(&p.id);
    let issue = |m: &str| {
        let t = vault.confirm.mint(action, secret, mint_stamp, m);
        if let Some(d) = &p.detail {
            reg().issued.lock().unwrap_or_else(|x| x.into_inner()).insert(token_key(&t), (d.clone(), Instant::now()));
        }
        t
    };
    match won {
        Won::Phone { method: m, by } => {
            let t = issue(METHOD);
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("ok {action} via phone ({m} by {by}, card {request})"), ingress);
            Ok((t, "phone"))
        }
        Won::Computer => {
            close_card(request, RequestState::Cancelled, "Confirmed on the computer first. Nothing to do here.");
            let t = issue(method);
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("ok {action} via {method} (card {request} withdrawn)"), ingress);
            Ok((t, "computer"))
        }
        Won::ComputerNo(e) => {
            close_card(request, RequestState::Cancelled, "Declined on the computer. Nothing was done.");
            deny(e.code);
            Err(e)
        }
        Won::Card(s) => {
            let (code, words) = match s {
                RequestState::Denied => ("phone_confirm_denied", "denied on the phone"),
                RequestState::Dismissed => ("phone_confirm_denied", "dismissed on the phone"),
                RequestState::Expired => ("phone_confirm_expired", "the phone card expired"),
                _ => ("phone_confirm_closed", "the phone card closed without an Allow"),
            };
            deny(code);
            Err(err(code, 403, format!("{words}: nothing was confirmed, and nothing was done.")))
        }
        Won::Expired => {
            close_card(request, RequestState::Expired, "Expired: nobody answered in time. Nothing was done.");
            deny("phone_confirm_expired");
            let why = local_note.map(|w| format!(" (the computer: {w})")).unwrap_or_default();
            Err(err(
                "phone_confirm_expired",
                409,
                format!("nobody answered on the computer or the phone within {} minutes{why}: nothing was confirmed. Run the command again.", WINDOW_SECS / 60),
            ))
        }
        Won::Gone => {
            deny("phone_confirm_closed");
            Err(err("phone_confirm_closed", 409, "that phone card closed before an answer: nothing was confirmed. Run the command again."))
        }
    }
}

// ── 3. the card's own confirm and its Allow ─────────────────────────

/// Only the owner answers: the owner token (the desktop app) or an Owner
/// login, never an agent, an Admin or a Member.
fn owner_answerer(c: &VaultCaller) -> Result<(), VaultError> {
    let owner = !c.is_passport() && matches!(c.actor, Actor::Owner | Actor::Login { role: Role::Owner, .. });
    if owner {
        Ok(())
    } else {
        Err(err("owner_only", 403, "only this server's owner answers a vault confirm from the owner's terminal"))
    }
}

fn waiting_card(request: Option<&str>) -> Result<Pending, VaultError> {
    let r = request.map(str::trim).filter(|r| !r.is_empty()).ok_or_else(|| err("bad_secret_id", 400, "secretId is the card's request id"))?;
    pending()
        .values()
        .find(|p| p.request == r && p.state == State::Waiting && p.expires_at > now())
        .cloned()
        .ok_or_else(|| err("bad_secret_id", 400, "that card isn't waiting for an answer"))
}

/// The checks that may confirm the card for `action`, from `c`: what that
/// answerer could use for the same step itself (module doc).
fn card_methods(vault: &LocalVault, c: &VaultCaller, action: &str) -> Vec<&'static str> {
    if presence::is_action(action) {
        presence::methods(c, action)
    } else {
        super::methods(vault, c)
    }
}

/// `/cli/vault/confirm/start` for [`CARD_CONFIRM`] (the phone's sheet).
pub(super) fn card_start(vault: &LocalVault, c: &VaultCaller, request: Option<&str>) -> Result<Value, VaultError> {
    owner_answerer(c)?;
    let p = waiting_card(request)?;
    let m = card_methods(vault, c, &p.action);
    if m.is_empty() {
        return Err(err("no_confirm_method", 409, "no confirm you can use here: sign in with an owner login and use its password"));
    }
    let (server, login) = super::confirm_identity(c);
    Ok(json!({ "methods": m, "expiresIn": 60, "confirmText": p.words, "server": server, "login": login }))
}

/// `/cli/vault/confirm/finish` for [`CARD_CONFIRM`]: run one of
/// [`card_methods`] and mint the card's token, bound to (the card's request,
/// this answerer).
pub(super) fn card_finish(vault: &LocalVault, c: &VaultCaller, b: &ConfirmBody, ingress: &str) -> Result<Value, VaultError> {
    owner_answerer(c)?;
    let p = waiting_card(b.secret_id.as_deref())?;
    let method = b.method.as_deref().unwrap_or("");
    let allowed = card_methods(vault, c, &p.action);
    if !allowed.contains(&method) {
        return Err(err("bad_method", 400, format!("this card takes {}", allowed.join(" or "))));
    }
    let stamp = c.actor.stamp();
    let pw = b.password.as_deref().unwrap_or("");
    let res = match method {
        presence::CONNECT_PASSWORD => match &c.actor {
            Actor::Login { username, .. } => super::check_connect_password(vault, username, pw),
            _ => Err(err("bad_method", 400, "connect-password is for a Connect login")),
        },
        "vault-password" => vault.check_confirm_password(pw, &stamp),
        presence::OWNER_LOGIN_PASSWORD => presence::owner_login_check(vault, &p.action, b.username.as_deref(), pw),
        m => match local_check(&p.action, m, &p.reason, p.secret.as_deref())(&AtomicBool::new(false)) {
            Local::Approved => Ok(()),
            Local::Declined(e) => Err(e),
            Local::Unavailable(h) => Err(err("confirm_failed", 403, h)),
        },
    };
    match res {
        Ok(()) => {
            let t = vault.confirm.mint(CARD_CONFIRM, Some(&p.request), &stamp, method);
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("ok {CARD_CONFIRM} {} via {method}", p.request), ingress);
            Ok(json!({ "token": t, "expiresIn": 60 }))
        }
        Err(e) => {
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("deny {CARD_CONFIRM} {} via {method}: {}", p.request, e.code), ingress);
            Err(e)
        }
    }
}

/// Is the card `id` (`vpc_…`) still waiting? (The card's `check`.)
pub(crate) fn is_waiting(id: &str) -> bool {
    pending().get(id).is_some_and(|p| p.state == State::Waiting && p.expires_at > now())
}

/// The card's Allow: `token` is the card's own confirm (bound to `request`
/// and the answerer `stamp`). Releases the terminal's step; returns its
/// words. `Err((code, message))`: nothing released.
pub(crate) fn approve(id: &str, request: &str, token: &str, stamp: &str, owner_token: bool, by: &str) -> Result<String, (String, String)> {
    let vault = crate::vault::get().map_err(|e| (e.code.to_string(), e.hint))?;
    let method = match vault.confirm.take(token, CARD_CONFIRM, Some(request), stamp) {
        Ok(HumanProof::Confirmed { method }) => method,
        Ok(HumanProof::None) => return Err(("vault_confirm_required".into(), "confirm again".into())),
        Err(e) => return Err((e.code.to_string(), e.hint)),
    };
    let mut g = pending();
    let Some(p) = g.get_mut(id) else {
        return Err(("stale".into(), "the terminal's step isn't waiting any more (answered on the computer, withdrawn or expired)".into()));
    };
    if p.request != request || p.state != State::Waiting || p.expires_at <= now() {
        return Err(("stale".into(), "the terminal's step isn't waiting any more (answered on the computer, withdrawn or expired)".into()));
    }
    // Never weaker than the computer: the card's confirm must be one the
    // same answerer could use for the step itself.
    if !strong_enough(&p.action, &method, owner_token) {
        return Err(("vault_confirm_required".into(), format!("this step takes your owner login's password, not {method}")));
    }
    p.state = State::Approved { method, by: by.to_string() };
    Ok(p.words.clone())
}

/// The method rule of the module doc, for the answerer kind: a first-setup
/// step takes an owner login's own password (or, from the owner token, this
/// computer's own check), never the vault password.
pub(crate) fn strong_enough(action: &str, method: &str, owner_token: bool) -> bool {
    if presence::is_action(action) {
        if owner_token {
            matches!(method, presence::LOCAL_AUTH | presence::WINDOWS_HELLO | presence::OWNER_LOGIN_PASSWORD)
        } else {
            method == presence::CONNECT_PASSWORD
        }
    } else {
        matches!(method, presence::CONNECT_PASSWORD | "vault-password" | presence::LOCAL_AUTH | presence::WINDOWS_HELLO)
    }
}

/// The grant route, with a token: a token the phone released for a share is
/// good for that share only (who gets it, how, the name). Any other share is
/// refused and the token burnt. Tokens with no share binding pass.
pub(super) fn check_grant(vault: &LocalVault, token: &str, detail: &str) -> Result<(), VaultError> {
    let bound = reg().issued.lock().unwrap_or_else(|p| p.into_inner()).remove(&token_key(token));
    match bound {
        Some((d, _)) if d != detail => {
            // A failed take burns the token.
            let _ = vault.confirm.take(token, "", None, "");
            Err(err(
                MISMATCH,
                403,
                format!("that confirm was for another share (a different target, delivery or name): nothing was shared. Run {CLI_NAME} vault grant again."),
            ))
        }
        _ => Ok(()),
    }
}

/// Test seam: this computer's own answer in the race, per test thread
/// (`None`: nobody at the computer). Never a real prompt.
#[cfg(test)]
pub(crate) mod test_seam {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Fake {
        Approve,
        Decline,
        /// Shown, and nobody answers until it is closed (or 10 s pass).
        Wait,
    }

    thread_local! {
        static FAKE: std::cell::Cell<Option<Fake>> = const { std::cell::Cell::new(None) };
        static CLOSED: std::cell::RefCell<Arc<AtomicBool>> = std::cell::RefCell::new(Arc::new(AtomicBool::new(false)));
    }

    /// This thread's answer from the computer, from now on.
    pub(crate) fn set(f: Option<Fake>) {
        FAKE.with(|c| c.set(f));
        CLOSED.with(|c| *c.borrow_mut() = Arc::new(AtomicBool::new(false)));
    }

    pub(super) fn get() -> (Option<Fake>, Arc<AtomicBool>) {
        (FAKE.with(std::cell::Cell::get), CLOSED.with(|c| c.borrow().clone()))
    }

    /// Was this thread's waiting prompt closed because the phone answered?
    pub(crate) fn closed() -> bool {
        CLOSED.with(|c| c.borrow().load(Ordering::SeqCst))
    }

    pub(super) fn run(f: Option<Fake>, closed: &AtomicBool, stop: &AtomicBool) -> Local {
        match f {
            None => Local::Unavailable("no one at this computer's screen".into()),
            Some(Fake::Approve) => Local::Approved,
            Some(Fake::Decline) => Local::Declined(err("confirm_failed", 403, "not approved")),
            Some(Fake::Wait) => {
                let until = Instant::now() + Duration::from_secs(10);
                while Instant::now() < until {
                    if stop.load(Ordering::SeqCst) {
                        closed.store(true, Ordering::SeqCst);
                        return Local::Unavailable("answered on your phone first".into());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Local::Unavailable("no one answered".into())
            }
        }
    }

    /// Make a waiting card's time run out now.
    pub(crate) fn expire(request: &str) {
        for p in pending().values_mut().filter(|p| p.request == request) {
            p.expires_at = now() - 1;
        }
    }

    /// The share binding a token carries (`None`: none).
    pub(crate) fn issued(token: &str) -> Option<String> {
        reg().issued.lock().unwrap_or_else(|p| p.into_inner()).get(&token_key(token)).map(|(d, _)| d.clone())
    }
}
