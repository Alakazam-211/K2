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
