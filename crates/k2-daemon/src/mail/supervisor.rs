//! Stalwart sidecar supervisor (mail slice S1).
//!
//! The ONLY module that knows Stalwart exists as a process (PRD §4.1).
//! Boundary rules are LICENSE rules (pre-mortem #2): Stalwart is never
//! linked, vendored, or patched — the supervisor downloads the PINNED
//! upstream release tarball (sha256-verified against constants baked
//! in at build time), runs it under systemd with K2's hardening
//! drop-in (§10), and drives it exclusively over its HTTP management
//! API ([`super::jmap`]).
//!
//! BOOTSTRAP MODEL (✔ LIVE-VERIFIED v0.16.10, 2026-07-10): started
//! with `--config <path>` and the file ABSENT, Stalwart enters
//! bootstrap mode (ephemeral store, plain-HTTP `http-recovery`
//! listener on :8080). The DETERMINISTIC credential path is
//! `STALWART_RECOVERY_ADMIN=admin:<password>` in the unit environment
//! — no journal scraping. Setup completes with ONE `x:Bootstrap/set`
//! (the only settable object in bootstrap mode): it writes the flat
//! DataStore JSON (`{"@type":"RocksDb","path":…}`) to the config path
//! — which must be WRITABLE BY THE stalwart USER (chown + the
//! ReadWritePaths drop-in line; the first live run failed exactly
//! there) — and returns the PROVISIONED admin (`admin@<domain>` + a
//! fresh random secret) in the reply. After a restart the server runs
//! in normal mode; the recovery env credential AND the bootstrap
//! principal `admin` remain valid in normal mode (verified) until
//! `recovery-off` strips the env, rotates `admin`, and rotates the
//! provisioned `admin@domain` secret before the final restart.
//!
//! The enable flow is an idempotent, RESUMABLE state machine: each
//! step records completion in `mail_server.enable_progress_json`
//! (0073) — a crashed/interrupted enable resumes instead of
//! re-downloading or re-minting; GET /cli/mail/status polls the same
//! JSON (the house persisted-step + poll pattern, clone/update
//! precedent). All system effects ride [`super::sysops::SystemOps`],
//! all management calls ride [`BootstrapApi`], all secrets ride
//! [`super::secrets::SecretStore`] — the whole machine unit-tests on
//! macOS as a sequence assertion with zero side effects.

use std::sync::atomic::{AtomicBool, Ordering};

use super::helper::{self, HelperState};
use super::jmap::StalwartClient;
use super::secrets::{generate_secret, FileSecretStore, SecretStore};
use super::sysops::{RealSystemOps, SystemOps};

// ── The pin, the on-box layout, the unit + drop-in ──────────────────────
//
// One table and one set of templates for the daemon AND the root helper:
// they live in `k2-root-proto` (prd-k2-root-helper-v1 §5.1), so a daemon
// and a helper of one release can never disagree. Re-exported here so the
// supervisor, upgrade and the legacy `k2-mail-helper` keep their paths.

pub use k2_root_proto::paths::{
    STALWART_BIN, STALWART_BIN_STAGING, STALWART_CONFIG, STALWART_CONFIG_DIR, STALWART_DATA_DIR,
    STALWART_DROPIN_DIR, STALWART_DROPIN_PATH, STALWART_LOG_DIR, STALWART_SNAPSHOT_DIR,
    STALWART_SNAPSHOT_MARKER, STALWART_UNIT, STALWART_UNIT_PATH, STALWART_USER,
};
pub use k2_root_proto::templates::{hardening_dropin, systemd_unit};

pub use k2_root_proto::stalwart::{
    artifact_for_arch, checksum_is_placeholder, previous_artifact_for_arch, upgrade_from_artifact,
    upgrade_from_release, upgrade_from_versions, StalwartArtifact, StalwartRelease,
    STALWART_OLDER_SHA256, STALWART_OLDER_VERSION, STALWART_PINNED_VERSION,
    STALWART_PREVIOUS_SHA256, STALWART_PREVIOUS_VERSION, STALWART_SHA256, STALWART_UPGRADE_FROM,
};
use k2_core::brand::CLI_NAME;
use k2_core::cli_name;

/// Enable fetches the pin from the Alakazam fork release (a mirror of
/// the upstream tarball); K2 does not rebuild Stalwart.
pub fn tarball_url(triple: &str) -> String {
    tarball_url_for(STALWART_PINNED_VERSION, triple)
}

/// The fork release tarball for `version` (pin or previous).
pub fn tarball_url_for(version: &str, triple: &str) -> String {
    format!(
        "https://github.com/Alakazam-211/stalwart/releases/download/v{version}/stalwart-{triple}.tar.gz"
    )
}

// ── On-disk / on-box layout (paths re-exported above) ───────────────────

/// Stalwart's bootstrap-mode listener (plain HTTP `http-recovery` on
/// :8080 — ✔ live-verified). After Bootstrap/set + restart the
/// DEFAULT registry listeners still include a world-bound plain-HTTP
/// `http` listener on :8080; `bind_setup_http_loopback` retargets that
/// bind to [`STALWART_SETUP_BIND`] so the remaining bootstrap window
/// is loopback-only. The port-plan step then retargets to
/// [`STALWART_MGMT_URL`] and the final restart applies it.
/// Public `/login` on :443 is a follow-up (do not close here).
pub const STALWART_SETUP_URL: &str = "http://127.0.0.1:8080";

/// Loopback bind for the bootstrap/recovery HTTP listener when we
/// still need :8080. Never `*:8080` / `[::]:8080`.
pub const STALWART_SETUP_BIND: &str = "127.0.0.1:8080";

/// The PERMANENT management endpoint the daemon talks to: the default
/// `http` listener retargeted to 127.0.0.1 only.
///
/// TLS DECISION (S1, foundation flagged): the mgmt path uses plain
/// HTTP on the loopback, both during bootstrap (Stalwart's own :8080
/// listener is plain HTTP) and permanently (this listener).
/// Rationale: loopback traffic never leaves the kernel, so TLS adds
/// no confidentiality against an attacker who couldn't already read
/// process memory — while a self-signed-cert mgmt listener would force
/// `danger_accept_invalid_certs` into the client during the pre-ACME
/// window, a strictly worse posture. The public :443/8443 HTTPS
/// listener (JMAP for real mail clients + ACME) is separate and keeps
/// full TLS. Pre-mortem #13 (mgmt binds localhost-only) holds either
/// way and the S6 doctor port-scans to assert it.
pub const STALWART_MGMT_URL: &str = "http://127.0.0.1:8180";

// ── Capability gate ─────────────────────────────────────────────────────

/// The single capability gate for the whole mail family (D3 +
/// pre-mortem #15): V1 runs on **Linux deployments only**.
///
/// RUNTIME `cfg!`, deliberately NOT a compile-time `#[cfg]` on the
/// module — the module compiles and unit-tests on macOS, and the
/// `/cli/mail/status` route reports `supported: false` so the Mac
/// Settings→Email page renders its example-only state from the
/// DAEMON's answer (never from `navigator.platform`: a Mac app driving
/// a remote Linux daemon must see the REAL page).
pub fn mail_supported() -> bool {
    cfg!(target_os = "linux")
}

/// The default domain the mail hostname implies — Stalwart's guided
/// setup REQUIRES one (it hosts the provisioned `admin@<domain>`
/// account). Deterministic registrable-domain guess: strip the
/// left-most label when the hostname has ≥3 labels
/// (`mail.acme.dev` → `acme.dev`), else the hostname itself. The
/// owner's real mail domains arrive later via `k2 mail domain add`
/// (which reuses this domain when it matches).
pub fn default_domain_for(hostname: &str) -> String {
    let labels: Vec<&str> = hostname.split('.').collect();
    if labels.len() >= 3 {
        labels[1..].join(".")
    } else {
        hostname.to_string()
    }
}

// ── Management-API seam for the bootstrap sequence ──────────────────────

/// The permanent admin account Stalwart provisions during
/// `x:Bootstrap/set` (returned ONCE in the set reply — the secret is
/// vaulted immediately and never logged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentials {
    /// `admin@<default domain>` (basic-auth username).
    pub username: String,
    pub secret: String,
}

/// The typed management operations the enable machine performs against
/// Stalwart, as a seam: the real impl is [`super::jmap::StalwartBootstrap`]
/// (HTTP against the loopback listeners); tests inject a recording fake.
pub trait BootstrapApi: Send {
    /// Basic-auth session against `base_url` (+ session-document
    /// discovery — the probe doubles as the credential check).
    fn authenticate(
        &mut self,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String>;
    /// Bearer session with the minted ApiKey. Field report: basic auth can
    /// be gone (provisioned admin missing, recovery secret deleted)
    /// while the api-key from setup still opens :8080.
    fn authenticate_bearer(&mut self, base_url: &str, api_key: &str) -> Result<(), String>;
    /// Complete Stalwart's guided setup (bootstrap mode only): ONE
    /// `x:Bootstrap/set` carrying hostname, default domain, the
    /// RocksDB DataStore, DKIM generation on, and the ACME choice
    /// (`request_tls_certificate` = the §5.3 `tls-alpn` plan). Returns
    /// the PROVISIONED admin credentials from the reply.
    fn complete_bootstrap(
        &mut self,
        hostname: &str,
        default_domain: &str,
        request_tls_certificate: bool,
    ) -> Result<AdminCredentials, String>;
    /// L2: after disable→enable with a **new** hostname (store already
    /// initialized — not the healthy alreadyEnabled no-op), patch
    /// Stalwart `x:SystemSettings` `defaultHostname` so the SMTP banner
    /// follows. Not re-bootstrap, not SIGTERM wipe.
    fn set_server_hostname(&mut self, hostname: &str) -> Result<(), String>;
    /// Bind the bootstrap/recovery HTTP listener to
    /// [`STALWART_SETUP_BIND`] (`127.0.0.1:8080`), never `*:8080`.
    /// Called while we still need :8080 during bootstrap; sockets
    /// move on the next restart. Does not close public `/login` on
    /// :443 (follow-up).
    fn bind_setup_http_loopback(&mut self) -> Result<(), String>;
    /// Apply the §5.3 port plan (NORMAL mode): destroy the
    /// IMAP/POP3/ManageSieve listeners (§10), retarget the :8080 http
    /// listener to the loopback mgmt bind, bind HTTPS per plan, add
    /// the :587 submission listener. Sockets move on the final
    /// restart.
    fn configure_listeners(&mut self, port_plan: &str) -> Result<(), String>;
    /// CAL43 (tls-alpn plan only): lock the mail hostname's Domain
    /// `certificateManagement.subjectAlternativeNames` to exactly
    /// `[hostname]`, keeping its `acmeProviderId`. Guided setup leaves
    /// that list empty, which Stalwart reads as "every autoconfig/
    /// mta-sts/MX name in one ACME order" (C24). Idempotent.
    fn lock_acme_cert_names(&mut self, hostname: &str) -> Result<(), String>;
    /// Create the admin-role `k2-daemon` service account in the
    /// default domain; returns its account id.
    fn create_service_account(&mut self, default_domain: &str) -> Result<String, String>;
    /// Mint the loopback-allowlisted ApiKey on the service account;
    /// returns the SECRET (shown once by Stalwart).
    fn mint_api_key(&mut self, account_id: &str) -> Result<String, String>;
    /// C20/L12: rotate a Stalwart admin-class password (vault + API).
    /// `username` is `admin` (bootstrap recovery principal) or
    /// `admin@domain` (provisioned).
    fn rotate_admin_secret(
        &mut self,
        username: &str,
        current: &str,
        new_secret: &str,
    ) -> Result<(), String>;
    /// True when this process already has a management session from
    /// an earlier enable step. recovery-off must not replace it.
    fn has_session(&self) -> bool {
        false
    }
}

// ── mail_server row helpers ─────────────────────────────────────────────

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Upsert the singleton row into `installing` at enable start; clears
/// stale last_error. Keeps any prior progress JSON (that's the resume
/// state).
fn ensure_installing_row(hostname: &str, port_plan: &str) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_server (id, status, pinned_version, hostname, port_plan, updated_at) \
         VALUES (1, 'installing', ?1, ?2, ?3, ?4) \
         ON CONFLICT(id) DO UPDATE SET status = 'installing', pinned_version = ?1, \
         hostname = ?2, port_plan = ?3, last_error = NULL, updated_at = ?4",
        rusqlite::params![STALWART_PINNED_VERSION, hostname, port_plan, now_secs()],
    )
    .map_err(|e| format!("mail_server upsert: {e}"))?;
    Ok(())
}

pub(crate) fn set_status(status: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE mail_server SET status = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![status, now_secs()],
    );
}

pub(crate) fn set_last_error(err: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE mail_server SET last_error = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![err, now_secs()],
    );
}

pub(crate) fn row_field(col: &str) -> Option<String> {
    // `col` is always a compile-time constant from this module — never
    // caller input — so the format! is not an injection surface.
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        &format!("SELECT {col} FROM mail_server WHERE id = 1"),
        [],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

pub(crate) fn set_row_field(col: &str, value: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        &format!("UPDATE mail_server SET {col} = ?1, updated_at = ?2 WHERE id = 1"),
        rusqlite::params![value, now_secs()],
    );
}

pub(crate) fn current_status() -> Option<String> {
    row_field("status")
}

/// SQLite `mail_server.status` overlaid with systemd `is-active`.
///
/// `state` is never `running` unless the unit is `active` (H1). SQLite is
/// the cache. `consistent` answers ONE question: does the SQLite row agree
/// with systemd? It is NOT the HTTP envelope `ok` (the status read itself
/// succeeded either way) — fb449bc1 put this verdict in `ok` and the CLI's
/// generic caller aborted exactly when there was a disagreement to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledStatus {
    pub state: String,
    pub consistent: bool,
    pub last_error: Option<String>,
}

/// Combine the singleton row with `systemctl is-active stalwart`.
///
/// `unit_state` is the trimmed `is-active` stdout (`active` / `inactive` /
/// `failed` / …). Empty means unknown (treated as not active). Callers that
/// have no Linux unit (Mac daemon, tests without a seam) skip this and serve
/// SQLite as-is.
///
/// | row                        | unit active                  | unit not active                    |
/// |----------------------------|------------------------------|------------------------------------|
/// | `not-installed`/`installing` | consistent, row as-is      | consistent, row as-is              |
/// | `disabled`                 | INCONSISTENT (running while disabled) | consistent                |
/// | `error` / `stopped`        | INCONSISTENT (unit is up)    | consistent                         |
/// | `running`/`degraded`/other | consistent                   | INCONSISTENT → `stopped`/`error`   |
pub fn reconcile_reported_status(
    sqlite_state: &str,
    last_error: Option<&str>,
    unit_state: &str,
) -> ReconciledStatus {
    let unit = if unit_state.trim().is_empty() {
        "unknown"
    } else {
        unit_state.trim()
    };
    let unit_active = unit == "active";
    let keep_err = last_error.map(str::to_string);
    let systemd_err = format!("systemd reports the stalwart unit is '{unit}'");

    // Enable in progress / nothing installed: don't invent a unit.
    if matches!(sqlite_state, "not-installed" | "installing") {
        return ReconciledStatus {
            state: sqlite_state.to_string(),
            consistent: true,
            last_error: keep_err,
        };
    }

    // `disable` ran `systemctl disable --now`: a down unit is agreement.
    // A running unit means inbound mail is still being accepted while K2
    // believes hostmail is off.
    if sqlite_state == "disabled" {
        if unit_active {
            return ReconciledStatus {
                state: "disabled".into(),
                consistent: false,
                last_error: Some(
                    "systemd reports the stalwart unit is 'active' while hostmail is disabled"
                        .to_string(),
                ),
            };
        }
        return ReconciledStatus {
            state: "disabled".into(),
            consistent: true,
            last_error: keep_err,
        };
    }

    // The row already says the server is not serving. A down unit agrees
    // (keep the recorded cause, else name systemd); an active unit means
    // the row is stale.
    if matches!(sqlite_state, "error" | "stopped") {
        if unit_active {
            return ReconciledStatus {
                state: sqlite_state.to_string(),
                consistent: false,
                last_error: keep_err.or(Some(format!(
                    "systemd reports the stalwart unit is 'active' while hostmail status is '{sqlite_state}'"
                ))),
            };
        }
        return ReconciledStatus {
            state: sqlite_state.to_string(),
            consistent: true,
            last_error: keep_err.or(Some(systemd_err)),
        };
    }

    // The row claims (some level of) serving: systemd must agree (H1/H2).
    if !unit_active {
        let state = if unit == "failed" { "error" } else { "stopped" };
        return ReconciledStatus {
            state: state.into(),
            consistent: false,
            last_error: Some(systemd_err),
        };
    }

    ReconciledStatus {
        state: sqlite_state.to_string(),
        consistent: true,
        last_error: keep_err,
    }
}

/// True only when the row says running AND systemd says the unit is active.
/// SQLite `running` with an inactive unit is a stale cache — not alreadyEnabled (H3).
/// Test predicate. Production enable uses [`is_already_enabled`].
#[cfg(test)]
pub fn is_already_enabled_with(sqlite_status: Option<&str>, unit_state: &str) -> bool {
    sqlite_status == Some("running") && unit_state.trim() == "active"
}

/// Live `systemctl is-active stalwart` (Linux). Tests inject via
/// [`set_test_unit_state`] so they never talk to a real systemd.
pub fn stalwart_unit_state() -> String {
    #[cfg(test)]
    {
        return test_unit_state();
    }
    #[cfg(not(test))]
    {
        RealSystemOps.systemctl_query(&["is-active", STALWART_UNIT])
    }
}

/// Restart the Stalwart unit so listeners re-read Certificate objects —
/// the fallback when `ReloadTlsCertificates` failed after a cert plant.
/// Rides the one restart chain ([`super::imap_listeners::restart_once`]):
/// the mail helper when it is installed, else `sudo -n
/// /usr/bin/systemctl restart stalwart` (boxes such as akzm have no
/// helper, but their sudoers allows systemctl). No open door is a loud
/// error. Not `hostmail disable`.
pub fn restart_stalwart_to_reload_tls() -> Result<(), String> {
    // Never start Stalwart under a running upgrade (it stops Stalwart to
    // snapshot the store). The upgrade's own start loads the new cert.
    if upgrade_running().load(Ordering::SeqCst) {
        return Err(UPGRADE_RUNNING_HINT.to_string());
    }
    #[cfg(test)]
    {
        // Tests never probe sudo or touch systemd; the door chain is
        // covered by `restart_to_reload_tls_with` over fake doors.
        return Ok(());
    }
    #[cfg(not(test))]
    {
        let mut doors = super::imap_listeners::LiveDoors { why: "TLS reload" };
        restart_to_reload_tls_with(&mut doors).map(|_| ())
    }
}

/// [`restart_stalwart_to_reload_tls`] over injected doors: helper, then
/// plain sudo; neither open = Err naming both fixes (the certificate is
/// stored and loads on Stalwart's next restart).
pub fn restart_to_reload_tls_with(
    doors: &mut dyn super::imap_listeners::RestartDoors,
) -> Result<super::imap_listeners::RestartPath, String> {
    use super::imap_listeners::{restart_once, RestartReport, SYSTEMCTL_PATH};
    match restart_once(doors)? {
        RestartReport::Restarted(path) => Ok(path),
        RestartReport::NotRestarted { helper: state } => Err(format!(
            "no door can restart Stalwart to load the certificate (mail helper: {}; \
             `sudo -n {SYSTEMCTL_PATH} restart stalwart` is not allowed). As root: \
             systemctl restart stalwart — or let the daemon do it: {}",
            state.as_str(),
            helper::install_command()
        )),
    }
}

/// `systemctl restart stalwart`, then wait (bounded) for the unit to be
/// active again. Stored listener changes only bind on a restart. Not
/// `hostmail disable`/`enable`. Tests never touch systemd.
pub fn restart_stalwart_and_wait(why: &str) -> Result<(), String> {
    #[cfg(test)]
    {
        let _ = why;
        return Ok(());
    }
    #[cfg(not(test))]
    {
        restart_stalwart_and_wait_with(&RealSystemOps, why)
    }
}

/// [`restart_stalwart_and_wait`] over injected ops. The restart rides the
/// mail helper's `systemctl restart stalwart` verb, so a box without the
/// helper stops here with the install command (no sudo attempt).
pub fn restart_stalwart_and_wait_with(ops: &dyn SystemOps, why: &str) -> Result<(), String> {
    if let Some(msg) = helper::unavailable_message(ops.mail_helper_state()) {
        return Err(msg);
    }
    ops.systemctl(&["restart", STALWART_UNIT]).map(|_| ())?;
    wait_stalwart_active_with(ops, why)
}

/// After a `systemctl restart stalwart` (by any door): wait, bounded,
/// for the unit to be active again. `is-active` is a non-root query.
pub fn wait_stalwart_active_with(ops: &dyn SystemOps, why: &str) -> Result<(), String> {
    // Listeners come back after the process is up.
    for _ in 0..30 {
        if ops.systemctl_query(&["is-active", STALWART_UNIT]).trim() == "active" {
            ops.sleep_ms(400);
            return Ok(());
        }
        ops.sleep_ms(200);
    }
    Err(format!(
        "stalwart unit did not become active after {why} restart"
    ))
}

/// Fresh probe of the mail root door (enable, restart, boot reconcile).
pub fn mail_helper_state() -> HelperState {
    #[cfg(test)]
    {
        TEST_HELPER_STATE.with(|c| c.borrow().unwrap_or(HelperState::Installed))
    }
    #[cfg(not(test))]
    {
        RealSystemOps.mail_helper_state()
    }
}

/// The legacy `k2-mail-helper` alone (root-helper status shows it next to
/// the root helper). Never probes in tests.
pub fn mail_helper_legacy_state() -> HelperState {
    if cfg!(test) || !mail_supported() {
        return HelperState::Missing;
    }
    helper::probe_state()
}

/// [`mail_helper_state`] cached for 30 s — for `GET /cli/mail/status`,
/// which the Settings page polls (each probe is a `sudo -n -l`).
pub fn mail_helper_state_cached() -> HelperState {
    #[cfg(test)]
    {
        mail_helper_state()
    }
    #[cfg(not(test))]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, HelperState)>> = Mutex::new(None);
        let mut slot = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, state)) = *slot {
            if at.elapsed() < Duration::from_secs(30) {
                return state;
            }
        }
        let state = mail_helper_state();
        *slot = Some((Instant::now(), state));
        state
    }
}

/// Test seam: inject the helper probe (default `Installed`).
#[cfg(test)]
pub(crate) fn set_test_helper_state(state: Option<HelperState>) {
    TEST_HELPER_STATE.with(|c| *c.borrow_mut() = state);
}

/// True when a previous `hostmail enable` ran to completion (guided
/// setup wrote config.json AND the final restart step is marked).
pub(crate) fn enable_completed() -> bool {
    is_store_initialized() && step_is_done("restart")
}

/// Steady-state management client from the singleton row (`api_url` +
/// the vaulted ApiKey). Unlike `domains::engine_from_db` this does not
/// require the cached status to be `running` — a boot-time caller may
/// still see the previous daemon's `stopped`.
pub(crate) fn mgmt_client_from_row() -> Result<StalwartClient, String> {
    let api_url = row_field("api_url").ok_or("no api_url recorded")?;
    let key_ref = row_field("api_key_ref").ok_or("no api_key_ref recorded")?;
    let key = FileSecretStore::default()
        .resolve(&key_ref)?
        .ok_or("api key missing from the mail secret store")?;
    Ok(StalwartClient::new(api_url, key))
}

pub fn is_already_enabled() -> bool {
    let unit = stalwart_unit_state();
    if unit.trim() != "active" {
        return false;
    }
    // C19 + a field report: sqlite running / config.json is NOT enough —
    // recovery-off can still be pending (401 on :8080) with ports up.
    // Healthy no-op only after the final restart step.
    is_store_initialized() && step_is_done("restart")
}

/// Guided setup wrote config.json but the final restart step is not
/// done (a customer box: recovery-off 401, ports already up). Resume must skip
/// preflight — "no MTA on :25" would hard-stop our own listeners.
pub fn is_enable_resume() -> bool {
    is_store_initialized() && !step_is_done("restart")
}

/// Port plan from the singleton row (resume without a fresh preflight).
pub fn stored_port_plan() -> Option<String> {
    row_field("port_plan")
}

/// True when guided setup has written config.json (normal mode).
/// Bootstrap's ephemeral `{STALWART_DATA_DIR}/data` is not enough.
/// `/etc/stalwart` is Stalwart's alone (0750), so when the daemon user may
/// not look, root answers (`mail store-state`, protocol 17).
pub fn is_store_initialized() -> bool {
    #[cfg(test)]
    {
        if let Some(v) = TEST_STORE_READY.with(|c| *c.borrow()) {
            return v;
        }
    }
    match std::fs::metadata(STALWART_CONFIG) {
        Ok(m) => m.is_file(),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            super::sysops::exists_as_root(STALWART_CONFIG).unwrap_or(false)
        }
        Err(_) => false,
    }
}

/// The step mark for "Stalwart's tree is Stalwart's alone": its config,
/// data and log dirs `0750`, files without group write or any bit for
/// others, and `UMask=0027` in the hardening drop-in (0.46.14).
pub const STORE_MODES_STEP: &str = "store-modes";

/// Stalwart's config, data and log dirs, in the order they are tightened.
pub const STALWART_TREE_DIRS: [&str; 3] = [STALWART_CONFIG_DIR, STALWART_DATA_DIR, STALWART_LOG_DIR];

/// Existing installs (made before 0.46.14, or while the root helper was
/// older): bring Stalwart's tree to the [`STORE_MODES_STEP`] modes, once.
///
/// Runs only on a completed install and only with a helper that serves
/// `mail tighten` (protocol 17); otherwise it waits for a later pass.
/// `mail tighten` on each dir (only clears bits; a second run changes
/// nothing), then the hardening drop-in is rewritten (now with
/// `UMask=0027`) and systemd reloaded. **Stalwart is not restarted**: the
/// 0750 dirs already keep everyone else out, and the umask applies from
/// Stalwart's next start (an upgrade, a certificate reload, a reboot).
/// `Ok(true)` = it ran now; `Ok(false)` = done before, or not yet possible.
pub fn ensure_store_modes_with(ops: &dyn SystemOps) -> Result<bool, String> {
    if step_is_done(STORE_MODES_STEP) || !enable_completed() || !ops.store_modes_supported() {
        return Ok(false);
    }
    for dir in STALWART_TREE_DIRS {
        ops.tighten_store(dir)?;
    }
    ops.write_file(STALWART_DROPIN_PATH, hardening_dropin().as_bytes(), 0o644)?;
    ops.systemctl(&["daemon-reload"])?;
    mark_step(STORE_MODES_STEP);
    Ok(true)
}

/// [`ensure_store_modes_with`] on this box, from the health loop. Holds
/// the enable latch (an enable or upgrade running means "later").
fn ensure_store_modes_live() {
    if upgrade_running().load(Ordering::SeqCst) || !try_begin_enable() {
        return;
    }
    let r = ensure_store_modes_with(&RealSystemOps);
    end_enable();
    match r {
        Ok(true) => k2_core::log_debug!(
            "[mail/supervisor] Stalwart's config, data and log dirs are now Stalwart's alone \
             (0750, UMask=0027 from its next start)"
        ),
        Ok(false) => {}
        Err(e) => k2_core::log_debug!("[mail/supervisor] tightening Stalwart's dirs: {e}"),
    }
}

/// C29: stamp a fresh lastNoopAt so later enables aren't frozen unix `at`s.
pub fn stamp_enable_noop() {
    let mut p = progress_load();
    p["lastNoopAt"] = serde_json::json!(now_secs());
    p["current"] = serde_json::json!("alreadyEnabled");
    progress_save(&p);
}

/// Live TLS probe result for [`tls_cert_status`]. Tests inject a fixture
/// so status never talks to the network.
#[derive(Clone, Debug)]
enum TlsProbe {
    /// No TCP connect or TLS handshake (timeout, refused, empty host).
    Missing,
    /// Handshake completed far enough to capture the leaf DER. The
    /// verifier **accepts** rcgen so we can detect it.
    Handshake { leaf_der: Vec<u8> },
}

const TLS_CERT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The TLS ports the live probe reads: HTTPS/JMAP (443), SMTP submission
/// over implicit TLS (465) and IMAPS (993). 465 and 993 are always
/// Stalwart's own listeners; 443 may be fronted by something else, so it
/// alone never proves what mail clients see (0.46.1 field report: status
/// said `issued` while 465/993 served the rcgen fallback).
const MAIL_TLS_PROBE_PORTS: [u16; 3] = [443, 465, 993];

/// C24 / L1: cert state on `status` without `--health`. Probe the live
/// TLS listeners on `{hostname}` (443, 465, 993 — in parallel, 2 s cap)
/// with SNI = the mail hostname and a verifier that **captures** the leaf
/// and accepts rcgen, so a serving self-signed cert is never reported as
/// `missing` or `issued`. The worst answering port decides `state`; each
/// port's own verdict rides `ports`. `issued` means a CA-issued leaf that
/// covers the mail hostname and has not expired — nothing is inferred
/// from files on disk. ACME is the mail hostname only — optional names
/// are not in the issuance request.
pub fn tls_cert_status(hostname: Option<&str>) -> serde_json::Value {
    let host = hostname
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    #[cfg(test)]
    {
        let probes = TEST_TLS_PROBE
            .with(|c| c.borrow().clone())
            .unwrap_or_else(|| vec![(443, TlsProbe::Missing)]);
        return tls_cert_json(host, &probes, now_secs());
    }
    #[cfg(not(test))]
    {
        let probes = probe_live_tls_ports(host, &MAIL_TLS_PROBE_PORTS);
        tls_cert_json(host, &probes, now_secs())
    }
}

/// One port's verdict on a captured probe (pure).
struct PortVerdict {
    state: &'static str,
    self_signed: bool,
    names: Vec<String>,
    expires_at: serde_json::Value,
    not_before: serde_json::Value,
}

/// Worse verdicts rank higher: the overall `state` is the worst port.
pub(crate) fn verdict_rank(state: &str) -> u8 {
    match state {
        "self-signed" => 5,
        "mismatch" => 4,
        "expired" => 3,
        "unknown" => 2,
        "issued" => 1,
        _ => 0, // missing
    }
}

fn port_verdict(host: &str, probe: &TlsProbe, now: i64) -> PortVerdict {
    let missing = || PortVerdict {
        state: "missing",
        self_signed: false,
        names: Vec::new(),
        expires_at: serde_json::Value::Null,
        not_before: serde_json::Value::Null,
    };
    let TlsProbe::Handshake { leaf_der } = probe else {
        return missing();
    };
    let Some(leaf) = parse_captured_leaf(leaf_der) else {
        // A leaf was served but it does not parse: never claim `issued`.
        return PortVerdict { state: "unknown", ..missing() };
    };
    let self_signed = looks_rcgen_or_self_signed(&leaf.issuer, &leaf.subject);
    let state = if self_signed {
        "self-signed"
    } else if !host.is_empty() && !leaf_covers_host(&leaf.dns_sans, host) {
        "mismatch"
    } else if leaf.not_after < now {
        "expired"
    } else {
        "issued"
    };
    PortVerdict {
        state,
        self_signed,
        names: leaf.dns_sans,
        expires_at: serde_json::json!(leaf.not_after),
        not_before: serde_json::json!(leaf.not_before),
    }
}

fn tls_cert_json(host: &str, probes: &[(u16, TlsProbe)], now: i64) -> serde_json::Value {
    let host_v = if host.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(host)
    };
    // 0.45.1 A2: `names` are the DNS SANs of the leaf actually served —
    // never assumed. No certificate served = no names (`host` stays set).
    let mut ports = Vec::new();
    let mut worst: Option<PortVerdict> = None;
    for (port, probe) in probes {
        let v = port_verdict(host, probe, now);
        ports.push(serde_json::json!({
            "port": port,
            "state": v.state,
            "selfSigned": v.self_signed,
            "names": v.names,
            "expiresAt": v.expires_at,
        }));
        if worst.as_ref().is_none_or(|w| verdict_rank(v.state) > verdict_rank(w.state)) {
            worst = Some(v);
        }
    }
    let w = worst.unwrap_or_else(|| port_verdict(host, &TlsProbe::Missing, now));
    serde_json::json!({
        "host": host_v,
        "names": w.names,
        "state": w.state,
        "selfSigned": w.self_signed,
        "expiresAt": w.expires_at,
        "notBefore": w.not_before,
        "ports": ports,
    })
}

/// What `status` reads from the captured leaf.
struct CapturedLeaf {
    issuer: String,
    subject: String,
    not_before: i64,
    not_after: i64,
    /// DNS names in the subjectAltName extension, lowercased, in order.
    dns_sans: Vec<String>,
}

fn parse_captured_leaf(der: &[u8]) -> Option<CapturedLeaf> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    let issuer = cert.issuer().to_string();
    let subject = cert.subject().to_string();
    let not_before = cert.validity().not_before.timestamp();
    let not_after = cert.validity().not_after.timestamp();
    let mut dns_sans = Vec::new();
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        for n in &ext.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(d) = n {
                let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
                if !d.is_empty() && !dns_sans.contains(&d) {
                    dns_sans.push(d);
                }
            }
        }
    }
    Some(CapturedLeaf { issuer, subject, not_before, not_after, dns_sans })
}

/// rcgen's default name, the words "self signed", or issuer == subject
/// (a certificate that signed itself — no CA leaf ever does).
fn looks_rcgen_or_self_signed(issuer: &str, subject: &str) -> bool {
    let hay = format!("{issuer} {subject}").to_ascii_lowercase();
    hay.contains("rcgen")
        || hay.contains("self signed")
        || hay.contains("self-signed")
        || (!issuer.trim().is_empty() && issuer.trim() == subject.trim())
}

/// Does a DNS SAN cover `host` (exact, or a one-label `*.` wildcard)?
fn leaf_covers_host(sans: &[String], host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    sans.iter().any(|san| {
        if *san == host {
            return true;
        }
        match (san.strip_prefix("*."), host.split_once('.')) {
            (Some(suffix), Some((label, rest))) => !label.is_empty() && rest == suffix,
            _ => false,
        }
    })
}

/// Probe every port in parallel (one thread each); the whole read is
/// capped at [`TLS_CERT_PROBE_TIMEOUT`] so `status` never hangs. A port
/// that does not answer in time reads `missing`.
#[cfg_attr(test, allow(dead_code))]
fn probe_live_tls_ports(host: &str, ports: &[u16]) -> Vec<(u16, TlsProbe)> {
    if host.is_empty() {
        return ports.iter().map(|p| (*p, TlsProbe::Missing)).collect();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    for port in ports {
        let (tx, host, port) = (tx.clone(), host.to_string(), *port);
        let _ = std::thread::Builder::new()
            .name("mail-tls-cert-probe".into())
            .spawn(move || {
                let addrs = match std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port)) {
                    Ok(a) => a.collect::<Vec<_>>(),
                    Err(_) => Vec::new(),
                };
                let _ = tx.send((port, probe_tls_endpoint(&host, &addrs, port == 443)));
            });
    }
    drop(tx);
    let deadline = std::time::Instant::now() + TLS_CERT_PROBE_TIMEOUT;
    let mut got: std::collections::HashMap<u16, TlsProbe> = std::collections::HashMap::new();
    while got.len() < ports.len() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok((port, probe)) => {
                got.insert(port, probe);
            }
            Err(_) => break,
        }
    }
    ports
        .iter()
        .map(|p| (*p, got.remove(p).unwrap_or(TlsProbe::Missing)))
        .collect()
}

/// One TLS handshake to the first reachable `addrs` entry with SNI =
/// `sni`. A capturing verifier accepts any leaf (including rcgen) so
/// classification can see it. ALPN (`http/1.1`, `h2`) is offered on the
/// HTTPS port only: a mail listener with no overlapping ALPN would
/// refuse the handshake before sending its certificate.
fn probe_tls_endpoint(sni: &str, addrs: &[std::net::SocketAddr], offer_http_alpn: bool) -> TlsProbe {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct CapturingVerifier {
        leaf: Mutex<Option<Vec<u8>>>,
    }

    impl ServerCertVerifier for CapturingVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            if let Ok(mut g) = self.leaf.lock() {
                *g = Some(end_entity.as_ref().to_vec());
            }
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    if sni.is_empty() || addrs.is_empty() {
        return TlsProbe::Missing;
    }
    let mut sock = None;
    for addr in addrs {
        if let Ok(s) = TcpStream::connect_timeout(addr, TLS_CERT_PROBE_TIMEOUT) {
            sock = Some(s);
            break;
        }
    }
    let mut sock = match sock {
        Some(s) => s,
        None => return TlsProbe::Missing,
    };
    let _ = sock.set_read_timeout(Some(TLS_CERT_PROBE_TIMEOUT));
    let _ = sock.set_write_timeout(Some(TLS_CERT_PROBE_TIMEOUT));
    let _ = sock.set_nodelay(true);

    let verifier = Arc::new(CapturingVerifier {
        leaf: Mutex::new(None),
    });
    let Ok(builder) = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions() else {
        return TlsProbe::Missing;
    };
    let mut config = builder
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    if offer_http_alpn {
        config.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
    }
    let Ok(server_name) = ServerName::try_from(sni.to_string()) else {
        return TlsProbe::Missing;
    };
    let Ok(mut conn) = ClientConnection::new(Arc::new(config), server_name) else {
        return TlsProbe::Missing;
    };
    while conn.is_handshaking() {
        match conn.complete_io(&mut sock) {
            Ok((0, 0)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    // No drain read after the handshake: a TLS 1.2 server sends nothing
    // more, and a blocking read would eat the whole time budget.

    let captured = verifier.leaf.lock().ok().and_then(|g| g.clone());
    match captured {
        Some(leaf_der) => TlsProbe::Handshake { leaf_der },
        None => TlsProbe::Missing,
    }
}

/// Overlay SQLite with systemd when we have a unit observation.
/// `None` = leave the row as-is (non-Linux / tests without a seam).
pub fn systemd_ground_truth() -> Option<String> {
    #[cfg(test)]
    {
        return test_unit_state_override();
    }
    #[cfg(not(test))]
    {
        if mail_supported() {
            Some(stalwart_unit_state())
        } else {
            None
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_UNIT_STATE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static TEST_STORE_READY: std::cell::RefCell<Option<bool>> = const { std::cell::RefCell::new(None) };
    static TEST_TLS_PROBE: std::cell::RefCell<Option<Vec<(u16, TlsProbe)>>> = const { std::cell::RefCell::new(None) };
    static TEST_HELPER_STATE: std::cell::RefCell<Option<HelperState>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_store_ready(ready: Option<bool>) {
    TEST_STORE_READY.with(|c| *c.borrow_mut() = ready);
}

#[cfg(test)]
pub(crate) fn mark_step_for_test(step: &str) {
    mark_step(step);
}

#[cfg(test)]
fn test_unit_state_override() -> Option<String> {
    TEST_UNIT_STATE.with(|c| c.borrow().clone())
}

#[cfg(test)]
fn test_unit_state() -> String {
    test_unit_state_override().unwrap_or_default()
}

/// Test seam: inject `systemctl is-active` so route tests never call systemd.
#[cfg(test)]
pub(crate) fn set_test_unit_state(state: Option<&str>) {
    TEST_UNIT_STATE.with(|c| *c.borrow_mut() = state.map(str::to_string));
}

/// Test seam: inject per-port captured leaves / missing probes so
/// cert-status tests never open a socket.
#[cfg(test)]
fn set_test_tls_cert(probes: Option<Vec<(u16, TlsProbe)>>) {
    TEST_TLS_PROBE.with(|c| *c.borrow_mut() = probes);
}

#[cfg(test)]
pub(crate) struct TestTlsCertGuard;

#[cfg(test)]
impl Drop for TestTlsCertGuard {
    fn drop(&mut self) {
        set_test_tls_cert(None);
    }
}

/// Every probed port (443, 465, 993) serves the same `probe`.
#[cfg(test)]
fn with_test_tls_cert(probe: TlsProbe) -> TestTlsCertGuard {
    with_test_tls_ports(MAIL_TLS_PROBE_PORTS.iter().map(|p| (*p, probe.clone())).collect())
}

#[cfg(test)]
fn with_test_tls_ports(probes: Vec<(u16, TlsProbe)>) -> TestTlsCertGuard {
    set_test_tls_cert(Some(probes));
    TestTlsCertGuard
}

/// RAII reset for [`set_test_unit_state`].
#[cfg(test)]
pub(crate) struct TestUnitStateGuard;

#[cfg(test)]
impl Drop for TestUnitStateGuard {
    fn drop(&mut self) {
        set_test_unit_state(None);
    }
}

#[cfg(test)]
pub(crate) fn with_test_unit_state(state: &str) -> TestUnitStateGuard {
    set_test_unit_state(Some(state));
    TestUnitStateGuard
}

fn progress_load() -> serde_json::Value {
    row_field("enable_progress_json")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({ "steps": {} }))
}

fn progress_save(v: &serde_json::Value) {
    set_row_field("enable_progress_json", &v.to_string());
}

fn step_is_done(step: &str) -> bool {
    progress_load()["steps"].get(step).is_some()
}

fn mark_step(step: &str) {
    let mut p = progress_load();
    p["steps"][step] = serde_json::json!({ "at": now_secs() });
    p["current"] = serde_json::Value::Null;
    progress_save(&p);
}

/// Drop resume marks so a later enable re-runs those steps.
fn unmark_steps(steps: &[&str]) {
    let mut p = progress_load();
    if let Some(obj) = p.get_mut("steps").and_then(|v| v.as_object_mut()) {
        for step in steps {
            obj.remove(*step);
        }
    }
    progress_save(&p);
}

fn set_current(step: &str) {
    let mut p = progress_load();
    p["current"] = serde_json::json!(step);
    progress_save(&p);
}

fn progress_extra_set(key: &str, value: &str) {
    let mut p = progress_load();
    p[key] = serde_json::json!(value);
    progress_save(&p);
}

fn progress_extra(key: &str) -> Option<String> {
    progress_load()[key].as_str().map(str::to_string)
}

/// Persist a non-fatal hint on enable progress (e.g. Caddy apply after
/// Enable). Does not change `mail_server.status`.
pub(crate) fn note_enable_progress_hint(key: &str, value: &str) {
    progress_extra_set(key, value);
}

/// Emit the standard daemon event on a supervised-state transition
/// (PRD §4.1: failures raise the standard event → app notification).
pub(crate) fn emit_state_change(previous: &str, state: &str, detail: Option<&str>) {
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::MailServerStateChanged,
        serde_json::json!({ "state": state, "previous": previous, "detail": detail }),
    );
}

// ── The enable state machine ────────────────────────────────────────────

/// Route-level "an enable is already running" latch.
pub fn enable_running() -> &'static AtomicBool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    &RUNNING
}

/// Try to claim the enable latch; `false` = another enable is running.
pub fn try_begin_enable() -> bool {
    enable_running()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub fn end_enable() {
    enable_running().store(false, Ordering::SeqCst);
}

/// Set while `k2 hostmail upgrade` runs (it ALSO holds the enable latch,
/// so enable, the health loop, the DAV backfill and the IMAP boot
/// reconcile stay out). Disable / uninstall / the TLS-reload restart
/// read this to refuse instead of racing the stop → snapshot window.
pub fn upgrade_running() -> &'static AtomicBool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    &RUNNING
}

/// The refusal every Stalwart-touching mutation gives mid-upgrade.
pub const UPGRADE_RUNNING_HINT: &str = concat!("a Stalwart upgrade is running (", cli_name!(), " hostmail upgrade) — \
     wait for it to finish (", cli_name!(), " hostmail status shows its progress), then retry");

/// CAL15: enable never replaces an installed binary of another version.
/// The refusal names the one verb that does.
pub fn enable_version_mismatch_message(installed: &str) -> String {
    format!(
        "installed Stalwart {installed} does not match this daemon's pin \
         {STALWART_PINNED_VERSION} — refusing to manage it. Run `{CLI_NAME} hostmail upgrade` \
         (try `{CLI_NAME} hostmail upgrade --dry-run` first): it stops Stalwart, snapshots the \
         data, installs {STALWART_PINNED_VERSION}, health-checks, and rolls back on \
         failure. Enable never replaces an installed version."
    )
}

/// The ordered step ids — the contract between the machine and status
/// renderers (the Settings→Email page mirrors this list in
/// `email-api.ts`; tests assert completeness against it today).
///
/// Reworked for the live-verified v0.16.10 flow: `config` now CLEARS a
/// stale config file (Stalwart itself writes it during bootstrap);
/// `bootstrap` replaces the journal-scrape `admin-password` step
/// (deterministic recovery credential + `x:Bootstrap/set`);
/// `restart-normal` leaves bootstrap mode; the enable-step named
/// `rotate-admin` is GONE (Bootstrap/set provisions a fresh random
/// admin secret itself) — leftover recovery rotation is `recovery-off`
/// plus the `k2 hostmail rotate-admin` verb; `server-config` binds
/// bootstrap HTTP to loopback :8080 then retargets the mgmt listener
/// to :8180; `recovery-off` strips the recovery env credential and
/// rotates principal `admin` before the final restart.
#[allow(dead_code)] // S7 Settings→Email consumes the ordering.
pub const ENABLE_STEPS: &[&str] = &[
    "preflight",
    "download",
    "verify",
    "extract",
    "system-user",
    "dirs",
    "config",
    "unit",
    "start",
    "bootstrap",
    "restart-normal",
    "server-config",
    "service-account",
    "api-key",
    "recovery-off",
    "restart",
];

/// How long the machine waits for a just-(re)started Stalwart to
/// answer its management listener.
const AUTH_RETRIES: u32 = 15;

/// Preflight → install → bootstrap, resumable. The caller (route) has
/// already run + passed preflight and picked `port_plan`; each step
/// here checks its completion marker first, so re-enable after a crash
/// resumes. On failure: `mail_server.status = 'error'`,
/// `last_error = "<step>: <error>"`, event raised, `Err` returned.
pub fn run_enable(
    ops: &dyn SystemOps,
    api: &mut dyn BootstrapApi,
    secrets: &dyn SecretStore,
    artifact: &StalwartArtifact,
    hostname: &str,
    port_plan: &str,
) -> Result<(), String> {
    // Defense-in-depth: refuse placeholder/malformed checksums even if
    // the caller skipped artifact_for_arch.
    if checksum_is_placeholder(artifact) {
        return Err(
            "refusing to install: artifact checksum is a placeholder — this build cannot \
             verify the Stalwart download"
                .to_string(),
        );
    }
    // Refuse to manage an unrecognized on-disk version (PRD §4): a row
    // installed by a DIFFERENT pin means an explicit upgrade path, not
    // a silent re-install over it.
    if let Some(installed) = row_field("installed_version") {
        if installed != STALWART_PINNED_VERSION {
            return Err(enable_version_mismatch_message(&installed));
        }
    }
    // Every install / unit / systemd step is a `sudo -n` of the mail
    // helper. Without it nothing below can succeed — stop before the
    // row flips to `installing`, before the 40 MB download, and before
    // any write, with the one root command that fixes it.
    if let Some(msg) = helper::unavailable_message(ops.mail_helper_state()) {
        return Err(format!("preflight: {msg}"));
    }

    // Capture BEFORE ensure_installing_row flips the row to `installing`
    // (and overwrites hostname). Disable (and a stale SQLite `running`
    // while the unit is down) must still `daemon-reload` + `enable --now`
    // + `restart` — the start/unit resume marks survive disable otherwise
    // (H4/H5). Previous hostname is the L2 retarget signal.
    let previous = current_status();
    let previous_hostname = row_field("hostname");
    let force_systemd_up = matches!(
        previous.as_deref(),
        Some("disabled") | Some("stopped") | Some("running")
    );
    ensure_installing_row(hostname, port_plan)?;
    let default_domain = default_domain_for(hostname);

    // A customer box: bootstrap/:8080 left sqlite running and progress.steps
    // marked bootstrap/server-config without writing config.json.
    // Resume must re-run guided setup. Keep dirs/unit/start.
    if !ops.path_exists(STALWART_CONFIG) {
        unmark_steps(&[
            "bootstrap",
            "restart-normal",
            "server-config",
            "service-account",
            "api-key",
            "recovery-off",
            "restart",
        ]);
    }

    let fail = |step: &str, err: String| -> String {
        let msg = format!("{step}: {err}");
        set_status("error");
        set_last_error(Some(&msg));
        emit_state_change("installing", "error", Some(&msg));
        // C20: strip STALWART_RECOVERY_ADMIN only after guided setup
        // wrote config.json. During bootstrap a fail rewrite was
        // leaving Stalwart on a self-minted password (a customer box's 401).
        // A failed recovery-off must not strip either: the success
        // path strips after auth. Stripping here drops the env from
        // the unit while the process still has it.
        if step != "recovery-off"
            && ops.path_exists(STALWART_CONFIG)
            && (step_is_done("unit") || ops.path_exists(STALWART_UNIT_PATH))
        {
            let _ = ops.write_file(STALWART_UNIT_PATH, systemd_unit(None).as_bytes(), 0o600);
            let _ = ops.systemctl(&["daemon-reload"]);
        }
        msg
    };

    // The route ran preflight before spawning us.
    if !step_is_done("preflight") {
        mark_step("preflight");
    }

    // ── Binary install (download → verify → extract as one resumable
    //    group: bytes live only in memory, so an incomplete group
    //    re-runs from download) ─────────────────────────────────────
    if !(step_is_done("extract") && ops.path_exists(STALWART_BIN)) {
        set_current("download");
        let url = tarball_url(artifact.triple);
        let bytes = ops.download(&url).map_err(|e| fail("download", e))?;
        mark_step("download");

        set_current("verify");
        if !crate::update_routes::verify_sha256(&bytes, artifact.sha256) {
            return Err(fail(
                "verify",
                format!(
                    "sha256 mismatch for {url} — download corrupted or upstream artifact \
                     changed; NOT installing"
                ),
            ));
        }
        mark_step("verify");

        set_current("extract");
        ops.extract_tar_gz_member(&bytes, "stalwart", STALWART_BIN, 0o755)
            .map_err(|e| fail("extract", e))?;
        mark_step("extract");
    }

    // ── System user + directories + config-clear + unit ───────────
    if !step_is_done("system-user") {
        set_current("system-user");
        ops.ensure_system_user(STALWART_USER)
            .map_err(|e| fail("system-user", e))?;
        mark_step("system-user");
    }
    if !step_is_done("dirs") {
        set_current("dirs");
        (|| -> Result<(), String> {
            ops.create_dir_all(STALWART_CONFIG_DIR)?;
            ops.create_dir_all(STALWART_DATA_DIR)?;
            ops.create_dir_all(STALWART_LOG_DIR)?;
            // The config dir too: Bootstrap/set writes config.json AS
            // THE stalwart USER (✔ live-verified perm failure without
            // this).
            ops.chown_recursive(STALWART_CONFIG_DIR, STALWART_USER)?;
            ops.chown_recursive(STALWART_DATA_DIR, STALWART_USER)?;
            ops.chown_recursive(STALWART_LOG_DIR, STALWART_USER)?;
            // Stalwart's alone: a protocol-17 helper made the dirs 0750
            // and its drop-in (the unit step) carries UMask=0027; tighten
            // whatever an earlier install left inside. An older helper
            // leaves this to the health loop once it updates.
            if ops.store_modes_supported() {
                for dir in STALWART_TREE_DIRS {
                    ops.tighten_store(dir)?;
                }
                mark_step(STORE_MODES_STEP);
            }
            Ok(())
        })()
        .map_err(|e| fail("dirs", e))?;
        mark_step("dirs");
    }
    if !step_is_done("config") {
        set_current("config");
        // C19: never delete an existing config.json (that was a customer box's
        // wipe). Bootstrap requires the file ABSENT only on a true
        // first install — skip the rm when a store already exists.
        let store_ready = ops.path_exists(STALWART_CONFIG)
            || ops.path_exists(&format!("{STALWART_DATA_DIR}/data"));
        if !store_ready && ops.path_exists(STALWART_CONFIG) {
            ops.remove_path(STALWART_CONFIG)
                .map_err(|e| fail("config", e))?;
        }
        mark_step("config");
    }
    if !step_is_done("unit") {
        set_current("unit");
        (|| -> Result<(), String> {
            let recovery_pw = generate_secret()?;
            let sref = secrets.store("recovery-admin", &recovery_pw)?;
            progress_extra_set("recoveryAdminRef", &sref);
            // 0600: the unit embeds the recovery credential until the
            // recovery-off step strips it.
            ops.write_file(
                STALWART_UNIT_PATH,
                systemd_unit(Some(&recovery_pw)).as_bytes(),
                0o600,
            )?;
            ops.create_dir_all(STALWART_DROPIN_DIR)?;
            ops.write_file(STALWART_DROPIN_PATH, hardening_dropin().as_bytes(), 0o644)?;
            ops.systemctl(&["daemon-reload"]).map(|_| ())
        })()
        .map_err(|e| fail("unit", e))?;
        mark_step("unit");
    }
    if !step_is_done("start") || force_systemd_up {
        set_current("start");
        (|| -> Result<(), String> {
            if force_systemd_up {
                ops.systemctl(&["daemon-reload"])?;
            }
            ops.systemctl(&["enable", "--now", STALWART_UNIT])?;
            if force_systemd_up {
                ops.systemctl(&["restart", STALWART_UNIT])?;
            }
            Ok(())
        })()
        .map_err(|e| fail("start", e))?;
        mark_step("start");
    }

    // ── Guided setup over the bootstrap listener ────────────────────
    if !step_is_done("bootstrap") {
        let store_ready = ops.path_exists(STALWART_CONFIG);
        if store_ready {
            // C19: config.json means guided setup already ran — never
            // re-bootstrap (Stalwart would move the data directory).
            // `{data}/data` alone is bootstrap ephemeral — keep going.
            mark_step("bootstrap");
        } else {
            set_current("bootstrap");
            let sref = progress_extra("recoveryAdminRef").ok_or_else(|| {
                fail(
                    "bootstrap",
                    "recovery admin ref missing from progress state".to_string(),
                )
            })?;
            let recovery_pw = secrets
                .resolve(&sref)
                .map_err(|e| fail("bootstrap", e))?
                .ok_or_else(|| {
                    fail(
                        "bootstrap",
                        format!("secret ref {sref} missing from the mail secret store"),
                    )
                })?;
            // A customer box: unit started without STALWART_RECOVERY_ADMIN so
            // Stalwart minted its own bootstrap password. Resume had
            // unit+start marked and did not restart. Rewrite + restart
            // with the vaulted secret, then auth.
            (|| -> Result<(), String> {
                ops.write_file(
                    STALWART_UNIT_PATH,
                    systemd_unit(Some(&recovery_pw)).as_bytes(),
                    0o600,
                )?;
                ops.systemctl(&["daemon-reload"])?;
                ops.systemctl(&["restart", STALWART_UNIT])?;
                Ok(())
            })()
            .map_err(|e| fail("bootstrap", e))?;
            // The service may still be coming up — poll the listener.
            authenticate_with_retry(ops, api, STALWART_SETUP_URL, "admin", &recovery_pw)
                .map_err(|e| fail("bootstrap", e))?;
            let creds = api
                .complete_bootstrap(
                    hostname,
                    &default_domain,
                    super::preflight::request_tls_certificate(port_plan),
                )
                .map_err(|e| fail("bootstrap", e))?;
            let sref = secrets
                .store("admin", &creds.secret)
                .map_err(|e| fail("bootstrap", e))?;
            set_row_field("admin_secret_ref", &sref);
            progress_extra_set("adminUsername", &creds.username);
            mark_step("bootstrap");
        }
    }

    // Leave bootstrap mode: the DataStore config file now exists, so a
    // restart boots the real store in normal mode.
    if !step_is_done("restart-normal") {
        set_current("restart-normal");
        ops.systemctl(&["restart", STALWART_UNIT])
            .map_err(|e| fail("restart-normal", e))?;
        mark_step("restart-normal");
    }

    // ── Provisioning over the normal-mode mgmt listener ─────────────
    // Authentication re-runs on every (re)entry while API steps remain
    // (it isn't a marked step): the provisioned admin + vaulted
    // secret. The default :8080 http listener answers until the final
    // restart applies the port plan; a post-final-restart RESUME finds
    // it on :8180 instead — try both.
    let store_ready = ops.path_exists(STALWART_CONFIG);
    // L2: disable then enable with a **new** name — not the healthy
    // alreadyEnabled no-op (that never reaches run_enable).
    let hostname_retarget = store_ready
        && previous_hostname
            .as_deref()
            .is_some_and(|old| old != hostname);
    let api_steps_remain = !(step_is_done("server-config")
        && step_is_done("service-account")
        && step_is_done("api-key"));
    if api_steps_remain || hostname_retarget {
        let auth_step = if api_steps_remain {
            "server-config"
        } else {
            "hostname"
        };
        authenticate_saved_admin(ops, api, secrets, &default_domain)
            .map_err(|e| fail(auth_step, e))?;
    }

    if hostname_retarget {
        set_current("hostname");
        api.set_server_hostname(hostname)
            .map_err(|e| fail("hostname", e))?;
    }

    if !step_is_done("server-config") {
        set_current("server-config");
        (|| -> Result<(), String> {
            // L12: bind recovery HTTP to 127.0.0.1:8080 (never *:8080)
            // while we still need 8080, then restart so the loopback
            // bind is live before the port-plan retargets to :8180.
            // CAL43: first thing in normal mode, so the guided-setup
            // ACME order (and every renewal after it) asks for the mail
            // host only. Stalwart ACME runs on the tls-alpn plan only.
            if super::preflight::request_tls_certificate(port_plan) {
                api.lock_acme_cert_names(hostname)?;
            }
            api.bind_setup_http_loopback()?;
            ops.systemctl(&["restart", STALWART_UNIT])?;
            authenticate_saved_admin(ops, api, secrets, &default_domain)?;
            api.configure_listeners(port_plan)
        })()
        .map_err(|e| fail("server-config", e))?;
        mark_step("server-config");
    }

    if !step_is_done("service-account") {
        set_current("service-account");
        let account_id = api
            .create_service_account(&default_domain)
            .map_err(|e| fail("service-account", e))?;
        progress_extra_set("serviceAccountId", &account_id);
        mark_step("service-account");
    }

    if !step_is_done("api-key") {
        set_current("api-key");
        let account_id = progress_extra("serviceAccountId").ok_or_else(|| {
            fail(
                "api-key",
                "service account id missing from progress state".to_string(),
            )
        })?;
        let secret = api
            .mint_api_key(&account_id)
            .map_err(|e| fail("api-key", e))?;
        let sref = secrets
            .store("api-key", &secret)
            .map_err(|e| fail("api-key", e))?;
        set_row_field("api_key_ref", &sref);
        set_row_field("api_url", STALWART_MGMT_URL);
        mark_step("api-key");
    }

    // Strip the recovery credential from the unit BEFORE the final
    // restart: ✔ live-verified the env credential AND bootstrap
    // principal `admin` still authenticate in NORMAL mode, so leaving
    // either would be a standing backdoor (pre-mortem #13 class).
    if !step_is_done("recovery-off") {
        set_current("recovery-off");
        (|| -> Result<(), String> {
            // Customer box, 2026-09-22: server-config auth.success for
            // admin@domain at 18:03:02, no rotation in the log,
            // recoveryAdminRef still present, same vaulted secret
            // 401s later. Log in as recovery `admin` on that 401.
            // A hard 401 is not "still starting" — do not retry it.
            // The steps just above already POSTed as this client.
            // Replacing it with a fresh GET /jmap/session can 401 the
            // next management call (the same box from zero, 20:46Z).
            if !api.has_session() {
                authenticate_saved_admin(ops, api, secrets, &default_domain)?;
            }
            if let Err(e) = rotate_leftover_admins(api, secrets, &default_domain) {
                if !is_auth_rejected(&e) {
                    return Err(e);
                }
                let sref = row_field("api_key_ref").ok_or_else(|| {
                    format!("{e} (no api-key to retry)")
                })?;
                let key = secrets.resolve(&sref)?.ok_or_else(|| {
                    format!("{e} (api-key ref {sref} missing)")
                })?;
                if api.authenticate_bearer(STALWART_SETUP_URL, &key).is_err() {
                    api.authenticate_bearer(STALWART_MGMT_URL, &key)
                        .map_err(|b| format!("{e} | api-key: {b}"))?;
                }
                rotate_leftover_admins(api, secrets, &default_domain)?;
            }
            ops.write_file(STALWART_UNIT_PATH, systemd_unit(None).as_bytes(), 0o600)?;
            ops.systemctl(&["daemon-reload"])?;
            Ok(())
        })()
        .map_err(|e| fail("recovery-off", e))?;
        mark_step("recovery-off");
    }

    // Final restart: applies the port plan (mgmt on :8180, SMTP
    // 25/465/587, IMAP/POP3/Sieve gone — ✔ live-verified listener
    // changes only take effect on restart) and drops the recovery env.
    if !step_is_done("restart") {
        set_current("restart");
        ops.systemctl(&["restart", STALWART_UNIT])
            .map_err(|e| fail("restart", e))?;
        mark_step("restart");
    }

    set_row_field("installed_version", STALWART_PINNED_VERSION);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "UPDATE mail_server SET installed_at = COALESCE(installed_at, ?1), \
             updated_at = ?1 WHERE id = 1",
            rusqlite::params![now_secs()],
        );
    }
    let mut p = progress_load();
    p["completedAt"] = serde_json::json!(now_secs());
    p["current"] = serde_json::Value::Null;
    progress_save(&p);
    set_status("running");
    set_last_error(None);
    emit_state_change("installing", "running", None);
    Ok(())
}

/// Authenticate against `base_url`, retrying while the service is
/// still coming up (bounded; sleeps ride [`SystemOps`] so tests are
/// instant).
fn authenticate_with_retry(
    ops: &dyn SystemOps,
    api: &mut dyn BootstrapApi,
    base_url: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let mut last_err = String::new();
    for attempt in 0..AUTH_RETRIES {
        match api.authenticate(base_url, username, password) {
            Ok(()) => return Ok(()),
            Err(e) if is_auth_rejected(&e) => {
                return Err(format!(
                    "management API rejected credentials at {base_url}: {e}"
                ));
            }
            Err(e) => last_err = e,
        }
        if attempt + 1 < AUTH_RETRIES {
            ops.sleep_ms(1000);
        }
    }
    Err(format!(
        "management API not reachable at {base_url}: {last_err}"
    ))
}

/// Result of rotating leftover recovery + provisioned admin secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotateAdminReport {
    pub recovery_principal: String,
    pub provisioned_admin: String,
}

/// L12: invalidate the bootstrap recovery principal `admin` and rotate
/// the provisioned `admin@domain` password. Does not wipe the store,
/// SIGTERM, or rewrite the unit. Used by `recovery-off` and
/// `k2 hostmail rotate-admin`.
pub fn rotate_leftover_admins(
    api: &mut dyn BootstrapApi,
    secrets: &dyn SecretStore,
    default_domain: &str,
) -> Result<RotateAdminReport, String> {
    // Bootstrap principal (not admin@domain). Fail loud if Stalwart
    // has no such account — leftover boxes still have it.
    // Customer box, 19:06:26: this rotate succeeded, the old vault key was
    // deleted, admin@domain was not an account, and the new password
    // was never stored. Save the new principal password first.
    let old_recovery_ref = progress_extra("recoveryAdminRef");
    let recovery_current = old_recovery_ref
        .as_ref()
        .and_then(|sref| secrets.resolve(sref).ok().flatten())
        .unwrap_or_default();
    let recovery_new = generate_secret()?;
    api.rotate_admin_secret("admin", &recovery_current, &recovery_new)?;
    let held = secrets.store("recovery-admin", &recovery_new)?;
    progress_extra_set("recoveryAdminRef", &held);

    let username =
        progress_extra("adminUsername").unwrap_or_else(|| format!("admin@{default_domain}"));
    let provisioned_admin = if username == "admin" {
        set_row_field("admin_secret_ref", &held);
        username
    } else {
        let sref = row_field("admin_secret_ref").ok_or_else(|| {
            "admin secret ref missing — cannot rotate the provisioned admin".to_string()
        })?;
        let current = secrets
            .resolve(&sref)?
            .ok_or_else(|| format!("secret ref {sref} missing from the mail secret store"))?;
        let new_pw = generate_secret()?;
        match api.rotate_admin_secret(&username, &current, &new_pw) {
            Ok(()) => {
                let new_ref = secrets.store("admin", &new_pw)?;
                set_row_field("admin_secret_ref", &new_ref);
                // Provisioned admin is the login. Drop the recovery
                // secret now that a failure cannot strand us.
                if let Some(old) = &old_recovery_ref {
                    let _ = secrets.delete(old);
                }
                if old_recovery_ref.as_ref() != Some(&held) {
                    let _ = secrets.delete(&held);
                }
                username
            }
            Err(e) if e.contains("not found") => {
                // Two customer boxes: Bootstrap stored admin@domain; Stalwart
                // only has principal `admin`. Keep that password.
                set_row_field("admin_secret_ref", &held);
                if let Some(old) = &old_recovery_ref {
                    if old != &held {
                        let _ = secrets.delete(old);
                    }
                }
                format!("{username} (not in Stalwart — skipped)")
            }
            Err(e) => return Err(e),
        }
    };
    Ok(RotateAdminReport {
        recovery_principal: "admin".to_string(),
        provisioned_admin,
    })
}

/// Live entry for POST `/cli/mail/server/rotate-admin`: talk to the
/// running server over the loopback ApiKey (same as doctor/domains).
/// Does not wipe the store. Rotates leftover principal `admin` when
/// present. Stored `adminUsername` (`admin@domain`) is rotated only if
/// that account exists — a customer box on 0.40.149 stored the name from Bootstrap
/// but Stalwart only has `admin` + `k2-daemon`. Missing provisioned
/// name is a skip, not 502 after already rotating `admin`.
pub fn rotate_leftover_admins_live() -> Result<RotateAdminReport, String> {
    if !mail_supported() {
        return Err(
            "the email server only works on Linux deployments; this daemon is not Linux"
                .to_string(),
        );
    }
    let (client, hostname) = super::domains::engine_from_db()?;
    let default_domain = default_domain_for(hostname.as_deref().unwrap_or(""));
    let secrets = FileSecretStore::default();

    let recovery_principal = if client.account_query_id("admin")?.is_some() {
        let recovery_new = generate_secret()?;
        client.rotate_account_secret("admin", &recovery_new)?;
        if let Some(sref) = progress_extra("recoveryAdminRef") {
            let _ = secrets.delete(&sref);
        }
        "admin".to_string()
    } else {
        "admin (not in Stalwart — skipped)".to_string()
    };

    let username =
        progress_extra("adminUsername").unwrap_or_else(|| format!("admin@{default_domain}"));
    let provisioned_admin = if username != "admin" && client.account_query_id(&username)?.is_some()
    {
        let new_pw = generate_secret()?;
        client.rotate_account_secret(&username, &new_pw)?;
        let new_ref = secrets.store("admin", &new_pw)?;
        set_row_field("admin_secret_ref", &new_ref);
        username
    } else if username == "admin" {
        username
    } else {
        format!("{username} (not in Stalwart — skipped)")
    };

    if recovery_principal.contains("skipped") && provisioned_admin.contains("skipped") {
        return Err(
            "neither leftover principal 'admin' nor stored adminUsername exist in Stalwart"
                .to_string(),
        );
    }
    Ok(RotateAdminReport {
        recovery_principal,
        provisioned_admin,
    })
}

/// Authenticate against the pre-plan (:8080) listener, falling back to
/// the post-plan mgmt listener (:8180) — a resume after the final
/// restart finds the mgmt API there instead.
/// HTTP 401 is a credential reject. Retrying it looks like a brute
/// force and will not make a wrong password right.
fn is_auth_rejected(err: &str) -> bool {
    err.contains("401")
}

/// Provisioned `admin@domain`, then leftover recovery principal `admin`.
/// Field report: the provisioned secret 401s after a successful login on the
/// same process, and `recoveryAdminRef` is still in progress.
fn authenticate_saved_admin(
    ops: &dyn SystemOps,
    api: &mut dyn BootstrapApi,
    secrets: &dyn SecretStore,
    default_domain: &str,
) -> Result<(), String> {
    let username =
        progress_extra("adminUsername").unwrap_or_else(|| format!("admin@{default_domain}"));
    let mut errors = Vec::new();
    if let Some(sref) = row_field("admin_secret_ref") {
        match secrets.resolve(&sref) {
            Ok(Some(pw)) => match authenticate_either(ops, api, &username, &pw) {
                Ok(()) => return Ok(()),
                Err(e) => errors.push(format!("{username}: {e}")),
            },
            Ok(None) => errors.push(format!(
                "{username}: secret ref {sref} missing from the mail secret store"
            )),
            Err(e) => errors.push(format!("{username}: {e}")),
        }
    } else {
        errors.push("admin secret ref missing".to_string());
    }
    if username != "admin" {
        if let Some(sref) = progress_extra("recoveryAdminRef") {
            match secrets.resolve(&sref) {
                Ok(Some(pw)) => match authenticate_either(ops, api, "admin", &pw) {
                    Ok(()) => return Ok(()),
                    Err(e) => errors.push(format!("admin: {e}")),
                },
                Ok(None) => errors.push(format!(
                    "admin: secret ref {sref} missing from the mail secret store"
                )),
                Err(e) => errors.push(format!("admin: {e}")),
            }
        }
    }
    if let Some(sref) = row_field("api_key_ref") {
        match secrets.resolve(&sref) {
            Ok(Some(key)) => {
                if api.authenticate_bearer(STALWART_SETUP_URL, &key).is_ok()
                    || api.authenticate_bearer(STALWART_MGMT_URL, &key).is_ok()
                {
                    return Ok(());
                }
                errors.push("api-key: rejected on :8080 and :8180".to_string());
            }
            Ok(None) => errors.push(format!(
                "api-key: secret ref {sref} missing from the mail secret store"
            )),
            Err(e) => errors.push(format!("api-key: {e}")),
        }
    }
    if errors.is_empty() {
        return Err("admin secret ref missing — cannot authenticate".to_string());
    }
    Err(format!(
        "admin authentication failed — {}",
        errors.join(" | ")
    ))
}

fn authenticate_either(
    ops: &dyn SystemOps,
    api: &mut dyn BootstrapApi,
    username: &str,
    password: &str,
) -> Result<(), String> {
    // One quick probe first (no retry storm when the other URL is the
    // live one). A 401 on :8080 is final for that password — try :8180
    // once, then stop.
    match api.authenticate(STALWART_SETUP_URL, username, password) {
        Ok(()) => return Ok(()),
        Err(e) if is_auth_rejected(&e) => {
            return api
                .authenticate(STALWART_MGMT_URL, username, password)
                .map_err(|second| {
                    format!("admin authentication failed on both listeners — {e}; {second}")
                });
        }
        Err(_) => {}
    }
    if api
        .authenticate(STALWART_MGMT_URL, username, password)
        .is_ok()
    {
        return Ok(());
    }
    // Neither answered instantly — the service may be restarting; give
    // the pre-plan URL the patient retry, then the mgmt URL once more.
    match authenticate_with_retry(ops, api, STALWART_SETUP_URL, username, password) {
        Ok(()) => Ok(()),
        Err(first) => api
            .authenticate(STALWART_MGMT_URL, username, password)
            .map_err(|second| {
                format!("admin authentication failed on both listeners — {first}; {second}")
            }),
    }
}

// ── Health ──────────────────────────────────────────────────────────────

/// Supervised health verdict (systemd + authed API ping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    NotInstalled,
    Running,
    Degraded(String),
    Stopped(String),
}

impl Health {
    pub fn as_status_str(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not-installed",
            Self::Running => "running",
            Self::Degraded(_) => "degraded",
            Self::Stopped(_) => "stopped",
        }
    }
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Degraded(d) | Self::Stopped(d) => Some(d),
            _ => None,
        }
    }
}

/// Pure verdict logic over injected observations: binary present →
/// unit active → authed API ping.
pub fn health_check_with(ops: &dyn SystemOps, api_ping: &dyn Fn() -> Result<(), String>) -> Health {
    if !ops.path_exists(STALWART_BIN) {
        return Health::NotInstalled;
    }
    let active = ops.systemctl_query(&["is-active", STALWART_UNIT]);
    if active != "active" {
        return Health::Stopped(format!(
            "systemd reports the stalwart unit is '{}'",
            if active.is_empty() {
                "unknown"
            } else {
                &active
            }
        ));
    }
    match api_ping() {
        Ok(()) => Health::Running,
        Err(e) => Health::Degraded(format!(
            "unit is active but the management API ping failed: {e}"
        )),
    }
}

/// Real health check: systemd state + an authenticated session-doc
/// fetch on the loopback mgmt endpoint. Persists the verdict onto the
/// `mail_server` row (only across running/degraded/stopped — never
/// fights `installing`/`disabled`/`error`) and raises the standard
/// event on a TRANSITION. Returns the verdict JSON for `?health=1`.
pub fn refresh_health() -> serde_json::Value {
    let ping = || -> Result<(), String> {
        let api_url = row_field("api_url").ok_or("no api_url recorded")?;
        let key_ref = row_field("api_key_ref").ok_or("no api_key_ref recorded")?;
        let key = FileSecretStore::default()
            .resolve(&key_ref)?
            .ok_or("api key missing from the mail secret store")?;
        StalwartClient::new(api_url, key).ping()
    };
    let health = health_check_with(&RealSystemOps, &ping);
    persist_health(&health);
    serde_json::json!({
        "state": health.as_status_str(),
        "detail": health.detail(),
    })
}

/// Status write + transition event for a health verdict.
fn persist_health(health: &Health) {
    let Some(previous) = current_status() else {
        return; // no row — nothing installed, nothing to persist
    };
    if matches!(previous.as_str(), "installing" | "disabled" | "error") {
        return;
    }
    let new_status = match health {
        Health::NotInstalled => return, // row says installed; binary gone is a Stopped-tier
        _ => health.as_status_str(),
    };
    if previous == new_status {
        return;
    }
    set_status(new_status);
    set_last_error(health.detail());
    emit_state_change(&previous, new_status, health.detail());
}

/// Background health cadence: one detached thread, 60 s period, only
/// on supported (Linux) daemons. Panics are contained per-tick.
pub fn spawn_health_loop() {
    if !mail_supported() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("mail-health".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            if enable_running().load(Ordering::SeqCst) {
                continue; // never fight the enable machine
            }
            if current_status().is_none() {
                continue; // not installed
            }
            let _ = std::panic::catch_unwind(|| {
                // Existing installs: Stalwart's tree for Stalwart alone,
                // once the root helper serves it (no restart).
                ensure_store_modes_live();
                let _ = refresh_health();
                // Auth-ban migrate window auto-restore (prd-hostmail-bans-v1 B26).
                super::bans::tick_bans_migrate_restore();
            });
        });
}

// ── Disable / uninstall / upgrade ───────────────────────────────────────

/// §4.1 disable: stop + disable the unit, KEEP all data. Domains stay
/// verified while MX points at a dead port — the ROUTE warns loudly.
pub fn disable_with(ops: &dyn SystemOps) -> Result<(), String> {
    ops.systemctl(&["disable", "--now", STALWART_UNIT])?;
    let previous = current_status().unwrap_or_else(|| "unknown".into());
    set_status("disabled");
    set_last_error(None);
    // Keep bootstrap/api-key marks (data stays). Clear start so re-enable
    // cannot skip `systemctl enable --now` (H4). Domains are not touched (H5).
    unmark_steps(&["start"]);
    emit_state_change(&previous, "disabled", None);
    Ok(())
}

pub fn disable() -> Result<(), String> {
    disable_with(&RealSystemOps)
}

/// §4.1 uninstall: disable + remove binary/unit/drop-in (+ the
/// explicit, double-confirmed data purge). Secrets and the singleton
/// row go last so a failed removal stays resumable. The ROUTE enforces
/// the typed-hostname confirmation before `purge_data` reaches here.
pub fn uninstall_with(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    purge_data: bool,
) -> Result<(), String> {
    // Unit may already be gone (resumed uninstall) — best-effort stop.
    let _ = ops.systemctl(&["disable", "--now", STALWART_UNIT]);
    ops.remove_path(STALWART_UNIT_PATH)?;
    ops.remove_path(STALWART_DROPIN_DIR)?;
    ops.systemctl(&["daemon-reload"])?;
    ops.remove_path(STALWART_BIN)?;
    if purge_data {
        ops.remove_path(STALWART_DATA_DIR)?;
        ops.remove_path(STALWART_LOG_DIR)?;
        ops.remove_path(STALWART_CONFIG_DIR)?;
        super::sysops::forget_store_state();
    }
    for col in ["admin_secret_ref", "api_key_ref"] {
        if let Some(sref) = row_field(col) {
            let _ = secrets.delete(&sref);
        }
    }
    // The recovery-admin secret lives only in progress state (cleared
    // by recovery-off on a completed enable; still present after an
    // aborted one).
    if let Some(sref) = progress_extra("recoveryAdminRef") {
        let _ = secrets.delete(&sref);
    }
    let previous = current_status().unwrap_or_else(|| "unknown".into());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
    }
    emit_state_change(&previous, "not-installed", None);
    Ok(())
}

pub fn uninstall(purge_data: bool) -> Result<(), String> {
    uninstall_with(&RealSystemOps, &FileSecretStore::default(), purge_data)
}

/// Explicit pinned upgrade (CAL7/CAL14/CAL15, pre-mortem #8): preflight,
/// then stop → snapshot config + data dir → install the verified pin →
/// start → health check, with snapshot restore + previous-binary reinstall
/// on failure. The engine lives in [`super::upgrade`]; this is the one
/// entry the owner route calls. NEVER called from boot, a daemon update,
/// or enable.
pub fn upgrade(req: super::upgrade::UpgradeRequest) -> super::upgrade::UpgradeStart {
    super::upgrade::start_live(req)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;

// Kept below `mod tests`: domains::acme's scan reads everything above it
// as production code, and this seam mints a (test) certificate.
/// Test seam for other modules: the TLS probe captures a Let's
/// Encrypt-looking leaf carrying `sans` (status reports `issued`).
#[cfg(test)]
pub(crate) fn with_test_issued_cert(sans: &[&str]) -> TestTlsCertGuard {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("ca key");
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "R11");
    dn.push(DnType::OrganizationName, "Let's Encrypt");
    ca.distinguished_name = dn;
    let ca_cert = ca.self_signed(&ca_key).expect("ca sign");
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
    let mut params = CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .expect("SAN params");
    // rcgen's default subject is "rcgen self signed cert": name the leaf.
    let mut ldn = DistinguishedName::new();
    ldn.push(DnType::CommonName, sans.first().copied().unwrap_or("leaf"));
    params.distinguished_name = ldn;
    let der = params.signed_by(&key, &ca_cert, &ca_key).expect("sign").der().to_vec();
    with_test_tls_cert(TlsProbe::Handshake { leaf_der: der })
}

/// What one probed port serves in [`with_test_served_ports`].
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum TestLeaf<'a> {
    /// A CA-signed leaf (issuer != subject) for these SANs, valid now.
    Issued(&'a [&'a str]),
    /// The same, but its notAfter is in the past.
    Expired(&'a [&'a str]),
    /// Stalwart's rcgen fallback (SAN localhost, issuer == subject).
    SelfSigned,
    /// Nothing answers on the port.
    Missing,
}

/// Test seam for other modules (0.46.1 live-cert re-order): each probed
/// port serves its own [`TestLeaf`]. Never opens a socket.
#[cfg(test)]
pub(crate) fn with_test_served_ports(ports: &[(u16, TestLeaf<'_>)]) -> TestTlsCertGuard {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};
    let ca_leaf = |sans: &[&str], expired: bool| -> Vec<u8> {
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("ca key");
        let mut ca = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "R11");
        dn.push(DnType::OrganizationName, "Let's Encrypt");
        ca.distinguished_name = dn;
        let ca_cert = ca.self_signed(&ca_key).expect("ca sign");
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
        let mut params = CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .expect("SAN params");
        let mut ldn = DistinguishedName::new();
        ldn.push(DnType::CommonName, sans.first().copied().unwrap_or("leaf"));
        params.distinguished_name = ldn;
        if expired {
            params.not_before = rcgen::date_time_ymd(2020, 1, 1);
            params.not_after = rcgen::date_time_ymd(2020, 4, 1);
        }
        params.signed_by(&key, &ca_cert, &ca_key).expect("sign").der().to_vec()
    };
    let probes = ports
        .iter()
        .map(|(port, leaf)| {
            let probe = match leaf {
                TestLeaf::Issued(sans) => TlsProbe::Handshake { leaf_der: ca_leaf(sans, false) },
                TestLeaf::Expired(sans) => TlsProbe::Handshake { leaf_der: ca_leaf(sans, true) },
                TestLeaf::SelfSigned => {
                    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
                    let params =
                        CertificateParams::new(vec!["localhost".to_string()]).expect("params");
                    TlsProbe::Handshake {
                        leaf_der: params.self_signed(&key).expect("self-sign").der().to_vec(),
                    }
                }
                TestLeaf::Missing => TlsProbe::Missing,
            };
            (*port, probe)
        })
        .collect();
    with_test_tls_ports(probes)
}
