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
