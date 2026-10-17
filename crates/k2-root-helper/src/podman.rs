//! `apps podman install|uninstall` (protocol 8, 0.46.1 lane P): K2's
//! pinned Podman 5 bundle for boxes whose distro ships Podman < 5
//! (`k2_root_proto::podman` has the pins, paths and canned bytes).
//!
//! Install, in order, and nothing written before the first three pass:
//! 1. the box is one the bundle is for (`/etc/os-release`: Ubuntu 24.04,
//!    Debian 12) and `k2app` exists with its proven shape;
//! 2. stdin's sha256 is this arch's pin (a mismatch is a refusal);
//! 3. every required member is in the tarball as a regular file (links,
//!    devices, `..` and anything off the member list are never written);
//! 4. the tree is staged at `/opt/k2/.podman-stage`, the marker written
//!    last, then renamed over `/opt/k2/podman` (the old tree is removed
//!    after the swap);
//! 5. the k2app-only Podman config (`containers.rootless.conf.d/<uid>/`),
//!    the user generator (Quadlet for k2app only) and, where Ubuntu
//!    restricts user namespaces, the `k2-podman` AppArmor profiles;
//! 6. the installed podman must answer the pinned version.
//!
//! Uninstall (daemon-only) is refused while any App process runs, removes
//! only what install wrote (a generator K2 didn't write stays), and
//! leaves the distro's Podman as it was.

use std::ffi::OsStr;
use std::io::Read;

use k2_root_proto::podman::*;
use serde_json::{json, Value as J};

use crate::apps::{
    app_user, apparmor_parser, busy_app_processes, ensure_subdir, exists, file_state, kread, put,
    read_cfg, read_cfg_text, split, tool_version, unput,
};
use crate::ctx::{Ctx, Fail, HResult};
use crate::fsx::{Dir, Kind};

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The box's os-release line the bundle is for, or a refusal.
pub fn supported(ctx: &Ctx) -> HResult<String> {
    let text = read_cfg_text(ctx, OS_RELEASE)?;
    supported_os(&text).ok_or_else(|| {
        let (id, ver) = os_id(&text).unwrap_or_else(|| ("unknown".into(), "?".into()));
        Fail::state(format!(
            "podman_os_unsupported: K2's Podman bundle is for Ubuntu 24.04 and Debian 12; this box is {id} {ver} (use the distro's Podman 5+)"
        ))
    })
}

/// The allow-listed members of a verified tarball, by destination.
pub fn extract_members(tarball: &[u8], art: &PodmanArtifact) -> HResult<Vec<(&'static Member, Vec<u8>)>> {
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(tarball));
    let mut tar = tar::Archive::new(dec);
    let entries = tar
        .entries()
        .map_err(|_| Fail::argv("podman install: archive unreadable"))?;
    let mut out: Vec<(&'static Member, Vec<u8>)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| Fail::argv("podman install: archive entry unreadable"))?;
        let path = entry
            .path()
            .map_err(|_| Fail::argv("podman install: archive path unreadable"))?
            .to_string_lossy()
            .into_owned();
        let Some(m) = member_for(art.top, &path) else { continue };
        if !entry.header().entry_type().is_file() {
            return Err(Fail::argv(format!("podman install: {} is not a regular file in the archive", m.dest)));
        }
        if out.iter().any(|(x, _)| x.dest == m.dest) {
            return Err(Fail::argv(format!("podman install: {} appears twice in the archive", m.dest)));
        }
        let mut buf = Vec::new();
        entry
            .take(MAX_MEMBER.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|_| Fail::argv("podman install: read member failed"))?;
        if buf.len() as u64 > MAX_MEMBER || buf.is_empty() {
            return Err(Fail::argv(format!("podman install: {} is empty or too large", m.dest)));
        }
        out.push((m, buf));
    }
    for m in MEMBERS.iter().filter(|m| m.required) {
        if !out.iter().any(|(x, _)| x.dest == m.dest) {
            return Err(Fail::argv(format!("podman install: the archive has no {}", m.src)));
        }
    }
    Ok(out)
}

/// `name` under `d`, created 0755 (root) when missing; a link or a file
/// there is a refusal.
fn subdir(ctx: &Ctx, d: &Dir, name: &str) -> HResult<Dir> {
    let n = OsStr::new(name);
    match d.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {
            d.mkdir(n, 0o755).map_err(|e| Fail::io_err("mkdir", e))?;
            let s = d.sub(n).map_err(|e| Fail::io_err("open", e))?;
            if let Some((u, g)) = ctx.root_owner() {
                s.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
            }
            s.fchmod(0o755).map_err(|e| Fail::io_err("chmod", e))?;
            Ok(s)
        }
        Kind::Dir => d.sub(n).map_err(|e| Fail::io_err("open", e)),
        _ => Err(Fail::state("refused: a link or a file where K2 expects a directory")),
    }
}

/// `rel` (a `a/b/c` path from [`DIRS`]/[`MEMBERS`]) under `root`.
fn walk(ctx: &Ctx, root: &Dir, rel: &str) -> HResult<Dir> {
    let mut parts = rel.split('/');
    let mut d = subdir(ctx, root, parts.next().expect("non-empty"))?;
    for part in parts {
        d = subdir(ctx, &d, part)?;
    }
    Ok(d)
}

fn write_file(ctx: &Ctx, d: &Dir, name: &str, bytes: &[u8], mode: u32) -> HResult<()> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let n = OsStr::new(name);
    let mut f = d.create_excl(n).map_err(|e| Fail::io_err("create", e))?;
    f.write_all(bytes).map_err(|e| Fail::io_err("write", e))?;
    if let Some((u, g)) = ctx.root_owner() {
        // SAFETY: fd owned by f.
        if unsafe { libc::fchown(f.as_raw_fd(), u, g) } != 0 {
            return Err(Fail::io_err("chown", std::io::Error::last_os_error()));
        }
    }
    // SAFETY: fd owned by f.
    if unsafe { libc::fchmod(f.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(Fail::io_err("chmod", std::io::Error::last_os_error()));
    }
    f.sync_all().map_err(|e| Fail::io_err("fsync", e))
}

/// The installed tree already is this pin (marker and every member).
fn tree_current(ctx: &Ctx, want_marker: &str) -> bool {
    let marker_ok = read_cfg(ctx, MARKER, 4096).ok().flatten().is_some_and(|b| b == want_marker.as_bytes());
    marker_ok
        && MEMBERS
            .iter()
            .filter(|m| m.required)
            .all(|m| exists(ctx, &format!("{PREFIX}/{}", m.dest)))
}

/// Stage + swap the tree. `Ok(true)` when it changed.
fn install_tree(ctx: &Ctx, members: &[(&'static Member, Vec<u8>)], marker_bytes: &str) -> HResult<bool> {
    let opt = ensure_subdir(ctx, "/", split(OPT).1)?;
    drop(opt);
    let k2 = ensure_subdir(ctx, OPT, OPT_K2_NAME)?;
    for leftover in [STAGE_NAME, OLD_NAME] {
        k2.remove_tree(OsStr::new(leftover)).map_err(|e| Fail::io_err("remove leftover", e))?;
    }
    let stage = subdir(ctx, &k2, STAGE_NAME)?;
    let res = (|| -> HResult<()> {
        for d in DIRS {
            walk(ctx, &stage, d)?;
        }
        for (m, bytes) in members {
            let (parent, name) = m.dest.rsplit_once('/').expect("members live in a dir");
            let d = walk(ctx, &stage, parent)?;
            write_file(ctx, &d, name, bytes, 0o755)?;
        }
        write_file(ctx, &stage, MARKER_NAME, marker_bytes.as_bytes(), 0o644)?;
        stage.fsync().map_err(|e| Fail::io_err("fsync", e))
    })();
    if let Err(f) = res {
        let _ = k2.remove_tree(OsStr::new(STAGE_NAME));
        return Err(f);
    }
    let prefix = OsStr::new(PREFIX_NAME);
    match k2.kind(prefix).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {}
        Kind::Dir => k2.rename(prefix, OsStr::new(OLD_NAME)).map_err(|e| Fail::io_err("move old tree", e))?,
        _ => {
            let _ = k2.remove_tree(OsStr::new(STAGE_NAME));
            return Err(Fail::state("refused: /opt/k2/podman is a link or a file"));
        }
    }
    k2.rename(OsStr::new(STAGE_NAME), prefix).map_err(|e| Fail::io_err("swap tree", e))?;
    k2.fsync().map_err(|e| Fail::io_err("fsync", e))?;
    k2.remove_tree(OsStr::new(OLD_NAME)).map_err(|e| Fail::io_err("remove old tree", e))?;
    Ok(true)
}

/// The generator file is K2's (second line is the mark).
fn generator_is_ours(ctx: &Ctx) -> Option<bool> {
    let b = read_cfg(ctx, USER_GENERATOR, 64 * 1024).ok().flatten()?;
    Some(String::from_utf8_lossy(&b).lines().nth(1) == Some(GENERATOR_MARK))
}

fn userns_restricted(ctx: &Ctx) -> bool {
    kread(ctx, USERNS_RESTRICT).is_some_and(|s| s.trim() == "1")
}

/// The k2app-only config, the generator and the AppArmor profiles.
fn install_config(ctx: &Ctx, uid: u32) -> HResult<J> {
    let mut changed = false;
    // Podman config, read by Podman for k2app's uid only.
    let _ = ensure_subdir(ctx, "/etc", split(ETC_CONTAINERS).1)?;
    let _ = ensure_subdir(ctx, ETC_CONTAINERS, ROOTLESS_CONF_D_NAME)?;
    let cd = ensure_subdir(ctx, ROOTLESS_CONF_PARENT, &uid.to_string())?;
    changed |= put(ctx, &cd, ROOTLESS_CONF_FILE, rootless_conf().as_bytes())?;

    // Quadlet for k2app. Never over a generator K2 didn't write.
    if generator_is_ours(ctx) == Some(false) {
        return Err(Fail::state(format!(
            "refused: {USER_GENERATOR} exists and K2 didn't write it (move it away, then prepare again)"
        )));
    }
    let gd = ensure_subdir(ctx, "/etc/systemd", split(USER_GENERATORS).1)?;
    let want = user_generator(uid);
    let same = gd
        .read_file(OsStr::new(USER_GENERATOR_NAME), 64 * 1024)
        .map_err(|e| Fail::io_err("read", e))?
        .is_some_and(|b| b == want.as_bytes());
    if !same {
        gd.write_atomic(OsStr::new(USER_GENERATOR_NAME), want.as_bytes(), 0o755, ctx.root_owner())
            .map_err(|e| Fail::io_err("write generator", e))?;
        changed = true;
    }

    // User namespaces for the bundle where Ubuntu restricts them.
    let apparmor = if userns_restricted(ctx) {
        let parser = apparmor_parser(ctx).ok_or_else(|| {
            Fail::state("apparmor_missing: this kernel restricts user namespaces through AppArmor but apparmor_parser isn't installed")
        })?;
        let ad = ensure_subdir(ctx, "/etc", "apparmor.d")?;
        let wrote = put(ctx, &ad, APPARMOR_USERNS_FILE_NAME, apparmor_userns_profiles().as_bytes())?;
        // --replace loads or reloads (idempotent).
        ctx.run_ok(parser, &["--replace", "--write-cache", APPARMOR_USERNS_FILE], None)?;
        changed |= wrote;
        json!({"needed": true, "profiles": APPARMOR_USERNS_PROFILES})
    } else {
        json!({"needed": false})
    };
    Ok(json!({"changed": changed, "apparmor": apparmor}))
}

/// `apps podman install`: this build's pin for the helper's arch.
pub fn install(ctx: &Ctx, stdin: &[u8]) -> HResult<J> {
    let os = supported(ctx)?;
    let art = artifact_for_arch(ctx.arch).map_err(Fail::argv)?;
    install_with(ctx, stdin, art, os)
}

/// The install against one pinned artifact (tests pin a fixture tarball;
/// production passes [`artifact_for_arch`]'s).
pub fn install_with(ctx: &Ctx, stdin: &[u8], art: &PodmanArtifact, os: String) -> HResult<J> {
    let got = sha256_hex(stdin);
    if got != art.sha256 {
        return Err(Fail::sig(format!(
            "podman install: bundle sha256 mismatch (want the pinned {BUNDLE_SOURCE} v{BUNDLE_VERSION} {} tarball); nothing installed",
            art.asset
        )));
    }
    let pw = app_user(ctx)?;
    let members = extract_members(stdin, art)?;
    let marker_bytes = marker(ctx.arch, art.sha256);
    let tree_changed = if tree_current(ctx, &marker_bytes) {
        false
    } else {
        install_tree(ctx, &members, &marker_bytes)?
    };
    let cfg = install_config(ctx, pw.uid)?;
    let version = tool_version(ctx, PODMAN);
    if version.as_deref() != Some(BUNDLE_VERSION) {
        return Err(Fail::state(format!(
            "podman_bundle_broken: {PODMAN} --version answered {} (want {BUNDLE_VERSION})",
            version.as_deref().unwrap_or("nothing")
        )));
    }
    Ok(json!({
        "installed": BUNDLE_VERSION,
        "source": BUNDLE_SOURCE,
        "sha256": art.sha256,
        "arch": ctx.arch,
        "os": os,
        "prefix": PREFIX,
        "k2appUid": pw.uid,
        "changed": tree_changed || cfg["changed"] == true,
        "apparmor": cfg["apparmor"],
    }))
}

/// `apps podman uninstall` (daemon-only).
pub fn uninstall(ctx: &Ctx) -> HResult<J> {
    let uid = app_user(ctx).ok().map(|p| p.uid);
    if let Some(u) = uid {
        let busy = busy_app_processes(ctx, u);
        if busy > 0 {
            return Err(Fail::state(format!(
                "apps_running: {busy} App process(es) still run as k2app; stop every App first (K2's Podman is their container engine)"
            )));
        }
    }
    let mut removed: Vec<&str> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    match generator_is_ours(ctx) {
        Some(true) => {
            if unput(ctx, USER_GENERATORS, None, USER_GENERATOR_NAME)? {
                removed.push(USER_GENERATOR);
            }
        }
        Some(false) => kept.push(format!("{USER_GENERATOR} (K2 didn't write it)")),
        None => {}
    }
    if let Some(u) = uid {
        if unput(ctx, ROOTLESS_CONF_PARENT, Some(&u.to_string()), ROOTLESS_CONF_FILE)? {
            removed.push("containers.rootless.conf.d/<k2app uid>/90-k2-podman.conf");
        }
        // The per-uid folder goes too when nothing else is in it.
        if let Ok(parent) = ctx.dir(ROOTLESS_CONF_PARENT) {
            let n = u.to_string();
            if parent.kind(OsStr::new(&n)).ok() == Some(Kind::Dir)
                && parent.sub(OsStr::new(&n)).and_then(|d| d.entries()).is_ok_and(|e| e.is_empty())
            {
                parent.remove_tree(OsStr::new(&n)).map_err(|e| Fail::io_err("remove", e))?;
            }
        }
    }
    if exists(ctx, APPARMOR_USERNS_FILE) {
        let loaded = kread(ctx, k2_root_proto::apps::APPARMOR_PROFILES)
            .is_some_and(|t| APPARMOR_USERNS_PROFILES.iter().any(|p| k2_root_proto::apps::profile_mode(&t, p).is_some()));
        if loaded {
            if let Some(parser) = apparmor_parser(ctx) {
                ctx.run_ok(parser, &["--remove", APPARMOR_USERNS_FILE], None)?;
            }
        }
        if unput(ctx, "/etc/apparmor.d", None, APPARMOR_USERNS_FILE_NAME)? {
            removed.push(APPARMOR_USERNS_FILE);
        }
    }
    if let Ok(k2) = ctx.dir(OPT_K2) {
        for name in [PREFIX_NAME, STAGE_NAME, OLD_NAME] {
            if k2.kind(OsStr::new(name)).map_err(|e| Fail::io_err("stat", e))? != Kind::Missing {
                k2.remove_tree(OsStr::new(name)).map_err(|e| Fail::io_err("remove tree", e))?;
                if name == PREFIX_NAME {
                    removed.push(PREFIX);
                }
            }
        }
    }
    Ok(json!({"changed": !removed.is_empty(), "removed": removed, "kept": kept, "version": BUNDLE_VERSION}))
}

// ── what the rest of the helper reads ───────────────────────────────────

/// The bundle is installed (its marker is a regular file under the root).
pub fn installed(ctx: &Ctx) -> bool {
    read_cfg(ctx, MARKER, 4096).ok().flatten().is_some()
}

/// `podman`, `crun` and `pasta` as k2app runs them.
pub fn effective_podman(ctx: &Ctx) -> &'static str {
    pick(installed(ctx), PODMAN, k2_root_proto::apps::PODMAN)
}
pub fn effective_crun(ctx: &Ctx) -> &'static str {
    pick(installed(ctx), CRUN, k2_root_proto::apps::CRUN)
}
pub fn effective_pasta(ctx: &Ctx) -> &'static str {
    pick(installed(ctx), PASTA, k2_root_proto::apps::PASTA)
}

/// `podmanBundle` in `apps status`.
pub fn status_json(ctx: &Ctx) -> J {
    let marker: Option<J> = read_cfg(ctx, MARKER, 4096)
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let current = artifact_for_arch(ctx.arch)
        .ok()
        .is_some_and(|a| marker.as_ref().is_some_and(|m| m["sha256"] == a.sha256));
    let uid = app_user(ctx).ok().map(|p| p.uid);
    let conf = match uid {
        Some(u) => json!(file_state(ctx, &format!("{}/{ROOTLESS_CONF_FILE}", rootless_conf_dir(u)), rootless_conf().as_bytes())),
        None => J::Null,
    };
    let generator = match (generator_is_ours(ctx), uid) {
        (None, _) => "missing",
        (Some(false), _) => "foreign",
        (Some(true), Some(u)) => file_state(ctx, USER_GENERATOR, user_generator(u).as_bytes()),
        (Some(true), None) => "drift",
    };
    let os = read_cfg_text(ctx, OS_RELEASE).ok().and_then(|t| supported_os(&t));
    json!({
        "installed": marker.is_some(),
        "version": marker.as_ref().map(|m| m["version"].clone()),
        "current": current,
        "pin": BUNDLE_VERSION,
        "prefix": PREFIX,
        "conf": conf,
        "generator": generator,
        "apparmor": if userns_restricted(ctx) {
            json!(file_state(ctx, APPARMOR_USERNS_FILE, apparmor_userns_profiles().as_bytes()))
        } else {
            json!("not_needed")
        },
        "osSupported": os,
    })
}
