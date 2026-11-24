//! Voice rooms, lane C: the voice BFF, joining, occupancy, moderation and
//! the `/cli/voice/*` routes behind `k2 voice` (prd-k2-voice-rooms-v1 v1.9
//! §3, §4.4–§4.6, §9; slice S3; vs-live V14–V16, V20).
//!
//! - [`routes`]: `/cli/voice/*`. Dark (404 `voice_rooms_off`) unless the
//!   `voice_rooms_preview` / `K2_VOICE_ROOMS` switch is on.
//! - [`bff`]: a person's Sew session held by this daemon; join, leave, the
//!   call's own Sew routes, moderation through the moderator's own session.
//! - [`dm_call`]: 1-to-1 calls in a Home chat DM (start, answer, decline,
//!   ring again, cancel), on the person's held Home chat session.
//! - [`occupancy`]: the SW-3 follower (who is in which room), headless,
//!   publishing `voice_rooms_changed {rev}` (hosting changes ride it too).
//! - [`sew_lane`]: the call door `/_k2/voice/sew/`: the native engine's
//!   `sewcc_` lane, and Sew's web kit ([`kit_door`] routes, [`kit_feed`]
//!   events) on the person's own login.
//! - [`signal_lane`]: the native media-signalling door `/rtc/v1` on an App's
//!   addresses (a non-K2 bearer, spliced to Sew's signal listener).
//! - [`lane`]: the voice lane's state (lane A publishes it; held joins read
//!   it, M11).
//!
//! The K2 Admin Space, its members, levels and rooms are lane B's
//! (`k2_core::tool_hook::admin_space`, `crate::voice_admin`); these routes
//! read its ledger and call its owner actions (count, rename, Close now).

pub mod bff;
pub mod dm_call;
pub mod events_ws;
pub mod kit_door;
pub mod kit_feed;
pub mod lane;
pub mod occupancy;
pub mod routes;
pub mod sew_lane;
pub mod signal_lane;
pub mod transcripts;

#[cfg(test)]
mod tests;

use serde::Serialize;

use k2_core::tool_hook::admin_space::{SpaceRow, SPACE_BLOCKED, SPACE_READY};
use k2_core::tool_hook::features::VoiceFeatures;
use k2_core::voice_rooms::Hosting as Tier;

use lane::{LaneKind, LaneState};
use k2_core::brand::PRODUCT_NAME;
use k2_core::{cli_name, product_name};

/// The clock the doors' rate limits (`kit_door`, `sew_lane`) read for one
/// credential's request. Real time; a test can hold one credential's
/// clock still ([`hold_rate_clock`]) so a slow run can't refill a bucket
/// the test emptied between the empty and the request.
pub(crate) fn rate_now(key: &str) -> std::time::Instant {
    #[cfg(test)]
    if let Some(at) = held_rate_clocks().lock().unwrap_or_else(|e| e.into_inner()).get(key) {
        return *at;
    }
    let _ = key;
    std::time::Instant::now()
}

#[cfg(test)]
fn held_rate_clocks() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static H: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = std::sync::OnceLock::new();
    H.get_or_init(Default::default)
}

/// Tests: from now on [`rate_now`] for `key` answers `at`.
#[cfg(test)]
pub(crate) fn hold_rate_clock(key: &str, at: std::time::Instant) {
    held_rate_clocks().lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string(), at);
}

/// The hosting state that hosts rooms.
pub const HOSTING_OK: &str = "ok";
/// `voice_off`'s detail while Sew lacks the 0.7.0 voice-rooms features.
pub const SEW_TOO_OLD: &str = "sew_too_old: Sew 0.7.0 is needed (server_governed_space, voice_channel_closing)";

/// Why this server can or can't host rooms now (§4.5 `hosting.state`),
/// with the line people see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosting {
    /// `ok`, `not_linux`, `preview_off`, `sew_off`, `voice_off` (with
    /// `detail`: Sew too old, or the K2 Admin Space blocked), `starting`,
    /// `not_pro`, `count_zero`, `voice_lane_reconnecting` or
    /// `voice_lane_down` (the client's `voice-wire.ts` set, plus
    /// `not_linux` / `preview_off`).
    pub state: String,
    /// The line people see (§4.5).
    pub line: String,
    /// The failed check or the reason, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Everything [`hosting_of`] decides from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingFacts {
    pub linux: bool,
    pub preview: bool,
    /// Chat / Voice Hosting's grant-hook target is enabled.
    pub chat_on: bool,
    /// Sew is installed with its own sign-ins (`standalone` identity), so
    /// it takes no K2 grants and can never host K2 voice rooms.
    pub sew_standalone: bool,
    pub flags: VoiceFeatures,
    pub space: Option<SpaceRow>,
    /// Lane E's Pro gate (`sew::pro_gate`).
    pub tier: Tier,
    /// The Pro gate's reason when not Pro (its plan, or "not known yet").
    pub not_pro_reason: Option<String>,
    pub count: u32,
    pub lane: LaneState,
}

fn h(state: &str, line: &str, detail: Option<String>) -> Hosting {
    Hosting { state: state.into(), line: line.into(), detail }
}

/// The hosting state from its facts (pure; §4.5's table, in order).
pub fn hosting_of(f: &HostingFacts) -> Hosting {
    if !f.linux {
        return h("not_linux", "Voice rooms are hosted on Linux servers. Sign in to one from the server list to join its rooms.", None);
    }
    if !f.preview {
        return h("preview_off", concat!("Voice rooms are a preview: the owner turns them on (", cli_name!(), " settings set voiceRoomsPreview true, or K2_VOICE_ROOMS=1)."), None);
    }
    if !f.chat_on {
        if f.sew_standalone {
            return h(
                "sew_off",
                concat!("This server's chat and voice use their own sign-ins (standalone), so it can't host ", product_name!(), " voice rooms."),
                Some(concat!("identity: standalone (", cli_name!(), " hostchat status)").into()),
            );
        }
        return h("sew_off", concat!("Voice rooms are off on this server: chat and voice aren't on (", cli_name!(), " hostchat status)."), None);
    }
    if !f.flags.server_governed_space {
        return h("voice_off", "Voice isn't set up: this server's Sew is too old for voice rooms.", Some(SEW_TOO_OLD.into()));
    }
    match &f.space {
        Some(s) if s.state == SPACE_BLOCKED => {
            let why = s.why.clone().unwrap_or_else(|| "blocked".into());
            return h("voice_off", concat!("Voice isn't set up: the ", product_name!(), " Admin Space can't be made."), Some(format!("{PRODUCT_NAME} Admin Space: {why}")));
        }
        Some(s) if s.state == SPACE_READY => {}
        _ => return h("starting", "Turning on voice rooms…", Some(concat!("the ", product_name!(), " Admin Space is being made").into())),
    }
    match f.tier {
        Tier::NotPro | Tier::Unknown => return h("not_pro", "Voice rooms need the Pro plan.", f.not_pro_reason.clone()),
        Tier::Ok => {}
    }
    if !f.flags.voice_channel_closing {
        return h("voice_off", "Voice isn't set up: this server's Sew is too old for voice rooms.", Some(SEW_TOO_OLD.into()));
    }
    if f.count == 0 {
        return h("count_zero", "This server hosts no rooms.", None);
    }
    match f.lane.state {
        LaneKind::Down => h("voice_lane_down", concat!("Voice is reconnecting to ", product_name!(), "'s network…"), f.lane.reason.clone()),
        LaneKind::Reconnecting => h("voice_lane_reconnecting", concat!("Voice is reconnecting to ", product_name!(), "'s network…"), f.lane.reason.clone()),
        LaneKind::Up | LaneKind::NotApplicable => h(HOSTING_OK, "Voice rooms are on.", None),
    }
}

/// The live facts.
pub fn hosting_facts() -> HostingFacts {
    let preview = k2_core::voice_preview::enabled();
    let skin = k2_core::paths::k2_home().join("skin.db").exists();
    let chat_on = preview
        && skin
        && k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND).ok().flatten().is_some_and(|t| t.enabled);
    let sew_standalone = !chat_on
        && crate::sew::store::load()
            .is_some_and(|row| crate::sew::supervisor::row_provider(&row) == k2_root_proto::sew::PROVIDER_STANDALONE);
    let (tier, not_pro_reason) = pro_tier();
    HostingFacts {
        linux: cfg!(target_os = "linux"),
        preview,
        chat_on,
        sew_standalone,
        flags: if chat_on { k2_core::tool_hook::sew_voice_features().unwrap_or_default() } else { VoiceFeatures::default() },
        space: if chat_on { k2_core::tool_hook::admin_space_view().ok().and_then(|v| v.space) } else { None },
        tier,
        not_pro_reason,
        count: k2_core::app_settings::load().voice_rooms_count,
        lane: lane::state(),
    }
}

/// Lane E's Pro gate (the cached plan tier; never the network), or a
/// test's stand-in (`voice_rooms::hosting_override`). Same rule as lane
/// B's `voice_admin` sweep.
pub fn pro_tier() -> (Tier, Option<String>) {
    if let Some(t) = k2_core::voice_rooms::hosting_override() {
        return (t, None);
    }
    let env = crate::sew::supervisor::live();
    match crate::sew::pro_gate::require_pro(&env) {
        Ok(()) => (Tier::Ok, None),
        Err(e) => (Tier::NotPro, Some(e.hint.clone())),
    }
}

/// Lane E's `hosting` JSON for `k2 voice status` (`{state: pro|not_pro,
/// tier, reading, fetchedAt, reason?}`).
pub fn pro_view() -> serde_json::Value {
    crate::sew::pro_gate::hosting(&crate::sew::supervisor::live())
}

/// This server's hosting state now.
pub fn hosting_state() -> Hosting {
    hosting_of(&hosting_facts())
}
