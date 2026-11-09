//! vault.db side of a refreshing sign-in (`Sign::OauthRefresh`,
//! prd-vault-for-agents-v1 §6): the person's value stays as its version, and
//! the current tokens live in `oauth_state`, sealed under the VDK and MAC'd.
//!
//! The network half (when to refresh, single-flight, the token POST) is the
//! daemon's `vault::oauth_refresh`; this crate does no network I/O.
//!
//! - A refresh writes a row and never a version (V18), and never moves `gen`.
//! - A person's new value resets the row ([`forget`]): the new sign-in starts
//!   over. A refresh that was in flight meanwhile is dropped
//!   ([`LocalVault::oauth_store_refresh`] checks the version it started from).
//! - Nothing here returns a value to a route: callers are the daemon's use
//!   path, after `resolve` said this requester may use the secret.

use super::*;
use crate::inject::{oauth_token_ok, ClientAuth, OauthBundle, Sign};

const STATE_ACTIVE: &str = "active";
const STATE_EXPIRED: &str = "expired";
/// Who the `refresh` audit rows name.
const AUDIT_ACTOR: &str = "system:oauth-refresh";

/// What the daemon needs for one use of a sign-in. Everything sensitive is
/// in [`SecretBytes`], so this can't be printed.
pub struct OauthCurrent {
    pub secret_id: String,
    pub name: String,
    /// The value version these tokens came from.
    pub version: u32,
    pub token_url: String,
    pub client_auth: ClientAuth,
    pub client_id: String,
    pub client_secret: Option<SecretBytes>,
    /// `None` once the sign-in has expired.
    pub access: Option<SecretBytes>,
    pub refresh: Option<SecretBytes>,
    /// Unix seconds; `None` = unknown.
    pub expires_at: Option<i64>,
    /// When the current token was stored (Unix seconds); `None` while the
    /// person's value is still the current one.
    pub issued_at: Option<i64>,
    /// The provider refused the refresh token: someone must sign in again.
    pub expired: bool,
    /// Moved out (box move): nothing can be stored, so don't spend a token.
    pub frozen: bool,
}

/// A sign-in's state as `list`, `show` and the doctor report it. Never a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthStatus {
    pub state: String,
    pub expires_at: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl OauthStatus {
    pub fn is_expired(&self) -> bool {
        self.state == STATE_EXPIRED
    }
}

pub(super) fn is_oauth_refresh(m: &SecretMeta) -> bool {
    m.inject.as_ref().and_then(|i| i.sign.as_ref()).is_some_and(Sign::is_oauth_refresh)
}

pub(super) fn fetch_only() -> VaultError {
    VaultError::forbidden("oauth_fetch_only", "a refreshing sign-in is used through K2 only (fetch or proxy); its refresh token never leaves K2")
}

/// Terms of a new secret (`set`). A refreshing sign-in stays at fetch level.
pub(super) fn check_terms(namespace: Namespace, inject: Option<&Inject>, deliveries: &[Delivery]) -> Result<()> {
    if !inject.and_then(|i| i.sign.as_ref()).is_some_and(Sign::is_oauth_refresh) {
        return Ok(());
    }
    if namespace != Namespace::User {
        return Err(VaultError::bad_request("bad_inject", "a refreshing sign-in is a user secret"));
    }
    if deliveries.iter().any(|d| d.level() > Delivery::Fetch.level()) {
        return Err(VaultError::bad_request(fetch_only().code, fetch_only().hint));
    }
    Ok(())
}

/// A policy change: the signer can't be taken off (the stored value holds
/// the refresh token, and without the signer it would be sent as a key),
/// and the delivery can't rise above fetch.
pub(super) fn check_policy_change(old: &SecretMeta, new: &SecretMeta) -> Result<()> {
    if is_oauth_refresh(old) && !is_oauth_refresh(new) {
        return Err(VaultError::bad_request(
            "oauth_signer_fixed",
            "this value is a refreshing sign-in, so it can't lose its signer; destroy it and save the token as a new secret",
        ));
    }
    check_terms(new.namespace, new.inject.as_ref(), &[new.default_delivery, new.delivery_ceiling])
}

/// Drop a secret's tokens (new value, or destroyed).
pub(super) fn forget(tx: &Transaction<'_>, secret_id: &str) -> Result<()> {
    tx.execute("DELETE FROM oauth_state WHERE secret_id = ?1", params![secret_id])?;
    Ok(())
}

fn seal_aad(vid: &[u8; 16], secret_id: &str, which: &str) -> Vec<u8> {
    let mut e = Enc::new("oauth-state");
    e.bytes(vid).str(secret_id).str(which);
    e.finish().to_vec()
}

fn seal(key: &Key32, aad: &[u8], v: &[u8]) -> Vec<u8> {
    let (nonce, ct) = xseal(key, aad, v);
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    out
}

fn unseal(key: &Key32, aad: &[u8], blob: &[u8]) -> Result<SecretBytes> {
    if blob.len() < 24 {
        return Err(VaultError::tamper("a sign-in's sealed token is too short"));
    }
    Ok(SecretBytes::new(xopen(key, &blob[..24], aad, &blob[24..])?))
}

struct StateRow {
    state: String,
    access_ct: Option<Vec<u8>>,
    refresh_ct: Option<Vec<u8>>,
    expires_at: Option<String>,
    last_error: Option<String>,
    updated_at: String,
    mac: Vec<u8>,
}

impl StateRow {
    fn mac_input(&self, secret_id: &str) -> Vec<u8> {
        let blob = |e: &mut Enc, v: &Option<Vec<u8>>| match v {
            Some(b) => {
                e.u8(1).bytes(b);
            }
            None => {
                e.u8(0);
            }
        };
        let mut e = Enc::new("oauth-state-row");
        e.str(secret_id).str(&self.state);
        blob(&mut e, &self.access_ct);
        blob(&mut e, &self.refresh_ct);
        e.opt_str(self.expires_at.as_deref()).opt_str(self.last_error.as_deref()).str(&self.updated_at);
        e.finish().to_vec()
    }
}

fn read_state(conn: &Connection, secret_id: &str) -> Result<Option<StateRow>> {
    Ok(conn
        .query_row(
            "SELECT state, access_ct, refresh_ct, expires_at, last_error, updated_at, row_mac FROM oauth_state WHERE secret_id = ?1",
            params![secret_id],
            |r| {
                Ok(StateRow {
                    state: r.get(0)?,
                    access_ct: r.get(1)?,
                    refresh_ct: r.get(2)?,
                    expires_at: r.get(3)?,
                    last_error: r.get(4)?,
                    updated_at: r.get(5)?,
                    mac: r.get(6)?,
                })
            },
        )
        .optional()?)
}

fn write_state(tx: &Transaction<'_>, key: &Key32, secret_id: &str, row: &StateRow) -> Result<()> {
    let mac = hmac(key, &row.mac_input(secret_id));
    tx.execute(
        "INSERT OR REPLACE INTO oauth_state (secret_id, state, access_ct, refresh_ct, expires_at, last_error, updated_at, row_mac)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![secret_id, row.state, row.access_ct, row.refresh_ct, row.expires_at, row.last_error, row.updated_at, mac.to_vec()],
    )?;
    Ok(())
}

/// Is the secret still the active one at `version`? Read inside the write's
/// transaction, so a `rotate` can't slip in between.
fn still_at_version(tx: &Transaction<'_>, secret_id: &str, version: u32) -> Result<bool> {
    let row: Option<(u32, String)> = tx
        .query_row("SELECT latest_version, status FROM secret WHERE id = ?1", params![secret_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.is_some_and(|(v, s)| v == version && s == SecretStatus::Active.as_str()))
}

impl LocalVault {
    fn oauth_row_checked(&self, conn: &Connection, keys: &Keys, secret_id: &str) -> Result<Option<StateRow>> {
        let Some(row) = read_state(conn, secret_id)? else { return Ok(None) };
        if !hmac_verify(&keys.row, &row.mac_input(secret_id), &row.mac) {
            self.tamper_once(format!("oauth_state row for secret {secret_id} failed its integrity check"));
            return Err(VaultError::tamper("a sign-in's stored tokens failed their integrity check"));
        }
        Ok(Some(row))
    }

    /// The tokens and client details of a sign-in. The daemon's use path
    /// calls this after `resolve` allowed the use; it isn't a route.
    pub fn oauth_current(&self, secret_id: &str) -> Result<OauthCurrent> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = self.find_row(&conn, &keys, &Selector::Id(secret_id.to_string()))?;
        let m = &row.meta;
        let Some(Sign::OauthRefresh { token_url, client_auth }) = m.inject.as_ref().and_then(|i| i.sign.clone()) else {
            return Err(VaultError::bad_request("not_a_sign_in", "this secret isn't a refreshing sign-in"));
        };
        if m.status != SecretStatus::Active {
            return Err(VaultError::forbidden("secret_not_active", "that secret is disabled"));
        }
        let value = self.decrypt_version(&conn, &keys, m, m.latest_version)?;
        let bundle = OauthBundle::parse(value.expose(), client_auth)?;
        let state = self.oauth_row_checked(&conn, &keys, secret_id)?;
        let mut cur = OauthCurrent {
            secret_id: m.id.clone(),
            name: m.name.clone(),
            version: m.latest_version,
            token_url,
            client_auth,
            client_id: bundle.client_id,
            client_secret: bundle.client_secret,
            access: Some(bundle.access_token),
            refresh: Some(bundle.refresh_token),
            expires_at: bundle.expires_at,
            issued_at: None,
            expired: false,
            frozen: self.is_frozen(),
        };
        if let Some(s) = state {
            if s.state == STATE_EXPIRED {
                cur.access = None;
                cur.refresh = None;
                cur.expires_at = None;
                cur.expired = true;
                return Ok(cur);
            }
            let (Some(a), Some(r)) = (s.access_ct.as_deref(), s.refresh_ct.as_deref()) else {
                return Err(VaultError::tamper("a sign-in's stored tokens are incomplete"));
            };
            cur.access = Some(unseal(&keys.vdk, &seal_aad(&self.vid, secret_id, "access"), a)?);
            cur.refresh = Some(unseal(&keys.vdk, &seal_aad(&self.vid, secret_id, "refresh"), r)?);
            cur.expires_at = s.expires_at.as_deref().and_then(parse_ts).map(|t| t.timestamp());
            cur.issued_at = parse_ts(&s.updated_at).map(|t| t.timestamp());
        }
        Ok(cur)
    }

    /// Store the tokens a refresh returned. `false`: the secret moved past
    /// `base_version` (or isn't active) since the refresh began, so nothing
    /// was written and the caller must read the new value.
    pub fn oauth_store_refresh(&self, secret_id: &str, base_version: u32, access: &SecretBytes, refresh: &SecretBytes, expires_at: i64) -> Result<bool> {
        if !oauth_token_ok(access.expose()) || !oauth_token_ok(refresh.expose()) {
            return Err(VaultError::bad_request("bad_token", "the token endpoint returned a token K2 can't send"));
        }
        let t = DateTime::<Utc>::from_timestamp(expires_at, 0).ok_or_else(|| VaultError::bad_request("bad_token", "the token's expiry is out of range"))?;
        self.mutate(false, |tx, keys| {
            if !still_at_version(tx, secret_id, base_version)? {
                return Ok((false, vec![]));
            }
            let row = StateRow {
                state: STATE_ACTIVE.into(),
                access_ct: Some(seal(&keys.vdk, &seal_aad(&self.vid, secret_id, "access"), access.expose())),
                refresh_ct: Some(seal(&keys.vdk, &seal_aad(&self.vid, secret_id, "refresh"), refresh.expose())),
                expires_at: Some(ts(t)),
                last_error: None,
                updated_at: ts(now()),
                mac: vec![],
            };
            write_state(tx, &keys.row, secret_id, &row)?;
            Ok((true, vec![]))
        })
    }

    /// The provider refused the refresh token: drop the tokens and mark the
    /// sign-in expired. `code` is a fixed word, never provider text. `false`:
    /// the secret moved past `base_version`, so nothing was written.
    pub fn oauth_mark_expired(&self, secret_id: &str, base_version: u32, code: &str) -> Result<bool> {
        self.mutate(false, |tx, keys| {
            if !still_at_version(tx, secret_id, base_version)? {
                return Ok((false, vec![]));
            }
            let row = StateRow {
                state: STATE_EXPIRED.into(),
                access_ct: None,
                refresh_ct: None,
                expires_at: None,
                last_error: Some(code.to_string()),
                updated_at: ts(now()),
                mac: vec![],
            };
            write_state(tx, &keys.row, secret_id, &row)?;
            Ok((true, vec![]))
        })
    }

    /// One `refresh` audit entry, rolled up (`ok`, `expired`, `error:<code>`).
    pub fn oauth_audit(&self, secret_id: &str, outcome: &str) {
        self.rollups.lock().add(&Entry::new("refresh", AUDIT_ACTOR, outcome).secret(secret_id, None));
    }

    /// A sign-in's state, or `None` while the person's value is still the
    /// current one (no refresh has happened).
    pub fn oauth_status(&self, secret_id: &str) -> Result<Option<OauthStatus>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        Ok(self.oauth_row_checked(&conn, &keys, secret_id)?.map(|s| OauthStatus {
            state: s.state,
            expires_at: s.expires_at,
            last_error: s.last_error,
            updated_at: s.updated_at,
        }))
    }

    /// The parts of a sign-in to mask on their own (the whole bundle is
    /// masked by the caller): its secrets and the current tokens. An
    /// unreadable row adds nothing here; the use path reports it.
    pub(super) fn oauth_redact_parts(&self, conn: &Connection, keys: &Keys, m: &SecretMeta, value: &SecretBytes) -> Vec<(String, SecretBytes)> {
        let Some(Sign::OauthRefresh { client_auth, .. }) = m.inject.as_ref().and_then(|i| i.sign.as_ref()) else {
            return vec![];
        };
        let mut parts: Vec<SecretBytes> = vec![];
        if let Ok(b) = OauthBundle::parse(value.expose(), *client_auth) {
            parts.push(b.access_token);
            parts.push(b.refresh_token);
            parts.extend(b.client_secret);
        }
        if let Ok(Some(s)) = self.oauth_row_checked(conn, keys, &m.id) {
            for (which, ct) in [("access", s.access_ct.as_deref()), ("refresh", s.refresh_ct.as_deref())] {
                if let Some(Ok(v)) = ct.map(|c| unseal(&keys.vdk, &seal_aad(&self.vid, &m.id, which), c)) {
                    parts.push(v);
                }
            }
        }
        parts.into_iter().filter(|p| p.len() >= SURFACE_MIN_LEN).map(|p| (m.name.clone(), p)).collect()
    }

    /// Rekey: verify each row under the old key, re-seal its tokens and
    /// re-MAC it under the new one, in the rekey's transaction.
    pub(super) fn rekey_oauth_state(&self, tx: &Transaction<'_>, old: &Keys, new: &Keys) -> Result<()> {
        let ids: Vec<String> = {
            let mut st = tx.prepare("SELECT secret_id FROM oauth_state")?;
            let it = st.query_map([], |r| r.get(0))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for id in ids {
            let Some(mut row) = read_state(tx, &id)? else { continue };
            if !hmac_verify(&old.row, &row.mac_input(&id), &row.mac) {
                return Err(VaultError::tamper(format!("sign-in {id}'s stored tokens failed their integrity check; rekey stopped")));
            }
            for (which, ct) in [("access", &mut row.access_ct), ("refresh", &mut row.refresh_ct)] {
                if let Some(c) = ct.as_ref() {
                    let aad = seal_aad(&self.vid, &id, which);
                    let pt = unseal(&old.vdk, &aad, c)?;
                    *ct = Some(seal(&new.vdk, &aad, pt.expose()));
                }
            }
            write_state(tx, &new.row, &id, &row)?;
        }
        Ok(())
    }
}
