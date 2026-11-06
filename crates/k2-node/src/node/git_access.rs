//! Compute git access on the node (protocol 8, prd-compute-git-access-v1,
//! GH#78): short-lived, read-only git tokens for the repos a job's plan
//! names (`Plan::git_access`).
//!
//! - **Asking** ([`Node::pull_git_cred`]): `git_cred_need` over the job's
//!   server session with a fresh one-time key; the answer opens only with
//!   that key and this exact (job, attempt, repo, purpose, req).
//! - **Source sync** (`sync_src.rs`): the node's own fetch of a granted
//!   GitHub remote carries the token as an HTTP header scoped to that
//!   repo's URL, in the git child's environment only
//!   ([`crate::sync::Git::fetch_remote_auth`]).
//! - **Inside the job** ([`GitHelper`], Linux job wall only): a unix socket
//!   in the job's root that speaks git's own `credential-cache` protocol.
//!   The job's git is pointed at it through `GIT_CONFIG_*` env
//!   ([`helper_config`]), so a plain `git clone https://github.com/…`
//!   works with nothing of ours inside the job. Every connection's peer
//!   must be a process of this job's own unit ([`cgroup_in_unit`]); only
//!   `get` is answered, only for a repo the plan names. A token handed out
//!   is added to the job's log redactor at once.
//!
//! The token is never in the job's environment, its plan, a file the node
//! writes, or a cache key: that's why such a job keeps `--cache`, its warm
//! slot and the shared sccache (a `--secret` job doesn't, `run.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::frames::{Assign, Frame, GitCred, GitCredNeed, GitCredRefused};
use k2_node_proto::gitcred::{self, codes, Cred, NodeKey, Purpose};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::{Node, Tenant};
use crate::journal::LateRedactor;

/// The server's answer to one `git_cred_need`.
pub enum GitAnswer {
    Cred(GitCred),
    Refused(GitCredRefused),
}

/// The helper's socket, in the job's root.
pub const SOCKET_NAME: &str = ".k2-git.sock";
/// Longest socket path we bind (macOS `sun_path` is 104 bytes).
const MAX_SOCKET_PATH: usize = 100;
/// Most a helper request may be (git sends a few short lines).
const MAX_REQUEST: usize = 64 * 1024;
/// How long one helper connection may take, the server's answer included.
const CONN_LIMIT: Duration = Duration::from_secs(45);

/// The node's request counter (`GitCredNeed::req`).
static REQ: AtomicU64 = AtomicU64::new(1);

/// The command a person runs to grant `repo` (a key) to the job's workspace.
pub fn grant_hint(repo: &str) -> String {
    let short = gitcred::owner_repo(repo).map(|(o, r)| format!("{o}/{r}")).unwrap_or_else(|| repo.to_string());
    format!("k2 compute git-access grant --github {short}")
}

impl Node {
    /// Does `t`'s session get the `git-access` feature? Protocol 8, and for
    /// a server that isn't this node's owner, the node owner's
    /// `tenant_git_access` (default on).
    pub fn git_access_refusal(&self, t: &Tenant) -> Option<&'static str> {
        if t.protocol() < k2_node_proto::frames::PROTOCOL_GIT {
            return Some("this server is too old for git access");
        }
        if !t.is_owner() && !self.local.lock().unwrap().policy.tenant_git_access {
            return Some("this node's owner gives git tokens to no other server (tenant_git_access = false)");
        }
        None
    }

    /// One token for `repo` (a key in `a.plan.git_access`), from the job's
    /// server: `git_cred_need` with a fresh one-time key, then wait (≤ the
    /// secret wait, at most 30 s) for this request's answer. `Err((code,
    /// detail))` with a code from [`codes`].
    pub(super) async fn pull_git_cred(&self, t: &Tenant, a: &Assign, repo: &str, purpose: Purpose) -> Result<Cred, (String, String)> {
        let fail = |c: &str, d: String| (c.to_string(), d);
        if !a.plan.git_access.iter().any(|r| r == repo) {
            return Err(fail(codes::NOT_GRANTED, format!("{repo} isn't granted to this job (a person runs: {})", grant_hint(repo))));
        }
        // The node owner's policy, now (it may have changed since the
        // assign): a refusal there ends every ask from another server.
        if let Some(why) = self.git_access_refusal(t) {
            return Err(fail(codes::UNAVAILABLE, why.into()));
        }
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        let controller_fp = t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).ok_or_else(|| fail(codes::UNAVAILABLE, "no server pin".into()))?;
        let key = NodeKey::generate().map_err(|e| fail(codes::UNAVAILABLE, e))?;
        let req = REQ.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();
        t.hub.git.lock().unwrap().insert((job.clone(), gen, req), tx);
        let epoch = *t.hub.connected.borrow();
        let need = GitCredNeed {
            job_id: job.clone(),
            generation: gen,
            attempt: a.fence.attempt,
            plan_digest: a.fence.plan_digest.clone(),
            req,
            repo: repo.to_string(),
            purpose,
            epk: key.public_b64(),
        };
        let wait = self.opts.secret_wait.min(Duration::from_secs(gitcred::NEED_TIMEOUT_SECS));
        let answer = if epoch != 0 && t.hub.send(Frame::GitCredNeed(need)) {
            let mut connected = t.hub.connected.subscribe();
            let deadline = tokio::time::sleep(wait);
            tokio::pin!(deadline);
            tokio::pin!(rx);
            loop {
                tokio::select! {
                    r = &mut rx => break r.ok(),
                    _ = &mut deadline => break None,
                    r = connected.changed() => {
                        if r.is_err() || *connected.borrow() != epoch { break None; }
                    }
                }
            }
        } else {
            None
        };
        t.hub.git.lock().unwrap().remove(&(job.clone(), gen, req));
        let msg = match answer {
            None => return Err(fail(codes::UNAVAILABLE, format!("no answer from the server within {}s (or the link dropped)", wait.as_secs()))),
            Some(GitAnswer::Refused(r)) => {
                // Only codes K2 defines pass through.
                let code = codes::ALL.iter().find(|c| **c == r.code).copied().unwrap_or(codes::UNAVAILABLE);
                let detail: String = r.message.chars().filter(|c| !c.is_control()).take(400).collect();
                return Err(fail(code, format!("the server refused: {detail}")));
            }
            Some(GitAnswer::Cred(c)) => c,
        };
        let binding = gitcred::Binding {
            controller_fp,
            node_fp: self.key.fingerprint(),
            job_id: job,
            generation: gen,
            attempt: a.fence.attempt,
            plan_digest: a.fence.plan_digest.clone(),
            req,
            repo: repo.to_string(),
            purpose,
        };
        key.open(&msg.epk, &msg.ct, &binding).map_err(|e| fail(codes::STALE, e))
    }

    /// Start the job's credential helper (Linux job wall): its socket in
    /// `root`, the peer check `peer`, the late redactor `late`. Returns the
    /// helper (kept until the job ends) and the env entries to append.
    pub(super) fn start_git_helper(
        self: &Arc<Self>,
        t: &Arc<Tenant>,
        a: &Assign,
        root: &Path,
        peer: PeerCheck,
        late: LateRedactor,
    ) -> Result<(GitHelper, Vec<(String, String)>), String> {
        let sock = root.join(SOCKET_NAME);
        let shown = sock.display().to_string();
        if shown.len() > MAX_SOCKET_PATH || !shown.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
            return Err(format!("the job's folder path ({shown}) is too long or has characters git can't take in a helper path"));
        }
        let _ = std::fs::remove_file(&sock);
        let listener = tokio::net::UnixListener::bind(&sock).map_err(|e| format!("bind {shown}: {e}"))?;
        {
            use std::os::unix::fs::PermissionsExt;
            // The job's user (another uid) must connect; the peer check, not
            // the mode, decides who gets an answer.
            std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666)).map_err(|e| format!("chmod {shown}: {e}"))?;
        }
        let (notes_tx, notes_rx) = mpsc::unbounded_channel::<String>();
        let ctx = Arc::new(HelperCtx {
            node: self.clone(),
            t: t.clone(),
            a: a.clone(),
            peer,
            late,
            notes: notes_tx,
            cache: Mutex::new(HashMap::new()),
            said: Mutex::new(std::collections::HashSet::new()),
        });
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    // Out of file descriptors and the like: don't spin.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                let c = ctx.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(CONN_LIMIT, c.serve(stream)).await;
                });
            }
        });
        Ok((GitHelper { task, sock: sock.clone(), notes: Mutex::new(Some(notes_rx)) }, helper_config(&sock)))
    }
}

/// Who may ask through a job's helper: `Err(why)` for anyone else.
pub type PeerCheck = Arc<dyn Fn(&tokio::net::UnixStream) -> Result<(), String> + Send + Sync>;

/// A job's running helper. Dropping it stops it and removes the socket.
pub struct GitHelper {
    task: tokio::task::JoinHandle<()>,
    sock: PathBuf,
    /// The helper's `[k2-node]` lines, until the job's journal takes them.
    notes: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
}

impl GitHelper {
    /// Its lines from now on go into `journal` (and the job's server is
    /// told there is more log).
    pub fn attach_journal(&self, journal: Arc<Mutex<crate::journal::JournalWriter>>, t: Arc<Tenant>) {
        let Some(mut rx) = self.notes.lock().unwrap().take() else { return };
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let _ = journal.lock().unwrap().append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {line}\n").as_bytes());
                t.ship.notify_one();
            }
        });
    }
}

impl Drop for GitHelper {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.sock);
    }
}

struct HelperCtx {
    node: Arc<Node>,
    t: Arc<Tenant>,
    a: Assign,
    peer: PeerCheck,
    late: LateRedactor,
    notes: mpsc::UnboundedSender<String>,
    /// repo → (token, keep until): in memory, this job only.
    cache: Mutex<HashMap<String, (Cred, i64)>>,
    /// Lines already said once (refusals).
    said: Mutex<std::collections::HashSet<String>>,
}

impl HelperCtx {
    fn note_once(&self, key: &str, line: String) {
        if self.said.lock().unwrap().insert(key.to_string()) {
            let _ = self.notes.send(line);
        }
    }

    async fn serve(&self, mut stream: tokio::net::UnixStream) {
        if let Err(why) = (self.peer)(&stream) {
            self.note_once(&format!("peer:{why}"), format!("git credentials: refused a caller from outside this job ({why})"));
            return;
        }
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > MAX_REQUEST {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        let Some(req) = parse_cache_request(&buf) else { return };
        let answer = self.answer(&req).await;
        if let Some(text) = answer {
            let _ = stream.write_all(text.as_bytes()).await;
            let mut text = text.into_bytes();
            k2_node_proto::secret::wipe(&mut text);
        }
        let _ = stream.shutdown().await;
    }

    /// The text for one request, or `None` (git then has no credential).
    async fn answer(&self, req: &CacheRequest) -> Option<String> {
        // `store` and `erase` are ignored: git never hands a token back to
        // be kept. Anything not https://github.com isn't ours.
        if req.action != "get" || req.protocol.as_deref() != Some("https") || !req.host.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(gitcred::GITHUB_HOST)) {
            return None;
        }
        let Some(repo) = req.path.as_deref().and_then(|p| gitcred::repo_key(gitcred::GITHUB_HOST, p)) else {
            self.note_once("nopath", "git credentials: git asked for github.com without a repo path (credential.useHttpPath was turned off?)".into());
            return None;
        };
        if !self.a.plan.git_access.contains(&repo) {
            let granted: Vec<String> = self.a.plan.git_access.iter().filter_map(|r| gitcred::owner_repo(r).map(|(o, n)| format!("{o}/{n}"))).collect();
            self.note_once(
                &format!("ng:{repo}"),
                format!(
                    "git credentials: {repo} isn't granted to this workspace on this node (granted: {}); a person runs: {}",
                    if granted.is_empty() { "none".to_string() } else { granted.join(", ") },
                    grant_hint(&repo)
                ),
            );
            return None;
        }
        let now = self.node.now();
        if let Some((c, until)) = self.cache.lock().unwrap().get(&repo) {
            if now < *until {
                return Some(cache_answer(c));
            }
        }
        match self.node.pull_git_cred(&self.t, &self.a, &repo, Purpose::Job).await {
            Ok(c) => {
                {
                    let mut l = self.late.lock().unwrap_or_else(|p| p.into_inner());
                    let base = l.clone().unwrap_or_else(|| Arc::new(k2_redact::Redactor::empty()));
                    *l = Some(Arc::new(base.with(&repo, c.password.as_bytes())));
                }
                let until = keep_until(now, c.expires_at);
                let _ = self.notes.send(format!("git credentials: a read-only token for {repo} (from the server; expires at {} UTC)", hhmm(c.expires_at)));
                let text = cache_answer(&c);
                self.cache.lock().unwrap().insert(repo, (c, until));
                Some(text)
            }
            Err((code, detail)) => {
                let _ = self.notes.send(format!("git credentials: {repo}: {code}: {detail}"));
                None
            }
        }
    }
}

/// How long the node keeps a token for a job: at most
/// [`gitcred::NODE_KEEP_SECS`], never within [`gitcred::EXPIRY_MARGIN_SECS`]
/// of its expiry.
pub fn keep_until(now: i64, expires_at: i64) -> i64 {
    (now + gitcred::NODE_KEEP_SECS).min(expires_at - gitcred::EXPIRY_MARGIN_SECS)
}

fn hhmm(unix: i64) -> String {
    let s = unix.rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// One `git credential-cache` request: `action=…`, `timeout=…`, then the
/// credential's lines (`protocol`, `host`, `path`, …; unknown keys
/// ignored, as git does).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CacheRequest {
    pub action: String,
    pub protocol: Option<String>,
    pub host: Option<String>,
    pub path: Option<String>,
}

/// Parse what `git credential-cache` writes to the daemon socket.
pub fn parse_cache_request(buf: &[u8]) -> Option<CacheRequest> {
    let text = std::str::from_utf8(buf).ok()?;
    let mut r = CacheRequest::default();
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        let (k, v) = line.split_once('=')?;
        match k {
            "action" => r.action = v.to_string(),
            "protocol" => r.protocol = Some(v.to_string()),
            "host" => r.host = Some(v.to_string()),
            "path" => r.path = Some(v.to_string()),
            _ => {}
        }
    }
    (!r.action.is_empty()).then_some(r)
}

/// The daemon's answer to `get`.
pub fn cache_answer(c: &Cred) -> String {
    format!("username={}\npassword={}\npassword_expiry_utc={}\n", c.username, c.password, c.expires_at)
}

/// The job's git config (appended through `GIT_CONFIG_*`): only this
/// helper for github.com (an empty value drops any other), the repo path
/// sent, ssh URLs to GitHub read as https.
pub fn helper_config(sock: &Path) -> Vec<(String, String)> {
    let h = "credential.https://github.com.helper".to_string();
    vec![
        (h.clone(), String::new()),
        (h, format!("cache --socket {}", sock.display())),
        ("credential.https://github.com.useHttpPath".into(), "true".into()),
        ("url.https://github.com/.insteadOf".into(), "git@github.com:".into()),
        ("url.https://github.com/.insteadOf".into(), "ssh://git@github.com/".into()),
    ]
}

/// Is a `/proc/<pid>/cgroup` (its text) inside systemd unit `unit`? Every
/// unified (`0::`) or `name=systemd` line must end in the unit, and there
/// must be one. The job's units have `ProtectControlGroups=yes` and no
/// delegation, so a job can't move a process out of (or into) its unit.
pub fn cgroup_in_unit(cgroup: &str, unit: &str) -> bool {
    let mut seen = false;
    for line in cgroup.lines() {
        let mut it = line.splitn(3, ':');
        let (Some(id), Some(ctl), Some(path)) = (it.next(), it.next(), it.next()) else { continue };
        if !(id == "0" && ctl.is_empty()) && ctl != "name=systemd" {
            continue;
        }
        if path.rsplit('/').next() != Some(unit) {
            return false;
        }
        seen = true;
    }
    seen
}

/// The Linux peer check: the connecting process (`SO_PEERCRED`) must be
/// in `unit`.
#[cfg(target_os = "linux")]
pub fn unit_peer_check(unit: String) -> PeerCheck {
    Arc::new(move |s: &tokio::net::UnixStream| {
        let cred = s.peer_cred().map_err(|e| format!("peer credentials: {e}"))?;
        let pid = cred.pid().ok_or("no peer pid")?;
        let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).map_err(|e| format!("read the peer's cgroup: {e}"))?;
        if cgroup_in_unit(&cg, &unit) {
            Ok(())
        } else {
            Err(format!("pid {pid} isn't in {unit}"))
        }
    })
}

/// A `--dev` node's peer check (no wall there: the job runs as the node's
/// own user, so nothing narrower can be told apart): the same uid.
pub fn same_uid_peer_check() -> PeerCheck {
    Arc::new(|s: &tokio::net::UnixStream| {
        let cred = s.peer_cred().map_err(|e| format!("peer credentials: {e}"))?;
        // SAFETY: getuid never fails.
        let me = unsafe { libc::getuid() };
        if cred.uid() == me {
            Ok(())
        } else {
            Err(format!("uid {} isn't this node's", cred.uid()))
        }
    })
}

impl Node {
    /// Git access inside a job (protocol 8): the helper for `a` and the
    /// git config to append to its env, or why the job gets none. Linux
    /// job wall: peers checked by unit. A `--dev` test node (no wall, never
    /// installed): peers checked by uid. Anything else: none (the job runs
    /// as the node's own user there and could reach what the node can).
    pub(super) fn git_helper_for(self: &Arc<Self>, t: &Arc<Tenant>, a: &Assign, root: &Path, unit_mode: bool, late: LateRedactor) -> Result<Vec<(String, String)>, String> {
        let peer: PeerCheck = if unit_mode {
            #[cfg(target_os = "linux")]
            {
                let unit = crate::admin::jobs::unit_name(&t.job_key(&a.fence.job_id, a.fence.generation)).ok_or("the job's unit name isn't valid")?;
                unit_peer_check(unit)
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Err("git credentials inside a job need a Linux node with the job wall".into());
            }
        } else if super::dev_test_exemption(self.opts.dev, &self.layout().config) {
            same_uid_peer_check()
        } else {
            return Err("git credentials inside a job need a Linux node with the job wall (sudo k2-node setup)".into());
        };
        let (helper, cfg) = self.start_git_helper(t, a, root, peer, late)?;
        self.git_helpers.lock().unwrap().insert((t.slot, a.fence.job_id.clone(), a.fence.generation), helper);
        Ok(cfg)
    }

    /// The job's journal is shared now: the helper's lines go into it.
    pub(super) fn git_helper_attach(&self, t: &Arc<Tenant>, job: &str, gen: u32, journal: &Arc<Mutex<crate::journal::JournalWriter>>) {
        if let Some(h) = self.git_helpers.lock().unwrap().get(&(t.slot, job.to_string(), gen)) {
            h.attach_journal(journal.clone(), t.clone());
        }
    }

    /// The attempt ended: its helper stops and its socket goes.
    pub(super) fn git_helper_drop(&self, t: &Tenant, job: &str, gen: u32) {
        let h = self.git_helpers.lock().unwrap().remove(&(t.slot, job.to_string(), gen));
        drop(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_requests_parse_like_git_writes_them() {
        let r = parse_cache_request(b"action=get\ntimeout=900\nprotocol=https\nhost=github.com\npath=acme/app.git\ncapability[]=authtype\n").unwrap();
        assert_eq!(r, CacheRequest { action: "get".into(), protocol: Some("https".into()), host: Some("github.com".into()), path: Some("acme/app.git".into()) });
        assert_eq!(parse_cache_request(b"action=store\ntimeout=900\nprotocol=https\nhost=github.com\npassword=x\n").unwrap().action, "store");
        assert_eq!(parse_cache_request(b""), None);
        assert_eq!(parse_cache_request(b"garbage"), None);
        assert_eq!(parse_cache_request(&[0xff, 0xfe]), None);
    }

    #[test]
    fn the_answer_and_the_config_are_what_git_reads() {
        let c = Cred { username: gitcred::USERNAME.into(), password: "ghs_k2testANSWER".into(), expires_at: 1_900_000_000 };
        assert_eq!(cache_answer(&c), "username=x-access-token\npassword=ghs_k2testANSWER\npassword_expiry_utc=1900000000\n");
        let cfg = helper_config(Path::new("/var/lib/k2-node/jobs/j-g1/.k2-git.sock"));
        assert_eq!(cfg[0], ("credential.https://github.com.helper".to_string(), String::new()), "other helpers dropped first");
        assert_eq!(cfg[1].1, "cache --socket /var/lib/k2-node/jobs/j-g1/.k2-git.sock");
        assert!(cfg.iter().any(|(k, v)| k == "credential.https://github.com.useHttpPath" && v == "true"));
        assert_eq!(grant_hint("github.com/acme/app"), "k2 compute git-access grant --github acme/app");
    }

    #[test]
    fn the_cgroup_check_takes_only_the_jobs_own_unit() {
        let unit = "k2job-abc-g1.service";
        assert!(cgroup_in_unit("0::/k2jobs.slice/k2job-abc-g1.service\n", unit));
        assert!(cgroup_in_unit("12:pids:/x\n1:name=systemd:/system.slice/k2job-abc-g1.service\n0::/system.slice/k2job-abc-g1.service\n", unit));
        assert!(!cgroup_in_unit("0::/k2jobs.slice/k2job-abc-g2.service\n", unit), "another attempt's unit");
        assert!(!cgroup_in_unit("0::/k2jobs.slice/k2job-abc-g1.service/inner\n", unit), "a sub-cgroup");
        assert!(!cgroup_in_unit("0::/user.slice/user-1000.slice/session-3.scope\n", unit), "a login shell");
        assert!(!cgroup_in_unit("1:name=systemd:/system.slice/k2job-abc-g1.service\n0::/user.slice/x.scope\n", unit), "one line off is enough to refuse");
        assert!(!cgroup_in_unit("", unit), "nothing to check");
        assert!(!cgroup_in_unit("12:pids:/system.slice/k2job-abc-g1.service\n", unit), "a v1 controller line alone proves nothing");
    }

    #[test]
    fn a_token_is_kept_at_most_ten_minutes_and_never_near_its_expiry() {
        assert_eq!(keep_until(1000, 1000 + 3600), 1000 + 600);
        assert_eq!(keep_until(1000, 1000 + 400), 1000 + 100);
        assert!(keep_until(1000, 1000 + 200) < 1000, "a token that expires soon is never reused");
        assert_eq!(hhmm(3600 * 13 + 60 * 7), "13:07");
    }
}
