use super::cards::{self, Card};
use super::marks::{self, Mark};
use super::post::{self, Line};
use super::*;
use crate::exit::Exit;
use k2_client::brand::CLI_NAME;

fn source(file: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file),
    )
    .unwrap()
}

fn line(words: &[&str]) -> Line {
    match post::parse(words) {
        Some(Ok(l)) => l,
        other => panic!("{words:?}: {other:?}"),
    }
}

fn stop(words: &[&str]) -> (String, Exit) {
    match post::parse(words) {
        Some(Err(s)) => s,
        other => panic!("{words:?}: {other:?}"),
    }
}

#[test]
fn the_verb_is_the_first_word_past_the_flags() {
    assert_eq!(first_word(&["--from", "ask", "bob", "hi"]), "bob");
    assert_eq!(first_word(&["--json", "ask", "bob"]), "ask");
    assert_eq!(first_word(&["--seq", "3", "unread"]), "unread");
    assert_eq!(first_word(&["--json"]), "");
    // A one-dash word is a word here.
    assert_eq!(first_word(&["-x", "bob"]), "-x");
    let shell = source("browser.sh");
    let row: String = VALUE_FLAGS.join("|");
    assert!(shell.contains(&format!("            {row})\n")), "{row}");
}

#[test]
fn a_write_and_a_read_read_their_words() {
    let l = line(&[
        "bob", "a", "b", "--from", "me", "--json", "--file", "x.rs:3",
    ]);
    assert_eq!((l.target.as_str(), l.text.as_str()), ("bob", "b"));
    assert_eq!((l.from.as_str(), l.json), ("me", true));
    assert_eq!(l.files, ["x.rs:3"]);
    let l = line(&["bob", "--seq", "007"]);
    assert_eq!(
        (l.one_seq.as_str(), l.before_seq.as_str(), l.limit.as_str()),
        ("7", "8", "1")
    );
    assert_eq!(stop(&[]), (post::usage(), Exit::Error));
    assert!(post::usage().starts_with(&format!(
        "Error: address required\nUsage: {CLI_NAME} thread <addr> \"text\""
    )));
    for bad in ["0", "x", "-1", "1.5"] {
        assert_eq!(stop(&["bob", "--seq", bad]).1, Exit::Error, "{bad}");
    }
    assert_eq!(stop(&["bob", "t", "--seq", "2"]).0, post::SEQ_ALONE);
    assert_eq!(stop(&["bob", "--seq"]).1, Exit::Error);
    assert_eq!(stop(&["bob", "--file", ""]).0, post::FILE_NEEDS);
    assert_eq!(
        stop(&["bob", "--nope"]),
        (post::unknown_flag("--nope"), Exit::Error)
    );
    assert_eq!(stop(&["bob", "--idempotency-key"]).1, Exit::Usage);
    for flag in ["--from", "--limit", "--before-seq"] {
        assert_eq!(stop(&["bob", flag]), (post::needs_value(flag), Exit::Usage));
    }
    // Specs Python would read in its own way go to the shell.
    assert_eq!(post::parse(&["bob", "--file", "~ada/x"]), None);
    assert_eq!(post::parse(&["bob", "--file", "a:\u{0663}"]), None);
    assert_eq!(post::parse(&["bob", "--file", "a\nb"]), None);
}

#[test]
fn bash_s_numbers() {
    assert_eq!(post::bash_number("10"), Some(10));
    assert_eq!(post::bash_number("010"), Some(10));
    assert_eq!(post::bash_number(""), None);
    assert_eq!(
        post::bash_number("18446744073709551617"),
        Some(1),
        "64-bit, wrapping"
    );
    assert_eq!(post::bash_arith("010"), Some(8));
    assert_eq!(post::bash_arith("08"), None);
    assert_eq!(post::bash_arith("9"), Some(9));
}

#[test]
fn a_spec_s_line() {
    assert_eq!(
        post::split_line("a.rs:3"),
        Some(("a.rs".into(), "3".into(), None))
    );
    assert_eq!(
        post::split_line("a:b.rs:010-12"),
        Some(("a:b.rs".into(), "10".into(), Some("12".into())))
    );
    assert_eq!(
        post::split_line("x:1:2"),
        Some(("x:1".into(), "2".into(), None)),
        "the first colon whose rest is a line"
    );
    assert_eq!(post::split_line(":3"), None);
    assert_eq!(post::split_line("a:3-"), None);
    assert_eq!(post::split_line("a:3-4-5"), None);
    let shell = source("browser.sh");
    assert!(shell.contains(r#"m = re.match(r"^(.+?):(\d+)(?:-(\d+))?$", spec)"#));
}

#[test]
fn the_attachments_json() {
    let dir = std::env::temp_dir().join(format!("k5-thread-att-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "x").unwrap();
    std::fs::write(dir.join("b:3"), "x").unwrap();
    let cwd = dir.to_string_lossy().into_owned();
    let got = post::attachments(
        &cwd,
        &[
            "src/a.rs:4-9".into(),
            "b:3".into(),
            "gone.rs:2".into(),
            "./src/../src/a.rs".into(),
        ],
    );
    assert_eq!(
        got,
        format!(
            r#"[{{"path": "{cwd}/src/a.rs", "line": 4, "lineEnd": 9}}, {{"path": "{cwd}/b:3"}}, {{"path": "gone.rs", "line": 2}}, {{"path": "{cwd}/src/a.rs"}}]"#
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_write_s_form_and_the_read_s_query() {
    let l = line(&["bob", "hi there", "--from", "me"]);
    assert_eq!(
        post::form(&l, "me", "k-1", Some("[]")),
        "addr=bob&text=hi%20there&from=me&via=thread&clientMsgId=k-1&attachments=%5B%5D"
    );
    assert_eq!(
        post::query(&line(&["bob", "--all", "--limit", "3"])),
        "addr=bob&limit=0"
    );
    assert_eq!(
        post::query(&line(&["bob", "--limit", "3", "--before-seq", "9"])),
        "addr=bob&limit=3&before_seq=9"
    );
    assert_eq!(
        post::query(&line(&["bob", "--seq", "4"])),
        "addr=bob&limit=1&before_seq=5"
    );
}

fn shown(stdout: &str, stderr: &str, exit: i32) -> py::Shown {
    py::Shown {
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.into(),
        exit,
    }
}

#[test]
fn the_write_s_lines() {
    assert_eq!(
        post::posted(b"{\"ok\":true,\"id\":\"m1\",\"seq\":4,\"from\":\"me\",\"attachments\":[{\"path\":\"a.rs\",\"line\":3,\"lineEnd\":9,\"size\":2048},{\"path\":\"b\",\"size\":10}]}\n"),
        shown("m1  seq=4  from=me\n  [file] a.rs:3-9 (2 KB)\n  [file] b (10 B)\n", "", 0)
    );
    assert_eq!(
        post::posted(b"{\"ok\":false,\"error\":{\"code\":\"nope\"}}\n"),
        shown("", "nope: {'code': 'nope'}\n", 2)
    );
    assert_eq!(
        post::posted(b"{\"ok\":false,\"error\":\"down\"}\n"),
        shown("", "down\n", 2)
    );
    assert_eq!(post::posted(b"<html>\n"), shown("<html>\n", "", 0));
    let raised = post::posted(b"[1]\n");
    assert_eq!(raised.exit, 1);
    assert!(raised
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
}

#[test]
fn the_read_s_lines() {
    let body = "{\"ok\":true,\"items\":[{\"seq\":1,\"doc\":{\"from\":\"ada\",\"body\":\"a\\r\\nb\\u2028c\"}},{\"seq\":2,\"doc\":{\"deleted\":true,\"from\":\"bob\",\"author\":{\"kind\":\"guest\",\"login\":\"g1\"},\"context\":{\"placement\":\"p\",\"text\":\"hé\"},\"attachments\":[{\"state\":\"hidden\"},{\"path\":\"x\",\"size\":1,\"state\":\"gone\"}]}}]}\n";
    assert_eq!(
        post::shown(body.as_bytes(), "", false, "bob"),
        shown(
            "[1] from ada: a\n  | b\n  | c\n[2] from guest g1: (message deleted)\n  [context from Garden widget p, not typed by guest g1] \"h\\u00e9\"\n  [file] (not shared with you)\n  [file] x (1 B) (gone)\n",
            "",
            0
        )
    );
    assert_eq!(
        post::shown(b"{\"ok\":true,\"items\":[]}\n", "", false, "bob"),
        shown("(empty)\n", "", 0)
    );
    assert_eq!(
        post::shown(
            b"{\"ok\":true,\"items\":[{\"seq\":1}]}\n",
            "5",
            false,
            "bob"
        ),
        shown(
            "",
            &format!("No message 5 on this Thread ({CLI_NAME} thread bob lists them as [N]).\n"),
            1
        )
    );
    assert_eq!(
        post::shown(
            b"{\"ok\":true,\"items\":[{\"seq\":5},{\"seq\":4}],\"has_more\":true}\n",
            "5",
            true,
            "bob"
        ),
        shown(
            "{\"ok\": true, \"items\": [{\"seq\": 5}], \"has_more\": false}\n",
            "",
            0
        )
    );
    assert_eq!(
        post::shown(b"{\"ok\":false}  \n", "", true, "bob"),
        shown("{\"ok\":false}\n", "", 2)
    );
    let raised = post::shown(
        b"{\"ok\":true,\"items\":[{\"doc\":{\"body\":5}}]}\n",
        "",
        false,
        "b",
    );
    assert_eq!(raised.exit, 1);
    assert!(raised
        .stderr
        .ends_with("AttributeError: 'int' object has no attribute 'replace'\n"));
}

#[test]
fn ask_s_words_body_and_lines() {
    let Some(Ok(Card::Ask(a))) = cards::parse(
        "ask",
        &[
            "ask",
            "bob",
            "Pick?",
            "--option",
            "a\r\nb",
            "--option",
            "",
            "--allow-custom",
            "--expires",
            "2h",
        ],
    ) else {
        panic!("ask");
    };
    assert_eq!(
        cards::ask_body(&a, "me", "k1"),
        r#"{"addr": "bob", "prompt": "Pick?", "from": "me", "idempotency_key": "k1", "option": ["a", "b", ""], "allow_custom": true, "expires": "2h"}"#
    );
    let stop = |w: &[&str]| match cards::parse("ask", w) {
        Some(Err(s)) => s,
        other => panic!("{w:?}: {other:?}"),
    };
    assert_eq!(stop(&["ask", "bob", "p"]).1, Exit::Error);
    assert_eq!(
        stop(&["ask", "bob", "p", "--option", "a", "--options", "b"]).0,
        "Error: mix of --options and --option is not allowed\n"
    );
    assert_eq!(
        stop(&["ask", "bob", "p", "x", "--option", "a"]).0,
        "Error: unexpected extra argument 'x'\n"
    );
    assert_eq!(
        stop(&["ask", "bob", "p", "--from"]).0,
        "Error: --from requires a value\n"
    );
    assert_eq!(
        stop(&["ask", "bob", "p", "--idempotency-key"]).1,
        Exit::Usage
    );
    assert_eq!(
        cards::hint(br#"{"hint":"split at ,"}"#),
        Some("hint: split at ,".into())
    );
    assert_eq!(cards::hint(br#"{"hint":""}"#), None);
    assert_eq!(cards::hint(b"[1]"), None);
    assert_eq!(
        cards::asked(b"{\"ok\":true,\"id\":\"c1\",\"seq\":7}\n"),
        shown("c1  card #7\n", "", 0)
    );
    assert_eq!(
        cards::asked(b"{\"ok\":true,\"id\":\"c1\"}\n"),
        shown("c1\n", "", 0)
    );
}

#[test]
fn secret_s_words_body_and_lines() {
    let Some(Ok(Card::Secret(v))) = cards::parse(
        "secret",
        &[
            "secret",
            "bob",
            "--name",
            "API",
            "--for",
            "a.com",
            "--once",
            "--inject-header",
            "X-Key",
            "--dest",
            "vault",
        ],
    ) else {
        panic!("secret");
    };
    assert_eq!(
        cards::secret_body(&v, "me"),
        r#"{"addr": "bob", "name": "API", "from": "me", "dest": "vault", "for": "a.com", "once": true, "inject": {"dst": "header:X-Key"}}"#
    );
    let stop = |w: &[&str]| match cards::parse("secret", w) {
        Some(Err(s)) => s,
        other => panic!("{w:?}: {other:?}"),
    };
    assert_eq!(
        stop(&["secret", "bob"]).0,
        "Error: secret --name is required\n"
    );
    assert_eq!(
        stop(&["secret", "bob", "--name", "A", "--dest", "x"]).0,
        "Error: --dest vault only\n"
    );
    assert_eq!(
        stop(&["secret", "bob", "--ttl"]).0,
        "Error: --ttl requires a value\n"
    );
    assert_eq!(
        cards::secret_json(b"{\"id\":\"c\",\"value\":\"v\",\"secret\":\"s\"}\n"),
        shown("{\"id\": \"c\"}\n", "", 0)
    );
    assert_eq!(
        cards::secret_shown(b"{\"ok\":true,\"id\":\"c\",\"sends\":\"API to a.com\"}\n"),
        shown("c\n", "card: API to a.com\n", 0)
    );
}

#[test]
fn withdraw_dismiss_and_delete() {
    let void = |w: &[&str]| match cards::parse(first_word(w), w) {
        Some(Ok(Card::Void(v))) => v,
        other => panic!("{w:?}: {other:?}"),
    };
    assert_eq!(void(&["void", "bob", "c1"]).verb, "withdraw");
    assert_eq!(
        void(&["dismiss", "bob", "c1", "--why", "done"]).verb,
        "dismiss"
    );
    // The shell's own rule: a second verb word before the address wins
    // over `withdraw`.
    assert_eq!(void(&["withdraw", "dismiss", "bob", "c1"]).verb, "dismiss");
    assert_eq!(
        cards::void_body(&void(&["dismiss", "bob", "c1", "--why", "done"])),
        r#"{"addr": "bob", "id": "c1", "reason": "done"}"#
    );
    assert_eq!(
        cards::voided(b"{\"ok\":false,\"error\":{\"code\":\"card_not_yours\",\"hint\":\"h\"}}\n"),
        shown("", "card_not_yours: h\n", 3)
    );
    assert_eq!(
        cards::voided(b"{\"ok\":true,\"id\":\"c1\",\"status\":\"voided\"}\n"),
        shown("c1  voided\n", "", 0)
    );
    let del = |w: &[&str]| match cards::parse("delete", w) {
        Some(Ok(Card::Delete(d))) => d,
        other => panic!("{w:?}: {other:?}"),
    };
    assert_eq!(
        cards::delete_body(&del(&["delete", "bob", " #007 "])),
        r#"{"addr": "bob", "seq": 7}"#
    );
    assert_eq!(
        cards::delete_body(&del(&["delete", "bob", "m_1"])),
        r#"{"addr": "bob", "id": "m_1"}"#
    );
    assert_eq!(cards::parse("delete", &["delete", "bob", "\u{00b2}"]), None);
    assert_eq!(
        cards::deleted(b"{\"ok\":false,\"error\":{\"code\":\"not_found\"}}\n", true),
        shown("{\"ok\":false,\"error\":{\"code\":\"not_found\"}}\n", "", 4)
    );
    assert_eq!(cards::deleted(b"nope\n", false), shown("nope\n", "", 2));
    assert_eq!(
        cards::deleted(
            b"{\"ok\":true,\"seq\":3,\"id\":\"m\",\"already\":true}\n",
            false
        ),
        shown("[3] m  already deleted\n", "", 0)
    );
}

#[test]
fn read_all_and_unread() {
    assert_eq!(
        marks::parse("read-all", &["read-all", "bob", "--json"]),
        Ok(Mark::ReadAll {
            agent: "bob".into(),
            json: true
        })
    );
    assert_eq!(
        marks::parse("read-all", &["read-all", "a", "b"])
            .unwrap_err()
            .1,
        Exit::Error
    );
    assert_eq!(
        marks::parse("unread", &["unread", "--kind"]),
        Err((post::needs_value("--kind"), Exit::Usage))
    );
    assert_eq!(
        marks::marked(b"{\"ok\":true,\"marked\":[1],\"unread\":0,\"waiting\":2}\n", "bob"),
        shown(
            "bob: marked 1 Thread read; unread now 0\n  2 cards still waiting on you (answer them to clear)\n",
            "",
            0
        )
    );
    assert_eq!(
        marks::counts(b"{\"ok\":true,\"person\":\"ada\",\"total\":3,\"waitingTotal\":1,\"byConversation\":[{\"kind\":\"thread\",\"id\":\"c\",\"unread\":120,\"waiting\":1,\"muted\":true}]}\n"),
        shown(
            "ada: 3 unread+waiting (badge), 1 waiting\n  thread  c  unread=99+  (1 waiting, muted)\n",
            "",
            0
        )
    );
    assert_eq!(marks::counts(b"x\n"), shown("x\n", "", 2));
}

#[test]
fn the_markdown_hint_counts() {
    let text = source("prelude.sh");
    assert!(text.contains(&format!(
        "        echo \"{}\" >&2",
        post::NUDGE.replace('`', "\\`")
    )));
}
