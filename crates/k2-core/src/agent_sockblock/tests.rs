//! The block's pure parts: the filter's decisions (through the
//! interpreter, for both architectures and both modes), the address parser
//! (also with arbitrary bytes), the switch, and on Linux the path rule
//! against a real folder with real sockets.

use super::filter::{self, Arch, SeccompData, RET_ALLOW, RET_ERRNO, RET_USER_NOTIF};
use super::*;

const ENOSYS_RET: u32 = RET_ERRNO | 38;
const EPERM_RET: u32 = RET_ERRNO | 1;

fn call(arch: Arch, nr: u32, args: [u64; 6]) -> SeccompData {
    SeccompData { nr: nr as i32, arch: arch.audit(), instruction_pointer: 0x7fff_0000_1234, args }
}

#[test]
fn the_filter_decides_each_call_as_the_review_lists() {
    for arch in [Arch::X86_64, Arch::Aarch64] {
        let n = arch.nrs();
        let e = filter::program(arch, Mode::Enforce);
        let r = filter::program(arch, Mode::Record);
        let both = |d: &SeccompData| (filter::run(&e, d), filter::run(&r, d));
        // 5. connect: always notifies, whatever its arguments.
        for args in [[3, 0x1000, 110, 0, 0, 0], [u64::MAX; 6], [0; 6]] {
            assert_eq!(both(&call(arch, n.connect, args)), (RET_USER_NOTIF, RET_USER_NOTIF), "{arch:?} connect {args:?}");
        }
        // 2. io_uring_setup.
        assert_eq!(both(&call(arch, n.io_uring_setup, [1, 0x1000, 0, 0, 0, 0])), (ENOSYS_RET, RET_USER_NOTIF), "{arch:?}");
        // 3. seccomp(): only with the new-listener flag (bit 3), in the low word.
        let sc = |flags: u64| both(&call(arch, n.seccomp, [1, flags, 0x1000, 0, 0, 0]));
        assert_eq!(sc(1 << 3), (EPERM_RET, RET_USER_NOTIF), "{arch:?} new listener");
        assert_eq!(sc((1 << 3) | (1 << 5) | 1), (EPERM_RET, RET_USER_NOTIF), "{arch:?} new listener with others");
        assert_eq!(sc(0), (RET_ALLOW, RET_ALLOW), "{arch:?} a plain filter");
        assert_eq!(sc(1), (RET_ALLOW, RET_ALLOW), "{arch:?} tsync");
        assert_eq!(sc(1 << 35), (RET_ALLOW, RET_ALLOW), "{arch:?} a high bit the kernel ignores");
        // 4. socket()/socketpair(): AF_UNIX + datagram, type flags masked.
        for nr in [n.socket, n.socketpair] {
            let s = |dom: u64, ty: u64| both(&call(arch, nr, [dom, ty, 0, 0, 0, 0]));
            assert_eq!(s(1, 2), (EPERM_RET, RET_USER_NOTIF), "{arch:?} {nr} unix dgram");
            assert_eq!(s(1, 2 | 0o4000 | 0o2000000), (EPERM_RET, RET_USER_NOTIF), "{arch:?} {nr} unix dgram, nonblock+cloexec");
            assert_eq!(s(1, 1), (RET_ALLOW, RET_ALLOW), "{arch:?} {nr} unix stream");
            assert_eq!(s(1, 5), (RET_ALLOW, RET_ALLOW), "{arch:?} {nr} unix seqpacket");
            assert_eq!(s(1, 1 | 0o4000), (RET_ALLOW, RET_ALLOW), "{arch:?} {nr} unix stream nonblock");
            assert_eq!(s(2, 2), (RET_ALLOW, RET_ALLOW), "{arch:?} {nr} inet dgram");
            assert_eq!(s(10, 1), (RET_ALLOW, RET_ALLOW), "{arch:?} {nr} inet6 stream");
            // The kernel reads int arguments: high words don't count.
            assert_eq!(s(1 | (7 << 32), 2 | (9 << 32)), (EPERM_RET, RET_USER_NOTIF), "{arch:?} {nr} high words");
        }
        // 6. Everything else.
        for nr in [0u32, 1, 2, 59, 221, 56, 435, 424] {
            if [n.connect, n.io_uring_setup, n.seccomp, n.socket, n.socketpair].contains(&nr) {
                continue;
            }
            assert_eq!(both(&call(arch, nr, [1, 2, 3, 4, 5, 6])), (RET_ALLOW, RET_ALLOW), "{arch:?} nr {nr}");
        }
        // 1. Another architecture: refused before the number is looked at.
        for other in [0x4000_0003u32, 0x4000_0028, Arch::X86_64.audit(), Arch::Aarch64.audit()] {
            if other == arch.audit() {
                continue;
            }
            let mut d = call(arch, n.connect, [0; 6]);
            d.arch = other;
            assert_eq!(both(&d), (ENOSYS_RET, RET_USER_NOTIF), "{arch:?} from arch {other:#x}");
            d.nr = 1;
            assert_eq!(both(&d), (ENOSYS_RET, RET_USER_NOTIF), "{arch:?} write from arch {other:#x}");
        }
    }
    // x32 numbers on x86-64, including a connect through them.
    let e = filter::program(Arch::X86_64, Mode::Enforce);
    for nr in [0x4000_0000u32 | 42, 0x4000_0000 | 41, 0x4000_0000, 0xffff_ffff] {
        assert_eq!(filter::run(&e, &call(Arch::X86_64, nr, [0; 6])), ENOSYS_RET, "x32 {nr:#x}");
    }
    assert!(Arch::X86_64.foreign(&call(Arch::X86_64, 0x4000_002a, [0; 6])));
    assert!(!Arch::X86_64.foreign(&call(Arch::X86_64, 42, [0; 6])));
    assert!(!Arch::Aarch64.foreign(&call(Arch::Aarch64, 0x4000_002a, [0; 6])), "no x32 on aarch64");
}

#[test]
fn the_filter_program_is_well_formed() {
    for arch in [Arch::X86_64, Arch::Aarch64] {
        for mode in [Mode::Enforce, Mode::Record] {
            let p = filter::program(arch, mode);
            assert!(p.len() < 64, "a short program: {}", p.len());
            assert_eq!(p.last().map(|i| i.code), Some(filter::RET_K), "it ends with a return");
            // The first instruction loads the architecture word.
            assert_eq!((p[0].code, p[0].k), (filter::LD_W_ABS, 4));
            for (pc, i) in p.iter().enumerate() {
                if i.code & 0x07 == 0x05 {
                    assert!(pc + 1 + usize::from(i.jt) < p.len() && pc + 1 + usize::from(i.jf) < p.len(), "jump at {pc} stays inside");
                }
                if i.code == filter::LD_W_ABS {
                    assert!(i.k % 4 == 0 && i.k < 64, "load at {pc} inside seccomp_data");
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn the_kernel_numbers_and_layouts_match() {
    let native = Arch::native().expect("x86-64 or aarch64");
    let n = native.nrs();
    assert_eq!(n.socket as libc::c_long, libc::SYS_socket);
    assert_eq!(n.socketpair as libc::c_long, libc::SYS_socketpair);
    assert_eq!(n.connect as libc::c_long, libc::SYS_connect);
    assert_eq!(n.seccomp as libc::c_long, libc::SYS_seccomp);
    assert_eq!(n.io_uring_setup as libc::c_long, libc::SYS_io_uring_setup);
    assert_eq!(std::mem::size_of::<SeccompData>(), 64);
    assert_eq!(std::mem::size_of::<sys::Notif>(), 80);
    assert_eq!(std::mem::size_of::<sys::NotifResp>(), 24);
    assert_eq!(sys::IOCTL_NOTIF_RECV, 0xC050_2100);
    assert_eq!(sys::IOCTL_NOTIF_SEND, 0xC018_2101);
    assert_eq!(sys::IOCTL_NOTIF_ID_VALID, 0x4008_2102);
    assert_eq!(filter::AF_UNIX as i32, libc::AF_UNIX);
    assert_eq!(filter::SOCK_DGRAM as i32, libc::SOCK_DGRAM);
}

fn sun(path: &[u8]) -> Vec<u8> {
    let mut v = 1u16.to_ne_bytes().to_vec();
    v.extend_from_slice(path);
    v
}

#[test]
fn unix_addresses_parse_as_the_kernel_reads_them() {
    assert_eq!(parse_unix(&sun(b"/run/user/1000/bus\0")), UnixName::Path(b"/run/user/1000/bus".to_vec()));
    // No terminator: the length ends it.
    assert_eq!(parse_unix(&sun(b"/tmp/x")), UnixName::Path(b"/tmp/x".to_vec()));
    // An embedded zero byte ends the name there.
    assert_eq!(parse_unix(&sun(b"/tmp/a\0/../b")), UnixName::Path(b"/tmp/a".to_vec()));
    assert_eq!(parse_unix(&sun(b"\0dbus-abc")), UnixName::Abstract(b"dbus-abc".to_vec()));
    assert_eq!(parse_unix(&sun(b"rel/sock\0")), UnixName::Path(b"rel/sock".to_vec()));
    // Only the family (unnamed), too short, too long, another family.
    assert_eq!(parse_unix(&sun(b"")), UnixName::Invalid);
    assert_eq!(parse_unix(&[1]), UnixName::Invalid);
    assert_eq!(parse_unix(&[]), UnixName::Invalid);
    assert_eq!(parse_unix(&sun(&[b'a'; 109])), UnixName::Invalid);
    assert_eq!(parse_unix(&sun(&[b'a'; 108])), UnixName::Path(vec![b'a'; 108]));
    let mut inet = sun(b"/tmp/x\0");
    inet[0] = 2;
    assert_eq!(parse_unix(&inet), UnixName::Invalid);
    assert_eq!(shown(&UnixName::Abstract(b"x".to_vec())).as_deref(), Some("@x"));
}

/// Arbitrary bytes never panic the parser, and what it returns holds.
#[test]
fn the_address_parser_takes_arbitrary_bytes() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for round in 0..200_000 {
        let len = (next() % 140) as usize;
        let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        // Half the time a Unix family, so the path forms get reached.
        if round % 2 == 0 && len >= 2 {
            b[..2].copy_from_slice(&1u16.to_ne_bytes());
        }
        if round % 7 == 0 && len >= 3 {
            b[2] = b'/';
        }
        match parse_unix(&b) {
            UnixName::Path(p) => {
                assert!(!p.is_empty() && p.len() <= SUN_LEN_MAX - 2 && !p.contains(&0), "{b:?}");
                assert_eq!(p[0], b[2]);
                assert!(len > 2 && len <= SUN_LEN_MAX);
            }
            UnixName::Abstract(a) => {
                assert_eq!(b[2], 0);
                assert_eq!(a.len(), len - 3);
            }
            UnixName::Invalid => assert!(len <= 2 || len > SUN_LEN_MAX || u16::from_ne_bytes([b[0], b[1]]) != 1),
        }
    }
}

#[test]
fn the_switch_and_paths() {
    assert_eq!(mode_from(None), Some(Mode::Record), "unset: record");
    assert_eq!(mode_from(Some("")), Some(Mode::Record));
    assert_eq!(mode_from(Some(" Enforce ")), Some(Mode::Enforce));
    assert_eq!(mode_from(Some("record")), Some(Mode::Record));
    for off in ["off", "0", "false", "no", "OFF"] {
        assert_eq!(mode_from(Some(off)), None, "{off}");
    }
    assert_eq!(mode_from(Some("strict")), Some(Mode::Record), "an unknown word stays at record");
    assert_eq!(kernel_version("6.8.0-149-generic\n"), Some((6, 8)));
    assert_eq!(kernel_version("5.19.17"), Some((5, 19)));
    assert_eq!(kernel_version("x"), None);
    assert!(plain_absolute("/home/u/.k2/run/cells/s.sock"));
    for bad in ["rel/x", "/a/../b", "/a/./b", "/a//b", "", "/a\0b"] {
        assert!(!plain_absolute(bad), "{bad:?}");
    }
    let spec = BlockSpec { mode: Mode::Record, hook_sock: None, k2_uid: 1, tmp_dir: "/t".into(), log: "/l".into() };
    let j = serde_json::to_value(&spec).unwrap();
    assert_eq!(j["mode"], "record");
    assert!(j.get("hook_sock").is_none());
    assert_eq!(serde_json::from_value::<BlockSpec>(j).unwrap(), spec);
}

#[cfg(target_os = "linux")]
mod paths {
    use super::super::supervisor::{Check, HookRule, Rules, TmpRule};
    use super::super::sys;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    fn rules(tmp: &Path, hook: Option<&Path>, uid: u32) -> Rules {
        let c = std::ffi::CString::new(tmp.to_str().unwrap()).unwrap();
        let dir = sys::openat2(libc::AT_FDCWD, &c, libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC, 0).unwrap();
        let dev = sys::fstat(dir.as_raw_fd()).unwrap().st_dev;
        Rules {
            hook: hook.map(|h| HookRule { path: h.to_str().unwrap().as_bytes().to_vec(), uid }),
            tmp: Some(TmpRule { prefix: tmp.to_str().unwrap().as_bytes().to_vec(), dir, dev }),
        }
    }

    fn allowed(r: &Rules, p: &Path) -> Result<(), String> {
        match r.check(p.to_str().unwrap().as_bytes()) {
            Check::Allowed(..) => Ok(()),
            Check::Refused(why) => Err(why),
        }
    }

    /// The S4 forms, against real sockets: a link, `..`, a hard link, a
    /// file that isn't a socket, all refused; a real socket in the temp
    /// folder (also through a link that stays inside) allowed, and a
    /// connect through its `/proc/self/fd` name reaches it.
    #[test]
    fn the_temp_folder_rule_holds_on_a_real_folder() {
        let root: PathBuf = crate::test_env::short_tmp(format!("k2sb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (tmp, out) = (root.join("t"), root.join("o"));
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let tmp = std::fs::canonicalize(&tmp).unwrap();
        let out = std::fs::canonicalize(&out).unwrap();
        let _own = UnixListener::bind(tmp.join("sub/own.sock")).unwrap();
        let _bus = UnixListener::bind(out.join("bus")).unwrap();
        std::fs::write(tmp.join("plain"), "x").unwrap();
        std::os::unix::fs::symlink(out.join("bus"), tmp.join("link")).unwrap();
        std::os::unix::fs::symlink(tmp.join("sub/own.sock"), tmp.join("inside")).unwrap();
        std::os::unix::fs::symlink(&out, tmp.join("dirlink")).unwrap();
        let r = rules(&tmp, None, 0);
        allowed(&r, &tmp.join("sub/own.sock")).expect("a real socket in its temp folder");
        allowed(&r, &tmp.join("inside")).expect("a link that stays inside");
        allowed(&r, &tmp.join("sub/../sub/own.sock")).expect(".. that stays inside");
        for (p, what) in [
            (tmp.join("link"), "a link out to another socket"),
            (tmp.join("dirlink/bus"), "a link to another folder on the way"),
            (tmp.join("../o/bus"), ".. out of the folder"),
            (tmp.join("sub/../../o/bus"), ".. out, deeper"),
            (tmp.join("plain"), "a file that isn't a socket"),
            (tmp.join("missing.sock"), "a name that isn't there"),
            (out.join("bus"), "a socket outside the folder"),
            (tmp.clone(), "the folder itself"),
        ] {
            let e = allowed(&r, &p).expect_err(what);
            println!("{what}: refused: {e}");
        }
        assert!(allowed(&r, Path::new("relative.sock")).is_err());
        // A hard link of a socket (the fenced agent can't make one across its
        // fence; one made inside has two names): refused.
        match std::fs::hard_link(tmp.join("sub/own.sock"), tmp.join("hard")) {
            Ok(()) => {
                let e = allowed(&r, &tmp.join("hard")).expect_err("a hard link");
                assert!(e.contains("names"), "{e}");
                let e = allowed(&r, &tmp.join("sub/own.sock")).expect_err("the original now has two names too");
                assert!(e.contains("names"), "{e}");
            }
            Err(e) => println!("this file system refused a hard link of a socket ({e}); that case wasn't checked here"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hook_rule_wants_its_exact_socket() {
        let root: PathBuf = crate::test_env::short_tmp(format!("k2sbh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cells")).unwrap();
        std::fs::create_dir_all(root.join("t")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let hook = root.join("cells/s.sock");
        let l = UnixListener::bind(&hook).unwrap();
        // SAFETY: no preconditions.
        let me = unsafe { libc::geteuid() };
        let r = rules(&root.join("t"), Some(hook.as_path()), me);
        allowed(&r, &hook).expect("its own hook socket");
        // Connecting through the checked handle reaches the same listener.
        let Check::Allowed(fd, _) = r.check(hook.to_str().unwrap().as_bytes()) else { panic!("allowed above") };
        let s = std::os::unix::net::UnixStream::connect(format!("/proc/self/fd/{}", fd.as_raw_fd())).expect("connect through /proc/self/fd");
        let (peer, _) = l.accept().unwrap();
        drop((s, peer));
        // Another owner, a link on the way, a non-socket, a near miss.
        let other = rules(&root.join("t"), Some(hook.as_path()), me + 1);
        assert!(allowed(&other, &hook).unwrap_err().contains("uid"));
        std::os::unix::fs::symlink(root.join("cells"), root.join("cl")).unwrap();
        let via_link = root.join("cl/s.sock");
        let r2 = rules(&root.join("t"), Some(via_link.as_path()), me);
        assert!(allowed(&r2, &via_link).unwrap_err().contains("link"), "a link on the way");
        std::fs::remove_file(&hook).unwrap();
        std::fs::write(&hook, "x").unwrap();
        assert!(allowed(&r, &hook).unwrap_err().contains("socket"));
        assert!(allowed(&r, &root.join("cells/s.sock2")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
