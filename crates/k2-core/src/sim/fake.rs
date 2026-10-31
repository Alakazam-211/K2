//! A fake lane for tests (core and daemon): an in-memory "device" with a
//! small settings screen. Tapping the Save button adds a `"Saved"` text;
//! typing sets the focused field's value. Records every action.
//!
//! Licences: [`FakeLane::licences`] are the ones its plan knows up front
//! (the probe lists each one not yet accepted); [`FakeLane::hidden_licences`]
//! are ones only the install finds (an install that fails for a licence the
//! owner was never asked for). The install needs every one of both.

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
    /// `(id, title)` the probe knows before setup.
    pub licences: Mutex<Vec<(String, String)>>,
    /// `(id, title)` only the install finds.
    pub hidden_licences: Mutex<Vec<(String, String)>>,
    /// The install fails as a tool error (a download that broke).
    pub fail_install: AtomicBool,
    pub installs: AtomicU64,
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
            licences: Mutex::new(vec![("fake-license".into(), "Fake SDK licence".into())]),
            hidden_licences: Mutex::new(Vec::new()),
            fail_install: AtomicBool::new(false),
            installs: AtomicU64::new(0),
        }
    }

    /// The text of a fake licence (its sha1 is what an acceptance binds).
    pub fn licence_text(id: &str, title: &str) -> String {
        format!("{title}: the terms of {id}.")
    }

    fn all_licences(&self) -> Vec<(String, String)> {
        let mut v = self.licences.lock().clone();
        v.extend(self.hidden_licences.lock().iter().cloned());
        v
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
        let mut needs = Vec::new();
        if !ready {
            let accepted = super::licence::accepted_ids(self.kind);
            for (id, title) in self.licences.lock().iter() {
                if !accepted.contains(id) {
                    needs.push(LaneNeed::Licence { id: id.clone(), title: title.clone() });
                }
            }
            needs.push(LaneNeed::Install { what: "the fake tools".into(), bytes: Some(1_000_000) });
        }
        LaneProbe { lane: self.kind, usable: true, ready, needs, detail: None, versions: vec![], setup: None }
    }

    fn install(&self, licence_accepted: &[String], _progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        let missing: Vec<LaneNeed> = self
            .all_licences()
            .into_iter()
            .filter(|(id, _)| !licence_accepted.contains(id))
            .map(|(id, title)| LaneNeed::Licence { id, title })
            .collect();
        if !missing.is_empty() {
            let ids: Vec<String> = missing.iter().filter_map(|n| if let LaneNeed::Licence { id, .. } = n { Some(format!("'{id}'")) } else { None }).collect();
            return Err(LaneError::NeedsSetup { message: format!("the fake tools need the licence {}, which the owner hasn't accepted", ids.join(", ")), needs: missing });
        }
        if self.fail_install.load(Ordering::SeqCst) {
            return Err(LaneError::tool("download https://fake.invalid/tools.zip: connection reset"));
        }
        self.ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn licence(&self, id: &str) -> Result<LicenceText, LaneError> {
        let (id, title) = self
            .all_licences()
            .into_iter()
            .find(|(lid, _)| lid == id)
            .ok_or_else(|| LaneError::invalid(format!("no fake licence '{id}'")))?;
        let text = Self::licence_text(&id, &title);
        Ok(LicenceText { sha1: super::licence::text_sha1(&text), id, title, text, url: None })
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
            Action::Touch { phase, at } => self.actions.lock().push(format!("touch {phase:?} {},{}", at.x, at.y)),
            Action::TwoTouch { phase, a, b } => {
                if !self.multi_touch() {
                    return Err(LaneError::unsupported("Pinch isn't available on this device yet."));
                }
                self.actions.lock().push(format!("two-touch {phase:?} {},{} {},{}", a.x, a.y, b.x, b.y))
            }
            other => self.actions.lock().push(other.verb().to_string()),
        }
        Ok(())
    }

    /// Like the real lanes: Android takes two fingers, iOS doesn't yet.
    fn multi_touch(&self) -> bool {
        self.kind == LaneKind::Android
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
