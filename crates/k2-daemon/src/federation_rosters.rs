//! `GET /cli/federation/peer-rosters` (0.46.1, eval-connections-pickers-v1
//! option b2): every agent a workspace can connect to, in one call, for the
//! one searchable Connections picker.
//!
//! The answer is a list of servers. "This server" comes first (the local
//! workspaces, for a local connection), then every TRUSTED paired server,
//! A to Z by the name shown (label, else subdomain), case-insensitive, then
//! by host. Each paired server is asked for its roster in parallel, each
//! with a [`PEER_ROSTERS_TIMEOUT`] cap, so an offline server costs at most
//! that and never holds up the others. Each server says how it went:
//!
//! - `ok`: it answered; `agents` is its roster, A to Z.
//! - `offline`: no answer (refused, unreachable, or the cap ran out).
//! - `rejected`: it answered 401/403 (it may no longer trust this server).
//! - `error`: anything else (another HTTP status, a bad body, air-gap
//!   refusing to dial a Connect host); `error` says what.
//!
//! `?peer=<selector>` asks one paired server only (the picker's Retry); the
//! "This server" entry is left out then.
//!
//! Pending and blocked pins are not listed (they can't be connected to).
//! A successful roster heals that peer's connection rows, as the
//! single-peer `peer-roster` route does. Auth: same as `peer-roster`
//! (owner/Admin or a scoped passport; the dispatcher and route policy gate).
//!
//! The order lives here, in the daemon, so every client shows the same
//! list: `handle_peers` (`k2 fed peers`, Settings → K2 Server →
//! Connections) and `peer-roster` use [`peers_in_order`] and
//! [`sort_roster_json`] too.

use std::time::Duration;

use crate::cli_response::CliResponse;
use crate::federation_routes::{
    gated_peer_base_url, get_peer_roster_typed, heal_rows_from_roster, peer_host,
    peer_selector_matches, RosterFetchError,
};
use k2_core::federation::{roster, FederationPeer, PeerStore, PeerTrust};

/// Per-server cap on one roster fetch for the picker.
pub const PEER_ROSTERS_TIMEOUT: Duration = Duration::from_secs(5);

/// The local entry's label.
pub const THIS_SERVER_LABEL: &str = "This server";

/// The name a client shows for a paired server: its label, else its
/// subdomain, else the start of its fingerprint.
pub(crate) fn peer_shown_label(p: &FederationPeer) -> String {
    let label = p.label.trim();
    if !label.is_empty() {
        return label.to_string();
    }
    let sub = p.subdomain.trim();
    if !sub.is_empty() {
        return sub.to_string();
    }
    p.fingerprint.chars().take(12).collect()
}

/// Case-insensitive sort key for a paired server: name shown, host,
/// fingerprint (the fingerprint only breaks exact ties).
fn peer_order_key(p: &FederationPeer) -> (String, String, String) {
    (peer_shown_label(p).to_lowercase(), peer_host(p).to_lowercase(), p.fingerprint.clone())
}

/// `peers` A to Z (see [`peer_order_key`]).
pub(crate) fn peers_in_order(peers: &[FederationPeer]) -> Vec<FederationPeer> {
    let mut out: Vec<(_, FederationPeer)> =
        peers.iter().map(|p| (peer_order_key(p), p.clone())).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter().map(|(_, p)| p).collect()
}

/// Case-insensitive sort key for an agent: the name shown (the workspace
/// name, else the handle), then the handle, then the address.
fn agent_order_key(a: &serde_json::Value) -> (String, String, String) {
    let s = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let handle = s("agent");
    let name = s("workspace_name");
    let shown = if name.is_empty() { handle.clone() } else { name };
    (shown.to_lowercase(), handle.to_lowercase(), s("address"))
}

/// Sort a JSON agent list A to Z (see [`agent_order_key`]).
fn sort_agents(agents: &mut [serde_json::Value]) {
    agents.sort_by_cached_key(agent_order_key);
}

/// Sort the agents of a roster body in place: the bare projection
/// (`{"agents":[…]}`) or the `peer-roster` wrapper (`{"roster":{…}}`).
/// Anything else is left alone.
pub(crate) fn sort_roster_json(body: &mut serde_json::Value) {
    if let Some(agents) = body.get_mut("agents").and_then(|v| v.as_array_mut()) {
        sort_agents(agents);
    } else if let Some(agents) = body
        .get_mut("roster")
        .and_then(|r| r.get_mut("agents"))
        .and_then(|v| v.as_array_mut())
    {
        sort_agents(agents);
    }
}

/// This server's workspaces as connection targets, A to Z. Every workspace
/// is listed (a local connection needs no remote-message switch); the
/// caller hides the one it is adding connections for.
fn local_agents() -> Result<Vec<serde_json::Value>, String> {
    let projects = k2_core::projects_ops::projects_list()?;
    let mut agents: Vec<serde_json::Value> = projects
        .into_iter()
        .map(|p| {
            let handle = p.handle.trim().to_ascii_lowercase();
            let agent = if !handle.is_empty() {
                handle
            } else {
                k2_core::workspace::agent_identity::resolve_agent_name(&p.path)
                    .map(|a| a.trim().to_ascii_lowercase())
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| p.name.trim().to_ascii_lowercase())
            };
            serde_json::json!({
                "workspace_id": p.id,
                "workspace_name": p.name,
                "agent": agent,
                "address": agent,
                "aliases": [],
            })
        })
        .collect();
    sort_agents(&mut agents);
    Ok(agents)
}

/// One paired server's outcome.
struct PeerOutcome {
    status: &'static str,
    error: Option<String>,
    agents: Vec<serde_json::Value>,
}

impl PeerOutcome {
    fn failed(status: &'static str, error: String) -> Self {
        PeerOutcome { status, error: Some(error), agents: Vec::new() }
    }
}

/// Ask one paired server for its roster (signed with `fp`/`ts`/`sig`).
fn fetch_one(peer: &FederationPeer, fp: &str, ts: i64, sig: &str, timeout: Duration) -> PeerOutcome {
    let base = match gated_peer_base_url(peer) {
        Ok(b) => b,
        Err(e) => return PeerOutcome::failed("error", e),
    };
    let body = match get_peer_roster_typed(&base, fp, ts, sig, timeout) {
        Ok(b) => b,
        Err(RosterFetchError::Transport { timed_out: true, .. }) => {
            return PeerOutcome::failed(
                "offline",
                format!("no answer within {} s", timeout.as_secs_f64().round()),
            )
        }
        Err(RosterFetchError::Transport { message, .. }) => {
            return PeerOutcome::failed("offline", message)
        }
        Err(RosterFetchError::Status(code @ (401 | 403))) => {
            return PeerOutcome::failed(
                "rejected",
                format!("it refused this server (HTTP {code}); it may no longer trust this server"),
            )
        }
        Err(e @ RosterFetchError::Status(_)) => return PeerOutcome::failed("error", e.to_string()),
        Err(RosterFetchError::Body(e)) => return PeerOutcome::failed("error", e),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return PeerOutcome::failed("error", format!("unreadable roster: {e}")),
    };
    let Some(list) = parsed.get("agents").and_then(|v| v.as_array()) else {
        return PeerOutcome::failed("error", "the answer had no agent list".to_string());
    };
    let mut agents = list.clone();
    // A7 S2: heal + bind this peer's rows, as the single-peer route does.
    let _ = heal_rows_from_roster(peer, &parsed);
    sort_agents(&mut agents);
    PeerOutcome { status: "ok", error: None, agents }
}

/// The route body, with the per-server cap as a parameter (tests use a
/// short one; the route uses [`PEER_ROSTERS_TIMEOUT`]). `Err` is a whole
/// route failure (the response to send).
pub(crate) fn peer_rosters_body(
    peer_selector: Option<&str>,
    timeout: Duration,
) -> Result<serde_json::Value, CliResponse> {
    let store = PeerStore::load().map_err(|e| {
        CliResponse::internal_error(format!("load peer store: {e}"))
    })?;
    let selector = peer_selector.map(str::trim).filter(|s| !s.is_empty());
    let trusted: Vec<FederationPeer> = store
        .list()
        .iter()
        .filter(|p| p.trust == PeerTrust::Trusted)
        .filter(|p| selector.is_none_or(|sel| peer_selector_matches(p, sel)))
        .cloned()
        .collect();
    if let Some(sel) = selector {
        if trusted.is_empty() {
            return Err(CliResponse {
                status: "404 Not Found",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("no trusted paired server matches '{sel}'") })
                    .to_string(),
            });
        }
    }
    let peers = peers_in_order(&trusted);

    // One signature serves every peer: the challenge is over OUR
    // fingerprint and the time, not the peer's.
    let signed = k2_core::federation::identity::load_or_create()
        .map_err(|e| format!("load keypair: {e}"))
        .and_then(|key| {
            let ts = chrono::Utc::now().timestamp();
            roster::sign_roster_request(&key, ts).map(|(fp, sig)| (fp, ts, sig))
        });

    let outcomes: Vec<PeerOutcome> = match &signed {
        Err(e) => peers.iter().map(|_| PeerOutcome::failed("error", e.clone())).collect(),
        Ok((fp, ts, sig)) => std::thread::scope(|scope| {
            let handles: Vec<_> = peers
                .iter()
                .map(|peer| scope.spawn(move || fetch_one(peer, fp, *ts, sig, timeout)))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        PeerOutcome::failed("error", "roster fetch panicked".to_string())
                    })
                })
                .collect()
        }),
    };

    let mut servers: Vec<serde_json::Value> = Vec::with_capacity(peers.len() + 1);
    if selector.is_none() {
        let local = match local_agents() {
            Ok(agents) => serde_json::json!({
                "kind": "local",
                "key": "local",
                "label": THIS_SERVER_LABEL,
                "host": "",
                "status": "ok",
                "error": null,
                "agents": agents,
            }),
            Err(e) => serde_json::json!({
                "kind": "local",
                "key": "local",
                "label": THIS_SERVER_LABEL,
                "host": "",
                "status": "error",
                "error": format!("list workspaces: {e}"),
                "agents": [],
            }),
        };
        servers.push(local);
    }
    for (peer, out) in peers.iter().zip(outcomes) {
        servers.push(serde_json::json!({
            "kind": "peer",
            "key": peer.fingerprint,
            "fingerprint": peer.fingerprint,
            "label": peer_shown_label(peer),
            "subdomain": peer.subdomain,
            "base_url": peer.base_url,
            "host": peer_host(peer),
            "repair_recommended": peer.pairing_version
                < k2_core::federation::pairing_v2::PAIRING_VERSION,
            "status": out.status,
            "error": out.error,
            "agents": out.agents,
        }));
    }
    Ok(serde_json::json!({
        "servers": servers,
        "timeout_secs": timeout.as_secs(),
    }))
}

/// `GET /cli/federation/peer-rosters[?peer=<selector>]` (see the module doc).
/// Blocking: run it on a blocking thread.
pub fn handle_peer_rosters(peer_selector: Option<&str>) -> CliResponse {
    match peer_rosters_body(peer_selector, PEER_ROSTERS_TIMEOUT) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(resp) => resp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn pin(label: &str, sub: &str, base: &str) -> FederationPeer {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = FederationPeer::pin(label, sub, key.public_key_pem()).unwrap();
        p.base_url = base.to_string();
        p
    }

    /// Save `peers` as Trusted with the roster capability; returns their fps.
    fn save_trusted(peers: Vec<FederationPeer>) -> Vec<String> {
        let mut store = PeerStore::default();
        let mut fps = Vec::new();
        for p in peers {
            let fp = store.upsert(p);
            store.set_trust(&fp, PeerTrust::Trusted);
            store.grant(&fp, "roster");
            fps.push(fp);
        }
        store.save().unwrap();
        fps
    }

    /// A port with nothing listening (bound, then closed).
    fn dead_port() -> u16 {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        l.local_addr().unwrap().port()
    }

    /// A server that accepts and never answers (a peer that hangs).
    fn hanging_port() -> u16 {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for s in l.incoming().flatten() {
                held.push(s); // keep it open, say nothing
            }
        });
        port
    }

    /// A server that answers every request with `status` and `body`.
    fn answering_port(status: &'static str, body: String) -> u16 {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        port
    }

    fn roster(entries: &[(&str, &str)]) -> String {
        let agents: Vec<serde_json::Value> = entries
            .iter()
            .map(|(name, handle)| {
                let id = uuid::Uuid::new_v4().to_string();
                serde_json::json!({
                    "workspace_id": id,
                    "workspace_name": name,
                    "agent": handle,
                    "address": format!("{id}::{handle}"),
                    "aliases": [],
                })
            })
            .collect();
        serde_json::json!({ "agents": agents }).to_string()
    }

    fn ok_body(r: Result<serde_json::Value, CliResponse>) -> serde_json::Value {
        match r {
            Ok(v) => v,
            Err(e) => panic!("peer-rosters failed: {} {}", e.status, e.body),
        }
    }

    fn labels(v: &serde_json::Value) -> Vec<String> {
        v["servers"].as_array().unwrap().iter().map(|s| s["label"].as_str().unwrap().to_string()).collect()
    }

    fn server<'a>(v: &'a serde_json::Value, label: &str) -> &'a serde_json::Value {
        v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["label"] == label)
            .unwrap_or_else(|| panic!("no server {label} in {v}"))
    }

    fn names(s: &serde_json::Value) -> Vec<String> {
        s["agents"].as_array().unwrap().iter().map(|a| a["workspace_name"].as_str().unwrap().to_string()).collect()
    }

    fn insert_project(name: &str, handle: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let path = std::env::temp_dir().join(format!("k2-rosters-{}", uuid::Uuid::new_v4()));
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), name, path.to_string_lossy(), handle],
        )
        .unwrap();
    }

    #[test]
    fn servers_and_agents_come_a_to_z_with_this_server_first() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let _base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
        insert_project("zeta desk", "zeta");
        insert_project("Alpha", "alpha");
        insert_project("billing", "billing");
        let rpm = answering_port(
            "200 OK",
            roster(&[("cortana", "cortana"), ("Argus", "argus"), ("billing", "billing-b"), ("Billing", "billing-a")]),
        );
        // Paired out of order, mixed case; one pending pin that must not show.
        let mut store = PeerStore::default();
        for (p, trust) in [
            (pin("aaa-pending", "pending", &format!("http://127.0.0.1:{}", dead_port())), PeerTrust::Pending),
            (pin("rpm", "rpm", &format!("http://127.0.0.1:{rpm}")), PeerTrust::Trusted),
            (pin("Mike", "mike", &format!("http://127.0.0.1:{}", dead_port())), PeerTrust::Trusted),
            (pin("alpha", "alpha", &format!("http://127.0.0.1:{}", dead_port())), PeerTrust::Trusted),
        ] {
            let fp = store.upsert(p);
            store.set_trust(&fp, trust);
        }
        store.save().unwrap();

        let v = ok_body(peer_rosters_body(None, Duration::from_secs(5)));
        assert_eq!(labels(&v), vec!["This server", "alpha", "Mike", "rpm"], "{v}");
        // The test DB is shared with other tests in this binary: check our
        // three come in order, and that the whole list is A to Z.
        let local = names(server(&v, "This server"));
        let ours: Vec<&str> = local
            .iter()
            .map(String::as_str)
            .filter(|n| ["Alpha", "billing", "zeta desk"].contains(n))
            .collect();
        assert_eq!(ours, vec!["Alpha", "billing", "zeta desk"], "{local:?}");
        let folded: Vec<String> = local.iter().map(|n| n.to_lowercase()).collect();
        let mut sorted = folded.clone();
        sorted.sort();
        assert_eq!(folded, sorted, "This server is A to Z: {local:?}");
        let rpm_s = server(&v, "rpm");
        assert_eq!(rpm_s["status"], "ok", "{rpm_s}");
        // Same name: the handle breaks the tie.
        assert_eq!(names(rpm_s), vec!["Argus", "Billing", "billing", "cortana"]);
        let handles: Vec<&str> = rpm_s["agents"].as_array().unwrap().iter().map(|a| a["agent"].as_str().unwrap()).collect();
        assert_eq!(handles, vec!["argus", "billing-a", "billing-b", "cortana"]);
        assert_eq!(server(&v, "Mike")["status"], "offline");
        assert!(server(&v, "Mike")["agents"].as_array().unwrap().is_empty());
        assert_eq!(v["timeout_secs"], 5);

        // handle_peers (k2 fed peers, Settings) lists pins in the same order.
        let peers: serde_json::Value =
            serde_json::from_str(&crate::federation_routes::handle_peers().body).unwrap();
        let order: Vec<&str> = peers["peers"].as_array().unwrap().iter().map(|p| p["label"].as_str().unwrap()).collect();
        assert_eq!(order, vec!["aaa-pending", "alpha", "Mike", "rpm"]);
    }

    #[test]
    fn statuses_say_offline_rejected_or_error_and_repair_is_flagged() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let _base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
        let refuses = answering_port("403 Forbidden", r#"{"error":"roster denied"}"#.into());
        let off = answering_port("404 Not Found", r#"{"error":"not found"}"#.into());
        let junk = answering_port("200 OK", "not json".into());
        let mut old = pin("old", "old", &format!("http://127.0.0.1:{}", dead_port()));
        old.pairing_version = 1;
        save_trusted(vec![
            pin("refuses", "refuses", &format!("http://127.0.0.1:{refuses}")),
            pin("off", "off", &format!("http://127.0.0.1:{off}")),
            pin("junk", "junk", &format!("http://127.0.0.1:{junk}")),
            old,
        ]);
        let v = ok_body(peer_rosters_body(None, Duration::from_secs(5)));
        assert_eq!(server(&v, "refuses")["status"], "rejected", "{v}");
        assert!(server(&v, "refuses")["error"].as_str().unwrap().contains("403"));
        assert_eq!(server(&v, "off")["status"], "error");
        assert!(server(&v, "off")["error"].as_str().unwrap().contains("HTTP 404"));
        assert_eq!(server(&v, "junk")["status"], "error");
        assert_eq!(server(&v, "old")["status"], "offline");
        assert_eq!(server(&v, "old")["repair_recommended"], true);
        assert_eq!(server(&v, "junk")["repair_recommended"], false);
    }

    #[test]
    fn each_server_is_capped_and_asked_in_parallel() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let _base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
        assert_eq!(PEER_ROSTERS_TIMEOUT, Duration::from_secs(5), "the picker's cap is 5 s");
        let ok = answering_port("200 OK", roster(&[("ops", "ops")]));
        save_trusted(vec![
            pin("hang-a", "hang-a", &format!("http://127.0.0.1:{}", hanging_port())),
            pin("hang-b", "hang-b", &format!("http://127.0.0.1:{}", hanging_port())),
            pin("hang-c", "hang-c", &format!("http://127.0.0.1:{}", hanging_port())),
            pin("fine", "fine", &format!("http://127.0.0.1:{ok}")),
        ]);
        let cap = Duration::from_millis(800);
        let t = std::time::Instant::now();
        let v = ok_body(peer_rosters_body(None, cap));
        let took = t.elapsed();
        assert!(took >= cap, "a hanging server is waited for up to the cap: {took:?}");
        assert!(
            took < cap * 2,
            "three hanging servers at once cost one cap, not three: {took:?}"
        );
        for l in ["hang-a", "hang-b", "hang-c"] {
            let s = server(&v, l);
            assert_eq!(s["status"], "offline", "{s}");
            assert!(s["error"].as_str().unwrap().contains("no answer within"), "{s}");
        }
        assert_eq!(server(&v, "fine")["status"], "ok");
        assert_eq!(names(server(&v, "fine")), vec!["ops"]);
    }

    #[test]
    fn one_peer_by_selector_leaves_this_server_out() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let _base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
        let ok = answering_port("200 OK", roster(&[("ops", "ops")]));
        let fps = save_trusted(vec![
            pin("one", "one", &format!("http://127.0.0.1:{ok}")),
            pin("two", "two", &format!("http://127.0.0.1:{}", dead_port())),
        ]);
        let v = ok_body(peer_rosters_body(Some(&fps[0]), Duration::from_secs(5)));
        assert_eq!(labels(&v), vec!["one"], "{v}");
        let Err(missing) = peer_rosters_body(Some("nobody"), Duration::from_secs(5)) else {
            panic!("an unknown selector must be a 404");
        };
        assert_eq!(missing.status, "404 Not Found", "{}", missing.body);
    }

    #[test]
    fn sort_roster_json_sorts_both_shapes() {
        let mut bare: serde_json::Value = serde_json::from_str(&roster(&[("b", "b"), ("A", "a")])).unwrap();
        sort_roster_json(&mut bare);
        assert_eq!(bare["agents"][0]["agent"], "a");
        let mut wrapped = serde_json::json!({ "peer": "x", "roster": bare.clone() });
        wrapped["roster"]["agents"].as_array_mut().unwrap().reverse();
        sort_roster_json(&mut wrapped);
        assert_eq!(wrapped["roster"]["agents"][0]["agent"], "a");
    }
}
