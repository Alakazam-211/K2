//! App user invites and set-password links (prd-agent-app-users-v1 §5.4,
//! S4/S5; Rosson 2026-10-24).
//!
//! One table, `app_user_invites` (skin.db step 6), holds every one-time
//! set-password link K2 mails or hands the owner:
//!
//! - `invite`: bring a person in to set their first password;
//! - `reset`: an App's own forgot-password flow (`password.forgot` through
//!   a platform pass), mailed to the person, never returned to the caller;
//! - `owner_reset`: the owner's reset (`k2 app user reset`): the only
//!   token whose use drops an agent tie ([`super::ties`]).
//!
//! The token is `k2inv_` + 32 random bytes (hex); only its SHA-256 is
//! stored. A new link for the same (person, App) voids the older unused
//! ones. A link's clock starts when its mail leaves ([`activate`]): while
//! an approval holds the mail, `expires_at` is NULL and the token never
//! works. Use ([`consume`]) is one compare-and-set inside one IMMEDIATE
//! transaction with the password write and the session revoke.
//!
//! How long a link lives is a per-App setting ([`InviteSettings`]):
//! [`DEFAULT_INVITE_TTL_SECS`] (72 h) and [`DEFAULT_RESET_TTL_SECS`]
//! (24 h) by default, set by the owner or the App's own agent between
//! [`MIN_TTL_SECS`] and [`MAX_TTL_SECS`]. The App may also give its own
//! subject and body ([`InviteTemplate`]); K2 always adds the link itself.

use rusqlite::{params, Connection, OptionalExtension};

use super::{hash_password, immediate_tx, normalize_app_target, now_secs, revoke_sessions_for_principal, sha256_hex, with_conn, ties};

/// Token prefix: never a pass (`k2skn_`), a key (`k2sk_`) or a refresh token.
pub const INVITE_PREFIX: &str = "k2inv_";

pub const PURPOSE_INVITE: &str = "invite";
pub const PURPOSE_RESET: &str = "reset";
pub const PURPOSE_OWNER_RESET: &str = "owner_reset";

/// Default life of an invite link (Rosson 2026-10-24).
pub const DEFAULT_INVITE_TTL_SECS: i64 = 72 * 3600;
/// Default life of a reset link (the App's forgot flow and owner resets).
pub const DEFAULT_RESET_TTL_SECS: i64 = 24 * 3600;
/// The shortest link an App or the owner may set.
pub const MIN_TTL_SECS: i64 = 15 * 60;
/// The longest.
pub const MAX_TTL_SECS: i64 = 30 * 86_400;
/// One invite per person per this long.
pub const INVITE_THROTTLE_SECS: i64 = 60;

/// Longest App subject line.
pub const MAX_SUBJECT_CHARS: usize = 200;
/// Longest App body.
pub const MAX_BODY_CHARS: usize = 4000;

// ── settings ─────────────────────────────────────────────────────────

/// skin.db columns on `app_login_settings` (additive at open: an older K2
/// just has no invites).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    for col in [
        "invite_ttl_secs INTEGER",
        "reset_ttl_secs INTEGER",
        "set_password_url TEXT",
        "invite_subject TEXT",
        "invite_body TEXT",
        "reset_subject TEXT",
        "reset_body TEXT",
    ] {
        let name = col.split_whitespace().next().unwrap_or_default();
        if !super::column_exists(conn, "app_login_settings", name)? {
            conn.execute(&format!("ALTER TABLE app_login_settings ADD COLUMN {col}"), [])
                .map_err(|e| format!("skin.db app_login_settings.{name}: {e}"))?;
        }
    }
    Ok(())
}

/// An App's own subject and body for one kind of mail (`None` = K2's).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteTemplate {
    pub subject: Option<String>,
    pub body: Option<String>,
}

/// One App's invite and reset settings (`host` = the box login).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteSettings {
    pub app: String,
    /// Effective invite link life, seconds.
    pub invite_ttl_secs: i64,
    /// Effective reset link life, seconds.
    pub reset_ttl_secs: i64,
    /// `true` when the App set its own value (else the default above).
    pub invite_ttl_set: bool,
    pub reset_ttl_set: bool,
    /// The App's own set-password page (`#k2inv_…` goes on its end).
    pub set_password_url: Option<String>,
    pub invite: InviteTemplate,
    pub reset: InviteTemplate,
}

/// A change to [`InviteSettings`]: `None` keeps a field; `Some(None)`
/// clears it back to K2's default.
#[derive(Debug, Clone, Default)]
pub struct InviteSettingsPatch {
    pub invite_ttl_secs: Option<Option<i64>>,
    pub reset_ttl_secs: Option<Option<i64>>,
    pub set_password_url: Option<Option<String>>,
    pub invite_subject: Option<Option<String>>,
    pub invite_body: Option<Option<String>>,
    pub reset_subject: Option<Option<String>>,
    pub reset_body: Option<Option<String>>,
}

fn blank(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn settings_conn(conn: &Connection, app: &str) -> Result<InviteSettings, String> {
    type Row = (Option<i64>, Option<i64>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
    let row: Option<Row> = conn
        .query_row(
            "SELECT invite_ttl_secs, reset_ttl_secs, set_password_url, invite_subject, invite_body, reset_subject, reset_body
               FROM app_login_settings WHERE app_id = ?1",
            params![app],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()
        .map_err(|e| format!("invite settings: {e}"))?;
    let (it, rt, url, isub, ibody, rsub, rbody) = row.unwrap_or_default();
    let ok = |v: Option<i64>| v.filter(|s| (MIN_TTL_SECS..=MAX_TTL_SECS).contains(s));
    Ok(InviteSettings {
        app: app.to_string(),
        invite_ttl_secs: ok(it).unwrap_or(DEFAULT_INVITE_TTL_SECS),
        reset_ttl_secs: ok(rt).unwrap_or(DEFAULT_RESET_TTL_SECS),
        invite_ttl_set: ok(it).is_some(),
        reset_ttl_set: ok(rt).is_some(),
        set_password_url: blank(url),
        invite: InviteTemplate { subject: blank(isub), body: blank(ibody) },
        reset: InviteTemplate { subject: blank(rsub), body: blank(rbody) },
    })
}

/// An App's invite settings (`host` or a published service id).
pub fn settings(app: &str) -> Result<InviteSettings, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| settings_conn(conn, &app))
}

/// Parse a link life: `15m`, `72h`, `30d`, `3600s` or bare seconds, kept
/// within [`MIN_TTL_SECS`]..=[`MAX_TTL_SECS`].
pub fn parse_ttl(raw: &str) -> Result<i64, String> {
    let t = raw.trim().to_ascii_lowercase();
    let (num, mult) = match t.chars().last() {
        Some('s') => (&t[..t.len() - 1], 1),
        Some('m') => (&t[..t.len() - 1], 60),
        Some('h') => (&t[..t.len() - 1], 3600),
        Some('d') => (&t[..t.len() - 1], 86_400),
        Some(c) if c.is_ascii_digit() => (t.as_str(), 1),
        _ => return Err(format!("a link life is like 15m, 72h or 30d (got {raw:?})")),
    };
    let n: i64 = num.trim().parse().map_err(|_| format!("a link life is like 15m, 72h or 30d (got {raw:?})"))?;
    let secs = n.checked_mul(mult).ok_or_else(|| "that link life is too long".to_string())?;
    if !(MIN_TTL_SECS..=MAX_TTL_SECS).contains(&secs) {
        return Err(format!("a link lives between 15 minutes and 30 days (got {raw:?})"));
    }
    Ok(secs)
}

/// Check an App's subject line (one line, at most [`MAX_SUBJECT_CHARS`]).
pub fn check_subject(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.chars().any(char::is_control) {
        return Err("a subject is one line of text".into());
    }
    if s.chars().count() > MAX_SUBJECT_CHARS {
        return Err(format!("a subject is at most {MAX_SUBJECT_CHARS} characters"));
    }
    check_no_link(s)?;
    Ok(s.to_string())
}

/// Check an App's body (at most [`MAX_BODY_CHARS`], text and newlines).
pub fn check_body(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r') {
        return Err("a body is plain text (newlines and tabs only)".into());
    }
    if s.chars().count() > MAX_BODY_CHARS {
        return Err(format!("a body is at most {MAX_BODY_CHARS} characters"));
    }
    check_no_link(s)?;
    Ok(s.replace("\r\n", "\n"))
}

/// The set-password link is K2's to place, never the template's, and a
/// template carries no other link either (security review 2026-10-25: a
/// template link is a phishing surface).
/// Does `s` read as a link a mail client would make clickable
/// (`http(s)://`, `www.`, a K2 token, a `{link}` slot, or a bare
/// `name.tld` such as `evil.example`)? Template values (`{app}`, `{name}`,
/// `{username}`, `{sender}`) that do are replaced before rendering
/// (verification review 2026-10-26, item 7).
pub fn link_like(s: &str) -> bool {
    if has_scheme_link(s) {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    lower.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | ';')).any(|word| {
        let word = word.trim_matches(|c: char| matches!(c, '.' | '!' | '?' | ':'));
        let labels: Vec<&str> = word.split('.').collect();
        labels.len() >= 2
            && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            && labels.last().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
    })
}

/// `http(s)://` (any `://`), `www.`, a K2 token or a `{link}` slot.
fn has_scheme_link(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    ["www.", "{link}", INVITE_PREFIX, "://"].iter().any(|n| lower.contains(n))
}

fn check_no_link(s: &str) -> Result<(), String> {
    let lower = s.to_ascii_lowercase();
    if lower.contains("{link}") || lower.contains(INVITE_PREFIX) {
        return Err("K2 adds the set-password link itself; a template can't place it ({link} and k2inv_ are not allowed)".into());
    }
    if lower.contains("http://") || lower.contains("https://") || lower.contains("www.") {
        return Err("a template carries no links (http://, https:// or www.): K2 adds the one set-password link itself".into());
    }
    Ok(())
}

/// `(scheme, host, port)` of an absolute `http(s)://` URL; `None` for
/// anything else (userinfo, an empty host, a bad port).
pub fn url_host(raw: &str) -> Option<(String, String, Option<u16>)> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
        None => (authority, None),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
        return None;
    }
    Some((scheme, host, port))
}

/// Check a set-password page (security review 2026-10-25): `https://` on
/// one of `allowed_hosts` (the App's own domains or K2's), or exactly
/// `http://localhost` / `http://127.0.0.1` (any port) for a local App; no
/// userinfo, no #fragment (K2 puts the token there).
pub fn check_set_password_url(raw: &str, allowed_hosts: &[String]) -> Result<String, String> {
    let u = raw.trim();
    let bad = || "set-password page: https:// on the App's own domain or this box's K2 domain, no #fragment (K2 adds #k2inv_… itself)".to_string();
    if u.contains('#') || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad());
    }
    let (scheme, host, _) = url_host(u).ok_or_else(bad)?;
    let local = scheme == "http" && (host == "localhost" || host == "127.0.0.1");
    let allowed = scheme == "https" && allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host));
    if !(local || allowed) {
        return Err(format!("{}; allowed hosts: {}", bad(), if allowed_hosts.is_empty() { "none yet".to_string() } else { allowed_hosts.join(", ") }));
    }
    Ok(u.to_string())
}

/// Apply `patch` to `app`'s settings (checked first; nothing is written
/// when any field is bad). Returns the new settings.
pub fn set_settings(app: &str, patch: &InviteSettingsPatch, allowed_hosts: &[String], actor: &str) -> Result<InviteSettings, String> {
    let app = normalize_app_target(Some(app));
    let ttl_ok = |v: &Option<Option<i64>>| match v {
        Some(Some(s)) if !(MIN_TTL_SECS..=MAX_TTL_SECS).contains(s) => Err("a link lives between 15 minutes and 30 days".to_string()),
        _ => Ok(()),
    };
    ttl_ok(&patch.invite_ttl_secs)?;
    ttl_ok(&patch.reset_ttl_secs)?;
    let text = |v: &Option<Option<String>>, check: fn(&str) -> Result<String, String>| -> Result<Option<Option<String>>, String> {
        match v {
            None => Ok(None),
            Some(None) => Ok(Some(None)),
            Some(Some(s)) if s.trim().is_empty() => Ok(Some(None)),
            Some(Some(s)) => check(s).map(|c| Some(Some(c))),
        }
    };
    let url = match &patch.set_password_url {
        None => None,
        Some(None) => Some(None),
        Some(Some(s)) if s.trim().is_empty() => Some(None),
        Some(Some(s)) => Some(Some(check_set_password_url(s, allowed_hosts)?)),
    };
    let isub = text(&patch.invite_subject, check_subject)?;
    let ibody = text(&patch.invite_body, check_body)?;
    let rsub = text(&patch.reset_subject, check_subject)?;
    let rbody = text(&patch.reset_body, check_body)?;
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        tx.execute(
            "INSERT INTO app_login_settings (app_id, updated_at, updated_by) VALUES (?1, ?2, ?3)
             ON CONFLICT(app_id) DO UPDATE SET updated_at = ?2, updated_by = ?3",
            params![app, now_secs(), actor],
        )
        .map_err(|e| format!("invite settings write: {e}"))?;
        let set = |col: &str, v: Option<rusqlite::types::Value>| -> Result<(), String> {
            tx.execute(&format!("UPDATE app_login_settings SET {col} = ?2 WHERE app_id = ?1"), params![app, v])
                .map(|_| ())
                .map_err(|e| format!("invite settings {col}: {e}"))
        };
        use rusqlite::types::Value as V;
        if let Some(v) = patch.invite_ttl_secs {
            set("invite_ttl_secs", v.map(V::Integer))?;
        }
        if let Some(v) = patch.reset_ttl_secs {
            set("reset_ttl_secs", v.map(V::Integer))?;
        }
        for (col, v) in [("set_password_url", url), ("invite_subject", isub), ("invite_body", ibody), ("reset_subject", rsub), ("reset_body", rbody)] {
            if let Some(v) = v {
                set(col, v.map(V::Text))?;
            }
        }
        let out = settings_conn(&tx, &app)?;
        tx.commit().map_err(|e| format!("invite settings commit: {e}"))?;
        Ok(out)
    })
}

// ── composing ────────────────────────────────────────────────────────

/// What fills a template (all K2's, never the caller's).
#[derive(Debug, Clone)]
pub struct MailVars<'a> {
    pub app: &'a str,
    pub name: &'a str,
    pub username: &'a str,
    pub sender: &'a str,
    pub expires: &'a str,
}

/// One single pass over `tpl`: `{app}` `{name}` `{username}` `{sender}`
/// `{expires}` become their values; values are never scanned again, and
/// anything else stays as written.
pub fn render(tpl: &str, v: &MailVars<'_>) -> String {
    let mut out = String::with_capacity(tpl.len() + 64);
    let mut rest = tpl;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let hit = [("{app}", v.app), ("{name}", v.name), ("{username}", v.username), ("{sender}", v.sender), ("{expires}", v.expires)]
            .into_iter()
            .find(|(k, _)| after.starts_with(k));
        match hit {
            Some((k, val)) => {
                out.push_str(val);
                rest = &after[k.len()..];
            }
            None => {
                out.push('{');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The composed mail: the App's subject and body (or K2's), then K2's own
/// link block, which the template never touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    pub subject: String,
    pub body: String,
}

/// Compose an invite (`purpose` = invite) or a reset (reset / owner_reset).
pub fn compose(purpose: &str, tpl: &InviteTemplate, v: &MailVars<'_>, link: &str) -> Composed {
    // A value an agent chose (an App's name, a person's name or username,
    // an agent's name) never carries a link into the mail.
    fn safe<'a>(value: &'a str, instead: &'static str) -> &'a str {
        if link_like(value) {
            instead
        } else {
            value
        }
    }
    let v = &MailVars {
        app: safe(v.app, "your App"),
        name: safe(v.name, "there"),
        // The username is how they sign in (often `first.last`): only a
        // scheme, `www.` or a token replaces it.
        username: if has_scheme_link(v.username) { "your account" } else { v.username },
        sender: safe(v.sender, "K2"),
        expires: v.expires,
    };
    let invite = purpose == PURPOSE_INVITE;
    let (default_subject, default_body) = if invite {
        ("You're invited to {app}", "Hello {name},\n\n{sender} invited you to {app}. Set your password to sign in.")
    } else {
        ("Reset your password for {app}", "Hello {name},\n\n{sender} sent you a link to set a new password for {app}.")
    };
    let subject = render(tpl.subject.as_deref().unwrap_or(default_subject), v);
    let lead = render(tpl.body.as_deref().unwrap_or(default_body), v);
    let body = format!(
        "{lead}\n\nSet your password: {link}\nThis link works once and expires {expires}.\n\nIf you weren't expecting this, ignore it. Nobody at {app} will ask for your password.\n",
        expires = v.expires,
        app = v.app,
    );
    Composed { subject: subject.replace(['\r', '\n'], " "), body }
}

/// The link: `<page>#k2inv_…` (the fragment never reaches a server log or
/// a Referer).
pub fn link_for(page: &str, token: &str) -> String {
    format!("{}#{token}", page.trim_end_matches('#'))
}

// ── tokens ───────────────────────────────────────────────────────────

/// One minted link (`token` is the secret, given out once).
#[derive(Debug, Clone)]
pub struct Minted {
    pub id: String,
    pub token: String,
    pub ttl_secs: i64,
}

/// Why a mint didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    /// One invite per person per [`INVITE_THROTTLE_SECS`].
    Throttled,
    /// The link would tie the person to a workspace other than the one
    /// they are tied to (verification review 2026-10-26, item 5): never
    /// minted; only an owner reset lifts the first tie.
    TiedElsewhere,
    Store(String),
}

fn generate_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    format!("{INVITE_PREFIX}{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// Mint a link for `principal_id` on `app`, voiding the older unused ones
/// for that (person, App). Not usable until [`activate`]. `sent_to`: the
/// address it is mailed to (`None` = handed to the owner); `set_email`:
/// on use, the person's stored email becomes `sent_to` (owner `--to`).
/// `tie_on_use`: `(workspace, reason)` the person is tied to when the link
/// is used, because an App page, an App key or a workspace mailbox could
/// see this token (security review 2026-10-25). `None` only for links that
/// leave K2's own hands for the person's alone (an owner reset).
#[allow(clippy::too_many_arguments)]
pub fn mint(
    principal_id: &str,
    app: &str,
    purpose: &str,
    ttl_secs: i64,
    sent_to: Option<&str>,
    set_email: bool,
    tie_on_use: Option<(&str, &str)>,
    created_by: &str,
) -> Result<Minted, MintError> {
    if ![PURPOSE_INVITE, PURPOSE_RESET, PURPOSE_OWNER_RESET].contains(&purpose) {
        return Err(MintError::Store(format!("unknown purpose {purpose}")));
    }
    let token = generate_token();
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_secs();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        if let Some((ws, _)) = tie_on_use {
            if ties::get_conn(&tx, principal_id)?.is_some_and(|t| t.workspace_id != ws) {
                return Ok(Err(MintError::TiedElsewhere));
            }
        }
        if purpose == PURPOSE_INVITE {
            let last: Option<i64> = tx
                .query_row(
                    "SELECT MAX(created_at) FROM app_user_invites WHERE principal_id = ?1 AND purpose = 'invite'",
                    params![principal_id],
                    |r| r.get(0),
                )
                .map_err(|e| format!("invite throttle: {e}"))?;
            if last.is_some_and(|t| now.saturating_sub(t) < INVITE_THROTTLE_SECS) {
                return Ok(Err(MintError::Throttled));
            }
        }
        // A new link voids the older unused ones for this (person, App),
        // but an invite or an App's forgot (reachable from outside) never
        // voids the owner's pending reset.
        tx.execute(
            "DELETE FROM app_user_invites WHERE principal_id = ?1 AND app = ?2 AND used_at IS NULL
               AND (purpose != 'owner_reset' OR ?3 = 'owner_reset')",
            params![principal_id, app, purpose],
        )
        .map_err(|e| format!("invite void: {e}"))?;
        let (tie_ws, tie_reason) = match tie_on_use {
            Some((ws, reason)) => (Some(ws), Some(reason)),
            None => (None, None),
        };
        tx.execute(
            "INSERT INTO app_user_invites (id, principal_id, app, purpose, token_hash, sent_to, set_email, ttl_secs, created_by, created_at, tie_workspace, tie_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![id, principal_id, app, purpose, sha256_hex(&token), sent_to, set_email as i64, ttl_secs, created_by, now, tie_ws, tie_reason],
        )
        .map_err(|e| format!("invite insert: {e}"))?;
        tx.commit().map_err(|e| format!("invite commit: {e}"))?;
        Ok(Ok(Minted { id: id.clone(), token: token.clone(), ttl_secs }))
    })
    .map_err(MintError::Store)?
}

/// Start a link's clock (its mail left, or the owner was handed it).
/// Returns its expiry; `None` when it is gone or already started.
pub fn activate(id: &str) -> Result<Option<i64>, String> {
    let now = now_secs();
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE app_user_invites SET expires_at = ?2 + ttl_secs WHERE id = ?1 AND expires_at IS NULL AND used_at IS NULL",
                params![id, now],
            )
            .map_err(|e| format!("invite activate: {e}"))?;
        if n == 0 {
            return Ok(None);
        }
        conn.query_row("SELECT expires_at FROM app_user_invites WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("invite activate: {e}"))
    })
}

/// The mail holding link `id` is waiting for approval as `outbound_id`.
pub fn hold_for(id: &str, outbound_id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("UPDATE app_user_invites SET outbound_id = ?2 WHERE id = ?1", params![id, outbound_id])
            .map(|_| ())
            .map_err(|e| format!("invite hold: {e}"))
    })
}

/// The held mail `outbound_id` left (the owner approved it): start the
/// clock of the link it carries. `None` when it carried none.
pub fn activate_for_outbound(outbound_id: &str) -> Result<Option<i64>, String> {
    let id: Option<String> = with_conn(|conn| {
        conn.query_row("SELECT id FROM app_user_invites WHERE outbound_id = ?1", params![outbound_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("invite lookup: {e}"))
    })?;
    match id {
        Some(id) => activate(&id),
        None => Ok(None),
    }
}

/// Drop a link whose mail never left (a refusal or a failed send).
pub fn remove(id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM app_user_invites WHERE id = ?1", params![id])
            .map(|_| ())
            .map_err(|e| format!("invite remove: {e}"))
    })
}

/// One link row (tests and the owner's view; never the token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRow {
    pub id: String,
    pub principal_id: String,
    pub app: String,
    pub purpose: String,
    pub sent_to: Option<String>,
    pub set_email: bool,
    pub ttl_secs: i64,
    pub outbound_id: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub used_at: Option<i64>,
}

/// Every link of `principal_id`, newest first.
pub fn rows_for(principal_id: &str) -> Result<Vec<InviteRow>, String> {
    with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT id, principal_id, app, purpose, sent_to, set_email, ttl_secs, outbound_id, created_at, expires_at, used_at
                   FROM app_user_invites WHERE principal_id = ?1 ORDER BY created_at DESC, rowid DESC",
            )
            .map_err(|e| format!("invite rows: {e}"))?;
        let rows = st
            .query_map(params![principal_id], |r| {
                Ok(InviteRow {
                    id: r.get(0)?,
                    principal_id: r.get(1)?,
                    app: r.get(2)?,
                    purpose: r.get(3)?,
                    sent_to: r.get(4)?,
                    set_email: r.get::<_, i64>(5)? != 0,
                    ttl_secs: r.get(6)?,
                    outbound_id: r.get(7)?,
                    created_at: r.get(8)?,
                    expires_at: r.get(9)?,
                    used_at: r.get(10)?,
                })
            })
            .map_err(|e| format!("invite rows: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("invite rows: {e}"))
    })
}

/// What a use did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consume {
    Ok,
    /// Unknown, voided, used, held (not yet mailed) or expired: one answer.
    BadToken,
}

/// Use a `k2inv_` link: set the password and end every session of the
/// person, in one IMMEDIATE transaction behind a compare-and-set on
/// `used_at` (two uses at once: one wins). An `owner_reset` also drops
/// the person's agent tie and, when the owner sent it to another address,
/// stores that address as their email. Nothing else drops a tie.
pub fn consume(token: &str, password: &str) -> Result<Consume, String> {
    let token = token.trim();
    if password.trim().is_empty() {
        return Err("password must not be empty".to_string());
    }
    // argon2 before the skin mutex (it also stands in for the dummy verify).
    let pw_hash = hash_password(password.trim())?;
    if !token.starts_with(INVITE_PREFIX) {
        return Ok(Consume::BadToken);
    }
    let token_hash = sha256_hex(token);
    let now = now_secs();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        type Row = (String, String, String, Option<String>, i64, Option<String>, Option<String>);
        let row: Option<Row> = tx
            .query_row(
                "SELECT i.id, i.principal_id, i.purpose, i.sent_to, i.set_email, i.tie_workspace, i.tie_reason FROM app_user_invites i
                   JOIN principals p ON p.id = i.principal_id WHERE i.token_hash = ?1",
                params![token_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .optional()
            .map_err(|e| format!("invite lookup: {e}"))?;
        let Some((id, principal_id, purpose, sent_to, set_email, tie_ws, tie_reason)) = row else { return Ok(Consume::BadToken) };
        let claimed = tx
            .execute(
                "UPDATE app_user_invites SET used_at = ?2
                 WHERE id = ?1 AND used_at IS NULL AND expires_at IS NOT NULL AND expires_at > ?2",
                params![id, now],
            )
            .map_err(|e| format!("invite use: {e}"))?;
        if claimed != 1 {
            return Ok(Consume::BadToken);
        }
        tx.execute("UPDATE principals SET password_hash = ?1 WHERE id = ?2", params![pw_hash, principal_id])
            .map_err(|e| format!("invite password: {e}"))?;
        revoke_sessions_for_principal(&tx, &principal_id)?;
        // A link an App page, App key or workspace mailbox could see ties
        // the person to that workspace as it is used (never dropped by it).
        if let Some(ws) = tie_ws.as_deref().filter(|w| !w.is_empty()) {
            let set_by = format!("link:{purpose}");
            ties::set_conn(&tx, &principal_id, ties::TieWrite { workspace_id: ws, set_by: &set_by, reason: tie_reason.as_deref().unwrap_or(ties::REASON_INVITE) })?;
        }
        if purpose == PURPOSE_OWNER_RESET {
            // The tie and the owner's agent mark go; an email kept from the
            // tied time stays unconfirmed, unless the owner gave the
            // address this link went to (then it is theirs, confirmed).
            ties::release_conn(&tx, &principal_id)?;
            if set_email != 0 {
                if let Some(addr) = sent_to.as_deref() {
                    super::ensure_email_free(&tx, addr, Some(&principal_id))?;
                    tx.execute("UPDATE principals SET email = ?1 WHERE id = ?2", params![addr, principal_id])
                        .map_err(|e| format!("invite email: {e}"))?;
                    ties::confirm_email_conn(&tx, &principal_id)?;
                }
            }
        }
        tx.commit().map_err(|e| format!("invite commit: {e}"))?;
        Ok(Consume::Ok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_password_page_is_https_on_an_allowed_host_or_exact_localhost() {
        let hosts = vec!["shop.box.k2.dev".to_string(), "door.example.test".to_string()];
        for ok in ["https://shop.box.k2.dev/set", "https://door.example.test", "http://localhost:3000/set", "http://127.0.0.1/x", "https://SHOP.box.k2.dev/set"] {
            assert!(check_set_password_url(ok, &hosts).is_ok(), "{ok}");
        }
        for bad in [
            "https://evil.example/set",
            "http://shop.box.k2.dev/set",
            "http://localhost.attacker.tld/set",
            "http://localhost@evil.example/set",
            "https://shop.box.k2.dev.evil.example/set",
            "https://shop.box.k2.dev/set#x",
            "javascript:alert(1)",
            "https://shop.box.k2.dev:99999/",
            "",
        ] {
            assert!(check_set_password_url(bad, &hosts).is_err(), "{bad}");
        }
        assert!(check_set_password_url("https://shop.box.k2.dev/set", &[]).is_err(), "no allowed host, no https page");
    }

    #[test]
    fn templates_carry_no_links() {
        for bad in ["see https://x.example", "HTTP://x", "visit www.example.com", "{link}", "k2inv_abc"] {
            assert!(check_body(bad).is_err(), "{bad}");
            assert!(check_subject(bad).is_err(), "{bad}");
        }
        assert!(check_body("Hi {name}, the {app} team saved you a seat.\nSee you soon.").is_ok());
    }

    #[test]
    fn values_that_read_as_links_never_reach_the_mail() {
        for bad in ["https://evil.example", "www.evil.example", "evil.example", "Visit evil.example now", "k2inv_x", "ftp://x", "a.b.co"] {
            assert!(link_like(bad), "{bad}");
        }
        for ok in ["Ada Lovelace", "Dr. Ada", "J.R.R. Tolkien", "Meet", "ada", "ada-2", "in 1.5 hours"] {
            assert!(!link_like(ok), "{ok}");
        }
        // A first.last username stays (they sign in with it); a scheme doesn't.
        let v = MailVars { app: "Meet", name: "Ada", username: "ada.lovelace", sender: "the bot", expires: "in 1 hour" };
        assert_eq!(render("{username}", &v), "ada.lovelace");
        let tpl = InviteTemplate { subject: None, body: Some("Sign in as {username}.".into()) };
        assert!(compose(PURPOSE_INVITE, &tpl, &v, "https://d.example/x").body.contains("Sign in as ada.lovelace."));
        let v2 = MailVars { username: "https://evil.example", ..v };
        assert!(compose(PURPOSE_INVITE, &tpl, &v2, "https://d.example/x").body.contains("Sign in as your account."));
        let v = MailVars { app: "shop.evil.example", name: "click www.evil.example", username: "ada", sender: "the bot", expires: "in 1 hour" };
        let c = compose(PURPOSE_INVITE, &InviteTemplate::default(), &v, "https://door.example/cli/skin/password/set#k2inv_t");
        assert!(!c.subject.contains("evil") && !c.body.contains("evil"), "{} / {}", c.subject, c.body);
        assert!(c.body.contains("Hello there,") && c.body.contains("your App"), "{}", c.body);
        assert_eq!(c.body.matches("https://").count(), 1, "only K2's own link: {}", c.body);
    }

    #[test]
    fn render_never_rescans_values() {
        let v = MailVars { app: "{name}", name: "Ada", username: "ada", sender: "the owner", expires: "in 1 hour" };
        assert_eq!(render("{app} / {name} / {nope}", &v), "{name} / Ada / {nope}");
        let c = compose(PURPOSE_OWNER_RESET, &InviteTemplate::default(), &v, "https://door.example/cli/skin/password/set#k2inv_t");
        assert!(c.subject.starts_with("Reset your password for"), "{}", c.subject);
        assert!(c.body.contains("Set your password: https://door.example/cli/skin/password/set#k2inv_t"), "{}", c.body);
    }
}
