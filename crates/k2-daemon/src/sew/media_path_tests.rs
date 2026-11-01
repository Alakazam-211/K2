//! The media-path machine (§6.6 M11) with an injected clock.

use super::*;

fn secs(t0: Instant, s: u64) -> Instant {
    t0 + Duration::from_secs(s)
}

#[test]
fn under_20_s_of_lane_down_sew_hears_nothing_and_joins_are_held() {
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    m.sew_started(); // Sew holds `up`
    m.observe(true, true, t0);
    assert_eq!(m.state(), PathState::Up);
    assert_eq!(gate_of(m.state()), JoinGate::Open);
    m.observe(true, false, secs(t0, 1));
    assert_eq!(m.state(), PathState::Reconnecting);
    assert_eq!(gate_of(m.state()), JoinGate::Hold);
    for s in [1, 5, 10, 20] {
        m.observe(true, false, secs(t0, s));
        assert_eq!(m.state(), PathState::Reconnecting, "{s}");
        assert_eq!(m.next_action(secs(t0, s)), None, "nothing to Sew at {s} s");
    }
    // 20 s of continuous lane-down (from 1 s): down.
    m.observe(true, false, secs(t0, 21));
    assert_eq!(m.state(), PathState::Down);
    assert_eq!(gate_of(m.state()), JoinGate::Refuse);
    assert_eq!(m.next_action(secs(t0, 21)), Some(Action::Send(SewSide::Down)));
    m.sent(SewSide::Down, true, secs(t0, 21));
    assert_eq!(m.next_action(secs(t0, 22)), None);
    // Up at once on the lane's first success: no hysteresis.
    m.observe(true, true, secs(t0, 23));
    assert_eq!(m.state(), PathState::Up);
    assert_eq!(m.next_action(secs(t0, 23)), Some(Action::Send(SewSide::Up)));
}

#[test]
fn a_flapping_lane_never_reaches_down() {
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    m.sew_started();
    for cycle in 0..5u64 {
        let base = cycle * 20;
        m.observe(true, false, secs(t0, base));
        m.observe(true, false, secs(t0, base + 19));
        assert_eq!(m.state(), PathState::Reconnecting);
        m.observe(true, true, secs(t0, base + 19));
        assert_eq!(m.state(), PathState::Up);
        assert_eq!(m.next_action(secs(t0, base + 19)), None, "Sew never heard down");
    }
}

/// Re-assert: while down K2 compares with Sew every 10 s; a Sew that
/// restarted (back to `up`) is told `down` again. A Sew start K2 does
/// re-sends at once.
#[test]
fn down_is_reasserted_after_a_sew_restart() {
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    m.sew_started();
    m.observe(true, false, t0);
    m.observe(true, false, secs(t0, 20));
    assert_eq!(m.next_action(secs(t0, 20)), Some(Action::Send(SewSide::Down)));
    m.sent(SewSide::Down, true, secs(t0, 20));
    assert_eq!(m.next_action(secs(t0, 29)), None);
    assert_eq!(m.next_action(secs(t0, 30)), Some(Action::ReadSew), "every 10 s while down");
    m.read(Some(SewSide::Up), secs(t0, 30));
    assert_eq!(m.next_action(secs(t0, 30)), Some(Action::Send(SewSide::Down)));
    m.sent(SewSide::Down, true, secs(t0, 30));
    m.sew_started();
    assert_eq!(m.next_action(secs(t0, 31)), Some(Action::Send(SewSide::Down)), "a Sew start forgets down");
}

/// Boot: K2 doesn't know what Sew holds; it reads once and corrects a
/// leftover `down` with `up` (and a down lane gets `down` after 20 s).
#[test]
fn boot_corrects_sew_both_ways() {
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    m.observe(true, true, t0);
    assert_eq!(m.next_action(t0), Some(Action::ReadSew));
    m.read(Some(SewSide::Down), t0);
    assert_eq!(m.next_action(t0), Some(Action::Send(SewSide::Up)));
    m.sent(SewSide::Up, true, t0);
    assert_eq!(m.next_action(secs(t0, 60)), None, "agreed and up: quiet");

    let mut m = Machine::new(t0);
    m.observe(true, false, t0);
    m.observe(true, false, secs(t0, 20));
    assert_eq!(m.next_action(secs(t0, 20)), Some(Action::Send(SewSide::Down)), "unknown and down: send");
}

/// Tunnel tier only: a move to direct (or the preview off) sends `up`.
#[test]
fn leaving_the_tunnel_tier_sends_up() {
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    m.sew_started();
    m.observe(true, false, t0);
    m.observe(true, false, secs(t0, 20));
    m.sent(SewSide::Down, true, secs(t0, 20));
    m.observe(false, false, secs(t0, 25));
    assert_eq!(m.state(), PathState::Off);
    assert_eq!(gate_of(m.state()), JoinGate::Open);
    assert_eq!(m.next_action(secs(t0, 25)), Some(Action::Send(SewSide::Up)));
}

/// Unreachable or 5xx: retried with backoff (1 s doubling, at most 30 s).
#[test]
fn failed_sends_back_off() {
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    m.sew_started();
    m.observe(true, false, t0);
    m.observe(true, false, secs(t0, 20));
    let mut now = secs(t0, 20);
    let mut waits = vec![];
    for _ in 0..7 {
        assert_eq!(m.next_action(now), Some(Action::Send(SewSide::Down)));
        m.sent(SewSide::Down, false, now);
        let mut w = 0;
        while m.next_action(now + Duration::from_secs(w)).is_none() {
            w += 1;
        }
        waits.push(w);
        now += Duration::from_secs(w);
    }
    assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30]);
}

#[test]
fn state_names_match_the_contract() {
    assert_eq!(PathState::Reconnecting.as_str(), "voice_lane_reconnecting");
    assert_eq!(PathState::Down.as_str(), "voice_lane_down");
    assert_eq!(REASON_LANE_DOWN, "voice_lane_down");
    assert_eq!(ROUTE, "/mgmt/v1/voice/media-path");
    assert_eq!(FEATURE_VOICE_MEDIA_PATH, "voice_media_path");
    assert_eq!(GRACE, Duration::from_secs(20));
    assert_eq!(JoinRefused::CODE, "media_path_down");
}
