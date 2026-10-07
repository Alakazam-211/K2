//! On-box ACME for custom-domain hostnames (C6/C17/C18).
//!
//! Proof order: DNS-01 if `dns_write`; else HTTP-01 on :80 **unless the
//! hostname is a CNAME** (LE follows it; `mail` → `mail.lztek.io:80` is
//! often closed / tls-alpn); else TLS-ALPN if this box can bind :443.
//! DNS-01 plants `_acme-challenge.<host>` TXT (a different owner name
//! than the `mail` CNAME). Never cert.k2.dev. Never silent rcgen.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use instant_acme::{
    Account, ChallengeType, Identifier, NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use k2_core::domains::{get_name, hostname_under_apex, DomainBinding, DomainName};
use rcgen::{CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

use crate::dns::proxy::proxy_request;
use crate::domains::renew;
use crate::domains::store::{self, AcmeConfig, InstalledPem};

const PROD_DIR: &str = "https://acme-v02.api.letsencrypt.org/directory";
const STAGING_DIR: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    Dns01,
    Http01,
    TlsAlpn01,
}

pub fn acme_txt_relative_name(hostname: &str, apex: &str) -> String {
    if hostname == apex {
        "_acme-challenge".into()
    } else if let Some(rest) = hostname.strip_suffix(&format!(".{apex}")) {
        format!("_acme-challenge.{rest}")
    } else {
        "_acme-challenge".into()
    }
}

fn dns_wait_secs() -> u64 {
    std::env::var("K2_ACME_DNS_WAIT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90)
}

fn acme_directory(cfg: &AcmeConfig) -> String {
    if let Ok(v) = std::env::var("K2_ACME_DIRECTORY") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    if std::env::var("K2_ACME_STAGING")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return STAGING_DIR.to_string();
    }
    if let Some(d) = cfg.directory.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return d.to_string();
    }
    // Tests default to staging so a mistaken live run cannot burn prod.
    if cfg!(test) {
        STAGING_DIR.to_string()
    } else {
        PROD_DIR.to_string()
    }
}

fn reject_k2_dev(hostname: &str) -> Result<(), String> {
    if hostname == "k2.dev" || hostname.ends_with(".k2.dev") {
        return Err(
            "Connect {label}.k2.dev names stay on cert.k2.dev — do not issue them here".into(),
        );
    }
    Ok(())
}

pub fn select_challenge(binding: &DomainBinding, hostname: &str) -> Result<ChallengeKind, String> {
    if binding.dns_write {
        if crate::dns::proxy::tunnel_bearer_token().is_err() {
            return Err(
                "DNS-01 needs a Connect tunnel token to plant TXT via k2 dns — \
pair K2 Connect, or point the name here and use HTTP-01/:80 or TLS-ALPN/:443"
                    .into(),
            );
        }
        return Ok(ChallengeKind::Dns01);
    }
    if hostname_is_cname(hostname) {
        return Err(format!(
            "{hostname} is a CNAME — HTTP-01 follows it (often to a host with :80 closed). \
Attach this apex with dns_write so we DNS-01 _acme-challenge (the CNAME at the mail \
label is fine), or point an A at this box"
        ));
    }
    if std::env::var("K2_ACME_HTTP01")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
        || port_seems_free(80)
    {
        return Ok(ChallengeKind::Http01);
    }
    if port_seems_free(443) {
        return Ok(ChallengeKind::TlsAlpn01);
    }
    Err(
        "no ACME proof available: zone is not dns_write (DNS-01), :80 is closed (HTTP-01), \
and :443 is not free (TLS-ALPN). Attach a K2-hosted zone or open :80"
            .into(),
    )
}

/// True if `hostname` itself is a CNAME. `_acme-challenge.<host>` is a
/// different owner name — DNS-01 still works while `mail` is a CNAME.
fn hostname_is_cname(hostname: &str) -> bool {
    if std::env::var("K2_ACME_HOSTNAME_IS_CNAME")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return true;
    }
    if cfg!(test) {
        return false;
    }
    lookup_cname(hostname)
}

fn lookup_cname(hostname: &str) -> bool {
    use hickory_resolver::config::{ResolverConfig, ResolverOpts};
    use hickory_resolver::proto::rr::RecordType;
    let mut config = ResolverConfig::google();
    for ns in ResolverConfig::cloudflare().name_servers() {
        config.add_name_server(ns.clone());
    }
    let Ok(resolver) = hickory_resolver::Resolver::new(config, ResolverOpts::default()) else {
        return false;
    };
    let q = if hostname.ends_with('.') {
        hostname.to_string()
    } else {
        format!("{hostname}.")
    };
    match resolver.lookup(q, RecordType::CNAME) {
        Ok(lookup) => lookup.iter().next().is_some(),
        Err(_) => false,
    }
}

fn port_seems_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Manual issue / renew (`k2 cert issue|renew`, `k2 hostmail cert renew`).
/// Recorded in `renewal.json` like a background attempt.
pub fn issue_attached(hostname: &str) -> Result<InstalledPem, String> {
    let result = issue_attached_core(hostname);
    renew::record_manual(hostname, chrono::Utc::now().timestamp(), &result);
    result.map(|i| i.pem).map_err(|e| e.to_string())
}

/// The one issuer path for an attached name (manual and background).
/// Holds the name's [`renew::NameLock`] for the whole run. The box
/// store's certificate is reused (re-planted for mail) only while more
/// than [`renew::RENEW_WINDOW_SECS`] remain; inside the window a new
/// certificate is ordered.
pub(crate) fn issue_attached_core(hostname: &str) -> Result<renew::Issued, renew::IssueError> {
    use renew::IssueError;
    reject_k2_dev(hostname).map_err(IssueError::NotQualified)?;
    let (binding, name) = lookup_attached(hostname).map_err(IssueError::NotQualified)?;
    let _lock = renew::NameLock::try_acquire(hostname)
        .ok_or_else(|| IssueError::Busy(hostname.to_string()))?;
    if let Some(pem) = reusable_inventory(hostname, chrono::Utc::now().timestamp()) {
        plant_mail_if_needed(hostname, &name, &pem).map_err(IssueError::Plant)?;
        return Ok(renew::Issued { pem, ordered: false });
    }
    let kind = select_challenge(&binding, hostname).map_err(IssueError::Order)?;
    issue_with_challenge(hostname, &binding, &name, kind)
}

/// CAL44: a certificate for exactly ONE extra mail-family name
/// (autoconfig / autodiscover / mta-sts / ua-auto-config under a hosted
/// apex), through DNS-01 on a K2-hosted zone only. The name need not be
/// attached in `k2 domain`; the apex's binding supplies the zone.
///
/// Writes the PEMs to the box store (`~/.k2/certs/<name>/`) and NEVER
/// plants: the caller adds it to Stalwart as a non-default certificate.
/// Never Stalwart's ACME, never a SAN on the mail cert, never HTTP-01 /
/// TLS-ALPN (the box's :443 is Stalwart's or Caddy's).
pub fn issue_extra_name(hostname: &str, binding: &DomainBinding) -> Result<InstalledPem, String> {
    check_extra_name_issuable(hostname, binding)?;
    let kind = select_challenge(binding, hostname)?;
    if kind != ChallengeKind::Dns01 {
        return Err(format!(
            "{hostname}: per-name certificates use DNS-01 only (got {kind:?})"
        ));
    }
    let pem = if fake_enabled() {
        fake_issue(hostname, binding, kind)?
    } else {
        live_issue(hostname, binding, kind)?
    };
    store::install(hostname, &pem.chain_pem, &pem.key_pem)?;
    Ok(pem)
}

/// The refusals [`issue_extra_name`] makes before any network call.
pub(crate) fn check_extra_name_issuable(
    hostname: &str,
    binding: &DomainBinding,
) -> Result<(), String> {
    reject_k2_dev(hostname)?;
    if hostname.eq_ignore_ascii_case(&binding.apex) || !hostname_under_apex(hostname, &binding.apex)
    {
        return Err(format!(
            "{hostname} is not a name under the apex {} — per-name certificates cover \
             one sub-name each, never the apex",
            binding.apex
        ));
    }
    if !binding.dns_write {
        return Err(format!(
            "zone {} is not K2-hosted (not attached with DNS write) — K2 issues per-name \
             certificates through DNS-01 on ns1/ns2.k2.dev only",
            binding.apex
        ));
    }
    if binding.is_pending_ns() {
        return Err(format!(
            "zone {} is not delegated to ns1/ns2.k2.dev yet (pending_ns) — a DNS-01 \
             record planted there is not visible to the CA",
            binding.apex
        ));
    }
    Ok(())
}

/// The box store's certificate when it is a live Let's Encrypt leaf for
/// `hostname` with at least [`renew::RENEW_WINDOW_SECS`] left.
fn reusable_inventory(hostname: &str, now: i64) -> Option<InstalledPem> {
    let installed = store::load(hostname)?;
    if !crate::domains::status::is_reusable_lets_encrypt(hostname, &installed.chain_pem) {
        return None;
    }
    let leaf = crate::domains::status::pem_leaf_info(&installed.chain_pem)?;
    if renew::needs_renewal(leaf.not_after, now) {
        return None;
    }
    Some(installed)
}

fn plant_mail_if_needed(
    hostname: &str,
    name: &DomainName,
    pem: &InstalledPem,
) -> Result<(), String> {
    if name.role != k2_core::domains::ROLE_MAIL {
        return Ok(());
    }
    store::plant_mail_pem(hostname, &pem.chain_pem, &pem.key_pem)?;
    crate::domains::status::reject_live_self_signed(hostname)?;
    // K2's issuer just planted and loaded the certificate of an attached
    // role-mail host: this is a K2-issuer box. Stalwart ACME → Manual
    // (never fails the plant; retried at the next renew / boot).
    crate::mail::cert_owner::after_k2_plant(hostname);
    Ok(())
}

/// Test helper (other modules): a Let's-Encrypt-like PEM for `hostname`
/// (90 days), as the box store would hold one from K2's issuer.
#[cfg(test)]
pub(crate) fn test_le_pem(hostname: &str) -> InstalledPem {
    let mut ca_params =
        CertificateParams::new(vec!["Let's Encrypt R11".to_string()]).expect("ca params");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::OrganizationName, "Let's Encrypt");
    ca_dn.push(DnType::CommonName, "R11");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().expect("ca key");
    let ca = ca_params.self_signed(&ca_key).expect("ca");
    let mut leaf = CertificateParams::new(vec![hostname.to_string()]).expect("leaf params");
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname.to_string());
    leaf.distinguished_name = dn;
    let leaf_key = KeyPair::generate().expect("leaf key");
    let cert = leaf.signed_by(&leaf_key, &ca, &ca_key).expect("sign");
    InstalledPem {
        hostname: hostname.to_string(),
        chain_pem: format!("{}{}", cert.pem(), ca.pem()),
        key_pem: leaf_key.serialize_pem(),
    }
}

fn lookup_attached(hostname: &str) -> Result<(DomainBinding, DomainName), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let name = get_name(&conn, hostname)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("hostname '{hostname}' is not attached"))?;
    let binding = k2_core::domains::get_binding(&conn, &name.apex)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("apex '{}' is not attached", name.apex))?;
    if !hostname_under_apex(hostname, &binding.apex) {
        return Err(format!(
            "hostname '{hostname}' is not under apex {}",
            binding.apex
        ));
    }
    Ok((binding, name))
}

fn issue_with_challenge(
    hostname: &str,
    binding: &DomainBinding,
    name: &DomainName,
    kind: ChallengeKind,
) -> Result<renew::Issued, renew::IssueError> {
    use renew::IssueError;
    let pem = if fake_enabled() {
        fake_issue(hostname, binding, kind)
    } else {
        live_issue(hostname, binding, kind)
    }
    .map_err(IssueError::Order)?;
    store::install(hostname, &pem.chain_pem, &pem.key_pem).map_err(IssueError::Order)?;
    plant_mail_if_needed(hostname, name, &pem).map_err(IssueError::Plant)?;
    Ok(renew::Issued { pem, ordered: true })
}

fn fake_enabled() -> bool {
    std::env::var("K2_ACME_FAKE")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
        || cfg!(test)
}

fn fake_issue(
    hostname: &str,
    binding: &DomainBinding,
    kind: ChallengeKind,
) -> Result<InstalledPem, String> {
    if kind == ChallengeKind::Dns01 {
        let rec = plant_txt(binding, hostname, "fake-dns01-token")?;
        std::thread::sleep(Duration::from_secs(dns_wait_secs().min(1)));
        let _ = delete_txt(&rec);
    }
    mint_fake_le(hostname)
}

fn mint_fake_le(hostname: &str) -> Result<InstalledPem, String> {
    let mut ca_params =
        CertificateParams::new(vec!["Let's Encrypt Fake CA".to_string()]).map_err(|e| e.to_string())?;
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "Let's Encrypt Fake");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().map_err(|e| e.to_string())?;
    let ca = ca_params.self_signed(&ca_key).map_err(|e| e.to_string())?;

    let mut leaf = CertificateParams::new(vec![hostname.to_string()]).map_err(|e| e.to_string())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname.to_string());
    leaf.distinguished_name = dn;
    let leaf_key = KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = leaf
        .signed_by(&leaf_key, &ca, &ca_key)
        .map_err(|e| e.to_string())?;
    Ok(InstalledPem {
        hostname: hostname.to_string(),
        chain_pem: format!("{}{}", cert.pem(), ca.pem()),
        key_pem: leaf_key.serialize_pem(),
    })
}

fn live_issue(
    hostname: &str,
    binding: &DomainBinding,
    kind: ChallengeKind,
) -> Result<InstalledPem, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("acme runtime: {e}"))?;
    rt.block_on(live_issue_async(hostname, binding, kind))
}

async fn live_issue_async(
    hostname: &str,
    binding: &DomainBinding,
    kind: ChallengeKind,
) -> Result<InstalledPem, String> {
    let cfg = store::load_acme_config();
    let directory = acme_directory(&cfg);
    let contacts: Vec<String> = cfg
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|e| {
            if e.starts_with("mailto:") {
                e.to_string()
            } else {
                format!("mailto:{e}")
            }
        })
        .into_iter()
        .collect();
    let contact_refs: Vec<&str> = contacts.iter().map(|s| s.as_str()).collect();

    let (account, creds) = if let Ok(raw) = std::fs::read_to_string(store::acme_account_path()) {
        let creds: instant_acme::AccountCredentials =
            serde_json::from_str(&raw).map_err(|e| format!("acme account: {e}"))?;
        let account = Account::builder()
            .map_err(|e| format!("acme account builder: {e}"))?
            .from_credentials(creds)
            .await
            .map_err(|e| format!("acme restore account: {e}"))?;
        (account, None)
    } else {
        Account::builder()
            .map_err(|e| format!("acme account builder: {e}"))?
            .create(
                &NewAccount {
                    contact: &contact_refs,
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                directory.clone(),
                None,
            )
            .await
            .map(|(a, c)| (a, Some(c)))
            .map_err(|e| format!("acme create account: {e}"))?
    };
    if let Some(creds) = creds {
        if let Ok(body) = serde_json::to_string_pretty(&creds) {
            let _ = store::save_acme_config(&cfg);
            let dir = store::certs_root();
            let _ = std::fs::create_dir_all(&dir);
            let path = store::acme_account_path();
            let _ = std::fs::write(&path, body);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    let identifiers = vec![Identifier::Dns(hostname.to_string())];
    let mut order = account
        .new_order(&NewOrder::new(&identifiers))
        .await
        .map_err(|e| format!("acme new order: {e}"))?;

    let challenge_ty = match kind {
        ChallengeKind::Dns01 => ChallengeType::Dns01,
        ChallengeKind::Http01 => ChallengeType::Http01,
        ChallengeKind::TlsAlpn01 => ChallengeType::TlsAlpn01,
    };

    let mut planted: Option<String> = None;
    let mut http_guard: Option<Http01Guard> = None;
    let mut authorizations = order.authorizations();
    while let Some(result) = authorizations.next().await {
        let mut authz = result.map_err(|e| format!("acme authz: {e}"))?;
        match authz.status {
            instant_acme::AuthorizationStatus::Pending => {}
            instant_acme::AuthorizationStatus::Valid => continue,
            other => return Err(format!("acme authorization {other:?}")),
        }
        let mut challenge = authz
            .challenge(challenge_ty.clone())
            .ok_or_else(|| format!("ACME server offered no {kind:?} challenge"))?;
        match kind {
            ChallengeKind::Dns01 => {
                let txt = challenge.key_authorization().dns_value();
                purge_existing_acme_txt(binding, hostname);
                let rec = plant_txt(binding, hostname, &txt)?;
                planted = Some(rec.clone());
                // Hickory's default Resolver starts a tokio runtime.
                // live_issue already block_on's one — nested Runtime::new
                // panics "Cannot start a runtime from within a runtime"
                // (lztek scratch-le1). Wait on a blocking thread.
                let binding_c = binding.clone();
                let hostname_c = hostname.to_string();
                let txt_c = txt.clone();
                let vis = tokio::task::spawn_blocking(move || {
                    wait_acme_txt_visible(&binding_c, &hostname_c, &txt_c)
                })
                .await
                .map_err(|e| format!("txt wait worker: {e}"))?;
                if let Err(e) = vis {
                    let _ = delete_txt(&rec);
                    return Err(e);
                }
            }
            ChallengeKind::Http01 => {
                let token = challenge.token.clone();
                let body = challenge.key_authorization().as_str().to_string();
                http_guard = Some(Http01Guard::spawn(&token, &body)?);
            }
            ChallengeKind::TlsAlpn01 => {
                return Err(
                    "TLS-ALPN-01 needs this box to own :443 for the ACME handshake — \
use DNS-01 (attach a K2-hosted zone) or HTTP-01 on :80"
                        .into(),
                );
            }
        }
        challenge
            .set_ready()
            .await
            .map_err(|e| format!("acme set_ready: {e}"))?;
    }
    drop(authorizations);

    let status = order
        .poll_ready(&RetryPolicy::default())
        .await
        .map_err(|e| format!("acme poll: {e}"))?;
    if let Some(id) = planted.take() {
        let _ = delete_txt(&id);
    }
    drop(http_guard);

    let mut params =
        CertificateParams::new(vec![hostname.to_string()]).map_err(|e| e.to_string())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname.to_string());
    params.distinguished_name = dn;
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let csr = params
        .serialize_request(&key)
        .map_err(|e| format!("csr: {e}"))?;

    // Ready → finalize. Valid → cert already issued (scratch-le3: finalize
    // returned orderNotReady status=valid). Invalid stays an error.
    match status {
        OrderStatus::Ready => {
            if let Err(e) = order.finalize_csr(csr.der()).await {
                let msg = e.to_string().to_ascii_lowercase();
                if !(msg.contains("valid") && msg.contains("notready")
                    || msg.contains("order's status (\"valid\")")
                    || msg.contains("status (\"valid\")"))
                {
                    return Err(format!("acme finalize: {e}"));
                }
            }
        }
        OrderStatus::Valid => {}
        other => return Err(format!("acme order not ready: {other:?}")),
    }
    let chain = order
        .poll_certificate(&RetryPolicy::default())
        .await
        .map_err(|e| format!("acme certificate: {e}"))?;
    Ok(InstalledPem {
        hostname: hostname.to_string(),
        chain_pem: chain,
        key_pem: key.serialize_pem(),
    })
}

fn acme_txt_fqdn(hostname: &str, apex: &str) -> String {
    let rel = acme_txt_relative_name(hostname, apex);
    if hostname == apex {
        format!("_acme-challenge.{apex}")
    } else {
        format!("{rel}.{apex}")
    }
}

/// Wait on **authoritative** ns1+ns2 first. Querying 8.8.8.8 before ns2
/// has the name caches NXDOMAIN for SOA minimum (3600s) — scratch-le2
/// Google served ns2 SOA 2026091922. Dual-syncd CAS can skip ns2.
fn wait_acme_txt_visible(binding: &DomainBinding, hostname: &str, value: &str) -> Result<(), String> {
    let fqdn = acme_txt_fqdn(hostname, &binding.apex);
    let budget = dns_wait_secs().max(90);
    let deadline = std::time::Instant::now() + Duration::from_secs(budget);
    while std::time::Instant::now() < deadline {
        let n1 = txt_has_at_ns("ns1.k2.dev", &fqdn, value);
        let n2 = txt_has_at_ns("ns2.k2.dev", &fqdn, value);
        if n1 && n2 {
            let g = txt_has_google(&fqdn, value);
            let c = txt_has_cloudflare(&fqdn, value);
            if g && c {
                std::thread::sleep(Duration::from_secs(15));
                if txt_has_google(&fqdn, value) && txt_has_cloudflare(&fqdn, value) {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    Err(format!(
        "planted _acme-challenge TXT but ns1.k2.dev and ns2.k2.dev do not both have {fqdn} \
(then 8.8.8.8+1.1.1.1) after {budget}s (zone {}). Dual-syncd lag — do not query public DNS \
until both NS have it (NXDOMAIN cache). Fresh hostname. Do not retry HTTP-01",
        binding.zone_id.as_deref().unwrap_or("?")
    ))
}

fn txt_matches(recs: &[Vec<String>], want: &str) -> bool {
    recs.iter().any(|chunks| {
        let s = chunks.concat();
        s == want || s.trim_matches('"') == want
    })
}

fn txt_has_google(fqdn: &str, want: &str) -> bool {
    use crate::mail::dns_verify::{DnsResolver, SystemResolver};
    let Ok(r) = SystemResolver::google_nocache() else {
        return false;
    };
    r.txt(fqdn).ok().is_some_and(|recs| txt_matches(&recs, want))
}

fn txt_has_cloudflare(fqdn: &str, want: &str) -> bool {
    use crate::mail::dns_verify::{DnsResolver, SystemResolver};
    let Ok(r) = SystemResolver::cloudflare_nocache() else {
        return false;
    };
    r.txt(fqdn).ok().is_some_and(|recs| txt_matches(&recs, want))
}

fn txt_has_at_ns(ns_host: &str, fqdn: &str, want: &str) -> bool {
    use crate::mail::dns_verify::{DnsResolver, SystemResolver};
    use hickory_resolver::config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts};
    use std::net::SocketAddr;
    let Ok(boot) = SystemResolver::google_nocache() else {
        return false;
    };
    let Ok(ips) = boot.a(ns_host) else {
        return false;
    };
    if ips.is_empty() {
        return false;
    }
    let mut config = ResolverConfig::new();
    for ip in ips {
        config.add_name_server(NameServerConfig::new(
            SocketAddr::from((ip, 53)),
            Protocol::Udp,
        ));
    }
    let mut opts = ResolverOpts::default();
    opts.cache_size = 0;
    let Ok(resolver) = hickory_resolver::Resolver::new(config, opts) else {
        return false;
    };
    match resolver.txt_lookup(if fqdn.ends_with('.') {
        fqdn.to_string()
    } else {
        format!("{fqdn}.")
    }) {
        Ok(lookup) => lookup.iter().any(|rdata| {
            let s = rdata.txt_data().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect::<Vec<_>>().concat();
            s == want || s.trim_matches('"') == want
        }),
        Err(_) => false,
    }
}

fn purge_existing_acme_txt(binding: &DomainBinding, hostname: &str) {
    let Some(zone_id) = binding.zone_id.as_deref().filter(|s| !s.is_empty()) else {
        return;
    };
    let rel = acme_txt_relative_name(hostname, &binding.apex);
    let path = format!("/api/dns/zones/{zone_id}");
    let Ok(resp) = proxy_request("GET", &path, Some("k2-acme"), None) else {
        return;
    };
    for id in acme_txt_record_ids(&resp.body, &rel) {
        let _ = delete_txt(&id);
    }
}

fn acme_txt_record_ids(body: &str, rel: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let mut ids = Vec::new();
    let mut arrays: Vec<&Vec<serde_json::Value>> = Vec::new();
    if let Some(a) = v.get("records").and_then(|x| x.as_array()) {
        arrays.push(a);
    }
    if let Some(a) = v.pointer("/zone/records").and_then(|x| x.as_array()) {
        arrays.push(a);
    }
    if let Some(a) = v.as_array() {
        arrays.push(a);
    }
    let rel_l = rel.to_ascii_lowercase();
    for arr in arrays {
        for rec in arr {
            let ty = rec
                .get("type")
                .or_else(|| rec.get("record_type"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if !ty.eq_ignore_ascii_case("TXT") {
                continue;
            }
            let name = rec
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim_end_matches('.')
                .to_ascii_lowercase();
            if name == rel_l || name.starts_with(&format!("{rel_l}.")) || name.ends_with(&rel_l) {
                if let Some(id) = rec.get("id").and_then(|x| x.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
    }
    ids
}

fn plant_txt(binding: &DomainBinding, hostname: &str, value: &str) -> Result<String, String> {
    let zone_id = binding
        .zone_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "dns_write binding has no zone id — cannot plant TXT".to_string())?;
    let name = acme_txt_relative_name(hostname, &binding.apex);
    let body = serde_json::json!({
        "type": "TXT",
        "name": name,
        "content": value,
        "ttl": 60,
    })
    .to_string();
    let path = format!("/api/dns/zones/{zone_id}/records");
    let resp = proxy_request("POST", &path, Some("k2-acme"), Some(&body))?;
    if resp.status != 200 && resp.status != 201 {
        return Err(format!(
            "plant _acme-challenge TXT failed HTTP {}: {}",
            resp.status, resp.body
        ));
    }
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or(serde_json::json!({}));
    v.pointer("/record/id")
        .or_else(|| v.get("id"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "plant TXT succeeded but response had no record id".to_string())
}

fn delete_txt(record_id: &str) -> Result<(), String> {
    let path = format!("/api/dns/records/{record_id}");
    let resp = proxy_request("DELETE", &path, Some("k2-acme"), None)?;
    if resp.status == 200 || resp.status == 204 || resp.status == 404 {
        Ok(())
    } else {
        Err(format!(
            "delete ACME TXT {record_id} HTTP {}: {}",
            resp.status, resp.body
        ))
    }
}

struct Http01Guard {
    stop: Arc<Mutex<bool>>,
}

impl Http01Guard {
    fn spawn(token: &str, body: &str) -> Result<Self, String> {
        let port: u16 = std::env::var("K2_ACME_HTTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(80);
        let listener = TcpListener::bind(("0.0.0.0", port))
            .or_else(|_| TcpListener::bind(("127.0.0.1", port)))
            .map_err(|e| {
                format!("HTTP-01 cannot bind :{port} ({e}) — open :80 or use DNS-01")
            })?;
        let _ = listener.set_nonblocking(true);
        let stop = Arc::new(Mutex::new(false));
        let stop_c = Arc::clone(&stop);
        let path = format!("/.well-known/acme-challenge/{token}");
        let payload = body.to_string();
        std::thread::spawn(move || loop {
            if *stop_c.lock().unwrap() {
                break;
            }
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let ok = req.contains(&path);
                    let resp = if ok {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        )
                    } else {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    };
                    let _ = sock.write_all(resp.as_bytes());
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        });
        Ok(Self { stop })
    }
}

impl Drop for Http01Guard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.stop.lock() {
            *g = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_txt_name_for_mail_and_apex() {
        assert_eq!(
            acme_txt_relative_name("lztek.io", "lztek.io"),
            "_acme-challenge"
        );
        assert_eq!(
            acme_txt_relative_name("mail.lztek.io", "lztek.io"),
            "_acme-challenge.mail"
        );
        assert_eq!(
            acme_txt_relative_name("a.b.lztek.io", "lztek.io"),
            "_acme-challenge.a.b"
        );
        assert_eq!(
            acme_txt_fqdn("scratch-acme.discover-nocode.com", "discover-nocode.com"),
            "_acme-challenge.scratch-acme.discover-nocode.com"
        );
        assert_eq!(
            acme_txt_fqdn("lztek.io", "lztek.io"),
            "_acme-challenge.lztek.io"
        );
    }

    #[test]
    fn acme_txt_record_ids_picks_txt_for_relative_name() {
        let body = r#"{"records":[
            {"id":"a","type":"CNAME","name":"scratch-le2"},
            {"id":"b","type":"TXT","name":"_acme-challenge.scratch-le2"},
            {"id":"c","type":"TXT","name":"_acme-challenge.other"}
        ]}"#;
        assert_eq!(
            acme_txt_record_ids(body, "_acme-challenge.scratch-le2"),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn k2_dev_names_are_rejected() {
        assert!(reject_k2_dev("rosson.k2.dev").is_err());
        assert!(reject_k2_dev("mail.lztek.io").is_ok());
    }

    #[test]
    fn fake_le_cert_is_not_rcgen() {
        let pem = mint_fake_le("mail.example.com").expect("mint");
        assert!(pem.chain_pem.contains("BEGIN CERTIFICATE"));
        assert!(pem.key_pem.contains("BEGIN"));
        assert!(
            !pem.chain_pem.to_ascii_lowercase().contains("rcgen"),
            "fake leaf must not look like rcgen"
        );
    }

    fn mint_le_like(hostname: &str) -> InstalledPem {
        let mut ca_params =
            CertificateParams::new(vec!["Let's Encrypt Authority X3".to_string()]).expect("ca params");
        let mut ca_dn = DistinguishedName::new();
        ca_dn.push(DnType::OrganizationName, "Let's Encrypt");
        ca_dn.push(DnType::CommonName, "Let's Encrypt Authority X3");
        ca_params.distinguished_name = ca_dn;
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().expect("ca key");
        let ca = ca_params.self_signed(&ca_key).expect("ca");

        let mut leaf = CertificateParams::new(vec![hostname.to_string()]).expect("leaf params");
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, hostname.to_string());
        leaf.distinguished_name = dn;
        let leaf_key = KeyPair::generate().expect("leaf key");
        let cert = leaf
            .signed_by(&leaf_key, &ca, &ca_key)
            .expect("sign");
        InstalledPem {
            hostname: hostname.to_string(),
            chain_pem: format!("{}{}", cert.pem(), ca.pem()),
            key_pem: leaf_key.serialize_pem(),
        }
    }

    fn attach_mail(apex: &str, hostname: &str) {
        let _ = k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, apex, None, false).unwrap();
        k2_core::domains::upsert_name(&conn, hostname, apex, k2_core::domains::ROLE_MAIL).unwrap();
    }

    #[test]
    fn inventory_le_pem_is_reusable_fake_ca_is_not() {
        let le = mint_le_like("mail.example.com");
        assert!(
            crate::domains::status::is_reusable_lets_encrypt("mail.example.com", &le.chain_pem),
            "LE-like leaf must skip ACME: issuer parse failed?"
        );
        assert!(
            !crate::domains::status::is_reusable_lets_encrypt("other.example.com", &le.chain_pem),
            "SAN mismatch must not skip ACME"
        );
        let fake = mint_fake_le("mail.example.com").expect("fake");
        assert!(
            !crate::domains::status::is_reusable_lets_encrypt("mail.example.com", &fake.chain_pem),
            "Let's Encrypt Fake CA must not skip ACME"
        );
    }

    #[test]
    fn issue_skips_acme_when_inventory_is_lets_encrypt() {
        let _home = crate::test_support::TempHome::new();
        attach_mail("skip-acme.test", "mail.skip-acme.test");
        let le = mint_le_like("mail.skip-acme.test");
        store::install("mail.skip-acme.test", &le.chain_pem, &le.key_pem).expect("install");
        let _j = store::set_test_jmap_plant(Some(Ok(())));
        let _p = crate::domains::status::set_test_probe(Some(crate::domains::status::ProbeResult {
            state: "issued".into(),
            self_signed: false,
            names: vec!["mail.skip-acme.test".into()],
            expires_at: Some(chrono::Utc::now().timestamp() + 86_400),
            issuer: Some("CN=R3, O=Let's Encrypt".into()),
        }));
        let pem = issue_attached("mail.skip-acme.test").expect("skip ACME");
        assert!(
            crate::domains::status::is_reusable_lets_encrypt("mail.skip-acme.test", &pem.chain_pem),
            "must keep inventory LE PEM, not mint Fake CA"
        );
        assert!(
            !pem.chain_pem.to_ascii_lowercase().contains("fake"),
            "skip-ACME must not replace LE with Fake CA"
        );
        assert_eq!(
            store::test_jmap_plant_calls(),
            1,
            "skip-ACME still plants into Stalwart"
        );
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, "skip-acme.test");
    }

    /// LE-like leaf valid from 90 days before `not_after_unix` (day
    /// granularity: rcgen dates are midnight UTC).
    fn mint_le_like_until(hostname: &str, not_after_unix: i64) -> InstalledPem {
        let mut ca_params =
            CertificateParams::new(vec!["Let's Encrypt R11".to_string()]).expect("ca params");
        let mut ca_dn = DistinguishedName::new();
        ca_dn.push(DnType::OrganizationName, "Let's Encrypt");
        ca_dn.push(DnType::CommonName, "R11");
        ca_params.distinguished_name = ca_dn;
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().expect("ca key");
        let ca = ca_params.self_signed(&ca_key).expect("ca");
        let ymd = |t: i64| {
            use chrono::Datelike;
            let d = chrono::DateTime::from_timestamp(t, 0).expect("ts").date_naive();
            rcgen::date_time_ymd(d.year(), d.month() as u8, d.day() as u8)
        };
        let mut leaf = CertificateParams::new(vec![hostname.to_string()]).expect("leaf params");
        leaf.not_before = ymd(not_after_unix - 90 * 86_400);
        leaf.not_after = ymd(not_after_unix);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, hostname.to_string());
        leaf.distinguished_name = dn;
        let leaf_key = KeyPair::generate().expect("leaf key");
        let cert = leaf.signed_by(&leaf_key, &ca, &ca_key).expect("sign");
        InstalledPem {
            hostname: hostname.to_string(),
            chain_pem: format!("{}{}", cert.pem(), ca.pem()),
            key_pem: leaf_key.serialize_pem(),
        }
    }

    fn attach_other(apex: &str, hostname: &str) {
        let _ = k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, apex, None, false).unwrap();
        k2_core::domains::upsert_name(&conn, hostname, apex, k2_core::domains::ROLE_OTHER).unwrap();
    }

    fn detach(apex: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, apex);
    }

    /// The renewal window: the box store's certificate is reused with 31
    /// days left, re-ordered with 29 days left or after expiry. Manual
    /// runs are recorded in renewal.json.
    #[test]
    fn manual_issue_reuses_outside_window_and_renews_inside_it() {
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_ACME_FAKE", "1");
        std::env::set_var("K2_ACME_HTTP01", "1");
        std::env::set_var("K2_ACME_DNS_WAIT_SECS", "0");
        let now = chrono::Utc::now().timestamp();
        let day = 86_400;
        for (host, not_after, want_reuse) in [
            ("w31.window.test", now + 32 * day, true),
            ("w29.window.test", now + 29 * day, false),
            ("expired.window.test", now - 2 * day, false),
        ] {
            attach_other("window.test", host);
            let installed = mint_le_like_until(host, not_after);
            store::install(host, &installed.chain_pem, &installed.key_pem).expect("install");
            let pem = issue_attached(host).expect("issue");
            if want_reuse {
                assert_eq!(pem.chain_pem, installed.chain_pem, "{host}: reused, no new order");
            } else {
                assert_ne!(pem.chain_pem, installed.chain_pem, "{host}: must re-order");
                assert!(pem.chain_pem.contains("BEGIN CERTIFICATE"));
                assert_eq!(
                    store::load(host).expect("store").chain_pem,
                    pem.chain_pem,
                    "{host}: the new certificate replaces the old one in the box store"
                );
            }
            let rec = renew::load().names[host].clone();
            assert_eq!(rec.last_trigger.as_deref(), Some("manual"));
            assert_eq!(
                rec.last_result.as_deref(),
                Some(if want_reuse { "planted" } else { "renewed" }),
                "{host}"
            );
            assert_eq!(rec.failures, 0);
        }
        detach("window.test");
        std::env::remove_var("K2_ACME_HTTP01");
    }

    /// A manual issue never runs while another run (the background
    /// renewer) holds the name, and that is not a recorded attempt.
    #[test]
    fn manual_issue_refuses_while_the_name_is_locked() {
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_ACME_FAKE", "1");
        std::env::set_var("K2_ACME_HTTP01", "1");
        attach_other("lock.test", "app.lock.test");
        let held = renew::NameLock::try_acquire("app.lock.test").expect("lock");
        let err = issue_attached("app.lock.test").expect_err("busy");
        assert!(err.contains("already in progress"), "{err}");
        assert!(store::load("app.lock.test").is_none(), "nothing ordered while locked");
        assert!(!renew::load().names.contains_key("app.lock.test"), "busy is not an attempt");
        let mut params = std::collections::HashMap::new();
        params.insert("hostname".to_string(), "app.lock.test".to_string());
        let resp = crate::domains::routes::handle_issue(&params);
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        assert!(resp.body.contains("\"busy\""), "{}", resp.body);
        drop(held);
        issue_attached("app.lock.test").expect("free again");
        detach("lock.test");
        std::env::remove_var("K2_ACME_HTTP01");
    }

    #[test]
    fn mail_issue_fails_loud_if_jmap_plant_fails() {
        let _home = crate::test_support::TempHome::new();
        attach_mail("plant-fail.test", "mail.plant-fail.test");
        std::env::set_var("K2_ACME_FAKE", "1");
        std::env::set_var("K2_ACME_HTTP01", "1");
        std::env::set_var("K2_ACME_DNS_WAIT_SECS", "0");
        let _j = store::set_test_jmap_plant(Some(Err(
            "x:Certificate/set: JMAP error 'invalidProperties': bad PEM".into(),
        )));
        let err = issue_attached("mail.plant-fail.test").expect_err("plant must fail loud");
        assert!(
            err.contains("invalidProperties") || err.contains("x:Certificate/set") || err.contains("bad PEM"),
            "{err}"
        );
        // Recorded: the certificate is in the box store but not in
        // Stalwart, so the renewer retries the plant (after backoff).
        let rec = renew::load().names["mail.plant-fail.test"].clone();
        assert!(rec.plant_pending);
        assert_eq!(rec.failures, 1);
        assert!(rec.next_attempt.is_some());
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, "plant-fail.test");
        std::env::remove_var("K2_ACME_HTTP01");
    }

    #[test]
    fn mail_issue_fails_if_probe_still_self_signed() {
        let _home = crate::test_support::TempHome::new();
        attach_mail("rcgen-probe.test", "mail.rcgen-probe.test");
        std::env::set_var("K2_ACME_FAKE", "1");
        std::env::set_var("K2_ACME_HTTP01", "1");
        std::env::set_var("K2_ACME_DNS_WAIT_SECS", "0");
        let _j = store::set_test_jmap_plant(Some(Ok(())));
        let _p = crate::domains::status::set_test_probe(Some(crate::domains::status::ProbeResult {
            state: "self-signed".into(),
            self_signed: true,
            names: vec!["localhost".into()],
            expires_at: Some(chrono::Utc::now().timestamp() + 86_400),
            issuer: Some("CN=rcgen self signed".into()),
        }));
        let err = issue_attached("mail.rcgen-probe.test").expect_err("rcgen probe must fail loud");
        assert!(
            err.contains("self-signed") || err.to_ascii_lowercase().contains("rcgen"),
            "{err}"
        );
        assert!(
            err.contains("did not pick up") || err.contains("plant"),
            "{err}"
        );
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, "rcgen-probe.test");
        std::env::remove_var("K2_ACME_HTTP01");
    }

    #[test]
    fn reject_if_self_signed_is_loud() {
        let rcgen = crate::domains::status::ProbeResult {
            state: "self-signed".into(),
            self_signed: true,
            names: vec!["mail.example.com".into()],
            expires_at: None,
            issuer: Some("CN=rcgen self signed".into()),
        };
        let err = crate::domains::status::reject_if_self_signed(&rcgen).expect_err("rcgen");
        assert!(err.contains("self-signed"), "{err}");
        let issued = crate::domains::status::ProbeResult {
            state: "issued".into(),
            self_signed: false,
            names: vec!["mail.example.com".into()],
            expires_at: None,
            issuer: Some("CN=R3, O=Let's Encrypt".into()),
        };
        crate::domains::status::reject_if_self_signed(&issued).expect("issued");
    }

    fn byo_binding() -> DomainBinding {
        DomainBinding {
            apex: "discover-nocode.com".into(),
            zone_id: None,
            dns_write: false,
            created_at: 0,
            status: None,
            nameservers: Vec::new(),
            auto_created: false,
        }
    }

    /// CAL44: the per-name issuer refuses (before any network call) a
    /// non-K2-hosted zone, a zone still pending NS, the apex itself, a
    /// name outside the apex, and k2.dev names.
    #[test]
    fn extra_name_issuer_refuses_before_any_network() {
        let k2_hosted = DomainBinding {
            apex: "example.com".into(),
            zone_id: Some("z1".into()),
            dns_write: true,
            created_at: 0,
            status: Some("active".into()),
            nameservers: Vec::new(),
            auto_created: false,
        };
        check_extra_name_issuable("autoconfig.example.com", &k2_hosted).expect("ok");
        let byo = DomainBinding {
            apex: "example.com".into(),
            ..byo_binding()
        };
        let err = issue_extra_name("autoconfig.example.com", &byo).expect_err("BYO zone");
        assert!(err.contains("not K2-hosted"), "{err}");
        let pending = DomainBinding {
            status: Some(k2_core::domains::ZONE_STATUS_PENDING_NS.into()),
            ..k2_hosted.clone()
        };
        let err = issue_extra_name("mta-sts.example.com", &pending).expect_err("pending NS");
        assert!(err.contains("pending_ns"), "{err}");
        let err = issue_extra_name("example.com", &k2_hosted).expect_err("apex");
        assert!(err.contains("never the apex"), "{err}");
        let err = issue_extra_name("autoconfig.example.org", &k2_hosted).expect_err("outside");
        assert!(err.contains("not a name under"), "{err}");
        let k2dev = DomainBinding {
            apex: "rosson.k2.dev".into(),
            ..k2_hosted
        };
        let err = issue_extra_name("autoconfig.rosson.k2.dev", &k2dev).expect_err("k2.dev");
        assert!(err.contains("cert.k2.dev"), "{err}");
    }

    #[test]
    fn cname_hostname_refuses_http01_without_dns_write() {
        std::env::set_var("K2_ACME_HOSTNAME_IS_CNAME", "1");
        let err = select_challenge(&byo_binding(), "mail.discover-nocode.com")
            .expect_err("CNAME must not HTTP-01");
        std::env::remove_var("K2_ACME_HOSTNAME_IS_CNAME");
        assert!(err.contains("CNAME"), "{err}");
        assert!(err.contains("dns_write") || err.contains("DNS-01"), "{err}");
    }

    #[test]
    fn dns_write_still_picks_dns01_when_hostname_is_cname() {
        std::env::set_var("K2_ACME_HOSTNAME_IS_CNAME", "1");
        let binding = DomainBinding {
            apex: "discover-nocode.com".into(),
            zone_id: Some("z1".into()),
            dns_write: true,
            created_at: 0,
            status: Some("active".into()),
            nameservers: Vec::new(),
            auto_created: false,
        };
        let kind = match select_challenge(&binding, "mail.discover-nocode.com") {
            Ok(k) => k,
            Err(e) => {
                std::env::remove_var("K2_ACME_HOSTNAME_IS_CNAME");
                assert!(
                    e.contains("tunnel token") || e.contains("DNS-01"),
                    "dns_write must attempt DNS-01, not HTTP-01: {e}"
                );
                return;
            }
        };
        std::env::remove_var("K2_ACME_HOSTNAME_IS_CNAME");
        assert_eq!(kind, ChallengeKind::Dns01);
    }

    /// K2-issuer boxes: a successful K2 plant+load of the mail certificate
    /// switches Stalwart ACME to Manual (cert_owner::after_k2_plant); a
    /// failed plant, a plant whose probe is still self-signed, or a
    /// non-mail name never does.
    #[test]
    fn mail_plant_switches_stalwart_acme_to_manual_only_after_success() {
        let _home = crate::test_support::TempHome::new();
        let issued_probe = |host: &str| crate::domains::status::ProbeResult {
            state: "issued".into(),
            self_signed: false,
            names: vec![host.into()],
            expires_at: Some(chrono::Utc::now().timestamp() + 86_400),
            issuer: Some("CN=R11, O=Let's Encrypt".into()),
        };
        let _ = crate::mail::cert_owner::take_test_after_plant();

        attach_mail("manual-ok.test", "mail.manual-ok.test");
        let le = mint_le_like("mail.manual-ok.test");
        store::install("mail.manual-ok.test", &le.chain_pem, &le.key_pem).expect("install");
        {
            let _j = store::set_test_jmap_plant(Some(Ok(())));
            let _p = crate::domains::status::set_test_probe(Some(issued_probe("mail.manual-ok.test")));
            issue_attached("mail.manual-ok.test").expect("plant");
        }
        assert_eq!(
            crate::mail::cert_owner::take_test_after_plant(),
            vec!["mail.manual-ok.test".to_string()],
            "one Manual switch after the plant"
        );

        // Stalwart refused the plant: no Manual switch.
        {
            let _j = store::set_test_jmap_plant(Some(Err("x:Certificate/set: invalidProperties".into())));
            let _p = crate::domains::status::set_test_probe(Some(issued_probe("mail.manual-ok.test")));
            issue_attached("mail.manual-ok.test").expect_err("plant refused");
        }
        assert!(crate::mail::cert_owner::take_test_after_plant().is_empty());

        // Planted but 443/465 still self-signed: no Manual switch.
        {
            let _j = store::set_test_jmap_plant(Some(Ok(())));
            let _p = crate::domains::status::set_test_probe(Some(crate::domains::status::ProbeResult {
                state: "self-signed".into(),
                self_signed: true,
                names: vec!["localhost".into()],
                expires_at: None,
                issuer: Some("CN=rcgen self signed".into()),
            }));
            issue_attached("mail.manual-ok.test").expect_err("still self-signed");
        }
        assert!(crate::mail::cert_owner::take_test_after_plant().is_empty());

        // A non-mail name never touches Stalwart.
        {
            let _ = k2_core::db::init_for_tests();
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_name(&conn, "app.manual-ok.test", "manual-ok.test", "other").unwrap();
        }
        let le = mint_le_like("app.manual-ok.test");
        store::install("app.manual-ok.test", &le.chain_pem, &le.key_pem).expect("install");
        issue_attached("app.manual-ok.test").expect("re-use, no plant");
        assert!(crate::mail::cert_owner::take_test_after_plant().is_empty());

        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, "manual-ok.test");
    }

    /// (d) No failure path ever falls back to a self-signed / rcgen
    /// certificate. Ratchet over the production source of every module
    /// that plants or renews mail / domain certificates: a certificate is
    /// only ever minted in `mint_fake_le` (reached only through
    /// `fake_issue`, itself only behind `fake_enabled()` — tests or the
    /// explicit K2_ACME_FAKE knob) or the test helper `test_le_pem`.
    #[test]
    fn no_production_path_mints_a_self_signed_certificate() {
        fn prod(src: &str) -> &str {
            src.split("\n#[cfg(test)]\nmod tests").next().expect("source")
        }
        /// (fn name, body) chunks of a source file.
        fn fns(src: &str) -> Vec<(String, String)> {
            let mut out = Vec::new();
            for chunk in src
                .split("\nfn ")
                .skip(1)
                .chain(src.split("\npub fn ").skip(1))
                .chain(src.split("\npub(crate) fn ").skip(1))
            {
                let name: String =
                    chunk.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                let body = chunk.split("\n}\n").next().unwrap_or(chunk).to_string();
                out.push((name, body));
            }
            out
        }
        let mint = [".self_signed(", "CertificateParams::new", "generate_simple_self_signed"];
        for (file, src) in [
            ("domains/store.rs", include_str!("store.rs")),
            ("domains/renew.rs", include_str!("renew.rs")),
            ("domains/routes.rs", include_str!("routes.rs")),
            ("mail/cert_names.rs", include_str!("../mail/cert_names.rs")),
            ("mail/cert_owner.rs", include_str!("../mail/cert_owner.rs")),
            ("mail/jmap.rs", include_str!("../mail/jmap.rs")),
            ("mail/routes_server.rs", include_str!("../mail/routes_server.rs")),
            ("mail/supervisor.rs", include_str!("../mail/supervisor.rs")),
        ] {
            for m in mint {
                assert!(!prod(src).contains(m), "{file} production code mints a certificate ({m})");
            }
        }
        let acme = prod(include_str!("acme.rs"));
        let acme_fns = fns(acme);
        for want in ["mint_fake_le", "fake_issue", "issue_with_challenge", "issue_extra_name"] {
            assert!(acme_fns.iter().any(|(n, _)| n == want), "scan must see fn {want}");
        }
        for (name, body) in acme_fns {
            if mint.iter().any(|m| body.contains(m)) {
                assert!(
                    name == "mint_fake_le" || name == "test_le_pem",
                    "acme.rs fn {name} mints a certificate"
                );
            }
            if name != "mint_fake_le" && body.contains("mint_fake_le(") {
                assert_eq!(name, "fake_issue", "mint_fake_le reached from {name}");
            }
            if name != "fake_issue" && body.contains("fake_issue(") {
                assert!(body.contains("if fake_enabled()"), "fn {name} reaches fake_issue without fake_enabled()");
            }
        }
        assert!(
            acme.contains("#[cfg(test)]\npub(crate) fn test_le_pem"),
            "test_le_pem stays test-only"
        );
    }
}
