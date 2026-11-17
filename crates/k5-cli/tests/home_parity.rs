//! Where the K2 home and a workspace's folder are, case by case: what the
//! shell CLI answers (its own lines of `cli/src/prelude.sh`, run under
//! bash) and what k5-cli answers (k2-client's rules, through `home`).
//!
//! Each row is made for real in a temp folder. A row marked `DIFFERS` is
//! a case where the two answer differently today; the code follows
//! k2-client and K2 rules on each before the CLI uses it. The test fails
//! when a marked row stops differing or an unmarked one starts, so the
//! list stays exact.

mod common;

use k5_cli::home::{k2_home_with, workspace_dot_dir, HomeError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A thing a row makes, relative to the user home or the workspace.
#[derive(Clone, Copy, Debug)]
enum Make {
    Dir(&'static str),
    File(&'static str),
    /// A symbolic link at `.0` to `.1` (relative to the link's folder).
    Link(&'static str, &'static str),
}

/// An answer: a folder under the user home (or the workspace), the value
/// of an override variable, or an error naming a variable.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Want {
    Under(&'static str),
    OverrideOf(&'static str),
    Error(&'static str),
}

/// What an override variable is set to in a row.
#[derive(Clone, Copy, Debug)]
enum Value {
    /// An absolute path, this name under the row's temp folder.
    Abs(&'static str),
    /// Exactly this text.
    Raw(&'static str),
}

const DIFFERS: bool = true;
const SAME: bool = false;

struct HomeRow {
    case: &'static str,
    make: &'static [Make],
    env: &'static [(&'static str, Value)],
    shell: Want,
    k5: Want,
    differs: bool,
}

const HOME_ROWS: &[HomeRow] = &[
    HomeRow { case: ".k2 is a folder", make: &[Make::Dir(".k2")], env: &[], shell: Want::Under(".k2"), k5: Want::Under(".k2"), differs: SAME },
    HomeRow {
        case: "both .k2 and .k2so are folders",
        make: &[Make::Dir(".k2"), Make::Dir(".k2so")],
        env: &[],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    HomeRow {
        case: "only .k2so is a folder (a machine the 0.40 migration never ran on)",
        make: &[Make::Dir(".k2so")],
        env: &[],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "neither folder (a fresh machine)",
        make: &[],
        env: &[],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2"),
        differs: DIFFERS,
    },
    HomeRow {
        case: ".k2 is a file",
        make: &[Make::File(".k2")],
        env: &[],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "only a .k5 folder (no code knows .k5 as the home)",
        make: &[Make::Dir(".k5")],
        env: &[],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2"),
        differs: DIFFERS,
    },
    HomeRow {
        case: ".k2 is a link to a folder",
        make: &[Make::Dir("real-home"), Make::Link(".k2", "real-home")],
        env: &[],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    HomeRow {
        case: ".k2so is the migration's link to .k2",
        make: &[Make::Dir(".k2"), Make::Link(".k2so", ".k2")],
        env: &[],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    HomeRow {
        case: ".k2 is a dangling link",
        make: &[Make::Link(".k2", "gone")],
        env: &[],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "K5_HOME_OVERRIDE is an absolute path",
        make: &[Make::Dir(".k2")],
        env: &[("K5_HOME_OVERRIDE", Value::Abs("rig-home"))],
        shell: Want::Under(".k2"),
        k5: Want::OverrideOf("K5_HOME_OVERRIDE"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "K2_HOME_OVERRIDE is an absolute path, no folder in the user home",
        make: &[],
        env: &[("K2_HOME_OVERRIDE", Value::Abs("rig-home"))],
        shell: Want::Under(".k2so"),
        k5: Want::OverrideOf("K2_HOME_OVERRIDE"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "both override names: K5_ wins",
        make: &[Make::Dir(".k2")],
        env: &[("K5_HOME_OVERRIDE", Value::Abs("new-rig")), ("K2_HOME_OVERRIDE", Value::Abs("old-rig"))],
        shell: Want::Under(".k2"),
        k5: Want::OverrideOf("K5_HOME_OVERRIDE"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "K5_HOME_OVERRIDE set but empty, K2_HOME_OVERRIDE absolute (the daemon's reader: set wins, even empty)",
        make: &[Make::Dir(".k2")],
        env: &[("K5_HOME_OVERRIDE", Value::Raw("")), ("K2_HOME_OVERRIDE", Value::Abs("rig-home"))],
        shell: Want::Under(".k2"),
        k5: Want::Error("K5_HOME_OVERRIDE"),
        differs: DIFFERS,
    },
    HomeRow {
        case: "K2_HOME_OVERRIDE is a relative path",
        make: &[Make::Dir(".k2")],
        env: &[("K2_HOME_OVERRIDE", Value::Raw("rel/home"))],
        shell: Want::Under(".k2"),
        k5: Want::Error("K2_HOME_OVERRIDE"),
        differs: DIFFERS,
    },
];

struct DotRow {
    case: &'static str,
    make: &'static [Make],
    shell: Want,
    k5: Want,
    differs: bool,
}

const DOT_ROWS: &[DotRow] = &[
    DotRow { case: "neither folder: a new workspace", make: &[], shell: Want::Under(".k2"), k5: Want::Under(".k2"), differs: SAME },
    DotRow { case: "only .k2", make: &[Make::Dir(".k2")], shell: Want::Under(".k2"), k5: Want::Under(".k2"), differs: SAME },
    DotRow {
        case: "only .k2so: an older workspace keeps its name",
        make: &[Make::Dir(".k2so")],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2so"),
        differs: SAME,
    },
    DotRow {
        case: "both: .k2 wins",
        make: &[Make::Dir(".k2"), Make::Dir(".k2so")],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    DotRow { case: "a file named .k2", make: &[Make::File(".k2")], shell: Want::Under(".k2so"), k5: Want::Under(".k2so"), differs: SAME },
    DotRow {
        case: "a file named .k2 beside a .k2so folder",
        make: &[Make::File(".k2"), Make::Dir(".k2so")],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2so"),
        differs: SAME,
    },
    DotRow { case: "a file named .k2so", make: &[Make::File(".k2so")], shell: Want::Under(".k2"), k5: Want::Under(".k2"), differs: SAME },
    DotRow {
        case: "only .k5 (neither rule knows it yet)",
        make: &[Make::Dir(".k5")],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    DotRow {
        case: ".k5 beside .k2so",
        make: &[Make::Dir(".k5"), Make::Dir(".k2so")],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2so"),
        differs: SAME,
    },
    DotRow {
        case: ".k2 is a link to a folder",
        make: &[Make::Dir("elsewhere"), Make::Link(".k2", "elsewhere")],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    DotRow {
        case: ".k2 is a link to a file",
        make: &[Make::File("a-file"), Make::Link(".k2", "a-file")],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2so"),
        differs: SAME,
    },
    DotRow {
        case: ".k2 is a dangling link",
        make: &[Make::Link(".k2", "gone")],
        shell: Want::Under(".k2"),
        k5: Want::Under(".k2"),
        differs: SAME,
    },
    DotRow {
        case: ".k2so is a link to a folder, no .k2",
        make: &[Make::Dir("elsewhere"), Make::Link(".k2so", "elsewhere")],
        shell: Want::Under(".k2so"),
        k5: Want::Under(".k2so"),
        differs: SAME,
    },
];

/// A fresh folder under the system temp folder, removed when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Temp {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock after 1970").as_nanos();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("k5-cli-{tag}-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir(&dir).unwrap_or_else(|e| panic!("make {}: {e}", dir.display()));
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            if !std::thread::panicking() {
                panic!("remove {}: {e}", self.0.display());
            }
        }
    }
}

fn has_link(make: &[Make]) -> bool {
    make.iter().any(|m| matches!(m, Make::Link(..)))
}

/// Makes `make` under `root`. False when it holds a link and links can't
/// be made here (not Unix): the caller leaves that row to Unix.
fn build(root: &Path, make: &[Make]) -> bool {
    if has_link(make) && !cfg!(unix) {
        return false;
    }
    for m in make {
        match *m {
            Make::Dir(name) => std::fs::create_dir(root.join(name)).unwrap_or_else(|e| panic!("mkdir {name}: {e}")),
            Make::File(name) => std::fs::write(root.join(name), b"not a folder").unwrap_or_else(|e| panic!("write {name}: {e}")),
            Make::Link(name, to) => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(to, root.join(name)).unwrap_or_else(|e| panic!("link {name} -> {to}: {e}"));
                #[cfg(not(unix))]
                unreachable!("{name} -> {to}");
            }
        }
    }
    true
}

/// The row's environment, with absolute values made under `root`.
fn row_env(root: &Path, env: &[(&'static str, Value)]) -> Vec<(&'static str, OsString)> {
    env.iter()
        .map(|(name, value)| {
            let v = match value {
                Value::Abs(sub) => root.join(sub).into_os_string(),
                Value::Raw(text) => OsString::from(text),
            };
            (*name, v)
        })
        .collect()
}

/// `want` as a result, for a row whose user home (or workspace) is `base`.
fn expect(want: Want, base: &Path, env: &[(&'static str, OsString)]) -> Result<PathBuf, String> {
    match want {
        Want::Under(name) => Ok(base.join(name)),
        Want::OverrideOf(var) => {
            Ok(PathBuf::from(&env.iter().find(|(n, _)| *n == var).unwrap_or_else(|| panic!("the row sets no {var}")).1))
        }
        Want::Error(var) => Err(var.to_string()),
    }
}

/// The marks are right: a row is marked `DIFFERS` exactly when its two
/// answers differ.
#[test]
fn every_row_is_marked_by_whether_the_answers_differ() {
    for row in HOME_ROWS {
        assert_eq!(row.differs, row.shell != row.k5, "home row {:?}: the DIFFERS mark is wrong", row.case);
        assert!(matches!(row.shell, Want::Under(_)), "home row {:?}: the shell always answers a folder", row.case);
    }
    for row in DOT_ROWS {
        assert_eq!(row.differs, row.shell != row.k5, "workspace row {:?}: the DIFFERS mark is wrong", row.case);
        assert!(!row.differs, "workspace row {:?}: the dot-dir rules agree today; a difference needs K2's ruling", row.case);
    }
}

#[test]
fn k5_cli_finds_the_home_by_k2_client_s_rule() {
    let mut ran = 0;
    for row in HOME_ROWS {
        let tmp = Temp::new("home");
        let user = tmp.0.join("user");
        std::fs::create_dir(&user).expect("make the user home");
        if !build(&user, row.make) {
            continue;
        }
        let env = row_env(&tmp.0, row.env);
        let lookup = |name: &str| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone());
        let got = k2_home_with(lookup, || Some(user.clone())).map_err(|e| match e {
            HomeError::Override(m) => m.name.to_string(),
            HomeError::NoUserHome => "no user home".to_string(),
        });
        assert_eq!(got, expect(row.k5, &user, &env), "home row {:?}", row.case);
        ran += 1;
    }
    assert!(ran >= HOME_ROWS.iter().filter(|r| !has_link(r.make)).count());
}

#[test]
fn k5_cli_finds_a_workspace_folder_by_the_daemon_s_rule() {
    let mut ran = 0;
    for row in DOT_ROWS {
        let tmp = Temp::new("ws");
        if !build(&tmp.0, row.make) {
            continue;
        }
        assert_eq!(Ok(workspace_dot_dir(&tmp.0)), expect(row.k5, &tmp.0, &[]), "workspace row {:?}", row.case);
        ran += 1;
    }
    assert!(ran >= DOT_ROWS.iter().filter(|r| !has_link(r.make)).count());
}

/// The shell's answer for every home row, from its own two lines.
#[cfg(unix)]
#[test]
fn the_shell_finds_the_home_as_the_rows_say() {
    let prelude = common::prelude();
    let first = r#"K2_HOME="${HOME}/.k2""#;
    let lines: Vec<&str> = prelude.lines().collect();
    let at = lines.iter().position(|l| *l == first).expect("prelude.sh sets K2_HOME from HOME");
    assert_eq!(lines.iter().filter(|l| **l == first).count(), 1, "K2_HOME is set from HOME once");
    let script = format!("{}\n{}\nprintf '%s' \"$K2_HOME\"", lines[at], lines[at + 1]);
    assert!(lines[at + 1].starts_with(r#"[ -d "$K2_HOME" ] || K2_HOME="#), "the fallback line moved: {}", lines[at + 1]);
    for row in HOME_ROWS {
        let tmp = Temp::new("shell-home");
        let user = tmp.0.join("user");
        std::fs::create_dir(&user).expect("make the user home");
        assert!(build(&user, row.make));
        let env = row_env(&tmp.0, row.env);
        let user_text = user.to_str().expect("a UTF-8 temp path");
        let mut vars: Vec<(&str, &str)> = vec![("HOME", user_text)];
        vars.extend(env.iter().map(|(n, v)| (*n, v.to_str().expect("UTF-8"))));
        let got = PathBuf::from(common::bash(&script, "k2", &vars));
        assert_eq!(Ok(got), expect(row.shell, &user, &env), "home row {:?}", row.case);
    }
}

/// The shell's answer for every workspace row, from its own `_ws_dot_dir`.
#[cfg(unix)]
#[test]
fn the_shell_finds_a_workspace_folder_as_the_rows_say() {
    let function = common::block(&common::prelude(), "_ws_dot_dir() {", "}");
    for row in DOT_ROWS {
        let tmp = Temp::new("shell-ws");
        assert!(build(&tmp.0, row.make));
        let root = tmp.0.to_str().expect("a UTF-8 temp path");
        let script = format!("{function}\n_ws_dot_dir \"$1\"");
        let got = common::bash_args(&script, &[root]);
        assert_eq!(Ok(PathBuf::from(got.trim_end_matches('\n'))), expect(row.shell, &tmp.0, &[]), "workspace row {:?}", row.case);
    }
}
