//! Who owns the mail certificate: K2 or Stalwart's own ACME.
//!
//! ONE definition, used by status, the doctor, the boot reconcile and the
//! owner command ([`classify`] / [`mail_cert_owner`]):
//!
//! **K2-issuer box** = the mail host is attached in the domain inventory
//! (`k2 domain`, role `mail`) AND the mail certificate is one K2's issuer
//! planted (the box store holds K2's Let's Encrypt leaf for it, not a
//! `k2 cert upload`, and `renewal.json` records the Stalwart Certificate K2
//! planted). The port plan does not decide it (a tls-alpn box can be
//! K2-owned), and neither does where the customer's zone lives.
//!
//! On a K2-issuer box Stalwart's ACME is switched OFF for the mail host's
//! Domain (`certificateManagement: Manual`): otherwise a Stalwart renewal
//! with the same name set re-points `defaultCertificateId` at its own
//! certificate (`acme/renew.rs`). The switch is one registry write that
//! touches nothing else (no Certificate, no default id, no reload, no
//! restart). It happens after every successful K2 plant/renew of the mail
//! certificate and once per boot ([`spawn_startup_reconcile`]), and is
//! idempotent. Never through hostmail disable/enable.
//!
//! Never flipped back automatically. A box that stops being K2-issuer
//! (mail host detached) keeps `Manual`; the doctor warns and names the
//! owner command `k2 hostmail cert owner --stalwart-acme`
//! ([`restore_stalwart_acme`]), which sets `Automatic` locked to the mail
//! host only (CAL43).

use crate::domains::renew::{self, RenewRecord};
use crate::domains::status::LeafInfo;
use crate::mail::jmap::{sans_locked_to, CertManagement, StalwartClient};

/// The remedy command for a box that is no longer K2-issuer.
pub const RESTORE_COMMAND: &str = "k2 hostmail cert owner --stalwart-acme";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertOwner {
    /// K2 issues, installs and renews the mail certificate.
    K2,
    /// The mail host is not attached: Stalwart's own ACME (tls-alpn plan)
    /// or whatever serves it on other plans; K2 only locks the name list.
    StalwartAcme,
    /// Attached, but no K2-planted certificate on record yet (or an
    /// uploaded one).
    Unknown,
}

impl CertOwner {
    pub fn as_str(self) -> &'static str {
        match self {
            CertOwner::K2 => "k2",
            CertOwner::StalwartAcme => "stalwart-acme",
            CertOwner::Unknown => "unknown",
        }
    }
}

/// THE K2-issuer definition. Pure.
pub fn classify(
    mail_attached: bool,
    leaf: Option<&LeafInfo>,
    rec: Option<&RenewRecord>,
) -> CertOwner {
    if !mail_attached {
        return CertOwner::StalwartAcme;
    }
    let planted = rec.is_some_and(|r| r.stalwart_cert_id.is_some());
    if planted && renew::k2_issued(leaf, rec) {
        CertOwner::K2
    } else {
        CertOwner::Unknown
    }
}

fn norm(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Is `host` in the domain inventory with role `mail`?
pub fn mail_host_attached(host: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::get_name(&conn, &norm(host))
        .ok()
        .flatten()
        .is_some_and(|n| n.role == k2_core::domains::ROLE_MAIL)
}

/// [`classify`] over the live inventory, box store and `renewal.json`.
/// Local reads only.
pub fn mail_cert_owner(host: &str) -> CertOwner {
    let host = norm(host);
    if host.is_empty() {
        return CertOwner::Unknown;
    }
    let file = renew::load();
    let leaf = renew::leaf_from_store(&host);
    classify(mail_host_attached(&host), leaf.as_ref(), file.names.get(&host))
}

// ── Stalwart side ───────────────────────────────────────────────────────

/// The registry calls this module makes. Production = [`StalwartClient`].
pub trait CertMgmtApi {
    fn mail_domain_id(&self, host: &str) -> Result<Option<String>, String>;
    fn get(&self, domain_id: &str) -> Result<CertManagement, String>;
    fn set_manual(&self, domain_id: &str) -> Result<(), String>;
    fn set_locked(&self, domain_id: &str, provider: &str, host: &str) -> Result<(), String>;
    fn acme_providers(&self) -> Result<Vec<String>, String>;
}

impl CertMgmtApi for StalwartClient {
    fn mail_domain_id(&self, host: &str) -> Result<Option<String>, String> {
        self.mail_hostname_domain_id(host)
    }
    fn get(&self, domain_id: &str) -> Result<CertManagement, String> {
        self.domain_get_certificate_management(domain_id)
    }
    fn set_manual(&self, domain_id: &str) -> Result<(), String> {
        self.domain_set_cert_management_manual(domain_id)
    }
    fn set_locked(&self, domain_id: &str, provider: &str, host: &str) -> Result<(), String> {
        self.domain_set_cert_management_locked(domain_id, provider, host)
    }
    fn acme_providers(&self) -> Result<Vec<String>, String> {
        self.acme_provider_ids()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualSwitch {
    /// Already Manual; nothing written.
    AlreadyManual,
    /// Was Automatic; one `x:Domain/set` wrote Manual.
    Switched,
}

fn domain_id_for(api: &dyn CertMgmtApi, host: &str) -> Result<String, String> {
    api.mail_domain_id(host)?
        .ok_or_else(|| format!("no Stalwart domain carries the mail hostname {host}"))
}

/// Make the mail host's Domain `Manual` (idempotent: no write when it
/// already is). An unknown `@type` is never touched.
pub fn ensure_manual(api: &dyn CertMgmtApi, host: &str) -> Result<ManualSwitch, String> {
    let host = norm(host);
    let id = domain_id_for(api, &host)?;
    match api.get(&id)? {
        CertManagement::Manual => Ok(ManualSwitch::AlreadyManual),
        CertManagement::Automatic { .. } => {
            api.set_manual(&id)?;
            Ok(ManualSwitch::Switched)
        }
        CertManagement::Other(t) => Err(format!(
            "domain {id} has certificateManagement '@type' '{t}' — K2 only knows \
             Manual/Automatic; not touching it"
        )),
    }
}

/// Outcome of [`restore_stalwart_acme`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Already Automatic with exactly the mail host; nothing written.
    AlreadyStalwart,
    /// Automatic with another name list → locked to the mail host.
    Locked,
    /// Manual → Automatic locked to the mail host (Stalwart queues one
    /// ACME order for it).
    Switched { provider: String },
}

/// The owner command `k2 hostmail cert owner --stalwart-acme`: hand the
/// mail certificate back to Stalwart's ACME, ordering the mail host only.
/// Refused while K2 owns the certificate (detach the mail host first).
pub fn restore_stalwart_acme(
    api: &dyn CertMgmtApi,
    host: &str,
    owner: CertOwner,
) -> Result<Restore, String> {
    let host = norm(host);
    if owner == CertOwner::K2 {
        return Err(restore_stalwart_acme_refusal(&host));
    }
    let id = domain_id_for(api, &host)?;
    match api.get(&id)? {
        CertManagement::Automatic {
            acme_provider_id,
            subject_alternative_names,
        } => {
            if sans_locked_to(&subject_alternative_names, &host) {
                return Ok(Restore::AlreadyStalwart);
            }
            api.set_locked(&id, &acme_provider_id, &host)?;
            Ok(Restore::Locked)
        }
        CertManagement::Manual => {
            let providers = api.acme_providers()?;
            let provider = match providers.as_slice() {
                [one] => one.clone(),
                [] => {
                    return Err(
                        "Stalwart has no ACME provider (it was set up without ACME) — K2 \
                         cannot turn Stalwart ACME on for the mail host; attach the mail host \
                         (`k2 domain name add <host> --role mail`) and let K2 issue it"
                            .into(),
                    )
                }
                many => {
                    return Err(format!(
                        "Stalwart has {} ACME providers ({}) — K2 will not guess which one \
                         orders the mail certificate",
                        many.len(),
                        many.join(", ")
                    ))
                }
            };
            api.set_locked(&id, &provider, &host)?;
            Ok(Restore::Switched { provider })
        }
        CertManagement::Other(t) => Err(format!(
            "domain {id} has certificateManagement '@type' '{t}' — not touching it"
        )),
    }
}

/// One reconcile decision (boot): only a K2-issuer box is ever written,
/// and only towards Manual. Returns the log line.
pub fn reconcile(api: &dyn CertMgmtApi, host: &str, owner: CertOwner) -> String {
    const P: &str = "[mail/cert-owner]";
    if owner != CertOwner::K2 {
        return format!(
            "{P} {host}: owner {} — Stalwart's certificate settings left as they are",
            owner.as_str()
        );
    }
    match ensure_manual(api, host) {
        Ok(ManualSwitch::AlreadyManual) => {
            format!("{P} {host}: K2 owns the certificate; Stalwart ACME already Manual")
        }
        Ok(ManualSwitch::Switched) => format!(
            "{P} {host}: K2 owns the certificate; switched Stalwart ACME to Manual \
             (no restart, certificate untouched)"
        ),
        Err(e) => format!("{P} {host}: FAILED to switch Stalwart ACME to Manual (retried at the next renew or boot): {e}"),
    }
}

// ── Routes: GET|POST /cli/mail/cert/owner ───────────────────────────────

fn err_json(status: &'static str, code: &str, hint: String) -> crate::cli_response::CliResponse {
    crate::cli_response::CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } })
            .to_string(),
    }
}

fn mail_hostname() -> Option<String> {
    super::supervisor::row_field("hostname")
        .map(|h| norm(&h))
        .filter(|h| !h.is_empty())
}

fn cert_management_json(cm: &Result<CertManagement, String>) -> serde_json::Value {
    match cm {
        Ok(CertManagement::Manual) => serde_json::json!({ "type": "Manual" }),
        Ok(CertManagement::Automatic {
            subject_alternative_names,
            ..
        }) => serde_json::json!({ "type": "Automatic", "names": subject_alternative_names }),
        Ok(CertManagement::Other(t)) => serde_json::json!({ "type": t }),
        Err(e) => serde_json::json!({ "error": e }),
    }
}

/// GET `/cli/mail/cert/owner` — who owns the mail certificate, plus
/// Stalwart's `certificateManagement` for the mail host when readable.
pub fn handle_get(
    _params: &std::collections::HashMap<String, String>,
) -> crate::cli_response::CliResponse {
    let Some(host) = mail_hostname() else {
        return err_json(
            "409 Conflict",
            "not_ready",
            "the mail server is not installed — no mail certificate to own".into(),
        );
    };
    let owner = mail_cert_owner(&host);
    let stalwart = match crate::mail::domains::engine_from_db() {
        Ok((client, _)) => domain_id_for(&client, &host).and_then(|id| client.get(&id)),
        Err(e) => Err(format!("mail server not reachable: {e}")),
    };
    crate::cli_response::CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "hostname": host,
            "owner": owner.as_str(),
            "attached": mail_host_attached(&host),
            "stalwartAcme": cert_management_json(&stalwart),
            "restoreCommand": RESTORE_COMMAND,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/cert/owner` `{"action":"stalwart-acme"}` — the owner
/// command: hand the mail certificate back to Stalwart's ACME (mail host
/// only). Refused while K2 owns it. Never hostmail disable/enable.
pub fn handle_post(body: &[u8]) -> crate::cli_response::CliResponse {
    let parsed: serde_json::Value = match std::str::from_utf8(body).map(str::trim) {
        Ok("") => serde_json::json!({}),
        Ok(t) => match serde_json::from_str(t) {
            Ok(v) => v,
            Err(e) => {
                return crate::cli_response::CliResponse::bad_request(format!(
                    "invalid JSON body: {e}"
                ))
            }
        },
        Err(_) => return crate::cli_response::CliResponse::bad_request("body is not UTF-8"),
    };
    if parsed["action"].as_str() != Some("stalwart-acme") {
        return err_json(
            "400 Bad Request",
            "usage",
            "action must be 'stalwart-acme' (K2 takes the certificate itself when the mail \
             host is attached and K2 issues it — there is no action for that)"
                .into(),
        );
    }
    let Some(host) = mail_hostname() else {
        return err_json(
            "409 Conflict",
            "not_ready",
            "the mail server is not installed — no mail certificate to hand over".into(),
        );
    };
    let owner = mail_cert_owner(&host);
    if owner == CertOwner::K2 {
        return err_json(
            "409 Conflict",
            "k2_owns",
            restore_stalwart_acme_refusal(&host),
        );
    }
    if let Some(resp) = crate::mail::routes_server::upgrade_running_response() {
        return resp;
    }
    let client = match crate::mail::domains::engine_from_db() {
        Ok((c, _)) => c,
        Err(e) => {
            return err_json(
                "409 Conflict",
                "not_ready",
                format!("the mail server is not reachable: {e}"),
            )
        }
    };
    if !super::supervisor::try_begin_enable() {
        return err_json(
            "409 Conflict",
            "busy",
            "an enable (or a boot reconcile) holds the mail server — try again in a minute".into(),
        );
    }
    let result = restore_stalwart_acme(&client, &host, owner);
    super::supervisor::end_enable();
    match result {
        Ok(r) => {
            let (result, hint) = match &r {
                Restore::AlreadyStalwart => (
                    "unchanged",
                    format!("Stalwart ACME already orders {host} only — nothing changed"),
                ),
                Restore::Locked => (
                    "locked",
                    format!("Stalwart ACME now orders {host} only"),
                ),
                Restore::Switched { .. } => (
                    "switched",
                    format!(
                        "Stalwart ACME is on again for {host} (mail host only); Stalwart \
                         queued one ACME order for it"
                    ),
                ),
            };
            crate::cli_response::CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "hostname": host,
                    "owner": "stalwart-acme",
                    "result": result,
                    "hint": hint,
                })
                .to_string(),
            )
        }
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

fn restore_stalwart_acme_refusal(host: &str) -> String {
    format!(
        "K2 owns the certificate for {host} (attached in `k2 domain` with a K2-planted \
         certificate) and renews it. Stalwart ACME stays off. To hand it back, first \
         `k2 domain name remove {host}`, then run this again"
    )
}

// ── Hooks ───────────────────────────────────────────────────────────────

/// After K2's issuer planted AND loaded the mail certificate for an
/// attached `role mail` host — by construction a K2-issuer box: switch
/// Stalwart ACME to Manual. Never fails the plant (the certificate is
/// already serving); a failed write is logged and retried at the next
/// renew or boot, and the doctor warns meanwhile.
pub fn after_k2_plant(host: &str) {
    #[cfg(test)]
    {
        TEST_AFTER_PLANT.with(|c| c.borrow_mut().push(norm(host)));
    }
    #[cfg(not(test))]
    {
        let line = match crate::mail::domains::engine_from_db() {
            Ok((client, _)) => reconcile(&client, host, CertOwner::K2),
            Err(e) => format!(
                "[mail/cert-owner] {host}: FAILED to switch Stalwart ACME to Manual \
                 (no management client: {e}) — retried at the next renew or boot"
            ),
        };
        k2_core::log_debug!("{line}");
    }
}

#[cfg(test)]
thread_local! {
    static TEST_AFTER_PLANT: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Test seam: hosts [`after_k2_plant`] ran for (drains).
#[cfg(test)]
pub(crate) fn take_test_after_plant() -> Vec<String> {
    TEST_AFTER_PLANT.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// Boot hook (main.rs, next to the IMAP/DAV reconciles). Linux only; one
/// detached thread; panics contained; never blocks boot.
pub fn spawn_startup_reconcile() {
    if !super::supervisor::mail_supported() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mail-cert-owner".into())
        .spawn(|| {
            if std::panic::catch_unwind(run_startup_reconcile_live).is_err() {
                k2_core::log_debug!("[mail/cert-owner] FAILED (daemon carries on): panicked");
            }
        });
    if let Err(e) = spawned {
        k2_core::log_debug!("[mail/cert-owner] FAILED (daemon carries on): thread spawn: {e}");
    }
}

const API_WAIT_STEP_SECS: u64 = 3;
/// ~10 min: covers the IMAP reconcile's Stalwart restart.
const API_WAIT_TRIES: u32 = 200;

fn run_startup_reconcile_live() {
    use super::supervisor;
    use std::sync::atomic::Ordering;
    const P: &str = "[mail/cert-owner]";

    let status = supervisor::current_status();
    if let Some(why) =
        super::imap_listeners::startup_gate(status.as_deref(), supervisor::enable_completed())
    {
        k2_core::log_debug!("{P} skipped because {why}");
        return;
    }
    let Some(host) = supervisor::row_field("hostname").filter(|h| !h.trim().is_empty()) else {
        k2_core::log_debug!("{P} skipped because the mail server has no hostname");
        return;
    };
    // Only a K2-issuer box is ever written: decide before touching Stalwart.
    if mail_cert_owner(&host) != CertOwner::K2 {
        k2_core::log_debug!("{}", reconcile_skip_line(&host));
        return;
    }
    let client = match supervisor::mgmt_client_from_row() {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("{P} skipped because no management client: {e}");
            return;
        }
    };
    // Same pattern as the DAV backfill: wait for the admin API, then the
    // enable lock (the IMAP reconcile may hold it across a restart).
    let mut up = false;
    let mut locked = false;
    let mut last_err = String::from("enable lock held");
    for _ in 0..API_WAIT_TRIES {
        if !supervisor::enable_running().load(Ordering::SeqCst) {
            if !up {
                match client.ping() {
                    Ok(()) => up = true,
                    Err(e) => last_err = e,
                }
            }
            if up && supervisor::try_begin_enable() {
                locked = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(API_WAIT_STEP_SECS));
    }
    if !locked {
        k2_core::log_debug!("{P} skipped (retried at the next renew or boot): {last_err}");
        return;
    }
    struct EndEnable;
    impl Drop for EndEnable {
        fn drop(&mut self) {
            super::supervisor::end_enable();
        }
    }
    let _guard = EndEnable;
    // Re-decide under the lock (a detach may have run meanwhile).
    let owner = mail_cert_owner(&host);
    k2_core::log_debug!("{}", reconcile(&client, &host, owner));
}

fn reconcile_skip_line(host: &str) -> String {
    format!(
        "[mail/cert-owner] {host}: owner {} — Stalwart's certificate settings left as they are",
        mail_cert_owner(host).as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn le() -> LeafInfo {
        LeafInfo { not_after: 2_000_000_000, issuer: "CN=R11, O=Let's Encrypt".into(), lets_encrypt: true }
    }

    fn planted() -> RenewRecord {
        RenewRecord {
            source: Some("k2".into()),
            stalwart_cert_id: Some("cert-k2".into()),
            ..Default::default()
        }
    }

    #[test]
    fn k2_issuer_needs_attached_mail_host_and_a_k2_planted_cert() {
        assert_eq!(classify(true, Some(&le()), Some(&planted())), CertOwner::K2);
        // Not attached: never K2, whatever the store holds.
        assert_eq!(classify(false, Some(&le()), Some(&planted())), CertOwner::StalwartAcme);
        // Attached but never planted into Stalwart.
        let unplanted = RenewRecord { source: Some("k2".into()), ..Default::default() };
        assert_eq!(classify(true, Some(&le()), Some(&unplanted)), CertOwner::Unknown);
        assert_eq!(classify(true, Some(&le()), None), CertOwner::Unknown);
        // Uploaded by hand: not K2's.
        let uploaded = RenewRecord { source: Some("uploaded".into()), ..planted() };
        assert_eq!(classify(true, Some(&le()), Some(&uploaded)), CertOwner::Unknown);
        // A non-Let's-Encrypt (self-signed) leaf is never K2's.
        let rcgen = LeafInfo { lets_encrypt: false, issuer: "CN=rcgen self signed".into(), ..le() };
        assert_eq!(classify(true, Some(&rcgen), Some(&planted())), CertOwner::Unknown);
        assert_eq!(classify(true, None, Some(&planted())), CertOwner::Unknown);
        assert_eq!(CertOwner::K2.as_str(), "k2");
        assert_eq!(CertOwner::StalwartAcme.as_str(), "stalwart-acme");
        assert_eq!(CertOwner::Unknown.as_str(), "unknown");
    }

    #[test]
    fn live_owner_reads_inventory_store_and_renewal_file() {
        let _home = crate::test_support::TempHome::new();
        let _ = k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding(&conn, "owner-live.test", None, false).unwrap();
            k2_core::domains::upsert_name(
                &conn,
                "mail.owner-live.test",
                "owner-live.test",
                k2_core::domains::ROLE_MAIL,
            )
            .unwrap();
        }
        assert_eq!(mail_cert_owner("mail.owner-live.test"), CertOwner::Unknown, "no cert yet");
        let pem = crate::domains::acme::test_le_pem("mail.owner-live.test");
        crate::domains::store::install("mail.owner-live.test", &pem.chain_pem, &pem.key_pem)
            .expect("install");
        renew::update(|f| {
            f.names.entry("mail.owner-live.test".into()).or_default().stalwart_cert_id =
                Some("cert-1".into())
        })
        .expect("renewal.json");
        assert_eq!(mail_cert_owner("Mail.Owner-Live.test."), CertOwner::K2);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = k2_core::domains::remove_binding(&conn, "owner-live.test");
        }
        assert_eq!(
            mail_cert_owner("mail.owner-live.test"),
            CertOwner::StalwartAcme,
            "detached = no longer K2-issuer"
        );
    }

    /// Recording fake registry.
    struct Fake {
        cm: RefCell<Result<CertManagement, String>>,
        providers: Vec<String>,
        set_err: Option<String>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(cm: CertManagement) -> Self {
            Self { cm: RefCell::new(Ok(cm)), providers: vec!["acme-p1".into()], set_err: None, calls: RefCell::new(vec![]) }
        }
        fn writes(&self) -> Vec<String> {
            self.calls.borrow().iter().filter(|c| c.starts_with("set")).cloned().collect()
        }
    }

    impl CertMgmtApi for Fake {
        fn mail_domain_id(&self, host: &str) -> Result<Option<String>, String> {
            self.calls.borrow_mut().push(format!("id {host}"));
            Ok(Some("dom-1".into()))
        }
        fn get(&self, _id: &str) -> Result<CertManagement, String> {
            self.calls.borrow_mut().push("get".into());
            self.cm.borrow().clone()
        }
        fn set_manual(&self, id: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("set-manual {id}"));
            if let Some(e) = &self.set_err {
                return Err(e.clone());
            }
            *self.cm.borrow_mut() = Ok(CertManagement::Manual);
            Ok(())
        }
        fn set_locked(&self, id: &str, provider: &str, host: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("set-locked {id} {provider} {host}"));
            *self.cm.borrow_mut() = Ok(CertManagement::Automatic {
                acme_provider_id: provider.into(),
                subject_alternative_names: vec![host.into()],
            });
            Ok(())
        }
        fn acme_providers(&self) -> Result<Vec<String>, String> {
            self.calls.borrow_mut().push("providers".into());
            Ok(self.providers.clone())
        }
    }

    fn auto(sans: &[&str]) -> CertManagement {
        CertManagement::Automatic {
            acme_provider_id: "acme-p1".into(),
            subject_alternative_names: sans.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn manual_is_written_once_and_is_idempotent() {
        let api = Fake::new(auto(&["mail.example.com"]));
        assert_eq!(ensure_manual(&api, "Mail.Example.com.").expect("switch"), ManualSwitch::Switched);
        assert_eq!(api.writes(), vec!["set-manual dom-1"]);
        assert_eq!(ensure_manual(&api, "mail.example.com").expect("again"), ManualSwitch::AlreadyManual);
        assert_eq!(api.writes(), vec!["set-manual dom-1"], "no second write");
        // Unknown @type: never touched.
        let api = Fake::new(CertManagement::Other("Future".into()));
        assert!(ensure_manual(&api, "mail.example.com").is_err());
        assert!(api.writes().is_empty());
    }

    /// The boot reconcile writes only on a K2-issuer box and only towards
    /// Manual; a detached box that is Manual stays Manual (no flip back).
    #[test]
    fn reconcile_only_switches_k2_boxes_and_never_flips_back() {
        let api = Fake::new(auto(&[]));
        let line = reconcile(&api, "mail.example.com", CertOwner::K2);
        assert!(line.contains("switched Stalwart ACME to Manual"), "{line}");
        assert_eq!(api.writes(), vec!["set-manual dom-1"]);
        let line = reconcile(&api, "mail.example.com", CertOwner::K2);
        assert!(line.contains("already Manual"), "{line}");
        assert_eq!(api.writes().len(), 1, "idempotent");

        for owner in [CertOwner::StalwartAcme, CertOwner::Unknown] {
            let detached = Fake::new(CertManagement::Manual);
            let line = reconcile(&detached, "mail.example.com", owner);
            assert!(line.contains("left as they are"), "{line}");
            assert!(detached.calls.borrow().is_empty(), "not even a read: {:?}", detached.calls.borrow());
            let tls_alpn = Fake::new(auto(&["mail.example.com"]));
            reconcile(&tls_alpn, "mail.example.com", owner);
            assert!(tls_alpn.writes().is_empty(), "non-K2 tls-alpn box unchanged");
        }

        let failing = Fake { set_err: Some("forbidden".into()), ..Fake::new(auto(&[])) };
        let line = reconcile(&failing, "mail.example.com", CertOwner::K2);
        assert!(line.contains("FAILED") && line.contains("forbidden"), "{line}");
    }

    #[test]
    fn restore_is_refused_while_k2_owns_and_locks_to_the_mail_host() {
        let api = Fake::new(CertManagement::Manual);
        let err = restore_stalwart_acme(&api, "mail.example.com", CertOwner::K2).expect_err("K2 owns");
        assert!(err.contains("k2 domain name remove mail.example.com"), "{err}");
        assert!(api.calls.borrow().is_empty());

        // Detached while Manual: Automatic with the one provider, mail host only.
        let api = Fake::new(CertManagement::Manual);
        assert_eq!(
            restore_stalwart_acme(&api, "mail.example.com", CertOwner::StalwartAcme).expect("restore"),
            Restore::Switched { provider: "acme-p1".into() }
        );
        assert_eq!(api.writes(), vec!["set-locked dom-1 acme-p1 mail.example.com"]);
        assert_eq!(
            restore_stalwart_acme(&api, "mail.example.com", CertOwner::StalwartAcme).expect("again"),
            Restore::AlreadyStalwart
        );
        assert_eq!(api.writes().len(), 1);

        // Automatic with the wide default list: locked.
        let api = Fake::new(auto(&[]));
        assert_eq!(
            restore_stalwart_acme(&api, "mail.example.com", CertOwner::Unknown).expect("lock"),
            Restore::Locked
        );
        assert_eq!(api.writes(), vec!["set-locked dom-1 acme-p1 mail.example.com"]);

        // No / several providers: refused, nothing written.
        let none = Fake { providers: vec![], ..Fake::new(CertManagement::Manual) };
        assert!(restore_stalwart_acme(&none, "mail.example.com", CertOwner::StalwartAcme)
            .expect_err("none")
            .contains("no ACME provider"));
        assert!(none.writes().is_empty());
        let many = Fake { providers: vec!["a".into(), "b".into()], ..Fake::new(CertManagement::Manual) };
        assert!(restore_stalwart_acme(&many, "mail.example.com", CertOwner::StalwartAcme)
            .expect_err("many")
            .contains("2 ACME providers"));
        assert!(many.writes().is_empty());
    }
}
