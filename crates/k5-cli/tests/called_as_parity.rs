//! What the front hands the shell CLI, and what the shell does with it.
//! The shell reads no name it was called by (every line it prints carries
//! the command's own name), so `called_as` is the front's alone (its own
//! tests are in `src/called_as.rs`) and the front hands no name on. The
//! shell still takes `K5_CLI_CALLED_AS` out of the environment with the
//! front's other variables, and reads the notice and via flags.

mod common;

#[cfg(unix)]
use k5_cli::called_as::fallthrough_env;

/// The prelude's lines that read the other two variables the front sets.
fn handoff_block() -> String {
    let last = r#"if [ "${K5_CLI_VIA:-}" = "rust" ]; then _K5_CLI_VIA="rust"; fi"#;
    common::block(&common::prelude(), r#"_K5_NOTICE_DONE="""#, last)
}

/// The prelude's line that takes the front's variables out of the
/// environment.
const UNSET_LINE: &str = "unset K5_CLI_CALLED_AS K5_CLI_NOTICE_DONE K5_CLI_VIA K5_CLI_WOULD_REFUSE";

#[test]
fn the_prelude_reads_no_name_and_still_holds_the_handoff() {
    let prelude = common::prelude();
    assert!(handoff_block().contains("K5_CLI_NOTICE_DONE"));
    assert!(
        !prelude.contains("_K2_CALLED_AS"),
        "the shell reads a name again"
    );
    assert!(
        !prelude.contains("${0##*/}"),
        "the shell reads $0's name again"
    );
    let unsets = prelude.lines().filter(|l| *l == UNSET_LINE).count();
    assert_eq!(unsets, 1, "{UNSET_LINE}");
}

/// What the front hands on is what the shell acts on: the notice marked
/// done and the via flag. No name, and one set by a caller goes out of the
/// environment unread, so nothing the shell starts sees it.
#[cfg(unix)]
#[test]
fn the_fallthrough_env_is_what_the_shell_reads() {
    let script = format!(
        "{}\n{UNSET_LINE}\nprintf '%s|%s|%s\\n' \"$_K5_NOTICE_DONE\" \"$_K5_CLI_VIA\" \"${{K5_CLI_CALLED_AS-unset}}\"",
        handoff_block()
    );
    let env = fallthrough_env();
    let mut set_by_caller = env.to_vec();
    set_by_caller.push(("K5_CLI_CALLED_AS", "ab1"));
    for argv0 in ["k2", "k5", "/opt/k5-front/libexec/k2-cli"] {
        assert_eq!(
            common::bash(&script, argv0, &env),
            "1|rust|unset\n",
            "called from {argv0:?}"
        );
        assert_eq!(
            common::bash(&script, argv0, &set_by_caller),
            "1|rust|unset\n",
            "called from {argv0:?}, a name set by the caller"
        );
    }
    // Without the front, none is set.
    assert_eq!(common::bash(&script, "k5", &[]), "||unset\n");
}

/// The prelude's lines that read the folder the front passes and close the
/// script's descriptor the front handed over.
fn script_dir_block() -> String {
    common::block(
        &common::prelude(),
        r#"_K5_SCRIPT_DIR="${K5_CLI_SCRIPT_DIR:-}""#,
        "fi",
    )
}

/// The folder arrives as it was set; empty or unset is none.
#[cfg(unix)]
#[test]
fn the_script_dir_is_what_the_shell_reads() {
    use k5_cli::called_as::SCRIPT_DIR_VAR;
    let script = format!(
        "{}\n{}\nprintf '[%s]\\n' \"$_K5_SCRIPT_DIR\"",
        handoff_block(),
        script_dir_block()
    );
    assert!(script_dir_block().contains(SCRIPT_DIR_VAR));
    assert_eq!(
        common::bash(&script, "k2", &[(SCRIPT_DIR_VAR, "/opt/k5 x/bin")]),
        "[/opt/k5 x/bin]\n"
    );
    assert_eq!(common::bash(&script, "k2", &[(SCRIPT_DIR_VAR, "")]), "[]\n");
    assert_eq!(common::bash(&script, "k2", &[]), "[]\n");
    // Taken out of the environment: a child never sees it.
    let child = format!(
        "{}\n{}\nsh -c 'echo \"[${{{var}-unset}}]\"'",
        handoff_block(),
        script_dir_block(),
        var = SCRIPT_DIR_VAR
    );
    assert_eq!(
        common::bash(&child, "k2", &[(SCRIPT_DIR_VAR, "/opt/k5")]),
        "[unset]\n"
    );
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
        assert_eq!(
            common::bash(&script, argv0, env),
            *want,
            "$0 {argv0:?}, env {env:?}"
        );
    }
    // The three standard ones are never closed.
    for n in ["0", "1", "2"] {
        let script = format!("{}\n{}\necho still", handoff_block(), script_dir_block());
        assert_eq!(
            common::bash(&script, &format!("/dev/fd/{n}"), via),
            "still\n",
            "/dev/fd/{n}"
        );
    }
}
