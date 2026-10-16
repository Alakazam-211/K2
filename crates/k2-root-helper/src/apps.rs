//! `apps` namespace (protocol 6): App runtime box prep
//! (`prd-k2-app-runtime-v1` §10, vs-live 2 AR38/AR45).
//!
//! Box prep runs once per box and every verb is idempotent: running it
//! again changes nothing and reloads nothing. Each `on` has an `off`
//! (daemon-only), and `apps unprep` undoes all of it except the user, its
//! subordinate ids, its home and the distro packages (removing a user and
//! recycling its uid is its own attack class; App data stays owned).
//!
//! Inputs never come from argv beyond the validated tokens of
//! `k2_root_proto::apps`: the `k2app` uid is read from `/etc/passwd`, the
//! data disk from `/proc/self/mountinfo` and sysfs, the daemon unit from
//! `daemon.toml` (caller-proven). Every file the helper writes is canned
//! bytes from the proto crate, written without following links.
//!
//! Kernel files (`/proc`, `/sys`) are read with plain opens that follow
//! the kernel's own links (`/sys/dev/block/<maj:min>` is a link); nothing
//! else is. Every path still resolves beneath the context root.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use k2_root_proto::apps::*;
use k2_root_proto::engine::{tmpfiles_conf, user_unit, verify_engine_build, TMPFILES_FILE, USER_UNIT, USER_UNIT_DIR, USER_UNIT_FILE};
use k2_root_proto::keys::trusted_keys;
use k2_root_proto::paths::{HELPER_DIR, RUNUSER, STATE_DIR, SYSTEMCTL, SYSTEMD_SYSTEM_DIR, USERADD};
use k2_root_proto::sig::decode_payload;

use crate::caller::{self, Caller};
use crate::ctx::{Ctx, Fail, HResult, KindStr};
use crate::fsx::{Dir, Kind};

type J = serde_json::Value;
use serde_json::json;

// ── small file helpers ──────────────────────────────────────────────────

/// Split a fixed absolute path into (parent, name).
fn split(abs: &str) -> (&str, &str) {
    let i = abs.rfind('/').expect("absolute");
    (if i == 0 { "/" } else { &abs[..i] }, &abs[i + 1..])
}

/// A regular file under the root, never through a link. `None` = missing
/// (also when a parent is missing).
fn read_cfg(ctx: &Ctx, abs: &str, cap: usize) -> HResult<Option<Vec<u8>>> {
    let (parent, name) = split(abs);
    let d = match ctx.dir(parent) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Fail::io_err("open", e)),
    };
    d.read_file(OsStr::new(name), cap)
        .map_err(|e| Fail::io_err("read", e))
}

fn read_cfg_text(ctx: &Ctx, abs: &str) -> HResult<String> {
    Ok(read_cfg(ctx, abs, 4 * 1024 * 1024)?
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default())
}

/// A kernel file (`/proc/…`, `/sys/…`). Follows the kernel's links.
fn kread(ctx: &Ctx, abs: &str) -> Option<String> {
    debug_assert!(abs.starts_with("/proc/") || abs.starts_with("/sys/"));
    std::fs::read_to_string(ctx.p(abs)).ok()
}

fn exists(ctx: &Ctx, abs: &str) -> bool {
    let (parent, name) = split(abs);
    ctx.dir(parent)
        .ok()
        .and_then(|d| d.kind(OsStr::new(name)).ok())
        .is_some_and(|k| k != Kind::Missing)
}

/// Open (creating 0755 root if missing) a directory directly under an
/// existing fixed parent. A link or a file there is a refusal.
fn ensure_subdir(ctx: &Ctx, parent: &str, name: &str) -> HResult<Dir> {
    let base = ctx
        .dir(parent)
        .map_err(|e| Fail::io_err("open parent", e))?;
    let n = OsStr::new(name);
    match base.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {
            base.mkdir(n, 0o755).map_err(|e| Fail::io_err("mkdir", e))?;
            let d = base.sub(n).map_err(|e| Fail::io_err("open", e))?;
            if let Some((u, g)) = ctx.root_owner() {
                d.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
            }
            Ok(d)
        }
        Kind::Dir => base.sub(n).map_err(|e| Fail::io_err("open", e)),
        _ => Err(Fail::state(
            "refused: a link or a file where K2 expects a directory",
        )),
    }
}

/// Write canned bytes to `dir/name` unless they are already there.
/// `Ok(true)` when something changed.
fn put(ctx: &Ctx, dir: &Dir, name: &str, bytes: &[u8]) -> HResult<bool> {
    let n = OsStr::new(name);
    match dir.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing | Kind::File => {}
        _ => {
            return Err(Fail::state(
                "refused: a link or a directory where K2 writes a file",
            ))
        }
    }
    let same = dir
        .read_file(n, 256 * 1024)
        .map_err(|e| Fail::io_err("read", e))?
        .is_some_and(|b| b == bytes);
    if !same {
        dir.write_atomic(n, bytes, 0o644, ctx.root_owner())
            .map_err(|e| Fail::io_err("write", e))?;
    }
    Ok(!same)
}

/// Remove `parent/dname/name` if it is there (a planted link is removed,
/// never followed). `Ok(true)` when something was removed.
fn unput(ctx: &Ctx, parent: &str, dname: Option<&str>, name: &str) -> HResult<bool> {
    let base = match ctx.dir(parent) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Fail::io_err("open", e)),
    };
    let dir = match dname {
        None => base,
        Some(d) => match base
            .kind(OsStr::new(d))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Missing => return Ok(false),
            Kind::Dir => base
                .sub(OsStr::new(d))
                .map_err(|e| Fail::io_err("open", e))?,
            _ => {
                return Err(Fail::state(
                    "refused: a link or a file where K2 expects a directory",
                ))
            }
        },
    };
    let n = OsStr::new(name);
    match dir.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => Ok(false),
        Kind::File | Kind::Symlink => {
            dir.unlink(n).map_err(|e| Fail::io_err("remove", e))?;
            Ok(true)
        }
        _ => Err(Fail::state("refused: a directory where K2 expects a file")),
    }
}

fn reload(ctx: &Ctx) -> HResult<()> {
    ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None).map(|_| ())
}

// ── the k2app user ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pw {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

pub fn parse_passwd(text: &str) -> Vec<Pw> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() != 7 {
                return None;
            }
            Some(Pw {
                name: f[0].to_string(),
                uid: f[2].parse().ok()?,
                gid: f[3].parse().ok()?,
                home: f[5].to_string(),
                shell: f[6].to_string(),
            })
        })
        .collect()
}

fn passwd(ctx: &Ctx) -> HResult<Vec<Pw>> {
    Ok(parse_passwd(&read_cfg_text(ctx, PASSWD_FILE)?))
}

const NOLOGIN: &[&str] = &[
    "/usr/sbin/nologin",
    "/sbin/nologin",
    "/bin/false",
    "/usr/bin/false",
];

/// Why an existing `k2app` is not the user box prep makes, if it isn't
/// (uid collisions, a login shell, a password, a privileged group).
fn shape_problem(ctx: &Ctx, pw: &Pw, all: &[Pw]) -> HResult<Option<&'static str>> {
    if pw.uid == 0 || pw.gid == 0 {
        return Ok(Some("k2app has uid or gid 0"));
    }
    if pw.uid >= 1000 {
        return Ok(Some("k2app is not a system user (uid >= 1000)"));
    }
    if all.iter().filter(|p| p.uid == pw.uid).count() > 1 {
        return Ok(Some("another user shares k2app's uid"));
    }
    if let Ok(daemons) = caller::load_daemons(ctx) {
        if daemons
            .iter()
            .any(|d| all.iter().any(|p| p.name == d.user && p.uid == pw.uid))
        {
            return Ok(Some("k2app's uid is the K2 daemon user's"));
        }
    }
    if !NOLOGIN.contains(&pw.shell.as_str()) {
        return Ok(Some("k2app has a login shell"));
    }
    if pw.home != APP_HOME {
        return Ok(Some("k2app's home is not /var/lib/k2app"));
    }
    // Primary or supplementary member of a group that gives more than its
    // own files.
    let groups = read_cfg_text(ctx, GROUP_FILE)?;
    for l in groups.lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() != 4 || !PRIVILEGED_GROUPS.contains(&f[0]) {
            continue;
        }
        let gid_hit = f[2].parse::<u32>().is_ok_and(|g| g == pw.gid);
        if gid_hit || f[3].split(',').any(|m| m == APP_USER) {
            return Ok(Some("k2app is in a privileged group"));
        }
    }
    // A usable password (or an empty one) is a login.
    let shadow = read_cfg_text(ctx, SHADOW_FILE)?;
    for l in shadow.lines() {
        let mut f = l.split(':');
        if f.next() == Some(APP_USER) {
            let h = f.next().unwrap_or("");
            if !(h.starts_with('!') || h.starts_with('*')) {
                return Ok(Some("k2app has a password"));
            }
        }
    }
    Ok(None)
}

/// The proven `k2app` entry: exists and has the right shape.
fn app_user(ctx: &Ctx) -> HResult<Pw> {
    let all = passwd(ctx)?;
    let pw = all
        .iter()
        .find(|p| p.name == APP_USER)
        .cloned()
        .ok_or_else(|| {
            Fail::state("box_not_prepared: the k2app user does not exist (apps ensure-user)")
        })?;
    if let Some(why) = shape_problem(ctx, &pw, &all)? {
        return Err(Fail::state(format!("refused: {why}")));
    }
    Ok(pw)
}

/// `/var/lib/k2app`: 0750, owned by k2app (fixes the mode useradd left).
fn fix_home(ctx: &Ctx, pw: &Pw) -> HResult<()> {
    let (parent, name) = split(APP_HOME);
    let base = ctx
        .dir(parent)
        .map_err(|e| Fail::io_err("open /var/lib", e))?;
    let n = OsStr::new(name);
    match base.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {
            base.mkdir(n, 0o750)
                .map_err(|e| Fail::io_err("mkdir home", e))?;
        }
        Kind::Dir => {}
        _ => return Err(Fail::state("refused: /var/lib/k2app is not a directory")),
    }
    let home = base.sub(n).map_err(|e| Fail::io_err("open home", e))?;
    let st = home.stat_self().map_err(|e| Fail::io_err("stat home", e))?;
    if ctx.real_owners && st.st_uid != pw.uid && st.st_uid != 0 {
        return Err(Fail::state(
            "refused: /var/lib/k2app belongs to another user",
        ));
    }
    if ctx.real_owners {
        home.fchown(pw.uid, pw.gid)
            .map_err(|e| Fail::io_err("chown home", e))?;
    }
    home.fchmod(0o750)
        .map_err(|e| Fail::io_err("chmod home", e))
}

pub fn ensure_user(ctx: &Ctx) -> HResult<J> {
    let all = passwd(ctx)?;
    let created = if all.iter().any(|p| p.name == APP_USER) {
        false
    } else {
        // A home with no user is data from an older k2app (another uid):
        // never adopt it silently.
        if exists(ctx, APP_HOME) {
            return Err(Fail::state(
                "refused: /var/lib/k2app exists but the k2app user does not (move it aside first)",
            ));
        }
        ctx.run_ok(
            USERADD,
            &[
                "--system",
                "--user-group",
                "--home-dir",
                APP_HOME,
                "--create-home",
                "--shell",
                APP_SHELL,
                "--comment",
                "K2 Apps",
                APP_USER,
            ],
            None,
        )?;
        true
    };
    let pw = app_user(ctx)?;
    fix_home(ctx, &pw)?;
    Ok(
        json!({"user": APP_USER, "uid": pw.uid, "gid": pw.gid, "home": APP_HOME, "created": created}),
    )
}

// ── the process exception's user (k2appx) ───────────────────────────────

/// Why an existing `k2appx` is not the user box prep makes. Same rules as
/// `k2app`, plus: never `k2app`'s uid (the owner of every container's user
/// namespace) and its home is the state root systemd makes.
fn process_user_problem(ctx: &Ctx, pw: &Pw, all: &[Pw]) -> HResult<Option<&'static str>> {
    if pw.uid == 0 || pw.gid == 0 {
        return Ok(Some("k2appx has uid or gid 0"));
    }
    if pw.uid >= 1000 {
        return Ok(Some("k2appx is not a system user (uid >= 1000)"));
    }
    if all.iter().filter(|p| p.uid == pw.uid).count() > 1 {
        return Ok(Some("another user shares k2appx's uid"));
    }
    if all.iter().any(|p| p.name == APP_USER && (p.uid == pw.uid || p.gid == pw.gid)) {
        return Ok(Some("k2appx shares k2app's uid or group"));
    }
    if let Ok(daemons) = caller::load_daemons(ctx) {
        if daemons
            .iter()
            .any(|d| all.iter().any(|p| p.name == d.user && p.uid == pw.uid))
        {
            return Ok(Some("k2appx's uid is the K2 daemon user's"));
        }
    }
    if !NOLOGIN.contains(&pw.shell.as_str()) {
        return Ok(Some("k2appx has a login shell"));
    }
    if pw.home != PROCESS_HOME {
        return Ok(Some("k2appx's home is not /var/lib/k2appx"));
    }
    let groups = read_cfg_text(ctx, GROUP_FILE)?;
    for l in groups.lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() != 4 {
            continue;
        }
        let member = f[2].parse::<u32>().is_ok_and(|g| g == pw.gid)
            || f[3].split(',').any(|m| m == PROCESS_USER);
        if member && (PRIVILEGED_GROUPS.contains(&f[0]) || f[0] == APP_USER) {
            return Ok(Some("k2appx is in a privileged group"));
        }
    }
    let shadow = read_cfg_text(ctx, SHADOW_FILE)?;
    for l in shadow.lines() {
        let mut f = l.split(':');
        if f.next() == Some(PROCESS_USER) {
            let h = f.next().unwrap_or("");
            if !(h.starts_with('!') || h.starts_with('*')) {
                return Ok(Some("k2appx has a password"));
            }
        }
    }
    Ok(None)
}

/// The proven `k2appx` entry.
fn process_user(ctx: &Ctx) -> HResult<Pw> {
    let all = passwd(ctx)?;
    let pw = all
        .iter()
        .find(|p| p.name == PROCESS_USER)
        .cloned()
        .ok_or_else(|| {
            Fail::state("box_not_prepared: the k2appx user does not exist (apps ensure-process-user)")
        })?;
    if let Some(why) = process_user_problem(ctx, &pw, &all)? {
        return Err(Fail::state(format!("refused: {why}")));
    }
    Ok(pw)
}

/// `apps ensure-process-user` (open): the `process` exception's own system
/// user. No home is made (systemd makes `/var/lib/k2appx/<id>` per App).
pub fn ensure_process_user(ctx: &Ctx) -> HResult<J> {
    app_user(ctx)?;
    let all = passwd(ctx)?;
    let created = if all.iter().any(|p| p.name == PROCESS_USER) {
        false
    } else {
        ctx.run_ok(
            USERADD,
            &[
                "--system",
                "--user-group",
                "--home-dir",
                PROCESS_HOME,
                "--no-create-home",
                "--shell",
                APP_SHELL,
                "--comment",
                "K2 process Apps",
                PROCESS_USER,
            ],
            None,
        )?;
        true
    };
    let pw = process_user(ctx)?;
    Ok(json!({"user": PROCESS_USER, "uid": pw.uid, "gid": pw.gid, "home": PROCESS_HOME, "created": created}))
}

fn process_user_json(ctx: &Ctx) -> J {
    let Ok(all) = passwd(ctx) else { return json!({"exists": false}) };
    match all.iter().find(|p| p.name == PROCESS_USER) {
        None => json!({"exists": false}),
        Some(p) => {
            let problem = process_user_problem(ctx, p, &all).ok().flatten();
            json!({"exists": true, "uid": p.uid, "ok": problem.is_none(), "problem": problem})
        }
    }
}

// ── subordinate ids ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    pub count: u64,
}

impl Range {
    fn end(&self) -> u64 {
        self.start + self.count
    }
    fn overlaps(&self, o: &Range) -> bool {
        self.start < o.end() && o.start < self.end()
    }
}

/// `name:start:count` lines; the owner is a name or a numeric uid.
pub fn parse_subid(text: &str) -> Vec<(String, Range)> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split(':').collect();
            if f.len() != 3 {
                return None;
            }
            let start: u64 = f[1].parse().ok()?;
            let count: u64 = f[2].parse().ok()?;
            (count > 0).then(|| (f[0].to_string(), Range { start, count }))
        })
        .collect()
}

fn is_app_owner(owner: &str, uid: u32) -> bool {
    owner == APP_USER || owner == uid.to_string()
}

/// k2app's ranges in one file, after checking none of them meets another
/// owner's range or a real uid/gid.
fn app_ranges(entries: &[(String, Range)], uid: u32, ids: &[u32]) -> HResult<Vec<Range>> {
    let mine: Vec<Range> = entries
        .iter()
        .filter(|(o, _)| is_app_owner(o, uid))
        .map(|(_, r)| *r)
        .collect();
    for m in &mine {
        if m.end() > SUBID_LIMIT {
            return Err(Fail::state(
                "refused: k2app's subordinate range runs past 2^32-1",
            ));
        }
        if entries
            .iter()
            .any(|(o, r)| !is_app_owner(o, uid) && r.overlaps(m))
        {
            return Err(Fail::state(
                "refused: k2app's subordinate range overlaps another user's",
            ));
        }
        if ids
            .iter()
            .any(|&i| (i as u64) >= m.start && (i as u64) < m.end())
        {
            return Err(Fail::state(
                "refused: k2app's subordinate range contains a real uid or gid",
            ));
        }
    }
    Ok(mine)
}

/// The first 65536-aligned block at or above the floor, above every range
/// in both files and every real id, that fits below 2^32-1.
pub fn next_block(all: &[Range], ids: &[u32]) -> Option<Range> {
    let top = all.iter().map(Range::end).max().unwrap_or(0);
    let top = top.max(ids.iter().map(|&i| i as u64 + 1).max().unwrap_or(0));
    let start = top.max(SUBID_FLOOR).div_ceil(SUBID_ALIGN) * SUBID_ALIGN;
    let r = Range {
        start,
        count: SUBID_COUNT,
    };
    (r.end() <= SUBID_LIMIT).then_some(r)
}

fn real_ids(ctx: &Ctx, all: &[Pw]) -> HResult<Vec<u32>> {
    let mut ids: Vec<u32> = all.iter().flat_map(|p| [p.uid, p.gid]).collect();
    for l in read_cfg_text(ctx, GROUP_FILE)?.lines() {
        if let Some(g) = l.split(':').nth(2).and_then(|g| g.parse().ok()) {
            ids.push(g);
        }
    }
    // 4294967295 is "no id", never a real one.
    ids.retain(|&i| i != u32::MAX);
    Ok(ids)
}

/// Every subuid entry, every subgid entry, then k2app's uid and gid ranges.
type SubidState = (
    Vec<(String, Range)>,
    Vec<(String, Range)>,
    Vec<Range>,
    Vec<Range>,
);

fn subid_state(ctx: &Ctx, pw: &Pw, ids: &[u32]) -> HResult<SubidState> {
    let su = parse_subid(&read_cfg_text(ctx, SUBUID_FILE)?);
    let sg = parse_subid(&read_cfg_text(ctx, SUBGID_FILE)?);
    let mu = app_ranges(&su, pw.uid, ids)?;
    let mg = app_ranges(&sg, pw.uid, ids)?;
    Ok((su, sg, mu, mg))
}

fn ranges_json(r: &[Range]) -> J {
    J::Array(
        r.iter()
            .map(|r| json!({"start": r.start, "count": r.count}))
            .collect(),
    )
}

pub fn subid(ctx: &Ctx) -> HResult<J> {
    let pw = app_user(ctx)?;
    let all = passwd(ctx)?;
    let ids = real_ids(ctx, &all)?;
    let (su, sg, mu, mg) = subid_state(ctx, &pw, &ids)?;
    if !mu.is_empty() && !mg.is_empty() {
        return Ok(
            json!({"changed": false, "subuid": ranges_json(&mu), "subgid": ranges_json(&mg)}),
        );
    }
    if !mu.is_empty() || !mg.is_empty() {
        return Err(Fail::state(
            "refused: k2app has subordinate uids or gids but not both (fix /etc/subuid and /etc/subgid by hand)",
        ));
    }
    let every: Vec<Range> = su.iter().chain(sg.iter()).map(|(_, r)| *r).collect();
    let block = next_block(&every, &ids)
        .ok_or_else(|| Fail::state("refused: no free subordinate id block"))?;
    let spec = format!("{}-{}", block.start, block.end() - 1);
    ctx.run_ok(
        USERMOD,
        &["--add-subuids", &spec, "--add-subgids", &spec, APP_USER],
        None,
    )?;
    let (_, _, mu, mg) = subid_state(ctx, &pw, &ids)?;
    if !mu.contains(&block) || !mg.contains(&block) {
        return Err(Fail::io("usermod did not record the subordinate range"));
    }
    Ok(json!({"changed": true, "subuid": ranges_json(&mu), "subgid": ranges_json(&mg)}))
}

// ── linger and delegation ───────────────────────────────────────────────

fn linger_on_disk(ctx: &Ctx) -> bool {
    exists(ctx, &format!("{LINGER_DIR}/{APP_USER}"))
}

pub fn linger(ctx: &Ctx, on: bool) -> HResult<J> {
    if on {
        app_user(ctx)?;
    }
    let was = linger_on_disk(ctx);
    if was != on {
        let verb = if on {
            "enable-linger"
        } else {
            "disable-linger"
        };
        ctx.run_ok(LOGINCTL, &[verb, APP_USER], None)?;
    }
    Ok(json!({"linger": on, "changed": was != on}))
}

fn user_unit_dropin_dir(uid: u32) -> String {
    format!("user@{uid}.service.d")
}

fn unit_active(ctx: &Ctx, unit: &str) -> bool {
    ctx.exec
        .run(
            SYSTEMCTL,
            &["is-active".into(), "--quiet".into(), unit.into()],
            None,
        )
        .is_ok_and(|o| o.status == 0)
}

pub fn delegate(ctx: &Ctx, on: bool) -> HResult<J> {
    let pw = app_user(ctx)?;
    let dname = user_unit_dropin_dir(pw.uid);
    let changed = if on {
        let d = ensure_subdir(ctx, SYSTEMD_SYSTEM_DIR, &dname)?;
        put(ctx, &d, DELEGATE_DROPIN_FILE, delegate_dropin().as_bytes())?
    } else {
        unput(ctx, SYSTEMD_SYSTEM_DIR, Some(&dname), DELEGATE_DROPIN_FILE)?
    };
    if changed {
        reload(ctx)?;
    }
    // A running user manager keeps its old delegation until it restarts;
    // box prep orders `delegate on` before `linger on`, so a first prep
    // never needs that. Say so instead of killing k2app's processes.
    let unit = format!("user@{}.service", pw.uid);
    let restart_needed = changed && unit_active(ctx, &unit);
    Ok(json!({"delegate": on, "changed": changed, "restartNeeded": restart_needed}))
}

// ── io.cost on the data disk ────────────────────────────────────────────

/// `maj:min` of the filesystem holding `path` (longest mount-point prefix
/// in `/proc/self/mountinfo`).
pub fn mount_dev(mountinfo: &str, path: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for l in mountinfo.lines() {
        let f: Vec<&str> = l.split(' ').collect();
        if f.len() < 5 {
            continue;
        }
        let (dev, mp) = (f[2], f[4]);
        let under = mp == "/" || path == mp || path.starts_with(&format!("{mp}/"));
        if under && best.as_ref().is_none_or(|(n, _)| mp.len() >= *n) {
            best = Some((mp.len(), dev.to_string()));
        }
    }
    best.map(|(_, d)| d)
}

fn devnum_ok(d: &str) -> bool {
    d.split_once(':').is_some_and(|(a, b)| {
        !a.is_empty()
            && !b.is_empty()
            && a.len() <= 6
            && b.len() <= 8
            && a.bytes().all(|c| c.is_ascii_digit())
            && b.bytes().all(|c| c.is_ascii_digit())
    })
}

/// The whole disks under a block device: a partition's parent, a dm/md
/// device's slaves (recursively). io.cost works on request-based disks.
pub fn leaf_disks(ctx: &Ctx, devnum: &str) -> Vec<String> {
    let sys = match std::fs::canonicalize(ctx.p("/sys")) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let start = ctx.p(&format!("/sys/dev/block/{devnum}"));
    walk_disk(&sys, &start, 0, &mut out);
    out.sort();
    out.dedup();
    out
}

fn walk_disk(sys: &Path, link: &Path, depth: u32, out: &mut Vec<String>) {
    if depth > 4 {
        return;
    }
    let Ok(mut p) = std::fs::canonicalize(link) else {
        return;
    };
    if !p.starts_with(sys) {
        return;
    }
    if p.join("partition").exists() {
        let Some(parent) = p.parent() else { return };
        p = parent.to_path_buf();
    }
    let slaves: Vec<PathBuf> = std::fs::read_dir(p.join("slaves"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    if !slaves.is_empty() {
        for s in slaves {
            walk_disk(sys, &s, depth + 1, out);
        }
        return;
    }
    if let Ok(dev) = std::fs::read_to_string(p.join("dev")) {
        let dev = dev.trim();
        if devnum_ok(dev) {
            out.push(dev.to_string());
        }
    }
}

/// `maj:min` → enabled, from `io.cost.qos` lines.
pub fn parse_qos(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let dev = it.next()?;
            let en = it.find_map(|kv| kv.strip_prefix("enable="))?;
            devnum_ok(dev).then(|| (dev.to_string(), en == "1"))
        })
        .collect()
}

fn data_disks(ctx: &Ctx) -> Vec<String> {
    let mi = kread(ctx, "/proc/self/mountinfo").unwrap_or_default();
    let mut devs = Vec::new();
    for p in [APP_HOME, "/var/lib"] {
        if let Some(d) = mount_dev(&mi, p) {
            if devnum_ok(&d) && !devs.contains(&d) {
                devs.push(d);
            }
        }
    }
    let mut disks: Vec<String> = devs.iter().flat_map(|d| leaf_disks(ctx, d)).collect();
    disks.sort();
    disks.dedup();
    disks
}

fn io_cost_json(ctx: &Ctx) -> J {
    let qos = kread(ctx, IO_COST_QOS);
    let disks = data_disks(ctx);
    let table = qos.as_deref().map(parse_qos).unwrap_or_default();
    let devices: Vec<J> = disks
        .iter()
        .map(|d| json!({"dev": d, "enabled": table.iter().any(|(x, e)| x == d && *e)}))
        .collect();
    let all_on = !disks.is_empty() && devices.iter().all(|d| d["enabled"] == true);
    json!({"available": qos.is_some(), "devices": devices, "enabled": qos.is_some() && all_on})
}

pub fn io_cost(ctx: &Ctx, on: bool) -> HResult<J> {
    let Some(qos) = kread(ctx, IO_COST_QOS) else {
        // Not the host's cgroup root (a container), or a kernel without
        // blk-iocost: nothing to do, and the doctor says so.
        return Ok(json!({"available": false, "changed": false, "devices": []}));
    };
    let disks = data_disks(ctx);
    if disks.is_empty() {
        return Err(Fail::state(
            "io.cost: could not find the disk under /var/lib/k2app",
        ));
    }
    let table = parse_qos(&qos);
    let cg = ctx
        .dir(CGROUP_ROOT)
        .map_err(|e| Fail::io_err("open cgroup root", e))?;
    let mut changed = false;
    for d in &disks {
        let now = table.iter().any(|(x, e)| x == d && *e);
        if now != on {
            let line = format!("{d} enable={}\n", if on { 1 } else { 0 });
            cg.write_existing(OsStr::new("io.cost.qos"), line.as_bytes())
                .map_err(|e| Fail::io(format!("io.cost.qos: {}", e.kind_str())))?;
            changed = true;
        }
    }
    let mut v = io_cost_json(ctx);
    v["changed"] = json!(changed);
    Ok(v)
}

// ── reserved ports ──────────────────────────────────────────────────────

/// `a-b,c,d-e` → ranges (empty string = none).
pub fn parse_port_list(s: &str) -> Option<Vec<(u32, u32)>> {
    let s = s.trim();
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(',')
        .map(|p| {
            let (a, b) = p.split_once('-').unwrap_or((p, p));
            let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            (a <= b && b <= 65_535).then_some((a, b))
        })
        .collect()
}

fn format_port_list(v: &[(u32, u32)]) -> String {
    v.iter()
        .map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn ours() -> (u32, u32) {
    (RESERVED_PORTS.0 as u32, RESERVED_PORTS.1 as u32)
}

fn covered(v: &[(u32, u32)]) -> bool {
    let (lo, hi) = ours();
    (lo..=hi).all(|p| v.iter().any(|(a, b)| *a <= p && p <= *b))
}

fn reserved_json(ctx: &Ctx) -> J {
    let rt = kread(ctx, RESERVED_PORTS_PROC);
    let list = rt.as_deref().and_then(parse_port_list);
    let file = read_cfg(ctx, SYSCTL_FILE, 64 * 1024).ok().flatten();
    json!({
        "range": format!("{}-{}", RESERVED_PORTS.0, RESERVED_PORTS.1),
        "runtime": rt.map(|s| s.trim().to_string()),
        "covered": list.as_deref().is_some_and(covered),
        "file": file.as_deref() == Some(sysctl_conf().as_bytes()),
    })
}

pub fn reserve_ports(ctx: &Ctx, on: bool) -> HResult<J> {
    let (parent, name) = split(SYSCTL_FILE);
    let mut changed = if on {
        let d = ensure_subdir(ctx, "/etc", split(parent).1)?;
        put(ctx, &d, name, sysctl_conf().as_bytes())?
    } else {
        unput(ctx, "/etc", Some(split(parent).1), name)?
    };
    let cur = kread(ctx, RESERVED_PORTS_PROC)
        .ok_or_else(|| Fail::state("no /proc/sys/net/ipv4/ip_local_reserved_ports"))?;
    let mut list =
        parse_port_list(&cur).ok_or_else(|| Fail::state("ip_local_reserved_ports unreadable"))?;
    let want = if on {
        if covered(&list) {
            None
        } else {
            list.push(ours());
            list.sort();
            Some(list)
        }
    } else if list.contains(&ours()) {
        list.retain(|r| *r != ours());
        Some(list)
    } else {
        None
    };
    if let Some(v) = want {
        let (p, n) = split(RESERVED_PORTS_PROC);
        let d = ctx
            .dir(p)
            .map_err(|e| Fail::io_err("open /proc/sys/net/ipv4", e))?;
        let text = format!("{}\n", format_port_list(&v));
        d.write_existing(OsStr::new(n), text.as_bytes())
            .map_err(|e| Fail::io(format!("ip_local_reserved_ports: {}", e.kind_str())))?;
        changed = true;
    }
    let mut v = reserved_json(ctx);
    v["changed"] = json!(changed);
    Ok(v)
}

// ── nftables ────────────────────────────────────────────────────────────

fn nft_list(ctx: &Ctx) -> Option<String> {
    let out = ctx
        .exec
        .run(
            NFT,
            &[
                "list".into(),
                "table".into(),
                "inet".into(),
                NFT_TABLE.into(),
            ],
            None,
        )
        .ok()?;
    (out.status == 0).then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn nft_json(ctx: &Ctx, uid: Option<u32>) -> J {
    let listed = nft_list(ctx);
    let current = match (&listed, uid) {
        (Some(t), Some(u)) => {
            t.contains(&format!("chain {NFT_CHAIN}")) && t.contains(&format!("meta skuid {u} "))
        }
        _ => false,
    };
    json!({"table": listed.is_some(), "current": current})
}

pub fn nft_apply(ctx: &Ctx, gw: PortRange, px: PortRange, db: PortRange) -> HResult<J> {
    let pw = app_user(ctx)?;
    let apps = load_nft_apps(ctx)?;
    let rules = app_rules(ctx, pw.uid, &apps, gw);
    // Process Apps (k2appx) get the same chain when their user exists.
    let pu = process_user(ctx).ok().map(|p| p.uid);
    let text = nft_ruleset(pw.uid, pu, gw, px, db, &rules);
    ctx.run_ok(NFT, &["-f", "-"], Some(text.as_bytes()))?;
    let mut v = nft_json(ctx, Some(pw.uid));
    v["ranges"] = json!({"gateway": gw.token(), "proxy": px.token(), "db": db.token()});
    v["appRules"] = json!(rules.len());
    Ok(v)
}

// ── per-App gateway rules (slice 3) ─────────────────────────────────────

/// `apps-nft.json`: App id → its gateway port, and App id → its approved
/// box services' host ports (0.46.0; absent in older files).
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NftApps {
    pub apps: std::collections::BTreeMap<String, u16>,
    #[serde(default, rename = "boxPorts", skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub box_ports: std::collections::BTreeMap<String, Vec<u16>>,
}

fn load_nft_apps(ctx: &Ctx) -> HResult<NftApps> {
    match read_cfg(ctx, NFT_APPS_STATE, 256 * 1024)? {
        None => Ok(NftApps::default()),
        Some(b) => serde_json::from_slice(&b).map_err(|_| Fail::state("apps-nft.json unreadable")),
    }
}

fn save_nft_apps(ctx: &Ctx, s: &NftApps) -> HResult<()> {
    let d = ensure_subdir(ctx, split(STATE_DIR).0, split(STATE_DIR).1)?;
    let bytes = serde_json::to_vec_pretty(s).map_err(|_| Fail::io("encode apps-nft.json"))?;
    put(ctx, &d, split(NFT_APPS_STATE).1, &bytes).map(|_| ())
}

/// One accept per (App, cgroup that exists now). Ids and ports come from
/// the state file, re-validated; cgroup paths are built, never read.
fn app_rules(ctx: &Ctx, uid: u32, s: &NftApps, gw: PortRange) -> Vec<AppRule> {
    let mut out = Vec::new();
    for (id, port) in &s.apps {
        let Some(id) = AppId::parse(id) else { continue };
        if !(gw.lo..=gw.hi).contains(port) {
            continue;
        }
        // Re-validated: a doctored state file never opens a refused port.
        let mut box_ports: Vec<u16> = s.box_ports.get(id.as_str()).cloned().unwrap_or_default();
        box_ports.retain(|p| k2_root_proto::apps::box_port_ok(*p));
        box_ports.truncate(k2_root_proto::apps::MAX_BOX_SERVICES);
        for (cg, level) in app_cgroups(uid, &id) {
            if ctx.p(&format!("{CGROUP_ROOT}/{cg}")).is_dir() {
                out.push(AppRule { cgroup: cg, level, gw_port: *port, box_ports: box_ports.clone() });
            }
        }
    }
    out
}

fn nft_table_current(ctx: &Ctx, uid: u32) -> bool {
    nft_list(ctx).is_some_and(|t| t.contains(&format!("chain {NFT_CHAIN}")) && t.contains(&format!("meta skuid {uid} ")))
}

fn reload_app_rules(ctx: &Ctx, uid: u32, s: &NftApps) -> HResult<usize> {
    if !nft_table_current(ctx, uid) {
        return Err(Fail::state("box_not_prepared: the k2app nftables table is missing or old (apps nft apply)"));
    }
    let rules = app_rules(ctx, uid, s, GATEWAY_PORTS);
    ctx.run_ok(NFT, &["-f", "-"], Some(nft_apps_script(&rules).as_bytes()))?;
    Ok(rules.len())
}

/// `apps nft app <id> <gw> [<box-port>…]` (daemon): record the App's
/// gateway port and its box services' host ports (the list replaces the
/// previous one) and reload the per-App chain. A port another App holds
/// is refused.
pub fn nft_app(ctx: &Ctx, id: &AppId, gw: u16, box_ports: &[u16]) -> HResult<J> {
    let pw = app_user(ctx)?;
    let mut s = load_nft_apps(ctx)?;
    if let Some((other, _)) = s.apps.iter().find(|(a, p)| **p == gw && a.as_str() != id.as_str()) {
        return Err(Fail::state(format!("refused: gateway port {gw} belongs to App {other}")));
    }
    if box_ports.len() > k2_root_proto::apps::MAX_BOX_SERVICES || !box_ports.iter().all(|p| k2_root_proto::apps::box_port_ok(*p)) {
        return Err(Fail::state("refused: a box service port is on the deny-list"));
    }
    let mut changed = s.apps.insert(id.as_str().to_string(), gw) != Some(gw);
    let before = s.box_ports.get(id.as_str()).cloned().unwrap_or_default();
    if before != box_ports {
        changed = true;
        if box_ports.is_empty() {
            s.box_ports.remove(id.as_str());
        } else {
            s.box_ports.insert(id.as_str().to_string(), box_ports.to_vec());
        }
    }
    if changed {
        save_nft_apps(ctx, &s)?;
    }
    let n = reload_app_rules(ctx, pw.uid, &s)?;
    let matched = app_cgroups(pw.uid, id)
        .iter()
        .filter(|(cg, _)| ctx.p(&format!("{CGROUP_ROOT}/{cg}")).is_dir())
        .count();
    Ok(json!({"app": id.as_str(), "gwPort": gw, "boxPorts": box_ports, "changed": changed, "rules": n, "cgroupsNow": matched}))
}

pub fn nft_app_remove(ctx: &Ctx, id: &AppId) -> HResult<J> {
    let pw = app_user(ctx)?;
    let mut s = load_nft_apps(ctx)?;
    let had_box = s.box_ports.remove(id.as_str()).is_some();
    let changed = s.apps.remove(id.as_str()).is_some() || had_box;
    if changed {
        save_nft_apps(ctx, &s)?;
    }
    let n = if nft_table_current(ctx, pw.uid) {
        reload_app_rules(ctx, pw.uid, &s)?
    } else {
        0
    };
    Ok(json!({"app": id.as_str(), "changed": changed, "rules": n}))
}

// ── guarded Apps' output (slice 3) ──────────────────────────────────────

/// Largest log answer.
const LOGS_CAP: usize = 1024 * 1024;

/// `apps logs <id> <deployment> <lines>` (daemon): the unit's lines from
/// the journal, matched on k2app's user manager AND the deployment's user unit (two
/// fields journald ANDs). Nothing from argv but validated tokens.
pub fn logs(ctx: &Ctx, id: &AppId, deployment: &str, lines: u32) -> HResult<J> {
    let pw = app_user(ctx)?;
    let unit = format!("{}.service", k2_root_proto::engine::unit_stem(id, deployment));
    let n = lines.to_string();
    // `_SYSTEMD_OWNER_UID`: the user manager the unit runs in (journald
    // derives it from the cgroup). Not `_UID`: an App writes as its own
    // mapped id (its slot), never as k2app.
    let uid = format!("_SYSTEMD_OWNER_UID={}", pw.uid);
    let um = format!("_SYSTEMD_USER_UNIT={unit}");
    let out = ctx.run_ok(
        k2_root_proto::paths::JOURNALCTL,
        &["--no-pager", "-o", "cat", "-n", &n, &uid, &um],
        None,
    )?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let truncated = text.len() > LOGS_CAP;
    if truncated {
        let cut = text.len() - LOGS_CAP;
        let cut = (cut..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
        text = text[cut..].to_string();
    }
    Ok(json!({"unit": unit, "text": text, "truncated": truncated}))
}

// ── the engine (S11) ────────────────────────────────────────────────────

/// `runuser -u k2app -- env XDG_RUNTIME_DIR=/run/user/<uid> systemctl --user …`.
fn user_systemctl(ctx: &Ctx, uid: u32, args: &[&str]) -> HResult<crate::ctx::ExecOut> {
    let rt = format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    let mut a: Vec<&str> = vec!["-u", APP_USER, "--", ENV, &rt, SYSTEMCTL, "--user"];
    a.extend_from_slice(args);
    ctx.run_ok(RUNUSER, &a, None)
}

fn engine_unit_state(ctx: &Ctx) -> &'static str {
    file_state(ctx, &format!("{USER_UNIT_DIR}/{USER_UNIT_FILE}"), user_unit().as_bytes())
}

/// Largest installed engine file the helper compares against.
const CAP_ENGINE_FILE: usize = 64 * 1024 * 1024;

/// `apps install-engine` (daemon; stdin `<sig><ELF>`): verify the build
/// (root-helper key, `file:k2-app-engine-linux-<arch>`, marker), install
/// `/usr/local/libexec/k2-app-engine` (root 0755, staged + renamed), the
/// canned user unit (k2app only) and the socket dir's tmpfiles entry (ACL
/// for each configured daemon user), then (re)start the engine as k2app.
/// Every check runs before any write; the binary is never executed here.
pub fn install_engine(ctx: &Ctx, payload: &[u8]) -> HResult<J> {
    let pw = app_user(ctx)?;
    let (sig, elf) = decode_payload(payload).map_err(Fail::sig)?;
    let keys = trusted_keys(ctx.test_keys);
    let built = verify_engine_build(elf, sig, &keys, ctx.arch).map_err(Fail::sig)?;
    let users: Vec<String> = {
        let mut u: Vec<String> = caller::load_daemons(ctx)
            .map_err(Fail::state)?
            .into_iter()
            .map(|d| d.user)
            .collect();
        u.sort();
        u.dedup();
        u
    };
    if !exists(ctx, &format!("/run/user/{}", pw.uid)) {
        return Err(Fail::state(
            "box_not_prepared: k2app's user manager isn't running (apps linger on)",
        ));
    }
    // The binary: the same directory rules as the helper's own update.
    let d = ctx
        .dir(HELPER_DIR)
        .map_err(|e| Fail::io_err("open /usr/local/libexec", e))?;
    let st = d.stat_self().map_err(|e| Fail::io_err("stat libexec", e))?;
    if (st.st_mode as u32) & 0o022 != 0 || (ctx.real_owners && st.st_uid != 0) {
        return Err(Fail::sig(
            "refused: /usr/local/libexec is not root-owned or is group/other-writable",
        ));
    }
    let (cur, stage) = (
        OsStr::new(split(ENGINE_PATH).1),
        OsStr::new(".k2-app-engine.new"),
    );
    match d.kind(cur).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing | Kind::File => {}
        _ => return Err(Fail::state("refused: the engine path is not a regular file")),
    }
    let same = d
        .read_file(cur, CAP_ENGINE_FILE)
        .ok()
        .flatten()
        .is_some_and(|b| b == elf);
    if !same {
        if d.kind(stage).map_err(|e| Fail::io_err("stat", e))? != Kind::Missing {
            d.unlink(stage)
                .map_err(|e| Fail::io_err("remove stale stage", e))?;
        }
        let res = (|| -> std::io::Result<()> {
            use std::io::Write;
            use std::os::fd::AsRawFd;
            let mut f = d.create_excl(stage)?;
            f.write_all(elf)?;
            if let Some((u, g)) = ctx.root_owner() {
                // SAFETY: fd owned by f.
                if unsafe { libc::fchown(f.as_raw_fd(), u, g) } != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // SAFETY: fd owned by f.
            if unsafe { libc::fchmod(f.as_raw_fd(), 0o755) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            f.sync_all()?;
            d.rename(stage, cur)?;
            d.fsync()
        })();
        if let Err(e) = res {
            let _ = d.unlink(stage);
            return Err(Fail::io_err("install engine", e));
        }
    }
    let ud = ensure_subdir(ctx, split(USER_UNIT_DIR).0, split(USER_UNIT_DIR).1)?;
    let unit_changed = put(ctx, &ud, USER_UNIT_FILE, user_unit().as_bytes())?;
    let tdir = split(TMPFILES_FILE).0;
    let td = ensure_subdir(ctx, split(tdir).0, split(tdir).1)?;
    let refs: Vec<&str> = users.iter().map(String::as_str).collect();
    let tmp_changed = put(
        ctx,
        &td,
        split(TMPFILES_FILE).1,
        tmpfiles_conf(&refs).as_bytes(),
    )?;
    ctx.run_ok(SYSTEMD_TMPFILES, &["--create", TMPFILES_FILE], None)?;
    user_systemctl(ctx, pw.uid, &["daemon-reload"])?;
    user_systemctl(ctx, pw.uid, &["enable", USER_UNIT])?;
    user_systemctl(ctx, pw.uid, &["restart", USER_UNIT])?;
    use sha2::Digest;
    let sha: String = sha2::Sha256::digest(elf)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(json!({
        "installed": true,
        "changedBinary": !same,
        "changedUnit": unit_changed,
        "changedTmpfiles": tmp_changed,
        "version": built.version.to_string(),
        "protocol": built.protocol,
        "sha256": sha,
        "keyId": built.key_id,
        "daemonUsers": users,
    }))
}

pub fn nft_remove(ctx: &Ctx) -> HResult<J> {
    let had = nft_list(ctx).is_some();
    if had {
        ctx.run_ok(NFT, &["delete", "table", "inet", NFT_TABLE], None)?;
    }
    Ok(json!({"table": false, "changed": had}))
}

// ── packages and the OCI runtime ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pm {
    Apt,
    Dnf,
    Pacman,
}

fn package_manager(ctx: &Ctx) -> Option<Pm> {
    if exists(ctx, APT_GET) {
        Some(Pm::Apt)
    } else if exists(ctx, DNF) {
        Some(Pm::Dnf)
    } else if exists(ctx, PACMAN) {
        Some(Pm::Pacman)
    } else {
        None
    }
}

fn pm_name(pm: Pm) -> &'static str {
    match pm {
        Pm::Apt => "apt",
        Pm::Dnf => "dnf",
        Pm::Pacman => "pacman",
    }
}

fn install(ctx: &Ctx, pm: Pm, pkgs: &[&str]) -> HResult<()> {
    match pm {
        Pm::Apt => {
            let fe = "DEBIAN_FRONTEND=noninteractive";
            ctx.run_ok(ENV, &[fe, APT_GET, "update", "-q"], None)?;
            let mut a = vec![
                fe,
                APT_GET,
                "install",
                "-y",
                "-q",
                "--no-install-recommends",
                "-o",
                "Dpkg::Options::=--force-confdef",
                "-o",
                "Dpkg::Options::=--force-confold",
            ];
            a.extend_from_slice(pkgs);
            ctx.run_ok(ENV, &a, None)?;
        }
        Pm::Dnf => {
            let mut a = vec!["install", "-y", "-q"];
            a.extend_from_slice(pkgs);
            ctx.run_ok(DNF, &a, None)?;
        }
        Pm::Pacman => {
            let mut a = vec!["-S", "--needed", "--noconfirm"];
            a.extend_from_slice(pkgs);
            ctx.run_ok(PACMAN, &a, None)?;
        }
    }
    Ok(())
}

/// The first `x.y[.z]` in a `--version` answer.
pub fn version_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            let parts: Vec<&str> = w.split('.').collect();
            parts.len() >= 2
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_string)
}

fn tool_version(ctx: &Ctx, abs: &str) -> Option<String> {
    if !exists(ctx, abs) {
        return None;
    }
    let out = ctx.exec.run(abs, &["--version".into()], None).ok()?;
    if out.status != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    version_in(&text).or_else(|| {
        text.lines()
            .next()
            .map(|l| l.trim().chars().take(60).collect())
    })
}

fn major_of(v: &str) -> Option<u32> {
    v.split('.').next()?.parse().ok()
}

fn engine_json(ctx: &Ctx) -> J {
    let podman = tool_version(ctx, PODMAN);
    let conmon = CONMON.iter().find_map(|p| tool_version(ctx, p));
    let conf = read_cfg_text(ctx, CONTAINERS_CONF).unwrap_or_default();
    let configured = [OciRuntime::Crun, OciRuntime::Youki]
        .into_iter()
        .find(|r| conf == containers_conf(*r))
        .map(OciRuntime::as_str);
    json!({
        "packageManager": package_manager(ctx).map(pm_name),
        "podman": podman,
        "podmanOk": podman.as_deref().and_then(major_of).is_some_and(|m| m >= PODMAN_MIN_MAJOR),
        "pasta": exists(ctx, PASTA),
        "crun": tool_version(ctx, CRUN),
        "youki": tool_version(ctx, YOUKI),
        "conmon": conmon,
        "runtime": configured,
    })
}

pub fn packages_install(ctx: &Ctx) -> HResult<J> {
    let pm = package_manager(ctx)
        .ok_or_else(|| Fail::state("no supported package manager (apt-get, dnf, pacman)"))?;
    let pkgs = match pm {
        Pm::Apt => APT_PACKAGES,
        Pm::Dnf => DNF_PACKAGES,
        Pm::Pacman => PACMAN_PACKAGES,
    };
    install(ctx, pm, pkgs)?;
    let v = engine_json(ctx);
    if v["podmanOk"] != true {
        return Err(Fail::state(format!(
            "podman_too_old: the distro ships Podman {} (Apps need {PODMAN_MIN_MAJOR}+: Debian 13, Ubuntu 26.04, Fedora 40+, Arch)",
            v["podman"].as_str().unwrap_or("none")
        )));
    }
    if v["pasta"] != true {
        return Err(Fail::state(
            "pasta_missing: the distro's passt package did not install /usr/bin/pasta",
        ));
    }
    Ok(v)
}

pub fn packages_install_youki(ctx: &Ctx) -> HResult<J> {
    let pm = package_manager(ctx)
        .ok_or_else(|| Fail::state("no supported package manager (apt-get, dnf, pacman)"))?;
    install(ctx, pm, &[YOUKI_PACKAGE]).map_err(|f| {
        Fail::state(format!(
            "youki_unavailable: the distro has no youki package ({})",
            f.msg
        ))
    })?;
    let v = engine_json(ctx);
    if v["youki"].is_null() {
        return Err(Fail::state(
            "youki_unavailable: /usr/bin/youki is missing after the install",
        ));
    }
    Ok(v)
}

pub fn runtime(ctx: &Ctx, rt: OciRuntime) -> HResult<J> {
    if !exists(ctx, rt.binary()) {
        return Err(Fail::state(format!(
            "{}_missing: install it first (apps packages install{})",
            rt.as_str(),
            if rt == OciRuntime::Youki {
                "-youki"
            } else {
                ""
            }
        )));
    }
    let d = ensure_subdir(ctx, "/etc", split(APP_ETC).1)?;
    let changed = put(
        ctx,
        &d,
        CONTAINERS_CONF_FILE,
        containers_conf(rt).as_bytes(),
    )?;
    let mut v = engine_json(ctx);
    v["changed"] = json!(changed);
    Ok(v)
}

// ── the process exception ───────────────────────────────────────────────

/// Running (or starting/stopping) process-exception instances.
fn process_instances(ctx: &Ctx) -> Vec<String> {
    let args: Vec<String> = [
        "list-units",
        "--plain",
        "--no-legend",
        "--state=active,activating,deactivating,reloading",
        "k2app-process@*",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let Ok(out) = ctx.exec.run(SYSTEMCTL, &args, None) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|u| {
            u.strip_prefix(PROCESS_UNIT_PREFIX)?
                .strip_suffix(".service")
        })
        .filter(|id| AppId::parse(id).is_some())
        .map(str::to_string)
        .collect()
}

fn process_unit_state(ctx: &Ctx) -> &'static str {
    match read_cfg(
        ctx,
        &format!("{SYSTEMD_SYSTEM_DIR}/{PROCESS_UNIT_FILE}"),
        64 * 1024,
    ) {
        Ok(Some(b)) if b == process_unit().as_bytes() => "ok",
        Ok(Some(_)) => "drift",
        Ok(None) => "missing",
        Err(_) => "refused",
    }
}

pub fn process_unit_set(ctx: &Ctx, on: bool) -> HResult<J> {
    let changed = if on {
        app_user(ctx)?;
        let d = ctx
            .dir(SYSTEMD_SYSTEM_DIR)
            .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
        put(ctx, &d, PROCESS_UNIT_FILE, process_unit().as_bytes())?
    } else {
        if !process_instances(ctx).is_empty() {
            return Err(Fail::state(
                "refused: process Apps are running (stop them first)",
            ));
        }
        unput(ctx, SYSTEMD_SYSTEM_DIR, None, PROCESS_UNIT_FILE)?
    };
    if changed {
        reload(ctx)?;
    }
    Ok(json!({"processUnit": process_unit_state(ctx), "changed": changed}))
}

/// `/run/systemd/system/k2app-process@<id>.service.d` (runtime: gone at
/// reboot; the daemon writes it before every start).
fn process_caps_dir(id: &AppId) -> String {
    format!("{RUNTIME_SYSTEMD_DIR}/{}.d", id.process_unit())
}

/// `apps process caps`: the App's memory and disk ceilings and its device
/// grants, as a runtime drop-in on its instance. Rendered here from
/// validated tokens; nothing else reaches the file.
pub fn process_caps(
    ctx: &Ctx,
    id: &AppId,
    mem_mib: u64,
    io: &IoMax,
    devices: &[String],
) -> HResult<J> {
    app_user(ctx)?;
    let grants: Vec<DeviceGrant> = devices
        .iter()
        .map(|t| device(t).ok_or_else(|| Fail::state("refused: unknown device")))
        .collect::<HResult<_>>()?;
    for g in &grants {
        if !exists(ctx, &g.path) {
            return Err(Fail::state(format!(
                "device_missing: {} is not on this box",
                g.token
            )));
        }
    }
    let body = process_caps_dropin(mem_mib, io, &grants);
    let (run, systemd) = split(RUNTIME_SYSTEMD_DIR);
    let (run_parent, run_name) = split(run);
    ensure_subdir(ctx, run_parent, run_name)?;
    ensure_subdir(ctx, run, systemd)?;
    let dir = process_caps_dir(id);
    let d = ensure_subdir(ctx, RUNTIME_SYSTEMD_DIR, split(&dir).1)?;
    let changed = put(ctx, &d, PROCESS_CAPS_DROPIN, body.as_bytes())?;
    if changed {
        reload(ctx)?;
    }
    Ok(json!({"app": id.as_str(), "caps": format!("{dir}/{PROCESS_CAPS_DROPIN}"), "changed": changed,
              "memoryMiB": mem_mib, "io": io, "devices": devices}))
}

pub fn process_start(ctx: &Ctx, id: &AppId) -> HResult<J> {
    app_user(ctx)?;
    if process_unit_state(ctx) != "ok" {
        return Err(Fail::state(
            "box_not_prepared: the process unit is missing or changed (apps process-unit on)",
        ));
    }
    if !exists(ctx, ENGINE_PATH) {
        return Err(Fail::state(
            "engine_missing: k2-app-engine is not installed on this box",
        ));
    }
    // Its own user, never k2app (see PROCESS_USER), and the nft table must
    // already cover that user (no unfiltered moment after the start).
    let pu = process_user(ctx)?;
    if !nft_list(ctx).is_some_and(|t| t.contains(&format!("meta skuid {} ", pu.uid))) {
        return Err(Fail::state(
            "box_not_prepared: the k2app nftables table doesn't cover process Apps yet (apps nft apply)",
        ));
    }
    // Never an uncapped App: the caps drop-in comes first.
    if !exists(ctx, &format!("{}/{PROCESS_CAPS_DROPIN}", process_caps_dir(id))) {
        return Err(Fail::state(
            "caps_missing: write the App's caps first (apps process caps)",
        ));
    }
    // AppArmor box: the template must carry the profile drop-in.
    if lsm(ctx) == Lsm::AppArmor
        && file_state(ctx, &process_apparmor_dropin_path(), process_apparmor_dropin().as_bytes()) != "ok"
    {
        return Err(Fail::state(
            "mac_missing: AppArmor is on but the k2-app profile isn't set up (apps apparmor on)",
        ));
    }
    ctx.run_ok(SYSTEMCTL, &["start", &id.process_unit()], None)?;
    Ok(json!({"app": id.as_str(), "unit": id.process_unit(), "started": true}))
}

/// `apps process purge <id>` (daemon): delete a stopped process App's
/// state (`/var/lib/k2appx/<id>`, owned by k2appx, so the engine can't)
/// and its runtime caps drop-in. Never while it runs.
pub fn process_purge(ctx: &Ctx, id: &AppId) -> HResult<J> {
    if process_instances(ctx).iter().any(|a| a == id.as_str()) {
        return Err(Fail::state("refused: the App is running (stop it first)"));
    }
    let mut removed = false;
    let (parent, name) = split(PROCESS_HOME);
    if let Ok(base) = ctx.dir(parent) {
        match base.kind(OsStr::new(name)).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Dir => {
                let home = base.sub(OsStr::new(name)).map_err(|e| Fail::io_err("open", e))?;
                if home.kind(OsStr::new(id.as_str())).map_err(|e| Fail::io_err("stat", e))? != Kind::Missing {
                    home.remove_tree(OsStr::new(id.as_str())).map_err(|e| Fail::io_err("remove", e))?;
                    removed = true;
                }
            }
            Kind::Missing => {}
            _ => return Err(Fail::state("refused: /var/lib/k2appx is not a directory")),
        }
    }
    let dir = process_caps_dir(id);
    let caps = unput(ctx, RUNTIME_SYSTEMD_DIR, Some(split(&dir).1), PROCESS_CAPS_DROPIN)?;
    if caps {
        reload(ctx)?;
    }
    Ok(json!({"app": id.as_str(), "purged": removed, "capsRemoved": caps}))
}

pub fn process_stop(ctx: &Ctx, id: &AppId) -> HResult<J> {
    ctx.run_ok(SYSTEMCTL, &["stop", &id.process_unit()], None)?;
    Ok(json!({"app": id.as_str(), "unit": id.process_unit(), "stopped": true}))
}

// ── MAC: the k2-app AppArmor profile ────────────────────────────────────

fn lsm(ctx: &Ctx) -> Lsm {
    Lsm::detect(
        kread(ctx, APPARMOR_ENABLED).as_deref(),
        kread(ctx, SELINUX_ENFORCE).as_deref(),
    )
}

fn apparmor_parser(ctx: &Ctx) -> Option<&'static str> {
    APPARMOR_PARSER.iter().copied().find(|p| exists(ctx, p))
}

fn process_apparmor_dropin_path() -> String {
    format!("{SYSTEMD_SYSTEM_DIR}/{PROCESS_DROPIN_DIR}/{PROCESS_APPARMOR_DROPIN}")
}

/// `mac` in `apps status`: which LSM is on, and whether K2's profile, the
/// engine's marker and the process drop-in are in place.
fn mac_json(ctx: &Ctx) -> J {
    let l = lsm(ctx);
    let loaded = kread(ctx, APPARMOR_PROFILES)
        .as_deref()
        .and_then(|t| profile_mode(t, APPARMOR_PROFILE));
    let profile = file_state(ctx, APPARMOR_FILE, apparmor_profile().as_bytes());
    let marker = file_state(ctx, MAC_MARKER, mac_marker().as_bytes());
    let dropin = file_state(
        ctx,
        &process_apparmor_dropin_path(),
        process_apparmor_dropin().as_bytes(),
    );
    let applied = match l {
        // SELinux: rootless Podman labels containers itself.
        Lsm::SeLinux => true,
        Lsm::AppArmor => {
            loaded.as_deref() == Some("enforce")
                && profile == "ok"
                && marker == "ok"
                && dropin == "ok"
        }
        Lsm::None => false,
    };
    json!({
        "lsm": l.as_str(),
        "profile": profile,
        "loaded": loaded,
        "marker": marker,
        "processDropin": dropin,
        "parser": apparmor_parser(ctx),
        "applied": applied,
    })
}

/// `apps apparmor on`: where AppArmor is the LSM, write the canned profile
/// and load it (enforce), then the engine's marker and the process
/// template's drop-in, in that order (nothing points at the profile before
/// it is loaded). Elsewhere a no-op that says why. `off` (daemon-only)
/// removes the pointers first, then unloads the profile.
pub fn apparmor(ctx: &Ctx, on: bool) -> HResult<J> {
    let l = lsm(ctx);
    let mut changed = false;
    if on {
        if l != Lsm::AppArmor {
            let mut v = mac_json(ctx);
            v["changed"] = json!(false);
            v["skipped"] = json!(match l {
                Lsm::SeLinux => "SELinux is enforcing: Podman's container_t labels are the MAC layer",
                _ => "no MAC layer on this box (AppArmor and SELinux are off)",
            });
            return Ok(v);
        }
        let parser = apparmor_parser(ctx).ok_or_else(|| {
            Fail::state("apparmor_missing: AppArmor is on but apparmor_parser isn't installed")
        })?;
        let ad = ctx
            .dir(APPARMOR_DIR)
            .map_err(|e| Fail::io_err("open /etc/apparmor.d", e))?;
        changed |= put(ctx, &ad, split(APPARMOR_FILE).1, apparmor_profile().as_bytes())?;
        let loaded = kread(ctx, APPARMOR_PROFILES)
            .as_deref()
            .and_then(|t| profile_mode(t, APPARMOR_PROFILE));
        if changed || loaded.as_deref() != Some("enforce") {
            // --replace loads or reloads; never --complain.
            ctx.run_ok(parser, &["--replace", "--write-cache", APPARMOR_FILE], None)?;
            changed = true;
        }
        let ed = ensure_subdir(ctx, "/etc", split(APP_ETC).1)?;
        changed |= put(ctx, &ed, MAC_MARKER_FILE, mac_marker().as_bytes())?;
        let dd = ensure_subdir(ctx, SYSTEMD_SYSTEM_DIR, PROCESS_DROPIN_DIR)?;
        if put(ctx, &dd, PROCESS_APPARMOR_DROPIN, process_apparmor_dropin().as_bytes())? {
            changed = true;
            reload(ctx)?;
        }
    } else {
        if unput(ctx, SYSTEMD_SYSTEM_DIR, Some(PROCESS_DROPIN_DIR), PROCESS_APPARMOR_DROPIN)? {
            changed = true;
            reload(ctx)?;
        }
        changed |= unput(ctx, "/etc", Some(split(APP_ETC).1), MAC_MARKER_FILE)?;
        let loaded = kread(ctx, APPARMOR_PROFILES)
            .as_deref()
            .and_then(|t| profile_mode(t, APPARMOR_PROFILE))
            .is_some();
        if loaded && exists(ctx, APPARMOR_FILE) {
            if let Some(parser) = apparmor_parser(ctx) {
                ctx.run_ok(parser, &["--remove", APPARMOR_FILE], None)?;
                changed = true;
            }
        }
        changed |= unput(ctx, APPARMOR_DIR, None, split(APPARMOR_FILE).1)?;
    }
    let mut v = mac_json(ctx);
    v["changed"] = json!(changed);
    Ok(v)
}

// ── cores slice and the Apps budget ─────────────────────────────────────

fn mem_total_mib(ctx: &Ctx) -> Option<u64> {
    let text = kread(ctx, "/proc/meminfo")?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

const UNIT_DIRS: &[&str] = &[
    "/etc/systemd/system",
    "/usr/lib/systemd/system",
    "/lib/systemd/system",
];

fn unit_exists(ctx: &Ctx, unit: &str) -> bool {
    UNIT_DIRS
        .iter()
        .any(|d| exists(ctx, &format!("{d}/{unit}")))
}

/// The core units on this box: (unit, which core). The daemon's unit is
/// the caller's own `daemon.toml` entry (system scope only).
fn core_units(ctx: &Ctx, who: &Caller) -> HResult<Vec<(String, Core)>> {
    let mut v = Vec::new();
    let me = crate::daemon_unit::caller_entry(ctx, who)?;
    if me.scope == "system" {
        v.push((me.unit, Core::Daemon));
    }
    if unit_exists(ctx, "postgresql@.service") {
        v.push(("postgresql@.service".into(), Core::Postgres));
    }
    if unit_exists(ctx, "stalwart.service") {
        v.push(("stalwart.service".into(), Core::Stalwart));
    }
    Ok(v)
}

pub fn cores_slice(ctx: &Ctx, who: &Caller, mem_min: Option<u64>) -> HResult<J> {
    let units = core_units(ctx, who)?;
    let sysd = ctx
        .dir(SYSTEMD_SYSTEM_DIR)
        .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
    let mut changed = false;
    match mem_min {
        Some(n) => {
            let total =
                mem_total_mib(ctx).ok_or_else(|| Fail::state("no MemTotal in /proc/meminfo"))?;
            if n > total / 2 {
                return Err(Fail::argv("cores floor above half of this box's memory"));
            }
            changed |= put(ctx, &sysd, CORES_SLICE, cores_slice_unit(n).as_bytes())?;
            for (u, core) in &units {
                let d = ensure_subdir(ctx, SYSTEMD_SYSTEM_DIR, &format!("{u}.d"))?;
                changed |= put(ctx, &d, CORES_DROPIN_FILE, cores_dropin(*core).as_bytes())?;
            }
        }
        None => {
            for (u, _) in &units {
                changed |= unput(
                    ctx,
                    SYSTEMD_SYSTEM_DIR,
                    Some(&format!("{u}.d")),
                    CORES_DROPIN_FILE,
                )?;
            }
            changed |= unput(ctx, SYSTEMD_SYSTEM_DIR, None, CORES_SLICE)?;
        }
    }
    if changed {
        reload(ctx)?;
    }
    let names: Vec<&str> = units.iter().map(|(u, _)| u.as_str()).collect();
    Ok(json!({
        "slice": CORES_SLICE,
        "memoryMinMib": mem_min,
        "units": names,
        "changed": changed,
        // Slice= moves a unit at its next restart; K2 restarts nothing here.
        "pendingRestart": if changed { names.clone() } else { Vec::new() },
    }))
}

pub fn budget(ctx: &Ctx, b: Option<(u64, u64)>) -> HResult<J> {
    let pw = app_user(ctx)?;
    let dname = user_unit_dropin_dir(pw.uid);
    let changed = match b {
        Some((mem, tasks)) => {
            let total =
                mem_total_mib(ctx).ok_or_else(|| Fail::state("no MemTotal in /proc/meminfo"))?;
            if mem > total {
                return Err(Fail::argv("Apps budget above this box's memory"));
            }
            let d = ensure_subdir(ctx, SYSTEMD_SYSTEM_DIR, &dname)?;
            put(
                ctx,
                &d,
                BUDGET_DROPIN_FILE,
                budget_dropin(mem, tasks).as_bytes(),
            )?
        }
        None => unput(ctx, SYSTEMD_SYSTEM_DIR, Some(&dname), BUDGET_DROPIN_FILE)?,
    };
    if changed {
        reload(ctx)?;
    }
    Ok(
        json!({"budget": b.map(|(m, t)| json!({"memoryMaxMib": m, "tasksMax": t})), "changed": changed}),
    )
}

// ── unprep ──────────────────────────────────────────────────────────────

/// Processes of `uid` other than its user manager and Podman's pause
/// process (an App, a shell, anything else).
fn busy_app_processes(ctx: &Ctx, uid: u32) -> usize {
    let Ok(rd) = std::fs::read_dir(ctx.p("/proc")) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let Some(status) = kread(ctx, &format!("/proc/{pid}/status")) else {
            continue;
        };
        let ruid = status
            .lines()
            .find_map(|l| l.strip_prefix("Uid:"))
            .and_then(|r| r.split_whitespace().next())
            .and_then(|r| r.parse::<u32>().ok());
        if ruid != Some(uid) {
            continue;
        }
        let comm = status
            .lines()
            .find_map(|l| l.strip_prefix("Name:"))
            .map(str::trim)
            .unwrap_or("");
        if !matches!(
            comm,
            "systemd" | "(sd-pam)" | "catatonit" | "podman pause" | "podman" | "k2-app-engine"
        ) {
            n += 1;
        }
    }
    n
}

pub fn unprep(ctx: &Ctx, who: &Caller) -> HResult<J> {
    if !process_instances(ctx).is_empty() {
        return Err(Fail::state(
            "refused: process Apps are running (stop them first)",
        ));
    }
    let pw = passwd(ctx)?.into_iter().find(|p| p.name == APP_USER);
    if let Some(pw) = &pw {
        let busy = busy_app_processes(ctx, pw.uid);
        if busy > 0 {
            return Err(Fail::state(format!(
                "refused: {busy} k2app processes are running (stop every App first)"
            )));
        }
    }
    let mut steps = Vec::new();
    let mut step = |name: &str, r: HResult<J>| -> HResult<()> {
        let v = r?;
        steps.push(
            json!({"step": name, "changed": v.get("changed").cloned().unwrap_or(json!(false))}),
        );
        Ok(())
    };
    if let Some(pw) = &pw {
        if exists(ctx, &format!("/run/user/{}", pw.uid)) {
            // The engine goes first; it holds no App (each is its own unit).
            let _ = user_systemctl(ctx, pw.uid, &["disable", "--now", USER_UNIT]);
        }
    }
    step(
        "engine unit remove",
        unput(ctx, split(USER_UNIT_DIR).0, Some(split(USER_UNIT_DIR).1), USER_UNIT_FILE)
            .map(|c| json!({"changed": c})),
    )?;
    let tdir = split(TMPFILES_FILE).0;
    step(
        "engine socket dir remove",
        unput(ctx, split(tdir).0, Some(split(tdir).1), split(TMPFILES_FILE).1)
            .map(|c| json!({"changed": c})),
    )?;
    step(
        "per-App nft state remove",
        unput(ctx, split(STATE_DIR).0, Some(split(STATE_DIR).1), split(NFT_APPS_STATE).1)
            .map(|c| json!({"changed": c})),
    )?;
    step("nft remove", nft_remove(ctx))?;
    step("apparmor off", apparmor(ctx, false))?;
    step("process-unit off", process_unit_set(ctx, false))?;
    step("cores-slice off", cores_slice(ctx, who, None))?;
    step("io-cost off", io_cost(ctx, false))?;
    step("reserve-ports off", reserve_ports(ctx, false))?;
    step(
        "runtime conf remove",
        unput(ctx, "/etc", Some(split(APP_ETC).1), CONTAINERS_CONF_FILE)
            .map(|c| json!({"changed": c})),
    )?;
    if pw.is_some() {
        step("budget off", budget(ctx, None))?;
        step("linger off", linger(ctx, false))?;
        // The user manager may already be gone; that is the goal.
        let _ = ctx
            .exec
            .run(LOGINCTL, &["terminate-user".into(), APP_USER.into()], None);
        step("delegate off", delegate(ctx, false))?;
    }
    Ok(json!({
        "unprepared": true,
        "steps": steps,
        "kept": ["the k2app user and its subordinate ids", "/var/lib/k2app", "distro packages", "the k2-app-engine binary"],
    }))
}

// ── status ──────────────────────────────────────────────────────────────

fn file_state(ctx: &Ctx, abs: &str, want: &[u8]) -> &'static str {
    match read_cfg(ctx, abs, 256 * 1024) {
        Ok(Some(b)) if b == want => "ok",
        Ok(Some(_)) => "drift",
        Ok(None) => "missing",
        Err(_) => "refused",
    }
}

pub fn status(ctx: &Ctx) -> HResult<J> {
    let all = passwd(ctx)?;
    let pw = all.iter().find(|p| p.name == APP_USER).cloned();
    let user = match &pw {
        None => json!({"exists": false}),
        Some(p) => {
            let problem = shape_problem(ctx, p, &all)?;
            json!({"exists": true, "uid": p.uid, "gid": p.gid, "home": p.home, "shell": p.shell,
                   "ok": problem.is_none(), "problem": problem})
        }
    };
    let subid = match &pw {
        None => json!(null),
        Some(p) => {
            let ids = real_ids(ctx, &all)?;
            match subid_state(ctx, p, &ids) {
                Ok((_, _, mu, mg)) => {
                    json!({"subuid": ranges_json(&mu), "subgid": ranges_json(&mg),
                    "count": mu.iter().map(|r| r.count).sum::<u64>(), "ok": !mu.is_empty() && !mg.is_empty()})
                }
                Err(f) => json!({"ok": false, "problem": f.msg}),
            }
        }
    };
    let (delegate, budget) = match &pw {
        None => (json!("missing"), json!(null)),
        Some(p) => {
            let dir = format!("{SYSTEMD_SYSTEM_DIR}/{}", user_unit_dropin_dir(p.uid));
            let d = file_state(
                ctx,
                &format!("{dir}/{DELEGATE_DROPIN_FILE}"),
                delegate_dropin().as_bytes(),
            );
            let b = read_cfg(ctx, &format!("{dir}/{BUDGET_DROPIN_FILE}"), 64 * 1024)
                .ok()
                .flatten();
            (json!(d), json!(b.is_some()))
        }
    };
    let cores = read_cfg_text(ctx, &format!("{SYSTEMD_SYSTEM_DIR}/{CORES_SLICE}"))?;
    let cores_min = cores
        .lines()
        .find_map(|l| l.strip_prefix("MemoryMin="))
        .map(str::to_string);
    let daemons = caller::load_daemons(ctx).unwrap_or_default();
    Ok(json!({
        "user": user,
        "subid": subid,
        "linger": linger_on_disk(ctx),
        "delegate": delegate,
        "budget": budget,
        "ioCost": io_cost_json(ctx),
        "reservedPorts": reserved_json(ctx),
        "nft": nft_json(ctx, pw.as_ref().map(|p| p.uid)),
        "processUnit": process_unit_state(ctx),
        "processApps": process_instances(ctx),
        "processUser": process_user_json(ctx),
        "mac": mac_json(ctx),
        "coresSlice": {"present": !cores.is_empty(), "memoryMin": cores_min,
                       "daemonUnits": daemons.iter().filter(|d| d.scope == "system").map(|d| d.unit.clone()).collect::<Vec<_>>()},
        "engine": engine_json(ctx),
        "enginePresent": exists(ctx, ENGINE_PATH),
        "engineUnit": engine_unit_state(ctx),
        "engineTmpfiles": exists(ctx, TMPFILES_FILE),
        "nftApps": load_nft_apps(ctx).map(|s| json!(s.apps)).unwrap_or(json!(null)),
        "nftBoxPorts": load_nft_apps(ctx).map(|s| json!(s.box_ports)).unwrap_or(json!(null)),
    }))
}
