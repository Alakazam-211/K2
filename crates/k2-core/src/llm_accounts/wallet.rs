//! Wallet operations: import the live login, finish a sign-in, switch,
//! remove, keep idle logins warm. Every operation that moves token bytes
//! holds the per-tool [`store::lock_tool`].

use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;

use super::pins;
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
    store::write_slot(tool, &e.id, bytes)?;
    let m = store::parse_meta(tool, bytes);
    update_meta(conn, &e.id, &meta_update_from(&m, state::SIGNED_IN))
}

const LOCK_WAIT: Duration = Duration::from_secs(20);

/// Refreshes an IDLE wallet slot in place (never the active one). The
/// daemon implements it: Claude through the OAuth refresh grant, Codex
/// and Grok through their own CLI run in the slot.
pub trait Refresher {
    fn refresh(&self, tool: Tool, id: &str) -> Result<(), String>;
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
    let label = unique_label(conn, tool, label)?;
    let e = super::create(conn, tool, &label, by)?;
    store::write_slot(tool, &e.id, &bytes)?;
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
            store::write_slot(tool, &e.id, &bytes)?;
            store::keychain::delete(&svc, &acct).map_err(WalletError::Io)?;
        }
    }
    let cred = store::slot_cred_path(tool, &e.id);
    if !store::slot_has_cred(tool, &e.id) {
        update_meta(conn, &e.id, &MetaUpdate {
            state: Some(state::NOT_SET_UP.into()),
            detail: Some(Some("sign-in did not finish".into())),
            ..Default::default()
        })?;
        return Err(WalletError::NotSignedIn(e.id));
    }
    let _ = store::set_mode(&cred, 0o600);
    let m = store::slot_meta(tool, &e.id);
    let mut u = meta_update_from(&m, state::SIGNED_IN);
    u.detail = Some(None);
    u.refreshed_at = Some(now());
    update_meta(conn, &e.id, &u)?;
    if active_id(conn, tool)?.is_none() && store::read_live(tool)?.is_none() {
        let bytes = store::read_slot(tool, &e.id)?.ok_or_else(|| WalletError::NotSignedIn(e.id.clone()))?;
        store::write_live(tool, &bytes)?;
        set_active(conn, tool, &e.id, by)?;
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
    /// The outgoing live login was saved back to its slot.
    pub saved_outgoing: bool,
    /// The incoming slot was refreshed before the swap.
    pub refreshed: bool,
    pub already_active: bool,
}

/// Make `target` the active login for its tool: save the outgoing live
/// login back to its slot (the CLI may have refreshed it), refresh the
/// incoming slot if it is about to expire (skipped under air-gap), then
/// write it into the tool's live store. Every session on the server
/// picks it up.
pub fn switch(
    conn: &Connection,
    tool: Tool,
    target_key: &str,
    by: Option<&str>,
    refresher: Option<&dyn Refresher>,
) -> Result<SwitchOutcome, WalletError> {
    let target = lookup(conn, Some(tool), target_key)?;
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    let current = active_id(conn, tool)?;
    if current.as_deref() == Some(target.id.as_str()) {
        return Ok(SwitchOutcome {
            tool: tool.as_str().into(),
            from: current,
            to: target.id,
            saved_outgoing: false,
            refreshed: false,
            already_active: true,
        });
    }
    if !store::slot_has_cred(tool, &target.id) {
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
        return Ok(SwitchOutcome {
            tool: tool.as_str().into(),
            from: current,
            to: target.id,
            saved_outgoing: false,
            refreshed: false,
            already_active: false,
        });
    }
    if owner.as_deref() == Some(target.id.as_str()) {
        // Back from an API key to the login already in the live store.
        set_active(conn, tool, &target.id, by)?;
        return Ok(SwitchOutcome {
            tool: tool.as_str().into(),
            from: current,
            to: target.id,
            saved_outgoing: false,
            refreshed: false,
            already_active: false,
        });
    }
    let _target_slot = store::lock_slot(tool, &target.id, LOCK_WAIT)?;
    // 1. Save the outgoing live login (the CLI may have refreshed it).
    //    A live login K2 doesn't know is kept as "Previous subscription".
    let live = store::read_live(tool)?;
    let mut saved = false;
    match (&owner, &live) {
        (Some(o), Some(bytes)) => {
            let _owner_slot = store::lock_slot(tool, o, LOCK_WAIT)?;
            store::write_slot(tool, o, bytes)?;
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
    // 2. Lazy refresh of the incoming idle slot.
    let mut refreshed = false;
    let meta = store::slot_meta(tool, &target.id);
    let expiring = meta.expires_at.map(|x| x <= now() + 300).unwrap_or(false);
    if expiring && meta.has_refresh && !crate::airgap::enabled() {
        if let Some(r) = refresher {
            match r.refresh(tool, &target.id) {
                Ok(()) => {
                    refreshed = true;
                    let m = store::slot_meta(tool, &target.id);
                    update_meta(conn, &target.id, &MetaUpdate { expires_at: Some(m.expires_at), refreshed_at: Some(now()), ..Default::default() })?;
                }
                Err(e) => {
                    // The CLI refreshes an expired access token itself as
                    // long as the refresh token is good, so a failed
                    // pre-swap refresh is recorded, not fatal.
                    let detail = if super::looks_like_secret(&e) { "refresh failed".to_string() } else { e };
                    update_meta(conn, &target.id, &MetaUpdate { detail: Some(Some(detail)), ..Default::default() })?;
                }
            }
        }
    }
    // 3. Swap in.
    let bytes = store::read_slot(tool, &target.id)?.ok_or_else(|| WalletError::NotSignedIn(target.id.clone()))?;
    store::write_live(tool, &bytes)?;
    set_active(conn, tool, &target.id, by)?;
    update_meta(conn, &target.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() })?;
    Ok(SwitchOutcome {
        tool: tool.as_str().into(),
        from: current,
        to: target.id,
        saved_outgoing: saved,
        refreshed,
        already_active: false,
    })
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
    if store::slot_has_cred(tool, &e.id) || e.last_used_at.is_some() {
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
    if let Some(bytes) = &live {
        match active_id(conn, tool)? {
            Some(cur) => store::write_slot(tool, &cur, bytes)?,
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
        Some(b) if Some(fingerprint(&b)) != ticket.snapshot_fp => Ok(Some(b)),
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
    store::write_slot(tool, &ticket.new_id, new_bytes)?;
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
            let bytes = store::read_slot(tool, prev)?.ok_or_else(|| WalletError::NotSignedIn(prev.clone()))?;
            store::write_live(tool, &bytes)?;
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
            if let Some(bytes) = store::read_slot(tool, prev)? {
                store::write_live(tool, &bytes)?;
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

/// Soft remove an idle login: move its slot to
/// `~/.k2/llm-accounts/.removed/<id>-<ts>/` and mark the row removed.
/// The active login can't be removed (switch first).
pub fn remove(conn: &Connection, id: &str) -> Result<RemoveOutcome, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    let _lock = store::lock_tool(tool, LOCK_WAIT)?;
    if active_id(conn, tool)?.as_deref() == Some(e.id.as_str())
        || live_owner(conn, tool)?.as_deref() == Some(e.id.as_str())
    {
        return Err(WalletError::ActiveLogin);
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
    Ok(RemoveOutcome { id: e.id, moved_to })
}

/// Recompute one entry's state from its store (live for the active one,
/// the slot otherwise). Local reads only; no vendor call.
pub fn recheck(conn: &Connection, id: &str) -> Result<Entry, WalletError> {
    let e = lookup(conn, None, id)?;
    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
    if e.is_api_key() {
        let st = if store::has_api_key(tool, &e.id) { state::SIGNED_IN } else { state::NOT_SET_UP };
        update_meta(conn, &e.id, &MetaUpdate { state: Some(st.into()), ..Default::default() })?;
        return get(conn, &e.id)?.ok_or(WalletError::NotFound(e.id));
    }
    let is_active = live_owner(conn, tool)?.as_deref() == Some(e.id.as_str());
    let m = if is_active { store::live_meta(tool)? } else { store::slot_meta(tool, &e.id) };
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
            let meta = store::slot_meta(tool, &e.id);
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
                    let m = store::slot_meta(tool, &e.id);
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
    let bytes = store::read_slot(Tool::Claude, id).map_err(|e| e.to_string())?.ok_or("no saved sign-in")?;
    let body = claude_refresh_body(&bytes)?;
    let resp = post(CLAUDE_TOKEN_URL, &body)?;
    let now_ms = now() * 1000;
    let updated = claude_apply_refresh(&bytes, &resp, now_ms)?;
    store::write_slot(Tool::Claude, id, &updated).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod test_access {
    pub(crate) use super::{claude_apply_refresh, claude_refresh_body};
}
