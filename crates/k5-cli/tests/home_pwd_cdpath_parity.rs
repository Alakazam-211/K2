//! HOME unset, `CDPATH` set and a `PWD` bash would tidy: lines the binary
//! used to hand to the shell in those environments, now its own, against
//! the shell CLI's on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code. The rules are shared ones:
//!
//! - `PWD` ([`k5_cli::offline::shell_pwd`]): bash keeps an exported `PWD`
//!   that names this folder; bash 3.2 (macOS's `/bin/bash`) prints it as
//!   it is, bash 4.4 and later (Linux) tidy it for `pwd`. Each OS's rows
//!   run on that OS, with that bash first on `PATH` (the test fails
//!   otherwise).
//! - HOME unset ([`k5_cli::offline::home_check`]): bash's prelude stops
//!   under `set -u` at its K2 home, for every command; the binary refuses
//!   at its front, in K2's words (the named difference
//!   `home-unset-prefix`), and reads and sends nothing first. An empty
//!   `HOME` is a `HOME`.
//! - `CDPATH` ([`k5_cli::shellpath::cdpath`]): searched only for a
//!   relative name not starting with `.` or `..` parts; no entry has it,
//!   `cd` goes as with none; a non-empty entry that has it is where `cd`
//!   goes, and `cd` prints it (`$(cd x && pwd)` then holds it twice).

#![cfg(unix)]

mod common;

use common::parity::*;

const ON: &str = r#"{"enabled":true}"#;

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// The bash the rig runs is the one the binary models here: macOS's
/// `/bin/bash` 3.2, or bash 4.4 or later elsewhere.
fn bash_is_the_modelled_one() {
    let v = bash_version();
    if cfg!(target_os = "macos") {
        assert_eq!(
            v,
            (3, 2),
            "the PWD rows model macOS's /bin/bash 3.2: put /bin first on PATH"
        );
    } else {
        assert!(v >= (4, 4), "the PWD rows model bash 4.4 or later: {v:?}");
    }
}

/// Each `PWD` shape, the plain one first.
fn pwd_shapes(rig: &Rig) -> Vec<(&'static str, String)> {
    let p = rig.proj.to_string_lossy().into_owned();
    let last = rig.proj.file_name().unwrap().to_string_lossy().into_owned();
    vec![
        ("plain", p.clone()),
        ("/.", format!("{p}/.")),
        ("..", format!("{p}/../{last}")),
        ("a leading //", format!("/{p}")),
        ("a trailing /", format!("{p}/")),
    ]
}

/// A page and a verb that sends `$(pwd)`, under each `PWD` shape.
#[test]
fn a_pwd_bash_would_tidy() {
    bash_is_the_modelled_one();
    let rig = Rig::new("permission", "pwd");
    for (tag, pwd) in pwd_shapes(&rig) {
        rig.check(&with(
            vec![("PWD", pwd)],
            case(
                leak(format!("permission help, PWD {tag}")),
                &["help"],
                &[],
                vec![ok(ON)],
                owner_and_port,
                0,
                1,
            ),
        ));
    }
    let rig = Rig::new("sources", "pwd");
    for (tag, pwd) in pwd_shapes(&rig) {
        rig.check(&with(
            vec![("PWD", pwd)],
            case(
                leak(format!("sources access on, this folder, PWD {tag}")),
                &["access", "on"],
                &[],
                vec![ok(r#"{"workspace":"acme","agentsCanManageGardens":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ));
    }
}

/// No `HOME`: every form stops at the front (`home-unset-prefix`, K2's
/// ruling): one kind of form each, a page, a daemon call, an offline line
/// and `--version`. Nothing is sent.
#[test]
fn home_unset_is_refused_at_the_front() {
    let rows: Vec<(&'static str, &'static [&'static str])> = vec![
        ("permission", &["help"]),
        ("agent", &["list"]),
        ("study", &["errors"]),
        ("whoami", &[]),
        ("", &["--version"]),
    ];
    for (verb, args) in rows {
        let rig = Rig::new(verb, "nohome");
        rig.check(&with(
            vec![("HOME", UNSET.to_string()), ("K2_PORT", PORT.to_string())],
            Case {
                compare: Compare::HomeUnset,
                ..case(
                    leak(format!("{verb} {}, HOME unset", args.join(" "))),
                    args,
                    &[],
                    vec![],
                    owner_and_port,
                    1,
                    0,
                )
            },
        ));
    }
}

/// A `K*_PORT` setting that is no port is refused before `HOME` is read,
/// on both sides (exit 2).
#[test]
fn a_bad_port_setting_comes_first() {
    let rig = Rig::new("agent", "nohome-port");
    rig.check(&with(
        vec![("HOME", UNSET.to_string()), ("K2_PORT", "70000".to_string())],
        case(
            "agent list, HOME unset, K2_PORT 70000",
            &["list"],
            &[],
            vec![],
            nothing,
            2,
            0,
        ),
    ));
}

/// `HOME` set but empty: `set -u` doesn't trip on it; both read the K2
/// home `/.k2` (none) and go on with the settings given.
#[test]
fn an_empty_home_is_a_home() {
    for (verb, args, replies, requests) in [
        ("permission", &["help"][..], vec![ok(ON)], 1),
        ("agent", &["list"][..], vec![ok(r#"{"ok":true,"agents":[]}"#)], 1),
    ] {
        let rig = Rig::new(verb, "emptyhome");
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&with(
            with_passport(&[("HOME", ""), ("K2_NO_OWNER_FALLBACK", "1")]),
            case(
                leak(format!("{verb} {}, HOME empty", args.join(" "))),
                args,
                &[],
                replies,
                nothing,
                0,
                requests,
            ),
        ));
    }
}

/// A file a read would hang on.
fn fifo(p: &std::path::Path) {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid C string and a plain mode.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo {}", p.display());
}

/// With no `HOME`, the binary reads no K2 home, token or passport file and
/// sends nothing before its refusal, for every router form (and its
/// `--help`): every place a read could reach holds FIFOs (a read hangs, the
/// run passes its deadline and the test fails), and the stub must get no
/// request.
#[test]
fn home_unset_reads_and_sends_nothing() {
    let rig = Rig::new("homefifo", "all");
    let k2home = rig.dir.join("fifo-k2home");
    let secrets = rig.dir.join("fifo-secrets");
    for d in [&k2home, &secrets, &rig.proj.join(".k2")] {
        std::fs::create_dir_all(d).unwrap();
    }
    for f in [
        "heartbeat.port",
        "heartbeat.token",
        "daemon.port",
        "daemon.token",
        "settings.json",
        "tunnel.json",
        "connect-account.json",
    ] {
        fifo(&k2home.join(f));
    }
    fifo(&secrets.join(".k2-passport"));
    let port = rig.stub().port.to_string();
    let table = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/router_forms.tsv");
    let forms = std::fs::read_to_string(table).expect("router_forms.tsv");
    let mut lines: Vec<Vec<String>> = Vec::new();
    for row in forms.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let r: Vec<&str> = row.split('\t').collect();
        if r[2] != "-" {
            continue;
        }
        let words: Vec<String> = (if r[1] == "-" { r[0] } else { r[1] })
            .split_whitespace()
            .map(String::from)
            .collect();
        let mut help = words.clone();
        help.push("--help".into());
        lines.push(words);
        lines.push(help);
    }
    assert!(lines.len() > 3000, "only {} lines", lines.len());
    let want = format!("{}\n", k5_cli::offline::HOME_UNSET_LINE);
    for argv in &lines {
        let mut cmd = std::process::Command::new(BIN);
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH"))
            .env("TMPDIR", rig.dir.join("tmp-rust"))
            .env("K5_HOME_OVERRIDE", &k2home)
            .env("K2_SECRETS_DIR", &secrets)
            .env("K2_PORT", &port)
            .env("K2_HOOK_TOKEN", PASSPORT)
            .current_dir(&rig.proj)
            .args(argv);
        let who = format!("{}: HOME unset", argv.join(" "));
        let o = run_within(&mut cmd, None, std::time::Duration::from_secs(20), &who);
        let got = (
            o.status.code(),
            o.stdout.is_empty(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        );
        assert_eq!(got, (Some(1), true, want.clone()), "{who}");
    }
    assert!(rig.stub().take_log().is_empty(), "a request reached the stub");
}

/// A `CDPATH` folder with `ws/` in it.
fn cdpath_folder(r: &Rig) -> String {
    let d = r.dir.join("cdpath");
    std::fs::create_dir_all(d.join("ws")).unwrap();
    d.to_string_lossy().into_owned()
}

/// Rows with `CDPATH` set: a folder no entry has (`cd` as with none), and
/// for onboarding one an entry has (`cd` goes there and prints it).
#[test]
fn cdpath_set() {
    let rig = Rig::new("onboarding", "cdpath");
    let cdp = cdpath_folder(&rig);
    std::fs::create_dir_all(rig.proj.join("ws")).unwrap();
    let scan = |name: &'static str, args: &'static [&'static str], cdpath: String| {
        with(
            vec![("CDPATH", cdpath)],
            case(
                name,
                args,
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        )
    };
    for c in [
        scan("onboarding scan, CDPATH set", &["scan"], cdp.clone()),
        scan(
            "onboarding scan, CDPATH with . first",
            &["scan"],
            format!(".:{cdp}"),
        ),
        scan(
            "onboarding scan ws, CDPATH has it: cd prints it",
            &["scan", "ws"],
            cdp.clone(),
        ),
        scan(
            "onboarding scan ws, an empty CDPATH entry has it: no line",
            &["scan", "ws"],
            format!(":{cdp}"),
        ),
        scan(
            "onboarding adopt, CDPATH has the folder",
            &["adopt", "CLAUDE.md", "ws"],
            format!("/nonexistent-k5-cdpath:{cdp}"),
        ),
        scan(
            "onboarding scan .k2, no CDPATH entry has it",
            &["scan", ".k2"],
            cdp.clone(),
        ),
    ] {
        rig.check(&c);
    }
    let rows: Vec<(&'static str, Case)> = vec![
        (
            "heartbeat",
            case(
                "heartbeat list --workspace .k2, CDPATH set",
                &["list", "--workspace", ".k2"],
                &[],
                vec![ok(r#"{"heartbeats":[]}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        (
            "remind",
            case(
                "remind list --workspace .k2, CDPATH set",
                &["list", "--workspace", ".k2"],
                &[],
                vec![ok(r#"{"reminders":[]}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        (
            "db",
            case(
                "db --workspace .k2 list, CDPATH set",
                &["--workspace", ".k2", "list"],
                &[],
                vec![ok(r#"{"ok":true,"databases":[]}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        (
            "workspace",
            case(
                "workspace open .k2, CDPATH set",
                &["open", ".k2"],
                &[],
                vec![ok(r#"{"ok":true,"workspace":{"id":"w1","name":"x","path":"/srv/x"}}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        (
            "changes",
            case(
                "changes list --workspace .k2, CDPATH set",
                &["list", "--workspace", ".k2"],
                &[],
                vec![ok(r#"{"isRepo":false,"workspacePath":"/w"}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
    ];
    for (verb, c) in rows {
        let rig = Rig::new(verb, "cdpath");
        let cdp = cdpath_folder(&rig);
        rig.check(&with(vec![("CDPATH", cdp)], c));
    }
}

fn with_dash_folder(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("-ws")).unwrap();
}

/// A `--workspace` folder named as an option `cd` doesn't take: `cd`'s
/// two lines and its exit under `set -e`, nothing sent; bash's first line
/// has the script's path and line before it (`cd-option-prefix`).
#[test]
fn a_folder_cd_reads_as_a_bad_option() {
    bash_is_the_modelled_one();
    let exit = if bash_version() < (4, 0) { 1 } else { 2 };
    for (verb, args) in [
        ("changes", &["list", "--workspace", "-ws"][..]),
        ("sim", &["status", "--workspace", "-ws"][..]),
        ("wiki", &["status", "--workspace", "-ws"][..]),
    ] {
        let rig = Rig::new(verb, "dash");
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&Case {
            compare: Compare::CdOptionPrefix,
            ..case(
                leak(format!("{verb} {}", args.join(" "))),
                args,
                &[],
                vec![],
                with_dash_folder,
                exit,
                0,
            )
        });
    }
}

fn with_dash_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("-app.apk"), "x").unwrap();
}

/// `sim install` with a path starting with `-` (the named difference
/// `sim-install-dash-path`, K2's ruling 1): bash's `dirname`, `basename`
/// and `cd` read it as their options and bash sends the install with a
/// mangled path; the binary refuses it, exit 2, and sends nothing.
#[test]
fn sim_install_refuses_a_dash_path() {
    let rig = Rig::new("sim", "dashpath");
    let c = case(
        "sim install -app.apk",
        &["install", "-app.apk"],
        &[],
        vec![ok("{}")],
        with_dash_file,
        0,
        1,
    );
    rig.prepare(Side::Bash, &c);
    let b = rig.run(true, &c);
    let sent = rig.read(Side::Bash, false, c.name);
    assert_eq!(sent.len(), 1, "bash sends the install: {b:?}");
    let head = String::from_utf8_lossy(&sent[0].bytes).into_owned();
    assert!(
        head.starts_with("POST /cli/sim/install "),
        "bash's request: {head}"
    );
    assert!(
        !head.contains(&*rig.proj.join("-app.apk").to_string_lossy()),
        "bash sent the path as it is: the difference no longer holds: {head}"
    );
    let r = Case {
        replies: vec![],
        ..c
    };
    rig.prepare(Side::Rust, &r);
    let out = rig.run(false, &r);
    let sent = rig.read(Side::Rust, true, r.name);
    assert!(sent.is_empty(), "the binary sent {} request(s)", sent.len());
    let cli = k2_client::cli_name!();
    let hint = format!(
        "{cli} sim install: a path starting with \"-\" can't be read safely; use ./-app.apk instead"
    );
    assert_eq!(
        (out.code, out.stdout.as_str(), out.stderr.as_str()),
        (
            Some(2),
            "",
            format!(
                "Error: {hint}\n{{\"error\": {{\"code\": \"usage\", \"hint\": \"{}\"}}}}\n",
                hint.replace('"', "\\\"")
            )
            .as_str()
        ),
        "the binary's refusal"
    );
}
