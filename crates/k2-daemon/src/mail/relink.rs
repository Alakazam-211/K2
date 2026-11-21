//! Does a linked inbox need a person to link it again, and why (0.46.17).
//!
//! Before this, `k2 mail link list` said `connected` while every read
//! failed with "Gmail rejected the OAuth access token … must be
//! re-linked": the row's `status` only changed on the add and draft paths,
//! and the mail watcher's `needs_relink` (W30) only when someone watched
//! the inbox. Now:
//!
//! * **Every IMAP login and every OAuth token refresh** that the provider
//!   refuses ([`is_credential_refused`]) stamps the row `status = 'error'`
//!   with the reason ([`note_login_failure`]); the next login that works
//!   clears it (`external::record_check(Ok)`, as before).
//! * **The catalog** (`k2 mail link list`, `k2 mail inboxes`, Settings)
//!   carries `needsRelink` and `relinkReason`: [`relink_reason`] reads the
//!   row and the watcher's cursor. A running provider cool-down
//!   ([`crate::mail::imap_guard`]) reads `throttled` instead: linking
//!   again won't help then.
//! * **Mail access not granted** (a person unticked the Gmail permission
//!   on Google's page): the stored scopes say so, the XOAUTH2 error says
//!   so ([`explain_xoauth2_refusal`]) and `relinkReason` is
//!   `mail_access_not_granted`, rather than "the token expired".
//! * **A re-link** (`k2 mail link relink`, Settings' Sign in again: the
//!   in-place re-consent) resets the row's status and clears the watcher's
//!   mark at once (`external::upgrade_oauth_inbox` calls [`after_relink`]).
//! * The catalog's `signInRefused` is the same truth as `needsRelink`
//!   (credential refused, or mail access not granted); a network error or
//!   a provider throttle is never a refusal.

/// Did the provider refuse the credential itself (not the network, not a
/// busy server)? Narrower than the watcher's rule: a bare IMAP `NO` is not
/// a refusal here.
pub fn is_credential_refused(msg: &str) -> bool {
    if crate::mail::imap_guard::parse_tag(msg).is_some() || crate::mail::imap_guard::is_throttle(msg) {
        return false;
    }
    let m = msg.to_ascii_lowercase();
    [
        "re-link",
        "relink",
        "invalid_grant",
        "authenticationfailed",
        "invalidauthenticationtoken",
        "invalid credentials",
        "authentication failed",
        "[auth",
        "no refresh_token",
    ]
    .iter()
    .any(|n| m.contains(n))
}

/// A login or token refresh failed: stamp a credential refusal on the row
/// (the catalog then says `needs re-link`), and feed the provider guard.
pub fn note_login_failure(row_id: &str, msg: &str) {
    crate::mail::imap_guard::note_failure(row_id, msg);
    if is_credential_refused(msg) {
        crate::mail::external::record_check(row_id, Err(msg));
    }
}

/// Is mail asked for on this row but missing from the scopes the provider
/// granted? (`None` scopes = a row from before 0188: unknown, so no.)
pub fn mail_scope_missing(provider: Option<&str>, features: &str, granted: Option<&str>) -> bool {
    let Some(p) = provider.and_then(crate::mail::oauth::OauthProvider::from_str) else {
        return false;
    };
    let wants_mail = features.split(',').any(|f| f.trim() == "mail");
    match granted {
        Some(g) if wants_mail => !crate::mail::oauth::features_granted(p, g).mail,
        _ => false,
    }
}

/// The plain words for a Gmail XOAUTH2 refusal on this row: "mail access
/// not granted" when the stored scopes lack Gmail, else `msg` unchanged.
pub fn explain_xoauth2_refusal(row_id: &str, msg: String) -> String {
    if !crate::mail::imap_guard::is_xoauth2_refusal(&msg) {
        return msg;
    }
    let fields = crate::mail::external::read_oauth_fields(row_id).ok();
    let meta = crate::mail::external::read_oauth_meta(row_id).unwrap_or_default();
    let provider = fields.as_ref().and_then(|f| f.provider.as_deref());
    if mail_scope_missing(provider, &meta.features, meta.granted_scopes.as_deref()) {
        return concat!(
            "Gmail rejected the login: mail access not granted — Google's consent for this \
             inbox doesn't include the Gmail permission (it was unticked on Google's page). A \
             person links it again with '",
            k2_core::cli_name!(),
            " mail link relink <address>' and ticks the Gmail permission"
        )
        .to_string();
    }
    msg
}

/// Why `row_id` needs a person, if it does: `throttled` (the provider is
/// limiting the account; wait), `mail_access_not_granted`,
/// `credential_refused`. Reads on a held connection (the catalog holds the
/// DB lock).
pub fn relink_reason(conn: &rusqlite::Connection, row_id: &str) -> Option<&'static str> {
    if crate::mail::imap_guard::cooldown_left(row_id).is_some() {
        return Some("throttled");
    }
    let row: Option<(String, Option<String>, Option<String>, String, Option<String>)> = conn
        .query_row(
            "SELECT status, last_error, provider, features, granted_scopes FROM mail_external_inboxes WHERE id = ?1",
            rusqlite::params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();
    let (status, last_error, provider, features, granted) = row?;
    if mail_scope_missing(provider.as_deref(), &features, granted.as_deref()) {
        return Some("mail_access_not_granted");
    }
    let watcher_marked: bool = conn
        .query_row(
            "SELECT needs_relink FROM mail_watch_cursors WHERE source = 'linked' AND inbox_id = ?1",
            rusqlite::params![row_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n == 1)
        .unwrap_or(false);
    let refused = status == "error" && last_error.as_deref().is_some_and(is_credential_refused);
    (watcher_marked || refused).then_some("credential_refused")
}

/// After a successful in-place re-consent: the watcher may poll again at
/// once. (A provider cool-down is NOT cleared: a new token doesn't lift
/// the provider's limit on the account.)
pub fn after_relink(row_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = conn.execute(
        "UPDATE mail_watch_cursors SET needs_relink = 0, relink_cred = NULL, consecutive_errs = 0, \
         next_poll_at = 0 WHERE source = 'linked' AND inbox_id = ?1",
        rusqlite::params![row_id],
    ) {
        k2_core::log_debug!("[mail/relink] clear the watcher's mark for {row_id}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(status: &str, last_error: Option<&str>, provider: &str, features: &str, scopes: Option<&str>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let owner = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, port, username, created_at, \
             status, last_error, auth_kind, provider, features, granted_scopes) \
             VALUES (?1, ?2, ?3, 'imap.gmail.com', 993, ?3, 100, ?4, ?5, 'oauth', ?6, ?7, ?8)",
            rusqlite::params![id, owner, format!("rl-{}@gmail.com", &id[..8]), status, last_error, provider, features, scopes],
        )
        .expect("seed");
        id
    }

    fn reason(id: &str) -> Option<&'static str> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        relink_reason(&conn, id)
    }

    #[test]
    fn refusals_are_told_apart_from_network_and_throttle_failures() {
        assert!(is_credential_refused("Gmail rejected the OAuth access token (XOAUTH2 status 400) — the inbox must be re-linked"));
        assert!(is_credential_refused("oauth token for inbox x: token endpoint: invalid_grant"));
        assert!(is_credential_refused("login: No Response: [AUTHENTICATIONFAILED] Invalid credentials"));
        assert!(!is_credential_refused("connect: Connection refused (os error 61)"));
        assert!(!is_credential_refused("login: No Response: [ALERT] Too many simultaneous connections"));
        assert!(!is_credential_refused(&crate::mail::imap_guard::throttled_error(300)));
    }

    #[test]
    fn the_catalog_reason_follows_the_row_the_watcher_and_the_scopes() {
        let gmail = "openid email https://mail.google.com/";
        let ok = seed("connected", None, "gmail", "mail", Some(gmail));
        assert_eq!(reason(&ok), None);
        note_login_failure(&ok, "Gmail rejected the OAuth access token (XOAUTH2 status 400) — the inbox must be re-linked");
        assert_eq!(reason(&ok), Some("credential_refused"), "a refused read shows");
        crate::mail::external::record_check(&ok, Ok(()));
        assert_eq!(reason(&ok), None, "the next good login clears it");
        // A network failure is not a re-link.
        note_login_failure(&ok, "connect: timed out");
        assert_eq!(reason(&ok), None);

        let unticked = seed("connected", None, "gmail", "mail", Some("openid email"));
        assert_eq!(reason(&unticked), Some("mail_access_not_granted"));
        let explained = explain_xoauth2_refusal(&unticked, "Gmail rejected the OAuth access token (XOAUTH2 status 400)".into());
        assert!(explained.contains("mail access not granted"), "{explained}");
        let other = explain_xoauth2_refusal(&ok, "Gmail rejected the OAuth access token (XOAUTH2 status 400)".into());
        assert!(!other.contains("not granted"), "{other}");

        // The catalog carries the inbox id, signInRefused and the reason.
        note_login_failure(&ok, "Gmail rejected the OAuth access token (XOAUTH2 status 400) — the inbox must be re-linked");
        let all = crate::mail::access::catalog_json(None);
        let row = all["inboxes"].as_array().expect("inboxes").iter().find(|i| i["id"] == ok.as_str()).cloned().expect("listed by id");
        assert_eq!(row["signInRefused"], true, "{row}");
        assert_eq!(row["needsRelink"], true);
        assert_eq!(row["relinkReason"], "credential_refused");
        crate::mail::external::record_check(&ok, Ok(()));

        // The watcher's mark counts, and a re-link clears it.
        let watched = seed("connected", None, "gmail", "mail", Some(gmail));
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_watch_cursors (source, inbox_id, needs_relink) VALUES ('linked', ?1, 1)",
                rusqlite::params![watched],
            )
            .expect("cursor");
        }
        assert_eq!(reason(&watched), Some("credential_refused"));
        after_relink(&watched);
        assert_eq!(reason(&watched), None);
    }

    #[test]
    fn a_cooldown_reads_throttled_and_a_relink_does_not_lift_it() {
        let id = seed("connected", None, "gmail", "mail", Some("https://mail.google.com/"));
        assert!(crate::mail::imap_guard::note_failure(&id, "Too many simultaneous connections"));
        assert_eq!(reason(&id), Some("throttled"));
        after_relink(&id);
        assert_eq!(reason(&id), Some("throttled"), "a re-link doesn't lift the provider's limit");
        assert!(crate::mail::imap_guard::acquire_within(&id, std::time::Duration::ZERO).is_err());
        crate::mail::imap_guard::reset_for_test(&id);
    }
}
