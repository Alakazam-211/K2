use super::print::ttl_ms;
use super::py::{exit_of, int_text};
use super::*;

#[test]
fn urlencode_is_quote_plus() {
    let q = urlencode(&[
        ("path".into(), "/a b/é?.md".into()),
        ("argv".into(), r#"["x"]"#.into()),
    ]);
    assert_eq!(q, "path=%2Fa+b%2F%C3%A9%3F.md&argv=%5B%22x%22%5D");
}

#[test]
fn ints_read_as_python_reads_them() {
    assert_eq!(int_text(" 0042 ").as_deref(), Some("42"));
    assert_eq!(int_text("-0").as_deref(), Some("0"));
    assert_eq!(int_text("1_000").as_deref(), Some("1000"));
    assert_eq!(int_text("1__0"), None);
    assert_eq!(int_text("x"), None);
    assert_eq!(exit_of(&Value::Int("-1".into())).code(), 255);
    assert_eq!(exit_of(&Value::Int("260".into())).code(), 4);
    assert_eq!(exit_of(&Value::Bool(true)).code(), 1);
}

#[test]
fn claim_ttls_are_milliseconds() {
    assert_eq!(ttl_ms("30s"), Some(30_000));
    assert_eq!(ttl_ms("5m"), Some(300_000));
    assert_eq!(ttl_ms("250"), Some(250));
    assert_eq!(ttl_ms("250ms"), Some(250));
    assert_eq!(ttl_ms("0"), None);
    assert_eq!(ttl_ms("1h"), None);
    assert_eq!(ttl_ms(""), None);
}

#[test]
fn another_server_and_boards_go_to_the_shell() {
    assert!(handed_back(&["cat", "--host", "h", "a.md"]));
    assert!(handed_back(&["cat", "--host=h", "a.md"]));
    assert!(handed_back(&["cat", "--token-stdin", "a.md"]));
    assert!(handed_back(&["add", "--title", "x", "plan.K5BOARD"]));
    assert!(!handed_back(&["cat", "plan.k5board"]));
    assert!(!handed_back(&["add", "--x", "a.md"]));
}

#[test]
fn a_refusal_names_its_exit() {
    // 409 by code; error.exit wins.
    let mut pr = py::Pr::default();
    let r = fail_lines(
        &mut pr,
        "409",
        br#"{"error":{"code":"base_expired","hint":"read again"}}"#,
        false,
    );
    assert_eq!(r.unwrap(), Exit::TooOld);
    assert_eq!(
        pr.err,
        "Error: read again\n{\"error\": {\"code\": \"base_expired\", \"hint\": \"read again\", \"status\": 409}}\n"
    );
    let mut pr = py::Pr::default();
    let r = fail_lines(&mut pr, "000", b"", true);
    assert_eq!(r.unwrap(), Exit::Error);
    assert_eq!(
        pr.err,
        format!("{{\"error\": {{\"code\": \"daemon_unreachable\", \"hint\": \"no answer from the {PRODUCT_NAME} daemon\", \"status\": 0}}}}\n")
    );
    let mut pr = py::Pr::default();
    let r = fail_lines(&mut pr, "422", br#"{"error":{"code":"x","exit":7}}"#, true);
    assert_eq!(r.unwrap(), Exit::DaemonUnreachable);
}
