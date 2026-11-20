use super::*;

#[test]
fn a_restore_that_never_ends_in_bash_goes_to_the_shell() {
    assert!(shells(&["restore", "w", "--rev"]));
    assert!(shells(&["restore", "--prefix", "a/", "--at"]));
    assert!(!shells(&["restore", "w", "--rev", "3"]));
    assert!(!shells(&["restore", "w", "--dry-run"]));
    assert!(!shells(&["restore", "w", "--json"]));
    // An unexpected word ends the loop first.
    assert!(!shells(&["restore", "w", "x", "--rev"]));
    assert!(!shells(&["list", "--rev"]));
}

#[test]
fn an_import_body_is_python_s() {
    let jsonl = "{\"space\":\"data:w\",\"rev\":3}\n{\"key\":\"a\",\"value\":{\"é\":1.50},\"rev\":1}\r\n\n{\"key\":\"b\",\"value\":null}";
    assert_eq!(
        import_body("w", "merge", jsonl).unwrap(),
        r#"{"widget":"w","mode":"merge","items":[{"key":"a","value":{"é":1.5}},{"key":"b","value":null}]}"#
    );
    assert_eq!(
        import_body("w", "replace", " [{\"key\":\"a\",\"value\":1}]").unwrap(),
        r#"{"widget":"w","mode":"replace","items":[{"key":"a","value":1}]}"#
    );
    let (said, exit) = import_body("w", "merge", "{\"key\":1}\n\nnope").unwrap_err();
    assert_eq!(
        (said.as_str(), exit),
        ("Error: line 3 is not JSON\n", Exit::Usage)
    );
    let (said, _) = import_body("w", "merge", "[1]").unwrap_err();
    assert_eq!(said, "Error: row 1 is not a JSON object\n");
    let (said, _) = import_body("w", "merge", "{\"key\":\"a\"}").unwrap_err();
    assert_eq!(said, "Error: row 1 needs \"key\" and \"value\"\n");
    let (said, exit) = import_body("w", "merge", "[1,").unwrap_err();
    assert!(
        said.ends_with("json.decoder.JSONDecodeError: Expecting value: line 1 column 4 (char 3)\n")
    );
    assert_eq!(exit, Exit::Error);
}

#[test]
fn an_export_s_blob_refs() {
    let a = format!("k2blob:sha256-{}", "a".repeat(64));
    let b = format!("k2blob:sha256-{}", "b".repeat(64));
    let jsonl = format!(
        "{{\"space\":\"x\",\"v\":\"{a}\"}}\n{{\"key\":\"k\",\"value\":[\"{b}\",{{\"x\":\"{a}\\n\"}},\"k2blob:sha256-short\"]}}\n\n{{\"key\":\"j\",\"value\":\"{b}\"}}\n"
    );
    assert_eq!(blob_refs(&jsonl).unwrap(), vec![a, b]);
}
