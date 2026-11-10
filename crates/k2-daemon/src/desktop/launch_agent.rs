//! macOS: the `dev.k2.deskd` LaunchAgent that starts K2 Screen (§3.2,
//! vs-live R25). `enable` writes the plist and bootstraps it into the
//! user's GUI domain; `disable` boots it out and removes the plist.
//!
//! **Why a LaunchAgent.** TCC credits a grant to the responsible process.
//! Started by launchd, deskd is its own; spawned by the daemon, the daemon
//! would be, and the daemon must hold no grant (every agent PTY it spawns
//! would inherit it).
//!
//! The plist:
//! - `LimitLoadToSessionType = Aqua`: never runs without a GUI session
//!   (bootstrap fails without one; the tab says `no_session`).
//! - `ProcessType = Interactive`, `KeepAlive = true`, `RunAtLoad = true`.
//! - `AssociatedBundleIdentifiers = [dev.k2.app]`: Login Items lists it
//!   under K2. macOS 13+ shows "Background Items Added" on first load, a
//!   free at-screen notice.
//!
//! Every `launchctl` call goes through a [`Runner`], so tests drive a fake
//! and never touch the real launchd.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use k2_deskd_proto::{APP_NAME, BINARY_NAME, LAUNCH_AGENT_LABEL};

/// The K2 app's bundle id (what Login Items groups K2 Screen under).
pub const APP_BUNDLE_ID: &str = "dev.k2.app";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a program and returns its exit status and output.
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output>;
}

/// The real one (windowless on Windows through `bg_proc`, plain here).
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
        let out = k2_core::bg_proc::command(program).args(args).stdin(std::process::Stdio::null()).output()?;
        Ok(Output {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// No GUI session for this user (a headless mini's service user, SSH
    /// only): launchd can't load an Aqua agent.
    NoSession(String),
    /// The person turned K2 Screen off under System Settings → General →
    /// Login Items (vs-live R25).
    BlockedInLoginItems,
    Failed(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::NoSession(m) => write!(f, "no GUI session: {m}"),
            AgentError::BlockedInLoginItems => write!(f, "K2 Screen is turned off in System Settings → General → Login Items"),
            AgentError::Failed(m) => write!(f, "{m}"),
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `<home>/Library/LaunchAgents/dev.k2.deskd.plist`.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library").join("LaunchAgents").join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

/// The K2 Screen executable for a daemon running at `daemon_exe`:
/// `K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon` →
/// `K2.app/Contents/Helpers/K2 Screen.app/Contents/MacOS/k2-deskd`.
/// `None` for any other layout (the standalone macOS daemon, a cargo
/// build): macOS v1 is bundled-app installs only (vs-live R7, `needs_app`).
pub fn screen_exe_for(daemon_exe: &Path) -> Option<PathBuf> {
    let macos = daemon_exe.parent()?;
    let contents = macos.parent()?;
    let daemon_app = contents.parent()?;
    let helpers = daemon_app.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" || daemon_app.extension()? != "app" || helpers.file_name()? != "Helpers" {
        return None;
    }
    Some(helpers.join(APP_NAME).join("Contents").join("MacOS").join(BINARY_NAME))
}

pub fn plist_xml(program: &Path, log: &Path) -> String {
    let p = xml_escape(&program.to_string_lossy());
    let log = xml_escape(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>Program</key>
    <string>{p}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{p}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>AssociatedBundleIdentifiers</key>
    <array>
        <string>{app}</string>
    </array>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        app = APP_BUNDLE_ID,
    )
}

/// The service target `gui/<uid>/dev.k2.deskd`.
pub fn service_target(uid: u32) -> String {
    format!("gui/{uid}/{LAUNCH_AGENT_LABEL}")
}

/// Does `launchctl print-disabled gui/<uid>` list the label as disabled?
pub fn listed_disabled(print_disabled: &str) -> bool {
    print_disabled.lines().any(|l| {
        let l = l.trim();
        l.starts_with(&format!("\"{LAUNCH_AGENT_LABEL}\"")) && (l.ends_with("disabled") || l.ends_with("true"))
    })
}

/// launchctl's way of saying the GUI domain isn't there.
pub fn says_no_session(out: &Output) -> bool {
    let t = format!("{} {}", out.stdout, out.stderr).to_ascii_lowercase();
    out.status == 125 || t.contains("domain does not support") || t.contains("could not find domain") || t.contains("no such domain")
}

pub struct Agent {
    pub home: PathBuf,
    pub uid: u32,
    pub program: PathBuf,
    pub runner: Arc<dyn Runner>,
}

impl Agent {
    fn launchctl(&self, args: &[&str]) -> Result<Output, AgentError> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.runner.run("/bin/launchctl", &args).map_err(|e| AgentError::Failed(format!("launchctl: {e}")))
    }

    pub fn plist(&self) -> PathBuf {
        plist_path(&self.home)
    }

    /// Writes the plist (0644, atomically) and loads it. Loading an agent
    /// that is already loaded boots the old one out first, so a new
    /// `Program` path (after an app move) takes effect.
    pub fn install(&self) -> Result<(), AgentError> {
        let path = self.plist();
        let dir = path.parent().expect("plist has a folder");
        std::fs::create_dir_all(dir).map_err(|e| AgentError::Failed(format!("{}: {e}", dir.display())))?;
        let log = self.home.join(".k2").join("logs").join("deskd.log");
        if let Some(d) = log.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = dir.join(format!(".{LAUNCH_AGENT_LABEL}.plist.{}.tmp", std::process::id()));
        std::fs::write(&tmp, plist_xml(&self.program, &log)).map_err(|e| AgentError::Failed(format!("{}: {e}", tmp.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
        }
        std::fs::rename(&tmp, &path).map_err(|e| AgentError::Failed(format!("{}: {e}", path.display())))?;

        let domain = format!("gui/{}", self.uid);
        let target = service_target(self.uid);
        if self.launchctl(&["print", &target])?.status == 0 {
            let _ = self.launchctl(&["bootout", &target]);
        }
        // A plist turned off with `launchctl disable` (or by K2 before) is
        // turned back on; one blocked in Login Items stays blocked.
        let _ = self.launchctl(&["enable", &target]);
        let path_s = path.to_string_lossy().into_owned();
        let out = self.launchctl(&["bootstrap", &domain, &path_s])?;
        if out.status == 0 {
            return Ok(());
        }
        if says_no_session(&out) {
            return Err(AgentError::NoSession(out.stderr.trim().to_string()));
        }
        let disabled = self.launchctl(&["print-disabled", &domain]).map(|o| listed_disabled(&o.stdout)).unwrap_or(false);
        if disabled || out.stderr.to_ascii_lowercase().contains("disabled") {
            return Err(AgentError::BlockedInLoginItems);
        }
        Err(AgentError::Failed(format!("launchctl bootstrap exited {}: {}", out.status, out.stderr.trim())))
    }

    /// Boots the agent out (deskd gets SIGTERM and releases what it holds)
    /// and removes the plist. Not loaded is fine.
    pub fn bootout(&self) -> Result<(), AgentError> {
        let target = service_target(self.uid);
        let out = self.launchctl(&["bootout", &target])?;
        // 3 / 113: no such process / not loaded.
        let gone = out.status == 0 || out.status == 3 || out.status == 113 || out.stderr.contains("No such process") || out.stderr.contains("Could not find service");
        match std::fs::remove_file(self.plist()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(AgentError::Failed(format!("{}: {e}", self.plist().display()))),
        }
        if gone {
            Ok(())
        } else {
            Err(AgentError::Failed(format!("launchctl bootout exited {}: {}", out.status, out.stderr.trim())))
        }
    }

    /// Restarts deskd (`kickstart -k`: SIGTERM, launchd starts it again).
    /// Used after a protocol mismatch (an update replaced the bundle while
    /// the old deskd ran).
    pub fn kickstart(&self) -> Result<(), AgentError> {
        let out = self.launchctl(&["kickstart", "-k", &service_target(self.uid)])?;
        if out.status == 0 {
            Ok(())
        } else {
            Err(AgentError::Failed(format!("launchctl kickstart exited {}: {}", out.status, out.stderr.trim())))
        }
    }

    pub fn loaded(&self) -> bool {
        self.launchctl(&["print", &service_target(self.uid)]).map(|o| o.status == 0).unwrap_or(false)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A fake launchctl: records calls, answers from a table keyed by the
    /// first argument, and keeps a loaded flag.
    #[derive(Default)]
    pub struct FakeLaunchctl {
        pub calls: Mutex<Vec<Vec<String>>>,
        pub loaded: Mutex<bool>,
        pub answers: Mutex<HashMap<String, Output>>,
    }

    fn ok() -> Output {
        Output { status: 0, stdout: String::new(), stderr: String::new() }
    }

    impl Runner for FakeLaunchctl {
        fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
            assert_eq!(program, "/bin/launchctl", "only launchctl is run");
            self.calls.lock().unwrap().push(args.to_vec());
            if let Some(a) = self.answers.lock().unwrap().get(&args[0]) {
                return Ok(a.clone());
            }
            let mut loaded = self.loaded.lock().unwrap();
            Ok(match args[0].as_str() {
                "print" if *loaded => ok(),
                "print" => Output { status: 113, stdout: String::new(), stderr: "Could not find service".into() },
                "bootstrap" => {
                    *loaded = true;
                    ok()
                }
                "bootout" if *loaded => {
                    *loaded = false;
                    ok()
                }
                "bootout" => Output { status: 3, stdout: String::new(), stderr: "No such process".into() },
                _ => ok(),
            })
        }
    }

    fn tmp_home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-deskd-la-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn agent(home: &Path, fake: Arc<FakeLaunchctl>) -> Agent {
        Agent { home: home.to_path_buf(), uid: 501, program: PathBuf::from("/Applications/K2.app/Contents/Helpers/K2 Screen.app/Contents/MacOS/k2-deskd"), runner: fake }
    }

    #[test]
    fn screen_exe_sits_beside_the_daemon_helper_app() {
        let d = Path::new("/Applications/K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon");
        assert_eq!(screen_exe_for(d).unwrap(), PathBuf::from("/Applications/K2.app/Contents/Helpers/K2 Screen.app/Contents/MacOS/k2-deskd"));
        assert_eq!(screen_exe_for(Path::new("/usr/local/bin/k2-daemon")), None);
        assert_eq!(screen_exe_for(Path::new("/x/target/debug/k2-daemon")), None);
    }

    #[test]
    fn plist_has_the_r25_keys_and_escapes_the_path() {
        let x = plist_xml(Path::new("/A&B/K2 Screen.app/Contents/MacOS/k2-deskd"), Path::new("/h/.k2/logs/deskd.log"));
        for want in [
            "<string>dev.k2.deskd</string>",
            "<key>LimitLoadToSessionType</key>\n    <string>Aqua</string>",
            "<key>ProcessType</key>\n    <string>Interactive</string>",
            "<key>KeepAlive</key>\n    <true/>",
            "<string>dev.k2.app</string>",
            "/A&amp;B/K2 Screen.app/Contents/MacOS/k2-deskd",
        ] {
            assert!(x.contains(want), "missing {want:?} in\n{x}");
        }
        assert!(!x.contains("/A&B"));
    }

    #[test]
    fn install_writes_the_plist_and_bootstraps_into_the_gui_domain() {
        let home = tmp_home("i");
        let fake = Arc::new(FakeLaunchctl::default());
        let a = agent(&home, fake.clone());
        a.install().unwrap();
        assert!(a.plist().is_file());
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.contains(&vec!["bootstrap".into(), "gui/501".into(), a.plist().to_string_lossy().into_owned()]), "{calls:?}");
        assert!(a.loaded());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn install_over_a_loaded_agent_boots_it_out_first() {
        let home = tmp_home("r");
        let fake = Arc::new(FakeLaunchctl::default());
        *fake.loaded.lock().unwrap() = true;
        let a = agent(&home, fake.clone());
        a.install().unwrap();
        let calls: Vec<String> = fake.calls.lock().unwrap().iter().map(|c| c[0].clone()).collect();
        let bo = calls.iter().position(|c| c == "bootout").expect("bootout");
        let bs = calls.iter().position(|c| c == "bootstrap").expect("bootstrap");
        assert!(bo < bs, "{calls:?}");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn disable_boots_out_and_the_plist_goes_away() {
        let home = tmp_home("d");
        let fake = Arc::new(FakeLaunchctl::default());
        let a = agent(&home, fake.clone());
        a.install().unwrap();
        a.bootout().unwrap();
        assert!(!a.plist().exists());
        assert!(!a.loaded());
        // Again: not loaded and no plist is still fine.
        a.bootout().unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn no_gui_session_is_its_own_error() {
        let home = tmp_home("n");
        let fake = Arc::new(FakeLaunchctl::default());
        fake.answers.lock().unwrap().insert("bootstrap".into(), Output { status: 125, stdout: String::new(), stderr: "Bootstrap failed: 125: Domain does not support specified action".into() });
        let a = agent(&home, fake);
        assert!(matches!(a.install().unwrap_err(), AgentError::NoSession(_)));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn blocked_in_login_items_is_its_own_error() {
        let home = tmp_home("b");
        let fake = Arc::new(FakeLaunchctl::default());
        fake.answers.lock().unwrap().insert("bootstrap".into(), Output { status: 5, stdout: String::new(), stderr: "Bootstrap failed: 5: Input/output error".into() });
        fake.answers.lock().unwrap().insert("print-disabled".into(), Output { status: 0, stdout: "disabled services = {\n\t\"dev.k2.deskd\" => disabled\n}\n".into(), stderr: String::new() });
        let a = agent(&home, fake);
        assert_eq!(a.install().unwrap_err(), AgentError::BlockedInLoginItems);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn print_disabled_parsing() {
        assert!(listed_disabled("\t\"dev.k2.deskd\" => disabled\n"));
        assert!(listed_disabled("\t\"dev.k2.deskd\" => true\n"));
        assert!(!listed_disabled("\t\"dev.k2.deskd\" => enabled\n"));
        assert!(!listed_disabled("\t\"dev.k2.deskdx\" => disabled\n"));
    }
}
