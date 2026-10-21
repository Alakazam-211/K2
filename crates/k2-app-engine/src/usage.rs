//! Box usage on the engine side (prd-app-resources-disk-guard-v1 §5.3,
//! §9.2; 0.46.3, no protocol bump: the verbs are announced in `version`'s
//! `verbs`).
//!
//! - **`disk-usage`**: the daemon can't read `/var/lib/k2app` (0750,
//!   `k2app`'s), so the engine measures. Each App folder's top-level
//!   children through `nice -n 19 ionice -c3 podman unshare timeout N du
//!   -xk -d 1` (the files belong to the App's mapped ids, like
//!   `remove_app`'s `podman unshare rm`); the Seeds and the build folder
//!   with a plain one-filesystem walk (they are `k2app`'s own files); the
//!   image store from `podman system df`. One time budget for all of it;
//!   what it doesn't reach is marked `partial`.
//! - **`seeds-gc`**: remove exactly the engine Seed copies the daemon
//!   names, and never one a run state or unit file still names (`skipped`).
//!
//! Measuring never writes; the sweep removes only `seeds/<hash>` folders.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use k2_root_proto::apps::AppId;
use k2_root_proto::engine::{self as wire, codes, seed_hash_ok, AppDisk, DiskUsage, ImagesDisk, SeedsGcAnswer};
use serde_json::Value;

use crate::server::{podman_bin, Engine};

type R<T> = Result<T, (&'static str, String)>;

const NICE: &str = "/usr/bin/nice";
const IONICE: &str = "/usr/bin/ionice";

/// The command that measures one App folder (`<dir>` appended): the idle
/// priority wrappers when the box has them, then `podman unshare timeout
/// <secs> du -xk -d 1`.
pub(crate) fn app_du_argv(secs: u64) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if Path::new(NICE).is_file() {
        v.extend([NICE.into(), "-n".into(), "19".into()]);
    }
    if Path::new(IONICE).is_file() {
        v.extend([IONICE.into(), "-c3".into()]);
    }
    v.extend([podman_bin().into(), "unshare".into(), "timeout".into(), "-s".into(), "KILL".into(), secs.to_string()]);
    v.extend(["du".into(), "-xk".into(), "-d".into(), "1".into()]);
    v
}

/// `du -k -d 1 <dir>` output → the App's parts. Lines are `<KiB>\t<path>`;
/// the line for `dir` itself is the total, its direct children are the
/// parts (`data`, `cache`, `logs`; everything else is `other`). A cut-off
/// run (time limit) still yields the children it printed.
pub fn parse_app_du(out: &str, dir: &Path) -> (AppDisk, bool) {
    let mut d = AppDisk::default();
    let mut total: Option<u64> = None;
    for line in out.lines() {
        let Some((n, p)) = line.split_once('\t') else { continue };
        let Ok(kib) = n.trim().parse::<u64>() else { continue };
        let bytes = kib.saturating_mul(1024);
        let p = Path::new(p.trim_end());
        if p == dir {
            total = Some(bytes);
            continue;
        }
        if p.parent() != Some(dir) {
            continue;
        }
        match p.file_name().and_then(|n| n.to_str()) {
            Some("data") => d.data = bytes,
            Some("cache") => d.cache = bytes,
            Some("logs") => d.logs = bytes,
            _ => d.other = d.other.saturating_add(bytes),
        }
    }
    match total {
        Some(t) => {
            // The folder's own blocks and loose files are "other" too.
            let named = d.data.saturating_add(d.cache).saturating_add(d.logs).saturating_add(d.other);
            d.other = d.other.saturating_add(t.saturating_sub(named));
            (d, true)
        }
        None => (d, false),
    }
}

/// Allocated bytes under `root` (`st_blocks × 512`, like `du`): `lstat`
/// only (symlinks never followed), one filesystem, each hard-linked file
/// once. `Err(partial)` when the deadline passes first.
pub fn tree_bytes(root: &Path, deadline: Instant) -> Result<u64, u64> {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::symlink_metadata(root) else { return Ok(0) };
    let dev = meta.dev();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut total = meta.blocks().saturating_mul(512);
    if !meta.is_dir() {
        return Ok(total);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if Instant::now() >= deadline {
            return Err(total);
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.dev() != dev {
                continue;
            }
            if !m.is_dir() && m.nlink() > 1 && !seen.insert((m.dev(), m.ino())) {
                continue;
            }
            total = total.saturating_add(m.blocks().saturating_mul(512));
            if m.is_dir() {
                stack.push(e.path());
            }
        }
    }
    Ok(total)
}

/// `podman system df --format json` → the image store. Podman 4 and 5
/// give `[{"Type":"Images","RawSize":…,"RawReclaimable":…}, …]`.
pub fn parse_system_df(out: &str) -> Option<ImagesDisk> {
    let v: Value = serde_json::from_str(out.trim()).ok()?;
    let row = v.as_array()?.iter().find(|r| r["Type"].as_str().is_some_and(|t| t.eq_ignore_ascii_case("images")))?;
    let total = row["RawSize"].as_u64().or_else(|| row["Size"].as_u64())?;
    let unused = row["RawReclaimable"].as_u64().or_else(|| row["Reclaimable"].as_u64()).unwrap_or(0);
    Some(ImagesDisk { total_bytes: total, unused_bytes: unused.min(total) })
}

impl Engine {
    /// The `disk-usage` verb (§5.3).
    pub(crate) fn disk_usage(&self) -> R<Value> {
        let t0 = Instant::now();
        let deadline = t0 + Duration::from_secs(wire::DISK_USAGE_SECS);
        let mut out = DiskUsage::default();
        // Seeds and builds first: they are cheap and the sweep needs the list.
        if let Ok(rd) = std::fs::read_dir(self.seeds()) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !seed_hash_ok(&name) {
                    continue;
                }
                match tree_bytes(&e.path(), deadline) {
                    Ok(b) => {
                        out.seeds.insert(name, b);
                    }
                    Err(b) => {
                        out.seeds.insert(name, b);
                        out.partial = true;
                    }
                }
            }
        }
        match tree_bytes(&self.builds_dir(), deadline) {
            Ok(b) => out.builds = b,
            Err(b) => {
                out.builds = b;
                out.partial = true;
            }
        }
        out.images = self
            .run(podman_bin(), &["system", "df", "--format", "json"], None, 60)
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| parse_system_df(&String::from_utf8_lossy(&o.stdout)));
        let mut apps: Vec<(String, std::path::PathBuf)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.apps()) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if AppId::parse(&name).is_some() && e.path().is_dir() {
                    apps.push((name, e.path()));
                }
            }
        }
        apps.sort();
        for (name, dir) in apps {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs();
            if left < 2 {
                out.apps.insert(name, AppDisk { partial: true, error: Some("time budget used up".into()), ..AppDisk::default() });
                out.partial = true;
                continue;
            }
            let d = self.measure_app(&dir, left.min(60));
            out.partial |= d.partial;
            out.apps.insert(name, d);
        }
        out.took_ms = t0.elapsed().as_millis() as u64;
        serde_json::to_value(&out).map_err(|e| (codes::IO, e.to_string()))
    }

    fn measure_app(&self, dir: &Path, secs: u64) -> AppDisk {
        let mut argv = self.du_argv(secs);
        argv.push(dir.to_string_lossy().to_string());
        let (prog, args) = argv.split_first().expect("du argv");
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.run(prog, &args, None, secs + 5) {
            Err((_, e)) => AppDisk { partial: true, error: Some(e), ..AppDisk::default() },
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                let (mut d, complete) = parse_app_du(&text, dir);
                if !complete || !o.ok() {
                    d.partial = true;
                    let why = o.why();
                    d.error = Some(if why.is_empty() { "du stopped early".into() } else { why });
                }
                d
            }
        }
    }

    /// The `seeds-gc` verb (§9.2): remove exactly `remove`, never a Seed a
    /// run state or unit file names.
    pub(crate) fn seeds_gc(&self, remove: &[String]) -> R<Value> {
        if remove.len() > wire::MAX_SEEDS_GC {
            return Err((codes::TOO_LARGE, format!("at most {} Seeds per seeds-gc", wire::MAX_SEEDS_GC)));
        }
        if let Some(bad) = remove.iter().find(|h| !seed_hash_ok(h)) {
            return Err((codes::BAD_REQUEST, format!("seed hash: {}", bad.chars().take(80).collect::<String>())));
        }
        let in_use = self.seeds_in_use();
        let mut a = SeedsGcAnswer::default();
        let mut done: HashSet<&str> = HashSet::new();
        for h in remove {
            if !done.insert(h.as_str()) {
                continue;
            }
            let dir = self.seeds().join(h);
            if in_use.iter().any(|u| u.contains(h.as_str())) {
                a.skipped.push(h.clone());
                continue;
            }
            if !dir.is_dir() {
                a.absent.push(h.clone());
                continue;
            }
            let bytes = tree_bytes(&dir, Instant::now() + Duration::from_secs(30)).unwrap_or_else(|b| b);
            // Out of the store's name first (a Seed is whole or absent),
            // then delete. A leftover `.tmp-` is cleaned at the next start.
            let tmp = self.seeds().join(format!(".tmp-gc-{h}-{}", std::process::id()));
            // A Seed that can't be moved stays whole and is reported kept;
            // the rest of the list still goes.
            if std::fs::rename(&dir, &tmp).is_err() {
                a.skipped.push(h.clone());
                continue;
            }
            if let Err(e) = std::fs::remove_dir_all(&tmp) {
                eprintln!("k2-app-engine: seeds-gc: remove {}: {e} (cleaned at the next start)", tmp.display());
            }
            a.freed_bytes = a.freed_bytes.saturating_add(bytes);
            a.removed.push(h.clone());
        }
        serde_json::to_value(&a).map_err(|e| (codes::IO, e.to_string()))
    }

    /// The text of every run state and unit file (a Seed in use is named
    /// in one: the Quadlet's `Volume=<seeds>/<hash>:/app`, a build's or a
    /// stage's spec).
    fn seeds_in_use(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut read_tree = |root: &Path, depth: usize| {
            let mut stack = vec![(root.to_path_buf(), 0usize)];
            while let Some((d, n)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
                    if m.is_dir() && n < depth {
                        stack.push((e.path(), n + 1));
                    } else if m.is_file() && m.len() <= 4 << 20 {
                        if let Ok(t) = std::fs::read_to_string(e.path()) {
                            out.push(t);
                        }
                    }
                }
            }
        };
        read_tree(&self.quadlets(), 2);
        read_tree(&self.user_units(), 2);
        read_tree(&self.state().join("run"), 2);
        out
    }

    fn du_argv(&self, secs: u64) -> Vec<String> {
        match self.du_override() {
            Some(v) => v.to_vec(),
            None => app_du_argv(secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn du_output_becomes_parts() {
        let dir = Path::new("/var/lib/k2app/apps/jobs");
        let out = "4687500\t/var/lib/k2app/apps/jobs/data\n97656\t/var/lib/k2app/apps/jobs/cache\n48828\t/var/lib/k2app/apps/jobs/logs\n\
                   100\t/var/lib/k2app/apps/jobs/stage-d1\n12\t/var/lib/k2app/apps/jobs/data/deep\n4834100\t/var/lib/k2app/apps/jobs\n";
        let (d, complete) = parse_app_du(out, dir);
        assert!(complete);
        assert_eq!(d.data, 4687500 * 1024);
        assert_eq!(d.cache, 97656 * 1024);
        assert_eq!(d.logs, 48828 * 1024);
        // stage (100) + the folder's own (4834100 - 4834084 = 16).
        assert_eq!(d.other, (100 + 16) * 1024);
        assert_eq!(d.total(), 4834100 * 1024);
        // Cut off before the total line: the parts it printed, incomplete.
        let (d, complete) = parse_app_du("10\t/var/lib/k2app/apps/jobs/data\n", dir);
        assert!(!complete);
        assert_eq!(d.data, 10 * 1024);
    }

    #[test]
    fn system_df_json() {
        let out = r#"[{"Type":"Images","Total":3,"Active":1,"RawSize":4000000000,"RawReclaimable":1200000000,"Size":"4GB","Reclaimable":"1.2GB (30%)"},
                      {"Type":"Containers","Total":1,"Active":1,"RawSize":10,"RawReclaimable":0}]"#;
        assert_eq!(parse_system_df(out), Some(ImagesDisk { total_bytes: 4000000000, unused_bytes: 1200000000 }));
        assert_eq!(parse_system_df("not json"), None);
        assert_eq!(parse_system_df("[]"), None);
    }

    #[test]
    fn the_wrapped_du_runs_at_idle_priority_inside_the_namespace() {
        let v = app_du_argv(60);
        let i = v.iter().position(|a| a == "unshare").expect("podman unshare");
        assert_eq!(&v[i + 1..], &["timeout", "-s", "KILL", "60", "du", "-xk", "-d", "1"]);
        assert!(v[i - 1].ends_with("podman"), "{v:?}");
        if Path::new(NICE).is_file() {
            assert_eq!(&v[..3], &[NICE, "-n", "19"]);
        }
    }

    fn hash(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn answer(e: &Engine, r: k2_root_proto::engine::Request) -> Value {
        e.handle(r, &mut std::io::empty()).unwrap_or_else(|(c, m)| panic!("{c}: {m}"))
    }

    /// `disk-usage` end to end on a fake k2app home: each App's parts (with
    /// files owned by a mapped id when the test runs as root on Linux), the
    /// Seeds by hash, the build folder; a malformed App folder name is
    /// never measured.
    #[test]
    fn disk_usage_measures_apps_seeds_and_builds() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        let w = |p: &str, n: usize| {
            let f = home.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, vec![7u8; n]).unwrap();
        };
        w(&format!("seeds/{}/server.js", hash('a')), 64 * 1024);
        w("seeds/.tmp-half/x", 1024);
        w("engine/builds/b-1/out.tar", 32 * 1024);
        w("apps/jobs/data/db.sqlite", 256 * 1024);
        w("apps/jobs/cache/c", 16 * 1024);
        w("apps/jobs/logs/d1.log", 8 * 1024);
        w("apps/jobs/stage-d1/code.js", 4 * 1024);
        w("apps/Not_An_App/x", 4 * 1024);
        #[cfg(target_os = "linux")]
        if unsafe { libc::geteuid() } == 0 {
            // Files of the App's mapped ids (what `podman unshare` is for).
            let f = std::ffi::CString::new(home.join("apps/jobs/data/db.sqlite").to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::chown(f.as_ptr(), 100_000, 100_000) }, 0);
        }
        let e = Engine::new(home.clone(), "/tmp".into()).with_du_command(vec!["/usr/bin/du".into(), "-xk".into(), "-d".into(), "1".into()]);
        let v = answer(&e, k2_root_proto::engine::Request::DiskUsage);
        let a: DiskUsage = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(a.apps.keys().collect::<Vec<_>>(), vec!["jobs"], "{v}");
        let jobs = &a.apps["jobs"];
        assert!(!jobs.partial, "{v}");
        assert!(jobs.data >= 256 * 1024 && jobs.data < 1024 * 1024, "{v}");
        assert!(jobs.cache >= 16 * 1024 && jobs.logs >= 8 * 1024 && jobs.other >= 4 * 1024, "{v}");
        assert_eq!(a.seeds.keys().cloned().collect::<Vec<_>>(), vec![hash('a')], "only whole Seeds, by hash: {v}");
        assert!(a.seeds[&hash('a')] >= 64 * 1024);
        assert!(a.builds >= 32 * 1024, "{v}");
        assert!(!a.partial, "{v}");
        // The version answer announces both verbs (no protocol bump).
        let ver = e.handle(k2_root_proto::engine::Request::Version, &mut std::io::empty()).unwrap();
        assert_eq!(ver["protocol"], 3);
        assert_eq!(ver["verbs"], serde_json::json!(["disk-usage", "seeds-gc"]));
    }

    /// `seeds-gc` removes exactly what it is told, refuses a Seed a unit
    /// file still mounts, reports the absent, and refuses bad hashes.
    #[test]
    fn seeds_gc_removes_only_the_named_and_unused() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        for c in ['a', 'b', 'c'] {
            let f = home.join("seeds").join(hash(c)).join("s.js");
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, vec![1u8; 8192]).unwrap();
        }
        let e = Engine::new(home.clone(), "/tmp".into());
        let unit = e.quadlets().join("k2app-jobs-d1.container");
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, format!("[Container]\nVolume={}:/app:ro\n", home.join("seeds").join(hash('a')).display())).unwrap();
        let v = answer(&e, k2_root_proto::engine::Request::SeedsGc { remove: vec![hash('a'), hash('b'), hash('d'), hash('b')] });
        let a: SeedsGcAnswer = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(a.removed, vec![hash('b')], "{v}");
        assert_eq!(a.skipped, vec![hash('a')], "a unit mounts it: {v}");
        assert_eq!(a.absent, vec![hash('d')], "{v}");
        assert!(a.freed_bytes >= 8192);
        assert!(home.join("seeds").join(hash('a')).is_dir());
        assert!(!home.join("seeds").join(hash('b')).exists());
        assert!(home.join("seeds").join(hash('c')).is_dir(), "never named, never touched");
        let leftovers: Vec<_> = std::fs::read_dir(home.join("seeds")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
        assert!(leftovers.is_empty(), "no half-removed Seed left");
        for bad in ["../../etc".to_string(), "A".repeat(64), "a".repeat(63)] {
            let err = e.handle(k2_root_proto::engine::Request::SeedsGc { remove: vec![bad.clone()] }, &mut std::io::empty()).unwrap_err();
            assert_eq!(err.0, codes::BAD_REQUEST, "{bad}");
        }
        let too_many = vec![hash('c'); wire::MAX_SEEDS_GC + 1];
        assert_eq!(e.handle(k2_root_proto::engine::Request::SeedsGc { remove: too_many }, &mut std::io::empty()).unwrap_err().0, codes::TOO_LARGE);
        assert!(home.join("seeds").join(hash('c')).is_dir());
    }

    /// A one-filesystem walk counts allocated blocks, a hard link once and
    /// never follows a symlink.
    #[test]
    fn tree_bytes_counts_like_du() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("t");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a"), vec![1u8; 64 * 1024]).unwrap();
        std::fs::hard_link(root.join("sub/a"), root.join("b")).unwrap();
        std::os::unix::fs::symlink("/", root.join("link")).unwrap();
        let one = tree_bytes(&root, Instant::now() + Duration::from_secs(30)).unwrap();
        std::fs::remove_file(root.join("b")).unwrap();
        let again = tree_bytes(&root, Instant::now() + Duration::from_secs(30)).unwrap();
        assert_eq!(one, again, "the hard link counted once");
        assert!((64 * 1024..1024 * 1024).contains(&one), "{one}: the symlink to / was not followed");
        assert_eq!(tree_bytes(&root, Instant::now()), Err(std::fs::symlink_metadata(&root).map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.blocks() * 512
        }).unwrap()), "a passed deadline stops at once, partial");
    }
}
