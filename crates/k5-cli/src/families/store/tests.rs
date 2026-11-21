use super::*;

fn form(words: &[&str]) -> Form {
    parse(words, false).form
}

fn ask(words: &[&str], env_json: bool) -> Ask {
    match parse(words, env_json).form {
        Form::Py(a) => *a,
        other => panic!("{words:?}: not a call: {other:?}"),
    }
}

#[test]
fn the_words_come_to_the_shell_s_forms() {
    assert_eq!(form(&[]), Form::Help);
    assert_eq!(form(&["get", "c", "--widget", "w"]), Form::Shell);
    assert_eq!(
        form(&["nope"]),
        Form::Usage(
            concat!(
                "unknown store command 'nope' — see ",
                cli_name!(),
                " store --help"
            )
            .into()
        )
    );
    assert_eq!(form(&["list", "x"]), Form::Usage("unexpected 'x'".into()));
    assert_eq!(form(&["--workspace", "w"]), Form::Help);
    // `put --json <doc>` takes the document; `--json` before a flag or
    // at the end is the output switch.
    let a = ask(&["put", "c", "--json", "{\"a\":1}", "--id", "d"], false);
    assert_eq!(
        (
            a.name.as_str(),
            a.doc_json.as_str(),
            a.doc_id.as_str(),
            a.json
        ),
        ("c", "{\"a\":1}", "d", false)
    );
    let a = ask(&["put", "c", "--json"], false);
    assert!(a.json && a.doc_json.is_empty());
    assert_eq!(form(&["put", "c", "--id"]), Form::Shift("--id".into()));
    // `get` with one word: `shift 2` fails, and the loop reads the word.
    assert_eq!(form(&["get", "c"]), Form::Usage("unexpected 'c'".into()));
    let a = ask(&["get", "--json"], false);
    assert!(a.json && a.name == "--json");
    let a = ask(&["get"], false);
    assert!(a.name.is_empty() && a.doc_id.is_empty());
    assert_eq!(form(&["query", "c", "--where"]), Form::Shift("--where".into()));
    let a = ask(&["query", "c", "--where", "x", "--limit", "5"], false);
    assert_eq!(a.limit, "5");
    // `rm` and `drop` take no flags: extra words are ignored, and the
    // caller's `D_JSON` is the output switch.
    let a = ask(&["rm", "c", "d", "--json", "extra"], true);
    assert!(a.json && a.doc_id == "d");
    let a = ask(&["drop", "c", "x"], false);
    assert!(!a.json && a.name == "c");
}
