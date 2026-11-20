use super::*;
use crate::families::traceback;

#[test]
fn python_s_int() {
    assert_eq!(py_int(" +1_000 ").as_deref(), Some("1000"));
    assert_eq!(py_int("-007").as_deref(), Some("-7"));
    assert_eq!(py_int("-0").as_deref(), Some("0"));
    for bad in ["", " ", "1__0", "_1", "1_", "1.5", "x", "+"] {
        assert_eq!(py_int(bad), None, "{bad:?}");
    }
}

#[test]
fn the_readers() {
    let s = sender_reader(br#"{"sender":{"id":"s1","address":"a@x","path":"hosted","state":"ready","fromName":"A","envelope":"b.x","notReady":{"code":"dns","hint":"add it"},"next":"wait"}}"#);
    assert_eq!(
        s.stdout,
        "s1  a@x  hosted  ready  unassigned (A)\n  sends through b.x (bounces and the signature use it; the From is a@x)\n  not ready: dns: add it\n  next: wait\n"
    );
    assert_eq!(sender_reader(b"{}").stdout, "no sender\n");
    let t = test_reader(br#"{"sender":"s1","result":"sent","to":"o@x","checks":{"dmarc":{"ok":false,"hint":null},"dkimPublished":true,"spfPublished":false}}"#);
    assert_eq!(
        t.stdout,
        "test email s1: sent (to o@x)\n  DMARC: missing: None\n  DKIM key published: True; SPF published: False\n"
    );
    let a = agents_reader(br#"{"agents":[{"workspace":"w","on":false,"offReason":3}]}"#);
    assert_eq!(
        a.stderr,
        traceback(&[(7, "<module>")], "TypeError: can only concatenate str (not \"int\") to str")
    );
    let l = log_reader(br#"{"items":[{"createdAt":0,"id":"m1","status":"sent","to":["a@x","b@x"],"subject":"hi"},{"id":"m2","to":[5]}]}"#);
    assert_eq!(l.stdout, "1970-01-01 00:00  m1  sent       a@x +1  hi\n");
    assert_eq!(
        l.stderr,
        traceback(&[(10, "<module>")], "TypeError: unsupported operand type(s) for +: 'int' and 'str'")
    );
    let sup = suppressions_reader(br#"{"suppressions":[]}"#, "a@x");
    assert_eq!(sup.stdout, "No suppressed addresses matching that one.\n");
}
