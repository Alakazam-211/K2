use super::*;
use crate::families::traceback;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// `text` as this build's brand says it: the command and product names
/// put in for `k2` and `K2`.
fn b(text: &str) -> String {
    text.replace("k2 ", &format!("{CLI_NAME} "))
        .replace("K2", PRODUCT_NAME)
}

fn report(words: &[&str]) -> Result<Report, String> {
    parse_report(words, &mut || b"from stdin".to_vec())
}

#[test]
fn a_report_s_words_and_checks() {
    let r = report(&["--category=bug", "--scope", "cli", "--command", "a x", "hi"]).unwrap();
    assert_eq!(
        (
            r.text.as_slice(),
            r.category.as_str(),
            r.scope.as_str(),
            r.command.as_str()
        ),
        (&b"hi"[..], "bug", "cli", "a x")
    );
    assert_eq!(report(&["-"]).unwrap().text, b"from stdin");
    assert!(report(&["--", "-x", "--json"]).unwrap().json);
    assert_eq!(report(&["--", "-x"]).unwrap().text, b"-x");
    for (words, line) in [
        (
            &["a", "b"][..],
            "usage: one text (quote it): k2 send-feedback \"<text>\"",
        ),
        (&["a", "--", "b"], "usage: one text (quote it)"),
        (
            &["--bogus"],
            "Unknown flag: --bogus (k2 send-feedback --help)",
        ),
        (&["--category"], "usage: --category bug|idea|praise|other"),
        (
            &["--category", "x", "a"],
            "usage: --category is bug, idea, praise or other",
        ),
        (&["--scope=", "a"], "usage: --scope cli|platform"),
        (
            &["--scope", "cli", "a"],
            "usage: --scope cli names the command you ran: --command \"<the command>\"",
        ),
        (
            &["--command", " \t", "a"],
            "usage: --command needs the command you ran",
        ),
        (
            &["--command", "a\nb", "a"],
            "usage: --command is the one line you ran, without a new line",
        ),
        (
            &["--diagnostics", "--diagnostics-preview"],
            "usage: --diagnostics or --diagnostics-preview, not both",
        ),
        (
            &[" \n"],
            "usage: k2 send-feedback \"<text>\" (or - for stdin). k2 send-feedback --help",
        ),
    ] {
        assert_eq!(report(words), Err(b(line)), "{words:?}");
    }
    assert!(report(&["--diagnostics-preview"]).is_ok());
}

#[test]
fn settings_words() {
    assert_eq!(
        parse_settings(&["--enabled", "yes", "--agents", "1", "--json"]),
        Ok(SettingsAsk {
            enabled: Some(true),
            agents: Some("on"),
            json: true,
            ..SettingsAsk::default()
        })
    );
    assert_eq!(
        parse_settings(&["--enabled", "x", "--bogus"]),
        Err("usage: --enabled takes on or off".into())
    );
    assert_eq!(
        parse_settings(&["--agents", "some"]),
        Err("usage: --agents takes off, cli or all".into())
    );
    assert!(parse_settings(&["--attach", " "])
        .unwrap_err()
        .starts_with("usage: --attach <parts"));
    assert!(
        parse_settings(&["--attach", "logs", "--agent-diagnostics", "on"])
            .unwrap_err()
            .ends_with("not both")
    );
}

#[test]
fn the_settings_page() {
    let page = render(br#"{"enabled":true,"agents":true,"agentPlatform":false,"agentsAsked":false,"agentAttach":["logs"],"attachParts":[{"id":"logs","label":"Recent logs"},{"id":"config","label":"Config \u00e9"}],"canSend":true,"message":"","service":{"message":"up"},"pending":2,"endpoint":"https://x"}"#);
    assert_eq!(
        String::from_utf8(page).unwrap(),
        b("Send feedback to K2 on this server\n  enabled:            on\n  agents report:      problems with K2 commands\n  answered:           not asked yet\n  agents may attach:  (the versions always go)\n    on   logs           Recent logs\n    off  config         Config \u{e9}\n  can send now:       yes\n  service:            up\n  waiting to send:    2\n  sends to:           https://x\n")
    );
    let old = render(br#"{"enabled":false,"agents":true,"agentDiagnostics":false,"canSend":false,"message":"off"}"#);
    assert_eq!(
        String::from_utf8(old).unwrap(),
        b("Send feedback to K2 on this server\n  enabled:            off\n  agents:             on\n  agent diagnostics:  off\n  can send now:       no\n    off\n  sends to:           \n")
    );
}

#[test]
fn the_list_and_the_result() {
    let l = list_reader(br#"{"items":[]}"#);
    assert_eq!(l.stdout, b("No feedback to K2 sent from here yet.\n"));
    let bad = list_reader(br#"{"items":[{"createdAt":"x"}]}"#);
    assert_eq!(
        bad.stderr,
        traceback(
            &[(9, "<module>")],
            "TypeError: unsupported operand type(s) for /: 'str' and 'int'"
        )
    );
    let sent =
        result(br#"{"item":{"id":"pf_1","state":"sent","category":"bug","hasDiagnostics":true}}"#);
    assert_eq!(
        sent.stdout,
        b("Sent to Alakazam Labs: pf_1 (bug).\n  The K2 details you approved are included.\n")
    );
    let queued =
        result(br#"{"item":{"id":"pf_1","state":"queued","category":"idea","errorMessage":5}}"#);
    assert_eq!(
        queued.stdout,
        "Saved on this server: pf_1 (idea). It sends when it can.\n"
    );
    assert_eq!(
        queued.stderr,
        traceback(
            &[(12, "<module>")],
            "TypeError: can only concatenate str (not \"int\") to str"
        )
    );
}

#[test]
fn surrogateescape() {
    let v = cp::env_text(b"a\xffb\"");
    assert_eq!(
        json::to_string(&v, &Options::default()).unwrap(),
        r#""a\udcffb\"""#
    );
}
