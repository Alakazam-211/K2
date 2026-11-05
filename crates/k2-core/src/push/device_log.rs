//! One `[push]` line per push device registration added, changed or removed
//! (0.46.9). A phone that stops getting pushes is usually a row that went
//! away: an APNs 410, the phone turning push off, a duplicate token, a login
//! that ended. Before this, only the session reaper said anything, so a
//! missing row had no story. Now every insert, update and delete of
//! `push_devices` (K2 Companion) and `app_push_devices` (a custom App's own
//! phone app) goes through [`record`]:
//!
//! - one line in the daemon log, at the default level, tagged `[push]`;
//! - one record in an in-memory ring of the last [`RING_CAP`], read back by
//!   the owner with `k2 push log` next to the send attempts.
//!
//! PRIVACY: a record holds a device id cut to 8 characters, the platform,
//! the login, a short session id, an 8-hex token fingerprint
//! ([`crate::push_devices::token_fingerprint`], a hash, never a slice of the
//! token) and the App id for App rows. Never the vendor token, a push key or
//! message content.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many device events the ring keeps.
pub const RING_CAP: usize = 200;

/// How many characters of a session id a record keeps (`cu_` + 9).
pub const SESSION_LEN: usize = 12;

/// What happened to the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Add,
    Update,
    Remove,
}

impl Action {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Remove => "remove",
        }
    }
}

/// Why a row was added, changed or removed. The wire words are what
/// `k2 push log` prints after `reason=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A phone registered a device id this server didn't have.
    Register,
    /// The same phone registered again with the same login, session and token.
    Refresh,
    /// The same phone, login and session, with a new vendor token.
    TokenRotated,
    /// The device id now belongs to another login or another session.
    Moved,
    /// Another row of the same person held the same vendor token (an old
    /// device id): it was replaced by the new one.
    DuplicateToken,
    /// The phone's own unregister call (push turned off in the app).
    PhoneOff,
    /// The owner or an Admin removed it (`k2 push device remove`, or an
    /// App's owner removed one device).
    OwnerRemove,
    /// Apple or Google said the token is dead (APNs 410 Unregistered, FCM
    /// NotRegistered).
    VendorDead,
    /// The login that registered it logged out.
    Logout,
    /// The login was revoked (remove, disable, password or role change,
    /// kick, a revoked device sign-in).
    Revoked,
    /// The login's session reached its end date.
    SessionExpired,
    /// The login's session ended another way (an invalid record dropped, a
    /// newer pairing of the same device).
    SessionEnded,
    /// The sweep (boot and the session reaper) found no live login for it.
    NoLiveLogin,
    /// The Connect user was removed.
    UserRemoved,
    /// App push: the guest's device sign-in or pass ended.
    SignInEnded,
    /// App push: the guest passed the per-guest device limit; the least
    /// recently seen went.
    DeviceCap,
    /// App push: the App was removed.
    AppRemoved,
}

impl Reason {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Refresh => "refresh",
            Self::TokenRotated => "token_rotated",
            Self::Moved => "moved",
            Self::DuplicateToken => "duplicate_token",
            Self::PhoneOff => "phone_off",
            Self::OwnerRemove => "owner_remove",
            Self::VendorDead => "vendor_dead",
            Self::Logout => "logout",
            Self::Revoked => "revoked",
            Self::SessionExpired => "session_expired",
            Self::SessionEnded => "session_ended",
            Self::NoLiveLogin => "no_live_login",
            Self::UserRemoved => "user_removed",
            Self::SignInEnded => "sign_in_ended",
            Self::DeviceCap => "device_cap",
            Self::AppRemoved => "app_removed",
        }
    }
}

/// One change to one push device row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEvent {
    /// Unix ms when the change happened.
    pub at_ms: i64,
    /// `add` | `update` | `remove`.
    pub action: &'static str,
    /// [`Reason::as_wire`].
    pub reason: &'static str,
    /// The device id, first [`super::attempts::SHORT_LEN`] characters.
    pub device: String,
    /// `apns` | `fcm`.
    pub platform: String,
    /// Companion: the login (`owner` or a Connect user name). App: the
    /// guest's principal id.
    pub login: String,
    /// The login session, first [`SESSION_LEN`] characters (`cu_…` for a
    /// Companion login; the device sign-in or pass id for an App guest).
    /// `None` for the owner token and rows registered before 0.46.3.
    pub session: Option<String>,
    /// [`crate::push_devices::token_fingerprint`] of the vendor token.
    pub token: String,
    /// The App id for an App's own phone app; `None` for K2 Companion.
    pub app: Option<String>,
    /// For `moved`: where it came from (`login=… session=…`). For
    /// `duplicate_token`: the device id that replaced it.
    pub detail: Option<String>,
}

impl DeviceEvent {
    /// The log line (no timestamp; [`crate::log_debug!`] adds it).
    pub fn line(&self) -> String {
        let mut s = format!(
            "[push] device {} reason={} device={} platform={} login={} session={} token={}",
            self.action,
            self.reason,
            self.device,
            self.platform,
            self.login,
            self.session.as_deref().unwrap_or("-"),
            self.token
        );
        if let Some(a) = &self.app {
            s.push_str(&format!(" app={a}"));
        }
        if let Some(d) = &self.detail {
            s.push_str(&format!(" {d}"));
        }
        s
    }
}

/// The first [`SESSION_LEN`] characters of a session id.
pub fn short_session(sid: &str) -> String {
    sid.trim().chars().take(SESSION_LEN).collect()
}

/// The row facts a record is built from. `token` is the full vendor token:
/// it is hashed here and never kept.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    pub device_id: &'a str,
    pub platform: &'a str,
    pub login: &'a str,
    pub session: Option<&'a str>,
    pub token: &'a str,
    pub app: Option<&'a str>,
}

impl<'a> Row<'a> {
    /// A K2 Companion row.
    pub fn companion(device_id: &'a str, platform: &'a str, login: &'a str, session: Option<&'a str>, token: &'a str) -> Self {
        Self { device_id, platform, login, session, token, app: None }
    }
}

/// Build the record for one row change.
pub fn event(action: Action, reason: Reason, row: Row<'_>, detail: Option<String>) -> DeviceEvent {
    DeviceEvent {
        at_ms: super::unix_now_ms(),
        action: action.as_wire(),
        reason: reason.as_wire(),
        device: super::attempts::short(row.device_id),
        platform: row.platform.to_string(),
        login: row.login.to_string(),
        session: row.session.map(str::trim).filter(|s| !s.is_empty()).map(short_session),
        token: crate::push_devices::token_fingerprint(row.token),
        app: row.app.map(str::to_string),
        detail,
    }
}

/// Log and keep one row change.
pub fn record(action: Action, reason: Reason, row: Row<'_>, detail: Option<String>) {
    emit(event(action, reason, row, detail));
}

/// Log and keep a built record (one built inside a transaction and emitted
/// after its commit).
pub fn emit(e: DeviceEvent) {
    crate::log_debug!("{}", e.line());
    ring().push(e);
}

/// A bounded list of device events, oldest first.
#[derive(Debug, Default)]
pub struct Ring {
    items: VecDeque<DeviceEvent>,
}

impl Ring {
    pub const fn new() -> Self {
        Self { items: VecDeque::new() }
    }

    /// Keep `e`, dropping the oldest past [`RING_CAP`].
    pub fn push(&mut self, e: DeviceEvent) {
        while self.items.len() >= RING_CAP {
            self.items.pop_front();
        }
        self.items.push_back(e);
    }

    /// Oldest first; `device` keeps one device's (matched on its first
    /// [`super::attempts::SHORT_LEN`] characters).
    pub fn filtered(&self, device: Option<&str>) -> Vec<DeviceEvent> {
        let want = device.map(super::attempts::short).filter(|d| !d.is_empty());
        self.items.iter().filter(|e| want.as_ref().is_none_or(|w| &e.device == w)).cloned().collect()
    }
}

fn ring() -> std::sync::MutexGuard<'static, Ring> {
    static RING: Mutex<Ring> = Mutex::new(Ring::new());
    RING.lock().unwrap_or_else(|p| p.into_inner())
}

/// The process-wide ring ([`Ring::filtered`]).
pub fn recent(device: Option<&str>) -> Vec<DeviceEvent> {
    ring().filtered(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_never_holds_the_token_and_cuts_ids() {
        let token = "80f2c1d3e4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1";
        let sid = "cu_0123456789abcdef0123456789abcdef";
        let e = event(
            Action::Remove,
            Reason::VendorDead,
            Row::companion("abcdef0123456789", "apns", "gina", Some(sid), token),
            None,
        );
        assert_eq!(e.device, "abcdef01");
        assert_eq!(e.session.as_deref(), Some("cu_012345678"));
        assert_eq!(e.token, crate::push_devices::token_fingerprint(token));
        let line = e.line();
        assert_eq!(
            line,
            format!(
                "[push] device remove reason=vendor_dead device=abcdef01 platform=apns login=gina session=cu_012345678 token={}",
                e.token
            )
        );
        assert!(!line.contains(&token[..12]), "no token slice in the line: {line}");
        assert!(!line.contains(sid), "no whole session id: {line}");
        let json = serde_json::to_string(&e).expect("json");
        assert!(!json.contains(&token[..12]), "no token slice in the record: {json}");
    }

    #[test]
    fn app_rows_and_details_ride_the_line() {
        let row = Row { device_id: "phone-1", platform: "fcm", login: "prn_x", session: None, token: "t", app: Some("acme") };
        let e = event(Action::Update, Reason::Moved, row, Some("from login=a session=-".into()));
        assert_eq!(e.session, None);
        assert!(e.line().contains(" session=- "), "{}", e.line());
        assert!(e.line().contains(" app=acme from login=a session=-"), "{}", e.line());
    }

    #[test]
    fn ring_keeps_the_last_200_and_filters_by_device() {
        let mut ring = Ring::new();
        for i in 0..(RING_CAP + 3) {
            let id = if i % 2 == 0 { "aaaaaaaa-1" } else { "bbbbbbbb-2" };
            let mut e = event(Action::Add, Reason::Register, Row::companion(id, "apns", "owner", None, "t"), None);
            e.at_ms = i as i64;
            ring.push(e);
        }
        let all = ring.filtered(None);
        assert_eq!(all.len(), RING_CAP);
        assert_eq!(all[0].at_ms, 3, "the oldest three dropped");
        assert!(ring.filtered(Some("aaaaaaaa-rest")).iter().all(|e| e.device == "aaaaaaaa"));
        assert!(ring.filtered(Some("cccccccc")).is_empty());
    }
}
