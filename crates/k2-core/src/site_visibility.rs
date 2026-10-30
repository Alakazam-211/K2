//! `k2 publish` visibility: who may open a published site (PRD
//! `prd-publish-visibility-and-dev-mode-v1`, k2.db 0253).
//!
//! A site has one of three modes: **private** (the owner or an Admin, signed
//! in through a one-time link), **PIN** (K2's PIN page first) and **public**.
//! A covered label with no row is private: fail closed. The daemon's gate
//! (k2-daemon `site_gate`) enforces this before a site's port is dialed; the
//! routes and the CLI change it. Everything here takes a `&Connection`, so a
//! caller picks its lock and tests can use a scratch database.
//!
//! Secrets: a PIN is stored only as an argon2id hash (the parameters of K2
//! Connect users), and session ids and one-time codes only as SHA-256 hashes.
//! Hash a PIN with [`hash_pin`] *before* taking the database lock: it is slow
//! on purpose. No `Debug`, `Serialize` or log line here carries a secret.
//!
//! **Moving from "everything is public" (vs-live PA-13).** The migration
//! writes `public` for every tunnelled service. A label only the control plane
//! knows gets its `public` row from [`grandfather_map`], which the daemon runs
//! on the persisted map at boot and on each fetched map **before** that map
//! routes anything, until the first fetched map settled it. A label created
//! after the upgrade gets a `private` row at creation ([`note_new_label`]), so
//! a slow first fetch can't grandfather it by mistake.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const MODE_PRIVATE: &str = "private";
pub const MODE_PIN: &str = "pin";
pub const MODE_PUBLIC: &str = "public";

/// `public_reason` of a site that was public when 0.46.7 arrived.
pub const REASON_BEFORE: &str = "public before 0.46.7";
/// `set_by` of a row the upgrade wrote.
pub const SET_BY_UPGRADE: &str = "upgrade";
/// `user` of a session or code minted with the owner token (no login behind
/// it). A login is its username; the gate re-checks that login's role.
pub const USER_OWNER_TOKEN: &str = "owner-token";
/// `user` of a session a right PIN opened.
pub const USER_PIN: &str = "pin";

/// K2's cookie for a private sign-in (host-only: `__Host-`).
pub const COOKIE_SITE: &str = "__Host-k2_site";
/// K2's cookie for a PIN session.
pub const COOKIE_PIN: &str = "__Host-k2_pin";
/// The reserved path family K2's own pages live under on a gated label.
pub const RESERVED_PATH: &str = "/_k2/site/";

/// A one-time link lives this long (PV-5).
pub const OPEN_CODE_SECS: i64 = 60;
/// A private session lives this long (P-D5).
pub const OPEN_SESSION_SECS: i64 = 12 * 3600;
/// PIN session life in days: default and bounds (PV-9).
pub const PIN_DAYS_DEFAULT: i64 = 7;
pub const PIN_DAYS_MIN: i64 = 1;
pub const PIN_DAYS_MAX: i64 = 30;
/// PIN length in characters (PV-7); under [`PIN_WARN_BELOW`] gets a warning.
pub const PIN_MIN: usize = 6;
pub const PIN_MAX: usize = 64;
pub const PIN_WARN_BELOW: usize = 8;
/// Dev idle stop: default and bounds in seconds (DM-6). `0` = off.
pub const IDLE_DEFAULT_SECS: i64 = 3600;
pub const IDLE_MIN_SECS: i64 = 300;
pub const IDLE_MAX_SECS: i64 = 86_400;

/// Why a change was refused: `<code>: <plain words>`.
pub type R<T> = Result<T, String>;

fn db(e: rusqlite::Error) -> String {
    format!("site_visibility_db: {e}")
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Private,
    Pin,
    Public,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Private => MODE_PRIVATE,
            Mode::Pin => MODE_PIN,
            Mode::Public => MODE_PUBLIC,
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            MODE_PRIVATE => Some(Mode::Private),
            MODE_PIN => Some(Mode::Pin),
            MODE_PUBLIC => Some(Mode::Public),
            _ => None,
        }
    }
}

/// One label's visibility. `explicit: false` = no row: the private default.
/// The PIN hash stays out of `Serialize` and `Debug`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteVisibility {
    pub label: String,
    pub mode: Mode,
    pub dev: bool,
    /// A PIN is set (`mode == Pin` always has one).
    pub pin_set: bool,
    pub pin_generation: i64,
    pub pin_days: i64,
    /// Dev idle stop; `None` = [`IDLE_DEFAULT_SECS`], `Some(0)` = off.
    pub idle_secs: Option<i64>,
    pub set_by: String,
    pub set_at: i64,
    pub public_reason: Option<String>,
    pub explicit: bool,
    #[serde(skip)]
    pin_hash: Option<String>,
}

impl std::fmt::Debug for SiteVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteVisibility")
            .field("label", &self.label)
            .field("mode", &self.mode)
            .field("dev", &self.dev)
            .field("pin_set", &self.pin_set)
            .field("pin_generation", &self.pin_generation)
            .field("explicit", &self.explicit)
            .finish_non_exhaustive()
    }
}

impl SiteVisibility {
    /// What a label with no row is: private.
    fn default_for(label: &str) -> Self {
        SiteVisibility {
            label: label.to_string(),
            mode: Mode::Private,
            dev: false,
            pin_set: false,
            pin_generation: 0,
            pin_days: PIN_DAYS_DEFAULT,
            idle_secs: None,
            set_by: String::new(),
            set_at: 0,
            public_reason: None,
            explicit: false,
            pin_hash: None,
        }
    }

    /// Does a right PIN check out against this row? Slow (argon2id): never
    /// under the database lock. `false` when no PIN is set.
    pub fn verify_pin(&self, pin: &str) -> bool {
        self.pin_hash.as_deref().is_some_and(|h| crate::connect_users::verify_hash(pin, h))
    }
}

/// A nested label, lowercase. Same rule as `k2 publish client-ip`.
fn norm(label: &str) -> R<String> {
    crate::tunnel::client_ip_labels::normalize_label(label).map_err(|e| format!("bad_label: {e}"))
}

/// Does this table speak for `label`? K2's reserved labels never (PV-1; a real
/// App is decided by the gate, which can see the App store).
pub fn is_covered(label: &str) -> bool {
    !crate::skin::is_reserved_nested_label(label)
}

fn refuse_reserved(label: &str) -> R<()> {
    if is_covered(label) {
        Ok(())
    } else {
        Err(format!("reserved_label: '{label}' is K2's own: its visibility isn't set here."))
    }
}

// ── reading ───────────────────────────────────────────────────────────

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SiteVisibility> {
    let mode: String = r.get("mode")?;
    let pin_hash: Option<String> = r.get("pin_hash")?;
    Ok(SiteVisibility {
        label: r.get("label")?,
        // A value the CHECK can't let in reads as the fail-closed mode.
        mode: Mode::parse(&mode).unwrap_or(Mode::Private),
        dev: r.get::<_, i64>("dev")? != 0,
        pin_set: pin_hash.is_some(),
        pin_generation: r.get("pin_generation")?,
        pin_days: r.get("pin_days")?,
        idle_secs: r.get("idle_secs")?,
        set_by: r.get("set_by")?,
        set_at: r.get("set_at")?,
        public_reason: r.get("public_reason")?,
        explicit: true,
        pin_hash,
    })
}

/// The stored row, if any.
pub fn get(conn: &Connection, label: &str) -> R<Option<SiteVisibility>> {
    let label = label.trim().to_ascii_lowercase();
    conn.query_row("SELECT * FROM site_visibility WHERE label = ?1", params![label], row).optional().map_err(db)
}

/// The label's visibility, row or not: no row = private (`explicit: false`).
pub fn effective(conn: &Connection, label: &str) -> R<SiteVisibility> {
    let label = label.trim().to_ascii_lowercase();
    Ok(get(conn, &label)?.unwrap_or_else(|| SiteVisibility::default_for(&label)))
}

/// [`effective`]'s mode alone.
pub fn effective_mode(conn: &Connection, label: &str) -> R<Mode> {
    Ok(effective(conn, label)?.mode)
}

/// Every stored row, by label.
pub fn list(conn: &Connection) -> R<Vec<SiteVisibility>> {
    let mut st = conn.prepare("SELECT * FROM site_visibility ORDER BY label").map_err(db)?;
    let rows = st.query_map([], row).map_err(db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
}

// ── changes ───────────────────────────────────────────────────────────

fn changes_tx() -> &'static tokio::sync::watch::Sender<u64> {
    static TX: std::sync::OnceLock<tokio::sync::watch::Sender<u64>> = std::sync::OnceLock::new();
    TX.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// Wakes on every change made through this module. The gate re-checks open
/// connections on it, so a mode change or a revoke cuts a live socket at once.
pub fn subscribe() -> tokio::sync::watch::Receiver<u64> {
    changes_tx().subscribe()
}

fn changed() {
    changes_tx().send_modify(|n| *n = n.wrapping_add(1));
}

/// Write `label`'s row: `set` is the `SET` clause (`?1` = label, `?2` = by,
/// `?3` = now, the rest from `extra`). A row that wasn't there starts from the
/// private default.
fn upsert(conn: &Connection, label: &str, by: &str, set: &str, extra: &[&dyn rusqlite::ToSql]) -> R<()> {
    let now = now_unix();
    conn.execute(
        "INSERT OR IGNORE INTO site_visibility (label, mode, set_by, set_at) VALUES (?1, 'private', ?2, ?3)",
        params![label, by, now],
    )
    .map_err(db)?;
    let sql = format!("UPDATE site_visibility SET {set}, set_by = ?2, set_at = ?3 WHERE label = ?1");
    let mut p: Vec<&dyn rusqlite::ToSql> = vec![&label, &by, &now];
    p.extend_from_slice(extra);
    conn.execute(&sql, p.as_slice()).map_err(db)?;
    Ok(())
}

fn end_pin_sessions(conn: &Connection, label: &str) -> R<usize> {
    conn.execute("DELETE FROM site_sessions WHERE label = ?1 AND kind = 'pin'", params![label]).map_err(db)
}

fn clear_pin_limits(conn: &Connection, label: &str) -> R<()> {
    conn.execute(
        "DELETE FROM site_pin_limits WHERE scope = ?1 OR scope LIKE ?2",
        params![site_scope(label), format!("{}%", addr_scope_prefix(label))],
    )
    .map_err(db)?;
    Ok(())
}

/// Make a site private. PIN sessions end, and so does the PIN itself.
pub fn set_private(conn: &Connection, label: &str, by: &str) -> R<SiteVisibility> {
    let label = norm(label)?;
    refuse_reserved(&label)?;
    upsert(conn, &label, by, "mode = 'private', pin_hash = NULL, public_reason = NULL, pin_generation = pin_generation + 1", &[])?;
    end_pin_sessions(conn, &label)?;
    changed();
    effective(conn, &label)
}

/// Make a site public, on purpose: the reason is required. Sessions end (a
/// later move back to private asks everyone again).
pub fn set_public(conn: &Connection, label: &str, reason: &str, by: &str) -> R<SiteVisibility> {
    let label = norm(label)?;
    refuse_reserved(&label)?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("reason_required: say why this site is public (--reason).".into());
    }
    let reason: String = reason.chars().filter(|c| !c.is_control()).take(200).collect();
    upsert(conn, &label, by, "mode = 'public', pin_hash = NULL, public_reason = ?4, pin_generation = pin_generation + 1", &[&reason])?;
    revoke_sessions(conn, &label)?;
    changed();
    effective(conn, &label)
}

/// Is `pin` a PIN K2 accepts? `Ok(true)` = accepted, with the short-PIN
/// warning owed. Refusals never quote it.
pub fn validate_pin(pin: &str) -> R<bool> {
    let n = pin.chars().count();
    if !(PIN_MIN..=PIN_MAX).contains(&n) {
        return Err(format!("pin_length: a PIN is {PIN_MIN} to {PIN_MAX} characters."));
    }
    if pin.chars().any(char::is_control) {
        return Err("pin_characters: a PIN can't hold control characters or line breaks.".into());
    }
    Ok(n < PIN_WARN_BELOW)
}

/// A PIN after [`hash_pin`]. It holds no plaintext and prints nothing.
pub struct PinHash {
    phc: String,
    /// Under [`PIN_WARN_BELOW`] characters: the caller warns.
    pub short: bool,
}

impl std::fmt::Debug for PinHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinHash(..)")
    }
}

/// Check and hash a PIN (argon2id, K2 Connect users' parameters; the gate's
/// [`SiteVisibility::verify_pin`] reads the same form). Slow: call it before
/// taking the database lock.
pub fn hash_pin(pin: &str) -> R<PinHash> {
    let short = validate_pin(pin)?;
    Ok(PinHash { phc: crate::connect_users::hash_password(pin)?, short })
}

/// Turn PIN mode on with this PIN (or change the PIN). The generation goes up
/// and every PIN session of the site ends. `pin_days` is the session life,
/// `None` keeps the stored one.
pub fn set_pin(conn: &Connection, label: &str, pin: &PinHash, pin_days: Option<i64>, by: &str) -> R<SiteVisibility> {
    let label = norm(label)?;
    refuse_reserved(&label)?;
    if let Some(d) = pin_days {
        if !(PIN_DAYS_MIN..=PIN_DAYS_MAX).contains(&d) {
            return Err(format!("pin_days: --pin-days is {PIN_DAYS_MIN} to {PIN_DAYS_MAX}."));
        }
    }
    upsert(
        conn,
        &label,
        by,
        "mode = 'pin', pin_hash = ?4, public_reason = NULL, pin_generation = pin_generation + 1, pin_days = COALESCE(?5, pin_days)",
        &[&pin.phc, &pin_days],
    )?;
    end_pin_sessions(conn, &label)?;
    // A new PIN starts its limits fresh: the old PIN's tries say nothing now.
    clear_pin_limits(conn, &label)?;
    changed();
    effective(conn, &label)
}

/// Mark a site dev or not, with its idle stop (`None` = the default, `Some(0)`
/// = off, else [`IDLE_MIN_SECS`] to [`IDLE_MAX_SECS`]). The mode is untouched;
/// a new row is private. Who may turn the idle stop off is the route's call.
pub fn set_dev(conn: &Connection, label: &str, dev: bool, idle_secs: Option<i64>, by: &str) -> R<SiteVisibility> {
    let label = norm(label)?;
    refuse_reserved(&label)?;
    if let Some(s) = idle_secs {
        if s != 0 && !(IDLE_MIN_SECS..=IDLE_MAX_SECS).contains(&s) {
            return Err("idle_range: --idle (how long an unused dev site runs before K2 stops it) is 5m to 24h, or off.".into());
        }
    }
    upsert(conn, &label, by, "dev = ?4, idle_secs = ?5", &[&(dev as i64), &idle_secs])?;
    changed();
    effective(conn, &label)
}

/// A label was created after the upgrade: write its `private` row now, so no
/// later pass can grandfather it. An existing row stays as it is (a public
/// service whose hostname comes back stays public).
pub fn note_new_label(conn: &Connection, label: &str, by: &str) -> R<()> {
    let label = norm(label)?;
    if !is_covered(&label) {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO site_visibility (label, mode, set_by, set_at) VALUES (?1, 'private', ?2, ?3)",
        params![label, by, now_unix()],
    )
    .map_err(db)?;
    changed();
    Ok(())
}

/// The label's hostname is gone: its row, sessions, codes and limits go, so a
/// new label of that name starts private, not as the old site was.
pub fn forget_label(conn: &Connection, label: &str) -> R<()> {
    let label = norm(label)?;
    conn.execute("DELETE FROM site_visibility WHERE label = ?1", params![label]).map_err(db)?;
    conn.execute("DELETE FROM site_sessions WHERE label = ?1", params![label]).map_err(db)?;
    conn.execute("DELETE FROM site_open_codes WHERE label = ?1", params![label]).map_err(db)?;
    clear_pin_limits(conn, &label)?;
    changed();
    Ok(())
}

fn site_scope(label: &str) -> String {
    format!("site:{label}")
}

fn addr_scope_prefix(label: &str) -> String {
    format!("addr:{label}:")
}

// ── doors ─────────────────────────────────────────────────────────────

/// The refusal code for a door the gate doesn't cover (PV-4).
pub const DOOR_UNSUPPORTED: &str = "visibility_door_unsupported";

/// Why `private` and `pin` can't be promised on this box right now, or `None`.
/// With E2E off the relay ends TLS and the daemon's nested routing is off, so
/// the gate never sees the traffic (vs-live PA-3). `<code>: <plain words>`.
pub fn door_refusal() -> Option<String> {
    let cfg = match crate::tunnel::config::load_without_token() {
        Ok(c) => c,
        Err(e) => return Some(format!("{DOOR_UNSUPPORTED}: the tunnel config can't be read, so K2 can't promise its gate covers this site ({e}).")),
    };
    (!crate::tunnel::config::e2e_enabled(&cfg)).then(|| {
        format!("{DOOR_UNSUPPORTED}: E2E is off on this server, so K2's gate can't see this site's traffic and can't make it private or add a PIN. Turn E2E back on first; until then this site stays as it is.")
    })
}

// ── one-time links ────────────────────────────────────────────────────

fn random_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).iter().map(|x| format!("{x:02x}")).collect()
}

/// A path K2 will redirect to: local to the site, never another host. `None`
/// for anything else (`//host`, `/\host`, control characters, no leading `/`).
pub fn safe_path(raw: &str) -> Option<String> {
    let p = raw.trim();
    let p = if p.is_empty() { "/" } else { p };
    let ok = p.starts_with('/')
        && !p.starts_with("//")
        && p.len() <= 2048
        && !p.chars().any(|c| c.is_control() || c == '\\' || c == ' ')
        // K2's own pages are never a landing place (no redirect loops).
        && !p.starts_with(RESERVED_PATH);
    ok.then(|| p.to_string())
}

/// A minted one-time link's code. The value leaves here once and is never
/// stored. It prints nothing.
pub struct OpenCode {
    pub code: String,
    pub expires_at: i64,
}

impl std::fmt::Debug for OpenCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenCode(expires_at: {})", self.expires_at)
    }
}

/// Mint a one-time link's code for `label` (single use, [`OPEN_CODE_SECS`]).
/// `user` is the login, or [`USER_OWNER_TOKEN`]. The route decides who may
/// ask (`person_only`); this doesn't check.
pub fn mint_open_code(conn: &Connection, label: &str, path: &str, user: &str) -> R<OpenCode> {
    let label = norm(label)?;
    refuse_reserved(&label)?;
    let path = safe_path(path).ok_or("bad_path: the path must be on this site, like / or /some/page.")?;
    let code = random_token();
    let expires_at = now_unix() + OPEN_CODE_SECS;
    conn.execute(
        "INSERT INTO site_open_codes (code_hash, label, user, path, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token_hash(&code), label, user, path, expires_at],
    )
    .map_err(db)?;
    Ok(OpenCode { code, expires_at })
}

/// The address of the one-time link for `code`, `None` while the box has no
/// subdomain.
pub fn open_url(label: &str, code: &str) -> Option<String> {
    let primary = crate::tunnel::subdomains::current().primary.clone();
    (!primary.is_empty()).then(|| {
        format!("https://{label}.{primary}.{}{RESERVED_PATH}open?code={code}", crate::tunnel::config::SUBDOMAIN_HOST)
    })
}

/// A code that checked out: who minted it and where they're going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    pub user: String,
    pub path: String,
}

/// Spend a code on `label`. `None` = unknown, already used, expired, or another
/// label's (a code for another label stays unspent). Atomic: of two tries at
/// once, one wins.
pub fn redeem_open_code(conn: &Connection, label: &str, code: &str) -> R<Option<Redeemed>> {
    let now = now_unix();
    conn.query_row(
        "DELETE FROM site_open_codes WHERE code_hash = ?1 AND label = ?2 AND expires_at > ?3 RETURNING user, path",
        params![token_hash(code), label, now],
        |r| Ok(Redeemed { user: r.get(0)?, path: r.get(1)? }),
    )
    .optional()
    .map_err(db)
}

// ── sessions ──────────────────────────────────────────────────────────

/// How a session came to be: a one-time link, or a right PIN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Open,
    Pin,
}

impl SessionKind {
    fn as_str(self) -> &'static str {
        match self {
            SessionKind::Open => "open",
            SessionKind::Pin => "pin",
        }
    }
}

/// A new session's cookie value (shown once, never stored) and its life.
pub struct NewSession {
    pub token: String,
    pub max_age: i64,
}

impl std::fmt::Debug for NewSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NewSession(max_age: {})", self.max_age)
    }
}

/// Start a session on `label`. A PIN session's life is the row's `pin_days`
/// and it is tied to the row's PIN generation.
pub fn create_session(conn: &Connection, label: &str, kind: SessionKind, user: &str) -> R<NewSession> {
    let vis = effective(conn, label)?;
    let max_age = match kind {
        SessionKind::Open => OPEN_SESSION_SECS,
        SessionKind::Pin => vis.pin_days * 86_400,
    };
    let token = random_token();
    let now = now_unix();
    conn.execute(
        "INSERT INTO site_sessions (id_hash, label, kind, user, pin_generation, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![token_hash(&token), vis.label, kind.as_str(), user, vis.pin_generation, now, now + max_age],
    )
    .map_err(db)?;
    Ok(NewSession { token, max_age })
}

/// A live session, as the gate needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub kind: SessionKind,
    pub user: String,
    pub expires_at: i64,
    pub notice_seen: bool,
}

/// The live session `token` opens on `label`, or `None`: unknown, expired,
/// another label's, or a PIN session from an older PIN. Whether the mode lets
/// it in, and whether its user still may, is the gate's call.
pub fn live_session(conn: &Connection, label: &str, token: &str) -> R<Option<LiveSession>> {
    let now = now_unix();
    let found = conn
        .query_row(
            "SELECT s.kind, s.user, s.expires_at, s.notice_seen, s.pin_generation, v.pin_generation
             FROM site_sessions s LEFT JOIN site_visibility v ON v.label = s.label
             WHERE s.id_hash = ?1 AND s.label = ?2 AND s.expires_at > ?3",
            params![token_hash(token), label, now],
            |r| {
                let kind: String = r.get(0)?;
                Ok((kind, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)? != 0, r.get::<_, i64>(4)?, r.get::<_, Option<i64>>(5)?))
            },
        )
        .optional()
        .map_err(db)?;
    let Some((kind, user, expires_at, notice_seen, session_gen, row_gen)) = found else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "open" => SessionKind::Open,
        "pin" if row_gen == Some(session_gen) => SessionKind::Pin,
        _ => return Ok(None),
    };
    Ok(Some(LiveSession { kind, user, expires_at, notice_seen }))
}

/// The dev notice was shown for this session (DM-4).
pub fn mark_notice_seen(conn: &Connection, label: &str, token: &str) -> R<()> {
    conn.execute(
        "UPDATE site_sessions SET notice_seen = 1 WHERE id_hash = ?1 AND label = ?2",
        params![token_hash(token), label],
    )
    .map_err(db)?;
    changed();
    Ok(())
}

/// One session, as `k2 publish sessions` shows it. Never the id.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub kind: SessionKind,
    pub user: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// The live sessions of `label`, newest first.
pub fn list_sessions(conn: &Connection, label: &str) -> R<Vec<SessionView>> {
    let label = label.trim().to_ascii_lowercase();
    let mut st = conn
        .prepare("SELECT kind, user, created_at, expires_at FROM site_sessions WHERE label = ?1 AND expires_at > ?2 ORDER BY created_at DESC")
        .map_err(db)?;
    let rows = st
        .query_map(params![label, now_unix()], |r| {
            let kind: String = r.get(0)?;
            Ok(SessionView {
                kind: if kind == "pin" { SessionKind::Pin } else { SessionKind::Open },
                user: r.get(1)?,
                created_at: r.get(2)?,
                expires_at: r.get(3)?,
            })
        })
        .map_err(db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
}

/// End every session of `label`, private and PIN. Returns how many.
pub fn revoke_sessions(conn: &Connection, label: &str) -> R<usize> {
    let label = label.trim().to_ascii_lowercase();
    let n = conn.execute("DELETE FROM site_sessions WHERE label = ?1", params![label]).map_err(db)?;
    changed();
    Ok(n)
}

/// Drop what has run out (PA-6: hourly). Returns how many rows went.
pub fn sweep_expired(conn: &Connection) -> R<usize> {
    let now = now_unix();
    let mut n = conn.execute("DELETE FROM site_sessions WHERE expires_at <= ?1", params![now]).map_err(db)?;
    n += conn.execute("DELETE FROM site_open_codes WHERE expires_at <= ?1", params![now]).map_err(db)?;
    n += conn
        .execute(
            "DELETE FROM site_pin_limits WHERE locked_until <= ?1 AND window_start <= ?2",
            params![now, now - PIN_SITE_WINDOW_SECS],
        )
        .map_err(db)?;
    Ok(n)
}

// ── PIN limits (PV-8, kept across restarts: PA-6) ─────────────────────

/// Wrong tries per client address: this many in the window, then a wait.
pub const PIN_ADDR_TRIES: i64 = 5;
pub const PIN_ADDR_WINDOW_SECS: i64 = 15 * 60;
pub const PIN_ADDR_LOCK_SECS: i64 = 15 * 60;
/// Wrong tries per site: this many in the window, then new tries wait.
pub const PIN_SITE_TRIES: i64 = 30;
pub const PIN_SITE_WINDOW_SECS: i64 = 3600;
pub const PIN_SITE_LOCK_SECS: i64 = 3600;

/// May this try go ahead?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinAttempt {
    /// Check the PIN. `site_tripped`: this try locked the site, if it's wrong
    /// (the owner gets the one notice).
    Go { site_tripped: bool },
    /// Locked. `site`: the whole site, not just this address.
    Wait { retry_after: i64, site: bool },
}

fn limit_row(conn: &Connection, scope: &str) -> R<Option<(i64, i64, i64)>> {
    conn.query_row(
        "SELECT fails, window_start, locked_until FROM site_pin_limits WHERE scope = ?1",
        params![scope],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(db)
}

/// Count one more try on `scope`. Returns whether it just locked.
fn limit_bump(conn: &Connection, scope: &str, tries: i64, window: i64, lock: i64, now: i64) -> R<bool> {
    let (fails, start) = match limit_row(conn, scope)? {
        Some((f, s, _)) if now - s < window => (f, s),
        _ => (0, now),
    };
    let fails = fails + 1;
    let locked_until = if fails >= tries { now + lock } else { 0 };
    conn.execute(
        "INSERT INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(scope) DO UPDATE SET fails = excluded.fails, window_start = excluded.window_start, locked_until = excluded.locked_until",
        params![scope, fails, start, locked_until],
    )
    .map_err(db)?;
    Ok(fails == tries)
}

/// Before checking a PIN: refuse a locked address or site, else count the try
/// (counted first, so a burst at once can't pass the limit). `ip` is the
/// address K2 vouches for; `None` (an old relay) means only the site's limit
/// applies. A right PIN gives its try back ([`pin_attempt_right`]).
pub fn pin_attempt_begin(conn: &Connection, label: &str, ip: Option<&str>, now: i64) -> R<PinAttempt> {
    let tx = conn.unchecked_transaction().map_err(db)?;
    let site = site_scope(label);
    let addr = ip.map(|i| format!("{}{i}", addr_scope_prefix(label)));
    if let Some((_, _, until)) = limit_row(&tx, &site)? {
        if until > now {
            return Ok(PinAttempt::Wait { retry_after: until - now, site: true });
        }
    }
    if let Some(a) = &addr {
        if let Some((_, _, until)) = limit_row(&tx, a)? {
            if until > now {
                return Ok(PinAttempt::Wait { retry_after: until - now, site: false });
            }
        }
        limit_bump(&tx, a, PIN_ADDR_TRIES, PIN_ADDR_WINDOW_SECS, PIN_ADDR_LOCK_SECS, now)?;
    }
    let site_tripped = limit_bump(&tx, &site, PIN_SITE_TRIES, PIN_SITE_WINDOW_SECS, PIN_SITE_LOCK_SECS, now)?;
    tx.commit().map_err(db)?;
    Ok(PinAttempt::Go { site_tripped })
}

/// The PIN was right: this address starts over, and the site gets the try back
/// (so a person who knows the PIN never adds to a lock).
pub fn pin_attempt_right(conn: &Connection, label: &str, ip: Option<&str>) -> R<()> {
    if let Some(i) = ip {
        conn.execute("DELETE FROM site_pin_limits WHERE scope = ?1", params![format!("{}{i}", addr_scope_prefix(label))]).map_err(db)?;
    }
    conn.execute(
        "UPDATE site_pin_limits SET fails = MAX(fails - 1, 0), locked_until = CASE WHEN fails - 1 < ?2 THEN 0 ELSE locked_until END WHERE scope = ?1",
        params![site_scope(label), PIN_SITE_TRIES],
    )
    .map_err(db)?;
    Ok(())
}

// ── the move from "everything is public" (PA-13) ──────────────────────

const META_GRANDFATHER: &str = "grandfather";
const META_GRANDFATHER_PRIMARY: &str = "grandfather_primary";
const STATE_PENDING: &str = "pending";
const STATE_DONE: &str = "done";

fn meta(conn: &Connection, key: &str) -> R<Option<String>> {
    conn.query_row("SELECT value FROM site_visibility_meta WHERE key = ?1", params![key], |r| r.get(0)).optional().map_err(db)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> R<()> {
    conn.execute(
        "INSERT INTO site_visibility_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(db)?;
    Ok(())
}

const META_UPGRADE_CARD: &str = "upgrade_card";

/// Is the one card about sites public from before 0.46.7 (PV-11) still owed?
/// Not while the pass over the routing map is: only that settles which they are.
pub fn upgrade_card_due(conn: &Connection) -> R<bool> {
    Ok(!grandfather_pending(conn)? && meta(conn, META_UPGRADE_CARD)?.as_deref() != Some(STATE_DONE))
}

/// The upgrade card is on a Thread (or there was nothing to say).
pub fn mark_upgrade_card_done(conn: &Connection) -> R<()> {
    set_meta(conn, META_UPGRADE_CARD, STATE_DONE)
}

/// Is the upgrade's pass over the routing map still owed?
pub fn grandfather_pending(conn: &Connection) -> R<bool> {
    Ok(meta(conn, META_GRANDFATHER)?.as_deref() == Some(STATE_PENDING))
}

/// Boot, with the subdomain in `tunnel.json`: a box with no tunnel owes
/// nothing (it has no public sites, and an account it pairs with later isn't
/// this box's past); a box with one owes the pass for that subdomain only.
pub fn settle_grandfather_at_boot(conn: &Connection, subdomain: &str) -> R<()> {
    if !grandfather_pending(conn)? || meta(conn, META_GRANDFATHER_PRIMARY)?.is_some() {
        return Ok(());
    }
    let subdomain = subdomain.trim().to_ascii_lowercase();
    if subdomain.is_empty() {
        set_meta(conn, META_GRANDFATHER, STATE_DONE)
    } else {
        set_meta(conn, META_GRANDFATHER_PRIMARY, &subdomain)
    }
}

/// While the pass is owed: every covered label in this routing map with no row
/// becomes `public` (reason [`REASON_BEFORE`]). Run it **before** the map
/// routes anything. `fetched` = the control plane's own list (not the copy on
/// disk), which settles the pass. A map for another subdomain changes nothing.
/// Returns how many rows it wrote.
pub fn grandfather_map<'a>(
    conn: &Connection,
    primary: &str,
    labels: impl IntoIterator<Item = &'a str>,
    fetched: bool,
) -> R<usize> {
    if !grandfather_pending(conn)? {
        return Ok(0);
    }
    if let Some(owed) = meta(conn, META_GRANDFATHER_PRIMARY)? {
        if !owed.eq_ignore_ascii_case(primary.trim()) {
            return Ok(0);
        }
    }
    let now = now_unix();
    let tx = conn.unchecked_transaction().map_err(db)?;
    let mut n = 0;
    for label in labels {
        let label = label.trim().to_ascii_lowercase();
        if label.is_empty() || !is_covered(&label) {
            continue;
        }
        n += tx
            .execute(
                "INSERT OR IGNORE INTO site_visibility (label, mode, set_by, set_at, public_reason) VALUES (?1, 'public', ?2, ?3, ?4)",
                params![label, SET_BY_UPGRADE, now, REASON_BEFORE],
            )
            .map_err(db)?;
    }
    if fetched {
        set_meta(&tx, META_GRANDFATHER, STATE_DONE)?;
    }
    tx.commit().map_err(db)?;
    if n > 0 {
        changed();
    }
    Ok(n)
}

/// The sites the upgrade kept public that nobody has decided on yet: the list
/// behind the owner's one notice card (PV-11).
pub fn grandfathered_public(conn: &Connection) -> R<Vec<SiteVisibility>> {
    let mut st = conn
        .prepare("SELECT * FROM site_visibility WHERE mode = 'public' AND set_by = ?1 ORDER BY label")
        .map_err(db)?;
    let rows = st.query_map(params![SET_BY_UPGRADE], row).map_err(db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
}

#[cfg(test)]
#[path = "site_visibility_tests.rs"]
mod tests;
