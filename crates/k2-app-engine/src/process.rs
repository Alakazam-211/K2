//! The `process` exception (`prd-k2-app-runtime-v1` §4.3, AR45): an App
//! the owner moved out of the guarded container (for a device a container
//! can't reach) runs as a sandboxed **system** unit,
//! `k2app-process@<id>.service`, as `k2appx` — never as `k2app`, which owns
//! every guarded container's user namespace.
//!
//! The engine's part (it never starts anything; the root helper does):
//! - [`stage`]: copy the deployment's Seed into
//!   `apps/<id>/stage-<deployment>/code`, write the resolved
//!   `spec.json` (argv and the whole environment), the file secrets and the
//!   App pass, all world-readable *inside the App's own folder*: the unit
//!   binds only `apps/<id>/process` (read-only) under a tmpfs that hides
//!   the rest of `k2app`'s home, so `k2appx` reads its own and nothing else;
//! - [`activate`]: make a staged deployment the unit's `process/` folder
//!   (the daemon stopped the unit first);
//! - [`run`]: `k2-app-engine process-run <id>` inside the unit: read the
//!   spec, make the state folders, exec the command.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use k2_root_proto::apps::AppId;
use k2_root_proto::engine::{self as wire, codes, gateway_port, hex_decode, process as paths, ProcessRun, ProcessSpec};
use serde_json::{json, Value};

use crate::server::Engine;

type R<T> = Result<T, (&'static str, String)>;

fn bad(m: impl Into<String>) -> (&'static str, String) {
    (codes::BAD_REQUEST, m.into())
}

fn io(e: impl std::fmt::Display) -> (&'static str, String) {
    (codes::IO, e.to_string())
}

/// Variables the engine sets itself (a deployment can't).
const OWN: &[&str] = &[
    "PATH", "HOME", "TMPDIR", "PORT", "NODE_OPTIONS", "K2_PASS_FILE", "K2_SECRETS_DIR", "K2_APP_DATA", "K2_APP_CACHE", "K2_HOST", "K2_PORT",
    "K2_GATEWAY_URL",
];

pub const CHILD_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

fn app_dir(e: &Engine, app: &AppId) -> PathBuf {
    e.apps().join(app.as_str())
}

/// The environment `process-run` gives the command.
pub fn resolve_env(s: &ProcessSpec, gw: u16, secret_env: &BTreeMap<String, String>, has_files: bool, has_pass: bool) -> BTreeMap<String, String> {
    let app = AppId::parse(&s.app).expect("checked");
    let state = paths::state(&app);
    let dir = paths::dir(&app);
    let mut env: BTreeMap<String, String> = s.env.iter().filter(|(k, _)| !OWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    env.extend(secret_env.iter().filter(|(k, _)| !OWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())));
    env.insert("PATH".into(), CHILD_PATH.into());
    env.insert("HOME".into(), format!("{state}/home"));
    env.insert("TMPDIR".into(), "/tmp".into());
    env.insert("PORT".into(), s.port.to_string());
    env.insert("K2_APP_DATA".into(), format!("{state}/data"));
    env.insert("K2_APP_CACHE".into(), format!("{state}/cache"));
    env.insert("K2_HOST".into(), "127.0.0.1".into());
    env.insert("K2_PORT".into(), gw.to_string());
    env.insert("K2_GATEWAY_URL".into(), format!("http://127.0.0.1:{gw}"));
    env.insert(
        "NODE_OPTIONS".into(),
        format!("--max-old-space-size={} --report-on-fatalerror --report-directory={state}/data/.k2-diagnostics", s.memory_mib / 2),
    );
    if has_files {
        env.insert("K2_SECRETS_DIR".into(), format!("{dir}/secrets"));
    }
    if has_pass {
        env.insert("K2_PASS_FILE".into(), format!("{dir}/pass"));
    }
    env
}

/// Copy a Seed tree: directories 0755, files 0644 (0755 when executable),
/// links kept as links; nothing is followed out of the tree.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new().mode(0o755).create(to)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let src = e.path();
        let dst = to.join(e.file_name());
        let m = std::fs::symlink_metadata(&src)?;
        let ft = m.file_type();
        if ft.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if ft.is_dir() {
            n += copy_tree(&src, &dst)?;
        } else if ft.is_file() {
            let mode = if m.permissions().mode() & 0o111 != 0 { 0o755 } else { 0o644 };
            let mut r = std::fs::File::open(&src)?;
            let mut w = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).custom_flags(libc::O_NOFOLLOW).open(&dst)?;
            n += std::io::copy(&mut r, &mut w)?;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode))?;
        }
        // Sockets, fifos and devices are never copied.
    }
    Ok(n)
}

fn write_file(p: &Path, bytes: &[u8], mode: u32) -> R<()> {
    crate::sys::write_atomic(p, bytes, mode).map_err(io)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).map_err(io)
}

/// `process-stage`: the deployment's code, spec, secrets and pass under
/// `apps/<id>/stage-<deployment>/`. Returns the App's gateway port.
pub fn stage(e: &Engine, s: &ProcessSpec) -> R<Value> {
    let app = wire::check_process(s).map_err(|w| bad(format!("process-stage: {w}")))?;
    let seed = e.seeds().join(&s.seed);
    if !seed.is_dir() {
        return Err((codes::NOT_FOUND, "Seed not on the engine (seed-put first)".into()));
    }
    let slot = e.assign_slot(&app)?;
    let gw = gateway_port(slot).ok_or((codes::NO_SLOT, "slot outside the gateway range".into()))?;
    let dir = app_dir(e, &app);
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(io)?;
    }
    let stage = dir.join(format!("stage-{}", s.deployment));
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(io)?;
    }
    let res = (|| -> R<Value> {
        // 0755 all the way down (the engine's umask is 077): k2appx reads
        // this folder through the unit's bind mount.
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new().mode(0o755).create(&stage).map_err(io)?;
            std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
        }
        copy_tree(&seed, &stage.join("code")).map_err(|er| (codes::IO, format!("copy the Seed: {er}")))?;
        let mut secret_env = BTreeMap::new();
        let mut files = false;
        for it in &s.secrets {
            let v = hex_decode(&it.value_hex).ok_or_else(|| bad("secret value"))?;
            match (&it.env, &it.file) {
                (Some(var), None) => {
                    let text = String::from_utf8(v).map_err(|_| bad(format!("secret {var} is not text; deliver it as a file")))?;
                    if text.chars().any(|c| c == '\0') {
                        return Err(bad(format!("secret {var} holds a NUL")));
                    }
                    secret_env.insert(var.clone(), text);
                }
                (None, Some(f)) => {
                    let d = stage.join("secrets");
                    if !d.exists() {
                        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
                        std::fs::DirBuilder::new().mode(0o755).create(&d).map_err(io)?;
                        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
                    }
                    write_file(&d.join(f), &v, 0o644)?;
                    files = true;
                }
                _ => return Err(bad("secret target")),
            }
        }
        let has_pass = match &s.pass_hex {
            Some(p) => {
                write_file(&stage.join("pass"), &hex_decode(p).ok_or_else(|| bad("pass"))?, 0o644)?;
                true
            }
            None => false,
        };
        let run = ProcessRun {
            app: app.as_str().into(),
            deployment: s.deployment.clone(),
            port: s.port,
            gw_port: gw,
            argv: s.argv.clone(),
            env: resolve_env(s, gw, &secret_env, files, has_pass),
        };
        write_file(&stage.join("spec.json"), &serde_json::to_vec(&run).map_err(io)?, 0o644)?;
        Ok(json!({ "staged": true, "gwPort": gw, "slot": slot }))
    })();
    if res.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    res
}

/// `process-activate`: `stage-<deployment>` becomes `process/` (the old
/// one is removed). The daemon stopped the unit first.
pub fn activate(e: &Engine, app: &str, dep: &str) -> R<Value> {
    let app = AppId::parse(app).ok_or_else(|| bad("app id"))?;
    if !wire::deployment_ok(dep) {
        return Err(bad("deployment id"));
    }
    let dir = app_dir(e, &app);
    let stage = dir.join(format!("stage-{dep}"));
    if !stage.join("spec.json").is_file() {
        return Err((codes::NOT_FOUND, format!("deployment {dep} isn't staged (process-stage first)")));
    }
    let cur = dir.join("process");
    let old = dir.join(format!(".process-old-{}", std::process::id()));
    if cur.exists() {
        std::fs::rename(&cur, &old).map_err(io)?;
    }
    if let Err(er) = std::fs::rename(&stage, &cur) {
        let _ = std::fs::rename(&old, &cur);
        return Err(io(er));
    }
    let _ = std::fs::remove_dir_all(&old);
    // Other stages (a deploy that never activated) go too.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for d in rd.flatten() {
            if d.file_name().to_string_lossy().starts_with("stage-") {
                let _ = std::fs::remove_dir_all(d.path());
            }
        }
    }
    Ok(json!({ "active": dep }))
}

fn read_spec(path: &Path) -> Result<ProcessRun, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut b = Vec::new();
    f.take(4 << 20).read_to_end(&mut b).map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))
}

/// `process-state`: which deployment `process/` holds.
pub fn state(e: &Engine, app: &str) -> R<Value> {
    let app = AppId::parse(app).ok_or_else(|| bad("app id"))?;
    let p = app_dir(e, &app).join("process").join("spec.json");
    if !p.is_file() {
        return Ok(json!({ "deployment": null }));
    }
    let s = read_spec(&p).map_err(io)?;
    Ok(json!({ "deployment": s.deployment, "port": s.port, "gwPort": s.gw_port }))
}

/// Uninstall: the code and stages (not the state under /var/lib/k2appx).
pub fn remove(e: &Engine, app: &AppId) {
    let dir = app_dir(e, app);
    let _ = std::fs::remove_dir_all(dir.join("process"));
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for d in rd.flatten() {
            let n = d.file_name().to_string_lossy().to_string();
            if n.starts_with("stage-") || n.starts_with(".process-old-") {
                let _ = std::fs::remove_dir_all(d.path());
            }
        }
    }
}

/// `k2-app-engine process-run <id>`, inside `k2app-process@<id>` as
/// `k2appx`: read the spec, make the state folders, exec the command.
/// Returns only on failure (the exit code).
pub fn run(id: &str) -> i32 {
    let Some(app) = AppId::parse(id) else {
        eprintln!("k2-app-engine process-run: not an App id");
        return 2;
    };
    let spec = match read_spec(Path::new(&paths::spec(&app))) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("k2-app-engine process-run: {e}");
            return 78;
        }
    };
    if spec.app != app.as_str() || spec.argv.is_empty() {
        eprintln!("k2-app-engine process-run: the spec is for another App or has no command");
        return 78;
    }
    let state = paths::state(&app);
    for sub in ["data", "cache", "home", "data/.k2-diagnostics"] {
        use std::os::unix::fs::DirBuilderExt;
        let p = format!("{state}/{sub}");
        if let Err(e) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&p) {
            eprintln!("k2-app-engine process-run: mkdir {p}: {e}");
            return 73;
        }
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&spec.argv[0])
        .args(&spec.argv[1..])
        .env_clear()
        .envs(&spec.env)
        .current_dir(paths::code(&app))
        .exec();
    eprintln!("k2-app-engine process-run: exec {}: {err}", spec.argv[0]);
    127
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_root_proto::engine::SecretItem;

    fn spec() -> ProcessSpec {
        ProcessSpec {
            app: "gpu-job".into(),
            deployment: "d-0123456789ab".into(),
            seed: "c".repeat(64),
            port: 41_000,
            argv: vec!["python3".into(), "serve.py".into()],
            env: [("MODEL".to_string(), "small".to_string()), ("PATH".to_string(), "/evil".to_string())].into(),
            memory_mib: 2048,
            secrets: vec![
                SecretItem { env: Some("HF_TOKEN".into()), file: None, value_hex: wire::hex_encode(b"hf_x") },
                SecretItem { env: None, file: Some("db.json".into()), value_hex: wire::hex_encode(b"{}") },
            ],
            pass_hex: Some(wire::hex_encode(b"k2skn_p")),
        }
    }

    fn engine_with_seed() -> (tempfile::TempDir, Engine) {
        let d = tempfile::tempdir().unwrap();
        let ids = "k2app:1048576:4194304\n";
        std::fs::write(d.path().join("subuid"), ids).unwrap();
        std::fs::write(d.path().join("subgid"), ids).unwrap();
        let e = Engine::new(d.path().to_path_buf(), "/tmp".into()).with_subid_files(d.path().join("subuid"), d.path().join("subgid"));
        let seed = d.path().join("seeds").join("c".repeat(64));
        std::fs::create_dir_all(seed.join("lib")).unwrap();
        std::fs::write(seed.join("serve.py"), "print(1)\n").unwrap();
        std::fs::write(seed.join("lib/run.sh"), "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(seed.join("lib/run.sh"), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::os::unix::fs::symlink("/etc/passwd", seed.join("evil")).unwrap();
        std::fs::create_dir_all(d.path().join("engine")).unwrap();
        (d, e)
    }

    #[test]
    fn stage_then_activate_resolves_everything_the_unit_needs() {
        let (d, e) = engine_with_seed();
        let v = stage(&e, &spec()).unwrap();
        let gw = v["gwPort"].as_u64().unwrap();
        assert!((38_600..=38_899).contains(&gw));
        let st = d.path().join("apps/gpu-job/stage-d-0123456789ab");
        assert_eq!(std::fs::read_to_string(st.join("code/serve.py")).unwrap(), "print(1)\n");
        assert_eq!(std::fs::read_link(st.join("code/evil")).unwrap(), Path::new("/etc/passwd"), "a link stays a link");
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&st.join("code/lib/run.sh")), 0o755, "executable bit kept, readable by k2appx");
        assert_eq!(mode(&st.join("code/serve.py")), 0o644);
        assert_eq!(mode(&st), 0o755);
        assert_eq!(std::fs::read(st.join("secrets/db.json")).unwrap(), b"{}");
        assert_eq!(std::fs::read(st.join("pass")).unwrap(), b"k2skn_p");
        let run = read_spec(&st.join("spec.json")).unwrap();
        assert_eq!(run.env["PATH"], CHILD_PATH, "a deployment never sets PATH");
        assert_eq!(run.env["MODEL"], "small");
        assert_eq!(run.env["HF_TOKEN"], "hf_x");
        assert_eq!(run.env["PORT"], "41000");
        assert_eq!(run.env["HOME"], "/var/lib/k2appx/gpu-job/home");
        assert_eq!(run.env["K2_APP_DATA"], "/var/lib/k2appx/gpu-job/data");
        assert_eq!(run.env["K2_PORT"], gw.to_string());
        assert_eq!(run.env["K2_SECRETS_DIR"], "/var/lib/k2app/apps/gpu-job/process/secrets");
        assert_eq!(run.env["K2_PASS_FILE"], "/var/lib/k2app/apps/gpu-job/process/pass");
        assert!(run.env["NODE_OPTIONS"].starts_with("--max-old-space-size=1024 "));

        assert_eq!(state(&e, "gpu-job").unwrap()["deployment"], Value::Null);
        activate(&e, "gpu-job", "d-0123456789ab").unwrap();
        assert!(d.path().join("apps/gpu-job/process/spec.json").is_file());
        assert!(!st.exists());
        assert_eq!(state(&e, "gpu-job").unwrap()["deployment"], "d-0123456789ab");

        // A second deploy replaces it; a stale stage is cleared.
        let mut s2 = spec();
        s2.deployment = "d-00000000000b".into();
        stage(&e, &s2).unwrap();
        let mut s3 = spec();
        s3.deployment = "d-00000000000c".into();
        stage(&e, &s3).unwrap();
        activate(&e, "gpu-job", "d-00000000000b").unwrap();
        assert_eq!(state(&e, "gpu-job").unwrap()["deployment"], "d-00000000000b");
        assert!(!d.path().join("apps/gpu-job/stage-d-00000000000c").exists());
        assert_eq!(stage(&e, &s2).unwrap()["gwPort"].as_u64(), Some(gw), "the slot is the App's, kept across deploys");

        remove(&e, &AppId::parse("gpu-job").unwrap());
        assert!(!d.path().join("apps/gpu-job/process").exists());
    }

    #[test]
    fn stage_refuses_and_leaves_nothing_behind() {
        let (d, e) = engine_with_seed();
        let mut s = spec();
        s.seed = "d".repeat(64);
        assert_eq!(stage(&e, &s).unwrap_err().0, codes::NOT_FOUND);
        let mut s = spec();
        s.secrets[0].value_hex = wire::hex_encode(&[0xff, 0xfe]);
        assert!(stage(&e, &s).is_err(), "a binary env secret is refused");
        assert!(!d.path().join("apps/gpu-job/stage-d-0123456789ab").exists());
        let mut s = spec();
        s.argv = vec![];
        assert_eq!(stage(&e, &s).unwrap_err().0, codes::BAD_REQUEST);
        assert!(activate(&e, "gpu-job", "d-0123456789ab").is_err(), "nothing staged");
        assert!(activate(&e, "../x", "d-0123456789ab").is_err());
    }
}
