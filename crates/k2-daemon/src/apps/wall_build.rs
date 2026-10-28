//! S16: App builds run inside the wall (`prd-k2-app-runtime-v1` §4.1 "App
//! build", S16; engine protocol 3 `build`).
//!
//! K2 never infers an install from a `package.json` it finds (told Sew
//! 2026-10-16): only the manifest's declared `build` argv runs, a lockfile
//! is required next to a `package.json`, and the build runs in a container
//! in the App's own slot (its ids, no capabilities, K2's seccomp profile,
//! the MAC shim, no secrets, no gateway link) with egress only through the
//! App's proxy, which lets the package registries through while it runs
//! (`egress_proxy::build_window`). On the guarded path install scripts are
//! off (a dependency's `postinstall` never runs); the open path keeps them.
//! The built tree comes back as a tar and is stored as the Seed (its row
//! says `built_from` the source tree and `createdWith.engine = "wall"`).
//!
//! Where the wall isn't ready (macOS, an unprepared box) `k2 app build`
//! keeps stage 1's behaviour (the folder as the agent built it) and says
//! so. `k2 app build` with no App builds in the reserved `build` slot.

use std::io::Read;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::deploy::manifest::Manifest;
use k2_core::deploy::{self, codes};
use k2_root_proto::engine::{self as wire, BuildSpec, Request};
#[cfg(unix)]
use k2_root_proto::engine::Answer;
use serde_json::{json, Value as J};

/// The engine slot a build without an App runs in: `build` is a reserved
/// App id, so no App ever takes it.
pub const BUILD_SLOT: &str = "build";

/// The lockfiles a `package.json` needs beside it (S16: no unpinned
/// installs in the wall).
pub const LOCKFILES: &[&str] = &["package-lock.json", "npm-shrinkwrap.json", "pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb"];

/// `lockfile_required` when the folder has a `package.json` and no lockfile.
pub fn lockfile_problem(folder: &Path) -> Option<String> {
    if !folder.join("package.json").is_file() {
        return None;
    }
    if LOCKFILES.iter().any(|l| folder.join(l).is_file()) {
        return None;
    }
    Some(format!(
        "the folder has a package.json but no lockfile ({}): a build in the wall installs exactly what is pinned. Commit one (npm install --package-lock-only) and build again",
        LOCKFILES.join(", ")
    ))
}

/// What a build in the wall did.
#[derive(Debug, Clone)]
pub struct Built {
    /// The built tree (a temp folder the caller stores, then removes).
    pub tree: PathBuf,
    pub output: String,
    pub ms: u64,
    pub source: String,
}

/// A failed build: the code, the words, the build's own output.
#[derive(Debug, Clone)]
pub struct Failed {
    pub code: &'static str,
    pub message: String,
    pub output: Option<String>,
}

fn fail(code: &'static str, m: impl Into<String>) -> Failed {
    Failed { code, message: m.into(), output: None }
}

pub mod fail_codes {
    pub const BUILD_FAILED: &str = "build_failed";
    pub const LOCKFILE_REQUIRED: &str = "lockfile_required";
}

#[cfg(unix)]
fn tmp_dir() -> PathBuf {
    k2_core::paths::k2_home().join("apps").join("tmp")
}

fn new_build_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("b-{}", &u[..12])
}

/// Run `m.build` over `folder` in `slot`'s wall. `scripts` = install
/// scripts allowed (the open path).
pub fn run(slot: &str, folder: &Path, m: &Manifest, scripts: bool, log: &dyn Fn(&str)) -> Result<Built, Failed> {
    if m.build.is_empty() {
        return Err(fail(codes::BAD_REQUEST, "the manifest declares no build"));
    }
    if let Some(p) = lockfile_problem(folder) {
        return Err(fail(fail_codes::LOCKFILE_REQUIRED, p));
    }
    let app = k2_root_proto::apps::AppId::parse(slot).ok_or_else(|| fail(codes::APP_ID_INVALID, format!("'{slot}' can't hold a build slot")))?;
    super::open::ready().map_err(|e| fail(codes::ENGINE_UNAVAILABLE, e))?;
    // The source tree reaches the engine like a Seed (content-addressed);
    // its own copy here goes once the engine has it, unless a stored Seed
    // already is that tree.
    let excludes = deploy::seed::build_excludes(&m.exclude);
    let stored = deploy::seed::store_folder(folder, &excludes).map_err(|e| fail(e.code, e.message))?;
    let had_row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        deploy::store::get_seed(&conn, &stored.hash).ok().flatten().is_some()
    };
    let drop_source = || {
        if !had_row && !stored.reused {
            let _ = deploy::seed::remove_tree(&stored.hash);
        }
    };
    let seed = k2_core::deploy::engine::SeedRef { hash: stored.hash.clone(), path: deploy::seed::seed_path(&stored.hash) };
    let res = (|| {
        let image = super::guarded::ensure_full_image().map_err(|e| fail(e.code, e.message))?;
        super::guarded::ensure_seed(&seed).map_err(|e| fail(e.code, e.message))?;
        // The slot, its id mapping and its gateway port (the nft rule that
        // admits the App's own proxy port is keyed on it).
        let p = super::guarded::call(&Request::Prepare { app: app.as_str().into(), image: image.clone(), seed: seed.hash.clone() }, None, Duration::from_secs(960))
            .map_err(|e| fail(e.code, e.message))?;
        let gw = p["gwPort"].as_u64().and_then(|g| u16::try_from(g).ok()).ok_or_else(|| fail(codes::ENGINE_UNAVAILABLE, "prepare gave no gateway port"))?;
        if let Err(e) = super::guarded::install_gateway_rule(app.as_str(), gw) {
            log(&format!("build: the slot's rule didn't load ({e}); the build may not reach its proxy"));
        }
        let px = super::guarded::proxy_port_of(&p, Some(gw));
        super::guarded::ensure_proxy(app.as_str(), px).map_err(|e| fail(codes::ENGINE_UNAVAILABLE, format!("the build's egress proxy: {e}")))?;
        if app.as_str() != BUILD_SLOT {
            super::egress_proxy::reload(app.as_str());
        }
        let _window = super::egress_proxy::build_window(app.as_str());
        let spec = BuildSpec {
            app: app.as_str().into(),
            build: new_build_id(),
            seed: seed.hash.clone(),
            image,
            argv: m.build.clone(),
            env: m.env.iter().filter(|(k, _)| wire::env_name_ok(k) && !k.starts_with("K2_")).map(|(k, v)| (k.clone(), v.clone())).collect(),
            scripts,
            timeout_secs: wire::BUILD_TIMEOUT_DEFAULT,
            resources: wire::Resources::k2_default(),
        };
        wire::check_build(&spec).map_err(|w| fail(codes::MANIFEST_INVALID, format!("the build can't run: {w}")))?;
        log(&format!("build {} in the wall: {} (slot {}, scripts {})", spec.build, spec.argv.join(" "), app.as_str(), if scripts { "on" } else { "off" }));
        call_build(&spec)
    })();
    drop_source();
    res.map(|mut b| {
        b.source = stored.hash.clone();
        b
    })
}

/// The `build` request: an answer line, then the built tree's tar.
#[cfg(unix)]
fn call_build(spec: &BuildSpec) -> Result<Built, Failed> {
    use std::os::unix::net::UnixStream;
    if !cfg!(target_os = "linux") {
        return Err(fail(codes::ENGINE_UNAVAILABLE, "builds in the wall run on Linux boxes only"));
    }
    let mut c = UnixStream::connect(wire::SOCKET).map_err(|e| fail(codes::ENGINE_UNAVAILABLE, format!("the App engine isn't answering ({e})")))?;
    let _ = c.set_read_timeout(Some(Duration::from_secs(spec.timeout_secs + 600)));
    let _ = c.set_write_timeout(Some(Duration::from_secs(60)));
    let mut line = serde_json::to_vec(&Request::Build(Box::new(spec.clone()))).map_err(|e| fail(codes::BAD_REQUEST, e.to_string()))?;
    line.push(b'\n');
    c.write_all(&line).map_err(|e| fail(codes::ENGINE_UNAVAILABLE, format!("engine write: {e}")))?;
    let _ = c.shutdown(std::net::Shutdown::Write);
    let mut r = BufReader::new(c);
    let mut ans = String::new();
    r.by_ref().take(4 * wire::MAX_OUTPUT as u64).read_line(&mut ans).map_err(|e| fail(codes::ENGINE_UNAVAILABLE, format!("engine answer: {e}")))?;
    let a: Answer = serde_json::from_str(ans.trim()).map_err(|_| fail(codes::ENGINE_UNAVAILABLE, "the App engine gave no answer"))?;
    if !a.ok {
        return Err(fail(codes::ENGINE_UNAVAILABLE, format!("App engine: {}", a.message.unwrap_or_default())));
    }
    let output = a.data["output"].as_str().unwrap_or("").to_string();
    let ms = a.data["ms"].as_u64().unwrap_or(0);
    let Some(len) = a.data["tarLen"].as_u64() else {
        let why = if a.data["timedOut"] == true { "timed out".to_string() } else { format!("exited {}", a.data["exitCode"]) };
        return Err(Failed { code: fail_codes::BUILD_FAILED, message: format!("the build {why} (its output is below)"), output: Some(output) });
    };
    if len > wire::MAX_BUILD_TAR {
        return Err(fail(codes::BAD_REQUEST, format!("the built tree is {len} bytes; the cap is {}", wire::MAX_BUILD_TAR)));
    }
    let dir = tmp_dir();
    std::fs::create_dir_all(&dir).map_err(|e| fail("io_error", format!("mkdir {}: {e}", dir.display())))?;
    let tree = dir.join(format!("built-{}", spec.build));
    let _ = std::fs::remove_dir_all(&tree);
    std::fs::create_dir_all(&tree).map_err(|e| fail("io_error", e.to_string()))?;
    let unpacked = unpack(&mut r.take(len), &tree);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&tree);
        return Err(fail(codes::SEED_INVALID, format!("the built tree: {e}")));
    }
    Ok(Built { tree, output, ms, source: String::new() })
}

#[cfg(not(unix))]
fn call_build(_: &BuildSpec) -> Result<Built, Failed> {
    Err(fail(codes::ENGINE_UNAVAILABLE, "builds in the wall run on Linux boxes only"))
}

/// Unpack the engine's tar into `dst`: files, folders and links only, no
/// owners or modes beyond the exec bit, nothing outside `dst` (the tar
/// crate refuses `..` and absolute paths; the Seed store refuses a link
/// that points outside the tree).
pub fn unpack(r: &mut dyn Read, dst: &Path) -> Result<usize, String> {
    let mut a = tar::Archive::new(r);
    a.set_preserve_permissions(false);
    a.set_preserve_ownerships(false);
    a.set_unpack_xattrs(false);
    let mut n = 0usize;
    for e in a.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        let kind = e.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            continue;
        }
        #[cfg(unix)]
        let exec = kind.is_file() && e.header().mode().is_ok_and(|m| m & 0o111 != 0);
        let path = e.path().map_err(|e| e.to_string())?.to_path_buf();
        if !e.unpack_in(dst).map_err(|er| format!("{}: {er}", path.display()))? {
            return Err(format!("{} would land outside the tree", path.display()));
        }
        #[cfg(unix)]
        if exec {
            use std::os::unix::fs::PermissionsExt;
            let p = dst.join(&path);
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        n += 1;
    }
    Ok(n)
}

/// A build log id: `b-` and 12 hex digits.
pub fn is_build_log_id(id: &str) -> bool {
    id.len() == 14 && id.starts_with("b-") && id[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Keep a failed App build's output beside its deployment logs (capped,
/// owner-only). `None` for the reserved build slot or no output.
pub(crate) fn save_build_log(slot: &str, output: Option<&str>) -> Option<String> {
    let out = output.filter(|o| !o.trim().is_empty())?;
    if slot == BUILD_SLOT {
        return None;
    }
    let id = new_build_id();
    let p = deploy::deployment_log_path(slot, &id);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    // The last MiB (the end of a build's output says why it failed).
    let mut start = out.len().saturating_sub(1 << 20);
    while !out.is_char_boundary(start) {
        start += 1;
    }
    std::fs::write(&p, &out[start..]).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    Some(id)
}

/// The refusal for a failed build, its output attached as untrusted App
/// text (§5.9: it is what the App's build printed).
pub(crate) fn failed_refusal(slot: &str, f: Failed) -> super::routes::Refusal {
    let mut r = super::routes::no(f.code, if f.code == fail_codes::BUILD_FAILED || f.code == fail_codes::LOCKFILE_REQUIRED { 422 } else { 503 }, f.message);
    if let Some(o) = f.output {
        r.extra = Some(json!({ "buildOutput": deploy::framing::frame(slot, "build", &o, 64 * 1024) }));
    }
    r
}

/// Build in the wall when the manifest declares a build and the box has
/// the wall; store the result as the Seed. `Ok(None)` = not run here (no
/// build declared, or no wall on this box: the reason is the string).
pub(crate) fn build_and_store(slot: &str, folder: &Path, signer: &str, opts: &deploy::validate::Options, scripts: bool) -> Result<Result<(deploy::build::Built, J), String>, super::routes::Refusal> {
    let (report, manifest) = deploy::validate::validate_tree(folder, opts);
    let Some(m) = manifest.filter(|_| report.ok) else { return Ok(Err("validate failed".into())) };
    if m.build.is_empty() {
        return Ok(Err("no build declared".into()));
    }
    if let Err(why) = super::open::ready() {
        return Ok(Err(why));
    }
    let lines = std::sync::Mutex::new(Vec::<String>::new());
    let built = run(slot, folder, &m, scripts, &|l| lines.lock().unwrap_or_else(|e| e.into_inner()).push(l.to_string())).map_err(|f| {
        // An App's failed build keeps its output as a build log (`b-…`, read
        // with k2 app logs) and says which step failed, for the failure
        // history and the agent's wake (0.46.3).
        let log = save_build_log(slot, f.output.as_deref());
        let mut r = failed_refusal(slot, f);
        let mut extra = r.extra.take().unwrap_or_else(|| json!({}));
        extra["buildStep"] = json!(m.build.join(" "));
        if let Some(id) = log {
            extra["buildLog"] = json!(id);
        }
        r.extra = Some(extra);
        r
    })?;
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        deploy::build::build_folder(&conn, &built.tree, signer, opts)
    };
    let _ = std::fs::remove_dir_all(&built.tree);
    let mut b = stored.map_err(|f| {
        let mut r = super::routes::no(f.code, 422, format!("the built tree didn't validate: {}", f.message));
        r.extra = f.report.map(|rep| json!({ "report": rep }));
        r
    })?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut cw = b.seed.created_with.clone();
        cw["engine"] = json!("wall");
        cw["baseImage"] = json!(super::guarded::full_image_for(std::env::consts::ARCH).map(|i| i.local()));
        conn.execute(
            "UPDATE seeds SET built_from = ?2, created_with = ?3, source_ref = COALESCE(source_ref, ?4) WHERE hash = ?1",
            rusqlite::params![b.seed.hash, format!("seed:{}", built.source), cw.to_string(), deploy::build::git_head(folder)],
        )
        .map_err(super::routes::db_err)?;
        if let Some(row) = deploy::store::get_seed(&conn, &b.seed.hash).map_err(super::routes::db_err)? {
            b.seed = row;
        }
    }
    let out = deploy::framing::frame(slot, "build", &built.output, 16 * 1024);
    Ok(Ok((b, json!({ "inWall": true, "ms": built.ms, "from": built.source, "output": out, "log": lines.into_inner().unwrap_or_default() }))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Scratch;

    #[test]
    fn a_package_json_needs_a_lockfile() {
        let d = Scratch::new();
        assert_eq!(lockfile_problem(d.path()), None, "no package.json: nothing to pin");
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        let p = lockfile_problem(d.path()).unwrap();
        assert!(p.contains("package-lock.json") && p.contains("no lockfile"), "{p}");
        std::fs::write(d.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(lockfile_problem(d.path()), None);
    }

    #[test]
    fn the_built_tar_unpacks_inside_the_tree_only() {
        let d = Scratch::new();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(2);
        h.set_mode(0o755);
        b.append_data(&mut h, "bin/run", &b"#!"[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o600);
        h.set_uid(65_537);
        b.append_data(&mut h, "node_modules/x/index.js", &b"1"[..]).unwrap();
        let t = b.into_inner().unwrap();
        let n = unpack(&mut &t[..], d.path()).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read(d.path().join("bin/run")).unwrap(), b"#!");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.path().join("bin/run")).unwrap().permissions().mode() & 0o111, 0o111, "exec bit kept");
        }
        // A path that climbs out is refused (the tar crate never writes it).
        let mut raw = tar::Header::new_gnu();
        raw.set_size(1);
        raw.set_mode(0o644);
        let mut evil = Vec::new();
        {
            let name = b"../escape";
            raw.as_old_mut().name[..name.len()].copy_from_slice(name);
            raw.set_cksum();
            let mut bb = tar::Builder::new(&mut evil);
            bb.append(&raw, &b"x"[..]).unwrap();
            bb.finish().unwrap();
        }
        let e = Scratch::new();
        let inner = e.path().join("tree");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(unpack(&mut &evil[..], &inner).is_err());
        assert!(!e.path().join("escape").exists());
    }

    #[test]
    fn build_ids_fit_the_wire() {
        for _ in 0..20 {
            assert!(wire::build_id_ok(&new_build_id()));
        }
        assert!(k2_root_proto::apps::AppId::parse(BUILD_SLOT).is_some());
        assert!(deploy::RESERVED_APP_IDS.contains(&BUILD_SLOT), "no App can take the build slot");
    }
}
