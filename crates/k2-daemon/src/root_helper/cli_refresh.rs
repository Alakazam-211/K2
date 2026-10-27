//! Keep a root-owned `/usr/local/bin/k2` at the daemon's version (0.46.3).
//!
//! The daemon stages the CLI itself at boot (`k2_core::cli_stage`), but
//! only where it may write: a `/usr/local/bin/k2` that root installed (an
//! older provision, a hand copy) froze at its first version — lztek ran a
//! 0.40.148 CLI against a 0.46.x daemon. After each auto-heal pass the
//! daemon asks the root helper (`helper cli-refresh`, protocol 9) to
//! replace that file with the CLI compiled into the signed helper. The
//! helper is at the daemon's version by then, so its CLI is the daemon's;
//! the daemon also checks the helper's `cliSha256` equals the hash of the
//! CLI it embeds itself before it asks.
//!
//! Idempotent: a current file is never touched (no root call at all), the
//! helper never downgrades, and a symlink, a non-K2 file, a copy the
//! daemon owns, a linked or writable `/usr/local/bin`, or a box whose
//! `daemon.toml` names another daemon as the system owner is left alone.
//! Reading the file never blocks (a FIFO at the path can't hang
//! `GET /cli/root-helper/status`, which agents may call, or auto-heal).

use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use serde_json::{json, Value};

use k2_root_proto::cli::{CLI_HEAD_BYTES, CLI_MAX_BYTES};
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
    /// `/usr/local/bin` is not plainly root's (a link, not root-owned,
    /// group/other-writable): the helper refuses it, so never ask.
    pub dir_problem: Option<String>,
}

/// The CLI this daemon embeds: what the helper's copy must equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonCli {
    pub version: String,
    pub sha256: String,
}

/// Side effects, so tests never run sudo or read the real `/usr/local`.
pub trait CliOps {
    fn inspect(&self) -> CliFile;
    fn probe(&self) -> RootHelperState;
    /// (user, unit, scope) of this daemon ([`super::own_unit`]).
    fn own_unit(&self) -> (String, String, String);
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
    /// by a user the daemon stages for itself, another daemon's).
    NotManaged { reason: String },
    /// Stale, and the helper can't refresh it yet.
    NeedsHelper { reason: String },
    Failed { reason: String },
}

/// The daemon.toml entry that owns `/usr/local/bin/k2` is the first
/// system-scope one (the helper enforces the same rule). `None` when this
/// daemon is it, or the helper lists no daemons (the helper decides).
fn other_owner(version_json: &Value, own: &(String, String, String)) -> Option<String> {
    let (user, unit, scope) = own;
    let daemons = version_json["daemons"].as_array().filter(|d| !d.is_empty())?;
    if scope != "system" {
        return Some("a user-scope daemon does not own /usr/local/bin/k2".into());
    }
    let first = daemons.iter().find(|d| d["scope"] == "system");
    let mine = first.is_some_and(|d| d["user"] == user.as_str() && d["unit"] == unit.as_str());
    (!mine).then(|| "another K2 daemon owns /usr/local/bin/k2 (the first system entry in daemon.toml)".into())
}

/// One decision + (at most) one root call.
pub fn refresh_with(ops: &dyn CliOps, want_cli: &DaemonCli) -> CliOutcome {
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
    let Some(want) = Semver::parse(&want_cli.version) else {
        return not_managed("the daemon's CLI version is not x.y.z");
    };
    if have >= want {
        return CliOutcome::Current { version: have.to_string() };
    }
    if !f.root_owned {
        return not_managed("not root-owned (the daemon stages its own copy at boot)");
    }
    if let Some(why) = &f.dir_problem {
        return not_managed(why);
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
    let vj = st.version_json.clone().unwrap_or(Value::Null);
    if let Some(why) = other_owner(&vj, &ops.own_unit()) {
        return not_managed(&why);
    }
    let carries = vj["cliVersion"].as_str().and_then(Semver::parse);
    if carries != Some(want) {
        return CliOutcome::NeedsHelper {
            reason: format!(
                "the root helper carries CLI {}, not the daemon's {want}; it refreshes once the \
                 helper is at the daemon's version",
                carries.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            ),
        };
    }
    // Same version is not enough: the bytes root would write must be the
    // CLI this daemon embeds (a dev build, a hand-patched helper).
    if vj["cliSha256"].as_str() != Some(want_cli.sha256.as_str()) {
        return CliOutcome::NeedsHelper {
            reason: format!(
                "the root helper's CLI {want} is not this daemon's build (cliSha256 {} ≠ {}); \
                 it refreshes once the helper and daemon come from one release",
                vj["cliSha256"].as_str().unwrap_or("?"),
                want_cli.sha256
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
pub fn run_once(ops: &dyn CliOps, want_cli: &DaemonCli, who: &str) -> CliOutcome {
    let out = refresh_with(ops, want_cli);
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
        "dirProblem": f.dir_problem,
        "daemonCliVersion": daemon_cli_version,
        "lastAttempt": last.at,
        "lastResult": last.outcome,
    })
}

pub fn status_value() -> Value {
    status_with(&LiveCliOps.inspect(), &daemon_cli().version)
}

/// The CLI this daemon embeds (= the daemon's version in a release) and
/// its sha256, computed once.
pub fn daemon_cli() -> DaemonCli {
    static CLI: OnceLock<DaemonCli> = OnceLock::new();
    CLI.get_or_init(|| {
        use sha2::Digest;
        let sha = sha2::Sha256::digest(k2_core::cli_stage::CLI_CONTENT.as_bytes());
        DaemonCli {
            version: k2_core::cli_stage::embedded_version().unwrap_or_default(),
            sha256: sha.iter().map(|b| format!("{b:02x}")).collect(),
        }
    })
    .clone()
}

/// What is wrong with the directory `dir` for a root write, if anything:
/// it (or its parent) is a link, it is not a directory, or (when
/// `real_owners`) it is not root-owned or is group/other-writable. The
/// helper refuses the same, so the daemon never spends a root call on it.
#[cfg(unix)]
fn dir_problem(dir: &std::path::Path, real_owners: bool) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    for d in [Some(dir), dir.parent()].into_iter().flatten() {
        match std::fs::symlink_metadata(d) {
            Ok(m) if m.file_type().is_symlink() => return Some("directory is a link".into()),
            Ok(m) if !m.is_dir() => return Some("directory is a link".into()),
            Ok(_) => {}
            Err(_) => return Some("directory unreadable".into()),
        }
    }
    let m = std::fs::symlink_metadata(dir).ok()?;
    (real_owners && (m.uid() != 0 || m.mode() & 0o022 != 0)).then(|| "directory not root-owned".into())
}

/// Read what is at `path` (never through a link, never blocking: the
/// open is `O_NOFOLLOW | O_NONBLOCK` and every fact after it comes from
/// `fstat` on that fd). `real_owners` false in tests (temp dirs are not
/// root's). Pub for tests.
pub fn inspect_path(path: &std::path::Path, real_owners: bool) -> CliFile {
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
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        f.root_owned = meta.uid() == 0;
        f.dir_problem = path.parent().and_then(|d| dir_problem(d, real_owners));
        if !f.regular {
            return f;
        }
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path);
        let Ok(file) = opened else {
            // A link swapped in (ELOOP) or gone: not what lstat said.
            return CliFile { regular: false, ..f };
        };
        let Ok(m) = file.metadata() else {
            return CliFile { regular: false, ..f };
        };
        // The fd's facts, not the lstat's: a FIFO or device swapped in is
        // never read.
        f.regular = m.is_file();
        f.root_owned = m.uid() == 0;
        if !f.regular || m.len() > CLI_MAX_BYTES {
            return f;
        }
        // The marker sits in the first lines; never read the whole 3 MB.
        let mut head = Vec::with_capacity(CLI_HEAD_BYTES);
        let _ = file.take(CLI_HEAD_BYTES as u64).read_to_end(&mut head);
        f.version = k2_core::cli_stage::k2_cli_version_of(&String::from_utf8_lossy(&head));
    }
    #[cfg(not(unix))]
    {
        let _ = real_owners;
        f.regular = false;
    }
    f
}

/// Production: the real file, the sudo door.
pub struct LiveCliOps;

impl CliOps for LiveCliOps {
    fn inspect(&self) -> CliFile {
        inspect_path(std::path::Path::new(CLI_PATH), true)
    }
    fn probe(&self) -> RootHelperState {
        super::state()
    }
    fn own_unit(&self) -> (String, String, String) {
        super::own_unit()
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
        own: (String, String, String),
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
        fn own_unit(&self) -> (String, String, String) {
            self.own.clone()
        }
        fn refresh(&self) -> Result<Value, String> {
            self.calls.set(self.calls.get() + 1);
            self.answer.borrow().clone()
        }
    }

    const SHA: &str = "5ca1ab1e";

    fn want() -> DaemonCli {
        DaemonCli { version: "0.46.3".into(), sha256: SHA.into() }
    }

    fn helper_with(protocol: u32, cli_ns: bool, cli_version: &str, extra: Value) -> RootHelperState {
        let mut vj = json!({
            "protocol": protocol,
            "namespaces": {"cli": cli_ns},
            "cliVersion": cli_version,
            "cliSha256": SHA,
            "daemons": [{"user": "k2", "unit": "k2-daemon.service", "scope": "system"}],
        });
        if let (Some(o), Some(x)) = (vj.as_object_mut(), extra.as_object()) {
            for (k, v) in x {
                o.insert(k.clone(), v.clone());
            }
        }
        RootHelperState {
            kind: Kind::Root,
            version: Some(cli_version.into()),
            protocol,
            path: "/usr/local/libexec/k2-root-helper".into(),
            package_owned: false,
            version_json: Some(vj),
            error: None,
        }
    }

    fn helper(protocol: u32, cli_ns: bool, cli_version: &str) -> RootHelperState {
        helper_with(protocol, cli_ns, cli_version, json!({}))
    }

    fn stale_root_file(v: &str) -> CliFile {
        CliFile {
            present: true,
            symlink: false,
            regular: true,
            root_owned: true,
            version: Some(v.into()),
            dir_problem: None,
        }
    }

    fn own() -> (String, String, String) {
        ("k2".into(), "k2-daemon.service".into(), "system".into())
    }

    fn fake(file: CliFile, state: RootHelperState, answer: Result<Value, String>) -> Fake {
        Fake { file, state, own: own(), answer: RefCell::new(answer), calls: Cell::new(0) }
    }

    #[test]
    fn a_stale_root_owned_cli_is_refreshed_through_the_helper() {
        let f = fake(
            stale_root_file("0.40.148"),
            helper(9, true, "0.46.3"),
            Ok(json!({"result": "updated", "from": "0.40.148", "to": "0.46.3"})),
        );
        assert_eq!(
            refresh_with(&f, &want()),
            CliOutcome::Refreshed { from: "0.40.148".into(), to: "0.46.3".into() }
        );
        assert_eq!(f.calls.get(), 1);
    }

    #[test]
    fn a_current_cli_never_costs_a_root_call() {
        for v in ["0.46.3", "0.47.0"] {
            let f = fake(stale_root_file(v), helper(9, true, "0.46.3"), Err("must not run".into()));
            assert_eq!(refresh_with(&f, &want()), CliOutcome::Current { version: v.into() });
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
            (CliFile { version: Some("0.46.2-rc1".into()), ..stale_root_file("x") }, "not the K2 CLI"),
            (CliFile { root_owned: false, ..stale_root_file("0.40.148") }, "not root-owned"),
            (
                CliFile { dir_problem: Some("directory is a link".into()), ..stale_root_file("0.40.148") },
                "directory is a link",
            ),
            (
                CliFile { dir_problem: Some("directory not root-owned".into()), ..stale_root_file("0.40.148") },
                "directory not root-owned",
            ),
        ];
        for (file, want_reason) in cases {
            let f = fake(file, helper(9, true, "0.46.3"), Err("must not run".into()));
            match refresh_with(&f, &want()) {
                CliOutcome::NotManaged { reason } => assert!(reason.starts_with(want_reason), "{reason}"),
                other => panic!("{want_reason}: {other:?}"),
            }
            assert_eq!(f.calls.get(), 0, "{want_reason}");
        }
    }

    #[test]
    fn an_older_helper_or_one_off_the_daemons_build_does_not_refresh() {
        for (st, want_reason) in [
            (helper(8, false, "0.46.2"), "protocol 9"),
            (helper(9, false, "0.46.3"), "protocol 9"),
            (RootHelperState::missing(), "protocol 9"),
            (helper(9, true, "0.46.2"), "carries CLI 0.46.2"),
            // Same version, different bytes (a dev build, a patched helper).
            (helper_with(9, true, "0.46.3", json!({"cliSha256": "deadbeef"})), "cliSha256 deadbeef"),
            // A protocol-9 helper that does not say its CLI's hash.
            (helper_with(9, true, "0.46.3", json!({"cliSha256": null})), "cliSha256 ?"),
        ] {
            let f = fake(stale_root_file("0.40.148"), st, Err("must not run".into()));
            match refresh_with(&f, &want()) {
                CliOutcome::NeedsHelper { reason } => assert!(reason.contains(want_reason), "{reason}"),
                other => panic!("{want_reason}: {other:?}"),
            }
            assert_eq!(f.calls.get(), 0);
        }
    }

    #[test]
    fn only_the_first_system_daemon_in_daemon_toml_asks() {
        let two = json!({"daemons": [
            {"user": "k2", "unit": "k2-daemon.service", "scope": "system"},
            {"user": "k2b", "unit": "k2b-daemon.service", "scope": "system"},
            {"user": "k2u", "unit": "k2-daemon.service", "scope": "user"},
        ]});
        // The owner asks.
        let f = fake(
            stale_root_file("0.40.148"),
            helper_with(9, true, "0.46.3", two.clone()),
            Ok(json!({"result": "updated", "from": "0.40.148", "to": "0.46.3"})),
        );
        assert!(matches!(refresh_with(&f, &want()), CliOutcome::Refreshed { .. }));
        // A second system daemon and a user-scope one never do.
        for (who, want_reason) in [
            (("k2b".into(), "k2b-daemon.service".into(), "system".into()), "another K2 daemon owns"),
            (("k2u".into(), "k2-daemon.service".into(), "user".into()), "a user-scope daemon"),
        ] {
            let mut f = fake(stale_root_file("0.40.148"), helper_with(9, true, "0.46.3", two.clone()), Err("must not run".into()));
            f.own = who;
            match refresh_with(&f, &want()) {
                CliOutcome::NotManaged { reason } => assert!(reason.starts_with(want_reason), "{reason}"),
                other => panic!("{want_reason}: {other:?}"),
            }
            assert_eq!(f.calls.get(), 0);
        }
    }

    #[test]
    fn helper_answers_map_to_outcomes() {
        let run = |answer| refresh_with(&fake(stale_root_file("0.40.148"), helper(9, true, "0.46.3"), answer), &want());
        assert_eq!(
            run(Ok(json!({"result": "unchanged", "version": "0.46.3"}))),
            CliOutcome::Current { version: "0.46.3".into() }
        );
        assert_eq!(
            run(Ok(json!({"result": "skipped", "reason": "symlink"}))),
            CliOutcome::NotManaged { reason: "symlink".into() }
        );
        // The helper's own refusals (a linked or writable /usr/local/bin,
        // another daemon's file) are "not managed", never "failed".
        assert_eq!(
            run(Ok(json!({"result": "skipped", "reason": "directory is a link"}))),
            CliOutcome::NotManaged { reason: "directory is a link".into() }
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
    fn the_daemon_cli_is_the_embedded_one() {
        use sha2::Digest;
        let d = daemon_cli();
        assert_eq!(d.version, env!("CARGO_PKG_VERSION"), "cli/k2 K2_CLI_VERSION = the daemon's version");
        let sha = sha2::Sha256::digest(include_bytes!("../../../../cli/k2"));
        assert_eq!(d.sha256, sha.iter().map(|b| format!("{b:02x}")).collect::<String>());
    }

    /// The helper decides with `k2_root_proto::cli`, the daemon (and boot
    /// staging) with `k2_core::cli_stage`: they read every case alike.
    #[test]
    fn the_helper_and_core_parsers_agree() {
        use k2_root_proto::cli::{cli_version_of, marker_after_line, VERSION_CASES};
        let mut cases: Vec<String> = VERSION_CASES.iter().map(|(t, _)| t.to_string()).collect();
        cases.push(marker_after_line(29));
        cases.push(marker_after_line(30));
        cases.push(k2_core::cli_stage::CLI_CONTENT[..CLI_HEAD_BYTES].to_string());
        for text in &cases {
            assert_eq!(
                cli_version_of(text.as_bytes()),
                k2_core::cli_stage::k2_cli_version_of(text),
                "{:?}",
                &text[..text.len().min(80)]
            );
        }
        for (text, want) in VERSION_CASES {
            assert_eq!(k2_core::cli_stage::k2_cli_version_of(text).as_deref(), *want, "{text:?}");
        }
    }

    struct Rm(std::path::PathBuf);
    impl Drop for Rm {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp() -> Rm {
        let base = std::env::temp_dir().join(format!("k2-cli-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("bin")).unwrap();
        Rm(base)
    }

    // Server-only feature (Linux boxes): the root helper's /usr/local/bin/k2
    // refresh; not shipped on personal Windows machines.
    #[cfg(unix)]
    #[test]
    fn inspect_reads_the_marker_and_never_follows_a_link() {
        let dir = tmp();
        let p = dir.0.join("bin/k2");
        std::fs::write(&p, "#!/bin/bash\n# k2\nK2_CLI_VERSION=\"0.40.148\"\n").unwrap();
        let f = inspect_path(&p, false);
        assert!(f.present && f.regular && !f.symlink);
        assert_eq!(f.version.as_deref(), Some("0.40.148"));
        assert_eq!(f.dir_problem, None);
        let l = dir.0.join("bin/link");
        std::os::unix::fs::symlink(&p, &l).unwrap();
        let f = inspect_path(&l, false);
        assert!(f.present && f.symlink && f.version.is_none());
        assert!(!inspect_path(&dir.0.join("bin/none"), false).present);
        // Over 16 MB: not read, not the K2 CLI.
        let big = std::fs::File::create(dir.0.join("bin/big")).unwrap();
        big.set_len(CLI_MAX_BYTES + 1).unwrap();
        drop(big);
        let mut head = std::fs::OpenOptions::new().write(true).open(dir.0.join("bin/big")).unwrap();
        std::io::Write::write_all(&mut head, b"#!/bin/bash\nK2_CLI_VERSION=\"0.40.148\"\n").unwrap();
        let f = inspect_path(&dir.0.join("bin/big"), false);
        assert!(f.regular && f.version.is_none(), "{f:?}");
    }

    // Server-only feature (Linux boxes): the root helper's /usr/local/bin/k2
    // refresh; not shipped on personal Windows machines.
    #[cfg(unix)]
    #[test]
    fn inspect_names_a_linked_or_writable_directory() {
        let dir = tmp();
        std::fs::write(dir.0.join("bin/k2"), "#!/bin/bash\nK2_CLI_VERSION=\"0.40.148\"\n").unwrap();
        // ostree-style: /usr/local/bin is a link to the real dir.
        std::os::unix::fs::symlink(dir.0.join("bin"), dir.0.join("linked")).unwrap();
        let f = inspect_path(&dir.0.join("linked/k2"), false);
        assert_eq!(f.dir_problem.as_deref(), Some("directory is a link"));
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let bin = dir.0.join("bin");
        let chmod = |mode: u32| std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(mode)).expect("chmod bin");
        // SAFETY: geteuid has no preconditions.
        let root = unsafe { libc::geteuid() } == 0;
        // Not root's: with real owners on, it is named. A non-root run owns
        // the temp dir already; a root run (the soak box) hands it to
        // nobody, so the uid rule is what answers either way.
        chmod(0o755);
        if root {
            std::os::unix::fs::chown(&bin, Some(65534), Some(65534)).expect("chown bin to nobody");
        }
        assert_ne!(std::fs::metadata(&bin).expect("stat bin").uid(), 0, "the fixture is not root's");
        let f = inspect_path(&dir.0.join("bin/k2"), true);
        assert_eq!(f.dir_problem.as_deref(), Some("directory not root-owned"));
        if root {
            // Root's and 0755: nothing to name. Group- or world-writable
            // root's dir: named all the same.
            std::os::unix::fs::chown(&bin, Some(0), Some(0)).expect("chown bin back to root");
            assert_eq!(inspect_path(&dir.0.join("bin/k2"), true).dir_problem, None, "a root-owned 0755 dir is fine");
            for mode in [0o775, 0o757] {
                chmod(mode);
                let f = inspect_path(&dir.0.join("bin/k2"), true);
                assert_eq!(f.dir_problem.as_deref(), Some("directory not root-owned"), "mode {mode:o}");
            }
        }
    }

    /// A FIFO at the path: `GET /cli/root-helper/status` (open to agents)
    /// and auto-heal must answer at once, not wait for a writer.
    // Server-only feature (Linux boxes): the root helper's /usr/local/bin/k2
    // refresh; not shipped on personal Windows machines.
    #[cfg(unix)]
    #[test]
    fn inspect_never_blocks_on_a_fifo() {
        let dir = tmp();
        let p = dir.0.join("bin/k2");
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo");
        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = p.clone();
        std::thread::spawn(move || {
            let _ = tx.send(inspect_path(&p2, false));
        });
        let f = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("inspect_path hung on a FIFO");
        assert!(f.present && !f.regular && f.version.is_none(), "{f:?}");
        let v = status_with(&f, "0.46.3");
        assert_eq!(v["state"], "not_k2");
    }
}
