//! Where things are, who may run, the shared lock, files and the audit log.
//!
//! Same rules as k2sign (`scripts/k2sign/k2sign.py` `Ctx`): signer verbs run
//! only as `_k2sign`, admin verbs only as root; test mode
//! (`K2_SIGNER_TEST_ROOT`) is refused as root, as the signing user and as
//! `K2_SIGNER_TEST_SIGNER_USER`, so it can never open a hole even if sudo's
//! env_reset were loosened. Only in test mode are `K2_SIGNER_TEST_NOW` and
//! `K2_SIGNER_TEST_GITHUB_API` read.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::{refused, Fail, R};

/// The role user both signers run as (V28).
pub const SIGNER_USER: &str = "_k2sign";
/// The only PATH any tool runs under.
pub const SYS_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub const ENV_TEST_ROOT: &str = "K2_SIGNER_TEST_ROOT";
pub const ENV_TEST_NOW: &str = "K2_SIGNER_TEST_NOW";
pub const ENV_TEST_SIGNER_USER: &str = "K2_SIGNER_TEST_SIGNER_USER";
pub const ENV_TEST_GITHUB_API: &str = "K2_SIGNER_TEST_GITHUB_API";

#[derive(Debug, Clone)]
pub struct Ctx {
    pub admin: bool,
    pub test: bool,
    /// `""` in production; the test root otherwise.
    pub root: PathBuf,
    now_override: Option<i64>,
    github_api_override: Option<String>,
    /// `_k2sign`'s uid and gid, when the user exists.
    signer_ids: Option<(u32, u32)>,
}

fn user_ids(name: &str) -> Option<(u32, u32)> {
    let c = CString::new(name).ok()?;
    // SAFETY: getpwnam returns a pointer into static storage or null; we copy
    // the two integers out before any other libc call.
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() {
            None
        } else {
            Some(((*pw).pw_uid, (*pw).pw_gid))
        }
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

impl Ctx {
    /// The production context, or test mode when `K2_SIGNER_TEST_ROOT` is set
    /// (and the caller is neither root nor the signing user).
    pub fn from_env(admin: bool) -> R<Self> {
        let signer_ids = user_ids(SIGNER_USER);
        let root = std::env::var(ENV_TEST_ROOT).unwrap_or_default();
        if !root.is_empty() {
            let p = PathBuf::from(&root);
            if !p.is_absolute() || !p.is_dir() {
                return refused(format!("{ENV_TEST_ROOT} must be an existing absolute directory"));
            }
            let mut refused_uids = vec![0u32];
            if let Some((u, _)) = signer_ids {
                refused_uids.push(u);
            }
            if let Ok(extra) = std::env::var(ENV_TEST_SIGNER_USER) {
                if !extra.is_empty() {
                    match user_ids(&extra) {
                        Some((u, _)) => refused_uids.push(u),
                        None => return refused(format!("{ENV_TEST_SIGNER_USER} names no user: {extra}")),
                    }
                }
            }
            if refused_uids.contains(&euid()) {
                return refused(format!(
                    "test mode ({ENV_TEST_ROOT}) is refused as root or the signing user: they never take test overrides"
                ));
            }
            let now_override = match std::env::var(ENV_TEST_NOW) {
                Ok(s) if !s.is_empty() => Some(s.parse::<i64>().map_err(|_| Fail::Usage(format!("{ENV_TEST_NOW} is not a number")))?),
                _ => None,
            };
            let github_api_override = std::env::var(ENV_TEST_GITHUB_API).ok().filter(|s| !s.is_empty());
            return Ok(Self { admin, test: true, root: p, now_override, github_api_override, signer_ids });
        }
        if admin {
            if euid() != 0 {
                return refused("k2-signer-admin runs only as root (sudo /usr/local/libexec/k2-signer/k2-signer-admin ...)");
            }
            if signer_ids.is_none() {
                return refused(format!("there is no {SIGNER_USER} user (run scripts/k2sign/install.sh first)"));
            }
        } else {
            match signer_ids {
                Some((u, _)) if u == euid() => {}
                _ => {
                    return refused(format!(
                        "k2-signer runs only as {SIGNER_USER} (sudo -n -u {SIGNER_USER} /usr/local/libexec/k2-signer/k2-signer ...)"
                    ))
                }
            }
        }
        Ok(Self { admin, test: false, root: PathBuf::new(), now_override: None, github_api_override: None, signer_ids })
    }

    /// A test context rooted at `root` (library tests; no identity checks).
    pub fn for_test(root: &Path, admin: bool, now: i64) -> Self {
        Self {
            admin,
            test: true,
            root: root.to_path_buf(),
            now_override: Some(now),
            github_api_override: None,
            signer_ids: None,
        }
    }

    pub fn with_now(mut self, now: i64) -> Self {
        self.now_override = Some(now);
        self
    }

    pub fn with_github_api(mut self, api: &str) -> Self {
        self.github_api_override = Some(api.to_string());
        self
    }

    fn at(&self, p: &str) -> PathBuf {
        if self.root.as_os_str().is_empty() {
            PathBuf::from(p)
        } else {
            self.root.join(p.trim_start_matches('/'))
        }
    }

    pub fn libexec(&self) -> PathBuf {
        self.at("/usr/local/libexec/k2-signer")
    }
    pub fn etc(&self) -> PathBuf {
        self.at("/usr/local/etc/k2-signer")
    }
    pub fn projects_dir(&self) -> PathBuf {
        self.etc().join("projects")
    }
    /// k2sign's etc (its policy's `release_commit_files`, its compute pins).
    pub fn k2sign_etc(&self) -> PathBuf {
        self.at("/usr/local/etc/k2sign")
    }
    /// `/var/k2sign`: the role user's home, shared with k2sign.
    pub fn var(&self) -> PathBuf {
        self.at("/var/k2sign")
    }
    /// k2sign's state dir (shared registries and the lock).
    pub fn state(&self) -> PathBuf {
        self.var().join("state")
    }
    /// k2-signer's own state.
    pub fn own_dir(&self) -> PathBuf {
        self.var().join("k2-signer")
    }
    pub fn tmp(&self) -> PathBuf {
        self.own_dir().join("tmp")
    }

    pub fn now(&self) -> i64 {
        if let Some(n) = self.now_override.filter(|_| self.test) {
            return n;
        }
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    /// The GitHub API base: `https://api.github.com`; test mode may point at
    /// a local fake on 127.0.0.1.
    pub fn github_api(&self) -> R<String> {
        if self.test {
            if let Some(api) = &self.github_api_override {
                let ok = api
                    .strip_prefix("http://127.0.0.1:")
                    .is_some_and(|p| (2..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()));
                if !ok {
                    return refused(format!("{ENV_TEST_GITHUB_API} must be http://127.0.0.1:<port>"));
                }
                return Ok(api.clone());
            }
        }
        Ok("https://api.github.com".to_string())
    }

    /// Admin writes under `/var/k2sign` belong to the signing user.
    pub fn own(&self, path: &Path) -> R<()> {
        if self.admin && !self.test {
            if let Some((u, g)) = self.signer_ids {
                std::os::unix::fs::chown(path, Some(u), Some(g))?;
            }
        }
        Ok(())
    }
}

// ── files ──────────────────────────────────────────────────────────────

pub fn mode_of(path: &Path) -> R<u32> {
    Ok(fs::symlink_metadata(path)?.permissions().mode() & 0o7777)
}

/// Write `data` atomically (temp file, fsync, rename) with `mode`.
pub fn write_file(ctx: &Ctx, path: &Path, data: &[u8], mode: u32) -> R<()> {
    let rnd = k2_node_proto::crypto::random_bytes(4).map_err(Fail::Internal)?;
    let tmp = PathBuf::from(format!("{}.tmp-{}", path.display(), k2_node_proto::crypto::hex(&rnd)));
    let res = (|| -> R<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        ctx.own(&tmp)?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// A directory, made 0700 (and the signing user's) when missing.
pub fn mkdirs(ctx: &Ctx, path: &Path) -> R<()> {
    if !path.is_dir() {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        ctx.own(path)?;
    }
    Ok(())
}

/// Append one line to a file the signing user owns (registries, audit).
pub fn append_line(ctx: &Ctx, path: &Path, line: &str) -> R<()> {
    let mut f = OpenOptions::new().append(true).create(true).mode(0o600).open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    ctx.own(path)?;
    Ok(())
}

/// Production: a file the signer trusts is root-owned, not a symlink, and
/// writable by root only.
pub fn check_root_owned(ctx: &Ctx, path: &Path) -> R<()> {
    if ctx.test {
        return Ok(());
    }
    let m = fs::symlink_metadata(path).map_err(|e| Fail::Refused(format!("{}: {e}", path.display())))?;
    if m.file_type().is_symlink() {
        return refused(format!("{} is a symlink", path.display()));
    }
    if m.uid() != 0 || m.mode() & 0o022 != 0 {
        return refused(format!("{} must be owned by root and writable only by root", path.display()));
    }
    Ok(())
}

/// Read a small file the signer trusts (root-owned in production).
pub fn read_trusted(ctx: &Ctx, path: &Path, max: u64) -> R<Vec<u8>> {
    check_root_owned(ctx, path)?;
    let f = File::open(path).map_err(|e| Fail::Refused(format!("{}: {e}", path.display())))?;
    let mut buf = Vec::new();
    f.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return refused(format!("{} is larger than {max} bytes", path.display()));
    }
    Ok(buf)
}

/// Read stdin up to `limit` bytes (refused above it).
pub fn read_stdin(limit: usize) -> R<Vec<u8>> {
    let mut buf = Vec::new();
    std::io::stdin().lock().take(limit as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > limit {
        return refused(format!("input is larger than {limit} bytes"));
    }
    Ok(buf)
}

pub fn sha256_hex(b: &[u8]) -> String {
    k2_node_proto::crypto::sha256_hex(b)
}

// ── the lock (shared with k2sign: state/.lock) ─────────────────────────

/// One lock for every state change, the same file k2sign flocks, so the two
/// signers never interleave a registry update (V28, H4).
pub struct Locked(File);

impl Locked {
    pub fn take(ctx: &Ctx) -> R<Self> {
        mkdirs(ctx, &ctx.state())?;
        let p = ctx.state().join(".lock");
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(&p)?;
        ctx.own(&p)?;
        f.lock()?;
        Ok(Self(f))
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

// ── time ───────────────────────────────────────────────────────────────

/// `2026-11-07T12:00:00Z`.
pub fn iso(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// `2026-11-07 12:00 UTC`.
pub fn hhmm(epoch: i64) -> String {
    let s = iso(epoch);
    format!("{} {} UTC", &s[..10], &s[11..16])
}

// ── audit (k2sign's signatures.tsv format) ─────────────────────────────

pub struct AuditLine<'a> {
    pub event: &'a str,
    pub slot: &'a str,
    pub asset: &'a str,
    pub sha: &'a str,
    pub key_id: &'a str,
    pub tc: &'a str,
    pub detail: &'a str,
}

impl<'a> AuditLine<'a> {
    pub fn event(event: &'a str, detail: &'a str) -> Self {
        Self { event, slot: "", asset: "", sha: "", key_id: "", tc: "", detail }
    }
}

/// Append to `<dir>/signatures.tsv` (k2sign's columns, so one reader reads
/// both signers' logs).
pub fn audit(ctx: &Ctx, dir: &Path, l: &AuditLine) -> R<()> {
    if let Some(parent) = dir.parent() {
        mkdirs(ctx, parent)?;
    }
    mkdirs(ctx, dir)?;
    let path = dir.join("signatures.tsv");
    if !path.exists() {
        append_line(ctx, &path, "time\tevent\tslot\tasset\tsha256\tkey_id\ttrusted_comment\tdetail")?;
    }
    let clean = |s: &str| s.replace('\t', "\\t").replace('\n', " ");
    let line = [iso(ctx.now()).as_str(), l.event, l.slot, l.asset, l.sha, l.key_id, l.tc, l.detail]
        .iter()
        .map(|s| clean(s))
        .collect::<Vec<_>>()
        .join("\t");
    append_line(ctx, &path, &line)
}

// ── pins (root-owned files, hashed into pins.sha256) ───────────────────

/// Every file whose hash `pins.sha256` must hold: `(name, path)`.
pub fn pinned_files(ctx: &Ctx) -> R<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for n in ["k2-signer", "k2-signer-admin", "k2-signer-bin"] {
        out.push((format!("libexec/{n}"), ctx.libexec().join(n)));
    }
    for n in ["trust.json", "approver.pem"] {
        let p = ctx.etc().join(n);
        if p.exists() {
            out.push((format!("etc/{n}"), p));
        }
    }
    if ctx.projects_dir().is_dir() {
        let mut names: Vec<String> = fs::read_dir(ctx.projects_dir())?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".toml"))
            .collect();
        names.sort();
        for n in names {
            out.push((format!("etc/projects/{n}"), ctx.projects_dir().join(&n)));
        }
    }
    Ok(out)
}

fn file_sha(path: &Path) -> R<String> {
    let mut f = File::open(path)?;
    let mut h = k2_node_proto::crypto::Sha256Stream::default();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finish_hex())
}

pub fn sha256_file(path: &Path) -> R<String> {
    file_sha(path)
}

pub fn pins_text(ctx: &Ctx) -> R<String> {
    let mut s = String::new();
    for (name, path) in pinned_files(ctx)? {
        if !path.is_file() {
            return refused(format!("missing {} (re-run scripts/k2sign/install-k2-signer.sh)", path.display()));
        }
        s.push_str(&format!("{}  {name}\n", file_sha(&path)?));
    }
    Ok(s)
}

/// Rewrite `pins.sha256` (admin, after any change to a pinned file).
pub fn repin(ctx: &Ctx) -> R<String> {
    let text = pins_text(ctx)?;
    let path = ctx.etc().join("pins.sha256");
    let tmp = ctx.etc().join("pins.sha256.tmp");
    fs::write(&tmp, &text)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    fs::rename(&tmp, &path)?;
    Ok(text)
}

/// Every pinned file is present, root-owned and hashes to its pin; nothing
/// pinned is missing.
pub fn check_pins(ctx: &Ctx) -> R<()> {
    let pins_path = ctx.etc().join("pins.sha256");
    if !pins_path.is_file() {
        return refused("no pins.sha256 (re-run scripts/k2sign/install-k2-signer.sh)");
    }
    for d in [ctx.libexec(), ctx.etc()] {
        check_root_owned(ctx, &d)?;
    }
    check_root_owned(ctx, &pins_path)?;
    let mut want = std::collections::BTreeMap::new();
    for line in fs::read_to_string(&pins_path)?.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (h, n) = line.split_once("  ").ok_or_else(|| Fail::Refused(format!("pins.sha256 has a bad line: {line:?}")))?;
        if !k2_node_proto::ci::is_fp(h) || n.is_empty() || n.contains(char::is_whitespace) {
            return refused(format!("pins.sha256 has a bad line: {line:?}"));
        }
        want.insert(n.to_string(), h.to_string());
    }
    let files = pinned_files(ctx)?;
    for (name, path) in &files {
        let Some(h) = want.get(name) else {
            return refused(format!("pins mismatch: {name} (not pinned)"));
        };
        if !path.is_file() {
            return refused(format!("pins mismatch: {name} (missing)"));
        }
        check_root_owned(ctx, path)?;
        if &file_sha(path)? != h {
            return refused(format!("pins mismatch: {name}"));
        }
    }
    for name in want.keys() {
        if !files.iter().any(|(n, _)| n == name) {
            return refused(format!("pins mismatch: {name} (pinned but missing)"));
        }
    }
    Ok(())
}

/// `/var/k2sign` is the signing user's alone (0700).
pub fn check_var(ctx: &Ctx) -> R<()> {
    let v = ctx.var();
    if !v.is_dir() {
        return refused(format!("{} is missing (run scripts/k2sign/install.sh)", v.display()));
    }
    if mode_of(&v)? != 0o700 {
        return refused(format!("{} must be mode 700", v.display()));
    }
    if !ctx.test {
        let uid = fs::symlink_metadata(&v)?.uid();
        if ctx.signer_ids.map(|(u, _)| u) != Some(uid) {
            return refused(format!("{} must be owned by {SIGNER_USER}", v.display()));
        }
    }
    Ok(())
}

/// A scratch dir under the signer's tmp, removed on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(ctx: &Ctx) -> R<Self> {
        mkdirs(ctx, &ctx.own_dir())?;
        mkdirs(ctx, &ctx.tmp())?;
        let rnd = k2_node_proto::crypto::random_bytes(8).map_err(Fail::Internal)?;
        let d = ctx.tmp().join(k2_node_proto::crypto::hex(&rnd));
        fs::create_dir(&d)?;
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
        Ok(Self(d))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_formats_unix_seconds() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_800_000_000), "2027-01-15T08:00:00Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(hhmm(1_800_000_000), "2027-01-15 08:00 UTC");
    }
}
