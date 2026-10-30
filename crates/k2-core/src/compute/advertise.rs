//! "Machines you can use" (Rosson 2026-10-11), the node machine's half:
//! while "Make this computer a node" is on, this server tells the K2
//! Connect control plane that it offers compute (node key, CPU, RAM, disk,
//! enrollment state), so the owner's other servers can list it and switch
//! it on with one click. The control plane answers with **enroll offers**:
//! one-click tickets a controller on the same account asked it to deliver.
//!
//! Wire contract: `.k2/prds/compute-oneclick-k2-connect-spec-v2.md`
//! (`POST /compute/advertise`, tunnel bearer token). Until K2 Connect
//! ships the route it answers 404/405 and the loop backs off for hours
//! ([`Posted::Unsupported`]); nothing else changes.
//!
//! Pure parts here (payload, offer decision, hardware probe, HTTP call);
//! the loop and applying an offer live in `k2-daemon`
//! (`compute_advertise`).

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Body schema version.
pub const VERSION: u32 = 1;
/// Poll while waiting to be enrolled (an offer may be on its way).
pub const WAITING_POLL_SECS: u64 = 20;
/// Poll while enrolled (only facts change).
pub const ENROLLED_POLL_SECS: u64 = 120;
/// Bounds on the control plane's `next_poll_secs`.
pub const MIN_POLL_SECS: u64 = 15;
pub const MAX_POLL_SECS: u64 = 900;
/// Back-off after a 404/405 (the route isn't there yet).
pub const UNSUPPORTED_BACKOFF_SECS: u64 = 6 * 3600;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// What this machine has (best effort; 0 = unknown).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Hardware {
    pub cpu_cores: u64,
    pub mem_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_free_bytes: u64,
}

fn meminfo_total() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
}

fn sysctl_memsize() -> Option<u64> {
    let out = crate::bg_proc::command("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// `(free, total)` bytes of the filesystem holding `path`.
#[cfg(unix)]
fn disk(path: &Path) -> (u64, u64) {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()) else {
        return (0, 0);
    };
    // SAFETY: statvfs fills the zeroed struct; we read it only on success.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return (0, 0);
    }
    let frsize = st.f_frsize as u64;
    (st.f_bavail as u64 * frsize, st.f_blocks as u64 * frsize)
}

#[cfg(windows)]
fn disk(path: &Path) -> (u64, u64) {
    crate::fs_commands::win_volume(path).map(|v| (v.avail, v.total)).unwrap_or((0, 0))
}

#[cfg(not(any(unix, windows)))]
fn disk(_path: &Path) -> (u64, u64) {
    (0, 0)
}

/// Probe this machine. Disk is the filesystem holding the node's home
/// (where jobs run), else `/`.
pub fn probe(node_home: &Path) -> Hardware {
    let at = if node_home.exists() { node_home } else { Path::new("/") };
    let (free, total) = disk(at);
    Hardware {
        cpu_cores: std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(0),
        mem_bytes: meminfo_total().or_else(sysctl_memsize).unwrap_or(0),
        disk_total_bytes: total,
        disk_free_bytes: free,
    }
}

/// Host of a controller URL (`https://rosson.k2.dev/` → `rosson.k2.dev`).
pub fn host_of(url: &str) -> String {
    let s = url.trim().to_ascii_lowercase();
    let s = s.split_once("://").map(|(_, r)| r.to_string()).unwrap_or(s);
    let s = s.split('/').next().unwrap_or("").to_string();
    s.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h.to_string()).unwrap_or(s)
}

/// A controller host K2 Connect accepts: `^[a-z0-9.-]{1,253}$` (the
/// control plane's `valid_controller_host`). Anything else (an IPv6
/// literal like `[::1`, an underscore) would 400 every advertise, so the
/// node leaves the field out instead.
pub fn valid_controller_host(s: &str) -> bool {
    (1..=253).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
}

/// A node name from a subdomain label: `[a-z0-9-]`, alphanumeric first,
/// at most 32.
pub fn name_from_label(label: &str) -> String {
    let mut s: String = label
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    while s.starts_with('-') {
        s.remove(0);
    }
    s.truncate(32);
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() {
        "node".into()
    } else {
        s
    }
}

/// The answer to one offer, sent back on the next advertise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// One enroll offer from the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Offer {
    pub id: String,
    pub controller_sub: String,
    pub ticket: String,
    #[serde(default)]
    pub exp: i64,
}

/// The control plane's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Answer {
    #[serde(default)]
    pub next_poll_secs: Option<u64>,
    #[serde(default)]
    pub offers: Vec<Offer>,
}

/// Node states that mean "enrolled (or enrolling) with some controller".
fn enrolled_state(s: &str) -> bool {
    matches!(s, "pending" | "connecting" | "online" | "offline")
}

/// The advertise body for this machine, or `None` when it is not a node
/// (nothing installed or the node service hasn't written its key yet).
/// `local_status` is `compute::local::status()`; `sub` this server's label.
pub fn payload(local_status: &serde_json::Value, sub: &str, hw: &Hardware, acks: &[Ack], k2_version: &str) -> Option<serde_json::Value> {
    if local_status["installed"] != true {
        return None;
    }
    let node = &local_status["node"];
    let fp = node["node_fp"].as_str().map(str::trim).filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))?;
    let state = node["state"].as_str().unwrap_or("unenrolled");
    let controller = node["controller"].as_str().map(host_of).filter(|h| valid_controller_host(h) && enrolled_state(state));
    let name = node["name"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| name_from_label(sub));
    let control = local_status["control"]["state"]
        .as_str()
        .or_else(|| node["control"].as_str())
        .unwrap_or("active");
    let sas = if state == "pending" { node["sas"].as_str().map(|s| s.replace(' ', "")) } else { None };
    Some(serde_json::json!({
        "v": VERSION,
        "enabled": true,
        "node_fp": fp.to_ascii_lowercase(),
        "node_name": name,
        "node_state": state,
        "controller": controller,
        "control": control,
        "sas": sas,
        "os": crate::compute::local::platform(),
        "arch": std::env::consts::ARCH,
        "cpu_cores": hw.cpu_cores,
        "mem_bytes": hw.mem_bytes,
        "disk_total_bytes": hw.disk_total_bytes,
        "disk_free_bytes": hw.disk_free_bytes,
        "node_version": node["version"].as_str(),
        "k2_version": k2_version,
        "acks": acks,
    }))
}

/// "This machine stopped offering compute" (deletes the row).
pub fn off_payload() -> serde_json::Value {
    serde_json::json!({ "v": VERSION, "enabled": false })
}

/// How soon to advertise again.
pub fn next_poll(answer: Option<&Answer>, local_status: &serde_json::Value) -> Duration {
    let state = local_status["node"]["state"].as_str().unwrap_or("unenrolled");
    let default = if enrolled_state(state) && state != "pending" { ENROLLED_POLL_SECS } else { WAITING_POLL_SECS };
    let secs = answer.and_then(|a| a.next_poll_secs).unwrap_or(default).clamp(MIN_POLL_SECS, MAX_POLL_SECS);
    Duration::from_secs(secs)
}

/// What to do with one offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Enroll with this ticket (`local/enable` with it).
    Apply { controller_url: String, name: String, ticket: String },
    /// Answer without enrolling (`ok` = nothing to do, not an error).
    Skip { ok: bool, code: &'static str },
}

/// Decide an offer against this node's state. Never trusts the offer's
/// own fields over the ticket's: the ticket names the controller, this
/// node's key and the name, and the controller verifies its signature.
pub fn decide(offer: &Offer, local_status: &serde_json::Value, now: i64) -> Decision {
    let Ok(t) = crate::compute::proto::ticket::Ticket::decode(&offer.ticket) else {
        return Decision::Skip { ok: false, code: "bad_offer" };
    };
    let sub = t.controller_sub.trim().to_ascii_lowercase();
    let label_ok = !sub.is_empty() && sub.len() <= 63 && sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !label_ok || !offer.controller_sub.trim().eq_ignore_ascii_case(&sub) {
        return Decision::Skip { ok: false, code: "bad_offer" };
    }
    if t.exp <= now {
        return Decision::Skip { ok: false, code: "offer_expired" };
    }
    let node = &local_status["node"];
    let own_fp = node["node_fp"].as_str().unwrap_or("").trim().to_ascii_lowercase();
    if own_fp.is_empty() || t.node_fp.trim().to_ascii_lowercase() != own_fp {
        return Decision::Skip { ok: false, code: "wrong_node" };
    }
    if !crate::compute::enroll::valid_name(&t.node_name) {
        return Decision::Skip { ok: false, code: "bad_offer" };
    }
    let host = format!("{sub}.k2.dev");
    let state = node["state"].as_str().unwrap_or("unenrolled");
    if enrolled_state(state) {
        let current = node["controller"].as_str().map(host_of).unwrap_or_default();
        if current == host {
            return Decision::Skip { ok: true, code: "already_enrolled" };
        }
        // Never move a node between controllers on an offer: its owner
        // removes it from the other server first.
        return Decision::Skip { ok: false, code: "enrolled_elsewhere" };
    }
    if local_status["enrollRequestPending"] == true {
        return Decision::Skip { ok: false, code: "busy" };
    }
    Decision::Apply { controller_url: format!("https://{host}"), name: t.node_name, ticket: offer.ticket.clone() }
}

/// Result of one advertise call.
#[derive(Debug)]
pub enum Posted {
    Answered(Answer),
    /// 404/405: this control plane has no `/compute/advertise` yet.
    Unsupported,
    /// 400: the control plane refused this body (e.g. an ack it won't
    /// take). Resending the same body would fail forever, so the loop
    /// drops its pending acks.
    Refused(String),
}

/// `POST <control plane>/compute/advertise` with the tunnel bearer token.
pub fn post(token: &str, body: &serde_json::Value) -> Result<Posted, String> {
    crate::airgap::refuse()?;
    let t = token.trim();
    if t.is_empty() {
        return Err("no tunnel token".into());
    }
    let url = format!("{}/compute/advertise", crate::tunnel::subdomains::control_plane_base());
    let resp = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))?
        .post(&url)
        .header("Authorization", format!("Bearer {t}"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status == 404 || status == 405 {
        return Ok(Posted::Unsupported);
    }
    if status == 400 {
        let snippet: String = text.chars().take(200).collect();
        return Ok(Posted::Refused(format!("POST {url}: HTTP 400: {snippet}")));
    }
    if !(200..300).contains(&status) {
        let snippet: String = text.chars().take(200).collect();
        return Err(format!("POST {url}: HTTP {status}: {snippet}"));
    }
    if text.trim().is_empty() {
        return Ok(Posted::Answered(Answer::default()));
    }
    serde_json::from_str(&text).map(Posted::Answered).map_err(|e| format!("parse /compute/advertise answer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FP: &str = "abababababababababababababababababababababababababababababababab";

    fn status(state: &str, controller: Option<&str>) -> serde_json::Value {
        json!({
            "installed": true,
            "node": {"node_fp": FP, "state": state, "controller": controller, "name": "mini-1", "version": "0.45.4", "sas": "123 456"},
            "control": {"state": "active"},
            "enrollRequestPending": false,
        })
    }

    fn ticket(sub: &str, fp: &str, name: &str, exp: i64) -> String {
        crate::compute::ticket::mint_with_seed(&[7u8; 32], "k2-compute-ticket-2026-10", sub, fp, name, exp, "AAAAAAAAAAAAAAAAAAAAAA").unwrap().1
    }

    fn offer(sub: &str, t: String) -> Offer {
        Offer { id: "o1".into(), controller_sub: sub.into(), ticket: t, exp: 2_000_000_000 }
    }

    #[test]
    fn hosts_and_names() {
        assert_eq!(host_of("https://Rosson.k2.dev/"), "rosson.k2.dev");
        assert_eq!(host_of("http://10.0.0.5:38471/x"), "10.0.0.5");
        assert_eq!(host_of("rosson.k2.dev"), "rosson.k2.dev");
        assert_eq!(name_from_label("Mini_1"), "mini-1");
        assert_eq!(name_from_label("--"), "node");
        assert_eq!(name_from_label(&"a".repeat(40)).len(), 32);
    }

    #[test]
    fn payload_only_for_an_installed_node_with_a_key() {
        let hw = Hardware { cpu_cores: 8, mem_bytes: 1, disk_total_bytes: 2, disk_free_bytes: 3 };
        assert!(payload(&json!({"installed": false}), "z13", &hw, &[], "1").is_none());
        assert!(payload(&json!({"installed": true, "node": null}), "z13", &hw, &[], "1").is_none());
        assert!(payload(&json!({"installed": true, "node": {"node_fp": "short"}}), "z13", &hw, &[], "1").is_none());

        let acks = vec![Ack { id: "o1".into(), ok: true, code: None }];
        let p = payload(&status("online", Some("https://rosson.k2.dev")), "z13", &hw, &acks, "0.45.4").unwrap();
        assert_eq!(p["enabled"], true);
        assert_eq!(p["node_fp"], FP);
        assert_eq!(p["controller"], "rosson.k2.dev");
        assert_eq!(p["cpu_cores"], 8);
        assert_eq!(p["sas"], serde_json::Value::Null, "the code is only sent while pending");
        assert_eq!(p["acks"][0]["id"], "o1");
        assert!(p["acks"][0].get("code").is_none());

        let p = payload(&status("pending", Some("https://rosson.k2.dev")), "z13", &hw, &[], "0.45.4").unwrap();
        assert_eq!(p["sas"], "123456");

        let mut s = status("unenrolled", Some("https://old.k2.dev"));
        s["node"]["name"] = serde_json::Value::Null;
        let p = payload(&s, "Z13_flow", &hw, &[], "0.45.4").unwrap();
        assert_eq!(p["controller"], serde_json::Value::Null, "no controller unless enrolled");
        assert_eq!(p["node_name"], "z13-flow");
        assert_eq!(off_payload()["enabled"], false);
    }

    #[test]
    fn controller_is_sent_only_when_connect_accepts_the_host() {
        let hw = Hardware::default();
        for (url, want) in [
            ("https://rosson.k2.dev", json!("rosson.k2.dev")),
            ("http://10.0.0.5:38471", json!("10.0.0.5")),
            ("http://[::1]:38471", serde_json::Value::Null),
            ("https://under_score.k2.dev", serde_json::Value::Null),
        ] {
            let p = payload(&status("online", Some(url)), "z13", &hw, &[], "1").unwrap();
            assert_eq!(p["controller"], want, "{url}");
        }
        assert!(valid_controller_host("rosson.k2.dev"));
        assert!(!valid_controller_host(""));
        assert!(!valid_controller_host(&"a".repeat(254)));
        assert!(!valid_controller_host("Rosson.k2.dev"));
    }

    #[test]
    fn offers_apply_only_to_an_unenrolled_node_with_its_own_key() {
        let now = 1_800_000_000;
        let good = offer("rosson", ticket("rosson", FP, "z13", now + 600));
        match decide(&good, &status("unenrolled", None), now) {
            Decision::Apply { controller_url, name, .. } => {
                assert_eq!(controller_url, "https://rosson.k2.dev");
                assert_eq!(name, "z13");
            }
            d => panic!("expected apply, got {d:?}"),
        }
        assert!(matches!(decide(&good, &status("revoked", Some("https://rosson.k2.dev")), now), Decision::Apply { .. }));
        assert_eq!(decide(&good, &status("online", Some("https://rosson.k2.dev")), now), Decision::Skip { ok: true, code: "already_enrolled" });
        assert_eq!(decide(&good, &status("offline", Some("https://other.k2.dev")), now), Decision::Skip { ok: false, code: "enrolled_elsewhere" });
        let mut busy = status("unenrolled", None);
        busy["enrollRequestPending"] = json!(true);
        assert_eq!(decide(&good, &busy, now), Decision::Skip { ok: false, code: "busy" });

        let expired = offer("rosson", ticket("rosson", FP, "z13", now - 1));
        assert_eq!(decide(&expired, &status("unenrolled", None), now), Decision::Skip { ok: false, code: "offer_expired" });
        let other_key = offer("rosson", ticket("rosson", &"cd".repeat(32), "z13", now + 600));
        assert_eq!(decide(&other_key, &status("unenrolled", None), now), Decision::Skip { ok: false, code: "wrong_node" });
        let mislabelled = offer("evil", ticket("rosson", FP, "z13", now + 600));
        assert_eq!(decide(&mislabelled, &status("unenrolled", None), now), Decision::Skip { ok: false, code: "bad_offer" });
        let garbage = Offer { id: "x".into(), controller_sub: "rosson".into(), ticket: "!!".into(), exp: 0 };
        assert_eq!(decide(&garbage, &status("unenrolled", None), now), Decision::Skip { ok: false, code: "bad_offer" });
    }

    #[test]
    fn poll_cadence_follows_state_and_is_bounded() {
        assert_eq!(next_poll(None, &status("unenrolled", None)).as_secs(), WAITING_POLL_SECS);
        assert_eq!(next_poll(None, &status("pending", Some("x"))).as_secs(), WAITING_POLL_SECS);
        assert_eq!(next_poll(None, &status("online", Some("x"))).as_secs(), ENROLLED_POLL_SECS);
        let a = Answer { next_poll_secs: Some(1), offers: vec![] };
        assert_eq!(next_poll(Some(&a), &status("online", None)).as_secs(), MIN_POLL_SECS);
        let a = Answer { next_poll_secs: Some(99_999), offers: vec![] };
        assert_eq!(next_poll(Some(&a), &status("online", None)).as_secs(), MAX_POLL_SECS);
    }

    #[test]
    fn answers_parse_with_and_without_offers() {
        let a: Answer = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        assert!(a.offers.is_empty());
        let a: Answer = serde_json::from_str(r#"{"ok":true,"next_poll_secs":30,"offers":[{"id":"o","controller_sub":"rosson","ticket":"t","exp":5}]}"#).unwrap();
        assert_eq!(a.next_poll_secs, Some(30));
        assert_eq!(a.offers[0].controller_sub, "rosson");
    }

    #[test]
    fn probe_reports_cores_and_disk() {
        let hw = probe(Path::new("/definitely/not/here"));
        assert!(hw.cpu_cores >= 1);
        assert!(hw.disk_total_bytes > 0, "statvfs on / must work");
    }
}
