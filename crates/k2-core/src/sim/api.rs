//! Route shapes for `/cli/sim/*` (the contract the `k2 sim` CLI and the
//! Mobile tab code against). JSON is camelCase. Every mutating route is
//! POST-only (405 on GET).
//!
//! | Route | Method | Who | Body / query → answer |
//! |---|---|---|---|
//! | `/cli/sim/status` | GET | anyone who can open the workspace; agents need the switch only for verbs | `?workspace=&refresh=1` → [`StatusResp`] |
//! | `/cli/sim/devices` | GET | same | `?platform=` → [`DevicesResp`] |
//! | `/cli/sim/boot` | POST | owner, Admin, Member (tab), agent with `sim` | [`BootReq`] → [`SimView`] |
//! | `/cli/sim/stop` | POST | same | [`WorkspaceReq`] → [`SimView`] |
//! | `/cli/sim/erase` | POST | same | [`WorkspaceReq`] → `{ok}` |
//! | `/cli/sim/install` | POST | same | [`InstallReq`] → [`InstallResp`] |
//! | `/cli/sim/launch` | POST | same | [`AppReq`] → [`ActResp`] |
//! | `/cli/sim/terminate` | POST | same | [`AppReq`] → [`ActResp`] |
//! | `/cli/sim/tree` | GET | same | `?workspace=&all=1&find=&boxes=1` → [`TreeResp`] |
//! | `/cli/sim/act` | POST | same | [`ActReq`] → [`ActResp`] (exit 2 when held) |
//! | `/cli/sim/screenshot` | POST | same | [`ScreenshotReq`] → [`ScreenshotResp`] |
//! | `/cli/sim/logs` | GET | same | `?workspace=&app=&sinceSecs=&grep=&maxLines=` → [`LogsResp`] |
//! | `/cli/sim/wait` | POST | same | [`WaitReq`] → [`WaitResp`] |
//! | `/cli/sim/hold` | POST | people only (not agents) | [`HoldReq`] → [`SimView`] |
//! | `/cli/sim/setup` | POST | owner / Admin | [`SetupReq`] → [`StatusResp`] (licence accept + first-use install) |
//! | `/cli/sim/settings` | GET / POST | owner / Admin to write | [`crate::sim::limits::SimSettings`] → [`SettingsResp`] |
//! | `/cli/sim/stream` | GET (WS) | people only | see [`crate::sim::stream`] |
//!
//! **Errors** use the daemon's usual `{error, code, message}` body. Codes:
//! `sim_switch_off` (403, exit 3, `Ask with:` line), `sim_held` (409, exit
//! 2), `sim_limit` (409), `sim_needs_setup` (409, exit 3; names the
//! owner's card), `sim_no_device` (409), `sim_stale_element` (409),
//! plus [`crate::sim::lane::LaneError::code`].
//!
//! **Events** on the session-events bus: `sim_state` (`{workspace, sim,
//! state}`), `open_mobile` (`{workspace, sim}`, after a boot without
//! `noTab`), `sim_hold` (`{workspace, sim, hold}`).

use serde::{Deserialize, Serialize};

use super::hold::HoldView;
use super::lane::{Appearance, Button, DeviceProfile, Direction, InstallOutcome, Key, LaneKind, LaneProbe, Orientation};
use super::limits::{Limits, SimSettings};
use super::stream::SimState;
use super::tree::Snapshot;

/// Most routes act on one workspace's device. `workspace` is a handle or
/// a `projects.id`; an agent may omit it (its own workspace) and may not
/// name another (403).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// `android` | `ios`; default: the first usable lane (android on
    /// Linux, ios on a Mac with Xcode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// A [`DeviceProfile::id`] or its name (`"Pixel 9"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Don't ask connected clients to open the Mobile tab.
    #[serde(default)]
    pub no_tab: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Path to the `.apk` / `.app`, absolute or relative to the workspace;
    /// must resolve inside the workspace.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Package name / bundle id.
    pub app: String,
}

/// One device's state, as every client sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimView {
    /// `s<n>`.
    pub sim: String,
    pub workspace: String,
    pub workspace_handle: String,
    pub lane: LaneKind,
    pub state: SimState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<DeviceProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booted_at: Option<i64>,
    pub last_used_at: i64,
    pub hold: HoldView,
    pub viewers: u32,
    /// Why it failed / stopped (`idle`, `limit`, an error message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResp {
    /// Every lane this daemon knows, usable or not.
    pub lanes: Vec<LaneProbe>,
    /// This workspace's device, if it has one (running or stopped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim: Option<SimView>,
    /// Devices running on the box now.
    pub running: u32,
    pub limits: Limits,
    /// The agent switch "Allow mobile devices" for this workspace.
    pub agents_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesResp {
    pub lane: LaneKind,
    pub devices: Vec<DeviceProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResp {
    #[serde(flatten)]
    pub outcome: InstallOutcome,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeResp {
    pub snapshot: Snapshot,
    /// The rendered text (`k2 sim tree` prints this as-is).
    pub text: String,
}

/// An agent / CLI action. Element ids come from the latest snapshot
/// (`e5` or `t7:e5`); a stale id fails with `sim_stale_element`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum ActSpec {
    Tap {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        element: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f32>,
        #[serde(default, rename = "holdMs", skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u32>,
    },
    Type {
        /// Tap this element first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
        /// Plain text, or…
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// …a vault secret's name; the value never leaves the daemon.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
    Swipe {
        direction: Direction,
        /// Swipe inside this element (default: the screen's middle).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
    },
    Key { key: Key },
    Button { button: Button },
    /// `left` / `right` turn from the current orientation; or an absolute
    /// orientation.
    Rotate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<Direction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orientation: Option<Orientation>,
    },
    OpenUrl { url: String },
    Location { lat: f64, lon: f64 },
    Appearance { appearance: Appearance },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub action: ActSpec,
    /// Skip the after-action tree read (faster; no diff).
    #[serde(default)]
    pub no_diff: bool,
}

/// `ok tap e5 · 140ms · +e9 text "Saved"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActResp {
    pub ok: bool,
    pub line: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff: Vec<String>,
    /// The snapshot id the diff was read into (ids in `diff` belong to it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    pub took_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Crop to this element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotResp {
    /// Absolute path under the workspace's `.k2/sim/shots/`.
    pub path: String,
    /// Points resolution (the PNG is scaled to points).
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsResp {
    /// Redacted, oldest first.
    pub lines: Vec<String>,
    /// More lines matched than the caps allow.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Wait until an element's label / value contains this. Without it:
    /// wait until the device is free (no hold) and booted.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "for")]
    pub for_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitResp {
    pub ok: bool,
    pub line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    pub took_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HoldAction {
    TakeOver,
    HandBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub action: HoldAction,
}

/// Owner / Admin: accept a lane's licence on the card and start its
/// first-use install. `acceptLicence` must name the licence id the card
/// showed (and its text hash), so a changed licence needs a new card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupReq {
    pub lane: LaneKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_sha1: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResp {
    pub settings: SimSettings,
    pub effective: Limits,
}
