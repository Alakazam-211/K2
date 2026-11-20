//! `k5-cli mail` owner verbs against the shell CLI's `cli/k2 mail`, on the
//! parity rig ([`common::parity`]): the same requests (urllib's, from
//! `_mail_py`), stdout, stderr and exit code.
//!
//! - link (and external): add over IMAP with `--pass-ref`, add over OAuth
//!   (the device and the browser flow, connected after a pending answer,
//!   denied with the admin consent link, an error from the status), upgrade,
//!   calendars (show, on, off), list (the BLOCKED note, `--json`), remove.
//! - access grant, revoke, primary, level and manage; oauth-config show,
//!   set and clear; sending (show and set, each mode's note).
//! - Each `cmd_mail_*`'s usage lines, `--pass-stdin` and `--secret-stdin`
//!   with nothing on stdin (exit 2, nothing sent), and the help pages of
//!   `link` and `access` alone.
//! - An agent refused (owner only, exit 3) and the permission card.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` anywhere prints its help and sends nothing.
//!
//! The rig gives the CLI no stdin, so a secret piped on stdin is not
//! driven here.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "mail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

const LINKED: &str = r#"{"ok":true,"inboxes":[
 {"address":"me@globex.example","source":"linked","status":"ok","host":"imap.globex.example","tls":"implicit-tls",
  "primary":{"workspace":"alpha"},"grants":[{"workspace":"beta","level":"send","sendState":"blocked_off","agentSend":"approval"},{"projectId":"p-2"}]},
 {"address":"bot@acme.example","source":"hosted"}]}"#;

const OAUTH_CONFIG: &str = r#"{"ok":true,"providers":{"gmail":{"source":"custom","clientId":"g-1","secretSet":true},
 "microsoft":{"clientId":"m-1","configured":false,"tenant":"organizations"}}}"#;

const CONFIG: &str = r#"{"ok":true,"workspace":{"project":"/w/alpha","agentSend":{"effective":"approval","override":"approval","default":"off"}}}"#;

#[test]
fn mail_owner_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        case(
            "link alone: the help",
            &["link"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "external alone: the help",
            &["external"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "access alone: the help",
            &["access"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        usage_case("link, an unknown sub-command", &["link", "move"]),
        usage_case("link grant moved", &["link", "grant", "a@x", "ws"]),
        usage_case(
            "link add, a provider not known",
            &["link", "add", "a@x", "--provider", "yahoo"],
        ),
        usage_case(
            "link add over OAuth, no workspace",
            &["link", "add", "a@x", "--provider", "gmail"],
        ),
        usage_case(
            "link add over OAuth with a host",
            &[
                "link",
                "add",
                "a@x",
                "--provider",
                "gmail",
                "--workspace",
                "w",
                "--host",
                "h",
            ],
        ),
        usage_case(
            "link add, features without a provider",
            &["link", "add", "a@x", "--features", "mail"],
        ),
        usage_case(
            "link add, no host",
            &["link", "add", "a@x", "--workspace", "w"],
        ),
        usage_case(
            "link add, both secrets",
            &[
                "link",
                "add",
                "a@x",
                "--host",
                "h",
                "--workspace",
                "w",
                "--pass-stdin",
                "--pass-ref",
                "env:X",
            ],
        ),
        usage_case(
            "link add, nothing on stdin",
            &[
                "link",
                "add",
                "a@x",
                "--host",
                "h",
                "--workspace",
                "w",
                "--pass-stdin",
            ],
        ),
        usage_case(
            "link add, no secret",
            &["link", "add", "a@x", "--host", "h", "--workspace", "w"],
        ),
        usage_case(
            "link add, a port not a number",
            &["link", "add", "a@x", "--port", "99x"],
        ),
        usage_case(
            "link add, an smtp port not a number",
            &["link", "add", "a@x", "--smtp-port", ""],
        ),
        usage_case("link upgrade, no features", &["link", "upgrade", "a@x"]),
        usage_case(
            "link upgrade with a workspace",
            &[
                "link",
                "upgrade",
                "a@x",
                "--features",
                "mail",
                "--workspace",
                "w",
            ],
        ),
        usage_case("link calendars, a word", &["link", "calendars", "maybe"]),
        usage_case(
            "link calendars, a provider",
            &["link", "calendars", "on", "--provider", "gmail"],
        ),
        usage_case("link remove, no address", &["link", "remove"]),
        usage_case("link, an unknown flag", &["link", "list", "--all"]),
        usage_case("link, two addresses", &["link", "remove", "a@x", "b@x"]),
        usage_case(
            "link, a value flag at the end",
            &["link", "add", "a@x", "--host"],
        ),
        usage_case(
            "access, an unknown sub-command",
            &["access", "give", "a@x", "w"],
        ),
        usage_case("access grant, no workspace", &["access", "grant", "a@x"]),
        usage_case(
            "access grant, a bad level",
            &["access", "grant", "a@x", "w", "--level", "admin"],
        ),
        usage_case("access level, no level", &["access", "level", "a@x", "w"]),
        usage_case(
            "access revoke, the calendar switch",
            &[
                "access",
                "revoke",
                "a@x",
                "w",
                "--calendar-omit-description",
                "on",
            ],
        ),
        usage_case(
            "access grant, the calendar switch bad",
            &[
                "access",
                "grant",
                "a@x",
                "w",
                "--calendar-omit-description",
                "yes",
            ],
        ),
        usage_case(
            "access grant, a manage flag",
            &["access", "grant", "a@x", "w", "--on"],
        ),
        usage_case(
            "access, three words",
            &["access", "revoke", "a@x", "w", "x"],
        ),
        usage_case(
            "access, an unknown flag",
            &["access", "revoke", "a@x", "w", "--now"],
        ),
        usage_case(
            "oauth-config, an unknown sub-command",
            &["oauth-config", "reset"],
        ),
        usage_case(
            "oauth-config, a provider not known",
            &["oauth-config", "show", "--provider", "yahoo"],
        ),
        usage_case(
            "oauth-config set, no provider",
            &["oauth-config", "set", "--client-id", "x"],
        ),
        usage_case(
            "oauth-config set, no client id",
            &["oauth-config", "set", "--provider", "gmail"],
        ),
        usage_case(
            "oauth-config set, a Microsoft secret",
            &[
                "oauth-config",
                "set",
                "--provider",
                "microsoft",
                "--client-id",
                "x",
                "--secret-stdin",
            ],
        ),
        usage_case(
            "oauth-config set, nothing on stdin",
            &[
                "oauth-config",
                "set",
                "--provider",
                "gmail",
                "--client-id",
                "x",
                "--secret-stdin",
            ],
        ),
        usage_case(
            "oauth-config clear, no provider",
            &["oauth-config", "clear"],
        ),
        usage_case("oauth-config, a word", &["oauth-config", "show", "x"]),
        usage_case("sending, no workspace", &["sending"]),
        usage_case("sending, a bad mode", &["sending", "w", "maybe"]),
        usage_case("sending, three words", &["sending", "w", "on", "x"]),
        usage_case("sending, an unknown flag", &["sending", "w", "--all"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_owner_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "answer");
    let v = vec![
        // covers: mail link
        ok_case(
            "link add over IMAP",
            &[
                "link",
                "add",
                "me@globex.example",
                "--host",
                "imap.globex.example",
                "--workspace",
                "alpha",
                "--port",
                "0993",
                "--tls",
                "implicit-tls",
                "--user",
                "me",
                "--name",
                "Me",
                "--drafts-folder",
                "Drafts",
                "--smtp-host",
                "smtp.globex.example",
                "--smtp-port",
                "465",
                "--smtp-tls",
                "implicit-tls",
                "--pass-ref",
                "env:IMAP_PW",
            ],
            r#"{"ok":true,"address":"me@globex.example","draftsFolder":"Drafts"}"#,
        ),
        ok_case(
            "external add, no drafts folder",
            &[
                "external",
                "add",
                "me@globex.example",
                "--host",
                "h",
                "--workspace",
                "alpha",
                "--pass-ref",
                "/abs/pw",
            ],
            r#"{"ok":true,"address":"me@globex.example"}"#,
        ),
        ok_case("link list", &["link", "list"], LINKED),
        // covers: mail link --json
        ok_case("link --json is the list", &["link", "--json"], LINKED),
        ok_case(
            "link list, none",
            &["link", "list"],
            r#"{"ok":true,"inboxes":[]}"#,
        ),
        ok_case(
            "link remove",
            &["link", "remove", "me@globex.example"],
            r#"{"ok":true,"address":"me@globex.example","revokeHint":"also revoke the app password"}"#,
        ),
        ok_case(
            "link calendars",
            &["link", "calendars"],
            r#"{"ok":true,"enabled":true}"#,
        ),
        ok_case(
            "link calendars off",
            &["link", "calendars", "off"],
            r#"{"ok":true,"linkedCalendars":"off","changed":true}"#,
        ),
        ok_case(
            "link calendars on --json",
            &["link", "calendars", "on", "--json"],
            r#"{"ok":true,"enabled":true}"#,
        ),
        case(
            "link add over OAuth, the device flow, connected",
            &[
                "link",
                "add",
                "me@globex.example",
                "--provider",
                "microsoft",
                "--workspace",
                "alpha",
                "--features",
                "mail,calendar-read",
            ],
            &[],
            vec![
                ok(
                    r#"{"ok":true,"flow":"device","linkId":"l1","verificationUrl":"https://login.example/device","userCode":"ABCD","features":"mail,calendar-read"}"#,
                ),
                ok(r#"{"ok":true,"state":"pending"}"#),
                ok(r#"{"ok":true,"state":"connected","address":"me@globex.example"}"#),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "link upgrade, the browser flow, connected, --json",
            &[
                "link",
                "upgrade",
                "me@globex.example",
                "--features",
                "calendar",
                "--json",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"flow":"browser","linkId":7}"#),
                ok(r#"{"ok":true,"state":"connected","hint":"done"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "link upgrade, connected",
            &[
                "link",
                "upgrade",
                "me@globex.example",
                "--features",
                "calendar",
                "--provider",
                "gmail",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"flow":"browser","linkId":"l2"}"#),
                ok(r#"{"ok":true,"state":"connected"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "link add over OAuth, denied with the consent link",
            &[
                "link",
                "add",
                "me@globex.example",
                "--provider",
                "microsoft",
                "--workspace",
                "alpha",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"flow":"device","linkId":"l3"}"#),
                ok(
                    r#"{"ok":true,"state":"denied","hint":"An admin must approve.","adminConsentUrl":"https://login.example/consent"}"#,
                ),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "link add over OAuth, expired",
            &[
                "link",
                "add",
                "me@globex.example",
                "--provider",
                "gmail",
                "--workspace",
                "alpha",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"flow":"browser","linkId":"l4"}"#),
                ok(r#"{"ok":true,"state":"expired","code":"link_expired"}"#),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "link add over OAuth, the status refused",
            &[
                "link",
                "add",
                "me@globex.example",
                "--provider",
                "gmail",
                "--workspace",
                "alpha",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"flow":"browser","linkId":"l5"}"#),
                answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"owner only"}}"#,
                ),
            ],
            owner_and_port,
            3,
            2,
        ),
        // covers: mail access
        ok_case(
            "access grant",
            &["access", "grant", "me@globex.example", "beta"],
            r#"{"ok":true,"address":"me@globex.example"}"#,
        ),
        // covers: mail access --json
        ok_case(
            "access grant --json, the calendar switch",
            &[
                "access",
                "grant",
                "me@globex.example",
                "beta",
                "--level",
                "draft",
                "--calendar-omit-description",
                "on",
                "--json",
            ],
            r#"{"ok":true,"hint":"granted"}"#,
        ),
        ok_case(
            "access revoke",
            &["access", "revoke", "me@globex.example", "beta"],
            r#"{"ok":true,"address":"me@globex.example"}"#,
        ),
        ok_case(
            "access primary",
            &["access", "primary", "me@globex.example", "beta"],
            r#"{"ok":true,"address":"me@globex.example"}"#,
        ),
        ok_case(
            "access level, the primary",
            &[
                "access",
                "level",
                "me@globex.example",
                "alpha",
                "--level",
                "send",
                "--calendar-omit-description",
                "off",
            ],
            r#"{"ok":true,"address":"me@globex.example","isPrimary":true}"#,
        ),
        ok_case(
            "access manage",
            &["access", "manage", "me@globex.example", "beta", "--delete"],
            r#"{"ok":true,"address":"me@globex.example","canManage":true,"canDelete":true}"#,
        ),
        ok_case(
            "access manage --off",
            &[
                "access",
                "manage",
                "me@globex.example",
                "beta",
                "--off",
                "--delete",
            ],
            r#"{"ok":true,"address":"me@globex.example"}"#,
        ),
        // covers: mail oauth-config
        ok_case("oauth-config", &["oauth-config"], OAUTH_CONFIG),
        ok_case(
            "oauth-config show, one provider",
            &["oauth-config", "show", "--provider", "microsoft"],
            OAUTH_CONFIG,
        ),
        // covers: mail oauth-config --json
        ok_case(
            "oauth-config --json",
            &["oauth-config", "--json"],
            OAUTH_CONFIG,
        ),
        ok_case(
            "oauth-config set",
            &[
                "oauth-config",
                "set",
                "--provider",
                "microsoft",
                "--tenant",
                "contoso.example",
            ],
            r#"{"ok":true,"provider":"microsoft","clientId":"m-1","tenant":"contoso.example"}"#,
        ),
        ok_case(
            "oauth-config clear",
            &["oauth-config", "clear", "--provider", "gmail"],
            r#"{"ok":true,"providers":{"gmail":{"source":"builtin","clientId":"g-0"}}}"#,
        ),
        ok_case("sending", &["sending", "alpha"], CONFIG),
        ok_case(
            "sending, the default off",
            &["sending", "alpha"],
            r#"{"ok":true,"workspace":{"agentSend":{"effective":"off"}}}"#,
        ),
        ok_case("sending --json", &["sending", "alpha", "--json"], CONFIG),
        case(
            "sending on",
            &["sending", "alpha", "on"],
            &[],
            vec![
                ok(r#"{"ok":true}"#),
                ok(r#"{"ok":true,"workspace":{"agentSend":{"effective":"on","override":"on"}}}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an agent refused",
            &["sending", "alpha", "on"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "owner only",
            &["access", "revoke", "me@globex.example", "beta"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner can do that."}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "oauth-config, providers that are a list",
                &["oauth-config"],
                &[],
                vec![ok(r#"{"ok":true,"providers":[1]}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent (the front hands a help flag to the shell).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    const LINES: &[&[&str]] = &[
        &[
            "link",
            "add",
            "a@x",
            "--host",
            "h",
            "--workspace",
            "w",
            "--pass-ref",
            "env:X",
            "-h",
        ],
        &[
            "link",
            "--help",
            "add",
            "a@x",
            "--provider",
            "gmail",
            "--workspace",
            "w",
        ],
        &["link", "upgrade", "a@x", "--features", "mail", "--help"],
        &["link", "calendars", "on", "-h"],
        &["link", "remove", "a@x", "--help"],
        &["external", "add", "a@x", "-h"],
        &["access", "grant", "a@x", "w", "-h"],
        &["access", "revoke", "--help", "a@x", "w"],
        &["access", "manage", "a@x", "w", "--on", "-h"],
        &[
            "oauth-config",
            "set",
            "--provider",
            "gmail",
            "--client-id",
            "x",
            "--help",
        ],
        &["oauth-config", "clear", "-h", "--provider", "gmail"],
        &["sending", "w", "on", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}
