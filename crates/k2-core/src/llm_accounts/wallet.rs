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

/// Keep a live login K2 doesn't know (no active entry) instead of
/// overwriting it: a new idle "Previous subscription" entry.
fn save_unknown_live(conn: &Connection, tool: Tool, bytes: &[u8], by: Option<&str>) -> Result<(), WalletError> {
    let label = unique_label(conn, tool, "Previous subscription")?;
    let e = super::create(conn, tool, &label, by)?;
    slots::write_slot(tool, &e.id, bytes)?;
    let m = store::parse_meta(tool, bytes);
    update_meta(conn, &e.id, &meta_update_from(&m, state::SIGNED_IN))
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

/// The refusal when a credential is not the account K2 expected. Never
/// names a token; says where each identity came from.
fn account_mismatch(what: &str, label: &str, expected: &(AccountIdentity, Source), actual: &(AccountIdentity, Source)) -> WalletError {
    crate::log_debug!(
        "[llm-accounts] WARN account mismatch ({what}): token {label:?} expected {} ({}), found {} ({})",
        expected.0.describe(),
        expected.1.describe(),
        actual.0.describe(),
        actual.1.describe()
    );
    let hint = if what == "capture" && actual.1 == Source::ClaudeConfig {
        // Anthropic couldn't be asked (air-gap, offline, slow): say exactly
        // what the file names and what was expected.
        format!(
            "K2 stopped the switch: it couldn't check with Anthropic which account is live, and ~/.claude.json names {} as the signed-in account, but you expected \"{label}\" ({}) to be live. Nothing changed. If ~/.claude.json is just out of date, run /login as {} once in a Claude tab, then try again.",
            actual.0.describe(),
            expected.0.describe(),
            expected.0.describe()
        )
    } else if what == "capture" {
        format!(
            "K2 stopped the switch: the sign-in live on this server belongs to {} ({}), but you expected \"{label}\" ({}) to be live. Saving it would put another account's sign-in into \"{label}\". Nothing changed. If someone signed in outside K2, add that sign-in on Settings → Language Models → Subs / Tokens, or sign this token in again.",
            actual.0.describe(),
            actual.1.describe(),
            expected.0.describe()
        )
    } else {
        format!(
            "K2 stopped the switch: the saved sign-in for \"{label}\" belongs to {} ({}), not {} ({}). The server default is unchanged. Sign \"{label}\" in again on Settings → Language Models → Subs / Tokens.",
            actual.0.describe(),
            actual.1.describe(),
            expected.0.describe(),
            expected.1.describe()
        )
    };
    WalletError::Conflict("account_mismatch", hint)
}

/// Refuse when both identities are known and name different accounts.
fn check_same(
    what: &str,
    label: &str,
    expected: &Option<(AccountIdentity, Source)>,
    actual: &Option<(AccountIdentity, Source)>,
) -> Result<(), WalletError> {
    if let (Some(e), Some(a)) = (expected, actual) {
        if e.0.same_account(&a.0) == Some(false) {
            return Err(account_mismatch(what, label, e, a));
        }
    }
    Ok(())
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
    slots::write_slot(tool, &e.id, &bytes)?;
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
    if tool == Tool::Claude && store::keychain_enabled() {
        let svc = store::claude_keychain_service(Some(&dir));
        let acct = store::claude_keychain_account();
        if let Ok(Some(bytes)) = store::keychain::read(&svc, &acct) {
            slots::write_slot(tool, &e.id, &bytes)?;
            store::keychain::delete(&svc, &acct).map_err(WalletError::Io)?;
        }
    }
    let cred = store::slot_cred_path(tool, &e.id);
    if !slots::has_cred(tool, &e.id) {
        update_meta(conn, &e.id, &MetaUpdate {
            state: Some(state::NOT_SET_UP.into()),
            detail: Some(Some("sign-in did not finish".into())),
            ..Default::default()
        })?;
        return Err(WalletError::NotSignedIn(e.id));
    }
    let _ = store::set_mode(&cred, 0o600);
    if tool == Tool::Claude {
        // A fresh sign-in may be another account than the slot held
        // before: K2's record follows the sign-in's own profile.
        let record = identity::slot_profile_path(&e.id);
        let fresh = std::fs::read(dir.join(".claude.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .map(|v| v["oauthAccount"].clone())
            .filter(|v| v.is_object());
        let saved = match fresh {
            Some(p) => match identity::save_slot_profile(&e.id, &p) {
                Ok(()) => true,
                Err(err) => {
                    crate::log_debug!("[llm-accounts] claude: profile not recorded at sign-in ({err})");
                    false
                }
            },
            None => false,
        };
        if !saved {
            let _ = std::fs::remove_file(&record);
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
        if expiring && meta.has_refresh && !crate::airgap::enabled() {
            if let Some(r) = refresher {
                pre.refresh = Some(r.refresh(tool, &target.id));
            }
        }
    }
    let Some(incoming) = slots::read_slot(tool, &target.id)? else {
        return Ok(pre);
    };
    let live = store::read_live(tool)?.filter(|b| store::validate_cred(tool, b).is_ok());
    let ask = |b: &[u8]| refresher.and_then(|r| r.identify(tool, b));
    pre.creds = Some(PrecheckCreds {
        live: live.as_deref().map(|b| (fingerprint(b), ask(b))),
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
                c.live.as_ref().filter(|(lfp, _)| *lfp == fp).and_then(|(_, id)| id.clone())
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
    //    The live bytes go into the owner's slot only when they are the
    //    owner's account: a `/login` outside K2, or a CLI writing an old
    //    token back, can leave another account live. Never cross logins.
    let owner_entry = match owner.as_deref() {
        Some(o) => get(conn, o)?,
        None => None,
    };
    let owner_expected = owner_entry.as_ref().and_then(|e| identity::expected_identity(tool, e));
    let live_actual = live.as_deref().and_then(|b| identity::actual_identity(tool, b, Some(ident), true));
    if let (Some(oe), Some(_)) = (&owner_entry, &live) {
        check_same("capture", &oe.label, &owner_expected, &live_actual)?;
    }
    let mut saved = false;
    match (&owner, &live) {
        (Some(o), Some(bytes)) => {
            let _owner_slot = store::lock_slot(tool, o, LOCK_WAIT)?;
            slots::write_slot(tool, o, bytes)?;
            let m = store::parse_meta(tool, bytes);
            update_meta(conn, o, &MetaUpdate { expires_at: Some(m.expires_at), state: Some(state::SIGNED_IN.into()), ..Default::default() })?;
            saved = true;
        }
        (None, Some(bytes)) => {
            save_unknown_live(conn, tool, bytes, by)?;
            saved = true;
        }
        _ => {}
    }
    // 2. Lazy refresh of the incoming idle slot (phase 1 already did it
    //    in the two-phase switch; here only its result is recorded).
    let refresh_result = match pre {
        Some(p) => p.refresh.clone(),
        None => {
            let meta = slots::slot_meta(tool, &target.id);
            let expiring = meta.expires_at.map(|x| x <= now() + 300).unwrap_or(false);
            match refresher {
                Some(r) if expiring && meta.has_refresh && !crate::airgap::enabled() => Some(r.refresh(tool, &target.id)),
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
    if let Some(c) = pre_creds {
        if fingerprint(&bytes) != c.incoming {
            return Err(changed(tool));
        }
    }
    let target_expected = identity::expected_identity(tool, &target);
    let incoming_actual = identity::actual_identity(tool, &bytes, Some(ident), false);
    check_same("restore", &target.label, &target_expected, &incoming_actual)?;
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
    Ok(outcome(saved, refreshed, false, profile_swapped, verified))
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
}

/// Start a live-swap login: save the current live login into its wallet
/// slot (importing it as "Default" if nothing is active yet) and remember
/// its fingerprint.
pub fn begin_live_swap(conn: &Connection, tool: Tool, new_id: &str, by: Option<&str>) -> Result<LiveSwapTicket, WalletError> {
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    let live = store::read_live(tool)?;
    // A broken live store is fingerprinted (so a change is noticed) but
    // never copied into a slot.
    if let Some(bytes) = live.as_ref().filter(|b| store::validate_cred(tool, b).is_ok()) {
        match active_id(conn, tool)? {
            Some(cur) => {
                // Same rule as a switch: never save another account's
                // sign-in into the active token's slot.
                if let Some(e) = get(conn, &cur)? {
                    let expected = identity::expected_identity(tool, &e);
                    let actual = identity::actual_identity(tool, bytes, None, true);
                    check_same("capture", &e.label, &expected, &actual)?;
                }
                slots::write_slot(tool, &cur, bytes)?
            }
            None => {
                import_live_locked(conn, tool, by, "Default")?;
            }
        }
    }
    Ok(LiveSwapTicket {
        tool,
        new_id: new_id.to_string(),
        previous_id: active_id(conn, tool)?,
        snapshot_fp: live.as_deref().map(fingerprint),
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
    slots::write_slot(tool, &ticket.new_id, new_bytes)?;
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
    let m = if is_active { store::live_meta(tool)? } else { slots::slot_meta(tool, &e.id) };
    let st = if e.state == state::SIGNING_IN {
        state::SIGNING_IN
    } else if !m.present {
        if is_active { state::NEEDS_LOGIN } else { state::NOT_SET_UP }
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
            match refresher.refresh(tool, &e.id) {
                Ok(()) => {
                    let m = slots::slot_meta(tool, &e.id);
                    let mut u = meta_update_from(&m, state::SIGNED_IN);
                    u.refreshed_at = Some(now_secs);
                    u.detail = Some(None);
                    update_meta(conn, &e.id, &u)?;
                    out.push(WarmResult { id: e.id, tool: tool.as_str().into(), refreshed: true, error: None });
                }
                Err(err) => {
                    let detail = if super::looks_like_secret(&err) { "refresh failed".to_string() } else { err.clone() };
                    let mut u = MetaUpdate { detail: Some(Some(detail.clone())), ..Default::default() };
                    if refresh_error_is_permanent(&err) {
                        u.state = Some(state::NEEDS_LOGIN.into());
                    }
                    update_meta(conn, &e.id, &u)?;
                    out.push(WarmResult { id: e.id, tool: tool.as_str().into(), refreshed: false, error: Some(detail) });
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
    slots::write_slot(Tool::Claude, id, &updated).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod test_access {
    pub(crate) use super::{claude_apply_refresh, claude_refresh_body};
}
