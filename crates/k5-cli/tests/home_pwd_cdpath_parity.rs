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
//! - HOME unset ([`k5_cli::offline::shell_home`]): bash sets `HOME` to
//!   this user's folder in the password database. The rows read that
//!   folder for real, so the test first fails when it holds a K2 home,
//!   and every row brings its own port (the stub's) and a made-up
//!   passport, with the owner fallback off: nothing real is read or
//!   reached. Rows are picked whose output names no folder of that home.
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

/// The password database's folder for this user holds no K2 home (the
/// test says only whether one is there, never the folder).
fn passwd_home_has_no_k2_home() {
    let home = k5_cli::offline::system_passwd_home()
        .expect("the password database's folder for this user, as text")
        .expect("an entry for this user in the password database");
    for name in [".k2", ".k2so"] {
        let there = std::path::Path::new(&home).join(name).exists();
        assert!(
            !there,
            "the password database's home folder holds {name} (present): the HOME-unset rows would read it"
        );
    }
}

/// Settings with no `HOME`: the stub's port, a made-up passport, the
/// owner fallback off.
fn no_home() -> Vec<(&'static str, String)> {
    with_passport(&[("K2_NO_OWNER_FALLBACK", "1"), ("HOME", UNSET)])
}

#[test]
fn home_unset() {
    passwd_home_has_no_k2_home();
    let rows: Vec<(&'static str, Case)> = vec![
        (
            "permission",
            case(
                "permission help, HOME unset",
                &["help"],
                &[],
                vec![ok(ON)],
                nothing,
                0,
                1,
            ),
        ),
        (
            "agent",
            case(
                "agent list, HOME unset",
                &["list"],
                &[],
                vec![ok(r#"{"ok":true,"agents":[]}"#)],
                nothing,
                0,
                1,
            ),
        ),
        (
            "focus-group",
            case(
                "focus-group list, HOME unset",
                &["list"],
                &[],
                vec![ok(r#"{"ok":true,"groups":[]}"#)],
                nothing,
                0,
                1,
            ),
        ),
        (
            "publish",
            case(
                "publish subdomain list, HOME unset",
                &["subdomain", "list"],
                &[],
                vec![ok(r#"{"ok":true,"subdomains":[]}"#)],
                nothing,
                0,
                1,
            ),
        ),
    ];
    for (verb, c) in rows {
        let rig = Rig::new(verb, "nohome");
        rig.check(&with(no_home(), c));
    }
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
