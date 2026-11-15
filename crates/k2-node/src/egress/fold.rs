//! What the node makes of a proxy's `egress.jsonl` (prd-compute-mac-no-lan-v1
//! §5.6): the job log's live refusal lines, the end summary, and the
//! job's host list (one row per `host:port` and verdict).

use std::collections::HashMap;

use super::{codes, hostport, plain_url, Decision};

/// One row of the job's host list (the result's `network_hosts`).
pub use k2_node_proto::results::NetworkHost as HostRow;

/// Live lines per `host:port` before the rest are only counted.
pub const LIVE_PER_HOST: u32 = 3;
/// Host list rows in the job log (the result keeps up to
/// [`k2_node_proto::results::MAX_NETWORK_HOSTS`]).
pub const LOG_ROWS: usize = 100;

/// A job's whole host list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostList {
    pub rows: Vec<HostRow>,
    /// Decision lines the proxy's log cap left out.
    pub dropped: u64,
}

fn verdict(ev: &str) -> Option<&'static str> {
    match ev {
        "open" | "close" => Some("allowed"),
        "refused" => Some("refused"),
        "failed" => Some("failed"),
        _ => None,
    }
}

fn rank(v: &str) -> u8 {
    match v {
        "allowed" => 0,
        "refused" => 1,
        _ => 2,
    }
}

/// Fold decision lines into the host list: allowed rows first, then
/// refused, then failed, each in the order first seen.
pub fn fold<'a>(lines: impl IntoIterator<Item = &'a Decision>) -> HostList {
    let mut rows: Vec<HostRow> = Vec::new();
    let mut at: HashMap<(String, u16, &'static str), usize> = HashMap::new();
    let mut dropped = 0;
    for d in lines {
        if d.ev == "dropped" {
            dropped += d.n;
            continue;
        }
        let Some(v) = verdict(&d.ev) else { continue };
        let i = *at.entry((d.host.clone(), d.port, v)).or_insert_with(|| {
            rows.push(HostRow { host: d.host.clone(), port: d.port, verdict: v.into(), reason: None, addr: None, conns: 0, bytes_in: 0, bytes_out: 0 });
            rows.len() - 1
        });
        let r = &mut rows[i];
        match d.ev.as_str() {
            "close" => {
                r.bytes_in += d.bytes_in;
                r.bytes_out += d.bytes_out;
            }
            _ => {
                r.conns += 1;
                if d.addr.is_some() {
                    r.addr = d.addr.clone();
                }
                if d.why.is_some() {
                    r.reason = d.why.clone();
                }
            }
        }
    }
    rows.sort_by_key(|r| rank(&r.verdict));
    HostList { rows, dropped }
}

/// Parse `egress.jsonl` text (bad lines are skipped).
pub fn parse_lines(text: &str) -> Vec<Decision> {
    text.lines().filter_map(|l| serde_json::from_str::<Decision>(l).ok()).collect()
}

/// `1.2 GB`, `96 KB`, `412 B` (binary units).
pub fn human_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / (K * K))
    } else {
        format!("{:.1} GB", f / (K * K * K))
    }
}

fn what(d_host: &str, port: u16, code: Option<&str>) -> String {
    if code == Some(codes::PLAIN_HTTP) {
        plain_url(d_host, port)
    } else {
        hostport(d_host, port)
    }
}

/// The live job-log line for one decision (`None` for an allowed one),
/// without the `[k2-node] ` prefix.
pub fn live_line(d: &Decision) -> Option<String> {
    let why = d.why.as_deref().unwrap_or("refused");
    let is_name = d.host.parse::<std::net::IpAddr>().is_err();
    match d.ev.as_str() {
        "refused" if d.host.is_empty() => Some(format!("network: refused a request: {why}")),
        "refused" => {
            let w = what(&d.host, d.port, d.code.as_deref());
            let tail = if d.code.as_deref() == Some(codes::PRIVATE) { " (no-lan keeps jobs off the local network)" } else { "" };
            Some(match &d.addr {
                Some(a) if is_name => format!("network: refused {w} → {a}: {why}{tail}"),
                _ => format!("network: refused {w}: {why}{tail}"),
            })
        }
        "failed" => Some(format!("network: couldn't reach {}: {why}", hostport(&d.host, d.port))),
        _ => None,
    }
}

/// The live lines as the node writes them: the first [`LIVE_PER_HOST`]
/// per `host:port`, then counted (the end summary has them all).
#[derive(Debug, Default)]
pub struct Live {
    seen: HashMap<(String, u16), u32>,
}

impl Live {
    pub fn line(&mut self, d: &Decision) -> Option<String> {
        let l = live_line(d)?;
        let n = self.seen.entry((d.host.clone(), d.port)).or_insert(0);
        *n += 1;
        match *n {
            n if n < LIVE_PER_HOST => Some(l),
            n if n == LIVE_PER_HOST => Some(format!("{l} (more for {} are only counted, in the summary at the end)", hostport(&d.host, d.port))),
            _ => None,
        }
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The end summary line (no prefix).
pub fn summary_line(l: &HostList) -> String {
    let allowed: Vec<&HostRow> = l.rows.iter().filter(|r| r.verdict == "allowed").collect();
    let conns: u64 = allowed.iter().map(|r| r.conns).sum();
    let (inn, out): (u64, u64) = (allowed.iter().map(|r| r.bytes_in).sum(), allowed.iter().map(|r| r.bytes_out).sum());
    let mut s = format!(
        "network: no-lan proxy: {} to {}, {} in, {} out",
        plural(conns, "connection", "connections"),
        plural(allowed.len() as u64, "host", "hosts"),
        human_bytes(inn),
        human_bytes(out)
    );
    for (v, label) in [("refused", "refused"), ("failed", "couldn't reach")] {
        let rows: Vec<&HostRow> = l.rows.iter().filter(|r| r.verdict == v).collect();
        if rows.is_empty() {
            continue;
        }
        let each: Vec<String> = rows
            .iter()
            .map(|r| {
                let why = r.reason.as_deref().unwrap_or(v);
                let w = if r.reason.as_deref().is_some_and(|x| x.starts_with("plain HTTP")) { plain_url(&r.host, r.port) } else { hostport(&r.host, r.port) };
                match &r.addr {
                    Some(a) if r.host.parse::<std::net::IpAddr>().is_err() => format!("{w} ({a}, {why})"),
                    _ => format!("{w} ({why})"),
                }
            })
            .collect();
        s.push_str(&format!("; {label} {}: {}", plural(rows.len() as u64, "host", "hosts"), each.join(", ")));
    }
    if l.dropped > 0 {
        s.push_str(&format!("; {} over the proxy's log cap weren't kept", plural(l.dropped, "decision", "decisions")));
    }
    s
}

/// The host list for the job log (no prefix): a heading, then one row per
/// host, at most [`LOG_ROWS`].
pub fn host_lines(l: &HostList) -> Vec<String> {
    if l.rows.is_empty() {
        return vec!["network: hosts this job reached or tried: none".into()];
    }
    let n = |v: &str| l.rows.iter().filter(|r| r.verdict == v).count();
    let mut head = format!("network: hosts this job reached or tried ({} allowed, {} refused", n("allowed"), n("refused"));
    if n("failed") > 0 {
        head.push_str(&format!(", {} unreachable", n("failed")));
    }
    head.push_str("):");
    let width = l.rows.iter().take(LOG_ROWS).map(|r| hostport(&r.host, r.port).len()).max().unwrap_or(0);
    let mut out = vec![head];
    for r in l.rows.iter().take(LOG_ROWS) {
        let hp = hostport(&r.host, r.port);
        let row = if r.verdict == "allowed" {
            format!("  allowed  {hp:<width$}  {:>5}   in {:<9} out {}", plural(r.conns, "conn ", "conns"), human_bytes(r.bytes_in), human_bytes(r.bytes_out))
        } else {
            let why = match (&r.addr, r.reason.as_deref()) {
                (Some(a), Some(w)) if r.host.parse::<std::net::IpAddr>().is_err() => format!("{a}, {w}"),
                (_, Some(w)) => w.to_string(),
                (Some(a), None) => a.clone(),
                (None, None) => String::new(),
            };
            let label = if r.verdict == "refused" { "refused" } else { "failed " };
            format!("  {label}  {hp:<width$}  {:>5}   {why}", plural(r.conns, "try  ", "tries"))
        };
        out.push(row);
    }
    if l.rows.len() > LOG_ROWS {
        out.push(format!("  and {} more", l.rows.len() - LOG_ROWS));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(ev: &str, host: &str, port: u16) -> Decision {
        Decision::new(ev, host, port)
    }

    fn refused(host: &str, port: u16, addr: Option<&str>, code: &str, why: &str) -> Decision {
        Decision { addr: addr.map(str::to_string), code: Some(code.into()), why: Some(why.into()), ..d("refused", host, port) }
    }

    fn sample() -> Vec<Decision> {
        vec![
            Decision { addr: Some("151.101.0.1".into()), ..d("open", "static.crates.io", 443) },
            refused("gitlab.office.lan", 443, Some("192.168.1.20"), codes::PRIVATE, "private address"),
            Decision { bytes_in: 5 << 20, bytes_out: 2048, ..d("close", "static.crates.io", 443) },
            Decision { addr: Some("151.101.0.1".into()), ..d("open", "static.crates.io", 443) },
            Decision { bytes_in: 1 << 20, bytes_out: 1024, ..d("close", "static.crates.io", 443) },
            refused("gitlab.office.lan", 443, Some("192.168.1.20"), codes::PRIVATE, "private address"),
            refused("10.0.0.5", 22, Some("10.0.0.5"), codes::PRIVATE, "private address"),
            Decision { addr: Some("140.82.112.3".into()), ..d("open", "github.com", 443) },
            Decision { code: Some(codes::DNS_FAILED.into()), why: Some("DNS lookup failed (does this node have internet?)".into()), ..d("failed", "index.crates.io", 443) },
        ]
    }

    #[test]
    fn the_host_list_has_one_row_per_host_and_verdict() {
        let l = fold(&sample());
        let keys: Vec<(String, u16, String, u64)> = l.rows.iter().map(|r| (r.host.clone(), r.port, r.verdict.clone(), r.conns)).collect();
        assert_eq!(
            keys,
            vec![
                ("static.crates.io".into(), 443, "allowed".into(), 2),
                ("github.com".into(), 443, "allowed".into(), 1),
                ("gitlab.office.lan".into(), 443, "refused".into(), 2),
                ("10.0.0.5".into(), 22, "refused".into(), 1),
                ("index.crates.io".into(), 443, "failed".into(), 1),
            ]
        );
        let s = &l.rows[0];
        assert_eq!((s.bytes_in, s.bytes_out, s.addr.as_deref()), (6 << 20, 3072, Some("151.101.0.1")));
        assert_eq!((l.rows[1].bytes_in, l.rows[1].bytes_out), (0, 0), "a tunnel still open at the end has no close line yet");
        let r = &l.rows[2];
        assert_eq!((r.addr.as_deref(), r.reason.as_deref(), r.bytes_in), (Some("192.168.1.20"), Some("private address"), 0));
    }

    #[test]
    fn the_summary_names_each_refused_host_once() {
        let s = summary_line(&fold(&sample()));
        assert_eq!(
            s,
            "network: no-lan proxy: 3 connections to 2 hosts, 6.0 MB in, 3 KB out; refused 2 hosts: gitlab.office.lan:443 (192.168.1.20, private address), 10.0.0.5:22 (private address); couldn't reach 1 host: index.crates.io:443 (DNS lookup failed (does this node have internet?))"
        );
        assert_eq!(s.matches("gitlab.office.lan").count(), 1);
        let none = summary_line(&HostList::default());
        assert_eq!(none, "network: no-lan proxy: 0 connections to 0 hosts, 0 B in, 0 B out");
        let dropped = summary_line(&HostList { rows: vec![], dropped: 7 });
        assert!(dropped.ends_with("; 7 decisions over the proxy's log cap weren't kept"), "{dropped}");
    }

    #[test]
    fn the_host_lines_are_capped() {
        let lines = host_lines(&fold(&sample()));
        assert_eq!(lines[0], "network: hosts this job reached or tried (2 allowed, 2 refused, 1 unreachable):");
        assert_eq!(lines.len(), 6);
        assert!(lines[1].starts_with("  allowed  static.crates.io:443") && lines[1].contains("2 conns") && lines[1].contains("in 6.0 MB") && lines[1].ends_with("out 3 KB"), "{}", lines[1]);
        assert!(lines[3].starts_with("  refused  gitlab.office.lan:443") && lines[3].contains("2 tries") && lines[3].ends_with("192.168.1.20, private address"), "{}", lines[3]);
        assert!(lines[4].ends_with("1 try     private address"), "{:?}", lines[4]);
        let many: Vec<Decision> = (0..150u16).map(|i| d("open", &format!("h{i}.example.com"), 443)).collect();
        let l = fold(&many);
        assert_eq!(l.rows.len(), 150, "the result keeps every row");
        let lines = host_lines(&l);
        assert_eq!(lines.len(), 1 + LOG_ROWS + 1);
        assert_eq!(lines.last().unwrap(), "  and 50 more");
        assert_eq!(host_lines(&HostList::default()), vec!["network: hosts this job reached or tried: none".to_string()]);
    }

    #[test]
    fn live_lines_say_what_and_why_and_stop_after_three() {
        let r = refused("gitlab.office.lan", 443, Some("192.168.1.20"), codes::PRIVATE, "private address");
        assert_eq!(live_line(&r).unwrap(), "network: refused gitlab.office.lan:443 → 192.168.1.20: private address (no-lan keeps jobs off the local network)");
        let lit = refused("10.0.0.5", 22, Some("10.0.0.5"), codes::PRIVATE, "private address");
        assert_eq!(live_line(&lit).unwrap(), "network: refused 10.0.0.5:22: private address (no-lan keeps jobs off the local network)");
        let v6 = refused("fd00::1", 443, Some("fd00::1"), codes::PRIVATE, "private address");
        assert_eq!(live_line(&v6).unwrap(), "network: refused [fd00::1]:443: private address (no-lan keeps jobs off the local network)");
        let local = refused("nas.local", 443, None, codes::LOCAL_NAME, "local name (not looked up)");
        assert_eq!(live_line(&local).unwrap(), "network: refused nas.local:443: local name (not looked up)");
        let plain = refused("example.com", 80, None, codes::PLAIN_HTTP, "plain HTTP isn't carried; use https");
        assert_eq!(live_line(&plain).unwrap(), "network: refused http://example.com/: plain HTTP isn't carried; use https");
        let failed = Decision { code: Some(codes::DNS_FAILED.into()), why: Some("DNS lookup failed (does this node have internet?)".into()), ..d("failed", "static.crates.io", 443) };
        assert_eq!(live_line(&failed).unwrap(), "network: couldn't reach static.crates.io:443: DNS lookup failed (does this node have internet?)");
        assert_eq!(live_line(&d("open", "github.com", 443)), None);
        let mut live = Live::default();
        let shown: Vec<Option<String>> = (0..5).map(|_| live.line(&r)).collect();
        assert!(shown[0].is_some() && shown[1].is_some());
        assert!(shown[2].as_deref().unwrap().ends_with("(more for gitlab.office.lan:443 are only counted, in the summary at the end)"));
        assert_eq!((shown[3].as_deref(), shown[4].as_deref()), (None, None));
        assert!(live.line(&lit).is_some(), "another host:port has its own count");
    }

    #[test]
    fn the_log_lines_parse_back_and_bad_lines_are_skipped() {
        let lines: Vec<String> = sample().iter().map(|x| serde_json::to_string(x).unwrap()).collect();
        let text = format!("{}\nnot json\n{{\"ev\":\"dropped\",\"t\":1,\"n\":4}}\n", lines.join("\n"));
        let back = parse_lines(&text);
        assert_eq!(back.len(), sample().len() + 1);
        assert_eq!(&back[..sample().len()], &sample()[..]);
        assert_eq!(fold(&back).dropped, 4);
        let j: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(j, serde_json::json!({"t": j["t"], "ev": "close", "host": "static.crates.io", "port": 443, "in": 5 << 20, "out": 2048}));
    }

    #[test]
    fn bytes_read_like_people_write_them() {
        assert_eq!(human_bytes(412), "412 B");
        assert_eq!(human_bytes(96 * 1024), "96 KB");
        assert_eq!(human_bytes(4_300_000), "4.1 MB");
        assert_eq!(human_bytes(1_288_490_189), "1.2 GB");
    }
}
