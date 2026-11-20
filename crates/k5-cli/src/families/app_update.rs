//! `k5 app-update`: retired. As the shell CLI's `app-update` arm
//! (`cli/src/router.sh`): after the settings and the two gates
//! ([`crate::prelude`]), `fail_deprecated`'s lines
//! (`cli/src/deprecation.sh`) pointing at `update --app`, exit 1, whatever
//! words follow. It is an open tool and sends nothing.
//!
//! The shell answers ([`run`] returns `None`): `-h` or `--help` (the
//! front sends those on), a word that is not UTF-8, and a `python3` the
//! binary can't tell.

use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::prelude;
use crate::python;
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["app-update"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `fail_deprecated "app-update" "k2 update --app"`.
pub fn lines() -> [&'static str; 6] {
    [
        "Error: 'app-update' is no longer supported.",
        "",
        "Use instead:",
        concat!("  ", cli_name!(), " update --app"),
        "",
        concat!(
            "Run '",
            cli_name!(),
            " help-deprecated' for the full map of retired verbs."
        ),
    ]
}

/// `k5 app-update ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    args.iter().map(|a| a.to_str()).collect::<Option<Vec<_>>>()?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    for l in lines() {
        out::stderr_line(l);
    }
    Some(Exit::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_s_arm_and_lines() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src");
        let read = |f: &str| {
            std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"))
        };
        assert!(read("router.sh").contains(
            "    app-update)\n        # Phase 2.1 A1: hard-deprecated. Use update --app.\n        fail_deprecated \"app-update\" \"k2 update --app\"\n        ;;"
        ));
        assert!(read("deprecation.sh").contains(
            "Error: '$old_verb' is no longer supported.\n\nUse instead:\n  $new_form\n\nRun 'k2 help-deprecated' for the full map of retired verbs.\nEOF\n    exit 1"
        ));
    }
}
