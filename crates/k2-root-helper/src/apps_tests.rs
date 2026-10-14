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

const APP_UID: u32 = 996;

fn write(t: &T, abs: &str, body: &str) {
    let p = t.p(abs);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn read(t: &T, abs: &str) -> String {
    std::fs::read_to_string(t.p(abs)).unwrap()
}

fn json(o: &crate::Outcome) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap()
}

/// /etc/passwd, group, shadow for a box with the daemon user `k2`.
fn base_users(t: &T) {
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
fn with_app_user(t: &T) {
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

fn rule(t: &T, prog: &str, tok: &str, status: i32, stdout: &str) {
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

fn calls_of(t: &T, prog: &str) -> Vec<String> {
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
        &["apps", "nft", "apply", "38600-38899", "38900-38949", "5432"],
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
    let o = t.call(&["apps", "nft", "apply", "38600-38899", "38900-38949", "5432"]);
    ok(&o);
    let fed: Vec<_> = t
        .calls()
        .into_iter()
        .filter(|(p, a, _)| p == NFT && a == &vec!["-f".to_string(), "-".to_string()])
        .collect();
    assert_eq!(fed.len(), 1);
    let want = nft_ruleset(APP_UID, GATEWAY_PORTS, PROXY_PORTS, DB_PORTS);
    assert_eq!(fed[0].2.as_deref(), Some(want.as_bytes()));
    assert!(want.contains("meta skuid 996 jump k2app_v1"));

    // Removing it is the daemon's.
    assert_eq!(t.call(&["apps", "nft", "remove"]).code, exit::NOT_DAEMON);
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

fn bin(t: &T, abs: &str) {
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
    assert_eq!(
        calls_of(&t, ENV),
        vec![
            "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get update -q".to_string(),
            "DEBIAN_FRONTEND=noninteractive /usr/bin/apt-get install -y -q --no-install-recommends -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold podman crun passt uidmap dbus-user-session".to_string(),
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
        vec!["install -y -q podman crun passt shadow-utils"]
    );
    let t = box_root();
    bin(&t, PACMAN);
    let _ = t.call(&["apps", "packages", "install-youki"]);
    assert_eq!(calls_of(&t, PACMAN), vec!["-S --needed --noconfirm youki"]);

    let t = box_root();
    let o = t.call(&["apps", "packages", "install"]);
    assert_eq!(o.code, exit::STATE, "no package manager");
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
