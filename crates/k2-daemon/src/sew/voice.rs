//! Sew voice on K2 (S29 K13): `sew-media` with TURN over TLS, so a call is
//! TURN-TLS (TCP 5349) where UDP is blocked plus WebRTC DTLS-SRTP end to
//! end, on a box K2 already runs Sew on.
//!
//! What K2 does (Sew does the rest through `sew media install`):
//! - **Ports preflight**: TCP 7880 (loopback signalling), TCP 7881 (ICE-TCP),
//!   UDP 7882 (media) and TCP 5349 (TURN-TLS) free unless `sew-media` holds
//!   them. K2 opens no firewall (K2 Web owns the host and provider rules).
//! - **Node IP**: the IPv4 the K2 Connect control plane sees for this box
//!   (`PUT /turn/dns/<label>` answers it), unless the owner names one. A box
//!   whose interfaces do not carry that IP is behind NAT: warned, since Sew
//!   needs 1:1 NAT without port remapping.
//! - **TURN host + cert** (k2-connect turn mode, `docs/turn.md`):
//!   `turn-<label>.k2.dev` (one label; the `turn-` prefix is reserved). First
//!   `PUT /turn/dns/<label>` (grey-cloud A record to the box), then a CSR for
//!   exactly that one SAN (CN the same) from a P-256 key of its OWN (never
//!   the tunnel key), `POST /cert {"mode":"turn"}`. The key lives in the K2
//!   vault (`engine:sew`, kind `turn-key`); the public chain and the renewal
//!   state live in `~/.k2/sew/voice.json`. Renewal reuses the key, so Sew
//!   swaps the chain with no engine restart (`sew media turn-cert
//!   --when-idle`). `403 ip_unverified` is retried after the next frpc
//!   Login (the relay learns the box's IP there), with backoff.
//! - **Served check**: after install and on `status?health=1`, K2 connects
//!   to `<node ip>:5349` (`:443` on the `passthrough` tier) with SNI
//!   `turn-<label>.k2.dev` and compares the leaf it is served with the leaf
//!   it issued (`k2_turn_tls_mismatch` if not).
//! - **TURN tier** (0.46.1, front443): `direct` (sew-media on public TCP
//!   5349, the default) or `passthrough` — when the daemon's :443 front
//!   serves, sew-media listens on 127.0.0.1:5349 behind it, the front
//!   splices `turn-<label>.k2.dev` there after a PROXY v2 header, and the
//!   only ICE URL is `turns:turn-<label>.k2.dev:443` (Sew TURN-PT, rev 168).
//!   The cert and key are staged exactly as for `direct`; the key never
//!   leaves sew-media. [`set_tier`] moves an installed engine between the two
//!   (`media reconfigure restart-now`: the listeners change).
//! - **`voice.public_url`**: `wss://voice.<label>.k2.dev` by default (K2
//!   points the nested `voice` label at 127.0.0.1:7880), or any `wss://`
//!   origin on this box's own k2.dev label or a custom domain bound to it,
//!   e.g. an App that proxies Sew's `/rtc` same-origin (Hearth's BFF). Set
//!   at install or later ([`set_url`]); Sew's core reads it at start, so a
//!   change restarts `sew.service`.
//! - **Status**: `sew media status` mapped into `/cli/sew/status` `voice`,
//!   with `voice_*` / `k2_turn_*` alarms (degraded) and setup warnings
//!   (TURN not on 443, no outside probe yet: never degraded).
//! - **Outside probe** (S49 §17.1 ask 13): a person or agent tests a call
//!   from another network and records it (`k2 hostchat media probe`). A
//!   current probe (under 30 days, same node IP and TURN tier) with a
//!   failed leg is `k2_voice_probe_failed` (degraded); an older one is the
//!   `voice_probe_stale` setup warning. K2 cannot probe from outside itself.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use k2_root_proto::sew::{
    self as pin, pinned_media_artifact, MAX_TARBALL, SEW_MEDIA_LOOPBACK_PORT, SEW_MEDIA_TCP_PORT, SEW_MEDIA_UDP_PORT,
    SEW_TURN_TLS_PORT, UNIT_DB, UNIT_MEDIA, UNIT_SEW,
};

use super::ops::CpError;
use super::supervisor::{self as sup, no, Env, Fail, R};

/// Renew the TURN cert inside this window before `notAfter`.
pub const RENEW_WINDOW_SECS: i64 = 30 * 86_400;
/// `k2_turn_cert_expiring` inside this window.
pub const EXPIRING_SECS: i64 = 14 * 86_400;
/// Retry backoff after a failed renewal: 2 min doubling, at most 1 h.
pub const RETRY_MIN_SECS: i64 = 120;
pub const RETRY_MAX_SECS: i64 = 3600;
/// The vault kind of the TURN key.
pub const TURN_KEY_KIND: &str = "turn-key";
/// Box-level nested label for the default signalling URL.
pub const VOICE_LABEL: &str = k2_core::skin::VOICE_NESTED_LABEL;

/// The voice record (`~/.k2/sew/voice.json`). Holds no secret: the TURN
/// key is a vault ref.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceState {
    /// `sew media install` succeeded (voice is set up).
    pub installed: bool,
    /// The `voice.public_url` Sew holds (or will, once installed).
    pub public_url: Option<String>,
    /// `direct` or `passthrough` (front443); absent = `direct` (pre-0.46.1).
    pub turn_tier: Option<String>,
    pub label: Option<String>,
    pub turn_host: Option<String>,
    /// The advertised IPv4[,IPv6].
    pub node_ip: Option<String>,
    /// `control_plane` (observed by K2 Connect) or `manual`.
    pub node_ip_source: Option<String>,
    pub turn_key_ref: Option<String>,
    /// The issued chain (public), leaf first.
    pub cert_pem: Option<String>,
    pub cert_not_after: Option<i64>,
    pub cert_leaf_sha256: Option<String>,
    pub cert_issued_at: Option<i64>,
    /// Last served check: `ok`, `mismatch` or `nothing`.
    pub served: Option<String>,
    pub served_at: Option<i64>,
    /// The nested `voice` label points at sew-media's loopback port.
    pub voice_label: bool,
    /// `{step, result: running|done|error, error?, at}`.
    pub progress: Option<Value>,
    pub last_error: Option<String>,
    pub next_try_at: Option<i64>,
    pub failures: u32,
    /// The renewal is waiting for an frpc Login past this count.
    pub logins_at_failure: Option<u64>,
    /// Last outside probe K2 recorded (`{udp, tcp, turnTls, at}`).
    pub probe: Option<Value>,
    pub installed_at: Option<i64>,
    pub updated_at: i64,
    pub warnings: Vec<String>,
}

/// The TURN tier behind the front.
pub const TIER_PASSTHROUGH: &str = "passthrough";
pub const TIER_DIRECT: &str = "direct";
/// The public port the front owns.
pub const FRONT_PORT: u16 = 443;

/// The tier an installed engine runs (`direct` unless recorded).
pub fn tier(st: &VoiceState) -> &str {
    st.turn_tier.as_deref().unwrap_or(TIER_DIRECT)
}

/// The public TURN-TLS port of a tier.
pub fn tier_port(tier: &str) -> u16 {
    if tier == TIER_PASSTHROUGH {
        FRONT_PORT
    } else {
        SEW_TURN_TLS_PORT
    }
}

/// The host the front routes to sew-media's passthrough listener, when
/// voice is installed on that tier.
pub fn front_route_host(env: &Env) -> Option<String> {
    let st = load(env);
    (st.installed && tier(&st) == TIER_PASSTHROUGH).then_some(st.turn_host).flatten()
}

pub fn load(env: &Env) -> VoiceState {
    env.ops.voice_load().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save(env: &Env, st: &mut VoiceState) {
    st.updated_at = env.ops.now();
    let v = serde_json::to_value(&*st).expect("plain struct");
    if let Err(e) = env.ops.voice_save(Some(&v)) {
        k2_core::log_debug!("[sew] voice state: {e}");
    }
}

fn update(env: &Env, f: impl FnOnce(&mut VoiceState)) -> VoiceState {
    let mut st = load(env);
    f(&mut st);
    save(env, &mut st);
    st
}

// ── names and URLs ───────────────────────────────────────────────────────

pub fn turn_host(label: &str) -> String {
    format!("turn-{label}.k2.dev")
}

pub fn default_voice_url(label: &str) -> String {
    format!("wss://{VOICE_LABEL}.{label}.k2.dev")
}

fn under(host: &str, base: &str) -> bool {
    host == base || host.strip_suffix(base).is_some_and(|h| h.ends_with('.'))
}

/// A host this box may hand clients: under its own `<label>.k2.dev` or a
/// custom domain bound to it. Never `turn-*` hosts.
pub fn host_allowed(host: &str, label: Option<&str>, apexes: &[String]) -> bool {
    let h = host.to_ascii_lowercase();
    if label.is_some_and(|l| under(&h, &format!("{l}.k2.dev"))) {
        return true;
    }
    apexes.iter().any(|a| !a.is_empty() && under(&h, &a.to_ascii_lowercase()))
}

fn url_host<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    url.strip_prefix(scheme)?.strip_prefix("://")
}

fn not_ours(url: &str, label: Option<&str>) -> Fail {
    no(
        "url_not_on_this_box",
        400,
        format!(
            "{url} is not on this box: use a host under {} or a custom domain bound to this box (k2 domain attach)",
            label.map(|l| format!("{l}.k2.dev")).unwrap_or_else(|| "<label>.k2.dev".into())
        ),
    )
}

/// `voice.public_url`: `wss://<host>` on this box.
pub fn check_voice_url(env: &Env, url: &str) -> R<String> {
    let url = url.trim();
    if !pin::voice_url_ok(url) {
        return Err(no("bad_request", 400, "voice URL must be wss://<host> (lowercase, no port, no path), e.g. wss://hearth.<box>.k2.dev"));
    }
    let label = env.ops.box_label();
    let host = url_host(url, "wss").unwrap_or("");
    if !host_allowed(host, label.as_deref(), &env.ops.bound_apexes()) {
        return Err(not_ours(url, label.as_deref()));
    }
    Ok(url.to_string())
}

/// `server.public_url` (`https://<host>`) on this box.
pub fn check_https_url(env: &Env, url: &str) -> R<String> {
    let url = url.trim();
    if url.is_empty() || !pin::public_url_ok(url) {
        return Err(no("bad_request", 400, "must be exactly https://<host> (lowercase, no port, no path)"));
    }
    let label = env.ops.box_label();
    let host = url_host(url, "https").unwrap_or("");
    if !host_allowed(host, label.as_deref(), &env.ops.bound_apexes()) {
        return Err(not_ours(url, label.as_deref()));
    }
    Ok(url.to_string())
}

fn public_ip_ok(s: &str) -> bool {
    match s.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => crate::mail::cert_names::is_public_v4(&v4),
        Ok(std::net::IpAddr::V6(v6)) => crate::mail::cert_names::is_global_v6(&v6),
        Err(_) => false,
    }
}

// ── preflight ────────────────────────────────────────────────────────────

fn check(id: &str, status: &str, detail: impl Into<String>, hint: impl Into<String>) -> Value {
    json!({"id": id, "status": status, "detail": detail.into(), "hint": hint.into()})
}

/// The voice ports: (label, addr, port, udp).
pub const PORTS: [(&str, &str, u16, bool); 4] = [
    ("signalling (loopback)", "127.0.0.1", SEW_MEDIA_LOOPBACK_PORT, false),
    ("ICE-TCP", "0.0.0.0", SEW_MEDIA_TCP_PORT, false),
    ("media", "0.0.0.0", SEW_MEDIA_UDP_PORT, true),
    ("TURN-TLS", "0.0.0.0", SEW_TURN_TLS_PORT, false),
];

/// Read-only voice checks (`GET /cli/sew/media/preflight`; install refuses
/// on any `fail`).
pub fn preflight(env: &Env) -> Vec<Value> {
    let mut c = Vec::new();
    if !env.ops.linux() {
        c.push(check("os", "fail", "not Linux", "Chat / Voice Hosting runs on Linux boxes only"));
        return c;
    }
    match store_row() {
        Some(r) if matches!(r.status.as_str(), "running" | "degraded") => c.push(check("chat", "ok", format!("Chat / Voice Hosting is {}", r.status), "")),
        Some(r) => c.push(check("chat", "fail", format!("Chat / Voice Hosting is {}", r.status), "voice needs Chat / Voice Hosting running: k2 hostchat enable")),
        None => c.push(check("chat", "fail", "Chat / Voice Hosting is not installed", "k2 hostchat enable first")),
    }
    match sup::helper_check(env) {
        Ok(()) => c.push(check("helper", "ok", "k2-root-helper engine sew", "")),
        Err(f) => c.push(check("helper", "fail", f.code, f.hint)),
    }
    match pinned_media_artifact(&env.ops.arch()) {
        Ok((v, _)) => c.push(check("pin", "ok", format!("sew-media {v}"), "")),
        Err(e) => c.push(check("pin", "fail", e, "this K2 build pins no sew-media release for this CPU")),
    }
    match env.ops.box_label() {
        Some(l) => c.push(check("label", "ok", format!("{l}.k2.dev; TURN host {}", turn_host(&l)), "")),
        None => c.push(check("label", "fail", "no K2 Connect label", "voice needs K2 Connect for the TURN host and certificate")),
    }
    let media_up = env.ops.unit(UNIT_MEDIA).is_active();
    let taken: Vec<String> = PORTS
        .iter()
        .filter(|(_, a, p, u)| !media_up && env.ops.port_taken(a, *p, *u))
        .map(|(n, _, p, u)| format!("{} {p} ({n})", if *u { "UDP" } else { "TCP" }))
        .collect();
    if taken.is_empty() {
        c.push(check(
            "ports",
            "ok",
            if media_up { "sew-media holds its ports".to_string() } else { "TCP 7880 (loopback), TCP 7881, UDP 7882, TCP 5349 free".to_string() },
            "open TCP 7881, UDP 7882 and TCP 5349 in the host AND provider firewalls (K2 opens none)",
        ));
    } else {
        c.push(check("ports", "fail", format!("taken: {}", taken.join(", ")), "free them or stop what holds them"));
    }
    // S49 §17.1 ask 10: say the box's call capacity before installing.
    if let Some(mem) = env.ops.mem_total() {
        let (m, cap) = call_capacity(mem);
        c.push(check(
            "capacity",
            "info",
            format!("up to {cap} people in calls at once on this {:.0} GiB box (sew-media gets {m} MiB)", mem as f64 / (1024.0 * 1024.0 * 1024.0)),
            "Sew's shared profile; more RAM raises it (at most 500)",
        ));
    }
    if env.ops.front_serving() {
        c.push(check("turn_443", "ok", "K2's :443 front serves this box", "TURN-TLS rides :443 (passthrough tier): networks that allow only 443 can join calls"));
    } else if env.ops.port_taken("0.0.0.0", 443, false) {
        c.push(check(
            "turn_443",
            "info",
            ":443 is taken here",
            "TURN-TLS uses 5349: people on networks that allow only 443 cannot join calls (k2 front enable puts TURN on 443)",
        ));
    }
    c
}

fn store_row() -> Option<super::store::Row> {
    super::store::load()
}

// ── TURN DNS + cert ──────────────────────────────────────────────────────

/// What `PUT /turn/dns/<label>` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDns {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
}

fn cp_fail(step: &str, e: &CpError) -> Fail {
    let (code, status): (&'static str, u16) = match e.code.as_str() {
        "ip_unverified" => ("turn_ip_unverified", 409),
        "rate_limited" => ("turn_rate_limited", 429),
        "subdomain_not_owned" | "invalid_token" | "nested_label_no_cert" => ("turn_not_allowed", 403),
        "label_too_long" => ("turn_label_too_long", 409),
        "record_conflict" => ("turn_record_conflict", 409),
        "zone_inactive" => ("turn_zone_inactive", 409),
        _ if e.status == 0 => ("connect_unreachable", 503),
        _ => ("turn_failed", 502),
    };
    let hint = match code {
        "turn_ip_unverified" => format!(
            "{step}: K2 Connect could not verify this box's IP ({}). It learns it at the next tunnel login: K2 retries on its own; \
             or run the install again in a minute",
            e.detail
        ),
        _ => format!("{step}: {e}"),
    };
    let mut f = no(code, status, hint);
    f.extra = Some(json!({"connect": {"status": e.status, "code": e.code}}));
    f
}

/// `PUT /turn/dns/<label>`: the A record (and the IP K2 Connect sees).
pub fn turn_dns(env: &Env, label: &str, manual: Option<&str>) -> R<TurnDns> {
    let mut body = json!({});
    if let Some(ips) = manual {
        for ip in ips.split(',') {
            match ip.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(_)) => body["ipv4"] = json!(ip),
                Ok(std::net::IpAddr::V6(_)) => body["ipv6"] = json!(ip),
                Err(_) => {}
            }
        }
    }
    let v = env.ops.cp("PUT", &format!("/turn/dns/{label}"), Some(&body)).map_err(|e| cp_fail("TURN DNS", &e))?;
    let want = turn_host(label);
    if v.get("host").and_then(Value::as_str) != Some(want.as_str()) {
        return Err(no("turn_failed", 502, format!("TURN DNS answered host {:?}, expected {want}", v.get("host"))));
    }
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|x| public_ip_ok(x)).map(str::to_string);
    let dns = TurnDns { ipv4: s("ipv4"), ipv6: s("ipv6") };
    if dns.ipv4.is_none() && dns.ipv6.is_none() {
        return Err(no("turn_failed", 502, "TURN DNS answered no public IP"));
    }
    Ok(dns)
}

/// The TURN key from the vault, or a new P-256 one stored there.
fn turn_key(env: &Env, st: &VoiceState) -> R<(rcgen::KeyPair, Option<String>)> {
    if let Some(r) = st.turn_key_ref.as_deref() {
        if let Some(pem_text) = env.secrets.resolve(r).map_err(|e| no("vault_closed", 503, e))? {
            let k = rcgen::KeyPair::from_pem(&pem_text).map_err(|e| no("turn_key_bad", 500, format!("the vault's TURN key does not parse: {e}")))?;
            return Ok((k, None));
        }
    }
    let k = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|e| no("turn_key_bad", 500, e.to_string()))?;
    let r = env.secrets.store(TURN_KEY_KIND, &k.serialize_pem()).map_err(|e| no("vault_closed", 503, e))?;
    Ok((k, Some(r)))
}

/// A CSR for exactly `turn-<label>.k2.dev` (the SAN, and the CN: rcgen's
/// default CN is refused by the broker).
pub fn turn_csr(label: &str, key: &rcgen::KeyPair) -> Result<String, String> {
    let host = turn_host(label);
    let mut params = rcgen::CertificateParams::new(vec![host.clone()]).map_err(|e| e.to_string())?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, host);
    params.distinguished_name = dn;
    params.serialize_request(key).map_err(|e| e.to_string())?.pem().map_err(|e| e.to_string())
}

/// The issued chain is for exactly our host and our key.
pub fn chain_matches(chain: &str, host: &str, key: &rcgen::KeyPair) -> Result<i64, String> {
    use x509_parser::pem::Pem;
    let sans = k2_core::tunnel::cert_broker::cert_dns_sans(chain);
    if sans != vec![host.to_string()] {
        return Err(format!("the TURN certificate names {sans:?}, expected exactly [{host}]"));
    }
    let mut reader = chain.as_bytes();
    let (pem, _) = Pem::read(std::io::Cursor::new(&mut reader)).map_err(|e| format!("chain PEM: {e}"))?;
    let cert = pem.parse_x509().map_err(|e| format!("leaf: {e}"))?;
    if cert.tbs_certificate.subject_pki.raw != key.public_key_der().as_slice() {
        return Err("the TURN certificate is not for K2's TURN key".into());
    }
    if !pin::turn_chain_pem_ok(chain) {
        return Err("the TURN chain is not 1–4 PEM certificates".into());
    }
    Ok(cert.validity().not_after.timestamp())
}

/// The cert K2 already holds, when it is for this host and key and
/// outside the renewal window (an install re-run must not spend K2
/// Connect's turn budget: 5 issuances / 24 h / label).
fn reusable(env: &Env, label: &str, st: &VoiceState) -> Option<(String, String)> {
    let chain = st.cert_pem.as_deref()?;
    if st.cert_not_after.is_none_or(|t| t - env.ops.now() < RENEW_WINDOW_SECS) {
        return None;
    }
    let key_pem = env.secrets.resolve(st.turn_key_ref.as_deref()?).ok()??;
    let key = rcgen::KeyPair::from_pem(&key_pem).ok()?;
    chain_matches(chain, &turn_host(label), &key).ok()?;
    Some((chain.to_string(), key_pem))
}

/// DNS (always: it also tells K2 the box's IP), then the cert K2 holds or a
/// new one for the vault key. Returns (chain, key PEM, dns).
fn issue_or_reuse(env: &Env, label: &str, st: &mut VoiceState, manual_ip: Option<&str>) -> R<(String, String, TurnDns)> {
    if let Some((chain, key_pem)) = reusable(env, label, st) {
        let dns = turn_dns(env, label, manual_ip)?;
        return Ok((chain, key_pem, dns));
    }
    issue(env, label, st, manual_ip)
}

/// DNS, then a cert for the vault key. Returns (chain, key PEM, dns).
fn issue(env: &Env, label: &str, st: &mut VoiceState, manual_ip: Option<&str>) -> R<(String, String, TurnDns)> {
    let dns = turn_dns(env, label, manual_ip)?;
    let (key, new_ref) = turn_key(env, st)?;
    if let Some(r) = new_ref {
        // Recorded at once: a failed issuance below must not orphan the
        // vault key (the next run reuses it).
        st.turn_key_ref = Some(r);
        save(env, st);
    }
    let csr = turn_csr(label, &key).map_err(|e| no("turn_failed", 500, format!("CSR: {e}")))?;
    let body = json!({"csr": csr, "subdomain": label, "token": "", "mode": "turn"});
    let v = env.ops.cp("POST", "/cert", Some(&body)).map_err(|e| cp_fail("TURN certificate", &e))?;
    let chain = v.get("cert").and_then(Value::as_str).filter(|c| !c.trim().is_empty()).ok_or_else(|| no("turn_failed", 502, "the broker returned no certificate"))?;
    let host = turn_host(label);
    let not_after = chain_matches(chain, &host, &key).map_err(|e| no("turn_failed", 502, e))?;
    let now = env.ops.now();
    st.cert_pem = Some(chain.to_string());
    st.cert_not_after = Some(not_after);
    st.cert_leaf_sha256 = crate::domains::status::pem_leaf_sha256(chain);
    st.cert_issued_at = Some(now);
    st.turn_host = Some(host);
    st.label = Some(label.to_string());
    Ok((chain.to_string(), key.serialize_pem(), dns))
}

fn node_ip_of(dns: &TurnDns) -> String {
    [dns.ipv4.clone(), dns.ipv6.clone()].into_iter().flatten().collect::<Vec<_>>().join(",")
}

// ── install ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct InstallRequest {
    /// `voice.public_url`; default `wss://voice.<label>.k2.dev`.
    pub public_url: Option<String>,
    /// The node IP(s) to advertise; default the one K2 Connect sees.
    pub node_ip: Option<String>,
    /// The sew-media release tarball, for a box without internet (the
    /// bytes must match this build's pin). Without it the pinned file comes
    /// from K2's cache or Sew's GitHub release (0.46.7).
    pub tarball_path: Option<std::path::PathBuf>,
}

/// Validate a request without side effects (the route answers 400 before
/// it starts a machine).
pub fn check_request(env: &Env, req: &InstallRequest) -> R<()> {
    if let Some(u) = &req.public_url {
        check_voice_url(env, u)?;
    }
    if let Some(ips) = &req.node_ip {
        if ips.split(',').count() > 2 || !ips.split(',').all(public_ip_ok) {
            return Err(no("bad_request", 400, "nodeIp is one public IPv4 and/or one global IPv6, comma-separated"));
        }
    }
    if let Some(p) = &req.tarball_path {
        if !p.is_absolute() {
            return Err(no("bad_request", 400, "the sew-media tarball path must be absolute"));
        }
    }
    Ok(())
}

/// The synchronous part of `POST /cli/sew/media/install`: refusals, the
/// latch, preflight. Returns the latch; the machine runs next.
pub fn install_begin<'a>(env: &Env<'a>, req: &InstallRequest) -> R<sup::Latch<'a>> {
    sup::need_released(env)?;
    if !env.ops.linux() {
        return Err(no("unsupported", 409, "Chat / Voice Hosting runs on Linux boxes only"));
    }
    check_request(env, req)?;
    let latch = sup::begin(env)?;
    let checks = preflight(env);
    if checks.iter().any(|c| c["status"] == "fail") {
        let mut f = no("preflight_failed", 409, "the box is not ready for voice; nothing was changed (see report)");
        f.extra = Some(json!({"report": checks}));
        return Err(f);
    }
    update(env, |st| {
        st.progress = Some(json!({"step": "stage", "result": "running", "at": env.ops.now()}));
        st.last_error = None;
    });
    Ok(latch)
}

fn step(env: &Env, name: &str) {
    update(env, |st| {
        if let Some(p) = st.progress.as_mut() {
            p["step"] = json!(name);
        }
    });
}

/// The install machine: stage, TURN DNS + cert, `sew media install`, the
/// voice label, start, served check. Records progress; a failure leaves
/// `progress.result = error`; a re-run resumes (staging and the key are
/// reused).
pub fn install_machine(env: &Env, req: &InstallRequest) -> Result<Value, Fail> {
    let r = install_steps(env, req);
    match &r {
        Ok(_) => sup::changed("voice-installed"),
        Err(f) => record_failure(env, f),
    }
    r
}

/// Record why a voice install stopped (`progress.result = error`).
pub fn record_failure(env: &Env, f: &Fail) {
    let msg = format!("{}: {}", f.code, f.hint);
    let now = env.ops.now();
    update(env, |st| {
        let p = st.progress.get_or_insert_with(|| json!({"step": null, "at": now}));
        p["result"] = json!("error");
        p["error"] = json!(msg.clone());
        st.last_error = Some(msg);
    });
    sup::changed("voice-failed");
}

fn install_steps(env: &Env, req: &InstallRequest) -> R<Value> {
    let label = env.ops.box_label().ok_or_else(|| no("no_connect_label", 409, "voice needs K2 Connect (the TURN host is turn-<label>.k2.dev)"))?;
    let mut st = load(env);
    let url = match &req.public_url {
        Some(u) => check_voice_url(env, u)?,
        None => st.public_url.clone().unwrap_or_else(|| default_voice_url(&label)),
    };

    step(env, "stage");
    let note = |d: Value| {
        update(env, |st| {
            if let Some(p) = st.progress.as_mut() {
                p["download"] = d.clone();
            }
        });
    };
    stage_media_bytes(env, req.tarball_path.as_deref(), "k2 hostchat media install --tarball", &note)?;

    step(env, "turn-dns");
    step(env, "turn-cert");
    let (chain, key_pem, dns) = issue_or_reuse(env, &label, &mut st, req.node_ip.as_deref())?;
    save(env, &mut st);
    let node_ip = match &req.node_ip {
        Some(ip) => ip.clone(),
        None => node_ip_of(&dns),
    };
    let mut warnings = Vec::new();
    let local = env.ops.public_ips();
    if !node_ip.split(',').any(|ip| local.iter().any(|l| l == ip)) {
        warnings.push(format!(
            "this box's interfaces do not carry {node_ip}: voice needs 1:1 NAT with the same port numbers (no remapping) for UDP 7882, TCP 7881 and TCP 5349"
        ));
    }

    step(env, "install");
    let host = turn_host(&label);
    // Behind the :443 front: sew-media on 127.0.0.1:5349, PROXY v2 first,
    // advertised as 443 only (Sew's passthrough default).
    let tier = if env.ops.front_serving() && env.ops.passthrough_supported() { TIER_PASSTHROUGH } else { TIER_DIRECT };
    let turn_port = tier_port(tier);
    let mut warnings_tier = Vec::new();
    if env.ops.front_serving() && tier == TIER_DIRECT {
        warnings_tier.push(format!(
            "the :443 front serves, but this Sew has no passthrough TURN tier (Sew {} or later): TURN stays on TCP 5349",
            pin::PASSTHROUGH_MIN_VERSION
        ));
    }
    let mut body = json!({
        "public_url": url,
        "node_ip": node_ip,
        "udp_port": SEW_MEDIA_UDP_PORT,
        "tcp_port": SEW_MEDIA_TCP_PORT,
        "turn_tls": tier,
        "turn_domain": host,
        "sysctl": "apply",
        "turn_cert_pem": chain,
        "turn_key_pem": key_pem,
    });
    if tier == TIER_DIRECT {
        body["turn_advertise_port"] = json!(SEW_TURN_TLS_PORT);
    }
    let out = sup::sew_run(env, &["media", "install"], Some(body.to_string().as_bytes())).map_err(|e| no("media_install_failed", 500, e))?;
    if let Some(w) = out.pointer("/sew/warnings").and_then(Value::as_array) {
        warnings.extend(w.iter().filter_map(Value::as_str).map(str::to_string));
    }
    warnings.extend(warnings_tier);

    step(env, "label");
    let want_label = url == default_voice_url(&label);
    let mut label_ok = st.voice_label;
    if want_label {
        match env.ops.nested_label(VOICE_LABEL, Some(&format!("127.0.0.1:{SEW_MEDIA_LOOPBACK_PORT}"))) {
            Ok(()) => label_ok = true,
            Err(e) => warnings.push(format!("the nested voice label could not be pointed at sew-media ({e}): K2 Connect Pro is needed for voice.{label}.k2.dev")),
        }
    } else if st.voice_label && env.ops.nested_label(VOICE_LABEL, None).is_ok() {
        label_ok = false;
    }

    step(env, "start");
    sup::poll_units(env, &[UNIT_MEDIA], 60).map_err(|e| no("media_start_failed", 500, e))?;

    step(env, "verify");
    if tier == TIER_PASSTHROUGH {
        // The front routes the TURN name only once voice is recorded on
        // this tier: record it, rebuild the front's table, then check.
        update(env, |s| {
            s.installed = true;
            s.turn_tier = Some(tier.to_string());
        });
        env.ops.front_routes_changed();
    }
    let ip4 = node_ip.split(',').next().unwrap_or("").to_string();
    let served = served_check(env, &ip4, &host, st.cert_leaf_sha256.as_deref(), turn_port);
    if served != "ok" {
        warnings.push(format!(
            "TURN-TLS check: {ip4}:{turn_port} served {served} for {host} (a firewall, or another service on {turn_port}?)"
        ));
    }

    let now = env.ops.now();
    st.installed = true;
    st.turn_tier = Some(tier.to_string());
    st.public_url = Some(url.clone());
    st.node_ip = Some(node_ip.clone());
    st.node_ip_source = Some(if req.node_ip.is_some() { "manual" } else { "control_plane" }.into());
    st.voice_label = label_ok;
    st.served = Some(served.clone());
    st.served_at = Some(now);
    st.installed_at = st.installed_at.or(Some(now));
    st.last_error = None;
    st.failures = 0;
    st.next_try_at = None;
    st.logins_at_failure = None;
    st.warnings = warnings.clone();
    st.progress = Some(json!({"step": null, "result": "done", "at": now}));
    save(env, &mut st);
    Ok(json!({
        "ok": true,
        "state": "installed",
        "publicUrl": url,
        "turnHost": host,
        "nodeIp": node_ip,
        "turnTier": tier,
        "turnTls": {"port": turn_port, "served": served, "certNotAfter": st.cert_not_after},
        "ports": {"udp": SEW_MEDIA_UDP_PORT, "tcp": SEW_MEDIA_TCP_PORT, "turnTls": turn_port},
        "warnings": warnings,
        "next": format!(
            "open TCP 7881, UDP 7882 and TCP {turn_port} in the host and provider firewalls, test a call from outside, then \
             k2 hostchat media probe --udp ok --tcp ok --turn-tls ok"
        ),
    }))
}

/// Stage the pinned sew-media: the path given, else (when the helper does
/// not already hold it) K2's cache or Sew's GitHub release (0.46.7). `cmd`
/// is what an offline box runs instead; `note` gets download progress.
pub(crate) fn stage_media_bytes(env: &Env, path: Option<&std::path::Path>, cmd: &str, note: &dyn Fn(Value)) -> R<()> {
    let (v, art) = pinned_media_artifact(&env.ops.arch()).map_err(|e| no("media_unpinned", 409, e))?;
    let staged = sup::helper_json(env, &["status"]).ok().and_then(|s| s.pointer("/staged/media").and_then(Value::as_str).map(str::to_string));
    if staged.as_deref() == Some(v) && path.is_none() {
        return Ok(());
    }
    let fetched;
    let p = match path {
        Some(p) => p,
        None => {
            let f = super::fetch::ReleaseFile::media(pin::pinned().version, v, art);
            fetched = sup::obtain_release(env, &f, cmd, note).map_err(|e| no("media_tarball_needed", 409, e))?;
            fetched.as_path()
        }
    };
    let bytes = env.ops.read_file(p, MAX_TARBALL).map_err(|e| no("bad_request", 400, e))?;
    if !crate::update_routes::verify_sha256(&bytes, art.tarball_sha256) {
        return Err(no("sha256_mismatch", 400, format!("{} is not sew-media {v} for {} — NOT installing", p.display(), art.triple)));
    }
    sup::helper_stdin(env, &["stage-media"], &bytes).map(|_| ()).map_err(|e| no("helper_error", 500, e))
}

/// `ok` (our leaf), `mismatch` (another cert) or `nothing` (no TLS), on
/// the tier's public port.
pub fn served_check(env: &Env, ip: &str, host: &str, ours: Option<&str>, port: u16) -> String {
    match env.ops.served_leaf(ip, host, port) {
        Some(l) if Some(l.as_str()) == ours => "ok".into(),
        Some(_) => "mismatch".into(),
        None => "nothing".into(),
    }
}

// ── voice URL later ──────────────────────────────────────────────────────

/// Set `voice.public_url` on an installed engine. Sew's documented path
/// (docs/voice.md): `sew media install --reconfigure` with the new
/// `public_url` in its JSON input (never `sew config set`), which rewrites
/// `[voice] public_url`; then a `sew.service` restart (Sew's core reads it
/// at start). Moves the nested `voice` label with it.
pub fn set_url(env: &Env, url: &str) -> R<Value> {
    if !env.ops.linux() {
        return Err(no("unsupported", 409, "Chat / Voice Hosting runs on Linux boxes only"));
    }
    let url = check_voice_url(env, url)?;
    let st = load(env);
    if !st.installed {
        // Remembered: the next media install uses it.
        update(env, |s| s.public_url = Some(url.clone()));
        return Ok(json!({"ok": true, "publicUrl": url, "applied": false, "hint": "voice is not installed yet: k2 hostchat media install uses this URL"}));
    }
    if st.public_url.as_deref() == Some(url.as_str()) {
        return Ok(json!({"ok": true, "publicUrl": url, "applied": true, "changed": false}));
    }
    sup::helper_check(env)?;
    let _l = sup::begin(env)?;
    let body = json!({"public_url": url});
    sup::sew_run(env, &["media", "reconfigure"], Some(body.to_string().as_bytes())).map_err(|e| no("media_reconfigure_failed", 500, e))?;
    sup::helper_json(env, &["systemctl", "restart"]).map_err(|e| no("restart_failed", 500, e))?;
    let mut warnings = Vec::new();
    let label = env.ops.box_label();
    let want_label = label.as_deref().is_some_and(|l| url == default_voice_url(l));
    let mut label_ok = st.voice_label;
    if want_label && !st.voice_label {
        match env.ops.nested_label(VOICE_LABEL, Some(&format!("127.0.0.1:{SEW_MEDIA_LOOPBACK_PORT}"))) {
            Ok(()) => label_ok = true,
            Err(e) => warnings.push(format!("the nested voice label could not be pointed at sew-media: {e}")),
        }
    } else if !want_label && st.voice_label {
        match env.ops.nested_label(VOICE_LABEL, None) {
            Ok(()) => label_ok = false,
            Err(e) => warnings.push(format!("the old nested voice label could not be removed: {e}")),
        }
    }
    update(env, |s| {
        s.public_url = Some(url.clone());
        s.voice_label = label_ok;
    });
    sup::changed("voice-url");
    Ok(json!({"ok": true, "publicUrl": url, "applied": true, "changed": true, "restarted": [UNIT_SEW], "warnings": warnings}))
}

// ── TURN tier (front443) ─────────────────────────────────────────────────

/// The chain and key K2 already holds for the TURN host (no issuance: a
/// tier move must not spend K2 Connect's turn budget).
fn held_material(env: &Env, st: &VoiceState) -> R<(String, String, String)> {
    let host = st.turn_host.clone().ok_or_else(|| no("voice_not_installed", 409, "no TURN host recorded"))?;
    let chain = st.cert_pem.clone().ok_or_else(|| no("k2_turn_cert_missing", 409, "no TURN certificate recorded: k2 hostchat media renew"))?;
    let r = st.turn_key_ref.as_deref().ok_or_else(|| no("k2_turn_cert_missing", 409, "no TURN key recorded"))?;
    let key_pem = env
        .secrets
        .resolve(r)
        .map_err(|e| no("vault_closed", 503, e))?
        .ok_or_else(|| no("k2_turn_cert_missing", 409, "the TURN key is gone from the vault: k2 hostchat media renew"))?;
    Ok((host, chain, key_pem))
}

/// Move an installed engine to the `passthrough` tier (behind the :443
/// front) or back to `direct` (public 5349). `sew media install
/// --reconfigure --restart --now` with the tier, the TURN host and the
/// same cert and key K2 holds: the TURN listeners change, so sew-media
/// restarts and calls in progress drop (Sew docs/voice.md). A no-op when
/// the engine already runs that tier, or voice isn't installed.
pub fn set_tier(env: &Env, passthrough: bool) -> R<Value> {
    let want = if passthrough { TIER_PASSTHROUGH } else { TIER_DIRECT };
    let st = load(env);
    if !st.installed {
        return Ok(json!({"ok": true, "installed": false, "turnTier": want, "changed": false}));
    }
    if tier(&st) == want {
        return Ok(json!({"ok": true, "installed": true, "turnTier": want, "changed": false}));
    }
    if passthrough && !env.ops.passthrough_supported() {
        return Err(no(
            pin::SEW_PASSTHROUGH_UNSUPPORTED,
            409,
            format!("this Sew has no passthrough TURN tier (needs Sew {} or later): TURN stays on TCP 5349", pin::PASSTHROUGH_MIN_VERSION),
        ));
    }
    sup::helper_check(env)?;
    let _l = sup::begin(env)?;
    let (host, chain, key_pem) = held_material(env, &st)?;
    let mut body = json!({
        "turn_tls": want,
        "turn_domain": host,
        "turn_cert_pem": chain,
        "turn_key_pem": key_pem,
    });
    if !passthrough {
        body["turn_advertise_port"] = json!(SEW_TURN_TLS_PORT);
    }
    sup::sew_run(env, &["media", "reconfigure", "restart-now"], Some(body.to_string().as_bytes()))
        .map_err(|e| no("media_reconfigure_failed", 500, e))?;
    update(env, |s| s.turn_tier = Some(want.to_string()));
    sup::changed("voice-tier");
    Ok(json!({
        "ok": true,
        "installed": true,
        "turnTier": want,
        "changed": true,
        "turnTlsPort": tier_port(want),
        "restarted": [UNIT_MEDIA],
        "note": "sew-media restarted: calls in progress dropped",
    }))
}

// ── renewal ──────────────────────────────────────────────────────────────

fn backoff(failures: u32) -> i64 {
    (RETRY_MIN_SECS << failures.min(10).saturating_sub(1)).clamp(RETRY_MIN_SECS, RETRY_MAX_SECS)
}

/// Is a renewal due now? (inside the window or no cert, and the backoff
/// passed, or an `ip_unverified` wait saw a new frpc Login).
pub fn renew_due(env: &Env, st: &VoiceState) -> bool {
    if !st.installed {
        return false;
    }
    let now = env.ops.now();
    let in_window = st.cert_not_after.is_none_or(|t| t - now < RENEW_WINDOW_SECS);
    if !in_window {
        return false;
    }
    if st.logins_at_failure.is_some_and(|n| env.ops.frpc_logins() > n) {
        return true;
    }
    st.next_try_at.is_none_or(|t| now >= t)
}

/// Renew the TURN cert with the same key (`PUT /turn/dns` first: a new box
/// IP moves the record and Sew's node IP), then `sew media turn-cert
/// --when-idle`. `force` ignores the window and the backoff.
pub fn renew(env: &Env, force: bool) -> R<Value> {
    let st0 = load(env);
    if !st0.installed {
        return Err(no("voice_not_installed", 409, "voice is not installed: k2 hostchat media install"));
    }
    if !force && !renew_due(env, &st0) {
        return Ok(json!({"ok": true, "renewed": false, "certNotAfter": st0.cert_not_after}));
    }
    sup::helper_check(env)?;
    let _l = sup::begin(env)?;
    let label = env.ops.box_label().ok_or_else(|| no("no_connect_label", 409, "no K2 Connect label"))?;
    let mut st = load(env);
    let manual = (st.node_ip_source.as_deref() == Some("manual")).then(|| st.node_ip.clone()).flatten();
    let res = (|| -> R<Value> {
        let (chain, key_pem, dns) = issue(env, &label, &mut st, manual.as_deref())?;
        let mut moved = None;
        if manual.is_none() {
            let ip = node_ip_of(&dns);
            if st.node_ip.as_deref() != Some(ip.as_str()) {
                let body = json!({"node_ip": ip});
                sup::sew_run(env, &["media", "reconfigure", "restart-now"], Some(body.to_string().as_bytes()))
                    .map_err(|e| no("media_reconfigure_failed", 500, e))?;
                moved = Some(ip.clone());
                st.node_ip = Some(ip);
            }
        }
        let body = json!({"cert_pem": chain, "key_pem": key_pem});
        sup::sew_run(env, &["media", "turn-cert"], Some(body.to_string().as_bytes())).map_err(|e| no("turn_cert_failed", 500, e))?;
        Ok(json!({"ok": true, "renewed": true, "certNotAfter": st.cert_not_after, "nodeIpMoved": moved}))
    })();
    match &res {
        Ok(_) => {
            st.failures = 0;
            st.next_try_at = None;
            st.logins_at_failure = None;
            st.last_error = None;
            save(env, &mut st);
            sup::changed("voice-cert-renewed");
        }
        Err(f) => {
            st.failures = st.failures.saturating_add(1);
            st.next_try_at = Some(env.ops.now() + backoff(st.failures));
            st.logins_at_failure = (f.code == "turn_ip_unverified").then(|| env.ops.frpc_logins());
            st.last_error = Some(format!("renew: {}: {}", f.code, f.hint));
            save(env, &mut st);
        }
    }
    res
}

/// One health-loop tick: renew when due. Never fights a lifecycle step.
pub fn tick(env: &Env) {
    let st = load(env);
    if !renew_due(env, &st) || env.latch.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Err(f) = renew(env, false) {
        k2_core::log_debug!("[sew] voice renew: {}", f.code);
    }
}

// ── probe, uninstall ─────────────────────────────────────────────────────

/// Record an outside probe (a person or an agent tested a call from off
/// the box): `sew media probe-record … --from k2`.
pub fn probe_record(env: &Env, udp: bool, tcp: bool, tls: bool) -> R<Value> {
    if !load(env).installed {
        return Err(no("voice_not_installed", 409, "voice is not installed: k2 hostchat media install"));
    }
    sup::helper_check(env)?;
    let r = |b: bool| if b { "ok" } else { "fail" };
    sup::sew_run(env, &["media", "probe-record", r(udp), r(tcp), r(tls)], None).map_err(|e| no("probe_record_failed", 500, e))?;
    let now = env.ops.now();
    // The config the probe tested: a later node IP or tier change makes it
    // stale (Sew's doctor: a record must match the current config).
    let st = load(env);
    let (ip, t) = (st.node_ip.clone(), tier(&st).to_string());
    let port = tier_port(&t);
    update(env, |s| {
        s.probe = Some(json!({"udp": r(udp), "tcp": r(tcp), "turnTls": r(tls), "at": now, "nodeIp": ip, "tier": t, "turnTlsPort": port}))
    });
    sup::changed("voice-probe");
    let mut out = json!({"ok": true, "probe": {"udp": r(udp), "tcp": r(tcp), "turnTls": r(tls), "turnTlsPort": port}});
    if !(udp && tcp && tls) {
        out["alarm"] = json!(ALARM_PROBE_FAILED);
        out["hint"] = json!(probe_fail_hint(udp, tcp, tls, port));
    }
    Ok(out)
}

/// A fresh outside probe recorded a failure: degraded (S49 §17.1 ask 13).
pub const ALARM_PROBE_FAILED: &str = "k2_voice_probe_failed";
/// An outside probe is current for this long (Sew's doctor: 30 days).
pub const PROBE_FRESH_SECS: i64 = 30 * 86_400;

fn probe_fail_hint(udp: bool, tcp: bool, tls: bool, port: u16) -> String {
    let mut legs = Vec::new();
    if !udp {
        legs.push(format!("UDP {SEW_MEDIA_UDP_PORT}"));
    }
    if !tcp {
        legs.push(format!("TCP {SEW_MEDIA_TCP_PORT}"));
    }
    if !tls {
        legs.push(format!("TURN-TLS on TCP {port}"));
    }
    format!(
        "from outside, {} did not answer: open it in the host AND provider firewalls (K2 opens none), then test again \
         and record it with k2 hostchat media probe",
        legs.join(", ")
    )
}

/// How the recorded outside probe reads now: `None` (never probed),
/// `stale` (older than 30 days, or the node IP / TURN tier changed since),
/// `failed` (current, a leg failed) or `ok`.
pub fn probe_verdict(st: &VoiceState, now: i64) -> Option<&'static str> {
    let p = st.probe.as_ref()?;
    let at = p.get("at").and_then(Value::as_i64).unwrap_or(0);
    let moved = |k: &str, cur: Option<&str>| p.get(k).and_then(Value::as_str).is_some_and(|v| Some(v) != cur);
    if now - at > PROBE_FRESH_SECS || moved("nodeIp", st.node_ip.as_deref()) || moved("tier", Some(tier(st))) {
        return Some("stale");
    }
    let leg_ok = |k: &str| p.get(k).and_then(Value::as_str) == Some("ok");
    Some(if leg_ok("udp") && leg_ok("tcp") && leg_ok("turnTls") { "ok" } else { "failed" })
}

/// Sew's install participant cap for a K2 box (`sew media capacity
/// --profile shared`, S49 §7.3 / rev 88 D8), mirrored so the preflight can
/// say it before anything is installed: `sew-media` `MemoryMax` = 8% of RAM
/// rounded down to 64 MiB, clamped to 768 MiB … 3 GiB; cap = min(500,
/// floor10((0.9 × M − 256 MiB) / 2.0 MiB)). Sew's install writes the
/// authoritative number. Returns `(memory_max_mib, cap)`.
pub fn call_capacity(total_bytes: u64) -> (u64, u64) {
    let ram_mib = total_bytes / (1024 * 1024);
    let m = (ram_mib * 8 / 100 / 64 * 64).clamp(768, 3 * 1024);
    let usable_tenths = (9 * m).saturating_sub(256 * 10);
    let n = usable_tenths / 20;
    (m, (n / 10 * 10).min(500))
}

/// Remove voice: `sew media uninstall -y`, the TURN DNS record, the nested
/// label, the vault key and the record. Chat keeps running.
///
/// The person already confirmed (the route needs `confirm: true`); the
/// root helper passes Sew's `-y` (0.46.3: Sew asks "Uninstall voice? Live
/// calls end." and, with no terminal to ask on, refused). Idempotent: when
/// Sew (or the helper) says Sew or voice is not installed, K2's own voice
/// records are still cleaned up, so a partly removed box can always be
/// finished.
pub fn uninstall(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(no("unsupported", 409, "Chat / Voice Hosting runs on Linux boxes only"));
    }
    let st = load(env);
    if !st.installed && st.turn_key_ref.is_none() && st.progress.is_none() {
        return Ok(json!({"ok": true, "state": "not-installed", "alreadyRemoved": true}));
    }
    sup::helper_check(env)?;
    let _l = sup::begin(env)?;
    let mut warnings = Vec::new();
    let mut engine = "removed";
    if let Err(e) = sup::sew_run(env, &["media", "uninstall"], None) {
        if sew_needs_confirm(&e) {
            // A root helper from before 0.46.3 runs Sew without `-y`.
            return Err(no(
                "helper_outdated",
                409,
                format!(
                    "this root helper runs Sew's voice uninstall without -y, so Sew asked for a terminal: update the \
                     helper (k2 root-helper update), then run k2 hostchat media uninstall --yes again ({e})"
                ),
            ));
        }
        if !media_not_installed(&e) {
            return Err(no("media_uninstall_failed", 500, e));
        }
        engine = "already-gone";
        warnings.push(format!("Sew had no voice engine to remove ({e}); K2's own voice records were cleaned up"));
    }
    warnings.extend(forget(env, &st));
    sup::changed("voice-removed");
    Ok(json!({"ok": true, "state": "not-installed", "engine": engine, "warnings": warnings}))
}

/// Sew's (or the helper's) words for "nothing to uninstall": the Sew
/// binary is gone (`sew is not installed`, the helper), Sew's layout is
/// gone (`Sew is not installed (… missing)`, Sew's preflight), or voice is
/// (`voice is not installed`).
pub fn media_not_installed(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("sew is not installed") || e.contains("voice is not installed")
}

/// Sew's own confirm refusing for want of a terminal (no `-y` passed).
fn sew_needs_confirm(err: &str) -> bool {
    err.contains("needs confirmation: pass -y")
}

/// Drop everything K2 holds for voice (DNS record, nested label, vault
/// key, record). Best effort; returns what could not be undone.
pub fn forget(env: &Env, st: &VoiceState) -> Vec<String> {
    let mut w = Vec::new();
    if let Some(l) = st.label.clone().or_else(|| env.ops.box_label()) {
        if st.turn_host.is_some() {
            if let Err(e) = env.ops.cp("DELETE", &format!("/turn/dns/{l}"), None) {
                w.push(format!("TURN DNS record not removed: {e}"));
            }
        }
    }
    if st.voice_label {
        if let Err(e) = env.ops.nested_label(VOICE_LABEL, None) {
            w.push(format!("nested voice label not removed: {e}"));
        }
    }
    if let Some(r) = &st.turn_key_ref {
        if let Err(e) = env.secrets.delete(r) {
            w.push(format!("TURN key not deleted from the vault: {e}"));
        }
    }
    if let Err(e) = env.ops.voice_save(None) {
        w.push(e);
    }
    // front443 routes the TURN name from the voice record: rebuild its
    // table now that the record is gone (voice uninstall and purge), not at
    // the next voice install.
    env.ops.front_routes_changed();
    w
}

// ── status ───────────────────────────────────────────────────────────────

/// Sew's own voice alarms that describe a setup choice, never `degraded`.
pub const SETUP_ALARMS: &[&str] = &["port_443_unavailable", "port_443_unprobed"];

/// `status.voice` plus the alarms (→ degraded) and setup warnings it adds
/// to the chat status. `live` re-checks the served TURN leaf.
pub fn status(env: &Env, live: bool) -> (Value, Vec<String>, Vec<Value>) {
    let st = load(env);
    let mut alarms = Vec::new();
    let mut warnings = Vec::new();
    let now = env.ops.now();
    if !st.installed {
        let state = match st.progress.as_ref().and_then(|p| p.get("result")).and_then(Value::as_str) {
            Some("running") => "installing",
            Some("error") => "error",
            _ => "not-installed",
        };
        let v = json!({
            "state": state, "publicUrl": st.public_url, "progress": st.progress, "lastError": st.last_error,
            "ports": {"udp": SEW_MEDIA_UDP_PORT, "tcp": SEW_MEDIA_TCP_PORT, "turnTls": SEW_TURN_TLS_PORT},
        });
        return (v, alarms, warnings);
    }
    let engine = sup::sew_run(env, &["media", "status"], None).ok().and_then(|v| v.get("sew").cloned());
    let unit = env.ops.unit(UNIT_MEDIA);
    let eng_state = engine.as_ref().and_then(|e| e.get("state")).and_then(Value::as_str).map(str::to_string);
    let mut state = if !unit.is_active() {
        alarms.push("voice_engine_down".to_string());
        if unit.active == "failed" { "error" } else { "stopped" }.to_string()
    } else {
        eng_state.clone().unwrap_or_else(|| "running".into())
    };
    for a in engine.as_ref().and_then(|e| e.get("alarms")).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if SETUP_ALARMS.contains(&a) {
            warnings.push(json!({"code": a, "alarm": a, "message": voice_warning(a)}));
        } else {
            alarms.push(if a.starts_with("voice_") { a.to_string() } else { format!("voice_{a}") });
        }
    }
    match st.cert_not_after {
        None => alarms.push("k2_turn_cert_missing".into()),
        Some(t) if t <= now => alarms.push("k2_turn_cert_expired".into()),
        Some(t) if t - now < EXPIRING_SECS => alarms.push("k2_turn_cert_expiring".into()),
        _ => {}
    }
    let mut served = st.served.clone();
    if live {
        if let (Some(ip), Some(host)) = (st.node_ip.as_deref().and_then(|n| n.split(',').next()), st.turn_host.as_deref()) {
            let s = served_check(env, ip, host, st.cert_leaf_sha256.as_deref(), tier_port(tier(&st)));
            update(env, |v| {
                v.served = Some(s.clone());
                v.served_at = Some(now);
            });
            served = Some(s);
        }
    }
    if served.as_deref() == Some("mismatch") {
        alarms.push("k2_turn_tls_mismatch".into());
    }
    match probe_verdict(&st, now) {
        None => warnings.push(json!({"code": "voice_unprobed", "message": "no outside probe yet: test a call from another network, then k2 hostchat media probe"})),
        Some("stale") => warnings.push(json!({
            "code": "voice_probe_stale",
            "message": "the last outside probe is older than 30 days or tested another node IP or TURN tier: test a call from another network again, then k2 hostchat media probe",
        })),
        Some("failed") => alarms.push(ALARM_PROBE_FAILED.into()),
        _ => {}
    }
    if state == "running" && !alarms.is_empty() {
        state = "degraded".into();
    }
    if !env.ops.unit(UNIT_DB).is_active() || !env.ops.unit(UNIT_SEW).is_active() {
        state = "stopped".into();
    }
    let v = json!({
        "state": state,
        "publicUrl": st.public_url,
        "turnHost": st.turn_host,
        "nodeIp": st.node_ip,
        "nodeIpSource": st.node_ip_source,
        "cert": {"notAfter": st.cert_not_after, "daysLeft": st.cert_not_after.map(|t| (t - now) / 86_400),
                 "leafSha256": st.cert_leaf_sha256, "issuedAt": st.cert_issued_at},
        "turnTier": tier(&st),
        "turnTls": {"port": tier_port(tier(&st)), "served": served, "checkedAt": st.served_at},
        "voiceLabel": st.voice_label,
        "ports": {"udp": SEW_MEDIA_UDP_PORT, "tcp": SEW_MEDIA_TCP_PORT, "turnTls": tier_port(tier(&st))},
        "probe": st.probe,
        "probeVerdict": probe_verdict(&st, now),
        "unit": unit.json(),
        // Sew 0.3.1 `sew media status --json` turn {tier, ice_urls, source}.
        "turnEngine": super::health::media_turn(engine.as_ref()),
        "engine": engine,
        "alarms": alarms,
        "renewal": {"lastError": st.last_error, "nextTryAt": st.next_try_at, "failures": st.failures,
                    "waitingForTunnelLogin": st.logins_at_failure.is_some()},
        "warnings": st.warnings,
    });
    (v, alarms, warnings)
}

/// `status.voice` while chat itself is not up: K2's record only (no engine
/// call, no extra alarms; the chat state already says why).
pub fn status_offline(env: &Env) -> (Value, Vec<String>, Vec<Value>) {
    let st = load(env);
    let state = if st.installed { "stopped" } else { "not-installed" };
    let v = json!({
        "state": state, "publicUrl": st.public_url, "turnHost": st.turn_host, "nodeIp": st.node_ip,
        "cert": {"notAfter": st.cert_not_after}, "progress": st.progress, "lastError": st.last_error,
    });
    (v, Vec::new(), Vec::new())
}

fn voice_warning(code: &str) -> &'static str {
    match code {
        "port_443_unavailable" => "Calls reach the relay on TCP 5349: people on networks that allow only port 443 cannot join calls.",
        "port_443_unprobed" => "TURN on 443 has not been probed from outside.",
        _ => "",
    }
}

// Server-only feature (Linux boxes): Sew is hosted on a box through the root
// helper (stage, units, media); not shipped on personal Windows machines.
#[cfg(all(test, unix))]
#[path = "voice_tests.rs"]
mod tests;
