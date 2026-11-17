//! `called_as` answers as the shell CLI does: the block of
//! `cli/src/prelude.sh` that reads the name runs under bash for every row,
//! and its answer must be the Rust one.

mod common;

#[cfg(unix)]
use k5_cli::called_as::{called_as, CalledAs, Os};
#[cfg(unix)]
use std::ffi::OsStr;

/// The prelude's lines that set `_K2_CALLED_AS`.
fn called_as_block() -> String {
    common::block(&common::prelude(), r#"_k2_name="${0##*/}""#, "unset _k2_name")
}

/// The prelude's lines that read the other two variables the front sets.
fn handoff_block() -> String {
    let last = r#"if [ "${K5_CLI_VIA:-}" = "rust" ]; then _K5_CLI_VIA="rust"; fi"#;
    common::block(&common::prelude(), r#"_K5_NOTICE_DONE="""#, last)
}

#[test]
fn the_prelude_still_holds_both_blocks() {
    assert!(called_as_block().contains("K5_CLI_CALLED_AS"));
    assert!(handoff_block().contains("K5_CLI_NOTICE_DONE"));
}

#[cfg(unix)]
#[test]
fn every_argv0_and_value_reads_as_the_shell_reads_it() {
    let script = format!("{}\nprintf '%s\\n' \"$_K2_CALLED_AS\"", called_as_block());
    let argv0s = [
        "k2", "k5", "/usr/local/bin/k5", "./k5", "/tmp/x/k2", "k2so", "some-copy", "", "/", "/opt/k5/", "K5", "k5.exe", "k5 ",
        "C:\\bin\\k5",
    ];
    let values = [
        None,
        Some(""),
        Some("k2"),
        Some("k5"),
        Some("K5"),
        Some("k2 "),
        Some("/usr/local/bin/k5"),
        Some("k5-cli"),
        Some("k2so"),
    ];
    let mut wrong = Vec::new();
    for argv0 in argv0s {
        for value in values {
            let env: Vec<(&str, &str)> = value.map(|v| ("K5_CLI_CALLED_AS", v)).into_iter().collect();
            let shell = common::bash(&script, argv0, &env);
            let lookup = |name: &str| value.filter(|_| name == "K5_CLI_CALLED_AS").map(str::to_string);
            let rust = called_as(OsStr::new(argv0), lookup, Os::Unix);
            if shell.trim_end_matches('\n') != rust.name() {
                wrong.push(format!("argv0 {argv0:?}, K5_CLI_CALLED_AS={value:?}: shell {shell:?}, rust {:?}", rust.name()));
            }
        }
    }
    assert!(wrong.is_empty(), "{} rows differ:\n{}", wrong.len(), wrong.join("\n"));
}

/// What the front hands on is what the shell acts on: the name, the
/// notice marked done, and the via flag.
#[cfg(unix)]
#[test]
fn the_fallthrough_env_is_what_the_shell_reads() {
    let script = format!(
        "{}\n{}\nprintf '%s|%s|%s\\n' \"$_K2_CALLED_AS\" \"$_K5_NOTICE_DONE\" \"$_K5_CLI_VIA\"",
        called_as_block(),
        handoff_block()
    );
    for who in [CalledAs::K2Name, CalledAs::K5Name] {
        for argv0 in ["k2", "k5", "/opt/k5-front/libexec/k2-cli"] {
            let env = who.fallthrough_env();
            let shell = common::bash(&script, argv0, &env);
            assert_eq!(shell, format!("{}|1|rust\n", who.name()), "{who:?} called from {argv0:?}");
        }
    }
    // Without the front, none of the three is set.
    assert_eq!(common::bash(&script, "k5", &[]), "k5||\n");
}

/// The prelude's lines that read the folder the front passes and close the
/// script's descriptor the front handed over.
fn script_dir_block() -> String {
    common::block(&common::prelude(), r#"_K5_SCRIPT_DIR="${K5_CLI_SCRIPT_DIR:-}""#, "fi")
}

/// The folder arrives as it was set; empty or unset is none.
#[cfg(unix)]
#[test]
fn the_script_dir_is_what_the_shell_reads() {
    use k5_cli::called_as::SCRIPT_DIR_VAR;
    let script = format!("{}\n{}\nprintf '[%s]\\n' \"$_K5_SCRIPT_DIR\"", handoff_block(), script_dir_block());
    assert!(script_dir_block().contains(SCRIPT_DIR_VAR));
    assert_eq!(common::bash(&script, "k2", &[(SCRIPT_DIR_VAR, "/opt/k5 x/bin")]), "[/opt/k5 x/bin]\n");
    assert_eq!(common::bash(&script, "k2", &[(SCRIPT_DIR_VAR, "")]), "[]\n");
    assert_eq!(common::bash(&script, "k2", &[]), "[]\n");
    // Taken out of the environment: a child never sees it.
    let child = format!("{}\n{}\nsh -c 'echo \"[${{{var}-unset}}]\"'", handoff_block(), script_dir_block(), var = SCRIPT_DIR_VAR);
    assert_eq!(common::bash(&child, "k2", &[(SCRIPT_DIR_VAR, "/opt/k5")]), "[unset]\n");
}

/// Run from the front as `/dev/fd/N`, the shell closes N, so what it
/// starts doesn't inherit it. Only a plain number above 2, and only
/// through the front.
#[cfg(unix)]
#[test]
fn the_front_s_descriptor_is_closed_and_nothing_else() {
    let script = format!(
        "exec 9</dev/null\n{}\n{}\nif {{ : <&9; }} 2>/dev/null; then echo open; else echo closed; fi",
        handoff_block(),
        script_dir_block()
    );
    type Env<'a> = &'a [(&'a str, &'a str)];
    let via: Env = &[("K5_CLI_VIA", "rust")];
    let rows: &[(&str, Env, &str)] = &[
        ("/dev/fd/9", via, "closed\n"),
        ("/dev/fd/9", &[], "open\n"),
        ("/dev/fd/09", via, "open\n"),
        ("/dev/fd/9x", via, "open\n"),
        ("/dev/fd/", via, "open\n"),
        ("/tmp/fd/9", via, "open\n"),
        ("9", via, "open\n"),
    ];
    for (argv0, env, want) in rows {
        assert_eq!(common::bash(&script, argv0, env), *want, "$0 {argv0:?}, env {env:?}");
    }
    // The three standard ones are never closed.
    for n in ["0", "1", "2"] {
        let script = format!("{}\n{}\necho still", handoff_block(), script_dir_block());
        assert_eq!(common::bash(&script, &format!("/dev/fd/{n}"), via), "still\n", "/dev/fd/{n}");
    }
}
