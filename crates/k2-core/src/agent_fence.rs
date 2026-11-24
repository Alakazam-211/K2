//! Agent Isolation M1 (`plan-agent-isolation-build-v1` §3.1 steps 2–3, §4):
//! the exec helper that puts an agent's terminal into its own cgroup and,
//! when the owner turned the agent's folder fence on, under a Landlock
//! fence, before the agent's program runs.
//!
//! The daemon starts every isolated session as
//! `k2-daemon --agent-exec -- <program> <args…>` with the plan in the
//! environment ([`ENV_SPEC`], a JSON [`ExecSpec`]). The helper, already the
//! session's child (same pid, same PTY, same session):
//! 1. joins its agent's cgroup (writes `0` to its `cgroup.procs`) and
//!    reads `/proc/self/cgroup` back: anything else ends the spawn;
//! 2. with a fence: sets no-new-privileges and applies one Landlock
//!    ruleset (the [`FenceView`]): read-write for the workspace, its
//!    private temp dir, its own harness login and the shared build caches;
//!    read-only for the system and the toolchains; nothing else. No rule
//!    covers `/sys/fs/cgroup` for writing or `~/.k2` (only its CLI, the
//!    daemon's port file and its own login slot), so the agent can't leave
//!    its cgroup, read the owner token or open a sibling's folder;
//! 3. drops [`ENV_SPEC`] and `exec`s the program.
//!
//! Every failure ends the spawn with one line on the terminal and exit
//! 126: an isolated session never runs outside its cgroup or unfenced
//! ("never silently unfenced", plan §6 ground rules).
//!
//! The same code runs for both terminal backends (k2-ptyd and the
//! in-process one), so neither needs async-signal-safe code after `fork`.
//!
//! Linux only. Elsewhere [`run_from_args`] refuses (the daemon never
//! wraps a spawn there).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The helper's first argument (`k2-daemon --agent-exec -- <argv…>`).
pub const ARG: &str = "--agent-exec";
/// The helper's plan, as JSON (removed before the program runs).
pub const ENV_SPEC: &str = "K2_AGENT_EXEC";
/// The helper's exit status when it refuses to start the program.
pub const EXIT_REFUSED: i32 = 126;

/// What the helper does before `exec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecSpec {
    /// Format version (1).
    pub v: u32,
    /// The agent cgroup's directory (`/sys/fs/cgroup/<unit>/agents/a-<ws>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<String>,
    /// M1b: the root helper moves this process into the agent's root-owned
    /// cgroup (this directory) once the spawn reports its pid; the helper
    /// waits for it (bounded), and runs nothing until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_cgroup: Option<String>,
    /// The folder fence, when the agent's owner turned it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<FenceView>,
    /// Variables removed before `exec` (both terminal backends start the
    /// child with the daemon's whole environment, so only the helper can
    /// take one away).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
    /// Variables set before `exec`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, String>,
    /// M2: the Unix-socket block (fenced sessions only): the helper stays
    /// as the session's supervisor ([`crate::agent_sockblock`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sockblock: Option<crate::agent_sockblock::BlockSpec>,
}

/// One fence: the paths the agent may open. Paths that don't exist are
/// skipped, except `required` ones (the workspace).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FenceView {
    pub rw: Vec<String>,
    pub ro: Vec<String>,
    #[serde(default)]
    pub required: Vec<String>,
}

// ── the default view (plan §4, hard-coded for M1: no grant table) ─────

/// What [`default_view`] needs.
pub struct ViewInputs<'a> {
    pub home: &'a Path,
    pub k2_home: &'a Path,
    /// The agent's workspace folder.
    pub workspace: &'a Path,
    /// The agent's private temp dir (made by the daemon, 0700).
    pub tmp_dir: &'a Path,
    /// The session's environment as the daemon built it (the harness home
    /// variables name the login slot this session runs on).
    pub env: &'a BTreeMap<String, String>,
    /// The secrets folder the vault made for this session (review V2: never
    /// taken from the environment, which a spawn caller can set).
    pub own_secrets: Option<&'a Path>,
}

/// System paths an agent reads (and executes from).
pub const SYSTEM_RO: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/nix", "/snap", "/run", "/var",
    "/proc", "/sys", "/dev", "/tmp", "/home/linuxbrew/.linuxbrew",
];

/// Devices a terminal program writes.
pub const DEVICES_RW: &[&str] = &[
    "/dev/null", "/dev/zero", "/dev/full", "/dev/random", "/dev/urandom", "/dev/tty", "/dev/pts", "/dev/ptmx", "/dev/shm",
];

/// Toolchains and non-secret dotfiles under `$HOME`, read-only.
pub const HOME_RO: &[&str] = &[
    ".rustup", ".cargo/bin", ".cargo/env", ".cargo/config", ".cargo/config.toml", ".nvm", ".bun/bin", ".deno/bin",
    ".local/bin", ".local/share/pnpm", ".pyenv", ".volta", ".gitconfig", ".config/git", ".bashrc", ".bash_profile",
    ".profile", ".zshrc", ".zshenv", ".zprofile", ".inputrc", ".tmux.conf", ".vimrc", ".config/nvim", ".editorconfig",
    ".npmrc.k2-allowed",
];

/// Shared build caches, read-write (plan Q6: yes for one owner's agents).
pub const HOME_CACHES_RW: &[&str] = &[
    ".cargo/registry", ".cargo/git", ".npm", ".bun/install/cache", ".cache/pip", ".cache/uv", ".cache/go-build",
    "go/pkg/mod", ".cache/pnpm", ".local/share/pnpm/store", ".cache/yarn",
];

/// The harness homes a session may run on: env var, default dir, default
/// extra file.
const HARNESS_HOMES: &[(&str, &str, Option<&str>)] = &[
    ("CLAUDE_CONFIG_DIR", ".claude", Some(".claude.json")),
    ("CODEX_HOME", ".codex", None),
    ("GROK_HOME", ".grok", None),
    ("GEMINI_HOME", ".gemini", None),
];

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `true` when `inner` is `outer` or under it (lexically).
fn under(inner: &Path, outer: &Path) -> bool {
    inner.starts_with(outer)
}

/// The git dirs a worktree workspace commits into (`<ws>/.git` is a file
/// `gitdir: X`; X's `commondir` names the main `.git`).
pub fn git_dirs(workspace: &Path) -> Vec<PathBuf> {
    let dot = workspace.join(".git");
    let Ok(text) = std::fs::read_to_string(&dot) else { return Vec::new() };
    let Some(gd) = text.lines().find_map(|l| l.strip_prefix("gitdir:")).map(str::trim) else { return Vec::new() };
    let gd = if Path::new(gd).is_absolute() { PathBuf::from(gd) } else { workspace.join(gd) };
    let mut out = vec![gd.clone()];
    if let Ok(c) = std::fs::read_to_string(gd.join("commondir")) {
        let c = c.trim();
        let common = if Path::new(c).is_absolute() { PathBuf::from(c) } else { gd.join(c) };
        out.push(common);
    }
    out
}

/// The fence for one agent session. Refuses a workspace the fence can't
/// separate from the owner's secrets (the home folder itself, or a folder
/// holding `~/.k2`).
pub fn default_view(i: &ViewInputs) -> Result<FenceView, String> {
    default_view_in(i, &secrets_roots())
}

/// The session's own secrets folder (`$K2_SECRETS_DIR`, set by the vault at
/// spawn) as an environment variable name.
pub const SECRETS_DIR_ENV: &str = "K2_SECRETS_DIR";

/// The vault's secrets-root candidates on Linux, in its order (the vault
/// and the fence take them from here, so they can't disagree): the
/// daemon's `$K2_RUNTIME_DIR` alone when set; else the unit's
/// `RuntimeDirectory` (not in test processes), `$XDG_RUNTIME_DIR/k2`,
/// `/run/k2`, and last `/dev/shm/k2-<uid>` (review V3: only when nothing
/// else can be made; fenced sessions then can't create shared memory).
pub fn secrets_root_candidates(override_dir: Option<&Path>, systemd_dir: Option<&Path>, xdg_runtime_dir: Option<&Path>, uid: u32, is_test: bool) -> Vec<PathBuf> {
    if let Some(d) = override_dir {
        return vec![d.to_path_buf()];
    }
    let mut v = Vec::new();
    if let Some(s) = systemd_dir.filter(|_| !is_test) {
        v.push(s.to_path_buf());
    }
    if let Some(x) = xdg_runtime_dir {
        v.push(x.join("k2"));
    }
    v.push(PathBuf::from("/run/k2"));
    v.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    v
}

/// The secrets root that is `/dev/shm`'s, when the box keeps session
/// secrets there (no other root could be made): `agents check` and the
/// fence switch say fenced sessions there can't create shared memory.
pub fn shm_secrets_root(roots: &[PathBuf]) -> Option<&PathBuf> {
    roots.iter().find(|r| r.starts_with("/dev/shm"))
}

/// Where K2 keeps per-session secrets folders and its runtime files on this
/// box, those that exist: every vault candidate (above, with and without
/// the override), K2's runtime dir chain (`/run/user/<uid>/k2`), and the
/// vault's ssh-agent fallback folders (`/tmp/k2a-<uid>-<ns>`). A fenced view
/// leaves them all out but the session's own folder: they hold other
/// sessions' secret files, passports and ssh-agent sockets, and `/run`,
/// `/tmp` and `/dev/shm` are otherwise open.
pub fn secrets_roots() -> Vec<PathBuf> {
    #[cfg(unix)]
    // SAFETY: geteuid has no failure mode.
    let uid = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let uid = 0u32;
    let env = |k: &str| std::env::var_os(k).filter(|x| !x.is_empty()).map(PathBuf::from);
    let xdg = env("XDG_RUNTIME_DIR");
    let mut v: Vec<PathBuf> = Vec::new();
    v.extend(env(crate::runtime_dir::RUNTIME_DIR_ENV));
    v.extend(secrets_root_candidates(None, crate::runtime_dir::systemd_runtime_directory().as_deref(), xdg.as_deref(), uid, false));
    v.push(PathBuf::from(format!("/run/user/{uid}/k2")));
    let agent_prefix = format!("k2a-{uid}-");
    if let Ok(rd) = std::fs::read_dir("/tmp") {
        v.extend(rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&agent_prefix)).map(|e| e.path()));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for p in v.into_iter().filter(|p| p.is_absolute() && p.exists()) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// [`default_view`] with the secrets roots given (tests).
pub fn default_view_in(i: &ViewInputs, secrets: &[PathBuf]) -> Result<FenceView, String> {
    if !i.workspace.is_absolute() {
        return Err(format!("the workspace path {} isn't absolute", i.workspace.display()));
    }
    // Checked and opened as the kernel will see them (review F2: a
    // workspace path that is a link to the home would pass a lexical check).
    let ws_c = canon(i.workspace);
    let ws = ws_c.as_path();
    let (home_c, k2_c) = (canon(i.home), canon(i.k2_home));
    if ws == home_c || under(&k2_c, ws) || under(&home_c, ws) || ws == i.home || under(i.k2_home, ws) || under(i.home, ws) {
        return Err(format!(
            "the workspace {} holds the home folder or ~/.k2, so a fence can't keep the owner's keys out; \
             give the agent its own folder first",
            ws.display()
        ));
    }
    if under(ws, i.k2_home) || under(ws, &k2_c) {
        return Err(format!("the workspace {} is inside ~/.k2", ws.display()));
    }
    let mut v = FenceView { required: vec![s(ws)], ..FenceView::default() };
    v.ro.extend(SYSTEM_RO.iter().map(|p| p.to_string()));
    v.rw.extend(DEVICES_RW.iter().map(|p| p.to_string()));
    v.rw.push(s(ws));
    v.rw.push(s(i.tmp_dir));
    // M2 review F2: the workspace's `.git` file is the agent's own, so the
    // git dirs it names are never opened from it. One that resolves inside
    // the workspace is already open; one outside (a worktree of an outside
    // repo, or a file planted to name another folder) refuses the fence.
    for d in git_dirs(ws) {
        let dc = canon(&d);
        if !dc.starts_with(ws) {
            return Err(format!(
                "the workspace's .git names a git folder outside it ({}); a fence opens only the workspace, \
                 so give the agent a clone of its own instead of a worktree",
                d.display()
            ));
        }
    }
    for rel in HOME_RO {
        v.ro.push(s(&i.home.join(rel)));
    }
    for rel in HOME_CACHES_RW {
        v.rw.push(s(&i.home.join(rel)));
    }
    // The CLI and the daemon's port (never the token files), and K2's own
    // hook scripts (notify, status line), read and run, when the folder
    // holds only plain files of this user's (no links, no folders).
    v.ro.push(s(&i.k2_home.join("bin")));
    if hooks_are_plain(&i.k2_home.join("hooks")) {
        v.ro.push(s(&i.k2_home.join("hooks")));
    }
    v.ro.push(s(&i.k2_home.join("daemon.port")));
    v.ro.push(s(&i.k2_home.join("heartbeat.port")));
    // The harness logins this session runs on: exactly its own slot when
    // pinned (`~/.k2/llm-accounts/<tool>/<id>/`), else the tool's own home.
    let slots = i.k2_home.join("llm-accounts");
    harness_rules(i, &slots, &mut v)?;
    // No rule may cover the home folder or ~/.k2 as a whole (a home under
    // /tmp, or under /var, would sit inside a system rule): such a rule is
    // replaced by its contents minus the path down to them.
    // Nor K2's secrets roots (other sessions' secret files, passports and
    // ssh-agent sockets): the session's own folder is opened again by name,
    // read-only, when it is one of theirs.
    let mut avoid: Vec<PathBuf> = [i.home, i.k2_home].iter().map(|p| canon(p)).collect();
    let roots: Vec<PathBuf> = secrets.iter().map(|p| canon(p)).collect();
    avoid.extend(roots.iter().cloned());
    // Review V1 (and its re-review R1): the base rules go to the exec helper
    // resolved, BEFORE the carve, so the helper can open each with no link
    // followed; a carved folder's children stay as listed (never resolved
    // again: one swapped for a link after the listing opens nothing).
    v.ro = resolve_then_carve(&v.ro, &avoid);
    v.rw = resolve_then_carve(&v.rw, &avoid);
    v.required = v.required.iter().map(|r| s(&canon(Path::new(r)))).collect();
    let own = i.own_secrets.map(canon);
    if let Some(own) = own.filter(|o| roots.iter().any(|r| o.starts_with(r) && o != r)) {
        v.ro.push(s(&own));
    }
    Ok(v)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The base rules resolved first, then carved; carved children as listed.
fn resolve_then_carve(rules: &[String], avoid: &[PathBuf]) -> Vec<String> {
    let resolved: Vec<String> = rules.iter().map(|r| s(&canon(Path::new(r)))).collect();
    carve_all(&resolved, avoid)
}

fn carve_all(rules: &[String], avoid: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rules {
        let p = canon(Path::new(r));
        if avoid.iter().any(|a| a.starts_with(&p)) {
            out.extend(carve(&p, avoid).iter().map(|c| s(c)));
        } else {
            out.push(r.clone());
        }
    }
    out
}

/// `entry`'s contents, minus every path in `avoid` and the folders on the
/// way down to them (recursively): what a rule on `entry` may open without
/// opening an avoided path.
pub fn carve(entry: &Path, avoid: &[PathBuf]) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(entry) else { return Vec::new() };
    let mut out = Vec::new();
    let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    kids.sort();
    for k in kids {
        // Review V1: a link is never a rule (it would open its target): a
        // carved folder's children are taken as they are, links skipped.
        match std::fs::symlink_metadata(&k) {
            Ok(m) if m.file_type().is_symlink() => {
                crate::log_debug!("[agents] fence view: {} is a link, left out", k.display());
                continue;
            }
            Ok(_) => {}
            Err(_) => continue,
        }
        if avoid.iter().any(|a| *a == k) {
            continue;
        }
        if avoid.iter().any(|a| a.starts_with(&k)) {
            out.extend(carve(&k, avoid));
        } else {
            out.push(k);
        }
    }
    out
}

// ── what the daemon reads for an agent (M2, v2 §1.5 S0) ──────────────

/// Folders and files under `$HOME` the daemon never reads for an agent (to
/// hand over to another agent's inbox, publish or ship), fenced or not: the
/// usual credential stores and the harness logins. K2's own home is never
/// read either (except the agent's own private temp dir).
pub const HOME_NEVER: &[&str] = &[
    ".ssh", ".aws", ".azure", ".gnupg", ".config/gh", ".config/gcloud", ".config/op", ".docker", ".kube",
    ".netrc", ".git-credentials", ".npmrc", ".pypirc", ".password-store", ".local/share/keyrings",
    ".claude", ".claude.json", ".codex", ".grok", ".gemini", ".cargo/credentials", ".cargo/credentials.toml",
    "Library/Keychains",
];

/// Who the daemon reads for: one agent's folders.
pub struct AgentReach<'a> {
    pub home: &'a Path,
    pub k2_home: &'a Path,
    /// The agent's workspace folder.
    pub workspace: &'a Path,
    /// The agent's private temp dir (`~/.k2/agents/<ws>/tmp`), when it has one.
    pub tmp_dir: Option<&'a Path>,
    /// The agent's folder fence is on: only its own folders.
    pub fenced: bool,
}

/// Why the daemon may not read `path` for this agent; `None`: it may. The
/// path is resolved (links followed) first; one that can't be resolved is
/// refused. Then [`resolved_refusal`].
pub fn reach_refusal(r: &AgentReach, path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return Some(format!("{} isn't an absolute path", path.display()));
    }
    match std::fs::canonicalize(path) {
        Ok(p) => resolved_refusal(r, &p, path),
        Err(e) => Some(format!("{} can't be resolved ({})", path.display(), e.kind())),
    }
}

/// [`reach_refusal`] for a path already resolved (`real`; `shown` names it
/// in the reason). Refusals first, in this order, before anything is
/// allowed: K2's own folder (except the agent's own temp dir), a folder
/// holding it, a credential folder of [`HOME_NEVER`] (by where it resolves
/// too) or a folder holding one. Then a fenced agent reads only inside its
/// workspace and its temp dir. Nothing is taken from the agent's own files
/// (a repo pointer file in the workspace naming another folder counts for
/// nothing here).
pub fn resolved_refusal(r: &AgentReach, real: &Path, shown: &Path) -> Option<String> {
    let k2 = canon(r.k2_home);
    // The temp dir counts only where it really is, inside K2's folder.
    let tmp = r.tmp_dir.map(canon).filter(|t| t.starts_with(&k2));
    let in_tmp = tmp.as_ref().is_some_and(|t| under(real, t));
    if !in_tmp && (under(real, &k2) || under(real, r.k2_home)) {
        return Some(format!("{} is in K2's own folder, which K2 never reads for an agent", shown.display()));
    }
    if k2.starts_with(real) || r.k2_home.starts_with(real) {
        return Some(format!("{} holds K2's own folder, which K2 never reads for an agent", shown.display()));
    }
    let home = canon(r.home);
    for rel in HOME_NEVER {
        let spots = [home.join(rel), r.home.join(rel), canon(&r.home.join(rel))];
        if spots.iter().any(|c| under(real, c)) {
            return Some(format!("{} is in ~/{rel}, a credential folder K2 never reads for an agent (secrets go through the vault)", shown.display()));
        }
        if spots.iter().any(|c| c.starts_with(real) && c.as_path() != real) {
            return Some(format!("{} holds ~/{rel}, a credential folder K2 never reads for an agent", shown.display()));
        }
    }
    if r.fenced && !in_tmp && !under(real, &canon(r.workspace)) {
        return Some(format!("{} is outside this agent's workspace, and its folder fence is on", shown.display()));
    }
    None
}

/// Open `path` once for the daemon to read for this agent, and check the
/// file it actually opened: links are followed by the kernel at the open,
/// the opened file's own path is read back from the handle and checked
/// ([`resolved_refusal`]), and the caller reads from the handle, never by
/// name again (a swap after the check changes nothing). Only a regular
/// file; never blocks on a FIFO. `Ok((file, real path))`.
#[cfg(unix)]
pub fn open_for_agent(r: &AgentReach, path: &Path) -> Result<(std::fs::File, PathBuf), String> {
    use std::os::unix::fs::OpenOptionsExt;
    if !path.is_absolute() {
        return Err(format!("{} isn't an absolute path", path.display()));
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(path)
        .map_err(|e| format!("{} can't be opened ({})", path.display(), e.kind()))?;
    let meta = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a regular file", path.display()));
    }
    // M2 review: a hard link made before the fence went on is a second name
    // for a file the fence may not reach; a fenced agent's file has one.
    {
        use std::os::unix::fs::MetadataExt;
        if r.fenced && meta.nlink() > 1 {
            return Err(format!("{} has more than one name (a hard link), and this agent's folder fence is on: copy it first", path.display()));
        }
    }
    let real = fd_path(&f).ok_or_else(|| format!("{}: can't read where the opened file is", path.display()))?;
    if let Some(why) = resolved_refusal(r, &real, path) {
        return Err(why);
    }
    Ok((f, real))
}

/// The path of an open file, from the handle (Linux: `/proc/self/fd`;
/// macOS: `F_GETPATH`).
#[cfg(unix)]
pub fn fd_path(f: &std::fs::File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/self/fd/{}", f.as_raw_fd())).ok().filter(|p| p.is_absolute())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH writes at most PATH_MAX bytes into `buf`.
        let rc = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
        if rc == -1 {
            return None;
        }
        let n = buf.iter().position(|b| *b == 0)?;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n])))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = f.as_raw_fd();
        None
    }
}

/// The harness logins this session runs on: exactly its own slot when
/// pinned (`~/.k2/llm-accounts/<tool>/<id>/`), else the tool's own home.
fn harness_rules(i: &ViewInputs, slots: &Path, v: &mut FenceView) -> Result<(), String> {
    for (var, default_dir, default_file) in HARNESS_HOMES {
        match i.env.get(*var).map(String::as_str).filter(|v| !v.trim().is_empty()) {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                let tool_home = i.home.join(default_dir);
                // Only one login slot (`~/.k2/llm-accounts/<tool>/<id>`), the
                // tool's own default home, or the home the daemon itself was
                // started with for that tool (its owner's choice). Anything
                // else gets no rule and the spawn refuses (review F2).
                let rel = dir.strip_prefix(slots).ok().map(|r| r.components().count());
                let slot = rel == Some(2) && !dir.components().any(|c| matches!(c, std::path::Component::ParentDir));
                let daemon_own = std::env::var(var).ok().filter(|d| !d.trim().is_empty()).map(PathBuf::from);
                let known = slot || canon(&dir) == canon(&tool_home) || daemon_own.as_deref().is_some_and(|d| canon(d) == canon(&dir));
                if !known {
                    return Err(format!("{var} points at {}, which is neither its login slot nor the tool's own home", dir.display()));
                }
                if under(&canon(&dir), &canon(i.k2_home)) && !slot {
                    return Err(format!("{var} points inside ~/.k2 but not at one login slot: {}", dir.display()));
                }
                v.rw.push(s(&dir));
                // A slot's links into the tool's own home: only the ones K2's
                // slot code makes, each to the same-named entry there.
                if slot {
                    for (target, write) in slot_links(&dir, &tool_home, var) {
                        if !under(&target, &canon(i.k2_home)) {
                            if write {
                                v.rw.push(s(&target));
                            } else {
                                v.ro.push(s(&target));
                            }
                        }
                    }
                }
            }
            None => {
                v.rw.push(s(&i.home.join(default_dir)));
                if let Some(f) = default_file {
                    v.rw.push(s(&i.home.join(f)));
                }
            }
        }
    }
    Ok(())
}

/// The Codex home entries a fenced session may reach through its slot's
/// links, by name (review R2): `Some(true)` read-write (its conversations
/// and their state), `Some(false)` read-only (shared skills, prompts,
/// instructions), `None` never: `auth.json`, `config.toml` (its notify and
/// MCP commands run in unfenced sessions), and every name not listed (a
/// link K2 didn't make, or a new Codex entry, stays outside the fence
/// until it is listed here).
pub fn codex_link_access(name: &str) -> Option<bool> {
    const RW: &[&str] = &["sessions", "archived_sessions", "history.jsonl"];
    const RO: &[&str] = &["skills", "prompts", "AGENTS.md", "rules"];
    if RW.contains(&name) || codex_state_db(name) {
        return Some(true);
    }
    RO.contains(&name).then_some(false)
}

/// Codex's state database: `state_<n>.sqlite`, with `-wal` / `-shm`.
fn codex_state_db(name: &str) -> bool {
    let base = name.strip_suffix("-wal").or_else(|| name.strip_suffix("-shm")).unwrap_or(name);
    base.strip_prefix("state_")
        .and_then(|r| r.strip_suffix(".sqlite"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The links inside a login slot that K2's slot code makes
/// (`llm_accounts::pins`), and where they point: each must resolve to the
/// same-named entry directly inside the tool's own home (a Claude slot:
/// only [`crate::llm_accounts::pins::CLAUDE_SHARED_ENTRIES`]). A link the
/// agent planted (the slot is read-write to it) to anywhere else gets no
/// rule (review F2). A Codex slot only by name too ([`codex_link_access`],
/// review R2): it writes through its conversation links and reads its
/// skills; a Claude slot only reads its linked settings and skills.
pub fn slot_links(slot: &Path, tool_home: &Path, var: &str) -> Vec<(PathBuf, bool)> {
    let tool_home = canon(tool_home);
    let Ok(rd) = std::fs::read_dir(slot) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_symlink()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let write = match var {
                "CLAUDE_CONFIG_DIR" if crate::llm_accounts::pins::CLAUDE_SHARED_ENTRIES.contains(&name.as_str()) => false,
                "CODEX_HOME" => codex_link_access(&name)?,
                _ => return None,
            };
            let target = std::fs::canonicalize(e.path()).ok()?;
            (target == tool_home.join(&name)).then_some((target, write))
        })
        .collect()
}

// ── the environment of a fenced session ───────────────────────────────

/// Variables a fenced session never gets: they hand it something outside
/// its fence (the owner's ssh-agent, the session bus).
pub const FENCE_ENV_DROP: &[&str] = &["SSH_AUTH_SOCK", "SSH_AGENT_PID", "GPG_AGENT_INFO", "DBUS_SESSION_BUS_ADDRESS"];

/// The fence's environment, as the helper applies it: the drops above,
/// and the private temp dir as `TMPDIR`.
pub fn fence_env(spec: &mut ExecSpec, tmp_dir: &Path) {
    spec.unset.extend(FENCE_ENV_DROP.iter().map(|k| k.to_string()));
    spec.set.insert("TMPDIR".into(), s(tmp_dir));
}

// ── Landlock (raw syscalls; ABI 1–5) ──────────────────────────────────

/// Landlock filesystem rights (`include/uapi/linux/landlock.h`).
pub mod access {
    pub const EXECUTE: u64 = 1 << 0;
    pub const WRITE_FILE: u64 = 1 << 1;
    pub const READ_FILE: u64 = 1 << 2;
    pub const READ_DIR: u64 = 1 << 3;
    pub const REMOVE_DIR: u64 = 1 << 4;
    pub const REMOVE_FILE: u64 = 1 << 5;
    pub const MAKE_CHAR: u64 = 1 << 6;
    pub const MAKE_DIR: u64 = 1 << 7;
    pub const MAKE_REG: u64 = 1 << 8;
    pub const MAKE_SOCK: u64 = 1 << 9;
    pub const MAKE_FIFO: u64 = 1 << 10;
    pub const MAKE_BLOCK: u64 = 1 << 11;
    pub const MAKE_SYM: u64 = 1 << 12;
    /// ABI 2.
    pub const REFER: u64 = 1 << 13;
    /// ABI 3.
    pub const TRUNCATE: u64 = 1 << 14;
    /// ABI 5.
    pub const IOCTL_DEV: u64 = 1 << 15;
    /// The rights a rule on a file (not a directory) may carry.
    pub const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
    pub const RO: u64 = EXECUTE | READ_FILE | READ_DIR;
}

/// Every filesystem right a ruleset handles at Landlock ABI `abi` (all of
/// them: what isn't granted is denied). `0` below ABI 1.
pub fn handled_for_abi(abi: i64) -> u64 {
    use access::*;
    if abi < 1 {
        return 0;
    }
    let mut h = (1u64 << 13) - 1;
    if abi >= 2 {
        h |= REFER;
    }
    if abi >= 3 {
        h |= TRUNCATE;
    }
    if abi >= 5 {
        h |= IOCTL_DEV;
    }
    h
}

/// The rights one rule carries: read-only or read-write, cut to what the
/// ruleset handles, and to file rights when the path is a file.
pub fn rule_rights(rw: bool, is_dir: bool, handled: u64) -> u64 {
    let want = if rw { handled } else { access::RO };
    let want = want & handled;
    if is_dir {
        want
    } else {
        want & access::FILE_RIGHTS
    }
}

/// Landlock scopes (ABI 6, Linux 6.12; review F11): a fenced agent can't
/// signal processes outside its domain (the daemon, ptyd, other agents) or
/// connect to an abstract Unix socket outside it. Below ABI 6 there is no
/// scope: signals stay a known gap until the Jail or Guard.
pub const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
pub const SCOPE_SIGNAL: u64 = 1 << 1;

pub fn scoped_for_abi(abi: i64) -> u64 {
    if abi >= 6 {
        SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL
    } else {
        0
    }
}

/// What the fence did (for the session's audit line and the tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FenceReport {
    pub abi: i64,
    pub rules: usize,
    pub skipped: Vec<String>,
}

#[cfg(target_os = "linux")]
mod ll {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    const CREATE_RULESET_VERSION: u32 = 1;
    const RULE_PATH_BENEATH: i32 = 1;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    /// The kernel's Landlock ABI (≤ 0: not available, with the errno).
    pub fn abi() -> Result<i64, std::io::Error> {
        // SAFETY: the version query takes no attribute.
        let r = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0usize, CREATE_RULESET_VERSION) };
        if r < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(r)
        }
    }

    pub fn create(handled: u64, scoped: u64) -> std::io::Result<OwnedFd> {
        let attr = RulesetAttr { handled_access_fs: handled, handled_access_net: 0, scoped };
        // SAFETY: attr outlives the call; the size is its size (the
        // kernel accepts a larger struct whose extra fields are zero).
        let fd = unsafe {
            libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, std::mem::size_of::<RulesetAttr>(), 0u32)
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a new fd owned from here.
        Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }

    pub fn add(ruleset: &OwnedFd, path: &OwnedFd, allowed: u64) -> std::io::Result<()> {
        let attr = PathBeneathAttr { allowed_access: allowed, parent_fd: path.as_raw_fd() };
        // SAFETY: attr outlives the call.
        let r = unsafe {
            libc::syscall(libc::SYS_landlock_add_rule, ruleset.as_raw_fd(), RULE_PATH_BENEATH, &attr as *const PathBeneathAttr, 0u32)
        };
        if r < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn restrict(ruleset: &OwnedFd) -> std::io::Result<()> {
        // SAFETY: plain prctl/syscall on our own process.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0u32) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

/// The kernel's Landlock ABI, or why there is none.
pub fn landlock_abi() -> Result<i64, String> {
    #[cfg(target_os = "linux")]
    {
        match ll::abi() {
            Ok(n) if n >= 1 => Ok(n),
            Ok(n) => Err(format!("Landlock ABI {n}")),
            Err(e) if e.raw_os_error() == Some(libc::EOPNOTSUPP) => Err("Landlock is built in but not enabled at boot (lsm=)".into()),
            Err(e) if e.raw_os_error() == Some(libc::ENOSYS) => Err("this kernel has no Landlock".into()),
            Err(e) => Err(format!("Landlock: {e}")),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("Landlock is Linux only".into())
    }
}

/// Fence this process (and everything it runs from now on) with `view`.
/// No way back: Landlock domains only ever narrow.
pub fn apply_fence(view: &FenceView) -> Result<FenceReport, String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{FromRawFd, OwnedFd};
        let abi = landlock_abi()?;
        let handled = handled_for_abi(abi);
        let ruleset = ll::create(handled, scoped_for_abi(abi)).map_err(|e| format!("Landlock ruleset: {e}"))?;
        let mut rep = FenceReport { abi, ..FenceReport::default() };
        for req in &view.required {
            if !Path::new(req).exists() {
                return Err(format!("{req} doesn't exist"));
            }
        }
        let rules = view.ro.iter().map(|p| (p, false)).chain(view.rw.iter().map(|p| (p, true)));
        for (p, rw) in rules {
            let c = std::ffi::CString::new(p.as_str()).map_err(|_| format!("NUL in {p:?}"))?;
            // Review V1: no link followed anywhere in the path (the daemon
            // sends resolved paths); a link there opens nothing.
            let fd = open_no_links(&c);
            if fd < 0 {
                let e = std::io::Error::last_os_error();
                if view.required.contains(p) {
                    return Err(format!("{p}: {e}"));
                }
                rep.skipped.push(format!("{p}: {e}"));
                continue;
            }
            // SAFETY: just opened.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let Some(is_dir) = kind_of(&fd) else {
                rep.skipped.push(format!("{p}: a link or an unknown kind"));
                continue;
            };
            ll::add(&ruleset, &fd, rule_rights(rw, is_dir, handled)).map_err(|e| format!("Landlock rule {p}: {e}"))?;
            rep.rules += 1;
        }
        ll::restrict(&ruleset).map_err(|e| format!("Landlock restrict: {e}"))?;
        Ok(rep)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = view;
        Err("the folder fence is Linux only in this release".into())
    }
}

/// `open(path, O_PATH)` following no link in any component: `openat2` with
/// `RESOLVE_NO_SYMLINKS`, or `O_NOFOLLOW` (the last component only) on a
/// kernel without it. `-1` with errno set on failure.
#[cfg(target_os = "linux")]
fn open_no_links(c: &std::ffi::CStr) -> libc::c_int {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    let how = OpenHow { flags: (libc::O_PATH | libc::O_CLOEXEC) as u64, mode: 0, resolve: RESOLVE_NO_SYMLINKS };
    // SAFETY: openat2 with a NUL-terminated path and a sized struct.
    let fd = unsafe { libc::syscall(libc::SYS_openat2, libc::AT_FDCWD, c.as_ptr(), &how as *const OpenHow, std::mem::size_of::<OpenHow>()) } as libc::c_int;
    if fd >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOSYS) {
        return fd;
    }
    // SAFETY: O_PATH open; the caller owns the fd.
    unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) }
}

/// A rule's kind from its open handle: `Some(true)` a folder, `Some(false)`
/// a file or device, `None` a link (opened as itself) or unreadable.
#[cfg(target_os = "linux")]
fn kind_of(fd: &std::os::fd::OwnedFd) -> Option<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: fstat into a zeroed struct on an open fd.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return None;
    }
    match st.st_mode & libc::S_IFMT {
        libc::S_IFLNK => None,
        libc::S_IFDIR => Some(true),
        _ => Some(false),
    }
}

/// `~/.k2/hooks` is opened only when it is a real folder of this user's
/// holding plain files only.
fn hooks_are_plain(dir: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(dir) else { return false };
    if !m.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no failure mode.
        if m.uid() != unsafe { libc::geteuid() } {
            return false;
        }
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    rd.flatten().all(|e| e.file_type().is_ok_and(|t| t.is_file()))
}

/// The largest spec the exec helper is handed (it travels in one
/// environment string; Linux takes up to 128 KiB there).
pub const SPEC_MAX: usize = 120 * 1024;

/// The spec as the exec helper reads it, or a refusal that says why when
/// the view is too large to hand over (review V4).
pub fn spec_json(spec: &ExecSpec) -> Result<String, String> {
    let json = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    if json.len() > SPEC_MAX {
        let rules = spec.fence.as_ref().map_or(0, |v| v.ro.len() + v.rw.len());
        return Err(format!(
            "the folder fence's view has {rules} rules ({} KiB), more than a session start can carry ({} KiB): a folder K2 carves (/tmp, /dev, /run) holds too many entries; the session doesn't start",
            json.len() / 1024,
            SPEC_MAX / 1024
        ));
    }
    Ok(json)
}

/// Join the cgroup at `dir` (this process; children follow) and confirm
/// the kernel put it there.
pub fn join_cgroup(dir: &str) -> Result<(), String> {
    use std::io::Write;
    let procs = Path::new(dir).join("cgroup.procs");
    let mut f = std::fs::OpenOptions::new().write(true).open(&procs).map_err(|e| format!("{}: {e}", procs.display()))?;
    f.write_all(b"0").map_err(|e| format!("join {dir}: {e}"))?;
    let me = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("/proc/self/cgroup: {e}"))?;
    let now = crate::agents_leaf::parse_self_cgroup(&me).ok_or("no cgroup v2 path in /proc/self/cgroup")?;
    let want = dir.strip_prefix(crate::agent_cgroup::CGROUP_FS).unwrap_or(dir);
    if now.trim_end_matches('/') != want.trim_end_matches('/') {
        return Err(format!("joined {dir} but the kernel says {now}"));
    }
    Ok(())
}

/// How long the helper waits for the root helper to place it (M1b).
pub const WAIT_PLACED_SECS: u64 = 30;

/// M1b: wait until the kernel says this process is in `dir` (the root
/// helper moved it there), and that the cgroup is root's.
pub fn wait_placed(dir: &str, within: std::time::Duration) -> Result<(), String> {
    let want = dir.strip_prefix(crate::agent_cgroup::CGROUP_FS).unwrap_or(dir).trim_end_matches('/').to_string();
    let t = std::time::Instant::now();
    loop {
        let me = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("/proc/self/cgroup: {e}"))?;
        let now = crate::agents_leaf::parse_self_cgroup(&me).ok_or("no cgroup v2 path in /proc/self/cgroup")?;
        if now.trim_end_matches('/') == want {
            // Review B3: the cgroup it is in now is root's and holds it, by
            // one handle (never a lookup by name after reading /proc).
            if !crate::agent_cgroup::held_by(std::path::Path::new(dir), std::process::id(), 0) {
                return Err(format!("{dir} is not a root-owned cgroup holding this process"));
            }
            return Ok(());
        }
        if t.elapsed() >= within {
            return Err(format!("the root helper didn't place it in {dir} within {}s (still in {now})", within.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The helper: `k2-daemon --agent-exec -- <program> [args…]` (argv as the
/// process got it). Never returns on success.
pub fn run_from_args(args: &[std::ffi::OsString]) -> i32 {
    let refuse = |why: String| -> i32 {
        eprintln!("{} agent isolation refused to start this terminal: {why}. Nothing ran.", crate::brand::PRODUCT_NAME);
        EXIT_REFUSED
    };
    let rest: Vec<&std::ffi::OsString> = args.iter().skip(2).collect();
    let (dashdash, argv) = match rest.split_first() {
        Some((d, a)) => (d.to_str() == Some("--"), a),
        None => (false, &[][..]),
    };
    if !dashdash || argv.is_empty() {
        return refuse("usage: --agent-exec -- <program> [args…]".into());
    }
    let Some(raw) = std::env::var_os(ENV_SPEC) else { return refuse(format!("{ENV_SPEC} is missing")) };
    let spec: ExecSpec = match serde_json::from_slice(raw.as_encoded_bytes()) {
        Ok(s) => s,
        Err(e) => return refuse(format!("{ENV_SPEC}: {e}")),
    };
    if spec.v != 1 {
        return refuse(format!("{ENV_SPEC} version {} (this build reads 1)", spec.v));
    }
    if let Some(dir) = spec.cgroup.as_deref() {
        if let Err(e) = join_cgroup(dir) {
            return refuse(format!("its agent cgroup: {e}"));
        }
    }
    if let Some(dir) = spec.wait_cgroup.as_deref() {
        if let Err(e) = wait_placed(dir, std::time::Duration::from_secs(WAIT_PLACED_SECS)) {
            return refuse(format!("its agent cgroup: {e}"));
        }
    }
    // M2: the Unix-socket block's log and temp folder handle are opened
    // before the fence (the log is outside the view); only with a fence.
    #[cfg(target_os = "linux")]
    let block = match (spec.sockblock.as_ref(), spec.fence.is_some()) {
        (Some(b), true) => match crate::agent_sockblock::supervisor::prepare(b) {
            Ok(p) => p,
            Err(e) => return refuse(format!("its Unix-socket block: {e}")),
        },
        (Some(_), false) => return refuse("a Unix-socket block without a folder fence".into()),
        (None, _) => None,
    };
    #[cfg(not(target_os = "linux"))]
    if spec.sockblock.is_some() {
        return refuse("the Unix-socket block is Linux only".into());
    }
    if let Some(view) = spec.fence.as_ref() {
        if let Err(e) = apply_fence(view) {
            return refuse(format!("its folder fence: {e}"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The program's environment: this one minus the plan and the
        // fence's drops, plus its settings (this process's own stays put).
        // Under the block the program is the supervisor's child, in this
        // session (never one of its own: it keeps the terminal).
        #[cfg(target_os = "linux")]
        let mut c = if block.is_some() { crate::bg_proc::command_keeping_session(argv[0]) } else { crate::bg_proc::command(argv[0]) };
        #[cfg(not(target_os = "linux"))]
        let mut c = crate::bg_proc::command(argv[0]);
        c.args(&argv[1..]).env_remove(ENV_SPEC);
        for k in &spec.unset {
            c.env_remove(k);
        }
        for (k, v) in &spec.set {
            c.env(k, v);
        }
        #[cfg(target_os = "linux")]
        if let Some(p) = block {
            return crate::agent_sockblock::supervisor::run(p, c);
        }
        let e = c.exec();
        eprintln!("{}: {}: {e}", crate::brand::PRODUCT_NAME, argv[0].to_string_lossy());
        127
    }
    #[cfg(not(unix))]
    {
        refuse("Unix only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2 (S0): what the daemon reads for an agent. A fenced agent: its
    /// workspace and private temp dir only. Any agent: never K2's home or
    /// a credential folder, also through a link in its workspace.
    #[cfg(unix)]
    #[test]
    fn the_daemon_reads_only_what_the_agent_may() {
        let root = std::env::temp_dir().join(format!("k2-reach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let k2 = home.join(".k2");
        let ws = home.join("DevProjects/A");
        let other = home.join("DevProjects/B");
        let tmp = k2.join("agents/ws-a/tmp");
        for d in [&ws, &other, &tmp, &home.join(".ssh"), &home.join(".config/gh")] {
            std::fs::create_dir_all(d).unwrap();
        }
        for f in [ws.join("report.md"), other.join("notes.md"), tmp.join("bundle.tar.gz"), home.join(".ssh/id_ed25519"), k2.join("daemon.token"), home.join(".config/gh/hosts.yml")] {
            std::fs::write(&f, "x").unwrap();
        }
        std::os::unix::fs::symlink(home.join(".ssh/id_ed25519"), ws.join("key")).unwrap();
        std::os::unix::fs::symlink(k2.join("daemon.token"), ws.join("token")).unwrap();
        for fenced in [false, true] {
            let r = AgentReach { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: Some(&tmp), fenced };
            let what = if fenced { "fenced" } else { "unfenced" };
            assert_eq!(reach_refusal(&r, &ws.join("report.md")), None, "{what}: its workspace");
            assert_eq!(reach_refusal(&r, &tmp.join("bundle.tar.gz")), None, "{what}: its temp dir");
            for secret in [home.join(".ssh/id_ed25519"), k2.join("daemon.token"), home.join(".config/gh/hosts.yml"), ws.join("key"), ws.join("token")] {
                assert!(reach_refusal(&r, &secret).is_some(), "{what}: {} is refused", secret.display());
            }
            assert!(reach_refusal(&r, Path::new("relative.md")).is_some(), "{what}: a relative path");
            assert_eq!(reach_refusal(&r, &other.join("notes.md")).is_some(), fenced, "{what}: another workspace");
            assert_eq!(reach_refusal(&r, &ws.join("../B/notes.md")).is_some(), fenced, "{what}: another workspace by ..");
        }
        // Another agent's temp dir is K2's folder to this one.
        let r = AgentReach { home: &home, k2_home: &k2, workspace: &other, tmp_dir: Some(&k2.join("agents/ws-b/tmp")), fenced: false };
        assert!(reach_refusal(&r, &tmp.join("bundle.tar.gz")).is_some(), "another agent's temp dir");
        // M2 review: a folder holding K2's folder or a credential folder (the
        // home, the root) is refused; an unresolvable path is refused for
        // every agent (L4).
        for fenced in [false, true] {
            let r = AgentReach { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: Some(&tmp), fenced };
            assert!(reach_refusal(&r, &home).is_some(), "the home folder");
            assert!(reach_refusal(&r, Path::new("/")).is_some(), "the root");
            assert!(reach_refusal(&r, &ws.join("gone.md")).is_some(), "an unresolvable path");
        }
        // L3: a credential folder that is itself a link is matched by where it resolves.
        let vault_dir = root.join("elsewhere-aws");
        std::fs::create_dir_all(&vault_dir).unwrap();
        std::fs::write(vault_dir.join("credentials"), "x").unwrap();
        std::os::unix::fs::symlink(&vault_dir, home.join(".aws")).unwrap();
        let r = AgentReach { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: Some(&tmp), fenced: false };
        assert!(reach_refusal(&r, &home.join(".aws/credentials")).is_some(), "~/.aws through its link");
        assert!(reach_refusal(&r, &vault_dir.join("credentials")).is_some(), "~/.aws's target by its own name");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M2 review F2: the workspace's `.git` file is the agent's own. A git
    /// folder it names counts for nothing in the reach check, and the fence
    /// refuses one outside the workspace (a planted `.git` naming `~/.ssh`,
    /// or a `commondir` of `../../../.k2`) instead of opening it.
    #[cfg(unix)]
    #[test]
    fn a_planted_git_file_opens_nothing() {
        let root = std::env::temp_dir().join(format!("k2-gitfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let k2 = home.join(".k2");
        let ws = home.join("DevProjects/A");
        let tmp = k2.join("agents/ws-a/tmp");
        for d in [&ws, &tmp, &home.join(".ssh"), &ws.join("g")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(home.join(".ssh/id_ed25519"), "KEY").unwrap();
        std::fs::write(k2.join("daemon.token"), "TOKEN").unwrap();
        let env = BTreeMap::new();
        let view = |ws: &Path| default_view(&ViewInputs { home: &home, k2_home: &k2, workspace: ws, tmp_dir: &tmp, env: &env, own_secrets: None });
        // `.git` naming ~/.ssh.
        std::fs::write(ws.join(".git"), format!("gitdir: {}\n", home.join(".ssh").display())).unwrap();
        let r = AgentReach { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: Some(&tmp), fenced: true };
        assert!(reach_refusal(&r, &home.join(".ssh/id_ed25519")).is_some(), "a git dir named by the agent opens nothing");
        let e = view(&ws).expect_err("the fence refuses a git folder outside the workspace");
        assert!(e.contains("outside it"), "{e}");
        // `.git` naming a folder inside, whose commondir climbs to ~/.k2.
        std::fs::write(ws.join(".git"), "gitdir: g\n").unwrap();
        std::fs::write(ws.join("g/commondir"), "../../../.k2\n").unwrap();
        let e = view(&ws).expect_err("a commondir out to ~/.k2 refuses the fence");
        assert!(e.contains("outside it"), "{e}");
        // Both inside the workspace: the fence is the workspace's, nothing more.
        std::fs::create_dir_all(ws.join("main.git")).unwrap();
        std::fs::write(ws.join("g/commondir"), "../main.git\n").unwrap();
        let v = view(&ws).expect("git folders inside the workspace are fine");
        for p in v.rw.iter().chain(&v.ro) {
            let p = canon(Path::new(p));
            assert!(!p.starts_with(canon(&k2)) || p.starts_with(canon(&tmp)) || p.ends_with("bin") || p.ends_with("hooks") || p.ends_with("daemon.port") || p.ends_with("heartbeat.port"), "{}", p.display());
            assert!(!p.starts_with(canon(&home.join(".ssh"))), "{}", p.display());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The secrets roots leave a fenced view (other sessions' secret files,
    /// passports and ssh-agent sockets), except the folder the vault made for
    /// this session (review V2: an input of its own, never the environment),
    /// opened read-only; the rest of a folder holding a root stays open;
    /// `~/.k2/hooks` is read and run when it holds plain files only.
    #[test]
    fn a_fenced_view_leaves_other_sessions_secrets_out() {
        let root = canon(&std::env::temp_dir()).join(format!("k2-secview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let k2 = home.join(".k2");
        let ws = home.join("DevProjects/A");
        let tmp = k2.join("agents/ws-a/tmp");
        let secrets = root.join("run-k2");
        let (own, other) = (secrets.join("own-1"), secrets.join("other-2"));
        let beside = root.join("beside");
        for d in [&ws, &tmp, &own, &other, &beside, &k2.join("hooks")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(other.join(".k2-passport"), "P").unwrap();
        std::fs::write(k2.join("hooks/notify.sh"), "#!/bin/sh\n").unwrap();
        let covers = |rules: &[String], p: &Path| rules.iter().any(|r| canon(p).starts_with(canon(Path::new(r))));
        // A system-wide rule over the root (the way `/run` and `/dev/shm` are).
        let carved = carve_all(&[root.display().to_string()], &[canon(&home), canon(&secrets)]);
        assert!(!covers(&carved, &other.join(".k2-passport")) && !covers(&carved, &own), "{carved:?}");
        assert!(covers(&carved, &beside), "what sits beside the roots stays open: {carved:?}");
        // The view: the vault's folder is opened, read-only.
        let mut env = BTreeMap::new();
        env.insert(SECRETS_DIR_ENV.to_string(), other.display().to_string());
        let i = ViewInputs { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: &tmp, env: &env, own_secrets: Some(&own) };
        let v = default_view_in(&i, &[secrets.clone()]).unwrap();
        assert!(covers(&v.ro, &own) && !covers(&v.rw, &own), "the session's own folder is read only: {:?}", v.ro);
        for (name, rules) in [("ro", &v.ro), ("rw", &v.rw)] {
            assert!(!covers(rules, &other), "{name}: the environment names another folder; it stays out: {rules:?}");
        }
        assert!(covers(&v.ro, &k2.join("hooks")), "the hooks are read and run: {:?}", v.ro);
        // No vault folder: nothing under the root opens, whatever the environment says.
        let i = ViewInputs { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: &tmp, env: &env, own_secrets: None };
        let v = default_view_in(&i, &[secrets.clone()]).unwrap();
        assert!(!covers(&v.ro, &other) && !covers(&v.rw, &other) && !covers(&v.ro, &own), "{:?}", v.ro);
        // A folder outside every root, named as the session's own: nothing.
        let i = ViewInputs { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: &tmp, env: &env, own_secrets: Some(&other) };
        let v = default_view_in(&i, &[own.clone()]).unwrap();
        assert!(!covers(&v.ro, &other) && !covers(&v.rw, &other), "{:?}", v.ro);
        // A link in the hooks folder: the folder stays out.
        std::os::unix::fs::symlink(home.join(".ssh"), k2.join("hooks/x")).unwrap();
        let v = default_view_in(&i, &[]).unwrap();
        assert!(!covers(&v.ro, &k2.join("hooks")), "{:?}", v.ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review V1: a link in a carved folder (as a fenced session can plant
    /// in /dev/shm) is never a rule: one to the owner's keys, one into
    /// another session's secrets folder; a real folder beside them is.
    #[test]
    fn a_link_planted_in_a_carved_folder_is_never_a_rule() {
        let root = canon(&std::env::temp_dir()).join(format!("k2-carvelink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let shm = root.join("shm");
        let secrets = shm.join("k2-1000");
        let other = secrets.join("other-2");
        for d in [&home.join(".ssh"), &other, &shm.join("plain")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(home.join(".ssh"), shm.join("x")).unwrap();
        std::os::unix::fs::symlink(&other, shm.join("y")).unwrap();
        let rules = carve_all(&[shm.display().to_string()], &[canon(&home), canon(&secrets)]);
        assert!(rules.iter().any(|r| Path::new(r) == shm.join("plain")), "{rules:?}");
        for link in ["x", "y"] {
            assert!(!rules.iter().any(|r| Path::new(r) == shm.join(link)), "{link} is a link, never a rule: {rules:?}");
        }
        assert!(!rules.iter().any(|r| canon(Path::new(r)).starts_with(canon(&home))), "{rules:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Re-review R1: a base rule that is a link is resolved before the carve;
    /// the carved children are its real folder's, listed as they are, and a
    /// child that is a link is left out (never resolved again).
    #[test]
    fn base_rules_resolve_before_the_carve() {
        let root = canon(&std::env::temp_dir()).join(format!("k2-r1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        let (keep, secret, elsewhere) = (real.join("keep"), real.join("k2-1000"), root.join("elsewhere"));
        for d in [&keep, &secret, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, real.join("swapped")).unwrap();
        let rules = resolve_then_carve(&[root.join("alias").display().to_string()], &[secret.clone()]);
        assert_eq!(rules, [keep.display().to_string()], "the real folder's own children, the link child out");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review V4: a view too large to hand over is refused, saying why.
    #[test]
    fn a_view_too_large_to_hand_over_is_refused() {
        let view = FenceView { rw: vec![], ro: (0..5000).map(|n| format!("/tmp/an-entry-with-a-fairly-long-name-{n:05}")).collect(), required: vec![] };
        let spec = ExecSpec { v: 1, cgroup: None, wait_cgroup: None, fence: Some(view), unset: vec![], set: BTreeMap::new(), sockblock: None };
        let e = spec_json(&spec).expect_err("refused");
        assert!(e.contains("5000 rules") && e.contains("doesn't start"), "{e}");
        let small = ExecSpec { v: 1, cgroup: None, wait_cgroup: None, fence: Some(FenceView::default()), unset: vec![], set: BTreeMap::new(), sockblock: None };
        assert!(spec_json(&small).is_ok());
    }

    /// One list of roots for the vault and the fence, the `/dev/shm` one last.
    #[test]
    fn the_secrets_roots_keep_dev_shm_last() {
        let c = secrets_root_candidates(None, Some(Path::new("/run/k2-daemon")), Some(Path::new("/run/user/1000")), 1000, false);
        assert_eq!(c, [PathBuf::from("/run/k2-daemon"), PathBuf::from("/run/user/1000/k2"), PathBuf::from("/run/k2"), PathBuf::from("/dev/shm/k2-1000")]);
        assert_eq!(secrets_root_candidates(Some(Path::new("/x")), None, None, 1, false), [PathBuf::from("/x")]);
        assert_eq!(shm_secrets_root(&c), Some(&PathBuf::from("/dev/shm/k2-1000")));
    }

    /// M2 review F3: the daemon reads a file for an agent from the handle it
    /// checked. A link out is refused by where it opens; a swap after the
    /// open changes nothing; only regular files.
    #[cfg(unix)]
    #[test]
    fn the_daemon_reads_from_the_handle_it_checked() {
        use std::io::Read;
        let root = std::env::temp_dir().join(format!("k2-handle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let k2 = home.join(".k2");
        let ws = home.join("DevProjects/A");
        let tmp = k2.join("agents/ws-a/tmp");
        for d in [&ws, &tmp, &home.join(".ssh")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(home.join(".ssh/id_ed25519"), "KEY").unwrap();
        std::fs::write(ws.join("x"), "PLAIN").unwrap();
        std::os::unix::fs::symlink(home.join(".ssh/id_ed25519"), ws.join("key")).unwrap();
        let r = AgentReach { home: &home, k2_home: &k2, workspace: &ws, tmp_dir: Some(&tmp), fenced: true };
        let e = open_for_agent(&r, &ws.join("key")).expect_err("a link out is checked where it opens");
        assert!(e.contains(".ssh"), "{e}");
        let (mut f, real) = open_for_agent(&r, &ws.join("x")).expect("its own file");
        assert_eq!(real, canon(&ws.join("x")), "the handle's own path");
        // The agent swaps the name for a link to the key after the check.
        std::fs::remove_file(ws.join("x")).unwrap();
        std::os::unix::fs::symlink(home.join(".ssh/id_ed25519"), ws.join("x")).unwrap();
        let mut got = String::new();
        f.read_to_string(&mut got).unwrap();
        assert_eq!(got, "PLAIN", "the read comes from the checked handle, never the name");
        // Not a regular file: refused, without blocking.
        let fifo = ws.join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(open_for_agent(&r, &fifo).unwrap_err().contains("regular file"));
        // A hard link made before the fence went on: refused for a fenced
        // agent, read for an unfenced one (its courtesy check is the path).
        std::fs::write(ws.join("one"), "ONE").unwrap();
        std::fs::hard_link(ws.join("one"), ws.join("two")).unwrap();
        assert!(open_for_agent(&r, &ws.join("two")).unwrap_err().contains("more than one name"));
        let open = AgentReach { fenced: false, ..r };
        assert!(open_for_agent(&open, &ws.join("two")).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn inputs<'a>(home: &'a Path, k2: &'a Path, ws: &'a Path, tmp: &'a Path, env: &'a BTreeMap<String, String>) -> ViewInputs<'a> {
        ViewInputs { home, k2_home: k2, workspace: ws, tmp_dir: tmp, env, own_secrets: None }
    }

    #[test]
    fn the_default_view_opens_the_workspace_and_never_k2_home() {
        let home = Path::new("/home/k2");
        let k2 = home.join(".k2");
        let ws = home.join("DevProjects/A");
        let tmp = k2.join("agents/ws-a/tmp");
        let env = BTreeMap::new();
        let v = default_view(&inputs(home, &k2, &ws, &tmp, &env)).unwrap();
        assert!(v.rw.contains(&"/home/k2/DevProjects/A".to_string()));
        assert_eq!(v.required, vec!["/home/k2/DevProjects/A".to_string()]);
        for p in v.rw.iter().chain(&v.ro) {
            assert_ne!(p, "/home/k2", "never the whole home");
            assert_ne!(p, "/home/k2/.k2", "never the whole ~/.k2");
            assert!(!p.ends_with("daemon.token") && !p.ends_with("heartbeat.token"), "{p}");
            assert!(!p.starts_with("/sys/fs/cgroup") || !v.rw.contains(p), "{p}");
            assert!(!p.contains("DevProjects/B"), "{p}");
            assert!(!p.ends_with(".ssh") && !p.ends_with(".aws") && !p.ends_with(".gnupg"), "{p}");
            assert!(!p.ends_with(".cargo/credentials.toml") && !p.ends_with(".config/gh"), "{p}");
            if p.starts_with("/home/k2/.k2") {
                assert!(
                    p == "/home/k2/.k2/bin" || p.ends_with(".port") || p.starts_with("/home/k2/.k2/agents/ws-a/tmp"),
                    "only the CLI, the port files and its own temp dir under ~/.k2: {p}"
                );
            }
        }
        assert!(!v.rw.iter().any(|p| p == "/sys" || p == "/sys/fs/cgroup"), "no write under /sys");
        // The pool login's own homes when no slot is pinned.
        assert!(v.rw.contains(&"/home/k2/.claude".to_string()) && v.rw.contains(&"/home/k2/.claude.json".to_string()));
    }

    #[test]
    fn a_pinned_login_opens_exactly_its_own_slot() {
        let home = Path::new("/home/k2");
        let k2 = home.join(".k2");
        let ws = home.join("w/A");
        let tmp = PathBuf::from("/tmp/x");
        let mut env = BTreeMap::new();
        env.insert("CLAUDE_CONFIG_DIR".to_string(), "/home/k2/.k2/llm-accounts/claude/acct-1".to_string());
        let v = default_view(&inputs(home, &k2, &ws, &tmp, &env)).unwrap();
        assert!(v.rw.contains(&"/home/k2/.k2/llm-accounts/claude/acct-1".to_string()));
        assert!(!v.rw.iter().any(|p| p == "/home/k2/.claude" || p == "/home/k2/.k2/llm-accounts" || p == "/home/k2/.k2/llm-accounts/claude"));
        // Not one slot: the wallet itself, or another part of ~/.k2.
        for bad in ["/home/k2/.k2/llm-accounts", "/home/k2/.k2/llm-accounts/claude", "/home/k2/.k2", "/home/k2/.k2/llm-accounts/claude/a/b"] {
            env.insert("CLAUDE_CONFIG_DIR".to_string(), bad.to_string());
            assert!(default_view(&inputs(home, &k2, &ws, &tmp, &env)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_workspace_holding_the_home_or_k2_home_is_refused() {
        let home = Path::new("/home/k2");
        let k2 = home.join(".k2");
        let tmp = PathBuf::from("/tmp/x");
        let env = BTreeMap::new();
        for ws in ["/home/k2", "/home", "/", "/home/k2/.k2", "/home/k2/.k2/x"] {
            assert!(default_view(&inputs(home, &k2, Path::new(ws), &tmp, &env)).is_err(), "{ws}");
        }
        assert!(default_view(&inputs(home, &k2, Path::new("relative/ws"), &tmp, &env)).is_err());
    }

    /// A home under a system rule (/tmp here; /var/lib/k2 on some boxes):
    /// the system rule is carved so it never covers the home or ~/.k2.
    #[test]
    fn a_rule_never_covers_the_home_or_k2_home() {
        let root = std::env::temp_dir().join(format!("k2-carve-{}", uuid::Uuid::new_v4().simple()));
        let home = root.join("homes/k2");
        let k2 = home.join(".k2");
        for d in [k2.clone(), home.join("ws/A"), root.join("other/x"), root.join("homes/someone")] {
            std::fs::create_dir_all(&d).unwrap();
        }
        let avoid = vec![canon(&home), canon(&k2)];
        let got: Vec<PathBuf> = carve(&canon(&root), &avoid);
        let r = canon(&root);
        assert_eq!(got, vec![r.join("homes/someone"), r.join("other")], "siblings stay, the home and the path down to it don't");
        let env = BTreeMap::new();
        let v = FenceView { ro: vec![s(&root)], ..FenceView::default() };
        let carved = carve_all(&v.ro, &avoid);
        assert!(!carved.iter().any(|p| Path::new(p) == r || canon(Path::new(p)) == canon(&home)), "{carved:?}");
        let tmp = root.join("tmp-a");
        let view = default_view(&inputs(&home, &k2, &home.join("ws/A"), &tmp, &env)).unwrap();
        for p in view.ro.iter().chain(&view.rw) {
            let c = canon(Path::new(p));
            assert!(!canon(&k2).starts_with(&c), "a rule covers ~/.k2: {p}");
            assert!(!canon(&home).starts_with(&c), "a rule covers the home: {p}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review F2: a link the agent plants in its own (read-write) slot, a
    /// harness home pointing anywhere else, or a workspace that is a link to
    /// the home never widens the fence.
    #[test]
    fn planted_links_and_odd_harness_homes_never_widen_the_fence() {
        let root = std::env::temp_dir().join(format!("k2-f2-{}", uuid::Uuid::new_v4().simple()));
        let home = root.join("home");
        let k2 = home.join(".k2");
        let slot = k2.join("llm-accounts/codex/acct-1");
        for d in [slot.clone(), home.join(".ssh"), home.join(".codex/sessions"), home.join("ws/A"), home.join(".claude/skills")] {
            std::fs::create_dir_all(&d).unwrap();
        }
        std::fs::write(home.join(".codex/config.toml"), "x").unwrap();
        // K2's own link (same name in the tool home) and two planted ones.
        std::os::unix::fs::symlink(home.join(".codex/sessions"), slot.join("sessions")).unwrap();
        std::os::unix::fs::symlink(home.join(".ssh"), slot.join("x")).unwrap();
        std::os::unix::fs::symlink(&home, slot.join("everything")).unwrap();
        std::os::unix::fs::symlink(home.join(".codex/config.toml"), slot.join("renamed.toml")).unwrap();
        // Review R2: same-named links to the default login and its config.
        std::fs::write(home.join(".codex/auth.json"), "{}").unwrap();
        std::os::unix::fs::symlink(home.join(".codex/auth.json"), slot.join("auth.json")).unwrap();
        std::os::unix::fs::symlink(home.join(".codex/config.toml"), slot.join("config.toml")).unwrap();
        std::fs::write(home.join(".codex/state_5.sqlite"), "").unwrap();
        std::os::unix::fs::symlink(home.join(".codex/state_5.sqlite"), slot.join("state_5.sqlite")).unwrap();
        assert_eq!(slot_links(&slot, &home.join(".codex"), "CODEX_HOME").len(), 2, "only sessions and the state db: {:?}", slot_links(&slot, &home.join(".codex"), "CODEX_HOME"));
        assert_eq!((codex_link_access("auth.json"), codex_link_access("config.toml"), codex_link_access("state_5.sqlite-wal")), (None, None, Some(true)));
        assert_eq!((codex_link_access("state_.sqlite"), codex_link_access("skills"), codex_link_access("anything")), (None, Some(false), None));
        let tmp = root.join("tmp");
        let mut env = BTreeMap::new();
        env.insert("CODEX_HOME".to_string(), s(&slot));
        let v = default_view(&inputs(&home, &k2, &home.join("ws/A"), &tmp, &env)).unwrap();
        let all: Vec<PathBuf> = v.rw.iter().chain(&v.ro).map(|p| canon(Path::new(p))).collect();
        assert!(all.contains(&canon(&home.join(".codex/sessions"))), "K2's own slot link is followed: {all:?}");
        for bad in [home.join(".ssh"), canon(&home), home.join(".codex/config.toml"), home.join(".codex/auth.json")] {
            assert!(!all.contains(&canon(&bad)), "a planted link widened the fence to {}: {all:?}", bad.display());
        }
        // A harness home that is neither a slot nor the tool's own home.
        env.insert("CODEX_HOME".to_string(), s(&home.join(".ssh")));
        assert!(default_view(&inputs(&home, &k2, &home.join("ws/A"), &tmp, &env)).is_err());
        env.insert("CODEX_HOME".to_string(), s(&home.join(".codex")));
        assert!(default_view(&inputs(&home, &k2, &home.join("ws/A"), &tmp, &env)).is_ok(), "the tool's own home");
        // A workspace that is a link to the home.
        std::os::unix::fs::symlink(&home, root.join("ws-link")).unwrap();
        let env = BTreeMap::new();
        assert!(default_view(&inputs(&home, &k2, &root.join("ws-link"), &tmp, &env)).is_err(), "a link to the home is the home");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rights_follow_the_abi_and_the_file_kind() {
        use access::*;
        assert_eq!((scoped_for_abi(4), scoped_for_abi(5)), (0, 0), "no scopes before ABI 6");
        assert_eq!(scoped_for_abi(6), SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL);
        assert_eq!(handled_for_abi(0), 0);
        assert_eq!(handled_for_abi(1), (1 << 13) - 1);
        assert_eq!(handled_for_abi(4), handled_for_abi(3), "ABI 4 adds network rights only");
        assert!(handled_for_abi(3) & TRUNCATE != 0 && handled_for_abi(2) & TRUNCATE == 0);
        assert!(handled_for_abi(5) & IOCTL_DEV != 0);
        let h = handled_for_abi(4);
        assert_eq!(rule_rights(false, true, h), RO);
        assert_eq!(rule_rights(false, false, h), EXECUTE | READ_FILE, "a file rule can't carry READ_DIR");
        assert_eq!(rule_rights(true, false, h), EXECUTE | READ_FILE | WRITE_FILE | TRUNCATE);
        assert_eq!(rule_rights(true, true, h), h);
        assert_eq!(rule_rights(false, true, h) & (WRITE_FILE | MAKE_REG | REMOVE_FILE), 0, "read-only means read-only");
    }

    #[test]
    fn the_fence_env_drops_the_owners_agents_and_sets_tmpdir() {
        let mut spec = ExecSpec { v: 1, cgroup: None, wait_cgroup: None, fence: None, unset: vec![], set: BTreeMap::new(), sockblock: None };
        fence_env(&mut spec, Path::new("/home/k2/.k2/agents/a/tmp"));
        assert!(spec.unset.iter().any(|k| k == "SSH_AUTH_SOCK"));
        assert_eq!(spec.set["TMPDIR"], "/home/k2/.k2/agents/a/tmp");
        assert!(!spec.unset.iter().any(|k| k == "PATH" || k == "K2_HOOK_TOKEN" || k == "K2_HOOK_SOCK"), "the passport and socket stay");
    }

    #[test]
    fn the_spec_round_trips() {
        let spec = ExecSpec { v: 1, cgroup: Some("/sys/fs/cgroup/x/agents/a-1".into()), wait_cgroup: None, fence: None, unset: vec![], set: BTreeMap::new(), sockblock: None };
        let j = serde_json::to_string(&spec).unwrap();
        assert_eq!(serde_json::from_str::<ExecSpec>(&j).unwrap(), spec);
        assert!(!j.contains("fence") && !j.contains("unset"), "no fence keys when off: {j}");
    }
}
