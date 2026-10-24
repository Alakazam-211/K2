//! `creds` namespace (protocol 10, `k2 vault systemd-cred`) over the temp
//! root and fakes of `tests.rs`. A fake `systemctl` (version, `show`,
//! reload, restart) and a fake `systemd-creds`; nothing runs the real ones.

use super::*;

const VALUE: &[u8] = b"k2test-ingest-value-0123456789";
const CIPHER: &[u8] = b"FAKE-CIPHERTEXT-k2test\n";
const STORE: &str = "/etc/k2/credstore.encrypted";
const CRED: &str = "/etc/k2/credstore.encrypted/ingest_key.cred";
const DROPIN: &str = "/etc/systemd/system/k2-watch.service.d/k2-cred-ingest_key.conf";
const ALLOW: &str = "/etc/k2-root-helper/systemd-cred-units";
const STATE: &str = "/var/lib/k2-root-helper/creds.json";
const DAEMON_B_PID: i32 = 4343;
const K2B_UID: u32 = 1002;

fn json(o: &crate::Outcome) -> serde_json::Value {
    ok(o);
    serde_json::from_slice(&o.stdout).unwrap()
}

fn out(status: i32, stdout: &[u8]) -> ExecOut {
    ExecOut { status, stdout: stdout.to_vec(), stderr: vec![] }
}

/// One unit as the fake systemd sees it.
#[derive(Clone)]
struct U {
    name: String,
    load: &'static str,
    id: String,
    names: Vec<String>,
    /// Credential lines the unit's own fragment has, as unit-file text
    /// (`LoadCredential=id:path`, `SetCredential=id:x`, `ImportCredential=glob`).
    creds: String,
    /// Inactive and unreferenced: systemd garbage-collected it, so GetUnit
    /// fails ("not loaded") while LoadUnit loads it (A3-P1).
    gc: bool,
}

fn unit(name: &str) -> U {
    U { name: name.into(), load: "loaded", id: name.into(), names: vec![name.into()], creds: String::new(), gc: false }
}

/// What systemd holds after loading the fragment, then every drop-in in
/// name order: one map of LoadCredential*= by ID (a later line with the
/// same ID replaces the earlier one, encrypted or not), SetCredential*= ids
/// and ImportCredential= globs. Answered the way `busctl --json=short
/// get-property` prints it (review A2-N1: never `systemctl show` text).
fn effective(root: &std::path::Path, u: &U, prop: &str) -> String {
    let mut text = u.creds.clone();
    let d = under(root, &format!("/etc/systemd/system/{}.d", u.id));
    let mut files: Vec<_> = std::fs::read_dir(&d).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.sort();
    for f in files {
        text.push_str(&std::fs::read_to_string(f).unwrap_or_default());
    }
    let mut load: Vec<(String, bool, String)> = vec![];
    let mut set: Vec<String> = vec![];
    let mut import: Vec<String> = vec![];
    for l in text.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let (id, path) = v.split_once(':').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((v.to_string(), v.to_string()));
        match k {
            "LoadCredential" | "LoadCredentialEncrypted" => {
                load.retain(|(i, _, _)| *i != id);
                load.push((id, k == "LoadCredentialEncrypted", path));
            }
            "SetCredential" | "SetCredentialEncrypted" => set.push(id),
            "ImportCredential" => import.push(v.to_string()),
            _ => {}
        }
    }
    let pairs = |enc: bool| -> String {
        let items: Vec<String> =
            load.iter().filter(|(_, e, _)| *e == enc).map(|(i, _, p)| format!("[{:?},{:?}]", i, p)).collect();
        format!(r#"{{"type":"a(ss)","data":[{}]}}"#, items.join(","))
    };
    match prop {
        "LoadCredential" => pairs(false),
        "LoadCredentialEncrypted" => pairs(true),
        "SetCredential" => format!(r#"{{"type":"a(say)","data":[{}]}}"#, set.iter().map(|i| format!("[{i:?},[1,2]]")).collect::<Vec<_>>().join(",")),
        "SetCredentialEncrypted" => r#"{"type":"a(say)","data":[]}"#.to_string(),
        "ImportCredentialEx" => r#"{"type":"a(ss)","data":[]}"#.to_string(),
        _ => format!(r#"{{"type":"as","data":[{}]}}"#, import.iter().map(|g| format!("{g:?}")).collect::<Vec<_>>().join(",")),
    }
}

/// A box with systemd `version`, `units` known to systemd, the allow list
/// `allow` (None = no file), a TPM2 when `tpm2`, and a working
/// `systemd-creds encrypt`. The fake `show` adds K2's drop-in line when
/// K2's drop-in file is on disk.
fn creds_box_with(version: u32, units: Vec<U>, allow: Option<&str>, tpm2: bool) -> T {
    let t = box_root();
    mkdirs(&t.root, &["/etc", "/usr/bin", "/var/lib"]);
    // Modes set explicitly: a node's umask may be 002.
    std::fs::set_permissions(t.p("/etc/k2-root-helper"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let units_b = units.clone();
    std::fs::write(t.p("/usr/bin/systemd-creds"), b"fake").unwrap();
    if let Some(a) = allow {
        std::fs::write(t.p(ALLOW), a).unwrap();
        std::fs::set_permissions(t.p(ALLOW), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let mut log = t.exec.0.borrow_mut();
    log.programs.insert(
        SYSTEMCTL.into(),
        Rc::new(move |a: &[String]| match a.first().map(String::as_str) {
            Some("--version") => out(0, format!("systemd {version} ({version}.4-1)\n+PAM +AUDIT\n").as_bytes()),
            // systemd's real `systemctl show -p` text: `Key=value` lines.
            Some("show") => {
                let name = a.last().cloned().unwrap_or_default();
                let Some(u) = units.iter().find(|u| u.name == name || u.names.contains(&name)) else {
                    return out(0, format!("LoadState=not-found\nId={name}\nNames={name}\n").as_bytes());
                };
                out(0, format!("LoadState={}\nId={}\nNames={}\n", u.load, u.id, u.names.join(" ")).as_bytes())
            }
            _ => out(0, b""),
        }),
    );
    log.programs_in.insert(
        k2_root_proto::creds::SYSTEMD_CREDS.into(),
        Rc::new(move |a: &[String], stdin: Option<&[u8]>| match a.first().map(String::as_str) {
            Some("has-tpm2") => out(if tpm2 { 0 } else { 1 }, b"partial\n"),
            Some("encrypt") => {
                assert_eq!(stdin, Some(VALUE), "the value reaches systemd-creds on stdin");
                out(0, CIPHER)
            }
            _ => out(1, b""),
        }),
    );
    let units2 = units_b.clone();
    let root2 = t.root.clone();
    log.programs.insert(
        k2_root_proto::creds::BUSCTL.into(),
        Rc::new(move |a: &[String]| {
            assert_eq!(a.first().map(String::as_str), Some("--json=short"));
            match a.get(1).map(String::as_str) {
                Some("call") if matches!(a.get(5).map(String::as_str), Some("GetUnit" | "LoadUnit")) => {
                    let name = a.last().cloned().unwrap_or_default();
                    let get = a.get(5).map(String::as_str) == Some("GetUnit");
                    match units2.iter().find(|u| u.name == name || u.names.contains(&name)) {
                        Some(u) if get && u.gc => ExecOut { status: 1, stdout: vec![], stderr: b"Call failed: Unit not loaded.".to_vec() },
                        Some(u) => out(0, format!(r#"{{"type":"o","data":["/org/freedesktop/systemd1/unit/{}"]}}"#, k2_root_proto::creds::bus_escape(&u.id)).as_bytes()),
                        None => ExecOut { status: 1, stdout: vec![], stderr: b"Call failed: Unit not loaded.".to_vec() },
                    }
                }
                Some("get-property") => {
                    let path = a.get(3).cloned().unwrap_or_default();
                    let prop = a.last().cloned().unwrap_or_default();
                    match units2.iter().find(|u| path.ends_with(&k2_root_proto::creds::bus_escape(&u.id))) {
                        Some(u) => out(0, effective(&root2, u, &prop).as_bytes()),
                        None => out(1, b""),
                    }
                }
                _ => out(1, b""),
            }
        }),
    );
    drop(log);
    t
}

const OPS: &str = "k2-watch.service\nk2-control-plane.service\nk2-ops-push.service\n";

fn creds_box(version: u32, units: &[&str], tpm2: bool) -> T {
    creds_box_with(version, units.iter().map(|u| unit(u)).collect(), Some(OPS), tpm2)
}

fn creds_calls(t: &T) -> Vec<(String, Vec<String>, Option<Vec<u8>>)> {
    t.calls()
        .into_iter()
        .filter(|(p, a, _)| !(p == SYSTEMCTL && a.iter().any(|x| x == "MainPID")))
        .collect()
}

fn count(t: &T, prog: &str, first: &str) -> usize {
    creds_calls(t).iter().filter(|(p, a, _)| p == prog && a.first().map(String::as_str) == Some(first)).count()
}

fn restarts(t: &T) -> Vec<Vec<String>> {
    creds_calls(t).into_iter().filter(|(p, a, _)| p == SYSTEMCTL && a.iter().any(|x| x.contains("restart"))).map(|(_, a, _)| a).collect()
}

fn audit_has_no_value(t: &T) {
    let needle = std::str::from_utf8(VALUE).unwrap();
    for l in t.audit.0.borrow().iter() {
        assert!(!l.contains(needle), "value in the audit: {l}");
    }
}

fn state(t: &T) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(t.p(STATE)).unwrap()).unwrap()
}

/// A second daemon on the box (user k2b, unit k2test-daemon-b.service),
/// and the caller switched to it.
fn add_daemon_b(t: &T) {
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n\
                [[daemon]]\nuser = \"k2b\"\nunit = \"k2test-daemon-b.service\"\nscope = \"system\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    t.exec.0.borrow_mut().main_pids.insert("k2test-daemon-b.service".into(), DAEMON_B_PID);
    let mut p = t.procs.0.borrow_mut();
    p.users.insert("k2b".into(), K2B_UID);
    p.procs.insert(
        DAEMON_B_PID,
        P { exe: "/home/k2b/.local/bin/k2-daemon".into(), exe_owner: K2B_UID, ppid: 1, ruid: K2B_UID, sid: DAEMON_B_PID },
    );
}

fn as_daemon_b(t: &T) {
    let mut p = t.procs.0.borrow_mut();
    let s = p.procs.get_mut(&SUDO_PID).unwrap();
    s.ppid = DAEMON_B_PID;
    s.ruid = K2B_UID;
    s.sid = DAEMON_B_PID;
    p.own_sid = DAEMON_B_PID;
}

fn as_daemon_a(t: &T) {
    let mut p = t.procs.0.borrow_mut();
    let s = p.procs.get_mut(&SUDO_PID).unwrap();
    s.ppid = DAEMON_PID;
    s.ruid = K2_UID;
    s.sid = DAEMON_PID;
    p.own_sid = DAEMON_PID;
}

#[test]
fn put_seals_the_value_from_stdin_into_k2s_own_0700_store_and_records_owner_and_hash() {
    let t = creds_box(255, &["k2-watch.service"], true);
    let v = json(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    assert_eq!(v["path"], CRED);
    assert_eq!(v["withKey"], "auto");
    assert_eq!(v["tpm2"], true);
    assert_eq!(std::fs::read(t.p(CRED)).unwrap(), CIPHER, "the ciphertext, never the value");
    assert_eq!(std::fs::metadata(t.p(CRED)).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(t.p(STORE)).unwrap().permissions().mode() & 0o777, 0o700);
    assert!(!t.p("/etc/credstore.encrypted").exists(), "never systemd's shared credstore");
    let enc: Vec<_> = creds_calls(&t)
        .into_iter()
        .filter(|(p, a, _)| p == k2_root_proto::creds::SYSTEMD_CREDS && a[0] == "encrypt")
        .collect();
    assert_eq!(enc.len(), 1);
    assert_eq!(enc[0].1, vec!["encrypt", "--name=ingest_key", "--with-key=auto", "-", "-"]);
    assert_eq!(enc[0].2.as_deref(), Some(VALUE));
    for (_, a, _) in creds_calls(&t) {
        assert!(a.iter().all(|x| !x.contains("k2test-ingest")), "value in argv: {a:?}");
    }
    let st = state(&t);
    assert_eq!(st["creds"][0]["name"], "ingest_key");
    assert_eq!(st["creds"][0]["owner"], "k2");
    assert_eq!(st["creds"][0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(std::fs::metadata(t.p(STATE)).unwrap().permissions().mode() & 0o777, 0o600);
    let line: serde_json::Value = serde_json::from_str(t.audit.0.borrow().last().unwrap()).unwrap();
    assert_eq!((line["ns"].as_str(), line["verb"].as_str(), line["class"].as_str()), (Some("creds"), Some("creds put ingest_key"), Some("daemon")));
    assert_eq!((line["withKey"].as_str(), line["tpm2"].as_bool()), (Some("auto"), Some(true)));
    audit_has_no_value(&t);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    assert_eq!(count(&t, k2_root_proto::creds::SYSTEMD_CREDS, "encrypt"), 2, "a rotation replaces K2's own file");
}

#[test]
fn put_never_overwrites_a_cred_k2_did_not_write_or_one_changed_by_hand() {
    let t = creds_box(255, &[], false);
    mkdirs(&t.root, &[STORE]);
    // Modes set explicitly: a node's umask may be 002.
    std::fs::set_permissions(t.p("/etc/k2"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(t.p(STORE), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(t.p(CRED), b"theirs").unwrap();
    let o = t.call_in(&["creds", "put", "ingest_key"], VALUE);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert_eq!(std::fs::read(t.p(CRED)).unwrap(), b"theirs");
    assert_eq!(count(&t, k2_root_proto::creds::SYSTEMD_CREDS, "encrypt"), 0);
    assert_eq!(t.call(&["creds", "remove", "ingest_key"]).code, exit::STATE);
    assert!(t.p(CRED).exists());
    // K2's own file, changed by hand afterwards: no longer K2's.
    std::fs::remove_file(t.p(CRED)).unwrap();
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    std::fs::write(t.p(CRED), b"edited by an admin").unwrap();
    let o = t.call_in(&["creds", "put", "ingest_key"], VALUE);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains("changed by hand"), "{}", o.stderr);
    let o = t.call(&["creds", "remove", "ingest_key"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert_eq!(std::fs::read(t.p(CRED)).unwrap(), b"edited by an admin");
}

#[test]
fn a_linked_or_open_store_or_state_dir_is_refused() {
    let t = creds_box(255, &[], false);
    mkdirs(&t.root, &["/elsewhere", "/etc/k2"]);
    symlink(t.p("/elsewhere"), t.p(STORE)).unwrap();
    let o = t.call_in(&["creds", "put", "ingest_key"], VALUE);
    assert_ne!(o.code, exit::OK);
    assert!(std::fs::read_dir(t.p("/elsewhere")).unwrap().next().is_none(), "nothing written through the link");
    std::fs::remove_file(t.p(STORE)).unwrap();
    for (dir, mode) in [(STORE, 0o750), ("/etc/k2", 0o777)] {
        mkdirs(&t.root, &[STORE]);
        std::fs::set_permissions(t.p("/etc/k2"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(t.p(STORE), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(t.p(dir), std::fs::Permissions::from_mode(mode)).unwrap();
        let o = t.call_in(&["creds", "put", "ingest_key"], VALUE);
        assert_eq!(o.code, exit::STATE, "{dir} {mode:o}: {}", o.stderr);
        assert!(!t.p(CRED).exists());
    }
    std::fs::set_permissions(t.p("/etc/k2"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(t.p(STORE), std::fs::Permissions::from_mode(0o700)).unwrap();
    mkdirs(&t.root, &["/var/lib/k2-root-helper"]);
    std::fs::set_permissions(t.p("/var/lib/k2-root-helper"), std::fs::Permissions::from_mode(0o777)).unwrap();
    let o = t.call_in(&["creds", "put", "ingest_key"], VALUE);
    assert_eq!(o.code, exit::STATE, "a group/other-writable state dir: {}", o.stderr);
}

#[test]
fn put_refusals_empty_oversize_old_systemd_and_not_the_daemon() {
    let t = creds_box(255, &[], false);
    assert_eq!(t.call_in(&["creds", "put", "ingest_key"], b"").code, exit::ARGV);
    let big = vec![b'a'; k2_root_proto::creds::CAP_CRED + 1];
    let o = t.call_in(&["creds", "put", "ingest_key"], &big);
    assert_eq!((o.code, o.stderr.as_str()), (exit::ARGV, "stdin too large"));
    t.as_agent();
    let o = run(&t.ctx, &["creds", "put", "ingest_key"], NoRead);
    assert_eq!(o.code, exit::NOT_DAEMON, "the caller check comes before stdin is read");
    for argv in [
        &["creds", "bind", "k2-watch.service", "ingest_key"][..],
        &["creds", "unbind", "k2-watch.service", "ingest_key"],
        &["creds", "restart", "k2-watch.service"],
        &["creds", "remove", "ingest_key"],
        // Which root units load K2 credentials is not for agents (B-L4).
        &["creds", "status"],
    ] {
        assert_eq!(t.call(argv).code, exit::NOT_DAEMON, "{argv:?}");
    }
    t.as_daemon();
    assert_eq!(json(&t.call(&["creds", "status"]))["systemd"], 255);
    assert!(!t.p(CRED).exists());
    let old = creds_box(249, &[], false);
    let o = old.call_in(&["creds", "put", "ingest_key"], VALUE);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains("systemd 250"), "{}", o.stderr);
    assert_eq!(json(&old.call(&["creds", "status"]))["supported"], false);
}

/// Review A-L2 / B-M2: a failed write leaves no record behind.
#[test]
fn a_failing_systemd_creds_records_nothing_and_never_echoes_the_value() {
    let t = creds_box(255, &[], false);
    t.exec.0.borrow_mut().programs_in.insert(
        k2_root_proto::creds::SYSTEMD_CREDS.into(),
        Rc::new(|_a: &[String], stdin: Option<&[u8]>| ExecOut {
            status: 1,
            stdout: vec![],
            stderr: [b"bad input: ".as_slice(), stdin.unwrap_or_default()].concat(),
        }),
    );
    let o = t.call_in(&["creds", "put", "ingest_key"], VALUE);
    assert_eq!(o.code, exit::IO);
    assert!(!o.stderr.contains("k2test-ingest") && o.stderr.contains("withheld"), "{}", o.stderr);
    assert!(!t.p(CRED).exists());
    assert!(!t.p(STATE).exists() || state(&t)["creds"].as_array().unwrap().is_empty(), "no phantom ownership");
    audit_has_no_value(&t);
}

#[test]
fn bind_refuses_every_deny_list_unit_and_daemon_toml_units_even_when_allowed() {
    let mut denied: Vec<String> = k2_root_proto::creds::K2_OWN_UNITS.iter().map(|s| s.to_string()).collect();
    denied.extend(
        ["k2app-x--d-1.service", "k2job-j1-g1.service", "k2test-x.service", "ssh.service", "sshd.service", "systemd-journald.service", "dbus.service", "k2-daemon-two.service"]
            .map(String::from),
    );
    let allow: String = denied.iter().map(|u| format!("{u}\n")).collect();
    let t = creds_box_with(255, denied.iter().map(|u| unit(u)).collect(), Some(&allow), false);
    let toml = "[[daemon]]\nuser = \"k2\"\nunit = \"k2-daemon.service\"\nscope = \"system\"\n\
                [[daemon]]\nuser = \"k2b\"\nunit = \"k2-daemon-two.service\"\nscope = \"system\"\n";
    std::fs::write(t.p(DAEMON_TOML), toml).unwrap();
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    for u in &denied {
        let o = t.call(&["creds", "bind", u, "ingest_key"]);
        assert_eq!(o.code, exit::ARGV, "{u}: {}", o.stderr);
        assert!(o.stderr.starts_with("refused unit: ") && !o.stderr.contains(u.as_str()), "{u}: {}", o.stderr);
        assert!(!t.p(&format!("/etc/systemd/system/{u}.d")).exists(), "{u}");
        assert_eq!(t.call(&["creds", "restart", u]).code, exit::ARGV, "restart {u}");
    }
    // Templates, instances and prefix names never even parse.
    for u in ["foo@.service", "foo@bar.service", "k2-.service", "sew-.service"] {
        assert_eq!(t.call(&["creds", "bind", u, "ingest_key"]).code, exit::ARGV, "{u}");
    }
    assert_eq!(count(&t, SYSTEMCTL, "daemon-reload"), 0);
    assert!(restarts(&t).is_empty());
}

/// Review A-H2 / B-M1: fail closed on the allow list; aliases and other
/// names are checked; the unit must be loaded.
#[test]
fn bind_is_fail_closed_on_the_allow_list_and_resolves_aliases() {
    let mut aliased = unit("k2-watch.service");
    aliased.names = vec!["k2-watch.service".into(), "sshd.service".into()];
    let mut masked = unit("k2-ops-push.service");
    masked.load = "masked";
    let mut alias_of_daemon = unit("k2-daemon.service");
    alias_of_daemon.names = vec!["k2-daemon.service".into(), "k2-control-plane.service".into()];
    let units = vec![aliased, masked, alias_of_daemon, unit("other.service")];
    // No allow-list file: nothing is allowed.
    let t = creds_box_with(255, units.clone(), None, false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    let o = t.call(&["creds", "bind", "other.service", "ingest_key"]);
    assert_eq!(o.code, exit::ARGV, "{}", o.stderr);
    assert!(o.stderr.contains("allow list"), "{}", o.stderr);
    let t = creds_box_with(255, units, Some(&format!("{OPS}other.service\n")), false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    // A unit with a denied alias among its names.
    let o = t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]);
    assert_eq!(o.code, exit::ARGV, "{}", o.stderr);
    assert!(o.stderr.contains("one of its names"), "{}", o.stderr);
    // An allowed name that is an alias of K2's daemon: Id differs.
    let o = t.call(&["creds", "bind", "k2-control-plane.service", "ingest_key"]);
    assert_eq!(o.code, exit::ARGV, "{}", o.stderr);
    assert!(o.stderr.contains("alias"), "{}", o.stderr);
    let o = t.call(&["creds", "bind", "k2-ops-push.service", "ingest_key"]);
    assert_eq!(o.code, exit::STATE, "masked: {}", o.stderr);
    let o = t.call(&["creds", "bind", "nosuch.service", "ingest_key"]);
    assert_ne!(o.code, exit::OK, "{}", o.stderr);
    ok(&t.call(&["creds", "bind", "other.service", "ingest_key"]));
    // A group-writable allow list is refused outright.
    std::fs::set_permissions(t.p(ALLOW), std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(t.call(&["creds", "bind", "other.service", "ingest_key"]).code, exit::STATE);
    std::fs::set_permissions(t.p(ALLOW), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(t.p(ALLOW), "foo@.service\n").unwrap();
    assert_eq!(t.call(&["creds", "bind", "other.service", "ingest_key"]).code, exit::STATE, "a bad line refuses the file");
}

/// Review A-M1: a credential ID the unit already gets elsewhere.
#[test]
fn bind_refuses_a_credential_id_the_unit_already_loads() {
    let mut u = unit("k2-watch.service");
    u.creds = "LoadCredential=ingest_key:/etc/ops/ingest.key\n".into();
    let mut g = unit("k2-ops-push.service");
    g.creds = "ImportCredential=ingest*\n".into();
    let mut s2 = unit("k2-control-plane.service");
    s2.creds = "SetCredential=ingest_key:literal\n".into();
    let t = creds_box_with(255, vec![u, g, s2], Some(OPS), false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    for name in ["k2-watch.service", "k2-ops-push.service", "k2-control-plane.service"] {
        let o = t.call(&["creds", "bind", name, "ingest_key"]);
        assert_eq!(o.code, exit::STATE, "{name}: {}", o.stderr);
        assert!(o.stderr.contains("already gets a credential"), "{}", o.stderr);
    }
    assert!(!t.p(DROPIN).exists());
    assert_eq!(count(&t, SYSTEMCTL, "daemon-reload"), 0);
}

#[test]
fn bind_writes_the_canned_drop_in_reloads_once_verifies_and_restart_is_rate_limited() {
    let t = creds_box(255, &["k2-watch.service", "k2-control-plane.service", "k2-ops-push.service"], false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    let v = json(&t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]));
    assert_eq!((v["changed"].as_bool(), v["dropin"].as_str()), (Some(true), Some(DROPIN)));
    assert_eq!(
        std::fs::read_to_string(t.p(DROPIN)).unwrap(),
        "# Written by K2 (k2 vault systemd-cred). K2 rewrites or removes this file; don't edit it.\n\
         [Service]\nLoadCredentialEncrypted=ingest_key:/etc/k2/credstore.encrypted/ingest_key.cred\n",
        "the drop-in bytes, exactly; the credential by absolute path"
    );
    assert_eq!(std::fs::metadata(t.p(DROPIN)).unwrap().permissions().mode() & 0o777, 0o644);
    assert_eq!(count(&t, SYSTEMCTL, "daemon-reload"), 1);
    assert_eq!(state(&t)["dropins"][0]["owner"], "k2");
    assert_eq!(json(&t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]))["changed"], false);
    assert_eq!(count(&t, SYSTEMCTL, "daemon-reload"), 1);
    std::fs::write(t.p(DROPIN), "[Service]\nExecStartPre=/bin/sh -c id\n").unwrap();
    assert_eq!(json(&t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]))["changed"], true, "a hand edit is put back");
    let v = json(&t.call(&["creds", "restart", "k2-watch.service"]));
    assert_eq!(v["restart"], "try-restart");
    assert_eq!(restarts(&t), vec![vec!["--no-block".to_string(), "try-restart".into(), "k2-watch.service".into()]], "try-restart only, never blocking");
    let o = t.call(&["creds", "restart", "k2-watch.service"]);
    assert_eq!(o.code, exit::STATE, "a second restart within 30 s: {}", o.stderr);
    assert_eq!(restarts(&t).len(), 1);
    let s = json(&t.call(&["creds", "status"]));
    assert_eq!(s["dropins"][0]["dropin"], "canned");
    assert_eq!(s["creds"][0]["matches"], true);
    assert_eq!(s["allowedUnits"].as_array().unwrap().len(), 3);
}

/// An admin drop-in that sorts after K2's would win: bind checks systemd's
/// effective source and takes its own drop-in back out.
#[test]
fn bind_takes_its_drop_in_back_when_systemd_still_loads_another_source() {
    let t = creds_box(255, &["k2-watch.service"], false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    mkdirs(&t.root, &["/etc/systemd/system/k2-watch.service.d"]);
    std::fs::write(
        t.p("/etc/systemd/system/k2-watch.service.d/zz-admin.conf"),
        "[Service]\nLoadCredentialEncrypted=ingest_key:/etc/ops/other.cred\n",
    )
    .unwrap();
    let o = t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(!t.p(DROPIN).exists(), "K2's drop-in was taken back out");
    assert!(state(&t)["dropins"].as_array().unwrap().is_empty(), "no phantom drop-in record");
    // ...so remove (the route's rollback) succeeds.
    let v = json(&t.call(&["creds", "remove", "ingest_key"]));
    assert_eq!(v["removed"], true);
}

#[test]
fn bind_refuses_a_missing_cred_an_unknown_unit_and_a_foreign_drop_in() {
    let t = creds_box(255, &["k2-watch.service"], false);
    assert_eq!(t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]).code, exit::STATE, "no cred yet");
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    mkdirs(&t.root, &["/etc/systemd/system/k2-watch.service.d"]);
    std::fs::write(t.p(DROPIN), "theirs").unwrap();
    assert_eq!(t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]).code, exit::STATE);
    assert_eq!(std::fs::read_to_string(t.p(DROPIN)).unwrap(), "theirs");
    assert_eq!(t.call(&["creds", "unbind", "k2-watch.service", "ingest_key"]).code, exit::STATE);
    assert!(t.p(DROPIN).exists());
    assert_eq!(t.call(&["creds", "restart", "k2-watch.service"]).code, exit::STATE, "no K2 binding on that unit");
    assert!(restarts(&t).is_empty());
}

/// Review A-H1: one daemon never touches another daemon's entries.
#[test]
fn another_daemon_cannot_overwrite_unbind_remove_or_restart_your_credentials() {
    let t = creds_box(255, &["k2-watch.service"], false);
    add_daemon_b(&t);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    ok(&t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]));
    as_daemon_b(&t);
    for (argv, stdin) in [
        (&["creds", "put", "ingest_key"][..], VALUE),
        (&["creds", "bind", "k2-watch.service", "ingest_key"], b"".as_slice()),
        (&["creds", "unbind", "k2-watch.service", "ingest_key"], b""),
        (&["creds", "restart", "k2-watch.service"], b""),
        (&["creds", "remove", "ingest_key"], b""),
    ] {
        let o = t.call_in(argv, stdin);
        assert_eq!(o.code, exit::STATE, "{argv:?}: {}", o.stderr);
    }
    let s = json(&t.call(&["creds", "status"]));
    assert!(s["creds"].as_array().unwrap().is_empty() && s["dropins"].as_array().unwrap().is_empty(), "B sees none of A's: {s}");
    assert_eq!(std::fs::read(t.p(CRED)).unwrap(), CIPHER);
    assert!(t.p(DROPIN).exists());
    assert!(restarts(&t).is_empty());
    as_daemon_a(&t);
    ok(&t.call(&["creds", "unbind", "k2-watch.service", "ingest_key"]));
    ok(&t.call(&["creds", "remove", "ingest_key"]));
}

#[test]
fn a_symlinked_drop_in_dir_is_refused() {
    let t = creds_box(255, &["k2-watch.service"], false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    mkdirs(&t.root, &["/elsewhere"]);
    symlink(t.p("/elsewhere"), t.p("/etc/systemd/system/k2-watch.service.d")).unwrap();
    assert_ne!(t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]).code, exit::OK);
    assert!(std::fs::read_dir(t.p("/elsewhere")).unwrap().next().is_none());
}

#[test]
fn unbind_and_remove_undo_only_what_k2_wrote() {
    let t = creds_box(255, &["k2-watch.service"], false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    ok(&t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]));
    assert_eq!(t.call(&["creds", "remove", "ingest_key"]).code, exit::STATE, "still bound");
    assert!(t.p(CRED).exists());
    assert_eq!(json(&t.call(&["creds", "unbind", "k2-watch.service", "ingest_key"]))["changed"], true);
    assert!(!t.p(DROPIN).exists());
    assert_eq!(json(&t.call(&["creds", "unbind", "k2-watch.service", "ingest_key"]))["changed"], false);
    assert_eq!(json(&t.call(&["creds", "remove", "ingest_key"]))["removed"], true);
    assert!(!t.p(CRED).exists());
    let st = state(&t);
    assert!(st["creds"].as_array().unwrap().is_empty() && st["dropins"].as_array().unwrap().is_empty(), "{st}");
    assert_eq!(json(&t.call(&["creds", "remove", "ingest_key"]))["removed"], false);
}

#[test]
fn a_doctored_state_file_fails_closed() {
    let t = creds_box(255, &["k2-watch.service"], false);
    mkdirs(&t.root, &["/var/lib/k2-root-helper"]);
    std::fs::write(t.p(STATE), r#"{"creds": [{"name": "../../etc/shadow", "owner": "k2", "sha256": "x"}], "dropins": []}"#).unwrap();
    assert_eq!(t.call_in(&["creds", "put", "ingest_key"], VALUE).code, exit::STATE);
    std::fs::write(t.p(STATE), r#"{"creds": [], "dropins": [], "extra": 1}"#).unwrap();
    assert_eq!(t.call_in(&["creds", "put", "ingest_key"], VALUE).code, exit::STATE, "unknown fields refused");
    assert_eq!(t.call(&["creds", "remove", "../../etc/shadow"]).code, exit::ARGV);
}

#[test]
fn version_announces_creds_at_protocol_10_and_the_old_state_file_is_untouched() {
    let t = creds_box(255, &["k2-watch.service"], false);
    let v = json(&t.call(&["helper", "version"]));
    assert_eq!(v["protocol"], 10);
    assert_eq!(v["namespaces"]["creds"], true);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    assert!(!t.p(STATE_JSON).exists() || !std::fs::read_to_string(t.p(STATE_JSON)).unwrap().contains("creds"));
}

/// Review B2-N3: files a crash left between a write and its record.
#[test]
fn status_lists_orphans_with_their_cleanup() {
    let t = creds_box(255, &["k2-watch.service"], false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    std::fs::write(t.p("/etc/k2/credstore.encrypted/lost.cred"), b"x").unwrap();
    mkdirs(&t.root, &["/etc/systemd/system/k2-ops-push.service.d"]);
    std::fs::write(t.p("/etc/systemd/system/k2-ops-push.service.d/k2-cred-lost.conf"), b"x").unwrap();
    std::fs::write(t.p("/etc/systemd/system/k2-ops-push.service.d/admin.conf"), b"x").unwrap();
    let s = json(&t.call(&["creds", "status"]));
    let o = s["orphans"].as_array().unwrap();
    assert_eq!(o.len(), 2, "{s}");
    assert_eq!(o[0]["cleanup"], "rm -- '/etc/k2/credstore.encrypted/lost.cred'");
    assert_eq!(o[1]["cleanup"], "rm -- '/etc/systemd/system/k2-ops-push.service.d/k2-cred-lost.conf' && systemctl daemon-reload");
    // A name K2 never writes gets no command to paste (A3-P4).
    std::fs::write(t.p("/etc/k2/credstore.encrypted/x$(reboot).cred"), b"x").unwrap();
    let s = json(&t.call(&["creds", "status"]));
    let odd = s["orphans"].as_array().unwrap().iter().find(|o| o["path"].as_str().unwrap().contains("reboot")).cloned().unwrap();
    assert!(!odd["cleanup"].as_str().unwrap().contains("rm"), "{odd}");
}

/// Review A3-P1: an inactive, unreferenced unit (garbage-collected, so
/// GetUnit fails) still binds: the helper asks LoadUnit.
#[test]
fn a_garbage_collected_unit_still_binds_through_load_unit() {
    let mut u = unit("k2-watch.service");
    u.gc = true;
    let t = creds_box_with(255, vec![u], Some(OPS), false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    ok(&t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]));
    assert!(t.p(DROPIN).exists());
    let calls: Vec<Vec<String>> = creds_calls(&t).into_iter().filter(|(p, _, _)| p == k2_root_proto::creds::BUSCTL).map(|(_, a, _)| a).collect();
    assert!(calls.iter().any(|a| a.get(5).map(String::as_str) == Some("LoadUnit")), "{calls:?}");
    assert!(calls.iter().all(|a| a.get(5).map(String::as_str) != Some("GetUnit")), "never GetUnit: {calls:?}");
}

/// Review B2-N5: the allow list's directory is checked too.
#[test]
fn a_writable_allow_list_directory_refuses() {
    let t = creds_box(255, &["k2-watch.service"], false);
    ok(&t.call_in(&["creds", "put", "ingest_key"], VALUE));
    std::fs::set_permissions(t.p("/etc/k2-root-helper"), std::fs::Permissions::from_mode(0o777)).unwrap();
    let o = t.call(&["creds", "bind", "k2-watch.service", "ingest_key"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(!t.p(DROPIN).exists());
}
