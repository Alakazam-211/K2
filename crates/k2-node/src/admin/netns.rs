//! A job's own network namespace with dummy interfaces (0.46.14,
//! `k2 compute run --netns-dummy`, [`k2_node_proto::jobnet`]), root side.
//!
//! A job asking for dummy interfaces can't make them itself: inside its
//! unit it runs as the job user, and on Ubuntu 24.04 the AppArmor
//! user-namespace restriction refuses the unprivileged namespace it would
//! need. So the root executor makes the namespace before the unit exists:
//!
//! 1. `ip netns add k2job-<key>` (a bind mount under [`NETNS_DIR`]);
//! 2. `lo` up; each dummy `ip link add <name> type dummy`, its address
//!    (`nodad` for IPv6, so it is usable at once), up;
//! 3. an IPv4 default route through the first dummy (0.46.18,
//!    `ip -4 route add default dev <first>`; no IPv6 default): a browser's
//!    local-address lookup (a UDP connect to a public IP) then finds the
//!    dummy's address, so it gathers an ICE host candidate. A dummy
//!    discards everything sent to it, so the route leads nowhere;
//! 4. the unit joins it with `NetworkNamespacePath=` instead of
//!    `PrivateNetwork=yes` ([`super::jobs::systemd_run_argv`]). Nothing
//!    leaves it: no internet, no LAN, as under `PrivateNetwork` (and the
//!    unit's address filter denies all but loopback and the dummies).
//!
//! The namespace goes when the node releases the unit (`release`), when a
//! start fails, and, for one whose unit is gone (an executor that died),
//! at the next start that makes one ([`sweep`]). Deleting the mount while
//! the job still stops is safe: the namespace lives as long as a process
//! is in it.
//!
//! Only fixed argv, never a shell; names and addresses are checked with
//! [`k2_node_proto::jobnet::check`] before any command runs.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use k2_node_proto::jobnet::{self, Dummy, JobNet, JobNetMode};

/// Where `ip netns` keeps named namespaces.
pub const NETNS_DIR: &str = "/run/netns";

/// Every job namespace's name starts with this (the unit's prefix).
pub const NETNS_PREFIX: &str = super::jobs::UNIT_PREFIX;

/// The namespace of a job unit: its name without `.service`
/// (`k2job-j1-g1`).
pub fn netns_name(unit: &str) -> Option<String> {
    super::jobs::valid_unit(unit).then(|| unit.trim_end_matches(".service").to_string())
}

/// The path a unit joins (`NetworkNamespacePath=`).
pub fn netns_path(name: &str) -> PathBuf {
    Path::new(NETNS_DIR).join(name)
}

fn ip_prog() -> Option<PathBuf> {
    ["/usr/sbin", "/sbin", "/usr/bin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join("ip"))
        .find(|p| p.is_file())
}

/// Can this machine make job namespaces at all (Linux, iproute2)?
pub fn available() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("job network namespaces are Linux only".into());
    }
    ip_prog()
        .map(|_| ())
        .ok_or_else(|| "ip (iproute2) not found".into())
}

/// The `ip` commands that make namespace `name` with `dummies`, in order
/// (each an argv without the program). Pure, so tests read exactly what
/// root runs.
pub fn make_argvs(name: &str, dummies: &[Dummy]) -> Vec<Vec<String>> {
    let s = |x: &str| x.to_string();
    let mut out = vec![
        vec![s("netns"), s("add"), s(name)],
        vec![s("-n"), s(name), s("link"), s("set"), s("lo"), s("up")],
    ];
    for d in dummies {
        out.push(vec![
            s("-n"),
            s(name),
            s("link"),
            s("add"),
            d.name.clone(),
            s("type"),
            s("dummy"),
        ]);
        let mut addr = vec![
            s("-n"),
            s(name),
            s("addr"),
            s("add"),
            d.addr.clone(),
            s("dev"),
            d.name.clone(),
        ];
        if d.addr.contains(':') {
            // IPv6: no duplicate address detection (a fresh namespace has
            // nobody to collide with), so the address is usable at once.
            addr.push(s("nodad"));
        }
        out.push(addr);
        out.push(vec![
            s("-n"),
            s(name),
            s("link"),
            s("set"),
            d.name.clone(),
            s("up"),
        ]);
    }
    // 0.46.18: the IPv4 default route through the first dummy (it is up by
    // now), so a local-address lookup finds an address (a browser's ICE
    // host candidate); the dummy drops what reaches it. No IPv6 default.
    if let Some(first) = dummies.first() {
        out.push(default_route_argv(name, &first.name));
    }
    out
}

/// `ip -n <ns> -4 route add default dev <dummy>`: the job namespace's IPv4
/// default route, through its first dummy.
pub fn default_route_argv(name: &str, dummy: &str) -> Vec<String> {
    ["-n", name, "-4", "route", "add", "default", "dev", dummy]
        .iter()
        .map(|x| x.to_string())
        .collect()
}

fn ip(args: &[String]) -> Result<(), String> {
    let prog = ip_prog().ok_or("ip (iproute2) not found")?;
    let out = std::process::Command::new(prog)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir("/")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ip: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ip {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Make namespace `name` with `dummies` (checked first), all or nothing:
/// a step that fails deletes what was made. A leftover of the same name
/// (an executor restart) is deleted first. Returns the path a unit joins.
pub fn make(name: &str, dummies: &[Dummy]) -> Result<PathBuf, String> {
    if !name.starts_with(NETNS_PREFIX) || name.contains('/') || name.len() > 200 {
        return Err(format!("{name:?} isn't a job namespace name"));
    }
    jobnet::check(&JobNet {
        mode: JobNetMode::None,
        dummies: dummies.to_vec(),
    })?;
    available()?;
    if netns_path(name).exists() {
        let _ = ip(&["netns".into(), "del".into(), name.into()]);
    }
    let cmds = make_argvs(name, dummies);
    ip(&cmds[0])?;
    for c in &cmds[1..] {
        if let Err(e) = ip(c) {
            let _ = remove(name);
            return Err(e);
        }
    }
    Ok(netns_path(name))
}

/// Delete namespace `name` (its mount; the namespace itself goes with its
/// last process). Nothing to do when it isn't there.
pub fn remove(name: &str) -> Result<(), String> {
    if !name.starts_with(NETNS_PREFIX) || name.contains('/') {
        return Err(format!("{name:?} isn't a job namespace name"));
    }
    if !netns_path(name).exists() {
        return Ok(());
    }
    ip(&["netns".into(), "del".into(), name.into()])
}

/// The job namespaces under `dir` whose unit `live` says isn't loaded
/// (pure: the caller passes the listing and the check).
pub fn stale<'a>(
    names: impl IntoIterator<Item = &'a str>,
    live: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with(NETNS_PREFIX) && !n.contains("selftest"))
        .filter(|n| {
            super::jobs::valid_unit(&format!("{n}.service")) && !live(&format!("{n}.service"))
        })
        .map(str::to_string)
        .collect();
    out.sort();
    out
}

/// Delete every job namespace whose unit is gone (an executor that died
/// between a start and its release). Returns what went.
pub fn sweep() -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(NETNS_DIR) else {
        return Vec::new();
    };
    let names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let gone = stale(names.iter().map(String::as_str), |u| {
        super::jobs::unit_status(u).is_ok_and(|s| s.loaded)
    });
    gone.into_iter().filter(|n| remove(n).is_ok()).collect()
}

/// The executor's check, as root, before it says `netns` in `Ready`: make
/// a namespace with one IPv4 and one IPv6 dummy, see both addresses in
/// it, delete it. Only a pass is remembered (once per executor process).
pub fn selftest() -> Result<(), String> {
    static PASSED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if PASSED.get().is_some() {
        return Ok(());
    }
    available()?;
    let name = format!("{NETNS_PREFIX}selftest-netns-{}", std::process::id());
    let dummies = [
        Dummy {
            name: "k2self4".into(),
            addr: "10.254.254.254/32".into(),
        },
        Dummy {
            name: "k2self6".into(),
            addr: "fd00:6b32::1/128".into(),
        },
    ];
    let r = make(&name, &dummies).and_then(|_| {
        let prog = ip_prog().ok_or("ip (iproute2) not found")?;
        let out = std::process::Command::new(prog)
            .args(["-n", &name, "-o", "addr", "show"])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("ip: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        for d in &dummies {
            if !text
                .lines()
                .any(|l| l.contains(&d.name) && l.contains(&d.addr))
            {
                return Err(format!(
                    "{} has no {} in the test namespace: {}",
                    d.name,
                    d.addr,
                    text.trim()
                ));
            }
        }
        Ok(())
    });
    let _ = remove(&name);
    r?;
    let _ = PASSED.set(());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_job_units_only() {
        assert_eq!(
            netns_name("k2job-j1-g1.service").as_deref(),
            Some("k2job-j1-g1")
        );
        assert_eq!(
            netns_name("k2job-t3-0123456789abcdef.service").as_deref(),
            Some("k2job-t3-0123456789abcdef")
        );
        assert_eq!(netns_name("sshd.service"), None);
        assert_eq!(netns_name("k2job-../x.service"), None);
        assert_eq!(
            netns_path("k2job-j1-g1"),
            PathBuf::from("/run/netns/k2job-j1-g1")
        );
    }

    #[test]
    fn root_runs_exactly_these_ip_commands() {
        let d = vec![
            Dummy {
                name: "sewlane0".into(),
                addr: "10.254.254.1/32".into(),
            },
            Dummy {
                name: "v6".into(),
                addr: "fd00::1/128".into(),
            },
        ];
        let a = make_argvs("k2job-j1-g1", &d);
        let lines: Vec<String> = a.iter().map(|x| x.join(" ")).collect();
        assert_eq!(
            lines,
            vec![
                "netns add k2job-j1-g1",
                "-n k2job-j1-g1 link set lo up",
                "-n k2job-j1-g1 link add sewlane0 type dummy",
                "-n k2job-j1-g1 addr add 10.254.254.1/32 dev sewlane0",
                "-n k2job-j1-g1 link set sewlane0 up",
                "-n k2job-j1-g1 link add v6 type dummy",
                "-n k2job-j1-g1 addr add fd00::1/128 dev v6 nodad",
                "-n k2job-j1-g1 link set v6 up",
                // 0.46.18: IPv4 default via the first dummy only, last.
                "-n k2job-j1-g1 -4 route add default dev sewlane0",
            ]
        );
        let routes = |a: &[Vec<String>]| a.iter().filter(|x| x.iter().any(|w| w == "route")).count();
        assert_eq!(routes(&a), 1, "one default route, IPv4, no IPv6 default");
        assert!(!lines.iter().any(|l| l.contains("route") && (l.contains(" -6 ") || l.contains("dev v6"))), "{lines:?}");
        let none = make_argvs("k2job-x-g1", &[]);
        assert_eq!(none.len(), 2, "no dummies: the namespace and its lo");
        assert_eq!(routes(&none), 0, "no dummies: no default route");
        // An IPv6-only first dummy still carries the IPv4 default (the ask:
        // the first dummy, IPv4).
        let v6_first = make_argvs("k2job-y-g1", &d[1..]);
        assert_eq!(v6_first.last().unwrap().join(" "), "-n k2job-y-g1 -4 route add default dev v6");
    }

    #[test]
    fn make_refuses_bad_names_and_addresses_before_any_command() {
        let ok = vec![Dummy {
            name: "sewlane0".into(),
            addr: "10.254.254.1/32".into(),
        }];
        assert!(make("not-a-job", &ok)
            .unwrap_err()
            .contains("isn't a job namespace"));
        assert!(make("k2job-a/b", &ok)
            .unwrap_err()
            .contains("isn't a job namespace"));
        let public = vec![Dummy {
            name: "x".into(),
            addr: "8.8.8.8/32".into(),
        }];
        assert!(make("k2job-j1-g1", &public)
            .unwrap_err()
            .contains("never a public address"));
        let lo = vec![Dummy {
            name: "lo".into(),
            addr: "10.0.0.1/32".into(),
        }];
        assert!(make("k2job-j1-g1", &lo).unwrap_err().contains("loopback"));
        let five: Vec<Dummy> = (0..5)
            .map(|i| Dummy {
                name: format!("d{i}"),
                addr: format!("10.0.0.{}/32", i + 1),
            })
            .collect();
        assert!(make("k2job-j1-g1", &five)
            .unwrap_err()
            .contains("at most 4"));
        assert!(remove("../etc").is_err());
    }

    #[test]
    fn the_sweep_takes_only_job_namespaces_whose_unit_is_gone() {
        let names = [
            "k2job-j1-g1",
            "k2job-j2-g1",
            "k2job-selftest-netns-42",
            "cni-1234",
            "k2job-bad name",
        ];
        let live = |u: &str| u == "k2job-j1-g1.service";
        assert_eq!(stale(names, live), vec!["k2job-j2-g1".to_string()]);
    }
}
