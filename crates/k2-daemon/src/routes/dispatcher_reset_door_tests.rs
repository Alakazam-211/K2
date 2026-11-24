//! `reset_door_admit`'s tests: a child of `routes::dispatcher` (moved out
//! unchanged for the line ratchet).

use super::*;

#[test]
fn the_reset_door_has_its_own_budget() {
    let l = crate::login_throttle::LoginLimiter::new();
    let peer: Option<std::net::IpAddr> = Some("192.0.2.7".parse().expect("ip"));
    for i in 0..crate::login_throttle::RESET_LIMIT {
        assert!(reset_door_admit(&l, Ingress::Lan, peer, "", &[], None, None).is_some(), "{i}");
    }
    assert!(reset_door_admit(&l, Ingress::Lan, peer, "", &[], None, None).is_none(), "over the per-client reset budget");
    // Logins from the same address are another door.
    let skin_key = crate::login_throttle::skin_ip_key("192.0.2.7");
    assert_eq!(l.check_and_record(&skin_key, l.now()), crate::login_throttle::Verdict::Allow);
    // An App's link: one bucket per App.
    for _ in 0..crate::login_throttle::RESET_LINK_LIMIT {
        assert!(reset_door_admit(&l, Ingress::App, None, "", &[], None, Some("svc-a")).is_some());
    }
    assert!(reset_door_admit(&l, Ingress::App, None, "", &[], None, Some("svc-a")).is_none());
    assert!(reset_door_admit(&l, Ingress::App, None, "", &[], None, Some("svc-b")).is_some(), "another App's link");
    // The anonymous ceiling is the reset door's own.
    for _ in 0..crate::login_throttle::GLOBAL_FAIL_LIMIT {
        l.record_global_failure(crate::login_throttle::Door::Reset, l.now());
    }
    assert!(reset_door_admit(&l, Ingress::Tunnel, None, "", &[], None, None).is_none(), "anonymous ceiling");
    assert!(!l.global_ceiling_reached(crate::login_throttle::Door::Skin, l.now()), "logins keep their own ceiling");
    assert!(reset_door_admit(&l, Ingress::Loopback, Some("127.0.0.1".parse().expect("ip")), "", &[], None, None).is_some(), "local is never anonymous");
}

/// 0.46.12: a reset through an App's link counts the visitor the App's
/// server named (`X-K2-Client-IP`, the address K2 gave it) on its own
/// bucket first, like a link login (PB-G3). A visitor over its own
/// budget is refused without spending the App's, so one visitor's junk
/// resets can't use up every other guest's resets on that App. A
/// loopback or missing visitor has no bucket of its own (never one shared
/// key for everyone); the App's bucket still holds.
#[test]
fn a_link_reset_counts_the_named_visitor_before_the_app() {
    use crate::login_throttle::{RESET_LIMIT, RESET_LINK_LIMIT};
    let l = crate::login_throttle::LoginLimiter::new();
    let a = "X-K2-Client-IP: 203.0.113.7\r\n";
    for i in 0..RESET_LIMIT {
        assert!(reset_door_admit(&l, Ingress::App, None, a, &[], None, Some("svc-v")).is_some(), "visitor a, {i}");
    }
    for i in 0..5 {
        assert!(reset_door_admit(&l, Ingress::App, None, a, &[], None, Some("svc-v")).is_none(), "visitor a is over its own reset budget ({i})");
    }
    // The refused tries spent nothing of the App's bucket: two more
    // visitors still get their whole budgets, until the App's is used up.
    let others = ["X-K2-Client-IP: 198.51.100.9\r\n", "X-K2-Client-IP: 2001:db8:9:9::1\r\n"];
    let mut admitted = RESET_LIMIT;
    for h in others {
        for _ in 0..RESET_LIMIT {
            if admitted < RESET_LINK_LIMIT {
                assert!(reset_door_admit(&l, Ingress::App, None, h, &[], None, Some("svc-v")).is_some(), "{h} after {admitted}");
                admitted += 1;
            }
        }
    }
    assert_eq!(admitted, RESET_LINK_LIMIT, "the App's bucket is exactly used up");
    assert!(
        reset_door_admit(&l, Ingress::App, None, "X-K2-Client-IP: 192.0.2.44\r\n", &[], None, Some("svc-v")).is_none(),
        "the App's bucket still caps the link"
    );
    // Another App is untouched; a loopback "visitor" is no visitor.
    for i in 0..RESET_LIMIT + 1 {
        assert!(
            reset_door_admit(&l, Ingress::App, None, "X-K2-Client-IP: 127.0.0.1\r\n", &[], None, Some("svc-w")).is_some(),
            "loopback names no visitor: only svc-w's bucket counts ({i})"
        );
    }
    // Logins are another table.
    let skin_key = crate::login_throttle::skin_ip_key("203.0.113.7");
    assert_eq!(l.check_and_record(&skin_key, l.now()), crate::login_throttle::Verdict::Allow);
}
