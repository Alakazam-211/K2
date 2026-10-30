//! Wallet operations: import the live login, finish a sign-in, switch,
//! remove, keep idle logins warm. Every operation that moves token bytes
//! holds the per-tool [`store::lock_tool`].

use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;

use super::identity::{self, AccountIdentity, Source};
use super::{pins, slots};
use super::store::{self, CredMeta};
use super::{
    active_id, get, list_for, live_owner, lookup, now, set_active, set_active_with_live, state,
    update_meta, Entry, MetaUpdate, Tool, WalletError,
};

fn pinned_conflict(e: &Entry) -> WalletError {
    WalletError::Conflict(
        "login_pinned",
        format!(
            "{} is set for a workspace or chat. A subscription can't be live in two places (sign-ins rotate), so set those back to Server default first.",
            e.label
        ),
    )
}

const LOCK_WAIT: Duration = Duration::from_secs(20);

/// The daemon's vendor client. `refresh` refreshes an IDLE wallet slot
/// in place (never the active one): Claude through the OAuth refresh
/// grant, Codex and Grok through their own CLI run in the slot.
/// `identify` asks the vendor which account a credential belongs to
/// (Claude: `/api/oauth/profile`); `None` when it can't tell (offline,
/// air-gap, an expired access token, tests).
pub trait Refresher {
    fn refresh(&self, tool: Tool, id: &str) -> Result<(), String>;
    fn identify(&self, _tool: Tool, _cred: &[u8]) -> Option<AccountIdentity> {
        None
    }
}

// ── Sign-ins made outside K2 (Rosson 2026-10-27) ────────────────────
//
// "If someone logs into claude code in the terminal and it alters the
// subscription that had already been stored in the Subs / Tokens it
// shouldn't break the ability to add other accounts or even re-add the
// previous account that was there before but ended up getting swapped."
//
// prd-llm-tokens-outside-logins-v1 (+ vs-live LT-A1…A16), first cut:
// - one guarded slot write ([`file_bytes`], LT-A4): a slot only takes
//   bytes of its own recorded account, or bytes with no identity yet when
//   the token has no account on record;
// - a live sign-in of another account is saved as that account's own
//   token, never refused and never filed under the default (LT-A5/A7); an
//   account K2 can't name yet goes to a "checking" token (LT-A8);
// - a slot that holds another account's sign-in (drift) is moved out, and
//   its token says "sign in again" (LT-A9);
// - one Owner/Admin notice per account (LT-A12).

/// `created_by` of a token K2 saved from a sign-in made outside K2.
pub const OUTSIDE_K2: &str = "outside-k2";
/// `switched_by` when K2 points the default at what is really live.
pub const BY_OUTSIDE: &str = "outside K2";

/// The daemon's "a running session uses this slot" probe (registered at
/// start; unset in core tests = never in use).
static IN_USE: std::sync::OnceLock<fn(&str) -> bool> = std::sync::OnceLock::new();

/// Register the daemon's in-use probe (once).
pub fn set_in_use_probe(f: fn(&str) -> bool) {
    let _ = IN_USE.set(f);
}

fn slot_in_use(id: &str) -> bool {
    IN_USE.get().is_some_and(|f| f(id))
}

/// A CLI owns this slot right now (pinned to a chat or workspace, or a
/// running session uses it): K2 never writes it.
fn slot_busy(conn: &Connection, id: &str) -> Result<bool, WalletError> {
    Ok(pins::is_pinned(conn, id)? || slot_in_use(id))
}

/// A sign-in K2 found under the wrong token (or couldn't name) and saved
/// as its account's own token. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adopted {
    pub tool: String,
    /// The token that now holds it.
    pub id: String,
    pub label: String,
    /// The account as a person reads it (the email the token list shows).
    pub account: String,
    /// A new token; `false`: that account's existing token was updated.
    pub created: bool,
    /// Tell the Owners and Admins (once per account on this server).
    pub notify: bool,
    /// `live` (the tool's live sign-in) or `slot` (another token's slot).
    pub found_in: String,
    /// The token it was found under.
    pub found_under: String,
    /// The account's existing token, when that one couldn't take it
    /// (pinned or in use): "same account as".
    pub same_account_as: Option<String>,
}

/// What is known about bytes before they go into a slot.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Incoming<'a> {
    /// Their account, and how it was known.
    Known(&'a AccountIdentity, Source),
    /// Nobody can name them yet.
    Unknown,
    /// Refreshed by K2 from the slot's own bytes (the same sign-in).
    OwnRefresh,
}

impl Incoming<'_> {
    fn of(found: Option<&(AccountIdentity, Source)>) -> Incoming<'_> {
        match found {
            Some((id, src)) => Incoming::Known(id, *src),
            None => Incoming::Unknown,
        }
    }
}

fn cross_file_refusal() -> WalletError {
    WalletError::Conflict(
        "account_mismatch",
        "K2 never files one account's sign-in under another account's token; nothing was written".into(),
    )
}

/// THE ONE WAY a server token's slot is written (LT-A4). A slot takes:
/// - any bytes while its token has no account on record (a first sign-in,
///   an adopted token, a "checking" token); their account, when known, is
///   recorded then;
/// - bytes of its own recorded account (a weak, label-only record needs a
///   vendor or credential answer);
/// - bytes of the same sign-in as the bytes it holds (same refresh token),
///   and K2's own refresh of them.
///
/// Anything else is refused: no path files one account's sign-in under
/// another's token. A source scan test keeps every other `write_slot`
/// call out of `llm_accounts`.
pub(crate) fn file_bytes(
    tool: Tool,
    id: &str,
    entry: Option<&Entry>,
    bytes: &[u8],
    incoming: Incoming<'_>,
    origin: &str,
) -> Result<(), WalletError> {
    let record = match entry {
        Some(e) => identity::recorded_identity(tool, e),
        None => identity::read_record(tool, id).map(|r| {
            let src = if r.weak() { Source::Label } else { Source::K2Record };
            (r.identity(), src)
        }),
    };
    let same_sign_in = || matches!(slots::read_slot(tool, id), Ok(Some(old)) if identity::same_grant(tool, &old, bytes));
    let allowed = match (&record, incoming) {
        (_, Incoming::OwnRefresh) => true,
        (None, _) => true,
        (Some((rec, rsrc)), Incoming::Known(who, src)) => match rec.same_account(who) {
            Some(true) => *rsrc != Source::Label || matches!(src, Source::Vendor | Source::Credential) || same_sign_in(),
            Some(false) => false,
            None => same_sign_in(),
        },
        (Some(_), Incoming::Unknown) => same_sign_in(),
    };
    if !allowed {
        crate::log_debug!("[llm-accounts] {}: refused a slot write into {id}: not that token's account", tool.as_str());
        return Err(cross_file_refusal());
    }
    slots::write_slot(tool, id, bytes)?;
    if let Incoming::Known(who, src) = incoming {
        if who.is_known() && tool.subscription_supported() {
            if let Err(err) = identity::write_record(tool, id, who, src, origin) {
                crate::log_debug!("[llm-accounts] {}: account not recorded for {id} ({err})", tool.as_str());
            }
        }
    }
    Ok(())
}

/// The live sign-in against its owner's recorded account: the
/// credential or the vendor, Claude's own `~/.claude.json` (LT-A3), else
/// the same sign-in as the owner's own slot (lineage), unless a verified
/// answer says otherwise or the owner's slot is itself suspect.
fn live_verdict(
    tool: Tool,
    owner: &Entry,
    live: &[u8],
    ident: Option<&dyn Fn(Tool, &[u8]) -> Option<AccountIdentity>>,
) -> (identity::Verdict, Option<(AccountIdentity, Source)>) {
    let record = identity::recorded_identity(tool, owner);
    let actual = identity::actual_identity(tool, live, ident, true);
    let v = identity::verdict(record.as_ref(), actual.as_ref());
    // Nobody named it (no vendor answer, and ~/.claude.json is K2's own
    // writing): the same sign-in as the owner's own slot is the owner's
    // (lineage), unless that slot is itself suspect. A named account
    // (vendor, credential, a config Claude wrote) is never overridden.
    if v == identity::Verdict::Unknown {
        if let Ok(Some(slot)) = slots::read_slot(tool, &owner.id) {
            if identity::same_grant(tool, live, &slot) && !slot_suspect(tool, owner, &slot, ident) {
                return (identity::Verdict::Match, None);
            }
        }
    }
    (v, actual)
}

/// Does a token's slot name another account than its record (credential,
/// vendor, or Claude's own config in that slot)?
fn slot_suspect(tool: Tool, e: &Entry, slot: &[u8], ident: Option<&dyn Fn(Tool, &[u8]) -> Option<AccountIdentity>>) -> bool {
    let record = identity::recorded_identity(tool, e);
    let found = identity::slot_identity(tool, &e.id, slot, ident);
    matches!(identity::verdict(record.as_ref(), found.as_ref()), identity::Verdict::Mismatch(..))
}

fn expires_of(tool: Tool, b: &[u8]) -> i64 {
    store::parse_meta(tool, b).expires_at.unwrap_or(i64::MIN)
}

/// A label for an account's new token: its email (cut to the label rule),
/// else "<Tool> outside K2"; unique.
fn account_label(conn: &Connection, tool: Tool, who: &AccountIdentity) -> Result<String, WalletError> {
    let base = who
        .email
        .as_deref()
        .map(|e| e.chars().take(36).collect::<String>())
        .and_then(|e| super::validate_label(&e).ok())
        .unwrap_or_else(|| format!("{} outside K2", tool.display()));
    unique_label(conn, tool, &base)
}

// One notice per account per server (LT-A12): the account keys already
// told about, in `~/.k2/llm-accounts/<tool>/outside-decisions.json`
// (0600, account keys only).

pub fn outside_decisions_path(tool: Tool) -> std::path::PathBuf {
    store::tool_dir(tool).join("outside-decisions.json")
}

fn outside_decisions(tool: Tool) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read(outside_decisions_path(tool))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// First time this account is seen outside K2 on this server? Then note
/// it (so it is told about once) and say so.
fn first_notice(tool: Tool, who: &AccountIdentity) -> bool {
    let Some(key) = identity::account_key(who) else { return false };
    let mut all = outside_decisions(tool);
    if all.contains_key(&key) {
        return false;
    }
    all.insert(key, serde_json::json!({ "noticedAt": now() }));
    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(all)).unwrap_or_default();
    let res = store::ensure_private_dir(&store::tool_dir(tool)).and_then(|_| store::write_private(&outside_decisions_path(tool), &bytes));
    if let Err(e) = res {
        crate::log_debug!("[llm-accounts] {}: outside notice not noted ({e})", tool.as_str());
    }
    true
}

/// Save `bytes` (a sign-in of `who`) as that account's own token: its
/// existing token when that one isn't pinned or in use (its older,
/// different sign-in goes to `.removed/` first), else a new token named
/// after the account. `exclude`: the token it was found under. `held`: a
/// slot lock the caller holds. `fresher_only`: keep an existing token's
/// own sign-in unless `bytes` expire later (a drifted copy may be older).
/// Never logs the account.
#[allow(clippy::too_many_arguments)]
fn adopt_sign_in(
    conn: &Connection,
    tool: Tool,
    bytes: &[u8],
    who: &(AccountIdentity, Source),
    profile: Option<&serde_json::Value>,
    exclude: Option<&Entry>,
    held: Option<&str>,
    fresher_only: bool,
    found_in: &str,
    outside: bool,
) -> Result<Adopted, WalletError> {
    let profile = profile
        .filter(|p| AccountIdentity::from_oauth_account(p).is_some_and(|i| i.same_account(&who.0) == Some(true)))
        .cloned()
        .unwrap_or_else(|| who.0.to_oauth_account());
    let found_under = exclude.map(|e| e.label.clone()).unwrap_or_default();
    let mut same_account_as = None;
    let mut matched = None;
    for e in list_for(conn, tool)? {
        if exclude.is_some_and(|x| x.id == e.id) || e.is_api_key() || e.state == state::SIGNING_IN {
            continue;
        }
        if !identity::recorded_identity(tool, &e).is_some_and(|(r, _)| r.same_account(&who.0) == Some(true)) {
            continue;
        }
        if held != Some(e.id.as_str()) && slot_busy(conn, &e.id)? {
            same_account_as.get_or_insert(e.id.clone());
            continue;
        }
        matched = Some(e);
        break;
    }
    if let Some(m) = matched {
        let _slot = if held == Some(m.id.as_str()) { None } else { Some(store::lock_slot(tool, &m.id, LOCK_WAIT)?) };
        let old = slots::read_slot(tool, &m.id)?;
        let write = match &old {
            None => true,
            Some(o) => !fresher_only || expires_of(tool, bytes) > expires_of(tool, o),
        };
        if write {
            if let Some(o) = old.as_deref().filter(|o| !identity::same_grant(tool, o, bytes)) {
                // Its own older sign-in is kept in Trash, never lost.
                store::set_aside_slot_cred(tool, &m.id, o, "replaced")?;
            }
            file_bytes(tool, &m.id, Some(&m), bytes, Incoming::Known(&who.0, who.1), identity::origin::OUTSIDE_K2)?;
            let meta = store::parse_meta(tool, bytes);
            update_meta(conn, &m.id, &MetaUpdate {
                expires_at: Some(meta.expires_at),
                state: Some(state::SIGNED_IN.into()),
                detail: Some(None),
                ..Default::default()
            })?;
        }
        if tool == Tool::Claude {
            if let Err(err) = identity::save_slot_profile(&m.id, &profile) {
                crate::log_debug!("[llm-accounts] claude: profile not recorded for {} ({err})", m.id);
            }
        }
        crate::log_debug!(
            "[llm-accounts] {}: a sign-in found in {found_in} belongs to token {}; saved there ({})",
            tool.as_str(),
            m.id,
            if write { "updated" } else { "kept its own, fresher" }
        );
        return Ok(Adopted {
            tool: tool.as_str().into(),
            id: m.id.clone(),
            label: m.label.clone(),
            account: who.0.describe(),
            created: false,
            notify: false,
            found_in: found_in.into(),
            found_under,
            same_account_as: None,
        });
    }
    let label = account_label(conn, tool, &who.0)?;
    let e = super::create(conn, tool, &label, Some(if outside { OUTSIDE_K2 } else { "k2" }))?;
    let origin = if outside { identity::origin::OUTSIDE_K2 } else { identity::origin::K2_SIGN_IN };
    file_bytes(tool, &e.id, Some(&e), bytes, Incoming::Known(&who.0, who.1), origin)?;
    let mut u = meta_update_from(&store::parse_meta(tool, bytes), state::SIGNED_IN);
    u.email = u.email.or_else(|| who.0.email.clone());
    update_meta(conn, &e.id, &u)?;
    if tool == Tool::Claude {
        if let Err(err) = identity::save_slot_profile(&e.id, &profile) {
            crate::log_debug!("[llm-accounts] claude: profile not recorded for {} ({err})", e.id);
        }
    }
    crate::log_debug!("[llm-accounts] {}: a sign-in found in {found_in} saved as new token {}", tool.as_str(), e.id);
    Ok(Adopted {
        tool: tool.as_str().into(),
        id: e.id,
        label,
        account: who.0.describe(),
        created: true,
        notify: outside && first_notice(tool, &who.0),
        found_in: found_in.into(),
        found_under,
        same_account_as,
    })
}

/// A sign-in nobody can name yet (Claude offline, or an expired access
/// token and a `~/.claude.json` K2 wrote itself): its own new token in
/// state `unknown` with no account on record (LT-A8). Checked and named
/// later ([`resolve_unverified`]); never merged blind.
fn file_unverified(conn: &Connection, tool: Tool, bytes: &[u8], by: Option<&str>) -> Result<Entry, WalletError> {
    let label = unique_label(conn, tool, &format!("{} sign-in - checking", tool.display()))?;
    let e = super::create(conn, tool, &label, by)?;
    file_bytes(tool, &e.id, Some(&e), bytes, Incoming::Unknown, identity::origin::OUTSIDE_K2)?;
    let mut u = meta_update_from(&store::parse_meta(tool, bytes), state::UNKNOWN);
    u.detail = Some(Some("K2 is checking which account this sign-in belongs to.".into()));
    update_meta(conn, &e.id, &u)?;
    crate::log_debug!("[llm-accounts] {}: a sign-in K2 can't name yet saved as {}", tool.as_str(), e.id);
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// Keep a live login when no subscription token is on record as live:
/// its account's own token when it names one, else a "checking" token.
fn save_unknown_live(conn: &Connection, tool: Tool, bytes: &[u8], by: Option<&str>) -> Result<(), WalletError> {
    match identity::actual_identity(tool, bytes, None, true) {
        Some(who) => adopt_sign_in(conn, tool, bytes, &who, None, None, None, false, "live", false).map(|_| ()),
        None => file_unverified(conn, tool, bytes, by).map(|_| ()),
    }
}

/// The live store now holds `new_owner`'s sign-in: record it (the server
/// default when the old owner was the default; the subscription left
/// behind an API-key default otherwise).
fn make_live_owner(conn: &Connection, tool: Tool, old_owner: &str, new_owner: &str, by: Option<&str>) -> Result<(), WalletError> {
    match active_id(conn, tool)? {
        Some(a) if a != old_owner => super::set_active_with_live(conn, tool, &a, Some(new_owner), by),
        _ => set_active(conn, tool, new_owner, by),
    }
}

/// The token keeps showing the account it is on record as (a live
/// `.claude.json` read may have copied another account's email onto it).
fn keep_recorded_email(conn: &Connection, tool: Tool, e: &Entry) -> Result<(), WalletError> {
    if let Some((rec, Source::K2Record)) = identity::recorded_identity(tool, e) {
        if let Some(email) = rec.email.filter(|m| e.email.as_deref().is_none_or(|x| !x.eq_ignore_ascii_case(m))) {
            update_meta(conn, &e.id, &MetaUpdate { email: Some(email), ..Default::default() })?;
        }
    }
    Ok(())
}

/// Is `e`'s slot holding another account's sign-in? Then save it as that
/// account's own token, take it out of `e`'s slot (a copy goes to
/// `.removed/`) and mark `e` "sign in again" (its recorded account stays
/// on show) (LT-A9). `known`: a sign-in already identified (the live one
/// just adopted); the same refresh token is the same account. `held`: a
/// slot lock the caller holds. A busy slot (pinned, in use) is left alone.
pub(crate) fn heal_drift(
    conn: &Connection,
    tool: Tool,
    e: &Entry,
    ident: Option<&dyn Fn(Tool, &[u8]) -> Option<AccountIdentity>>,
    known: Option<(&[u8], &(AccountIdentity, Source))>,
    held: Option<&str>,
) -> Result<Option<Adopted>, WalletError> {
    if e.is_api_key() || e.state == state::SIGNING_IN || e.removed_at.is_some() {
        return Ok(None);
    }
    if held != Some(e.id.as_str()) && slot_busy(conn, &e.id)? {
        return Ok(None);
    }
    let _slot = if held == Some(e.id.as_str()) {
        None
    } else {
        match store::lock_slot(tool, &e.id, Duration::from_millis(300)) {
            Ok(l) => Some(l),
            Err(_) => return Ok(None),
        }
    };
    let Some(bytes) = slots::read_slot(tool, &e.id)?.filter(|b| store::validate_cred(tool, b).is_ok()) else {
        return Ok(None);
    };
    let Some(rec) = identity::recorded_identity(tool, e) else { return Ok(None) };
    let found = match known {
        Some((kb, who)) if identity::same_grant(tool, kb, &bytes) => Some(who.clone()),
        _ => identity::slot_identity(tool, &e.id, &bytes, ident),
    };
    let Some(who) = found.filter(|(w, _)| rec.0.same_account(w) == Some(false)) else { return Ok(None) };
    let a = adopt_sign_in(conn, tool, &bytes, &who, None, Some(e), held, true, "slot", true)?;
    store::set_aside_slot_cred(tool, &e.id, &bytes, "drift")?;
    if tool == Tool::Claude {
        // Claude's config in that slot names the token's own account again.
        let p = match identity::slot_profile(&e.id) {
            Some((p, Source::K2Record)) => p,
            _ => rec.0.to_oauth_account(),
        };
        if let Err(err) = identity::reset_slot_config_profile(&e.id, &p) {
            crate::log_debug!("[llm-accounts] claude: slot config of {} not reset ({err})", e.id);
        }
    }
    // Vault storage keeps the last copy that matched (another account's
    // file is never sealed, LT-A13): the token is back with no new
    // sign-in. A sealed copy that is itself another account's is kept
    // disabled (Trash), so it isn't read again.
    let restored = match slots::read_slot(tool, &e.id) {
        Ok(Some(b)) => {
            let again = identity::slot_identity(tool, &e.id, &b, ident);
            if matches!(identity::verdict(Some(&rec), again.as_ref()), identity::Verdict::Mismatch(..)) {
                slots::removed(tool, &e.id);
                None
            } else {
                Some(b)
            }
        }
        _ => None,
    };
    let meta = match &restored {
        Some(b) => MetaUpdate {
            state: Some(state::SIGNED_IN.into()),
            detail: Some(Some(format!(
                "Another account's sign-in was in its slot; K2 moved it to \"{}\" and put this token's own saved copy back.",
                a.label
            ))),
            email: rec.0.email.clone(),
            expires_at: Some(store::parse_meta(tool, b).expires_at),
            ..Default::default()
        },
        None => MetaUpdate {
            state: Some(state::NEEDS_LOGIN.into()),
            detail: Some(Some(format!(
                "Its saved sign-in turned out to be another account's, so K2 moved it to \"{}\". Sign it in again.",
                a.label
            ))),
            email: rec.0.email.clone(),
            expires_at: Some(None),
            ..Default::default()
        },
    };
    update_meta(conn, &e.id, &meta)?;
    crate::log_debug!("[llm-accounts] {}: token {} held another account's sign-in; moved to {}, needs sign-in", tool.as_str(), e.id, a.id);
    Ok(Some(a))
}

/// The last live verdict per tool (in memory): the shared usage probe
/// doesn't record a reading on the default's row while the live sign-in
/// is another account's (LT-A6).
static LAST_LIVE_MISMATCH: std::sync::Mutex<[bool; 4]> = std::sync::Mutex::new([false; 4]);

fn tool_index(tool: Tool) -> usize {
    Tool::ALL.iter().position(|t| *t == tool).unwrap_or(0)
}

fn note_live_verdict(tool: Tool, mismatch: bool) {
    LAST_LIVE_MISMATCH.lock().unwrap_or_else(|e| e.into_inner())[tool_index(tool)] = mismatch;
}

/// Is the live sign-in known to be another account than the default's
/// (last reconcile)? Then usage readings of the live store are not the
/// default's.
pub fn live_is_other_account(tool: Tool) -> bool {
    LAST_LIVE_MISMATCH.lock().unwrap_or_else(|e| e.into_inner())[tool_index(tool)]
}

/// The vendor's answer about the live sign-in, asked with NO lock held
/// (phase 0 of a switch, boot, keep-warm). Only its fingerprint is kept.
#[derive(Debug, Clone)]
pub struct LiveAnswer {
    fp: [u8; 32],
    who: Option<AccountIdentity>,
}

impl LiveAnswer {
    pub fn ask(tool: Tool, refresher: Option<&dyn Refresher>) -> Option<LiveAnswer> {
        let live = store::read_live(tool).ok().flatten().filter(|b| store::validate_cred(tool, b).is_ok())?;
        Some(LiveAnswer { fp: fingerprint(&live), who: refresher.and_then(|r| r.identify(tool, &live)) })
    }
}

/// Settle the live sign-in now (LT-A5): when it is another account than
/// the token on record as live, save it as that account's own token and
/// make that token the live owner (`switched_by` "outside K2"); the old
/// owner's slot is left as it was, then checked for drift. An account K2
/// can't name moves nothing. `mirror`: on a match, copy the live sign-in
/// into the owner's slot (LT-A14), so switching back after an outside
/// `/login` needs no new sign-in. Never writes the live store.
pub fn reconcile_live(conn: &Connection, tool: Tool, answer: Option<&LiveAnswer>, mirror: bool) -> Result<Vec<Adopted>, WalletError> {
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    reconcile_live_locked(conn, tool, answer, mirror, None)
}

fn reconcile_live_locked(
    conn: &Connection,
    tool: Tool,
    answer: Option<&LiveAnswer>,
    mirror: bool,
    held: Option<&str>,
) -> Result<Vec<Adopted>, WalletError> {
    let mut out = Vec::new();
    if !tool.subscription_supported() {
        return Ok(out);
    }
    let Some(o) = live_owner(conn, tool)? else { return Ok(out) };
    let Some(owner) = get(conn, &o)? else { return Ok(out) };
    let Some(live) = store::read_live(tool)?.filter(|b| store::validate_cred(tool, b).is_ok()) else {
        return Ok(out);
    };
    let ask = |_: Tool, b: &[u8]| answer.filter(|a| a.fp == fingerprint(b)).and_then(|a| a.who.clone());
    let ident: &dyn Fn(Tool, &[u8]) -> Option<AccountIdentity> = &ask;
    let (v, actual) = live_verdict(tool, &owner, &live, Some(ident));
    note_live_verdict(tool, matches!(v, identity::Verdict::Mismatch(..)));
    match v {
        identity::Verdict::Match => {
            let same = matches!(slots::read_slot(tool, &owner.id), Ok(Some(s)) if s == live);
            if mirror && !same && held != Some(owner.id.as_str()) && !slot_busy(conn, &owner.id)? {
                let _slot = store::lock_slot(tool, &owner.id, LOCK_WAIT)?;
                file_bytes(tool, &owner.id, Some(&owner), &live, Incoming::of(actual.as_ref()), identity::origin::IMPORT)?;
            }
        }
        identity::Verdict::Mismatch(who, src) => {
            let profile = if tool == Tool::Claude { identity::live_profile() } else { None };
            let who = (who, src);
            let a = adopt_sign_in(conn, tool, &live, &who, profile.as_ref(), Some(&owner), held, false, "live", true)?;
            make_live_owner(conn, tool, &owner.id, &a.id, Some(BY_OUTSIDE))?;
            keep_recorded_email(conn, tool, &owner)?;
            out.push(a);
            if let Some(d) = heal_drift(conn, tool, &owner, Some(ident), Some((live.as_slice(), &who)), held)? {
                out.push(d);
            }
            note_live_verdict(tool, false);
        }
        identity::Verdict::Unknown => {}
    }
    Ok(out)
}

/// Name the "checking" tokens (LT-A8): each idle one that doesn't share
/// the live sign-in is refreshed when it is about to expire, then asked
/// about. Its account's existing token (not pinned, not in use) takes the
/// bytes when they are newer and the checking row is deleted (its slot
/// goes to `.removed/`); otherwise the row is renamed after the account
/// and recorded, with the notice. After 24 h with no answer it says K2
/// couldn't check; it is never merged blind.
pub fn resolve_unverified(conn: &Connection, tool: Tool, refresher: &dyn Refresher, now_secs: i64) -> Result<Vec<Adopted>, WalletError> {
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    let mut out = Vec::new();
    let owner = live_owner(conn, tool)?;
    for e in list_for(conn, tool)? {
        if e.state != state::UNKNOWN || e.is_api_key() || identity::read_record(tool, &e.id).is_some() {
            continue;
        }
        if owner.as_deref() == Some(e.id.as_str()) || slot_busy(conn, &e.id)? || identity::shares_live_grant(tool, &e.id) {
            continue;
        }
        let Ok(mut slot) = store::lock_slot(tool, &e.id, Duration::from_secs(5)) else { continue };
        let meta = slots::slot_meta(tool, &e.id);
        if meta.has_refresh && meta.expires_at.is_some_and(|x| x <= now_secs + 300) && !crate::airgap::enabled() {
            if let Err(err) = refresher.refresh(tool, &e.id) {
                crate::log_debug!("[llm-accounts] {}: checking token {} not refreshed ({})", tool.as_str(), e.id, if super::looks_like_secret(&err) { "refresh failed" } else { &err });
            }
        }
        let Some(bytes) = slots::read_slot(tool, &e.id)? else { continue };
        let ident = |t: Tool, b: &[u8]| refresher.identify(t, b);
        let Some(who) = identity::actual_identity(tool, &bytes, Some(&ident), false) else {
            if now_secs - e.created_at > 24 * 3600 {
                update_meta(conn, &e.id, &MetaUpdate {
                    detail: Some(Some("K2 couldn't check which account this sign-in belongs to. Keep it, or Remove it.".into())),
                    ..Default::default()
                })?;
            }
            continue;
        };
        let eligible = list_for(conn, tool)?.into_iter().find(|x| {
            x.id != e.id
                && !x.is_api_key()
                && x.state != state::SIGNING_IN
                && identity::read_record(tool, &x.id).is_some_and(|r| r.identity().same_account(&who.0) == Some(true))
        });
        if let Some(x) = eligible.filter(|x| !slot_busy(conn, &x.id).unwrap_or(true)) {
            let _xslot = store::lock_slot(tool, &x.id, LOCK_WAIT)?;
            let newer = match slots::read_slot(tool, &x.id)? {
                None => true,
                Some(old) => expires_of(tool, &bytes) > expires_of(tool, &old),
            };
            if newer {
                if let Some(old) = slots::read_slot(tool, &x.id)?.filter(|o| !identity::same_grant(tool, o, &bytes)) {
                    store::set_aside_slot_cred(tool, &x.id, &old, "replaced")?;
                }
                file_bytes(tool, &x.id, Some(&x), &bytes, Incoming::Known(&who.0, who.1), identity::origin::IMPORT)?;
                let m = store::parse_meta(tool, &bytes);
                update_meta(conn, &x.id, &MetaUpdate { expires_at: Some(m.expires_at), state: Some(state::SIGNED_IN.into()), detail: Some(None), ..Default::default() })?;
            }
            store::set_aside_slot_cred(tool, &e.id, &bytes, "merged")?;
            let dir = store::slot_dir(tool, &e.id);
            if dir.exists() {
                let dest = store::removed_root().join(format!("{}-{}-{}", tool.as_str(), e.id, now()));
                store::rename_locked_slot_dir(&mut slot, &dir, &dest).map_err(|err| WalletError::Io(err.to_string()))?;
            }
            conn.execute("DELETE FROM llm_accounts WHERE id = ?1", rusqlite::params![e.id])?;
            out.push(Adopted {
                tool: tool.as_str().into(),
                id: x.id.clone(),
                label: x.label.clone(),
                account: who.0.describe(),
                created: false,
                notify: false,
                found_in: "checking".into(),
                found_under: e.label.clone(),
                same_account_as: None,
            });
            continue;
        }
        let label = account_label(conn, tool, &who.0)?;
        super::rename(conn, &e.id, &label)?;
        identity::write_record(tool, &e.id, &who.0, who.1, identity::origin::OUTSIDE_K2)?;
        if tool == Tool::Claude {
            let _ = identity::save_slot_profile(&e.id, &who.0.to_oauth_account());
        }
        let mut u = MetaUpdate { state: Some(state::SIGNED_IN.into()), detail: Some(None), ..Default::default() };
        u.email = who.0.email.clone();
        update_meta(conn, &e.id, &u)?;
        conn.execute("UPDATE llm_accounts SET created_by = ?2 WHERE id = ?1", rusqlite::params![e.id, OUTSIDE_K2])?;
        out.push(Adopted {
            tool: tool.as_str().into(),
            id: e.id.clone(),
            label,
            account: who.0.describe(),
            created: true,
            notify: first_notice(tool, &who.0),
            found_in: "checking".into(),
            found_under: e.label.clone(),
            same_account_as: None,
        });
    }
    Ok(out)
}

/// The refusal when a token's saved sign-in is not its account. Never
/// names a token; says where each identity came from.
fn account_mismatch(label: &str, expected: &(AccountIdentity, Source), actual: &(AccountIdentity, Source)) -> WalletError {
    // No account names in the log (the refusal itself shows them to the person).
    crate::log_debug!(
        "[llm-accounts] WARN account mismatch: token {label:?} is on record as one account ({}), its sign-in is another ({})",
        expected.1.describe(),
        actual.1.describe()
    );
    WalletError::Conflict(
        "account_mismatch",
        format!(
            "K2 stopped the switch: the saved sign-in for \"{label}\" belongs to {} ({}), not {} ({}). The server default is unchanged. Sign \"{label}\" in again on Settings → Language Models → Subs / Tokens.",
            actual.0.describe(),
            actual.1.describe(),
            expected.0.describe(),
            expected.1.describe()
        ),
    )
}
/// Claude only: make the token's profile the live `oauthAccount`, so
/// `/status` names the account that is live. Best effort: the token swap
/// already happened; a failure is logged. Returns whether it was written.
fn swap_in_profile(tool: Tool, id: &str, vendor: Option<&AccountIdentity>) -> bool {
    if tool != Tool::Claude {
        return false;
    }
    let profile = identity::slot_profile(id).map(|(p, _)| p).or_else(|| vendor.map(AccountIdentity::to_oauth_account));
    let Some(p) = profile else {
        crate::log_debug!("[llm-accounts] claude: no profile on record for {id}; /status may still name the previous account");
        return false;
    };
    match identity::set_live_profile(&p) {
        Ok(written) => written,
        Err(err) => {
            crate::log_debug!(
                "[llm-accounts] WARN claude: could not update .claude.json oauthAccount ({err}); /status may name the previous account"
            );
            false
        }
    }
}

fn meta_update_from(m: &CredMeta, st: &str) -> MetaUpdate {
    MetaUpdate {
        state: Some(st.to_string()),
        email: m.email.clone(),
        org: m.org.clone(),
        plan: m.plan.clone(),
        expires_at: Some(m.expires_at),
        ..Default::default()
    }
}

fn unique_label(conn: &Connection, tool: Tool, base: &str) -> Result<String, WalletError> {
    let existing: Vec<String> = list_for(conn, tool)?.into_iter().map(|e| e.label.to_lowercase()).collect();
    if !existing.contains(&base.to_lowercase()) {
        return Ok(base.to_string());
    }
    for n in 2..1000 {
        let l = format!("{base} {n}");
        if !existing.contains(&l.to_lowercase()) {
            return Ok(l);
        }
    }
    Err(WalletError::DuplicateLabel(base.to_string()))
}

/// Import the tool's live login as the active wallet entry when nothing
/// is active yet (boot, first run). Returns the new entry, or `None`
/// when a login is already active or the tool is signed out.
pub fn import_live(conn: &Connection, tool: Tool, by: Option<&str>) -> Result<Option<Entry>, WalletError> {
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    import_live_locked(conn, tool, by, "Default")
}

fn import_live_locked(conn: &Connection, tool: Tool, by: Option<&str>, label: &str) -> Result<Option<Entry>, WalletError> {
    if active_id(conn, tool)?.is_some() {
        return Ok(None);
    }
    let Some(bytes) = store::read_live(tool)? else {
        return Ok(None);
    };
    if let Err(why) = store::validate_cred(tool, &bytes) {
        // Never copy a broken live store into the wallet.
        crate::log_debug!("[llm-accounts] {}: live login not imported ({why})", tool.as_str());
        return Ok(None);
    }
    let label = unique_label(conn, tool, label)?;
    let e = super::create(conn, tool, &label, by)?;
    // The account it names now (Claude: its own ~/.claude.json, written by
    // Claude, since K2 has written none yet) is its record from here on.
    let who = identity::actual_identity(tool, &bytes, None, true);
    file_bytes(tool, &e.id, Some(&e), &bytes, Incoming::of(who.as_ref()), identity::origin::IMPORT)?;
    if tool == Tool::Claude {
        // The live profile names the account at import (the moment K2
        // first sees this login); later switches compare against it.
        if let Some(p) = identity::live_profile() {
            if let Err(err) = identity::save_slot_profile(&e.id, &p) {
                crate::log_debug!("[llm-accounts] claude: profile not recorded at import ({err})");
            }
        }
    }
    let mut m = store::parse_meta(tool, &bytes);
    let live = store::live_meta(tool).unwrap_or_default();
    m.email = m.email.or(live.email);
    m.org = m.org.or(live.org);
    update_meta(conn, &e.id, &meta_update_from(&m, state::SIGNED_IN))?;
    set_active(conn, tool, &e.id, by)?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id)).map(Some)
}

/// Start a new login: the row (state `signing_in`) and its private slot
/// folder. The daemon then runs the tool's own login command with the
/// slot as the tool's home.
pub fn begin_new(conn: &Connection, tool: Tool, label: &str, by: Option<&str>) -> Result<Entry, WalletError> {
    let e = super::create(conn, tool, label, by)?;
    store::ensure_private_dir(&store::slot_dir(tool, &e.id)).map_err(|err| WalletError::Io(err.to_string()))?;
    update_meta(conn, &e.id, &MetaUpdate { state: Some(state::SIGNING_IN.into()), ..Default::default() })?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// Mark an existing entry as signing in again (re-login).
pub fn begin_relogin(conn: &Connection, id: &str) -> Result<Entry, WalletError> {
    begin_relogin_settled(conn, id, None).map(|(e, _)| e)
}

/// [`begin_relogin`], first settling the live sign-in: when the token is
/// on record as live but someone signed in to another account outside
/// K2, that sign-in is saved as its own token, so this one can be signed
/// in again (re-adding a token whose sign-in got swapped always works).
pub fn begin_relogin_settled(conn: &Connection, id: &str, by: Option<&str>) -> Result<(Entry, Vec<Adopted>), WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    let mut adopted = Vec::new();
    if tool.subscription_supported() && live_owner(conn, tool)?.as_deref() == Some(e.id.as_str()) {
        let _ = by;
        adopted = reconcile_live(conn, tool, None, false)?;
    }
    begin_relogin_inner(conn, &e.id).map(|e| (e, adopted))
}

fn begin_relogin_inner(conn: &Connection, id: &str) -> Result<Entry, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    if live_owner(conn, tool)?.as_deref() == Some(e.id.as_str()) {
        // Logging the active login in again belongs to the CLI itself
        // (`/login`); K2 signs in into a wallet slot only.
        return Err(WalletError::ActiveLogin);
    }
    if e.is_api_key() {
        return Err(WalletError::Conflict("api_key_login", "API tokens have no sign-in; replace the key instead".into()));
    }
    if pins::is_pinned(conn, &e.id)? {
        return Err(pinned_conflict(&e));
    }
    store::ensure_private_dir(&store::slot_dir(tool, &e.id)).map_err(|err| WalletError::Io(err.to_string()))?;
    update_meta(conn, &e.id, &MetaUpdate { state: Some(state::SIGNING_IN.into()), detail: Some(None), ..Default::default() })?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// After the login command exits: move a macOS keychain item Claude made
/// for the slot into the slot file (then delete that temporary item),
/// tighten modes, record metadata. When the tool has no active login and
/// its live store is empty, the new login becomes the active one.
pub fn finalize_login(conn: &Connection, id: &str, by: Option<&str>) -> Result<Entry, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    let dir = store::slot_dir(tool, &e.id);
    // The CLI's own sign-in: Claude's per-slot keychain item (macOS), else
    // the file the CLI wrote into the slot.
    let mut keychain_item = None;
    if tool == Tool::Claude && store::keychain_enabled() {
        let svc = store::claude_keychain_service(Some(&dir));
        let acct = store::claude_keychain_account();
        if let Ok(Some(bytes)) = store::keychain::read(&svc, &acct) {
            keychain_item = Some((svc, acct, bytes));
        }
    }
    let fresh = match &keychain_item {
        Some((_, _, b)) => Some(b.clone()),
        None => slots::read_slot(tool, &e.id)?,
    };
    let Some(fresh) = fresh else {
        update_meta(conn, &e.id, &MetaUpdate {
            state: Some(state::NOT_SET_UP.into()),
            detail: Some(Some("sign-in did not finish".into())),
            ..Default::default()
        })?;
        return Err(WalletError::NotSignedIn(e.id));
    };
    // Who signed in: the credential (Codex, Grok), else the profile this
    // sign-in itself wrote into the slot (Claude).
    let profile = if tool == Tool::Claude {
        std::fs::read(dir.join(".claude.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .map(|v| v["oauthAccount"].clone())
            .filter(|v| v.is_object())
    } else {
        None
    };
    let who = identity::cred_identity(tool, &fresh)
        .map(|i| (i, Source::Credential))
        .or_else(|| profile.as_ref().and_then(AccountIdentity::from_oauth_account).map(|i| (i, Source::ClaudeConfig)));
    let record = identity::read_record(tool, &e.id).map(|r| (r.identity(), Source::K2Record)).or_else(|| {
        // A token from before records: what K2 had on file for it.
        if tool == Tool::Claude {
            identity::slot_profile(&e.id)
                .filter(|(_, s)| *s == Source::K2Record)
                .and_then(|(p, s)| AccountIdentity::from_oauth_account(&p).map(|i| (i, s)))
        } else {
            None
        }
    });
    if let (Some(rec), Some(w)) = (&record, &who) {
        if rec.0.same_account(&w.0) == Some(false) {
            // Signing a token in again as ANOTHER account (LT-A11): a token
            // means one account, so the sign-in goes to that account's
            // token and this one still needs its own.
            let a = adopt_sign_in(conn, tool, &fresh, w, profile.as_ref(), Some(&e), None, false, "sign_in", false)?;
            if let Some((svc, acct, _)) = &keychain_item {
                store::keychain::delete(svc, acct).map_err(WalletError::Io)?;
            }
            if keychain_item.is_none() {
                store::set_aside_slot_cred(tool, &e.id, &fresh, "signed-in-elsewhere")?;
            }
            if tool == Tool::Claude {
                if let Some((p, Source::K2Record)) = identity::slot_profile(&e.id) {
                    let _ = identity::reset_slot_config_profile(&e.id, &p);
                }
            }
            let hint = format!(
                "That sign-in is {}'s, so K2 saved it as \"{}\". \"{}\" still needs {}: sign it in again with that account.",
                w.0.describe(),
                a.label,
                e.label,
                rec.0.describe()
            );
            let st = if slots::has_cred(tool, &e.id) { state::SIGNED_IN } else { state::NEEDS_LOGIN };
            update_meta(conn, &e.id, &MetaUpdate { state: Some(st.into()), detail: Some(Some(hint.clone())), ..Default::default() })?;
            return Err(WalletError::Moved(hint, vec![a]));
        }
    }
    file_bytes(tool, &e.id, Some(&e), &fresh, Incoming::of(who.as_ref()), identity::origin::K2_SIGN_IN)?;
    if let Some((svc, acct, _)) = &keychain_item {
        store::keychain::delete(svc, acct).map_err(WalletError::Io)?;
    }
    let _ = store::set_mode(&store::slot_cred_path(tool, &e.id), 0o600);
    if let Some(p) = &profile {
        if let Err(err) = identity::save_slot_profile(&e.id, p) {
            crate::log_debug!("[llm-accounts] claude: profile not recorded at sign-in ({err})");
        }
    }
    let m = slots::slot_meta(tool, &e.id);
    let mut u = meta_update_from(&m, state::SIGNED_IN);
    u.detail = Some(None);
    u.refreshed_at = Some(now());
    update_meta(conn, &e.id, &u)?;
    if active_id(conn, tool)?.is_none() && store::read_live(tool)?.is_none() {
        let bytes = slots::read_slot(tool, &e.id)?.ok_or_else(|| WalletError::NotSignedIn(e.id.clone()))?;
        store::write_live(tool, &bytes)?;
        set_active(conn, tool, &e.id, by)?;
        swap_in_profile(tool, &e.id, None);
    }
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// Result of a switch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchOutcome {
    pub tool: String,
    pub from: Option<String>,
    pub to: String,
    /// Labels (for the audit line and the CLI).
    pub from_label: Option<String>,
    pub to_label: String,
    /// Claude: `oauthAccount` in `.claude.json` now names the new login.
    pub profile_swapped: bool,
    /// How the incoming login's account was confirmed (`vendor`,
    /// `credential`), when it was.
    pub verified_by: Option<Source>,
    /// The outgoing live login was saved back to its slot.
    pub saved_outgoing: bool,
    /// The incoming slot was refreshed before the swap.
    pub refreshed: bool,
    pub already_active: bool,
    /// Sign-ins found under the wrong token and saved as their account's
    /// own token on the way (a sign-in made outside K2, slot drift).
    pub adopted: Vec<Adopted>,
    /// Tokens whose slot held another account's sign-in: they need
    /// signing in again (labels).
    pub needs_sign_in: Vec<String>,
}

/// A tool's switch state: which token is the default, which is in the
/// live store, and since when. A two-phase switch compares it before and
/// after its network calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub active: Option<String>,
    pub live_owner: Option<String>,
    pub switched_at: Option<i64>,
}

pub fn generation(conn: &Connection, tool: Tool) -> Result<Generation, WalletError> {
    let info = super::active_info(conn, tool)?;
    Ok(Generation {
        active: info.as_ref().map(|i| i.account_id.clone()),
        live_owner: live_owner(conn, tool)?,
        switched_at: info.map(|i| i.switched_at),
    })
}

fn changed(tool: Tool) -> WalletError {
    WalletError::Conflict(
        "changed",
        format!(
            "The {} tokens changed while K2 checked which accounts they belong to (another switch, a sign-in, or the CLI renewing its token). Nothing was switched; try again.",
            tool.display()
        ),
    )
}

/// What phase 1 of a switch learned without the database: the incoming
/// slot's refresh, and who the live and incoming credentials belong to.
/// Only fingerprints are kept, never the bytes.
#[derive(Debug, Clone)]
pub struct Precheck {
    tool: Tool,
    target_id: String,
    generation: Generation,
    /// Set when phase 1 looked at the credentials (a subscription that
    /// has to be swapped in).
    creds: Option<PrecheckCreds>,
    refresh: Option<Result<(), String>>,
}

#[derive(Debug, Clone)]
struct PrecheckCreds {
    live: Option<([u8; 32], Option<AccountIdentity>)>,
    incoming: [u8; 32],
    incoming_id: Option<AccountIdentity>,
    /// The live owner's own slot (checked for drift when the live
    /// sign-in turns out to be another account's).
    owner_slot: Option<([u8; 32], Option<AccountIdentity>)>,
}

/// Phase 1 of a switch. Holds NO database lock: the caller reads
/// [`generation`] briefly first, then calls this, then
/// [`switch_prechecked`] under the database. It refreshes an expiring
/// incoming slot (under that slot's lock only) and asks the vendor which
/// accounts the live and incoming credentials belong to (no lock at
/// all). Phase 2 refuses with `changed` if anything moved in between.
pub fn precheck(tool: Tool, target: &Entry, gen: Generation, refresher: Option<&dyn Refresher>) -> Result<Precheck, WalletError> {
    let mut pre = Precheck { tool, target_id: target.id.clone(), generation: gen, creds: None, refresh: None };
    let swaps = !target.is_api_key()
        && pre.generation.active.as_deref() != Some(target.id.as_str())
        && pre.generation.live_owner.as_deref() != Some(target.id.as_str())
        && target.state != state::SIGNING_IN
        && slots::has_cred(tool, &target.id);
    if !swaps {
        // Phase 2 answers (already active, API key, refusals) without
        // the network.
        return Ok(pre);
    }
    {
        let _slot = store::lock_slot(tool, &target.id, LOCK_WAIT)?;
        let meta = slots::slot_meta(tool, &target.id);
        let expiring = meta.expires_at.map(|x| x <= now() + 300).unwrap_or(false);
        // Never refresh a copy of the live sign-in (LT-A10).
        if expiring && meta.has_refresh && !crate::airgap::enabled() && !identity::shares_live_grant(tool, &target.id) {
            if let Some(r) = refresher {
                pre.refresh = Some(r.refresh(tool, &target.id));
            }
        }
    }
    let Some(incoming) = slots::read_slot(tool, &target.id)? else {
        return Ok(pre);
    };
    let live = store::read_live(tool)?.filter(|b| store::validate_cred(tool, b).is_ok());
    let owner_slot = match pre.generation.live_owner.as_deref() {
        Some(o) if o != target.id => slots::read_slot(tool, o)?.filter(|b| store::validate_cred(tool, b).is_ok()),
        _ => None,
    };
    let ask = |b: &[u8]| refresher.and_then(|r| r.identify(tool, b));
    let live_id = live.as_deref().map(|b| (fingerprint(b), ask(b)));
    pre.creds = Some(PrecheckCreds {
        owner_slot: owner_slot.as_deref().map(|b| {
            let fp = fingerprint(b);
            // The same bytes as live: one question, not two.
            match &live_id {
                Some((lfp, id)) if *lfp == fp => (fp, id.clone()),
                _ => (fp, ask(b)),
            }
        }),
        live: live_id,
        incoming: fingerprint(&incoming),
        incoming_id: ask(&incoming),
    });
    Ok(pre)
}

/// Make `target` the active login for its tool: save the outgoing live
/// login back to its slot (the CLI may have refreshed it), refresh the
/// incoming slot if it is about to expire (skipped under air-gap), then
/// write it into the tool's live store. Every session on the server
/// picks it up. One phase: any vendor call runs inside the caller's
/// locks. The daemon uses [`precheck`] + [`switch_prechecked`] instead,
/// so it never waits on the network while holding the database.
pub fn switch(
    conn: &Connection,
    tool: Tool,
    target_key: &str,
    by: Option<&str>,
    refresher: Option<&dyn Refresher>,
) -> Result<SwitchOutcome, WalletError> {
    switch_inner(conn, tool, target_key, by, refresher, None)
}

/// Phase 2 of a switch: everything [`switch`] does, with no network. The
/// vendor's answers come from `pre`; if the default, the live credential
/// or the incoming slot changed since phase 1, it refuses (`changed`).
pub fn switch_prechecked(
    conn: &Connection,
    tool: Tool,
    target_key: &str,
    by: Option<&str>,
    pre: &Precheck,
) -> Result<SwitchOutcome, WalletError> {
    if pre.tool != tool {
        return Err(changed(tool));
    }
    switch_inner(conn, tool, target_key, by, None, Some(pre))
}

fn switch_inner(
    conn: &Connection,
    tool: Tool,
    target_key: &str,
    by: Option<&str>,
    refresher: Option<&dyn Refresher>,
    pre: Option<&Precheck>,
) -> Result<SwitchOutcome, WalletError> {
    let target = lookup(conn, Some(tool), target_key)?;
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    if let Some(p) = pre {
        if p.target_id != target.id || generation(conn, tool)? != p.generation {
            return Err(changed(tool));
        }
    }
    let current = active_id(conn, tool)?;
    let from_label = match current.as_deref() {
        Some(c) => get(conn, c)?.map(|e| e.label),
        None => None,
    };
    let outcome = |saved: bool, refreshed: bool, already: bool, profile: bool, verified: Option<Source>| SwitchOutcome {
        tool: tool.as_str().into(),
        from: current.clone(),
        to: target.id.clone(),
        from_label: from_label.clone(),
        to_label: target.label.clone(),
        profile_swapped: profile,
        verified_by: verified,
        saved_outgoing: saved,
        refreshed,
        already_active: already,
        adopted: Vec::new(),
        needs_sign_in: Vec::new(),
    };
    if current.as_deref() == Some(target.id.as_str()) {
        return Ok(outcome(false, false, true, false, None));
    }
    if target.state == state::SIGNING_IN {
        // Its slot may be rewritten under it at any moment.
        return Err(WalletError::Conflict(
            "signing_in",
            format!("\"{}\" is signing in right now; finish or cancel that sign-in first", target.label),
        ));
    }
    if !slots::has_cred(tool, &target.id) {
        // Vault storage with the vault closed: say so (503), not "no
        // sign-in" (the read answers `vault_unavailable`).
        if target.is_api_key() {
            slots::read_api_key(tool, &target.id)?;
        } else {
            slots::read_slot(tool, &target.id)?;
        }
        return Err(WalletError::NotSignedIn(target.id));
    }
    if !target.is_api_key() && pins::is_pinned(conn, &target.id)? {
        return Err(pinned_conflict(&target));
    }
    // The subscription login whose credential is in the live store now.
    let owner = live_owner(conn, tool)?;
    if target.is_api_key() {
        // An API key is env-injected into new sessions; the live store is
        // left as it is and remembered as the subscription in it.
        set_active_with_live(conn, tool, &target.id, owner.as_deref(), by)?;
        return Ok(outcome(false, false, false, false, None));
    }
    if owner.as_deref() == Some(target.id.as_str()) {
        // Back from an API key to the login already in the live store.
        set_active(conn, tool, &target.id, by)?;
        return Ok(outcome(false, false, false, false, None));
    }
    // Two-phase: phase 1 must have looked at these credentials.
    let pre_creds = match pre {
        Some(p) => Some(p.creds.as_ref().ok_or_else(|| changed(tool))?),
        None => None,
    };
    // Who a credential belongs to: phase 1's answer for the exact bytes
    // it saw (never a network call here), or the vendor in one phase.
    let ident = |t: Tool, b: &[u8]| -> Option<AccountIdentity> {
        match pre_creds {
            Some(c) => {
                let fp = fingerprint(b);
                if fp == c.incoming {
                    return c.incoming_id.clone();
                }
                [c.live.as_ref(), c.owner_slot.as_ref()]
                    .into_iter()
                    .flatten()
                    .find(|(lfp, _)| *lfp == fp)
                    .and_then(|(_, id)| id.clone())
            }
            None => refresher.and_then(|r| r.identify(t, b)),
        }
    };
    let ident: &dyn Fn(Tool, &[u8]) -> Option<AccountIdentity> = &ident;
    let _target_slot = store::lock_slot(tool, &target.id, LOCK_WAIT)?;
    // 0. The incoming login must be whole before anything moves.
    let incoming = slots::read_slot(tool, &target.id)?.ok_or_else(|| WalletError::NotSignedIn(target.id.clone()))?;
    store::validate_cred(tool, &incoming).map_err(|why| store::invalid_cred(tool, "saved sign-in", why))?;
    // 1. Save the outgoing live login (the CLI may have refreshed it).
    //    A live login K2 doesn't know is kept as "Previous subscription".
    //    A live store that isn't a whole credential (e.g. a write cut off
    //    part way) is NEVER mirrored: it would overwrite the outgoing
    //    login's only good copy. It is replaced by the swap below.
    let live_raw = store::read_live(tool)?;
    // What the swap below expects to replace (A3): if the CLI rewrites
    // its store after this read, `write_live_expecting` refuses with
    // `changed` instead of overwriting the refreshed token.
    let live_fp = live_raw.as_deref().map(fingerprint);
    let live = live_raw.filter(|bytes| match store::validate_cred(tool, bytes) {
        Ok(()) => true,
        Err(why) => {
            crate::log_debug!("[llm-accounts] {}: live login not saved back ({why})", tool.as_str());
            false
        }
    });
    if let Some(c) = pre_creds {
        if live.as_deref().map(fingerprint) != c.live.as_ref().map(|(fp, _)| *fp) {
            return Err(changed(tool));
        }
    }
    //    The live bytes are filed where they belong (LT-A7), never refused:
    //    - the owner's own account (or the same sign-in as its slot): into
    //      the owner's slot, as before;
    //    - another account (a `/login` outside K2): that account's own
    //      token, which becomes the live owner; the old owner's slot is left
    //      as it was (switching back needs no sign-in), then checked for
    //      drift;
    //    - an account nobody can name yet: a "checking" token (LT-A8).
    let mut adopted: Vec<Adopted> = Vec::new();
    let mut needs_sign_in: Vec<String> = Vec::new();
    let mut owner = owner;
    let mut saved = false;
    let mut live_actual = None;
    if let Some(bytes) = &live {
        let owner_now = match owner.as_deref() {
            Some(o) => get(conn, o)?,
            None => None,
        };
        match owner_now {
            Some(oe) => {
                let (v, actual) = live_verdict(tool, &oe, bytes, Some(ident));
                live_actual = actual.clone();
                note_live_verdict(tool, false);
                let unknown_into_owner = v == identity::Verdict::Unknown && identity::recorded_identity(tool, &oe).is_none();
                match v {
                    identity::Verdict::Match => {
                        let _owner_slot = if oe.id == target.id { None } else { Some(store::lock_slot(tool, &oe.id, LOCK_WAIT)?) };
                        file_bytes(tool, &oe.id, Some(&oe), bytes, Incoming::of(actual.as_ref()), identity::origin::IMPORT)?;
                        let m = store::parse_meta(tool, bytes);
                        update_meta(conn, &oe.id, &MetaUpdate { expires_at: Some(m.expires_at), state: Some(state::SIGNED_IN.into()), ..Default::default() })?;
                        saved = true;
                    }
                    identity::Verdict::Mismatch(w, src) => {
                        let who = (w, src);
                        let profile = if tool == Tool::Claude { identity::live_profile() } else { None };
                        let a = adopt_sign_in(conn, tool, bytes, &who, profile.as_ref(), Some(&oe), Some(&target.id), false, "live", true)?;
                        make_live_owner(conn, tool, &oe.id, &a.id, Some(BY_OUTSIDE))?;
                        keep_recorded_email(conn, tool, &oe)?;
                        owner = Some(a.id.clone());
                        saved = true;
                        adopted.push(a);
                        if let Some(d) = heal_drift(conn, tool, &oe, Some(ident), Some((bytes.as_slice(), &who)), Some(&target.id))? {
                            needs_sign_in.push(oe.label.clone());
                            adopted.push(d);
                        }
                    }
                    identity::Verdict::Unknown if unknown_into_owner => {
                        let _owner_slot = if oe.id == target.id { None } else { Some(store::lock_slot(tool, &oe.id, LOCK_WAIT)?) };
                        file_bytes(tool, &oe.id, Some(&oe), bytes, Incoming::Unknown, identity::origin::IMPORT)?;
                        saved = true;
                    }
                    identity::Verdict::Unknown => {
                        let u = file_unverified(conn, tool, bytes, by)?;
                        make_live_owner(conn, tool, &oe.id, &u.id, by)?;
                        owner = Some(u.id);
                        saved = true;
                    }
                }
            }
            None => {
                save_unknown_live(conn, tool, bytes, by)?;
                saved = true;
            }
        }
    }
    let owner_expected = match owner.as_deref() {
        Some(o) => get(conn, o)?.and_then(|e| identity::expected_identity(tool, &e)),
        None => None,
    };
    // 2. Lazy refresh of the incoming idle slot (phase 1 already did it
    //    in the two-phase switch; here only its result is recorded).
    let refresh_result = match pre {
        Some(p) => p.refresh.clone(),
        None => {
            let meta = slots::slot_meta(tool, &target.id);
            let expiring = meta.expires_at.map(|x| x <= now() + 300).unwrap_or(false);
            match refresher {
                Some(r) if expiring && meta.has_refresh && !crate::airgap::enabled() && !identity::shares_live_grant(tool, &target.id) => {
                    Some(r.refresh(tool, &target.id))
                }
                _ => None,
            }
        }
    };
    let mut refreshed = false;
    match refresh_result {
        Some(Ok(())) => {
            refreshed = true;
            let m = slots::slot_meta(tool, &target.id);
            update_meta(conn, &target.id, &MetaUpdate { expires_at: Some(m.expires_at), refreshed_at: Some(now()), ..Default::default() })?;
        }
        Some(Err(e)) => {
            // The CLI refreshes an expired access token itself as long as
            // the refresh token is good, so a failed pre-swap refresh is
            // recorded, not fatal.
            let detail = if super::looks_like_secret(&e) { "refresh failed".to_string() } else { e };
            update_meta(conn, &target.id, &MetaUpdate { detail: Some(Some(detail)), ..Default::default() })?;
        }
        None => {}
    }
    // 3. The incoming sign-in must be the account K2 has on record for it.
    let bytes = slots::read_slot(tool, &target.id)?.ok_or_else(|| WalletError::NotSignedIn(target.id.clone()))?;
    // The live sign-in turned out to be the target's own account: step 1
    // just saved it into the target's slot (fresher than phase 1's copy).
    let target_took_live = owner.as_deref() == Some(target.id.as_str()) && live.as_deref() == Some(bytes.as_slice());
    if let (Some(c), false) = (pre_creds, target_took_live) {
        if fingerprint(&bytes) != c.incoming {
            return Err(changed(tool));
        }
    }
    //    A slot holding another account's sign-in (drift) is never made
    //    live under this label: K2 moves it to that account's own token,
    //    marks this one "sign in again" and refuses, with the default as
    //    it now is (the live sign-in, adopted above if it had to be).
    let incoming_actual = identity::slot_identity(tool, &target.id, &bytes, Some(ident));
    let target_expected = identity::expected_identity(tool, &target);
    if let identity::Verdict::Mismatch(..) = identity::verdict(target_expected.as_ref(), incoming_actual.as_ref()) {
        let Some(moved) = heal_drift(conn, tool, &target, Some(ident), None, Some(&target.id))? else {
            let (Some(exp), Some(act)) = (&target_expected, &incoming_actual) else { unreachable!("a mismatch has both sides") };
            return Err(account_mismatch(&target.label, exp, act));
        };
        let hint = format!(
            "K2 stopped the switch: the saved sign-in for \"{}\" belongs to {}, not {}. K2 saved it as the token \"{}\", and \"{}\" needs signing in again (Settings → Language Models → Subs / Tokens, or `k2 llm tokens login {} \"{}\"`). The server default is unchanged.",
            target.label,
            moved.account,
            target_expected.as_ref().map(|(i, _)| i.describe()).unwrap_or_else(|| "the account on record".into()),
            moved.label,
            target.label,
            tool.as_str(),
            target.label
        );
        adopted.push(moved);
        return Err(WalletError::Moved(hint, adopted));
    }
    // 4. Swap in, only over the live login step 1 saved.
    store::write_live_expecting(tool, &bytes, Some(live_fp)).map_err(|e| match e {
        WalletError::Conflict("live_changed", _) => changed(tool),
        other => other,
    })?;
    set_active(conn, tool, &target.id, by)?;
    update_meta(conn, &target.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() })?;
    // 5. Claude's profile follows the token (`/status` reads it).
    let mut profile_swapped = false;
    if tool == Tool::Claude {
        // Keep the outgoing profile with its token, but only when it
        // names the outgoing account (the live file may be stale).
        if let (Some(o), true) = (&owner, saved) {
            if let Some(lp) = identity::live_profile() {
                let lid = AccountIdentity::from_oauth_account(&lp);
                let same = match (&lid, &owner_expected, &live_actual) {
                    (Some(l), Some((exp, _)), _) => l.same_account(exp) == Some(true),
                    (Some(l), None, Some((act, Source::Vendor))) => l.same_account(act) == Some(true),
                    _ => false,
                };
                if same {
                    if let Err(err) = identity::save_slot_profile(o, &lp) {
                        crate::log_debug!("[llm-accounts] claude: outgoing profile not kept ({err})");
                    }
                }
            }
        }
        // An incoming slot K2 had no record for, now confirmed by Anthropic.
        if let Some((id, Source::Vendor)) = &incoming_actual {
            if identity::slot_profile(&target.id).is_none() {
                let _ = identity::save_slot_profile(&target.id, &id.to_oauth_account());
            }
        }
        let vendor = incoming_actual.as_ref().filter(|(_, s)| *s == Source::Vendor).map(|(id, _)| id);
        profile_swapped = swap_in_profile(tool, &target.id, vendor);
    }
    let verified = incoming_actual.as_ref().map(|(_, s)| *s).filter(|s| matches!(s, Source::Vendor | Source::Credential));
    let mut out = outcome(saved, refreshed, false, profile_swapped, verified);
    out.adopted = adopted;
    out.needs_sign_in = needs_sign_in;
    Ok(out)
}

// ── Login methods ───────────────────────────────────────────────────

/// How a new login is captured for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginMethod {
    /// The tool's own login runs with its home variable pointed at the
    /// wallet slot, so the active login is never touched. Used by Claude
    /// (`CLAUDE_CONFIG_DIR`; the macOS keychain item name is predictable
    /// and is moved into the slot), Codex (`CODEX_HOME` + file store) and
    /// Grok (`GROK_HOME`).
    TempHome,
    /// Fallback for a tool whose home override can't be trusted: snapshot
    /// the live login, let the tool's normal login replace it, capture the
    /// new credential when the live store changes, then restore the
    /// previous login. Signing in temporarily switches the tool for every
    /// session on the server.
    LiveSwap,
}

/// The login method per tool. `K2_LLM_LOGIN_LIVE=grok,claude` flips
/// tools to the fallback without a rebuild (field escape hatch if a CLI
/// stops honoring its home variable).
pub fn login_method(tool: Tool) -> LoginMethod {
    if let Ok(v) = std::env::var("K2_LLM_LOGIN_LIVE") {
        if v.split(',').any(|t| t.trim().eq_ignore_ascii_case(tool.as_str())) {
            return LoginMethod::LiveSwap;
        }
    }
    LoginMethod::TempHome
}

/// Delete a login that never got a credential (cancelled or failed
/// first sign-in): the row and its slot. A no-op for an entry that holds
/// a saved sign-in or was ever active.
pub fn discard_unfinished(conn: &Connection, id: &str) -> Result<(), WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    if slots::has_cred(tool, &e.id) || e.last_used_at.is_some() {
        return Ok(());
    }
    let dir = store::slot_dir(tool, &e.id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|err| WalletError::Io(err.to_string()))?;
    }
    conn.execute("DELETE FROM llm_accounts WHERE id = ?1", rusqlite::params![e.id])?;
    Ok(())
}

fn fingerprint(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// State of a live-swap login. The fingerprint stays in memory only.
#[derive(Debug, Clone)]
pub struct LiveSwapTicket {
    pub tool: Tool,
    /// The new wallet entry being signed in.
    pub new_id: String,
    /// The login that was active when sign-in started (restored after).
    pub previous_id: Option<String>,
    snapshot_fp: Option<[u8; 32]>,
    /// The live sign-in was another account's (made outside K2): saved as
    /// its own token before this sign-in started.
    pub adopted: Vec<Adopted>,
}

/// Start a live-swap login: save the current live login into its wallet
/// slot (importing it as "Default" if nothing is active yet) and remember
/// its fingerprint.
pub fn begin_live_swap(conn: &Connection, tool: Tool, new_id: &str, by: Option<&str>) -> Result<LiveSwapTicket, WalletError> {
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    // Same rule as a switch: a live sign-in of another account than its
    // owner's is saved as its own token (and is the live owner from now
    // on), never into the owner's slot, and never blocks this sign-in.
    let adopted = reconcile_live_locked(conn, tool, None, false, None)?;
    let live = store::read_live(tool)?;
    // A broken live store is fingerprinted (so a change is noticed) but
    // never copied into a slot.
    if let Some(bytes) = live.as_ref().filter(|b| store::validate_cred(tool, b).is_ok()) {
        match (active_id(conn, tool)?, live_owner(conn, tool)?) {
            (Some(_), Some(owner)) => {
                let oe = get(conn, &owner)?.ok_or_else(|| WalletError::NotFound(owner.clone()))?;
                let (v, actual) = live_verdict(tool, &oe, bytes, None);
                if v == identity::Verdict::Match || identity::recorded_identity(tool, &oe).is_none() {
                    let _slot = store::lock_slot(tool, &owner, LOCK_WAIT)?;
                    file_bytes(tool, &owner, Some(&oe), bytes, Incoming::of(actual.as_ref()), identity::origin::IMPORT)?;
                } else {
                    // K2 can't tell whose it is: never into the owner's slot.
                    let u = file_unverified(conn, tool, bytes, by)?;
                    make_live_owner(conn, tool, &owner, &u.id, by)?;
                }
            }
            // An API-key default with no subscription on record as live:
            // as before, the default's slot keeps it for the restore.
            (Some(cur), None) => {
                let ce = get(conn, &cur)?.ok_or_else(|| WalletError::NotFound(cur.clone()))?;
                file_bytes(tool, &cur, Some(&ce), bytes, Incoming::Unknown, identity::origin::IMPORT)?
            }
            (None, _) => {
                import_live_locked(conn, tool, by, "Default")?;
            }
        }
    }
    Ok(LiveSwapTicket {
        tool,
        new_id: new_id.to_string(),
        // What goes back into the live store afterwards: the subscription
        // on record as live (behind an API-key default too).
        previous_id: match live_owner(conn, tool)? {
            Some(o) => Some(o),
            None => active_id(conn, tool)?,
        },
        snapshot_fp: live.as_deref().map(fingerprint),
        adopted,
    })
}

/// Has the live store changed since the snapshot? Returns the new bytes
/// (they stay inside the wallet code path and are never logged).
pub fn live_swap_poll(ticket: &LiveSwapTicket) -> Result<Option<Vec<u8>>, WalletError> {
    match store::read_live(ticket.tool)? {
        // Changed AND whole: a login still being written is not captured.
        Some(b) if Some(fingerprint(&b)) != ticket.snapshot_fp && store::validate_cred(ticket.tool, &b).is_ok() => {
            Ok(Some(b))
        }
        _ => Ok(None),
    }
}

/// The live store changed: save the new credential as the new slot, then
/// restore the previous login so no other session moves without asking
/// (the UI then offers "Make the new login active?"). With no previous
/// login the new one stays active.
pub fn live_swap_complete(conn: &Connection, ticket: &LiveSwapTicket, new_bytes: &[u8], by: Option<&str>) -> Result<Entry, WalletError> {
    let tool = ticket.tool;
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    // The tool's own login just wrote the store and (Claude) its profile:
    // that names the new account.
    let who = identity::actual_identity(tool, new_bytes, None, true);
    let new_entry = get(conn, &ticket.new_id)?.ok_or_else(|| WalletError::NotFound(ticket.new_id.clone()))?;
    file_bytes(tool, &ticket.new_id, Some(&new_entry), new_bytes, Incoming::of(who.as_ref()), identity::origin::K2_SIGN_IN)?;
    if tool == Tool::Claude {
        // The tool's own login just wrote the new account's profile.
        if let Some(p) = identity::live_profile() {
            if let Err(err) = identity::save_slot_profile(&ticket.new_id, &p) {
                crate::log_debug!("[llm-accounts] claude: profile not recorded at sign-in ({err})");
            }
        }
    }
    let mut m = store::parse_meta(tool, new_bytes);
    if tool == Tool::Claude {
        let (e, o) = store::claude_account_labels(&store::claude_live_global_config());
        m.email = m.email.or(e);
        m.org = m.org.or(o);
    }
    let mut u = meta_update_from(&m, state::SIGNED_IN);
    u.detail = Some(None);
    u.refreshed_at = Some(now());
    update_meta(conn, &ticket.new_id, &u)?;
    match &ticket.previous_id {
        Some(prev) => {
            let bytes = slots::read_slot(tool, prev)?.ok_or_else(|| WalletError::NotSignedIn(prev.clone()))?;
            store::write_live(tool, &bytes)?;
            swap_in_profile(tool, prev, None);
        }
        None => set_active(conn, tool, &ticket.new_id, by)?,
    }
    get(conn, &ticket.new_id)?.ok_or(WalletError::NotFound(ticket.new_id.clone()))
}

/// Timeout, cancel or failure: put the previous login back into the live
/// store (when there was one) and delete the half-made slot.
pub fn live_swap_abort(conn: &Connection, ticket: &LiveSwapTicket) -> Result<(), WalletError> {
    let tool = ticket.tool;
    {
        let _lock = store::lock_tool(tool, LOCK_WAIT)?;
        if let Some(prev) = &ticket.previous_id {
            if let Some(bytes) = slots::read_slot(tool, prev)? {
                store::write_live(tool, &bytes)?;
                swap_in_profile(tool, prev, None);
            }
        }
    }
    discard_unfinished(conn, &ticket.new_id)
}

/// Result of a remove.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveOutcome {
    pub id: String,
    pub moved_to: Option<String>,
}

/// What clearing a tool's server default did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearOutcome {
    pub tool: String,
    /// The token that was the server default (`None`: there was none).
    pub cleared: Option<String>,
    pub cleared_label: Option<String>,
    /// The subscription still signed in to the tool's own store, which is
    /// what chats on Server default use without the API token, so it is
    /// the default again; `None`: the tool has no server default now.
    pub now_default: Option<String>,
    pub now_default_label: Option<String>,
}

/// Leave a tool with no server default token (smoke r6: the only token,
/// an API token, could never be removed). Only an API token can be
/// cleared: it reaches chats as an env var K2 adds. A subscription sits in
/// the tool's own store, and every chat on Server default uses it
/// whatever K2 records, so it is refused (`subscription_default`) with
/// what to do instead. The caller checks Admin and above.
pub fn clear_default(conn: &Connection, tool: Tool, by: Option<&str>) -> Result<ClearOutcome, WalletError> {
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    clear_default_locked(conn, tool, by)
}

fn clear_default_locked(conn: &Connection, tool: Tool, by: Option<&str>) -> Result<ClearOutcome, WalletError> {
    let mut out = ClearOutcome {
        tool: tool.as_str().into(),
        cleared: None,
        cleared_label: None,
        now_default: None,
        now_default_label: None,
    };
    let Some(info) = super::active_info(conn, tool)? else {
        return Ok(out);
    };
    let active = get(conn, &info.account_id)?.filter(|e| e.removed_at.is_none());
    if let Some(e) = &active {
        if !e.is_api_key() {
            return Err(WalletError::Conflict("subscription_default", subscription_default_hint(tool, e)));
        }
        out.cleared = Some(e.id.clone());
        out.cleared_label = Some(e.label.clone());
    }
    let back = match info.live_account_id.as_deref() {
        Some(l) => get(conn, l)?.filter(|e| e.removed_at.is_none() && !e.is_api_key()),
        None => None,
    };
    match back {
        Some(l) => {
            set_active(conn, tool, &l.id, by)?;
            out.now_default = Some(l.id);
            out.now_default_label = Some(l.label);
        }
        None => {
            conn.execute("DELETE FROM llm_active WHERE tool = ?1", rusqlite::params![tool.as_str()])?;
        }
    }
    Ok(out)
}

fn subscription_default_hint(tool: Tool, e: &Entry) -> String {
    format!(
        "{} is a subscription signed in to {}'s own store, so every chat on Server default uses it whatever K2 records. \
         Make another token the server default first (`k2 llm tokens switch {} <label>`, Admin or Owner), or sign {} out in a terminal outside K2.",
        e.label,
        tool.display(),
        tool.as_str(),
        tool.display()
    )
}

/// Why the server default can't simply be removed, and how to.
fn active_refusal(tool: Tool, e: &Entry) -> WalletError {
    if e.is_api_key() {
        WalletError::Conflict(
            "active_login",
            format!(
                "{} is the server default {} token. Remove it and leave no default: `k2 llm tokens remove {} \"{}\" --clear-default` \
                 (Admin or Owner; Settings asks you to confirm), or make another token the server default first.",
                e.label,
                tool.display(),
                tool.as_str(),
                e.label
            ),
        )
    } else {
        WalletError::Conflict("active_login", subscription_default_hint(tool, e))
    }
}

/// Soft remove an idle login: move its slot to
/// `~/.k2/llm-accounts/.removed/<id>-<ts>/` and mark the row removed.
/// The server default can't be removed (switch first; an API token may
/// clear the default with [`remove_clearing_default`]).
pub fn remove(conn: &Connection, id: &str) -> Result<RemoveOutcome, WalletError> {
    remove_inner(conn, id, false, None)
}

/// [`remove`], and when the token is the server default API token, clear
/// the default first ([`clear_default`]; the person confirmed it, the
/// caller checked Admin and above).
pub fn remove_clearing_default(conn: &Connection, id: &str, by: Option<&str>) -> Result<RemoveOutcome, WalletError> {
    remove_inner(conn, id, true, by)
}

fn remove_inner(conn: &Connection, id: &str, clear: bool, by: Option<&str>) -> Result<RemoveOutcome, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    if active_id(conn, tool)?.as_deref() == Some(e.id.as_str()) {
        if !(clear && e.is_api_key()) {
            return Err(active_refusal(tool, &e));
        }
        clear_default_locked(conn, tool, by)?;
    }
    if live_owner(conn, tool)?.as_deref() == Some(e.id.as_str()) {
        // A subscription left in the tool's own store while an API token
        // is the default: chats still fall back to it.
        return Err(WalletError::Conflict("active_login", subscription_default_hint(tool, &e)));
    }
    if pins::is_pinned(conn, &e.id)? {
        return Err(pinned_conflict(&e));
    }
    let dir = store::slot_dir(tool, &e.id);
    let mut moved_to = None;
    if dir.exists() {
        let root = store::removed_root();
        store::ensure_private_dir(&root).map_err(|err| WalletError::Io(err.to_string()))?;
        let dest = root.join(format!("{}-{}-{}", tool.as_str(), e.id, now()));
        std::fs::rename(&dir, &dest).map_err(|err| WalletError::Io(err.to_string()))?;
        moved_to = Some(dest.to_string_lossy().into_owned());
    }
    super::mark_removed(conn, &e.id)?;
    // Vault storage: the sealed copy is kept, disabled (can be undone).
    slots::removed(tool, &e.id);
    Ok(RemoveOutcome { id: e.id, moved_to })
}

/// Recompute one entry's state from its store (live for the active one,
/// the slot otherwise). Local reads only; no vendor call.
pub fn recheck(conn: &Connection, id: &str) -> Result<Entry, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    if e.is_api_key() {
        let st = if slots::has_api_key(tool, &e.id) { state::SIGNED_IN } else { state::NOT_SET_UP };
        update_meta(conn, &e.id, &MetaUpdate { state: Some(st.into()), ..Default::default() })?;
        return get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id));
    }
    let is_active = live_owner(conn, tool)?.as_deref() == Some(e.id.as_str());
    let mut m = if is_active { store::live_meta(tool)? } else { slots::slot_meta(tool, &e.id) };
    // LT-A6: the live store's account labels go on the token on record as
    // live only when the live sign-in IS that account (field 2026-10-27:
    // the default token showed the outside login's email). An idle slot's
    // labels never rename the token's account either. Otherwise the row
    // keeps the account it is on record as.
    let record = identity::recorded_identity(tool, &e);
    let take_labels = if is_active {
        match store::read_live(tool)?.filter(|b| store::validate_cred(tool, b).is_ok()) {
            Some(live) => live_verdict(tool, &e, &live, None).0 == identity::Verdict::Match,
            None => false,
        }
    } else {
        match (m.email.as_deref().and_then(|x| AccountIdentity::from_email(Some(x))), &record) {
            (Some(shown), Some((rec, _))) => rec.same_account(&shown) != Some(false),
            _ => true,
        }
    };
    if !take_labels {
        m.email = record.as_ref().and_then(|(r, _)| r.email.clone()).filter(|x| e.email.as_deref() != Some(x.as_str()));
        m.org = None;
    }
    let st = if e.state == state::SIGNING_IN {
        state::SIGNING_IN
    } else if !m.present {
        if is_active {
            state::NEEDS_LOGIN
        } else if e.state == state::NEEDS_LOGIN {
            // Its sign-in was moved out (drift): "sign in again".
            state::NEEDS_LOGIN
        } else {
            state::NOT_SET_UP
        }
    } else if e.state == state::NEEDS_LOGIN && !is_active {
        state::NEEDS_LOGIN
    } else {
        state::SIGNED_IN
    };
    update_meta(conn, &e.id, &meta_update_from(&m, st))?;
    get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id))
}

/// How often an idle slot is refreshed even when its access token is not
/// near expiry, so the refresh token keeps rolling.
pub const WARM_EVERY_SECS: i64 = 24 * 3600;
/// Refresh an idle slot whose access token expires within this window.
pub const WARM_BEFORE_EXPIRY_SECS: i64 = 3600;

/// Is an idle slot due for a keep-warm refresh?
pub fn warm_due(tool: Tool, meta: &CredMeta, entry: &Entry, now: i64) -> bool {
    if !meta.present {
        return false;
    }
    if tool != Tool::Grok && !meta.has_refresh {
        return false;
    }
    let last = entry
        .refreshed_at
        .or(meta.last_refresh)
        .or(entry.last_used_at)
        .unwrap_or(entry.created_at);
    if now - last >= WARM_EVERY_SECS {
        return true;
    }
    match meta.expires_at {
        Some(x) => tool != Tool::Grok && x - now <= WARM_BEFORE_EXPIRY_SECS,
        None => false,
    }
}

/// One keep-warm outcome.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarmResult {
    pub id: String,
    pub tool: String,
    pub refreshed: bool,
    pub error: Option<String>,
    /// The refreshed slot turned out to hold another account's sign-in:
    /// where K2 moved it (the token now needs signing in again).
    pub adopted: Option<Adopted>,
}

/// Is a refresh error permanent (the refresh token is dead)?
pub fn refresh_error_is_permanent(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    l.contains("invalid_grant") || l.contains("revoked") || l.contains("401") || l.contains("unauthorized")
}

/// Refresh every idle slot that is due. Never the active login, never
/// under air-gap. Each refresh holds the tool lock.
/// Refresh ownership: the CLI refreshes the login in the live store and
/// any pinned slot a running session uses (`in_use`); K2 refreshes every
/// other subscription slot. API keys never refresh.
pub fn keep_warm(
    conn: &Connection,
    refresher: &dyn Refresher,
    now_secs: i64,
    in_use: &dyn Fn(&str) -> bool,
) -> Result<Vec<WarmResult>, WalletError> {
    if crate::airgap::enabled() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for tool in Tool::SUBSCRIPTION {
        let owner = live_owner(conn, tool)?;
        for e in list_for(conn, tool)? {
            if owner.as_deref() == Some(e.id.as_str())
                || e.state == state::SIGNING_IN
                || e.is_api_key()
                || in_use(&e.id)
            {
                continue;
            }
            let meta = slots::slot_meta(tool, &e.id);
            if !warm_due(tool, &meta, &e, now_secs) {
                continue;
            }
            let _lock = match store::lock_tool(tool, Duration::from_secs(5)) {
                Ok(l) => l,
                Err(_) => continue,
            };
            // The slot lock also blocks a pinned session from starting on
            // this login mid-refresh.
            let _slot = match store::lock_slot(tool, &e.id, Duration::from_secs(5)) {
                Ok(l) => l,
                Err(_) => continue,
            };
            // Re-check under the locks: a switch may have made it live, or
            // a pinned session may have started on it.
            if live_owner(conn, tool)?.as_deref() == Some(e.id.as_str()) || in_use(&e.id) {
                continue;
            }
            // Never refresh a copy of the live sign-in: the refresh would
            // revoke the live CLI's token (LT-A10).
            if identity::shares_live_grant(tool, &e.id) {
                continue;
            }
            match refresher.refresh(tool, &e.id) {
                Ok(()) => {
                    // The access token is fresh now, so the vendor can say
                    // whose it is: the drift check (LT-A9).
                    let ident = |t: Tool, b: &[u8]| refresher.identify(t, b);
                    if let Some(moved) = heal_drift(conn, tool, &e, Some(&ident), None, Some(&e.id))? {
                        out.push(WarmResult { id: e.id, tool: tool.as_str().into(), refreshed: true, error: None, adopted: Some(moved) });
                        continue;
                    }
                    let m = slots::slot_meta(tool, &e.id);
                    let mut u = meta_update_from(&m, state::SIGNED_IN);
                    u.refreshed_at = Some(now_secs);
                    u.detail = Some(None);
                    if let (Some(shown), Some((rec, _))) =
                        (m.email.as_deref().and_then(|x| AccountIdentity::from_email(Some(x))), identity::recorded_identity(tool, &e))
                    {
                        if rec.same_account(&shown) == Some(false) {
                            u.email = None;
                            u.org = None;
                        }
                    }
                    update_meta(conn, &e.id, &u)?;
                    out.push(WarmResult { id: e.id, tool: tool.as_str().into(), refreshed: true, error: None, adopted: None });
                }
                Err(err) => {
                    let detail = if super::looks_like_secret(&err) { "refresh failed".to_string() } else { err.clone() };
                    let mut u = MetaUpdate { detail: Some(Some(detail.clone())), ..Default::default() };
                    if refresh_error_is_permanent(&err) {
                        u.state = Some(state::NEEDS_LOGIN.into());
                    }
                    update_meta(conn, &e.id, &u)?;
                    out.push(WarmResult { id: e.id, tool: tool.as_str().into(), refreshed: false, error: Some(detail), adopted: None });
                }
            }
        }
    }
    Ok(out)
}

// ── Claude refresh grant (pure helpers; HTTP lives in the daemon) ──

/// Claude Code's OAuth client id and token endpoint (binary 2.1.292).
pub const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// The scopes Claude asks for when a credential records none.
pub const CLAUDE_DEFAULT_SCOPES: &[&str] = &[
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

/// Build the JSON body of Claude's refresh grant from a slot's bytes.
/// Crate-visible only through [`claude_refresh_slot`].
pub(crate) fn claude_refresh_body(slot: &[u8]) -> Result<serde_json::Value, String> {
    let v: serde_json::Value = serde_json::from_slice(slot).map_err(|_| "unreadable credential".to_string())?;
    let o = &v["claudeAiOauth"];
    let rt = o["refreshToken"].as_str().filter(|s| !s.is_empty()).ok_or("no refresh token")?;
    let scopes: Vec<String> = o["scopes"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| CLAUDE_DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect());
    Ok(serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": rt,
        "client_id": CLAUDE_CLIENT_ID,
        "scope": scopes.join(" "),
    }))
}

/// Apply a refresh response to a slot's bytes (keeps every other field:
/// subscriptionType, rateLimitTier, …). `expiresAt` is milliseconds, as
/// Claude stores it.
pub(crate) fn claude_apply_refresh(slot: &[u8], resp: &serde_json::Value, now_ms: i64) -> Result<Vec<u8>, String> {
    let mut v: serde_json::Value = serde_json::from_slice(slot).map_err(|_| "unreadable credential".to_string())?;
    let at = resp["access_token"].as_str().filter(|s| !s.is_empty()).ok_or("refresh response has no access_token")?;
    let exp = resp["expires_in"].as_i64().ok_or("refresh response has no expires_in")?;
    let o = v["claudeAiOauth"].as_object_mut().ok_or("unreadable credential")?;
    o.insert("accessToken".into(), at.into());
    if let Some(rt) = resp["refresh_token"].as_str().filter(|s| !s.is_empty()) {
        o.insert("refreshToken".into(), rt.into());
    }
    o.insert("expiresAt".into(), (now_ms + exp * 1000).into());
    if let Some(scope) = resp["scope"].as_str().filter(|s| !s.trim().is_empty()) {
        let list: Vec<serde_json::Value> = scope.split_whitespace().map(|s| s.into()).collect();
        o.insert("scopes".into(), list.into());
    }
    serde_json::to_vec(&v).map_err(|e| e.to_string())
}

/// Refresh an idle Claude slot with the OAuth refresh grant. `post` does
/// the HTTP (JSON body → JSON response, or an error string with the
/// status); tests pass a fake. The caller holds the tool lock and has
/// checked the slot is not active and air-gap is off.
pub fn claude_refresh_slot(
    id: &str,
    post: &dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String>,
) -> Result<(), String> {
    let bytes = slots::read_slot(Tool::Claude, id).map_err(|e| e.to_string())?.ok_or("no saved sign-in")?;
    let body = claude_refresh_body(&bytes)?;
    let resp = post(CLAUDE_TOKEN_URL, &body)?;
    let now_ms = now() * 1000;
    let updated = claude_apply_refresh(&bytes, &resp, now_ms)?;
    // Refreshed from the slot's own bytes: the same sign-in (LT-A4).
    file_bytes(Tool::Claude, id, None, &updated, Incoming::OwnRefresh, identity::origin::K2_SIGN_IN).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod test_access {
    pub(crate) use super::{claude_apply_refresh, claude_refresh_body};
}
