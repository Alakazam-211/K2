//! On-box PEM store for custom-domain certs (`~/.k2/certs/<hostname>/`).
//! Private key is 0600; chain is 0644. Never silent-rcgen.

use std::fs;
use std::path::{Path, PathBuf};

pub fn certs_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("certs")
}

pub fn host_dir(hostname: &str) -> PathBuf {
    certs_root().join(hostname)
}

pub fn key_path(hostname: &str) -> PathBuf {
    host_dir(hostname).join("privkey.pem")
}

pub fn chain_path(hostname: &str) -> PathBuf {
    host_dir(hostname).join("fullchain.pem")
}

pub fn acme_account_path() -> PathBuf {
    certs_root().join("acme-account.json")
}

pub fn acme_config_path() -> PathBuf {
    certs_root().join("acme-config.json")
}

#[derive(Debug, Clone)]
pub struct InstalledPem {
    pub hostname: String,
    pub chain_pem: String,
    pub key_pem: String,
}

pub fn load(hostname: &str) -> Option<InstalledPem> {
    let chain = fs::read_to_string(chain_path(hostname)).ok()?;
    let key = fs::read_to_string(key_path(hostname)).ok()?;
    if chain.trim().is_empty() || key.trim().is_empty() {
        return None;
    }
    Some(InstalledPem {
        hostname: hostname.to_string(),
        chain_pem: chain,
        key_pem: key,
    })
}

pub fn install(hostname: &str, chain_pem: &str, key_pem: &str) -> Result<PathBuf, String> {
    if chain_pem.contains("BEGIN CERTIFICATE") && key_pem.contains("BEGIN") {
        // ok
    } else {
        return Err("upload requires PEM certificate and private key".into());
    }
    let dir = host_dir(hostname);
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    write_tmp_rename(&chain_path(hostname), chain_pem.as_bytes(), 0o644)?;
    write_tmp_rename(&key_path(hostname), key_pem.as_bytes(), 0o600)?;
    Ok(dir)
}

pub(crate) fn write_tmp_rename(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let tmp = path.with_extension(format!(
        "tmp.{}",
        std::process::id()
    ));
    fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {:o} {}: {e}", mode, tmp.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })?;
    Ok(())
}

#[cfg(test)]
pub fn key_mode_is_0600(hostname: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(key_path(hostname))
            .map(|m| m.permissions().mode() & 0o777 == 0o600)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = hostname;
        true
    }
}

/// Plant PEMs into Stalwart so 443/465 serve them (role=mail).
///
/// v0.16.10 keeps TLS material in RocksDB, not a certs directory, and
/// systemd `ProtectHome=yes` cannot read `~/.k2/certs`. The real plant
/// is JMAP `x:Certificate/set` + `defaultCertificateId`. Copying into
/// `/var/lib/stalwart/certs` (or `/etc/stalwart/certs`) is extra when
/// those dirs exist — missing dirs must not look like success.
pub fn plant_mail_pem(hostname: &str, chain_pem: &str, key_pem: &str) -> Result<(), String> {
    if chain_pem.trim().is_empty() || key_pem.trim().is_empty() {
        return Err("plant mail PEM: empty chain or private key".into());
    }
    for dir in ["/var/lib/stalwart/certs", "/etc/stalwart/certs"] {
        let p = Path::new(dir);
        if !p.is_dir() {
            continue;
        }
        let crt = p.join(format!("{hostname}.crt"));
        let key = p.join(format!("{hostname}.key"));
        let _ = write_tmp_rename(&crt, chain_pem.as_bytes(), 0o644)
            .and_then(|_| write_tmp_rename(&key, key_pem.as_bytes(), 0o600));
    }
    jmap_plant_certificate(hostname, chain_pem, key_pem)
}

fn jmap_plant_certificate(hostname: &str, chain_pem: &str, key_pem: &str) -> Result<(), String> {
    #[cfg(test)]
    {
        let _ = (hostname, chain_pem, key_pem);
        TEST_JMAP_PLANT_CALLS.with(|c| *c.borrow_mut() += 1);
        if let Some(r) = TEST_JMAP_PLANT.with(|c| c.borrow().clone()) {
            return r;
        }
        return Err(
            "plant mail PEM: Stalwart JMAP required (RocksDB; no certs dir) — \
inject TEST_JMAP_PLANT"
                .into(),
        );
    }
    #[cfg(not(test))]
    jmap_plant_via_engine(hostname, chain_pem, key_pem)
}

#[cfg_attr(test, allow(dead_code))]
fn jmap_plant_via_engine(hostname: &str, chain_pem: &str, key_pem: &str) -> Result<(), String> {
    let (client, _) = crate::mail::domains::engine_from_db().map_err(|e| {
        format!(
            "plant mail PEM into Stalwart via JMAP (certs live in RocksDB, not a files dir): {e}"
        )
    })?;
    let report = plant_default_and_load(hostname, &mut LiveMailTls { client }, chain_pem, key_pem)?;
    k2_core::log_debug!(
        "[domains/store] planted {hostname} as Stalwart certificate {} (loaded by {}{}){}",
        report.cert_id,
        report.loaded_by,
        report
            .reload_note
            .as_deref()
            .map(|n| format!("; {n}"))
            .unwrap_or_default(),
        if report.removed.is_empty() && report.cleanup_error.is_none() {
            String::new()
        } else {
            format!(
                "; removed earlier K2 certificates {:?}{}",
                report.removed,
                report
                    .cleanup_error
                    .as_deref()
                    .map(|e| format!(" (cleanup: {e})"))
                    .unwrap_or_default()
            )
        }
    );
    Ok(())
}

/// The Stalwart calls the mail-certificate plant makes. Production =
/// [`LiveMailTls`]; tests inject a recording fake.
pub trait MailTlsOps {
    /// `x:Certificate/set` create + `defaultCertificateId` → the new id.
    fn plant_default(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String>;
    /// `x:Action/set` `ReloadTlsCertificates` (no restart).
    fn reload_tls(&mut self) -> Result<(), String>;
    /// The existing `restart_stalwart_to_reload_tls` path (mail helper).
    fn restart_stalwart(&mut self) -> Result<(), String>;
    /// `x:Certificate/set` destroy (an id already gone is Ok).
    fn destroy_certificate(&mut self, id: &str) -> Result<(), String>;
}

pub struct LiveMailTls {
    pub client: crate::mail::jmap::StalwartClient,
}

impl MailTlsOps for LiveMailTls {
    fn plant_default(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String> {
        self.client.certificate_plant(chain_pem, key_pem)
    }
    fn reload_tls(&mut self) -> Result<(), String> {
        self.client.reload_tls_certificates()
    }
    fn restart_stalwart(&mut self) -> Result<(), String> {
        crate::mail::supervisor::restart_stalwart_to_reload_tls()
    }
    fn destroy_certificate(&mut self, id: &str) -> Result<(), String> {
        self.client.certificate_destroy(id)
    }
}

/// What [`plant_default_and_load`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlantReport {
    pub cert_id: String,
    /// `action` (ReloadTlsCertificates) | `restart` (fallback).
    pub loaded_by: &'static str,
    pub reload_note: Option<String>,
    /// Earlier certificates K2 planted for this name, now destroyed.
    pub removed: Vec<String>,
    pub cleanup_error: Option<String>,
}

/// The mail host's certificate IS Stalwart's default (it also answers
/// clients that send no SNI name): plant it as a new Certificate and
/// point `defaultCertificateId` at it, then load it with
/// `ReloadTlsCertificates` — a restart (mail helper) only if that action
/// fails; never hostmail disable/enable. Once loaded, the Certificates K2
/// planted for this name before are destroyed (Stalwart already prefers
/// the newest notAfter per name and deletes expired ones itself; this
/// keeps one object per name). Certificates K2 did not plant are never
/// touched. Fails loud when neither load path works.
pub fn plant_default_and_load(
    hostname: &str,
    ops: &mut dyn MailTlsOps,
    chain_pem: &str,
    key_pem: &str,
) -> Result<PlantReport, String> {
    let name = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    let not_after = super::status::pem_leaf_info(chain_pem).map(|l| l.not_after);
    let id = ops
        .plant_default(chain_pem, key_pem)
        .map_err(|e| format!("plant mail PEM via x:Certificate/set: {e}"))?;
    // Record the new id before loading: if the load fails, the next plant
    // still knows to remove this object.
    let stale = super::renew::update(|f| {
        let rec = f.names.entry(name.clone()).or_default();
        if let Some(prev) = rec.stalwart_cert_id.replace(id.clone()) {
            if prev != id && !rec.stale_cert_ids.contains(&prev) {
                rec.stale_cert_ids.push(prev);
            }
        }
        rec.planted_not_after = not_after;
        rec.stale_cert_ids.clone()
    })
    .unwrap_or_else(|e| {
        k2_core::log_debug!("[domains/store] renewal.json: {e}");
        Vec::new()
    });
    let (loaded_by, reload_note) = match ops.reload_tls() {
        Ok(()) => ("action", None),
        Err(action_err) => match ops.restart_stalwart() {
            Ok(()) => (
                "restart",
                Some(format!("ReloadTlsCertificates failed ({action_err}); restarted Stalwart instead")),
            ),
            Err(restart_err) => {
                return Err(format!(
                    "planted {name} as Stalwart certificate {id}, but ReloadTlsCertificates \
                     failed ({action_err}) and restart stalwart to load planted cert \
                     (not hostmail disable) failed ({restart_err}) — it loads on Stalwart's \
                     next restart"
                ))
            }
        },
    };
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for old in stale.iter().filter(|s| **s != id) {
        match ops.destroy_certificate(old) {
            Ok(()) => removed.push(old.clone()),
            Err(e) => errors.push(format!("{old}: {e}")),
        }
    }
    if !removed.is_empty() {
        let gone = removed.clone();
        let _ = super::renew::update(|f| {
            if let Some(rec) = f.names.get_mut(&name) {
                rec.stale_cert_ids.retain(|s| !gone.contains(s));
            }
        });
    }
    Ok(PlantReport {
        cert_id: id,
        loaded_by,
        reload_note,
        removed,
        cleanup_error: (!errors.is_empty()).then(|| errors.join("; ")),
    })
}

#[cfg(test)]
thread_local! {
    static TEST_JMAP_PLANT: std::cell::RefCell<Option<Result<(), String>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_JMAP_PLANT_CALLS: std::cell::RefCell<usize> =
        const { std::cell::RefCell::new(0) };
}

#[cfg(test)]
pub(crate) struct JmapPlantGuard;

#[cfg(test)]
impl Drop for JmapPlantGuard {
    fn drop(&mut self) {
        TEST_JMAP_PLANT.with(|c| *c.borrow_mut() = None);
        TEST_JMAP_PLANT_CALLS.with(|c| *c.borrow_mut() = 0);
    }
}

/// Test seam: skip the live Stalwart client. `None` fails loud (the
/// production "no certs dir" path must never look like Ok).
#[cfg(test)]
pub(crate) fn set_test_jmap_plant(result: Option<Result<(), String>>) -> JmapPlantGuard {
    TEST_JMAP_PLANT.with(|c| *c.borrow_mut() = result);
    TEST_JMAP_PLANT_CALLS.with(|c| *c.borrow_mut() = 0);
    JmapPlantGuard
}

#[cfg(test)]
pub(crate) fn test_jmap_plant_calls() -> usize {
    TEST_JMAP_PLANT_CALLS.with(|c| *c.borrow())
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcmeConfig {
    pub email: Option<String>,
    pub directory: Option<String>,
}

pub fn load_acme_config() -> AcmeConfig {
    fs::read_to_string(acme_config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_acme_config(cfg: &AcmeConfig) -> Result<(), String> {
    let dir = certs_root();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let body = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    write_tmp_rename(&acme_config_path(), body.as_bytes(), 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_key_is_0600() {
        let _home = crate::test_support::TempHome::new();
        let chain = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        let key = "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n";
        install("mail.example.com", chain, key).expect("install");
        assert!(key_mode_is_0600("mail.example.com"));
        assert!(load("mail.example.com").is_some());
    }

    #[test]
    fn plant_mail_pem_without_certs_dir_is_not_ok() {
        let _g = set_test_jmap_plant(None);
        let err = plant_mail_pem(
            "mail.example.com",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n",
        )
        .expect_err("missing certs dir must not look like success");
        assert!(
            err.contains("JMAP") || err.contains("certs dir") || err.contains("RocksDB"),
            "{err}"
        );
        assert!(test_jmap_plant_calls() >= 1, "JMAP plant must be attempted");
    }

    /// Recording fake for the Stalwart calls of the mail plant.
    struct FakeTls {
        calls: Vec<String>,
        reload: Result<(), String>,
        restart: Result<(), String>,
        destroy_fail: Vec<String>,
        next: usize,
    }

    impl FakeTls {
        fn new() -> Self {
            Self { calls: Vec::new(), reload: Ok(()), restart: Ok(()), destroy_fail: Vec::new(), next: 0 }
        }
    }

    impl MailTlsOps for FakeTls {
        fn plant_default(&mut self, _c: &str, _k: &str) -> Result<String, String> {
            self.next += 1;
            self.calls.push(format!("plant-default cert-{}", self.next));
            Ok(format!("cert-{}", self.next))
        }
        fn reload_tls(&mut self) -> Result<(), String> {
            self.calls.push("reload".into());
            self.reload.clone()
        }
        fn restart_stalwart(&mut self) -> Result<(), String> {
            self.calls.push("restart".into());
            self.restart.clone()
        }
        fn destroy_certificate(&mut self, id: &str) -> Result<(), String> {
            self.calls.push(format!("destroy {id}"));
            if self.destroy_fail.iter().any(|d| d == id) {
                return Err("forbidden".into());
            }
            Ok(())
        }
    }

    const CHAIN: &str = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
    const KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n";

    #[test]
    fn mail_plant_is_default_then_reload_action_never_restart() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::new();
        let r = plant_default_and_load("mail.example.com", &mut ops, CHAIN, KEY).expect("plant");
        assert_eq!(ops.calls, vec!["plant-default cert-1", "reload"], "no restart when the action works");
        assert_eq!(r.loaded_by, "action");
        assert!(r.removed.is_empty(), "first plant: nothing of K2's to remove");
        let rec = super::super::renew::load().names["mail.example.com"].clone();
        assert_eq!(rec.stalwart_cert_id.as_deref(), Some("cert-1"));
    }

    #[test]
    fn mail_plant_falls_back_to_restart_then_fails_loud() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::new();
        ops.reload = Err("validationFailed".into());
        let r = plant_default_and_load("mail.example.com", &mut ops, CHAIN, KEY).expect("restart path");
        assert_eq!(ops.calls, vec!["plant-default cert-1", "reload", "restart"]);
        assert_eq!(r.loaded_by, "restart");
        assert!(r.reload_note.unwrap().contains("validationFailed"));

        let mut ops = FakeTls::new();
        ops.reload = Err("validationFailed".into());
        ops.restart = Err("the mail helper is not installed".into());
        let err = plant_default_and_load("mail.example.com", &mut ops, CHAIN, KEY).expect_err("both fail");
        assert!(err.contains("validationFailed") && err.contains("mail helper"), "{err}");
        assert!(!ops.calls.iter().any(|c| c.starts_with("destroy")), "never remove the old cert while the new one is not loaded");
    }

    #[test]
    fn mail_plant_removes_only_the_certs_k2_planted_before_once_loaded() {
        let _home = crate::test_support::TempHome::new();
        // First plant whose load failed: cert-1 is in Stalwart, unloaded.
        let mut ops = FakeTls::new();
        ops.reload = Err("x".into());
        ops.restart = Err("y".into());
        plant_default_and_load("mail.example.com", &mut ops, CHAIN, KEY).expect_err("unloaded");
        // Next plant (renewal): cert-2 becomes default and loads; cert-1 goes.
        let mut ops2 = FakeTls { next: 1, ..FakeTls::new() };
        let r = plant_default_and_load("mail.example.com", &mut ops2, CHAIN, KEY).expect("plant");
        assert_eq!(ops2.calls, vec!["plant-default cert-2", "reload", "destroy cert-1"]);
        assert_eq!(r.removed, vec!["cert-1".to_string()]);
        // A failed cleanup is kept for the next plant, never fatal.
        let mut ops3 = FakeTls { next: 2, destroy_fail: vec!["cert-2".into()], ..FakeTls::new() };
        let r = plant_default_and_load("mail.example.com", &mut ops3, CHAIN, KEY).expect("plant");
        assert!(r.cleanup_error.unwrap().contains("cert-2"));
        let rec = super::super::renew::load().names["mail.example.com"].clone();
        assert_eq!(rec.stalwart_cert_id.as_deref(), Some("cert-3"));
        assert_eq!(rec.stale_cert_ids, vec!["cert-2".to_string()]);
        // Other names' certificates are never touched.
        let mut ops4 = FakeTls { next: 10, ..FakeTls::new() };
        plant_default_and_load("mail.example.org", &mut ops4, CHAIN, KEY).expect("plant");
        assert_eq!(ops4.calls, vec!["plant-default cert-11", "reload"]);
    }

    #[test]
    fn plant_mail_pem_ok_when_jmap_injected() {
        let _g = set_test_jmap_plant(Some(Ok(())));
        plant_mail_pem(
            "mail.example.com",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n",
        )
        .expect("injected JMAP plant");
        assert_eq!(test_jmap_plant_calls(), 1);
    }
}
