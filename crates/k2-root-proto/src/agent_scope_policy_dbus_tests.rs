//! The bus policy under dbus-daemon's own config parser (the test box
//! showed a malformed file is dropped whole while `ReloadConfig` still
//! answers 0). A throwaway session bus reads the file through
//! `<includedir>`, the way the system bus reads `system.d`; dbus-daemon
//! reports a file it drops there as "Encountered error … while parsing"
//! (and carries on, as on the box). Linux only; there a missing
//! dbus-daemon fails the test (install the `dbus-daemon` package), never a
//! skip.

use super::*;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let d = std::env::temp_dir().join(format!("k2-bus-policy-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&d).expect("temp dir");
        TempDir(d)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// dbus-daemon run on a session bus config that includes `policy`:
/// (whether it came up and printed its address, its stderr).
fn parse_under_dbus_daemon(policy: &str) -> (bool, String) {
    let t = TempDir::new();
    let sysd = t.0.join("system.d");
    std::fs::create_dir_all(&sysd).expect("system.d");
    std::fs::write(sysd.join(BUS_POLICY_FILE), policy).expect("policy file");
    let conf = t.0.join("bus.conf");
    std::fs::write(&conf, bus_conf(&sysd)).expect("bus config");
    let mut child = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", conf.display()))
        .args(["--nofork", "--nopidfile", "--print-address"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("dbus-daemon could not run ({e}): this test needs it (the dbus-daemon package); it is never skipped"));
    // The address line once the config is read and the bus listens (on
    // loopback TCP: no socket path to outgrow a long temp dir); EOF
    // when dbus-daemon refused its config and exited.
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout")).read_line(&mut line).expect("read the address");
    let _ = child.kill();
    let _ = child.wait();
    let mut err = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut err).expect("read stderr");
    (line.starts_with("tcp:"), err)
}

fn bus_conf(includedir: &Path) -> String {
    format!(
        "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN\"\n \
         \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
         <busconfig>\n  <type>session</type>\n  <listen>tcp:host=127.0.0.1,port=0</listen>\n  \
         <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    <allow own=\"*\"/>\n  </policy>\n  \
         <includedir>{}</includedir>\n</busconfig>\n",
        includedir.display()
    )
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "dbus-daemon's config check runs on the Linux test nodes")]
fn the_bus_policy_parses_under_dbus_daemon_and_a_member_without_an_interface_does_not() {
    // `root`: a user every box has (an unknown name is only a warning, but
    // this keeps the check to the rules themselves).
    let policy = bus_policy("root").expect("text");
    let (up, err) = parse_under_dbus_daemon(&policy);
    assert!(up, "dbus-daemon did not start with the policy included: {err}");
    assert!(!err.contains("Encountered error"), "dbus-daemon dropped the policy: {err}");

    // The line the test box rejected: a member with no interface.
    let bad = policy.replace(
        "  </policy>",
        "    <deny send_destination=\"org.freedesktop.systemd1\" send_member=\"AttachProcessesToUnit\"/>\n  </policy>",
    );
    assert_ne!(bad, policy);
    let (_, err) = parse_under_dbus_daemon(&bad);
    assert!(err.contains("Encountered error"), "dbus-daemon took a member-only deny; the check would not catch it: {err:?}");
}
