use super::*;

fn same(w: &str) -> Option<String> {
    Some(w.to_string())
}

fn form(words: &[&str]) -> Option<Form> {
    parse(words, &same)
}

#[test]
fn the_verbs_and_their_aliases() {
    assert_eq!(
        form(&[]),
        Some(Form::List {
            json: false,
            ws: None
        })
    );
    assert_eq!(
        form(&["", "--json"]),
        Some(Form::List {
            json: false,
            ws: None
        })
    );
    assert_eq!(
        form(&["schedule", "list", "--json", "--workspace", "w"]),
        Some(Form::List {
            json: true,
            ws: Some("w".into())
        })
    );
    assert_eq!(
        form(&["list", "--archived", "--json"]),
        Some(Form::Archived { json: true })
    );
    assert_eq!(
        form(&["schedule", "archive", "-x", "a", "b"]),
        Some(Form::Remove {
            name: "a".into(),
            purge: false
        })
    );
    assert_eq!(
        form(&["signal", "fire", "a", "--force"]),
        Some(Form::Fire {
            name: "a".into(),
            force: true
        })
    );
    assert_eq!(
        form(&["restart", "a", "--workspace=w", "--json"]),
        Some(Form::Restart {
            name: "a".into(),
            ws: Some("w".into()),
            json: true
        })
    );
    assert_eq!(
        form(&["use-pinned-session", "1", "a"]),
        Some(Form::Pinned {
            on: true,
            name: "a".into()
        })
    );
    assert_eq!(form(&["help"]), Some(Form::Help));
}

#[test]
fn the_shell_s_own_forms() {
    assert_eq!(form(&["wake"]), None);
    assert_eq!(form(&["signal", "wake"]), None);
    assert_eq!(form(&["session", "a", "--set"]), None);
    assert_eq!(form(&["status", "a", "--limit"]), None);
    assert_eq!(form(&["log", "-n"]), None);
    assert_eq!(form(&["add", "--name"]), None);
    assert_eq!(form(&["add", "--name", "a", "--daily", "--time"]), None);
}

#[test]
fn the_stops() {
    let Some(Form::Stop(lines, exit)) = form(&["frobnicate"]) else {
        panic!("not a stop");
    };
    assert_eq!(exit, Exit::Usage);
    assert_eq!(lines[0], "Unknown 'heartbeat' subcommand: frobnicate");
    let Some(Form::Stop(lines, exit)) = form(&["on"]) else {
        panic!("not a stop");
    };
    assert_eq!(exit, Exit::Error);
    assert_eq!(lines[0], "Error: 'heartbeat on' is no longer supported.");
    let Some(Form::Stop(_, exit)) = form(&["restart"]) else {
        panic!("not a stop");
    };
    assert_eq!(exit, Exit::Usage);
    let Some(Form::Stop(_, exit)) = form(&["enable"]) else {
        panic!("not a stop");
    };
    assert_eq!(exit, Exit::Error);
    let Some(Form::Stop(lines, _)) = form(&["edit", "a", "--instructions", " \n"]) else {
        panic!("not a stop");
    };
    assert_eq!(
        lines,
        vec!["Error: instructions cannot be blank".to_string()]
    );
}
