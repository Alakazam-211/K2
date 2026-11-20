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
    assert_eq!(until_secs("2026-11-20T01:00:00+01:00", 0), Ok(1_795_132_800));
    assert!(until_secs("2026-11-20T00:00:00", 0).unwrap_err().contains("time zone"));
    assert!(until_secs("soon", 0).unwrap_err().starts_with("usage: --until 3d"));
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
        Some(Parsed::Usage("usage: bad duration '1x' (use 2h, 90m, 1h30m, 300s)".into()))
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
        compute_json(&[("name", "m"), ("count_n", "03"), ("code_s", "c"), ("for", "")]),
        r#"{"code": "c", "name": "m", "count": 3}"#
    );
}
