//! The virtual desktop manager with a fake runner and a fake clock (L3's
//! "fails first" list), and, behind `xvfb-tests`, against a real Xvfb.

use super::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};

const GB: u64 = 1 << 30;

/// Everything the fake host and screen did, in order.
pub(crate) type Log = Arc<Mutex<Vec<String>>>;

pub(crate) struct Fake {
    pub(crate) log: Log,
    pub(crate) facts: Mutex<BoxFacts>,
    pub(crate) boot: Mutex<String>,
    pub(crate) programs: Mutex<Vec<&'static str>>,
    pub(crate) systemd: bool,
    pub(crate) freezer: bool,
    pub(crate) fail_freeze: Mutex<bool>,
    /// Displays in use (a lock or a socket in /tmp).
    pub(crate) taken: Mutex<HashSet<u32>>,
    /// Displays another X server grabs first: Xvfb on one exits at once.
    pub(crate) race: Mutex<HashSet<u32>>,
    pub(crate) next_pid: Mutex<u32>,
    pub(crate) dead: Mutex<HashSet<String>>,
    pub(crate) frozen: Mutex<HashSet<String>>,
    pub(crate) alive: Mutex<HashSet<u32>>,
    /// `Some(why)`: /tmp/.X11-unix is missing or wrong.
    pub(crate) x11_dir: Mutex<Option<String>>,
    /// Where at-spi-bus-launcher is (None: not installed).
    pub(crate) atspi: Mutex<Option<String>>,
}

impl Fake {
    pub(crate) fn new(systemd: bool, freezer: bool) -> Arc<Fake> {
        Arc::new(Fake {
            log: Arc::new(Mutex::new(Vec::new())),
            facts: Mutex::new(BoxFacts { mem_total: 64 * GB, mem_available: 32 * GB, cores: 16 }),
            boot: Mutex::new("boot-1".into()),
            programs: Mutex::new(vec!["Xvfb", "icewm-session", "dbus-run-session", "apt-get"]),
            systemd,
            freezer,
            fail_freeze: Mutex::new(false),
            taken: Mutex::new(HashSet::new()),
            race: Mutex::new(HashSet::new()),
            next_pid: Mutex::new(4000),
            dead: Mutex::new(HashSet::new()),
            frozen: Mutex::new(HashSet::new()),
            alive: Mutex::new(HashSet::new()),
            x11_dir: Mutex::new(None),
            atspi: Mutex::new(None),
        })
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.log.lock().unwrap().clear();
    }

    pub(crate) fn set_facts(&self, gb: u64, available: u64, cores: u32) {
        *self.facts.lock().unwrap() = BoxFacts { mem_total: gb * GB, mem_available: available, cores };
    }

    /// What Xvfb does with its explicit `:<n>`: writes it to the file `sh
    /// -c` hands its fd 3 to, or (a display grabbed in a race) exits.
    /// `false`: it exited.
    fn write_display(&self, argv: &[String]) -> bool {
        let x = argv.iter().position(|a| a == "Xvfb").expect("Xvfb");
        let n: u32 = argv[x + 1].strip_prefix(':').expect("an explicit display").parse().expect("a number");
        if self.race.lock().unwrap().contains(&n) {
            return false;
        }
        let i = argv.iter().position(|a| a == "k2-desktop").expect("the displayfd shell");
        std::fs::write(&argv[i + 1], format!("{n}\n")).expect("display file");
        self.taken.lock().unwrap().insert(n);
        true
    }
}

fn ok(stdout: &str) -> io::Result<Output> {
    Ok(Output { status: 0, stdout: stdout.into(), stderr: String::new() })
}

fn failed(stderr: &str) -> io::Result<Output> {
    Ok(Output { status: 1, stdout: String::new(), stderr: stderr.into() })
}

impl Host for Fake {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
        self.log.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        match program {
            "systemd-run" => {
                let unit = args.iter().find_map(|a| a.strip_prefix("--unit=")).expect("a unit").to_string();
                self.dead.lock().unwrap().remove(&unit);
                if unit.ends_with("-x.service") && !self.write_display(args) {
                    self.dead.lock().unwrap().insert(unit);
                }
                ok("")
            }
            "systemctl" => {
                assert_eq!(args[0], "--user", "always the user's own manager");
                let verb = args[1].as_str();
                let rest = &args[2..];
                match verb {
                    "freeze" => {
                        if *self.fail_freeze.lock().unwrap() {
                            return failed("Unit does not support freezing");
                        }
                        self.frozen.lock().unwrap().insert(rest[0].clone());
                        ok("")
                    }
                    "thaw" => {
                        self.frozen.lock().unwrap().remove(&rest[0]);
                        ok("")
                    }
                    "is-active" => {
                        let dead = self.dead.lock().unwrap();
                        let any_dead = rest.iter().filter(|u| !u.starts_with("--")).any(|u| dead.contains(u));
                        if any_dead {
                            failed("")
                        } else {
                            ok("")
                        }
                    }
                    "show" if rest.contains(&"FreezerState".to_string()) => {
                        let u = rest.last().unwrap();
                        ok(if self.frozen.lock().unwrap().contains(u) { "frozen\n" } else { "running\n" })
                    }
                    "show" if rest.contains(&"ControlGroup".to_string()) => {
                        ok(&format!("/user.slice/user-1000.slice/user@1000.service/k2.slice/k2-desktops.slice/{}\n", rest.last().unwrap()))
                    }
                    "show" => ok("73400320\n"),
                    "stop" => {
                        // Stopping the slice ends both services.
                        let slug = rest[0].trim_start_matches("k2-desktops-").trim_end_matches(".slice").to_string();
                        let mut dead = self.dead.lock().unwrap();
                        dead.insert(format!("k2-desktop-{slug}-x.service"));
                        dead.insert(format!("k2-desktop-{slug}-session.service"));
                        ok("")
                    }
                    "set-property" => ok(""),
                    other => panic!("unexpected systemctl {other}"),
                }
            }
            "journalctl" => ok("(EE) Fatal server error: boom\n"),
            other => panic!("unexpected program {other}"),
        }
    }

    fn spawn_detached(&self, argv: &[String]) -> io::Result<u32> {
        self.log.lock().unwrap().push(format!("spawn {}", argv.join(" ")));
        let up = !argv.iter().any(|a| a == "Xvfb") || self.write_display(argv);
        let mut p = self.next_pid.lock().unwrap();
        *p += 1;
        if up {
            self.alive.lock().unwrap().insert(*p);
        }
        Ok(*p)
    }

    fn alive(&self, pid: u32) -> bool {
        self.alive.lock().unwrap().contains(&pid)
    }

    fn kill_group(&self, pid: u32) {
        self.log.lock().unwrap().push(format!("kill {pid}"));
        self.alive.lock().unwrap().remove(&pid);
    }

    fn facts(&self) -> BoxFacts {
        *self.facts.lock().unwrap()
    }

    fn boot_id(&self) -> String {
        self.boot.lock().unwrap().clone()
    }

    fn has_program(&self, name: &str) -> bool {
        self.programs.lock().unwrap().contains(&name)
    }

    fn package_manager(&self) -> Option<&'static str> {
        self.programs.lock().unwrap().contains(&"apt-get").then_some("apt")
    }

    fn systemd_user(&self) -> bool {
        self.systemd
    }

    fn freezer(&self) -> bool {
        self.freezer
    }

    fn x11_socket_dir(&self) -> Result<(), String> {
        match &*self.x11_dir.lock().unwrap() {
            None => Ok(()),
            Some(why) => Err(why.clone()),
        }
    }

    fn display_taken(&self, n: u32) -> bool {
        self.taken.lock().unwrap().contains(&n)
    }

    fn reclaim(&self, cgroup: &str, bytes: u64) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("reclaim {cgroup} {bytes}"));
        Ok(())
    }

    fn atspi_launcher(&self) -> Option<String> {
        self.atspi.lock().unwrap().clone()
    }
}

pub(crate) struct FakeClock(pub(crate) AtomicI64);

impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

pub(crate) struct FakeScreen(pub(crate) Log);

impl Screen for FakeScreen {
    fn release_keys(&self, desktop: &str) {
        self.0.lock().unwrap().push(format!("screen release_keys {desktop}"));
    }
    fn close(&self, desktop: &str, state: State) {
        self.0.lock().unwrap().push(format!("screen close {desktop} {}", state.as_wire()));
    }
}

pub(crate) struct Rig {
    pub(crate) host: Arc<Fake>,
    pub(crate) clock: Arc<FakeClock>,
    pub(crate) m: Arc<Manager>,
    pub(crate) home: PathBuf,
}

impl Rig {
    pub(crate) fn new(systemd: bool, freezer: bool) -> Rig {
        let host = Fake::new(systemd, freezer);
        Rig::with(host, None)
    }

    fn with(host: Arc<Fake>, home: Option<PathBuf>) -> Rig {
        let home = home.unwrap_or_else(|| {
            let d = std::env::temp_dir().join(format!("k2-vdesktop-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&d).unwrap();
            d
        });
        let clock = Arc::new(FakeClock(AtomicI64::new(1_000_000)));
        let mut cfg = Config::for_home(&home);
        cfg.display_wait = Duration::from_secs(2);
        let m = Manager::new(host.clone(), clock.clone(), cfg);
        m.set_screen(Arc::new(FakeScreen(host.log.clone())));
        Rig { host, clock, m, home }
    }

    /// A second daemon on the same box (a restart).
    fn restarted(&self) -> Rig {
        Rig::with(self.host.clone(), Some(self.home.clone()))
    }

    pub(crate) fn advance(&self, secs: i64) {
        self.clock.0.fetch_add(secs, Ordering::SeqCst);
    }

    pub(crate) fn runs(&self, prefix: &str) -> Vec<String> {
        self.host.lines().into_iter().filter(|l| l.starts_with(prefix)).collect()
    }

    fn record(&self, id: &str) -> Record {
        let p = self.home.join(".k2").join("desktop").join(vd::folder_slug(id)).join("state.json");
        serde_json::from_slice(&std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).expect("state.json")
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn size(w: u32, h: u32) -> Option<Size> {
    Some(Size { width: w, height: h })
}

#[test]
fn start_runs_xvfb_then_the_session_with_a_clean_env_and_never_ac() {
    let r = Rig::new(true, true);
    assert_eq!(r.m.start("virtual", Asker::Person, size(1600, 900)), Ok(State::Ready));
    let runs = r.runs("systemd-run");
    assert_eq!(runs.len(), 2, "{runs:?}");
    let x = &runs[0];
    for want in [
        "--user",
        "--unit=k2-desktop-virtual-x.service",
        "--slice=k2-desktops-virtual.slice",
        "--property=TasksMax=512",
        "-- /usr/bin/env -i ",
        "K2_NO_OWNER_FALLBACK=1",
        "Xvfb :100 -displayfd 3 -screen 0 1600x900x24 -nolisten tcp -nolisten local -extension GLX -auth ",
    ] {
        assert!(x.contains(want), "{want} in {x}");
    }
    assert!(!x.split(' ').any(|a| a == "-ac"), "never -ac: {x}");
    for line in [&runs[0], &runs[1]] {
        let k2_vars: Vec<&str> = line.split(' ').filter(|a| a.starts_with("K2_")).collect();
        assert_eq!(k2_vars, vec!["K2_NO_OWNER_FALLBACK=1"], "only the guard: {line}");
    }
    let session = &runs[1];
    for want in ["--unit=k2-desktop-virtual-session.service", "--slice=k2-desktops-virtual.slice", "DISPLAY=:100", "dbus-run-session -- icewm-session"] {
        assert!(session.contains(want), "{want} in {session}");
    }
    // Xvfb first, then the session, then the limits.
    let all = r.host.lines();
    let pos = |p: &str| all.iter().position(|l| l.contains(p)).unwrap_or_else(|| panic!("{p} in {all:?}"));
    assert!(pos("-x.service") < pos("-session.service"));
    assert!(pos("-session.service") < pos("set-property --runtime k2-desktops-virtual.slice MemoryMax=2147483648 TasksMax=512 CPUQuota=200%"));
    assert!(all.iter().any(|l| l == "systemctl --user set-property --runtime k2-desktops.slice MemoryMax=17179869184 CPUQuota=800%"), "{all:?}");
    // The cookie: 0600, one wild MIT-MAGIC-COOKIE-1.
    let cookie = r.home.join(".k2/desktop/virtual/Xauthority");
    let bytes = std::fs::read(&cookie).expect("cookie");
    assert_eq!(bytes.len(), 44);
    assert_eq!(&bytes[8..26], b"MIT-MAGIC-COOKIE-1");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&cookie).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(r.home.join(".k2/desktop/virtual")).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let rec = r.record("virtual");
    assert_eq!((rec.state, rec.display.as_deref(), rec.size), (State::Ready, Some(":100"), size(1600, 900)));
    assert_eq!(rec.boot_id, "boot-1");
}

/// L8 (§3.9): agent-desktop reads the accessibility tree over AT-SPI, so
/// the session starts at-spi-bus-launcher on the desktop's own bus when it
/// is installed (it publishes the bus on the display), and not otherwise.
#[test]
fn the_session_starts_the_accessibility_bus_when_installed() {
    let r = Rig::new(true, true);
    *r.host.atspi.lock().unwrap() = Some("/usr/libexec/at-spi-bus-launcher".into());
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    let session = r.runs("systemd-run").into_iter().find(|l| l.contains("-session.service")).expect("the session");
    assert!(
        session.contains(&format!("dbus-run-session -- /bin/sh -c {ATSPI_SH} k2-session /usr/libexec/at-spi-bus-launcher icewm-session")),
        "{session}"
    );
    let r = Rig::new(true, true);
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    let session = r.runs("systemd-run").into_iter().find(|l| l.contains("-session.service")).expect("the session");
    assert!(session.ends_with("dbus-run-session -- icewm-session"), "no launcher, no wrapper: {session}");
}

/// LD-31: a browser profile folder per desktop, 0700, never shared.
#[test]
fn each_desktop_has_its_own_profile_folder() {
    let r = Rig::new(true, true);
    let a = r.m.profile_dir("virtual:w1").expect("made");
    let b = r.m.profile_dir("virtual:w2").expect("made");
    assert_ne!(a, b);
    assert_eq!(a, r.home.join(".k2/desktop/virtual-w1/profile"));
    for p in [&a, &b] {
        let m = std::fs::symlink_metadata(p).unwrap();
        assert!(m.is_dir() && !m.file_type().is_symlink(), "{}", p.display());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(m.permissions().mode() & 0o777, 0o700, "{}", p.display());
        }
    }
    // Kept across a close (an agent's logins in it stay its own).
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    std::fs::write(a.join("Cookies"), b"x").unwrap();
    r.m.close_with("virtual:w1", CloseReason::ClosedBy { by: "test".into() });
    assert!(a.join("Cookies").is_file());
    assert_eq!(r.m.profile_dir("virtual:w1").unwrap(), a);
}

#[test]
fn a_new_cookie_each_start() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    let first = std::fs::read(r.home.join(".k2/desktop/virtual/Xauthority")).unwrap();
    r.m.close_with("virtual", CloseReason::ClosedBy { by: "rosson".into() });
    assert!(!r.home.join(".k2/desktop/virtual/Xauthority").exists(), "a closed desktop's cookie goes");
    r.m.start("virtual", Asker::Person, None).unwrap();
    let second = std::fs::read(r.home.join(".k2/desktop/virtual/Xauthority")).unwrap();
    assert_ne!(first, second);
}

#[test]
fn the_size_is_clamped() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, size(800, 600)).unwrap();
    assert!(r.runs("systemd-run")[0].contains("1280x720x24"));
    assert_eq!(r.m.status("virtual").size, size(1280, 720));
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    assert_eq!(r.m.status("virtual:w1").size, size(1280, 720), "no pane: 1280×720");
    r.m.start("virtual:w2", Asker::Person, size(4000, 3000)).unwrap();
    assert_eq!(r.m.status("virtual:w2").size, size(1920, 1080));
}

#[test]
fn two_ids_get_two_folders_slices_and_displays() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.start("virtual:9c1e-44", Asker::Agent, None).unwrap();
    let (a, b) = (r.record("virtual"), r.record("virtual:9c1e-44"));
    assert_eq!(a.display.as_deref(), Some(":100"));
    assert_eq!(b.display.as_deref(), Some(":101"));
    assert_ne!(a.units.slice, b.units.slice);
    assert!(r.home.join(".k2/desktop/virtual").is_dir() && r.home.join(".k2/desktop/virtual-9c1e-44").is_dir());
    assert!(r.runs("systemd-run").iter().any(|l| l.contains("--slice=k2-desktops-virtual:9c1e\\x2d44.slice")));
    assert_eq!(r.m.screen_env("virtual:9c1e-44").unwrap()[0], ("DISPLAY".to_string(), ":101".to_string()));
}

#[test]
fn status_shows_the_limits_for_this_box() {
    let r = Rig::new(true, true);
    r.host.set_facts(8, 4 * GB, 4);
    r.m.start("virtual", Asker::Person, None).unwrap();
    let st = r.m.status("virtual");
    let j = st.virtual_json(None);
    assert_eq!(j["limits"]["desktops"], json!({"max": 4, "used": 1}));
    assert_eq!(j["limits"]["awake"], json!({"max": 2, "used": 1}));
    assert_eq!(j["limits"]["memoryBytes"], json!({"max": 2 * GB, "each": 2 * GB}));
    assert_eq!(j["limits"]["cpuCores"], 2);
    assert_eq!(j["memoryBytes"], 73400320);
    assert_eq!(j["canSleep"], true);
    assert_eq!(st.wire_state(), "ready");
}

#[test]
fn a_desktop_past_the_exist_limit_is_refused_with_the_list() {
    let r = Rig::new(true, true);
    r.host.set_facts(4, 2 * GB, 8);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.start("virtual:a", Asker::Agent, None).unwrap();
    r.host.clear();
    let e = r.m.start("virtual:b", Asker::Agent, None).unwrap_err();
    assert_eq!(e, StartError::TooMany { closable: vec!["virtual".into(), "virtual:a".into()] });
    assert_eq!(e.code(), "too_many_desktops");
    assert!(r.runs("systemd-run").is_empty(), "nothing started");
    assert!(!r.m.known().iter().any(|(id, _)| id == "virtual:b"), "a refused first start leaves nothing behind");
}

#[test]
fn a_start_past_the_awake_limit_waits_and_goes_when_a_slot_frees_people_first() {
    let r = Rig::new(true, true);
    r.host.set_facts(8, 4 * GB, 2); // one awake at a time
    assert_eq!(r.m.start("virtual:a", Asker::Agent, None), Ok(State::Ready));
    assert_eq!(r.m.start("virtual:b", Asker::Agent, None), Ok(State::Waiting));
    assert_eq!(r.m.start("virtual", Asker::Person, size(1600, 900)), Ok(State::Waiting));
    assert_eq!(r.m.status("virtual").waiting, Some(1), "the person first");
    assert_eq!(r.m.status("virtual:b").waiting, Some(2));
    assert_eq!(r.m.status("virtual:b").wire_state(), "waiting");
    assert!(r.m.status("virtual").message().contains("runs 1 at a time"));
    r.host.clear();
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    assert_eq!(r.m.state("virtual:a"), State::Asleep);
    assert_eq!(r.m.state("virtual"), State::Ready, "the person's desktop took the slot");
    assert!(r.runs("systemd-run")[0].contains("1600x900x24"), "at the size it asked for");
    assert_eq!(r.m.state("virtual:b"), State::Waiting);
    assert_eq!(r.m.status("virtual:b").waiting, Some(1));
}

#[test]
fn sleep_releases_keys_then_closes_the_link_then_freezes() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.host.clear();
    r.advance(vd::SLEEP_AFTER - 1);
    r.m.tick();
    assert_eq!(r.m.state("virtual"), State::Ready);
    r.advance(1);
    r.m.tick();
    let acts: Vec<String> = r.host.lines().into_iter().filter(|l| !l.contains("is-active")).collect();
    assert_eq!(
        acts,
        vec![
            "screen release_keys virtual".to_string(),
            "screen close virtual asleep".to_string(),
            "systemctl --user freeze k2-desktop-virtual-session.service".to_string(),
            "systemctl --user freeze k2-desktop-virtual-x.service".to_string(),
            "systemctl --user show -p ControlGroup --value k2-desktops-virtual.slice".to_string(),
            "reclaim /user.slice/user-1000.slice/user@1000.service/k2.slice/k2-desktops.slice/k2-desktops-virtual.slice 2147483648".to_string(),
        ]
    );
    assert_eq!(r.m.state("virtual"), State::Asleep);
    let rec = r.record("virtual");
    assert_eq!((rec.state, rec.asleep_since), (State::Asleep, Some(r.m.now())));
    assert!(r.m.screen_env("virtual").is_none(), "K2 Screen doesn't attach to an asleep desktop");
    assert_eq!(r.m.status("virtual").next_close_at, Some(r.m.now() + vd::CLOSE_ASLEEP_AFTER));
}

#[test]
fn a_present_viewer_keeps_it_awake_and_a_hidden_one_does_not() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.viewer_joined("virtual", "dv1", "rosson", true);
    for _ in 0..40 {
        r.advance(30);
        r.m.viewer_present("virtual", "dv1");
        r.m.tick();
    }
    assert_eq!(r.m.state("virtual"), State::Ready, "20 minutes in view");
    assert_eq!(r.m.status("virtual").present, vec![("dv1".to_string(), "rosson".to_string())]);
    r.m.viewer_hidden("virtual", "dv1", true);
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    assert_eq!(r.m.state("virtual"), State::Asleep);
}

#[test]
fn a_start_of_an_asleep_desktop_wakes_it_by_thawing() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    r.host.clear();
    assert_eq!(r.m.start("virtual", Asker::Person, size(1920, 1080)), Ok(State::Ready));
    assert_eq!(r.host.lines(), vec!["systemctl --user thaw k2-desktop-virtual-x.service".to_string(), "systemctl --user thaw k2-desktop-virtual-session.service".to_string()]);
    assert_eq!(r.m.status("virtual").size, size(1280, 720), "waking keeps the size");
    assert_eq!(r.record("virtual").state, State::Ready);
}

#[test]
fn a_repeated_start_returns_the_running_desktop() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    assert_eq!(r.m.start("virtual", Asker::Person, size(1920, 1080)), Ok(State::Ready));
    assert_eq!(r.m.start("virtual", Asker::Agent, None), Ok(State::Ready));
    assert_eq!(r.runs("systemd-run").len(), 2, "started once");
    assert_eq!(r.record("virtual").display.as_deref(), Some(":100"));
}

#[test]
fn a_start_under_the_memory_floor_is_refused() {
    let r = Rig::new(true, true);
    r.host.set_facts(8, vd::MEMORY_FLOOR - 1, 4);
    let e = r.m.start("virtual", Asker::Person, None).unwrap_err();
    assert_eq!(e, StartError::LowMemory);
    assert_eq!((e.status(), e.code()), (503, "low_memory"));
    assert!(r.runs("systemd-run").is_empty());
}

#[test]
fn a_server_missing_xvfb_is_not_set_up_and_says_what_to_run() {
    let r = Rig::new(true, true);
    r.host.programs.lock().unwrap().retain(|p| *p != "Xvfb");
    let e = r.m.start("virtual", Asker::Person, None).unwrap_err();
    assert_eq!(e, StartError::NotSetUp { missing: vec!["Xvfb"], manual: Some("sudo apt-get update && sudo apt-get install --no-install-recommends xvfb icewm fonts-dejavu-core".into()) });
    let st = r.m.status("virtual");
    assert_eq!(st.wire_state(), "not_set_up");
    assert_eq!(st.message(), "This server has no screen. K2 can start a desktop on it once three small packages are installed.");
    assert_eq!(st.virtual_json(None)["setupCard"], vd::SETUP_CARD, "the size goes on the Set up card");
    assert!(vd::SETUP_CARD.contains("about 300 MB") && vd::SETUP_CARD.contains("about 70 MB"));
    assert_eq!(st.virtual_json(None)["hasBrowser"], false);
    r.host.programs.lock().unwrap().push("firefox");
    assert_eq!(r.m.status("virtual").virtual_json(None)["hasBrowser"], true, "a browser on the box");
    assert_eq!(st.virtual_json(None)["manual"], "sudo apt-get update && sudo apt-get install --no-install-recommends xvfb icewm fonts-dejavu-core");
}

#[test]
fn no_x11_socket_folder_is_refused_with_why() {
    let r = Rig::new(true, true);
    *r.host.x11_dir.lock().unwrap() = Some("/tmp/.X11-unix doesn't exist".into());
    let e = r.m.start("virtual", Asker::Person, None).unwrap_err();
    assert_eq!(e, StartError::NoSocketDir("/tmp/.X11-unix doesn't exist".into()));
    assert_eq!((e.status(), e.code()), (409, "no_x11_socket_dir"));
    assert!(e.hint(&r.m.limits()).contains("install -d -m 1777 /tmp/.X11-unix"));
    assert!(r.runs("systemd-run").is_empty(), "nothing started");
}

#[test]
fn the_x11_folder_check_wants_1777() {
    let d = std::env::temp_dir().join(format!("k2-x11dir-{}", uuid::Uuid::new_v4().simple()));
    assert!(x11_dir_ok(&d).unwrap_err().contains("doesn't exist"));
    std::fs::create_dir_all(&d).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(x11_dir_ok(&d).unwrap_err().contains("not 1777"));
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o1777)).unwrap();
    }
    assert_eq!(x11_dir_ok(&d), Ok(()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_sessions_display_0_is_never_touched_and_k2_picks_from_100() {
    // On disk: a live session's :0 (its lock and its socket) and :100.
    let tmp = std::env::temp_dir().join(format!("k2-xtmp-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(tmp.join(".X11-unix")).unwrap();
    for f in [".X0-lock", ".X11-unix/X0", ".X100-lock"] {
        std::fs::write(tmp.join(f), "session").unwrap();
    }
    assert!(display_taken_in(&tmp, 0) && display_taken_in(&tmp, 100) && !display_taken_in(&tmp, 101));
    assert_eq!(vd::pick_display(|n| display_taken_in(&tmp, n), 0), Some(101));
    for f in [".X0-lock", ".X11-unix/X0", ".X100-lock"] {
        assert_eq!(std::fs::read_to_string(tmp.join(f)).unwrap(), "session", "{f} untouched");
    }
    let _ = std::fs::remove_dir_all(&tmp);

    // The manager asks Xvfb for the display it picked, explicitly.
    let r = Rig::new(true, true);
    r.host.taken.lock().unwrap().extend([0, 100]);
    r.m.start("virtual", Asker::Person, None).unwrap();
    let x = &r.runs("systemd-run")[0];
    assert!(x.contains(" Xvfb :101 -displayfd 3 "), "{x}");
    assert!(!x.contains(" Xvfb :0 ") && !x.contains(" Xvfb -displayfd"), "never :0, never Xvfb's own pick: {x}");
    assert_eq!(r.record("virtual").display.as_deref(), Some(":101"));
}

#[test]
fn a_display_grabbed_in_a_race_moves_to_the_next() {
    for systemd in [true, false] {
        let r = Rig::new(systemd, true);
        r.host.race.lock().unwrap().insert(100);
        assert_eq!(r.m.start("virtual", Asker::Person, None), Ok(State::Ready), "systemd {systemd}");
        assert_eq!(r.record("virtual").display.as_deref(), Some(":101"), "systemd {systemd}");
        let xs: Vec<String> = r.host.lines().into_iter().filter(|l| l.contains(" Xvfb :")).collect();
        assert_eq!(xs.len(), 2, "{xs:?}");
        assert!(xs[0].contains(" Xvfb :100 ") && xs[1].contains(" Xvfb :101 "), "{xs:?}");
    }
}

#[test]
fn reclaim_eagain_is_normal() {
    assert_eq!(reclaim_result(Ok(())), Ok(()));
    #[cfg(unix)]
    assert_eq!(reclaim_result(Err(io::Error::from_raw_os_error(libc::EAGAIN))), Ok(()), "EAGAIN: it took what it could");
    assert_eq!(reclaim_result(Err(io::Error::from(io::ErrorKind::WouldBlock))), Ok(()));
    assert!(reclaim_result(Err(io::Error::from(io::ErrorKind::PermissionDenied))).is_err());
}

#[test]
fn openbox_when_icewm_is_missing() {
    let r = Rig::new(true, true);
    r.host.programs.lock().unwrap().retain(|p| *p != "icewm-session");
    r.host.programs.lock().unwrap().push("openbox-session");
    r.m.start("virtual", Asker::Person, None).unwrap();
    assert!(r.runs("systemd-run")[1].contains("dbus-run-session -- openbox-session"));
}

#[test]
fn adopt_takes_back_an_awake_and_an_asleep_desktop() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    r.m.viewer_joined("virtual", "dv1", "rosson", true);
    r.advance(vd::SLEEP_AFTER);
    r.m.viewer_present("virtual", "dv1");
    r.m.tick();
    assert_eq!((r.m.state("virtual"), r.m.state("virtual:w1")), (State::Ready, State::Asleep));
    let asleep_since = r.record("virtual:w1").asleep_since;

    let r2 = r.restarted();
    r2.clock.0.store(r.m.now() + 60, Ordering::SeqCst);
    r2.host.clear();
    r2.m.adopt();
    assert_eq!(r2.m.state("virtual"), State::Ready);
    assert_eq!(r2.m.state("virtual:w1"), State::Asleep);
    assert_eq!(r2.m.status("virtual:w1").next_close_at, asleep_since.map(|t| t + vd::CLOSE_ASLEEP_AFTER), "asleep since before the restart");
    assert!(r2.runs("systemd-run").is_empty(), "adopting starts nothing");
    // The 10-minute count starts over at adoption.
    assert_eq!(r2.m.status("virtual").next_sleep_at, Some(r2.m.now() + vd::SLEEP_AFTER));
    assert_eq!(r2.m.screen_env("virtual").unwrap()[0].1, ":100");
}

#[test]
fn an_agents_hold_survives_a_daemon_restart() {
    // L9 on a live box: after `systemctl restart k2-daemon` the working
    // agent's 30-minute hold was gone, so a sidecar of its workspace took
    // the desktop, and with nobody viewing it would sleep under the agent
    // 10 minutes after the restart.
    let r = Rig::new(true, true);
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    let h = r.m.hold("virtual:w1", "w1").expect("the hold");
    let r2 = r.restarted();
    r2.clock.0.store(r.m.now() + 60, Ordering::SeqCst);
    r2.m.adopt();
    assert_eq!(r2.m.state("virtual:w1"), State::Ready);
    assert_eq!(r2.m.hold("virtual:w1", "w1/helper").expect_err("still held by w1"), h, "the sidecar is told who holds it until when");
    assert_eq!(r2.m.status("virtual:w1").hold, Some(h.clone()));
    // No viewer: the hold, not the restart, decides when it may sleep.
    r2.advance(vd::SLEEP_AFTER + 60);
    r2.m.tick();
    assert_eq!(r2.m.state("virtual:w1"), State::Ready, "held: no sleep 10 minutes after the restart");
    // A release before the restart is kept too.
    assert!(r2.m.release("virtual:w1", "w1"));
    let r3 = r2.restarted();
    r3.clock.0.store(r2.m.now() + 1, Ordering::SeqCst);
    r3.m.adopt();
    assert!(r3.m.hold("virtual:w1", "w1/helper").is_ok(), "released: free after the restart");
}

#[test]
fn a_state_file_whose_units_are_gone_reads_as_closed_and_is_cleaned() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.host.dead.lock().unwrap().insert("k2-desktop-virtual-x.service".into());
    let r2 = r.restarted();
    r2.host.clear();
    r2.m.adopt();
    let st = r2.m.status("virtual");
    assert_eq!(st.state, State::Closed);
    assert_eq!(st.closed.as_ref().map(|c| c.reason.code()), Some("crashed"));
    assert!(st.message().starts_with("The desktop stopped unexpectedly"), "{}", st.message());
    assert!(r2.host.lines().iter().any(|l| l == "systemctl --user stop k2-desktops-virtual.slice"), "what was left is stopped");
    assert_eq!(r2.record("virtual").state, State::Closed);
    assert!(!r2.home.join(".k2/desktop/virtual/display").exists());
}

#[test]
fn a_reboot_closes_with_its_reason() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    *r.host.boot.lock().unwrap() = "boot-2".into();
    let r2 = r.restarted();
    r2.m.adopt();
    let st = r2.m.status("virtual");
    assert_eq!(st.closed.as_ref().map(|c| &c.reason), Some(&CloseReason::Restarted));
    assert_eq!(st.message(), "Your desktop was closed when this server restarted.");
    assert_eq!(st.reason_json()["code"], "restarted");
}

#[test]
fn xvfb_dying_is_reported_and_cleaned_up() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.viewer_joined("virtual", "dv1", "rosson", true);
    r.host.dead.lock().unwrap().insert("k2-desktop-virtual-x.service".into());
    r.host.clear();
    r.advance(15);
    r.m.tick();
    let st = r.m.status("virtual");
    assert_eq!(st.closed.as_ref().map(|c| c.reason.clone()), Some(CloseReason::Crashed { detail: "(EE) Fatal server error: boom".into() }));
    assert!(r.host.lines().contains(&"screen close virtual closed".to_string()));
    assert!(r.host.lines().contains(&"systemctl --user stop k2-desktops-virtual.slice".to_string()));
}

#[test]
fn closed_after_a_day_asleep() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    r.host.clear();
    r.advance(vd::CLOSE_ASLEEP_AFTER);
    r.m.tick();
    assert_eq!(r.m.state("virtual"), State::Closed);
    assert_eq!(r.m.status("virtual").message(), "Your desktop was closed after a day asleep. Start a new one?");
    let all = r.host.lines();
    let pos = |p: &str| all.iter().position(|l| l == p).unwrap_or_else(|| panic!("{p} in {all:?}"));
    assert!(pos("systemctl --user thaw k2-desktop-virtual-x.service") < pos("systemctl --user stop k2-desktops-virtual.slice"), "thawed before it is stopped");
}

#[test]
fn an_awake_desktop_is_never_closed_for_memory() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    r.m.viewer_joined("virtual", "dv1", "rosson", true);
    r.advance(vd::SLEEP_AFTER);
    r.m.viewer_present("virtual", "dv1");
    r.m.tick();
    assert_eq!(r.m.state("virtual:w1"), State::Asleep);
    r.host.set_facts(64, 100 << 20, 16);
    r.m.viewer_present("virtual", "dv1");
    r.m.tick();
    assert_eq!(r.m.state("virtual:w1"), State::Closed);
    assert_eq!(r.m.status("virtual:w1").message(), "Your desktop was closed because this server was low on memory.");
    assert_eq!(r.m.state("virtual"), State::Ready);
}

#[test]
fn an_agent_hold_keeps_it_awake_and_one_holder_at_a_time() {
    let r = Rig::new(true, true);
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    let h = r.m.hold("virtual:w1", "k2/w1").unwrap();
    assert_eq!(r.m.hold("virtual:w1", "k2/w1-helper").unwrap_err(), h);
    r.advance(vd::SLEEP_AFTER + 60);
    r.m.tick();
    assert_eq!(r.m.state("virtual:w1"), State::Ready);
    assert_eq!(r.m.status("virtual:w1").hold, Some(h));
    assert!(r.m.release("virtual:w1", "k2/w1"));
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    assert_eq!(r.m.state("virtual:w1"), State::Asleep);
}

#[test]
fn without_a_freezer_it_closes_after_ten_minutes_and_says_so() {
    let r = Rig::new(true, false);
    r.m.start("virtual", Asker::Person, None).unwrap();
    assert!(!r.m.status("virtual").can_sleep);
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    let st = r.m.status("virtual");
    assert_eq!(st.state, State::Closed);
    assert!(st.message().contains("can't pause a desktop"), "{}", st.message());
    assert!(!r.host.lines().iter().any(|l| l.contains("freeze")));
}

#[test]
fn a_freeze_that_fails_closes_instead() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    *r.host.fail_freeze.lock().unwrap() = true;
    r.advance(vd::SLEEP_AFTER);
    r.m.tick();
    assert_eq!(r.m.status("virtual").closed.map(|c| c.reason), Some(CloseReason::NobodyViewing));
}

#[test]
fn with_no_user_manager_the_programs_run_detached_and_close_on_stop() {
    let r = Rig::new(false, true);
    assert!(!r.m.can_sleep(), "no user manager: no sleep");
    r.m.start("virtual", Asker::Person, None).unwrap();
    assert!(r.runs("systemd-run").is_empty());
    let spawns = r.runs("spawn");
    assert_eq!(spawns.len(), 2);
    assert!(spawns[0].starts_with("spawn /usr/bin/env -i ") && spawns[0].contains("K2_NO_OWNER_FALLBACK=1") && spawns[0].contains(" Xvfb :100 -displayfd 3"));
    assert!(spawns[1].contains("DISPLAY=:100") && spawns[1].contains("icewm-session"));
    let rec = r.record("virtual");
    assert_eq!((rec.mode, rec.pids.len()), (RunMode::Detached, 2));
    assert!(r.m.close_with("virtual", CloseReason::ClosedBy { by: "rosson".into() }));
    assert_eq!(r.runs("kill").len(), 2);
    assert_eq!(r.m.status("virtual").message(), "The desktop was closed by rosson.");
}

#[test]
fn close_all_closes_every_virtual_desktop() {
    let r = Rig::new(true, true);
    r.m.start("virtual", Asker::Person, None).unwrap();
    r.m.start("virtual:w1", Asker::Agent, None).unwrap();
    assert_eq!(r.m.close_all(CloseReason::SwitchedOff), 2);
    assert_eq!((r.m.state("virtual"), r.m.state("virtual:w1")), (State::Closed, State::Closed));
}

#[test]
fn a_bad_id_is_refused() {
    let r = Rig::new(true, true);
    assert_eq!(r.m.start("console", Asker::Person, None), Err(StartError::BadId));
    assert_eq!(r.m.start("virtual:a:b", Asker::Person, None), Err(StartError::BadId));
}

#[test]
fn parsers() {
    assert_eq!(systemd_version("systemd 255 (255.4-1ubuntu8.4)\n+PAM +AUDIT"), Some(255));
    assert_eq!(systemd_version(""), None);
    assert_eq!(parse_meminfo("MemTotal:        8000000 kB\nMemFree: 1 kB\nMemAvailable:    4000000 kB\n"), (8_192_000_000, 4_096_000_000));
}

// ── a real Xvfb (Linux; `--features xvfb-tests`) ─────────────────────
//
// Needs `Xvfb` and /tmp/.X11-unix (1777). A box makes that folder at boot;
// a compute job has its own /tmp, so its command first runs
// `mkdir -p -m 1777 /tmp/.X11-unix`.

#[cfg(all(target_os = "linux", feature = "xvfb-tests"))]
mod xvfb {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    /// A raw X11 client: just enough protocol to prove the cookie and to
    /// make and read back a window.
    pub struct X {
        s: UnixStream,
        pub root: u32,
        base: u32,
        next: u32,
    }

    fn pad4(n: usize) -> usize {
        (4 - n % 4) % 4
    }

    /// The cookie in a desktop's Xauthority file (the last 16 bytes).
    pub fn cookie_of(path: &Path) -> Vec<u8> {
        let b = std::fs::read(path).expect("Xauthority");
        b[b.len() - 16..].to_vec()
    }

    /// Connect to `:<n>` with `cookie` (None: no auth). `Err` holds the
    /// server's refusal.
    pub fn connect(display: &str, cookie: Option<&[u8]>) -> Result<X, String> {
        let n = display.trim_start_matches(':');
        let mut s = UnixStream::connect(format!("/tmp/.X11-unix/X{n}")).map_err(|e| format!("connect: {e}"))?;
        let (name, data): (&[u8], &[u8]) = match cookie {
            Some(c) => (b"MIT-MAGIC-COOKIE-1", c),
            None => (b"", b""),
        };
        let mut req = vec![b'l', 0, 11, 0, 0, 0];
        req.extend_from_slice(&(name.len() as u16).to_le_bytes());
        req.extend_from_slice(&(data.len() as u16).to_le_bytes());
        req.extend_from_slice(&[0, 0]);
        req.extend_from_slice(name);
        req.extend(std::iter::repeat_n(0u8, pad4(name.len())));
        req.extend_from_slice(data);
        req.extend(std::iter::repeat_n(0u8, pad4(data.len())));
        s.write_all(&req).unwrap();
        let mut head = [0u8; 8];
        s.read_exact(&mut head).map_err(|e| format!("setup reply: {e}"))?;
        let extra = u16::from_le_bytes([head[6], head[7]]) as usize * 4;
        let mut body = vec![0u8; extra];
        s.read_exact(&mut body).unwrap();
        if head[0] != 1 {
            let why = String::from_utf8_lossy(&body[..(head[1] as usize).min(body.len())]).into_owned();
            return Err(format!("refused: {why}"));
        }
        let base = u32::from_le_bytes(body[4..8].try_into().unwrap());
        let vendor = u16::from_le_bytes([body[16], body[17]]) as usize;
        let formats = body[21] as usize;
        let screen = 32 + vendor + pad4(vendor) + 8 * formats;
        let root = u32::from_le_bytes(body[screen..screen + 4].try_into().unwrap());
        Ok(X { s, root, base, next: 1 })
    }

    impl X {
        fn send(&mut self, b: &[u8]) {
            self.s.write_all(b).unwrap();
        }

        fn reply(&mut self) -> Vec<u8> {
            let mut head = [0u8; 32];
            self.s.read_exact(&mut head).unwrap();
            assert_ne!(head[0], 0, "X error {}", head[1]);
            let more = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize * 4;
            let mut v = head.to_vec();
            let mut rest = vec![0u8; more];
            self.s.read_exact(&mut rest).unwrap();
            v.extend(rest);
            v
        }

        /// A mapped window at (x, y, w, h) named `name`.
        pub fn window(&mut self, x: i16, y: i16, w: u16, h: u16, name: &str) -> u32 {
            let id = self.base | self.next;
            self.next += 1;
            let mut r = vec![1u8, 0];
            r.extend_from_slice(&8u16.to_le_bytes());
            r.extend_from_slice(&id.to_le_bytes());
            r.extend_from_slice(&self.root.to_le_bytes());
            r.extend_from_slice(&x.to_le_bytes());
            r.extend_from_slice(&y.to_le_bytes());
            r.extend_from_slice(&w.to_le_bytes());
            r.extend_from_slice(&h.to_le_bytes());
            r.extend_from_slice(&0u16.to_le_bytes());
            r.extend_from_slice(&1u16.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            self.send(&r);
            let n = name.len();
            let mut p = vec![18u8, 0];
            p.extend_from_slice(&((6 + (n + pad4(n)) / 4) as u16).to_le_bytes());
            p.extend_from_slice(&id.to_le_bytes());
            p.extend_from_slice(&39u32.to_le_bytes()); // WM_NAME
            p.extend_from_slice(&31u32.to_le_bytes()); // STRING
            p.extend_from_slice(&[8, 0, 0, 0]);
            p.extend_from_slice(&(n as u32).to_le_bytes());
            p.extend_from_slice(name.as_bytes());
            p.extend(std::iter::repeat_n(0u8, pad4(n)));
            self.send(&p);
            let mut m = vec![8u8, 0];
            m.extend_from_slice(&2u16.to_le_bytes());
            m.extend_from_slice(&id.to_le_bytes());
            self.send(&m);
            id
        }

        /// (x, y, w, h) of a window.
        pub fn geometry(&mut self, id: u32) -> (i16, i16, u16, u16) {
            let mut r = vec![14u8, 0];
            r.extend_from_slice(&2u16.to_le_bytes());
            r.extend_from_slice(&id.to_le_bytes());
            self.send(&r);
            let v = self.reply();
            let i = |o: usize| i16::from_le_bytes([v[o], v[o + 1]]);
            let u = |o: usize| u16::from_le_bytes([v[o], v[o + 1]]);
            (i(12), i(14), u(16), u(18))
        }

        /// The root window's children (QueryTree).
        pub fn children(&mut self) -> Vec<u32> {
            let mut r = vec![15u8, 0];
            r.extend_from_slice(&2u16.to_le_bytes());
            r.extend_from_slice(&self.root.to_le_bytes());
            self.send(&r);
            let v = self.reply();
            let n = u16::from_le_bytes([v[16], v[17]]) as usize;
            (0..n).map(|i| u32::from_le_bytes(v[32 + 4 * i..36 + 4 * i].try_into().unwrap())).collect()
        }

        /// A window's WM_NAME.
        pub fn name(&mut self, id: u32) -> String {
            let mut r = vec![20u8, 0];
            r.extend_from_slice(&6u16.to_le_bytes());
            r.extend_from_slice(&id.to_le_bytes());
            r.extend_from_slice(&39u32.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&1000u32.to_le_bytes());
            self.send(&r);
            let v = self.reply();
            let n = u32::from_le_bytes(v[16..20].try_into().unwrap()) as usize;
            String::from_utf8_lossy(&v[32..32 + n]).into_owned()
        }
    }

    /// The manager on this machine with a session that needs no window
    /// manager (compute nodes have Xvfb but no IceWM).
    pub fn real() -> (Arc<Manager>, PathBuf) {
        let home = std::env::temp_dir().join(format!("k2-vdesktop-x-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let mut cfg = Config::for_home(&home);
        cfg.session = Some(vec!["sleep".into(), "86400".into()]);
        cfg.display_wait = Duration::from_secs(10);
        // Each test is its own process (nextest), with its own manager:
        // spread their numbers so two never race for one (in a daemon,
        // START_LOCK does that).
        cfg.display_base = 200 + (uuid::Uuid::new_v4().as_u128() % 600) as u32;
        (Manager::new(Arc::new(RealHost::default()), Arc::new(SystemClock), cfg), home)
    }

    fn id() -> String {
        format!("virtual:it{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
    }

    #[test]
    fn a_real_desktop_takes_its_cookie_and_nothing_else() {
        let (m, home) = real();
        let id = id();
        assert_eq!(m.start(&id, Asker::Person, None), Ok(State::Ready));
        let display = m.status(&id).display.expect("a display");
        let cookie = cookie_of(&home.join(".k2/desktop").join(vd::folder_slug(&id)).join("Xauthority"));
        let mut x = connect(&display, Some(&cookie)).expect("the desktop's cookie opens it");
        assert!(x.root != 0);
        assert!(connect(&display, None).is_err(), "no cookie: refused");
        assert!(connect(&display, Some(&[0u8; 16])).is_err(), "a wrong cookie: refused");
        // No abstract socket (-nolisten local).
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{}", display.trim_start_matches(':'))).unwrap();
        assert!(UnixStream::connect_addr(&addr).is_err(), "the abstract socket isn't offered");
        // And nothing on TCP.
        let port = 6000 + display.trim_start_matches(':').parse::<u16>().unwrap();
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "no TCP listener");
        let w = x.window(10, 20, 300, 200, "k2-test");
        assert_eq!(x.geometry(w), (10, 20, 300, 200));
        m.close_with(&id, CloseReason::ClosedBy { by: "test".into() });
        let _ = std::fs::remove_dir_all(home);
    }

    /// L8: two agents get two desktops, and neither's input reaches the
    /// other's windows: each display takes only its own cookie, and a
    /// window made on one is not on the other.
    #[test]
    fn two_agents_desktops_keep_to_themselves() {
        let (m, home) = real();
        let (a, b) = (id(), id());
        m.start(&a, Asker::Agent, None).unwrap();
        m.start(&b, Asker::Agent, None).unwrap();
        let (da, db) = (m.status(&a).display.unwrap(), m.status(&b).display.unwrap());
        assert_ne!(da, db, "two displays");
        let ca = cookie_of(&home.join(".k2/desktop").join(vd::folder_slug(&a)).join("Xauthority"));
        let cb = cookie_of(&home.join(".k2/desktop").join(vd::folder_slug(&b)).join("Xauthority"));
        assert_ne!(ca, cb, "two cookies");
        assert!(connect(&db, Some(&ca)).is_err(), "A's cookie doesn't open B's display");
        assert!(connect(&da, Some(&cb)).is_err(), "B's cookie doesn't open A's display");
        let mut xa = connect(&da, Some(&ca)).unwrap();
        let mut xb = connect(&db, Some(&cb)).unwrap();
        let before = xb.children();
        let w = xa.window(30, 40, 200, 100, "agent a");
        assert!(xa.children().contains(&w), "on A");
        assert_eq!(xb.children(), before, "nothing new on B");
        // Profiles: one each, never the same folder.
        assert_ne!(m.profile_dir(&a).unwrap(), m.profile_dir(&b).unwrap());
        for id in [&a, &b] {
            m.close_with(id, CloseReason::ClosedBy { by: "test".into() });
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_second_start_returns_the_same_desktop_unchanged() {
        let (m, home) = real();
        let id = id();
        m.start(&id, Asker::Person, None).unwrap();
        let first = m.status(&id).display;
        let cookie = cookie_of(&home.join(".k2/desktop").join(vd::folder_slug(&id)).join("Xauthority"));
        let mut x = connect(first.as_deref().unwrap(), Some(&cookie)).unwrap();
        let w = x.window(5, 6, 120, 80, "kept");
        assert_eq!(m.start(&id, Asker::Person, Some(Size { width: 1920, height: 1080 })), Ok(State::Ready));
        assert_eq!(m.status(&id).display, first);
        assert_eq!(x.geometry(w), (5, 6, 120, 80), "the window is still there");
        assert_eq!(x.name(w), "kept");
        m.close_with(&id, CloseReason::ClosedBy { by: "test".into() });
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn killing_xvfb_is_reported_and_cleaned_up() {
        let (m, home) = real();
        let id = id();
        m.start(&id, Asker::Person, None).unwrap();
        let rec: Record = serde_json::from_slice(&std::fs::read(home.join(".k2/desktop").join(vd::folder_slug(&id)).join("state.json")).unwrap()).unwrap();
        match rec.mode {
            RunMode::Detached => {
                // SAFETY: the Xvfb we started.
                unsafe { libc::kill(rec.pids[0] as i32, libc::SIGKILL) };
            }
            RunMode::Systemd => {
                RealHost::default().run("systemctl", &["--user".into(), "kill".into(), "--signal=KILL".into(), rec.units.x.clone()]).unwrap();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while m.state(&id) != State::Closed && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            m.tick();
        }
        let st = m.status(&id);
        assert_eq!(st.closed.as_ref().map(|c| c.reason.code()), Some("crashed"), "{st:?}");
        assert!(!home.join(".k2/desktop").join(vd::folder_slug(&id)).join("Xauthority").exists());
        let _ = std::fs::remove_dir_all(home);
    }

    #[cfg(feature = "xvfb-systemd-tests")]
    #[test]
    fn a_window_is_the_same_after_freeze_and_thaw() {
        let (m, home) = real();
        assert!(m.can_sleep(), "this test runs where the user manager can freeze (a fleet box as its K2 user)");
        let id = id();
        m.start(&id, Asker::Person, None).unwrap();
        let display = m.status(&id).display.unwrap();
        let cookie = cookie_of(&home.join(".k2/desktop").join(vd::folder_slug(&id)).join("Xauthority"));
        let mut x = connect(&display, Some(&cookie)).unwrap();
        let w = x.window(40, 50, 640, 480, "before sleep");
        assert_eq!(x.geometry(w), (40, 50, 640, 480));
        m.do_sleep(&id);
        assert_eq!(m.state(&id), State::Asleep);
        assert_eq!(m.start(&id, Asker::Person, None), Ok(State::Ready));
        assert_eq!(x.geometry(w), (40, 50, 640, 480));
        assert_eq!(x.name(w), "before sleep");
        m.close_with(&id, CloseReason::ClosedBy { by: "test".into() });
        let _ = std::fs::remove_dir_all(home);
    }
}
