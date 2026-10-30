//! `xcrun simctl` against K2's own device set, through a fakeable runner.
//!
//! Every simctl call is built by [`Simctl::args`], which always puts
//! `--set <K2 set>` first, so K2 never lists, boots or erases one of the
//! person's own Simulators (vs-live test 3). The runner is the seam the unit
//! tests replace (M-A18).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value as J;

/// The lane's internal error: a stable `code` plus a message for the agent.
/// `lane.rs` maps codes onto the shared `LaneError` variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimError {
    pub code: &'static str,
    pub message: String,
}

impl SimError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

pub type SimResult<T> = Result<T, SimError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Booted,
    Booting,
    Shutdown,
    ShuttingDown,
    Unknown,
}

/// The prefix of every device K2 creates: `K2-<workspace>-<model>`.
pub const DEVICE_PREFIX: &str = "K2-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs one program to completion, or kills it at `timeout`.
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> SimResult<Output>;

    /// Like [`Runner::run`], with `input` on the program's stdin (text that
    /// must never be on a command line, e.g. `simctl pbcopy`).
    fn run_input(&self, program: &str, _args: &[String], _input: &[u8], _timeout: Duration) -> SimResult<Output> {
        Err(SimError::new("spawn_failed", format!("this runner can't send input to {program}")))
    }
}

/// The real runner.
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> SimResult<Output> {
        run_process(program, args, None, timeout)
    }

    fn run_input(&self, program: &str, args: &[String], input: &[u8], timeout: Duration) -> SimResult<Output> {
        run_process(program, args, Some(input), timeout)
    }
}

fn run_process(program: &str, args: &[String], input: Option<&[u8]>, timeout: Duration) -> SimResult<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SimError::new("spawn_failed", format!("could not run {program}: {e}")))?;
    if let Some(bytes) = input {
        // Feed stdin on a thread (a big input never deadlocks against a
        // full stdout pipe); dropping the handle closes it (EOF).
        let mut stdin = child.stdin.take().expect("piped stdin");
        let bytes = bytes.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }
    // Drain both pipes on threads so a chatty child never blocks on a full pipe.
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let t_out = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.read_to_end(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SimError::new(
                    "timeout",
                    format!("{program} {} took longer than {}s", args.join(" "), timeout.as_secs()),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(SimError::new("spawn_failed", format!("waiting for {program}: {e}"))),
        }
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Ok(Output { status: status.code().unwrap_or(-1), stdout, stderr })
}

/// One simulator device as simctl lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub state: DeviceState,
    pub device_type: String,
    pub runtime: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceType {
    pub name: String,
    pub identifier: String,
    pub family: String,
    /// The `.simdevicetype` bundle (its profile.plist has the screen size).
    pub bundle_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub identifier: String,
    pub name: String,
    pub version: String,
    pub device_types: Vec<DeviceType>,
}

pub fn parse_state(s: &str) -> DeviceState {
    match s {
        "Booted" => DeviceState::Booted,
        "Booting" => DeviceState::Booting,
        "Shutdown" => DeviceState::Shutdown,
        "Shutting Down" => DeviceState::ShuttingDown,
        _ => DeviceState::Unknown,
    }
}

/// `simctl list -j devices` → devices.
pub fn parse_devices(json: &str) -> SimResult<Vec<SimDevice>> {
    let v: J = serde_json::from_str(json).map_err(|e| SimError::new("simctl_output", format!("simctl list devices: {e}")))?;
    let map = v
        .get("devices")
        .and_then(J::as_object)
        .ok_or_else(|| SimError::new("simctl_output", "simctl list devices: no `devices` object"))?;
    let mut out = Vec::new();
    for (runtime, list) in map {
        for d in list.as_array().into_iter().flatten() {
            let s = |k: &str| d.get(k).and_then(J::as_str).unwrap_or_default().to_string();
            out.push(SimDevice {
                udid: s("udid"),
                name: s("name"),
                state: parse_state(&s("state")),
                device_type: s("deviceTypeIdentifier"),
                runtime: runtime.clone(),
                available: d.get("isAvailable").and_then(J::as_bool).unwrap_or(false),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.udid.cmp(&b.udid)));
    Ok(out)
}

/// `simctl list -j runtimes` → available iOS runtimes, newest first.
pub fn parse_ios_runtimes(json: &str) -> SimResult<Vec<Runtime>> {
    let v: J = serde_json::from_str(json).map_err(|e| SimError::new("simctl_output", format!("simctl list runtimes: {e}")))?;
    let list = v
        .get("runtimes")
        .and_then(J::as_array)
        .ok_or_else(|| SimError::new("simctl_output", "simctl list runtimes: no `runtimes` array"))?;
    let mut out: Vec<Runtime> = list
        .iter()
        .filter(|r| r.get("platform").and_then(J::as_str) == Some("iOS"))
        .filter(|r| r.get("isAvailable").and_then(J::as_bool) == Some(true))
        .map(|r| {
            let s = |k: &str| r.get(k).and_then(J::as_str).unwrap_or_default().to_string();
            Runtime {
                identifier: s("identifier"),
                name: s("name"),
                version: s("version"),
                device_types: r
                    .get("supportedDeviceTypes")
                    .and_then(J::as_array)
                    .into_iter()
                    .flatten()
                    .map(|t| DeviceType {
                        name: t.get("name").and_then(J::as_str).unwrap_or_default().to_string(),
                        identifier: t.get("identifier").and_then(J::as_str).unwrap_or_default().to_string(),
                        family: t.get("productFamily").and_then(J::as_str).unwrap_or_default().to_string(),
                        bundle_path: t.get("bundlePath").and_then(J::as_str).unwrap_or_default().to_string(),
                    })
                    .collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
    Ok(out)
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Anything outside `[A-Za-z0-9-]` folded to one `-`, so a folded part
/// never contains `--`.
pub fn fold(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(c == '-' && o.ends_with('-')) {
            o.push(c);
        }
    }
    o.trim_matches('-').to_string()
}

/// `K2-<workspace>--<model>`. The `--` can't occur inside a folded part, so
/// the workspace reads back unambiguously (`sales` vs `sales-reviewer`).
pub fn device_name(workspace: &str, model: &str) -> String {
    format!("{DEVICE_PREFIX}{}--{}", fold(workspace), fold(model))
}

/// The folded workspace of a K2 device name; `None` for anything K2 didn't name.
pub fn workspace_of(name: &str) -> Option<&str> {
    name.strip_prefix(DEVICE_PREFIX)?.split_once("--").map(|(w, _)| w).filter(|w| !w.is_empty())
}

/// simctl, always against one device set.
#[derive(Clone)]
pub struct Simctl {
    set: PathBuf,
    runner: Arc<dyn Runner>,
}

const QUICK: Duration = Duration::from_secs(30);
const SLOW: Duration = Duration::from_secs(240);

impl Simctl {
    pub fn new(set: PathBuf, runner: Arc<dyn Runner>) -> Self {
        Self { set, runner }
    }

    pub fn set_path(&self) -> &Path {
        &self.set
    }

    pub fn runner(&self) -> &Arc<dyn Runner> {
        &self.runner
    }

    /// `simctl --set <set> <rest…>`: the only way this module builds a simctl call.
    pub fn args(&self, rest: &[&str]) -> Vec<String> {
        let mut a = vec!["simctl".to_string(), "--set".to_string(), self.set.to_string_lossy().into_owned()];
        a.extend(rest.iter().map(|s| s.to_string()));
        a
    }

    fn call(&self, rest: &[&str], timeout: Duration) -> SimResult<Output> {
        // simctl refuses a --set that doesn't exist yet (seen live on the
        // first probe); K2's own set folder is ours to create.
        let _ = std::fs::create_dir_all(&self.set);
        let out = self.runner.run("xcrun", &self.args(rest), timeout)?;
        if out.status != 0 {
            return Err(simctl_error(rest.first().copied().unwrap_or(""), &out));
        }
        Ok(out)
    }

    pub fn devices(&self) -> SimResult<Vec<SimDevice>> {
        parse_devices(&self.call(&["list", "-j", "devices"], QUICK)?.stdout)
    }

    pub fn ios_runtimes(&self) -> SimResult<Vec<Runtime>> {
        parse_ios_runtimes(&self.call(&["list", "-j", "runtimes"], QUICK)?.stdout)
    }

    /// Creates a device and returns its UDID.
    pub fn create(&self, name: &str, device_type: &str, runtime: &str) -> SimResult<String> {
        let out = self.call(&["create", name, device_type, runtime], QUICK)?;
        let udid = out.stdout.trim().to_string();
        if udid.len() != 36 {
            return Err(SimError::new("simctl_output", format!("simctl create printed {udid:?}, not a UDID")));
        }
        Ok(udid)
    }

    pub fn boot(&self, udid: &str) -> SimResult<()> {
        match self.call(&["boot", udid], SLOW) {
            // Already booted is fine (a re-adopted device).
            Err(e) if e.message.contains("current state: Booted") => Ok(()),
            r => r.map(|_| ()),
        }
    }

    /// Waits until the device has finished booting, at most `within`.
    pub fn bootstatus(&self, udid: &str, within: Duration) -> SimResult<()> {
        self.call(&["bootstatus", udid, "-b"], within).map(|_| ())
    }

    /// Whether an app is installed (its bundle container exists).
    pub fn has_app(&self, udid: &str, bundle_id: &str) -> SimResult<bool> {
        let out = self.runner.run("xcrun", &self.args(&["get_app_container", udid, bundle_id]), QUICK)?;
        Ok(out.status == 0)
    }

    /// Posts a Darwin notification inside the device (the Simulator's own
    /// Shake gesture is `com.apple.UIKit.SimulatorShake`).
    pub fn notify(&self, udid: &str, name: &str) -> SimResult<()> {
        self.call(&["spawn", udid, "notifyutil", "-p", name], QUICK).map(|_| ())
    }

    pub fn shutdown(&self, udid: &str) -> SimResult<()> {
        match self.call(&["shutdown", udid], SLOW) {
            Err(e) if e.message.contains("current state: Shutdown") => Ok(()),
            r => r.map(|_| ()),
        }
    }

    pub fn erase(&self, udid: &str) -> SimResult<()> {
        self.call(&["erase", udid], SLOW).map(|_| ())
    }

    pub fn delete(&self, udid: &str) -> SimResult<()> {
        self.call(&["delete", udid], SLOW).map(|_| ())
    }

    pub fn install(&self, udid: &str, app: &Path) -> SimResult<()> {
        let p = app.to_string_lossy();
        self.call(&["install", udid, &p], SLOW).map(|_| ())
    }

    /// Launches an app; returns its pid.
    pub fn launch(&self, udid: &str, bundle_id: &str) -> SimResult<u32> {
        let out = self.call(&["launch", udid, bundle_id], QUICK)?;
        // "dev.k2.simfixture: 7693"
        out.stdout
            .trim()
            .rsplit(": ")
            .next()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| SimError::new("simctl_output", format!("simctl launch printed {:?}", out.stdout.trim())))
    }

    pub fn terminate(&self, udid: &str, bundle_id: &str) -> SimResult<()> {
        self.call(&["terminate", udid, bundle_id], QUICK).map(|_| ())
    }

    pub fn open_url(&self, udid: &str, url: &str) -> SimResult<()> {
        self.call(&["openurl", udid, url], QUICK).map(|_| ())
    }

    pub fn location(&self, udid: &str, lat: f64, lon: f64) -> SimResult<()> {
        let at = format!("{lat},{lon}");
        self.call(&["location", udid, "set", &at], QUICK).map(|_| ())
    }

    /// Puts `text` on the device's pasteboard (`simctl pbcopy`, over stdin:
    /// the text is never on a command line).
    pub fn pbcopy(&self, udid: &str, text: &str) -> SimResult<()> {
        let _ = std::fs::create_dir_all(&self.set);
        let out = self.runner.run_input("xcrun", &self.args(&["pbcopy", udid]), text.as_bytes(), QUICK)?;
        if out.status != 0 {
            return Err(simctl_error("pbcopy", &out));
        }
        Ok(())
    }

    pub fn appearance(&self, udid: &str, dark: bool) -> SimResult<()> {
        self.call(&["ui", udid, "appearance", if dark { "dark" } else { "light" }], QUICK).map(|_| ())
    }

    /// `log show` inside the device, compact style.
    pub fn log_show(&self, udid: &str, last_secs: u64, predicate: Option<&str>) -> SimResult<String> {
        let last = format!("{}s", last_secs.max(1));
        let mut rest = vec!["spawn", udid, "log", "show", "--last", &last, "--style", "compact"];
        if let Some(p) = predicate {
            rest.push("--predicate");
            rest.push(p);
        }
        Ok(self.call(&rest, Duration::from_secs(60))?.stdout)
    }
}

/// simctl's failure text, turned into something an agent can act on.
pub fn simctl_error(verb: &str, out: &Output) -> SimError {
    let text = format!("{}{}", out.stderr.trim(), if out.stdout.trim().is_empty() { String::new() } else { format!(" {}", out.stdout.trim()) });
    let lower = text.to_lowercase();
    if verb == "install" && (lower.contains("ios-simulator") || lower.contains("built for ios") || lower.contains("incompatible") || lower.contains("platform")) {
        return SimError::new(
            "wrong_platform",
            format!("This .app isn't built for the iOS Simulator. Build it for the Simulator (xcodebuild -sdk iphonesimulator, or -destination 'generic/platform=iOS Simulator') and install that .app. simctl said: {text}"),
        );
    }
    if lower.contains("invalid device") || lower.contains("no devices are booted") {
        return SimError::new("no_device", format!("That device isn't in K2's device set any more. Run k2 sim boot. simctl said: {text}"));
    }
    if lower.contains("unable to boot") && lower.contains("current state") {
        return SimError::new("bad_state", format!("simctl {verb}: {text}"));
    }
    SimError::new("simctl_failed", format!("simctl {verb} failed: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every call; answers from a script keyed on the simctl verb.
    pub(crate) struct FakeRunner {
        pub calls: Mutex<Vec<(String, Vec<String>)>>,
        pub answer: Box<dyn Fn(&str, &[String]) -> SimResult<Output> + Send + Sync>,
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[String], _t: Duration) -> SimResult<Output> {
            self.calls.lock().unwrap().push((program.to_string(), args.to_vec()));
            (self.answer)(program, args)
        }

        /// Records stdin as a last `<stdin>…` argument (tests only).
        fn run_input(&self, program: &str, args: &[String], input: &[u8], _t: Duration) -> SimResult<Output> {
            let mut a = args.to_vec();
            a.push(format!("<stdin>{}", String::from_utf8_lossy(input)));
            self.calls.lock().unwrap().push((program.to_string(), a));
            (self.answer)(program, args)
        }
    }

    fn ok(s: &str) -> SimResult<Output> {
        Ok(Output { status: 0, stdout: s.to_string(), stderr: String::new() })
    }

    #[test]
    fn every_simctl_call_names_k2s_device_set_first() {
        let fake = Arc::new(FakeRunner {
            calls: Mutex::new(vec![]),
            answer: Box::new(|_, a| match a.get(3).map(String::as_str) {
                Some("list") => ok(r#"{"devices":{},"runtimes":[]}"#),
                Some("create") => ok("AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB\n"),
                Some("launch") => ok("dev.k2.app: 42\n"),
                Some("spawn") => ok("log line\n"),
                _ => ok(""),
            }),
        });
        let s = Simctl::new(PathBuf::from("/k2home/sim/devices"), fake.clone());
        let u = "AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB";
        s.devices().unwrap();
        s.ios_runtimes().unwrap();
        assert_eq!(s.create("K2-ws-iPhone-17", "t", "r").unwrap(), u);
        s.boot(u).unwrap();
        s.bootstatus(u, Duration::from_secs(5)).unwrap();
        s.has_app(u, "dev.k2.app").unwrap();
        s.notify(u, "com.apple.UIKit.SimulatorShake").unwrap();
        s.install(u, Path::new("/tmp/A.app")).unwrap();
        assert_eq!(s.launch(u, "dev.k2.app").unwrap(), 42);
        s.terminate(u, "dev.k2.app").unwrap();
        s.open_url(u, "https://k2.dev").unwrap();
        s.location(u, 1.5, -2.25).unwrap();
        s.appearance(u, true).unwrap();
        s.log_show(u, 120, Some("process == \"A\"")).unwrap();
        s.shutdown(u).unwrap();
        s.erase(u).unwrap();
        s.delete(u).unwrap();
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 17);
        for (prog, args) in calls.iter() {
            assert_eq!(prog, "xcrun");
            assert_eq!(&args[..3], &["simctl", "--set", "/k2home/sim/devices"], "call without K2's set: {args:?}");
        }
    }

    #[test]
    fn devices_parse_with_runtime_and_state() {
        let j = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-5":[{"udid":"U1","name":"K2-ws-iPhone-17","state":"Booted","isAvailable":true,"deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17"}],"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[]}}"#;
        let d = parse_devices(j).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].state, DeviceState::Booted);
        assert_eq!(d[0].runtime, "com.apple.CoreSimulator.SimRuntime.iOS-26-5");
        assert_eq!(d[0].device_type, "com.apple.CoreSimulator.SimDeviceType.iPhone-17");
        assert!(d[0].available);
        assert_eq!(parse_devices("{}").unwrap_err().code, "simctl_output");
    }

    #[test]
    fn runtimes_keep_available_ios_only_newest_first() {
        let j = r#"{"runtimes":[
          {"identifier":"r18","name":"iOS 18.6","version":"18.6","platform":"iOS","isAvailable":true,"supportedDeviceTypes":[{"name":"iPhone 16","identifier":"t16","productFamily":"iPhone"}]},
          {"identifier":"r26","name":"iOS 26.5","version":"26.5","platform":"iOS","isAvailable":true,"supportedDeviceTypes":[{"name":"iPhone 17","identifier":"t17","productFamily":"iPhone"}]},
          {"identifier":"rw","name":"watchOS 12","version":"12.0","platform":"watchOS","isAvailable":true,"supportedDeviceTypes":[]},
          {"identifier":"rx","name":"iOS 27.0","version":"27.0","platform":"iOS","isAvailable":false,"supportedDeviceTypes":[]}]}"#;
        let r = parse_ios_runtimes(j).unwrap();
        assert_eq!(r.iter().map(|r| r.identifier.as_str()).collect::<Vec<_>>(), ["r26", "r18"]);
        assert_eq!(r[0].device_types[0].name, "iPhone 17");
    }

    #[test]
    fn device_names_are_prefixed_folded_and_read_back() {
        assert_eq!(device_name("sales/reviewer", "iPhone 17 Pro"), "K2-sales-reviewer--iPhone-17-Pro");
        assert_eq!(device_name("  my ws!! ", "iPad (A16)"), "K2-my-ws--iPad-A16");
        assert_eq!(workspace_of(&device_name("sales", "iPhone 17")), Some("sales"));
        assert_eq!(workspace_of(&device_name("sales-reviewer", "iPhone 17")), Some("sales-reviewer"));
        assert_eq!(workspace_of("iPhone 17"), None);
        assert_eq!(workspace_of("K2-nodashes"), None);
    }

    #[test]
    fn failures_keep_simctls_words_and_teach() {
        let fake = Arc::new(FakeRunner {
            calls: Mutex::new(vec![]),
            answer: Box::new(|_, _| {
                Ok(Output { status: 149, stdout: String::new(), stderr: "An error was encountered processing the command (domain=IXErrorDomain, code=13): Failed to find matching arch for input file ... built for iOS".into() })
            }),
        });
        let s = Simctl::new(PathBuf::from("/s"), fake);
        let e = s.install("U", Path::new("/a/A.app")).unwrap_err();
        assert_eq!(e.code, "wrong_platform");
        assert!(e.message.contains("-sdk iphonesimulator"), "{}", e.message);
        assert!(e.message.contains("IXErrorDomain"), "{}", e.message);
    }

    #[test]
    fn boot_and_shutdown_accept_the_state_they_ask_for() {
        let fake = Arc::new(FakeRunner {
            calls: Mutex::new(vec![]),
            answer: Box::new(|_, a| {
                let msg = if a[3] == "boot" { "Unable to boot device in current state: Booted" } else { "Unable to shutdown device in current state: Shutdown" };
                Ok(Output { status: 149, stdout: String::new(), stderr: msg.into() })
            }),
        });
        let s = Simctl::new(PathBuf::from("/s"), fake);
        s.boot("U").unwrap();
        s.shutdown("U").unwrap();
    }

    #[test]
    fn the_real_runner_kills_at_the_timeout() {
        let t = Instant::now();
        let e = ProcessRunner.run("/bin/sleep", &["5".to_string()], Duration::from_millis(200)).unwrap_err();
        assert_eq!(e.code, "timeout");
        assert!(t.elapsed() < Duration::from_secs(3));
        let ok = ProcessRunner.run("/bin/echo", &["hi".to_string()], Duration::from_secs(5)).unwrap();
        assert_eq!((ok.status, ok.stdout.as_str()), (0, "hi\n"));
    }
}

#[cfg(test)]
pub(crate) use tests::FakeRunner;
