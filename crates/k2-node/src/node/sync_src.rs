//! A job's code: the tenant's mirror → (fetch | the server's bundle) →
//! worktree → dirty patch, and the blob transfers over that tenant's
//! session.
//!
//! V49: a non-owner server's history comes over `https` from a public
//! address only ([`crate::sync::Git::fetch_public`]), and never when its
//! network is `no-lan` or `none`; otherwise its own bundle. Every git call
//! is hardened (`crate::sync::HARDEN`).

use std::path::Path;
use std::time::Duration;

use k2_node_proto::crypto;
use k2_node_proto::frames::{Assign, Frame, Need, SrcRan, TransferKind};
use k2_node_proto::refusal;
use tokio::sync::{mpsc, watch};

use super::{kind_name, Node, StopReason, Tenant, TransferEvent, TransferKey};
use crate::journal::JournalWriter;
use crate::runner::JobDirs;
use crate::sync::{valid_sha, Git};

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
                        Some(TransferEvent::Failed(f)) => break Some(Err(f.code)),
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

    /// Mirror → (fetch | bundle) → worktree → dirty. `Err((code, detail))`.
    pub(super) async fn sync_src(
        &self,
        t: &Tenant,
        a: &Assign,
        dirs: &JobDirs,
        journal: &mut JournalWriter,
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<SrcRan, (String, String)> {
        let src = a.plan.src.as_ref().ok_or(("sync_failed".to_string(), "no src".to_string()))?;
        let fail = |c: &str, d: String| (c.to_string(), d);
        if !valid_sha(&src.commit) {
            return Err(fail("sync_failed", format!("commit {:?} is not a full sha", src.commit)));
        }
        if src.project_key.is_empty() || !src.project_key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(fail("sync_failed", "project key is not a plain name".into()));
        }
        let git = Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home))
            .ok_or_else(|| fail(refusal::TOOL_MISSING, "git is not installed on the node".into()))?;
        let mirror = t.paths.mirror(&src.project_key);
        let mut sys = |msg: String| {
            let _ = journal.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {msg}\n").as_bytes());
            t.ship.notify_one();
        };
        git.ensure_mirror(&mirror).await.map_err(|e| fail("sync_failed", e))?;
        if !git.has_commit(&mirror, &src.commit).await {
            if let Some(url) = &src.remote_url {
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
        if !git.has_commit(&mirror, &src.commit).await {
            let have = git.tips(&mirror).await.unwrap_or_default();
            let blob = dirs.root.join("bundle");
            let need = Need { job_id: a.fence.job_id.clone(), generation: a.fence.generation, kind: TransferKind::Bundle, want: src.commit.clone(), have };
            self.fetch_blob(t, need, &blob, stop).await.map_err(|e| fail(if e == "stopped" { "stopped" } else { "sync_failed" }, e))?;
            git.fetch_bundle(&mirror, &blob).await.map_err(|e| fail("sync_failed", e))?;
            let _ = std::fs::remove_file(&blob);
            sys("applied the controller's bundle".into());
            if !git.has_commit(&mirror, &src.commit).await {
                return Err(fail("sync_failed", format!("commit {} not found after the bundle", src.commit)));
            }
        }
        let _ = std::fs::remove_dir_all(&dirs.src);
        git.worktree_add(&mirror, &dirs.src, &src.commit).await.map_err(|e| fail("sync_failed", e))?;
        let tree = git.tree_of(&mirror, &src.commit).await.map_err(|e| fail("sync_failed", e))?;
        let mut dirty_sha = None;
        if let Some(d) = &src.dirty {
            let blob = dirs.root.join("dirty.tar");
            let need = Need { job_id: a.fence.job_id.clone(), generation: a.fence.generation, kind: TransferKind::Dirty, want: src.commit.clone(), have: vec![] };
            self.fetch_blob(t, need, &blob, stop).await.map_err(|e| fail(if e == "stopped" { "stopped" } else { "sync_failed" }, e))?;
            let got = crypto::sha256_hex(&std::fs::read(&blob).map_err(|e| fail("sync_failed", format!("read dirty blob: {e}")))?);
            if !crypto::ct_eq(got.as_bytes(), d.sha256.as_bytes()) {
                return Err(fail("sync_failed", "dirty blob doesn't match the plan's digest".into()));
            }
            git.apply_dirty(&dirs.src, &blob, &dirs.dirty).await.map_err(|e| fail("sync_failed", e))?;
            sys("applied the dirty patch and untracked files".into());
            dirty_sha = Some(d.sha256.clone());
        }
        Ok(SrcRan { commit: src.commit.clone(), tree, dirty_sha256: dirty_sha, slot: None, warmth: None })
    }
}
