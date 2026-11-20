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
fn only_a_host_the_binary_can_t_send_to_goes_to_the_shell() {
    assert!(handed_back(&[
        "cat",
        "--host",
        "https://box.example/prefix",
        "a.md"
    ]));
    assert!(handed_back(&["cat", "--host=[::1]:4100", "a.md"]));
    assert!(handed_back(&["cores", "--host", "a b"]));
    assert!(!handed_back(&["cat", "--host", "box.example", "a.md"]));
    assert!(!handed_back(&[
        "cat",
        "--host=http://127.0.0.1:4100",
        "--token-stdin",
        "a.md"
    ]));
    assert!(!handed_back(&["cat", "--host=", "a.md"]));
    assert!(!handed_back(&["cat", "a.md", "--host"]));
    assert!(!handed_back(&["cat", "--token", "t", "a.md"]));
    assert!(!handed_back(&["add", "--title", "x", "plan.K5BOARD"]));
}

#[test]
fn a_board_verb_on_a_board_file_is_the_board_s() {
    assert!(board::call(&["add", "--title", "x", "plan.K5BOARD"]));
    assert!(board::call(&["read", "s-1", "plan.k5board\n"]));
    assert!(!board::call(&["cat", "plan.k5board"]));
    assert!(!board::call(&["add", "--x", "a.md"]));
    assert!(!board::call(&["add", "--file=plan.k5board"]));
    assert!(!board::call(&["plan.k5board"]));
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

#[test]
fn a_byte_that_is_not_utf8_goes_out_as_python_writes_it() {
    use super::py::{escaped, stdin_text_as};
    use crate::cards::Decode;
    let body = b"{\"p\": \"caf\xe9\"}";
    // A UTF-8 locale: Python's decode raises.
    let e = stdin_text_as(body, 3, Some(Decode::Strict)).unwrap_err();
    assert!(
        e.text().ends_with(
            "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 10: invalid continuation byte\n"
        ),
        "{}",
        e.text()
    );
    // The C locale: the byte stands for itself (surrogateescape).
    let t = stdin_text_as(body, 3, Some(Decode::Lossy)).unwrap();
    assert_eq!(t, format!("{{\"p\": \"caf{}\"}}", escaped(0xe9)));
    let mut pr = py::Pr::default();
    pr.line(&t);
    pr.line(&py::dumps(&py::s(&t)));
    pr.line(&crate::out::py::repr(&py::s(&t)));
    pr.eline(&t);
    let mut want: Vec<u8> = b"{\"p\": \"caf\xe9\"}\n".to_vec();
    want.extend_from_slice(br#""{\"p\": \"caf\udce9\"}""#);
    want.push(b'\n');
    want.extend_from_slice(br#"'{"p": "caf\udce9"}'"#);
    want.push(b'\n');
    assert_eq!(pr.out, want, "{}", String::from_utf8_lossy(&pr.out));
    assert_eq!(pr.err, "{\"p\": \"caf\\udce9\"}\n");
    // Text that is UTF-8 passes as it is.
    let mut pr = py::Pr::default();
    pr.line("é \\udbff\\udf41");
    assert_eq!(pr.out, "é \\udbff\\udf41\n".as_bytes());
}
