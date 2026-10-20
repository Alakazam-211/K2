//! `apps` namespace tests (protocol 6) over the temp root and fakes of
//! `tests.rs`. Nothing here runs useradd, nft, systemctl or apt for real:
//! the fake exec records argv and, where a real tool would change files,
//! a test effect changes the temp root the same way.

use std::os::unix::fs::{symlink, PermissionsExt};
use std::rc::Rc;

use k2_root_proto::apps::*;
use k2_root_proto::paths::{SYSTEMCTL, USERADD};
use k2_root_proto::verbs::exit;

use super::{box_root, ok, T};
use crate::ctx::ExecOut;

pub(super) const APP_UID: u32 = 996;

pub(super) fn write(t: &T, abs: &str, body: &str) {
    let p = t.p(abs);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

pub(super) fn read(t: &T, abs: &str) -> String {
    std::fs::read_to_string(t.p(abs)).unwrap()
}

pub(super) fn json(o: &crate::Outcome) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap()
}

/// /etc/passwd, group, shadow for a box with the daemon user `k2`.
pub(super) fn base_users(t: &T) {
    write(
        t,
        PASSWD_FILE,
        "root:x:0:0:root:/root:/bin/bash\nk2:x:1001:1001::/home/k2:/bin/bash\nnobody:x:65534:65534::/nonexistent:/usr/sbin/nologin\n",
    );
    write(
        t,
        GROUP_FILE,
        "root:x:0:\nsudo:x:27:k2\nk2:x:1001:\nnogroup:x:65534:\n",
    );
    write(
        t,
        SHADOW_FILE,
        "root:*:19000:0:99999:7:::\nk2:$6$hash:19000:0:99999:7:::\n",
    );
}

const APP_PW_LINE: &str = "k2app:x:996:996:K2 Apps:/var/lib/k2app:/usr/sbin/nologin\n";

/// The box after `apps ensure-user`.
pub(super) fn with_app_user(t: &T) {
    base_users(t);
    let mut pw = read(t, PASSWD_FILE);
    pw.push_str(APP_PW_LINE);
    write(t, PASSWD_FILE, &pw);
    let mut g = read(t, GROUP_FILE);
    g.push_str("k2app:x:996:\n");
    write(t, GROUP_FILE, &g);
    let mut s = read(t, SHADOW_FILE);
    s.push_str("k2app:!:19000::::::\n");
    write(t, SHADOW_FILE, &s);
    std::fs::create_dir_all(t.p(APP_HOME)).unwrap();
}

/// `useradd` appends k2app and makes its home, like the real one.
fn useradd_effect(t: &T) {
    let root = t.root.clone();
    t.exec.0.borrow_mut().effects.push((
        USERADD.into(),
        Rc::new(move |_args: &[String]| {
            let add = |abs: &str, line: &str| {
                let p = crate::ctx::under(&root, abs);
                let mut s = std::fs::read_to_string(&p).unwrap_or_default();
                s.push_str(line);
                std::fs::write(&p, s).unwrap();
            };
            add(PASSWD_FILE, APP_PW_LINE);
            add(GROUP_FILE, "k2app:x:996:\n");
            add(SHADOW_FILE, "k2app:!:19000::::::\n");
            let home = crate::ctx::under(&root, APP_HOME);
            std::fs::create_dir_all(&home).unwrap();
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
        }),
    ));
}

/// `usermod --add-subuids S-E --add-subgids S-E k2app` appends both lines.
fn usermod_effect(t: &T) {
    let root = t.root.clone();
    t.exec.0.borrow_mut().effects.push((
        USERMOD.into(),
        Rc::new(move |args: &[String]| {
            let spec = &args[1];
            let (s, e) = spec.split_once('-').unwrap();
            let (s, e): (u64, u64) = (s.parse().unwrap(), e.parse().unwrap());
            for f in [SUBUID_FILE, SUBGID_FILE] {
                let p = crate::ctx::under(&root, f);
                let mut txt = std::fs::read_to_string(&p).unwrap_or_default();
                txt.push_str(&format!("k2app:{s}:{}\n", e - s + 1));
                std::fs::write(&p, txt).unwrap();
            }
        }),
    ));
}

pub(super) fn rule(t: &T, prog: &str, tok: &str, status: i32, stdout: &str) {
    t.exec.0.borrow_mut().rules.push((
        prog.into(),
        tok.into(),
        ExecOut {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
        },
    ));
}

pub(super) fn calls_of(t: &T, prog: &str) -> Vec<String> {
    t.calls()
        .into_iter()
        .filter(|(p, _, _)| p == prog)
        .map(|(_, a, _)| a.join(" "))
        .collect()
}

fn reloads(t: &T) -> usize {
    calls_of(t, SYSTEMCTL)
        .iter()
        .filter(|c| *c == "daemon-reload")
        .count()
}

// ── user ────────────────────────────────────────────────────────────────

const PROC_UID: u32 = 995;
const PROC_PW_LINE: &str = "k2appx:x:995:995:K2 process Apps:/var/lib/k2appx:/usr/sbin/nologin\n";

/// The box after `apps ensure-process-user` (k2app first).
fn with_process_user(t: &T) {
    with_app_user(t);
    write(t, PASSWD_FILE, &(read(t, PASSWD_FILE) + PROC_PW_LINE));
    write(t, GROUP_FILE, &(read(t, GROUP_FILE) + "k2appx:x:995:\n"));
    write(t, SHADOW_FILE, &(read(t, SHADOW_FILE) + "k2appx:!:19000::::::\n"));
}

/// `nft list table inet k2app` answers with both users' jumps.
fn nft_lists_both(t: &T) {
    rule(
        t,
        NFT,
        "list",
        0,
        "table inet k2app {\n\tchain output {\n\t\tmeta skuid 996 jump k2app_v4\n\t\tmeta skuid 995 jump k2app_v4\n\t}\n\tchain k2app_v4 {\n\t}\n}\n",
    );
}

#[test]
fn ensure_process_user_is_its_own_system_user_never_k2app() {
    let t = box_root();
    // k2app first.
    base_users(&t);
    let o = t.call(&["apps", "ensure-process-user"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("box_not_prepared"), "{}", o.stderr);

    let t = box_root();
    with_app_user(&t);
    let root = t.root.clone();
    t.exec.0.borrow_mut().effects.push((
        USERADD.into(),
        Rc::new(move |_args: &[String]| {
            let add = |abs: &str, line: &str| {
                let p = crate::ctx::under(&root, abs);
                let mut s = std::fs::read_to_string(&p).unwrap_or_default();
                s.push_str(line);
                std::fs::write(&p, s).unwrap();
            };
            add(PASSWD_FILE, PROC_PW_LINE);
            add(GROUP_FILE, "k2appx:x:995:\n");
            add(SHADOW_FILE, "k2appx:!:19000::::::\n");
        }),
    ));
    let v = json(&t.call(&["apps", "ensure-process-user"]));
    assert_eq!(v["created"], true, "{v}");
    assert_eq!(v["uid"], PROC_UID);
    assert_eq!(
        calls_of(&t, USERADD),
        vec!["--system --user-group --home-dir /var/lib/k2appx --no-create-home --shell /usr/sbin/nologin --comment K2 process Apps k2appx"]
    );
    ok(&t.call(&["apps", "ensure-process-user"]));
    assert_eq!(calls_of(&t, USERADD).len(), 1, "once");
    assert!(!t.p(PROCESS_HOME).exists(), "systemd makes the per-App state dirs");

    // Bad shapes are refused, never adopted.
    for (line, grp) in [
        ("k2appx:x:996:996::/var/lib/k2appx:/usr/sbin/nologin\n", ""),
        ("k2appx:x:995:996::/var/lib/k2appx:/usr/sbin/nologin\n", ""),
        ("k2appx:x:995:995::/var/lib/k2app:/usr/sbin/nologin\n", ""),
        ("k2appx:x:995:995::/var/lib/k2appx:/bin/sh\n", ""),
        ("k2appx:x:1995:1995::/var/lib/k2appx:/usr/sbin/nologin\n", ""),
        ("k2appx:x:995:995::/var/lib/k2appx:/usr/sbin/nologin\n", "k2app:x:996:k2appx\n"),
        ("k2appx:x:995:995::/var/lib/k2appx:/usr/sbin/nologin\n", "video:x:44:\ndisk:x:6:k2appx\n"),
    ] {
        let t = box_root();
        with_app_user(&t);
        write(&t, PASSWD_FILE, &(read(&t, PASSWD_FILE) + line));
        write(&t, GROUP_FILE, &(read(&t, GROUP_FILE) + grp));
        write(&t, SHADOW_FILE, &(read(&t, SHADOW_FILE) + "k2appx:!:1::::::\n"));
        let o = t.call(&["apps", "ensure-process-user"]);
        assert_eq!(o.code, exit::STATE, "{line:?} {grp:?}: {}", o.stderr);
        assert!(o.stderr.starts_with("refused:"), "{}", o.stderr);
    }
}

#[test]
fn nft_covers_process_apps_once_their_user_exists() {
    let t = box_root();
    with_process_user(&t);
    ok(&t.call(&["apps", "nft", "apply", "38600-38899", "38900-38999", "5432"]));
    let fed: Vec<_> = t
        .calls()
        .into_iter()
        .filter(|(p, a, _)| p == NFT && a == &vec!["-f".to_string(), "-".to_string()])
        .collect();
    let want = nft_ruleset(APP_UID, Some(PROC_UID), GATEWAY_PORTS, PROXY_PORTS, DB_PORTS, &[]);
    assert_eq!(fed[0].2.as_deref(), Some(want.as_bytes()));
    assert!(want.contains("meta skuid 995 jump k2app_v4"));
}

#[test]
fn process_purge_removes_a_stopped_apps_state_only() {
    let t = box_root();
    with_process_user(&t);
    write(&t, "/var/lib/k2appx/jobs/data/db.sqlite", "x");
    write(&t, "/var/lib/k2appx/other/data/keep", "y");
    std::fs::create_dir_all(t.p("/etc/secret")).unwrap();
    symlink(t.p("/etc/secret"), t.p("/var/lib/k2appx/jobs/link")).unwrap();
    ok(&t.call(&["apps", "process", "caps", "jobs", "512", "-.-.-.-"]));
    rule(&t, SYSTEMCTL, "list-units", 0, "k2app-process@jobs.service loaded active running K2 App jobs (process)\n");
    let o = t.call(&["apps", "process", "purge", "jobs"]);
    assert_eq!(o.code, exit::STATE, "never while it runs: {}", o.stderr);
    t.exec.0.borrow_mut().rules.clear();
    let v = json(&t.call(&["apps", "process", "purge", "jobs"]));
    assert_eq!(v["purged"], true, "{v}");
    assert_eq!(v["capsRemoved"], true);
    assert!(!t.p("/var/lib/k2appx/jobs").exists());
    assert!(t.p("/etc/secret").exists(), "a link inside is removed, never followed");
    assert!(t.p("/var/lib/k2appx/other/data/keep").exists());
    t.as_agent();
    assert_eq!(t.call(&["apps", "process", "purge", "other"]).code, exit::NOT_DAEMON);
}

#[test]
fn ensure_user_creates_a_locked_system_user_once() {
    let t = box_root();
    base_users(&t);
    useradd_effect(&t);
    let o = t.call(&["apps", "ensure-user"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["created"], true);
    assert_eq!(v["uid"], APP_UID);
    assert_eq!(
        calls_of(&t, USERADD),
        vec!["--system --user-group --home-dir /var/lib/k2app --create-home --shell /usr/sbin/nologin --comment K2 Apps k2app"]
    );
    let mode = std::fs::metadata(t.p(APP_HOME))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o750, "home tightened to 0750");

    // Again: nothing runs.
    let o = t.call(&["apps", "ensure-user"]);
    ok(&o);
    assert_eq!(json(&o)["created"], false);
    assert_eq!(calls_of(&t, USERADD).len(), 1);
}

#[test]
fn ensure_user_refuses_every_bad_shape_and_creates_nothing() {
    let cases: &[(&str, &str, &str)] = &[
        // (passwd line, extra group line, shadow line)
        ("k2app:x:0:996::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        ("k2app:x:996:0::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        // The daemon user's uid (collision) — also a duplicate uid.
        ("k2app:x:1001:1001::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        // Same uid as another system user.
        ("k2app:x:996:996::/var/lib/k2app:/usr/sbin/nologin\nother:x:996:996::/:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        ("k2app:x:1500:1500::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        ("k2app:x:996:996::/var/lib/k2app:/bin/bash\n", "", "k2app:!:1::::::\n"),
        ("k2app:x:996:996::/home/k2app:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        ("k2app:x:996:996::/var/lib/k2app:/usr/sbin/nologin\n", "docker:x:998:k2app\n", "k2app:!:1::::::\n"),
        ("k2app:x:996:27::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app:!:1::::::\n"),
        ("k2app:x:996:996::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app:$6$abc:1::::::\n"),
        ("k2app:x:996:996::/var/lib/k2app:/usr/sbin/nologin\n", "", "k2app::1::::::\n"),
    ];
    for (pw, grp, sh) in cases {
        let t = box_root();
        base_users(&t);
        write(&t, PASSWD_FILE, &(read(&t, PASSWD_FILE) + pw));
        write(&t, GROUP_FILE, &(read(&t, GROUP_FILE) + grp));
        write(&t, SHADOW_FILE, &(read(&t, SHADOW_FILE) + sh));
        let o = t.call(&["apps", "ensure-user"]);
        assert_eq!(o.code, exit::STATE, "{pw:?} {grp:?} {sh:?}: {}", o.stderr);
        assert!(o.stderr.starts_with("refused:"), "{}", o.stderr);
        assert!(calls_of(&t, USERADD).is_empty());
        assert!(!t.p(APP_HOME).exists(), "no home made for a bad user");
    }
}

#[test]
fn ensure_user_never_adopts_an_orphan_or_linked_home() {
    // A home left by an older k2app (another uid): refused, untouched.
    let t = box_root();
    base_users(&t);
    useradd_effect(&t);
    std::fs::create_dir_all(t.p("/var/lib/k2app/apps/old")).unwrap();
    let o = t.call(&["apps", "ensure-user"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(calls_of(&t, USERADD).is_empty());

    // The home is a link to somewhere else: refused, the target untouched.
    let t = box_root();
    with_app_user(&t);
    std::fs::remove_dir(t.p(APP_HOME)).unwrap();
    std::fs::create_dir_all(t.p("/etc/elsewhere")).unwrap();
    std::fs::set_permissions(
        t.p("/etc/elsewhere"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink(t.p("/etc/elsewhere"), t.p(APP_HOME)).unwrap();
    let o = t.call(&["apps", "ensure-user"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    let mode = std::fs::metadata(t.p("/etc/elsewhere"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "never chmod through the link");
}

// ── subordinate ids ─────────────────────────────────────────────────────

#[test]
fn subid_allocates_one_aligned_block_above_everything_once() {
    let t = box_root();
    with_app_user(&t);
    usermod_effect(&t);
    write(&t, SUBUID_FILE, "admin:100000:65536\nbig:2000000:65536\n");
    write(&t, SUBGID_FILE, "admin:100000:65536\n");
    let o = t.call(&["apps", "subid"]);
    ok(&o);
    // Highest end 2065536 → next 65536-aligned start 2097152.
    let start = 2_097_152u64;
    let end = start + SUBID_COUNT - 1;
    assert_eq!(
        calls_of(&t, USERMOD),
        vec![format!(
            "--add-subuids {start}-{end} --add-subgids {start}-{end} k2app"
        )]
    );
    let v = json(&o);
    assert_eq!(v["changed"], true);
    assert_eq!(v["subuid"][0]["start"], start);
    assert_eq!(v["subgid"][0]["count"], SUBID_COUNT);

    // Again: nothing runs.
    let o = t.call(&["apps", "subid"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], false);
    assert_eq!(calls_of(&t, USERMOD).len(), 1);
}

#[test]
fn subid_starts_at_the_floor_on_an_empty_box() {
    let all = [crate::apps::Range {
        start: 100_000,
        count: 65_536,
    }];
    let b = crate::apps::next_block(&all, &[0, 1001, 65_534]).unwrap();
    assert_eq!(b.start, SUBID_FLOOR);
    // No room below 2^32-1: no block.
    let huge = [crate::apps::Range {
        start: 4_292_000_000,
        count: 1_000,
    }];
    assert!(crate::apps::next_block(&huge, &[]).is_none());
    // A real uid high up (a domain user) pushes the block above it.
    let b = crate::apps::next_block(&[], &[3_000_000]).unwrap();
    assert!(b.start > 3_000_000 && b.start.is_multiple_of(SUBID_ALIGN));
}

#[test]
fn subid_refuses_collisions_and_half_state() {
    let cases: &[(&str, &str)] = &[
        // Overlaps another user's range: that user could map onto App ids.
        (
            "admin:100000:65536\nk2app:150000:65536\n",
            "k2app:150000:65536\n",
        ),
        // Contains real uids (root, the daemon user).
        ("k2app:0:65536\n", "k2app:0:65536\n"),
        ("k2app:1000:4096\n", "k2app:1000:4096\n"),
        // Numeric owner form counts as k2app too.
        ("996:0:65536\n", "996:0:65536\n"),
        // Uids but no gids.
        ("k2app:2097152:65536\n", ""),
        // Past 2^32-1.
        ("k2app:4294900000:100000\n", "k2app:4294900000:100000\n"),
    ];
    for (su, sg) in cases {
        let t = box_root();
        with_app_user(&t);
        usermod_effect(&t);
        write(&t, SUBUID_FILE, su);
        write(&t, SUBGID_FILE, sg);
        let o = t.call(&["apps", "subid"]);
        assert_eq!(o.code, exit::STATE, "{su:?}/{sg:?}: {}", o.stderr);
        assert!(calls_of(&t, USERMOD).is_empty());
    }
}

#[test]
fn box_verbs_need_the_user_first() {
    let t = box_root();
    base_users(&t);
    for args in [
        &["apps", "subid"][..],
        &["apps", "linger", "on"],
        &["apps", "delegate", "on"],
        &["apps", "nft", "apply", "38600-38899", "38900-38999", "5432"],
        &["apps", "process-unit", "on"],
    ] {
        let o = t.call(args);
        assert_eq!(o.code, exit::STATE, "{args:?}");
        assert!(
            o.stderr.contains("box_not_prepared"),
            "{args:?}: {}",
            o.stderr
        );
    }
    assert!(t.non_query_calls().is_empty(), "{:?}", t.non_query_calls());
}

// ── linger and delegation ───────────────────────────────────────────────

#[test]
fn linger_and_delegate_converge_and_only_the_daemon_turns_them_off() {
    let t = box_root();
    with_app_user(&t);
    let root = t.root.clone();
    t.exec.0.borrow_mut().effects.push((
        LOGINCTL.into(),
        Rc::new(move |args: &[String]| {
            let f = crate::ctx::under(&root, &format!("{LINGER_DIR}/k2app"));
            match args[0].as_str() {
                "enable-linger" => {
                    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                    std::fs::write(f, "").unwrap();
                }
                "disable-linger" => std::fs::remove_file(f).unwrap(),
                _ => {}
            }
        }),
    ));
    // user@996 not running yet (box prep orders delegate before linger).
    rule(&t, SYSTEMCTL, "is-active", 3, "");

    let o = t.call(&["apps", "delegate", "on"]);
    ok(&o);
    assert_eq!(json(&o)["restartNeeded"], false);
    let dropin = "/etc/systemd/system/user@996.service.d/k2-delegate.conf";
    assert_eq!(read(&t, dropin), delegate_dropin());
    assert_eq!(reloads(&t), 1);
    ok(&t.call(&["apps", "delegate", "on"]));
    assert_eq!(reloads(&t), 1, "same bytes: no second reload");

    ok(&t.call(&["apps", "linger", "on"]));
    ok(&t.call(&["apps", "linger", "on"]));
    assert_eq!(calls_of(&t, LOGINCTL), vec!["enable-linger k2app"]);

    // An agent may tighten, never loosen.
    t.as_agent();
    for args in [&["apps", "linger", "off"][..], &["apps", "delegate", "off"]] {
        assert_eq!(t.call(args).code, exit::NOT_DAEMON, "{args:?}");
    }
    assert!(t.p(dropin).exists());
}

#[test]
fn delegate_refuses_a_linked_dropin_dir() {
    let t = box_root();
    with_app_user(&t);
    std::fs::create_dir_all(t.p("/var/lib/elsewhere")).unwrap();
    symlink(
        t.p("/var/lib/elsewhere"),
        t.p("/etc/systemd/system/user@996.service.d"),
    )
    .unwrap();
    let o = t.call(&["apps", "delegate", "on"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(std::fs::read_dir(t.p("/var/lib/elsewhere"))
        .unwrap()
        .next()
        .is_none());
    assert_eq!(reloads(&t), 0);
    // A planted link AT the drop-in file: refused, its target never written.
    std::fs::remove_file(t.p("/etc/systemd/system/user@996.service.d")).unwrap();
    std::fs::create_dir_all(t.p("/etc/systemd/system/user@996.service.d")).unwrap();
    symlink(
        t.p("/etc/shadow-victim"),
        t.p("/etc/systemd/system/user@996.service.d/k2-delegate.conf"),
    )
    .unwrap();
    let o = t.call(&["apps", "delegate", "on"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(!t.p("/etc/shadow-victim").exists());
}

// ── io.cost ─────────────────────────────────────────────────────────────

/// A fake sysfs: /var/lib on md2 (9:2) built from two NVMe partitions.
fn raid_sysfs(t: &T) {
    let r = |abs: &str| t.p(abs);
    for (disk, dev, part, pdev) in [
        ("nvme0n1", "259:0", "nvme0n1p2", "259:2"),
        ("nvme1n1", "259:3", "nvme1n1p2", "259:5"),
    ] {
        let d = format!("/sys/devices/pci0000:00/{disk}");
        write(t, &format!("{d}/dev"), &format!("{dev}\n"));
        write(t, &format!("{d}/{part}/dev"), &format!("{pdev}\n"));
        write(t, &format!("{d}/{part}/partition"), "2\n");
        std::fs::create_dir_all(r("/sys/class/block")).unwrap();
        symlink(
            format!("../../devices/pci0000:00/{disk}/{part}"),
            r(&format!("/sys/class/block/{part}")),
        )
        .unwrap();
    }
    write(t, "/sys/devices/virtual/block/md2/dev", "9:2\n");
    std::fs::create_dir_all(r("/sys/devices/virtual/block/md2/slaves")).unwrap();
    for part in ["nvme0n1p2", "nvme1n1p2"] {
        symlink(
            format!("../../../../../class/block/{part}"),
            r(&format!("/sys/devices/virtual/block/md2/slaves/{part}")),
        )
        .unwrap();
    }
    std::fs::create_dir_all(r("/sys/dev/block")).unwrap();
    symlink("../../devices/virtual/block/md2", r("/sys/dev/block/9:2")).unwrap();
    write(
        t,
        "/proc/self/mountinfo",
        "22 1 9:2 / / rw,relatime - ext4 /dev/md2 rw\n30 22 0:26 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n",
    );
}

#[test]
fn io_cost_finds_the_raid_members_and_writes_qos() {
    let t = box_root();
    with_app_user(&t);
    raid_sysfs(&t);
    assert_eq!(
        crate::apps::leaf_disks(&t.ctx, "9:2"),
        vec!["259:0", "259:3"]
    );
    write(
        &t,
        IO_COST_QOS,
        "259:0 enable=0 ctrl=auto\n259:3 enable=0 ctrl=auto\n",
    );
    let o = t.call(&["apps", "io-cost", "on"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["changed"], true);
    // The fake knob keeps the last write (the kernel keeps every line).
    assert_eq!(read(&t, IO_COST_QOS), "259:3 enable=1\n");

    // Already on: nothing written.
    write(
        &t,
        IO_COST_QOS,
        "259:0 enable=1 ctrl=auto\n259:3 enable=1 ctrl=auto\n",
    );
    let o = t.call(&["apps", "io-cost", "on"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], false);
    assert_eq!(json(&o)["enabled"], true);

    // Off is daemon-only.
    t.as_agent();
    assert_eq!(t.call(&["apps", "io-cost", "off"]).code, exit::NOT_DAEMON);
}

#[test]
fn io_cost_without_the_knob_is_a_reported_skip_and_links_out_of_sys_are_ignored() {
    let t = box_root();
    with_app_user(&t);
    raid_sysfs(&t);
    let o = t.call(&["apps", "io-cost", "on"]);
    ok(&o);
    assert_eq!(json(&o)["available"], false);

    // /sys/dev/block/<dev> pointing outside /sys never resolves to a disk.
    std::fs::remove_file(t.p("/sys/dev/block/9:2")).unwrap();
    std::fs::create_dir_all(t.p("/etc/fake-disk")).unwrap();
    std::fs::write(t.p("/etc/fake-disk/dev"), "8:0\n").unwrap();
    symlink(t.p("/etc/fake-disk"), t.p("/sys/dev/block/9:2")).unwrap();
    assert!(crate::apps::leaf_disks(&t.ctx, "9:2").is_empty());
}

#[test]
fn mountinfo_picks_the_longest_mount_point() {
    let mi = "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 8:17 / /var/lib rw - ext4 /dev/sdb1 rw\n3 1 8:33 / /var/lib/k2app rw - xfs /dev/sdc1 rw\n4 1 8:49 / /var/libx rw - ext4 /dev/sdd1 rw\n";
    assert_eq!(
        crate::apps::mount_dev(mi, "/var/lib/k2app").as_deref(),
        Some("8:33")
    );
    assert_eq!(
        crate::apps::mount_dev(mi, "/var/lib").as_deref(),
        Some("8:17")
    );
    assert_eq!(
        crate::apps::mount_dev(mi, "/var/libz").as_deref(),
        Some("8:1")
    );
    assert_eq!(
        crate::apps::parse_qos("8:0 enable=1 ctrl=user\nbad\n8:16 enable=0\n"),
        vec![("8:0".into(), true), ("8:16".into(), false)]
    );
}

// ── reserved ports ──────────────────────────────────────────────────────

#[test]
fn reserve_ports_merges_with_others_and_reverses_exactly() {
    let t = box_root();
    write(&t, RESERVED_PORTS_PROC, "8080,9000-9010\n");
    let o = t.call(&["apps", "reserve-ports", "on"]);
    ok(&o);
    assert_eq!(
        read(&t, RESERVED_PORTS_PROC),
        "8080,9000-9010,38600-38999\n"
    );
    assert_eq!(read(&t, SYSCTL_FILE), sysctl_conf());
    let v = json(&o);
    assert_eq!(v["covered"], true);
    assert_eq!(v["file"], true);

    let o = t.call(&["apps", "reserve-ports", "on"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], false);

    // Covered by someone's wider range already: left alone.
    write(&t, RESERVED_PORTS_PROC, "38000-39999\n");
    ok(&t.call(&["apps", "reserve-ports", "on"]));
    assert_eq!(read(&t, RESERVED_PORTS_PROC), "38000-39999\n");

    write(&t, RESERVED_PORTS_PROC, "8080,38600-38999\n");
    ok(&t.call(&["apps", "reserve-ports", "off"]));
    assert_eq!(read(&t, RESERVED_PORTS_PROC), "8080\n");
    assert!(!t.p(SYSCTL_FILE).exists());
}

#[test]
fn reserve_ports_refuses_a_planted_link() {
    let t = box_root();
    write(&t, RESERVED_PORTS_PROC, "\n");
    write(&t, "/etc/victim", "keep\n");
    std::fs::create_dir_all(t.p("/etc/sysctl.d")).unwrap();
    symlink(t.p("/etc/victim"), t.p(SYSCTL_FILE)).unwrap();
    let o = t.call(&["apps", "reserve-ports", "on"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert_eq!(read(&t, "/etc/victim"), "keep\n");
    // And a linked sysctl.d is refused too.
    let t = box_root();
    write(&t, RESERVED_PORTS_PROC, "\n");
    std::fs::create_dir_all(t.p("/var/lib/elsewhere")).unwrap();
    symlink(t.p("/var/lib/elsewhere"), t.p("/etc/sysctl.d")).unwrap();
    assert_eq!(t.call(&["apps", "reserve-ports", "on"]).code, exit::STATE);
    assert!(std::fs::read_dir(t.p("/var/lib/elsewhere"))
        .unwrap()
        .next()
        .is_none());
}

// ── nftables ────────────────────────────────────────────────────────────

#[test]
fn nft_apply_feeds_the_canned_ruleset_for_the_looked_up_uid() {
    let t = box_root();
    with_app_user(&t);
    t.as_agent(); // tightening: open
    let o = t.call(&["apps", "nft", "apply", "38600-38899", "38900-38999", "5432"]);
    ok(&o);
    let fed: Vec<_> = t
        .calls()
        .into_iter()
        .filter(|(p, a, _)| p == NFT && a == &vec!["-f".to_string(), "-".to_string()])
        .collect();
    assert_eq!(fed.len(), 1);
    let want = nft_ruleset(APP_UID, None, GATEWAY_PORTS, PROXY_PORTS, DB_PORTS, &[]);
    assert_eq!(fed[0].2.as_deref(), Some(want.as_bytes()));
    assert!(want.contains("meta skuid 996 jump k2app_v4"));

    // Removing it is the daemon's, and so is any other set of ranges (an
    // agent can't open more loopback ports to Apps).
    assert_eq!(t.call(&["apps", "nft", "remove"]).code, exit::NOT_DAEMON);
    let o = t.call(&["apps", "nft", "apply", "1024-2047", "38900-38999", "5432"]);
    assert_eq!(o.code, exit::NOT_DAEMON);
    assert_eq!(calls_of(&t, NFT).iter().filter(|c| *c == "-f -").count(), 1);
}

#[test]
fn nft_remove_deletes_only_when_present() {
    let t = box_root();
    rule(&t, NFT, "list", 1, "");
    let o = t.call(&["apps", "nft", "remove"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], false);
    assert!(calls_of(&t, NFT).iter().all(|c| !c.starts_with("delete")));

    let t = box_root();
    rule(&t, NFT, "list", 0, "table inet k2app {\n}\n");
    ok(&t.call(&["apps", "nft", "remove"]));
    assert!(calls_of(&t, NFT).contains(&"delete table inet k2app".to_string()));
}

// ── packages and runtime ────────────────────────────────────────────────

pub(super) fn bin(t: &T, abs: &str) {
    write(t, abs, "#!fake\n");
}

#[test]
fn packages_install_uses_the_distro_and_gates_on_podman_5_with_pasta() {
    let t = box_root();
    bin(&t, APT_GET);
    bin(&t, PODMAN);
    bin(&t, PASTA);
    bin(&t, CRUN);
    rule(&t, PODMAN, "--version", 0, "podman version 4.9.3\n");
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("podman_too_old"), "{}", o.stderr);
    assert!(o.stderr.contains("doesn't upgrade a Podman it didn't install"), "{}", o.stderr);
    // The box's own Podman (4.9.3 here) is never on the list (F4).
    assert_eq!(
        calls_of(&t, ENV),
        vec![
            "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get update -q".to_string(),
            format!("{APT_SIM} {APT_DISTRO_KEEP}"),
            format!("{APT_Y} {APT_DISTRO_KEEP}"),
        ]
    );

    let t = box_root();
    bin(&t, APT_GET);
    bin(&t, PODMAN);
    bin(&t, CRUN);
    rule(&t, PODMAN, "--version", 0, "podman version 5.4.2\n");
    rule(&t, CRUN, "--version", 0, "crun version 1.21\ncommit: x\n");
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE, "no pasta");
    assert!(o.stderr.starts_with("pasta_missing"), "{}", o.stderr);
    bin(&t, PASTA);
    let o = t.call(&["apps", "packages", "install"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["podman"], "5.4.2");
    assert_eq!(v["podmanOk"], true);
    assert_eq!(v["crun"], "1.21");
    assert_eq!(v["packageManager"], "apt");

    // dnf and pacman get their own fixed vectors.
    let t = box_root();
    bin(&t, DNF);
    let _ = t.call(&["apps", "packages", "install"]);
    assert_eq!(
        calls_of(&t, DNF),
        vec!["install -y -q podman crun passt shadow-utils skopeo catatonit"]
    );
    let t = box_root();
    bin(&t, PACMAN);
    let _ = t.call(&["apps", "packages", "install-youki"]);
    assert_eq!(calls_of(&t, PACMAN), vec!["-S --needed --noconfirm youki"]);

    let t = box_root();
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE, "no package manager");
}

/// The engine binaries `packages install` checks after the install.
fn engine_bins(t: &T) {
    bin(t, PODMAN);
    bin(t, PASTA);
    bin(t, CRUN);
    rule(t, PODMAN, "--version", 0, "podman version 5.4.2\n");
}

/// apt (via env) or pacman installing `apparmor` puts the parser in place.
fn apparmor_install_effect(t: &T, prog: &str) {
    let root = t.root.clone();
    t.exec.0.borrow_mut().effects.push((
        prog.into(),
        Rc::new(move |args: &[String]| {
            if args.iter().any(|a| a == "apparmor") {
                let p = crate::ctx::under(&root, "/usr/sbin/apparmor_parser");
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "#!fake\n").unwrap();
            }
        }),
    ));
}

/// apt's real install and its simulation (same options, `--no-upgrade`).
pub(super) const APT_Y: &str = "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get install -y -q --no-install-recommends --no-upgrade -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold";
pub(super) const APT_SIM: &str = "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get install -s -q --no-install-recommends --no-upgrade -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold";
/// The distro path on a box with no Podman.
pub(super) const APT_DISTRO: &str = "podman crun passt uidmap dbus-user-session skopeo catatonit nftables";
/// The distro path on a box whose Podman is kept.
pub(super) const APT_DISTRO_KEEP: &str = "crun passt uidmap dbus-user-session skopeo catatonit nftables";
/// K2's Podman bundle runs k2app: only what the bundle can't carry.
pub(super) const APT_BUNDLE: &str = "uidmap dbus-user-session skopeo nftables";
/// `engine_bins` puts a Podman on the box, so these tests are the keep path.
const APT_INSTALL: &str = "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get install -y -q --no-install-recommends --no-upgrade -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold crun passt uidmap dbus-user-session skopeo catatonit nftables";

#[test]
fn packages_install_adds_apparmor_when_the_kernel_has_it_and_the_parser_is_missing() {
    // apt: AppArmor on, no parser → the fixed `apparmor` package rides the
    // same install, then the parser is there and `apparmor on` loads k2-app.
    let t = box_root();
    with_app_user(&t);
    bin(&t, APT_GET);
    engine_bins(&t);
    write(&t, APPARMOR_ENABLED, "Y\n");
    std::fs::create_dir_all(t.p(APPARMOR_DIR)).unwrap();
    apparmor_install_effect(&t, ENV);
    let v = ok_json(&t.call(&["apps", "packages", "install"]));
    assert_eq!(
        calls_of(&t, ENV),
        vec![
            "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get update -q".to_string(),
            format!("{APT_SIM} {APT_DISTRO_KEEP} apparmor"),
            format!("{APT_INSTALL} apparmor"),
        ]
    );
    assert_eq!(v["apparmor"]["needed"], true, "{v}");
    assert_eq!(v["apparmor"]["installed"], serde_json::json!(["apparmor"]));
    assert_eq!(v["apparmor"]["parser"], PARSER);
    // Audited with the package it added.
    assert!(
        t.audit.0.borrow().iter().any(|l| l.contains("\"verb\":\"apps packages install\"")
            && l.contains("\"apparmorInstalled\":[\"apparmor\"]")),
        "{:?}",
        t.audit.0.borrow()
    );
    let a = ok_json(&t.call(&["apps", "apparmor", "on"]));
    assert_eq!(a["changed"], true, "{a}");
    assert_eq!(calls_of(&t, PARSER), vec![format!("--replace --write-cache {APPARMOR_FILE}")]);

    // Idempotent: the parser is there now, so the next prep adds nothing.
    let v = ok_json(&t.call(&["apps", "packages", "install"]));
    assert_eq!(v["apparmor"]["needed"], false, "{v}");
    assert_eq!(calls_of(&t, ENV).last().unwrap(), APT_INSTALL);
    assert!(!t.audit.0.borrow().last().unwrap().contains("apparmorInstalled"));

    // pacman: the same fixed name.
    let t = box_root();
    bin(&t, PACMAN);
    engine_bins(&t);
    write(&t, APPARMOR_ENABLED, "Y\n");
    apparmor_install_effect(&t, PACMAN);
    let v = ok_json(&t.call(&["apps", "packages", "install"]));
    assert_eq!(
        calls_of(&t, PACMAN),
        vec!["-S --needed --noconfirm crun passt skopeo catatonit apparmor"]
    );
    assert_eq!(v["apparmor"]["parser"], PARSER);

    // The package installed but no parser where K2 looks: a clear stop.
    let t = box_root();
    bin(&t, APT_GET);
    engine_bins(&t);
    write(&t, APPARMOR_ENABLED, "Y\n");
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.starts_with("apparmor_missing: installed the distro's apparmor package"), "{}", o.stderr);

    // dnf: no fixed name, so nothing is added and the answer says why.
    let t = box_root();
    bin(&t, DNF);
    engine_bins(&t);
    write(&t, APPARMOR_ENABLED, "Y\n");
    let v = ok_json(&t.call(&["apps", "packages", "install"]));
    assert_eq!(calls_of(&t, DNF), vec!["install -y -q crun passt shadow-utils skopeo catatonit"]);
    assert_eq!(v["apparmor"]["needed"], true);
    assert_eq!(v["apparmor"]["installed"], serde_json::json!([]));
    assert!(v["apparmor"]["note"].as_str().unwrap().contains("no fixed AppArmor package for dnf"), "{v}");
}

#[test]
fn packages_install_leaves_apparmor_alone_when_the_kernel_has_it_off_or_selinux_enforces() {
    for (enabled, enforce) in [(None, None), (Some("N\n"), None), (Some("Y\n"), Some("1\n"))] {
        let t = box_root();
        bin(&t, APT_GET);
        engine_bins(&t);
        if let Some(e) = enabled {
            write(&t, APPARMOR_ENABLED, e);
        }
        if let Some(e) = enforce {
            write(&t, SELINUX_ENFORCE, e);
        }
        let v = ok_json(&t.call(&["apps", "packages", "install"]));
        assert_eq!(calls_of(&t, ENV).last().unwrap(), APT_INSTALL, "{enabled:?} {enforce:?}");
        assert_eq!(v["apparmor"]["needed"], false, "{v}");
        assert!(v["apparmor"]["parser"].is_null(), "{v}");
    }
}

#[test]
fn youki_is_opt_in_daemon_only_and_needs_its_binary() {
    let t = box_root();
    // crun is the default: open, but the binary must be there.
    let o = t.call(&["apps", "runtime", "crun"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("crun_missing"), "{}", o.stderr);
    bin(&t, CRUN);
    t.as_agent();
    let o = t.call(&["apps", "runtime", "crun"]);
    ok(&o);
    assert_eq!(read(&t, CONTAINERS_CONF), containers_conf(OciRuntime::Crun));
    assert_eq!(json(&o)["runtime"], "crun");
    // youki: the daemon's call.
    assert_eq!(t.call(&["apps", "runtime", "youki"]).code, exit::NOT_DAEMON);

    let t = box_root();
    let o = t.call(&["apps", "runtime", "youki"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("youki_missing"), "{}", o.stderr);
    bin(&t, YOUKI);
    rule(
        &t,
        YOUKI,
        "--version",
        0,
        "youki version 0.5.3\ncommit: x\n",
    );
    let o = t.call(&["apps", "runtime", "youki"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["runtime"], "youki");
    assert_eq!(v["youki"], "0.5.3");
    // Switching back is a normal converge.
    bin(&t, CRUN);
    ok(&t.call(&["apps", "runtime", "crun"]));
    assert_eq!(read(&t, CONTAINERS_CONF), containers_conf(OciRuntime::Crun));
}

#[test]
fn version_parsing() {
    use crate::apps::version_in;
    assert_eq!(version_in("podman version 5.4.2").as_deref(), Some("5.4.2"));
    assert_eq!(
        version_in("crun version 1.14.1\ncommit: abc\n").as_deref(),
        Some("1.14.1")
    );
    assert_eq!(
        version_in("conmon version 2.1.10\n").as_deref(),
        Some("2.1.10")
    );
    assert_eq!(version_in("youki version v0.5.3").as_deref(), Some("0.5.3"));
    assert_eq!(version_in("pasta 2025_05_03.b2a7b6b"), None);
}

// ── process exception ───────────────────────────────────────────────────

#[test]
fn process_unit_is_canned_and_only_the_daemon_starts_apps() {
    let t = box_root();
    with_app_user(&t);
    let unit = "/etc/systemd/system/k2app-process@.service";
    ok(&t.call(&["apps", "process-unit", "on"]));
    assert_eq!(read(&t, unit), process_unit());
    assert_eq!(reloads(&t), 1);
    ok(&t.call(&["apps", "process-unit", "on"]));
    assert_eq!(reloads(&t), 1);

    // No engine yet: a clear refusal, systemd never asked.
    let o = t.call(&["apps", "process", "start", "jobs"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("engine_missing"), "{}", o.stderr);
    bin(&t, ENGINE_PATH);
    // Its own user and an nft table that already covers it.
    let o = t.call(&["apps", "process", "start", "jobs"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("k2appx"), "{}", o.stderr);
    with_process_user_lines(&t);
    let o = t.call(&["apps", "process", "start", "jobs"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("nftables"), "{}", o.stderr);
    nft_lists_both(&t);
    // Never uncapped: the caps drop-in comes first.
    let o = t.call(&["apps", "process", "start", "jobs"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("caps_missing"), "{}", o.stderr);
    ok(&t.call(&["apps", "process", "caps", "jobs", "512", "-.41943040.-.2000"]));
    ok(&t.call(&["apps", "process", "start", "jobs"]));
    ok(&t.call(&["apps", "process", "stop", "jobs"]));
    let sc = calls_of(&t, SYSTEMCTL);
    assert!(
        sc.contains(&"start k2app-process@jobs.service".to_string()),
        "{sc:?}"
    );
    assert!(
        sc.contains(&"stop k2app-process@jobs.service".to_string()),
        "{sc:?}"
    );

    // A doctored template: start refuses until it is put back.
    write(&t, unit, "[Service]\nUser=root\nExecStart=/bin/sh\n");
    let o = t.call(&["apps", "process", "start", "jobs"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);

    // Agents can't start or stop Apps.
    t.as_agent();
    for v in ["start", "stop"] {
        assert_eq!(
            t.call(&["apps", "process", v, "jobs"]).code,
            exit::NOT_DAEMON
        );
    }
}

#[test]
fn process_caps_render_a_runtime_dropin_from_tokens_only() {
    let t = box_root();
    with_app_user(&t);
    let caps = "/run/systemd/system/k2app-process@jobs.service.d/k2-caps.conf";
    let v = json(&t.call(&["apps", "process", "caps", "jobs", "512", "-.41943040.-.2000"]));
    assert_eq!(v["changed"], true, "{v}");
    assert_eq!(
        read(&t, caps),
        process_caps_dropin(512, &IoMax::k2_default(), &[])
    );
    assert_eq!(reloads(&t), 1);
    ok(&t.call(&["apps", "process", "caps", "jobs", "512", "-.41943040.-.2000"]));
    assert_eq!(reloads(&t), 1, "same caps: no reload");

    // A device grant needs the node on this box.
    let o = t.call(&["apps", "process", "caps", "jobs", "1024", "-.-.-.-", "dri-render128"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("device_missing"), "{}", o.stderr);
    write(&t, "/dev/dri/renderD128", "");
    ok(&t.call(&["apps", "process", "caps", "jobs", "1024", "-.-.-.-", "dri-render128"]));
    let body = read(&t, caps);
    assert!(body.contains("\nMemoryMax=1024M\n"), "{body}");
    assert!(body.contains("\nDeviceAllow=/dev/dri/renderD128 rw\n"), "{body}");
    assert!(body.contains("\nSupplementaryGroups=render\n"), "{body}");
    assert!(!body.contains("IOWrite"), "{body}");

    // A planted link where the drop-in dir goes is refused, not followed.
    let t2 = box_root();
    with_app_user(&t2);
    std::fs::create_dir_all(t2.p("/run/systemd/system")).unwrap();
    symlink(t2.p("/etc"), t2.p("/run/systemd/system/k2app-process@jobs.service.d")).unwrap();
    let o = t2.call(&["apps", "process", "caps", "jobs", "512", "-.-.-.-"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(!t2.p("/etc/k2-caps.conf").exists());

    // Agents can't set an App's limits.
    t.as_agent();
    assert_eq!(
        t.call(&["apps", "process", "caps", "jobs", "512", "-.-.-.-"]).code,
        exit::NOT_DAEMON
    );
}

#[test]
fn process_start_needs_the_mac_dropin_on_an_apparmor_box() {
    let t = box_root();
    apparmor_box(&t);
    with_process_user_lines(&t);
    nft_lists_both(&t);
    bin(&t, ENGINE_PATH);
    ok(&t.call(&["apps", "process-unit", "on"]));
    ok(&t.call(&["apps", "process", "caps", "jobs", "512", "-.-.-.-"]));
    let o = t.call(&["apps", "process", "start", "jobs"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("mac_missing"), "{}", o.stderr);
    ok(&t.call(&["apps", "apparmor", "on"]));
    ok(&t.call(&["apps", "process", "start", "jobs"]));
}

#[test]
fn process_unit_off_refuses_while_an_app_runs() {
    let t = box_root();
    with_app_user(&t);
    ok(&t.call(&["apps", "process-unit", "on"]));
    rule(
        &t,
        SYSTEMCTL,
        "list-units",
        0,
        "k2app-process@jobs.service loaded active running K2 App jobs (process)\n",
    );
    let o = t.call(&["apps", "process-unit", "off"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(t.p("/etc/systemd/system/k2app-process@.service").exists());
}

// ── MAC: the k2-app AppArmor profile ────────────────────────────────────

const PARSER: &str = "/usr/sbin/apparmor_parser";
const AA_DROPIN: &str = "/etc/systemd/system/k2app-process@.service.d/k2-apparmor.conf";

/// k2appx on a box that already has k2app.
fn with_process_user_lines(t: &T) {
    write(t, PASSWD_FILE, &(read(t, PASSWD_FILE) + PROC_PW_LINE));
    write(t, GROUP_FILE, &(read(t, GROUP_FILE) + "k2appx:x:995:\n"));
    write(t, SHADOW_FILE, &(read(t, SHADOW_FILE) + "k2appx:!:19000::::::\n"));
}

fn apparmor_box(t: &T) {
    with_app_user(t);
    write(t, APPARMOR_ENABLED, "Y\n");
    std::fs::create_dir_all(t.p(APPARMOR_DIR)).unwrap();
    bin(t, PARSER);
}

fn ok_json(o: &crate::Outcome) -> serde_json::Value {
    ok(o);
    json(o)
}

#[test]
fn apparmor_on_loads_the_profile_before_anything_points_at_it() {
    let t = box_root();
    apparmor_box(&t);
    let v = ok_json(&t.call(&["apps", "apparmor", "on"]));
    assert_eq!(v["changed"], true);
    assert_eq!(read(&t, APPARMOR_FILE), apparmor_profile());
    assert_eq!(read(&t, MAC_MARKER), mac_marker());
    assert_eq!(read(&t, AA_DROPIN), process_apparmor_dropin());
    assert_eq!(
        calls_of(&t, PARSER),
        vec![format!("--replace --write-cache {APPARMOR_FILE}")]
    );
    // The parser ran before the drop-in's daemon-reload.
    let order: Vec<String> = t
        .calls()
        .into_iter()
        .map(|(p, a, _)| format!("{p} {}", a.join(" ")))
        .collect();
    let parse_at = order.iter().position(|c| c.starts_with(PARSER)).unwrap();
    let reload_at = order
        .iter()
        .position(|c| c.ends_with("daemon-reload"))
        .unwrap();
    assert!(parse_at < reload_at, "{order:?}");
    assert_eq!(reloads(&t), 1);

    // The kernel now lists it: a second run changes nothing.
    write(
        &t,
        APPARMOR_PROFILES,
        "docker-default (enforce)\nk2-app (enforce)\n",
    );
    let v = ok_json(&t.call(&["apps", "apparmor", "on"]));
    assert_eq!(v["changed"], false, "{v}");
    assert_eq!(v["applied"], true, "{v}");
    assert_eq!(v["lsm"], "apparmor");
    assert_eq!(calls_of(&t, PARSER).len(), 1);
    assert_eq!(reloads(&t), 1);

    // Loaded in complain mode (someone ran aa-complain): enforce again.
    write(&t, APPARMOR_PROFILES, "k2-app (complain)\n");
    let v = ok_json(&t.call(&["apps", "apparmor", "on"]));
    assert_eq!(v["changed"], true);
    assert_eq!(calls_of(&t, PARSER).len(), 2);

    // A doctored profile is put back and reloaded.
    write(&t, APPARMOR_PROFILES, "k2-app (enforce)\n");
    write(
        &t,
        APPARMOR_FILE,
        "profile k2-app flags=(complain) { file, capability, }\n",
    );
    let v = ok_json(&t.call(&["apps", "apparmor", "on"]));
    assert_eq!(v["changed"], true);
    assert_eq!(read(&t, APPARMOR_FILE), apparmor_profile());
    assert_eq!(calls_of(&t, PARSER).len(), 3);

    let st = ok_json(&t.call(&["apps", "status"]));
    assert_eq!(st["mac"]["applied"], true, "{}", st["mac"]);
    assert_eq!(st["mac"]["loaded"], "enforce");
}

#[test]
fn apparmor_on_is_a_no_op_under_selinux_or_without_mac() {
    for (enabled, enforce, why) in [
        (Some("Y\n"), Some("1\n"), "SELinux"),
        (None, None, "no MAC"),
        (Some("N\n"), Some("0\n"), "no MAC"),
    ] {
        let t = box_root();
        with_app_user(&t);
        bin(&t, PARSER);
        if let Some(e) = enabled {
            write(&t, APPARMOR_ENABLED, e);
        }
        if let Some(e) = enforce {
            write(&t, SELINUX_ENFORCE, e);
        }
        let v = ok_json(&t.call(&["apps", "apparmor", "on"]));
        assert_eq!(v["changed"], false);
        assert!(v["skipped"].as_str().unwrap().contains(why), "{v}");
        assert!(!t.p(APPARMOR_FILE).exists());
        assert!(!t.p(MAC_MARKER).exists());
        assert!(!t.p(AA_DROPIN).exists());
        assert!(calls_of(&t, PARSER).is_empty());
        let st = ok_json(&t.call(&["apps", "status"]));
        assert_eq!(st["mac"]["applied"], why == "SELinux", "{}", st["mac"]);
    }
}

#[test]
fn apparmor_on_fails_closed_when_the_profile_does_not_load() {
    let t = box_root();
    apparmor_box(&t);
    rule(&t, PARSER, "--replace", 1, "");
    let o = t.call(&["apps", "apparmor", "on"]);
    assert_eq!(o.code, exit::IO, "{}", o.stderr);
    // Nothing points at a profile the kernel doesn't have.
    assert!(!t.p(MAC_MARKER).exists());
    assert!(!t.p(AA_DROPIN).exists());

    // AppArmor on but no parser: refused before writing anything.
    let t = box_root();
    with_app_user(&t);
    write(&t, APPARMOR_ENABLED, "Y\n");
    std::fs::create_dir_all(t.p(APPARMOR_DIR)).unwrap();
    let o = t.call(&["apps", "apparmor", "on"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("apparmor_missing"), "{}", o.stderr);
    assert!(!t.p(APPARMOR_FILE).exists());
}

#[test]
fn apparmor_off_is_the_daemons_and_removes_pointers_first() {
    let t = box_root();
    apparmor_box(&t);
    ok(&t.call(&["apps", "apparmor", "on"]));
    write(&t, APPARMOR_PROFILES, "k2-app (enforce)\n");
    t.as_agent();
    assert_eq!(t.call(&["apps", "apparmor", "off"]).code, exit::NOT_DAEMON);
    // Turning it on (tightening) is open.
    ok(&t.call(&["apps", "apparmor", "on"]));
    t.as_daemon();
    let v = ok_json(&t.call(&["apps", "apparmor", "off"]));
    assert_eq!(v["changed"], true);
    assert!(!t.p(AA_DROPIN).exists());
    assert!(!t.p(MAC_MARKER).exists());
    assert!(!t.p(APPARMOR_FILE).exists());
    assert!(calls_of(&t, PARSER).contains(&format!("--remove {APPARMOR_FILE}")));

    // A planted link where the marker lives is removed, never followed.
    let t = box_root();
    apparmor_box(&t);
    std::fs::create_dir_all(t.p(APP_ETC)).unwrap();
    symlink(t.p("/etc/passwd"), t.p(MAC_MARKER)).unwrap();
    ok(&t.call(&["apps", "apparmor", "off"]));
    assert!(t.p("/etc/passwd").exists());
    assert!(std::fs::symlink_metadata(t.p(MAC_MARKER)).is_err());
}

// ── cores slice and budget ──────────────────────────────────────────────

#[test]
fn cores_slice_writes_dashless_slice_and_dropins_without_killmode() {
    let t = box_root();
    with_app_user(&t);
    write(&t, "/proc/meminfo", "MemTotal:        8000000 kB\n");
    write(&t, "/lib/systemd/system/postgresql@.service", "[Service]\n");
    let o = t.call(&["apps", "cores-slice", "768"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(
        v["units"],
        serde_json::json!(["k2-daemon.service", "postgresql@.service"])
    );
    assert_eq!(
        read(&t, "/etc/systemd/system/k2cores.slice"),
        cores_slice_unit(768)
    );
    let d = read(&t, "/etc/systemd/system/k2-daemon.service.d/k2-cores.conf");
    assert_eq!(d, cores_dropin(Core::Daemon));
    assert!(!d.contains("KillMode"));
    assert_eq!(
        read(
            &t,
            "/etc/systemd/system/postgresql@.service.d/k2-cores.conf"
        ),
        cores_dropin(Core::Postgres)
    );
    assert!(
        !t.p("/etc/systemd/system/stalwart.service.d").exists(),
        "no Stalwart on this box"
    );
    assert_eq!(reloads(&t), 1);

    // Above half the box: refused, nothing changed.
    let o = t.call(&["apps", "cores-slice", "5000"]);
    assert_eq!(o.code, exit::ARGV);
    assert_eq!(
        read(&t, "/etc/systemd/system/k2cores.slice"),
        cores_slice_unit(768)
    );

    ok(&t.call(&["apps", "cores-slice", "off"]));
    assert!(!t.p("/etc/systemd/system/k2cores.slice").exists());
    assert!(!t
        .p("/etc/systemd/system/k2-daemon.service.d/k2-cores.conf")
        .exists());

    t.as_agent();
    assert_eq!(
        t.call(&["apps", "cores-slice", "768"]).code,
        exit::NOT_DAEMON
    );
}

#[test]
fn budget_is_bounded_by_the_box() {
    let t = box_root();
    with_app_user(&t);
    write(&t, "/proc/meminfo", "MemTotal:        8000000 kB\n");
    ok(&t.call(&["apps", "budget", "4096", "4096"]));
    assert_eq!(
        read(&t, "/etc/systemd/system/user@996.service.d/k2-budget.conf"),
        budget_dropin(4096, 4096)
    );
    assert_eq!(t.call(&["apps", "budget", "9000", "4096"]).code, exit::ARGV);
    ok(&t.call(&["apps", "budget", "off"]));
    assert!(!t
        .p("/etc/systemd/system/user@996.service.d/k2-budget.conf")
        .exists());
}

// ── unprep and status ───────────────────────────────────────────────────

fn proc_entry(t: &T, pid: u32, name: &str, uid: u32) {
    write(
        t,
        &format!("/proc/{pid}/status"),
        &format!("Name:\t{name}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
    );
}

#[test]
fn unprep_refuses_while_apps_run_then_undoes_config_but_keeps_the_user() {
    let t = box_root();
    with_app_user(&t);
    write(&t, "/proc/meminfo", "MemTotal:        8000000 kB\n");
    write(&t, RESERVED_PORTS_PROC, "\n");
    write(&t, "/proc/self/mountinfo", "");
    bin(&t, CRUN);
    rule(&t, SYSTEMCTL, "is-active", 3, "");
    for args in [
        &["apps", "delegate", "on"][..],
        &["apps", "reserve-ports", "on"],
        &["apps", "process-unit", "on"],
        &["apps", "runtime", "crun"],
        &["apps", "cores-slice", "768"],
        &["apps", "budget", "4096", "4096"],
    ] {
        ok(&t.call(args));
    }
    // A running App (node as k2app): refused, nothing undone.
    proc_entry(&t, 4000, "systemd", APP_UID);
    proc_entry(&t, 4001, "node", APP_UID);
    proc_entry(&t, 4002, "bash", 1001);
    let o = t.call(&["apps", "unprep"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(t.p("/etc/systemd/system/k2app-process@.service").exists());

    std::fs::remove_dir_all(t.p("/proc/4001")).unwrap();
    let o = t.call(&["apps", "unprep"]);
    ok(&o);
    for gone in [
        "/etc/systemd/system/k2app-process@.service",
        "/etc/systemd/system/k2cores.slice",
        "/etc/systemd/system/user@996.service.d/k2-delegate.conf",
        "/etc/systemd/system/user@996.service.d/k2-budget.conf",
        SYSCTL_FILE,
        CONTAINERS_CONF,
    ] {
        assert!(!t.p(gone).exists(), "{gone}");
    }
    assert!(read(&t, PASSWD_FILE).contains("k2app:"), "the user stays");
    assert!(t.p(APP_HOME).exists(), "its home stays");

    t.as_agent();
    assert_eq!(t.call(&["apps", "unprep"]).code, exit::NOT_DAEMON);
}

#[test]
fn status_reads_without_the_lock_and_reports_each_piece() {
    let t = box_root();
    with_app_user(&t);
    usermod_effect(&t);
    write(&t, RESERVED_PORTS_PROC, "38600-38999\n");
    write(&t, "/proc/self/mountinfo", "");
    bin(&t, PODMAN);
    bin(&t, PASTA);
    rule(&t, PODMAN, "--version", 0, "podman version 5.4.2\n");
    rule(&t, NFT, "list", 1, "");
    ok(&t.call(&["apps", "subid"]));
    // Hold the apps lock: status still answers (reads take no lock).
    let _held = crate::take_lock(&t.ctx, "apps").unwrap();
    t.as_agent();
    let o = t.call(&["apps", "status"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["user"]["exists"], true);
    assert_eq!(v["user"]["ok"], true);
    assert_eq!(v["user"]["uid"], APP_UID);
    assert_eq!(v["subid"]["ok"], true);
    assert_eq!(v["subid"]["count"], SUBID_COUNT);
    assert_eq!(v["linger"], false);
    assert_eq!(v["delegate"], "missing");
    assert_eq!(v["reservedPorts"]["covered"], true);
    assert_eq!(v["reservedPorts"]["file"], false);
    assert_eq!(v["nft"]["table"], false);
    assert_eq!(v["processUnit"], "missing");
    assert_eq!(v["engine"]["podman"], "5.4.2");
    assert_eq!(v["engine"]["podmanOk"], true);
    assert_eq!(v["engine"]["pasta"], true);
    assert_eq!(v["engine"]["runtime"], serde_json::Value::Null);
    assert_eq!(v["enginePresent"], false);
    // A mutating verb waits for the lock and gives up.
    let o = t.call(&["apps", "ensure-user"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("busy"), "{}", o.stderr);
}

#[test]
fn every_apps_verb_is_audited_with_its_class() {
    let t = box_root();
    with_app_user(&t);
    t.as_agent();
    let _ = t.call(&["apps", "unprep"]);
    let _ = t.call(&["apps", "status"]);
    let a = t.audit.0.borrow();
    assert!(a.iter().any(|l| l.contains("\"ns\":\"apps\"")
        && l.contains("\"verb\":\"apps unprep\"")
        && l.contains("caller_not_daemon")));
    assert!(a
        .iter()
        .any(|l| l.contains("\"verb\":\"apps status\"") && l.contains("\"class\":\"open\"")));
}

// ── pg hba-app (protocol 6, AR38) ───────────────────────────────────────

const HBA_BASE: &str = "# PostgreSQL Client Authentication\n\nlocal all postgres peer\nhost all all 127.0.0.1/32 scram-sha-256\n";

fn with_clusters(t: &T, vers: &[&str]) {
    for v in vers {
        write(t, &format!("/etc/postgresql/{v}/main/pg_hba.conf"), HBA_BASE);
        std::fs::set_permissions(
            t.p(&format!("/etc/postgresql/{v}/main/pg_hba.conf")),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
    }
}

#[test]
fn hba_app_adds_and_removes_the_canned_line_in_every_cluster() {
    let t = box_root();
    with_clusters(&t, &["16", "17"]);
    rule(&t, SYSTEMCTL, "is-active", 0, "");
    let o = t.call(&["pg", "hba-app", "add", "nsi-skin"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["role"], "app_nsi_skin");
    assert_eq!(v["changed"], serde_json::json!(["16", "17"]));
    assert_eq!(v["reloaded"], true);
    for ver in ["16", "17"] {
        let p = format!("/etc/postgresql/{ver}/main/pg_hba.conf");
        let text = read(&t, &p);
        assert!(text.contains("\nhost app_nsi_skin app_nsi_skin 127.0.0.1/32 scram-sha-256\n"), "{text}");
        assert!(text.find("app_nsi_skin").unwrap() < text.find("local all postgres").unwrap(), "above every rule");
        let mode = std::fs::metadata(t.p(&p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "the file keeps its mode");
    }
    assert_eq!(calls_of(&t, SYSTEMCTL).iter().filter(|c| *c == "reload postgresql.service").count(), 1);
    // Idempotent: no write, no reload.
    let o = t.call(&["pg", "hba-app", "add", "nsi-skin"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], serde_json::json!([]));
    assert_eq!(calls_of(&t, SYSTEMCTL).iter().filter(|c| *c == "reload postgresql.service").count(), 1);
    let o = t.call(&["pg", "hba-app", "remove", "nsi-skin"]);
    ok(&o);
    assert_eq!(read(&t, "/etc/postgresql/16/main/pg_hba.conf"), HBA_BASE);
}

#[test]
fn hba_app_is_daemon_only_and_refuses_links_and_damage_before_writing() {
    let t = box_root();
    with_clusters(&t, &["16"]);
    t.as_agent();
    let o = t.call(&["pg", "hba-app", "add", "jobs"]);
    assert_eq!(o.code, exit::NOT_DAEMON);
    assert_eq!(read(&t, "/etc/postgresql/16/main/pg_hba.conf"), HBA_BASE);

    // A second cluster with a damaged block: neither file is written.
    let t = box_root();
    with_clusters(&t, &["16"]);
    write(
        &t,
        "/etc/postgresql/17/main/pg_hba.conf",
        &format!("{HBA_BEGIN}\nhost all all 0.0.0.0/0 trust\n{HBA_END}\n"),
    );
    let o = t.call(&["pg", "hba-app", "add", "jobs"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(o.stderr.contains("line K2 didn't write"), "{}", o.stderr);
    assert_eq!(read(&t, "/etc/postgresql/16/main/pg_hba.conf"), HBA_BASE);

    // A linked pg_hba.conf is refused.
    let t = box_root();
    with_clusters(&t, &["16"]);
    std::fs::remove_file(t.p("/etc/postgresql/16/main/pg_hba.conf")).unwrap();
    write(&t, "/etc/shadow-copy", "x\n");
    symlink(t.p("/etc/shadow-copy"), t.p("/etc/postgresql/16/main/pg_hba.conf")).unwrap();
    let o = t.call(&["pg", "hba-app", "add", "jobs"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert_eq!(read(&t, "/etc/shadow-copy"), "x\n");

    // A linked cluster dir is refused.
    let t = box_root();
    with_clusters(&t, &["16"]);
    std::fs::create_dir_all(t.p("/srv/evil/main")).unwrap();
    write(&t, "/srv/evil/main/pg_hba.conf", HBA_BASE);
    symlink(t.p("/srv/evil"), t.p("/etc/postgresql/15")).unwrap();
    let o = t.call(&["pg", "hba-app", "add", "jobs"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert_eq!(read(&t, "/srv/evil/main/pg_hba.conf"), HBA_BASE);

    // No cluster at all; and Postgres not running = written, not reloaded.
    let t = box_root();
    let o = t.call(&["pg", "hba-app", "add", "jobs"]);
    assert_eq!(o.code, exit::STATE);
    with_clusters(&t, &["16"]);
    rule(&t, SYSTEMCTL, "is-active", 3, "");
    let o = t.call(&["pg", "hba-app", "add", "jobs"]);
    ok(&o);
    assert_eq!(json(&o)["reloaded"], false);
    assert!(calls_of(&t, SYSTEMCTL).iter().all(|c| !c.starts_with("reload")));
}

// ── per-App gateway rules (slice 3) ─────────────────────────────────────

fn current_table(t: &T) {
    rule(t, NFT, "list", 0, "table inet k2app {\n\tchain output {\n\t\tmeta skuid 996 jump k2app_v4\n\t}\n\tchain k2app_v4 {\n\t}\n}\n");
}

fn container_cgroup(t: &T, id: &str) -> String {
    let safe = id.replace('-', "_");
    let p = format!("{CGROUP_ROOT}/user.slice/user-996.slice/user@996.service/k2apps.slice/k2apps-{safe}.slice");
    std::fs::create_dir_all(t.p(&p)).unwrap();
    p
}

fn nft_scripts(t: &T) -> Vec<String> {
    t.calls()
        .into_iter()
        .filter(|(p, a, _)| p == NFT && a == &vec!["-f".to_string(), "-".to_string()])
        .map(|(_, _, i)| String::from_utf8(i.unwrap_or_default()).unwrap())
        .collect()
}

#[test]
fn nft_app_admits_only_the_apps_own_port_by_cgroup_and_is_daemon_only() {
    let t = box_root();
    with_app_user(&t);
    current_table(&t);
    container_cgroup(&t, "nsi-skin");
    let o = t.call(&["apps", "nft", "app", "nsi-skin", "38601"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["rules"], 1);
    assert_eq!(v["cgroupsNow"], 1);
    let sc = nft_scripts(&t);
    assert_eq!(sc.len(), 1);
    assert!(sc[0].starts_with("flush chain inet k2app k2app_apps\n"), "{}", sc[0]);
    assert!(sc[0].contains(
        "add rule inet k2app k2app_apps socket cgroupv2 level 5 \"user.slice/user-996.slice/user@996.service/k2apps.slice/k2apps-nsi_skin.slice\" oifname \"lo\" tcp dport { 38601, 38901 } accept"
    ));
    // A second App: both rules, each its own port. A port already held is refused.
    container_cgroup(&t, "nsi");
    ok(&t.call(&["apps", "nft", "app", "nsi", "38600"]));
    let last = nft_scripts(&t).pop().unwrap();
    assert_eq!(last.matches("add rule").count(), 2);
    assert!(last.contains("k2apps-nsi.slice\" oifname \"lo\" tcp dport { 38600, 38900 } accept"));
    let o = t.call(&["apps", "nft", "app", "jobs", "38600"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("belongs to App nsi"), "{}", o.stderr);
    // A full re-apply keeps the per-App rules (boot, drift).
    t.as_agent();
    ok(&t.call(&["apps", "nft", "apply", "38600-38899", "38900-38999", "5432"]));
    let full = nft_scripts(&t).pop().unwrap();
    assert!(full.contains("k2apps-nsi_skin.slice\" oifname \"lo\" tcp dport { 38601, 38901 } accept"));
    assert!(!full.contains("38600-38899"), "never the whole gateway range");
    // Agents can't add or remove rules.
    assert_eq!(t.call(&["apps", "nft", "app", "jobs", "38602"]).code, exit::NOT_DAEMON);
    assert_eq!(t.call(&["apps", "nft", "app-remove", "nsi"]).code, exit::NOT_DAEMON);
}

#[test]
fn nft_app_needs_the_current_table_and_remove_drops_the_rule() {
    let t = box_root();
    with_app_user(&t);
    rule(&t, NFT, "list", 1, "");
    let o = t.call(&["apps", "nft", "app", "jobs", "38600"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("box_not_prepared"), "{}", o.stderr);
    // An old (v1) table is not current either.
    let t = box_root();
    with_app_user(&t);
    rule(&t, NFT, "list", 0, "table inet k2app {\n\tchain output {\n\t\tmeta skuid 996 jump k2app_v1\n\t}\n}\n");
    assert_eq!(t.call(&["apps", "nft", "app", "jobs", "38600"]).code, exit::STATE);
    // Nor a v2 one (it admitted the whole proxy range; v3 is per-App, S15).
    let t = box_root();
    with_app_user(&t);
    rule(&t, NFT, "list", 0, "table inet k2app {\n\tchain output {\n\t\tmeta skuid 996 jump k2app_v2\n\t}\n\tchain k2app_v2 {\n\t}\n}\n");
    assert_eq!(t.call(&["apps", "nft", "app", "jobs", "38600"]).code, exit::STATE);

    let t = box_root();
    with_app_user(&t);
    current_table(&t);
    // No cgroup yet (the App isn't running): recorded, no rule loaded.
    let o = t.call(&["apps", "nft", "app", "jobs", "38600"]);
    ok(&o);
    assert_eq!(json(&o)["rules"], 0);
    container_cgroup(&t, "jobs");
    ok(&t.call(&["apps", "nft", "app", "jobs", "38600"]));
    assert_eq!(nft_scripts(&t).pop().unwrap().matches("add rule").count(), 1);
    let o = t.call(&["apps", "nft", "app-remove", "jobs"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], true);
    assert_eq!(nft_scripts(&t).pop().unwrap(), "flush chain inet k2app k2app_apps\n");
    let st = json(&t.call(&["apps", "status"]));
    assert_eq!(st["nftApps"], serde_json::json!({}));
}

// ── the engine install (S11) ────────────────────────────────────────────

fn engine_payload(version: &str) -> Vec<u8> {
    use k2_root_proto::engine::{format_marker, trusted_comment};
    let elf = k2_root_proto::testsign::fake_elf("x86_64", &format_marker(version, 1, "x86_64"));
    let sig = k2_root_proto::testsign::sign_with_test_key(&elf, &trusted_comment("x86_64", version, 1));
    k2_root_proto::sig::encode_payload(&sig, &elf)
}

fn engine_box(t: &T) {
    with_app_user(t);
    std::fs::create_dir_all(t.p("/run/user/996")).unwrap();
    std::fs::create_dir_all(t.p("/etc/systemd/user")).unwrap();
    std::fs::create_dir_all(t.p("/etc/tmpfiles.d")).unwrap();
}

#[test]
fn install_engine_verifies_installs_and_starts_it_as_k2app() {
    let t = box_root();
    engine_box(&t);
    let payload = engine_payload("0.46.1");
    let o = t.call_in(&["apps", "install-engine"], &payload);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["version"], "0.46.1");
    assert_eq!(v["protocol"], 1);
    assert_eq!(v["changedBinary"], true);
    assert_eq!(v["daemonUsers"], serde_json::json!(["k2"]));
    let (_, elf) = k2_root_proto::sig::decode_payload(&payload).unwrap();
    assert_eq!(std::fs::read(t.p(ENGINE_PATH)).unwrap(), elf);
    assert_eq!(std::fs::metadata(t.p(ENGINE_PATH)).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(read(&t, "/etc/systemd/user/k2-app-engine.service"), k2_root_proto::engine::user_unit());
    assert!(read(&t, "/etc/tmpfiles.d/k2app.conf").contains("a+ /run/k2app - - - - u:k2:x\n"));
    assert_eq!(calls_of(&t, SYSTEMD_TMPFILES), vec!["--create /etc/tmpfiles.d/k2app.conf".to_string()]);
    let ru = calls_of(&t, k2_root_proto::paths::RUNUSER);
    assert_eq!(
        ru,
        vec![
            "-u k2app -- /usr/bin/env XDG_RUNTIME_DIR=/run/user/996 /usr/bin/systemctl --user daemon-reload".to_string(),
            "-u k2app -- /usr/bin/env XDG_RUNTIME_DIR=/run/user/996 /usr/bin/systemctl --user enable k2-app-engine.service".to_string(),
            "-u k2app -- /usr/bin/env XDG_RUNTIME_DIR=/run/user/996 /usr/bin/systemctl --user restart k2-app-engine.service".to_string(),
        ]
    );
    // Same bytes again: no rewrite, still a restart (so a crashed engine comes back).
    let o = t.call_in(&["apps", "install-engine"], &payload);
    ok(&o);
    assert_eq!(json(&o)["changedBinary"], false);
    let st = json(&t.call(&["apps", "status"]));
    assert_eq!(st["enginePresent"], true);
    assert_eq!(st["engineUnit"], "ok");
    assert_eq!(st["engineTmpfiles"], true);
}

#[test]
fn install_engine_refuses_agents_bad_signatures_and_other_binaries() {
    let t = box_root();
    engine_box(&t);
    t.as_agent();
    // Daemon class: refused before stdin is read.
    let o = crate::run(&t.ctx, &["apps", "install-engine"], super::NoRead);
    assert_eq!(o.code, exit::NOT_DAEMON);
    assert!(!t.p(ENGINE_PATH).exists());

    let t = box_root();
    engine_box(&t);
    // A tampered binary.
    let mut p = engine_payload("0.46.1");
    let n = p.len();
    p[n - 3] ^= 1;
    assert_eq!(t.call_in(&["apps", "install-engine"], &p).code, exit::SIGNATURE);
    // The helper's own signed build is not an engine (domain separation).
    let elf = k2_root_proto::testsign::fake_elf("x86_64", &k2_root_proto::version::format_marker("0.46.1", 6, "x86_64"));
    let sig = k2_root_proto::testsign::sign_with_test_key(&elf, &k2_root_proto::sig::trusted_comment("x86_64", "0.46.1", 6));
    let o = t.call_in(&["apps", "install-engine"], &k2_root_proto::sig::encode_payload(&sig, &elf));
    assert_eq!(o.code, exit::SIGNATURE);
    assert!(o.stderr.contains("another file"), "{}", o.stderr);
    assert!(!t.p(ENGINE_PATH).exists());
    assert!(calls_of(&t, k2_root_proto::paths::RUNUSER).is_empty());
    // A planted link at the engine path is refused, never followed.
    write(&t, "/etc/evil", "x");
    symlink(t.p("/etc/evil"), t.p(ENGINE_PATH)).unwrap();
    let o = t.call_in(&["apps", "install-engine"], &engine_payload("0.46.1"));
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert_eq!(read(&t, "/etc/evil"), "x");
    // No user manager (linger off): refused before any write.
    let t = box_root();
    engine_box(&t);
    std::fs::remove_dir(t.p("/run/user/996")).unwrap();
    let o = t.call_in(&["apps", "install-engine"], &engine_payload("0.46.1"));
    assert_eq!(o.code, exit::STATE);
    assert!(!t.p(ENGINE_PATH).exists());
}

#[test]
fn logs_read_one_deployments_unit_for_the_daemon_only() {
    let t = box_root();
    with_app_user(&t);
    rule(&t, k2_root_proto::paths::JOURNALCTL, "-o", 0, "esc a1 up on 41000\n");
    let o = t.call(&["apps", "logs", "nsi-skin", "d-0123456789ab", "200"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["text"], "esc a1 up on 41000\n");
    assert_eq!(v["unit"], "k2app-nsi-skin--d-0123456789ab.service");
    assert_eq!(
        calls_of(&t, k2_root_proto::paths::JOURNALCTL),
        vec!["--no-pager -o cat -n 200 _SYSTEMD_OWNER_UID=996 _SYSTEMD_USER_UNIT=k2app-nsi-skin--d-0123456789ab.service".to_string()]
    );
    t.as_agent();
    assert_eq!(t.call(&["apps", "logs", "nsi-skin", "d-0123456789ab", "200"]).code, exit::NOT_DAEMON);
}

#[test]
fn nft_app_box_ports_join_the_apps_rule_and_a_new_list_replaces_the_old() {
    let t = box_root();
    with_app_user(&t);
    current_table(&t);
    container_cgroup(&t, "hearth");
    let o = t.call(&["apps", "nft", "app", "hearth", "38602", "8470", "7880"]);
    ok(&o);
    assert_eq!(json(&o)["boxPorts"], serde_json::json!([8470, 7880]));
    let sc = nft_scripts(&t).pop().unwrap();
    assert!(sc.contains("k2apps-hearth.slice\" oifname \"lo\" tcp dport { 38602, 38902, 8470, 7880 } accept"), "{sc}");
    let st = json(&t.call(&["apps", "status"]));
    assert_eq!(st["nftBoxPorts"], serde_json::json!({"hearth": [8470, 7880]}));
    // Revoke one: the list is replaced and the chain reloads without it.
    ok(&t.call(&["apps", "nft", "app", "hearth", "38602", "8470"]));
    let sc = nft_scripts(&t).pop().unwrap();
    assert!(sc.contains("tcp dport { 38602, 38902, 8470 } accept"), "{sc}");
    assert!(!sc.contains("7880"), "{sc}");
    // A full re-apply keeps them (boot, drift).
    t.as_agent();
    ok(&t.call(&["apps", "nft", "apply", "38600-38899", "38900-38999", "5432"]));
    assert!(nft_scripts(&t).pop().unwrap().contains("tcp dport { 38602, 38902, 8470 } accept"));
    t.as_daemon();
    // None: back to the gateway and proxy only; app-remove forgets them.
    ok(&t.call(&["apps", "nft", "app", "hearth", "38602"]));
    assert!(nft_scripts(&t).pop().unwrap().contains("tcp dport { 38602, 38902 } accept"));
    ok(&t.call(&["apps", "nft", "app", "hearth", "38602", "8470"]));
    ok(&t.call(&["apps", "nft", "app-remove", "hearth"]));
    let st = json(&t.call(&["apps", "status"]));
    assert_eq!(st["nftBoxPorts"], serde_json::json!({}));
    // The deny-list holds at the helper's door too.
    for bad in ["22", "5432", "38601", "2019"] {
        assert_ne!(t.call(&["apps", "nft", "app", "hearth", "38602", bad]).code, 0, "{bad}");
    }
}
