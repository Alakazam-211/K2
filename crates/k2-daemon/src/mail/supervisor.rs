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
/// glibc builds — K2 Linux deployments are Ubuntu/Debian-class boxes
/// (musl/Alpine support would add the musl triples here).
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

/// Upstream release-tarball URL for the pinned version (PRD §4.1 —
/// fetched from Stalwart's own GitHub releases at install time; K2
/// never redistributes the binary).
pub fn tarball_url(triple: &str) -> String {
    format!(
        "https://github.com/stalwartlabs/stalwart/releases/download/v{STALWART_PINNED_VERSION}/stalwart-{triple}.tar.gz"
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

/// Stalwart's first-run bootstrap listener (plain HTTP, the ":8080
/// setup listener" the bootstrap disables as its last act).
pub const STALWART_SETUP_URL: &str = "http://127.0.0.1:8080";

/// The PERMANENT management endpoint the daemon talks to: a dedicated
/// plain-HTTP listener bound to 127.0.0.1 only.
///
/// TLS DECISION (S1, foundation flagged): the mgmt path uses plain
/// HTTP on the loopback, both during bootstrap (Stalwart's own :8080
/// setup listener is plain HTTP) and permanently (this listener).
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
/// Hardening lives in the drop-in so the split mirrors PRD §10
/// verbatim and a future unit rewrite can't silently drop it.
pub fn systemd_unit() -> String {
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
         StandardOutput=journal\n\
         StandardError=journal\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// PRD §10, verbatim.
pub fn hardening_dropin() -> &'static str {
    "[Service]\n\
     ProtectSystem=strict\n\
     ProtectHome=yes\n\
     ReadWritePaths=/var/lib/stalwart /var/log/stalwart\n\
     NoNewPrivileges=yes\n\
     PrivateTmp=yes\n\
     CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n\
     AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
     Restart=on-failure\n"
}

/// Minimal first-boot config: RocksDB stores under the hardened data
/// dir + file logging. Everything else (hostname, listeners, spam
/// filter) is applied over the management API during bootstrap — the
/// v0.16 model keeps runtime config in the database, not this file.
///
/// ⚠ LIVE-BOX VERIFICATION REQUIRED (S1 acceptance, rpm/scratch box):
/// v0.16 rebuilt the config layer ("config.json" per current install
/// docs; pre-0.16 was TOML). The store/directory/tracer key shapes
/// below are the pre-0.16 names carried into JSON as the best
/// documented understanding — verify Stalwart v0.16.10 accepts this
/// file AND still enters bootstrap mode (one-time admin password in
/// the journal) when started with it. If v0.16 wants a different
/// bootstrap arrangement (e.g. empty config, CLI init), this function
/// is the single place to fix.
pub fn initial_config_json() -> String {
    serde_json::json!({
        "storage": {
            "data": "rocksdb",
            "blob": "rocksdb",
            "fts": "rocksdb",
            "lookup": "rocksdb",
            "directory": "internal"
        },
        "store": {
            "rocksdb": {
                "type": "rocksdb",
                "path": format!("{STALWART_DATA_DIR}/data"),
                "compression": "lz4"
            }
        },
        "directory": {
            "internal": { "type": "internal", "store": "rocksdb" }
        },
        "tracer": {
            "log": {
                "type": "log",
                "level": "info",
                "path": STALWART_LOG_DIR,
                "prefix": "stalwart.log",
                "rotate": "daily"
            }
        }
    })
    .to_string()
}

/// Extract the one-time admin password from Stalwart's first-run
/// journal output. Documented format (install docs, v0.16):
///
/// ```text
/// 🔑 Stalwart bootstrap mode - temporary administrator account
///    username: admin   password: XXXXXXXXXXXXXXXX
/// ```
///
/// The LAST occurrence wins (a re-flashed box may have logged more
/// than one bootstrap). Pure parser — unit-tested against the fixture;
/// the exact wording is ⚠ live-box-verified at S1 acceptance.
pub fn parse_bootstrap_admin_password(journal: &str) -> Option<String> {
    let mut armed = false;
    let mut found = None;
    for line in journal.lines() {
        if line.contains("bootstrap mode") {
            armed = true;
            continue;
        }
        if !armed {
            continue;
        }
        if let Some(idx) = line.find("password:") {
            if let Some(tok) = line[idx + "password:".len()..].split_whitespace().next() {
                if !tok.is_empty() {
                    found = Some(tok.to_string());
                }
            }
            armed = false;
        }
    }
    found
}

// ── Management-API seam for the bootstrap sequence ──────────────────────

/// The typed management operations the enable machine performs against
/// Stalwart, as a seam: the real impl is [`super::jmap::StalwartBootstrap`]
/// (HTTP against the loopback mgmt API); tests inject a recording fake.
pub trait BootstrapApi: Send {
    /// Basic-auth session against `base_url` + JMAP endpoint discovery
    /// (`/.well-known/jmap` — never hardcode `/api` vs `/jmap`).
    fn authenticate(&mut self, base_url: &str, username: &str, password: &str)
        -> Result<(), String>;
    fn set_hostname(&mut self, hostname: &str) -> Result<(), String>;
    /// Apply the §5.3 port plan: SMTP 25/465/587, HTTPS per plan
    /// (:443 for `tls-alpn`, 127.0.0.1:8443 otherwise), the loopback
    /// plain-HTTP mgmt listener, and NO IMAP/POP3/ManageSieve/CalDAV/
    /// CardDAV listeners (§10).
    fn configure_listeners(&mut self, port_plan: &str) -> Result<(), String>;
    fn enable_spam_filter(&mut self) -> Result<(), String>;
    /// Create the least-privilege `k2-daemon` service account; returns
    /// its principal id.
    fn create_service_account(&mut self) -> Result<String, String>;
    /// Mint the scoped, localhost-allowlisted ApiKey for the service
    /// account; returns the SECRET (shown once by Stalwart).
    fn mint_api_key(&mut self, account_id: &str) -> Result<String, String>;
    fn rotate_admin_password(&mut self, new_password: &str) -> Result<(), String>;
    /// Disable the :8080 setup listener (pre-mortem #13) — the LAST
    /// API act of bootstrap.
    fn disable_setup_listener(&mut self) -> Result<(), String>;
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
/// renderers (S7 orders the progress list with this; tests assert
/// completeness against it today).
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
    "admin-password",
    "server-config",
    "service-account",
    "api-key",
    "rotate-admin",
    "setup-listener-off",
    "restart",
];

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

    ensure_installing_row(hostname, port_plan)?;

    let fail = |step: &str, err: String| -> String {
        let msg = format!("{step}: {err}");
        set_status("error");
        set_last_error(Some(&msg));
        emit_state_change("installing", "error", Some(&msg));
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

    // ── System user + directories + config + unit ─────────────────
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
            ops.chown_recursive(STALWART_DATA_DIR, STALWART_USER)?;
            ops.chown_recursive(STALWART_LOG_DIR, STALWART_USER)
        })()
        .map_err(|e| fail("dirs", e))?;
        mark_step("dirs");
    }
    if !step_is_done("config") {
        set_current("config");
        ops.write_file(STALWART_CONFIG, initial_config_json().as_bytes(), 0o644)
            .map_err(|e| fail("config", e))?;
        mark_step("config");
    }
    if !step_is_done("unit") {
        set_current("unit");
        (|| -> Result<(), String> {
            ops.write_file(STALWART_UNIT_PATH, systemd_unit().as_bytes(), 0o644)?;
            ops.create_dir_all(STALWART_DROPIN_DIR)?;
            ops.write_file(STALWART_DROPIN_PATH, hardening_dropin().as_bytes(), 0o644)?;
            ops.systemctl(&["daemon-reload"]).map(|_| ())
        })()
        .map_err(|e| fail("unit", e))?;
        mark_step("unit");
    }
    if !step_is_done("start") {
        set_current("start");
        ops.systemctl(&["enable", "--now", STALWART_UNIT])
            .map_err(|e| fail("start", e))?;
        mark_step("start");
    }

    // ── Bootstrap over the management API ──────────────────────────
    // Authentication re-runs on EVERY (re)entry while API steps remain
    // (it isn't a marked step): pre-rotate we use the one-time journal
    // password, post-rotate the vaulted one. Both talk to the :8080
    // setup listener — the loopback mgmt listener only exists after
    // server-config + restart.
    let api_steps_remain = !step_is_done("setup-listener-off");
    if api_steps_remain {
        let password = match row_field("admin_secret_ref") {
            Some(sref) => secrets
                .resolve(&sref)
                .map_err(|e| fail("admin-password", e))?
                .ok_or_else(|| {
                    fail(
                        "admin-password",
                        format!("secret ref {sref} missing from the mail secret store"),
                    )
                })?,
            None => {
                set_current("admin-password");
                let mut captured = None;
                // The password line lands in the journal within the
                // first seconds of the first start; poll briefly.
                for attempt in 0..15 {
                    let journal = ops
                        .journalctl_unit(STALWART_UNIT, 200)
                        .map_err(|e| fail("admin-password", e))?;
                    if let Some(pw) = parse_bootstrap_admin_password(&journal) {
                        captured = Some(pw);
                        break;
                    }
                    if attempt < 14 {
                        ops.sleep_ms(1000);
                    }
                }
                let pw = captured.ok_or_else(|| {
                    fail(
                        "admin-password",
                        "one-time admin password not found in the stalwart journal \
                         (bootstrap-mode line missing) — is the service running?"
                            .to_string(),
                    )
                })?;
                mark_step("admin-password");
                pw
            }
        };
        api.authenticate(STALWART_SETUP_URL, "admin", &password)
            .map_err(|e| fail("admin-password", e))?;
    }

    if !step_is_done("server-config") {
        set_current("server-config");
        (|| -> Result<(), String> {
            api.set_hostname(hostname)?;
            api.configure_listeners(port_plan)?;
            api.enable_spam_filter()
        })()
        .map_err(|e| fail("server-config", e))?;
        mark_step("server-config");
    }

    if !step_is_done("service-account") {
        set_current("service-account");
        let account_id = api
            .create_service_account()
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

    if !step_is_done("rotate-admin") {
        set_current("rotate-admin");
        let new_password = generate_secret().map_err(|e| fail("rotate-admin", e))?;
        api.rotate_admin_password(&new_password)
            .map_err(|e| fail("rotate-admin", e))?;
        let sref = secrets
            .store("admin", &new_password)
            .map_err(|e| fail("rotate-admin", e))?;
        set_row_field("admin_secret_ref", &sref);
        mark_step("rotate-admin");
    }

    if !step_is_done("setup-listener-off") {
        set_current("setup-listener-off");
        api.disable_setup_listener()
            .map_err(|e| fail("setup-listener-off", e))?;
        mark_step("setup-listener-off");
    }

    // Final restart so the bootstrap listener set (mgmt on :8180, SMTP
    // ports, setup listener gone) is live. ⚠ live-box verification:
    // v0.16 may hot-apply listener changes; the restart is belt+braces
    // either way.
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
            "https://github.com/stalwartlabs/stalwart/releases/download/v0.16.10/stalwart-x86_64-unknown-linux-gnu.tar.gz"
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
    fn hardening_dropin_is_prd_10_verbatim() {
        let d = hardening_dropin();
        for directive in [
            "ProtectSystem=strict",
            "ProtectHome=yes",
            "ReadWritePaths=/var/lib/stalwart /var/log/stalwart",
            "NoNewPrivileges=yes",
            "PrivateTmp=yes",
            "CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE",
            "Restart=on-failure",
        ] {
            assert!(d.contains(directive), "missing {directive}");
        }
        let unit = systemd_unit();
        assert!(unit.contains("ExecStart=/usr/local/bin/stalwart --config /etc/stalwart/config.json"));
        assert!(unit.contains("User=stalwart"));
    }

    #[test]
    fn bootstrap_password_parser_reads_the_documented_journal_shape() {
        let journal = "\
starting Stalwart v0.16.10
🔑 Stalwart bootstrap mode - temporary administrator account
   username: admin   password: kHgv8PZLq2NwXbTA
listener started on 0.0.0.0:8080
";
        assert_eq!(
            parse_bootstrap_admin_password(journal).as_deref(),
            Some("kHgv8PZLq2NwXbTA")
        );
        // Last occurrence wins (re-flashed box logged twice).
        let twice = format!(
            "{journal}\n🔑 Stalwart bootstrap mode - temporary administrator account\n\
             username: admin   password: SECONDpw999\n"
        );
        assert_eq!(parse_bootstrap_admin_password(&twice).as_deref(), Some("SECONDpw999"));
        // A stray "password:" with no bootstrap banner does NOT match.
        assert_eq!(
            parse_bootstrap_admin_password("auth failed: bad password: tries=3"),
            None
        );
        assert_eq!(parse_bootstrap_admin_password(""), None);
    }

    // ── Enable-machine fakes ────────────────────────────────────────

    #[derive(Default)]
    struct FakeApi {
        calls: Vec<String>,
        fail_on: Option<&'static str>,
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
            self.check(&format!("authenticate {base} {user}"))
        }
        fn set_hostname(&mut self, hostname: &str) -> Result<(), String> {
            self.check(&format!("set_hostname {hostname}"))
        }
        fn configure_listeners(&mut self, plan: &str) -> Result<(), String> {
            self.check(&format!("configure_listeners {plan}"))
        }
        fn enable_spam_filter(&mut self) -> Result<(), String> {
            self.check("enable_spam_filter")
        }
        fn create_service_account(&mut self) -> Result<String, String> {
            self.check("create_service_account")?;
            Ok("principal-k2".into())
        }
        fn mint_api_key(&mut self, account_id: &str) -> Result<String, String> {
            self.check(&format!("mint_api_key {account_id}"))?;
            Ok("minted-api-key-secret".into())
        }
        fn rotate_admin_password(&mut self, _new: &str) -> Result<(), String> {
            self.check("rotate_admin_password")
        }
        fn disable_setup_listener(&mut self) -> Result<(), String> {
            self.check("disable_setup_listener")
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

    const BOOTSTRAP_JOURNAL: &str = "\
🔑 Stalwart bootstrap mode - temporary administrator account
   username: admin   password: one-time-pw-42
";

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
            journal: BOOTSTRAP_JOURNAL.to_string(),
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
                "chown stalwart /var/lib/stalwart".to_string(),
                "chown stalwart /var/log/stalwart".to_string(),
                format!("write /etc/stalwart/config.json ({} bytes, mode 644)", initial_config_json().len()),
                format!("write /etc/systemd/system/stalwart.service ({} bytes, mode 644)", systemd_unit().len()),
                "mkdir /etc/systemd/system/stalwart.service.d".to_string(),
                format!(
                    "write /etc/systemd/system/stalwart.service.d/k2-hardening.conf ({} bytes, mode 644)",
                    hardening_dropin().len()
                ),
                "systemctl daemon-reload".to_string(),
                "systemctl enable --now stalwart".to_string(),
                "journalctl stalwart -n 200".to_string(),
                "systemctl restart stalwart".to_string(),
            ]
        );
        // The management-API sequence, in order.
        assert_eq!(
            api.calls,
            vec![
                "authenticate http://127.0.0.1:8080 admin",
                "set_hostname mail.acme.dev",
                "configure_listeners tls-alpn",
                "enable_spam_filter",
                "create_service_account",
                "mint_api_key principal-k2",
                "rotate_admin_password",
                "disable_setup_listener",
            ]
        );
        // Secrets: api key + rotated admin, both vaulted.
        {
            let stored = secrets.stored.lock().unwrap();
            assert_eq!(stored.len(), 2);
            assert_eq!(stored[0].0, "api-key");
            assert_eq!(stored[0].1, "minted-api-key-secret");
            assert_eq!(stored[1].0, "admin");
            assert_eq!(stored[1].1.len(), 64, "rotated password is a generated secret");
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

    #[test]
    fn enable_resumes_after_a_mid_flow_failure_without_redoing_work() {
        let _g = db_guard();
        clean_row();
        let art = fake_artifact();

        // First run: the service-account call fails mid-bootstrap.
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            journal: BOOTSTRAP_JOURNAL.to_string(),
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
        // must NOT re-download/extract/start, must re-authenticate,
        // and must finish.
        let ops2 = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            journal: BOOTSTRAP_JOURNAL.to_string(),
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
        // journalctl re-read is fine (auth re-runs every entry); the
        // API sequence resumes AT the failed step.
        assert_eq!(
            api2.calls,
            vec![
                "authenticate http://127.0.0.1:8080 admin",
                "create_service_account",
                "mint_api_key principal-k2",
                "rotate_admin_password",
                "disable_setup_listener",
            ]
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
        assert!(!lines.iter().any(|l| l.contains("/etc/stalwart")), "{lines:?}");
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
}
