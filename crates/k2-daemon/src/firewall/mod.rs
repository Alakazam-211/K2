//! The public-ports view and the owner's firewall options (0.46.6,
//! `prd-custom-domains-path-a-0466-v1` §4–§5; vs-live PA-A4…A7, A12…A15).
//!
//! **The view** (`GET /cli/firewall/ports`, `k2 firewall ports --json`) is
//! the daemon's truth for "the ports this box serves publicly", computed
//! from K2's own config and state ([`Inputs`]), **never by scanning**.
//! K2 Web's E0 collector reads it (over SSH, as the daemon user) and pushes
//! `public` to the provider firewall; the owner sees the same list in
//! Settings → Network → Custom Domains → Firewall and `k2 firewall`.
//!
//! The contract (PRD §4.1):
//! 1. computed, never scanned;
//! 2. SSH is always in `public` (narrowed, never removed);
//! 3. a listener the collector observes in neither list is a **hold**,
//!    never an auto-allow;
//! 4. `knownNotPublic`: ports K2 binds on a non-loopback address that must
//!    stay closed upstream (expected, blocked, no hold; PA-A6);
//! 5. a `public` port with no listener is fine;
//! 6. no view (pre-0.46.6) or an unparsable one: no change upstream;
//! 7. `version` bumps only on a breaking change.
//!
//! Sources, in priority order (one row per port; [`k2_core::firewall::merge`]):
//!
//! | source | public when | ports |
//! |---|---|---|
//! | `ssh` | always | the owner's (default 22/tcp), optionally narrowed |
//! | `front443` | the :443 front is on or serving | 443/tcp (custom App names ride it; no :80 for Path A in 0.46.6, PA-A14) |
//! | `skin-direct` | Skin Direct's Caddy holds :443 itself, or sits behind the front | 443/tcp (when it holds it), 80/tcp; the 38472 fallback while it is the live listen (PA-A7) |
//! | `hostmail` | hosted mail is set up | 25, 465, 587, 993, **143** (IMAP STARTTLS, PA-A4), 80 (ACME HTTP-01); 443 when Stalwart holds it itself; not public: 995, 4190, 8080 |
//! | `sew-voice` | Sew voice is installed | 7881/tcp, 7882/udp, 5349/tcp on the `direct` TURN tier (PA-A5) |
//! | `daemon` (not public) | LAN listen is on | the daemon's HTTP port |
//! | `owner` | the owner opened it | as set |

pub mod routes;

#[cfg(test)]
mod tests;

use serde::Serialize;
use serde_json::{json, Value};

use k2_core::firewall::{merge, FirewallConfig, PortEntry, Proto, UpstreamAck, VIEW_VERSION};

/// Everything the view is computed from (gathered by [`gather`]; tests
/// build it by hand).
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub cfg: FirewallConfig,
    /// The :443 front: the owner turned it on, and/or it serves now.
    pub front_enabled: bool,
    pub front_serving: bool,
    /// Skin Direct's live Caddy listen (`:443`, `0.0.0.0:38472`,
    /// `127.0.0.1:38473`), when Direct mode runs.
    pub skin_direct_listen: Option<String>,
    /// Hosted mail is set up (running, degraded or stopped).
    pub mail_on: bool,
    /// Stalwart's HTTPS sits behind the front (`k2 front enable --mail`).
    pub mail_behind_front: bool,
    /// Sew voice installed, and on the `direct` TURN tier.
    pub voice_installed: bool,
    pub voice_tier_direct: bool,
    /// The daemon's HTTP port when it is bound on 0.0.0.0 (LAN listen).
    pub lan_port: Option<u16>,
    pub label: Option<String>,
    pub public_ips: Vec<String>,
    pub behind_nat: Option<bool>,
    pub os: &'static str,
    pub daemon: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BoxInfo {
    pub label: Option<String>,
    pub public_ips: Vec<String>,
    pub behind_nat: Option<bool>,
    pub os: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub hash: String,
    pub provider: String,
    pub applied_at: i64,
    pub recorded_at: i64,
    /// The applied hash is this view's (nothing changed since).
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    /// `k2` (K2 Web applies it at the provider) or `owner` (nobody does).
    pub managed_by: String,
    pub applied: Option<Applied>,
}

/// The contract document (PRD §4.1).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub version: u32,
    pub generated_at: i64,
    pub hash: String,
    pub daemon: String,
    #[serde(rename = "box")]
    pub box_info: BoxInfo,
    pub public: Vec<PortEntry>,
    pub known_not_public: Vec<PortEntry>,
    pub upstream: Upstream,
}

const ANY: &[String] = &[];

/// Pure: the view from its inputs.
pub fn compute(i: &Inputs) -> View {
    use Proto::{Tcp, Udp};
    let mut public: Vec<PortEntry> = Vec::new();
    let mut closed: Vec<PortEntry> = Vec::new();

    // SSH first, always (the lockout guard).
    public.push(PortEntry::public(i.cfg.ssh.port, Tcp, "ssh", &i.cfg.ssh.allow_from, "SSH (owner setting)"));

    let front = i.front_enabled || i.front_serving;
    if front {
        public.push(PortEntry::public(443, Tcp, "front443", ANY, "the :443 front: Apps on their own domains, mail web, TURN"));
    }

    match i.skin_direct_listen.as_deref() {
        Some(":443") => {
            public.push(PortEntry::public(443, Tcp, "skin-direct", ANY, "Skin Direct's HTTPS"));
            public.push(PortEntry::public(80, Tcp, "skin-direct", ANY, "Skin Direct: ACME HTTP-01 and the https redirect"));
        }
        Some(l) if l.starts_with("0.0.0.0:") => {
            if let Some(p) = l.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()) {
                public.push(PortEntry::public(p, Tcp, "skin-direct", ANY, "Skin Direct fallback (443 was busy)"));
            }
        }
        Some(_) if front => {
            public.push(PortEntry::public(80, Tcp, "skin-direct", ANY, "Skin Direct: ACME HTTP-01 and the https redirect"));
        }
        _ => {}
    }

    if i.mail_on {
        for (p, why) in [
            (25, "hosted mail: SMTP (MX)"),
            (465, "hosted mail: SMTP submission (implicit TLS)"),
            (587, "hosted mail: SMTP submission (STARTTLS)"),
            (993, "hosted mail: IMAP (implicit TLS)"),
            (143, "hosted mail: IMAP (STARTTLS), handed to mail clients"),
            (80, "hosted mail: ACME HTTP-01 (binds while a certificate is issued)"),
        ] {
            public.push(PortEntry::public(p, Tcp, "hostmail", ANY, why));
        }
        if !i.mail_behind_front && !front {
            public.push(PortEntry::public(443, Tcp, "hostmail", ANY, "hosted mail: webmail and mail-client setup (Stalwart's HTTPS)"));
        }
        closed.push(PortEntry::closed(995, Tcp, "hostmail", "POP3S: never public"));
        closed.push(PortEntry::closed(4190, Tcp, "hostmail", "ManageSieve: never public"));
        closed.push(PortEntry::closed(8080, Tcp, "hostmail", "Stalwart's plain HTTP: never public"));
    }

    if i.voice_installed {
        for (_, addr, port, udp) in crate::sew::voice::PORTS {
            if addr != "0.0.0.0" {
                continue;
            }
            if port == k2_root_proto::sew::SEW_TURN_TLS_PORT && !i.voice_tier_direct {
                continue; // TURN rides :443 behind the front (passthrough tier)
            }
            let why = if udp { "Chat / Voice Hosting: media" } else if port == k2_root_proto::sew::SEW_TURN_TLS_PORT { "Chat / Voice Hosting: TURN over TLS" } else { "Chat / Voice Hosting: ICE over TCP" };
            public.push(PortEntry::public(port, if udp { Udp } else { Tcp }, "sew-voice", ANY, why));
        }
    }

    if let Some(p) = i.lan_port {
        closed.push(PortEntry::closed(p, Tcp, "daemon", "K2 API (LAN listen on: LAN only)"));
    }

    for o in &i.cfg.open {
        public.push(PortEntry::public(o.port, o.proto, "owner", &o.allow_from, format!("owner: {}", o.label)));
    }

    let public = merge(public);
    // A port that is public is never also "known not public".
    let closed: Vec<PortEntry> =
        merge(closed).into_iter().filter(|c| !public.iter().any(|p| p.port == c.port && p.proto == c.proto)).collect();
    let hash = k2_core::firewall::hash(&public);
    let applied = i.cfg.upstream.as_ref().map(|a: &UpstreamAck| Applied {
        hash: a.hash.clone(),
        provider: a.provider.clone(),
        applied_at: a.applied_at,
        recorded_at: a.recorded_at,
        current: a.hash == hash,
    });
    View {
        version: VIEW_VERSION,
        generated_at: i.now,
        hash,
        daemon: i.daemon.clone(),
        box_info: BoxInfo {
            label: i.label.clone(),
            public_ips: i.public_ips.clone(),
            behind_nat: i.behind_nat,
            os: i.os.to_string(),
        },
        public,
        known_not_public: closed,
        upstream: Upstream { managed_by: i.cfg.managed_by().to_string(), applied },
    }
}

fn os_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}

/// The box's K2 label (`<sub>` of `<sub>.k2.dev`).
fn box_label() -> Option<String> {
    let primary = k2_core::tunnel::subdomains::current().primary.trim().to_ascii_lowercase();
    let label = if primary.is_empty() { k2_core::tunnel::config::load().ok()?.subdomain.trim().to_ascii_lowercase() } else { primary };
    let label = label.strip_suffix(".k2.dev").map(str::to_string).unwrap_or(label);
    (!label.is_empty()).then_some(label)
}

/// Gather the inputs from the daemon's live state. Blocking. `Err` only
/// when the owner's firewall file can't be read (never a silent default:
/// the view would drop their SSH narrowing or ports).
pub fn gather(daemon_port: u16) -> Result<Inputs, String> {
    let cfg = k2_core::firewall::load_checked()?;
    let front_cfg = k2_core::front443::load();
    let mail_status = if k2_core::db::try_shared().is_some() { crate::mail::supervisor::row_field("status") } else { None };
    let mail_on = matches!(mail_status.as_deref(), Some("running" | "degraded" | "stopped"));
    let voice = crate::sew::voice::load(&crate::sew::supervisor::live());
    let skin_direct_listen = k2_core::skin_door::live_listen().filter(|l| l != &format!("127.0.0.1:{}", k2_core::skin_door::LOOPBACK_PORT));
    Ok(Inputs {
        cfg,
        front_enabled: front_cfg.enabled && cfg!(target_os = "linux"),
        front_serving: crate::front443::serving(),
        skin_direct_listen,
        mail_on,
        mail_behind_front: front_cfg.mail_behind_front(),
        voice_installed: voice.installed,
        voice_tier_direct: crate::sew::voice::tier(&voice) != crate::sew::voice::TIER_PASSTHROUGH,
        lan_port: k2_core::listen::lan_bound().then_some(daemon_port),
        label: box_label(),
        public_ips: crate::apps::custom_domain_flow::box_ips().iter().map(|ip| ip.to_string()).collect(),
        behind_nat: crate::apps::custom_domain_flow::behind_nat(),
        os: os_name(),
        daemon: env!("CARGO_PKG_VERSION").to_string(),
        now: chrono::Utc::now().timestamp(),
    })
}

/// `GET /cli/firewall/ports`.
pub fn view(daemon_port: u16) -> Result<View, String> {
    gather(daemon_port).map(|i| compute(&i))
}

/// `GET /cli/firewall`: the view, the guard, who applies it, the :443 front
/// and passthrough names.
pub fn status_json(daemon_port: u16) -> Result<Value, String> {
    let v = view(daemon_port)?;
    let cfg = k2_core::firewall::load_checked()?;
    let applies = if v.upstream.managed_by == "k2" {
        "K2 applies this list at your provider's firewall."
    } else {
        "Nobody applies this list for you: copy the rules (k2 firewall rules --format nft|ufw)."
    };
    Ok(json!({
        "ok": true,
        "view": v,
        "ssh": cfg.ssh,
        "open": cfg.open,
        "guard": {
            "settings": cfg.guard,
            "counters": crate::apps::guard::counters_json(),
        },
        "applies": applies,
        "front": {
            "enabled": k2_core::front443::load().enabled,
            "serving": crate::front443::serving(),
            "passthrough": k2_core::front443::load_passthrough_checked().unwrap_or_default(),
        },
    }))
}

/// The text `k2 firewall` prints (`?format=text`).
pub fn status_text(s: &Value) -> String {
    let v = &s["view"];
    let mut out = String::new();
    out.push_str(&format!("firewall view  {} (v{}, {})\n", v["hash"].as_str().unwrap_or("?"), v["version"], v["daemon"].as_str().unwrap_or("?")));
    if let Some(l) = v["box"]["label"].as_str() {
        out.push_str(&format!("box            {l}.k2.dev"));
        if let Some(ips) = v["box"]["publicIps"].as_array().filter(|a| !a.is_empty()) {
            out.push_str(&format!("  ({})", ips.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
        }
        out.push('\n');
    }
    if v["box"]["behindNat"] == true {
        out.push_str("note           this box is behind a home or office router: custom domains use K2 Connect (0.46.7)\n");
    }
    out.push_str("public ports\n");
    for r in v["public"].as_array().into_iter().flatten() {
        let from = r["allowFrom"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        out.push_str(&format!(
            "  {:>5}/{:<3}  {:<11}  from {:<20}  {}\n",
            r["port"],
            r["proto"].as_str().unwrap_or(""),
            r["source"].as_str().unwrap_or(""),
            from,
            r["why"].as_str().unwrap_or("")
        ));
    }
    let closed = v["knownNotPublic"].as_array().cloned().unwrap_or_default();
    if !closed.is_empty() {
        out.push_str("K2 listens, keep closed upstream\n");
        for r in closed {
            out.push_str(&format!("  {:>5}/{:<3}  {:<11}  {}\n", r["port"], r["proto"].as_str().unwrap_or(""), r["source"].as_str().unwrap_or(""), r["why"].as_str().unwrap_or("")));
        }
    }
    out.push_str(&format!("applied by     {}\n", s["applies"].as_str().unwrap_or("")));
    if let Some(a) = v["upstream"]["applied"].as_object() {
        out.push_str(&format!(
            "last applied   {} at {} ({})\n",
            a.get("provider").and_then(Value::as_str).unwrap_or("?"),
            a.get("appliedAt").and_then(Value::as_i64).unwrap_or(0),
            if a.get("current") == Some(&Value::Bool(true)) { "this list" } else { "an older list: the next pass applies this one" }
        ));
    }
    let g = &s["guard"];
    let st = &g["settings"];
    out.push_str(&format!(
        "App guard      {}: {} connections per visitor, {} writes per visitor per App a minute\n",
        if st["enabled"] == true { "on" } else { "off" },
        st["connsPerIp"],
        st["writesPerMin"]
    ));
    if let Some(apps) = st["apps"].as_object().filter(|a| !a.is_empty()) {
        for (app, o) in apps {
            let on = match o["enabled"].as_bool() {
                Some(true) => "on",
                Some(false) => "off",
                None => "box default",
            };
            let w = o["writesPerMin"].as_u64().map(|n| format!(", {n} writes a minute")).unwrap_or_default();
            out.push_str(&format!("  {app}: {on}{w}\n"));
        }
    }
    let c = &g["counters"];
    out.push_str(&format!("guard counters refused connections {} · limited writes {}\n", c["perIpRefused"], c["guardLimited"]));
    if let Some(p) = s["front"]["passthrough"].as_array().filter(|a| !a.is_empty()) {
        out.push_str("passthrough on :443\n");
        for r in p {
            out.push_str(&format!("  {} -> {}{}\n", r["host"].as_str().unwrap_or("?"), r["to"].as_str().unwrap_or("?"), if r["proxyV2"] == true { " (PROXY v2)" } else { "" }));
        }
    }
    out
}
