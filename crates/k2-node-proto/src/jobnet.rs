//! A job's own network ask (0.46.14): `k2 compute run --network none`
//! and `--netns-dummy <name>=<addr>/<prefix>`.
//!
//! - The ask: [`JobNet`], carried as `Plan.network`. Absent unless asked,
//!   so every other plan and digest is unchanged; an older node computes
//!   another digest for a plan that has it and refuses the assign
//!   (`plan_mismatch`), so it never runs such a job with its network.
//! - A job can only **narrow** its network: the node runs it at the
//!   narrower of its own setting (node policy, then the server's share)
//!   and the job's ask. `none` is the narrowest there is.
//! - `none` (Linux): the job unit gets a network namespace of its own with
//!   loopback only (`PrivateNetwork=yes`, `IPAddressDeny=any` with
//!   `IPAddressAllow=localhost`). macOS: the Seatbelt `none` profile (no
//!   remote IP, no DNS; loopback, which on a Mac is also the Mac's own
//!   addresses, stays open).
//! - Dummy interfaces (Linux only, [`FEATURE_NETNS_DUMMY`]): root makes a
//!   named namespace for the job, brings `lo` up, adds each dummy
//!   interface with its address, up, and the unit joins it
//!   (`NetworkNamespacePath=`). For browsers that gather ICE host
//!   candidates on a non-loopback address; the namespace has no route
//!   out, so there is still no internet and no LAN.
//! - Offer features: [`FEATURE_JOB_NETWORK`] (the per-job `none`) and
//!   [`FEATURE_NETNS_DUMMY`]. A server sends `Plan.network` only to a node
//!   offering what it needs ([`plan_features`]); a node checks again
//!   before it runs one and fails it ([`codes`]) rather than run it with
//!   more network.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

/// Offer feature: this node runs a job at `--network none` whatever its
/// own setting (a job may narrow its network, never widen it).
pub const FEATURE_JOB_NETWORK: &str = "job-network";
/// Offer feature: this node makes a job's own network namespace with the
/// dummy interfaces its plan names (Linux job wall, a root executor that
/// says `netns`).
pub const FEATURE_NETNS_DUMMY: &str = "netns-dummy";

/// At most this many dummy interfaces per job.
pub const MAX_DUMMIES: usize = 4;
/// The kernel's interface name limit (`IFNAMSIZ` - 1).
pub const MAX_IFNAME: usize = 15;

/// The network a job asks for. Only `none` in this release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobNetMode {
    /// Loopback only (plus the dummy interfaces it names, Linux).
    None,
}

impl JobNetMode {
    pub fn as_str(self) -> &'static str {
        match self {
            JobNetMode::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(JobNetMode::None),
            _ => None,
        }
    }
}

/// One dummy interface in the job's own network namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dummy {
    /// The interface name ([`check_ifname`]).
    pub name: String,
    /// `<address>/<prefix>`, canonical ([`parse_cidr`], [`check_addr`]).
    pub addr: String,
}

impl Dummy {
    /// `sewlane0 10.254.254.1/32`.
    pub fn line(&self) -> String {
        format!("{} {}", self.name, self.addr)
    }
}

/// `Plan.network`: what the job asked for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobNet {
    pub mode: JobNetMode,
    /// Linux only; absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dummies: Vec<Dummy>,
}

impl JobNet {
    pub fn none() -> Self {
        JobNet {
            mode: JobNetMode::None,
            dummies: Vec::new(),
        }
    }

    /// One line for logs and views: `none`, `none; dummies: sewlane0
    /// 10.254.254.1/32`.
    pub fn line(&self) -> String {
        if self.dummies.is_empty() {
            return self.mode.as_str().to_string();
        }
        format!(
            "{}; dummies: {}",
            self.mode.as_str(),
            self.dummies
                .iter()
                .map(Dummy::line)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// The offer features a plan's `network` needs (sorted, unique).
pub fn plan_features(n: Option<&JobNet>) -> Vec<&'static str> {
    let Some(n) = n else { return Vec::new() };
    let mut out = vec![FEATURE_JOB_NETWORK];
    if !n.dummies.is_empty() {
        out.push(FEATURE_NETNS_DUMMY);
    }
    out.sort_unstable();
    out
}

/// The first feature `n` needs that `offered` lacks, if any.
pub fn missing_feature(n: Option<&JobNet>, offered: &[String]) -> Option<&'static str> {
    plan_features(n)
        .into_iter()
        .find(|f| !offered.iter().any(|o| o == f))
}

pub mod codes {
    /// A `--network` / `--netns-dummy` ask that isn't valid (exit 2).
    pub const JOB_NETWORK_INVALID: &str = "job_network_invalid";
    /// The node can't run this job at the network it asked for (no job
    /// wall, an older root executor, a Mac asked for dummies): it fails
    /// the job rather than run it with more network.
    pub const JOB_NETWORK_UNAVAILABLE: &str = "job_network_unavailable";
    /// No node this job may use runs jobs at `--network none` (or makes
    /// dummy interfaces) yet: it needs k2-node 0.46.15.
    pub const JOB_NETWORK_NEEDS_UPDATE: &str = "job_network_needs_update";
}

/// An interface name the kernel takes and that can't be mistaken for
/// another: 1–15 of `A-Z a-z 0-9 . _ -`, starting with a letter or digit,
/// not `lo` (the namespace's own loopback).
pub fn check_ifname(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_IFNAME {
        return Err(format!(
            "interface name {name:?}: 1-{MAX_IFNAME} characters (the kernel's limit)"
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(format!("interface name {name:?}: only A-Z a-z 0-9 . _ -"));
    }
    if !name.as_bytes()[0].is_ascii_alphanumeric() {
        return Err(format!(
            "interface name {name:?}: starts with a letter or digit"
        ));
    }
    if name == "lo" {
        return Err("interface name \"lo\" is the namespace's own loopback".into());
    }
    Ok(())
}

/// `<address>/<prefix>` → the address and prefix (the prefix is required).
pub fn parse_cidr(s: &str) -> Result<(IpAddr, u8), String> {
    let (a, p) = s
        .split_once('/')
        .ok_or_else(|| format!("{s:?}: <address>/<prefix>, e.g. 10.254.254.1/32"))?;
    let ip: IpAddr = a
        .parse()
        .map_err(|_| format!("{s:?}: {a:?} isn't an IPv4 or IPv6 address"))?;
    if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || p.len() > 3 {
        return Err(format!("{s:?}: the prefix is a number (/32, /24, /128)"));
    }
    let prefix: u8 = p
        .parse()
        .map_err(|_| format!("{s:?}: the prefix is a number"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if prefix == 0 || prefix > max {
        return Err(format!("{s:?}: the prefix is 1-{max}"));
    }
    Ok((ip, prefix))
}

/// The ranges a dummy address may come from: private, link-local and the
/// documentation/test ranges. Never a public address: the job must not be
/// able to pose as one, even inside its own namespace.
pub const ALLOWED_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
];
pub const ALLOWED_V6: &[(Ipv6Addr, u8)] = &[
    // Unique local (fc00::/7).
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    // Link-local.
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    // Documentation.
    (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
];

fn in_v4(a: Ipv4Addr, prefix: u8, net: Ipv4Addr, len: u8) -> bool {
    let mask = if len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(len))
    };
    prefix >= len && (u32::from(a) & mask) == (u32::from(net) & mask)
}

fn in_v6(a: Ipv6Addr, prefix: u8, net: Ipv6Addr, len: u8) -> bool {
    let mask = if len == 0 {
        0
    } else {
        u128::MAX << (128 - u32::from(len))
    };
    prefix >= len && (u128::from(a) & mask) == (u128::from(net) & mask)
}

/// A dummy address: `<address>/<prefix>` whose whole prefix lies inside
/// one of [`ALLOWED_V4`] / [`ALLOWED_V6`] (so `10.0.0.1/7` is refused).
/// Returns the canonical form (`fd00::1/128`, never `fd00:0::1/128`).
pub fn check_addr(s: &str) -> Result<String, String> {
    let (ip, prefix) = parse_cidr(s)?;
    let ok = match ip {
        IpAddr::V4(a) => ALLOWED_V4.iter().any(|(n, l)| in_v4(a, prefix, *n, *l)),
        IpAddr::V6(a) => ALLOWED_V6.iter().any(|(n, l)| in_v6(a, prefix, *n, *l)),
    };
    if !ok {
        return Err(format!(
            "{s}: a dummy address comes from a private, link-local or test range (10/8, 172.16/12, 192.168/16, 169.254/16, 192.0.2/24, 198.51.100/24, 203.0.113/24, fc00::/7, fe80::/10, 2001:db8::/32), with its whole prefix inside it; never a public address"
        ));
    }
    if ip.is_unspecified() {
        return Err(format!("{s}: not an unspecified address"));
    }
    Ok(format!("{ip}/{prefix}"))
}

/// `name=addr/prefix` (the CLI's `--netns-dummy`) → a checked [`Dummy`].
pub fn parse_dummy(s: &str) -> Result<Dummy, String> {
    let (name, addr) = s.split_once('=').ok_or_else(|| {
        format!("--netns-dummy {s:?}: <name>=<address>/<prefix>, e.g. sewlane0=10.254.254.1/32")
    })?;
    check_ifname(name)?;
    Ok(Dummy {
        name: name.to_string(),
        addr: check_addr(addr)?,
    })
}

/// Everything a plan's `network` must be (both ends check it): at most
/// [`MAX_DUMMIES`] dummies, each name and address valid, the address
/// canonical, no name or address twice.
pub fn check(n: &JobNet) -> Result<(), String> {
    if n.dummies.len() > MAX_DUMMIES {
        return Err(format!(
            "at most {MAX_DUMMIES} dummy interfaces per job ({} asked)",
            n.dummies.len()
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut addrs = std::collections::BTreeSet::new();
    for d in &n.dummies {
        check_ifname(&d.name)?;
        let canon = check_addr(&d.addr)?;
        if canon != d.addr {
            return Err(format!(
                "dummy address {:?} isn't canonical ({canon})",
                d.addr
            ));
        }
        if !names.insert(d.name.as_str()) {
            return Err(format!("dummy interface {} named twice", d.name));
        }
        let ip = canon.split('/').next().unwrap_or_default().to_string();
        if !addrs.insert(ip) {
            return Err(format!("dummy address {} given twice", d.addr));
        }
    }
    Ok(())
}

/// Every dummy address's prefix, for the unit's `IPAddressAllow=` (the
/// namespace is the job's own, with no route out; the allow only lets its
/// own interfaces carry traffic).
pub fn allow_prefixes(n: &JobNet) -> Vec<String> {
    n.dummies.iter().map(|d| d.addr.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_names_follow_the_kernels_rules_and_ours() {
        for ok in [
            "sewlane0",
            "a",
            "eth-test.1",
            "x_y",
            "0dummy",
            "fifteen-chars15",
        ] {
            assert_eq!(check_ifname(ok), Ok(()), "{ok}");
        }
        assert_eq!("fifteen-chars15".len(), 15);
        for bad in [
            "",
            "sixteen-chars-16",
            "lo",
            "has space",
            "a/b",
            "a:b",
            "-lead",
            ".lead",
            "_lead",
            "é",
            "a\0b",
        ] {
            assert!(check_ifname(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn addresses_come_only_from_private_link_local_or_test_ranges() {
        for (ok, canon) in [
            ("10.254.254.1/32", "10.254.254.1/32"),
            ("10.0.0.1/8", "10.0.0.1/8"),
            ("172.16.0.5/24", "172.16.0.5/24"),
            ("172.31.255.254/32", "172.31.255.254/32"),
            ("192.168.77.1/16", "192.168.77.1/16"),
            ("169.254.10.10/32", "169.254.10.10/32"),
            ("192.0.2.7/32", "192.0.2.7/32"),
            ("198.51.100.9/24", "198.51.100.9/24"),
            ("203.0.113.1/32", "203.0.113.1/32"),
            ("fd00::1/128", "fd00::1/128"),
            ("fd12:3456:789a:0000::0001/64", "fd12:3456:789a::1/64"),
            ("fc00::2/7", "fc00::2/7"),
            ("fe80::1/64", "fe80::1/64"),
            ("2001:db8::5/48", "2001:db8::5/48"),
        ] {
            assert_eq!(check_addr(ok).as_deref(), Ok(canon), "{ok}");
        }
        for bad in [
            "8.8.8.8/32",
            "1.1.1.1/32",
            "172.32.0.1/32",
            "11.0.0.1/32",
            "192.169.0.1/32",
            "100.64.0.1/32",
            "127.0.0.2/32",
            "0.0.0.0/32",
            "10.0.0.1/7",
            "192.168.1.1/15",
            "2606:4700::1111/128",
            "::1/128",
            "::/128",
            "fd00::1/6",
            "10.0.0.1",
            "10.0.0.1/",
            "10.0.0.1/33",
            "10.0.0.1/0",
            "fd00::1/129",
            "10.0.0.1/+8",
            "10.0.0.1/0032",
            "not-an-ip/32",
            "",
        ] {
            assert!(check_addr(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_cli_dummy_is_name_equals_cidr() {
        assert_eq!(
            parse_dummy("sewlane0=10.254.254.1/32"),
            Ok(Dummy {
                name: "sewlane0".into(),
                addr: "10.254.254.1/32".into()
            })
        );
        assert_eq!(parse_dummy("v6=fd00:0::1/128").unwrap().addr, "fd00::1/128");
        for bad in [
            "sewlane0",
            "sewlane0=",
            "=10.0.0.1/32",
            "lo=10.0.0.1/32",
            "sewlane0=8.8.8.8/32",
            "much-too-long-name=10.0.0.1/32",
        ] {
            assert!(parse_dummy(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_plan_has_at_most_four_distinct_canonical_dummies() {
        let d = |n: &str, a: &str| Dummy {
            name: n.into(),
            addr: a.into(),
        };
        let mut n = JobNet {
            mode: JobNetMode::None,
            dummies: (0..4)
                .map(|i| d(&format!("d{i}"), &format!("10.0.0.{}/32", i + 1)))
                .collect(),
        };
        assert_eq!(check(&n), Ok(()));
        n.dummies.push(d("d4", "10.0.0.9/32"));
        assert!(check(&n).unwrap_err().contains("at most 4"));
        let twice = JobNet {
            mode: JobNetMode::None,
            dummies: vec![d("a", "10.0.0.1/32"), d("a", "10.0.0.2/32")],
        };
        assert!(check(&twice).unwrap_err().contains("named twice"));
        let same_ip = JobNet {
            mode: JobNetMode::None,
            dummies: vec![d("a", "10.0.0.1/32"), d("b", "10.0.0.1/24")],
        };
        assert!(check(&same_ip).unwrap_err().contains("given twice"));
        let not_canon = JobNet {
            mode: JobNetMode::None,
            dummies: vec![d("a", "fd00:0::1/128")],
        };
        assert!(check(&not_canon).unwrap_err().contains("canonical"));
        let public = JobNet {
            mode: JobNetMode::None,
            dummies: vec![d("a", "8.8.8.8/32")],
        };
        assert!(check(&public).is_err());
        assert_eq!(check(&JobNet::none()), Ok(()));
    }

    #[test]
    fn wire_shape_and_features() {
        let n = JobNet {
            mode: JobNetMode::None,
            dummies: vec![Dummy {
                name: "sewlane0".into(),
                addr: "10.254.254.1/32".into(),
            }],
        };
        assert_eq!(
            serde_json::to_string(&n).unwrap(),
            r#"{"mode":"none","dummies":[{"name":"sewlane0","addr":"10.254.254.1/32"}]}"#
        );
        assert_eq!(
            serde_json::to_string(&JobNet::none()).unwrap(),
            r#"{"mode":"none"}"#,
            "no dummies: no field"
        );
        assert!(
            serde_json::from_str::<JobNet>(r#"{"mode":"on"}"#).is_err(),
            "a job can't ask for more network"
        );
        assert!(serde_json::from_str::<JobNet>(r#"{"mode":"none","extra":1}"#).is_err());
        assert_eq!(plan_features(None), Vec::<&str>::new());
        assert_eq!(
            plan_features(Some(&JobNet::none())),
            vec![FEATURE_JOB_NETWORK]
        );
        assert_eq!(
            plan_features(Some(&n)),
            vec![FEATURE_JOB_NETWORK, FEATURE_NETNS_DUMMY]
        );
        let old = vec!["needs".to_string(), FEATURE_JOB_NETWORK.to_string()];
        assert_eq!(missing_feature(Some(&JobNet::none()), &old), None);
        assert_eq!(missing_feature(Some(&n), &old), Some(FEATURE_NETNS_DUMMY));
        assert_eq!(missing_feature(Some(&n), &[]), Some(FEATURE_JOB_NETWORK));
        assert_eq!(n.line(), "none; dummies: sewlane0 10.254.254.1/32");
        assert_eq!(allow_prefixes(&n), vec!["10.254.254.1/32".to_string()]);
    }
}
