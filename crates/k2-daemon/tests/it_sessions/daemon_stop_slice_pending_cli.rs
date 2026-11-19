//! 0.46.16: once box prep's cores floor moved the daemon's unit to
//! k2cores.slice, systemd moves the running daemon only at a reboot. A stop
//! then start while the unit's processes live (KillMode=process: k2-ptyd,
//! agent sessions, frpc) fails to start the daemon at all (verified on
//! systemd 255). So `k2 daemon stop --keep-agents`, the maintenance stop
//! whose whole point is a start with the agents still alive, refuses while
//! the slice is pending and never calls `systemctl stop`; a plain stop
//! still stops, and says a reboot is what applies the slice.
//!
//! Real cli/k2, a fake `systemctl` (and `uname`) on PATH, a temp HOME.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_LOG"
case "$*" in
  *"--user cat"*) exit 1 ;;
  "cat k2-daemon.service") exit 0 ;;
  *"-p Slice"*) echo "$FAKE_SLICE" ;;
  *"-p ControlGroup"*) echo "$FAKE_CG" ;;
esac
exit 0
"#;

struct FakeBox {
    dir: PathBuf,
}

impl FakeBox {
    fn new() -> FakeBox {
        let dir = std::env::temp_dir().join(format!("k2-slice-pending-{}", uuid::Uuid::new_v4().simple()));
        for d in ["bin", "home/.k2"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let exe = |name: &str, body: &str| {
            let p = dir.join("bin").join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        exe("systemctl", FAKE_SYSTEMCTL);
        exe("uname", "#!/bin/sh\necho Linux\n");
        // The connection gate only wants the files to exist.
        std::fs::write(dir.join("home/.k2/heartbeat.port"), "1\n").unwrap();
        std::fs::write(dir.join("home/.k2/heartbeat.token"), "t\n").unwrap();
        FakeBox { dir }
    }

    fn stop(&self, args: &[&str], unit_slice: &str) -> (Output, Vec<String>) {
        let log = self.dir.join("systemctl.log");
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_dir_all(self.dir.join("home/.k2/run"));
        let cli = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        let out = Command::new(k2_core::test_env::bash_program())
            .arg(&cli)
            .args(["daemon", "stop"])
            .args(args)
            .env_clear()
            .envs(k2_core::test_env::cli_base_env())
            .env("PATH", format!("{}:/usr/bin:/bin", self.dir.join("bin").display()))
            .env("HOME", self.dir.join("home"))
            .env("FAKE_LOG", &log)
            .env("FAKE_SLICE", unit_slice)
            .env("FAKE_CG", "/system.slice/k2-daemon.service")
            .output()
            .expect("run cli/k2");
        let calls = std::fs::read_to_string(&log).unwrap_or_default().lines().map(str::to_string).collect();
        (out, calls)
    }

    fn intent(&self, name: &str) -> bool {
        self.dir.join("home/.k2/run").join(name).exists()
    }
}

impl Drop for FakeBox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stopped(calls: &[String]) -> bool {
    calls.iter().any(|c| c.split_whitespace().next() == Some("stop"))
}

#[test]
fn a_keep_agents_stop_refuses_while_the_daemons_slice_is_pending() {
    let b = FakeBox::new();
    let (out, calls) = b.stop(&["--keep-agents"], "k2cores.slice");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "refused: {err}");
    assert!(err.contains("\"error\":\"slice_pending\"") && err.contains("system.slice") && err.contains("k2cores.slice"), "{err}");
    assert!(err.contains("reboot the box"), "{err}");
    assert!(!stopped(&calls), "no systemctl stop: {calls:?}");
    assert!(!b.intent("keep-intent.json"), "no keep intent left behind");

    // The slice where the unit says: the maintenance stop runs as before.
    let (out, calls) = b.stop(&["--keep-agents"], "system.slice");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(calls.iter().any(|c| c == "stop k2-daemon.service"), "{calls:?}");
    assert!(b.intent("keep-intent.json"));
}

#[test]
fn a_plain_stop_still_stops_and_points_at_a_reboot() {
    let b = FakeBox::new();
    let (out, calls) = b.stop(&[], "k2cores.slice");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(calls.iter().any(|c| c == "stop k2-daemon.service"), "{calls:?}");
    assert!(err.contains("reboot the box"), "{err}");
    assert!(!err.contains("stop and start"), "never suggests a stop and start: {err}");
    let (out, _) = b.stop(&[], "system.slice");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("reboot"), "nothing pending, no note");
}
