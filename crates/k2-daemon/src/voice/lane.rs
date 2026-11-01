//! The voice lane's state as the voice BFF reads it (prd-k2-voice-rooms-v1
//! v1.9 §6.6 M11; vs-live V8, V16).
//!
//! **Who writes it.** Lane A's `frpc-voice` supervisor and media-path state
//! machine (`sew/media_path.rs`) calls [`publish`] on every transition:
//! - [`LaneState::not_applicable`]: no voice lane to wait on (direct mode,
//!   the tunnel preview off, or lane A not running). The default, so until
//!   lane A lands every join goes straight through.
//! - [`LaneState::up`]: `frpc-voice` reported `start proxy success` for the
//!   turn proxy.
//! - [`LaneState::reconnecting`]: the lane dropped less than 20 s ago
//!   (K2 state `voice_lane_reconnecting`). K2 sends Sew nothing.
//! - [`LaneState::down`]: 20 s of continuous lane-down (`voice_lane_down`);
//!   lane A has told Sew (SW-11 `media-path down`).
//!
//! **Who reads it.** `POST /cli/voice/join` ([`gate_join`]): under
//! `reconnecting` a join is held up to [`HOLD_MAX`]; it goes on as soon as
//! the lane is back (`up` / `not_applicable`), and is refused with the
//! reason when the lane goes `down` or the hold runs out. Under `down` a
//! join is refused at once. `GET /cli/voice/rooms` and `k2 voice status`
//! show the state.
//!
//! In-memory only: lane A re-publishes on daemon boot (M11 "corrected on
//! daemon boot").

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// The longest a join is held while the lane reconnects (M11: "a held join
/// waits at most 20 s").
pub const HOLD_MAX: Duration = Duration::from_secs(20);

/// The reason text K2 uses for a lane that is down (M11, SW-11 `reason`).
pub const REASON_VOICE_LANE_DOWN: &str = "voice_lane_down";

/// Where the lane is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneKind {
    /// No voice lane applies (direct mode, tunnel preview off, no lane A).
    NotApplicable,
    /// `frpc-voice` is connected and its turn proxy is up.
    Up,
    /// `voice_lane_reconnecting`: down for less than 20 s.
    Reconnecting,
    /// `voice_lane_down`: down for 20 s or more; Sew was told.
    Down,
}

/// The lane's state with when it entered it and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneState {
    pub state: LaneKind,
    /// Unix milliseconds the lane entered this state (`None` for
    /// `not_applicable`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    /// Why (lane A's text: `frpc-voice exited`, `login failed`,
    /// [`REASON_VOICE_LANE_DOWN`] …). Shown to people as is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl LaneState {
    pub fn not_applicable() -> LaneState {
        LaneState { state: LaneKind::NotApplicable, since_ms: None, reason: None }
    }
    pub fn up() -> LaneState {
        LaneState { state: LaneKind::Up, since_ms: Some(now_ms()), reason: None }
    }
    pub fn reconnecting(reason: &str) -> LaneState {
        LaneState { state: LaneKind::Reconnecting, since_ms: Some(now_ms()), reason: Some(reason.to_string()) }
    }
    pub fn down(reason: &str) -> LaneState {
        LaneState { state: LaneKind::Down, since_ms: Some(now_ms()), reason: Some(reason.to_string()) }
    }

    /// Joins go through now.
    pub fn admits_joins(&self) -> bool {
        matches!(self.state, LaneKind::NotApplicable | LaneKind::Up)
    }

    /// The text a refused join carries.
    pub fn reason_text(&self) -> String {
        self.reason.clone().unwrap_or_else(|| REASON_VOICE_LANE_DOWN.to_string())
    }
}

struct Slot {
    state: Mutex<LaneState>,
    changed: Condvar,
}

fn slot() -> &'static Slot {
    static S: std::sync::OnceLock<Slot> = std::sync::OnceLock::new();
    S.get_or_init(|| Slot { state: Mutex::new(LaneState::not_applicable()), changed: Condvar::new() })
}

/// Lane A: record a transition. Wakes every held join.
pub fn publish(s: LaneState) {
    let sl = slot();
    let mut g = sl.state.lock().unwrap_or_else(|p| p.into_inner());
    let moved = g.state != s.state;
    *g = s;
    drop(g);
    sl.changed.notify_all();
    if moved {
        crate::voice::occupancy::lane_moved();
    }
}

/// The lane's state now.
pub fn state() -> LaneState {
    slot().state.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// What a join may do (M11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Go on. `held_ms` = how long it waited for a reconnecting lane.
    Go { held_ms: u64 },
    /// Refuse with `voice_unavailable` / `media_path_down`: the lane is down,
    /// or it stayed down through the whole hold.
    Refuse { state: LaneState, held_ms: u64 },
}

/// Hold a join while the lane reconnects, at most `max` (production:
/// [`HOLD_MAX`]). Returns at once when the lane admits joins or is down.
pub fn gate_join(max: Duration) -> Gate {
    let sl = slot();
    let start = Instant::now();
    let mut g = sl.state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        let held_ms = start.elapsed().as_millis() as u64;
        match g.state {
            LaneKind::NotApplicable | LaneKind::Up => return Gate::Go { held_ms },
            LaneKind::Down => return Gate::Refuse { state: (*g).clone(), held_ms },
            LaneKind::Reconnecting => {
                let left = max.saturating_sub(start.elapsed());
                if left.is_zero() {
                    return Gate::Refuse { state: (*g).clone(), held_ms };
                }
                g = sl.changed.wait_timeout(g, left).unwrap_or_else(|p| p.into_inner()).0;
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|p| p.into_inner())
}
