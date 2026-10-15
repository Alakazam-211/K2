//! The OCI runtime shim: how a rootless App container gets its MAC
//! profile (`prd-k2-app-runtime-v1` §4.2 item 4).
//!
//! Rootless Podman refuses `--security-opt apparmor=…` (its AppArmor
//! support is root-only), but crun and youki apply `process.apparmorProfile`
//! from the OCI config themselves, unprivileged: a task may always move
//! from unconfined into a profile, even under no-new-privileges. So the
//! engine points Podman at a shim instead of the runtime
//! (`--runtime=<engine state>/oci/crun`, a link to this binary). Called as
//! `crun` or `youki`, the binary:
//! - on `create` / `run`: sets `process.apparmorProfile` in the bundle's
//!   `config.json`;
//! - on `exec --process <file>` (`podman exec`, `k2 app exec|shell`): sets
//!   `apparmorProfile` and `noNewPrivileges` in that process file, so an
//!   exec never runs outside the App's wall;
//! - then execs the real runtime with the same arguments.
//!
//! Only when box prep's marker (`/etc/k2app/apparmor`) is there and the
//! kernel says AppArmor is on; otherwise it passes straight through (SELinux
//! boxes: Podman's own `container_t` labels). A config it can't read or
//! write is a refusal, never an unconfined start.

use std::path::{Path, PathBuf};

use k2_root_proto::apps::{OciRuntime, APPARMOR_ENABLED, APPARMOR_PROFILE, MAC_MARKER};

/// `<engine state>/oci/<name>` → `/usr/local/libexec/k2-app-engine`.
pub const SHIM_DIR: &str = "oci";

/// Runtime options that take their value as the next argument (crun's
/// and youki's globals, and the subcommand options before the ones K2
/// reads).
const VALUE_FLAGS: &[&str] = &[
    "--root",
    "--log",
    "--log-format",
    "--log-level",
    "--cgroup-manager",
    "--pid-file",
    "--console-socket",
    "--preserve-fds",
    "--cwd",
    "--env",
    "-e",
    "--user",
    "-u",
    "--cap",
    "-c",
    "--apparmor",
    "--process-label",
    "--cgroup",
];

/// What a runtime call carries an OCI process in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `create` / `run`: `<bundle>/config.json`.
    Config(PathBuf),
    /// `exec --process <file>`.
    Process(PathBuf),
    /// Anything else (`start`, `kill`, `delete`, `state`, `--version`, …).
    None,
}

fn value_of<'a>(args: &'a [String], i: usize, long: &str, short: Option<&str>) -> Option<(&'a str, usize)> {
    let a = args[i].as_str();
    if let Some(v) = a.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
        return Some((v, 1));
    }
    if a == long || short.is_some_and(|s| a == s) {
        return args.get(i + 1).map(|v| (v.as_str(), 2));
    }
    None
}

/// Find the subcommand and the file the shim must patch. `cwd` is the
/// default bundle (the runtime's own default is the current directory).
pub fn target(args: &[String], cwd: &Path) -> Target {
    let mut i = 0;
    let mut sub: Option<(usize, &str)> = None;
    while i < args.len() {
        let a = args[i].as_str();
        if a.starts_with('-') {
            i += if VALUE_FLAGS.contains(&a) { 2 } else { 1 };
            continue;
        }
        sub = Some((i, a));
        break;
    }
    let Some((at, sub)) = sub else { return Target::None };
    // The subcommand's options run up to its first positional (the
    // container id); what follows an exec's id is the command, never ours.
    let rest = &args[at + 1..];
    let (long, short) = match sub {
        "create" | "run" => ("--bundle", "-b"),
        "exec" => ("--process", "-p"),
        _ => return Target::None,
    };
    let mut found: Option<&str> = None;
    let mut j = 0;
    while j < rest.len() {
        if let Some((v, step)) = value_of(rest, j, long, Some(short)) {
            found = Some(v);
            j += step;
            continue;
        }
        let a = rest[j].as_str();
        if !a.starts_with('-') {
            break;
        }
        j += if VALUE_FLAGS.contains(&a) { 2 } else { 1 };
    }
    let abs = |p: &str| {
        let p = PathBuf::from(p);
        if p.is_absolute() {
            p
        } else {
            cwd.join(p)
        }
    };
    match (sub, found) {
        ("exec", Some(p)) => Target::Process(abs(p)),
        // `exec <id> <cmd…>` takes the container's own process settings
        // (already patched at create).
        ("exec", None) => Target::None,
        (_, Some(b)) => Target::Config(abs(b).join("config.json")),
        (_, None) => Target::Config(cwd.join("config.json")),
    }
}

/// Put `profile` into an OCI config (`process.apparmorProfile`) or an exec
/// process (`apparmorProfile`, plus `noNewPrivileges`). Refuses a JSON
/// shape that isn't an OCI config or process.
pub fn patch(json: &[u8], is_process: bool, profile: &str) -> Result<Vec<u8>, String> {
    let mut v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("not JSON: {e}"))?;
    let proc_obj = if is_process {
        v.as_object_mut().ok_or("the exec process is not a JSON object")?
    } else {
        v.get_mut("process")
            .and_then(|p| p.as_object_mut())
            .ok_or("the OCI config has no process object")?
    };
    proc_obj.insert("apparmorProfile".into(), serde_json::Value::String(profile.into()));
    if is_process {
        proc_obj.insert("noNewPrivileges".into(), serde_json::Value::Bool(true));
    }
    serde_json::to_vec(&v).map_err(|e| e.to_string())
}

/// Should this call put the App under the profile? Marker present (box
/// prep loaded the profile) and AppArmor on.
pub fn wanted(marker: Option<&str>, apparmor_enabled: Option<&str>) -> bool {
    marker == Some(k2_root_proto::apps::mac_marker()) && apparmor_enabled.map(str::trim) == Some("Y")
}

/// Largest OCI file the shim rewrites.
const MAX_JSON: u64 = 16 << 20;

fn rewrite(path: &Path, is_process: bool) -> Result<(), String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = Vec::new();
    f.take(MAX_JSON + 1).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    if buf.len() as u64 > MAX_JSON {
        return Err(format!("{} is too large", path.display()));
    }
    let out = patch(&buf, is_process, APPARMOR_PROFILE)?;
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600)
    };
    crate::sys::write_atomic(path, &out, mode)
}

/// The shim's entry: patch (when wanted), then exec the real runtime.
/// Returns only on failure (the exit code).
pub fn main(rt: OciRuntime, args: &[String]) -> i32 {
    let marker = std::fs::read_to_string(MAC_MARKER).ok();
    let enabled = std::fs::read_to_string(APPARMOR_ENABLED).ok();
    if wanted(marker.as_deref(), enabled.as_deref()) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let res = match target(args, &cwd) {
            Target::Config(p) => rewrite(&p, false),
            Target::Process(p) => rewrite(&p, true),
            Target::None => Ok(()),
        };
        if let Err(e) = res {
            eprintln!("k2-app-engine (OCI shim): refusing to run an App without its AppArmor profile: {e}");
            return 125;
        }
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(rt.binary()).arg0(rt.binary()).args(args).exec();
    eprintln!("k2-app-engine (OCI shim): exec {}: {err}", rt.binary());
    127
}

/// Called as `crun` / `youki` (a link in the engine's `oci/` dir)?
pub fn runtime_for_argv0(argv0: &str) -> Option<OciRuntime> {
    let base = Path::new(argv0).file_name()?.to_str()?;
    OciRuntime::parse(base)
}

/// The shim path Podman gets for `rt`.
pub fn shim_path(state: &Path, rt: &str) -> PathBuf {
    state.join(SHIM_DIR).join(rt)
}

/// Make `<state>/oci/{crun,youki}` links to the engine binary (replacing
/// anything else there).
pub fn install_links(state: &Path, engine: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = state.join(SHIM_DIR);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    for rt in [OciRuntime::Crun, OciRuntime::Youki] {
        let link = dir.join(rt.as_str());
        if std::fs::read_link(&link).ok().as_deref() == Some(engine) {
            continue;
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(engine, &link).map_err(|e| format!("link {}: {e}", link.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_the_file_in_what_conmon_and_podman_pass() {
        let cwd = Path::new("/run/b");
        // conmon's create (crun globals first).
        let a = sv(&[
            "--root", "/run/user/996/crun", "--systemd-cgroup", "--log-format=json", "--log", "/x/log",
            "create", "--bundle", "/var/lib/k2app/.local/share/containers/storage/overlay-containers/abc/userdata",
            "--pid-file", "/x/pidfile", "abc",
        ]);
        assert_eq!(
            target(&a, cwd),
            Target::Config(PathBuf::from("/var/lib/k2app/.local/share/containers/storage/overlay-containers/abc/userdata/config.json"))
        );
        assert_eq!(target(&sv(&["create", "-b", "/b", "id"]), cwd), Target::Config(PathBuf::from("/b/config.json")));
        assert_eq!(target(&sv(&["run", "--bundle=/b2", "id"]), cwd), Target::Config(PathBuf::from("/b2/config.json")));
        assert_eq!(target(&sv(&["create", "id"]), cwd), Target::Config(PathBuf::from("/run/b/config.json")), "default bundle is the cwd");
        assert_eq!(target(&sv(&["create", "--bundle", "rel", "id"]), cwd), Target::Config(PathBuf::from("/run/b/rel/config.json")));
        // podman exec through conmon.
        let a = sv(&["--root", "/run/user/996/crun", "exec", "--pid-file", "/x/p", "--process", "/x/exec-process-123", "--detach", "--tty", "abc"]);
        assert_eq!(target(&a, cwd), Target::Process(PathBuf::from("/x/exec-process-123")));
        assert_eq!(target(&sv(&["exec", "-p", "/p.json", "abc"]), cwd), Target::Process(PathBuf::from("/p.json")));
        assert_eq!(target(&sv(&["exec", "--process=/p.json", "abc"]), cwd), Target::Process(PathBuf::from("/p.json")));
        // A global value that looks like a subcommand is skipped.
        assert_eq!(target(&sv(&["--root", "create", "exec", "-p", "/p", "id"]), cwd), Target::Process(PathBuf::from("/p")));
        // The exec'd command's own flags are never read as ours.
        assert_eq!(target(&sv(&["exec", "abc", "mkdir", "-p", "/x"]), cwd), Target::None);
        assert_eq!(target(&sv(&["create", "abc", "--bundle", "/evil"]), cwd), Target::Config(PathBuf::from("/run/b/config.json")));
        for none in [&["start", "abc"][..], &["kill", "abc", "TERM"], &["delete", "--force", "abc"], &["--version"], &["features"], &["exec", "abc", "sh"], &[]] {
            assert_eq!(target(&sv(none), cwd), Target::None, "{none:?}");
        }
    }

    #[test]
    fn patches_config_and_exec_process() {
        let cfg = br#"{"ociVersion":"1.0.0","process":{"args":["node"],"noNewPrivileges":true},"root":{"path":"rootfs"}}"#;
        let out: serde_json::Value = serde_json::from_slice(&patch(cfg, false, "k2-app").unwrap()).unwrap();
        assert_eq!(out["process"]["apparmorProfile"], "k2-app");
        assert_eq!(out["process"]["args"][0], "node");
        assert_eq!(out["root"]["path"], "rootfs");
        // An existing (other) profile is replaced, never kept.
        let cfg = br#"{"process":{"apparmorProfile":"unconfined"}}"#;
        let out: serde_json::Value = serde_json::from_slice(&patch(cfg, false, "k2-app").unwrap()).unwrap();
        assert_eq!(out["process"]["apparmorProfile"], "k2-app");
        let pr = br#"{"args":["/bin/sh"],"noNewPrivileges":false,"terminal":true}"#;
        let out: serde_json::Value = serde_json::from_slice(&patch(pr, true, "k2-app").unwrap()).unwrap();
        assert_eq!(out["apparmorProfile"], "k2-app");
        assert_eq!(out["noNewPrivileges"], true, "an exec never gains privileges");
        for bad in [&b"not json"[..], b"[]", br#"{"noprocess":{}}"#, br#"{"process":"x"}"#] {
            assert!(patch(bad, false, "k2-app").is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(patch(b"[1]", true, "k2-app").is_err());
    }

    #[test]
    fn only_with_the_marker_and_apparmor_on() {
        assert!(wanted(Some("k2-app\n"), Some("Y\n")));
        assert!(!wanted(None, Some("Y")));
        assert!(!wanted(Some("k2-app\n"), Some("N")));
        assert!(!wanted(Some("k2-app\n"), None));
        assert!(!wanted(Some("other\n"), Some("Y")));
    }

    #[test]
    fn rewrite_keeps_mode_and_links_point_at_the_engine() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        std::fs::write(&p, br#"{"process":{"args":["x"]}}"#).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        rewrite(&p, false).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["process"]["apparmorProfile"], APPARMOR_PROFILE);
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
        }
        std::fs::write(&p, b"garbage").unwrap();
        assert!(rewrite(&p, false).is_err(), "a config it can't patch is a refusal");
        assert!(rewrite(&d.path().join("missing.json"), true).is_err());

        let engine = Path::new("/usr/local/libexec/k2-app-engine");
        install_links(d.path(), engine).unwrap();
        install_links(d.path(), engine).unwrap();
        for rt in ["crun", "youki"] {
            assert_eq!(std::fs::read_link(shim_path(d.path(), rt)).unwrap(), engine);
        }
        assert_eq!(runtime_for_argv0("/var/lib/k2app/engine/oci/crun"), Some(OciRuntime::Crun));
        assert_eq!(runtime_for_argv0("youki"), Some(OciRuntime::Youki));
        assert_eq!(runtime_for_argv0("/usr/local/libexec/k2-app-engine"), None);
        assert_eq!(runtime_for_argv0("runc"), None);
    }
}
