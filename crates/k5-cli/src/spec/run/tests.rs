use super::*;
use crate::spec::{Arg, Call, Check, Item, Output, Parse};

const USAGE: &str = "usage: k2 compute stats <job> [--json]  (a job id or its first 8 characters)";

const JOB: Check = Check {
    ranges: &[
        (b'A', b'Z'),
        (b'a', b'z'),
        (b'0', b'9'),
        (b'_', b'_'),
        (b'-', b'-'),
    ],
    min: 4,
    max: 64,
};

/// The fixture's row (`crates/k5-cli/tests/data/spec_fixture.entries`).
static STATS: SpecVerb = SpecVerb {
    entry: "compute stats",
    words: &["compute", "stats"],
    parse: Parse {
        style: Style::For,
        usage: USAGE,
        args: &[Arg {
            name: "job",
            check: Some(JOB),
        }],
        json_flag: true,
    },
    call: Call {
        via: Via::Compute,
        method: Method::Get,
        path: "/cli/compute/stats",
        tcp: false,
        query: &[Item {
            key: "job",
            from: Source::Arg("job"),
            enc: Enc::Raw,
        }],
    },
    output: Output {
        json: Some(Print::Body),
        text: Print::Lines,
    },
    exits: &[],
    auth: Cred::Session,
};

static NODES: SpecVerb = SpecVerb {
    entry: "compute nodes",
    words: &["compute", "nodes"],
    parse: Parse {
        style: Style::None,
        usage: "usage: k2 compute nodes",
        args: &[],
        json_flag: true,
    },
    call: Call {
        via: Via::Compute,
        method: Method::Get,
        path: "/cli/compute/nodes",
        tcp: true,
        query: &[
            Item {
                key: "kind",
                from: Source::Literal("all"),
                enc: Enc::Raw,
            },
            Item {
                key: "q",
                from: Source::Arg("q"),
                enc: Enc::Quote,
            },
        ],
    },
    output: Output {
        json: Some(Print::Body),
        text: Print::Body,
    },
    exits: &[(404, 4)],
    auth: Cred::Session,
};

fn line<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec!["compute", "stats"];
    v.extend_from_slice(words);
    v
}

#[test]
fn the_for_style_is_cmd_compute_stats_loop() {
    let ok = |words: &[&str], json: bool, job: &str| {
        assert_eq!(
            parse(&STATS, &line(words)),
            Ok(Ask {
                json,
                values: vec![("job", job.to_string())]
            }),
            "{words:?}"
        );
    };
    ok(&["abcd"], false, "abcd");
    ok(&["abcd", "--json"], true, "abcd");
    ok(&["--json", "abcd", "--json"], true, "abcd");
    // An empty word leaves room for the next one.
    ok(&["", "abcd"], false, "abcd");
    for bad in [
        &[][..],
        // `[ -z "$job" ] || usage`: an empty word after the job is a
        // second one.
        &["abcd", ""],
        &["abc"],
        &["abcd", "efgh"],
        &["-x", "abcd"],
        &["abcd", "-"],
        &["abcd", "--JSON"],
        &["ab.cd"],
        &[""],
        &["--json"],
    ] {
        assert_eq!(parse(&STATS, &line(bad)), Err(USAGE), "{bad:?}");
    }
}

#[test]
fn the_none_style_reads_only_json() {
    let words = ["compute", "nodes", "x", "--y", "--json"];
    assert_eq!(
        parse(&NODES, &words),
        Ok(Ask {
            json: true,
            values: vec![]
        })
    );
    assert!(!parse(&NODES, &["compute", "nodes", "x"]).unwrap().json);
}

#[test]
fn the_query_is_bash_s() {
    let ask = |v: &str| Ask {
        json: false,
        values: vec![("job", v.to_string()), ("q", v.to_string())],
    };
    assert_eq!(query(&STATS, &ask("ab_C-9")), "job=ab_C-9");
    assert_eq!(
        query(&NODES, &ask("/w x&é")),
        "kind=all&q=%2Fw%20x%26%C3%A9"
    );
    // An empty value is left out.
    assert_eq!(query(&NODES, &ask("")), "kind=all");
}

#[test]
fn only_an_id_tool_is_run() {
    assert_eq!(id_tool_of(&STATS), Some("compute"));
    let open = SpecVerb {
        words: &["wiki", "list"],
        ..STATS
    };
    assert_eq!(id_tool_of(&open), None, "wiki is an open tool");
    let split = SpecVerb {
        words: &["agent", "identity"],
        ..STATS
    };
    assert_eq!(
        id_tool_of(&split),
        None,
        "agent alone is not agent-identity"
    );
}

fn ok(status: u16, body: &str) -> Result<Response, Error> {
    Ok(Response {
        status,
        body: body.as_bytes().to_vec(),
        channel: Channel::Tcp,
        permission: None,
        server_version: None,
    })
}

#[test]
fn the_raw_text_and_its_split_are_bash_s() {
    let s = |raw: &[u8]| {
        let (st, b) = split(raw);
        (st, String::from_utf8(b).unwrap())
    };
    assert_eq!(
        s(&raw(&ok(200, "{\"a\":1}\n\n"), 0)),
        ("200".into(), "{\"a\":1}\n\n".into())
    );
    assert_eq!(s(&raw(&ok(200, "a\0b"), 0)), ("200".into(), "ab".into()));
    assert_eq!(s(&raw(&ok(404, ""), 0)), ("404".into(), String::new()));
    // Each refused try's `-w` line comes first.
    assert_eq!(
        s(&raw(&ok(200, "{}"), 2)),
        ("200".into(), "\n000\n000{}".into())
    );
    // Nothing accepted the connection: four `-w` lines and the printf.
    let dead = Err(Error::TimedOut {
        endpoint: "/x".into(),
    });
    assert_eq!(
        s(&raw(&dead, 3)),
        ("000".into(), "\n000\n000\n000\n000".into())
    );
    assert_eq!(s(&raw(&dead, 0)), ("000".into(), "\n000".into()));
    assert_eq!(s(b""), ("000".into(), String::new()));
    assert_eq!(s(b"200"), ("200".into(), String::new()));
    assert_eq!(s(b"x\n"), ("000".into(), "x".into()));
}

#[test]
fn the_fail_line_is_compute_fail_s() {
    let f = |body: &str| fail_line("400", body.as_bytes());
    assert_eq!(
        f(r#"{"error":{"code":"bad","hint":"h é"}}"#),
        r#"{"error": "bad", "hint": "h \u00e9"}"#
    );
    assert_eq!(f(r#"{"error":{}}"#), r#"{"error": "error", "hint": ""}"#);
    assert_eq!(
        f(r#"{"error":{"code":null,"hint":[1]}}"#),
        r#"{"error": null, "hint": [1]}"#
    );
    assert_eq!(
        f(r#"{"error":"route_floor","message":"owner only"}"#),
        r#"{"error": "route_floor", "hint": "owner only"}"#
    );
    assert_eq!(
        f(r#"{"error":"x","message":1}"#),
        r#"{"error": "error", "hint": "x"}"#
    );
    assert_eq!(
        f(r#"{"error":[1,"a"]}"#),
        r#"{"error": "error", "hint": "[1, 'a']"}"#
    );
    assert_eq!(
        f(r#"{"ok":false}"#),
        r#"{"error": "error", "hint": "None"}"#
    );
    assert_eq!(f("[1]"), r#"{"error": "error", "hint": "None"}"#);
    assert_eq!(
        f("<html>oops</html>\n\n"),
        "daemon_error: <html>oops</html>"
    );
    assert_eq!(f("\n000\n000"), "daemon_error: \n000\n000");
    assert_eq!(
        f(&"é".repeat(300)),
        format!("daemon_error: {}", "é".repeat(200))
    );
    assert_eq!(
        fail_line("000", b""),
        format!("{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the {PRODUCT_NAME} daemon (status 000)\"}}")
    );
}

#[test]
fn the_exits_are_compute_fail_s() {
    for (status, want) in [
        ("400", 2),
        ("413", 2),
        ("404", 3),
        ("403", 3),
        ("500", 3),
        ("000", 3),
        ("201", 3),
    ] {
        assert_eq!(exit_for(&STATS, status).code(), want, "{status}");
    }
    assert_eq!(exit_for(&NODES, "404"), Exit::NotFound);
    assert_eq!(exit_for(&NODES, "400"), Exit::Usage);
}

#[test]
fn the_lines_are_compute_text_s() {
    let out = |body: &str| lines(body.as_bytes());
    assert_eq!(out(r#"{"text":["a","b é"]}"#), Shown::out("a\nb é\n"));
    for none in [
        r#"{}"#,
        r#"{"text":[]}"#,
        r#"{"text":null}"#,
        r#"{"text":""}"#,
        r#"{"text":0}"#,
        r#"{"text":false}"#,
    ] {
        assert_eq!(out(none), Shown::out(format!("{NO_TEXT}\n")), "{none}");
    }
    assert_eq!(out(r#"{"text":"ab"}"#), Shown::out("a\nb\n"));
    assert_eq!(out(r#"{"text":{"k":1,"j":2}}"#), Shown::out("k\nj\n"));
    let raises = |body: &str, exception: &str| {
        let s = out(body);
        assert_eq!((s.stdout.as_str(), s.exit), ("", Exit::Error), "{body}");
        let frame =
            "Traceback (most recent call last):\n  File \"<string>\", line 1, in <module>\n";
        let last = s
            .stderr
            .strip_prefix(frame)
            .unwrap_or_else(|| panic!("{body}: {}", s.stderr));
        assert!(
            last.starts_with(exception) && last.ends_with('\n') && last.lines().count() == 1,
            "{body}: {}",
            s.stderr
        );
    };
    raises(
        "",
        "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)\n",
    );
    raises("<html>", "json.decoder.JSONDecodeError: ");
    raises(
        "[1]",
        "AttributeError: 'list' object has no attribute 'get'",
    );
    raises(
        r#"{"text":[1]}"#,
        "TypeError: sequence item 0: expected str instance, int found",
    );
    raises(
        r#"{"text":["a",null]}"#,
        "TypeError: sequence item 1: expected str instance, NoneType found",
    );
    raises(r#"{"text":5}"#, "TypeError: can only join an iterable");
    raises(r#"{"text":true}"#, "TypeError: can only join an iterable");
}
