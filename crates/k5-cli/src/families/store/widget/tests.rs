use super::*;

fn w(words: &[&str]) -> Vec<String> {
    words.iter().map(|x| (*x).to_string()).collect()
}

fn parsed(words: &[&str]) -> Result<Args, String> {
    let mut json = false;
    gd_args(&w(words), &mut json)
}

#[test]
fn the_flags_read_as_the_shell_reads_them() {
    let a = parsed(&["k", "--value", "{}", "--if-rev", "007", "--json"]).unwrap();
    assert_eq!(
        (a.pos.as_str(), a.value.as_str(), a.has_value),
        ("k", "{}", true)
    );
    assert_eq!(a.ifrev, "007");
    assert_eq!(parsed(&["--value"]), Err("--value takes JSON".into()));
    assert_eq!(parsed(&["--nope"]), Err("unknown flag --nope".into()));
    assert_eq!(parsed(&["a", "b", "c"]), Err("unexpected 'c'".into()));
    // `-y` is a flag; any other single dash word is a positional.
    let a = parsed(&["-y", "-x"]).unwrap();
    assert!(a.yes && a.pos == "-x");
    // The digits check comes after the loop, over all four at once.
    assert_eq!(
        parsed(&["--limit", "5x", "--nope"]),
        Err("unknown flag --nope".into())
    );
    assert_eq!(
        parsed(&["--wait", "-1"]),
        Err("--if-rev, --limit, --since and --wait take whole numbers".into())
    );
}

#[test]
fn a_body_is_python_s_json_dumps() {
    let a = parsed(&[
        "k é",
        "--value",
        "{\"b\": [1, 2.50, \"ü\"]}",
        "--if-rev",
        "007",
    ])
    .unwrap();
    assert_eq!(
        body("put", "w", &a, false).unwrap(),
        r#"{"widget": "w", "key": "k \u00e9", "ifRev": 7, "value": {"b": [1, 2.5, "\u00fc"]}}"#
    );
    let a = parsed(&[]).unwrap();
    assert_eq!(
        body("rm", "w", &a, false).unwrap_err(),
        concat!(
            "Error: a key is required (",
            cli_name!(),
            " store rm --widget <w> <key>)\n"
        )
    );
    let a = parsed(&["k"]).unwrap();
    assert_eq!(
        body("put", "w", &a, false).unwrap_err(),
        "Error: put takes --value <json> or --file <path>\n"
    );
    let a = parsed(&["k", "--value", "{nope"]).unwrap();
    assert_eq!(
        body("put", "w", &a, false).unwrap_err(),
        "Error: the value is not JSON: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)\n"
    );
    let a = parsed(&["k", "--value", "[1]"]).unwrap();
    assert_eq!(
        body("patch", "w", &a, false).unwrap_err(),
        "Error: patch takes a JSON object of the fields to change (null removes a field)\n"
    );
    let a = parsed(&["k", "--file", "/nonexistent/k5-gd"]).unwrap();
    assert_eq!(
        body("put", "w", &a, false).unwrap_err(),
        "Traceback (most recent call last):\n  File \"<string>\", line 17, in <module>\nFileNotFoundError: [Errno 2] No such file or directory: '/nonexistent/k5-gd'\n"
    );
    let a = parsed(&["--agent", "ag", "--level", "read"]).unwrap();
    assert_eq!(
        body("grant", "w", &a, false).unwrap(),
        r#"{"widget": "w", "agent": "ag", "prefix": "", "level": "read"}"#
    );
    assert_eq!(
        body("unadmin", "w", &a, true).unwrap(),
        r#"{"widget": "w", "agent": "ag", "prefix": "", "dryRun": true}"#
    );
    let a = parsed(&["--agent", "ag", "--project", "p"]).unwrap();
    assert_eq!(
        body("revoke", "w", &a, false).unwrap_err(),
        "Error: name exactly one of --agent or --project\n"
    );
    assert_eq!(
        body("admin", "w", &a, false).unwrap_err(),
        "Error: a grant admin is one agent (--agent)\n"
    );
    let a = parsed(&["--project", "p", "--level", "admin"]).unwrap();
    assert_eq!(
        body("grant", "w", &a, false).unwrap_err(),
        "Error: --level is read or write\n"
    );
}

#[test]
fn a_batch_is_planned_in_parts() {
    assert!(batch_plan(" \n")
        .unwrap_err()
        .contains("reads ops on stdin"));
    assert_eq!(
        batch_plan(
            "{\"op\":\"put\",\"key\":\"a\",\"value\":1}\n\n{\"op\":\"delete\",\"key\":\"b\"}"
        )
        .unwrap(),
        vec![r#"[{"op":"put","key":"a","value":1},{"op":"delete","key":"b"}]"#.to_string()]
    );
    assert_eq!(batch_plan("[]").unwrap_err(), "Error: no ops\n");
    assert_eq!(
        batch_plan("[1,").unwrap_err(),
        "Error: the ops are not JSON: Expecting value: line 1 column 4 (char 3)\n"
    );
    let many: Vec<String> = (0..130).map(|i| format!("{{\"key\":\"k{i}\"}}")).collect();
    let plan = batch_plan(&format!("[{}]", many.join(","))).unwrap();
    assert_eq!(plan.len(), 3, "64, 64 and 2 ops");
    // Over 60 KiB of values closes a part early.
    let big = "x".repeat(40 * 1024);
    let plan = batch_plan(&format!(
        "[{{\"value\":\"{big}\"}},{{\"value\":\"{big}\"}}]"
    ))
    .unwrap();
    assert_eq!(plan.len(), 2);
    let too_many: Vec<String> = (0..1001).map(|_| "{}".to_string()).collect();
    assert_eq!(
        batch_plan(&format!("[{}]", too_many.join(","))).unwrap_err(),
        "Error: at most 1,000 ops in one batch\n"
    );
    // A string op: `"value" in op` is a substring test, then `op.get`.
    assert!(batch_plan("[\"value\"]")
        .unwrap_err()
        .ends_with("AttributeError: 'str' object has no attribute 'get'\n"));
}

#[test]
fn a_restore_body_keeps_python_s_key_order() {
    assert_eq!(
        restore_body(
            "restore",
            "w",
            &w(&["--rev", "4", "--prefix", "a/", "--rev", "05", "--dry-run"])
        )
        .unwrap(),
        r#"{"widget": "w", "toRev": 5, "dryRun": true, "prefixes": ["a/"]}"#
    );
    assert_eq!(
        restore_body("restore", "w", &w(&["--at", "2026-10-26T17:30\n"])).unwrap(),
        r#"{"widget": "w", "atLocal": "2026-10-26T17:30\n"}"#
    );
    assert_eq!(
        restore_body("restore", "w", &w(&["--at", "17"])).unwrap(),
        r#"{"widget": "w", "at": 17}"#
    );
    assert_eq!(
        restore_body("restore", "w", &w(&["--at", "today"])).unwrap_err(),
        "Error: --at takes 2026-10-26T17:30 (local time) or seconds since 1970\n"
    );
    assert_eq!(
        restore_body("restore", "w", &w(&["--prefix", "a/"])).unwrap_err(),
        "Error: name exactly one of --rev, --at or --snapshot\n"
    );
    assert_eq!(
        restore_body("snapshot", "w", &w(&["--label", "l", "--except", "x/"])).unwrap(),
        r#"{"widget": "w", "label": "l", "except": ["x/"]}"#
    );
    assert_eq!(
        restore_body("snapshot", "w", &w(&["--rev", "1"])).unwrap_err(),
        concat!(
            "Error: unexpected --rev (see ",
            cli_name!(),
            " store --help)\n"
        )
    );
    assert_eq!(
        restore_body("snapshot", "w", &w(&["--label"])).unwrap_err(),
        "Error: --label takes a value\n"
    );
}

#[test]
fn a_failure_reads_as_gd_fail_reads_it() {
    let b = br#"{"error":{"code":"conflict","hint":"rev moved"}}"#;
    assert_eq!(fail_code(b), "conflict");
    assert_eq!(fail_line(b), "Error: conflict: rev moved");
    assert_eq!(fail_code(br#"{"error":"not_granted"}"#), "not_granted");
    assert_eq!(
        fail_line(br#"{"error":"not_granted"}"#),
        "Error: not_granted"
    );
    assert_eq!(fail_line(br#"{"ok":false}"#), "Error: None");
    assert_eq!(fail_line(br#"{"error":{}}"#), "Error: error: ");
    assert_eq!(fail_line(b"[1]"), "Error: daemon returned: [1]");
    assert_eq!(fail_code(b"nope"), "");
    assert_eq!(fail_exit("blob_missing"), Exit::NotFound);
    assert_eq!(fail_exit("conflict"), Exit::Conflict);
    assert_eq!(fail_exit("core_off"), Exit::Refused);
    assert_eq!(fail_exit("else"), Exit::Error);
}

#[test]
fn readers_print_as_the_shell_s_python() {
    let r = restore_out(
        br#"{"families":["a/","b/"],"id":null,"rev":null,"written":2,"deleted":0,"keys":["a/1","b/2"]}"#,
    )
    .unwrap();
    assert_eq!(
        r.stdout,
        "Would write 2 and delete 0 key(s) in a/, b/:\n  a/1\n  b/2\n"
    );
    let r = restore_out(br#"{"families":[],"id":"r1","rev":9,"written":1,"deleted":1,"keys":[]}"#)
        .unwrap();
    assert_eq!(
        r.stdout,
        "Restored: wrote 1, deleted 1 (nothing) as rev 9, restore r1.\n"
    );
    assert_eq!(r.stderr, "Undo it with --rev 8.\n");
    let r =
        restore_out(br#"{"families":[],"id":null,"rev":null,"written":0,"deleted":0}"#).unwrap();
    assert_eq!(r.stdout, "Nothing to change.\n");
    let r = granted(
        br#"{"grant":{"level":"read","prefix":"","subjectKind":"agent","subjectId":"a1"}}"#,
    )
    .unwrap();
    assert_eq!(r.stdout, "Granted read on everything to agent a1\n");
    let (_, _, x) = granted(b"{}").unwrap_err();
    assert_eq!(x.exception, "KeyError: 'grant'");
}

#[test]
fn small_pieces() {
    assert_eq!(b64encode(b""), "");
    assert_eq!(b64encode(b"h"), "aA==");
    assert_eq!(b64encode(b"hi!?"), "aGkhPw==");
    assert_eq!(basename("a/b/"), "b");
    assert_eq!(basename("f"), "f");
    assert!(local_stamp("2026-10-26T17:30:01"));
    assert!(!local_stamp("2026-10-26 17:30"));
    assert_eq!(int("000"), Value::Int("0".into()));
    let mut dict = Dict::default();
    dict.set("a", s("1"));
    dict.set("b", s("2"));
    dict.set("a", s("3"));
    assert_eq!(dumps_default(&dict.value()), r#"{"a": "3", "b": "2"}"#);
}
