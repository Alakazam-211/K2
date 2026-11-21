use super::*;

fn call(words: &[&str]) -> Ask {
    match parse(words) {
        Parsed::Call(a) => *a,
        other => panic!("{words:?}: {other:?}"),
    }
}

#[test]
fn the_words_as_the_shell_reads_them() {
    assert_eq!(
        parse(&[]),
        Parsed::Call(Box::new(Ask {
            sub: "status".into(),
            ..Ask::default()
        }))
    );
    assert_eq!(parse(&["help"]), Parsed::Help);
    assert_eq!(
        parse(&["grant", "--bogus", "-h"]),
        Parsed::Early(format!(
            "usage: unknown flag '--bogus' ({CLI} desktop --help)"
        ))
    );
    assert_eq!(parse(&["grant", "-h", "--bogus"]), Parsed::Help);
    let a = call(&["--json"]);
    assert!(a.json && a.sub == "status");
    let a = call(&["policy", "--hide-app", "a", "--hide-app=b", "--desktop=v"]);
    assert_eq!(a.hides, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(a.desk, "v");
    assert_eq!(
        parse(&["status", "--desktop"]),
        Parsed::Early(value_usage("--desktop"))
    );
}

#[test]
fn a_timeout_is_digits_and_one_unit() {
    assert_eq!(timeout_secs("90s"), Timeout::Secs(90));
    assert_eq!(timeout_secs("2m"), Timeout::Secs(120));
    assert_eq!(timeout_secs("1h"), Timeout::Secs(3600));
    assert_eq!(timeout_secs("600"), Timeout::Secs(600));
    assert_eq!(timeout_secs("0"), Timeout::Secs(0));
    assert_eq!(timeout_secs("00s"), Timeout::Secs(0));
    // A leading 0 is decimal (`10#`), as _compute_secs reads it.
    assert_eq!(timeout_secs("08"), Timeout::Secs(8));
    assert_eq!(timeout_secs("010m"), Timeout::Secs(600));
    assert_eq!(
        timeout_secs("000999999999999h"),
        Timeout::Secs(999_999_999_999 * 3600)
    );
    for bad in ["", "s", "1x", "1hm", "x", "HOME", "1.5m", "-1", " 1", "1d"] {
        assert_eq!(timeout_secs(bad), Timeout::Bad, "{bad}");
    }
    assert_eq!(timeout_secs("9999999999999"), Timeout::Long);
    // The shell's helper, as its source spells it.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/desktop.sh"),
    )
    .expect("desktop.sh");
    assert!(src.contains("    [[ \"$1\" =~ ^[0-9]+[smh]?$ ]] || { echo \"$2\" >&2; exit 2; }\n"));
    for line in [SETUP_TIMEOUT_USAGE, ENV_TIMEOUT_USAGE] {
        assert!(src.contains(&format!("\"{line}\")")), "{line}");
    }
}

#[test]
fn the_bodies_python_writes() {
    let r = request(
        &call(&["start", "--size", "0160X0900", "--desktop", "v"]),
        "/p",
    )
    .unwrap();
    assert_eq!(
        r,
        Request::One {
            get: false,
            path: "/cli/desktop/start".into(),
            payload: r#"{"width": 160, "height": 900, "desktop": "v"}"#.into(),
            verb: "status_start",
        }
    );
    let r = request(&call(&["env", "--release"]), "/p").unwrap();
    assert_eq!(r, Request::Env(r#"{"release": true}"#.into()));
    let r = request(&call(&["allow", "on"]), "/p \u{e9}").unwrap();
    assert_eq!(
        r,
        Request::One {
            get: false,
            path: "/cli/agent-access/set".into(),
            payload: r#"{"workspace": "/p \u00e9", "toggle": "desktop", "value": true}"#.into(),
            verb: "allow",
        }
    );
    let r = request(&call(&["audit", "--since", "1d x"]), "").unwrap();
    assert_eq!(
        r,
        Request::One {
            get: true,
            path: "/cli/desktop/audit".into(),
            payload: "limit=50&since=1d%20x".into(),
            verb: "audit",
        }
    );
}

#[test]
fn shlex_quote_is_python_s() {
    assert_eq!(shlex_quote(""), "''");
    assert_eq!(shlex_quote(":1.0"), ":1.0");
    assert_eq!(shlex_quote("/a b"), "'/a b'");
    assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
}

#[test]
fn the_python_lines_are_where_the_shell_has_them() {
    assert_eq!(line::O_LOAD, 3);
    assert_eq!(line::V_LOAD, 3);
    assert_eq!(line::S_LOAD, 3);
    const { assert!(line::O_LIMITS_1 == line::O_LIMITS + 1 && line::O_LIMITS_2 == line::O_LIMITS + 2) };
}
