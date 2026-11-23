//! Fake door / fake heal ops only. Nothing here runs sudo or the network.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use k2_root_proto::keys::TEST_KEY;
use k2_root_proto::sig::trusted_comment;
use k2_root_proto::testsign::{fake_elf, sign_with_test_key};
use k2_root_proto::verbs::Verb;
use k2_root_proto::version::format_marker;

use super::heal::{heal_with, FetchError, HealOps, Outcome, Plan};
use super::*;

struct FakeDoor {
    present: bool,
    allowed: bool,
    version: Value,
    calls: Mutex<Vec<Vec<String>>>,
    answer: CallOut,
}

impl Door for FakeDoor {
    fn exists(&self, _p: &str) -> bool {
        self.present
    }
    fn sudo_allowed(&self, _p: &str) -> bool {
        self.allowed
    }
    fn call(&self, _p: &str, argv: &[String], _stdin: Option<&[u8]>) -> std::io::Result<CallOut> {
        self.calls.lock().unwrap().push(argv.to_vec());
        if argv == ["helper", "version"] {
            return Ok(CallOut {
                code: 0,
                stdout: self.version.to_string().into_bytes(),
                stderr: String::new(),
            });
        }
        Ok(self.answer.clone())
    }
}

fn door(present: bool, allowed: bool) -> FakeDoor {
    FakeDoor {
        present,
        allowed,
        version: json!({"name": "k2-root-helper", "version": "0.45.5", "protocol": 4, "packageOwned": false, "path": HELPER_PATH}),
        calls: Mutex::new(Vec::new()),
        answer: CallOut {
            code: 0,
            stdout: b"{}".to_vec(),
            stderr: String::new(),
        },
    }
}

#[test]
fn probe_order_root_missing_not_allowed() {
    let d = door(true, true);
    let st = probe_with(&d);
    assert_eq!(st.kind, Kind::Root);
    assert_eq!(st.protocol, 4);
    assert_eq!(st.version.as_deref(), Some("0.45.5"));
    assert!(st.usable(KEEP_MIN_PROTOCOL));

    let st = probe_with(&door(false, true));
    assert_eq!(st.kind, Kind::Missing);
    assert!(!st.usable(UPGRADE_MIN_PROTOCOL));
    let d = door(true, false);
    let st = probe_with(&d);
    assert_eq!(st.kind, Kind::NotAllowed);
    assert!(
        d.calls.lock().unwrap().is_empty(),
        "never runs the helper when sudo -l says no"
    );
}

#[test]
fn caller_not_daemon_teaches_the_fix() {
    let argv = vec!["mail".to_string(), "remove".into(), "data".into()];
    let e = map_call(
        &argv,
        CallOut {
            code: 77,
            stdout: vec![],
            stderr: "k2-root-helper: caller_not_daemon".into(),
        },
    )
    .unwrap_err();
    assert!(
        e.contains("caller_not_daemon") && e.contains("install-root-helper.sh"),
        "{e}"
    );
    let e = map_call(
        &argv,
        CallOut {
            code: 2,
            stdout: vec![],
            stderr: "refused arguments".into(),
        },
    )
    .unwrap_err();
    assert!(e.contains("k2 root-helper update"), "{e}");
    // A refused INPUT (not an unknown verb) is reported as-is.
    let e = map_call(
        &argv,
        CallOut {
            code: 2,
            stdout: vec![],
            stderr: "k2-root-helper: install-bin: tarball sha256 mismatch".into(),
        },
    )
    .unwrap_err();
    assert!(
        e.contains("sha256 mismatch") && !e.contains("older than"),
        "{e}"
    );
}

#[test]
fn off_catalog_argv_is_never_sent() {
    let d = door(true, true);
    let err = call_with(&d, &["mail".into(), "remove".into(), "/etc".into()], None).unwrap_err();
    assert!(err.contains("not sent"));
    assert!(d.calls.lock().unwrap().is_empty());
}

/// Every protocol-2 vector the supervisor sends maps onto a catalog verb.
#[test]
fn every_legacy_mail_vector_translates() {
    let pw = "0123456789abcdef".repeat(4);
    let unit_pw = systemd_unit(Some(&pw));
    let unit = systemd_unit(None);
    let dropin = hardening_dropin();
    let cases: Vec<(Vec<&str>, Option<&[u8]>, Vec<&str>, Option<Vec<u8>>)> = vec![
        (vec!["ensure-user"], None, vec!["mail", "ensure-user"], None),
        (
            vec!["mkdir", "/etc/stalwart"],
            None,
            vec!["mail", "mkdir", "config"],
            None,
        ),
        (
            vec!["mkdir", "/var/lib/stalwart"],
            None,
            vec!["mail", "mkdir", "data"],
            None,
        ),
        (
            vec!["mkdir", "/var/log/stalwart"],
            None,
            vec!["mail", "mkdir", "log"],
            None,
        ),
        (
            vec!["mkdir", "/etc/systemd/system/stalwart.service.d"],
            None,
            vec!["mail", "mkdir", "dropin-dir"],
            None,
        ),
        (
            vec!["chown", "/etc/stalwart"],
            None,
            vec!["mail", "chown", "config"],
            None,
        ),
        (
            vec!["chown", "/var/lib/stalwart"],
            None,
            vec!["mail", "chown", "data"],
            None,
        ),
        (
            vec!["chown", "/var/log/stalwart"],
            None,
            vec!["mail", "chown", "log"],
            None,
        ),
        (
            vec!["write", "/etc/systemd/system/stalwart.service"],
            Some(unit_pw.as_bytes()),
            vec!["mail", "render-unit"],
            Some(pw.clone().into_bytes()),
        ),
        (
            vec!["write", "/etc/systemd/system/stalwart.service"],
            Some(unit.as_bytes()),
            vec!["mail", "render-unit"],
            Some(vec![]),
        ),
        (
            vec![
                "write",
                "/etc/systemd/system/stalwart.service.d/k2-hardening.conf",
            ],
            Some(dropin.as_bytes()),
            vec!["mail", "render-dropin"],
            None,
        ),
        (
            vec!["install-bin"],
            Some(b"tarball"),
            vec!["mail", "install-bin"],
            Some(b"tarball".to_vec()),
        ),
        (
            vec!["systemctl", "daemon-reload"],
            None,
            vec!["mail", "systemctl", "daemon-reload"],
            None,
        ),
        (
            vec!["systemctl", "enable", "--now", "stalwart"],
            None,
            vec!["mail", "systemctl", "enable-now"],
            None,
        ),
        (
            vec!["systemctl", "restart", "stalwart"],
            None,
            vec!["mail", "systemctl", "restart"],
            None,
        ),
        (
            vec!["systemctl", "stop", "stalwart"],
            None,
            vec!["mail", "systemctl", "stop"],
            None,
        ),
        (
            vec!["systemctl", "disable", "--now", "stalwart"],
            None,
            vec!["mail", "systemctl", "disable-now"],
            None,
        ),
        (
            vec!["remove", "/etc/systemd/system/stalwart.service"],
            None,
            vec!["mail", "remove", "unit"],
            None,
        ),
        (
            vec!["remove", "/etc/systemd/system/stalwart.service.d"],
            None,
            vec!["mail", "remove", "dropin-dir"],
            None,
        ),
        (
            vec!["remove", "/usr/local/bin/stalwart"],
            None,
            vec!["mail", "remove", "bin"],
            None,
        ),
        (
            vec!["remove", "/var/lib/stalwart"],
            None,
            vec!["mail", "remove", "data"],
            None,
        ),
        (
            vec!["remove", "/var/log/stalwart"],
            None,
            vec!["mail", "remove", "log"],
            None,
        ),
        (
            vec!["remove", "/etc/stalwart"],
            None,
            vec!["mail", "remove", "config"],
            None,
        ),
        (vec!["version"], None, vec!["helper", "version"], None),
        (vec!["usage"], None, vec!["mail", "usage"], None),
        // Protocol 17: Stalwart's tree for Stalwart alone.
        (vec!["tighten", "/etc/stalwart"], None, vec!["mail", "tighten", "config"], None),
        (vec!["tighten", "/var/lib/stalwart"], None, vec!["mail", "tighten", "data"], None),
        (vec!["tighten", "/var/log/stalwart"], None, vec!["mail", "tighten", "log"], None),
        (vec!["store-state"], None, vec!["mail", "store-state"], None),
        (
            vec!["snapshot-data"],
            None,
            vec!["mail", "snapshot-data"],
            None,
        ),
        (
            vec!["restore-data"],
            None,
            vec!["mail", "restore-data"],
            None,
        ),
    ];
    for (legacy, stdin, want, want_stdin) in cases {
        // The legacy helper accepts it (the supervisor really sends it) …
        assert!(
            crate::mail::helper::parse_argv(&legacy).is_ok(),
            "{legacy:?}"
        );
        let (argv, input) = mail_argv(&legacy, stdin).unwrap_or_else(|e| panic!("{legacy:?}: {e}"));
        assert_eq!(argv, want, "{legacy:?}");
        assert_eq!(input, want_stdin, "{legacy:?}");
        // … and the root verb is a catalog verb.
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert!(Verb::parse(&refs).is_ok(), "{argv:?}");
    }
}

#[test]
fn unit_and_dropin_bytes_must_be_canned() {
    let mut forged = systemd_unit(Some(&"a".repeat(64)));
    forged.push_str("ExecStartPre=/bin/sh -c id\n");
    assert!(mail_argv(&["write", STALWART_UNIT_PATH], Some(forged.as_bytes())).is_err());
    let err = mail_argv(
        &["write", STALWART_UNIT_PATH],
        Some(systemd_unit(Some("SECRET-CANARY")).as_bytes()),
    )
    .unwrap_err();
    assert!(!err.contains("CANARY"));
    assert!(mail_argv(
        &["write", STALWART_DROPIN_PATH],
        Some(b"[Service]\nUser=root\n")
    )
    .is_err());
    assert!(mail_argv(&["chown", STALWART_DROPIN_DIR], None).is_err());
    assert!(mail_argv(&["tighten", STALWART_DROPIN_DIR], None).is_err());
    assert!(mail_argv(&["tighten", "/etc"], None).is_err());
    assert!(mail_argv(&["systemctl", "restart", "k2-daemon"], None).is_err());
}

#[test]
fn pg_vectors_translate() {
    for legacy in [
        vec!["systemctl", "enable", "--now", "postgresql"],
        vec!["systemctl", "disable", "--now", "postgresql"],
        vec!["systemctl", "daemon-reload"],
        vec!["systemctl", "restart", "postgresql"],
        vec!["install-ram-fence"],
        vec!["psql", "-d", "postgres", "-tA"],
        vec!["psql", "-d", "ws_abc", "-v", "ON_ERROR_STOP=1", "-tA"],
        vec!["psql", "-d", "myapp_test", "-v", "ON_ERROR_STOP=1"],
    ] {
        let v = pg_argv(&legacy).unwrap_or_else(|e| panic!("{legacy:?}: {e}"));
        assert_eq!(v[0], "pg");
        assert_eq!(
            &v[1..],
            legacy
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .as_slice()
        );
    }
    for bad in [
        vec!["psql", "-c", "select 1"],
        vec!["psql", "-d", "x;y"],
        vec!["systemctl", "start", "k2-daemon"],
    ] {
        assert!(pg_argv(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn own_unit_from_cgroup() {
    assert_eq!(
        parse_own_unit("0::/system.slice/k2-daemon.service\n"),
        Some(("k2-daemon.service".into(), "system".into()))
    );
    assert_eq!(
        parse_own_unit(
            "0::/user.slice/user-1001.slice/user@1001.service/app.slice/k2-daemon.service\n"
        ),
        Some(("k2-daemon.service".into(), "user".into()))
    );
    assert_eq!(
        parse_own_unit("0::/user.slice/user-1001.slice/session-3.scope\n"),
        None
    );
    assert_eq!(parse_own_unit(""), None);
    let cmd = install_command_for("0.45.5", "k2", "k2-daemon.service", "system");
    assert!(
        cmd.contains("/v0.45.5/install-root-helper.sh")
            && cmd.contains("--unit k2-daemon.service --scope system")
    );
}

// ── heal ────────────────────────────────────────────────────────────────

struct FakeHeal {
    states: RefCell<Vec<RootHelperState>>,
    files: HashMap<String, Vec<u8>>,
    updates: RefCell<Vec<usize>>,
    rollbacks: RefCell<u32>,
    fetches: RefCell<Vec<String>>,
    /// `false` = the hosted-mail busy latch is held elsewhere.
    latch_free: bool,
    busy: RefCell<bool>,
    /// Was the latch held during self_update?
    busy_during_update: RefCell<bool>,
}

impl HealOps for FakeHeal {
    fn probe(&self) -> RootHelperState {
        let mut s = self.states.borrow_mut();
        if s.len() > 1 {
            s.remove(0)
        } else {
            s[0].clone()
        }
    }
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        self.fetches.borrow_mut().push(url.to_string());
        self.files.get(url).cloned().ok_or(FetchError::NotFound)
    }
    fn self_update(&self, payload: &[u8]) -> Result<Value, String> {
        *self.busy_during_update.borrow_mut() = *self.busy.borrow();
        self.updates.borrow_mut().push(payload.len());
        Ok(json!({"result": "updated"}))
    }
    fn self_rollback(&self) -> Result<Value, String> {
        *self.rollbacks.borrow_mut() += 1;
        Ok(json!({"result": "rolled_back"}))
    }
    fn begin_busy(&self) -> bool {
        if self.latch_free {
            *self.busy.borrow_mut() = true;
        }
        self.latch_free
    }
    fn end_busy(&self) {
        *self.busy.borrow_mut() = false;
    }
}

fn root_at(v: &str, protocol: u32) -> RootHelperState {
    RootHelperState {
        kind: Kind::Root,
        version: Some(v.into()),
        protocol,
        ..RootHelperState::missing()
    }
}

const BASE: &str = "https://example.invalid/download";

fn plan() -> Plan {
    Plan {
        daemon_version: "0.45.6".into(),
        arch: "x86_64".into(),
        base_url: BASE.into(),
        airgap: false,
        keys: vec![TEST_KEY],
    }
}

fn signed_assets(version: &str) -> HashMap<String, Vec<u8>> {
    let elf = fake_elf("x86_64", &format_marker(version, 4, "x86_64"));
    let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", version, 4));
    let url = format!("{BASE}/v0.45.6/k2-root-helper-linux-x86_64");
    HashMap::from([(url.clone(), elf), (format!("{url}.sig"), sig.into_bytes())])
}

fn fake(states: Vec<RootHelperState>, files: HashMap<String, Vec<u8>>) -> FakeHeal {
    FakeHeal {
        states: RefCell::new(states),
        files,
        updates: RefCell::default(),
        rollbacks: RefCell::default(),
        fetches: RefCell::default(),
        latch_free: true,
        busy: RefCell::default(),
        busy_during_update: RefCell::default(),
    }
}

#[test]
fn heal_updates_an_older_helper_and_rechecks() {
    let f = fake(
        vec![root_at("0.45.5", 4), root_at("0.45.6", 4)],
        signed_assets("0.45.6"),
    );
    assert_eq!(
        heal_with(&f, &plan()),
        Outcome::Updated {
            from: "0.45.5".into(),
            to: "0.45.6".into()
        }
    );
    assert_eq!(f.updates.borrow().len(), 1);
    assert_eq!(*f.rollbacks.borrow(), 0);
}

#[test]
fn heal_rolls_back_when_the_new_helper_answers_wrong() {
    let f = fake(
        vec![root_at("0.45.5", 4), root_at("0.45.5", 4)],
        signed_assets("0.45.6"),
    );
    let out = heal_with(&f, &plan());
    assert!(matches!(out, Outcome::RolledBack { .. }), "{out:?}");
    assert_eq!(*f.rollbacks.borrow(), 1);
}

#[test]
fn heal_skips_and_refusals() {
    // Up to date: no fetch.
    let f = fake(vec![root_at("0.45.6", 4)], HashMap::new());
    assert!(matches!(heal_with(&f, &plan()), Outcome::UpToDate { .. }));
    assert!(f.fetches.borrow().is_empty());
    // Newer helper than the daemon (daemon rolled back): fine, no downgrade.
    let f = fake(vec![root_at("0.46.0", 4)], HashMap::new());
    assert!(matches!(heal_with(&f, &plan()), Outcome::UpToDate { .. }));
    // Air-gap: never fetches.
    let f = fake(vec![root_at("0.45.5", 4)], signed_assets("0.45.6"));
    let mut p = plan();
    p.airgap = true;
    assert!(matches!(heal_with(&f, &p), Outcome::Skipped { .. }));
    assert!(f.fetches.borrow().is_empty());
    // Package-owned.
    let mut st = root_at("0.45.5", 4);
    st.package_owned = true;
    assert!(matches!(
        heal_with(&fake(vec![st], HashMap::new()), &plan()),
        Outcome::Skipped { .. }
    ));
    // Missing / legacy: one root step.
    assert!(matches!(
        heal_with(
            &fake(vec![RootHelperState::missing()], HashMap::new()),
            &plan()
        ),
        Outcome::NeedsRoot { .. }
    ));
    assert!(matches!(
        heal_with(&fake(vec![root_at("0.45.0", 2)], HashMap::new()), &plan()),
        Outcome::NeedsRoot { .. }
    ));
    // 404 (dev build): retry, no root call.
    let f = fake(vec![root_at("0.45.5", 4)], HashMap::new());
    assert!(matches!(heal_with(&f, &plan()), Outcome::Retry { .. }));
    assert!(f.updates.borrow().is_empty());
    // Signed by an untrusted key (release keys empty): pre-verify refuses, no root call.
    let f = fake(vec![root_at("0.45.5", 4)], signed_assets("0.45.6"));
    let mut p = plan();
    p.keys = vec![];
    assert!(matches!(heal_with(&f, &p), Outcome::Retry { .. }));
    assert!(f.updates.borrow().is_empty());
    // A validly signed but different version.
    let f = fake(vec![root_at("0.45.5", 4)], signed_assets("0.45.7"));
    assert!(matches!(heal_with(&f, &plan()), Outcome::Retry { .. }));
    assert!(f.updates.borrow().is_empty());
}

/// Signed assets for `version` whose ELF is padded to `elf_len` bytes.
fn signed_assets_sized(version: &str, elf_len: usize) -> HashMap<String, Vec<u8>> {
    let mut elf = fake_elf("x86_64", &format_marker(version, 4, "x86_64"));
    assert!(elf_len >= elf.len());
    elf.resize(elf_len, 0);
    let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", version, 4));
    let url = format!("{BASE}/v{version}/k2-root-helper-linux-x86_64");
    HashMap::from([(url.clone(), elf), (format!("{url}.sig"), sig.into_bytes())])
}

fn plan_for(version: &str) -> Plan {
    Plan {
        daemon_version: version.into(),
        ..plan()
    }
}

#[test]
fn heal_sends_a_pre_0_46_18_helper_nothing_over_its_8_mib_cap() {
    use k2_root_proto::verbs::{CAP_SELF_UPDATE, CAP_SELF_UPDATE_LEGACY};
    // 0.46.17's real x86_64 helper: 8,836,200 bytes. A 0.46.16 helper
    // refuses it, so heal refuses first, without a root call, and names
    // the installer.
    let f = fake(vec![root_at("0.46.16", 18)], signed_assets_sized("0.46.17", 8_836_200));
    let out = heal_with(&f, &plan_for("0.46.17"));
    let Outcome::Retry { reason } = &out else { panic!("{out:?}") };
    assert!(reason.contains("install-root-helper.sh as root"), "{reason}");
    assert!(reason.contains("/v0.46.17/install-root-helper.sh"), "{reason}");
    assert!(reason.contains(&CAP_SELF_UPDATE_LEGACY.to_string()), "{reason}");
    assert!(reason.contains("(v0.46.16)"), "{reason}");
    assert!(f.updates.borrow().is_empty(), "no self-update was sent");
    // Unknown version: treated as old.
    let mut st = root_at("0.46.16", 18);
    st.version = None;
    let f = fake(vec![st], signed_assets_sized("0.46.17", 8_836_200));
    assert!(matches!(heal_with(&f, &plan_for("0.46.17")), Outcome::Retry { .. }));
    assert!(f.updates.borrow().is_empty());

    // Under 8 MiB with the signature: an old helper takes it.
    let elf_len = CAP_SELF_UPDATE_LEGACY - 4096;
    let f = fake(
        vec![root_at("0.46.16", 18), root_at("0.46.18", 18)],
        signed_assets_sized("0.46.18", elf_len),
    );
    assert_eq!(
        heal_with(&f, &plan_for("0.46.18")),
        Outcome::Updated { from: "0.46.16".into(), to: "0.46.18".into() }
    );
    let sent = f.updates.borrow()[0];
    assert!(sent <= CAP_SELF_UPDATE_LEGACY && sent > elf_len, "{sent}");

    // One byte over 8 MiB, payload and all: refused for an old helper...
    let sig_len = {
        let a = signed_assets_sized("0.46.18", elf_len);
        a[&format!("{BASE}/v0.46.18/k2-root-helper-linux-x86_64.sig")].len()
    };
    let over = CAP_SELF_UPDATE_LEGACY - 4 - sig_len + 1;
    let f = fake(vec![root_at("0.46.17", 18)], signed_assets_sized("0.46.18", over));
    assert!(matches!(heal_with(&f, &plan_for("0.46.18")), Outcome::Retry { .. }));
    assert!(f.updates.borrow().is_empty());
    // ...exactly 8 MiB goes.
    let f = fake(
        vec![root_at("0.46.17", 18), root_at("0.46.18", 18)],
        signed_assets_sized("0.46.18", over - 1),
    );
    assert!(matches!(heal_with(&f, &plan_for("0.46.18")), Outcome::Updated { .. }));
    assert_eq!(f.updates.borrow()[0], CAP_SELF_UPDATE_LEGACY);

    // A 0.46.18+ helper takes the big cap: 9 MiB goes.
    let f = fake(
        vec![root_at("0.46.18", 19), root_at("0.46.19", 19)],
        signed_assets_sized("0.46.19", 9 * 1024 * 1024),
    );
    assert!(matches!(heal_with(&f, &plan_for("0.46.19")), Outcome::Updated { .. }));
    // ...but not over 32 MiB.
    let f = fake(vec![root_at("0.46.18", 19)], signed_assets_sized("0.46.19", CAP_SELF_UPDATE));
    let out = heal_with(&f, &plan_for("0.46.19"));
    let Outcome::Retry { reason } = &out else { panic!("{out:?}") };
    assert!(reason.contains(&CAP_SELF_UPDATE.to_string()), "{reason}");
    assert!(f.updates.borrow().is_empty());
}

#[test]
fn heal_takes_the_mail_latch_only_for_the_install_window() {
    // Up to date: never asks for the latch.
    let mut f = fake(vec![root_at("0.45.6", 4)], HashMap::new());
    f.latch_free = false;
    assert!(matches!(heal_with(&f, &plan()), Outcome::UpToDate { .. }));
    // Latch held by a mail window: downloaded + verified, not installed.
    let mut f = fake(vec![root_at("0.45.5", 4)], signed_assets("0.45.6"));
    f.latch_free = false;
    assert_eq!(heal_with(&f, &plan()), Outcome::Deferred);
    assert!(f.updates.borrow().is_empty());
    // Latch free: held exactly around self-update + re-check, then released.
    let f = fake(
        vec![root_at("0.45.5", 4), root_at("0.45.6", 4)],
        signed_assets("0.45.6"),
    );
    assert!(matches!(heal_with(&f, &plan()), Outcome::Updated { .. }));
    assert!(
        *f.busy_during_update.borrow(),
        "self-update ran inside the latch"
    );
    assert!(!*f.busy.borrow(), "latch released");
}

#[test]
fn helper_needed_names_the_protocol_and_fix() {
    let m = helper_needed("keeper", KEEP_MIN_PROTOCOL, &RootHelperState::missing());
    assert!(
        m.starts_with("helper_needed: keeper needs k2-root-helper protocol 4"),
        "{m}"
    );
    assert!(m.contains("install-root-helper.sh"), "{m}");
}

#[test]
fn status_shape() {
    let v = status_json(&root_at("0.45.6", 4), "installed", false, None);
    assert_eq!(v["state"], "root");
    assert_eq!(v["protocol"], 4);
    assert_eq!(v["minProtocol"]["keep"], 4);
    assert!(v["installCommand"]
        .as_str()
        .unwrap()
        .contains("install-root-helper.sh"));
    // A leftover k2-mail-helper is reported but never used (retired at
    // install, 0.45.4): without the root helper the state is "missing".
    let v = status_json(&RootHelperState::missing(), "installed", false, None);
    assert_eq!(v["state"], "missing");
    assert_eq!(v["legacy"]["mail"], "installed");
    // Only the pg script still serves without the root helper.
    let v = status_json(&RootHelperState::missing(), "missing", true, None);
    assert_eq!(v["state"], "legacy");
    let v = status_json(&RootHelperState::missing(), "missing", false, None);
    assert_eq!(v["state"], "missing");
    let not_allowed = RootHelperState {
        kind: Kind::NotAllowed,
        ..RootHelperState::missing()
    };
    let v = status_json(&not_allowed, "installed", true, None);
    assert_eq!(
        v["state"], "not_allowed",
        "an installed-but-refused root helper is not 'legacy'"
    );
}

#[test]
fn auto_heal_needs_root_is_superseded_once_the_helper_is_installed() {
    use super::heal::{status_from, HealStatus};
    // The boot heal ran before the admin installed the helper.
    let before = HealStatus {
        last_attempt: Some(1_760_000_000),
        last_result: Some(Outcome::NeedsRoot {
            reason: "k2-root-helper is not installed".into(),
        }),
        next_attempt: Some(1_760_086_400),
    };
    // Live probe still says missing: the needs_root line stands.
    let v = status_from(&before, &RootHelperState::missing());
    assert_eq!(v["lastResult"]["result"], "needs_root");
    assert_eq!(v["lastResult"]["reason"], "k2-root-helper is not installed");
    assert!(v.get("superseded").is_none(), "{v}");

    // Installed now (protocol 4): no contradicting lastResult; the earlier
    // attempt is kept, labelled, with its time.
    let v = status_from(&before, &root_at("0.46.3", 4));
    assert!(v["lastResult"].is_null(), "{v}");
    assert_eq!(v["superseded"]["result"], "needs_root");
    assert_eq!(v["superseded"]["reason"], "k2-root-helper is not installed");
    assert_eq!(v["superseded"]["at"], 1_760_000_000);
    assert_eq!(
        v["superseded"]["why"],
        "the helper is installed now (v0.46.3, protocol 4)"
    );
    assert_eq!(v["lastAttempt"], 1_760_000_000);
    assert_eq!(v["nextAttempt"], 1_760_086_400);

    // Installed but too old to self-update (protocol 2): still needs root.
    let v = status_from(&before, &root_at("0.45.0", 2));
    assert_eq!(v["lastResult"]["result"], "needs_root");
    assert!(v.get("superseded").is_none(), "{v}");

    // Installed but sudoers refuses it: still needs root.
    let not_allowed = RootHelperState {
        kind: Kind::NotAllowed,
        protocol: 4,
        ..RootHelperState::missing()
    };
    let v = status_from(&before, &not_allowed);
    assert_eq!(v["lastResult"]["result"], "needs_root");

    // Other results are never rewritten, even with a healthy helper.
    let retry = HealStatus {
        last_result: Some(Outcome::Retry {
            reason: "offline".into(),
        }),
        ..before.clone()
    };
    let v = status_from(&retry, &root_at("0.46.3", 4));
    assert_eq!(v["lastResult"]["result"], "retry");
    assert!(v.get("superseded").is_none(), "{v}");

    // No heal yet: nulls, no superseded.
    let v = status_from(&HealStatus::default(), &root_at("0.46.3", 4));
    assert!(v["lastResult"].is_null());
    assert!(v.get("superseded").is_none(), "{v}");
}

// ── /boot-status `rootHelper` (fleet monitoring) ─────────────────────

fn seen(st: RootHelperState, legacy_pg: bool) -> Seen {
    Seen { st, legacy_pg }
}

fn never_airgap() -> bool {
    false
}

fn airgap_on() -> bool {
    true
}

/// Shape: off Linux null; on Linux exactly `{state, protocol, version}`,
/// with the words `k2 root-helper status` uses.
#[test]
fn boot_status_field_has_the_status_shape_and_is_null_off_linux() {
    let root = seen(root_at("0.46.13", 13), false);
    assert_eq!(boot_status_from(Some(&root), None, &never_airgap, "0.46.13", false), Value::Null);
    assert_eq!(boot_status_from(None, None, &never_airgap, "0.46.13", false), Value::Null);

    let v = boot_status_from(Some(&root), None, &never_airgap, "0.46.13", true);
    assert_eq!(v, json!({"state": "root", "protocol": 13, "version": "0.46.13"}));
    assert_eq!(v.as_object().map(|o| o.len()), Some(3), "{v}");

    // Before the first probe after boot.
    assert_eq!(
        boot_status_from(None, None, &never_airgap, "0.46.13", true),
        json!({"state": "unknown", "protocol": 0, "version": null})
    );

    let missing = seen(RootHelperState::missing(), false);
    assert_eq!(
        boot_status_from(Some(&missing), None, &never_airgap, "0.46.13", true),
        json!({"state": "missing", "protocol": 0, "version": null})
    );
    let legacy = seen(RootHelperState::missing(), true);
    assert_eq!(boot_status_from(Some(&legacy), None, &never_airgap, "0.46.13", true)["state"], "legacy");
    let not_allowed = seen(
        RootHelperState {
            kind: Kind::NotAllowed,
            ..RootHelperState::missing()
        },
        false,
    );
    assert_eq!(boot_status_from(Some(&not_allowed), None, &never_airgap, "0.46.13", true)["state"], "not_allowed");

    // The same words and numbers as `GET /cli/root-helper/status`.
    for s in [&root, &missing, &legacy, &not_allowed] {
        let status = status_json(&s.st, "absent", s.legacy_pg, None);
        let boot = boot_status_from(Some(s), None, &never_airgap, "0.46.13", true);
        assert_eq!(boot["state"], status["state"], "{boot} vs {status}");
        assert_eq!(boot["protocol"], status["protocol"]);
        assert_eq!(boot["version"], status["version"]);
    }
}

/// The finer states for a working helper: package, rolled_back, airgapped,
/// each one only while it is true.
#[test]
fn boot_status_field_names_package_rolled_back_and_airgapped() {
    let package = seen(
        RootHelperState {
            package_owned: true,
            ..root_at("0.46.1", 12)
        },
        false,
    );
    assert_eq!(boot_status_from(Some(&package), None, &never_airgap, "0.46.13", true)["state"], "package");

    let rolled = Outcome::RolledBack {
        from: "0.46.13".into(),
        to: "0.46.12".into(),
        reason: "after the update the helper answers None".into(),
    };
    let behind = seen(root_at("0.46.12", 12), false);
    assert_eq!(
        boot_status_from(Some(&behind), Some(&rolled), &never_airgap, "0.46.13", true),
        json!({"state": "rolled_back", "protocol": 12, "version": "0.46.12"})
    );
    // An admin installed the current helper since: plain root again.
    let current = seen(root_at("0.46.13", 13), false);
    assert_eq!(boot_status_from(Some(&current), Some(&rolled), &never_airgap, "0.46.13", true)["state"], "root");

    assert_eq!(boot_status_from(Some(&behind), None, &airgap_on, "0.46.13", true)["state"], "airgapped");
    assert_eq!(boot_status_from(Some(&current), None, &airgap_on, "0.46.13", true)["state"], "root");
    assert_eq!(boot_status_from(Some(&behind), None, &never_airgap, "0.46.13", true)["state"], "root");
    // A dev build (no x.y.z) is never "behind".
    assert_eq!(boot_status_from(Some(&behind), None, &airgap_on, "dev", true)["state"], "root");
}

/// Fleet monitoring reads it: never a path or error text.
#[test]
fn boot_status_field_carries_no_path_or_error_text() {
    let st = RootHelperState {
        path: "/opt/private-place/k2-root-helper".into(),
        error: Some("helper version: exit 1: sudo: token=abc123".into()),
        ..root_at("0.46.13", 13)
    };
    let v = boot_status_from(Some(&seen(st, false)), None, &never_airgap, "0.46.13", true);
    let text = v.to_string();
    for leak in ["path", "error", "private-place", "abc123", "sudo"] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
}

/// Cached: the field reads the last probe and never probes itself (the
/// test seam says root while the last remembered probe says missing).
#[test]
fn boot_status_value_reads_the_last_probe_and_never_probes() {
    let k2 = env!("CARGO_PKG_VERSION");
    test_seam::set(Some(root_at(k2, 99)));
    remember_seen(seen(RootHelperState::missing(), false));
    let v = boot_status_value();
    if cfg!(target_os = "linux") {
        assert_eq!(v["state"], "missing", "{v}");
    } else {
        assert!(v.is_null(), "{v}");
    }
    // A status read (the seam) does not change it; a remembered probe does.
    let _ = state();
    assert_eq!(boot_status_value(), v);
    remember_seen(seen(root_at(k2, 99), false));
    let v = boot_status_value();
    if cfg!(target_os = "linux") {
        assert_eq!(v, json!({"state": "root", "protocol": 99, "version": k2}));
    } else {
        assert!(v.is_null(), "{v}");
    }
    test_seam::set(None);
}
