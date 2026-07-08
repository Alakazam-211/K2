//! S1 preflight (PRD §5.1) — the read-only checklist that runs before
//! anything installs, rendered MiaB-style (✓/✖/? + prose).
//!
//! Pure logic in [`run_preflight`] over an injected [`PreflightEnv`] so
//! every branch unit-tests on macOS with ZERO side effects — tests
//! never bind ports (let alone <1024), never dial the network. The
//! real environment ([`RealPreflightEnv`]) probes with connect
//! attempts + `ss`/`getent`/`/proc` reads; it never binds privileged
//! ports either (a successful CONNECT means taken, refused means
//! free — no root needed, nothing held open).
//!
//! Non-Linux short-circuit (D3): the OS check hard-fails FIRST and the
//! remaining checks are marked skipped WITHOUT touching the env — so
//! the Mac "example page" preflight route is network-silent, and so
//! are the route tests.
//!
//! NOTE (public IP): the daemon has no existing raw-public-IP
//! mechanism — federation/K2 Connect identify boxes by tunnel-assigned
//! SUBDOMAIN (`k2_core::tunnel`), never by WAN IP. Mail genuinely
//! needs the IP (rDNS/PTR, SPF), so the real env asks a plain
//! what-is-my-ip endpoint (checkip.amazonaws.com, ipify fallback) —
//! the same approach the S6 doctor will need.

use std::time::Duration;

/// One row of the checklist. `status` maps to the UI glyphs:
/// pass = ✓, warn = ?, fail = ✖, info = neutral prose (e.g. the
/// port-plan row), skipped = not evaluated (non-Linux short-circuit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Info,
    Skipped,
}

impl CheckStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Info => "info",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreflightCheck {
    pub id: &'static str,
    pub label: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

/// The full report. `ok` = no `Fail` rows (warns don't block, PRD
/// §5.1); `port_plan` is the §5.3 detect-and-adapt selection —
/// `tls-alpn` when :443 is free (plan A), else `http-01` (plan C, the
/// no-assumptions default; the wizard upgrades to `dns-01`/plan B if
/// the owner supplies a DNS-API token in a later slice).
#[derive(Debug, Clone)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
    pub port_plan: Option<&'static str>,
    pub ok: bool,
}

impl PreflightReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": self.ok,
            "portPlan": self.port_plan,
            "checks": self.checks.iter().map(|c| serde_json::json!({
                "id": c.id,
                "label": c.label,
                "status": c.status.as_str(),
                "detail": c.detail,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Everything preflight observes about the box, injectable for tests
/// (the "port-checker trait" — tests NEVER probe real ports).
pub trait PreflightEnv {
    fn is_linux(&self) -> bool;
    /// Who (if anyone) listens on `port` locally. `Some(desc)` = taken
    /// (desc names the process when discoverable), `None` = free.
    fn port_listener(&self, port: u16) -> Option<String>;
    /// The box's public IPv4, if discoverable.
    fn public_ip(&self) -> Option<String>;
    /// Reverse-DNS of `ip`, if readable.
    fn rdns(&self, ip: &str) -> Option<String>;
    /// Outbound-:25 probe to an external MX (pre-mortem #6).
    /// `Ok(())` = connected; `Err(reason)` distinguishes timeout vs
    /// refused for the provider coaching.
    fn outbound_25(&self) -> Result<(), String>;
    fn disk_free_bytes(&self) -> Option<u64>;
    fn total_ram_bytes(&self) -> Option<u64>;
}

const MIN_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GB (soft)
const MIN_RAM_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB (soft)

/// Run the §5.1 checklist against `env`. Pure decision logic — every
/// observation comes through the trait.
pub fn run_preflight(env: &dyn PreflightEnv) -> PreflightReport {
    let mut checks = Vec::new();

    // OS gate first (D3) — a non-Linux daemon short-circuits the whole
    // list without touching the environment (network-silent on Mac).
    if !env.is_linux() {
        checks.push(PreflightCheck {
            id: "os",
            label: "Operating system is Linux",
            status: CheckStatus::Fail,
            detail: "Email server V1 runs on Linux deployments only; this page renders \
                     for example purposes on other systems."
                .into(),
        });
        for (id, label) in [
            ("mta", "No existing mail server on port 25"),
            ("port-465", "Port 465 available"),
            ("port-587", "Port 587 available"),
            ("port-443", "TLS / port plan"),
            ("public-ip", "Public IP discovered"),
            ("rdns", "Reverse DNS readable"),
            ("outbound-25", "Outbound port 25 reachable"),
            ("disk", "Disk space"),
            ("ram", "Memory"),
        ] {
            checks.push(PreflightCheck {
                id,
                label,
                status: CheckStatus::Skipped,
                detail: "skipped — not a Linux host".into(),
            });
        }
        return PreflightReport { checks, port_plan: None, ok: false };
    }
    checks.push(PreflightCheck {
        id: "os",
        label: "Operating system is Linux",
        status: CheckStatus::Pass,
        detail: String::new(),
    });

    // Existing MTA / SMTP ports (hard stops).
    match env.port_listener(25) {
        Some(who) => checks.push(PreflightCheck {
            id: "mta",
            label: "No existing mail server on port 25",
            status: CheckStatus::Fail,
            detail: format!(
                "this box already runs a mail server ({who} is listening on :25); \
                 K2 won't fight it"
            ),
        }),
        None => checks.push(PreflightCheck {
            id: "mta",
            label: "No existing mail server on port 25",
            status: CheckStatus::Pass,
            detail: String::new(),
        }),
    }
    for (id, label, port) in [
        ("port-465", "Port 465 available", 465u16),
        ("port-587", "Port 587 available", 587u16),
    ] {
        match env.port_listener(port) {
            Some(who) => checks.push(PreflightCheck {
                id,
                label,
                status: CheckStatus::Fail,
                detail: format!("{who} is listening on :{port}"),
            }),
            None => checks.push(PreflightCheck {
                id,
                label,
                status: CheckStatus::Pass,
                detail: String::new(),
            }),
        }
    }

    // :443 → port plan (§5.3; never a stop).
    let port_plan = match env.port_listener(443) {
        None => {
            checks.push(PreflightCheck {
                id: "port-443",
                label: "TLS / port plan",
                status: CheckStatus::Info,
                detail: ":443 is free — plan A: Stalwart binds :443, certificates via \
                         ACME TLS-ALPN-01 (zero-config)"
                    .into(),
            });
            "tls-alpn"
        }
        Some(who) => {
            checks.push(PreflightCheck {
                id: "port-443",
                label: "TLS / port plan",
                status: CheckStatus::Info,
                detail: format!(
                    ":443 is taken ({who}) — Stalwart's HTTPS moves to 127.0.0.1:8443 \
                     (never exposed off-box); certificates via HTTP-01 behind your \
                     existing proxy (plan C), upgradable to DNS-01 (plan B) with a DNS \
                     provider API token"
                ),
            });
            "http-01"
        }
    };

    // Public IP + rDNS (warn-only: NAT/CGNAT guidance).
    let ip = env.public_ip();
    match &ip {
        Some(ip) => checks.push(PreflightCheck {
            id: "public-ip",
            label: "Public IP discovered",
            status: CheckStatus::Pass,
            detail: ip.clone(),
        }),
        None => checks.push(PreflightCheck {
            id: "public-ip",
            label: "Public IP discovered",
            status: CheckStatus::Warn,
            detail: "could not discover a public IP — NAT/CGNAT likely; inbound mail \
                     cannot reach this box"
                .into(),
        }),
    }
    match ip.as_deref().map(|ip| (ip, env.rdns(ip))) {
        Some((_, Some(name))) => checks.push(PreflightCheck {
            id: "rdns",
            label: "Reverse DNS readable",
            status: CheckStatus::Pass,
            detail: name,
        }),
        Some((ip, None)) => checks.push(PreflightCheck {
            id: "rdns",
            label: "Reverse DNS readable",
            status: CheckStatus::Warn,
            detail: format!(
                "no PTR record for {ip} — set reverse DNS at your VPS provider \
                 (required for direct-send deliverability)"
            ),
        }),
        None => checks.push(PreflightCheck {
            id: "rdns",
            label: "Reverse DNS readable",
            status: CheckStatus::Skipped,
            detail: "skipped — no public IP to look up".into(),
        }),
    }

    // Outbound :25 (warn + coaching; direct-send gated off until pass).
    match env.outbound_25() {
        Ok(()) => checks.push(PreflightCheck {
            id: "outbound-25",
            label: "Outbound port 25 reachable",
            status: CheckStatus::Pass,
            detail: String::new(),
        }),
        Err(reason) => checks.push(PreflightCheck {
            id: "outbound-25",
            label: "Outbound port 25 reachable",
            status: CheckStatus::Warn,
            detail: format!(
                "{reason} — your provider likely blocks outbound SMTP; direct-send \
                 mode will stay unavailable until this passes (relay mode works \
                 regardless)"
            ),
        }),
    }

    // Disk / RAM (soft).
    match env.disk_free_bytes() {
        Some(free) if free >= MIN_DISK_BYTES => checks.push(PreflightCheck {
            id: "disk",
            label: "Disk space",
            status: CheckStatus::Pass,
            detail: format!("{:.1} GB free", free as f64 / 1e9),
        }),
        Some(free) => checks.push(PreflightCheck {
            id: "disk",
            label: "Disk space",
            status: CheckStatus::Warn,
            detail: format!(
                "only {:.1} GB free (recommend ≥ 2 GB for mail data)",
                free as f64 / 1e9
            ),
        }),
        None => checks.push(PreflightCheck {
            id: "disk",
            label: "Disk space",
            status: CheckStatus::Warn,
            detail: "could not determine free disk space".into(),
        }),
    }
    match env.total_ram_bytes() {
        Some(ram) if ram >= MIN_RAM_BYTES => checks.push(PreflightCheck {
            id: "ram",
            label: "Memory",
            status: CheckStatus::Pass,
            detail: format!("{:.1} GB total", ram as f64 / 1e9),
        }),
        Some(ram) => checks.push(PreflightCheck {
            id: "ram",
            label: "Memory",
            status: CheckStatus::Warn,
            detail: format!("only {:.1} GB RAM (recommend ≥ 1 GB)", ram as f64 / 1e9),
        }),
        None => checks.push(PreflightCheck {
            id: "ram",
            label: "Memory",
            status: CheckStatus::Warn,
            detail: "could not determine total memory".into(),
        }),
    }

    let ok = !checks.iter().any(|c| c.status == CheckStatus::Fail);
    PreflightReport { checks, port_plan: Some(port_plan), ok }
}

// ── The real environment ────────────────────────────────────────────────

/// Production observations. Constructed only at request time; probes
/// use short timeouts so the preflight route stays snappy.
pub struct RealPreflightEnv;

impl PreflightEnv for RealPreflightEnv {
    fn is_linux(&self) -> bool {
        super::supervisor::mail_supported()
    }

    fn port_listener(&self, port: u16) -> Option<String> {
        // `ss -ltnpH` names the holder when we're privileged; the
        // connect probe is the fallback truth for "is it taken".
        // Never binds anything.
        if let Ok(out) = std::process::Command::new("ss").args(["-ltnpH"]).output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(who) = parse_ss_listener(&text, port) {
                    return Some(who);
                }
            }
        }
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(400)) {
            Ok(_) => Some("an unidentified process".into()),
            Err(_) => None,
        }
    }

    fn public_ip(&self) -> Option<String> {
        for url in ["https://checkip.amazonaws.com", "https://api.ipify.org"] {
            let resp = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .ok()?
                .get(url)
                .send();
            if let Ok(resp) = resp {
                if let Ok(text) = resp.text() {
                    let text = text.trim().to_string();
                    if text.parse::<std::net::Ipv4Addr>().is_ok() {
                        return Some(text);
                    }
                }
            }
        }
        None
    }

    fn rdns(&self, ip: &str) -> Option<String> {
        // `getent hosts <ip>` does the PTR lookup through NSS on any
        // Linux box; column 2 is the canonical name.
        let out = std::process::Command::new("getent")
            .args(["hosts", ip])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .nth(1)
            .map(str::to_string)
    }

    fn outbound_25(&self) -> Result<(), String> {
        use std::net::ToSocketAddrs;
        // Deliberate, single connect to a major MX — a TCP handshake
        // only, no SMTP dialogue, no mail (pre-mortem #1 stays intact).
        let mut last = String::from("no MX host resolved");
        for host in ["aspmx.l.google.com:25", "gmail-smtp-in.l.google.com:25"] {
            match host.to_socket_addrs() {
                Ok(mut addrs) => {
                    if let Some(addr) = addrs.next() {
                        match std::net::TcpStream::connect_timeout(
                            &addr,
                            Duration::from_secs(5),
                        ) {
                            Ok(_) => return Ok(()),
                            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                                last = format!("connection to {host} timed out");
                            }
                            Err(e) => last = format!("connection to {host} failed: {e}"),
                        }
                    }
                }
                Err(e) => last = format!("resolve {host}: {e}"),
            }
        }
        Err(last)
    }

    fn disk_free_bytes(&self) -> Option<u64> {
        // Mail data lands under /var/lib (Stalwart's RocksDB store).
        k2_core::fs_commands::free_disk_space(std::path::Path::new("/var/lib"))
            .or_else(|| k2_core::fs_commands::free_disk_space(std::path::Path::new("/")))
    }

    fn total_ram_bytes(&self) -> Option<u64> {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_meminfo_total(&meminfo)
    }
}

/// Pure `ss -ltnpH` parser: the holder of `port`, if listed. Local
/// address is column 4 (`0.0.0.0:25`, `[::]:25`, `127.0.0.1:8080`);
/// the process blob (`users:(("exim4",pid=…))`) trails when running
/// privileged.
pub fn parse_ss_listener(ss_output: &str, port: u16) -> Option<String> {
    let want = format!(":{port}");
    for line in ss_output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        if !cols[3].ends_with(&want) {
            continue;
        }
        let proc_name = cols
            .iter()
            .find(|c| c.starts_with("users:(("))
            .and_then(|c| c.split('"').nth(1))
            .map(str::to_string);
        return Some(proc_name.unwrap_or_else(|| "an unidentified process".into()));
    }
    None
}

/// Pure `/proc/meminfo` parser: `MemTotal: <n> kB` → bytes.
pub fn parse_meminfo_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Configurable fake env — the injected "port checker"; tests
    /// never touch real ports or the network.
    pub(crate) struct FakeEnv {
        pub linux: bool,
        pub listeners: Vec<(u16, &'static str)>,
        pub ip: Option<&'static str>,
        pub rdns: Option<&'static str>,
        pub outbound_25: Result<(), &'static str>,
        pub disk: Option<u64>,
        pub ram: Option<u64>,
    }

    impl Default for FakeEnv {
        fn default() -> Self {
            Self {
                linux: true,
                listeners: vec![],
                ip: Some("203.0.113.7"),
                rdns: Some("mail.acme.dev"),
                outbound_25: Ok(()),
                disk: Some(40 * 1024 * 1024 * 1024),
                ram: Some(4 * 1024 * 1024 * 1024),
            }
        }
    }

    impl PreflightEnv for FakeEnv {
        fn is_linux(&self) -> bool {
            self.linux
        }
        fn port_listener(&self, port: u16) -> Option<String> {
            self.listeners
                .iter()
                .find(|(p, _)| *p == port)
                .map(|(_, who)| who.to_string())
        }
        fn public_ip(&self) -> Option<String> {
            self.ip.map(str::to_string)
        }
        fn rdns(&self, _ip: &str) -> Option<String> {
            self.rdns.map(str::to_string)
        }
        fn outbound_25(&self) -> Result<(), String> {
            self.outbound_25.map_err(str::to_string)
        }
        fn disk_free_bytes(&self) -> Option<u64> {
            self.disk
        }
        fn total_ram_bytes(&self) -> Option<u64> {
            self.ram
        }
    }

    /// An env that panics on ANY observation — proves the non-Linux
    /// short-circuit never probes (network-silence guarantee for the
    /// Mac route + these tests).
    struct PanicEnv;
    impl PreflightEnv for PanicEnv {
        fn is_linux(&self) -> bool {
            false
        }
        fn port_listener(&self, _: u16) -> Option<String> {
            panic!("non-Linux preflight must not probe ports")
        }
        fn public_ip(&self) -> Option<String> {
            panic!("non-Linux preflight must not dial the network")
        }
        fn rdns(&self, _: &str) -> Option<String> {
            panic!("non-Linux preflight must not resolve DNS")
        }
        fn outbound_25(&self) -> Result<(), String> {
            panic!("non-Linux preflight must not dial the network")
        }
        fn disk_free_bytes(&self) -> Option<u64> {
            panic!("non-Linux preflight must not stat disks")
        }
        fn total_ram_bytes(&self) -> Option<u64> {
            panic!("non-Linux preflight must not read meminfo")
        }
    }

    fn check<'a>(r: &'a PreflightReport, id: &str) -> &'a PreflightCheck {
        r.checks.iter().find(|c| c.id == id).expect(id)
    }

    #[test]
    fn clean_linux_box_passes_with_plan_a() {
        let r = run_preflight(&FakeEnv::default());
        assert!(r.ok);
        assert_eq!(r.port_plan, Some("tls-alpn"));
        for id in ["os", "mta", "port-465", "port-587", "public-ip", "rdns", "outbound-25", "disk", "ram"]
        {
            assert_eq!(check(&r, id).status, CheckStatus::Pass, "{id}");
        }
        assert_eq!(check(&r, "port-443").status, CheckStatus::Info);
        // JSON shape the UI renders.
        let v = r.to_json();
        assert_eq!(v["ok"], true);
        assert_eq!(v["portPlan"], "tls-alpn");
        assert_eq!(v["checks"].as_array().expect("array").len(), 10);
    }

    #[test]
    fn taken_443_selects_plan_c_not_a_stop() {
        let env = FakeEnv { listeners: vec![(443, "caddy")], ..FakeEnv::default() };
        let r = run_preflight(&env);
        assert!(r.ok, ":443 taken is never a stop");
        assert_eq!(r.port_plan, Some("http-01"));
        assert!(check(&r, "port-443").detail.contains("caddy"));
    }

    #[test]
    fn existing_mta_and_held_ports_hard_stop() {
        let env = FakeEnv {
            listeners: vec![(25, "postfix"), (587, "exim4")],
            ..FakeEnv::default()
        };
        let r = run_preflight(&env);
        assert!(!r.ok);
        let mta = check(&r, "mta");
        assert_eq!(mta.status, CheckStatus::Fail);
        assert!(mta.detail.contains("postfix"), "{}", mta.detail);
        assert!(mta.detail.contains("won't fight it"), "{}", mta.detail);
        assert_eq!(check(&r, "port-587").status, CheckStatus::Fail);
        assert!(check(&r, "port-587").detail.contains("exim4"));
        assert_eq!(check(&r, "port-465").status, CheckStatus::Pass);
    }

    #[test]
    fn soft_conditions_warn_but_do_not_block() {
        let env = FakeEnv {
            ip: None,
            rdns: None,
            outbound_25: Err("connection to aspmx.l.google.com:25 timed out"),
            disk: Some(500 * 1024 * 1024),
            ram: Some(512 * 1024 * 1024),
            ..FakeEnv::default()
        };
        let r = run_preflight(&env);
        assert!(r.ok, "warns never block");
        assert_eq!(check(&r, "public-ip").status, CheckStatus::Warn);
        assert!(check(&r, "public-ip").detail.contains("NAT/CGNAT"));
        assert_eq!(check(&r, "rdns").status, CheckStatus::Skipped, "no IP → nothing to look up");
        let ob = check(&r, "outbound-25");
        assert_eq!(ob.status, CheckStatus::Warn);
        assert!(ob.detail.contains("timed out"), "{}", ob.detail);
        assert!(ob.detail.contains("relay mode"), "{}", ob.detail);
        assert_eq!(check(&r, "disk").status, CheckStatus::Warn);
        assert_eq!(check(&r, "ram").status, CheckStatus::Warn);
    }

    #[test]
    fn non_linux_short_circuits_without_probing() {
        let r = run_preflight(&PanicEnv); // panics if anything probes
        assert!(!r.ok);
        assert_eq!(r.port_plan, None);
        assert_eq!(check(&r, "os").status, CheckStatus::Fail);
        assert!(r
            .checks
            .iter()
            .filter(|c| c.id != "os")
            .all(|c| c.status == CheckStatus::Skipped));
    }

    #[test]
    fn ss_output_parser_finds_port_holders() {
        let fixture = "\
LISTEN 0      4096         0.0.0.0:22        0.0.0.0:*    users:((\"sshd\",pid=711,fd=3))
LISTEN 0      100          0.0.0.0:25        0.0.0.0:*    users:((\"exim4\",pid=1204,fd=5))
LISTEN 0      511        127.0.0.1:8080      0.0.0.0:*
LISTEN 0      4096            [::]:443          [::]:*    users:((\"caddy\",pid=902,fd=9))
";
        assert_eq!(parse_ss_listener(fixture, 25).as_deref(), Some("exim4"));
        assert_eq!(parse_ss_listener(fixture, 443).as_deref(), Some("caddy"));
        assert_eq!(
            parse_ss_listener(fixture, 8080).as_deref(),
            Some("an unidentified process"),
            "listener without a process blob still counts as taken"
        );
        assert_eq!(parse_ss_listener(fixture, 587), None);
        // ":25" must not match ":2525"-style suffix collisions.
        assert_eq!(parse_ss_listener("LISTEN 0 1 0.0.0.0:2525 0.0.0.0:*", 25), None);
    }

    #[test]
    fn meminfo_parser() {
        let fixture = "MemTotal:        4030202 kB\nMemFree:          123456 kB\n";
        assert_eq!(parse_meminfo_total(fixture), Some(4030202 * 1024));
        assert_eq!(parse_meminfo_total("garbage"), None);
    }
}
