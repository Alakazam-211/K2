//! Which account a login belongs to, and Claude's `oauthAccount` profile.
//!
//! K2 never keeps a token to know whose it is. Sources, best first:
//!
//! - **Vendor.** The daemon asks the vendor with the credential itself
//!   (Claude: `GET /api/oauth/profile`, what Claude Code 2.1.300 calls
//!   `hYe`). Only through [`super::wallet::Refresher::identify`]; never in
//!   tests, never under air-gap.
//! - **Credential.** Codex's `id_token` names the user (`sub`, `email`).
//!   Claude's credential (`claudeAiOauth`) names no account.
//! - **K2 record.** `<slot>/k2-oauth-account.json`: the slot's Claude
//!   `oauthAccount` profile as K2 last confirmed it (no secrets: uuids,
//!   email, names, plan flags).
//! - **Claude config.** `oauthAccount` in a `.claude.json`: the slot's
//!   own (a temp-home sign-in writes it) or the live one (`~/.claude.json`,
//!   what `/status` prints).
//!
//! **Claude 2.1.300 and `~/.claude.json`** (checked against the installed
//! binary 2026-10-14): `/status` and `claude auth status` print
//! `oauthAccount.emailAddress` from the global config, not from the token.
//! Claude only replaces `oauthAccount` wholesale for the SAME
//! `accountUuid` (`QCo`), and a profile refresh merges display fields
//! only. So after a token swap the file keeps naming the old account until
//! K2 rewrites that one key ([`set_live_profile`]). The file is a pretty
//! printed JSON object Claude rewrites often; K2 splices the one key in
//! place, keeps every other byte, writes atomically and keeps a backup.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::store;
use super::{Entry, Tool, WalletError};

/// The account a credential or profile names. Never a secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    pub org_uuid: Option<String>,
}

/// Where an identity came from (said in errors and logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Asked of the vendor with the credential itself.
    Vendor,
    /// Read from the credential (Codex `id_token`).
    Credential,
    /// K2's saved copy of the slot's profile.
    K2Record,
    /// `oauthAccount` in a `.claude.json`.
    ClaudeConfig,
    /// Only the email K2 recorded for the token.
    Label,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::Vendor => "checked with the provider",
            Source::Credential => "from the sign-in itself",
            Source::K2Record => "K2's record of that token",
            Source::ClaudeConfig => "Claude's .claude.json, not checked with Anthropic",
            Source::Label => "the email K2 recorded for that token",
        }
    }
}

fn clean(v: Option<&str>) -> Option<String> {
    let s = v?.trim();
    if s.is_empty() || s.len() > 200 || super::looks_like_secret(s) {
        return None;
    }
    Some(s.to_string())
}

impl AccountIdentity {
    /// From a Claude `oauthAccount` object. `None` when it names neither
    /// an account uuid nor an email.
    pub fn from_oauth_account(v: &Value) -> Option<AccountIdentity> {
        let id = AccountIdentity {
            account_uuid: clean(v["accountUuid"].as_str()),
            email: clean(v["emailAddress"].as_str()),
            org_uuid: clean(v["organizationUuid"].as_str()),
        };
        id.is_known().then_some(id)
    }

    /// From Claude's `/api/oauth/profile` body
    /// (`{account:{uuid,email,…}, organization:{uuid,…}}`).
    pub fn from_claude_profile_response(v: &Value) -> Option<AccountIdentity> {
        let id = AccountIdentity {
            account_uuid: clean(v["account"]["uuid"].as_str()),
            email: clean(v["account"]["email"].as_str()).or_else(|| clean(v["account"]["email_address"].as_str())),
            org_uuid: clean(v["organization"]["uuid"].as_str()),
        };
        id.is_known().then_some(id)
    }

    pub fn from_email(email: Option<&str>) -> Option<AccountIdentity> {
        let id = AccountIdentity { email: clean(email), ..Default::default() };
        id.is_known().then_some(id)
    }

    pub fn is_known(&self) -> bool {
        self.account_uuid.is_some() || self.email.is_some()
    }

    /// `Some(true)` same account, `Some(false)` a different one, `None`
    /// when the two can't be compared (nothing in common to compare).
    pub fn same_account(&self, other: &AccountIdentity) -> Option<bool> {
        if let (Some(a), Some(b)) = (&self.account_uuid, &other.account_uuid) {
            return Some(a.eq_ignore_ascii_case(b));
        }
        if let (Some(a), Some(b)) = (&self.email, &other.email) {
            return Some(a.eq_ignore_ascii_case(b));
        }
        None
    }

    /// What a person reads: the email, else the start of the uuid.
    pub fn describe(&self) -> String {
        if let Some(e) = &self.email {
            return e.clone();
        }
        if let Some(u) = &self.account_uuid {
            return format!("account {}", u.chars().take(8).collect::<String>());
        }
        "an unknown account".into()
    }

    /// A minimal Claude `oauthAccount` (Claude fills the rest on its next
    /// profile fetch: it refetches when billingType etc. are missing).
    pub fn to_oauth_account(&self) -> Value {
        let mut o = serde_json::Map::new();
        if let Some(u) = &self.account_uuid {
            o.insert("accountUuid".into(), u.clone().into());
        }
        if let Some(e) = &self.email {
            o.insert("emailAddress".into(), e.clone().into());
        }
        if let Some(g) = &self.org_uuid {
            o.insert("organizationUuid".into(), g.clone().into());
        }
        Value::Object(o)
    }
}

// ── Identity from a credential ──────────────────────────────────────

fn jwt_claims(token: &str) -> Option<Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The account a credential names by itself (no network). Codex: the
/// `id_token`'s `sub` + `email`. Claude, Grok, Gemini: `None`.
pub fn cred_identity(tool: Tool, bytes: &[u8]) -> Option<AccountIdentity> {
    if tool != Tool::Codex {
        return None;
    }
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let claims = jwt_claims(v["tokens"]["id_token"].as_str()?)?;
    let id = AccountIdentity {
        account_uuid: clean(claims["sub"].as_str()),
        email: clean(claims["email"].as_str()),
        org_uuid: None,
    };
    id.is_known().then_some(id)
}

// ── Claude profile files ────────────────────────────────────────────

/// K2's copy of a slot's Claude profile.
pub const SLOT_PROFILE_FILE: &str = "k2-oauth-account.json";

pub fn slot_profile_path(id: &str) -> PathBuf {
    store::slot_dir(Tool::Claude, id).join(SLOT_PROFILE_FILE)
}

fn oauth_account_of(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    match &v["oauthAccount"] {
        o @ Value::Object(_) => Some(o.clone()),
        _ => None,
    }
}

/// Does a profile look like credential material? (It never should.)
fn profile_has_secret(p: &Value) -> bool {
    let s = p.to_string();
    s.contains("accessToken") || s.contains("refreshToken") || s.contains("Authorization") || s.contains("sk-ant-")
}

/// A slot's Claude profile: K2's record, else the slot's own
/// `.claude.json` (a temp-home sign-in writes it). With its source.
pub fn slot_profile(id: &str) -> Option<(Value, Source)> {
    if let Ok(bytes) = std::fs::read(slot_profile_path(id)) {
        if let Ok(v @ Value::Object(_)) = serde_json::from_slice::<Value>(&bytes) {
            if !profile_has_secret(&v) {
                return Some((v, Source::K2Record));
            }
        }
    }
    oauth_account_of(&store::slot_dir(Tool::Claude, id).join(".claude.json"))
        .filter(|v| !profile_has_secret(v))
        .map(|v| (v, Source::ClaudeConfig))
}

/// Save a slot's Claude profile (0600). Refuses anything that isn't an
/// object naming an account, or that carries credential-shaped text.
pub(crate) fn save_slot_profile(id: &str, profile: &Value) -> Result<(), WalletError> {
    if !profile.is_object() || AccountIdentity::from_oauth_account(profile).is_none() {
        return Err(WalletError::Io("not a Claude account profile".into()));
    }
    if profile_has_secret(profile) {
        return Err(WalletError::Io("refusing to save a profile that contains credentials".into()));
    }
    let dir = store::slot_dir(Tool::Claude, id);
    store::ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let bytes = serde_json::to_vec_pretty(profile).map_err(|e| WalletError::Io(e.to_string()))?;
    store::write_private(&slot_profile_path(id), &bytes).map_err(|e| WalletError::Io(e.to_string()))
}

/// The live Claude profile (`oauthAccount` in `~/.claude.json`, or in
/// `$CLAUDE_CONFIG_DIR/.claude.json`): what `/status` prints.
pub fn live_profile() -> Option<Value> {
    oauth_account_of(&store::claude_live_global_config())
}

/// Where K2 keeps the copy of `.claude.json` it took before its last
/// rewrite of `oauthAccount`.
pub fn live_config_backup_path() -> PathBuf {
    store::tool_dir(Tool::Claude).join("claude.json.before-switch")
}

/// Make `profile` the live `oauthAccount`, touching nothing else in the
/// file. `Ok(false)` when there is no live `.claude.json` (Claude makes
/// one on first run; there is nothing that could lie). Keeps a backup of
/// the file as it was, writes atomically (same mode), and retries when
/// Claude rewrote the file in between.
pub(crate) fn set_live_profile(profile: &Value) -> Result<bool, WalletError> {
    if !profile.is_object() || profile_has_secret(profile) {
        return Err(WalletError::Io("not a Claude account profile".into()));
    }
    let link = store::claude_live_global_config();
    // A symlinked dotfile: write the file it points at, keep the link.
    let path = match std::fs::symlink_metadata(&link) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(&link).map_err(|e| WalletError::Io(e.to_string()))?,
        Ok(_) => link,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(WalletError::Io(e.to_string())),
    };
    for _ in 0..3 {
        let before = std::fs::read(&path).map_err(|e| WalletError::Io(format!("{}: {e}", path.display())))?;
        let text = std::str::from_utf8(&before).map_err(|_| WalletError::Io(".claude.json is not UTF-8".into()))?;
        let new_text = splice_top_level_key(text, "oauthAccount", profile).map_err(WalletError::Io)?;
        if new_text.as_bytes() == before.as_slice() {
            return Ok(true);
        }
        let backup = live_config_backup_path();
        store::ensure_private_dir(backup.parent().expect("tool dir")).map_err(|e| WalletError::Io(e.to_string()))?;
        store::write_private(&backup, &before).map_err(|e| WalletError::Io(e.to_string()))?;
        let mode = file_mode(&path);
        // Claude rewrote it while K2 worked: start over from its bytes.
        if std::fs::read(&path).map_err(|e| WalletError::Io(e.to_string()))? != before {
            continue;
        }
        write_with_mode(&path, new_text.as_bytes(), mode).map_err(|e| WalletError::Io(e.to_string()))?;
        return Ok(true);
    }
    Err(WalletError::LockBusy("Claude (.claude.json kept changing)".into()))
}

#[cfg(unix)]
fn file_mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600)
}
#[cfg(not(unix))]
fn file_mode(_p: &Path) -> u32 {
    0o600
}

/// Atomic replace (tmp next to the file, fsync, rename), keeping `mode`.
fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.k2tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("claude.json"),
        std::process::id()
    ));
    let res = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        store::set_mode(&tmp, mode)?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

// ── The splice ──────────────────────────────────────────────────────

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// End (exclusive) of the JSON string starting at `i` (`b[i] == '"'`).
fn string_end(b: &[u8], i: usize) -> Result<usize, String> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    Err("unterminated string".into())
}

/// End (exclusive) of the JSON value starting at `i`.
fn value_end(b: &[u8], i: usize) -> Result<usize, String> {
    match b.get(i) {
        Some(b'"') => string_end(b, i),
        Some(b'{') | Some(b'[') => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            Err("unterminated object".into())
        }
        Some(_) => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            Ok(j)
        }
        None => Err("value expected".into()),
    }
}

/// `value` as JSON at one level of two-space indent (Claude's layout).
fn render_value(value: &Value) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into());
    pretty.replace('\n', "\n  ")
}

/// Replace (or add) one top-level key of a JSON object document, keeping
/// every other byte. The result is checked: it parses, and every other
/// top-level key is unchanged; anything else is refused.
pub fn splice_top_level_key(text: &str, key: &str, value: &Value) -> Result<String, String> {
    let original: Value = serde_json::from_str(text).map_err(|_| ".claude.json is not valid JSON; K2 left it alone".to_string())?;
    let Value::Object(orig_map) = &original else {
        return Err(".claude.json is not a JSON object; K2 left it alone".into());
    };
    let b = text.as_bytes();
    let open = skip_ws(b, 0);
    if b.get(open) != Some(&b'{') {
        return Err(".claude.json is not a JSON object".into());
    }
    let mut i = skip_ws(b, open + 1);
    let mut found: Option<(usize, usize)> = None;
    let mut close = None;
    let mut seen_any = false;
    while i < b.len() {
        if b[i] == b'}' {
            close = Some(i);
            break;
        }
        if b[i] != b'"' {
            return Err("unexpected text in .claude.json".into());
        }
        let k_end = string_end(b, i)?;
        let k: String = serde_json::from_str(&text[i..k_end]).map_err(|_| "bad key".to_string())?;
        let colon = skip_ws(b, k_end);
        if b.get(colon) != Some(&b':') {
            return Err("missing ':'".into());
        }
        let v_start = skip_ws(b, colon + 1);
        let v_end = value_end(b, v_start)?;
        if k == key {
            if found.is_some() {
                return Err(format!("{key} appears twice in .claude.json; K2 left it alone"));
            }
            found = Some((v_start, v_end));
        }
        seen_any = true;
        i = skip_ws(b, v_end);
        if b.get(i) == Some(&b',') {
            i = skip_ws(b, i + 1);
        }
    }
    let close = close.ok_or_else(|| "unterminated object".to_string())?;
    let rendered = render_value(value);
    let out = match found {
        Some((s, e)) => format!("{}{}{}", &text[..s], rendered, &text[e..]),
        None => {
            // Add it last, after the final value.
            let mut end = close;
            while end > open + 1 && matches!(b[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
                end -= 1;
            }
            let key_json = serde_json::to_string(key).map_err(|e| e.to_string())?;
            let sep = if seen_any { "," } else { "" };
            format!("{}{sep}\n  {key_json}: {rendered}\n{}", &text[..end], &text[close..])
        }
    };
    let parsed: Value = serde_json::from_str(&out).map_err(|_| "the edit did not produce valid JSON; K2 left the file alone".to_string())?;
    let Value::Object(new_map) = &parsed else {
        return Err("the edit did not produce an object".into());
    };
    if &new_map[key] != value {
        return Err("the edit did not set the profile; K2 left the file alone".into());
    }
    let others_same = orig_map.len() + usize::from(found.is_none()) == new_map.len()
        && orig_map.iter().filter(|(k, _)| k.as_str() != key).all(|(k, v)| new_map.get(k) == Some(v));
    if !others_same {
        return Err("the edit would have changed other settings; K2 left the file alone".into());
    }
    Ok(out)
}

// ── Expected and actual identity ────────────────────────────────────

/// Who K2 believes a token belongs to (no network).
pub fn expected_identity(tool: Tool, e: &Entry) -> Option<(AccountIdentity, Source)> {
    match tool {
        Tool::Claude => {
            if let Some((p, src)) = slot_profile(&e.id) {
                if let Some(id) = AccountIdentity::from_oauth_account(&p) {
                    return Some((id, src));
                }
            }
        }
        Tool::Codex => {
            if let Ok(Some(bytes)) = store::read_slot(tool, &e.id) {
                if let Some(id) = cred_identity(tool, &bytes) {
                    return Some((id, Source::Credential));
                }
            }
        }
        _ => return None,
    }
    AccountIdentity::from_email(e.email.as_deref()).map(|id| (id, Source::Label))
}

/// Who a credential really belongs to: the credential itself (Codex),
/// else the vendor (`identify`), else — for the live store only —
/// Claude's live `.claude.json`.
pub fn actual_identity(
    tool: Tool,
    bytes: &[u8],
    identify: Option<&dyn Fn(Tool, &[u8]) -> Option<AccountIdentity>>,
    live: bool,
) -> Option<(AccountIdentity, Source)> {
    if let Some(id) = cred_identity(tool, bytes) {
        return Some((id, Source::Credential));
    }
    if let Some(f) = identify {
        if let Some(id) = f(tool, bytes) {
            return Some((id, Source::Vendor));
        }
    }
    if live && tool == Tool::Claude {
        if let Some(id) = live_profile().as_ref().and_then(AccountIdentity::from_oauth_account) {
            return Some((id, Source::ClaudeConfig));
        }
    }
    None
}
