//! Keep a root-owned `/usr/local/bin/k2` at the daemon's version (0.46.3).
//!
//! The daemon stages the CLI itself at boot (`k2_core::cli_stage`), but
//! only where it may write: a `/usr/local/bin/k2` that root installed (an
//! older provision, a hand copy) froze at its first version — lztek ran a
//! 0.40.148 CLI against a 0.46.x daemon. After each auto-heal pass the
//! daemon asks the root helper (`helper cli-refresh`, protocol 9) to
//! replace that file with the CLI compiled into the signed helper. The
//! helper is at the daemon's version by then, so its CLI is the daemon's.
//!
//! Idempotent: a current file is never touched (no root call at all), the
//! helper never downgrades, and a symlink, a non-K2 file or a copy the
//! daemon owns is left alone.

use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};

use k2_root_proto::paths::CLI_PATH;
use k2_root_proto::version::{Semver, CLI_REFRESH_MIN_PROTOCOL};

use super::RootHelperState;

/// What is at `/usr/local/bin/k2`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliFile {
    pub present: bool,
    pub symlink: bool,
    pub regular: bool,
    pub root_owned: bool,
    /// `K2_CLI_VERSION` when the file is the K2 CLI.
    pub version: Option<String>,
}

/// Side effects, so tests never run sudo or read the real `/usr/local`.
pub trait CliOps {
    fn inspect(&self) -> CliFile;
    fn probe(&self) -> RootHelperState;
    fn refresh(&self) -> Result<Value, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CliOutcome {
    /// At (or above) the daemon's version; nothing to do.
    Current { version: String },
    /// Replaced through the root helper.
    Refreshed { from: String, to: String },
    /// Not K2's to refresh here (absent, a symlink, not the K2 CLI, owned
    /// by a user the daemon stages for itself).
    NotManaged { reason: String },
    /// Stale, and the helper can't refresh it yet.
    NeedsHelper { reason: String },
    Failed { reason: String },
}

/// One decision + (at most) one root call.
pub fn refresh_with(ops: &dyn CliOps, daemon_cli_version: &str) -> CliOutcome {
    let f = ops.inspect();
    let not_managed = |r: &str| CliOutcome::NotManaged { reason: r.to_string() };
    if !f.present {
        return not_managed("absent");
    }
    if f.symlink {
        return not_managed("a symlink (the app or a package owns it)");
    }
    if !f.regular {
        return not_managed("not a regular file");
    }
    let Some(have) = f.version.as_deref().and_then(Semver::parse) else {
        return not_managed("not the K2 CLI");
    };
    let Some(want) = Semver::parse(daemon_cli_version) else {
        return not_managed("the daemon's CLI version is not x.y.z");
    };
    if have >= want {
        return CliOutcome::Current { version: have.to_string() };
    }
    if !f.root_owned {
        return not_managed("not root-owned (the daemon stages its own copy at boot)");
    }
    let st = ops.probe();
    let serves = st.usable(CLI_REFRESH_MIN_PROTOCOL)
        && st
            .version_json
            .as_ref()
            .and_then(|v| v.pointer("/namespaces/cli"))
            .and_then(Value::as_bool)
            == Some(true);
    if !serves {
        return CliOutcome::NeedsHelper {
            reason: format!(
                "the root helper must speak protocol {CLI_REFRESH_MIN_PROTOCOL} to refresh it \
                 (k2 root-helper update)"
            ),
        };
    }
    let carries = st
        .version_json
        .as_ref()
        .and_then(|v| v["cliVersion"].as_str())
        .and_then(Semver::parse);
    if carries != Some(want) {
        return CliOutcome::NeedsHelper {
            reason: format!(
                "the root helper carries CLI {}, not the daemon's {want}; it refreshes once the \
                 helper is at the daemon's version",
                carries.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            ),
        };
    }
    match ops.refresh() {
        Err(e) => CliOutcome::Failed { reason: e },
        Ok(v) => match v["result"].as_str() {
            Some("updated") => CliOutcome::Refreshed {
                from: v["from"].as_str().unwrap_or(&have.to_string()).to_string(),
                to: v["to"].as_str().unwrap_or(&want.to_string()).to_string(),
            },
            Some("unchanged") => CliOutcome::Current {
                version: v["version"].as_str().unwrap_or_default().to_string(),
            },
            Some("skipped") => not_managed(v["reason"].as_str().unwrap_or("skipped by the helper")),
            _ => CliOutcome::Failed { reason: format!("unexpected answer: {v}") },
        },
    }
}

#[derive(Debug, Default, Clone)]
struct Last {
    at: Option<i64>,
    outcome: Option<CliOutcome>,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

/// One pass; records the result for status and audits a refresh.
pub fn run_once(ops: &dyn CliOps, daemon_cli_version: &str, who: &str) -> CliOutcome {
    let out = refresh_with(ops, daemon_cli_version);
    let audit = |outcome: String| {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "root_helper_cli_refresh",
            who,
            outcome,
            "daemon",
            "-",
            "cli",
        ))
    };
    match &out {
        CliOutcome::Refreshed { from, to } => audit(format!("ok {from} -> {to}")),
        CliOutcome::Failed { reason } => audit(format!("failed: {reason}")),
        _ => {}
    }
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some(Last {
        at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        ),
        outcome: Some(out.clone()),
    });
    out
}

/// The doctor line's facts: what is on disk now vs the daemon's CLI, and
/// the last refresh pass.
pub fn status_with(f: &CliFile, daemon_cli_version: &str) -> Value {
    let want = Semver::parse(daemon_cli_version);
    let have = f.version.as_deref().and_then(Semver::parse);
    let state = if !f.present {
        "absent"
    } else if f.symlink {
        "symlink"
    } else if have.is_none() {
        "not_k2"
    } else if want.is_some() && have < want {
        "stale"
    } else {
        "current"
    };
    let last = LAST.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_default();
    json!({
        "path": CLI_PATH,
        "state": state,
        "version": f.version,
        "rootOwned": f.root_owned,
        "daemonCliVersion": daemon_cli_version,
        "lastAttempt": last.at,
        "lastResult": last.outcome,
    })
}

pub fn status_value() -> Value {
    status_with(&LiveCliOps.inspect(), &daemon_cli_version())
}

/// The CLI version this daemon embeds (= the daemon's in a release).
pub fn daemon_cli_version() -> String {
    k2_core::cli_stage::embedded_version().unwrap_or_default()
}

/// Read what is at `path` (never through a link). Pub for tests.
pub fn inspect_path(path: &std::path::Path) -> CliFile {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return CliFile::default();
    };
    let mut f = CliFile {
        present: true,
        symlink: meta.file_type().is_symlink(),
        regular: meta.file_type().is_file(),
        ..CliFile::default()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        f.root_owned = meta.uid() == 0;
    }
    if f.regular {
        // The marker sits in the first lines; never read the whole 3 MB.
        use std::io::Read;
        let mut head = Vec::with_capacity(16 * 1024);
        if let Ok(file) = std::fs::File::open(path) {
            let _ = file.take(16 * 1024).read_to_end(&mut head);
        }
        f.version = k2_core::cli_stage::k2_cli_version_of(&String::from_utf8_lossy(&head));
    }
    f
}

/// Production: the real file, the sudo door.
pub struct LiveCliOps;

impl CliOps for LiveCliOps {
    fn inspect(&self) -> CliFile {
        inspect_path(std::path::Path::new(CLI_PATH))
    }
    fn probe(&self) -> RootHelperState {
        super::state()
    }
    fn refresh(&self) -> Result<Value, String> {
        let out = super::call_text(&["helper".into(), "cli-refresh".into()], None)?;
        serde_json::from_str(&out).map_err(|_| "cli-refresh answer is not JSON".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root_helper::Kind;
    use std::cell::{Cell, RefCell};

    struct Fake {
        file: CliFile,
        state: RootHelperState,
        answer: RefCell<Result<Value, String>>,
        calls: Cell<u32>,
    }

    impl CliOps for Fake {
        fn inspect(&self) -> CliFile {
            self.file.clone()
        }
        fn probe(&self) -> RootHelperState {
            self.state.clone()
        }
        fn refresh(&self) -> Result<Value, String> {
            self.calls.set(self.calls.get() + 1);
            self.answer.borrow().clone()
        }
    }

    fn helper(protocol: u32, cli_ns: bool, cli_version: &str) -> RootHelperState {
        RootHelperState {
            kind: Kind::Root,
            version: Some(cli_version.into()),
            protocol,
            path: "/usr/local/libexec/k2-root-helper".into(),
            package_owned: false,
            version_json: Some(json!({
                "protocol": protocol,
                "namespaces": {"cli": cli_ns},
                "cliVersion": cli_version,
            })),
            error: None,
        }
    }

    fn stale_root_file(v: &str) -> CliFile {
        CliFile {
            present: true,
            symlink: false,
            regular: true,
            root_owned: true,
            version: Some(v.into()),
        }
    }

    fn fake(file: CliFile, state: RootHelperState, answer: Result<Value, String>) -> Fake {
        Fake { file, state, answer: RefCell::new(answer), calls: Cell::new(0) }
    }

    #[test]
    fn a_stale_root_owned_cli_is_refreshed_through_the_helper() {
        let f = fake(
            stale_root_file("0.40.148"),
            helper(9, true, "0.46.3"),
            Ok(json!({"result": "updated", "from": "0.40.148", "to": "0.46.3"})),
        );
        assert_eq!(
            refresh_with(&f, "0.46.3"),
            CliOutcome::Refreshed { from: "0.40.148".into(), to: "0.46.3".into() }
        );
        assert_eq!(f.calls.get(), 1);
    }

    #[test]
    fn a_current_cli_never_costs_a_root_call() {
        for v in ["0.46.3", "0.47.0"] {
            let f = fake(stale_root_file(v), helper(9, true, "0.46.3"), Err("must not run".into()));
            assert_eq!(refresh_with(&f, "0.46.3"), CliOutcome::Current { version: v.into() });
            assert_eq!(f.calls.get(), 0, "{v}");
        }
    }

    #[test]
    fn not_k2s_to_refresh_is_left_alone_without_a_root_call() {
        let cases = [
            (CliFile::default(), "absent"),
            (CliFile { present: true, symlink: true, ..CliFile::default() }, "a symlink"),
            (CliFile { present: true, regular: false, ..CliFile::default() }, "not a regular file"),
            (CliFile { version: None, ..stale_root_file("x") }, "not the K2 CLI"),
            (CliFile { root_owned: false, ..stale_root_file("0.40.148") }, "not root-owned"),
        ];
        for (file, want) in cases {
            let f = fake(file, helper(9, true, "0.46.3"), Err("must not run".into()));
            match refresh_with(&f, "0.46.3") {
                CliOutcome::NotManaged { reason } => assert!(reason.starts_with(want), "{reason}"),
                other => panic!("{want}: {other:?}"),
            }
            assert_eq!(f.calls.get(), 0, "{want}");
        }
    }

    #[test]
    fn an_older_helper_or_one_off_the_daemons_version_does_not_refresh() {
        for (st, want) in [
            (helper(8, false, "0.46.2"), "protocol 9"),
            (helper(9, false, "0.46.3"), "protocol 9"),
            (RootHelperState::missing(), "protocol 9"),
            (helper(9, true, "0.46.2"), "carries CLI 0.46.2"),
        ] {
            let f = fake(stale_root_file("0.40.148"), st, Err("must not run".into()));
            match refresh_with(&f, "0.46.3") {
                CliOutcome::NeedsHelper { reason } => assert!(reason.contains(want), "{reason}"),
                other => panic!("{want}: {other:?}"),
            }
            assert_eq!(f.calls.get(), 0);
        }
    }

    #[test]
    fn helper_answers_map_to_outcomes() {
        let run = |answer| refresh_with(&fake(stale_root_file("0.40.148"), helper(9, true, "0.46.3"), answer), "0.46.3");
        assert_eq!(
            run(Ok(json!({"result": "unchanged", "version": "0.46.3"}))),
            CliOutcome::Current { version: "0.46.3".into() }
        );
        assert_eq!(
            run(Ok(json!({"result": "skipped", "reason": "symlink"}))),
            CliOutcome::NotManaged { reason: "symlink".into() }
        );
        assert_eq!(run(Err("exit 1".into())), CliOutcome::Failed { reason: "exit 1".into() });
    }

    #[test]
    fn doctor_status_names_a_stale_root_owned_cli() {
        let v = status_with(&stale_root_file("0.40.148"), "0.46.3");
        assert_eq!(v["state"], "stale");
        assert_eq!(v["rootOwned"], true);
        assert_eq!(v["version"], "0.40.148");
        assert_eq!(v["daemonCliVersion"], "0.46.3");
        assert_eq!(status_with(&stale_root_file("0.46.3"), "0.46.3")["state"], "current");
        assert_eq!(status_with(&CliFile::default(), "0.46.3")["state"], "absent");
    }

    #[test]
    fn inspect_reads_the_marker_and_never_follows_a_link() {
        let base = std::env::temp_dir().join(format!("k2-cli-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        struct Rm(std::path::PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Rm(base);
        let dir = &dir;
        let p = dir.0.join("k2");
        std::fs::write(&p, "#!/bin/bash\n# k2\nK2_CLI_VERSION=\"0.40.148\"\n").unwrap();
        let f = inspect_path(&p);
        assert!(f.present && f.regular && !f.symlink);
        assert_eq!(f.version.as_deref(), Some("0.40.148"));
        let l = dir.0.join("link");
        std::os::unix::fs::symlink(&p, &l).unwrap();
        let f = inspect_path(&l);
        assert!(f.present && f.symlink && f.version.is_none());
        assert!(!inspect_path(&dir.0.join("none")).present);
    }
}
