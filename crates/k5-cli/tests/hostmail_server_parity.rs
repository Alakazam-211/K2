//! `k5-cli hostmail enable`, `disable`, `uninstall`, `upgrade`,
//! `rotate-admin`, `password`, `app-password`, `person` and `import`
//! against the shell CLI's `cli/k2 hostmail …`, on the parity rig
//! ([`common::parity`]): the same requests (urllib's, from `_mail_py`),
//! stdout, stderr and exit code.
//!
//! - The words: each group's help with no words, each usage line, exit 2
//!   and nothing sent.
//! - Each verb's answer, plain and `--json`; upgrade's dry run, its watch
//!   to the end (succeeded, rolled back with the manual steps), and the
//!   no-op.
//! - A refusal with the permission card, answers the Python raises on,
//!   and no answer.
//! - `import --pass-stdin`: the password read as `$(cat)` reads it, sent
//!   in the request's body, and on no child's argv or environment.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::os::unix::fs::PermissionsExt;

const VERB: &str = "hostmail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn fail_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

fn help_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 0, 0)
}

fn traceback(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    compare(
        Compare::Traceback,
        case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1),
    )
}

fn piped(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

/// The upgrade watch's poll, short.
const POLL: (&str, &str) = ("K2_HOSTMAIL_UPGRADE_POLL", "0.01");

fn watch(name: &'static str, args: &'static [&'static str], replies: &[&str], code: i32) -> Case {
    case(
        name,
        args,
        &[POLL],
        replies.iter().map(|b| ok(b)).collect(),
        owner_and_port,
        code,
        replies.len(),
    )
}

#[test]
fn hostmail_server_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "swords");
    let v = vec![
        help_case("password alone: the help", &["password"]),
        help_case("app-password alone: the help", &["app-password"]),
        help_case("person alone: the help", &["person"]),
        help_case("password --help", &["password", "--help"]),
        help_case("app-password -h", &["app-password", "-h"]),
        help_case("person link -h", &["person", "link", "-h"]),
        usage_case("enable, no hostname", &["enable"]),
        usage_case(
            "enable, --hostname with no value",
            &["enable", "--hostname"],
        ),
        usage_case("enable, a stray word", &["enable", "mail.acme.example"]),
        usage_case("enable, an unknown flag", &["enable", "--name", "x"]),
        usage_case("disable, a stray word", &["disable", "now"]),
        usage_case(
            "uninstall, no --purge",
            &["uninstall", "--confirm-hostname", "h"],
        ),
        usage_case("uninstall, no hostname", &["uninstall", "--purge"]),
        usage_case(
            "uninstall, --confirm-hostname with no value",
            &["uninstall", "--purge", "--confirm-hostname"],
        ),
        usage_case("uninstall, a stray word", &["uninstall", "x"]),
        usage_case(
            "upgrade, dry run and acknowledge",
            &["upgrade", "--dry-run", "--acknowledge-failed"],
        ),
        usage_case("upgrade, an unknown flag", &["upgrade", "--force"]),
        usage_case("rotate-admin, a stray word", &["rotate-admin", "x"]),
        usage_case("password, an unknown sub-command", &["password", "set"]),
        usage_case("password rotate, no address", &["password", "rotate"]),
        usage_case(
            "password rotate, two",
            &["password", "rotate", "a@x", "b@x"],
        ),
        usage_case(
            "password rotate, a flag",
            &["password", "rotate", "a@x", "--all"],
        ),
        usage_case("password keep, nothing", &["password", "keep", "--json"]),
        usage_case(
            "password keep, all and an address",
            &["password", "keep", "a@x", "--all-existing"],
        ),
        usage_case(
            "app-password, an unknown sub-command",
            &["app-password", "rm"],
        ),
        usage_case("app-password add, no address", &["app-password", "add"]),
        usage_case(
            "app-password add, --label with no value",
            &["app-password", "add", "a@x", "--label"],
        ),
        usage_case(
            "app-password add, two",
            &["app-password", "add", "a@x", "b@x"],
        ),
        usage_case("app-password list, a flag", &["app-password", "list", "-x"]),
        usage_case(
            "app-password revoke, no id",
            &["app-password", "revoke", "a@x"],
        ),
        usage_case(
            "app-password revoke, three",
            &["app-password", "revoke", "a@x", "1", "2"],
        ),
        usage_case("app-password keep, no id", &["app-password", "keep", "a@x"]),
        usage_case(
            "app-password keep, all and an address",
            &["app-password", "keep", "--all-existing", "a@x"],
        ),
        usage_case("person, no state", &["person", "a@x"]),
        usage_case(
            "person, a state it doesn't know",
            &["person", "a@x", "maybe"],
        ),
        usage_case(
            "person, --all with no workspace",
            &["person", "--all", "on"],
        ),
        usage_case(
            "person, an address with --workspace",
            &["person", "a@x", "on", "--workspace", "w"],
        ),
        usage_case("person, three words", &["person", "a@x", "on", "off"]),
        usage_case("person, an unknown flag", &["person", "a@x", "on", "--x"]),
        usage_case(
            "person, --workspace with no value",
            &["person", "--all", "on", "--workspace"],
        ),
        usage_case("person link, no address", &["person", "link"]),
        usage_case("person link, no login", &["person", "link", "a@x"]),
        usage_case(
            "person link, a login and --unlink",
            &["person", "link", "a@x", "bob", "--unlink"],
        ),
        usage_case(
            "person link, three words",
            &["person", "link", "a@x", "bob", "carol"],
        ),
        usage_case("import, no address", &["import", "--maildir", "/m"]),
        usage_case("import, no source", &["import", "--address", "a@x"]),
        usage_case(
            "import, --maildir with no value",
            &["import", "a@x", "--maildir"],
        ),
        usage_case(
            "import, --imapsync with no value",
            &["import", "a@x", "--imapsync"],
        ),
        usage_case(
            "import, an unknown flag",
            &["import", "a@x", "--maildir", "/m", "--x"],
        ),
        usage_case("import, two addresses", &["import", "a@x", "b@x"]),
        piped(
            b"\n\n",
            usage_case(
                "import, an empty password piped",
                &["import", "a@x", "--imapsync", "imap.x", "--pass-stdin"],
            ),
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

const ROTATED: &str = r#"{"ok":true,"address":"sales@acme.example","password":"pw-1","username":"sales","imap":{"host":"mail.acme.example","port":993,"alpn":true},"submission":{"host":"mail.acme.example"},"apiKeysRevoked":2,"apiKeysWarning":"one key is still in use"}"#;

const DRY: &str = r#"{"ok":true,"dryRun":true,"ready":false,"from":"0.11.8","to":"0.12.1","state":"running","wasRunning":true,"blockers":[{"code":"disk","hint":"free some space"}],"dataBytes":1536,"configBytes":100,"snapshotDir":"/var/snap","freeBytes":5368709120,"outage":{"typicalSecs":20,"worstCaseSecs":90,"assumption":"a small store"},"plan":["stop","copy","start"],"rollback":["restore the copy"],"notes":["a note"]}"#;

#[test]
fn hostmail_server_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "sanswer");
    let v = vec![
        // covers: hostmail enable
        ok_case(
            "enable",
            &["enable", "--hostname", "mail.acme.example"],
            r#"{"ok":true,"helper":"/usr/local/libexec/mail-helper","hint":"enable requested."}"#,
        ),
        ok_case(
            "enable, no hint",
            &["enable", "--hostname", "mail.acme.example"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "enable, already on",
            &["enable", "--hostname", "mail.acme.example"],
            r#"{"ok":true,"alreadyEnabled":true}"#,
        ),
        // covers: hostmail enable --json
        ok_case(
            "enable --json",
            &["enable", "--json", "--hostname", "mail.acme.example"],
            r#"{"ok":true,"alreadyEnabled":true}"#,
        ),
        ok_case("disable", &["disable"], r#"{"ok":true}"#),
        ok_case(
            "disable --json",
            &["disable", "--json"],
            r#"{"ok":true,"hint":"off"}"#,
        ),
        // covers: hostmail uninstall
        ok_case(
            "uninstall",
            &[
                "uninstall",
                "--purge",
                "--confirm-hostname",
                "mail.acme.example",
            ],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail uninstall --json
        ok_case(
            "uninstall --json",
            &["uninstall", "--confirm-hostname", "h", "--purge", "--json"],
            r#"{"ok":true,"hint":"gone"}"#,
        ),
        // covers: hostmail upgrade
        ok_case(
            "upgrade --acknowledge-failed",
            &["upgrade", "--acknowledge-failed"],
            r#"{"ok":true}"#,
        ),
        fail_case("upgrade --dry-run, blocked", &["upgrade", "--dry-run"], DRY),
        ok_case(
            "upgrade --dry-run, the no-op",
            &["upgrade", "--dry-run"],
            r#"{"ok":true,"outcome":"noop"}"#,
        ),
        // covers: hostmail upgrade --json
        ok_case(
            "upgrade --dry-run --json, ready",
            &["upgrade", "--dry-run", "--json"],
            r#"{"ok":true,"dryRun":true,"ready":true}"#,
        ),
        ok_case(
            "upgrade --dry-run, ready, little data",
            &["upgrade", "--dry-run"],
            r#"{"ok":true,"ready":true,"dataBytes":0,"freeBytes":null,"outage":"soon","plan":[]}"#,
        ),
        ok_case(
            "upgrade, the no-op",
            &["upgrade"],
            r#"{"ok":true,"outcome":"noop"}"#,
        ),
        // covers: hostmail rotate-admin
        ok_case("rotate-admin", &["rotate-admin"], r#"{"ok":true}"#),
        // covers: hostmail rotate-admin --json
        ok_case(
            "rotate-admin --json",
            &["rotate-admin", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail password rotate
        ok_case(
            "password rotate",
            &["password", "rotate", "sales@acme.example"],
            ROTATED,
        ),
        ok_case(
            "password rotate, little known",
            &["password", "rotate", "sales@acme.example"],
            r#"{"ok":true,"imap":null}"#,
        ),
        // covers: hostmail password rotate --json
        ok_case(
            "password rotate --json",
            &["password", "rotate", "--json", "sales@acme.example"],
            ROTATED,
        ),
        // covers: hostmail password keep
        ok_case(
            "password keep",
            &["password", "keep", "sales@acme.example"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "password keep --all-existing",
            &["password", "keep", "--all-existing"],
            r#"{"ok":true,"kept":{"mailboxPasswords":4},"leftFlagged":{"createdAfterCutoff":1,"noCreationTime":"2"},"listErrors":["acme.example: down"]}"#,
        ),
        // covers: hostmail password keep --json
        ok_case(
            "password keep --all-existing --json",
            &["password", "keep", "--all-existing", "--json"],
            r#"{"ok":true,"kept":{}}"#,
        ),
        // covers: hostmail app-password add
        ok_case(
            "app-password add",
            &[
                "app-password",
                "add",
                "sales@acme.example",
                "--label",
                "phone",
            ],
            r#"{"ok":true,"id":"ap_1","label":"phone","secret":"s3cr3t"}"#,
        ),
        // covers: hostmail app-password add --json
        ok_case(
            "app-password add --json, no label",
            &["app-password", "add", "sales@acme.example", "--json"],
            r#"{"ok":true,"id":"ap_2"}"#,
        ),
        ok_case(
            "app-password add, nothing back",
            &["app-password", "add", "sales@acme.example"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail app-password list
        ok_case(
            "app-password list",
            &["app-password", "list", "sales@acme.example"],
            r#"{"ok":true,"appPasswords":[{"id":"ap_1","label":"phone","createdAt":"2026-01-02"},{"id":"ap_2"}]}"#,
        ),
        ok_case(
            "app-password list, none",
            &["app-password", "list", "sales@acme.example"],
            r#"{"ok":true,"appPasswords":[]}"#,
        ),
        // covers: hostmail app-password list --json
        ok_case(
            "app-password list --json",
            &["app-password", "list", "sales@acme.example", "--json"],
            r#"{"ok":true,"appPasswords":[]}"#,
        ),
        // covers: hostmail app-password revoke
        ok_case(
            "app-password revoke",
            &["app-password", "revoke", "sales@acme.example", "ap_1"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail app-password revoke --json
        ok_case(
            "app-password revoke --json",
            &[
                "app-password",
                "revoke",
                "sales@acme.example",
                "ap_1",
                "--json",
            ],
            r#"{"ok":true,"id":"ap_1"}"#,
        ),
        // covers: hostmail app-password keep
        ok_case(
            "app-password keep",
            &["app-password", "keep", "sales@acme.example", "ap_1"],
            r#"{"ok":true,"appPasswordId":"ap_1"}"#,
        ),
        ok_case(
            "app-password keep --all-existing",
            &["app-password", "keep", "--all-existing"],
            r#"{"ok":true,"kept":{"appPasswords":0},"leftFlagged":{}}"#,
        ),
        // covers: hostmail app-password keep --json
        ok_case(
            "app-password keep --json",
            &[
                "app-password",
                "keep",
                "sales@acme.example",
                "ap_1",
                "--json",
            ],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail person
        ok_case(
            "person on",
            &["person", "sales@acme.example", "on"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "person off",
            &["person", "sales@acme.example", "off"],
            r#"{"ok":true,"address":"sales@acme.example"}"#,
        ),
        // covers: hostmail person --json
        ok_case(
            "person --json",
            &["person", "--json", "sales@acme.example", "on"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "person --all",
            &["person", "--all", "--workspace", "alpha", "on"],
            r#"{"ok":true,"changed":["a@acme.example","b@acme.example"],"excludedSendIdentities":["bot@acme.example"]}"#,
        ),
        ok_case(
            "person --all off, none changed",
            &["person", "off", "--all", "--workspace", "alpha"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "person link",
            &["person", "link", "sales@acme.example", "bob"],
            r#"{"ok":true,"person":"bob"}"#,
        ),
        ok_case(
            "person link --unlink",
            &["person", "link", "sales@acme.example", "--unlink"],
            r#"{"ok":true,"person":null}"#,
        ),
        ok_case(
            "import a maildir",
            &[
                "import",
                "--address",
                "sales@acme.example",
                "--maildir",
                "/m",
                "--skip-inbox",
            ],
            r#"{"ok":true,"imported":12}"#,
        ),
        ok_case(
            "import --json",
            &["import", "sales@acme.example", "--maildir", "/m", "--json"],
            r#"{"ok":true,"imported":12}"#,
        ),
        ok_case(
            "import, a hint",
            &[
                "import",
                "sales@acme.example",
                "--imapsync",
                "from=imap.globex.example",
                "--user",
                "u",
                "--port",
                "993",
            ],
            r#"{"ok":true,"hint":"queued","address":null}"#,
        ),
        // The upgrade's watch: a step at a time to the end.
        watch(
            "upgrade, watched to success",
            &["upgrade"],
            &[
                r#"{"ok":true,"from":"0.11.8","to":"0.12.1","outage":{"typicalSecs":20}}"#,
                r#"{"ok":true,"upgrade":{"state":"running","steps":[{"step":"snapshot","ok":true}]}}"#,
                r#"{"error":{"code":"busy"}}"#,
                r#"{"ok":true,"upgrade":{"state":"succeeded","to":"0.12.1","steps":[{"step":"snapshot","ok":true},{"step":"start","ok":false,"detail":"slow"}]}}"#,
            ],
            0,
        ),
        watch(
            "upgrade --json, watched",
            &["upgrade", "--json"],
            &[
                r#"{"ok":true}"#,
                r#"{"ok":true,"upgrade":{"state":"aborted","error":"no disk"}}"#,
            ],
            1,
        ),
        watch(
            "upgrade, the rollback failed",
            &["upgrade"],
            &[
                r#"{"ok":true}"#,
                r#"{"ok":true,"upgrade":{"state":"rollback_failed","error":"boom","warning":"mail is down","manualSteps":["systemctl start stalwart","check the logs"]}}"#,
            ],
            1,
        ),
        case(
            "enable, refused",
            &["enable", "--hostname", "h"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        traceback(
            "app-password list, a row not a dict",
            &["app-password", "list", "a@x"],
            r#"{"ok":true,"appPasswords":[1]}"#,
        ),
        traceback(
            "person --all, changed a number",
            &["person", "--all", "--workspace", "w", "on"],
            r#"{"ok":true,"changed":5}"#,
        ),
        traceback(
            "password keep --all-existing, a count not a number",
            &["password", "keep", "--all-existing"],
            r#"{"ok":true,"leftFlagged":{"createdAfterCutoff":"x"}}"#,
        ),
        traceback(
            "password rotate, an endpoint not a dict",
            &["password", "rotate", "a@x"],
            r#"{"ok":true,"submission":[1]}"#,
        ),
        traceback(
            "upgrade --dry-run, a blocker not a dict",
            &["upgrade", "--dry-run"],
            r#"{"ok":true,"blockers":["x"]}"#,
        ),
        traceback(
            "upgrade --dry-run, a size not a number",
            &["upgrade", "--dry-run"],
            r#"{"ok":true,"dataBytes":"lots"}"#,
        ),
        compare(
            Compare::Traceback,
            usage_case_code(
                "import, a port not a number",
                &["import", "a@x", "--imapsync", "imap.x", "--port", "imaps"],
                1,
            ),
        ),
        case(
            "disable, no answer",
            &["disable"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "rotate-admin, the daemon down",
            &["rotate-admin"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

fn usage_case_code(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "shelp");
    const LINES: &[&[&str]] = &[
        &["enable", "--hostname", "h", "-h"],
        &["disable", "--help"],
        &["uninstall", "--purge", "--confirm-hostname", "h", "--help"],
        &["upgrade", "-h"],
        &["upgrade", "--acknowledge-failed", "--help"],
        &["rotate-admin", "-h"],
        &["password", "rotate", "a@x", "-h"],
        &["password", "keep", "--help", "--all-existing"],
        &["app-password", "add", "a@x", "-h"],
        &["app-password", "revoke", "a@x", "ap_1", "--help"],
        &["app-password", "keep", "-h", "a@x", "ap_1"],
        &["person", "a@x", "on", "-h"],
        &["person", "link", "a@x", "bob", "--help"],
        &["import", "--address", "a@x", "--maildir", "/m", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help never acts", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}

/// The password the piped cases send: it must reach the daemon in the
/// request's body, and no child's argv or environment.
const SECRET: &str = "CHILDTEST_PASSWORD_7Kq";

const IMPORT: &[&str] = &[
    "import",
    "sales@acme.example",
    "--imapsync",
    "from=imap.globex.example",
    "--user",
    "sales",
    "--pass-stdin",
];

const IMPORTED: &str = r#"{"ok":true,"imported":3,"address":"sales@acme.example"}"#;

/// The cases with a password on stdin: one line, no newline at the end,
/// two lines (both are read, as `$(cat)` reads them), and trailing
/// newlines (taken off).
fn piped_cases() -> Vec<Case> {
    vec![
        piped(
            b"CHILDTEST_PASSWORD_7Kq\n",
            ok_case("import, the password piped", IMPORT, IMPORTED),
        ),
        piped(
            b"CHILDTEST_PASSWORD_7Kq",
            ok_case("import, no newline at the end", IMPORT, IMPORTED),
        ),
        piped(
            b"CHILDTEST_PASSWORD_7Kq\nsecond line\n\n\n",
            ok_case("import, two lines", IMPORT, IMPORTED),
        ),
    ]
}

#[test]
fn a_piped_password_is_read_as_the_shell_reads_it() {
    let rig = Rig::new(VERB, "spiped");
    for c in &piped_cases() {
        rig.check(c);
    }
}

/// A folder of `python3` and `curl` that log their argv and environment
/// to `log`, then run the real program.
fn logging_programs(rig: &Rig, log: &std::path::Path) -> std::path::PathBuf {
    let dir = rig.dir.join("logging-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::env::var_os("PATH").expect("a PATH to run the tests with");
    for prog in ["python3", "curl"] {
        let real = std::env::split_paths(&path)
            .map(|d| d.join(prog))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("no {prog} on PATH"));
        let script = format!(
            "#!/bin/sh\n{{ printf '== {prog}'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; printf '\\n'; env; }} >> '{log}'\nexec '{real}' \"$@\"\n",
            log = log.display(),
            real = real.display(),
        );
        let file = dir.join(prog);
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// Each side's run of a piped case with `python3` and `curl` logging
/// what they were started with: the password reaches the daemon in the
/// request and no child's argv or environment. The shell starts its
/// Python (so the logging is seen to work); the binary starts nothing.
#[test]
fn a_piped_password_is_on_no_child_s_argv_or_environment() {
    let mut rig = Rig::new(VERB, "schildren");
    let log = rig.dir.join("children.log");
    rig.path_first = Some(logging_programs(&rig, &log));
    rig.tool_log = Some(log);
    for c in &piped_cases() {
        rig.prepare(Side::Bash, c);
        let out = rig.run(true, c);
        assert_eq!(out.code, Some(0), "{}: the shell: {out:?}", c.name);
        let sent = rig.read(Side::Bash, false, c.name);
        let children = rig.take_tool_log();
        assert!(
            children.contains("== python3 "),
            "{}: the shell's Python was not logged:\n{children}",
            c.name
        );
        assert!(
            !children.contains(SECRET),
            "{}: the password is on a child of the shell:\n{children}",
            c.name
        );
        assert_eq!(sent.len(), 1, "{}: the shell's requests", c.name);
        assert!(
            String::from_utf8_lossy(&sent[0].bytes).contains(SECRET),
            "{}: the shell did not send the password",
            c.name
        );

        let (out, sent) = rig.binary(c, false);
        let children = rig.take_tool_log();
        assert_eq!(out.code, Some(0), "{}: the binary: {out:?}", c.name);
        assert_eq!(children, "", "{}: the binary started a program", c.name);
        assert_eq!(sent.len(), 1, "{}: the binary's requests", c.name);
        assert!(
            sent[0].contains(SECRET),
            "{}: the binary did not send the password",
            c.name
        );
    }
}
