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

use super::jmap::StalwartClient;
use super::secrets::{generate_secret, FileSecretStore, SecretStore};
use super::sysops::{RealSystemOps, SystemOps};

// ── The pin ─────────────────────────────────────────────────────────────

/// The PINNED Stalwart version the supervisor installs and manages.
///
/// v0.16.10 — latest v0.16.x at S1 build time (released 2026-06-21).
/// Pinning is load-bearing: v0.16 removed the whole REST API relative
/// to earlier lines, upstream has no mgmt-API stability policy, and
/// the config format churns between minors (PRD §4, §16, pre-mortem
/// #8). Upgrades are explicit, K2-shipped, tested operations — the
/// supervisor REFUSES to manage an unrecognized on-disk version, and
/// the daemon updating itself never touches the Stalwart version.
pub const STALWART_PINNED_VERSION: &str = "0.16.10";

/// One pinned release artifact: Rust `std::env::consts::ARCH` name →
/// upstream target triple + the sha256 of the release tarball.
#[derive(Debug)]
pub struct StalwartArtifact {
    pub arch: &'static str,
    pub triple: &'static str,
    pub sha256: &'static str,
}

/// Baked-in checksums for the pinned release (PRD §4.1 — "checksums
/// baked into the daemon at build time").
///
/// REAL values for v0.16.10, verified TWO independent ways at bake
/// time (2026-07-08): (1) extracted from the release's signed sigstore
/// bundles (`…tar.gz.sigstore.json` → messageSignature.messageDigest),
/// (2) sha256 of the actually-downloaded tarballs. Both matched.
/// (Re-verified against a fresh download during the 2026-07-10 live
/// rework.) glibc builds — K2 Linux deployments are Ubuntu/Debian-
/// class boxes (musl/Alpine support would add the musl triples here).
pub const STALWART_SHA256: &[StalwartArtifact] = &[
    StalwartArtifact {
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        sha256: "3ec4ab7eff49f61280f2fe2e4f9645ce5308d1840286ef0c0437524f92ac6a33",
    },
    StalwartArtifact {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        sha256: "0c9a80174a8a187477ac0ae4fcca8b7e0f17c84f6cae7b9304d2539459ba44ac",
    },
];

/// True when a checksum entry is a placeholder / malformed — install
/// REFUSES to run in that state (the "never ship an unverifiable
/// install path" guard; a const table edited to `…PLACEHOLDER…` or a
/// truncated hex fails closed).
pub fn checksum_is_placeholder(a: &StalwartArtifact) -> bool {
    a.sha256.contains("PLACEHOLDER")
        || a.sha256.len() != 64
        || !a.sha256.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Resolve the pinned artifact for a `std::env::consts::ARCH` value.
pub fn artifact_for_arch(arch: &str) -> Result<&'static StalwartArtifact, String> {
    let art = STALWART_SHA256
        .iter()
        .find(|a| a.arch == arch)
        .ok_or_else(|| {
            format!("unsupported CPU architecture '{arch}' — K2 Mail ships x86_64 and aarch64")
        })?;
    if checksum_is_placeholder(art) {
        return Err(format!(
            "refusing to install: the baked-in sha256 for {arch} is a placeholder — \
             this build cannot verify the Stalwart download"
        ));
    }
    Ok(art)
}

/// Enable fetches from the Alakazam fork release; pin still 0.16.10;
/// K2 does not rebuild Stalwart.
pub fn tarball_url(triple: &str) -> String {
    format!(
        "https://github.com/Alakazam-211/stalwart/releases/download/v{STALWART_PINNED_VERSION}/stalwart-{triple}.tar.gz"
    )
}

// ── On-disk / on-box layout ─────────────────────────────────────────────

pub const STALWART_BIN: &str = "/usr/local/bin/stalwart";
pub const STALWART_CONFIG_DIR: &str = "/etc/stalwart";
pub const STALWART_CONFIG: &str = "/etc/stalwart/config.json";
pub const STALWART_DATA_DIR: &str = "/var/lib/stalwart";
pub const STALWART_LOG_DIR: &str = "/var/log/stalwart";
pub const STALWART_USER: &str = "stalwart";
pub const STALWART_UNIT: &str = "stalwart";
pub const STALWART_UNIT_PATH: &str = "/etc/systemd/system/stalwart.service";
pub const STALWART_DROPIN_DIR: &str = "/etc/systemd/system/stalwart.service.d";
pub const STALWART_DROPIN_PATH: &str = "/etc/systemd/system/stalwart.service.d/k2-hardening.conf";

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

// ── Systemd unit + §10 hardening drop-in ────────────────────────────────

/// The service unit K2 writes (upstream ships none for our layout).
/// `recovery_admin_password`: present during install (deterministic
/// bootstrap credential, ✔ live-verified `STALWART_RECOVERY_ADMIN`
/// env form); the `recovery-off` step rewrites the unit WITHOUT it
/// once the provisioned admin + ApiKey are vaulted, and rotates the
/// bootstrap principal `admin` (the env credential AND that principal
/// stay valid even in normal mode — verified — so leaving either would
/// be a standing backdoor). The unit is written 0600 for exactly this
/// reason. Hardening lives in the drop-in so the split mirrors PRD §10
/// verbatim and a future unit rewrite can't silently drop it.
pub fn systemd_unit(recovery_admin_password: Option<&str>) -> String {
    let env_line = match recovery_admin_password {
        Some(pw) => format!("Environment=STALWART_RECOVERY_ADMIN=admin:{pw}\n"),
        None => String::new(),
    };
    format!(
        "[Unit]\n\
         Description=Stalwart mail server (managed by the K2 Mail supervisor)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={STALWART_BIN} --config {STALWART_CONFIG}\n\
         User={STALWART_USER}\n\
         Group={STALWART_USER}\n\
         {env_line}\
         StandardOutput=journal\n\
         StandardError=journal\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// PRD §10, verbatim — plus `/etc/stalwart` in ReadWritePaths:
/// ✔ LIVE-VERIFIED the Bootstrap/set persists the DataStore config to
/// that path AS THE stalwart USER (the first live attempt failed with
/// `Permission denied (os error 13)` until the dir was writable).
pub fn hardening_dropin() -> &'static str {
    "[Service]\n\
     ProtectSystem=strict\n\
     ProtectHome=yes\n\
     ReadWritePaths=/var/lib/stalwart /var/log/stalwart /etc/stalwart\n\
     NoNewPrivileges=yes\n\
     PrivateTmp=yes\n\
     CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n\
     AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
     Restart=on-failure\n"
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
    fn authenticate(&mut self, base_url: &str, username: &str, password: &str)
        -> Result<(), String>;
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
}

// ── mail_server row helpers ─────────────────────────────────────────────

fn now_secs() -> i64 {
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

fn set_status(status: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE mail_server SET status = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![status, now_secs()],
    );
}

fn set_last_error(err: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE mail_server SET last_error = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![err, now_secs()],
    );
}

fn row_field(col: &str) -> Option<String> {
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

fn set_row_field(col: &str, value: &str) {
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
                last_error: Some(format!(
                    "systemd reports the stalwart unit is 'active' while hostmail status is '{sqlite_state}'"
                )),
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

/// Restart the Stalwart unit so listeners re-read Certificate objects.
/// JMAP `x:Certificate/set` persists in RocksDB but 443/465 keep the
/// previous acceptor until this restart. Not `hostmail disable`.
pub fn restart_stalwart_to_reload_tls() -> Result<(), String> {
    #[cfg(test)]
    {
        return Ok(());
    }
    #[cfg(not(test))]
    {
        RealSystemOps
            .systemctl(&["restart", STALWART_UNIT])
            .map(|_| ())?;
        // Listeners come back after the process is up.
        for _ in 0..30 {
            if stalwart_unit_state().trim() == "active" {
                std::thread::sleep(std::time::Duration::from_millis(400));
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err("stalwart unit did not become active after TLS reload restart".into())
    }
}

pub fn is_already_enabled() -> bool {
    let unit = stalwart_unit_state();
    if is_already_enabled_with(current_status().as_deref(), &unit) {
        return true;
    }
    // C19: unit active + initialized store is a healthy no-op even when
    // sqlite is not `running` (do not re-bootstrap / wipe config).
    unit.trim() == "active" && is_store_initialized()
}

/// True when config.json or the RocksDB data dir already exists.
pub fn is_store_initialized() -> bool {
    #[cfg(test)]
    {
        if let Some(v) = TEST_STORE_READY.with(|c| *c.borrow()) {
            return v;
        }
    }
    std::path::Path::new(STALWART_CONFIG).is_file()
        || std::path::Path::new(&format!("{STALWART_DATA_DIR}/data")).exists()
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

/// C24 / L1: cert state on `status` without `--health`. Probe the live
/// TLS listener on `{hostname}:443` (2s timeout) with a verifier that
/// **captures** the leaf and accepts rcgen so a serving self-signed
/// cert is not reported as `missing` + `selfSigned: false`. ACME is
/// the mail hostname only — optional names are not in the issuance request.
pub fn tls_cert_status(hostname: Option<&str>) -> serde_json::Value {
    let host = hostname
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    #[cfg(test)]
    {
        let probe = TEST_TLS_PROBE
            .with(|c| c.borrow().clone())
            .unwrap_or(TlsProbe::Missing);
        let dirs = TEST_CERTS_DIR.with(|c| *c.borrow()).unwrap_or(false);
        return tls_cert_json(host, probe, dirs);
    }
    #[cfg(not(test))]
    {
        let probe = probe_live_tls(host);
        let dirs = stalwart_certs_dir_has_files();
        tls_cert_json(host, probe, dirs)
    }
}

fn tls_cert_json(host: &str, probe: TlsProbe, certs_dir_has_files: bool) -> serde_json::Value {
    let host_v = if host.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(host)
    };
    let names: Vec<String> = if host.is_empty() {
        Vec::new()
    } else {
        vec![host.to_string()]
    };
    let (state, self_signed, expires_at) = match probe {
        TlsProbe::Missing => ("missing", false, serde_json::Value::Null),
        TlsProbe::Handshake { leaf_der } => match parse_captured_leaf(&leaf_der) {
            Some((issuer, subject, not_after)) => {
                let expires = serde_json::json!(not_after);
                if looks_rcgen_or_self_signed(&issuer, &subject) {
                    ("self-signed", true, expires)
                } else if looks_public_or_acme(&issuer) || certs_dir_has_files {
                    ("issued", false, expires)
                } else {
                    // Handshake succeeded with a non-rcgen leaf.
                    ("issued", false, expires)
                }
            }
            None => {
                // Leaf captured but unparseable: still not missing.
                ("issued", false, serde_json::Value::Null)
            }
        },
    };
    serde_json::json!({
        "host": host_v,
        "names": names,
        "state": state,
        "selfSigned": self_signed,
        "expiresAt": expires_at,
    })
}

fn parse_captured_leaf(der: &[u8]) -> Option<(String, String, i64)> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    let issuer = cert.issuer().to_string();
    let subject = cert.subject().to_string();
    let not_after = cert.validity().not_after.timestamp();
    Some((issuer, subject, not_after))
}

fn looks_rcgen_or_self_signed(issuer: &str, subject: &str) -> bool {
    let hay = format!("{issuer} {subject}").to_ascii_lowercase();
    hay.contains("rcgen") || hay.contains("self signed") || hay.contains("self-signed")
}

fn looks_public_or_acme(issuer: &str) -> bool {
    let hay = issuer.to_ascii_lowercase();
    hay.contains("let's encrypt")
        || hay.contains("letsencrypt")
        || hay.contains("zerossl")
        || hay.contains("zero ssl")
        || hay.contains("google trust")
        || hay.contains("gts ca")
        || hay.contains("amazon")
        || hay.contains("digicert")
        || hay.contains("sectigo")
        || hay.contains("acme")
}

#[cfg_attr(test, allow(dead_code))]
fn stalwart_certs_dir_has_files() -> bool {
    for dir in ["/var/lib/stalwart/certs", "/etc/stalwart/certs"] {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        if rd.filter_map(|e| e.ok()).any(|e| e.path().is_file()) {
            return true;
        }
    }
    false
}

/// Probe `{host}:443` with a 2s cap. A capturing verifier accepts the
/// leaf (including rcgen) so classification can see it. Never hangs
/// `status`: the worker is abandoned after the timeout.
#[cfg_attr(test, allow(dead_code))]
fn probe_live_tls(host: &str) -> TlsProbe {
    if host.is_empty() {
        return TlsProbe::Missing;
    }
    let host = host.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawn = std::thread::Builder::new()
        .name("mail-tls-cert-probe".into())
        .spawn(move || {
            let _ = tx.send(probe_live_tls_inner(&host));
        });
    if spawn.is_err() {
        return TlsProbe::Missing;
    }
    rx.recv_timeout(TLS_CERT_PROBE_TIMEOUT)
        .unwrap_or(TlsProbe::Missing)
}

#[cfg_attr(test, allow(dead_code))]
fn probe_live_tls_inner(host: &str) -> TlsProbe {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
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

    let addrs = match (host, 443u16).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(_) => return TlsProbe::Missing,
    };
    if addrs.is_empty() {
        return TlsProbe::Missing;
    }
    let mut sock = None;
    for addr in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&addr, TLS_CERT_PROBE_TIMEOUT) {
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
    config.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
    let Ok(server_name) = ServerName::try_from(host.to_string()) else {
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
    // Drain any leftover TLS bytes so a slow close does not matter.
    let _ = sock.read(&mut [0u8; 1]);
    let _ = sock.flush();

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
    static TEST_TLS_PROBE: std::cell::RefCell<Option<TlsProbe>> = const { std::cell::RefCell::new(None) };
    static TEST_CERTS_DIR: std::cell::RefCell<Option<bool>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_store_ready(ready: Option<bool>) {
    TEST_STORE_READY.with(|c| *c.borrow_mut() = ready);
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

/// Test seam: inject a captured leaf / missing probe so cert-status
/// tests never open a socket.
#[cfg(test)]
fn set_test_tls_cert(probe: Option<TlsProbe>, certs_dir: Option<bool>) {
    TEST_TLS_PROBE.with(|c| *c.borrow_mut() = probe);
    TEST_CERTS_DIR.with(|c| *c.borrow_mut() = certs_dir);
}

#[cfg(test)]
struct TestTlsCertGuard;

#[cfg(test)]
impl Drop for TestTlsCertGuard {
    fn drop(&mut self) {
        set_test_tls_cert(None, None);
    }
}

#[cfg(test)]
fn with_test_tls_cert(probe: TlsProbe, certs_dir: bool) -> TestTlsCertGuard {
    set_test_tls_cert(Some(probe), Some(certs_dir));
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
fn emit_state_change(previous: &str, state: &str, detail: Option<&str>) {
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
            return Err(format!(
                "installed Stalwart {installed} does not match this daemon's pin \
                 {STALWART_PINNED_VERSION} — refusing to manage it (upgrades are an \
                 explicit supervisor operation)"
            ));
        }
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

    let fail = |step: &str, err: String| -> String {
        let msg = format!("{step}: {err}");
        set_status("error");
        set_last_error(Some(&msg));
        emit_state_change("installing", "error", Some(&msg));
        // C20: after `unit` is written, any failure rewrites the unit
        // without STALWART_RECOVERY_ADMIN.
        if step_is_done("unit") || ops.path_exists(STALWART_UNIT_PATH) {
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
            ops.chown_recursive(STALWART_LOG_DIR, STALWART_USER)
        })()
        .map_err(|e| fail("dirs", e))?;
        mark_step("dirs");
    }
    if !step_is_done("config") {
        set_current("config");
        // C19: never delete an existing config.json (that was the noir
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
        let store_ready = ops.path_exists(STALWART_CONFIG)
            || ops.path_exists(&format!("{STALWART_DATA_DIR}/data"));
        if store_ready {
            // C19: initialized store — never re-bootstrap (Stalwart
            // would move the data directory).
            mark_step("bootstrap");
        } else {
        set_current("bootstrap");
        let sref = progress_extra("recoveryAdminRef").ok_or_else(|| {
            fail("bootstrap", "recovery admin ref missing from progress state".to_string())
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
    let store_ready = ops.path_exists(STALWART_CONFIG)
        || ops.path_exists(&format!("{STALWART_DATA_DIR}/data"));
    // L2: disable then enable with a **new** name — not the healthy
    // alreadyEnabled no-op (that never reaches run_enable).
    let hostname_retarget = store_ready
        && previous_hostname
            .as_deref()
            .is_some_and(|old| old != hostname);
    let api_steps_remain =
        !(step_is_done("server-config") && step_is_done("service-account") && step_is_done("api-key"));
    if api_steps_remain || hostname_retarget {
        let auth_step = if api_steps_remain {
            "server-config"
        } else {
            "hostname"
        };
        let username = progress_extra("adminUsername")
            .unwrap_or_else(|| format!("admin@{default_domain}"));
        let sref = row_field("admin_secret_ref").ok_or_else(|| {
            fail(
                auth_step,
                "admin secret ref missing — re-run enable".to_string(),
            )
        })?;
        let admin_pw = secrets
            .resolve(&sref)
            .map_err(|e| fail(auth_step, e))?
            .ok_or_else(|| {
                fail(
                    auth_step,
                    format!("secret ref {sref} missing from the mail secret store"),
                )
            })?;
        authenticate_either(ops, api, &username, &admin_pw)
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
            api.bind_setup_http_loopback()?;
            ops.systemctl(&["restart", STALWART_UNIT])?;
            let username = progress_extra("adminUsername")
                .unwrap_or_else(|| format!("admin@{default_domain}"));
            let sref = row_field("admin_secret_ref").ok_or_else(|| {
                "admin secret ref missing — re-run enable".to_string()
            })?;
            let admin_pw = secrets.resolve(&sref)?.ok_or_else(|| {
                format!("secret ref {sref} missing from the mail secret store")
            })?;
            authenticate_either(ops, api, &username, &admin_pw)?;
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
            fail("api-key", "service account id missing from progress state".to_string())
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
            ops.write_file(STALWART_UNIT_PATH, systemd_unit(None).as_bytes(), 0o600)?;
            ops.systemctl(&["daemon-reload"])?;
            rotate_leftover_admins(api, secrets, &default_domain)?;
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
            Err(e) => last_err = e,
        }
        if attempt + 1 < AUTH_RETRIES {
            ops.sleep_ms(1000);
        }
    }
    Err(format!("management API not reachable at {base_url}: {last_err}"))
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
    let recovery_current = progress_extra("recoveryAdminRef")
        .and_then(|sref| secrets.resolve(&sref).ok().flatten())
        .unwrap_or_default();
    let recovery_new = generate_secret()?;
    api.rotate_admin_secret("admin", &recovery_current, &recovery_new)?;
    if let Some(sref) = progress_extra("recoveryAdminRef") {
        let _ = secrets.delete(&sref);
    }

    let username = progress_extra("adminUsername")
        .unwrap_or_else(|| format!("admin@{default_domain}"));
    let sref = row_field("admin_secret_ref").ok_or_else(|| {
        "admin secret ref missing — cannot rotate the provisioned admin".to_string()
    })?;
    let current = secrets.resolve(&sref)?.ok_or_else(|| {
        format!("secret ref {sref} missing from the mail secret store")
    })?;
    let new_pw = generate_secret()?;
    api.rotate_admin_secret(&username, &current, &new_pw)?;
    let new_ref = secrets.store("admin", &new_pw)?;
    set_row_field("admin_secret_ref", &new_ref);
    Ok(RotateAdminReport {
        recovery_principal: "admin".to_string(),
        provisioned_admin: username,
    })
}

/// Live entry for POST `/cli/mail/server/rotate-admin`: talk to the
/// running server over the loopback ApiKey (same as doctor/domains).
/// Does not wipe the store. Rotates leftover principal `admin` when
/// present. Stored `adminUsername` (`admin@domain`) is rotated only if
/// that account exists — noir 0.40.149 stored the name from Bootstrap
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

    let username = progress_extra("adminUsername")
        .unwrap_or_else(|| format!("admin@{default_domain}"));
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
fn authenticate_either(
    ops: &dyn SystemOps,
    api: &mut dyn BootstrapApi,
    username: &str,
    password: &str,
) -> Result<(), String> {
    // One quick probe first (no retry storm when the other URL is the
    // live one).
    if api.authenticate(STALWART_SETUP_URL, username, password).is_ok() {
        return Ok(());
    }
    if api.authenticate(STALWART_MGMT_URL, username, password).is_ok() {
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
pub fn health_check_with(
    ops: &dyn SystemOps,
    api_ping: &dyn Fn() -> Result<(), String>,
) -> Health {
    if !ops.path_exists(STALWART_BIN) {
        return Health::NotInstalled;
    }
    let active = ops.systemctl_query(&["is-active", STALWART_UNIT]);
    if active != "active" {
        return Health::Stopped(format!(
            "systemd reports the stalwart unit is '{}'",
            if active.is_empty() { "unknown" } else { &active }
        ));
    }
    match api_ping() {
        Ok(()) => Health::Running,
        Err(e) => Health::Degraded(format!("unit is active but the management API ping failed: {e}")),
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

/// Post-S1 — explicit pinned upgrade: snapshot config + data dir,
/// swap the verified binary, health-check, auto-rollback on failure
/// (pre-mortem #8). NEVER called from any auto-update path.
#[allow(dead_code)] // wired when the first pin bump ships.
pub fn upgrade(to_version: &str) -> Result<(), String> {
    let _ = to_version;
    Err(super::not_built_err("S1", "mail supervisor upgrade"))
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::sysops::fake::FakeSystemOps;
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;

    /// The capability gate matches the compile target — on macOS dev
    /// boxes this asserts FALSE (and the module still compiled + ran,
    /// which is the point of the runtime cfg!).
    #[test]
    fn mail_supported_matches_target_os() {
        assert_eq!(mail_supported(), cfg!(target_os = "linux"));
    }

    #[test]
    fn pinned_artifacts_have_real_checksums_and_urls() {
        assert_eq!(STALWART_PINNED_VERSION, "0.16.10");
        for art in STALWART_SHA256 {
            assert!(
                !checksum_is_placeholder(art),
                "{}: pinned checksum must be real 64-hex",
                art.arch
            );
        }
        let art = artifact_for_arch("x86_64").expect("x86_64 supported");
        assert_eq!(
            tarball_url(art.triple),
            "https://github.com/Alakazam-211/stalwart/releases/download/v0.16.10/stalwart-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert!(artifact_for_arch("aarch64").is_ok());
        let err = artifact_for_arch("riscv64").expect_err("unsupported arch");
        assert!(err.contains("riscv64"), "{err}");
    }

    #[test]
    fn placeholder_checksum_refuses_install_before_any_effect() {
        let _g = db_guard();
        clean_row();
        let art = StalwartArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            sha256: "PLACEHOLDER_PLACEHOLDER_PLACEHOLDER_PLACEHOLDER_PLACEHOLDER_PLAC",
        };
        let ops = FakeSystemOps::default();
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect_err("must refuse");
        assert!(err.contains("placeholder"), "{err}");
        assert!(ops.recorded().is_empty(), "no effect may precede the guard");
        clean_row();
    }

    #[test]
    fn hardening_dropin_is_prd_10_verbatim_plus_config_dir() {
        let d = hardening_dropin();
        for directive in [
            "ProtectSystem=strict",
            "ProtectHome=yes",
            // /etc/stalwart is live-verified load-bearing: Bootstrap/set
            // writes config.json as the stalwart user.
            "ReadWritePaths=/var/lib/stalwart /var/log/stalwart /etc/stalwart",
            "NoNewPrivileges=yes",
            "PrivateTmp=yes",
            "CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE",
            "Restart=on-failure",
        ] {
            assert!(d.contains(directive), "missing {directive}");
        }
        let unit = systemd_unit(Some("recovery-pw"));
        assert!(unit.contains("ExecStart=/usr/local/bin/stalwart --config /etc/stalwart/config.json"));
        assert!(unit.contains("User=stalwart"));
        assert!(unit.contains("Environment=STALWART_RECOVERY_ADMIN=admin:recovery-pw"));
        // The recovery-off rewrite drops the credential entirely.
        let stripped = systemd_unit(None);
        assert!(!stripped.contains("STALWART_RECOVERY_ADMIN"));
    }

    #[test]
    fn default_domain_strips_the_host_label() {
        assert_eq!(default_domain_for("mail.acme.dev"), "acme.dev");
        assert_eq!(default_domain_for("mail.k2livebox.test"), "k2livebox.test");
        assert_eq!(default_domain_for("mx.mail.acme.dev"), "mail.acme.dev");
        // Two labels: the hostname IS the domain.
        assert_eq!(default_domain_for("acme.dev"), "acme.dev");
    }

    // ── Enable-machine fakes ────────────────────────────────────────

    #[derive(Default)]
    struct FakeApi {
        calls: Vec<String>,
        fail_on: Option<&'static str>,
        /// URLs authenticate() must refuse (post-restart resume story).
        refuse_urls: Vec<&'static str>,
    }

    impl FakeApi {
        fn check(&mut self, call: &str) -> Result<(), String> {
            self.calls.push(call.to_string());
            if self.fail_on == Some(call.split_whitespace().next().unwrap_or_default()) {
                return Err(format!("injected {call} failure"));
            }
            Ok(())
        }
    }

    impl BootstrapApi for FakeApi {
        fn authenticate(&mut self, base: &str, user: &str, _pw: &str) -> Result<(), String> {
            if self.refuse_urls.contains(&base) {
                self.calls.push(format!("authenticate-refused {base} {user}"));
                return Err(format!("connection refused: {base}"));
            }
            self.check(&format!("authenticate {base} {user}"))
        }
        fn complete_bootstrap(
            &mut self,
            hostname: &str,
            default_domain: &str,
            request_tls: bool,
        ) -> Result<AdminCredentials, String> {
            self.check(&format!(
                "complete_bootstrap {hostname} {default_domain} tls={request_tls}"
            ))?;
            Ok(AdminCredentials {
                username: format!("admin@{default_domain}"),
                secret: "provisioned-admin-secret".into(),
            })
        }
        fn set_server_hostname(&mut self, hostname: &str) -> Result<(), String> {
            self.check(&format!("set_server_hostname {hostname}"))
        }
        fn bind_setup_http_loopback(&mut self) -> Result<(), String> {
            self.check("bind_setup_http_loopback")
        }
        fn configure_listeners(&mut self, plan: &str) -> Result<(), String> {
            self.check(&format!("configure_listeners {plan}"))
        }
        fn create_service_account(&mut self, default_domain: &str) -> Result<String, String> {
            self.check(&format!("create_service_account {default_domain}"))?;
            Ok("acct-k2".into())
        }
        fn mint_api_key(&mut self, account_id: &str) -> Result<String, String> {
            self.check(&format!("mint_api_key {account_id}"))?;
            Ok("API_minted-key-secret".into())
        }
        fn rotate_admin_secret(
            &mut self,
            username: &str,
            _current: &str,
            new_secret: &str,
        ) -> Result<(), String> {
            self.check(&format!("rotate_admin_secret {username} {}", new_secret.len()))
        }
    }

    #[derive(Default)]
    struct FakeSecrets {
        stored: Mutex<Vec<(String, String)>>,
        deleted: Mutex<Vec<String>>,
    }

    impl SecretStore for FakeSecrets {
        fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
            self.stored
                .lock()
                .unwrap()
                .push((kind.to_string(), secret.to_string()));
            Ok(format!("mailsec_{kind}_test"))
        }
        fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
            Ok(self
                .stored
                .lock()
                .unwrap()
                .iter()
                .find(|(k, _)| sref.contains(k.as_str()))
                .map(|(_, s)| s.clone()))
        }
        fn delete(&self, sref: &str) -> Result<(), String> {
            self.deleted.lock().unwrap().push(sref.to_string());
            Ok(())
        }
    }

    const FAKE_BINARY: &[u8] = b"stalwart-binary-bytes-for-tests";

    fn fake_artifact() -> StalwartArtifact {
        let mut h = Sha256::new();
        h.update(FAKE_BINARY);
        let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        // Leak the hex into 'static — tests only.
        StalwartArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            sha256: Box::leak(hex.into_boxed_str()),
        }
    }

    fn db_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::mail::mail_server_test_lock()
    }

    fn clean_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
    }

    #[test]
    fn enable_machine_runs_the_full_sequence_and_lands_running() {
        let _g = db_guard();
        clean_row();
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        let art = fake_artifact();

        run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect("full enable succeeds");

        // The system-effect sequence, in order.
        assert_eq!(
            ops.recorded(),
            vec![
                format!("download {}", tarball_url(art.triple)),
                "extract stalwart -> /usr/local/bin/stalwart (mode 755)".to_string(),
                "useradd stalwart".to_string(),
                "mkdir /etc/stalwart".to_string(),
                "mkdir /var/lib/stalwart".to_string(),
                "mkdir /var/log/stalwart".to_string(),
                "chown stalwart /etc/stalwart".to_string(),
                "chown stalwart /var/lib/stalwart".to_string(),
                "chown stalwart /var/log/stalwart".to_string(),
                format!(
                    "write /etc/systemd/system/stalwart.service ({} bytes, mode 600)",
                    systemd_unit(Some(&secrets_stored_secret(&secrets, "recovery-admin"))).len()
                ),
                "mkdir /etc/systemd/system/stalwart.service.d".to_string(),
                format!(
                    "write /etc/systemd/system/stalwart.service.d/k2-hardening.conf ({} bytes, mode 644)",
                    hardening_dropin().len()
                ),
                "systemctl daemon-reload".to_string(),
                "systemctl enable --now stalwart".to_string(),
                "systemctl restart stalwart".to_string(),
                "systemctl restart stalwart".to_string(),
                format!(
                    "write /etc/systemd/system/stalwart.service ({} bytes, mode 600)",
                    systemd_unit(None).len()
                ),
                "systemctl daemon-reload".to_string(),
                "systemctl restart stalwart".to_string(),
            ]
        );
        // The management-API sequence, in order: recovery-admin auth →
        // guided setup → provisioned-admin auth → port plan → service
        // account → api key.
        assert_eq!(
            api.calls,
            vec![
                "authenticate http://127.0.0.1:8080 admin",
                "complete_bootstrap mail.acme.dev acme.dev tls=true",
                "authenticate http://127.0.0.1:8080 admin@acme.dev",
                "bind_setup_http_loopback",
                "authenticate http://127.0.0.1:8080 admin@acme.dev",
                "configure_listeners tls-alpn",
                "create_service_account acme.dev",
                "mint_api_key acct-k2",
                "rotate_admin_secret admin 64",
                "rotate_admin_secret admin@acme.dev 64",
            ]
        );
        // Secrets: recovery admin + provisioned admin + api key + rotated
        // admin; recovery deleted by recovery-off.
        {
            let stored = secrets.stored.lock().unwrap();
            assert_eq!(stored.len(), 4);
            assert_eq!(stored[0].0, "recovery-admin");
            assert_eq!(stored[0].1.len(), 64, "generated recovery password");
            assert_eq!(stored[1].0, "admin");
            assert_eq!(stored[1].1, "provisioned-admin-secret");
            assert_eq!(stored[2].0, "api-key");
            assert_eq!(stored[2].1, "API_minted-key-secret");
            assert_eq!(stored[3].0, "admin");
            assert_eq!(stored[3].1.len(), 64, "rotated admin password");
            assert_ne!(stored[3].1, stored[1].1, "C20 recovery-off rotates the admin secret");
            assert_eq!(
                *secrets.deleted.lock().unwrap(),
                vec!["mailsec_recovery-admin_test".to_string()],
                "recovery-off deletes the recovery secret"
            );
        }
        // Row landed running with refs + urls + version.
        assert_eq!(current_status().as_deref(), Some("running"));
        assert_eq!(row_field("api_url").as_deref(), Some(STALWART_MGMT_URL));
        assert_eq!(row_field("api_key_ref").as_deref(), Some("mailsec_api-key_test"));
        assert_eq!(row_field("admin_secret_ref").as_deref(), Some("mailsec_admin_test"));
        assert_eq!(row_field("installed_version").as_deref(), Some(STALWART_PINNED_VERSION));
        assert_eq!(row_field("last_error"), None);
        // Every step marked done.
        for step in ENABLE_STEPS {
            assert!(step_is_done(step), "step {step} not marked done");
        }
        clean_row();
    }

    /// L12: recovery-off / rotate-admin must rotate bootstrap principal
    /// `admin`, not only `admin@domain`.
    #[test]
    fn rotate_leftover_admins_records_bootstrap_principal_admin() {
        let _g = db_guard();
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, \
                 admin_secret_ref, updated_at) \
                 VALUES (1, 'running', ?1, 'mail.acme.dev', 'mailsec_admin_test', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        let secrets = FakeSecrets::default();
        secrets
            .store("admin", "old-provisioned")
            .expect("vault admin");
        progress_extra_set("adminUsername", "admin@acme.dev");
        let mut api = FakeApi::default();
        let report = rotate_leftover_admins(&mut api, &secrets, "acme.dev").expect("rotate");
        assert_eq!(report.recovery_principal, "admin");
        assert_eq!(report.provisioned_admin, "admin@acme.dev");
        assert_eq!(
            api.calls,
            vec![
                "rotate_admin_secret admin 64",
                "rotate_admin_secret admin@acme.dev 64",
            ],
            "must rotate leftover principal admin, not only admin@domain"
        );
        let mut fail_api = FakeApi {
            fail_on: Some("rotate_admin_secret"),
            ..FakeApi::default()
        };
        let err = rotate_leftover_admins(&mut fail_api, &secrets, "acme.dev")
            .expect_err("rotate of admin must fail loud");
        assert!(err.contains("injected"), "{err}");
        assert!(
            fail_api.calls.iter().any(|c| c.starts_with("rotate_admin_secret admin ")),
            "failure is on principal admin: {:?}",
            fail_api.calls
        );
        clean_row();
    }

    /// Extract the stored secret of `kind` (test helper for the unit
    /// byte-length assertion above).
    fn secrets_stored_secret(secrets: &FakeSecrets, kind: &str) -> String {
        secrets
            .stored
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, s)| s.clone())
            .expect("secret stored")
    }

    #[test]
    fn enable_resumes_after_a_mid_flow_failure_without_redoing_work() {
        let _g = db_guard();
        clean_row();
        let art = fake_artifact();

        // First run: the service-account call fails mid-provisioning.
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi { fail_on: Some("create_service_account"), ..FakeApi::default() };
        let secrets = FakeSecrets::default();
        let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "http-01")
            .expect_err("injected failure");
        assert!(err.starts_with("service-account:"), "{err}");
        assert_eq!(current_status().as_deref(), Some("error"));
        assert!(row_field("last_error").expect("recorded").contains("injected"));

        // Second run: binary already on disk + steps marked — resume
        // must NOT re-download/extract/start/re-bootstrap, must
        // re-authenticate as the PROVISIONED admin, and must finish.
        let ops2 = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            existing_paths: vec![STALWART_BIN.to_string()],
            ..FakeSystemOps::default()
        };
        let mut api2 = FakeApi::default();
        run_enable(&ops2, &mut api2, &secrets, &art, "mail.acme.dev", "http-01")
            .expect("resume succeeds");
        let ops_lines = ops2.recorded();
        assert!(
            !ops_lines.iter().any(|l| l.starts_with("download")),
            "resume must not re-download: {ops_lines:?}"
        );
        assert!(
            !ops_lines.iter().any(|l| l.contains("enable --now")),
            "resume must not re-run completed start: {ops_lines:?}"
        );
        // The API sequence resumes AT the failed step with a fresh
        // admin session (bootstrap NOT re-run — the admin is already
        // provisioned and vaulted).
        assert_eq!(
            api2.calls,
            vec![
                "authenticate http://127.0.0.1:8080 admin@acme.dev",
                "create_service_account acme.dev",
                "mint_api_key acct-k2",
                "rotate_admin_secret admin 64",
                "rotate_admin_secret admin@acme.dev 64",
            ]
        );
        assert_eq!(current_status().as_deref(), Some("running"));
        clean_row();
    }

    /// A resume AFTER the final restart finds :8080 dead — the machine
    /// falls back to the :8180 mgmt listener.
    #[test]
    fn resume_auth_falls_back_to_the_mgmt_listener() {
        let _g = db_guard();
        clean_row();
        let art = fake_artifact();
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            ..FakeSystemOps::default()
        };
        // First run fails at api-key (after server-config +
        // service-account) — the retry lands post-restart in spirit.
        let mut api = FakeApi { fail_on: Some("mint_api_key"), ..FakeApi::default() };
        let secrets = FakeSecrets::default();
        let _ = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "http-01")
            .expect_err("injected failure");

        let ops2 = FakeSystemOps {
            existing_paths: vec![STALWART_BIN.to_string()],
            ..FakeSystemOps::default()
        };
        let mut api2 = FakeApi { refuse_urls: vec![STALWART_SETUP_URL], ..FakeApi::default() };
        run_enable(&ops2, &mut api2, &secrets, &art, "mail.acme.dev", "http-01")
            .expect("resume succeeds via mgmt listener");
        assert!(
            api2.calls
                .contains(&format!("authenticate {STALWART_MGMT_URL} admin@acme.dev")),
            "{:?}",
            api2.calls
        );
        assert_eq!(current_status().as_deref(), Some("running"));
        clean_row();
    }

    #[test]
    fn checksum_mismatch_aborts_before_extract_and_records_error() {
        let _g = db_guard();
        clean_row();
        let ops = FakeSystemOps {
            download_body: b"tampered bytes".to_vec(),
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        let art = fake_artifact(); // hash of FAKE_BINARY ≠ hash of tampered bytes
        let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect_err("mismatch must abort");
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert!(err.contains("NOT installing"), "{err}");
        let lines = ops.recorded();
        assert_eq!(lines.len(), 1, "download only — nothing extracted/installed: {lines:?}");
        assert!(api.calls.is_empty());
        assert_eq!(current_status().as_deref(), Some("error"));
        assert!(row_field("last_error").expect("recorded").contains("sha256"));
        clean_row();
    }

    #[test]
    fn version_pin_refuses_to_manage_a_different_installed_version() {
        let _g = db_guard();
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, installed_version, updated_at) \
                 VALUES (1, 'running', ?1, '0.17.0', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        let ops = FakeSystemOps::default();
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        let art = fake_artifact();
        let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect_err("must refuse");
        assert!(err.contains("0.17.0"), "{err}");
        assert!(err.contains("refusing"), "{err}");
        assert!(ops.recorded().is_empty(), "no writes when refusing (PRD §4)");
        clean_row();
    }

    #[test]
    fn stale_config_file_is_cleared_before_first_start() {
        let _g = db_guard();
        clean_row();
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            existing_paths: vec![STALWART_CONFIG.to_string()],
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        let art = fake_artifact();
        let _ = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "http-01");
        assert!(
            !ops.recorded().iter().any(|l| l.contains(&format!("rm {STALWART_CONFIG}"))),
            "C19: never wipe an existing config.json: {:?}",
            ops.recorded()
        );
        assert!(
            !api.calls.iter().any(|c| c.starts_with("complete_bootstrap")),
            "C19: never re-bootstrap an initialized store: {:?}",
            api.calls
        );
        clean_row();
    }

    #[test]
    fn health_verdicts_map_binary_unit_and_ping() {
        // Not installed: no binary.
        let ops = FakeSystemOps::default();
        assert_eq!(health_check_with(&ops, &|| Ok(())), Health::NotInstalled);

        // Stopped: binary there, unit inactive.
        let ops = FakeSystemOps {
            existing_paths: vec![STALWART_BIN.to_string()],
            query_answers: [("is-active stalwart".to_string(), "inactive".to_string())]
                .into_iter()
                .collect(),
            ..FakeSystemOps::default()
        };
        let h = health_check_with(&ops, &|| Ok(()));
        assert_eq!(h.as_status_str(), "stopped");
        assert!(h.detail().expect("detail").contains("inactive"));

        // Degraded: active but API ping fails.
        let ops = FakeSystemOps {
            existing_paths: vec![STALWART_BIN.to_string()],
            query_answers: [("is-active stalwart".to_string(), "active".to_string())]
                .into_iter()
                .collect(),
            ..FakeSystemOps::default()
        };
        let h = health_check_with(&ops, &|| Err("connection refused".into()));
        assert_eq!(h.as_status_str(), "degraded");
        assert!(h.detail().expect("detail").contains("connection refused"));

        // Running: active + ping ok.
        assert_eq!(health_check_with(&ops, &|| Ok(())), Health::Running);
    }

    #[test]
    fn reconcile_status_never_running_unless_unit_active() {
        let running_inactive = reconcile_reported_status("running", None, "inactive");
        assert_eq!(running_inactive.state, "stopped");
        assert!(!running_inactive.consistent, "H2: running row + inactive unit disagree");
        assert_eq!(
            running_inactive.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'inactive'")
        );

        let running_failed = reconcile_reported_status("running", None, "failed");
        assert_eq!(running_failed.state, "error");
        assert!(!running_failed.consistent);
        assert_eq!(
            running_failed.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'failed'")
        );

        // Unknown (empty is-active output) is not active.
        let running_unknown = reconcile_reported_status("running", Some("old"), "  ");
        assert_eq!(running_unknown.state, "stopped");
        assert!(!running_unknown.consistent);
        assert_eq!(
            running_unknown.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'unknown'")
        );

        let degraded_inactive = reconcile_reported_status("degraded", Some("ping"), "inactive");
        assert_eq!(degraded_inactive.state, "stopped");
        assert!(!degraded_inactive.consistent);

        let running_active = reconcile_reported_status("running", None, "active");
        assert_eq!(
            running_active,
            ReconciledStatus { state: "running".into(), consistent: true, last_error: None }
        );

        let degraded_active =
            reconcile_reported_status("degraded", Some("connection refused"), "active");
        assert_eq!(
            degraded_active,
            ReconciledStatus {
                state: "degraded".into(),
                consistent: true,
                last_error: Some("connection refused".into()),
            }
        );
    }

    #[test]
    fn reconcile_status_disabled_agrees_with_a_down_unit() {
        let disabled_inactive = reconcile_reported_status("disabled", None, "inactive");
        assert_eq!(
            disabled_inactive,
            ReconciledStatus { state: "disabled".into(), consistent: true, last_error: None },
            "disabled + inactive is agreement, not an error"
        );

        // A recorded (non-systemd) cause is passed through untouched.
        let disabled_kept = reconcile_reported_status("disabled", Some("owner note"), "failed");
        assert_eq!(disabled_kept.state, "disabled");
        assert!(disabled_kept.consistent);
        assert_eq!(disabled_kept.last_error.as_deref(), Some("owner note"));

        let disabled_active = reconcile_reported_status("disabled", None, "active");
        assert_eq!(disabled_active.state, "disabled");
        assert!(!disabled_active.consistent, "unit running while disabled disagrees");
        assert_eq!(
            disabled_active.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'active' while hostmail is disabled")
        );
    }

    #[test]
    fn reconcile_status_error_and_stopped_rows() {
        let error_failed = reconcile_reported_status("error", Some("bootstrap failed"), "failed");
        assert_eq!(
            error_failed,
            ReconciledStatus {
                state: "error".into(),
                consistent: true,
                last_error: Some("bootstrap failed".into()),
            }
        );
        let error_inactive = reconcile_reported_status("error", None, "inactive");
        assert_eq!(error_inactive.state, "error");
        assert!(error_inactive.consistent);
        assert_eq!(
            error_inactive.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'inactive'")
        );
        let error_active = reconcile_reported_status("error", Some("bootstrap failed"), "active");
        assert_eq!(error_active.state, "error");
        assert!(!error_active.consistent);
        assert_eq!(
            error_active.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'active' while hostmail status is 'error'")
        );

        let stopped_inactive = reconcile_reported_status("stopped", None, "inactive");
        assert_eq!(stopped_inactive.state, "stopped");
        assert!(stopped_inactive.consistent);
        assert_eq!(
            stopped_inactive.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'inactive'")
        );
        let stopped_active = reconcile_reported_status("stopped", None, "active");
        assert_eq!(stopped_active.state, "stopped");
        assert!(!stopped_active.consistent);
        assert_eq!(
            stopped_active.last_error.as_deref(),
            Some("systemd reports the stalwart unit is 'active' while hostmail status is 'stopped'")
        );
    }

    #[test]
    fn reconcile_status_install_states_do_not_invent_a_unit() {
        for row in ["installing", "not-installed"] {
            for unit in ["inactive", "active", "failed", ""] {
                let r = reconcile_reported_status(row, Some("step 3"), unit);
                assert_eq!(
                    r,
                    ReconciledStatus {
                        state: row.into(),
                        consistent: true,
                        last_error: Some("step 3".into()),
                    },
                    "row={row} unit={unit:?}"
                );
            }
        }
    }

    #[test]
    fn already_enabled_requires_active_unit() {
        assert!(is_already_enabled_with(Some("running"), "active"));
        assert!(
            !is_already_enabled_with(Some("running"), "inactive"),
            "H3: SQLite running + inactive unit is NOT alreadyEnabled"
        );
        assert!(!is_already_enabled_with(Some("running"), ""));
        assert!(!is_already_enabled_with(Some("disabled"), "active"));
        assert!(!is_already_enabled_with(None, "active"));
    }

    #[test]
    fn already_enabled_healthy_store_is_noop_when_unit_active() {
        let _g = db_guard();
        clean_row();
        set_test_store_ready(Some(true));
        let _unit = with_test_unit_state("active");
        assert!(
            is_already_enabled(),
            "C19: unit active + store ready is alreadyEnabled even if sqlite is empty"
        );
        set_test_store_ready(None);
        clean_row();
    }

    fn progress_all_steps_except_start(admin_username: &str) -> String {
        let mut steps = serde_json::Map::new();
        for s in ENABLE_STEPS {
            if *s != "start" {
                steps.insert((*s).to_string(), serde_json::json!({ "at": 1 }));
            }
        }
        serde_json::json!({
            "steps": steps,
            "adminUsername": admin_username,
        })
        .to_string()
    }

    /// L2: disable then enable with a **new** hostname patches
    /// SystemSettings via the registry — does not re-bootstrap.
    #[test]
    fn hostname_retarget_sets_registry_hostname_without_rebootstrap() {
        let _g = db_guard();
        clean_row();
        let secrets = FakeSecrets::default();
        secrets.store("admin", "admin-secret").expect("vault admin");
        let progress = progress_all_steps_except_start("admin@old.dev");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, installed_version, \
                 hostname, admin_secret_ref, enable_progress_json, updated_at) \
                 VALUES (1, 'disabled', ?1, ?1, 'mail.old.dev', 'mailsec_admin_test', ?2, 100)",
                rusqlite::params![STALWART_PINNED_VERSION, progress],
            )
            .expect("seed disabled row");
        }
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            existing_paths: vec![
                STALWART_BIN.to_string(),
                STALWART_CONFIG.to_string(),
            ],
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let art = fake_artifact();
        run_enable(&ops, &mut api, &secrets, &art, "mail.new.dev", "tls-alpn")
            .expect("retarget enable");
        assert!(
            !api.calls.iter().any(|c| c.starts_with("complete_bootstrap")),
            "L2 must not re-bootstrap: {:?}",
            api.calls
        );
        assert!(
            api.calls.iter().any(|c| c == "set_server_hostname mail.new.dev"),
            "L2 must patch server hostname via registry: {:?}",
            api.calls
        );
        assert_eq!(row_field("hostname").as_deref(), Some("mail.new.dev"));
        clean_row();
    }

    #[test]
    fn hostname_unchanged_after_disable_does_not_retarget() {
        let _g = db_guard();
        clean_row();
        let secrets = FakeSecrets::default();
        secrets.store("admin", "admin-secret").expect("vault admin");
        let progress = progress_all_steps_except_start("admin@acme.dev");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, installed_version, \
                 hostname, admin_secret_ref, enable_progress_json, updated_at) \
                 VALUES (1, 'disabled', ?1, ?1, 'mail.acme.dev', 'mailsec_admin_test', ?2, 100)",
                rusqlite::params![STALWART_PINNED_VERSION, progress],
            )
            .expect("seed disabled row");
        }
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            existing_paths: vec![
                STALWART_BIN.to_string(),
                STALWART_CONFIG.to_string(),
            ],
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let art = fake_artifact();
        run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect("re-enable same hostname");
        assert!(
            !api.calls.iter().any(|c| c.starts_with("set_server_hostname")),
            "same hostname is not a retarget: {:?}",
            api.calls
        );
        assert!(
            !api.calls.iter().any(|c| c.starts_with("complete_bootstrap")),
            "must not re-bootstrap: {:?}",
            api.calls
        );
        clean_row();
    }

    #[test]
    fn fail_after_unit_strips_recovery_admin() {
        let _g = db_guard();
        clean_row();
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi {
            fail_on: Some("complete_bootstrap"),
            ..FakeApi::default()
        };
        let secrets = FakeSecrets::default();
        let art = fake_artifact();
        let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect_err("injected bootstrap failure");
        assert!(err.starts_with("bootstrap:"), "{err}");
        let unit = ops
            .written
            .lock()
            .unwrap()
            .get(STALWART_UNIT_PATH)
            .cloned()
            .expect("unit written");
        let text = String::from_utf8_lossy(&unit);
        assert!(
            !text.contains("STALWART_RECOVERY_ADMIN"),
            "C20: failed enable after unit must strip recovery admin: {text}"
        );
        clean_row();
    }

    #[test]
    fn disable_keeps_data_and_uninstall_purge_removes_everything() {
        let _g = db_guard();
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, \
                 admin_secret_ref, api_key_ref, updated_at) \
                 VALUES (1, 'running', ?1, 'mail.acme.dev', 'mailsec_admin_x', \
                 'mailsec_api-key_y', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }

        let ops = FakeSystemOps::default();
        disable_with(&ops).expect("disable");
        assert_eq!(ops.recorded(), vec!["systemctl disable --now stalwart"]);
        assert_eq!(current_status().as_deref(), Some("disabled"));

        let ops = FakeSystemOps::default();
        let secrets = FakeSecrets::default();
        uninstall_with(&ops, &secrets, true).expect("uninstall");
        assert_eq!(
            ops.recorded(),
            vec![
                "systemctl disable --now stalwart",
                "rm /etc/systemd/system/stalwart.service",
                "rm /etc/systemd/system/stalwart.service.d",
                "systemctl daemon-reload",
                "rm /usr/local/bin/stalwart",
                "rm /var/lib/stalwart",
                "rm /var/log/stalwart",
                "rm /etc/stalwart",
            ]
        );
        assert_eq!(
            *secrets.deleted.lock().unwrap(),
            vec!["mailsec_admin_x".to_string(), "mailsec_api-key_y".to_string()]
        );
        assert_eq!(current_status(), None, "row deleted → not-installed");

        // Uninstall WITHOUT purge keeps the data dirs.
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, updated_at) \
                 VALUES (1, 'disabled', ?1, 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        let ops = FakeSystemOps::default();
        uninstall_with(&ops, &FakeSecrets::default(), false).expect("uninstall no purge");
        let lines = ops.recorded();
        assert!(!lines.iter().any(|l| l.contains("/var/lib/stalwart")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("rm /etc/stalwart")), "{lines:?}");
        clean_row();
    }

    #[test]
    fn enable_after_disable_restarts_unit_and_persists_hostname() {
        let _g = db_guard();
        clean_row();
        let art = fake_artifact();
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        run_enable(&ops, &mut api, &secrets, &art, "mail.lztek.k2.dev", "http-01")
            .expect("first enable");
        assert!(step_is_done("start"), "start marked after first enable");

        let ops_dis = FakeSystemOps::default();
        disable_with(&ops_dis).expect("disable");
        assert_eq!(current_status().as_deref(), Some("disabled"));
        assert!(
            !step_is_done("start"),
            "disable must clear the start resume mark (H4)"
        );
        assert_eq!(row_field("hostname").as_deref(), Some("mail.lztek.k2.dev"));

        let ops2 = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            existing_paths: vec![STALWART_BIN.to_string()],
            ..FakeSystemOps::default()
        };
        let mut api2 = FakeApi::default();
        run_enable(
            &ops2,
            &mut api2,
            &secrets,
            &art,
            "mail.lztek.io",
            "http-01",
        )
        .expect("re-enable after disable");
        let lines = ops2.recorded();
        assert!(
            lines.iter().any(|l| l == "systemctl daemon-reload"),
            "H4: enable-after-disable must daemon-reload: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("enable --now")),
            "H4: enable-after-disable must enable --now: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "systemctl restart stalwart"),
            "H4: enable-after-disable must restart: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("download")),
            "re-enable must not re-download: {lines:?}"
        );
        assert_eq!(
            row_field("hostname").as_deref(),
            Some("mail.lztek.io"),
            "H5: enable --hostname after disable persists the new hostname"
        );
        assert_eq!(current_status().as_deref(), Some("running"));
        clean_row();
    }

    #[test]
    fn enable_latch_is_exclusive() {
        // Serialized with the other latch users via the DB guard.
        let _g = db_guard();
        assert!(try_begin_enable());
        assert!(!try_begin_enable(), "second claim must fail while held");
        end_enable();
        assert!(try_begin_enable());
        end_enable();
    }

    fn mint_leaf_der(cn: &str, org: Option<&str>) -> Vec<u8> {
        use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, cn);
        if let Some(org) = org {
            dn.push(DnType::OrganizationName, org);
        }
        params.distinguished_name = dn;
        let cert = params.self_signed(&key).expect("self-sign");
        cert.der().to_vec()
    }

    /// L1: a captured rcgen leaf is self-signed, never missing+false.
    #[test]
    fn tls_cert_status_rcgen_leaf_is_self_signed() {
        let der = mint_leaf_der("rcgen self signed cert", None);
        let _g = with_test_tls_cert(TlsProbe::Handshake { leaf_der: der }, false);
        let v = tls_cert_status(Some("mail.acme.dev"));
        assert_eq!(
            v["state"].as_str().expect("state"),
            "self-signed",
            "{v}"
        );
        assert_eq!(
            v["selfSigned"].as_bool().expect("selfSigned"),
            true,
            "{v}"
        );
        assert!(
            v["expiresAt"].as_i64().expect("expiresAt") > 0,
            "expiresAt parsed: {v}"
        );
        assert_eq!(v["host"].as_str().expect("host"), "mail.acme.dev", "{v}");
        assert_eq!(
            v["names"].as_array().expect("names")[0]
                .as_str()
                .expect("name"),
            "mail.acme.dev",
            "{v}"
        );
    }

    /// L1: handshake + ACME-looking issuer is issued, not self-signed.
    #[test]
    fn tls_cert_status_acme_leaf_is_issued() {
        let der = mint_leaf_der("R3", Some("Let's Encrypt"));
        let _g = with_test_tls_cert(TlsProbe::Handshake { leaf_der: der }, false);
        let v = tls_cert_status(Some("mail.acme.dev"));
        assert_eq!(v["state"].as_str().expect("state"), "issued", "{v}");
        assert_eq!(
            v["selfSigned"].as_bool().expect("selfSigned"),
            false,
            "{v}"
        );
        assert!(
            v["expiresAt"].as_i64().expect("expiresAt") > 0,
            "expiresAt parsed: {v}"
        );
    }

    /// L1: no TCP/TLS at all is missing + selfSigned false.
    #[test]
    fn tls_cert_status_no_tls_is_missing() {
        let _g = with_test_tls_cert(TlsProbe::Missing, true);
        let v = tls_cert_status(Some("mail.acme.dev"));
        assert_eq!(v["state"].as_str().expect("state"), "missing", "{v}");
        assert_eq!(
            v["selfSigned"].as_bool().expect("selfSigned"),
            false,
            "{v}"
        );
        assert!(v["expiresAt"].is_null(), "{v}");
    }

    /// L1: handshake + certs dir with files is issued even when the
    /// issuer is not an ACME household name.
    #[test]
    fn tls_cert_status_handshake_plus_certs_dir_is_issued() {
        let der = mint_leaf_der("mail.acme.dev", Some("Example Org"));
        let _g = with_test_tls_cert(TlsProbe::Handshake { leaf_der: der }, true);
        let v = tls_cert_status(Some("mail.acme.dev"));
        assert_eq!(v["state"].as_str().expect("state"), "issued", "{v}");
        assert_eq!(
            v["selfSigned"].as_bool().expect("selfSigned"),
            false,
            "{v}"
        );
        assert!(
            v["expiresAt"].as_i64().expect("expiresAt") > 0,
            "expiresAt parsed: {v}"
        );
    }

}
