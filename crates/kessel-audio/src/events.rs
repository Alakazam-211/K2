//! Events, levels and device descriptions.

use crate::error::AudioError;
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A level reading for one stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// Level over the last window (about 50 ms), dBov: 0 is full scale,
    /// -127 is silence.
    pub level_dbov: f32,
    /// Speaking: the level is above the speaking threshold, held for a short
    /// release time so the indicator doesn't flicker between words.
    pub speaking: bool,
}

impl Default for Level {
    fn default() -> Self {
        Level {
            level_dbov: -127.0,
            speaking: false,
        }
    }
}

/// Levels for this device and each remote participant, about every 50 ms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    /// This device's processed capture (after echo cancellation and noise
    /// suppression). Silent while capture is disabled.
    pub local: Level,
    /// Each participant's decoded audio, by participant id.
    pub participants: Vec<(u64, Level)>,
    /// Increases with each new reading.
    pub seq: u64,
}

/// One audio device.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceInfo {
    /// Stable id, usable with [`DeviceChoice::Id`]. Survives restarts where
    /// the platform allows.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The system default for its direction.
    pub is_default: bool,
}

/// The available devices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// Capture devices.
    pub inputs: Vec<DeviceInfo>,
    /// Playback devices.
    pub outputs: Vec<DeviceInfo>,
}

/// Which device to use for one direction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum DeviceChoice {
    /// The system default. Followed when it changes, where the platform
    /// supports that.
    #[default]
    Default,
    /// A specific device by [`DeviceInfo::id`]. If it goes away, the engine
    /// reports [`crate::ErrorCode::DeviceLost`] and falls back to the default.
    Id(String),
}

/// A device change: `None` leaves that direction as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSelection {
    /// New capture device.
    pub input: Option<DeviceChoice>,
    /// New playback device.
    pub output: Option<DeviceChoice>,
}

/// The devices in use right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevicesInUse {
    /// The capture device, if one is open.
    pub input: Option<DeviceInfo>,
    /// The playback device, if one is open.
    pub output: Option<DeviceInfo>,
}

/// Something the caller should know about.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The device list changed (hot-plug, or a default changed).
    Devices(DeviceList),
    /// The devices in use changed (a switch, or a fallback after a loss).
    DevicesInUse(DevicesInUse),
    /// A device error. The engine keeps running; see
    /// [`crate::ErrorCode`] for what it does about each.
    Error(AudioError),
    /// New levels (about every 50 ms).
    Levels(Levels),
}

/// The receiving end of the event queue (bounded, lock-free). When it is
/// full, the oldest event is dropped, so a slow reader sees the latest state.
#[derive(Clone)]
pub struct EventRx {
    pub(crate) q: Arc<ArrayQueue<Event>>,
}

impl EventRx {
    /// Take the oldest event, if any. Never blocks.
    pub fn try_recv(&self) -> Option<Event> {
        self.q.pop()
    }

    /// Wait up to `timeout` for an event (polls once a millisecond).
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(e) = self.q.pop() {
                return Some(e);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl std::fmt::Debug for EventRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRx").field("len", &self.q.len()).finish()
    }
}
