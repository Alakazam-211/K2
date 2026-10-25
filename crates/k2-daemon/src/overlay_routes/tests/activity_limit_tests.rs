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
    let mut guest = platform_pass("room-1");
    assert!(key_for(&HashMap::new(), Some(&guest)).is_some_and(|k| k.starts_with("pass:n1-")));
    guest.session = true;
    guest.principal_id = Some("p-42".into());
    assert_eq!(key_for(&HashMap::new(), Some(&guest)).as_deref(), Some("guest:p-42"), "one person across devices and Apps");
    let passport = params_of(&[(crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"), ("project_id", "ws-9"), ("from", "ws-9")]);
    let agent = crate::caller_workspace::principal_from_params(&passport).map(|p| p.workspace_uuid);
    assert_eq!(key_for(&passport, None), agent.map(|w| format!("agent:{w}")));
    assert!(key_for(&passport, None).is_some(), "an agent passport is limited");
    assert_eq!(key_for(&params_of(&[("cell_session_id", "c-1")]), None).as_deref(), Some("cell:c-1"));
    let login = crate::read_routes::with_read_login(Some("mel".into()), || key_for(&HashMap::new(), None));
    assert_eq!(login.as_deref(), Some("user:mel"));
    let owner = crate::read_routes::with_read_login(Some("owner".into()), || key_for(&HashMap::new(), None));
    assert_eq!(owner, None, "the owner token is exempt");
    assert_eq!(key_for(&HashMap::new(), None), None);
}
