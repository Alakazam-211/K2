use super::*;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The fixture entry with `edit` applied to its text.
fn fixture_with(edit: impl Fn(&str) -> String) -> Result<Vec<SpecRow>, String> {
    let text = std::fs::read_to_string(root().join(FIXTURE_ENTRIES))
        .unwrap_or_else(|e| panic!("read {FIXTURE_ENTRIES}: {e}"));
    rows_of("t.entries", &edit(&text))
}

fn problems(edit: impl Fn(&str) -> String) -> String {
    match fixture_with(edit) {
        Ok(rows) => panic!("no problem found: {rows:?}"),
        Err(e) => e,
    }
}

#[test]
fn the_lists_are_build_cli_s() {
    let script = std::fs::read_to_string(root().join("scripts/build-cli.sh"))
        .expect("read scripts/build-cli.sh");
    let line = script
        .lines()
        .find_map(|l| l.strip_prefix("SPEC_LISTS = "))
        .expect("build-cli.sh has a SPEC_LISTS line");
    let theirs: serde_json::Map<String, J> =
        serde_json::from_str(line).expect("SPEC_LISTS is JSON");
    let ours: serde_json::Map<String, J> = LISTS
        .iter()
        .map(|(k, l)| {
            (
                k.to_string(),
                J::Array(l.iter().map(|w| J::String(w.to_string())).collect()),
            )
        })
        .collect();
    assert_eq!(
        theirs, ours,
        "SPEC_LISTS in scripts/build-cli.sh and LISTS here"
    );
}

#[test]
fn the_fixture_is_one_row() {
    let rows = fixture_with(str::to_string).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        rows,
        [SpecRow {
            file: "t.entries".into(),
            entry: "compute stats".into(),
            words: vec!["compute".into(), "stats".into()],
            style: Style::For,
            usage: "usage: k2 compute stats <job> [--json]  (a job id or its first 8 characters)"
                .into(),
            args: vec![Arg {
                name: "job".into(),
                check: Some(Check {
                    ranges: vec![
                        (b'A', b'Z'),
                        (b'a', b'z'),
                        (b'0', b'9'),
                        (b'_', b'_'),
                        (b'-', b'-')
                    ],
                    min: 4,
                    max: 64
                })
            }],
            json_flag: true,
            method: "GET",
            path: "/cli/compute/stats".into(),
            tcp: false,
            query: vec![Item {
                key: "job".into(),
                from: Source::Arg("job".into()),
                raw: true
            }],
            json: Some(Print::Body),
            text: Print::Lines,
            exits: vec![],
        }]
    );
}

/// Today no entry under cli/src is a spec verb: the table is empty.
#[test]
fn no_entry_is_a_spec_verb_yet() {
    let rows = rows_in(&root()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows, []);
    let text = render_table(&rows, "super", "x");
    assert!(
        text.ends_with("pub static SPEC: &[SpecVerb] = &[];\n"),
        "{text}"
    );
}

#[test]
fn each_field_is_checked() {
    let cases: &[(&str, &str, &str)] = &[
        (
            r#""via": "compute""#,
            r#""via": "sidecar""#,
            r#"entry 1 (compute stats): "call" "via" must be one of compute, not "sidecar""#,
        ),
        (
            r#""method": "GET""#,
            r#""method": "POST""#,
            r#""call" "method" must be one of GET, not "POST""#,
        ),
        (
            r#""style": "for""#,
            r#""style": "while""#,
            r#""parse" "style" must be one of none, for, not "while""#,
        ),
        (
            r#""style": "for""#,
            r#""style": "none""#,
            "the style none reads no words, but job is required",
        ),
        (
            r#""auth": "session""#,
            r#""auth": "owner""#,
            r#""auth" must be one of session, not "owner""#,
        ),
        (
            r#""text": "lines""#,
            r#""text": "table""#,
            r#""output" "text" must be one of body, lines, not "table""#,
        ),
        (
            r#""impl": "spec""#,
            r#""impl": "bash""#,
            r#"parse, call, output, auth only go with "impl": "spec""#,
        ),
        (
            r#""impl": "spec""#,
            r#""impl": "rust""#,
            r#""impl" must be one of bash, native, spec, not "rust""#,
        ),
        (
            r#""^[A-Za-z0-9_-]{4,64}$""#,
            r#""^[A-Za-z0-9_-]+$""#,
            r#""parse" "check" for job must be ^[class]{min,max}$"#,
        ),
        (
            r#""^[A-Za-z0-9_-]{4,64}$""#,
            r#""^[A-z]{4,64}$""#,
            r#""parse" "check" for job must be ^[class]{min,max}$"#,
        ),
        (
            r#""check": {"job""#,
            r#""check": {"jb""#,
            "\"parse\" \"check\" names jb, which is not an arg",
        ),
        (
            r#""from": "<job>""#,
            r#""from": "<id>""#,
            "the query's job is from <id>, which is not an arg",
        ),
        (
            r#""from": "<job>""#,
            r#""from": "job""#,
            r#"the query's job is from <arg> or =literal, not "job""#,
        ),
        (
            r#""from": "<job>""#,
            r#""from": "=a b""#,
            r#"the query's job literal is A-Z, a-z, 0-9 and _ . -, not "a b""#,
        ),
        (
            r#""enc": "raw""#,
            r#""enc": "form""#,
            "the query's job enc is raw or quote, not form",
        ),
        (
            r#""key": "job""#,
            r#""key": "Job""#,
            r#"a query key is a-z, 0-9 and _, not "Job""#,
        ),
        (
            r#""path": "/cli/compute/stats""#,
            r#""path": "/v1/x""#,
            r#""call" "path" is a /cli/ route"#,
        ),
        (
            r#""json": "body", "#,
            "",
            r#""output" has "json" when the verb takes --json, and only then"#,
        ),
        (
            r#""auth": "session""#,
            r#""auth": "session", "colour": 1"#,
            r#""colour" is not an entry key"#,
        ),
        (
            r#""via": "compute""#,
            r#""via": "compute", "body": {}"#,
            r#""call" has no key "body""#,
        ),
        (
            r#""auth": "session""#,
            r#""exits": {"500": 4}, "auth": "session""#,
            r#""exits" maps 404 to an exit (0 to 255), not "500": 4"#,
        ),
        (
            r#""auth": "session""#,
            r#""exits": {"404": 300}, "auth": "session""#,
            r#""exits" maps 404 to an exit (0 to 255), not "404": 300"#,
        ),
        (
            r#""arg": null"#,
            r#""arg": "<v>""#,
            "a spec verb's only flag is --json, a switch",
        ),
        (
            "\"name\": \"compute stats\"",
            "\"name\": \"stats\"",
            "a spec verb's name is a noun and a verb",
        ),
    ];
    for (from, to, want) in cases {
        let got = problems(|t| {
            assert!(t.contains(from), "the fixture has no {from}");
            t.replacen(from, to, 1)
        });
        assert!(
            got.contains(want),
            "{from} -> {to}: wanted {want:?} in:\n{got}"
        );
        assert!(got.starts_with("t.entries: entry 1 ("), "{got}");
    }
}

#[test]
fn a_404_exit_is_the_row_s() {
    let rows = fixture_with(|t| {
        t.replacen(
            r#""auth": "session""#,
            r#""exits": {"404": 4}, "auth": "session""#,
            1,
        )
    })
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows[0].exits, [(404, 4)]);
    let text = render_table(&rows, "super", "x");
    assert!(text.contains("        exits: &[(404, 4)],\n"), "{text}");
}

#[test]
fn every_problem_is_reported() {
    let got = problems(|t| {
        t.replacen(r#""via": "compute""#, r#""via": "x""#, 1)
            .replacen(r#""auth": "session""#, r#""auth": "y""#, 1)
    });
    assert_eq!(got.lines().count(), 2, "{got}");
}

#[test]
fn an_entry_without_impl_is_no_row() {
    let rows = rows_of("t.entries", "    {\"name\": \"a b\", \"usage\": \"u\", \"description\": \"d\"},\n    {\"name\": \"c\", \"usage\": \"u\", \"description\": \"d\", \"impl\": \"native\"},\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows, []);
    let got = rows_of("t.entries", "    {\"name\": \"a b\", \"usage\": \"u\", \"description\": \"d\", \"auth\": \"session\"},\n").unwrap_err();
    assert_eq!(
        got,
        "t.entries: entry 1 (a b): auth only go with \"impl\": \"spec\""
    );
    assert!(rows_of("t.entries", "{}")
        .unwrap_err()
        .contains("must end with `},`"));
}

#[test]
fn a_check_is_a_closed_regex() {
    let c = parse_check("^[a-z0-9._-]{1,8}$").expect("a check");
    assert_eq!(
        c,
        Check {
            ranges: vec![
                (b'a', b'z'),
                (b'0', b'9'),
                (b'.', b'.'),
                (b'_', b'_'),
                (b'-', b'-')
            ],
            min: 1,
            max: 8
        }
    );
    for bad in [
        "^[a-z]{1,8}",
        "[a-z]{1,8}$",
        "^[a-z]+$",
        "^[a-y]{1,8}$",
        "^[a-z]{8,1}$",
        "^[a-z]{0,0}$",
        "^[-a]{1,2}$",
        "^[a/]{1,2}$",
        "^[]{1,2}$",
    ] {
        assert_eq!(parse_check(bad), None, "{bad}");
    }
}

#[test]
fn the_fixture_table_renders_its_row() {
    let rows = fixture_with(str::to_string).unwrap_or_else(|e| panic!("{e}"));
    let text = render_table(&rows, "k5_cli::spec", FIXTURE_ENTRIES);
    for want in [
        "use k5_cli::spec::{Arg, Call, Check, Cred, Enc, Item, Method, Output, Parse, Print, Source, SpecVerb, Style, Via};",
        "        entry: \"compute stats\",\n        words: &[\"compute\", \"stats\"],",
        "            args: &[Arg { name: \"job\", check: Some(Check { ranges: &[(b'A', b'Z'), (b'a', b'z'), (b'0', b'9'), (b'_', b'_'), (b'-', b'-')], min: 4, max: 64 }) }],",
        "            query: &[Item { key: \"job\", from: Source::Arg(\"job\"), enc: Enc::Raw }],",
        "            usage: concat!(\"usage: \", k2_client::cli_name!(), \" compute stats <job> [--json]  (a job id or its first 8 characters)\"),",
        "        output: Output { json: Some(Print::Body), text: Print::Lines },",
    ] {
        assert!(text.contains(want), "no {want:?} in:\n{text}");
    }
    // A hand-edited table is stale.
    let outs = outputs(&root()).unwrap_or_else(|e| panic!("{e}"));
    let fixture = outs
        .iter()
        .find(|o| o.path == FIXTURE_RS)
        .expect("the fixture table");
    assert_ne!(fixture.text.replace("min: 4", "min: 3"), fixture.text);
}

/// A usage line takes the command's name from the brand wherever it types
/// it as a word, and is a plain literal where it doesn't.
#[test]
fn the_command_name_comes_from_the_brand() {
    assert_eq!(branded("usage: x y"), "\"usage: x y\"");
    assert_eq!(
        branded("usage: k2 a, or k2"),
        "concat!(\"usage: \", k2_client::cli_name!(), \" a, or \", k2_client::cli_name!())"
    );
    assert_eq!(branded("k2 a"), "concat!(k2_client::cli_name!(), \" a\")");
    // Not a word: part of a path, a name or a token.
    for s in ["~/.k2 x", "k2so x", "ak2 x", "k2-x y", "k2: x"] {
        assert_eq!(branded(s), format!("{s:?}"), "{s}");
    }
}
