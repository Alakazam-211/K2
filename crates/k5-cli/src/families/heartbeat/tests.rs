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
    assert_eq!(form(&["wake"]), Some(Form::Wake(None)));
    assert_eq!(form(&["signal", "wake", "x"]), Some(Form::Wake(None)));
    assert_eq!(form(&["session", "a", "--set"]), None);
    assert_eq!(form(&["log", "-n"]), None);
    assert_eq!(form(&["add", "--name"]), None);
    assert_eq!(form(&["add", "--name", "a", "--daily", "--time"]), None);
}

#[test]
fn status_takes_a_limit_of_digits_only() {
    for (args, flag) in [
        (&["status", "a", "--limit"][..], "--limit"),
        (&["status", "a", "-n"][..], "-n"),
        (&["status", "a", "--limit", "5x"][..], "--limit"),
        (&["status", "a", "--limit", ""][..], "--limit"),
        (&["status", "--limit", "-3", "a"][..], "--limit"),
        (&["status", "a", "-n", "5", "--limit", "x"][..], "--limit"),
    ] {
        let Some(Form::Stop(lines, exit)) = form(args) else {
            panic!("{args:?}: not a stop");
        };
        assert_eq!(exit, Exit::Usage, "{args:?}");
        assert_eq!(
            lines,
            vec![format!("Error: {flag} needs a number")],
            "{args:?}"
        );
    }
    assert_eq!(
        form(&["status", "a", "-n", "5", "--limit", "007"]),
        Some(Form::Status {
            name: "a".into(),
            limit: "007".into()
        })
    );
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

#[test]
fn remind_alone_is_the_remind_help() {
    for rest in [&["remind"][..], &["remind", ""], &["remind", "", "x"]] {
        assert!(remind_alone(rest), "{rest:?}");
    }
    for rest in [
        &["reminders"][..],
        &["remind", "list"],
        &["remind", "help"],
        &["remind", "", "--in", "5m"],
        &["remind", "", "--help"],
        &[],
    ] {
        assert!(!remind_alone(rest), "{rest:?}");
    }
}
