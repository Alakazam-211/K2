//! Ship kind `box-backup` (0.46.9): the box's own backup client
//! (`k2-backup`, restic, nightly via `k2-backup.timer`) carries Sew's
//! bundles off the box, so K2 doesn't copy them anywhere itself.
//!
//! The owner says so with `k2 hostchat ship box-backup`. K2 only believes
//! it while it sees the evidence, judged each time status or the alarms
//! are computed (the reads are cached for [`CACHE_SECS`]):
//!
//! - `/etc/k2-backup/paths` lists `/var/lib/sew/backups` or a folder above
//!   it (`/`, `/var`, `/var/lib`, `/var/lib/sew`), by whole path
//!   components; simple globs count;
//! - `/etc/k2-backup/excludes` drops neither that folder, a folder above
//!   it, nor a bundle inside it (restic exclude patterns: exact paths,
//!   `*`, `?`, `[…]`, `**`, a pattern without a leading `/` matches at any
//!   depth, `!` re-includes, the last match wins, `$VAR` matches anything);
//! - `k2-backup.timer` is enabled.
//!
//! Without all three, `k2_backup_offbox_none` stays, with the reason.
//!
//! The daemon reads the two files itself when it may; when they are
//! root-only it asks the root helper (`engine sew box-backup`, protocol 13,
//! `namespaces.sewboxbackup`), which reads only those two files and the
//! timer, never the client's `env`. The judging here is pure ([`judge`]);
//! the reading is [`read_with`] over injected readers, and the machine
//! tests swap the whole read through `SewOps::box_backup_facts`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub use k2_root_proto::box_backup::{BoxBackupFacts, FileRead, CONFIG_DIR, EXCLUDES_FILE, PATHS_FILE, SEW_BACKUPS_DIR, TIMER_UNIT};

use crate::root_helper::RootHelperState;

/// The ship kind (`sew_server.ship_kind`).
pub const KIND: &str = "box-backup";
/// How long one reading of the files and the timer is reused.
pub const CACHE_SECS: u64 = 30;
/// A bundle-shaped name inside [`SEW_BACKUPS_DIR`]: an exclude that drops
/// it drops Sew's bundles even when it spares the folder itself.
const BUNDLE_PROBE: &str = "20260101T000000Z";

/// What K2 concluded from the facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub ok: bool,
    /// Why not, one short line each (empty when `ok`).
    pub reasons: Vec<String>,
    /// The `paths` line that covers Sew's bundles.
    pub covered_by: Option<String>,
    /// The `excludes` line that drops them.
    pub excluded_by: Option<String>,
    /// `systemctl is-enabled k2-backup.timer`'s word.
    pub timer: String,
}

impl Evidence {
    /// One line for status ("Off-box: …").
    pub fn line(&self) -> String {
        if self.ok {
            "the box backup (k2-backup) carries Sew's bundles".to_string()
        } else {
            format!("the box backup (k2-backup) is chosen, but K2 can't see it carry Sew's bundles: {}", self.reasons.join("; "))
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "ok": self.ok,
            "reason": (!self.reasons.is_empty()).then(|| self.reasons.join("; ")),
            "reasons": self.reasons,
            "coveredBy": self.covered_by,
            "excludedBy": self.excluded_by,
            "timer": self.timer,
            "line": self.line(),
        })
    }
}

fn file_path(name: &str) -> String {
    format!("{CONFIG_DIR}/{name}")
}

/// Judge the facts for [`SEW_BACKUPS_DIR`].
pub fn judge(f: &BoxBackupFacts) -> Evidence {
    judge_for(f, SEW_BACKUPS_DIR)
}

/// Judge the facts for `target` (an absolute folder).
pub fn judge_for(f: &BoxBackupFacts, target: &str) -> Evidence {
    let mut reasons = Vec::new();
    let paths = file_path(PATHS_FILE);
    let excludes = file_path(EXCLUDES_FILE);
    let covered_by = match &f.paths {
        FileRead::Missing => {
            reasons.push(format!("{paths} is missing (the box backup isn't set up here)"));
            None
        }
        FileRead::Error { error } => {
            reasons.push(format!("can't read {paths}: {error}"));
            None
        }
        FileRead::Text { text } => {
            let c = covering_path(text, target);
            if c.is_none() {
                reasons.push(format!("the box backup does not cover {target} (no line in {paths} is that folder or a folder above it)"));
            }
            c
        }
    };
    let excluded_by = match &f.excludes {
        // No exclude list: nothing is excluded.
        FileRead::Missing => None,
        FileRead::Error { error } => {
            reasons.push(format!("can't read {excludes}: {error}"));
            None
        }
        FileRead::Text { text } => {
            let e = excluding_pattern(text, target);
            if let Some(p) = &e {
                reasons.push(format!("the box backup excludes {target} ('{p}' in {excludes})"));
            }
            e
        }
    };
    if f.timer != "enabled" {
        reasons.push(format!("{TIMER_UNIT} is not enabled (systemctl is-enabled says '{}')", f.timer));
    }
    Evidence { ok: reasons.is_empty(), reasons, covered_by, excluded_by, timer: f.timer.clone() }
}

/// Lines of a restic list: trimmed, no blanks, no `#` comments.
fn list_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Path components; `None` for `..` (never judged).
fn components(p: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// The first `paths` line that covers `target`: an absolute path (globs
/// allowed) that is `target` or a folder above it by whole components, or
/// `target` followed only by `*` / `**` (everything inside it).
pub fn covering_path(paths: &str, target: &str) -> Option<String> {
    let t = components(target)?;
    list_lines(paths)
        .find(|line| {
            if !line.starts_with('/') {
                return false;
            }
            let Some(p) = components(line) else { return false };
            let above = (0..=t.len()).any(|n| glob_match(&p, &t[..n]));
            let inside = p.len() > t.len()
                && glob_match(&p[..t.len()], &t)
                && p[t.len()..].iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '*'));
            above || inside
        })
        .map(str::to_string)
}

/// One exclude line as matched: negated (`!`), and its components (a
/// relative pattern gets a leading `**`, `$VAR` becomes a wildcard).
fn exclude_pattern(line: &str) -> Option<(bool, Vec<String>)> {
    let (neg, body) = match line.strip_prefix('!') {
        Some(b) => (true, b.trim()),
        None => (false, line),
    };
    if body.is_empty() {
        return None;
    }
    let body = expand_vars(body);
    let mut comps: Vec<String> = Vec::new();
    if !body.starts_with('/') {
        comps.push("**".into());
    }
    for c in body.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c => comps.push(c.to_string()),
        }
    }
    // `/` alone: everything.
    if comps.is_empty() {
        comps.push("**".into());
    }
    Some((neg, comps))
}

/// `$NAME` / `${NAME}` (restic expands them from its environment, which K2
/// can't see): match anything. A component that is only a variable may
/// span folders (`$HOME`), so it becomes `**`.
fn expand_vars(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('{') => {
                chars.next();
                for d in chars.by_ref() {
                    if d == '}' {
                        break;
                    }
                }
                out.push('\u{0}');
            }
            Some(d) if d.is_ascii_alphanumeric() || d == '_' => {
                while chars.peek().is_some_and(|d| d.is_ascii_alphanumeric() || *d == '_') {
                    chars.next();
                }
                out.push('\u{0}');
            }
            _ => out.push('$'),
        }
    }
    out.split('/')
        .map(|c| if c == "\u{0}" { "**".to_string() } else { c.replace('\u{0}', "*") })
        .collect::<Vec<_>>()
        .join("/")
}

/// The exclude line that drops `target`: one that, last-match-wins with
/// `!` re-includes, excludes `target`, a folder above it (restic never
/// goes into an excluded folder, so a later `!` can't bring it back), or a
/// bundle inside it.
pub fn excluding_pattern(excludes: &str, target: &str) -> Option<String> {
    let t = components(target)?;
    let pats: Vec<(&str, bool, Vec<String>)> =
        list_lines(excludes).filter_map(|l| exclude_pattern(l).map(|(neg, c)| (l, neg, c))).collect();
    for n in 1..=t.len() {
        if let Some(l) = decide(&pats, &t[..n]) {
            return Some(l.to_string());
        }
    }
    let mut bundle = t.clone();
    bundle.push(BUNDLE_PROBE);
    decide(&pats, &bundle).map(str::to_string)
}

/// The exclude line in force for `path`: the last one that matches, unless
/// it is a `!` re-include.
fn decide<'a>(pats: &[(&'a str, bool, Vec<String>)], path: &[&str]) -> Option<&'a str> {
    let mut hit = None;
    for (line, neg, comps) in pats {
        let c: Vec<&str> = comps.iter().map(String::as_str).collect();
        if glob_match(&c, path) {
            hit = if *neg { None } else { Some(*line) };
        }
    }
    hit
}

/// Whole-path glob over components: `**` is any number of components
/// (none too), anything else matches one component ([`name_match`]).
pub fn glob_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| glob_match(rest, &path[i..])),
        Some((p, rest)) => match path.split_first() {
            Some((c, tail)) => name_match(p, c) && glob_match(rest, tail),
            None => false,
        },
    }
}

/// One component: `*` (any run), `?` (one char), `[abc]` / `[a-z]` /
/// `[!x]` / `[^x]`, `\x` (a literal x). A malformed class matches nothing.
pub fn name_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    match_at(&p, &n)
}

fn match_at(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => (0..=n.len()).any(|i| match_at(&p[1..], &n[i..])),
        Some('?') => !n.is_empty() && match_at(&p[1..], &n[1..]),
        Some('[') => {
            let Some(c) = n.first() else { return false };
            match class(&p[1..], *c) {
                Some((true, used)) => match_at(&p[1 + used..], &n[1..]),
                _ => false,
            }
        }
        Some('\\') if p.len() > 1 => n.first() == Some(&p[1]) && match_at(&p[2..], &n[1..]),
        Some(x) => n.first() == Some(x) && match_at(&p[1..], &n[1..]),
    }
}

/// A `[…]` class after its `[`: whether `c` is in it and how many chars
/// (through `]`) it used; `None` when it never closes.
fn class(p: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 0;
    let neg = matches!(p.first(), Some('!' | '^'));
    if neg {
        i += 1;
    }
    let mut hit = false;
    let mut first = true;
    while i < p.len() {
        let ch = p[i];
        if ch == ']' && !first {
            return Some((hit != neg, i + 1));
        }
        first = false;
        let lo = if ch == '\\' && i + 1 < p.len() {
            i += 1;
            p[i]
        } else {
            ch
        };
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            let hi = p[i + 2];
            if lo <= c && c <= hi {
                hit = true;
            }
            i += 3;
        } else {
            if lo == c {
                hit = true;
            }
            i += 1;
        }
    }
    None
}

// ── reading ─────────────────────────────────────────────────────────────

/// A file as the daemon user read it: the answer, or permission denied
/// (then the root helper reads it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direct {
    Read(FileRead),
    Denied,
}

/// Why a root-only file can't be read when the helper can't help.
pub const HELPER_TOO_OLD: &str =
    "it is root-only and this K2's root helper can't read it for K2 yet (it needs root helper protocol 13)";

/// Does this helper serve `engine sew box-backup`?
pub fn helper_serves(st: &RootHelperState) -> bool {
    st.usable(k2_root_proto::version::SEW_BOX_BACKUP_MIN_PROTOCOL)
        && st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/sewboxbackup")).and_then(Value::as_bool) == Some(true)
}

/// The facts: the daemon's own reads, and the root helper's whole answer
/// when either file is root-only (and the helper has the verb).
pub fn read_with(
    st: &RootHelperState,
    direct: impl Fn(&str) -> Direct,
    timer: impl FnOnce() -> String,
    helper: impl FnOnce(&[String]) -> Result<Value, String>,
) -> BoxBackupFacts {
    let paths = direct(PATHS_FILE);
    let excludes = direct(EXCLUDES_FILE);
    if paths == Direct::Denied || excludes == Direct::Denied {
        if helper_serves(st) {
            let argv = ["engine", "sew", "box-backup"].map(str::to_string);
            return match helper(&argv).and_then(|v| serde_json::from_value::<BoxBackupFacts>(v).map_err(|e| format!("answer: {e}"))) {
                Ok(f) => f,
                Err(e) => {
                    let err = FileRead::Error { error: format!("the root helper couldn't read it: {e}") };
                    BoxBackupFacts { paths: err.clone(), excludes: err, timer: timer() }
                }
            };
        }
    }
    let settle = |d: Direct| match d {
        Direct::Read(r) => r,
        Direct::Denied => FileRead::Error { error: HELPER_TOO_OLD.to_string() },
    };
    BoxBackupFacts { paths: settle(paths), excludes: settle(excludes), timer: timer() }
}

/// Read `CONFIG_DIR/<name>` as the daemon user.
pub fn direct_read(name: &str) -> Direct {
    use std::io::Read;
    let path = file_path(name);
    let f = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Direct::Read(FileRead::Missing),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Direct::Denied,
        Err(e) => return Direct::Read(FileRead::Error { error: e.to_string() }),
    };
    match f.metadata() {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Direct::Read(FileRead::Error { error: "not a regular file".into() }),
        Err(e) => return Direct::Read(FileRead::Error { error: e.to_string() }),
    }
    let cap = k2_root_proto::box_backup::CAP_FILE;
    let mut buf = Vec::new();
    if let Err(e) = f.take(cap as u64 + 1).read_to_end(&mut buf) {
        return Direct::Read(FileRead::Error { error: e.to_string() });
    }
    if buf.len() > cap {
        return Direct::Read(FileRead::Error { error: format!("larger than {cap} bytes") });
    }
    match String::from_utf8(buf) {
        Ok(text) => Direct::Read(FileRead::Text { text }),
        Err(_) => Direct::Read(FileRead::Error { error: "not UTF-8 text".into() }),
    }
}

/// `systemctl is-enabled k2-backup.timer` as the daemon user (no root
/// needed); `unknown` when it says nothing.
pub fn timer_word() -> String {
    k2_core::bg_proc::command("/usr/bin/systemctl")
        .args(["is-enabled", TIMER_UNIT])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

static CACHE: Mutex<Option<(Instant, BoxBackupFacts)>> = Mutex::new(None);

/// [`CACHE_SECS`]-old facts, or a fresh read.
pub fn cached(read: impl FnOnce() -> BoxBackupFacts) -> BoxBackupFacts {
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, f)) = c.as_ref() {
        if at.elapsed() < Duration::from_secs(CACHE_SECS) {
            return f.clone();
        }
    }
    let f = read();
    *c = Some((Instant::now(), f.clone()));
    f
}

/// Drop the cache (the owner just chose `box-backup`: judge it fresh).
pub fn forget() {
    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// The evidence on this box now.
pub fn evidence(env: &super::supervisor::Env) -> Evidence {
    judge(&env.ops.box_backup_facts())
}

#[cfg(test)]
#[path = "box_backup_tests.rs"]
mod tests;
