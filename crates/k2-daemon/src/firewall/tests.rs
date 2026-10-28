//! The public-ports view and the firewall routes (PRD §11.1 tests 1–6).

use std::collections::HashMap;

use super::*;
use crate::compute_routes::Who;
use k2_core::firewall::Proto;

fn base() -> Inputs {
    Inputs { os: "linux", daemon: "0.46.6".into(), now: 1_793_145_600, ..Inputs::default() }
}

fn has(rows: &[PortEntry], port: u16, proto: Proto) -> bool {
    rows.iter().any(|r| r.port == port && r.proto == proto)
}

fn row(rows: &[PortEntry], port: u16) -> &PortEntry {
    rows.iter().find(|r| r.port == port).unwrap_or_else(|| panic!("no row for {port}: {rows:?}"))
}

/// Test 1 + PA-A4/A6: mail on → 25/465/587/993/143/80 public and 4190 in
/// `knownNotPublic`; mail off → none of them.
#[test]
fn mail_ports_follow_hosted_mail() {
    let mut i = base();
    let off = compute(&i);
    for p in [25, 465, 587, 993, 143, 80, 4190, 995, 8080] {
        assert!(!has(&off.public, p, Proto::Tcp) && !has(&off.known_not_public, p, Proto::Tcp), "{p} with mail off");
    }
    i.mail_on = true;
    let on = compute(&i);
    for p in [25, 465, 587, 993, 143, 80] {
        assert_eq!(row(&on.public, p).source, "hostmail", "{p}");
    }
    for p in [4190, 995, 8080] {
        assert!(has(&on.known_not_public, p, Proto::Tcp), "{p} known not public");
        assert!(!has(&on.public, p, Proto::Tcp), "{p} never public");
    }
    assert_eq!(row(&on.public, 443).source, "hostmail", "Stalwart holds :443 itself with the front off");
}

/// Test 2: front off → no 443 (unless Skin Direct or mail holds it); on →
/// 443 from `front443`, and only 443 for Path A (no :80, PA-A14).
#[test]
fn the_front_owns_443() {
    let mut i = base();
    assert!(!has(&compute(&i).public, 443, Proto::Tcp));
    i.skin_direct_listen = Some(":443".into());
    let skin = compute(&i);
    assert_eq!(row(&skin.public, 443).source, "skin-direct");
    assert_eq!(row(&skin.public, 80).source, "skin-direct");
    i.skin_direct_listen = None;
    i.front_enabled = true;
    let front = compute(&i);
    assert_eq!(row(&front.public, 443).source, "front443");
    assert!(!has(&front.public, 80, Proto::Tcp), "Path A adds no :80 in 0.46.6");
    // Mail behind the front: one 443 row, the front's.
    i.mail_on = true;
    i.mail_behind_front = true;
    let both = compute(&i);
    assert_eq!(both.public.iter().filter(|r| r.port == 443).count(), 1);
    assert_eq!(row(&both.public, 443).source, "front443");
}

/// PA-A5, A7 and the LAN listen.
#[test]
fn voice_skin_fallback_and_lan_listen() {
    let mut i = base();
    i.voice_installed = true;
    i.voice_tier_direct = true;
    let v = compute(&i);
    assert!(has(&v.public, 7881, Proto::Tcp) && has(&v.public, 7882, Proto::Udp) && has(&v.public, 5349, Proto::Tcp), "{:?}", v.public);
    assert!(!has(&v.public, 7880, Proto::Tcp), "signalling is loopback");
    i.voice_tier_direct = false;
    assert!(!has(&compute(&i).public, 5349, Proto::Tcp), "TURN rides :443 on the passthrough tier");
    let mut i = base();
    i.skin_direct_listen = Some("0.0.0.0:38472".into());
    assert!(row(&compute(&i).public, 38472).why.contains("fallback"));
    let mut i = base();
    i.lan_port = Some(38471);
    let v = compute(&i);
    assert!(has(&v.known_not_public, 38471, Proto::Tcp) && !has(&v.public, 38471, Proto::Tcp));
}

/// Test 3 (view half) + test 4: SSH is always present; the hash is stable
/// across reads, moves on an owner port or SSH narrowing, and ignores
/// `knownNotPublic`.
#[test]
fn ssh_is_always_listed_and_the_hash_tracks_the_allow_list() {
    let i = base();
    let a = compute(&i);
    assert_eq!(a.public[0].source, "ssh");
    assert_eq!(a.public[0].allow_from, vec!["any".to_string()]);
    assert_eq!(a.hash, compute(&i).hash, "stable across reads");
    let mut j = base();
    j.lan_port = Some(38471);
    j.now += 99;
    assert_eq!(compute(&j).hash, a.hash, "knownNotPublic and time don't move it");
    let mut k = base();
    k.cfg.open.push(k2_core::firewall::OpenPort {
        port: 6000,
        proto: Proto::Udp,
        label: "game".into(),
        allow_from: vec!["198.51.100.0/24".into()],
        added_at: 0,
        added_by: "owner".into(),
    });
    let kv = compute(&k);
    assert_ne!(kv.hash, a.hash);
    assert_eq!(row(&kv.public, 6000).allow_from, vec!["198.51.100.0/24".to_string()]);
    let mut s = base();
    s.cfg.ssh.allow_from = vec!["203.0.113.0/24".into()];
    assert_ne!(compute(&s).hash, a.hash);
}

#[test]
fn the_contract_shape_is_stable() {
    let mut i = base();
    i.label = Some("box".into());
    let v = serde_json::to_value(compute(&i)).unwrap();
    for k in ["version", "generatedAt", "hash", "daemon", "box", "public", "knownNotPublic", "upstream"] {
        assert!(v.get(k).is_some(), "{k} missing: {v}");
    }
    assert_eq!(v["version"], 1);
    assert_eq!(v["upstream"]["managedBy"], "owner");
    assert_eq!(v["box"]["label"], "box");
    assert_eq!(v["public"][0]["allowFrom"][0], "any");
}

// ── routes ───────────────────────────────────────────────────────────

struct Env {
    _home: k2_core::test_env::TempHome,
}

fn env() -> Env {
    let home = k2_core::test_env::TempHome::new();
    crate::apps::custom_domain_flow::set_test_box_ips(Some(Vec::new()));
    crate::apps::guard::reload();
    Env { _home: home }
}

fn call(who: &Who, post: bool, path: &str, body: serde_json::Value) -> (String, serde_json::Value) {
    let r = routes::handle(path, who, post, body.to_string().as_bytes(), &HashMap::new(), 38471);
    let v = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::String(r.body.clone()));
    (r.status.to_string(), v)
}

fn agent() -> Who {
    Who::Agent { project_id: "ws-1".into(), session_id: "s-1".into() }
}

/// Test 3 (routes half): `close 22/tcp` and an empty `ssh allow` are
/// refused; open/close of an owner port moves the hash.
#[test]
fn ssh_can_be_narrowed_never_closed() {
    let _e = env();
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/close", serde_json::json!({"port": "22/tcp"}));
    assert_eq!(st, "409 Conflict", "{v}");
    assert_eq!(v["error"]["code"], "ssh_stays_open");
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/ssh", serde_json::json!({"allow": []}));
    assert_eq!(st, "400 Bad Request", "{v}");
    assert_eq!(v["error"]["code"], "ssh_needs_an_address");
    let (_, before) = call(&Who::Owner, false, "/cli/firewall/ports", serde_json::json!({}));
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/open", serde_json::json!({"port": "6000/udp", "label": "game server"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_ne!(v["hash"], before["hash"]);
    let (_, after) = call(&Who::Owner, false, "/cli/firewall/ports", serde_json::json!({}));
    assert!(after["public"].as_array().unwrap().iter().any(|r| r["port"] == 6000 && r["proto"] == "udp" && r["source"] == "owner"), "{after}");
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/close", serde_json::json!({"port": "6000/udp"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["hash"], before["hash"], "back to where it was");
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/ssh", serde_json::json!({"allow": ["198.51.100.7/24"]}));
    assert_eq!(st, "200 OK", "{v}");
    let (_, p) = call(&Who::Owner, false, "/cli/firewall/ports", serde_json::json!({}));
    assert_eq!(p["public"][0]["allowFrom"], serde_json::json!(["198.51.100.0/24"]));
}

/// Test 5 (PA-A12): an agent's change is refused `owner_only` with the
/// owner's command; the view is unchanged. Agents may read.
#[test]
fn an_agent_never_changes_the_firewall() {
    let _e = env();
    let (_, before) = call(&agent(), false, "/cli/firewall/ports", serde_json::json!({}));
    assert!(before["hash"].as_str().is_some_and(|h| h.starts_with("sha256:")), "agents read: {before}");
    for (path, body) in [
        ("/cli/firewall/open", serde_json::json!({"port": "6000/udp", "label": "x"})),
        ("/cli/firewall/ssh", serde_json::json!({"any": true})),
        ("/cli/firewall/close", serde_json::json!({"port": "6000/udp"})),
        ("/cli/firewall/guard", serde_json::json!({"on": false})),
        ("/cli/firewall/upstream", serde_json::json!({"managedBy": "k2"})),
        ("/cli/front/passthrough/add", serde_json::json!({"host": "a.example.com", "to": "127.0.0.1:9443"})),
    ] {
        let (st, v) = call(&agent(), true, path, body);
        assert_eq!(st, "403 Forbidden", "{path}: {v}");
        assert_eq!(v["error"]["code"], "owner_only", "{path}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("The owner runs: k2 "), "{path}: {v}");
    }
    let (_, after) = call(&agent(), false, "/cli/firewall/ports", serde_json::json!({}));
    assert_eq!(after["hash"], before["hash"]);
    // A Member login can't read or change it; an App pass gets nothing.
    let member = Who::Login { username: "m".into(), role: k2_core::connect_users::Role::Member };
    assert_eq!(call(&member, false, "/cli/firewall/ports", serde_json::json!({})).0, "403 Forbidden");
    assert_eq!(call(&Who::Wrong, false, "/cli/firewall", serde_json::json!({})).0, "403 Forbidden");
}

/// Test 6: every POST route answers 405 to GET (and the reads to POST).
#[test]
fn post_routes_refuse_get() {
    let _e = env();
    for p in routes::POST_ROUTES {
        assert_eq!(call(&Who::Owner, false, p, serde_json::json!({})).0, "405 Method Not Allowed", "{p}");
    }
    for p in ["/cli/firewall", "/cli/firewall/ports", "/cli/firewall/rules", "/cli/front/passthrough"] {
        assert_eq!(call(&Who::Owner, true, p, serde_json::json!({})).0, "405 Method Not Allowed", "{p}");
    }
}

/// KW-Q3/Q2: K2 Web's ack records the applied hash and flips managedBy;
/// a malformed hash is refused; an Admin login can't record it.
#[test]
fn the_upstream_ack_is_the_owners() {
    let _e = env();
    let (_, v) = call(&Who::Owner, false, "/cli/firewall/ports", serde_json::json!({}));
    let hash = v["hash"].as_str().unwrap().to_string();
    let admin = Who::Login { username: "a".into(), role: k2_core::connect_users::Role::Admin };
    let (st, _) = call(&admin, true, "/cli/firewall/upstream", serde_json::json!({"hash": hash, "provider": "hetzner"}));
    assert_eq!(st, "403 Forbidden");
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/upstream", serde_json::json!({"hash": "sha256:nope", "provider": "x"}));
    assert_eq!(st, "400 Bad Request", "{v}");
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/upstream", serde_json::json!({"hash": hash, "provider": "hetzner", "appliedAt": 1_700_000_000}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["upstream"]["managedBy"], "k2");
    assert_eq!(v["upstream"]["applied"]["current"], true);
    let (_, s) = call(&Who::Owner, false, "/cli/firewall", serde_json::json!({}));
    assert!(s["applies"].as_str().unwrap().starts_with("K2 applies"), "{s}");
}

#[test]
fn rules_render_for_unmanaged_boxes() {
    let _e = env();
    let mut params = HashMap::new();
    params.insert("format".to_string(), "ufw".to_string());
    let r = routes::handle("/cli/firewall/rules", &Who::Owner, false, b"", &params, 38471);
    assert_eq!(r.status, "200 OK");
    assert!(r.body.contains("sudo ufw allow 22/tcp"), "{}", r.body);
    params.insert("format".to_string(), "iptables".to_string());
    assert_eq!(routes::handle("/cli/firewall/rules", &Who::Owner, false, b"", &params, 38471).status, "400 Bad Request");
}

/// The guard settings: box-wide on/off and limits; per App needs a real App.
#[test]
fn guard_settings_round_trip() {
    let _e = env();
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/guard", serde_json::json!({"connsPerIp": 32, "writesPerMin": 60}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(crate::apps::guard::config().conns(), Some(32), "reloaded into the live guard");
    assert_eq!(crate::apps::guard::config().writes_for("any-app"), Some(60));
    let (st, _) = call(&Who::Owner, true, "/cli/firewall/guard", serde_json::json!({"connsPerIp": 0}));
    assert_eq!(st, "400 Bad Request");
    let (st, v) = call(&Who::Owner, true, "/cli/firewall/guard", serde_json::json!({"on": false}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(crate::apps::guard::config().conns(), None);
    let (st, _) = call(&Who::Owner, true, "/cli/firewall/guard", serde_json::json!({"reset": true}));
    assert_eq!(st, "200 OK");
    assert_eq!(crate::apps::guard::config().conns(), Some(64));
}

/// Q5(b): passthrough names validate (loopback only, never K2's ports) and
/// persist; status names them.
#[test]
fn passthrough_names_validate_and_persist() {
    let _e = env();
    for (body, code) in [
        (serde_json::json!({"host": "shop.example.com", "to": "10.0.0.5:443"}), "upstream_not_loopback"),
        (serde_json::json!({"host": "shop.example.com", "to": "127.0.0.1:8443"}), "upstream_port_reserved"),
        (serde_json::json!({"host": "shop.example.com", "to": "127.0.0.1:38471"}), "upstream_port_reserved"),
        (serde_json::json!({"host": "not a host", "to": "127.0.0.1:9443"}), "bad_host"),
    ] {
        let (_, v) = call(&Who::Owner, true, "/cli/front/passthrough/add", body.clone());
        assert_eq!(v["error"]["code"], code, "{body}: {v}");
    }
    let (st, v) = call(&Who::Owner, true, "/cli/front/passthrough/add", serde_json::json!({"host": "Shop.Example.com", "to": "127.0.0.1:9443"}));
    assert_eq!(st, "200 OK", "{v}");
    let (st, _) = call(&Who::Owner, true, "/cli/front/passthrough/add", serde_json::json!({"host": "*", "to": "127.0.0.1:9443"}));
    assert_eq!(st, "200 OK");
    let list = k2_core::front443::load_passthrough_checked().unwrap();
    assert_eq!(list.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(), vec!["*", "shop.example.com"]);
    let (st, _) = call(&Who::Owner, true, "/cli/front/passthrough/rm", serde_json::json!({"host": "shop.example.com"}));
    assert_eq!(st, "200 OK");
    let (st, _) = call(&Who::Owner, true, "/cli/front/passthrough/rm", serde_json::json!({"host": "shop.example.com"}));
    assert_eq!(st, "404 Not Found");
}

/// Q5(a): who holds :443, decided from what K2 knows.
#[test]
fn a_foreign_443_holder_is_named_never_fought() {
    use crate::front443::{classify_443, proc_net_listens, Holder443};
    let table = "  sl  local_address rem_address   st tx_queue\n   0: 00000000:01BB 00000000:0000 0A 00000000:00000000\n   1: 0100007F:9696 00000000:0000 0A 0\n";
    assert!(proc_net_listens(table, 443));
    assert!(!proc_net_listens(table, 80));
    let established = "  sl  local_address rem_address   st\n   0: 00000000:01BB 0A0B0C0D:D431 01 0\n";
    assert!(!proc_net_listens(established, 443), "only LISTEN sockets count");
    assert_eq!(classify_443(Some(true), true, false, false, vec![]), Holder443::Front);
    assert_eq!(classify_443(Some(false), false, false, false, vec![]), Holder443::Free);
    assert_eq!(classify_443(None, false, false, false, vec![]), Holder443::Unknown);
    assert_eq!(classify_443(Some(true), false, true, false, vec![]), Holder443::SkinDirect);
    assert_eq!(classify_443(Some(true), false, false, true, vec![]), Holder443::Mail);
    assert_eq!(classify_443(Some(true), false, false, true, vec!["caddy".into()]), Holder443::Foreign { units: vec!["caddy".into()] });
    assert_eq!(classify_443(Some(true), false, false, false, vec![]), Holder443::Foreign { units: vec![] });
    let m = crate::front443::foreign_443_message(&["caddy".into()]);
    assert!(m.contains("(caddy)") && m.contains("127.0.0.1:9443") && m.contains("k2 front passthrough add"), "{m}");
}

/// `k2 front status|enable|disable` (0.46.6 papercut): a shell tab is a
/// person at the box, as for the firewall; an agent is `403 owner_only`
/// with the owner's command (the CLI exits 3), never "invalid or missing
/// token"; a Member login and an App pass are refused.
#[test]
fn front_takes_the_firewalls_people() {
    let _e = env();
    let shell = Who::Shell { project_id: "ws-1".into(), session_id: "s-1".into() };
    for path in ["/cli/front/status", "/cli/front/enable", "/cli/front/disable"] {
        assert_eq!(routes::front_caller(&Who::Owner, path).unwrap_or_else(|r| panic!("{path}: owner refused: {}", r.body)), "owner");
        let actor = routes::front_caller(&shell, path).unwrap_or_else(|r| panic!("{path}: shell refused: {}", r.body));
        assert!(actor.starts_with("shell:"), "{path}: {actor}");
        let admin = Who::Login { username: "a".into(), role: k2_core::connect_users::Role::Admin };
        assert_eq!(routes::front_caller(&admin, path).unwrap_or_else(|r| panic!("{path}: admin refused: {}", r.body)), "user:a");

        let r = routes::front_caller(&agent(), path).expect_err("an agent is refused");
        assert_eq!(r.status, "403 Forbidden", "{path}");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only", "{path}: {v}");
        let verb = path.rsplit('/').next().unwrap();
        assert!(v["error"]["hint"].as_str().unwrap().contains(&format!("The owner runs: k2 front {verb}")), "{path}: {v}");

        let member = Who::Login { username: "m".into(), role: k2_core::connect_users::Role::Member };
        assert_eq!(routes::front_caller(&member, path).expect_err("member").status, "403 Forbidden");
        assert_eq!(routes::front_caller(&Who::Wrong, path).expect_err("app pass").status, "403 Forbidden");
        assert_eq!(routes::front_caller(&Who::Invalid, path).expect_err("no token").status, "403 Forbidden");
    }
}
