//! `k5-cli daemon start|stop|restart|survive|log|uninstall` and the
//! daemon row's usage against the shell CLI's `cli/k2 daemon …`, on the
//! parity rig ([`common::parity`]).
//!
//! No row can touch a real service. Every side runs with `PATH` set to
//! exactly two folders: first the stand-ins (`systemctl`, `launchctl`,
//! `loginctl`, `journalctl`, `sudo`, `curl`, `minisign`, `sc`,
//! `schtasks`, and `uname`, which says which OS the row is on), then a
//! folder of links to a short list of ordinary tools (bash, python3,
//! coreutils), none of them a service manager, `sudo` or a download tool
//! ([`assert_only_stand_ins`] checks it on every run). The stand-ins log
//! what they were run with; each row names the service calls it must
//! make, and the binary must make the same ones (the shell's `curl`
//! calls to the stub daemon go through the `curl` stand-in to the real
//! curl; the binary never runs curl). No token is ever in a stand-in's
//! arguments. A test also proves no absolute path to a service manager,
//! `sudo` or a download tool is in the shell's daemon verbs or the
//! binary's.
//!
//! Each side starts from the same files (the row's setup) and must print
//! the same, exit the same, send the same requests, run the same service
//! calls and leave the same files (the stop and keep intents, the port
//! and token files, the plist).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::path::{Path, PathBuf};

const VERB: &str = "daemon";
/// The owner token in `daemon.token`.
const DAEMON_TOKEN: &str = "daemon-tok-5151";
/// A remote server's token, read from stdin.
const REMOTE_TOKEN: &str = "remote-tok-6161";

/// The stand-ins' names.
const STAND_INS: &[&str] = &[
    "systemctl",
    "launchctl",
    "loginctl",
    "journalctl",
    "sudo",
    "curl",
    "minisign",
    "sc",
    "schtasks",
];

/// Programs no folder but the stand-ins' may hold.
const NEVER_REAL: &[&str] = &[
    "systemctl",
    "launchctl",
    "loginctl",
    "journalctl",
    "sudo",
    "doas",
    "pkexec",
    "su",
    "service",
    "initctl",
    "curl",
    "wget",
    "minisign",
    "sc",
    "schtasks",
    "uname",
    "git",
];

/// The ordinary tools the two CLIs may run, linked from this process's
/// `PATH` when there.
const TOOLS: &[&str] = &[
    "bash",
    "sh",
    "env",
    "python3",
    "awk",
    "sed",
    "grep",
    "tr",
    "cat",
    "head",
    "tail",
    "cut",
    "sort",
    "uniq",
    "wc",
    "date",
    "mkdir",
    "chmod",
    "mv",
    "rm",
    "cp",
    "ln",
    "id",
    "ps",
    "readlink",
    "realpath",
    "dirname",
    "basename",
    "mktemp",
    "sleep",
    "stat",
    "touch",
    "od",
    "base64",
    "sha256sum",
    "shasum",
    "kill",
    "find",
    "ls",
    "tee",
    "printf",
    "hostname",
    "whoami",
    "expr",
    "test",
    "true",
    "false",
    "tty",
    "stty",
    "getconf",
    "seq",
    "cmp",
    "diff",
    "timeout",
];

fn svc(r: &Rig) -> PathBuf {
    r.dir.join("svc")
}

fn exe(p: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(p, text).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Where `name` is on this process's `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file()))
}

/// The stand-ins, and the two tool folders (with and without `python3`).
fn folders(r: &Rig) {
    let d = svc(r);
    std::fs::create_dir_all(&d).unwrap();
    let log = r.dir.join("tools.log");
    let logline = |name: &str| {
        format!(
            "{{ printf '%s' '{name}'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'; }} >> '{}'\n",
            log.display()
        )
    };
    let dir = d.display();
    let systemctl = format!(
        r#"#!/bin/sh
{}scope=system
if [ "$1" = --user ]; then scope=user; shift; fi
[ -f '{dir}/systemd' ] || exit 1
read s u st pid slice cg < '{dir}/systemd'
[ "$s" = "$scope" ] || exit 1
case "$1" in
  cat)
    [ "$2" = "$u" ] || exit 1
    if [ -f '{dir}/systemd-exec' ]; then cat '{dir}/systemd-exec'; else echo "ExecStart=/opt/k2/k2-daemon"; fi ;;
  is-active) echo "$st"; [ "$st" = active ] ;;
  show)
    case "$3" in
      MainPID) echo "$pid" ;;
      Slice) echo "$slice" ;;
      ControlGroup) echo "$cg" ;;
      *) exit 1 ;;
    esac ;;
  stop|restart|start)
    [ "$2" = "$u" ] || exit 1
    echo "stand-in systemctl: $1 $2"
    if [ -f '{dir}/systemd-fail' ]; then echo "Failed to $1 $2: Access denied" >&2; exit 1; fi ;;
  *) exit 1 ;;
esac
"#,
        logline("systemctl")
    );
    exe(&d.join("systemctl"), &systemctl);
    let launchctl = format!(
        r#"#!/bin/sh
{}f='{dir}/launchctl-'"$1"
if [ -f "$f" ]; then cat "$f" >&2; fi
if [ -f "$f.rc" ]; then read rc < "$f.rc"; exit "$rc"; fi
exit 0
"#,
        logline("launchctl")
    );
    exe(&d.join("launchctl"), &launchctl);
    for name in [
        "loginctl",
        "journalctl",
        "sudo",
        "sc",
        "schtasks",
    ] {
        exe(
            &d.join(name),
            &format!(
                "#!/bin/sh\n{}echo \"stand-in {name}: never run\" >&2\nexit 1\n",
                logline(name)
            ),
        );
    }
    // `minisign`: never run, but in an install row (a `fixtures` folder):
    // `-V -P <key> -m <file> -x <sig> -Q` passes when the signature's
    // second line is `RUQ<key>` and its fourth is the file's first line,
    // and prints its trusted comment, as `minisign -Q` does.
    exe(
        &d.join("minisign"),
        &format!(
            r#"#!/bin/sh
{}if [ ! -d '{dir}/fixtures' ]; then echo "stand-in minisign: never run" >&2; exit 1; fi
key=; m=; x=; prev=
for a in "$@"; do
  case "$prev" in -P) key=$a ;; -m) m=$a ;; -x) x=$a ;; esac
  prev=$a
done
if [ "$(sed -n 2p "$x")" != "RUQ$key" ]; then echo "stand-in minisign: not this key" >&2; exit 1; fi
if [ "$(sed -n 4p "$x")" != "$(head -n 1 "$m")" ]; then echo "stand-in minisign: not this file" >&2; exit 1; fi
sed -n 3p "$x" | sed 's/^trusted comment: //'
"#,
            logline("minisign")
        ),
    );
    // `curl`: the real one for the shell's calls to the stub daemon; in
    // an install row (a `fixtures` folder) it never reaches a network and
    // serves each URL from its fixture, or fails as `curl -f` on a 404.
    let curl = which("curl").expect("a curl for the shell's calls to the stub daemon");
    exe(
        &d.join("curl"),
        &format!(
            r#"#!/bin/sh
{}if [ -d '{dir}/fixtures' ]; then
  url=; out=; prev=
  for a in "$@"; do
    if [ "$prev" = -o ]; then out=$a; fi
    case "$a" in http://*|https://*) url=$a ;; esac
    prev=$a
  done
  f='{dir}/fixtures/'"$(printf '%s' "$url" | tr -c 'A-Za-z0-9._-' '_')"
  if [ ! -f "$f" ]; then echo "curl: (22) The requested URL returned error: 404" >&2; exit 22; fi
  if [ -n "$out" ]; then cat "$f" > "$out"; else cat "$f"; fi
  exit 0
fi
exec '{}' "$@"
"#,
            logline("curl"),
            curl.display()
        ),
    );
    exe(
        &d.join("uname"),
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  -m) echo x86_64 ;;\n  *) if [ -f '{dir}/uname-s' ]; then cat '{dir}/uname-s'; else echo Linux; fi ;;\nesac\n"
        ),
    );
    for (sub, python) in [("sys", true), ("sys-nopy", false)] {
        let t = r.dir.join(sub);
        std::fs::create_dir_all(&t).unwrap();
        for name in TOOLS {
            if *name == "python3" && !python {
                continue;
            }
            if let Some(p) = which(name) {
                std::os::unix::fs::symlink(&p, t.join(name)).unwrap();
            }
        }
        for must in ["bash", "cat", "tail", "id"] {
            assert!(t.join(must).exists(), "no {must} for the rows");
        }
    }
}

/// `PATH` for a side: the stand-ins, then the tools.
fn path_for(r: &Rig, python: bool) -> String {
    let tools = r.dir.join(if python { "sys" } else { "sys-nopy" });
    format!("{}:{}", svc(r).display(), tools.display())
}

/// `path` is the stand-ins and one folder of tools, and only the
/// stand-ins' folder holds a service manager, `sudo` or a download tool.
fn assert_only_stand_ins(r: &Rig, path: &str) {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    assert_eq!(dirs.len(), 2, "PATH is the stand-ins and the tools: {path}");
    assert_eq!(dirs[0], svc(r), "the stand-ins come first: {path}");
    for name in STAND_INS {
        assert!(dirs[0].join(name).is_file(), "no stand-in {name}");
    }
    for name in NEVER_REAL {
        assert!(
            std::fs::symlink_metadata(dirs[1].join(name)).is_err(),
            "{name} is reachable outside the stand-ins: {}",
            dirs[1].display()
        );
    }
}

fn rig(tag: &str) -> Rig {
    let mut rig = Rig::new(VERB, tag);
    folders(&rig);
    rig.tool_log = Some(rig.dir.join("tools.log"));
    rig
}

fn plist(r: &Rig) -> PathBuf {
    r.home.join("Library/LaunchAgents/dev.k2.daemon.plist")
}

/// Every row's files start here: the owner and port the gate wants, the
/// daemon's own token, and no unit, plist or OS word from a row before.
fn base(r: &Rig) {
    owner_and_port(r);
    let d = svc(r);
    for f in [
        "systemd",
        "systemd-exec",
        "systemd-fail",
        "uname-s",
        "launchctl-load",
        "launchctl-load.rc",
        "launchctl-unload",
        "launchctl-kickstart",
    ] {
        let _ = std::fs::remove_file(d.join(f));
    }
    let _ = std::fs::remove_dir_all(r.home.join("Library"));
    for f in ["daemon.port", "daemon.stdout.log"] {
        let _ = std::fs::remove_file(r.k2home.join(f));
    }
    std::fs::write(r.k2home.join("daemon.token"), format!("{DAEMON_TOKEN}\n")).unwrap();
}

fn put(r: &Rig, name: &str, text: &str) {
    std::fs::write(svc(r).join(name), text).unwrap();
}

fn user_unit(r: &Rig) {
    base(r);
    put(
        r,
        "systemd",
        "user k2-daemon.service active 4242 user.slice /user.slice/k2-daemon.service\n",
    );
}

fn system_unit_failing(r: &Rig) {
    base(r);
    put(
        r,
        "systemd",
        "system k2-daemon.service active 4242 system.slice /system.slice/k2-daemon.service\n",
    );
    put(r, "systemd-fail", "");
}

fn slice_moved(r: &Rig) {
    base(r);
    put(
        r,
        "systemd",
        "system k2-daemon.service active 4242 k2cores.slice /system.slice/k2-daemon.service\n",
    );
}

fn no_daemon_token(r: &Rig) {
    base(r);
    let _ = std::fs::remove_file(r.k2home.join("daemon.token"));
}

fn with_log(r: &Rig) {
    base(r);
    std::fs::write(
        r.k2home.join("daemon.stdout.log"),
        "one\ntwo\nthree\nfour\n",
    )
    .unwrap();
}

fn mac(r: &Rig) {
    base(r);
    put(r, "uname-s", "Darwin\n");
}

fn mac_plist(r: &Rig) {
    mac(r);
    let p = plist(r);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "<plist/>\n").unwrap();
}

fn mac_loaded(r: &Rig) {
    mac_plist(r);
    let menubar = r.home.join("Library/LaunchAgents/dev.k2.menubar.plist");
    std::fs::write(menubar, "<plist/>\n").unwrap();
    put(
        r,
        "launchctl-load",
        "/x/dev.k2.daemon.plist: service already loaded\n",
    );
}

fn mac_load_fails(r: &Rig) {
    mac_plist(r);
    put(r, "launchctl-load", "Load failed: 5: Input/output error\n");
    put(r, "launchctl-load.rc", "5\n");
}

fn mac_not_loaded(r: &Rig) {
    mac_plist(r);
    put(r, "launchctl-unload", "Could not find specified service\n");
    put(
        r,
        "launchctl-kickstart",
        "Could not find service \"dev.k2.daemon\" in domain\n",
    );
}

fn mac_old_label(r: &Rig) {
    mac(r);
    let old = r
        .home
        .join("Library/LaunchAgents/com.k2so.k2so-daemon.plist");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(old, "<plist/>\n").unwrap();
}

fn other_os(r: &Rig) {
    base(r);
    put(r, "uname-s", "FreeBSD\n");
}

/// The files a verb may leave or take away.
fn files_after(r: &Rig) -> Vec<(&'static str, bool)> {
    vec![
        (
            "stop-intent",
            r.k2home.join("run/stop-intent.json").is_file(),
        ),
        (
            "keep-intent",
            r.k2home.join("run/keep-intent.json").is_file(),
        ),
        ("daemon.token", r.k2home.join("daemon.token").exists()),
        ("daemon.port", r.k2home.join("daemon.port").exists()),
        ("plist", plist(r).exists()),
    ]
}

/// The service calls a side made: its stand-ins' log without the shell's
/// `curl` calls (the binary never runs curl). No token is in it.
fn service_calls(r: &Rig, name: &str, native: bool) -> Vec<String> {
    let log = r.take_tool_log();
    for t in [OWNER, DAEMON_TOKEN, REMOTE_TOKEN] {
        assert!(
            !log.contains(t),
            "{name}: a token in a stand-in's arguments:\n{log}"
        );
    }
    // A line is a stand-in's: the shell's curl takes `-w` with a newline
    // in it, so its argument goes on to a line of its own.
    let calls: Vec<String> = log
        .lines()
        .filter(|l| STAND_INS.contains(&l.split(' ').next().unwrap_or("")))
        .map(str::to_string)
        .collect();
    if native {
        assert!(
            calls.iter().all(|l| !l.starts_with("curl")),
            "{name}: the binary ran curl:\n{log}"
        );
    }
    calls
        .into_iter()
        .filter(|l| !l.starts_with("curl"))
        .collect()
}

fn uid() -> String {
    let o = std::process::Command::new("id").arg("-u").output().unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

/// `want` with `{home}` and `{uid}` filled in.
fn calls(r: &Rig, want: &[&str]) -> Vec<String> {
    want.iter()
        .map(|w| {
            w.replace("{home}", &r.home.to_string_lossy())
                .replace("{uid}", &uid())
        })
        .collect()
}

/// A side of `c` with `PATH` set to the stand-ins and the tools.
fn side_case(r: &Rig, c: &Case, python: bool) -> Case {
    let path = path_for(r, python);
    assert_only_stand_ins(r, &path);
    let mut env = c.env.clone();
    env.push(("PATH", path));
    Case {
        name: c.name,
        args: c.args,
        env,
        replies: c.replies.clone(),
        setup: c.setup,
        cwd: c.cwd.clone(),
        no_tmpdir: c.no_tmpdir,
        code: c.code,
        requests: c.requests,
        compare: c.compare,
        no_python3: c.no_python3,
        stdin: c.stdin,
        deadline: c.deadline,
        bash32_empty_array: c.bash32_empty_array,
    }
}

type Seen = (
    Option<i32>,
    String,
    String,
    Vec<String>,
    Vec<String>,
    Vec<(&'static str, bool)>,
);

/// Runs `c` on the shell, the binary, and (`native`) the binary with no
/// `python3`, each on its own stub, and compares them. `want` is the
/// service calls the row must make. `native`: the binary answers the
/// line itself (else it hands it to the shell).
fn check(r: &Rig, c: &Case, want: &[&str], native: bool) {
    let mask = |side: Side, t: &str| t.replace(&r.stub_of(side).port.to_string(), "{port}");
    let bash_case = side_case(r, c, true);
    r.prepare(Side::Bash, &bash_case);
    let out = r.run(true, &bash_case);
    let log: Vec<String> = r
        .read(Side::Bash, false, c.name)
        .iter()
        .map(|l| mask(Side::Bash, &normalise(l, false, VERB, c.name)))
        .collect();
    let bash: Seen = (
        out.code,
        mask(Side::Bash, &out.stdout),
        mask(Side::Bash, &out.stderr),
        log,
        service_calls(r, c.name, false),
        files_after(r),
    );
    println!("-- bash: {bash:?}");
    assert_eq!(bash.0, Some(c.code), "{}: bash's exit", c.name);
    assert_eq!(bash.3.len(), c.requests, "{}: bash's requests", c.name);
    assert_eq!(bash.4, calls(r, want), "{}: bash's service calls", c.name);
    let sides: &[(Side, bool)] = if native {
        &[(Side::Rust, true), (Side::NoPython3, false)]
    } else {
        &[(Side::Rust, true)]
    };
    for (side, python) in sides {
        let run = side_case(r, c, *python);
        let (out, log) = r.binary(&run, !*python);
        let handed = k5_cli::fallthrough::cache_dir(&r.k2home).exists();
        assert_eq!(
            handed, !native,
            "{}: {side:?} handed it to the shell",
            c.name
        );
        let got: Seen = (
            out.code,
            mask(*side, &out.stdout),
            mask(*side, &out.stderr),
            log.iter().map(|l| mask(*side, l)).collect(),
            service_calls(r, c.name, native),
            files_after(r),
        );
        assert_eq!(got, bash, "{}: {side:?} against bash", c.name);
    }
}

fn with_stdin(c: Case, input: &'static [u8]) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

const CAT_USER: &str = "systemctl --user cat k2-daemon.service";
const CAT_USER_OLD: &str = "systemctl --user cat k2so-daemon.service";
const CAT_SYSTEM: &str = "systemctl cat k2-daemon.service";
const CAT_SYSTEM_OLD: &str = "systemctl cat k2so-daemon.service";
const NO_UNIT: &[&str] = &[CAT_USER, CAT_USER_OLD, CAT_SYSTEM, CAT_SYSTEM_OLD];

#[test]
fn daemon_stop_on_linux_answers_as_the_shell_does() {
    let r = rig("stop");
    let user = |verb: &'static str| -> Vec<&'static str> {
        vec![
            CAT_USER,
            "systemctl --user show -p Slice --value k2-daemon.service",
            "systemctl --user show -p ControlGroup --value k2-daemon.service",
            verb,
        ]
    };
    let system = |verb: &'static str| -> Vec<&'static str> {
        vec![
            CAT_USER,
            CAT_USER_OLD,
            CAT_SYSTEM,
            "systemctl show -p Slice --value k2-daemon.service",
            "systemctl show -p ControlGroup --value k2-daemon.service",
            verb,
        ]
    };
    let rows: Vec<(Case, Vec<&str>)> = vec![
        (
            // native: daemon stop
            case("stop: a user unit", &["stop"], &[], vec![], user_unit, 0, 0),
            user("systemctl --user stop k2-daemon.service"),
        ),
        (
            case(
                "stop: a system unit that needs root",
                &["stop"],
                &[],
                vec![],
                system_unit_failing,
                1,
                0,
            ),
            system("systemctl stop k2-daemon.service"),
        ),
        (
            case(
                "stop: the unit's slice moved",
                &["stop"],
                &[],
                vec![],
                slice_moved,
                0,
                0,
            ),
            system("systemctl stop k2-daemon.service"),
        ),
        (
            case(
                "stop: no unit, the daemon stops",
                &["stop"],
                &[],
                vec![ok(r#"{"stopping":true}"#)],
                base,
                0,
                1,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            case(
                "stop: no unit, the daemon says something else",
                &["stop"],
                &[],
                vec![ok(r#"{"stopping":false}"#)],
                base,
                0,
                1,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            case(
                "stop: no unit, no answer",
                &["stop"],
                &[],
                vec![Reply::Close],
                base,
                0,
                1,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            case(
                "stop: no unit, no daemon.token: nothing sent",
                &["stop"],
                &[],
                vec![],
                no_daemon_token,
                0,
                0,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            // native: daemon stop
            case(
                "stop --keep-agents: a user unit",
                &["stop", "--keep-agents"],
                &[],
                vec![],
                user_unit,
                0,
                0,
            ),
            user("systemctl --user stop k2-daemon.service"),
        ),
        (
            case(
                "stop --keep-agents: a system unit that needs root",
                &["stop", "--keep-agents"],
                &[],
                vec![],
                system_unit_failing,
                1,
                0,
            ),
            system("systemctl stop k2-daemon.service"),
        ),
        (
            case(
                "stop --keep-agents: the slice moved",
                &["stop", "--keep-agents"],
                &[],
                vec![],
                slice_moved,
                2,
                0,
            ),
            vec![
                CAT_USER,
                CAT_USER_OLD,
                CAT_SYSTEM,
                "systemctl show -p Slice --value k2-daemon.service",
                "systemctl show -p ControlGroup --value k2-daemon.service",
            ],
        ),
        (
            case(
                "stop --keep-agents: no unit",
                &["stop", "--keep-agents"],
                &[],
                vec![],
                base,
                2,
                0,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            case(
                "stop with a word it doesn't take",
                &["stop", "--now"],
                &[],
                vec![],
                user_unit,
                2,
                0,
            ),
            vec![],
        ),
    ];
    for (c, want) in rows {
        check(&r, &c, &want, true);
    }
}

#[test]
fn daemon_restart_answers_as_the_shell_does() {
    let r = rig("restart");
    let user_restart = vec![CAT_USER, "systemctl --user restart k2-daemon.service"];
    let ready = |v: &str| ok(&format!(r#"{{"phase":"ready","version":"{v}"}}"#));
    let rows: Vec<(Case, Vec<&str>)> = vec![
        (
            // native: daemon restart
            case(
                "restart: a user unit",
                &["restart"],
                &[],
                vec![],
                user_unit,
                0,
                0,
            ),
            user_restart.clone(),
        ),
        (
            // native: daemon restart
            case(
                "restart --wait: back up",
                &["restart", "--wait"],
                &[],
                vec![ready("9.9.9")],
                user_unit,
                0,
                1,
            ),
            user_restart.clone(),
        ),
        (
            case(
                "restart: a system unit that needs root",
                &["restart"],
                &[],
                vec![],
                system_unit_failing,
                1,
                0,
            ),
            vec![
                CAT_USER,
                CAT_USER_OLD,
                CAT_SYSTEM,
                "systemctl restart k2-daemon.service",
            ],
        ),
        (
            case(
                "restart: no unit, the daemon restarts",
                &["restart"],
                &[],
                vec![ok(r#"{"restarting":true}"#)],
                base,
                0,
                1,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            case(
                "restart: no unit, no answer",
                &["restart"],
                &[],
                vec![Reply::Close],
                base,
                1,
                1,
            ),
            NO_UNIT.to_vec(),
        ),
        (
            // native: daemon restart
            case(
                "restart --sessions",
                &["restart", "--sessions"],
                &[],
                vec![ok(r#"{"restarting":true,"endAgents":true}"#)],
                user_unit,
                0,
                1,
            ),
            vec![],
        ),
        (
            case(
                "restart --sessions --wait: no version",
                &["restart", "--wait", "--sessions"],
                &[],
                vec![ok(r#"{"restarting":true}"#), ok(r#"{"phase": "ready"}"#)],
                user_unit,
                0,
                2,
            ),
            vec![],
        ),
        (
            case(
                "restart --sessions: refused",
                &["restart", "--sessions"],
                &[],
                vec![Reply::Answer(403, r#"{"error":"owner_only"}"#.into())],
                user_unit,
                1,
                1,
            ),
            vec![],
        ),
        (
            case(
                "restart with a word it doesn't take",
                &["restart", "--wait", "--bogus"],
                &[],
                vec![],
                user_unit,
                2,
                0,
            ),
            vec![],
        ),
    ];
    for (c, want) in rows {
        check(&r, &c, &want, true);
    }
}

#[test]
fn daemon_restart_host_answers_as_the_shell_does() {
    let r = rig("host");
    const HOST: &str = "http://127.0.0.1:{port}";
    let rows: Vec<Case> = vec![
        // native: daemon restart
        // native: daemon restart
        with_stdin(
            case(
                "restart --host --token-stdin",
                &["restart", "--host", HOST, "--token-stdin"],
                &[],
                vec![ok(r#"{"restarting":true}"#)],
                base,
                0,
                1,
            ),
            b"remote-tok-6161\n",
        ),
        case(
            "restart --host=…/ --sessions --wait: daemon.token's token",
            &[
                "restart",
                "--host=http://127.0.0.1:{port}/",
                "--sessions",
                "--wait",
            ],
            &[],
            vec![
                ok(r#"{"restarting":true}"#),
                ok(r#"{"phase":"ready","version":"0.46.17"}"#),
            ],
            base,
            0,
            2,
        ),
        case(
            "restart --host: rejected",
            &["restart", "--host", HOST],
            &[],
            vec![Reply::Answer(403, r#"{"error":"forbidden"}"#.into())],
            base,
            1,
            1,
        ),
        case(
            "restart --host: an answer it doesn't know",
            &["restart", "--host", HOST],
            &[],
            vec![ok("fine")],
            base,
            1,
            1,
        ),
        case(
            "restart --host: no answer",
            &["restart", "--host", HOST],
            &[],
            vec![Reply::Close],
            base,
            1,
            1,
        ),
        case(
            "restart --host: no token anywhere",
            &["restart", "--host", HOST],
            &[],
            vec![],
            no_daemon_token,
            1,
            0,
        ),
        with_stdin(
            case(
                "restart --host --token-stdin --token: both",
                &["restart", "--host", HOST, "--token-stdin", "--token", "x"],
                &[],
                vec![],
                base,
                2,
                0,
            ),
            b"remote-tok-6161\n",
        ),
        case(
            "restart --host --token-stdin: nothing on stdin",
            &["restart", "--host", HOST, "--token-stdin"],
            &[],
            vec![],
            base,
            2,
            0,
        ),
    ];
    for c in rows {
        check(&r, &c, &[], true);
    }
}

#[test]
fn daemon_survive_answers_as_the_shell_does() {
    let r = rig("survive");
    let rows: Vec<Case> = vec![
        // native: daemon survive on
        case(
            "survive on",
            &["survive", "on"],
            &[],
            vec![ok(r#"{"terminal":{"ptyd":"on"}}"#)],
            base,
            0,
            1,
        ),
        // native: daemon survive off
        case(
            "survive off: the daemon kept it on",
            &["survive", "off", "extra"],
            &[],
            vec![ok(r#"{"terminal":{"ptyd":"on"}}"#)],
            base,
            1,
            1,
        ),
        case(
            "survive off: no terminal in the answer",
            &["survive", "off"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            base,
            0,
            1,
        ),
        case(
            "survive on: an error",
            &["survive", "on"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"owner_only"}"#.into())],
            base,
            3,
            1,
        ),
        case(
            "survive on: a list",
            &["survive", "on"],
            &[],
            vec![ok("[1, 2]")],
            base,
            3,
            1,
        ),
        case(
            "survive on: not JSON",
            &["survive", "on"],
            &[],
            vec![ok("nope")],
            base,
            1,
            1,
        ),
        // native: daemon survive status
        case(
            "survive: the setting and the backend",
            &["survive"],
            &[],
            vec![
                ok(r#"{"terminal":{"ptyd":" OFF "}}"#),
                ok(r#"{"ptyd":"on (k2-ptyd 3)","adopted":2}"#),
            ],
            base,
            0,
            2,
        ),
        case(
            "survive status: unset, no backend",
            &["survive", "status"],
            &[],
            vec![ok("{}"), Reply::Close],
            base,
            0,
            2,
        ),
        case(
            "survive status: settings not JSON",
            &["survive", "status"],
            &[],
            vec![ok("nope"), ok(r#"{"ptyd":"on"}"#)],
            base,
            1,
            2,
        ),
        // native: daemon survive help
        case(
            "survive help",
            &["survive", "help"],
            &[],
            vec![],
            base,
            0,
            0,
        ),
        case(
            "survive with a word it doesn't take",
            &["survive", "maybe"],
            &[],
            vec![],
            base,
            2,
            0,
        ),
    ];
    for c in rows {
        check(&r, &c, &[], true);
    }
}

#[test]
fn daemon_log_uninstall_start_and_usage_answer_as_the_shell_does() {
    let r = rig("misc");
    let rows: Vec<Case> = vec![
        // native: daemon log
        case("log: none yet", &["log"], &[], vec![], base, 0, 0),
        case("log -n 2", &["log", "-n", "2"], &[], vec![], with_log, 0, 0),
        // native: daemon logs
        case(
            "logs --tail 1 and a word it skips",
            &["logs", "x", "--tail", "1"],
            &[],
            vec![],
            with_log,
            0,
            0,
        ),
        case(
            "log --tail with no number tail takes",
            &["log", "--tail", "abc"],
            &[],
            vec![],
            with_log,
            1,
            0,
        ),
        // native: daemon uninstall
        case(
            "uninstall --yes on Linux",
            &["uninstall", "--yes"],
            &[],
            vec![],
            base,
            0,
            0,
        ),
        with_stdin(
            case("uninstall: y", &["uninstall"], &[], vec![], base, 0, 0),
            b"  y \n",
        ),
        with_stdin(
            case("uninstall: no", &["uninstall"], &[], vec![], base, 1, 0),
            b"n\n",
        ),
        case(
            "uninstall: the input ends",
            &["uninstall", "x", "--yes"],
            &[],
            vec![],
            base,
            1,
            0,
        ),
        // native: daemon start
        case(
            "start on Linux: no plist",
            &["start"],
            &[],
            vec![],
            base,
            1,
            0,
        ),
        // native: daemon --json
        case("the usage", &["--json"], &[], vec![], base, 1, 0),
        case(
            "the usage for any word",
            &["nosuch", "x"],
            &[],
            vec![],
            base,
            1,
            0,
        ),
        case(
            "other OS: stop with no plist",
            &["stop"],
            &[],
            vec![],
            other_os,
            0,
            0,
        ),
    ];
    for c in rows {
        check(&r, &c, &[], true);
    }
}

#[test]
fn daemon_verbs_on_macos_answer_as_the_shell_does() {
    let r = rig("mac");
    let load = "launchctl load -w {home}/Library/LaunchAgents/dev.k2.daemon.plist";
    let unload = "launchctl unload -w {home}/Library/LaunchAgents/dev.k2.daemon.plist";
    let kick = "launchctl kickstart -k gui/{uid}/dev.k2.daemon";
    let rows: Vec<(Case, Vec<&str>)> = vec![
        (
            case("start: loaded", &["start"], &[], vec![], mac_plist, 0, 0),
            vec![load],
        ),
        (
            case(
                "start: already loaded, and the menu bar's job",
                &["start"],
                &[],
                vec![],
                mac_loaded,
                0,
                0,
            ),
            vec![
                load,
                "launchctl load -w {home}/Library/LaunchAgents/dev.k2.menubar.plist",
            ],
        ),
        (
            case(
                "start: launchctl fails",
                &["start"],
                &[],
                vec![],
                mac_load_fails,
                0,
                0,
            ),
            vec![load],
        ),
        (
            case("start: no plist", &["start"], &[], vec![], mac, 1, 0),
            vec![],
        ),
        (
            case("stop", &["stop"], &[], vec![], mac_plist, 0, 0),
            vec![unload],
        ),
        (
            case(
                "stop: not loaded",
                &["stop"],
                &[],
                vec![],
                mac_not_loaded,
                0,
                0,
            ),
            vec![unload],
        ),
        (
            case("stop: no plist", &["stop"], &[], vec![], mac, 0, 0),
            vec![],
        ),
        (
            case(
                "stop --keep-agents: not Linux",
                &["stop", "--keep-agents"],
                &[],
                vec![],
                mac_plist,
                2,
                0,
            ),
            vec![],
        ),
        (
            case("restart", &["restart"], &[], vec![], mac_plist, 0, 0),
            vec![kick],
        ),
        (
            case(
                "restart: not loaded, so a start",
                &["restart"],
                &[],
                vec![],
                mac_not_loaded,
                0,
                0,
            ),
            vec![kick, load],
        ),
        (
            case(
                "restart: the old label",
                &["restart"],
                &[],
                vec![],
                mac_old_label,
                0,
                0,
            ),
            vec!["launchctl kickstart -k gui/{uid}/com.k2so.k2so-daemon"],
        ),
        (
            case(
                "uninstall --yes with a plist",
                &["uninstall", "-y"],
                &[],
                vec![],
                mac_plist,
                0,
                0,
            ),
            vec![unload],
        ),
    ];
    for (c, want) in rows {
        check(&r, &c, &want, true);
    }
}

#[test]
fn daemon_verbs_keep_the_gates() {
    let r = rig("gates");
    let rows: Vec<Case> = vec![
        case(
            "no daemon set up: the gate",
            &["stop"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "the guard",
            &["restart"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            port_only,
            3,
            0,
        ),
        case(
            "a session with no passport",
            &["survive", "on"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in rows {
        check(&r, &c, &[], true);
    }
}

/// Help never acts: with `-h` or `--help` on the line the front hands it
/// to the shell, which prints its usage line and touches nothing.
#[test]
fn daemon_help_lines_never_act() {
    let r = rig("help");
    let rows: Vec<Case> = vec![
        case("stop -h", &["stop", "-h"], &[], vec![], user_unit, 2, 0),
        case(
            "stop --keep-agents --help",
            &["stop", "--keep-agents", "--help"],
            &[],
            vec![],
            user_unit,
            2,
            0,
        ),
        case(
            "restart -h",
            &["restart", "-h"],
            &[],
            vec![],
            user_unit,
            2,
            0,
        ),
        case(
            "restart --host … --help",
            &["restart", "--host", "http://127.0.0.1:1", "--help"],
            &[],
            vec![],
            user_unit,
            2,
            0,
        ),
    ];
    for c in rows {
        check(&r, &c, &[], false);
    }
}

/// A value flag as the last word is the shell's (`install` never reaches
/// [`life::parse`]: [`k5_cli::families::daemon::install`] reads it).
#[test]
fn a_last_value_flag_is_the_shell_s() {
    for (verb, rest) in [
        ("install", &[][..]),
        ("log", &["--tail"][..]),
        ("logs", &["-n"][..]),
        ("restart", &["--host"][..]),
        ("restart", &["--token"][..]),
        ("restart", &["--host", "k2.example"][..]),
    ] {
        assert_eq!(
            k5_cli::families::daemon::life::parse(verb, rest),
            None,
            "{verb} {rest:?}"
        );
    }
}

/// Nothing in the shell's daemon verbs or the binary's names a service
/// manager, `sudo` or a download tool by an absolute path: every one of
/// them is found on `PATH`, where the rows put the stand-ins.
#[test]
fn no_service_manager_is_named_by_an_absolute_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        root.join("../../cli/src/daemon.sh"),
        root.join("../../cli/src/daemon-install.sh"),
        root.join("src/families/daemon.rs"),
        root.join("src/families/daemon/life.rs"),
        root.join("src/families/daemon/install.rs"),
    ];
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        for name in NEVER_REAL.iter().filter(|n| **n != "git" && **n != "uname") {
            for dir in ["/bin/", "/sbin/", "/usr/local/bin/", "/opt/homebrew/bin/"] {
                let abs = format!("{dir}{name}");
                for (i, line) in text.lines().enumerate() {
                    if let Some(at) = line.find(&abs) {
                        let after = line[at + abs.len()..].chars().next();
                        assert!(
                            after
                                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                            "{}:{}: {abs}",
                            f.display(),
                            i + 1
                        );
                    }
                }
            }
        }
    }
}

/// The lines name the command by the brand's name.
#[test]
fn daemon_life_lines_say_the_brand_s_name() {
    let r = rig("brand");
    let cli = k2_client::brand::CLI_NAME;
    let rows: [(&'static [&'static str], String); 4] = [
        (&["nosuch"], format!("Usage: {cli} daemon <status|")),
        (
            &["install", "--bogus"],
            format!("{cli} daemon install: unknown argument: --bogus"),
        ),
        (
            &["stop", "x"],
            format!("\"hint\":\"{cli} daemon stop [--keep-agents]\""),
        ),
        (&["survive", "x"], format!("Usage: {cli} daemon survive")),
    ];
    for (args, want) in rows {
        let c = side_case(&r, &case("brand", args, &[], vec![], base, 0, 0), true);
        let (out, _) = r.binary(&c, false);
        let _ = r.take_tool_log();
        assert!(out.stderr.contains(&want), "{args:?}: {}", out.stderr);
        if cli != "k2" {
            assert!(
                !out.stderr.contains("k2 daemon"),
                "{args:?}: {}",
                out.stderr
            );
        }
    }
}

// ── daemon install ──────────────────────────────────────────────────
//
// Every row runs on the stand-ins: `curl` serves the fixtures below and
// never a network, `minisign` checks the stand-in signatures, the units
// are the `systemctl` stand-in's. HOME and the K2 home are the rig's.
// Both sides must make the same calls, with the same URLs (asserted),
// print the same, exit the same and leave the same files under HOME.

/// The release the fixtures are.
const FIX_VERSION: &str = "0.46.9";
/// The stand-in daemon binary.
const FIX_BIN: &str = "#!/bin/sh\necho stand-in k2-daemon\n";
/// The update keys, U2 then U3 (the shell's `_DAEMON_UPDATE_PUBKEYS`).
const U2: &str = k5_cli::families::daemon::install::UPDATE_KEYS[0];
const U3: &str = k5_cli::families::daemon::install::UPDATE_KEYS[1];

fn tagged_manifest(v: &str) -> String {
    format!(
        "{}/v{v}/daemon-latest.json",
        k2_client::brand::RELEASE_DOWNLOAD_BASE
    )
}

fn asset_url(platform: &str) -> String {
    format!(
        "{}/v{FIX_VERSION}/k2-daemon-{platform}",
        k2_client::brand::RELEASE_DOWNLOAD_BASE
    )
}

/// A fixture's file name: the URL with every byte but `[A-Za-z0-9._-]`
/// made `_` (the `curl` stand-in's `tr -c`).
fn fixture_name(url: &str) -> String {
    url.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

fn fixtures(r: &Rig) -> PathBuf {
    svc(r).join("fixtures")
}

fn serve(r: &Rig, url: &str, body: &str) {
    std::fs::write(fixtures(r).join(fixture_name(url)), body).unwrap();
}

/// The sha256 of `text`, by this machine's own tool.
fn sha256_hex(r: &Rig, text: &str) -> String {
    let f = r.dir.join("sha-input");
    std::fs::write(&f, text).unwrap();
    let o = match which("sha256sum") {
        Some(p) => std::process::Command::new(p).arg(&f).output().unwrap(),
        None => std::process::Command::new(which("shasum").expect("a sha256 tool"))
            .args(["-a", "256"])
            .arg(&f)
            .output()
            .unwrap(),
    };
    assert!(o.status.success());
    String::from_utf8(o.stdout).unwrap()[..64].to_string()
}

/// A stand-in signature: `line2` (`RUQ<key>` is a good one), a trusted
/// comment naming `file` and `version`, and the binary's first line.
fn minisig(line2: &str, file: &str, version: &str) -> String {
    format!(
        "untrusted comment: signature from a stand-in key\n{line2}\ntrusted comment: timestamp:1700000000\tfile:{file}\tversion:{version}\n{}\n",
        FIX_BIN.lines().next().unwrap()
    )
}

/// How a row's release looks.
struct Release {
    /// Where the manifest is served.
    at: String,
    version: &'static str,
    platform: &'static str,
    /// The signature's second line.
    line2: String,
    tc_file: String,
    tc_version: &'static str,
    /// The manifest's sha256, else the binary's.
    sha: Option<&'static str>,
    /// `sig2` holds the signature itself, base64.
    inline_b64: bool,
}

impl Release {
    fn good() -> Release {
        Release {
            at: k2_client::brand::DAEMON_FEED_URL.to_string(),
            version: FIX_VERSION,
            platform: "linux-x86_64",
            line2: format!("RUQ{U2}"),
            tc_file: "k2-daemon-linux-x86_64".into(),
            tc_version: FIX_VERSION,
            sha: None,
            inline_b64: false,
        }
    }
}

fn b64(text: &str) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = text.as_bytes();
    let mut o = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                o.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                o.push('=');
            }
        }
    }
    o
}

/// Every install row starts here: [`base`], nothing installed under HOME,
/// and an empty fixtures folder (which puts `curl` and `minisign` in
/// their install mode).
fn install_base(r: &Rig) {
    base(r);
    for d in [".local", ".config", "opt", "sys"] {
        let _ = std::fs::remove_dir_all(r.home.join(d));
    }
    let _ = std::fs::remove_dir_all(fixtures(r));
    std::fs::create_dir_all(fixtures(r)).unwrap();
}

fn publish(r: &Rig, rel: &Release) {
    let asset = asset_url(rel.platform);
    let sig_url = format!("{asset}.sig2");
    let sig = minisig(&rel.line2, &rel.tc_file, rel.tc_version);
    let sha = match rel.sha {
        Some(s) => s.to_string(),
        None => sha256_hex(r, FIX_BIN),
    };
    let sig2 = if rel.inline_b64 { b64(&sig) } else { sig_url.clone() };
    let manifest = format!(
        "{{\n  \"version\": \"{}\",\n  \"platforms\": {{\n    \"{}\": {{\n      \"url\": \"{asset}\",\n      \"sig\": \"{asset}.sig\",\n      \"sha256\": \"{sha}\",\n      \"sig2\": \"{sig2}\"\n    }}\n  }}\n}}\n",
        rel.version, rel.platform
    );
    serve(r, &rel.at, &manifest);
    serve(r, &asset, FIX_BIN);
    serve(r, &sig_url, &sig);
}

fn fx_latest(r: &Rig) {
    install_base(r);
    publish(r, &Release::good());
}

fn fx_tagged_backup_key(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            at: tagged_manifest(FIX_VERSION),
            line2: format!("RUQ{U3}"),
            ..Release::good()
        },
    );
}

fn fx_tagged_other_version(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            at: tagged_manifest(FIX_VERSION),
            version: "0.46.10",
            ..Release::good()
        },
    );
}

fn fx_bad_sig(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            line2: "RUQnot-an-update-key".into(),
            ..Release::good()
        },
    );
}

fn fx_legacy_sig(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            line2: format!("RWQ{U2}"),
            ..Release::good()
        },
    );
}

fn fx_other_file(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            tc_file: "k2-daemon-macos-aarch64".into(),
            ..Release::good()
        },
    );
}

fn fx_other_signed_version(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            tc_version: "0.46.8",
            ..Release::good()
        },
    );
}

fn fx_sha_mismatch(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            sha: Some("0000000000000000000000000000000000000000000000000000000000000000"),
            ..Release::good()
        },
    );
}

fn fx_retired(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            version: "0.46.5",
            ..Release::good()
        },
    );
}

fn fx_other_platform(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            platform: "linux-aarch64",
            ..Release::good()
        },
    );
}

fn fx_inline_sig(r: &Rig) {
    install_base(r);
    publish(
        r,
        &Release {
            inline_b64: true,
            ..Release::good()
        },
    );
}

fn fx_mac(r: &Rig) {
    install_base(r);
    put(r, "uname-s", "Darwin\n");
    publish(
        r,
        &Release {
            platform: "macos-x86_64",
            tc_file: "k2-daemon-macos-x86_64".into(),
            ..Release::good()
        },
    );
}

fn fx_user_unit(r: &Rig) {
    install_base(r);
    put(
        r,
        "systemd",
        "user k2-daemon.service active 4242 user.slice /user.slice/k2-daemon.service\n",
    );
    put(r, "systemd-exec", "ExecStart=-@%h/opt/k2-daemon --serve\n");
    publish(r, &Release::good());
}

fn fx_user_unit_no_exec(r: &Rig) {
    fx_user_unit(r);
    put(r, "systemd-exec", "[Service]\nExecStart=\n");
}

fn fx_system_unit_failing(r: &Rig) {
    install_base(r);
    put(
        r,
        "systemd",
        "system k2-daemon.service active 4242 system.slice /system.slice/k2-daemon.service\n",
    );
    put(r, "systemd-exec", "ExecStart=%h/sys/k2-daemon\n");
    put(r, "systemd-fail", "");
    publish(r, &Release::good());
}

/// No manifest served at all.
fn fx_none(r: &Rig) {
    install_base(r);
}

/// No daemon set up (no port, no token), a release served.
fn fx_no_daemon(r: &Rig) {
    fx_latest(r);
    for f in ["heartbeat.port", "heartbeat.token"] {
        let _ = std::fs::remove_file(r.k2home.join(f));
    }
}

/// A `file://` release in the rig's secrets folder.
fn fx_file_urls(r: &Rig) {
    install_base(r);
    let dir = r.secrets.display().to_string();
    let sig = minisig(&format!("RUQ{U2}"), "k2-daemon-linux-x86_64", FIX_VERSION);
    std::fs::write(r.secrets.join("k2-daemon-linux-x86_64"), FIX_BIN).unwrap();
    std::fs::write(r.secrets.join("k2-daemon.sig2"), sig).unwrap();
    let manifest = format!(
        "{{\"version\":\"{FIX_VERSION}\",\"platforms\":{{\"linux-x86_64\":{{\"url\":\"file://{dir}/k2-daemon-linux-x86_64\",\"sha256\":\"{}\",\"sig2\":\"file://{dir}/k2-daemon.sig2\"}}}}}}\n",
        sha256_hex(r, FIX_BIN)
    );
    std::fs::write(r.secrets.join("daemon-latest.json"), manifest).unwrap();
}

/// `file://` URLs to a signature that isn't there.
fn fx_file_sig_missing(r: &Rig) {
    fx_file_urls(r);
    std::fs::remove_file(r.secrets.join("k2-daemon.sig2")).unwrap();
}

/// What a side left under HOME, outside the K2 home: each file's path,
/// text and whether it can run.
fn installed(r: &Rig) -> Vec<(String, String, bool)> {
    fn walk(dir: &Path, root: &Path, skip: &Path, out: &mut Vec<(String, String, bool)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd {
            let p = e.unwrap().path();
            if p == skip {
                continue;
            }
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                walk(&p, root, skip, out);
            } else {
                use std::os::unix::fs::PermissionsExt;
                out.push((
                    p.strip_prefix(root).unwrap().display().to_string(),
                    String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned(),
                    m.permissions().mode() & 0o111 != 0,
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(&r.home, &r.home, &r.k2home, &mut out);
    out.sort();
    out
}

/// `line` with each side's `mktemp -d` folder written `{tmp}`.
fn mask_tmp(r: &Rig, line: &str) -> String {
    let mut line = line.to_string();
    for sub in ["tmp-bash", "tmp-rust", "tmp-nopy"] {
        let prefix = format!("{}/k2-daemon-install.", r.dir.join(sub).display());
        while let Some(at) = line.find(&prefix) {
            let end = (at + prefix.len() + 6).min(line.len());
            line.replace_range(at..end, "{tmp}");
        }
    }
    line
}

/// The stand-ins' calls, `curl` among them, with the temporary folder
/// masked. No token is in any of them.
fn install_calls(r: &Rig, name: &str) -> Vec<String> {
    let log = r.take_tool_log();
    for t in [OWNER, DAEMON_TOKEN, REMOTE_TOKEN] {
        assert!(
            !log.contains(t),
            "{name}: a token in a stand-in's arguments:\n{log}"
        );
    }
    log.lines()
        .filter(|l| STAND_INS.contains(&l.split(' ').next().unwrap_or("")))
        .map(|l| mask_tmp(r, l))
        .collect()
}

/// No side leaves its `mktemp -d` folder behind.
fn assert_no_scratch(r: &Rig, name: &str) {
    for sub in ["tmp-bash", "tmp-rust", "tmp-nopy"] {
        let Ok(rd) = std::fs::read_dir(r.dir.join(sub)) else {
            continue;
        };
        for e in rd {
            let n = e.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !n.starts_with("k2-daemon-install."),
                "{name}: {sub}/{n} was left behind"
            );
        }
    }
}

type InstallSeen = (
    Option<i32>,
    String,
    String,
    Vec<String>,
    Vec<String>,
    Vec<(String, String, bool)>,
);

/// Runs `c` on the shell, the binary and the binary with no `python3`,
/// and compares them; `want` is the calls (`{home}`, `{uid}`, `{secrets}`
/// and `{tmp}` filled in) the shell must make.
fn check_install(r: &Rig, c: &Case, want: &[String]) {
    let want: Vec<String> = want
        .iter()
        .map(|w| {
            w.replace("{home}", &r.home.to_string_lossy())
                .replace("{uid}", &uid())
                .replace("{secrets}", &r.secrets.to_string_lossy())
        })
        .collect();
    let bash_case = side_case(r, c, true);
    r.prepare(Side::Bash, &bash_case);
    let out = r.run(true, &bash_case);
    let log = r.read(Side::Bash, false, c.name);
    let bash: InstallSeen = (
        out.code,
        mask_tmp(r, &out.stdout),
        mask_tmp(r, &out.stderr),
        log.iter()
            .map(|l| normalise(l, false, VERB, c.name))
            .collect(),
        install_calls(r, c.name),
        installed(r),
    );
    println!("-- bash: {bash:?}");
    assert_eq!(bash.0, Some(c.code), "{}: bash's exit", c.name);
    assert!(bash.3.is_empty(), "{}: bash asked a daemon", c.name);
    assert_eq!(bash.4, want, "{}: bash's calls", c.name);
    assert_no_scratch(r, c.name);
    for (side, python) in [(Side::Rust, true), (Side::NoPython3, false)] {
        let run = side_case(r, c, python);
        let (out, log) = r.binary(&run, !python);
        assert!(
            !k5_cli::fallthrough::cache_dir(&r.k2home).exists(),
            "{}: {side:?} handed it to the shell",
            c.name
        );
        let got: InstallSeen = (
            out.code,
            mask_tmp(r, &out.stdout),
            mask_tmp(r, &out.stderr),
            log,
            install_calls(r, c.name),
            installed(r),
        );
        assert_eq!(got, bash, "{}: {side:?} against bash", c.name);
        assert_no_scratch(r, c.name);
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn fetch(url: &str) -> String {
    format!("curl -fsSL {url}")
}

fn fetch_to(url: &str, file: &str) -> String {
    format!("curl -fsSL {url} -o {{tmp}}/{file}")
}

fn verify_with(key: &str) -> String {
    format!("minisign -V -P {key} -m {{tmp}}/k2-daemon -x {{tmp}}/k2-daemon.minisig -Q")
}

/// The calls of a download of the good release's asset and signature.
fn downloads(platform: &str) -> Vec<String> {
    let a = asset_url(platform);
    vec![
        fetch_to(&a, "k2-daemon"),
        fetch_to(&format!("{a}.sig2"), "k2-daemon.minisig"),
    ]
}

#[test]
fn daemon_install_downloads_verifies_and_installs_as_the_shell_does() {
    let r = rig("install");
    let latest = fetch(k2_client::brand::DAEMON_FEED_URL);
    let tagged = fetch(&tagged_manifest(FIX_VERSION));
    let no_unit = s(NO_UNIT);
    let cat_then = |head: Vec<String>, tail: Vec<String>| -> Vec<String> {
        let mut v = head;
        v.extend(no_unit.clone());
        v.extend(tail);
        v
    };
    let mut good = downloads("linux-x86_64");
    good.push(verify_with(U2));
    let rows: Vec<(Case, Vec<String>)> = vec![
        (
            // native: daemon install
            case(
                "install: the newest release, no unit yet",
                &["install"],
                &[],
                vec![],
                fx_latest,
                0,
                0,
            ),
            cat_then(vec![latest.clone()], good.clone()),
        ),
        (
            // native: daemon install --version
            case(
                "install --version v0.46.9: signed with the backup key",
                &["install", "--version", "v0.46.9"],
                &[],
                vec![],
                fx_tagged_backup_key,
                0,
                0,
            ),
            cat_then(vec![tagged.clone()], {
                let mut v = downloads("linux-x86_64");
                v.push(verify_with(U2));
                v.push(verify_with(U3));
                v
            }),
        ),
        (
            case(
                "install: a signature no update key made is refused",
                &["install"],
                &[],
                vec![],
                fx_bad_sig,
                1,
                0,
            ),
            cat_then(vec![latest.clone()], {
                let mut v = downloads("linux-x86_64");
                v.push(verify_with(U2));
                v.push(verify_with(U3));
                v
            }),
        ),
        (
            case(
                "install: a signature that is not prehashed is refused",
                &["install"],
                &[],
                vec![],
                fx_legacy_sig,
                1,
                0,
            ),
            cat_then(vec![latest.clone()], downloads("linux-x86_64")),
        ),
        (
            case(
                "install: a signature for another file is refused",
                &["install"],
                &[],
                vec![],
                fx_other_file,
                1,
                0,
            ),
            cat_then(vec![latest.clone()], good.clone()),
        ),
        (
            case(
                "install: a signature for another version is refused",
                &["install"],
                &[],
                vec![],
                fx_other_signed_version,
                1,
                0,
            ),
            cat_then(vec![latest.clone()], good.clone()),
        ),
        (
            case(
                "install: a sha256 that differs is refused",
                &["install"],
                &[],
                vec![],
                fx_sha_mismatch,
                1,
                0,
            ),
            cat_then(vec![latest.clone()], good.clone()),
        ),
        (
            case(
                "install: the signature inline in the manifest, base64",
                &["install"],
                &[],
                vec![],
                fx_inline_sig,
                0,
                0,
            ),
            cat_then(
                vec![latest.clone()],
                vec![
                    fetch_to(&asset_url("linux-x86_64"), "k2-daemon"),
                    verify_with(U2),
                ],
            ),
        ),
        (
            // native: daemon install --no-service
            case(
                "install --no-service: no unit looked for or written",
                &["install", "--no-service"],
                &[],
                vec![],
                fx_latest,
                0,
                0,
            ),
            {
                let mut v = vec![latest.clone()];
                v.extend(good.clone());
                v
            },
        ),
        (
            // native: daemon install --bin-dir
            case(
                "install --bin-dir: a user unit's binary is updated in place and restarted",
                &["install", "--bin-dir", "{secrets}/bin"],
                &[],
                vec![],
                fx_user_unit,
                0,
                0,
            ),
            {
                let mut v = vec![latest.clone(), CAT_USER.into(), CAT_USER.into()];
                v.extend(good.clone());
                v.push("systemctl --user restart k2-daemon.service".into());
                v
            },
        ),
        (
            case(
                "install: a system unit whose restart needs root",
                &["install"],
                &[],
                vec![],
                fx_system_unit_failing,
                1,
                0,
            ),
            {
                let mut v = vec![
                    latest.clone(),
                    CAT_USER.into(),
                    CAT_USER_OLD.into(),
                    CAT_SYSTEM.into(),
                    CAT_SYSTEM.into(),
                ];
                v.extend(good.clone());
                v.push("systemctl restart k2-daemon.service".into());
                if uid() != "0" {
                    v.push("sudo -n systemctl restart k2-daemon.service".into());
                }
                v
            },
        ),
        (
            case(
                "install: a unit whose ExecStart= can't be read",
                &["install"],
                &[],
                vec![],
                fx_user_unit_no_exec,
                1,
                0,
            ),
            vec![latest.clone(), CAT_USER.into(), CAT_USER.into()],
        ),
        (
            case(
                "install on macOS: the launchd plist",
                &["install"],
                &[],
                vec![],
                fx_mac,
                0,
                0,
            ),
            {
                let mut v = vec![latest.clone()];
                v.extend(downloads("macos-x86_64"));
                v.push(verify_with(U2));
                v
            },
        ),
        (
            // native: daemon install --manifest-url
            case(
                "install --manifest-url file://: every file is copied",
                &["install", "--manifest-url", "file://{secrets}/daemon-latest.json"],
                &[],
                vec![],
                fx_file_urls,
                0,
                0,
            ),
            cat_then(vec![], vec![verify_with(U2)]),
        ),
        (
            case(
                "install --manifest-url file://: the signature isn't there",
                &["install", "--manifest-url", "file://{secrets}/daemon-latest.json"],
                &[],
                vec![],
                fx_file_sig_missing,
                1,
                0,
            ),
            no_unit.clone(),
        ),
    ];
    for (c, want) in rows {
        check_install(&r, &c, &want);
    }
}

#[test]
fn daemon_install_refusals_and_dry_runs_answer_as_the_shell_does() {
    let r = rig("install-no");
    let latest = fetch(k2_client::brand::DAEMON_FEED_URL);
    let mut with_units = vec![latest.clone()];
    with_units.extend(s(NO_UNIT));
    let rows: Vec<(Case, Vec<String>)> = vec![
        (
            // native: daemon install --dry-run
            case(
                "install --dry-run",
                &["install", "--dry-run"],
                &[],
                vec![],
                fx_latest,
                0,
                0,
            ),
            with_units.clone(),
        ),
        (
            case(
                "install --dry-run --no-service --bin-dir",
                &["install", "--dry-run", "--no-service", "--bin-dir", "{secrets}/bin"],
                &[],
                vec![],
                fx_latest,
                0,
                0,
            ),
            vec![latest.clone()],
        ),
        (
            case(
                "install --dry-run with a user unit",
                &["install", "--dry-run"],
                &[],
                vec![],
                fx_user_unit,
                0,
                0,
            ),
            vec![latest.clone(), CAT_USER.into(), CAT_USER.into()],
        ),
        (
            case(
                "install --dry-run on macOS",
                &["install", "--dry-run"],
                &[],
                vec![],
                fx_mac,
                0,
                0,
            ),
            vec![latest.clone()],
        ),
        (
            case(
                "install --dry-run with no daemon set up: no connection gate",
                &["install", "--dry-run"],
                &[],
                vec![],
                fx_no_daemon,
                0,
                0,
            ),
            with_units.clone(),
        ),
        (
            case(
                "install --version 0.46.5: the retired key, nothing fetched",
                &["install", "--version", "0.46.5"],
                &[],
                vec![],
                fx_latest,
                1,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install --version banana: not a version, nothing fetched",
                &["install", "--version", "banana"],
                &[],
                vec![],
                fx_latest,
                1,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install: the newest manifest is older than the update keys",
                &["install"],
                &[],
                vec![],
                fx_retired,
                1,
                0,
            ),
            vec![latest.clone()],
        ),
        (
            case(
                "install: no artifact for this platform",
                &["install"],
                &[],
                vec![],
                fx_other_platform,
                1,
                0,
            ),
            vec![latest.clone()],
        ),
        (
            case(
                "install --version: the manifest is for another release",
                &["install", "--version", "0.46.9"],
                &[],
                vec![],
                fx_tagged_other_version,
                1,
                0,
            ),
            vec![fetch(&tagged_manifest(FIX_VERSION))],
        ),
        (
            case(
                "install: no manifest served",
                &["install"],
                &[],
                vec![],
                fx_none,
                1,
                0,
            ),
            vec![latest.clone()],
        ),
        (
            case(
                "install --manifest-url file:// that isn't there",
                &["install", "--manifest-url", "file://{secrets}/nope.json"],
                &[],
                vec![],
                fx_none,
                1,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install --bogus",
                &["install", "--bogus"],
                &[],
                vec![],
                fx_latest,
                2,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install --bin-dir as the last word",
                &["install", "--no-service", "--bin-dir"],
                &[],
                vec![],
                fx_latest,
                1,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install with air-gap on",
                &["install", "--bogus"],
                &[("K2_AIRGAP", "1")],
                vec![],
                fx_latest,
                1,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install: the guard",
                &["install", "--dry-run"],
                &[("K2_NO_OWNER_FALLBACK", "1")],
                vec![],
                port_only_install,
                3,
                0,
            ),
            vec![],
        ),
        (
            case(
                "install: a session with no passport",
                &["install", "--dry-run"],
                &[("K2_HOOK_SOCK", "{sock}")],
                vec![],
                fx_latest,
                3,
                0,
            ),
            vec![],
        ),
    ];
    for (c, want) in rows {
        check_install(&r, &c, &want);
    }
}

/// A release served, and a port but no token.
fn port_only_install(r: &Rig) {
    fx_latest(r);
    let _ = std::fs::remove_file(r.k2home.join("heartbeat.token"));
    let _ = std::fs::remove_file(r.k2home.join("daemon.token"));
}

/// Help never acts: `install -h` anywhere is the shell's help, with no
/// fetch, no service call and nothing written.
#[test]
fn daemon_install_help_never_acts() {
    let r = rig("install-help");
    let rows: Vec<Case> = vec![
        case("install -h", &["install", "-h"], &[], vec![], fx_user_unit, 0, 0),
        case(
            "install --dry-run --help",
            &["install", "--dry-run", "--help"],
            &[],
            vec![],
            fx_user_unit,
            0,
            0,
        ),
    ];
    for c in rows {
        check(&r, &c, &[], false);
        assert!(installed(&r).is_empty(), "{}: wrote a file", c.name);
    }
}
