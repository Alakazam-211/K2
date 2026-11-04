//! On-box PEM store for custom-domain certs (`~/.k2/certs/<hostname>/`).
//! Private key is 0600; chain is 0644. Never silent-rcgen.

use std::fs;
use std::path::{Path, PathBuf};

use crate::mail::jmap::{CertificateInfo, DefaultCertificate};

pub fn certs_root() -> PathBuf {
    k2_core::paths::home_dir()
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

/// Atomic write whose temp file is CREATED with `mode` (never written at
/// the umask and chmod'ed afterwards), then renamed over `path`.
pub(crate) fn write_tmp_rename(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    k2_core::fs_atomic::atomic_write_mode(path, bytes, mode)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Persist the ACME account credentials (`certs/acme-account.json`),
/// owner-only from creation.
pub fn save_acme_account(body: &str) -> Result<(), String> {
    write_tmp_rename(&acme_account_path(), body.as_bytes(), 0o600)
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
/// is JMAP `x:Certificate/set`; only the primary mail host also becomes
/// `defaultCertificateId` ([`plant_for_host`]). Copying into
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
        if let Some(ops) = TEST_MAIL_TLS.with(|c| c.borrow().clone()) {
            return plant_for_host(
                hostname,
                mail_primary_host().as_deref(),
                &mut *ops.borrow_mut(),
                chain_pem,
                key_pem,
                chrono::Utc::now().timestamp(),
            )
            .map(|_| ());
        }
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
    let report = plant_for_host(
        hostname,
        mail_primary_host().as_deref(),
        &mut LiveMailTls { client },
        chain_pem,
        key_pem,
        chrono::Utc::now().timestamp(),
    )?;
    k2_core::log_debug!(
        "[domains/store] planted {hostname} as {} Stalwart certificate {} (loaded by {}{}){}",
        if report.is_default { "the default," } else { "a non-default" },
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
    if let Some(line) = report.repair.as_ref().and_then(DefaultRepair::log_line) {
        k2_core::log_debug!("{line}");
    }
    Ok(())
}

fn norm_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// The box's primary mail host (`mail_server.hostname`): the one name whose
/// certificate is Stalwart's default. `k2 hostmail ptr set` changes it.
pub fn mail_primary_host() -> Option<String> {
    crate::mail::supervisor::row_field("hostname")
        .map(|h| norm_host(&h))
        .filter(|h| !h.is_empty())
}

/// Is `hostname` the box's primary mail host?
pub fn is_mail_primary(hostname: &str) -> bool {
    mail_primary_host().is_some_and(|p| p == norm_host(hostname))
}

/// The Stalwart calls the mail-certificate plant makes. Production =
/// [`LiveMailTls`]; tests inject a recording fake.
pub trait MailTlsOps {
    /// `x:Certificate/set` create + `defaultCertificateId` → the new id.
    fn plant_default(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String>;
    /// `x:Certificate/set` create only; the default stays where it is.
    fn add_certificate(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String>;
    /// Which Certificate answers clients that send no SNI name.
    fn default_certificate(&mut self) -> Result<DefaultCertificate, String>;
    /// Every stored Certificate, without its key.
    fn list_certificates(&mut self) -> Result<Vec<CertificateInfo>, String>;
    /// Point `defaultCertificateId` at a stored Certificate.
    fn set_default_certificate(&mut self, id: &str) -> Result<(), String>;
    /// `x:Action/set` `ReloadTlsCertificates` (no restart).
    fn reload_tls(&mut self) -> Result<(), String>;
    /// The existing `restart_stalwart_to_reload_tls` path (mail helper,
    /// else `sudo -n systemctl restart stalwart`).
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
    fn add_certificate(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String> {
        self.client.certificate_add(chain_pem, key_pem)
    }
    fn default_certificate(&mut self) -> Result<DefaultCertificate, String> {
        self.client.default_certificate_state()
    }
    fn list_certificates(&mut self) -> Result<Vec<CertificateInfo>, String> {
        self.client.certificate_list()
    }
    fn set_default_certificate(&mut self, id: &str) -> Result<(), String> {
        self.client.default_certificate_set(id)
    }
    fn reload_tls(&mut self) -> Result<(), String> {
        self.client.action_reload_tls_certificates()
    }
    fn restart_stalwart(&mut self) -> Result<(), String> {
        crate::mail::supervisor::restart_stalwart_to_reload_tls()
    }
    fn destroy_certificate(&mut self, id: &str) -> Result<(), String> {
        self.client.certificate_destroy(id)
    }
}

/// What a mail plant did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlantReport {
    pub cert_id: String,
    /// This plant made the new certificate Stalwart's default.
    pub is_default: bool,
    /// `action` (ReloadTlsCertificates) | `restart` (fallback).
    pub loaded_by: &'static str,
    pub reload_note: Option<String>,
    /// Earlier certificates K2 planted for this name, now destroyed.
    pub removed: Vec<String>,
    pub cleanup_error: Option<String>,
    /// The default-certificate check that ran after the plant.
    pub repair: Option<DefaultRepair>,
}

/// Install one role-`mail` host's certificate. Only the primary mail host
/// (`mail_server.hostname`) is Stalwart's default, because the default also
/// answers everyone who sends no SNI name or one Stalwart doesn't know, and
/// STARTTLS on 25 is mostly those. Every other mail host goes in beside it
/// and is served by SNI. Each run ends with the default-certificate check.
pub fn plant_for_host(
    hostname: &str,
    primary: Option<&str>,
    ops: &mut dyn MailTlsOps,
    chain_pem: &str,
    key_pem: &str,
    now: i64,
) -> Result<PlantReport, String> {
    let name = norm_host(hostname);
    let primary = primary.map(norm_host).filter(|p| !p.is_empty());
    match primary.as_deref() {
        Some(p) if p == name => {
            let mut report = plant_default_and_load(hostname, ops, chain_pem, key_pem)?;
            // After the cleanup: what this plant superseded must not win.
            // The caller's name lock (when it has one) is on this name.
            report.repair = Some(repair_default_certificate(p, ops, now));
            Ok(report)
        }
        _ => plant_other_host_and_load(hostname, primary.as_deref(), ops, chain_pem, key_pem, now),
    }
}

/// The mail host's certificate IS Stalwart's default (it also answers
/// clients that send no SNI name): plant it as a new Certificate and
/// point `defaultCertificateId` at it, then load it with
/// `ReloadTlsCertificates` — a restart (helper, else sudo) only if that action
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
    let name = norm_host(hostname);
    let id = ops
        .plant_default(chain_pem, key_pem)
        .map_err(|e| format!("plant mail PEM via x:Certificate/set: {e}"))?;
    let stale = record_planted(&name, &id, chain_pem);
    let (loaded_by, reload_note) = load_planted(&name, &id, ops)?;
    let (removed, cleanup_error) = remove_superseded(&name, &id, &stale, ops);
    Ok(PlantReport {
        cert_id: id,
        is_default: true,
        loaded_by,
        reload_note,
        removed,
        cleanup_error,
        repair: None,
    })
}

/// A role-`mail` host that isn't the primary: one more Certificate, default
/// untouched, loaded and cleaned up the same way. The default check runs
/// BEFORE the cleanup, so a box whose default is still this name's old
/// certificate moves off it first and that certificate can go. Unlike the
/// per-name certificates this is never refused while Stalwart has no live
/// default: the check has just looked for the primary's certificate, so
/// there is nothing right to give clients without SNI, and refusing would
/// only let this name's certificate lapse as well.
fn plant_other_host_and_load(
    hostname: &str,
    primary: Option<&str>,
    ops: &mut dyn MailTlsOps,
    chain_pem: &str,
    key_pem: &str,
    now: i64,
) -> Result<PlantReport, String> {
    let name = norm_host(hostname);
    let id = ops
        .add_certificate(chain_pem, key_pem)
        .map_err(|e| format!("plant mail PEM via x:Certificate/set: {e}"))?;
    let stale = record_planted(&name, &id, chain_pem);
    let (loaded_by, reload_note) = load_planted(&name, &id, ops)?;
    let repair = primary.map(|p| match super::renew::NameLock::try_acquire(p) {
        Some(_lock) => repair_default_certificate(p, ops, now),
        None => DefaultRepair::Busy,
    });
    let (removed, cleanup_error) = remove_superseded(&name, &id, &stale, ops);
    Ok(PlantReport {
        cert_id: id,
        is_default: false,
        loaded_by,
        reload_note,
        removed,
        cleanup_error,
        repair,
    })
}

/// Record the new id before loading: if the load fails, the next plant
/// still knows to remove this object. Returns the ids to remove.
fn record_planted(name: &str, id: &str, chain_pem: &str) -> Vec<String> {
    let not_after = super::status::pem_leaf_info(chain_pem).map(|l| l.not_after);
    super::renew::update(|f| {
        let rec = f.names.entry(name.to_string()).or_default();
        if let Some(prev) = rec.stalwart_cert_id.replace(id.to_string()) {
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
    })
}

/// `ReloadTlsCertificates`, a restart only if that fails. `Err` = neither.
fn reload_or_restart(ops: &mut dyn MailTlsOps) -> Result<(&'static str, Option<String>), String> {
    match ops.reload_tls() {
        Ok(()) => Ok(("action", None)),
        Err(action_err) => match ops.restart_stalwart() {
            Ok(()) => Ok((
                "restart",
                Some(format!("ReloadTlsCertificates failed ({action_err}); restarted Stalwart instead")),
            )),
            Err(restart_err) => Err(format!(
                "ReloadTlsCertificates failed ({action_err}) and restart stalwart to load planted \
                 cert (not hostmail disable) failed ({restart_err}) — it loads on Stalwart's next \
                 restart"
            )),
        },
    }
}

fn load_planted(
    name: &str,
    id: &str,
    ops: &mut dyn MailTlsOps,
) -> Result<(&'static str, Option<String>), String> {
    reload_or_restart(ops).map_err(|e| format!("planted {name} as Stalwart certificate {id}, but {e}"))
}

/// Destroy what this plant superseded, never the certificate that is the
/// default right now: that would leave the box with no default. One that is
/// stays on the list for the next plant (Stalwart deletes it once it
/// expires). Can't read the default = keep them all.
fn remove_superseded(
    name: &str,
    id: &str,
    stale: &[String],
    ops: &mut dyn MailTlsOps,
) -> (Vec<String>, Option<String>) {
    let old: Vec<&String> = stale.iter().filter(|s| *s != id).collect();
    if old.is_empty() {
        return (Vec::new(), None);
    }
    let default = match ops.default_certificate() {
        Ok(DefaultCertificate::Present(d)) => Some(d),
        Ok(_) => None,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("kept {old:?}: could not read the default certificate: {e}")),
            )
        }
    };
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for old in old {
        if default.as_ref() == Some(old) {
            errors.push(format!("{old}: kept, it is still the default certificate"));
            continue;
        }
        match ops.destroy_certificate(old) {
            Ok(()) => removed.push(old.clone()),
            Err(e) => errors.push(format!("{old}: {e}")),
        }
    }
    if !removed.is_empty() {
        let gone = removed.clone();
        let _ = super::renew::update(|f| {
            if let Some(rec) = f.names.get_mut(name) {
                rec.stale_cert_ids.retain(|s| !gone.contains(s));
            }
        });
    }
    (removed, (!errors.is_empty()).then(|| errors.join("; ")))
}

// ── The default certificate ─────────────────────────────────────────────

/// Is `cert_name` (a certificate's SAN) a name that covers `host`?
fn name_covers(cert_name: &str, host: &str) -> bool {
    let cert_name = norm_host(cert_name);
    match cert_name.strip_prefix("*.") {
        Some(rest) => host.split_once('.').is_some_and(|(_, parent)| parent == rest),
        None => cert_name == host,
    }
}

/// A certificate Stalwart would serve for `host` right now: covers the name,
/// inside its dates (Stalwart skips one not yet valid, and deletes an expired
/// one at its next load), not a self-signed placeholder.
fn serves_host(cert: &CertificateInfo, host: &str, now: i64) -> bool {
    cert.names.iter().any(|n| name_covers(n, host))
        && cert.not_before <= now
        && cert.not_after > now
        && !super::status::looks_rcgen(&cert.issuer, "")
}

/// What the default certificate should be, given what's in Stalwart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultDecision {
    /// The default already is the primary's newest certificate.
    Right { id: String },
    /// The primary's newest certificate is another one.
    Wrong { from: Option<String>, to: String },
    /// Stalwart holds no live certificate for the primary, so there is
    /// nothing right to point at. `current` is the default as it stands.
    NoLivePrimary { current: Option<String> },
}

/// Pure. The primary's newest certificate is the one with the latest
/// notAfter, the same rule Stalwart uses per name; a default that ties with
/// it is left alone.
pub fn decide_default(
    primary: &str,
    current: &DefaultCertificate,
    certs: &[CertificateInfo],
    now: i64,
) -> DefaultDecision {
    let primary = norm_host(primary);
    let current_id = match current {
        DefaultCertificate::Present(id) => Some(id.clone()),
        DefaultCertificate::Unset | DefaultCertificate::Missing(_) => None,
    };
    let best = certs
        .iter()
        .filter(|c| serves_host(c, &primary, now))
        .max_by(|a, b| a.not_after.cmp(&b.not_after).then_with(|| a.id.cmp(&b.id)));
    let Some(best) = best else {
        return DefaultDecision::NoLivePrimary { current: current_id };
    };
    let current_is_as_new = current_id.as_ref().is_some_and(|id| {
        certs
            .iter()
            .any(|c| &c.id == id && serves_host(c, &primary, now) && c.not_after == best.not_after)
    });
    match current_id {
        Some(id) if current_is_as_new => DefaultDecision::Right { id },
        from => DefaultDecision::Wrong { from, to: best.id.clone() },
    }
}

/// The warning when the primary has no live certificate in Stalwart, for the
/// log and the doctor.
pub fn no_live_primary_warning(primary: &str, current: Option<&str>) -> String {
    format!(
        "Stalwart has no live certificate for {primary}, so the default certificate stays {} \
         until it has one. Get one with `k2 domain name add {primary} --role mail` (if it isn't \
         attached yet), then `k2 cert issue {primary}`; it becomes the default then.",
        current.unwrap_or("unset")
    )
}

/// What [`repair_default_certificate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultRepair {
    AlreadyRight { id: String },
    /// `loaded_by`: `action` | `restart` | `next-restart` (stored, neither
    /// load worked; `note` says why).
    Repointed {
        primary: String,
        from: Option<String>,
        to: String,
        loaded_by: &'static str,
        note: Option<String>,
    },
    /// Nothing was changed. `why` is the warning.
    LeftAlone { primary: String, why: String },
    /// A run for the primary holds its name; that run sets the default.
    Busy,
}

impl DefaultRepair {
    /// The one log line, when there is something to say: a repair logs when
    /// it moves the default, and is silent from then on.
    pub fn log_line(&self) -> Option<String> {
        const P: &str = "[domains/store] default certificate:";
        match self {
            DefaultRepair::AlreadyRight { .. } | DefaultRepair::Busy => None,
            DefaultRepair::Repointed { primary, from, to, loaded_by, note } => Some(format!(
                "{P} was {}, not {primary}'s newest; now {to} (loaded by {loaded_by}{})",
                from.as_deref().unwrap_or("unset"),
                note.as_deref().map(|n| format!("; {n}")).unwrap_or_default(),
            )),
            DefaultRepair::LeftAlone { primary, why } => {
                Some(format!("{P} WARNING for {primary}: {why}"))
            }
        }
    }

    /// The `defaultCertificate` block of `k2 hostmail ptr set`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            DefaultRepair::AlreadyRight { id } => {
                serde_json::json!({ "state": "already-right", "certificateId": id })
            }
            DefaultRepair::Repointed { from, to, loaded_by, note, .. } => serde_json::json!({
                "state": "moved",
                "from": from,
                "certificateId": to,
                "loadedBy": loaded_by,
                "note": note,
            }),
            DefaultRepair::LeftAlone { why, .. } => {
                serde_json::json!({ "state": "left-alone", "warning": why })
            }
            DefaultRepair::Busy => serde_json::json!({
                "state": "busy",
                "warning": "another certificate run holds this name; it sets the default itself",
            }),
        }
    }
}

/// Make sure the default certificate is the primary mail host's newest. Runs
/// at boot and after every mail plant, so a box left wrong by an earlier K2
/// heals by itself. It only
/// ever moves the default onto a live certificate for the primary: the old
/// default stays until the write succeeded, nothing is destroyed here, and
/// when the primary has no live certificate (or Stalwart can't be read)
/// nothing changes and the caller logs the warning. Hot reload first; a
/// restart only if that fails; never hostmail disable/enable.
pub fn repair_default_certificate(
    primary: &str,
    ops: &mut dyn MailTlsOps,
    now: i64,
) -> DefaultRepair {
    let primary = norm_host(primary);
    let left_alone = |why: String| DefaultRepair::LeftAlone { primary: primary.clone(), why };
    let current = match ops.default_certificate() {
        Ok(c) => c,
        Err(e) => return left_alone(format!("could not read the default certificate: {e}")),
    };
    let certs = match ops.list_certificates() {
        Ok(c) => c,
        Err(e) => return left_alone(format!("could not read Stalwart's certificates: {e}")),
    };
    match decide_default(&primary, &current, &certs, now) {
        DefaultDecision::Right { id } => DefaultRepair::AlreadyRight { id },
        DefaultDecision::NoLivePrimary { current } => {
            left_alone(no_live_primary_warning(&primary, current.as_deref()))
        }
        DefaultDecision::Wrong { from, to } => {
            if let Err(e) = ops.set_default_certificate(&to) {
                return left_alone(format!(
                    "could not point the default at {to}, so it stays {}: {e}",
                    from.as_deref().unwrap_or("unset")
                ));
            }
            let (loaded_by, note) = match reload_or_restart(ops) {
                Ok(loaded) => loaded,
                Err(e) => ("next-restart", Some(e)),
            };
            DefaultRepair::Repointed { primary, from, to, loaded_by, note }
        }
    }
}

/// The check against the live server (boot). A run for the primary that
/// holds its name sets the default and checks it again.
pub fn repair_default_live(
    primary: &str,
    client: crate::mail::jmap::StalwartClient,
) -> DefaultRepair {
    let Some(_lock) = super::renew::NameLock::try_acquire(primary) else {
        return DefaultRepair::Busy;
    };
    let report = repair_default_certificate(
        primary,
        &mut LiveMailTls { client },
        chrono::Utc::now().timestamp(),
    );
    if let Some(line) = report.log_line() {
        k2_core::log_debug!("{line}");
    }
    report
}

#[cfg(test)]
thread_local! {
    static TEST_JMAP_PLANT: std::cell::RefCell<Option<Result<(), String>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_JMAP_PLANT_CALLS: std::cell::RefCell<usize> =
        const { std::cell::RefCell::new(0) };
}

#[cfg(test)]
thread_local! {
    static TEST_MAIL_TLS: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<dyn MailTlsOps>>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct MailTlsGuard;

#[cfg(test)]
impl Drop for MailTlsGuard {
    fn drop(&mut self) {
        TEST_MAIL_TLS.with(|c| *c.borrow_mut() = None);
    }
}

/// Test seam: run a mail plant through the real [`plant_for_host`] against
/// this fake (it reads the primary from the `mail_server` row).
#[cfg(test)]
pub(crate) fn set_test_mail_tls(ops: std::rc::Rc<std::cell::RefCell<dyn MailTlsOps>>) -> MailTlsGuard {
    TEST_MAIL_TLS.with(|c| *c.borrow_mut() = Some(ops));
    MailTlsGuard
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

/// A recording Stalwart for the mail plant: holds certificates and the
/// default, and records every write (reads aren't recorded).
#[cfg(test)]
pub(crate) mod fake_tls {
    use super::*;

    pub(crate) const DAY: i64 = 86_400;

    /// A certificate row: `name`, expiring `not_after`, issued 90 days before.
    pub(crate) fn cert(id: &str, name: &str, not_after: i64) -> CertificateInfo {
        CertificateInfo {
            id: id.to_string(),
            names: vec![name.to_string()],
            not_before: not_after - 90 * DAY,
            not_after,
            issuer: "CN=R11, O=Let's Encrypt".to_string(),
        }
    }

    pub(crate) struct FakeTls {
        pub(crate) calls: Vec<String>,
        pub(crate) reload: Result<(), String>,
        pub(crate) restart: Result<(), String>,
        pub(crate) destroy_fail: Vec<String>,
        pub(crate) next: usize,
        pub(crate) certs: Vec<CertificateInfo>,
        pub(crate) default: Option<String>,
        /// The name and notAfter the next planted certificate gets.
        pub(crate) planting: Option<(String, i64)>,
        pub(crate) read_err: Option<String>,
        pub(crate) list_err: Option<String>,
        pub(crate) set_default_err: Option<String>,
    }

    impl FakeTls {
        pub(crate) fn new() -> Self {
            Self {
                calls: Vec::new(),
                reload: Ok(()),
                restart: Ok(()),
                destroy_fail: Vec::new(),
                next: 0,
                certs: Vec::new(),
                default: None,
                planting: None,
                read_err: None,
                list_err: None,
                set_default_err: None,
            }
        }

        /// A box holding these certificates with this default.
        pub(crate) fn holding(certs: Vec<CertificateInfo>, default: Option<&str>) -> Self {
            Self { next: 100, certs, default: default.map(String::from), ..Self::new() }
        }

        pub(crate) fn state(&self) -> DefaultCertificate {
            match &self.default {
                None => DefaultCertificate::Unset,
                Some(id) if self.certs.iter().any(|c| &c.id == id) => {
                    DefaultCertificate::Present(id.clone())
                }
                Some(id) => DefaultCertificate::Missing(id.clone()),
            }
        }

        fn store(&mut self) -> String {
            self.next += 1;
            let id = format!("cert-{}", self.next);
            let (name, not_after) = self.planting.clone().unwrap_or_default();
            self.certs.push(if name.is_empty() {
                CertificateInfo {
                    id: id.clone(),
                    names: Vec::new(),
                    not_before: 0,
                    not_after: 0,
                    issuer: String::new(),
                }
            } else {
                cert(&id, &name, not_after)
            });
            id
        }
    }

    impl MailTlsOps for FakeTls {
        fn plant_default(&mut self, _c: &str, _k: &str) -> Result<String, String> {
            let id = self.store();
            self.default = Some(id.clone());
            self.calls.push(format!("plant-default {id}"));
            Ok(id)
        }
        fn add_certificate(&mut self, _c: &str, _k: &str) -> Result<String, String> {
            let id = self.store();
            self.calls.push(format!("add {id}"));
            Ok(id)
        }
        fn default_certificate(&mut self) -> Result<DefaultCertificate, String> {
            match &self.read_err {
                Some(e) => Err(e.clone()),
                None => Ok(self.state()),
            }
        }
        fn list_certificates(&mut self) -> Result<Vec<CertificateInfo>, String> {
            match &self.list_err {
                Some(e) => Err(e.clone()),
                None => Ok(self.certs.clone()),
            }
        }
        fn set_default_certificate(&mut self, id: &str) -> Result<(), String> {
            self.calls.push(format!("set-default {id}"));
            if let Some(e) = &self.set_default_err {
                return Err(e.clone());
            }
            self.default = Some(id.to_string());
            Ok(())
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
            self.certs.retain(|c| c.id != id);
            Ok(())
        }
    }
}

/// Driven by tests/mail/stalwart-default-cert-probe.py: K2's real plant and
/// repair against a real, throwaway Stalwart, with real TLS handshakes.
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::domains::acme::test_le_pem_until;
    use crate::domains::status::{pem_leaf_sha256, served_leaf_at, Served};
    use crate::mail::jmap::StalwartClient;

    const DAY: i64 = 86_400;

    #[test]
    #[ignore = "live: tests/mail/stalwart-default-cert-probe.py runs this against a throwaway Stalwart"]
    fn live_default_certificate_on_a_real_stalwart() {
        const PRIMARY: &str = "mail.primary.test";
        const SECOND: &str = "mail.second.test";
        const NEW: &str = "mail.new.test";
        const ABSENT: &str = "mail.absent.test";
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is unset"));
        let (url, key) = (env("DC_LIVE_URL"), env("DC_LIVE_KEY"));
        let port: u16 = env("DC_LIVE_TLS_PORT").parse().expect("port");
        let _home = crate::test_support::TempHome::new();
        let _ = k2_core::db::init_for_tests();
        let client = || StalwartClient::new(url.clone(), key.clone());
        let now = chrono::Utc::now().timestamp();

        // An IP as the server name: rustls sends no SNI.
        let served = |sni: &str| match served_leaf_at("127.0.0.1", sni, port) {
            Served::Leaf(s) => s,
            Served::Nothing => panic!("no TLS answer for {sni} on :{port}"),
        };
        let no_sni = || served("127.0.0.1");
        let sha = |pem: &super::InstalledPem| pem_leaf_sha256(&pem.chain_pem).expect("leaf sha");
        let plant = |host: &str, primary: &str, pem: &super::InstalledPem| {
            let r = plant_for_host(host, Some(primary), &mut LiveMailTls { client: client() }, &pem.chain_pem, &pem.key_pem, now)
                .unwrap_or_else(|e| panic!("plant {host}: {e}"));
            assert_eq!(r.loaded_by, "action", "hot reload for {host}: {:?}", r.reload_note);
            r
        };
        let repair = |primary: &str| {
            repair_default_certificate(primary, &mut LiveMailTls { client: client() }, now)
        };
        let force_default = |id: &str| {
            client().default_certificate_set(id).expect("set default");
            client().action_reload_tls_certificates().expect("reload");
        };

        // Two certificates; the second host's expires later, the case that lost.
        let p1 = test_le_pem_until(PRIMARY, now + 60 * DAY);
        let r = plant(PRIMARY, PRIMARY, &p1);
        assert!(r.is_default);
        let p1_id = r.cert_id;
        assert_eq!(no_sni(), sha(&p1), "primary plant: no SNI");

        let s1 = test_le_pem_until(SECOND, now + 80 * DAY);
        let r = plant(SECOND, PRIMARY, &s1);
        assert!(!r.is_default);
        assert_eq!(r.repair, Some(DefaultRepair::AlreadyRight { id: p1_id.clone() }));
        assert_eq!(served(PRIMARY), sha(&p1), "SNI primary");
        assert_eq!(served(SECOND), sha(&s1), "SNI second");
        assert_eq!(no_sni(), sha(&p1), "no SNI stays on the primary");
        assert_eq!(served("nobody.example.org"), sha(&p1), "an unknown SNI gets the primary");
        println!("DC_LIVE step two-certificates ok");

        // The second host renews: its old certificate goes, the default stays.
        let s2 = test_le_pem_until(SECOND, now + 85 * DAY);
        let r = plant(SECOND, PRIMARY, &s2);
        assert_eq!(r.cleanup_error, None);
        assert_eq!(r.removed.len(), 1, "{:?}", r.removed);
        let s2_id = r.cert_id;
        assert!(!client().certificate_list().expect("list").iter().any(|c| c.id == r.removed[0]));
        assert_eq!(served(SECOND), sha(&s2), "SNI second after renew");
        assert_eq!(no_sni(), sha(&p1), "no SNI after the second host's renew");
        println!("DC_LIVE step second-renew ok");

        // A box already wrong, as an earlier K2 left it.
        force_default(&s2_id);
        assert_eq!(no_sni(), sha(&s2), "fixture: the proof can see a wrong default");
        let healed = repair(PRIMARY);
        assert!(matches!(&healed, DefaultRepair::Repointed { to, loaded_by: "action", .. } if *to == p1_id), "{healed:?}");
        assert_eq!(no_sni(), sha(&p1), "repair: no SNI back on the primary, no restart");
        assert_eq!(repair(PRIMARY), DefaultRepair::AlreadyRight { id: p1_id.clone() }, "repair is once");
        println!("DC_LIVE step repair ok");

        // The boot path heals it too.
        force_default(&s2_id);
        let booted = crate::mail::cert_owner::boot_checks(PRIMARY, client());
        assert!(matches!(&booted, DefaultRepair::Repointed { to, .. } if *to == p1_id), "{booted:?}");
        assert_eq!(no_sni(), sha(&p1), "boot checks: no SNI back on the primary");
        println!("DC_LIVE step boot-checks ok");

        // No certificate for the primary, or only an expired one: nothing moves.
        let before = client().default_certificate_state().expect("state");
        assert!(matches!(repair(ABSENT), DefaultRepair::LeftAlone { .. }));
        let expired = test_le_pem_until(ABSENT, now - 2 * DAY);
        client().certificate_add(&expired.chain_pem, &expired.key_pem).expect("add expired");
        client().action_reload_tls_certificates().expect("reload");
        assert!(matches!(repair(ABSENT), DefaultRepair::LeftAlone { .. }));
        assert_eq!(client().default_certificate_state().expect("state"), before, "default unchanged");
        assert_eq!(no_sni(), sha(&p1), "no SNI unchanged");
        println!("DC_LIVE step left-alone ok");

        // A change of primary, the old name kept and renewing later than the new.
        let n1 = test_le_pem_until(NEW, now + 70 * DAY);
        let n1_id = plant(NEW, PRIMARY, &n1).cert_id;
        assert_eq!(no_sni(), sha(&p1), "before the change");
        assert!(matches!(repair(NEW), DefaultRepair::Repointed { .. }));
        assert_eq!(no_sni(), sha(&n1), "the new primary takes the default");
        let p2 = test_le_pem_until(PRIMARY, now + 90 * DAY);
        let r = plant(PRIMARY, NEW, &p2);
        assert!(!r.is_default);
        assert_eq!(r.repair, Some(DefaultRepair::AlreadyRight { id: n1_id }));
        assert_eq!(no_sni(), sha(&n1), "the old name's renewal doesn't take it back");
        assert_eq!(served(PRIMARY), sha(&p2), "the old name still has its own");
        println!("DC_LIVE step primary-change ok");

        // Rollback: the old name is the primary again.
        let p2_id = r.cert_id;
        assert!(matches!(repair(PRIMARY), DefaultRepair::Repointed { to, .. } if to == p2_id));
        assert_eq!(no_sni(), sha(&p2), "rollback");
        println!("DC_LIVE step rollback ok");

        println!("DC_LIVE sha:primary {}", sha(&p2));
    }
}

#[cfg(test)]
mod tests {
    use super::fake_tls::{cert, FakeTls, DAY};
    use super::*;

    #[cfg(unix)]
    #[test]
    fn acme_account_and_config_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _home = crate::test_support::TempHome::new();
        save_acme_account("{\"id\":\"x\"}").expect("save account");
        save_acme_config(&AcmeConfig::default()).expect("save config");
        for p in [acme_account_path(), acme_config_path()] {
            let mode = fs::metadata(&p).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} must be 0600", p.display());
        }
        assert_eq!(
            fs::read_to_string(acme_account_path()).expect("read"),
            "{\"id\":\"x\"}"
        );
    }

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

    const PRIMARY: &str = "mail.primary.test";
    const SECOND: &str = "mail.second.test";
    const NOW: i64 = 2_000_000_000;

    fn plant(host: &str, ops: &mut FakeTls) -> PlantReport {
        plant_for_host(host, Some(PRIMARY), ops, CHAIN, KEY, NOW).expect("plant")
    }

    /// The bug: a second mail host's issue and renew never move the default.
    #[test]
    fn second_host_issue_and_renew_leave_the_default_alone() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::holding(vec![cert("primary-1", PRIMARY, NOW + 60 * DAY)], Some("primary-1"));
        ops.planting = Some((SECOND.into(), NOW + 80 * DAY));

        let issued = plant(SECOND, &mut ops);
        assert!(!issued.is_default);
        assert_eq!(ops.calls, vec!["add cert-101", "reload"], "add + hot reload, nothing else");
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
        assert_eq!(issued.repair, Some(DefaultRepair::AlreadyRight { id: "primary-1".into() }));
        assert_eq!(issued.repair.as_ref().unwrap().log_line(), None, "nothing to log");

        // Renew: later notAfter than the primary, the case that used to win.
        ops.calls.clear();
        ops.planting = Some((SECOND.into(), NOW + 90 * DAY));
        let renewed = plant(SECOND, &mut ops);
        assert_eq!(ops.calls, vec!["add cert-102", "reload", "destroy cert-101"]);
        assert_eq!(renewed.removed, vec!["cert-101".to_string()]);
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
        let rec = super::super::renew::load().names[SECOND].clone();
        assert_eq!(rec.stalwart_cert_id.as_deref(), Some("cert-102"));
        assert!(rec.stale_cert_ids.is_empty(), "{:?}", rec.stale_cert_ids);
        assert!(
            !super::super::renew::load().names.contains_key(PRIMARY),
            "a second host's plant never touches the primary's record"
        );
    }

    #[test]
    fn primary_renew_still_sets_the_default_and_host_case_does_not_matter() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::holding(
            vec![cert("primary-1", PRIMARY, NOW + 10 * DAY), cert("second-1", SECOND, NOW + 60 * DAY)],
            Some("primary-1"),
        );
        ops.planting = Some((PRIMARY.into(), NOW + 90 * DAY));
        let r = plant_for_host("Mail.Primary.Test.", Some(" mail.primary.test. "), &mut ops, CHAIN, KEY, NOW)
            .expect("plant");
        assert!(r.is_default);
        assert_eq!(ops.calls, vec!["plant-default cert-101", "reload"]);
        assert_eq!(ops.state(), DefaultCertificate::Present("cert-101".into()));
        assert_eq!(r.repair, Some(DefaultRepair::AlreadyRight { id: "cert-101".into() }));
    }

    /// `k2 hostmail ptr set mail.<new>` with the old name kept: the default
    /// moves to the new primary's certificate, and renewing the old name
    /// (planted against the new primary) never takes it back.
    #[test]
    fn a_changed_primary_takes_the_default_and_the_old_name_never_takes_it_back() {
        let _home = crate::test_support::TempHome::new();
        const OLD: &str = "mail.old.test";
        const NEW: &str = "mail.new.test";
        let mut ops = FakeTls::holding(
            vec![cert("old-1", OLD, NOW + 70 * DAY), cert("new-1", NEW, NOW + 60 * DAY)],
            Some("old-1"),
        );
        let moved = repair_default_certificate(NEW, &mut ops, NOW);
        assert_eq!(ops.calls, vec!["set-default new-1", "reload"]);
        assert_eq!(ops.state(), DefaultCertificate::Present("new-1".into()));
        assert_eq!(moved.to_json()["state"], "moved");
        assert_eq!(moved.to_json()["from"], "old-1");
        assert_eq!(moved.to_json()["certificateId"], "new-1");

        // The old name renews, with a later notAfter than the new primary's.
        super::super::renew::update(|f| {
            f.names.entry(OLD.into()).or_default().stalwart_cert_id = Some("old-1".into())
        })
        .unwrap();
        ops.calls.clear();
        ops.planting = Some((OLD.into(), NOW + 90 * DAY));
        let renewed = plant_for_host(OLD, Some(NEW), &mut ops, CHAIN, KEY, NOW).expect("plant");
        assert!(!renewed.is_default);
        assert_eq!(ops.calls, vec!["add cert-101", "reload", "destroy old-1"]);
        assert_eq!(ops.state(), DefaultCertificate::Present("new-1".into()));
        assert_eq!(renewed.repair, Some(DefaultRepair::AlreadyRight { id: "new-1".into() }));

        // The new primary's own renewal still moves the default with it.
        ops.calls.clear();
        ops.planting = Some((NEW.into(), NOW + 90 * DAY));
        plant_for_host(NEW, Some(NEW), &mut ops, CHAIN, KEY, NOW).expect("plant");
        assert_eq!(ops.state(), DefaultCertificate::Present("cert-102".into()));
    }

    /// The same change before `mail.<new>` has a certificate: nothing moves,
    /// the warning says how to get one, and issuing it then moves the default.
    #[test]
    fn a_changed_primary_with_no_certificate_changes_nothing_and_says_so() {
        let _home = crate::test_support::TempHome::new();
        const OLD: &str = "mail.old.test";
        const NEW: &str = "mail.new.test";
        let mut ops = FakeTls::holding(vec![cert("old-1", OLD, NOW + 70 * DAY)], Some("old-1"));
        let r = repair_default_certificate(NEW, &mut ops, NOW);
        assert!(ops.calls.is_empty(), "{:?}", ops.calls);
        assert_eq!(ops.state(), DefaultCertificate::Present("old-1".into()));
        let v = r.to_json();
        assert_eq!(v["state"], "left-alone");
        let warning = v["warning"].as_str().expect("warning");
        assert!(warning.contains("no live certificate for mail.new.test"), "{warning}");
        assert!(warning.contains("stays old-1"), "{warning}");
        assert!(warning.contains("k2 domain name add mail.new.test --role mail"), "{warning}");
        assert!(warning.contains("k2 cert issue mail.new.test"), "{warning}");

        // The old name renews meanwhile: its old certificate is still the
        // default, so it stays until the default has somewhere to go.
        super::super::renew::update(|f| {
            f.names.entry(OLD.into()).or_default().stalwart_cert_id = Some("old-1".into())
        })
        .unwrap();
        ops.planting = Some((OLD.into(), NOW + 90 * DAY));
        let renewed = plant_for_host(OLD, Some(NEW), &mut ops, CHAIN, KEY, NOW).expect("plant");
        assert_eq!(ops.state(), DefaultCertificate::Present("old-1".into()), "never no default");
        assert!(renewed.cleanup_error.unwrap().contains("still the default"));

        // `k2 cert issue mail.<new>`: it's the primary, so it takes the default.
        ops.planting = Some((NEW.into(), NOW + 80 * DAY));
        let issued = plant_for_host(NEW, Some(NEW), &mut ops, CHAIN, KEY, NOW).expect("plant");
        assert!(issued.is_default);
        assert_eq!(ops.state(), DefaultCertificate::Present(issued.cert_id));
    }

    /// Unknown primary (no `mail_server.hostname`): never the default.
    #[test]
    fn no_primary_means_no_default_write_and_no_repair() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::new();
        let r = plant_for_host(SECOND, None, &mut ops, CHAIN, KEY, NOW).expect("plant");
        assert_eq!(ops.calls, vec!["add cert-1", "reload"]);
        assert_eq!(r.repair, None);
        assert_eq!(ops.state(), DefaultCertificate::Unset);
    }

    /// A second mail host is added even with no live default: clients without
    /// SNI have nothing right to get either way, and it must not lapse.
    #[test]
    fn second_host_goes_in_while_the_box_has_no_default() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::new();
        ops.planting = Some((SECOND.into(), NOW + 80 * DAY));
        let r = plant(SECOND, &mut ops);
        assert_eq!(ops.calls, vec!["add cert-1", "reload"]);
        assert_eq!(ops.state(), DefaultCertificate::Unset, "not made the default");
        let line = r.repair.unwrap().log_line().expect("warned");
        assert!(line.contains("WARNING") && line.contains("no live certificate for mail.primary.test"), "{line}");
    }

    #[test]
    fn second_host_stale_cleanup_is_kept_for_next_time_when_it_fails_or_the_load_failed() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::holding(vec![cert("primary-1", PRIMARY, NOW + 60 * DAY)], Some("primary-1"));
        ops.planting = Some((SECOND.into(), NOW + 80 * DAY));
        // The first plant's load fails both ways: loud, nothing removed.
        ops.reload = Err("x".into());
        ops.restart = Err("y".into());
        let err = plant_for_host(SECOND, Some(PRIMARY), &mut ops, CHAIN, KEY, NOW).expect_err("unloaded");
        assert!(err.contains("cert-101") && err.contains("x") && err.contains("y"), "{err}");
        assert!(!ops.calls.iter().any(|c| c.starts_with("destroy")), "{:?}", ops.calls);
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
        // The next plant loads and takes the unloaded one away, if it can.
        ops.reload = Ok(());
        ops.destroy_fail = vec!["cert-101".into()];
        let r = plant(SECOND, &mut ops);
        assert!(r.cleanup_error.unwrap().contains("cert-101"));
        let rec = super::super::renew::load().names[SECOND].clone();
        assert_eq!(rec.stale_cert_ids, vec!["cert-101".to_string()]);
        ops.destroy_fail.clear();
        let r = plant(SECOND, &mut ops);
        assert_eq!(r.removed, vec!["cert-101".to_string(), "cert-102".to_string()]);
        assert!(super::super::renew::load().names[SECOND].stale_cert_ids.is_empty());
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
    }

    /// The box that's wrong today, and a renewal of that very name: the
    /// default moves to the primary first, then the old certificate can go.
    #[test]
    fn renewing_the_host_that_is_wrongly_default_heals_then_cleans_up() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::holding(
            vec![cert("primary-1", PRIMARY, NOW + 40 * DAY), cert("second-1", SECOND, NOW + 41 * DAY)],
            Some("second-1"),
        );
        super::super::renew::update(|f| {
            f.names.entry(SECOND.into()).or_default().stalwart_cert_id = Some("second-1".into())
        })
        .unwrap();
        ops.planting = Some((SECOND.into(), NOW + 90 * DAY));
        let r = plant(SECOND, &mut ops);
        assert_eq!(
            ops.calls,
            vec!["add cert-101", "reload", "set-default primary-1", "reload", "destroy second-1"],
        );
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
        assert_eq!(r.removed, vec!["second-1".to_string()]);
        assert!(matches!(r.repair, Some(DefaultRepair::Repointed { .. })), "{:?}", r.repair);
    }

    /// The same box with no live primary certificate: never destroys the
    /// default out from under it. Removed later, once the default has moved.
    #[test]
    fn a_stale_certificate_that_is_the_default_is_never_destroyed() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::holding(vec![cert("second-1", SECOND, NOW + 5 * DAY)], Some("second-1"));
        super::super::renew::update(|f| {
            f.names.entry(SECOND.into()).or_default().stalwart_cert_id = Some("second-1".into())
        })
        .unwrap();
        ops.planting = Some((SECOND.into(), NOW + 90 * DAY));
        let r = plant(SECOND, &mut ops);
        assert_eq!(ops.calls, vec!["add cert-101", "reload"]);
        assert_eq!(ops.state(), DefaultCertificate::Present("second-1".into()));
        assert!(r.removed.is_empty());
        assert!(r.cleanup_error.unwrap().contains("still the default"));
        assert_eq!(super::super::renew::load().names[SECOND].stale_cert_ids, vec!["second-1".to_string()]);

        // The primary's certificate arrives; its plant moves the default.
        ops.calls.clear();
        ops.planting = Some((PRIMARY.into(), NOW + 90 * DAY));
        plant(PRIMARY, &mut ops);
        assert_eq!(ops.state(), DefaultCertificate::Present("cert-102".into()));
        ops.calls.clear();
        ops.planting = Some((SECOND.into(), NOW + 90 * DAY));
        let r = plant(SECOND, &mut ops);
        assert_eq!(r.removed, vec!["second-1".to_string(), "cert-101".to_string()]);
        assert_eq!(ops.state(), DefaultCertificate::Present("cert-102".into()));
    }

    /// Can't read the default = can't tell what is safe to remove.
    #[test]
    fn cleanup_removes_nothing_when_the_default_cannot_be_read() {
        let _home = crate::test_support::TempHome::new();
        let mut ops = FakeTls::holding(vec![cert("primary-1", PRIMARY, NOW + 60 * DAY)], Some("primary-1"));
        ops.planting = Some((SECOND.into(), NOW + 80 * DAY));
        plant(SECOND, &mut ops);
        ops.read_err = Some("mgmt down".into());
        ops.calls.clear();
        let r = plant(SECOND, &mut ops);
        assert_eq!(ops.calls, vec!["add cert-102", "reload"]);
        assert!(r.cleanup_error.unwrap().contains("mgmt down"));
        assert_eq!(super::super::renew::load().names[SECOND].stale_cert_ids, vec!["cert-101".to_string()]);
    }

    /// A run for the primary holds its name: leave the default to that run.
    #[test]
    fn second_host_plant_skips_the_check_while_the_primary_is_being_renewed() {
        let _home = crate::test_support::TempHome::new();
        let primary = "mail.primary-busy.test";
        let mut ops = FakeTls::holding(
            vec![cert("primary-1", primary, NOW + 60 * DAY), cert("second-1", SECOND, NOW + 61 * DAY)],
            Some("second-1"),
        );
        let _lock = super::super::renew::NameLock::try_acquire(primary).expect("lock");
        let r = plant_for_host(SECOND, Some(primary), &mut ops, CHAIN, KEY, NOW).expect("plant");
        assert_eq!(r.repair, Some(DefaultRepair::Busy));
        assert_eq!(ops.state(), DefaultCertificate::Present("second-1".into()));
    }

    #[test]
    fn repair_heals_a_wrong_default_once_with_a_hot_reload() {
        let mut ops = FakeTls::holding(
            vec![
                cert("primary-old", PRIMARY, NOW + 5 * DAY),
                cert("primary-new", PRIMARY, NOW + 60 * DAY),
                cert("second-1", SECOND, NOW + 61 * DAY),
            ],
            Some("second-1"),
        );
        let r = repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert_eq!(ops.calls, vec!["set-default primary-new", "reload"], "no restart when the reload works");
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-new".into()));
        let line = r.log_line().expect("one line");
        assert!(line.contains("second-1") && line.contains("primary-new") && line.contains("by action"), "{line}");
        // Second run: silent, no write.
        ops.calls.clear();
        let again = repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert_eq!(again, DefaultRepair::AlreadyRight { id: "primary-new".into() });
        assert_eq!(again.log_line(), None);
        assert!(ops.calls.is_empty(), "{:?}", ops.calls);
    }

    #[test]
    fn repair_moves_an_old_primary_certificate_to_the_newest_and_keeps_a_tie() {
        let mut ops = FakeTls::holding(
            vec![cert("p-old", PRIMARY, NOW + 5 * DAY), cert("p-new", PRIMARY, NOW + 60 * DAY)],
            Some("p-old"),
        );
        repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert_eq!(ops.state(), DefaultCertificate::Present("p-new".into()));
        // Same notAfter (a re-plant of the same PEM): no write, no flapping.
        let mut ops = FakeTls::holding(
            vec![cert("a", PRIMARY, NOW + 60 * DAY), cert("z", PRIMARY, NOW + 60 * DAY)],
            Some("a"),
        );
        assert_eq!(repair_default_certificate(PRIMARY, &mut ops, NOW), DefaultRepair::AlreadyRight { id: "a".into() });
        assert!(ops.calls.is_empty());
    }

    #[test]
    fn repair_points_an_unset_or_dangling_default_at_the_primary() {
        for default in [None, Some("gone")] {
            let mut ops = FakeTls::holding(vec![cert("primary-1", PRIMARY, NOW + 60 * DAY)], default);
            repair_default_certificate(PRIMARY, &mut ops, NOW);
            assert_eq!(ops.calls, vec!["set-default primary-1", "reload"]);
            assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
        }
    }

    fn assert_left_alone(mut ops: FakeTls, default: Option<&str>, want: &str) {
        let before = ops.state();
        let r = repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert!(ops.calls.is_empty(), "no write: {:?}", ops.calls);
        assert_eq!(ops.state(), before, "default unchanged");
        assert_eq!(ops.default.as_deref(), default);
        let line = r.log_line().expect("warned");
        assert!(line.contains("WARNING") && line.contains(want), "{line}");
    }

    #[test]
    fn repair_does_nothing_when_the_primary_has_no_live_certificate() {
        let second = || cert("second-1", SECOND, NOW + 61 * DAY);
        // Absent.
        assert_left_alone(FakeTls::holding(vec![second()], Some("second-1")), Some("second-1"), "no live certificate");
        // Expired, and expiring this very second.
        for not_after in [NOW - DAY, NOW] {
            let ops = FakeTls::holding(vec![cert("primary-1", PRIMARY, not_after), second()], Some("second-1"));
            assert_left_alone(ops, Some("second-1"), "no live certificate");
        }
        // Not valid yet: Stalwart wouldn't load it.
        let mut early = cert("primary-1", PRIMARY, NOW + 200 * DAY);
        early.not_before = NOW + DAY;
        assert_left_alone(FakeTls::holding(vec![early, second()], Some("second-1")), Some("second-1"), "no live certificate");
        // A self-signed placeholder doesn't count, however long it lasts.
        let mut rcgen = cert("primary-1", PRIMARY, NOW + 3650 * DAY);
        rcgen.issuer = "CN=rcgen self signed cert".into();
        assert_left_alone(FakeTls::holding(vec![rcgen, second()], Some("second-1")), Some("second-1"), "no live certificate");
        // No certificates, no default at all: stays that way.
        assert_left_alone(FakeTls::new(), None, "no live certificate");
    }

    #[test]
    fn repair_does_nothing_when_stalwart_cannot_be_read_or_written() {
        let held = || {
            FakeTls::holding(
                vec![cert("primary-1", PRIMARY, NOW + 60 * DAY), cert("second-1", SECOND, NOW + 61 * DAY)],
                Some("second-1"),
            )
        };
        let mut ops = held();
        ops.read_err = Some("mgmt down".into());
        assert!(repair_default_certificate(PRIMARY, &mut ops, NOW).log_line().unwrap().contains("mgmt down"));
        assert!(ops.calls.is_empty());

        let mut ops = held();
        ops.list_err = Some("unreadable row".into());
        assert_left_alone(ops, Some("second-1"), "unreadable row");

        // The write fails: the old default still stands and nothing reloads.
        let mut ops = held();
        ops.set_default_err = Some("forbidden".into());
        let r = repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert_eq!(ops.calls, vec!["set-default primary-1"]);
        assert_eq!(ops.state(), DefaultCertificate::Present("second-1".into()));
        assert!(r.log_line().unwrap().contains("forbidden"));
    }

    #[test]
    fn repair_restarts_only_when_the_reload_fails_and_says_when_neither_works() {
        let held = || {
            FakeTls::holding(
                vec![cert("primary-1", PRIMARY, NOW + 60 * DAY), cert("second-1", SECOND, NOW + 61 * DAY)],
                Some("second-1"),
            )
        };
        let mut ops = held();
        ops.reload = Err("validationFailed".into());
        let r = repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert_eq!(ops.calls, vec!["set-default primary-1", "reload", "restart"]);
        assert!(matches!(r, DefaultRepair::Repointed { loaded_by: "restart", .. }), "{r:?}");

        let mut ops = held();
        ops.reload = Err("validationFailed".into());
        ops.restart = Err("no door".into());
        let r = repair_default_certificate(PRIMARY, &mut ops, NOW);
        assert_eq!(ops.state(), DefaultCertificate::Present("primary-1".into()));
        let line = r.log_line().unwrap();
        assert!(line.contains("next-restart") && line.contains("no door"), "{line}");
    }

    /// Stalwart resolves `*.parent` for one label only, and so does this.
    #[test]
    fn a_wildcard_covers_one_label_only() {
        let wild = |id: &str, name: &str| cert(id, name, NOW + 60 * DAY);
        let decide = |certs: &[CertificateInfo]| {
            decide_default(PRIMARY, &DefaultCertificate::Unset, certs, NOW)
        };
        assert_eq!(
            decide(&[wild("w", "*.PRIMARY.test")]),
            DefaultDecision::Wrong { from: None, to: "w".into() }
        );
        assert_eq!(decide(&[wild("w", "*.test")]), DefaultDecision::NoLivePrimary { current: None });
        assert_eq!(decide(&[wild("w", "primary.test")]), DefaultDecision::NoLivePrimary { current: None });
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
