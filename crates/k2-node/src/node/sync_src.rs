//! A job's code: the tenant's mirror → (fetch | the server's bundle) →
//! worktree → dirty patch, and the blob transfers over that tenant's
//! session.
//!
//! V49: a non-owner server's history comes over `https` from a public
//! address only ([`crate::sync::Git::fetch_public`]), and never when its
//! network is `no-lan` or `none`; otherwise its own bundle. Every git call
//! is hardened (`crate::sync::HARDEN`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_node_proto::crypto;
use k2_node_proto::frames::{Assign, Frame, Need, SrcRan, TransferKind};
use k2_node_proto::refusal;
use tokio::sync::{mpsc, watch};

use super::{kind_name, Node, StopReason, Tenant, TransferEvent, TransferKey};
use crate::journal::JournalWriter;
use crate::runner::JobDirs;
use crate::sync::{valid_sha, Git};

/// A server's refusal of a blob as the job log says it: `code: message`
/// (the message is the server's reason, e.g. the git error; one line,
/// capped).
fn failed_reason(code: &str, message: &str) -> String {
    let msg: String = message.chars().map(|c| if c.is_control() { ' ' } else { c }).take(600).collect();
    match msg.trim() {
        "" => code.to_string(),
        m => format!("{code}: {m}"),
    }
}

impl Node {
    /// Ask `t`'s server for a blob and write it to `dest`. Retries across
    /// reconnects; stops on the job's stop signal or the overall timeout.
    async fn fetch_blob(&self, t: &Tenant, need: Need, dest: &Path, stop: &mut watch::Receiver<Option<StopReason>>) -> Result<(), String> {
        let key: TransferKey = (need.job_id.clone(), need.generation, kind_name(need.kind));
        let deadline = tokio::time::Instant::now() + self.opts.transfer_timeout;
        let mut connected = t.hub.connected.subscribe();
        loop {
            // Wait for a session.
            while *connected.borrow() == 0 {
                tokio::select! {
                    r = connected.changed() => { if r.is_err() { return Err("node shutting down".into()); } }
                    _ = stop.changed() => return Err("stopped".into()),
                    _ = tokio::time::sleep_until(deadline) => return Err("transfer timed out".into()),
                }
            }
            let epoch = *connected.borrow();
            let (tx, mut rx) = mpsc::unbounded_channel();
            t.hub.transfers.lock().unwrap().insert(key.clone(), tx);
            let _ = std::fs::remove_file(dest);
            let mut file = std::fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
            if !t.hub.send(Frame::Need(need.clone())) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            let mut next = 0u32;
            let mut hash = crypto::Sha256Stream::default();
            let mut got = 0u64;
            let outcome: Option<Result<(), String>> = loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        None => break None,
                        Some(TransferEvent::Failed(f)) => break Some(Err(failed_reason(&f.code, &f.message))),
                        Some(TransferEvent::Chunk(t)) => {
                            if t.seq != next {
                                break Some(Err(format!("transfer chunk {} out of order (want {next})", t.seq)));
                            }
                            next += 1;
                            let data = match crypto::unb64(&t.data) { Ok(d) => d, Err(e) => break Some(Err(e)) };
                            if data.len() > k2_node_proto::frames::TRANSFER_CHUNK {
                                break Some(Err("transfer chunk too large".into()));
                            }
                            got += data.len() as u64;
                            if got > t.total_bytes {
                                break Some(Err("transfer longer than announced".into()));
                            }
                            hash.update(&data);
                            if let Err(e) = std::io::Write::write_all(&mut file, &data) { break Some(Err(format!("write blob: {e}"))); }
                            if t.last {
                                let sha = std::mem::take(&mut hash).finish_hex();
                                if got != t.total_bytes || !crypto::ct_eq(sha.as_bytes(), t.sha256.as_bytes()) {
                                    break Some(Err("transfer digest mismatch".into()));
                                }
                                break Some(Ok(()));
                            }
                        }
                    },
                    r = connected.changed() => {
                        if r.is_err() || *connected.borrow() != epoch { break None; }
                    }
                    _ = stop.changed() => break Some(Err("stopped".into())),
                    _ = tokio::time::sleep_until(deadline) => break Some(Err("transfer timed out".into())),
                }
            };
            t.hub.transfers.lock().unwrap().remove(&key);
            match outcome {
                Some(r) => return r,
                None => continue, // session changed: ask again
            }
        }
    }

    /// One repo into `dest`: the tenant's mirror for `project_key`, fetched
    /// from `remote_url` or the server's bundle of `commit`, then a
    /// worktree at `commit`. Returns its tree. `Err((code, detail))`.
    #[allow(clippy::too_many_arguments)]
    async fn sync_repo(
        &self,
        t: &Tenant,
        a: &Assign,
        git: &Git,
        project_key: &str,
        remote_url: Option<&str>,
        commit: &str,
        dest: &Path,
        root: &Path,
        journal: &mut JournalWriter,
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<String, (String, String)> {
        let fail = |c: &str, d: String| (c.to_string(), d);
        if !valid_sha(commit) {
            return Err(fail("sync_failed", format!("commit {commit:?} is not a full sha")));
        }
        if !k2_node_proto::frames::valid_project_key(project_key) {
            return Err(fail("sync_failed", "project key is not a plain name".into()));
        }
        let mirror = t.paths.mirror(project_key);
        let mut sys = |msg: String| {
            let _ = journal.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {msg}\n").as_bytes());
            t.ship.notify_one();
        };
        git.ensure_mirror(&mirror).await.map_err(|e| fail("sync_failed", e))?;
        if !git.has_commit(&mirror, commit).await {
            if let Some(url) = remote_url {
                if t.is_owner() {
                    match git.fetch_remote(&mirror, url).await {
                        Ok(()) => sys(format!("fetched history from {url}")),
                        Err(e) => sys(format!("couldn't fetch from the remote ({e}); asking the controller")),
                    }
                } else {
                    // V49: another server's remote is fetched only over
                    // https from a public address, and never when its
                    // network is no-lan or none (the fetch runs outside its
                    // job's wall). Otherwise its server sends a bundle.
                    match self.job_network(t) {
                        Ok(k2_node_proto::share::Network::On) => match git.fetch_public(&mirror, url).await {
                            Ok(at) => sys(format!("fetched history over https from {at}")),
                            Err(e) => sys(format!("not fetching this remote ({e}); asking the controller for a bundle")),
                        },
                        Ok(n) => sys(format!("this server's network is {}: bundles only; asking the controller", n.as_str())),
                        Err((code, e)) => sys(format!("{code}: {e}; asking the controller for a bundle")),
                    }
                }
            }
        }
        if !git.has_commit(&mirror, commit).await {
            let have = git.tips(&mirror).await.unwrap_or_default();
            let blob = root.join("bundle");
            let need = Need { job_id: a.fence.job_id.clone(), generation: a.fence.generation, kind: TransferKind::Bundle, want: commit.to_string(), have };
            self.fetch_blob(t, need, &blob, stop).await.map_err(|e| fail(if e == "stopped" { "stopped" } else { "sync_failed" }, e))?;
            git.fetch_bundle(&mirror, &blob).await.map_err(|e| fail("sync_failed", e))?;
            let _ = std::fs::remove_file(&blob);
            sys("applied the controller's bundle".into());
            if !git.has_commit(&mirror, commit).await {
                return Err(fail("sync_failed", format!("commit {commit} not found after the bundle")));
            }
        }
        let _ = std::fs::remove_dir_all(dest);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| fail("sync_failed", format!("create {}: {e}", parent.display())))?;
        }
        git.worktree_add(&mirror, dest, commit).await.map_err(|e| fail("sync_failed", e))?;
        git.tree_of(&mirror, commit).await.map_err(|e| fail("sync_failed", e))
    }

    /// Mirror → (fetch | bundle) → worktree → dirty, then (`--with`) each
    /// sibling repo beside it, laid out as in the workspace. Returns what
    /// ran and the main checkout (the job's start). `Err((code, detail))`.
    pub(super) async fn sync_src(
        &self,
        t: &Tenant,
        a: &Assign,
        dirs: &JobDirs,
        journal: &mut JournalWriter,
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<(SrcRan, PathBuf), (String, String)> {
        let src = a.plan.src.as_ref().ok_or(("sync_failed".to_string(), "no src".to_string()))?;
        let fail = |c: &str, d: String| (c.to_string(), d);
        // `--with` (protocol 6): `src/` is the workspace's layout, the main
        // checkout at `src/<src_path>`. Checked before anything is written.
        let main_dir = match a.plan.src_path.as_deref() {
            Some(sp) => {
                k2_node_proto::frames::check_layout(sp, &a.plan.with).map_err(|e| fail("sync_failed", e))?;
                let _ = std::fs::remove_dir_all(&dirs.src);
                std::fs::create_dir_all(&dirs.src).map_err(|e| fail("sync_failed", format!("create {}: {e}", dirs.src.display())))?;
                if sp.is_empty() { dirs.src.clone() } else { dirs.src.join(sp) }
            }
            None if !a.plan.with.is_empty() => return Err(fail("sync_failed", "--with repos without the main repo's folder".into())),
            None => dirs.src.clone(),
        };
        let git = Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home))
            .ok_or_else(|| fail(refusal::TOOL_MISSING, "git is not installed on the node".into()))?;
        // The main repo into place (a `--with` job's `src/` already holds
        // nothing but the layout's folders).
        if a.plan.src_path.as_deref() == Some("") {
            // The workspace's own repo is the layout root: git wants the
            // folder absent.
            let _ = std::fs::remove_dir_all(&dirs.src);
        }
        let tree = self.sync_repo(t, a, &git, &src.project_key, src.remote_url.as_deref(), &src.commit, &main_dir, &dirs.root, journal, stop).await?;
        let mut sys = |msg: String| {
            let _ = journal.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {msg}\n").as_bytes());
            t.ship.notify_one();
        };
        let mut dirty_sha = None;
        if let Some(d) = &src.dirty {
            let blob = dirs.root.join("dirty.tar");
            let need = Need { job_id: a.fence.job_id.clone(), generation: a.fence.generation, kind: TransferKind::Dirty, want: src.commit.clone(), have: vec![] };
            self.fetch_blob(t, need, &blob, stop).await.map_err(|e| fail(if e == "stopped" { "stopped" } else { "sync_failed" }, e))?;
            let got = crypto::sha256_hex(&std::fs::read(&blob).map_err(|e| fail("sync_failed", format!("read dirty blob: {e}")))?);
            if !crypto::ct_eq(got.as_bytes(), d.sha256.as_bytes()) {
                return Err(fail("sync_failed", "dirty blob doesn't match the plan's digest".into()));
            }
            git.apply_dirty(&main_dir, &blob, &dirs.dirty).await.map_err(|e| fail("sync_failed", e))?;
            sys("applied the dirty patch and untracked files".into());
            dirty_sha = Some(d.sha256.clone());
        }
        for w in &a.plan.with {
            let dest = dirs.src.join(&w.path);
            self.sync_repo(t, a, &git, &w.project_key, w.remote_url.as_deref(), &w.commit, &dest, &dirs.root, journal, stop).await?;
            let _ = journal.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] --with {} @ {}\n", w.path, &w.commit[..w.commit.len().min(8)]).as_bytes());
        }
        Ok((SrcRan { commit: src.commit.clone(), tree, dirty_sha256: dirty_sha, slot: None, warmth: None }, main_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::failed_reason;

    #[test]
    fn a_refused_blob_keeps_the_servers_reason() {
        assert_eq!(
            failed_reason("sync_failed", "name the commit: fatal: update_ref failed\nnonexistent object"),
            "sync_failed: name the commit: fatal: update_ref failed nonexistent object"
        );
        assert_eq!(failed_reason("relay_cap", "  "), "relay_cap");
        assert_eq!(failed_reason("sync_failed", &"x".repeat(5000)).len(), "sync_failed: ".len() + 600);
    }
}
