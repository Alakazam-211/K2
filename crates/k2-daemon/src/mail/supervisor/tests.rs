//! The supervisor's tests (moved out of supervisor.rs so it stays under its
//! ratchet ceiling).

use super::*;
use crate::mail::helper;
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

/// 0.45.4: pin 0.16.25 (shas = upstream tarballs == upstream sigstore
/// digest, 2026-10-10; the fork release mirrors them); 0.16.20 and
/// 0.16.10 are the upgrade sources and rollback tables.
#[test]
fn pinned_artifacts_have_real_checksums_and_urls() {
    assert_eq!(STALWART_PINNED_VERSION, "0.16.25");
    assert_eq!(STALWART_PREVIOUS_VERSION, "0.16.20");
    assert_eq!(STALWART_OLDER_VERSION, "0.16.10");
    assert_eq!(upgrade_from_versions(), vec!["0.16.20", "0.16.10"]);
    for art in STALWART_SHA256
        .iter()
        .chain(STALWART_PREVIOUS_SHA256)
        .chain(STALWART_OLDER_SHA256)
    {
        assert!(
            !checksum_is_placeholder(art),
            "{}: pinned checksum must be real 64-hex",
            art.arch
        );
    }
    let art = artifact_for_arch("x86_64").expect("x86_64 supported");
    assert_eq!(
        art.sha256,
        "8ba9cc0ea2795121df5c1264af41e82353c5a3d6860161c4ab70817906d912d3"
    );
    assert_eq!(
        artifact_for_arch("aarch64").expect("aarch64").sha256,
        "7fc867e8bac70e00c372e8c8e8bc0e73e5f367f797daa5446f8c79225bdfb7e1"
    );
    assert_eq!(
        previous_artifact_for_arch("x86_64").expect("prev").sha256,
        "55184f166f89a0918c6523bb70be300370b63934357731b199dc26a12ca15abf"
    );
    assert_eq!(
        previous_artifact_for_arch("aarch64").expect("prev").sha256,
        "82183ce973665d2b99822b4a1f9911a8ba14788fd73ee93ce9be92352ccdea64"
    );
    assert_eq!(
        upgrade_from_artifact("0.16.20", "x86_64").expect("0.16.20").sha256,
        "55184f166f89a0918c6523bb70be300370b63934357731b199dc26a12ca15abf"
    );
    assert_eq!(
        upgrade_from_artifact("0.16.10", "x86_64").expect("0.16.10").sha256,
        "3ec4ab7eff49f61280f2fe2e4f9645ce5308d1840286ef0c0437524f92ac6a33"
    );
    assert_eq!(
        upgrade_from_artifact("0.16.10", "aarch64").expect("0.16.10").sha256,
        "0c9a80174a8a187477ac0ae4fcca8b7e0f17c84f6cae7b9304d2539459ba44ac"
    );
    // The pin itself is never an upgrade source; unknown versions refuse.
    for v in [STALWART_PINNED_VERSION, "0.16.18", "0.16.25-k2.1", ""] {
        let err = upgrade_from_artifact(v, "x86_64").expect_err(v);
        assert!(err.contains("0.16.20, 0.16.10"), "{v}: {err}");
    }
    assert_eq!(
        tarball_url(art.triple),
        "https://github.com/Alakazam-211/stalwart/releases/download/v0.16.25/stalwart-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(
        tarball_url_for(STALWART_PREVIOUS_VERSION, art.triple),
        "https://github.com/Alakazam-211/stalwart/releases/download/v0.16.20/stalwart-x86_64-unknown-linux-gnu.tar.gz"
    );
    let err = artifact_for_arch("riscv64").expect_err("unsupported arch");
    assert!(err.contains("riscv64"), "{err}");
    assert!(previous_artifact_for_arch("riscv64").is_err());
    assert!(upgrade_from_artifact("0.16.10", "riscv64").is_err());
}

/// CAL15: a box left on the previous pin (disabled / stopped / error)
/// gets a refusal from enable that names the one verb that upgrades.
#[test]
fn enable_refuses_the_previous_pin_and_names_hostmail_upgrade() {
    let _g = db_guard();
    clean_row();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, installed_version, updated_at) \
             VALUES (1, 'disabled', ?1, ?1, 100)",
            rusqlite::params![STALWART_PREVIOUS_VERSION],
        )
        .expect("seed row");
    }
    let ops = FakeSystemOps::default();
    let mut api = FakeApi::default();
    let secrets = FakeSecrets::default();
    let err = run_enable(&ops, &mut api, &secrets, &fake_artifact(), "mail.acme.dev", "tls-alpn")
        .expect_err("must refuse");
    assert!(err.contains(STALWART_PREVIOUS_VERSION), "{err}");
    assert!(err.contains(STALWART_PINNED_VERSION), "{err}");
    assert!(err.contains("k2 hostmail upgrade"), "{err}");
    assert!(err.contains("--dry-run"), "{err}");
    assert!(ops.recorded().is_empty(), "no effect: {:?}", ops.recorded());
    assert_eq!(current_status().as_deref(), Some("disabled"), "row untouched");
    clean_row();
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
        // What Stalwart writes is its own: files 0640, dirs 0750.
        "UMask=0027",
        "NoNewPrivileges=yes",
        "PrivateTmp=yes",
        "CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
        "AmbientCapabilities=CAP_NET_BIND_SERVICE",
        "Restart=on-failure",
    ] {
        assert!(d.contains(directive), "missing {directive}");
    }
    let unit = systemd_unit(Some("recovery-pw"));
    assert!(
        unit.contains("ExecStart=/usr/local/bin/stalwart --config /etc/stalwart/config.json")
    );
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
    /// Usernames that answer HTTP 401 (the field report's provisioned-admin reject).
    refuse_users: Vec<&'static str>,
    /// Usernames Stalwart does not have (field report: no admin@domain).
    missing_accounts: Vec<&'static str>,
    /// First rotate returns 401 until authenticate_bearer succeeds.
    fail_rotate_until_bearer: bool,
    bearer_ok: bool,
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
            self.calls
                .push(format!("authenticate-refused {base} {user}"));
            return Err(format!("connection refused: {base}"));
        }
        if self.refuse_users.contains(&user) {
            self.calls
                .push(format!("authenticate-rejected {base} {user}"));
            return Err("GET /jmap/session: HTTP 401 Unauthorized".to_string());
        }
        self.check(&format!("authenticate {base} {user}"))
    }
    fn authenticate_bearer(&mut self, base: &str, _api_key: &str) -> Result<(), String> {
        if self.refuse_urls.contains(&base) {
            self.calls
                .push(format!("authenticate-bearer-refused {base}"));
            return Err(format!("connection refused: {base}"));
        }
        self.bearer_ok = true;
        self.check(&format!("authenticate-bearer {base}"))
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
    fn lock_acme_cert_names(&mut self, hostname: &str) -> Result<(), String> {
        self.check(&format!("lock_acme_cert_names {hostname}"))
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
        if self.missing_accounts.contains(&username) {
            self.calls.push(format!("rotate-missing {username}"));
            return Err(format!("admin account '{username}' not found"));
        }
        if self.fail_rotate_until_bearer && !self.bearer_ok {
            self.calls.push(format!("rotate-401 {username}"));
            return Err(
                "POST http://127.0.0.1:8080/jmap/ HTTP 401 Unauthorized".to_string(),
            );
        }
        self.check(&format!(
            "rotate_admin_secret {username} {}",
            new_secret.len()
        ))
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

fn db_guard() -> crate::mail::MailServerTestGuard {
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
            // Stalwart's alone (protocol-17 helper): 0750 dirs, no
            // group write or other bits on anything inside.
            "tighten /etc/stalwart".to_string(),
            "tighten /var/lib/stalwart".to_string(),
            "tighten /var/log/stalwart".to_string(),
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
            // Bootstrap rewrites the unit with the vaulted recovery
            // env and restarts before restart-normal / server-config.
            format!(
                "write /etc/systemd/system/stalwart.service ({} bytes, mode 600)",
                systemd_unit(Some(&secrets_stored_secret(&secrets, "recovery-admin"))).len()
            ),
            "systemctl daemon-reload".to_string(),
            "systemctl restart stalwart".to_string(),
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
            "lock_acme_cert_names mail.acme.dev",
            "bind_setup_http_loopback",
            "authenticate http://127.0.0.1:8080 admin@acme.dev",
            "configure_listeners tls-alpn",
            "create_service_account acme.dev",
            "mint_api_key acct-k2",
            "authenticate http://127.0.0.1:8080 admin@acme.dev",
            "rotate_admin_secret admin 64",
            "rotate_admin_secret admin@acme.dev 64",
        ]
    );
    // Secrets: recovery admin + provisioned admin + api key +
    // the new principal password (saved before the provisioned
    // rotate) + rotated provisioned admin. Recovery ref deleted
    // only after that provisioned rotate succeeds.
    {
        let stored = secrets.stored.lock().unwrap();
        assert_eq!(stored.len(), 5, "{stored:?}");
        assert_eq!(stored[0].0, "recovery-admin");
        assert_eq!(stored[0].1.len(), 64, "generated recovery password");
        assert_eq!(stored[1].0, "admin");
        assert_eq!(stored[1].1, "provisioned-admin-secret");
        assert_eq!(stored[2].0, "api-key");
        assert_eq!(stored[2].1, "API_minted-key-secret");
        assert_eq!(stored[3].0, "recovery-admin");
        assert_eq!(
            stored[3].1.len(),
            64,
            "rotated principal password saved before the next step"
        );
        assert_eq!(stored[4].0, "admin");
        assert_eq!(stored[4].1.len(), 64, "rotated admin password");
        assert_ne!(
            stored[4].1, stored[1].1,
            "C20 recovery-off rotates the admin secret"
        );
        assert_eq!(
            *secrets.deleted.lock().unwrap(),
            vec!["mailsec_recovery-admin_test".to_string()],
            "recovery-off deletes the recovery secret"
        );
    }
    // Row landed running with refs + urls + version.
    assert_eq!(current_status().as_deref(), Some("running"));
    assert_eq!(row_field("api_url").as_deref(), Some(STALWART_MGMT_URL));
    assert_eq!(
        row_field("api_key_ref").as_deref(),
        Some("mailsec_api-key_test")
    );
    assert_eq!(
        row_field("admin_secret_ref").as_deref(),
        Some("mailsec_admin_test")
    );
    assert_eq!(
        row_field("installed_version").as_deref(),
        Some(STALWART_PINNED_VERSION)
    );
    assert_eq!(row_field("last_error"), None);
    // Every step marked done.
    for step in ENABLE_STEPS {
        assert!(step_is_done(step), "step {step} not marked done");
    }
    // A new install on a protocol-17 helper is already Stalwart's
    // alone: the health pass has nothing left to do.
    assert!(step_is_done(STORE_MODES_STEP));
    let before = ops.recorded().len();
    assert_eq!(ensure_store_modes_with(&ops), Ok(false));
    assert_eq!(ops.recorded().len(), before, "no second tighten");
    clean_row();
}

/// New installs: the helper makes Stalwart's config, data and log dirs
/// 0750 (the drop-in dir stays 0755), and the canned drop-in carries
/// UMask=0027, so what Stalwart writes is 0640 / 0750.
#[test]
fn new_install_modes_are_0750_dirs_and_umask_0027() {
    use k2_root_proto::verbs::MkdirTarget;
    assert_eq!(k2_root_proto::paths::STALWART_TREE_MASK, 0o750);
    for t in [MkdirTarget::Config, MkdirTarget::Data, MkdirTarget::Log] {
        assert_eq!(t.mode(), 0o750, "{t:?}");
    }
    assert_eq!(MkdirTarget::DropinDir.mode(), 0o755);
    assert_eq!(STALWART_TREE_DIRS, [STALWART_CONFIG_DIR, STALWART_DATA_DIR, STALWART_LOG_DIR]);
    assert!(hardening_dropin().lines().any(|l| l == "UMask=0027"));
    // The unit itself sets no umask (one place for it: the drop-in).
    assert!(!systemd_unit(None).contains("UMask"));
}

/// A helper older than protocol 17 cannot tighten: enable leaves the
/// tree as that helper made it and the step unmarked, so the health
/// pass does it once the helper updates.
#[test]
fn enable_on_an_older_helper_leaves_tightening_to_the_health_pass() {
    let _g = db_guard();
    clean_row();
    let ops = FakeSystemOps {
        download_body: FAKE_BINARY.to_vec(),
        store_modes: false,
        ..FakeSystemOps::default()
    };
    let mut api = FakeApi::default();
    let secrets = FakeSecrets::default();
    let art = fake_artifact();
    run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
        .expect("full enable succeeds");
    assert!(
        !ops.recorded().iter().any(|l| l.starts_with("tighten ")),
        "{:?}",
        ops.recorded()
    );
    assert!(!step_is_done(STORE_MODES_STEP));
    clean_row();
}

/// Existing installs: the health pass tightens Stalwart's config, data
/// and log dirs, rewrites the drop-in (UMask=0027) and reloads systemd
/// once. It never restarts or stops Stalwart, and a second pass records
/// nothing.
#[test]
fn existing_install_is_tightened_once_without_a_restart() {
    let _g = db_guard();
    clean_row();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, updated_at) \
             VALUES (1, 'running', ?1, 1)",
            rusqlite::params![STALWART_PINNED_VERSION],
        )
        .expect("seed row");
    }
    set_test_store_ready(Some(true));
    for s in ENABLE_STEPS {
        mark_step(s);
    }

    // An older helper: nothing yet, nothing marked.
    let old = FakeSystemOps {
        store_modes: false,
        ..FakeSystemOps::default()
    };
    assert_eq!(ensure_store_modes_with(&old), Ok(false));
    assert!(old.recorded().is_empty(), "{:?}", old.recorded());
    assert!(!step_is_done(STORE_MODES_STEP));

    let ops = FakeSystemOps::default();
    assert_eq!(ensure_store_modes_with(&ops), Ok(true));
    assert_eq!(
        ops.recorded(),
        vec![
            "tighten /etc/stalwart".to_string(),
            "tighten /var/lib/stalwart".to_string(),
            "tighten /var/log/stalwart".to_string(),
            format!(
                "write /etc/systemd/system/stalwart.service.d/k2-hardening.conf ({} bytes, mode 644)",
                hardening_dropin().len()
            ),
            "systemctl daemon-reload".to_string(),
        ]
    );
    assert!(
        !ops.recorded().iter().any(|l| l.contains("restart") || l.contains("stop")),
        "never a Stalwart restart: {:?}",
        ops.recorded()
    );
    let written = ops.written.lock().unwrap().get(STALWART_DROPIN_PATH).cloned();
    assert_eq!(written.as_deref(), Some(hardening_dropin().as_bytes()));
    for line in ops.recorded() {
        helper::recorded_line_allowlisted(&line).unwrap_or_else(|e| panic!("{line}: {e}"));
    }
    assert!(step_is_done(STORE_MODES_STEP));

    // Idempotent: the second pass records nothing.
    assert_eq!(ensure_store_modes_with(&ops), Ok(false));
    assert_eq!(ops.recorded().len(), 5, "{:?}", ops.recorded());

    // A failed tighten leaves the step unmarked for the next pass.
    unmark_steps(&[STORE_MODES_STEP]);
    let failing = FakeSystemOps::default();
    failing.fail("tighten /var/lib/stalwart");
    assert!(ensure_store_modes_with(&failing).is_err());
    assert!(!step_is_done(STORE_MODES_STEP));
    assert_eq!(ensure_store_modes_with(&failing), Ok(true));
    assert!(step_is_done(STORE_MODES_STEP));

    set_test_store_ready(None);
    clean_row();
}

/// Not installed (or an enable that never finished): nothing to tighten.
#[test]
fn store_modes_wait_for_a_completed_install() {
    let _g = db_guard();
    clean_row();
    let ops = FakeSystemOps::default();
    set_test_store_ready(Some(false));
    assert_eq!(ensure_store_modes_with(&ops), Ok(false));
    set_test_store_ready(Some(true));
    assert_eq!(ensure_store_modes_with(&ops), Ok(false), "restart step not done");
    assert!(ops.recorded().is_empty(), "{:?}", ops.recorded());
    set_test_store_ready(None);
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
        fail_api
            .calls
            .iter()
            .any(|c| c.starts_with("rotate_admin_secret admin ")),
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
    let mut api = FakeApi {
        fail_on: Some("create_service_account"),
        ..FakeApi::default()
    };
    let secrets = FakeSecrets::default();
    let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "http-01")
        .expect_err("injected failure");
    assert!(err.starts_with("service-account:"), "{err}");
    // CAL43: http-01 = Caddy owns the cert, Stalwart ACME is off —
    // the cert-name lock is a tls-alpn-only call.
    assert!(
        !api.calls.iter().any(|c| c.starts_with("lock_acme_cert_names")),
        "{:?}",
        api.calls
    );
    assert!(api.calls.iter().any(|c| c == "configure_listeners http-01"), "{:?}", api.calls);
    assert_eq!(current_status().as_deref(), Some("error"));
    assert!(row_field("last_error")
        .expect("recorded")
        .contains("injected"));

    // Second run: binary already on disk + steps marked — resume
    // must NOT re-download/extract/start/re-bootstrap, must
    // re-authenticate as the PROVISIONED admin, and must finish.
    let ops2 = FakeSystemOps {
        download_body: FAKE_BINARY.to_vec(),
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
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
            "authenticate http://127.0.0.1:8080 admin@acme.dev",
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
    let mut api = FakeApi {
        fail_on: Some("mint_api_key"),
        ..FakeApi::default()
    };
    let secrets = FakeSecrets::default();
    let _ = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "http-01")
        .expect_err("injected failure");

    let ops2 = FakeSystemOps {
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api2 = FakeApi {
        refuse_urls: vec![STALWART_SETUP_URL],
        ..FakeApi::default()
    };
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

/// Field report: server-config already logged in as admin@domain, then the
/// same vaulted secret 401s. recoveryAdminRef is still present and
/// Stalwart did not rotate. Resume must log in as recovery `admin`
/// and must not hammer the 401.
#[test]
fn recovery_off_uses_recovery_admin_when_provisioned_admin_401s() {
    let _g = db_guard();
    clean_row();
    let secrets = FakeSecrets::default();
    secrets
        .store("recovery-admin", "recovery-pw")
        .expect("vault recovery");
    secrets.store("admin", "provisioned").expect("vault admin");
    let mut steps = serde_json::Map::new();
    for s in ENABLE_STEPS {
        if *s != "recovery-off" && *s != "restart" {
            steps.insert((*s).to_string(), serde_json::json!({ "at": 1 }));
        }
    }
    let progress = serde_json::json!({
        "steps": steps,
        "adminUsername": "admin@acme.dev",
        "recoveryAdminRef": "mailsec_recovery-admin_test",
    })
    .to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, \
             admin_secret_ref, enable_progress_json, port_plan, updated_at) \
             VALUES (1, 'error', ?1, 'mail.acme.dev', 'mailsec_admin_test', ?2, \
             'tls-alpn', 100)",
            rusqlite::params![STALWART_PINNED_VERSION, progress],
        )
        .expect("seed row");
    }
    let ops = FakeSystemOps {
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api = FakeApi {
        refuse_users: vec!["admin@acme.dev"],
        ..FakeApi::default()
    };
    let art = fake_artifact();
    run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
        .expect("recovery admin fallback finishes enable");
    let rejected = api
        .calls
        .iter()
        .filter(|c| c.contains("authenticate-rejected"))
        .count();
    assert!(
        rejected <= 4,
        "a 401 must not be retried like a down listener: {:?}",
        api.calls
    );
    assert!(
        api.calls
            .iter()
            .any(|c| c == "authenticate http://127.0.0.1:8080 admin"),
        "must log in as recovery admin: {:?}",
        api.calls
    );
    assert!(
        !api.calls
            .iter()
            .any(|c| c.starts_with("complete_bootstrap")),
        "must not re-bootstrap: {:?}",
        api.calls
    );
    assert_eq!(
        ops.recorded()
            .iter()
            .filter(|l| *l == "systemctl restart stalwart")
            .count(),
        1,
        "only the final restart: {:?}",
        ops.recorded()
    );
    assert!(step_is_done("recovery-off"));
    assert!(step_is_done("restart"));
    assert_eq!(current_status().as_deref(), Some("running"));
    clean_row();
}

/// Field report, 19:06: principal `admin` was rotated and the vault key
/// deleted; `admin@domain` is not an account; the minted api-key
/// still opens :8080. Finish recovery-off on that key, keep the
/// new principal password, and do not fail the step.
#[test]
fn recovery_off_uses_api_key_when_provisioned_admin_is_missing() {
    let _g = db_guard();
    clean_row();
    let secrets = FakeSecrets::default();
    secrets.store("admin", "short-admin").expect("vault admin");
    secrets
        .store("api-key", "minted-key")
        .expect("vault api key");
    let mut steps = serde_json::Map::new();
    for s in ENABLE_STEPS {
        if *s != "recovery-off" && *s != "restart" {
            steps.insert((*s).to_string(), serde_json::json!({ "at": 1 }));
        }
    }
    let progress = serde_json::json!({
        "steps": steps,
        "adminUsername": "admin@acme.dev",
        "recoveryAdminRef": "mailsec_gone_b58a6a09a542",
    })
    .to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, \
             admin_secret_ref, api_key_ref, enable_progress_json, port_plan, updated_at) \
             VALUES (1, 'error', ?1, 'mail.acme.dev', 'mailsec_admin_test', \
             'mailsec_api-key_test', ?2, 'tls-alpn', 100)",
            rusqlite::params![STALWART_PINNED_VERSION, progress],
        )
        .expect("seed row");
    }
    let ops = FakeSystemOps {
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api = FakeApi {
        refuse_users: vec!["admin@acme.dev"],
        missing_accounts: vec!["admin@acme.dev"],
        ..FakeApi::default()
    };
    let art = fake_artifact();
    run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
        .expect("api-key fallback finishes enable");
    assert!(
        api.calls
            .iter()
            .any(|c| c == "authenticate-bearer http://127.0.0.1:8080"),
        "must log in with the api key: {:?}",
        api.calls
    );
    assert!(
        api.calls
            .iter()
            .any(|c| c == "rotate_admin_secret admin 64"),
        "must rotate principal admin and keep that password: {:?}",
        api.calls
    );
    assert!(
        api.calls
            .iter()
            .any(|c| c == "rotate-missing admin@acme.dev"),
        "missing admin@domain is a skip: {:?}",
        api.calls
    );
    assert!(
        !api.calls
            .iter()
            .any(|c| c.starts_with("complete_bootstrap")),
        "must not re-bootstrap: {:?}",
        api.calls
    );
    assert_eq!(
        row_field("admin_secret_ref").as_deref(),
        Some("mailsec_recovery-admin_test"),
        "kept principal password becomes the admin secret"
    );
    assert!(step_is_done("recovery-off"));
    assert!(step_is_done("restart"));
    assert_eq!(current_status().as_deref(), Some("running"));
    clean_row();
}

/// Field report, from zero, 20:46Z: api-key was minted, then recovery-off's
/// first management POST 401'd. Retry that rotate with the api-key.
#[test]
fn recovery_off_retries_rotate_with_api_key_after_post_401() {
    let _g = db_guard();
    clean_row();
    let secrets = FakeSecrets::default();
    secrets
        .store("recovery-admin", "recovery-pw")
        .expect("vault recovery");
    secrets.store("admin", "provisioned").expect("vault admin");
    secrets.store("api-key", "minted-key").expect("vault api key");
    let mut steps = serde_json::Map::new();
    for s in ENABLE_STEPS {
        if *s != "recovery-off" && *s != "restart" {
            steps.insert((*s).to_string(), serde_json::json!({ "at": 1 }));
        }
    }
    let progress = serde_json::json!({
        "steps": steps,
        "adminUsername": "admin@acme.dev",
        "recoveryAdminRef": "mailsec_recovery-admin_test",
    })
    .to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, \
             admin_secret_ref, api_key_ref, enable_progress_json, port_plan, updated_at) \
             VALUES (1, 'error', ?1, 'mail.acme.dev', 'mailsec_admin_test', \
             'mailsec_api-key_test', ?2, 'tls-alpn', 100)",
            rusqlite::params![STALWART_PINNED_VERSION, progress],
        )
        .expect("seed row");
    }
    let ops = FakeSystemOps {
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api = FakeApi {
        fail_rotate_until_bearer: true,
        ..FakeApi::default()
    };
    let art = fake_artifact();
    run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
        .expect("401 retry with api-key finishes enable");
    assert!(
        api.calls.iter().any(|c| c == "rotate-401 admin"),
        "first rotate must 401: {:?}",
        api.calls
    );
    assert!(
        api.calls
            .iter()
            .any(|c| c == "authenticate-bearer http://127.0.0.1:8080"),
        "must retry as the api-key: {:?}",
        api.calls
    );
    assert!(
        api.calls
            .iter()
            .any(|c| c == "rotate_admin_secret admin 64"),
        "retry must rotate: {:?}",
        api.calls
    );
    assert!(step_is_done("recovery-off"));
    assert!(step_is_done("restart"));
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
    assert_eq!(
        lines.len(),
        1,
        "download only — nothing extracted/installed: {lines:?}"
    );
    assert!(api.calls.is_empty());
    assert_eq!(current_status().as_deref(), Some("error"));
    assert!(row_field("last_error")
        .expect("recorded")
        .contains("sha256"));
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
    assert!(
        ops.recorded().is_empty(),
        "no writes when refusing (PRD §4)"
    );
    clean_row();
}

/// The akzm / customer-box failure: no helper on the box. Enable
/// stops before the row flips, before the download, before any
/// extract or write — with the install command, not EACCES.
#[test]
fn enable_stops_before_any_effect_when_the_helper_is_missing() {
    let _g = db_guard();
    for (state, prefix) in [
        (HelperState::Missing, "preflight: mail helper not installed"),
        (
            HelperState::NotAllowed,
            "preflight: mail helper not allowed by sudoers",
        ),
    ] {
        clean_row();
        let ops = FakeSystemOps {
            download_body: FAKE_BINARY.to_vec(),
            helper_state: state,
            ..FakeSystemOps::default()
        };
        let mut api = FakeApi::default();
        let secrets = FakeSecrets::default();
        let art = fake_artifact();
        let err = run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
            .expect_err("no helper must stop enable");
        assert!(err.starts_with(prefix), "{err}");
        assert!(
            err.ends_with(&format!("run as root: {}", helper::install_command())),
            "{err}"
        );
        assert!(!err.contains("Permission denied"), "{err}");
        assert!(
            ops.recorded().is_empty(),
            "no download / extract / write / systemctl: {:?}",
            ops.recorded()
        );
        assert!(api.calls.is_empty());
        assert_eq!(
            current_status(),
            None,
            "row must not flip to installing/error"
        );
        assert!(!step_is_done("download"));
    }
    clean_row();
}

/// Fake restart doors for the TLS-reload fallback.
struct TlsDoors {
    helper: HelperState,
    sudo_ok: bool,
    calls: Vec<String>,
}

impl crate::mail::imap_listeners::RestartDoors for TlsDoors {
    fn helper_state(&mut self) -> HelperState {
        self.calls.push("helper?".into());
        self.helper
    }
    fn plain_sudo_allowed(&mut self) -> bool {
        self.calls.push("sudo?".into());
        self.sudo_ok
    }
    fn restart(
        &mut self,
        path: crate::mail::imap_listeners::RestartPath,
    ) -> Result<(), String> {
        self.calls.push(format!("restart {path:?}"));
        Ok(())
    }
}

/// The TLS-reload restart fallback is the 0.44.2 chain: helper when
/// installed, else `sudo -n systemctl restart stalwart`; neither open
/// is a loud error (never a silent Ok).
#[test]
fn tls_reload_restart_uses_helper_then_plain_sudo_then_fails_loud() {
    use crate::mail::imap_listeners::RestartPath;
    let mut helper = TlsDoors { helper: HelperState::Installed, sudo_ok: false, calls: vec![] };
    assert_eq!(restart_to_reload_tls_with(&mut helper).expect("helper"), RestartPath::MailHelper);
    assert_eq!(helper.calls, vec!["helper?", "restart MailHelper"]);

    // akzm: no helper, sudoers allows systemctl.
    let mut sudo = TlsDoors { helper: HelperState::Missing, sudo_ok: true, calls: vec![] };
    assert_eq!(restart_to_reload_tls_with(&mut sudo).expect("sudo"), RestartPath::PlainSudo);
    assert_eq!(sudo.calls, vec!["helper?", "sudo?", "restart PlainSudo"]);

    let mut none = TlsDoors { helper: HelperState::Missing, sudo_ok: false, calls: vec![] };
    let err = restart_to_reload_tls_with(&mut none).expect_err("no door = loud error");
    assert!(err.contains("no door can restart Stalwart"), "{err}");
    assert!(err.contains("mail helper: missing"), "{err}");
    assert!(err.contains("sudo -n /usr/bin/systemctl restart stalwart"), "{err}");
    assert!(err.contains(&helper::install_command()), "{err}");
    assert_eq!(none.calls, vec!["helper?", "sudo?"], "never a restart attempt");
}

#[test]
fn restart_rides_the_helper_and_stops_with_the_fix_when_missing() {
    let missing = FakeSystemOps {
        helper_state: HelperState::Missing,
        ..FakeSystemOps::default()
    };
    let err = restart_stalwart_and_wait_with(&missing, "TLS reload")
        .expect_err("no helper = no restart attempt");
    assert!(err.starts_with("mail helper not installed"), "{err}");
    assert!(err.contains(&helper::install_command()), "{err}");
    assert!(missing.recorded().is_empty(), "{:?}", missing.recorded());

    let mut answers = std::collections::HashMap::new();
    answers.insert("is-active stalwart".to_string(), "active".to_string());
    let ok = FakeSystemOps {
        query_answers: answers,
        ..FakeSystemOps::default()
    };
    restart_stalwart_and_wait_with(&ok, "TLS reload").expect("restart succeeds");
    assert_eq!(
        ok.recorded(),
        vec![
            "systemctl restart stalwart".to_string(),
            "systemctl? is-active stalwart".to_string(),
        ]
    );
    // The restart line is a helper verb (systemctl via sudo -n helper).
    helper::recorded_line_allowlisted("systemctl restart stalwart")
        .expect("restart is a helper systemctl vector");

    let never = FakeSystemOps::default();
    let err = restart_stalwart_and_wait_with(&never, "IMAP listener")
        .expect_err("inactive unit must time out");
    assert_eq!(
        err,
        "stalwart unit did not become active after IMAP listener restart"
    );
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
        !ops.recorded()
            .iter()
            .any(|l| l.contains(&format!("rm {STALWART_CONFIG}"))),
        "C19: never wipe an existing config.json: {:?}",
        ops.recorded()
    );
    assert!(
        !api.calls
            .iter()
            .any(|c| c.starts_with("complete_bootstrap")),
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
    assert!(
        !running_inactive.consistent,
        "H2: running row + inactive unit disagree"
    );
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
        ReconciledStatus {
            state: "running".into(),
            consistent: true,
            last_error: None
        }
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
        ReconciledStatus {
            state: "disabled".into(),
            consistent: true,
            last_error: None
        },
        "disabled + inactive is agreement, not an error"
    );

    // A recorded (non-systemd) cause is passed through untouched.
    let disabled_kept = reconcile_reported_status("disabled", Some("owner note"), "failed");
    assert_eq!(disabled_kept.state, "disabled");
    assert!(disabled_kept.consistent);
    assert_eq!(disabled_kept.last_error.as_deref(), Some("owner note"));

    let disabled_active = reconcile_reported_status("disabled", None, "active");
    assert_eq!(disabled_active.state, "disabled");
    assert!(
        !disabled_active.consistent,
        "unit running while disabled disagrees"
    );
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
        Some("bootstrap failed"),
        "field report: keep the bootstrap: 401, do not overlay systemd mismatch"
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
        Some(
            "systemd reports the stalwart unit is 'active' while hostmail status is 'stopped'"
        )
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
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, updated_at) \
             VALUES (1, 'running', ?1, 1)",
            rusqlite::params![STALWART_PINNED_VERSION],
        )
        .expect("seed row");
    }
    set_test_store_ready(Some(true));
    mark_step("restart");
    let _unit = with_test_unit_state("active");
    assert!(
        is_already_enabled(),
        "C19: unit active + config.json + restart step is alreadyEnabled"
    );
    set_test_store_ready(None);
    clean_row();
}

#[test]
fn sqlite_running_without_config_json_is_not_already_enabled() {
    let _g = db_guard();
    clean_row();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
             VALUES (1, 'running', 'test', 'mail.ironwood.com', 1)",
            [],
        )
        .expect("seed running row");
    }
    set_test_store_ready(Some(false));
    let _unit = with_test_unit_state("active");
    assert!(
        !is_already_enabled(),
        "field report: sqlite running + unit active + no config.json must resume enable"
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
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api = FakeApi::default();
    let art = fake_artifact();
    run_enable(&ops, &mut api, &secrets, &art, "mail.new.dev", "tls-alpn")
        .expect("retarget enable");
    assert!(
        !api.calls
            .iter()
            .any(|c| c.starts_with("complete_bootstrap")),
        "L2 must not re-bootstrap: {:?}",
        api.calls
    );
    assert!(
        api.calls
            .iter()
            .any(|c| c == "set_server_hostname mail.new.dev"),
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
        existing_paths: vec![STALWART_BIN.to_string(), STALWART_CONFIG.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api = FakeApi::default();
    let art = fake_artifact();
    run_enable(&ops, &mut api, &secrets, &art, "mail.acme.dev", "tls-alpn")
        .expect("re-enable same hostname");
    assert!(
        !api.calls
            .iter()
            .any(|c| c.starts_with("set_server_hostname")),
        "same hostname is not a retarget: {:?}",
        api.calls
    );
    assert!(
        !api.calls
            .iter()
            .any(|c| c.starts_with("complete_bootstrap")),
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
        text.contains("STALWART_RECOVERY_ADMIN"),
        "C20: bootstrap failed before config.json — keep the vaulted recovery env: {text}"
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
        vec![
            "mailsec_admin_x".to_string(),
            "mailsec_api-key_y".to_string()
        ]
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
    assert!(
        !lines.iter().any(|l| l.contains("/var/lib/stalwart")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("rm /etc/stalwart")),
        "{lines:?}"
    );
    clean_row();
}

/// Every mkdir/chown/write/rm/systemctl line the fake machine records
/// is one of the helper vectors. The expected sequences above stay
/// free of `sudo`. `systemctl? is-active` is a health query, not a verb.
#[test]
fn helper_allowlist_covers_fake_enable_and_purge() {
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
    let lines = ops.recorded();
    assert!(lines.iter().any(|l| l.starts_with("mkdir ")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("chown ")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("write ")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("systemctl ")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("rm ")),
        "enable must not remove: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("rm /etc/stalwart/config.json")),
        "C19: enable must not rm config.json: {lines:?}"
    );
    for line in &lines {
        if helper::is_privileged_recording(line) {
            helper::recorded_line_allowlisted(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        }
    }

    clean_row();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
             VALUES (1, 'running', ?1, 'mail.acme.dev', 100)",
            rusqlite::params![STALWART_PINNED_VERSION],
        )
        .expect("seed row");
    }
    let ops = FakeSystemOps::default();
    uninstall_with(&ops, &FakeSecrets::default(), true).expect("purge");
    let lines = ops.recorded();
    assert!(lines.iter().any(|l| l.starts_with("rm ")), "{lines:?}");
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("/etc/stalwart/config.json")),
        "{lines:?}"
    );
    for line in &lines {
        if helper::is_privileged_recording(line) {
            helper::recorded_line_allowlisted(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        }
    }

    let ops = FakeSystemOps {
        existing_paths: vec![STALWART_BIN.to_string()],
        query_answers: [("is-active stalwart".to_string(), "active".to_string())]
            .into_iter()
            .collect(),
        ..FakeSystemOps::default()
    };
    let _ = health_check_with(&ops, &|| Ok(()));
    let lines = ops.recorded();
    assert!(
        lines.iter().any(|l| l == "systemctl? is-active stalwart"),
        "{lines:?}"
    );
    assert!(helper::parse_argv(&["systemctl", "is-active", "stalwart"]).is_err());
    for line in &lines {
        if line.starts_with("systemctl?") {
            assert!(helper::recorded_line_allowlisted(line).is_err(), "{line}");
        }
    }
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
    run_enable(
        &ops,
        &mut api,
        &secrets,
        &art,
        "mail.lumon.k2.dev",
        "http-01",
    )
    .expect("first enable");
    assert!(step_is_done("start"), "start marked after first enable");

    let ops_dis = FakeSystemOps::default();
    disable_with(&ops_dis).expect("disable");
    assert_eq!(current_status().as_deref(), Some("disabled"));
    assert!(
        !step_is_done("start"),
        "disable must clear the start resume mark (H4)"
    );
    assert_eq!(row_field("hostname").as_deref(), Some("mail.lumon.k2.dev"));

    let ops2 = FakeSystemOps {
        download_body: FAKE_BINARY.to_vec(),
        existing_paths: vec![STALWART_BIN.to_string()],
        ..FakeSystemOps::default()
    };
    let mut api2 = FakeApi::default();
    run_enable(&ops2, &mut api2, &secrets, &art, "mail.lumon.io", "http-01")
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
        Some("mail.lumon.io"),
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

fn mint_leaf_der_with_sans(cn: &str, org: Option<&str>, sans: &[&str]) -> Vec<u8> {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
    let mut params = CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .expect("SAN params");
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    if let Some(org) = org {
        dn.push(DnType::OrganizationName, org);
    }
    params.distinguished_name = dn;
    let cert = params.self_signed(&key).expect("self-sign");
    cert.der().to_vec()
}

/// A leaf for `sans` signed by a Let's Encrypt-looking CA (issuer !=
/// subject). `expired` backdates it. Returns (leaf DER, leaf key DER).
fn mint_ca_leaf(sans: &[&str], expired: bool) -> (Vec<u8>, Vec<u8>) {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("ca key");
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "R11");
    dn.push(DnType::OrganizationName, "Let's Encrypt");
    ca.distinguished_name = dn;
    let ca_cert = ca.self_signed(&ca_key).expect("ca self-sign");
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("leaf key");
    let mut leaf = CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .expect("leaf params");
    let mut ldn = DistinguishedName::new();
    ldn.push(DnType::CommonName, sans.first().copied().unwrap_or("leaf"));
    leaf.distinguished_name = ldn;
    if expired {
        leaf.not_before = rcgen::date_time_ymd(2020, 1, 1);
        leaf.not_after = rcgen::date_time_ymd(2020, 4, 1);
    }
    let cert = leaf.signed_by(&key, &ca_cert, &ca_key).expect("ca-sign leaf");
    (cert.der().to_vec(), key.serialize_der())
}

fn handshake(der: Vec<u8>) -> TlsProbe {
    TlsProbe::Handshake { leaf_der: der }
}

/// L1: a captured rcgen leaf is self-signed, never missing+false.
/// 0.45.1 A2: `names` are the served leaf's real SANs (both of them),
/// not an assumed `[host]`.
#[test]
fn tls_cert_status_rcgen_leaf_is_self_signed() {
    let der = mint_leaf_der_with_sans(
        "rcgen self signed cert",
        None,
        &["mail.acme.dev", "Autoconfig.Acme.dev"],
    );
    let _g = with_test_tls_cert(handshake(der));
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"].as_str().expect("state"), "self-signed", "{v}");
    assert_eq!(v["selfSigned"].as_bool().expect("selfSigned"), true, "{v}");
    assert!(
        v["expiresAt"].as_i64().expect("expiresAt") > 0,
        "expiresAt parsed: {v}"
    );
    assert_eq!(v["host"].as_str().expect("host"), "mail.acme.dev", "{v}");
    assert_eq!(
        v["names"],
        serde_json::json!(["mail.acme.dev", "autoconfig.acme.dev"]),
        "{v}"
    );
}

/// 0.45.1 A2 / HF10: no certificate served → `names` is empty and
/// `host` stays set. 0.46.1: a CA leaf with no SAN for the host is a
/// `mismatch`, never `issued`.
#[test]
fn tls_cert_status_names_are_empty_without_a_served_san() {
    let _g = with_test_tls_cert(TlsProbe::Missing);
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["names"], serde_json::json!([]), "{v}");
    assert_eq!(v["host"], "mail.acme.dev", "{v}");
    drop(_g);
    let (der, _) = mint_ca_leaf(&[], false);
    let _g = with_test_tls_cert(handshake(der));
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"], "mismatch", "{v}");
    assert_eq!(v["names"], serde_json::json!([]), "{v}");
}

/// L1: a CA-issued leaf covering the host is issued, not self-signed.
#[test]
fn tls_cert_status_acme_leaf_is_issued() {
    let (der, _) = mint_ca_leaf(&["mail.acme.dev"], false);
    let _g = with_test_tls_cert(handshake(der));
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"].as_str().expect("state"), "issued", "{v}");
    assert_eq!(v["selfSigned"].as_bool().expect("selfSigned"), false, "{v}");
    assert!(
        v["expiresAt"].as_i64().expect("expiresAt") > 0,
        "expiresAt parsed: {v}"
    );
    assert!(v["notBefore"].as_i64().expect("notBefore") > 0, "{v}");
    assert_eq!(v["ports"].as_array().expect("ports").len(), 3, "{v}");
}

/// L1: no TCP/TLS at all is missing + selfSigned false.
#[test]
fn tls_cert_status_no_tls_is_missing() {
    let _g = with_test_tls_cert(TlsProbe::Missing);
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"].as_str().expect("state"), "missing", "{v}");
    assert_eq!(v["selfSigned"].as_bool().expect("selfSigned"), false, "{v}");
    assert!(v["expiresAt"].is_null(), "{v}");
}

/// 0.46.1: a certificate that signed itself (issuer == subject) is
/// self-signed even without rcgen's name, and certificate files on
/// disk no longer turn a served leaf into `issued`.
#[test]
fn tls_cert_status_issuer_equals_subject_is_self_signed() {
    let der = mint_leaf_der_with_sans("mail.acme.dev", Some("Example Org"), &["mail.acme.dev"]);
    let _g = with_test_tls_cert(handshake(der));
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"].as_str().expect("state"), "self-signed", "{v}");
    assert_eq!(v["selfSigned"].as_bool().expect("selfSigned"), true, "{v}");
}

/// 0.46.1: an expired CA leaf reads `expired`; a wildcard SAN covers
/// one label; an unparseable leaf is `unknown`, never `issued`.
#[test]
fn tls_cert_status_expired_wildcard_and_unparseable() {
    let (der, _) = mint_ca_leaf(&["mail.acme.dev"], true);
    let _g = with_test_tls_cert(handshake(der));
    assert_eq!(tls_cert_status(Some("mail.acme.dev"))["state"], "expired");
    drop(_g);
    let (der, _) = mint_ca_leaf(&["*.acme.dev"], false);
    let _g = with_test_tls_cert(handshake(der));
    assert_eq!(tls_cert_status(Some("mail.acme.dev"))["state"], "issued");
    assert_eq!(tls_cert_status(Some("a.mail.acme.dev"))["state"], "mismatch");
    drop(_g);
    let _g = with_test_tls_cert(handshake(vec![0x30, 0x03, 0x01, 0x02, 0x03]));
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"], "unknown", "{v}");
    assert_eq!(v["selfSigned"], false, "{v}");
}

/// 0.46.1 field report (partii on a customer box): 443 answered with a real
/// certificate while 465 and 993 — Stalwart's own listeners — served
/// the rcgen fallback. The worst port decides; each port is listed.
#[test]
fn tls_cert_status_worst_port_wins() {
    let (good, _) = mint_ca_leaf(&["mail.acme.dev"], false);
    let rcgen = mint_leaf_der_with_sans("rcgen self signed cert", None, &["localhost"]);
    let _g = with_test_tls_ports(vec![
        (443, handshake(good)),
        (465, handshake(rcgen.clone())),
        (993, handshake(rcgen)),
    ]);
    let v = tls_cert_status(Some("mail.acme.dev"));
    assert_eq!(v["state"], "self-signed", "{v}");
    assert_eq!(v["selfSigned"], true, "{v}");
    assert_eq!(v["names"], serde_json::json!(["localhost"]), "{v}");
    let ports = v["ports"].as_array().expect("ports");
    let by = |p: u64| ports.iter().find(|x| x["port"] == p).expect("port listed")["state"].clone();
    assert_eq!(by(443), "issued", "{v}");
    assert_eq!(by(465), "self-signed", "{v}");
    assert_eq!(by(993), "self-signed", "{v}");
    drop(_g);
    // A port that does not answer (993 not bound) never outranks a
    // real answer.
    let (good, _) = mint_ca_leaf(&["mail.acme.dev"], false);
    let _g = with_test_tls_ports(vec![
        (443, handshake(good.clone())),
        (465, handshake(good)),
        (993, TlsProbe::Missing),
    ]);
    assert_eq!(tls_cert_status(Some("mail.acme.dev"))["state"], "issued");
}

/// A real local TLS endpoint (rustls server on 127.0.0.1) serving one
/// leaf for one connection.
fn serve_tls_once(leaf_der: Vec<u8>, key_der: Vec<u8>) -> std::net::SocketAddr {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::sync::Arc;
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("server protocol versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(leaf_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
    .expect("server cert");
    let cfg = Arc::new(cfg);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut conn = rustls::ServerConnection::new(cfg).expect("server conn");
        let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        while conn.is_handshaking() {
            match conn.complete_io(&mut sock) {
                Ok((0, 0)) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    addr
}

/// 0.46.1: fake TLS endpoints, real SNI handshakes. A self-signed
/// rcgen leaf (SAN localhost) reads `self-signed`; a CA leaf matching
/// the SNI reads `issued`; a CA leaf for another name reads
/// `mismatch`; a closed port reads `missing`.
#[test]
fn tls_probe_against_fake_endpoints() {
    let host = "mail.acme.dev";
    // The rcgen fallback, the way Stalwart mints it (SAN localhost).
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).expect("params");
    let rc = params.self_signed(&key).expect("self-sign");
    let addr = serve_tls_once(rc.der().to_vec(), key.serialize_der());
    let probe = probe_tls_endpoint(host, &[addr], false);
    assert!(matches!(probe, TlsProbe::Handshake { .. }), "handshake captured: {probe:?}");
    let v = tls_cert_json(host, &[(465, probe)], now_secs());
    assert_eq!(v["state"], "self-signed", "{v}");
    assert_eq!(v["selfSigned"], true, "{v}");
    assert_eq!(v["names"], serde_json::json!(["localhost"]), "{v}");

    let (leaf, key) = mint_ca_leaf(&[host], false);
    let addr = serve_tls_once(leaf, key);
    let probe = probe_tls_endpoint(host, &[addr], true);
    let v = tls_cert_json(host, &[(443, probe)], now_secs());
    assert_eq!(v["state"], "issued", "{v}");
    assert_eq!(v["selfSigned"], false, "{v}");
    assert_eq!(v["names"], serde_json::json!([host]), "{v}");

    let (leaf, key) = mint_ca_leaf(&["other.acme.dev"], false);
    let addr = serve_tls_once(leaf, key);
    let probe = probe_tls_endpoint(host, &[addr], false);
    let v = tls_cert_json(host, &[(993, probe)], now_secs());
    assert_eq!(v["state"], "mismatch", "{v}");

    // Nothing listening: missing, not issued.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let dead = closed.local_addr().expect("addr");
    drop(closed);
    let v = tls_cert_json(host, &[(993, probe_tls_endpoint(host, &[dead], false))], now_secs());
    assert_eq!(v["state"], "missing", "{v}");
}
