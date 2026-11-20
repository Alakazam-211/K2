use super::*;
use crate::families::traceback;

#[test]
fn the_words_are_the_shells() {
    assert_eq!(parse(&[]), Ask::Help);
    assert_eq!(parse(&["help"]), Ask::Help);
    assert_eq!(
        parse(&["verify", "a", "b"]),
        Ask::Local(vec!["verify".into(), "a".into(), "b".into()])
    );
    assert_eq!(
        parse(&["request", "--payload"]),
        Ask::Stop(vec![], Exit::Error)
    );
    assert_eq!(parse(&["status", "--out"]), Ask::Stop(vec![], Exit::Error));
    assert!(
        matches!(parse(&["request"]), Ask::Stop(l, Exit::Usage) if l[0].starts_with("Usage: "))
    );
    assert!(
        matches!(parse(&["request", "--x"]), Ask::Stop(l, Exit::Usage) if l[0].contains("'--x'"))
    );
    assert_eq!(
        parse(&[
            "request",
            "--payload",
            "p",
            "--wait",
            "--to",
            "ada",
            "--json",
            "--out",
            "o"
        ]),
        Ask::Request {
            payload: "p".into(),
            to: "ada".into(),
            wait: true,
            out: "o".into(),
            json: true
        }
    );
    assert_eq!(
        parse(&["status", "a", "b", "--json"]),
        Ask::Status {
            id: "b".into(),
            out: String::new(),
            json: true
        }
    );
    assert!(matches!(
        parse(&["status", "--out", "f"]),
        Ask::Stop(_, Exit::Usage)
    ));
    assert!(matches!(
        parse(&["status", "-x"]),
        Ask::Stop(_, Exit::Usage)
    ));
    assert!(matches!(parse(&["cancel", ""]), Ask::Stop(_, Exit::Usage)));
    assert_eq!(parse(&["cancel", "ap_1", "x"]), Ask::Cancel("ap_1".into()));
    assert_eq!(parse(&["agents"]), Ask::AgentsList);
    assert_eq!(parse(&["agents", ""]), Ask::AgentsList);
    assert_eq!(
        parse(&["agents", "w", "on"]),
        Ask::AgentsSet("w".into(), true)
    );
    assert!(matches!(
        parse(&["agents", "w", "On"]),
        Ask::Stop(_, Exit::Usage)
    ));
    assert!(matches!(parse(&["nope"]), Ask::Stop(l, Exit::Usage) if l[0].ends_with(": nope")));
}

#[test]
fn a_request_and_a_list() {
    let one = print(br#"{"id":"ap_1","state":"approved","kind":"release","person":"ada","answerBy":1763600000,"device":"phone"}"#);
    assert_eq!(
        one.stdout,
        "ap_1  approved   release  to ada, answer by 00:53:20 UTC  (on phone)\n"
    );
    let none = print(br#"{"requests":[]}"#);
    assert_eq!(none.stdout, "No approval requests from this agent.\n");
    let rows = print(br#"{"requests":[{"id":"a","state":"waiting"},{"id":"b"}]}"#);
    assert_eq!(rows.stdout, "a  waiting      to , answer by -\n");
    assert_eq!(
        rows.stderr,
        traceback(&[(18, "<module>"), (8, "line")], "KeyError: 'state'")
    );
    assert_eq!(rows.exit, Exit::Error);
    let t = print(br#"{"id":"a","state":"x","answerBy":"soon"}"#);
    assert_eq!(
        t.stderr,
        traceback(
            &[(13, "<module>"), (8, "line"), (6, "t")],
            "TypeError: 'str' object cannot be interpreted as an integer"
        )
    );
    let bare = print(b"[1]");
    assert_eq!(
        bare.stderr,
        traceback(
            &[(4, "<module>")],
            "AttributeError: 'list' object has no attribute 'get'"
        )
    );
}

#[test]
fn the_agents_list() {
    let s = agents_reader(
        br#"{"agents":[{"workspace":"w","grantedBy":"owner"},{"workspace":2,"grantedBy":null}]}"#,
    );
    assert_eq!(
        s.stdout,
        "w  (turned on by owner)\n2  (turned on by None)\n"
    );
    assert_eq!(
        agents_reader(br#"{"agents":null}"#).stdout,
        "No agent may ask for signed approvals yet.\n"
    );
    let bad = agents_reader(br#"{"agents":["w"]}"#);
    assert_eq!(
        bad.stderr,
        traceback(
            &[(4, "<module>"), (4, "<genexpr>")],
            "TypeError: string indices must be integers, not 'str'"
        )
    );
}

#[test]
fn the_record() {
    let dir = std::env::temp_dir().join(format!("k5-approve-rec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("rec.txt");
    let path = f.to_str().unwrap();
    let s = write_record(br#"{"id":"a","state":"approved","recordText":"R\n"}"#, path);
    assert_eq!(s.exit, Exit::Ok);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "R\n");
    let no = write_record(br#"{"id":"a","state":"declined"}"#, path);
    assert_eq!(
        no.stderr,
        "Error: a is declined; there is no approval record to write\n"
    );
    assert_eq!(no.exit, Exit::Error);
    let gone = write_record(
        br#"{"id":"a","state":"approved","recordText":"R"}"#,
        dir.join("no/such").to_str().unwrap(),
    );
    assert!(gone.stderr.ends_with(&format!(
        "FileNotFoundError: [Errno 2] No such file or directory: '{}'\n",
        dir.join("no/such").display()
    )));
    std::fs::remove_dir_all(&dir).unwrap();
}
