use super::*;
use crate::families::skin_token::tests::{k2, source};

/// `cmd_api`, as the shell has it.
fn cmd_api() -> String {
    let text = source("talk-and-workspace.sh");
    let start = text.find("cmd_api() {").expect("cmd_api");
    let end = start + text[start..].find("\n}\n").expect("its end");
    text[start..end].to_string()
}

/// The `python3 -c '…'` scripts in `cmd_api`, in order, as `-c` numbers
/// their lines (line 1 is the empty one after `-c '`).
fn single_quoted_python() -> Vec<Vec<String>> {
    let f = cmd_api();
    f.split("python3 -c '")
        .skip(1)
        .map(|py| {
            let end = py.find("\n')\"").expect("the end of the Python");
            py[..end].split('\n').map(str::to_string).collect()
        })
        .collect()
}

#[test]
fn the_shell_s_lines() {
    let f = cmd_api();
    let echo = |indent: usize, text: &str, tail: &str| {
        format!("{}echo \"{}\"{tail}", " ".repeat(indent), k2(text))
    };
    for shell in [
        r#"    local sub="${1:-status}""#.to_string(),
        // Apart from the assignment, so `set -e` ends the command on a
        // call that failed.
        "            local resp saved".to_string(),
        r#"            resp="$(cli_post_json "/cli/settings/update" "{\"apiEnabled\":${val}}")""#
            .to_string(),
        r#"            saved="$(K2_API_RESP="$resp" python3 -c '"#.to_string(),
        r#"')" || { printf '%s\n' "$saved"; exit 1; }"#.to_string(),
        r#"            if [[ "$saved" == "off" ]]; then"#.to_string(),
        r#"                _k2_cfg_add url "${BASE_URL}/boot-status""#.to_string(),
        r#"                    _k2_cfg_add header "Authorization: Bearer ${TOKEN}""#.to_string(),
        r#"                boot="$(_k2_curl "$_K2_CFG" -s --connect-timeout 5 --max-time 10 2>/dev/null || true)""#
            .to_string(),
        r#"                eff="$(K2_API_BOOT="$boot" python3 -c '"#.to_string(),
        r#"                if [[ "$eff" == "on" ]]; then"#.to_string(),
        echo(20, STILL_ON, " >&2"),
        "                    exit 1".to_string(),
        echo(16, &setting_line(false), ""),
        r#"                if [[ "$eff" != "off" ]]; then"#.to_string(),
        echo(20, NOTE_OFF, ""),
        echo(16, &setting_line(true), ""),
        r#"            _k2_cfg_add url "${BASE_URL}/boot-status""#.to_string(),
        r#"                _k2_cfg_add header "Authorization: Bearer ${TOKEN}""#.to_string(),
        r#"            boot="$(_k2_curl "$_K2_CFG" -s --connect-timeout 5 --max-time 10 2>/dev/null || true)""#
            .to_string(),
        r#"            settings="$(cli_request "/cli/settings/get" || true)""#.to_string(),
        "            exit 2".to_string(),
    ] {
        assert!(
            f.lines().any(|l| l == shell),
            "cmd_api no longer has {shell:?}"
        );
    }
    // The still-ON line comes before the OFF line, which it replaces.
    let still = f.find(echo(20, STILL_ON, " >&2").as_str()).expect("the still-ON line");
    let off = f.find(echo(16, &setting_line(false), "").as_str()).expect("the OFF line");
    assert!(still < off);
    // -h or --help anywhere: the usage on stdout, exit 0, nothing sent.
    assert!(f.contains("    if _wants_help \"$@\"; then\n        _api_usage\n        return 0\n"));
    // An unknown verb: the usage on stderr, exit 2.
    assert!(f.contains("            _api_usage >&2\n            exit 2\n"));
    let text = source("talk-and-workspace.sh");
    let usage = text
        .split_once("\n_api_usage() {\n")
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
    let at_line = |lines: &[String], n: u32| lines[n as usize - 1].trim().to_string();
    let scripts = single_quoted_python();
    assert_eq!(scripts.len(), 2, "on/off's two python3 -c scripts");
    let saved = &scripts[0];
    assert_eq!(
        at_line(saved, 3),
        r#"raw = os.environ.get("K2_API_RESP", "")"#
    );
    assert_eq!(
        at_line(saved, 7),
        r#"print("Error: daemon returned an unparseable response: %r" % raw[:200])"#
    );
    assert_eq!(
        UNPARSEABLE,
        "Error: daemon returned an unparseable response: "
    );
    assert_eq!(at_line(saved, line::switch::IN), r#"if "error" in d:"#);
    assert_eq!(
        at_line(saved, line::switch::INDEX),
        r#"print("Error: %s" % d["error"])"#
    );
    assert_eq!(
        at_line(saved, line::switch::GET),
        r#"print("on" if d.get("apiEnabled") else "off")"#
    );
    assert_eq!(saved.len(), 12, "{saved:#?}");
    let boot: Vec<String> = scripts[1].iter().map(|l| l.trim().to_string()).collect();
    assert_eq!(
        boot,
        [
            "",
            "import json, os",
            "try:",
            r#"d = json.loads(os.environ.get("K2_API_BOOT", "") or "{}")"#,
            "except Exception:",
            "d = {}",
            r#"api = d.get("api") if isinstance(d, dict) else None"#,
            r#"e = api.get("enabled") if isinstance(api, dict) else None"#,
            r#"print("on" if e is True else ("off" if e is False else "?"))"#,
        ]
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

fn failed(stdout: &str, exit: Exit) -> Saved {
    Saved::Failed(Shown {
        stdout: stdout.to_string(),
        stderr: String::new(),
        exit,
    })
}

#[test]
fn the_saved_reader() {
    assert_eq!(saved_reader(r#"{"apiEnabled":true}"#), Saved::On);
    assert_eq!(saved_reader(r#"{"apiEnabled":1,"x":2}"#), Saved::On);
    assert_eq!(saved_reader(r#"{"apiEnabled":false}"#), Saved::Off);
    assert_eq!(saved_reader("{}"), Saved::Off);
    assert_eq!(
        saved_reader(r#"{"error":{"code":"owner_only"}}"#),
        failed("Error: {'code': 'owner_only'}\n", Exit::Error)
    );
    // `saved=$(…)` keeps no trailing newline and no NUL; printf adds one.
    assert_eq!(
        saved_reader(r#"{"error":"a\u0000b\n\n"}"#),
        failed("Error: ab\n", Exit::Error)
    );
    assert_eq!(
        saved_reader(""),
        failed(
            "Error: daemon returned an unparseable response: ''\n",
            Exit::Error
        )
    );
    // A raise: the traceback on stderr, an empty line on stdout, exit 1.
    let raised = |raw: &str| match saved_reader(raw) {
        Saved::Failed(s) => {
            assert_eq!((s.stdout.as_str(), s.exit), ("\n", Exit::Error), "{raw}");
            s.stderr
        }
        other => panic!("{raw}: {other:?}"),
    };
    assert!(raised("\"an error\"").ends_with(
        "line 10, in <module>\nTypeError: string indices must be integers, not 'str'\n"
    ));
    assert!(raised("5")
        .ends_with("line 9, in <module>\nTypeError: argument of type 'int' is not iterable\n"));
    assert!(raised("[]")
        .ends_with("line 12, in <module>\nAttributeError: 'list' object has no attribute 'get'\n"));
    assert!(raised("\"plain\"")
        .ends_with("line 12, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"));
}

#[test]
fn the_off_reader() {
    let off = format!("{}\n", setting_line(false));
    // An env flag forces the surface on: stderr only, exit 1, no OFF line.
    let s = off_reader(r#"{"version":"0.46.17","api":{"enabled":true}}"#);
    assert_eq!(s, Shown::err(format!("{STILL_ON}\n"), Exit::Error));
    assert_eq!(
        off_reader(r#"{"api":{"enabled":false}}"#),
        Shown::out(off.clone())
    );
    // Anything but true or false is unknown: the OFF line and the note.
    let unknown = Shown::out(format!("{off}{NOTE_OFF}\n"));
    for boot in [
        "",
        "not json",
        r#"{"ok":true}"#,
        r#"{"api":null}"#,
        r#"{"api":{"enabled":1}}"#,
        r#"{"api":{"enabled":"true"}}"#,
        r#"{"api":[true]}"#,
        "[1]",
        "5",
    ] {
        assert_eq!(off_reader(boot), unknown, "{boot}");
    }
    // A key given twice: Python's dict keeps the last.
    assert_eq!(
        off_reader(r#"{"api":{"enabled":true},"api":{"enabled":false}}"#),
        Shown::out(off)
    );
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
