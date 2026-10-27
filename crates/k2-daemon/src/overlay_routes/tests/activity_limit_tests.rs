//! N1 (prd-app-caps-0464-v1 §6.3, AC29–AC31): `GET /cli/thread/activity`
//! is limited per caller (2/s, burst 10) before any tracker work, and the
//! owner is never limited.

use super::*;

fn platform_pass(project_id: &str) -> SkinPass {
    SkinPass {
        id: format!("n1-{}", uuid::Uuid::new_v4()),
        principal_id: None,
        username: "deck".into(),
        caps: vec![crate::skin_routes::THREAD_READ.into(), crate::skin_routes::ACTIVITY_READ.into()],
        rooms: vec![project_id.to_string()],
        session: false,
        room_policy: Default::default(),
        app: String::new(),
        room_paths: Default::default(),
    }
}

fn catch_up(pass: &SkinPass, handle: &str) -> CliResponse {
    with_request_skin(Some(pass.clone()), || {
        dispatch("/cli/thread/activity", &params_of(&[("addr", handle)])).expect("an overlay route")
    })
}

/// The 11th call in one burst is 429 with both retry fields, a refused
/// call never takes the tracker lock, a second caller is unaffected, and
/// the owner token is never limited.
#[test]
fn the_eleventh_catch_up_is_refused_without_touching_the_tracker() {
    let handle = format!("ovln1{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    let pass = platform_pass(&project_id);
    let before = crate::thread_activity::run_calls_here();
    // The burst: at least 10 answer (a slow machine may refill a token or
    // two while they run), then a 429.
    let mut answered = 0;
    loop {
        let r = catch_up(&pass, &handle);
        if r.status == "429 Too Many Requests" {
            break;
        }
        assert_eq!(r.status, "200 OK", "call {answered}: {}", r.body);
        answered += 1;
        assert!(answered <= 20, "the limit never refused");
    }
    assert!(answered >= 10, "a burst of 10 is allowed (got {answered})");
    let allowed = crate::thread_activity::run_calls_here();
    assert!(allowed >= before + 10, "allowed calls reach the tracker ({before} → {allowed})");
    // Refused calls are microseconds apart: no token refills meanwhile.
    for i in 0..50 {
        let r = catch_up(&pass, &handle);
        assert_eq!(r.status, "429 Too Many Requests", "refused call {i}: {}", r.body);
        let v = json_body(&r);
        assert_eq!(v["error"], "rate_limited", "{v}");
        let ms = v["retryAfterMs"].as_u64().expect("retryAfterMs");
        assert!((1..=500).contains(&ms), "{v}");
        assert_eq!(v["retryAfterSecs"].as_u64(), Some(ms.div_ceil(1000)), "{v}");
    }
    assert_eq!(crate::thread_activity::run_calls_here(), allowed, "50 refused calls never took the tracker lock");
    // Another pass (another caller) is unaffected.
    let r = catch_up(&platform_pass(&project_id), &handle);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    // The owner token (no pass, no passport, no login): never limited.
    for i in 0..30 {
        let r = dispatch("/cli/thread/activity", &params_of(&[("addr", &handle)])).expect("route");
        assert_eq!(r.status, "200 OK", "owner call {i}: {}", r.body);
    }
    let r = crate::read_routes::with_read_login(Some("owner".into()), || {
        dispatch("/cli/thread/activity", &params_of(&[("addr", &handle)])).expect("route")
    });
    assert_eq!(r.status, "200 OK", "{}", r.body);
}

#[test]
fn callers_are_keyed_by_person_pass_login_and_agent() {
    use crate::thread_activity_limit::key_for;
    use crate::thread_activity_limit::hashed_key;
    let mut guest = platform_pass("room-1");
    let pass_key = key_for(&HashMap::new(), Some(&guest)).expect("a pass is limited");
    assert_eq!(pass_key, hashed_key("pass", &guest.id));
    guest.session = true;
    guest.principal_id = Some("p-42".into());
    assert_eq!(key_for(&HashMap::new(), Some(&guest)), Some(hashed_key("guest", "p-42")), "one person across devices and Apps");
    // An agent: the request's principal slot (TCP passport and cell alike).
    let slot = crate::session_token::HookPrincipal { workspace_uuid: "ws-9".into(), agent_address: "ws-9".into() };
    let agent = crate::caller_workspace::with_request_principal(Some(slot), || key_for(&HashMap::new(), None));
    assert_eq!(agent, Some(hashed_key("agent", "ws-9")));
    let login = crate::read_routes::with_read_login(Some("mel".into()), || key_for(&HashMap::new(), None));
    assert_eq!(login, Some(hashed_key("user", "mel")));
    let owner = crate::read_routes::with_read_login(Some("owner".into()), || key_for(&HashMap::new(), None));
    assert_eq!(owner, None, "the owner token is exempt");
    assert_eq!(key_for(&HashMap::new(), None), None);
    // Review B2: a login's raw query params never move it to another key:
    // not an agent's (draining it), not a fresh one per cell id.
    let spoof = params_of(&[
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"),
        ("project_id", "ws-9"),
        ("from", "ws-9"),
        ("cell_session_id", "fresh-1"),
    ]);
    let spoofed = crate::read_routes::with_read_login(Some("mel".into()), || key_for(&spoof, None));
    assert_eq!(spoofed, Some(hashed_key("user", "mel")), "the login's own key, whatever it sends");
    let again = crate::read_routes::with_read_login(Some("mel".into()), || {
        key_for(&params_of(&[("cell_session_id", "fresh-2")]), None)
    });
    assert_eq!(again, Some(hashed_key("user", "mel")));
    // Fixed length, however long the name.
    let long = "x".repeat(10_000);
    let k = crate::read_routes::with_read_login(Some(long), || key_for(&HashMap::new(), None)).expect("key");
    assert_eq!(k.len(), "user:".len() + 32, "{k}");
}

// ── 0.46.5 activity SDK (review D2/F8): the batched catch-up ───────────

fn rooms_catch_up(pass: &SkinPass, rooms: &str) -> CliResponse {
    with_request_skin(Some(pass.clone()), || {
        dispatch(THREAD_ACTIVITY_ROOMS_ROUTE, &params_of(&[("rooms", rooms)])).expect("an overlay route")
    })
}

/// Fifteen rooms catch up in ONE read and one rate-limit token (one GET
/// per room hit the 10-a-person burst on every reconnect): the answer
/// names every room the pass hears, with null for no turn; a room the
/// pass doesn't hold is `unknown`, never a turn.
#[test]
fn fifteen_rooms_catch_up_in_one_read_and_one_token() {
    let mut rooms = Vec::new();
    for _ in 0..15 {
        let handle = format!("ovlrm{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        rooms.push(project_id);
    }
    let mut pass = platform_pass(&rooms[0]);
    pass.rooms = rooms.clone();
    let r = rooms_catch_up(&pass, "all");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_body(&r);
    let turns = v["turns"].as_object().expect("turns");
    let mut got: Vec<&String> = turns.keys().collect();
    got.sort();
    let mut want: Vec<&String> = rooms.iter().collect();
    want.sort();
    assert_eq!(got, want, "every room the pass hears: {v}");
    assert!(turns.values().all(serde_json::Value::is_null), "no turn runs: {v}");
    // A named list: an id off the pass is unknown, same as one that doesn't exist.
    let r = rooms_catch_up(&pass, &format!("{},not-a-room", rooms[1]));
    let v = json_body(&r);
    assert_eq!(v["turns"].as_object().map(|m| m.len()), Some(1), "{v}");
    assert_eq!(v["unknown"], serde_json::json!(["not-a-room"]), "{v}");
    // One token per read, whatever the set: more reads still answer (10
    // in all), then the burst is spent.
    let mut answered = 2;
    loop {
        let r = rooms_catch_up(&pass, "all");
        if r.status == "429 Too Many Requests" {
            break;
        }
        assert_eq!(r.status, "200 OK", "read {answered}: {}", r.body);
        answered += 1;
        assert!(answered <= 20, "the limit never refused");
    }
    assert!(answered >= 10, "15 rooms cost one token a read (got {answered} reads)");
}

/// The ear's rules hold room by room: thread:read without activity:read
/// hears no turn (the room is left out); a bad `rooms` is usage; an
/// agent's passport is refused (one Thread at a time); the owner token
/// reads every pinned room and is never limited.
#[test]
fn the_batched_catch_up_keeps_the_ear_rules() {
    let handle = format!("ovlrn{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    let mut thread_only = platform_pass(&project_id);
    thread_only.caps = vec![crate::skin_routes::THREAD_READ.into()];
    let v = json_body(&rooms_catch_up(&thread_only, "all"));
    assert_eq!(v["turns"], serde_json::json!({}), "no activity:read, no turn: {v}");
    let r = rooms_catch_up(&platform_pass(&project_id), " ");
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let slot = crate::session_token::HookPrincipal { workspace_uuid: project_id.clone(), agent_address: project_id.clone() };
    let r = crate::caller_workspace::with_request_principal(Some(slot), || {
        dispatch(THREAD_ACTIVITY_ROOMS_ROUTE, &params_of(&[("rooms", "all")])).expect("route")
    });
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    for _ in 0..15 {
        let r = dispatch(THREAD_ACTIVITY_ROOMS_ROUTE, &params_of(&[("rooms", "all")])).expect("route");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert!(json_body(&r)["turns"].as_object().is_some_and(|m| m.contains_key(&project_id)), "{}", r.body);
    }
}
