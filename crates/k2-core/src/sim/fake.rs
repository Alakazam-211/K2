//! A fake lane for tests (core and daemon): an in-memory "device" with a
//! small settings screen. Tapping the Save button adds a `"Saved"` text;
//! typing sets the focused field's value. Records every action.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::lane::*;
use super::tree::{RawNode, Rect, Role};

pub struct FakeLane {
    pub kind: LaneKind,
    pub ready: AtomicBool,
    pub running: Mutex<Vec<String>>,
    pub actions: Mutex<Vec<String>>,
    pub saved: AtomicBool,
    pub email: Mutex<String>,
    pub boots: AtomicU64,
    pub fail_boot: AtomicBool,
}

impl FakeLane {
    pub fn new(kind: LaneKind) -> FakeLane {
        FakeLane {
            kind,
            ready: AtomicBool::new(true),
            running: Mutex::new(Vec::new()),
            actions: Mutex::new(Vec::new()),
            saved: AtomicBool::new(false),
            email: Mutex::new(String::new()),
            boots: AtomicU64::new(0),
            fail_boot: AtomicBool::new(false),
        }
    }

    pub fn profile() -> DeviceProfile {
        DeviceProfile { id: "fake_phone".into(), name: "Fake Phone".into(), width_pt: 400, height_pt: 800, scale: 2.0, os: "fake-1".into(), default: true }
    }

    fn node(role: Role, label: &str, frame: (f32, f32, f32, f32), clickable: bool) -> RawNode {
        RawNode {
            role: Some(role),
            label: Some(label.to_string()),
            enabled: true,
            clickable,
            frame: Some(Rect { x: frame.0, y: frame.1, w: frame.2, h: frame.3 }),
            ..Default::default()
        }
    }

    fn check_running(&self, dev: &DeviceRef) -> Result<(), LaneError> {
        if self.running.lock().contains(&dev.device_id) {
            Ok(())
        } else {
            Err(LaneError::not_running("fake device is not running"))
        }
    }
}

impl SimLane for FakeLane {
    fn kind(&self) -> LaneKind {
        self.kind
    }

    fn probe(&self) -> LaneProbe {
        let ready = self.ready.load(Ordering::SeqCst);
        LaneProbe {
            lane: self.kind,
            usable: true,
            ready,
            needs: if ready { vec![] } else { vec![LaneNeed::Licence { id: "fake-license".into(), title: "Fake SDK licence".into() }] },
            detail: None,
            versions: vec![],
        }
    }

    fn install(&self, licence_accepted: &[String], _progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
        if !licence_accepted.iter().any(|l| l == "fake-license") {
            return Err(LaneError::NeedsSetup { message: "accept the fake licence".into(), needs: vec![] });
        }
        self.ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn profiles(&self) -> Result<Vec<DeviceProfile>, LaneError> {
        Ok(vec![Self::profile()])
    }

    fn boot(&self, req: &BootRequest) -> Result<BootedDevice, LaneError> {
        if self.fail_boot.load(Ordering::SeqCst) {
            return Err(LaneError::tool("fake boot failed"));
        }
        self.boots.fetch_add(1, Ordering::SeqCst);
        let id = format!("fake-{}", req.workspace_key);
        self.running.lock().push(id.clone());
        Ok(BootedDevice { device_id: id, profile: Self::profile(), pid: None, lane_state: serde_json::json!({}) })
    }

    fn adopt(&self, dev: &DeviceRef) -> Result<Option<BootedDevice>, LaneError> {
        Ok(self.running.lock().contains(&dev.device_id).then(|| BootedDevice {
            device_id: dev.device_id.clone(),
            profile: Self::profile(),
            pid: None,
            lane_state: dev.lane_state.clone(),
        }))
    }

    fn stop(&self, dev: &DeviceRef) -> Result<(), LaneError> {
        self.running.lock().retain(|d| d != &dev.device_id);
        Ok(())
    }

    fn erase(&self, workspace_key: &str, data_dir: &Path) -> Result<(), LaneError> {
        self.running.lock().retain(|d| d != &format!("fake-{workspace_key}"));
        if data_dir.exists() {
            std::fs::remove_dir_all(data_dir).map_err(|e| LaneError::tool(e.to_string()))?;
        }
        Ok(())
    }

    fn install_app(&self, dev: &DeviceRef, artifact: &Path) -> Result<InstallOutcome, LaneError> {
        self.check_running(dev)?;
        Ok(InstallOutcome { app_id: artifact.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), version: None, replaced: false })
    }

    fn launch(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        self.check_running(dev)?;
        self.actions.lock().push(format!("launch {app_id}"));
        Ok(())
    }

    fn terminate(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        self.check_running(dev)?;
        self.actions.lock().push(format!("terminate {app_id}"));
        Ok(())
    }

    fn tree(&self, dev: &DeviceRef) -> Result<RawTree, LaneError> {
        self.check_running(dev)?;
        let mut root = Self::node(Role::Window, "", (0.0, 0.0, 400.0, 800.0), false);
        root.label = None;
        let mut email = Self::node(Role::Textfield, "Email", (16.0, 100.0, 368.0, 48.0), true);
        email.value = Some(self.email.lock().clone());
        root.children.push(email);
        root.children.push(Self::node(Role::Button, "Save", (150.0, 700.0, 100.0, 48.0), true));
        if self.saved.load(Ordering::SeqCst) {
            root.children.push(Self::node(Role::Text, "Saved", (16.0, 760.0, 200.0, 20.0), false));
        }
        Ok(RawTree { width_pt: 400.0, height_pt: 800.0, app_id: Some("com.fake.app".into()), title: None, roots: vec![root], took_ms: 1 })
    }

    fn act(&self, dev: &DeviceRef, action: &Action) -> Result<(), LaneError> {
        self.check_running(dev)?;
        match action {
            Action::Tap { at } => {
                if at.y >= 700.0 && at.y < 748.0 && at.x >= 150.0 && at.x < 250.0 {
                    self.saved.store(true, Ordering::SeqCst);
                }
                self.actions.lock().push(format!("tap {},{}", at.x, at.y));
            }
            Action::Type { text } => {
                self.email.lock().push_str(text.expose());
                self.actions.lock().push(format!("type {:?}", text));
            }
            // A person's keyboard: never the text itself (it may be a pasted secret).
            Action::Keystrokes { text } => self.actions.lock().push(format!("keys {}", text.len_chars())),
            Action::Paste { text } => self.actions.lock().push(format!("paste {}", text.len_chars())),
            Action::Key { key, mods } => {
                let mut s = format!("key {key:?}");
                if mods.shift {
                    s.push_str("+shift");
                }
                if mods.alt {
                    s.push_str("+alt");
                }
                self.actions.lock().push(s)
            }
            Action::Edit { op } => self.actions.lock().push(format!("edit {op:?}")),
            other => self.actions.lock().push(other.verb().to_string()),
        }
        Ok(())
    }

    fn screenshot(&self, dev: &DeviceRef) -> Result<Frame, LaneError> {
        self.check_running(dev)?;
        Ok(Frame {
            width_px: 800,
            height_px: 1600,
            scale: 2.0,
            orientation: Orientation::Portrait,
            seq: 1,
            captured_at: Instant::now(),
            data: FrameData::Rgb888(vec![128u8; 800 * 1600 * 3]),
        })
    }

    fn frames(&self, dev: &DeviceRef, _opts: FrameOpts) -> Result<Box<dyn FrameSource>, LaneError> {
        self.check_running(dev)?;
        Ok(Box::new(FakeFrames { seq: 0 }))
    }

    fn logs(&self, dev: &DeviceRef, q: &LogQuery) -> Result<Vec<String>, LaneError> {
        self.check_running(dev)?;
        let mut v: Vec<String> = (0..10).map(|i| format!("I/fake: line {i}")).collect();
        v.push("I/fake: token=sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
        if let Some(g) = &q.grep {
            v.retain(|l| l.contains(g.as_str()));
        }
        Ok(v)
    }
}

/// A 4x8 px gray frame, a new one on every pull.
pub struct FakeFrames {
    seq: u64,
}

impl FrameSource for FakeFrames {
    fn next_frame(&mut self, _wait: Duration) -> Result<Option<Frame>, LaneError> {
        self.seq += 1;
        Ok(Some(Frame {
            width_px: 4,
            height_px: 8,
            scale: 1.0,
            orientation: Orientation::Portrait,
            seq: self.seq,
            captured_at: Instant::now(),
            data: FrameData::Rgb888(vec![(self.seq % 255) as u8; 4 * 8 * 3]),
        }))
    }
}
