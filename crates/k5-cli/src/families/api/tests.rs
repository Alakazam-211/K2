use super::*;
use crate::families::skin_token::tests::{k2, source};

/// `cmd_api`, as the shell has it.
fn cmd_api() -> String {
    let text = source("talk-and-workspace.sh");
    let start = text.find("cmd_api() {").expect("cmd_api");
    let end = start + text[start..].find("\n}\n").expect("its end");
    text[start..end].to_string()
}

#[test]
fn the_shell_s_lines() {
    let f = cmd_api();
    for shell in [
        r#"    local sub="${1:-status}""#,
        r#"            cli_post_json "/cli/settings/update" "{\"apiEnabled\":${val}}" | python3 -c ""#,
        r#"            _k2_cfg_add url "${BASE_URL}/boot-status""#,
        r#"                _k2_cfg_add header "Authorization: Bearer ${TOKEN}""#,
        r#"            boot="$(_k2_curl "$_K2_CFG" -s --connect-timeout 5 --max-time 10 2>/dev/null || true)""#,
        r#"            settings="$(cli_request "/cli/settings/get" || true)""#,
        "            exit 2",
    ] {
        assert!(
            f.lines().any(|l| l == shell),
            "cmd_api no longer has {shell:?}"
        );
    }
    // -h or --help anywhere: the usage on stdout, exit 0, nothing sent.
    assert!(f.contains("    if _wants_help \"$@\"; then\n        _api_usage\n        return 0\n"));
    // An unknown verb: the usage on stderr, exit 2.
    assert!(f.contains("            _api_usage >&2\n            exit 2\n"));
    let text = source("talk-and-workspace.sh");
    let usage = text
        .split_once("_api_usage() {\n")
        .expect("_api_usage")
        .1
        .split_once("\n}\n")
        .expect("its end")
        .0;
    let want: Vec<String> = USAGE
        .iter()
        .map(|l| format!("    echo \"{}\"", k2(l)))
        .collect();
    assert_eq!(usage.lines().collect::<Vec<_>>(), want);
    assert!(f.contains(&format!("    print('{}')\n", k2(NOTE_OFF))));
    assert!(f.contains(&format!("    print('{FORCED}')\n")));
    let router = source("router.sh");
    assert!(router.contains("    api)\n"));
    assert!(router.contains("        shift; cmd_api \"$@\"\n"));
    // No tool: an open verb.
    let policy = source("cli-tools-policy.sh");
    assert!(!policy.contains("        api) echo"));
}

#[test]
fn the_python() {
    let f = cmd_api();
    let switch = super::super::shell_python("talk-and-workspace.sh", "cmd_api() {");
    let at_line = |lines: &[String], n: u32| lines[n as usize - 1].trim().to_string();
    assert_eq!(at_line(&switch, line::switch::IN), "if 'error' in d:");
    assert_eq!(
        at_line(&switch, line::switch::INDEX),
        "print('Error: %s' % d['error'])"
    );
    assert_eq!(
        at_line(&switch, line::switch::GET),
        "on = bool(d.get('apiEnabled'))"
    );
    let doc = f.split_once("python3 <<'PY'\n").expect("the heredoc").1;
    let doc: Vec<String> = doc.lines().map(str::to_string).collect();
    assert_eq!(
        at_line(&doc, line::status::BOOT),
        "eff = boot.get('api', {}).get('enabled')"
    );
    assert_eq!(
        at_line(&doc, line::status::SETTINGS),
        "setting = settings.get('apiEnabled')"
    );
}

#[test]
fn the_words() {
    assert_eq!(parse(&[]), Ask::Status);
    assert_eq!(parse(&[""]), Ask::Status);
    assert_eq!(parse(&["on", "x"]), Ask::Switch(true));
    assert_eq!(parse(&["off"]), Ask::Switch(false));
    assert_eq!(parse(&["ON"]), Ask::Usage);
}

#[test]
fn the_switch_reader() {
    assert_eq!(
        switch_reader(r#"{"apiEnabled":true}"#).stdout,
        "Public /v1 API setting: ON (live now \u{2014} no restart needed)\n"
    );
    assert_eq!(
        switch_reader("{}").stdout,
        format!("Public /v1 API setting: OFF (live now \u{2014} no restart needed)\n{NOTE_OFF}\n")
    );
    let e = switch_reader(r#"{"error":{"code":"owner_only"}}"#);
    assert_eq!(
        (e.stdout.as_str(), e.exit),
        ("Error: {'code': 'owner_only'}\n", Exit::Error)
    );
    let e = switch_reader("");
    assert_eq!(
        (e.stdout.as_str(), e.exit),
        (
            "Error: daemon returned an unparseable response: ''\n",
            Exit::Error
        )
    );
    let e = switch_reader("\"an error\"");
    assert!(e.stderr.ends_with(
        "line 10, in <module>\nTypeError: string indices must be integers, not 'str'\n"
    ));
    let e = switch_reader("5");
    assert!(e
        .stderr
        .ends_with("line 9, in <module>\nTypeError: argument of type 'int' is not iterable\n"));
    let e = switch_reader("[]");
    assert!(e
        .stderr
        .ends_with("line 12, in <module>\nAttributeError: 'list' object has no attribute 'get'\n"));
}

#[test]
fn the_status_reader() {
    assert_eq!(
        status_reader("", "").stdout,
        "Public /v1 API surface: ?\n  Settings switch:      ?\n"
    );
    assert_eq!(
        status_reader(r#"{"api":{"enabled":true}}"#, r#"{"apiEnabled":false}"#).stdout,
        format!("Public /v1 API surface: on\n  Settings switch:      off\n{FORCED}\n")
    );
    assert_eq!(
        status_reader("not json", r#"{"apiEnabled":0}"#).stdout,
        "Public /v1 API surface: ?\n  Settings switch:      off\n"
    );
    let e = status_reader(r#"{"api":null}"#, "{}");
    assert_eq!(
        e.stderr,
        "Traceback (most recent call last):\n  File \"<stdin>\", line 9, in <module>\nAttributeError: 'NoneType' object has no attribute 'get'\n"
    );
    assert_eq!(e.exit, Exit::Error);
}
