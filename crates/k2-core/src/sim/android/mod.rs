//! The Android emulator lane (`prd-mobile-dev-v1` §3.2, L-A; S0 measured
//! on a Ryzen KVM box: cold boot 22–37 s, Quick Boot 4.3 s, ~3.5–3.9 GB
//! RSS, gRPC frames ~59 fps at points size, taps 1–5 ms, uiautomator ~2 s).
//!
//! Everything lives under `~/.k2/sim/android`:
//! - `sdk/`: platform-tools, emulator, one `google_apis` image (first use,
//!   after the box owner accepts the licence; [`sdk`]);
//! - `devices/<workspace-key>/`: the workspace's AVD (`avd/k2-<key>.avd`),
//!   the emulator's home, its runtime dir (the gRPC token file, 0700) and
//!   its log;
//! - `user/`: K2's own adb key.
//!
//! K2 runs its own adb server on [`ADB_SERVER_PORT`], never the person's,
//! and passes a clean environment to every tool ([`proc`]). The emulator's
//! console, adb and gRPC ports bind loopback only; gRPC needs the token
//! the emulator writes to its 0600 discovery file, which only the daemon
//! reads. Typed text goes over gRPC, never argv.

pub mod apk;
pub mod frames;
pub mod grpc;
pub mod proc;
pub mod sdk;
pub mod ui;
pub mod xml;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::json;

use super::lane::*;

/// K2's own adb server (the default 5037 may be the person's).
pub const ADB_SERVER_PORT: u16 = 5137;
/// Emulator console ports are even, 5554–5682 (adb = console + 1).
const CONSOLE_PORTS: std::ops::RangeInclusive<u16> = 5554..=5682;
const GRPC_BASE: u16 = 8554;
const ADB_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const TREE_TIMEOUT: Duration = Duration::from_secs(20);
const GRPC_TIMEOUT: Duration = Duration::from_secs(15);

/// A phone the lane offers (portrait px and dpi).
struct Phone {
    id: &'static str,
    name: &'static str,
    w: u32,
    h: u32,
    dpi: u32,
}

const PHONES: &[Phone] = &[
    Phone { id: "pixel_9", name: "Pixel 9", w: 1080, h: 2424, dpi: 420 },
    Phone { id: "pixel_9_pro_xl", name: "Pixel 9 Pro XL", w: 1344, h: 2992, dpi: 480 },
    Phone { id: "pixel_8a", name: "Pixel 8a", w: 1080, h: 2400, dpi: 420 },
    Phone { id: "small_phone", name: "Small phone", w: 720, h: 1280, dpi: 320 },
];

fn phone_profile(p: &Phone, default: bool) -> DeviceProfile {
    let scale = p.dpi as f32 / 160.0;
    DeviceProfile {
        id: p.id.to_string(),
        name: p.name.to_string(),
        width_pt: (p.w as f32 / scale).round() as u32,
        height_pt: (p.h as f32 / scale).round() as u32,
        scale,
        os: format!("android-{} google_apis {}", sdk::API, sdk::host_abi()),
        default,
    }
}

fn find_phone(id_or_name: Option<&str>) -> Result<&'static Phone, LaneError> {
    match id_or_name.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(&PHONES[0]),
        Some(want) => PHONES
            .iter()
            .find(|p| p.id.eq_ignore_ascii_case(want) || p.name.eq_ignore_ascii_case(want))
            .ok_or_else(|| LaneError::invalid(format!("no Android device '{want}' (try: {})", PHONES.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))),
    }
}

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("k2-sim-android")
            .enable_all()
            .build()
            .expect("the Android lane's runtime starts")
    })
}

/// The lane-private state stored on the sim row (no secrets).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    console_port: u16,
    grpc_port: u16,
    pid: u32,
    profile: String,
    width_px: u32,
    height_px: u32,
    scale: f32,
}

pub struct AndroidLane {
    root: PathBuf,
    sdk: sdk::Sdk,
}

impl AndroidLane {
    /// `root` = `~/.k2/sim/android`.
    pub fn new(root: PathBuf) -> AndroidLane {
        AndroidLane { sdk: sdk::Sdk::new(root.join("sdk")), root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn avd_name(key: &str) -> String {
        format!("k2-{key}")
    }

    fn env(&self, data_dir: Option<&Path>) -> HashMap<String, String> {
        let mut e = HashMap::new();
        let sdk = self.sdk.root.display().to_string();
        e.insert("ANDROID_SDK_ROOT".into(), sdk.clone());
        e.insert("ANDROID_HOME".into(), sdk);
        e.insert("ANDROID_USER_HOME".into(), self.root.join("user").display().to_string());
        e.insert("ANDROID_ADB_SERVER_PORT".into(), ADB_SERVER_PORT.to_string());
        if let Some(d) = data_dir {
            e.insert("ANDROID_AVD_HOME".into(), d.join("avd").display().to_string());
            e.insert("ANDROID_EMULATOR_HOME".into(), d.join("home").display().to_string());
            e.insert("XDG_RUNTIME_DIR".into(), d.join("run").display().to_string());
        }
        e
    }

    fn adb(&self, args: &[&str], timeout: Duration, max_out: usize) -> Result<proc::Out, LaneError> {
        proc::run(&self.sdk.adb(), args, &self.env(None), timeout, max_out)
    }

    fn serial(st: &State) -> String {
        format!("emulator-{}", st.console_port)
    }

    /// `adb -s <serial> shell <cmd>` (the command is a device shell line;
    /// callers quote every value with [`proc::sh_quote`]).
    fn shell(&self, st: &State, cmd: &str, timeout: Duration) -> Result<proc::Out, LaneError> {
        let s = Self::serial(st);
        self.adb(&["-s", &s, "shell", cmd], timeout, 4 * 1024 * 1024)
    }

    fn shell_ok(&self, st: &State, cmd: &str, what: &str) -> Result<String, LaneError> {
        let o = self.shell(st, cmd, ADB_TIMEOUT)?;
        if !o.ok() {
            return Err(LaneError::tool(o.failure(what)));
        }
        Ok(o.text())
    }

    fn state(dev: &DeviceRef) -> Result<State, LaneError> {
        serde_json::from_value(dev.lane_state.clone()).map_err(|e| LaneError::not_running(format!("no Android device state ({e})")))
    }

    /// The emulator's discovery file for `console_port` under `run_dir`:
    /// `(pid, grpc port, token)`.
    fn discovery(run_dir: &Path, console_port: u16) -> Option<(u32, u16, String)> {
        let dir = run_dir.join("avd").join("running");
        for e in std::fs::read_dir(&dir).ok()?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(pid) = name.strip_prefix("pid_").and_then(|s| s.strip_suffix(".ini")).and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string());
            if get("port.serial=").and_then(|p| p.parse::<u16>().ok()) != Some(console_port) {
                continue;
            }
            let (Some(port), Some(tok)) = (get("grpc.port=").and_then(|p| p.parse().ok()), get("grpc.token=")) else { continue };
            return Some((pid, port, tok));
        }
        None
    }

    fn grpc(&self, dev: &DeviceRef, st: &State) -> Result<grpc::EmuGrpc, LaneError> {
        let (_, port, tok) = Self::discovery(&dev.data_dir.join("run"), st.console_port)
            .ok_or_else(|| LaneError::not_running("the emulator's control file is gone; is it still running?"))?;
        Ok(grpc::EmuGrpc::new(port, tok))
    }

    fn block<T>(fut: impl std::future::Future<Output = Result<T, grpc::GrpcError>>) -> Result<T, LaneError> {
        rt().block_on(async { tokio::time::timeout(GRPC_TIMEOUT, fut).await })
            .map_err(|_| LaneError::timeout("the emulator did not answer in time"))?
            .map_err(|e| LaneError::tool(e.to_string()))
    }

    fn port_free(p: u16) -> bool {
        std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()
    }

    /// Even console port with console+1 free, and a free gRPC port.
    fn pick_ports() -> Result<(u16, u16), LaneError> {
        for c in CONSOLE_PORTS.step_by(2) {
            if Self::port_free(c) && Self::port_free(c + 1) {
                let g = GRPC_BASE + (c - 5554) / 2;
                if Self::port_free(g) {
                    return Ok((c, g));
                }
            }
        }
        Err(LaneError::unavailable("no free emulator ports on 127.0.0.1 (5554–5683)"))
    }

    fn write_avd(&self, req: &BootRequest, phone: &Phone) -> Result<String, LaneError> {
        let name = Self::avd_name(&req.workspace_key);
        let avd_home = req.data_dir.join("avd");
        let avd_dir = avd_home.join(format!("{name}.avd"));
        for d in [&avd_dir, &req.data_dir.join("home"), &req.data_dir.join("run"), &self.root.join("user")] {
            std::fs::create_dir_all(d).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", d.display())))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(req.data_dir.join("run"), std::fs::Permissions::from_mode(0o700));
        }
        let ini = format!("avd.ini.encoding=UTF-8\npath={}\ntarget=android-{}\n", avd_dir.display(), sdk::API);
        std::fs::write(avd_home.join(format!("{name}.ini")), ini).map_err(|e| LaneError::tool(e.to_string()))?;
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 4);
        let config = format!(
            "avd.ini.encoding=UTF-8\nAvdId={name}\navd.ini.displayname={} (K2)\nPlayStore.enabled=false\nabi.type={abi}\nhw.cpu.arch={arch}\n\
             hw.cpu.ncore={cores}\nhw.ramSize=2048\nhw.lcd.width={}\nhw.lcd.height={}\nhw.lcd.density={}\nhw.keyboard=yes\n\
             hw.audioInput=no\nhw.audioOutput=no\nhw.gpu.enabled=yes\nhw.gpu.mode=swiftshader_indirect\n\
             image.sysdir.1={}\ntag.id=google_apis\ntarget=android-{}\ndisk.dataPartition.size=6G\nfastboot.forceColdBoot=no\n",
            phone.name,
            phone.w,
            phone.h,
            phone.dpi,
            self.sdk.image_rel(),
            sdk::API,
            abi = sdk::host_abi(),
            arch = if sdk::host_abi() == "x86_64" { "x86_64" } else { "arm64" },
        );
        std::fs::write(avd_dir.join("config.ini"), config).map_err(|e| LaneError::tool(e.to_string()))?;
        Ok(name)
    }

    fn log_tail(data_dir: &Path) -> String {
        let s = std::fs::read_to_string(data_dir.join("emulator.log")).unwrap_or_default();
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        crate::redaction::redact(&lines[lines.len().saturating_sub(12)..].join("\n"))
    }

    fn booted(&self, st: &State) -> bool {
        self.shell(st, "getprop sys.boot_completed", Duration::from_secs(5)).map(|o| o.ok() && o.text().trim() == "1").unwrap_or(false)
    }

    fn kvm_check() -> Result<(), (String, Option<LaneNeed>)> {
        #[cfg(target_os = "linux")]
        {
            match std::fs::OpenOptions::new().read(true).write(true).open("/dev/kvm") {
                Ok(_) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Err(("/dev/kvm is missing: this server has no hardware virtualisation (KVM) for the emulator".into(), None))
                }
                Err(e) => {
                    let user = std::env::var("USER").unwrap_or_else(|_| "the K2 user".into());
                    Err((
                        format!("/dev/kvm: {e} for {user}"),
                        Some(LaneNeed::OwnerStep {
                            what: format!("Let {user} use KVM (add it to the kvm group), then restart K2"),
                            command: format!("sudo usermod -aG kvm {user}"),
                        }),
                    ))
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            let o = std::process::Command::new("sysctl").args(["-n", "kern.hv_support"]).output();
            match o {
                Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "1" => Ok(()),
                _ => Err(("this Mac has no Hypervisor.framework support".into(), None)),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(("the Android lane runs on Linux and macOS".into(), None))
        }
    }

    fn rotation(st: &State, o: Orientation) -> u32 {
        let _ = st;
        match o {
            Orientation::Portrait => 0,
            Orientation::LandscapeLeft => 1,
            Orientation::PortraitUpsideDown => 2,
            Orientation::LandscapeRight => 3,
        }
    }

    /// Current display size in px for the orientation (`wm size` is the
    /// natural size; landscape swaps it).
    fn screen_px(&self, st: &State) -> (u32, u32) {
        let rot = self.shell(st, "settings get system user_rotation", Duration::from_secs(5)).ok().and_then(|o| o.text().trim().parse::<u32>().ok()).unwrap_or(0);
        if rot % 2 == 1 {
            (st.height_px, st.width_px)
        } else {
            (st.width_px, st.height_px)
        }
    }

    fn touch(&self, g: &grpc::EmuGrpc, st: &State, at: Point, down: bool) -> Result<(), LaneError> {
        let ev = grpc::TouchEvent {
            touches: vec![grpc::Touch {
                x: (at.x * st.scale).round() as i32,
                y: (at.y * st.scale).round() as i32,
                identifier: 0,
                pressure: if down { 1 } else { 0 },
                ..Default::default()
            }],
            display: 0,
        };
        let g = g.clone();
        Self::block(async move { g.send_touch(&ev).await })
    }

    fn key_name(k: Key) -> &'static str {
        match k {
            Key::Enter => "Enter",
            Key::Backspace => "Backspace",
            Key::Delete => "Delete",
            Key::Tab => "Tab",
            Key::Escape => "Escape",
            Key::ArrowUp => "ArrowUp",
            Key::ArrowDown => "ArrowDown",
            Key::ArrowLeft => "ArrowLeft",
            Key::ArrowRight => "ArrowRight",
        }
    }

    fn keycode(b: Button) -> &'static str {
        match b {
            Button::Home => "KEYCODE_HOME",
            Button::Back => "KEYCODE_BACK",
            Button::Lock => "KEYCODE_SLEEP",
            Button::VolumeUp => "KEYCODE_VOLUME_UP",
            Button::VolumeDown => "KEYCODE_VOLUME_DOWN",
            Button::AppSwitch => "KEYCODE_APP_SWITCH",
            Button::DevMenu => "KEYCODE_MENU",
        }
    }
}

impl SimLane for AndroidLane {
    fn kind(&self) -> LaneKind {
        LaneKind::Android
    }

    fn probe(&self) -> LaneProbe {
        let mut p = LaneProbe { lane: LaneKind::Android, usable: false, ready: false, needs: vec![], detail: None, versions: vec![] };
        if let Err((why, need)) = Self::kvm_check() {
            p.detail = Some(why);
            p.needs.extend(need);
            return p;
        }
        p.usable = true;
        if !crate::sim::licence::accepted_ids(LaneKind::Android).iter().any(|l| l == sdk::LICENCE_ID) {
            p.needs.push(LaneNeed::Licence { id: sdk::LICENCE_ID.into(), title: sdk::LICENCE_TITLE.into() });
        }
        let missing = self.sdk.missing();
        if !missing.is_empty() {
            p.needs.push(LaneNeed::Install { what: format!("Android {} (from Google)", missing.join(", ")), bytes: Some(2_200_000_000) });
        }
        for (k, rel) in [("emulator", "emulator"), ("platform-tools", "platform-tools"), ("image", self.sdk.image_rel().as_str())] {
            if let Some(rev) = std::fs::read_to_string(self.sdk.root.join(rel).join(".k2-package.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["revision"].as_str().map(str::to_string))
            {
                p.versions.push((k.to_string(), rev));
            }
        }
        p.ready = p.needs.is_empty();
        p.detail = Some(if p.ready { "ready".into() } else { p.needs.iter().map(need_line).collect::<Vec<_>>().join("; ") });
        p
    }

    fn licence(&self, id: &str) -> Result<LicenceText, LaneError> {
        sdk::licence(&self.sdk, id)
    }

    fn install(&self, licence_accepted: &[String], progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
        if !licence_accepted.iter().any(|l| l == sdk::LICENCE_ID) {
            return Err(LaneError::NeedsSetup {
                message: "the server's owner hasn't accepted the Android SDK licence".into(),
                needs: vec![LaneNeed::Licence { id: sdk::LICENCE_ID.into(), title: sdk::LICENCE_TITLE.into() }],
            });
        }
        std::fs::create_dir_all(&self.sdk.root).map_err(|e| LaneError::tool(format!("mkdir {}: {e}", self.sdk.root.display())))?;
        sdk::install(&self.sdk, licence_accepted, progress)
    }

    fn profiles(&self) -> Result<Vec<DeviceProfile>, LaneError> {
        Ok(PHONES.iter().enumerate().map(|(i, p)| phone_profile(p, i == 0)).collect())
    }

    fn boot(&self, req: &BootRequest) -> Result<BootedDevice, LaneError> {
        if !self.sdk.installed() {
            return Err(LaneError::NeedsSetup { message: "the Android SDK is not installed yet".into(), needs: self.probe().needs });
        }
        let phone = find_phone(req.profile_id.as_deref())?;
        // A device left running for this workspace (another daemon run)
        // is stopped first: one device per workspace.
        for pid in proc::pids_matching(&format!("-avd {}", Self::avd_name(&req.workspace_key))) {
            proc::terminate(pid, Duration::from_secs(5));
        }
        let name = self.write_avd(req, phone)?;
        let (console, grpc_port) = Self::pick_ports()?;
        let run_dir = req.data_dir.join("run");
        let _ = std::fs::remove_dir_all(run_dir.join("avd").join("running"));
        let log = std::fs::File::create(req.data_dir.join("emulator.log")).map_err(|e| LaneError::tool(e.to_string()))?;
        let log2 = log.try_clone().map_err(|e| LaneError::tool(e.to_string()))?;
        let mut cmd = std::process::Command::new(self.sdk.emulator());
        cmd.args([
            "-avd",
            &name,
            "-no-window",
            "-no-audio",
            "-no-boot-anim",
            "-gpu",
            "swiftshader_indirect",
            "-port",
            &console.to_string(),
            "-grpc",
            &grpc_port.to_string(),
            "-grpc-use-token",
            "-no-metrics",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log2);
        proc::clean_env(&mut cmd, &self.env(Some(&req.data_dir)));
        // Its own process group: the device outlives a daemon restart and is
        // re-adopted (M-A15).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| LaneError::tool(format!("start the emulator: {e}")))?;
        let t0 = Instant::now();
        let fail = |child: &mut std::process::Child, msg: String| {
            let _ = child.kill();
            for pid in proc::pids_matching(&format!("-avd {name}")) {
                proc::terminate(pid, Duration::from_secs(3));
            }
            LaneError::tool(format!("{msg}\n{}", Self::log_tail(&req.data_dir)))
        };
        // Wait for the discovery file (pid + gRPC token), then boot complete.
        let mut found = None;
        let mut st: Option<State> = None;
        loop {
            if t0.elapsed() > req.boot_timeout {
                let _ = child.kill();
                return Err(LaneError::timeout(format!("the emulator did not finish booting in {}s\n{}", req.boot_timeout.as_secs(), Self::log_tail(&req.data_dir))));
            }
            if found.is_none() {
                found = Self::discovery(&run_dir, console);
            }
            if let Some((pid, gport, _)) = &found {
                if !proc::pid_alive(*pid) {
                    return Err(fail(&mut child, "the emulator exited while booting".into()));
                }
                let s = st.get_or_insert_with(|| State {
                    console_port: console,
                    grpc_port: *gport,
                    pid: *pid,
                    profile: phone.id.to_string(),
                    width_px: phone.w,
                    height_px: phone.h,
                    scale: phone.dpi as f32 / 160.0,
                });
                if self.booted(s) {
                    break;
                }
            } else if let Ok(Some(status)) = child.try_wait() {
                // The launcher may exec qemu and exit 0; only an early
                // failure with no discovery file is fatal.
                if !status.success() {
                    return Err(fail(&mut child, format!("the emulator failed to start ({status})")));
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let st = st.ok_or_else(|| LaneError::tool("booted without state"))?;
        Ok(BootedDevice {
            device_id: format!("{name}@{console}"),
            profile: phone_profile(phone, phone.id == PHONES[0].id),
            pid: Some(st.pid),
            lane_state: serde_json::to_value(&st).map_err(|e| LaneError::tool(e.to_string()))?,
        })
    }

    fn adopt(&self, dev: &DeviceRef) -> Result<Option<BootedDevice>, LaneError> {
        let Ok(st) = Self::state(dev) else { return Ok(None) };
        if !self.alive(dev) {
            return Ok(None);
        }
        let phone = find_phone(Some(&st.profile)).unwrap_or(&PHONES[0]);
        Ok(Some(BootedDevice {
            device_id: dev.device_id.clone(),
            profile: phone_profile(phone, false),
            pid: Some(st.pid),
            lane_state: dev.lane_state.clone(),
        }))
    }

    fn alive(&self, dev: &DeviceRef) -> bool {
        let Ok(st) = Self::state(dev) else { return false };
        proc::pid_alive(st.pid) && proc::cmdline(st.pid).is_some_and(|c| c.contains(&Self::avd_name(&dev.workspace_key)))
    }

    fn stop(&self, dev: &DeviceRef) -> Result<(), LaneError> {
        let st = Self::state(dev)?;
        if !self.alive(dev) {
            return Ok(());
        }
        // `emu kill` saves the Quick Boot snapshot (S0: 5.2 s), so the
        // next boot takes ~4 s.
        let s = Self::serial(&st);
        let _ = self.adb(&["-s", &s, "emu", "kill"], Duration::from_secs(10), 4096);
        let t0 = Instant::now();
        while proc::pid_alive(st.pid) && t0.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(200));
        }
        if proc::pid_alive(st.pid) {
            proc::terminate(st.pid, Duration::from_secs(5));
        }
        Ok(())
    }

    fn erase(&self, workspace_key: &str, data_dir: &Path) -> Result<(), LaneError> {
        let devices = self.root.join("devices");
        if !data_dir.starts_with(&devices) || data_dir == devices {
            return Err(LaneError::invalid(format!("{} is not an Android device folder", data_dir.display())));
        }
        for pid in proc::pids_matching(&format!("-avd {}", Self::avd_name(workspace_key))) {
            proc::terminate(pid, Duration::from_secs(5));
        }
        if data_dir.exists() {
            std::fs::remove_dir_all(data_dir).map_err(|e| LaneError::tool(format!("remove {}: {e}", data_dir.display())))?;
        }
        Ok(())
    }

    fn install_app(&self, dev: &DeviceRef, artifact: &Path) -> Result<InstallOutcome, LaneError> {
        let st = Self::state(dev)?;
        if artifact.is_dir() || artifact.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() != Some("apk") {
            return Err(LaneError::invalid(format!("{} is not an .apk (an .aab needs bundletool to make APKs first)", artifact.display())));
        }
        let info = apk::read(artifact).map_err(LaneError::invalid)?;
        let had = self.shell(&st, &format!("pm path {}", proc::sh_quote(&info.package)), ADB_TIMEOUT).map(|o| o.ok() && o.text().contains("package:")).unwrap_or(false);
        let s = Self::serial(&st);
        let path = artifact.display().to_string();
        let o = self.adb(&["-s", &s, "install", "-r", "-t", &path], INSTALL_TIMEOUT, 64 * 1024)?;
        if !o.ok() || !o.text().contains("Success") {
            return Err(LaneError::tool(o.failure("adb install")));
        }
        Ok(InstallOutcome { app_id: info.package, version: info.version_name, replaced: had })
    }

    fn launch(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        let st = Self::state(dev)?;
        let out = self.shell_ok(&st, &format!("cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {}", proc::sh_quote(app_id)), "find the app's launcher")?;
        let component = out.lines().map(str::trim).filter(|l| l.contains('/')).last().ok_or_else(|| LaneError::invalid(format!("{app_id} is not installed or has no launcher screen")))?;
        let o = self.shell_ok(&st, &format!("am start -W -n {}", proc::sh_quote(component)), "launch")?;
        if o.contains("Error:") {
            return Err(LaneError::tool(crate::redaction::redact(o.trim())));
        }
        Ok(())
    }

    fn terminate(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        let st = Self::state(dev)?;
        self.shell_ok(&st, &format!("am force-stop {}", proc::sh_quote(app_id)), "stop the app").map(|_| ())
    }

    fn tree(&self, dev: &DeviceRef) -> Result<RawTree, LaneError> {
        let st = Self::state(dev)?;
        let t0 = Instant::now();
        let s = Self::serial(&st);
        let screen = self.screen_px(&st);
        // Right after a launch or a transition uiautomator may answer
        // "could not get idle state" / "null root node": try up to three
        // times before failing with the device's own words.
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(600));
            }
            let o = self.adb(&["-s", &s, "exec-out", "uiautomator", "dump", "/dev/tty"], TREE_TIMEOUT, 8 * 1024 * 1024)?;
            let text = o.text();
            match ui::parse_dump(&text, st.scale, screen) {
                Ok(mut t) => {
                    t.took_ms = t0.elapsed().as_millis() as u64;
                    return Ok(t);
                }
                Err(e) => {
                    let said: String = text.trim().lines().next().unwrap_or("").chars().take(200).collect();
                    last_err = if said.is_empty() { format!("{e} ({})", o.failure("uiautomator dump")) } else { format!("{e}: {said}") };
                }
            }
        }
        Err(LaneError::tool(crate::redaction::redact(&format!("read the screen: {last_err}"))))
    }

    fn act(&self, dev: &DeviceRef, action: &Action) -> Result<(), LaneError> {
        let st = Self::state(dev)?;
        match action {
            Action::Tap { at } => {
                let g = self.grpc(dev, &st)?;
                self.touch(&g, &st, *at, true)?;
                std::thread::sleep(Duration::from_millis(40));
                self.touch(&g, &st, *at, false)
            }
            Action::LongPress { at, hold } => {
                let g = self.grpc(dev, &st)?;
                self.touch(&g, &st, *at, true)?;
                std::thread::sleep(*hold);
                self.touch(&g, &st, *at, false)
            }
            Action::Swipe { from, to, duration } => {
                let g = self.grpc(dev, &st)?;
                let steps = (duration.as_millis() / 16).clamp(4, 120) as u32;
                self.touch(&g, &st, *from, true)?;
                for i in 1..=steps {
                    let f = i as f32 / steps as f32;
                    let p = Point { x: from.x + (to.x - from.x) * f, y: from.y + (to.y - from.y) * f };
                    self.touch(&g, &st, p, true)?;
                    std::thread::sleep(*duration / steps);
                }
                self.touch(&g, &st, *to, false)
            }
            Action::Touch { phase, at } => {
                let g = self.grpc(dev, &st)?;
                self.touch(&g, &st, *at, !matches!(phase, TouchPhase::Up))
            }
            Action::Type { text } => {
                let g = self.grpc(dev, &st)?;
                // Over gRPC only: never argv, never a log line.
                let ev = grpc::KeyboardEvent { event_type: grpc::KEY_KEYPRESS, text: text.expose().to_string(), ..Default::default() };
                Self::block(async move { g.send_key(&ev).await })
            }
            Action::Key { key } => {
                let g = self.grpc(dev, &st)?;
                let ev = grpc::KeyboardEvent { event_type: grpc::KEY_KEYPRESS, key: Self::key_name(*key).to_string(), ..Default::default() };
                Self::block(async move { g.send_key(&ev).await })
            }
            Action::Button { button } => self.shell_ok(&st, &format!("input keyevent {}", Self::keycode(*button)), "press the button").map(|_| ()),
            Action::Rotate { orientation } => self
                .shell_ok(
                    &st,
                    &format!("settings put system accelerometer_rotation 0 && settings put system user_rotation {}", Self::rotation(&st, *orientation)),
                    "rotate",
                )
                .map(|_| ()),
            Action::OpenUrl { url } => {
                let o = self.shell_ok(&st, &format!("am start -a android.intent.action.VIEW -d {}", proc::sh_quote(url)), "open the URL")?;
                if o.contains("Error:") {
                    return Err(LaneError::invalid(crate::redaction::redact(o.trim())));
                }
                Ok(())
            }
            Action::Location { lat, lon } => {
                let g = self.grpc(dev, &st)?;
                let gps = grpc::GpsState { latitude: *lat, longitude: *lon, satellites: 6, ..Default::default() };
                Self::block(async move { g.set_gps(&gps).await })
            }
            Action::Appearance { appearance } => self
                .shell_ok(&st, &format!("cmd uimode night {}", if *appearance == Appearance::Dark { "yes" } else { "no" }), "set dark mode")
                .map(|_| ()),
        }
    }

    fn screenshot(&self, dev: &DeviceRef) -> Result<Frame, LaneError> {
        let st = Self::state(dev)?;
        let g = self.grpc(dev, &st)?;
        let img = Self::block(async move { g.screenshot(&grpc::ImageFormat { format: grpc::FMT_RGB888, ..Default::default() }).await })?;
        let (w, h) = img.format.as_ref().map(|f| (f.width, f.height)).unwrap_or((img.width, img.height));
        if img.image.len() != (w as usize) * (h as usize) * 3 {
            return Err(LaneError::tool(format!("screenshot of {} bytes is not {w}x{h} RGB", img.image.len())));
        }
        Ok(Frame {
            width_px: w,
            height_px: h,
            scale: st.scale,
            orientation: if w > h { Orientation::LandscapeLeft } else { Orientation::Portrait },
            seq: img.seq as u64,
            captured_at: Instant::now(),
            data: FrameData::Rgb888(img.image),
        })
    }

    fn frames(&self, dev: &DeviceRef, opts: FrameOpts) -> Result<Box<dyn FrameSource>, LaneError> {
        let st = Self::state(dev)?;
        let g = self.grpc(dev, &st)?;
        // A square box of the long side in points: the emulator keeps the
        // aspect ratio, so the frame is points-sized in either orientation.
        let long = ((st.width_px.max(st.height_px)) as f32 / st.scale).round() as u32;
        Ok(Box::new(frames::start(g, long, long, Orientation::Portrait, opts)?))
    }

    fn logs(&self, dev: &DeviceRef, q: &LogQuery) -> Result<Vec<String>, LaneError> {
        let st = Self::state(dev)?;
        let mut cmd = format!("logcat -d -v epoch -t {}", (q.max_lines * 20).clamp(100, 20_000));
        if let Some(app) = &q.app_id {
            let pid = self.shell(&st, &format!("pidof -s {}", proc::sh_quote(app)), ADB_TIMEOUT)?.text().trim().to_string();
            if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
                return Err(LaneError::invalid(format!("{app} is not running (launch it first)")));
            }
            cmd.push_str(&format!(" --pid={pid}"));
        }
        let o = self.shell(&st, &cmd, ADB_TIMEOUT)?;
        if !o.ok() {
            return Err(LaneError::tool(o.failure("logcat")));
        }
        let since = match q.since {
            Some(d) => {
                let now = self.shell(&st, "date +%s", Duration::from_secs(5))?.text().trim().parse::<f64>().unwrap_or(0.0);
                Some(now - d.as_secs_f64())
            }
            None => None,
        };
        let mut out = Vec::new();
        let mut bytes = 0usize;
        for l in o.text().lines() {
            if l.starts_with("--------- beginning of") {
                continue;
            }
            if let Some(min) = since {
                let t = l.split_whitespace().next().and_then(|t| t.parse::<f64>().ok()).unwrap_or(0.0);
                if t < min {
                    continue;
                }
            }
            if let Some(g) = &q.grep {
                if !l.contains(g.as_str()) {
                    continue;
                }
            }
            bytes += l.len();
            if bytes > q.max_bytes {
                break;
            }
            out.push(l.to_string());
        }
        let start = out.len().saturating_sub(q.max_lines);
        Ok(out.split_off(start))
    }
}

fn need_line(n: &LaneNeed) -> String {
    match n {
        LaneNeed::Licence { title, .. } => format!("the owner must accept the {title}"),
        LaneNeed::Install { what, .. } => format!("downloads {what} on first use"),
        LaneNeed::OwnerStep { what, command } => format!("{what}: {command}"),
    }
}

/// Lane-private state for tests and the live test binary.
#[doc(hidden)]
pub fn state_json(console: u16, grpc_port: u16, pid: u32) -> serde_json::Value {
    json!({ "consolePort": console, "grpcPort": grpc_port, "pid": pid, "profile": "pixel_9", "widthPx": 1080, "heightPx": 2424, "scale": 2.625 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_in_points_and_pixel_9_is_the_default() {
        let lane = AndroidLane::new(std::env::temp_dir().join("k2-android-profiles"));
        let p = lane.profiles().expect("profiles");
        assert_eq!(p[0].name, "Pixel 9");
        assert!(p[0].default && p.iter().filter(|x| x.default).count() == 1);
        assert_eq!((p[0].width_pt, p[0].height_pt), (411, 923));
        assert!((p[0].scale - 2.625).abs() < 1e-6);
        assert_eq!(find_phone(Some("pixel 8A")).expect("by name").id, "pixel_8a");
        assert!(find_phone(Some("iPhone 16")).is_err());
    }

    #[test]
    fn erase_refuses_paths_outside_its_devices_folder() {
        let lane = AndroidLane::new(std::env::temp_dir().join("k2-android-erase"));
        assert!(lane.erase("abc", Path::new("/tmp")).is_err());
        assert!(lane.erase("abc", &lane.root().join("devices")).is_err());
        assert!(lane.erase("abc", &lane.root().join("devices").join("abc")).is_ok(), "a missing folder is fine");
    }

    #[test]
    fn state_round_trips_and_has_no_token() {
        let v = state_json(5554, 8554, 42);
        let st: State = serde_json::from_value(v.clone()).expect("state");
        assert_eq!((st.console_port, st.grpc_port, st.pid), (5554, 8554, 42));
        assert!(!v.to_string().contains("token"));
    }
}
