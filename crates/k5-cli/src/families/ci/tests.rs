use super::*;

#[test]
fn compute_secs_is_the_shell_s() {
    assert_eq!(compute_secs("0300"), Ok(Some("300".into())));
    assert_eq!(compute_secs("1h30m"), Ok(Some("5400".into())));
    assert_eq!(compute_secs("2h5s"), Ok(Some("7205".into())));
    assert!(compute_secs("h").is_err());
    assert!(compute_secs("1x").is_err());
    assert!(compute_secs("").is_err());
    assert_eq!(compute_secs("99999999999999999999"), Ok(None));
}

#[test]
fn until_reads_the_shell_s_forms() {
    assert_eq!(until_secs("3d", 100), Ok(100 + 3 * 86_400));
    assert_eq!(until_secs(" 1700000000 ", 0), Ok(1_700_000_000));
    assert_eq!(until_secs("2026-11-20T00:00:00Z", 0), Ok(1_795_132_800));
    assert_eq!(
        until_secs("2026-11-20T01:00:00+01:00", 0),
        Ok(1_795_132_800)
    );
    assert!(until_secs("2026-11-20T00:00:00", 0)
        .unwrap_err()
        .contains("time zone"));
    assert!(until_secs("soon", 0)
        .unwrap_err()
        .starts_with("usage: --until 3d"));
}

#[test]
fn the_words_as_cmd_ci_reads_them() {
    assert_eq!(parse(&[]), Some(Parsed::Help));
    assert_eq!(
        parse(&["--json"]).and_then(|p| match p {
            Parsed::Call(a) => Some(a.sub),
            _ => None,
        }),
        Some("--json".into())
    );
    assert_eq!(
        parse(&["status", "--mem", "x"]),
        Some(Parsed::Usage("usage: --mem is a number".into()))
    );
    assert_eq!(
        parse(&["monitor", "add", "m", "--count", "x"]),
        Some(Parsed::Usage("usage: --mcount is a number".into()))
    );
    assert_eq!(
        parse(&["status", "--timeout", "1x"]),
        Some(Parsed::Usage(
            "usage: bad duration '1x' (use 2h, 90m, 1h30m, 300s)".into()
        ))
    );
    let Some(Parsed::Call(a)) = parse(&["init", "--mem", "16G", "--", "make", "-h"]) else {
        panic!()
    };
    assert_eq!(a.mem, "16");
    assert_eq!(a.argv, vec!["make".to_string(), "-h".to_string()]);
}

#[test]
fn compute_json_puts_secrets_first() {
    assert_eq!(
        compute_json(&[
            ("name", "m"),
            ("count_n", "03"),
            ("code_s", "c"),
            ("for", "")
        ]),
        r#"{"code": "c", "name": "m", "count": 3}"#
    );
}

#[test]
fn the_gate_skips_the_prelude_s_help_forms() {
    use super::gate_skipped;
    for words in [
        &[][..],
        &["help"],
        &["-h"],
        &["--help"],
        &["status", "--json", "-h"],
        &["signer", "remove", "x", "--help"],
    ] {
        assert!(gate_skipped(words), "{words:?}");
    }
    for words in [
        &["status"][..],
        &["init", "--", "make", "-h"],
        &["status", "help"],
    ] {
        assert!(!gate_skipped(words), "{words:?}");
    }
}

#[test]
fn the_files_helpers_read_paths_as_python_and_lines_as_read() {
    use super::files::{join, tab_fields};
    assert_eq!(join("out", "provenance"), "out/provenance");
    assert_eq!(join("out/", "x"), "out/x");
    assert_eq!(join("out", "/abs"), "/abs");
    assert_eq!(join("", "x"), "x");
    assert_eq!(
        tab_fields("a\tb c\t\n\t\tlab\t\tjob\tmore\nlast"),
        vec![
            ("a".to_string(), "b c".to_string()),
            ("lab".to_string(), "job\tmore".to_string()),
        ]
    );
    assert_eq!(
        tab_fields("-\tdocs\n"),
        vec![("-".to_string(), "docs".to_string())]
    );
}

#[test]
fn a_reader_that_raises_is_one_plain_line() {
    use super::py::plain;
    let bad = format!(
        "bad_response: {} ci can't read the daemon's answer (a daemon on another {} version?)\n",
        k2_client::brand::CLI_NAME,
        k2_client::brand::PRODUCT_NAME
    );
    assert_eq!(plain("KeyError: 'run'"), bad);
    assert_eq!(plain("json.decoder.JSONDecodeError: Expecting value"), bad);
    assert_eq!(plain("TypeError: 'int' object is not iterable"), bad);
    assert_eq!(
        plain("PermissionError: [Errno 13] Permission denied: 'x'"),
        "io_error: [Errno 13] Permission denied: 'x'\n"
    );
    assert_eq!(plain("OSError"), "io_error: \n");
}
