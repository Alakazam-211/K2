//! The Android emulator lane (`prd-mobile-dev-v1` §3.2, L-A).
//!
//! Stub for the S1 contract push: probes only. The real lane (SDK on
//! first use, AVD per workspace, headless boot, gRPC frames and input,
//! uiautomator tree, adb install/launch/logcat) lands in L-A.

pub mod grpc;

use std::path::{Path, PathBuf};

use super::lane::*;

pub struct AndroidLane {
    root: PathBuf,
}

impl AndroidLane {
    /// `root` = `~/.k2/sim/android` (SDK under `root/sdk`, devices under
    /// `root/devices`).
    pub fn new(root: PathBuf) -> AndroidLane {
        AndroidLane { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn not_yet() -> LaneError {
        LaneError::unavailable("the Android lane is not built yet (0.46.6 L-A)")
    }
}

impl SimLane for AndroidLane {
    fn kind(&self) -> LaneKind {
        LaneKind::Android
    }

    fn probe(&self) -> LaneProbe {
        LaneProbe { lane: LaneKind::Android, usable: false, ready: false, needs: vec![], detail: Some("not built yet".into()), versions: vec![] }
    }

    fn install(&self, _licence_accepted: &[String], _progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
        Err(Self::not_yet())
    }

    fn profiles(&self) -> Result<Vec<DeviceProfile>, LaneError> {
        Err(Self::not_yet())
    }

    fn boot(&self, _req: &BootRequest) -> Result<BootedDevice, LaneError> {
        Err(Self::not_yet())
    }

    fn adopt(&self, _dev: &DeviceRef) -> Result<Option<BootedDevice>, LaneError> {
        Ok(None)
    }

    fn stop(&self, _dev: &DeviceRef) -> Result<(), LaneError> {
        Err(Self::not_yet())
    }

    fn erase(&self, _workspace_key: &str, _data_dir: &Path) -> Result<(), LaneError> {
        Err(Self::not_yet())
    }

    fn install_app(&self, _dev: &DeviceRef, _artifact: &Path) -> Result<InstallOutcome, LaneError> {
        Err(Self::not_yet())
    }

    fn launch(&self, _dev: &DeviceRef, _app_id: &str) -> Result<(), LaneError> {
        Err(Self::not_yet())
    }

    fn terminate(&self, _dev: &DeviceRef, _app_id: &str) -> Result<(), LaneError> {
        Err(Self::not_yet())
    }

    fn tree(&self, _dev: &DeviceRef) -> Result<RawTree, LaneError> {
        Err(Self::not_yet())
    }

    fn act(&self, _dev: &DeviceRef, _action: &Action) -> Result<(), LaneError> {
        Err(Self::not_yet())
    }

    fn screenshot(&self, _dev: &DeviceRef) -> Result<Frame, LaneError> {
        Err(Self::not_yet())
    }

    fn frames(&self, _dev: &DeviceRef, _opts: FrameOpts) -> Result<Box<dyn FrameSource>, LaneError> {
        Err(Self::not_yet())
    }

    fn logs(&self, _dev: &DeviceRef, _q: &LogQuery) -> Result<Vec<String>, LaneError> {
        Err(Self::not_yet())
    }
}
