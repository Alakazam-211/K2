//! Packages on this node, per server (protocol 7,
//! prd-compute-self-serve-environments-v1 §4, §5.3, §6.2).
//!
//! - **Host facts** ([`Node::refresh_host`]): read with the tools probe.
//! - **The package view** ([`PkgCache`]): root's approved list and policy
//!   file, which of the approved and recently asked names are installed,
//!   and the index age. [`Node::pkg_status`] cuts one server's view for
//!   its offer.
//! - **Requests** ([`Ledger`]): any server with a session may ask for names
//!   (`pkg_request`); the node checks them, runs a dry run as the node user
//!   (no root), keeps them (at most 10 open per server, 24 h), and
//!   forwards them to its owner server (`pkg_request_fwd`). The owner
//!   decides: a deny comes back as `pkg_request_decide`, an approval as the
//!   signed admin command `packages.allow {packages, install, request}`, so
//!   root's guards run. The outcome goes to the asking server
//!   (`pkg_request_done`), with the owner server's label only, never its
//!   people. Kept in `<state>/pkg-requests.json` across restarts.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use k2_node_proto::frames::Frame;
use k2_node_proto::pkgpolicy::{self, codes, state, DrySummary, PkgRequest, PkgRequestDecide, PkgRequestDone, PkgRequestFwd, PkgStatus, PolicyFile};
use serde::{Deserialize, Serialize};

use super::{Node, Tenant};
use k2_node_proto::share::Slot;
use crate::pkgsys::{Backend, HostCtx};

/// Names a server asked for that the offer reports on, per server.
const ASKED_KEEP: usize = 64;

/// Root's files and what is installed (refreshed with the tools probe and
/// after an install).
#[derive(Debug, Clone, Default)]
pub struct PkgCache {
    pub manager: Option<String>,
    pub approved: Vec<String>,
    pub policy: PolicyFile,
    /// Canonical names known installed.
    pub installed: BTreeSet<String>,
    pub index_age_secs: Option<u64>,
    /// Recent `sys:` names per server slot.
    pub asked: BTreeMap<Slot, VecDeque<String>>,
    /// The last install found the database out of date (§10).
    pub index_stale: bool,
}

/// One open request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenReq {
    /// The node's id (`npr_…`).
    pub id: String,
    pub slot: Slot,
    /// The asking server's label (`own` for the owner).
    pub server: String,
    pub server_request: String,
    pub names: Vec<String>,
    #[serde(default)]
    pub natives: BTreeMap<String, Vec<String>>,
    pub job_id: Option<String>,
    pub why: Option<String>,
    pub asked_by: String,
    pub created_at: i64,
    pub expires_at: i64,
    #[serde(default)]
    pub dry: Option<DrySummary>,
    /// Forwarded to the owner server's live session.
    #[serde(default)]
    pub forwarded: bool,
    /// Decided: the outcome still to deliver to the asker.
    #[serde(default)]
    pub done: Option<PkgRequestDone>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub open: Vec<OpenReq>,
}

impl Ledger {
    fn path(node: &Node) -> PathBuf {
        node.layout().state().join("pkg-requests.json")
    }

    pub fn load(node: &Node) -> Ledger {
        std::fs::read_to_string(Self::path(node)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(&self, node: &Node) {
        if let Ok(b) = serde_json::to_vec(self) {
            if let Err(e) = crate::util::atomic_write(&Self::path(node), &b, 0o600) {
                crate::nlog!("package requests: {e}");
            }
        }
    }

    /// Open (undecided) requests from one server.
    pub fn open_for(&self, slot: Slot) -> usize {
        self.open.iter().filter(|r| r.slot == slot && r.done.is_none()).count()
    }
}

/// A server's label as the owner sees it: `own`, or its `servers.toml` label.
pub fn server_label(t: &Tenant) -> String {
    if t.is_owner() {
        return "own".into();
    }
    t.share().map(|s| s.entry.label).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("slot {}", t.slot))
}

/// A server's key fingerprint (empty without a pin).
pub fn server_fp(t: &Tenant) -> String {
    t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).unwrap_or_default()
}

impl Node {
    /// The package manager and host for package work: a test's override,
    /// else this machine's (Linux, with the root installer).
    pub fn pkg_backend(&self) -> Option<(HostCtx, Arc<dyn Backend>)> {
        if let (Some(h), Some(b)) = (&self.opts.pkg_host, &self.opts.pkg_backend) {
            return Some((h.clone(), b.clone()));
        }
        // Only the installed node (`/etc/k2-node`) speaks for the machine's
        // packages; a dev or test node never does.
        if self.layout().config != std::path::Path::new(crate::paths::DEFAULT_CONFIG_DIR) || !crate::packages::installer_present() {
            return None;
        }
        Some((crate::pkgsys::host_ctx()?, Arc::new(crate::pkgsys::system()?) as Arc<dyn Backend>))
    }

    /// Read the host facts (with the tools probe).
    pub fn refresh_host(&self) {
        let mac = self.tools.lock().unwrap().get("macos").cloned();
        let mut h = crate::sysinfo::host_facts(mac.as_deref());
        if let Some((hc, _)) = &self.pkg_backend() {
            h.pkg = Some(hc.mgr.name().to_string());
        }
        *self.host.lock().unwrap() = h;
    }

    /// Re-read root's files and what is installed (blocking calls).
    pub fn refresh_pkg_cache(&self) {
        let Some((h, b)) = self.pkg_backend() else {
            let mut c = self.pkg.lock().unwrap();
            let asked = std::mem::take(&mut c.asked);
            *c = PkgCache { asked, ..Default::default() };
            return;
        };
        let approved = crate::packages::read_approved(&self.pkg_paths().0).unwrap_or_default();
        let policy = crate::packages::read_policy(&self.pkg_paths().1).unwrap_or_else(|e| {
            crate::nlog!("package policy: {e}");
            PolicyFile::default()
        });
        let asked: Vec<String> = self.pkg.lock().unwrap().asked.values().flatten().cloned().collect();
        let mut names: Vec<String> = approved.iter().take(pkgpolicy::MAX_STATUS_NAMES).cloned().chain(asked).collect();
        names.sort();
        names.dedup();
        let installed: BTreeSet<String> = names.into_iter().filter(|n| crate::pkgsys::name_installed(b.as_ref(), &h, n)).collect();
        let mut c = self.pkg.lock().unwrap();
        c.manager = Some(h.mgr.name().to_string());
        c.approved = approved;
        c.policy = policy;
        c.installed = installed;
        c.index_age_secs = b.index_age_secs();
    }

    /// Root's approved list and policy file (a test node's own copies when
    /// its config dir isn't `/etc/k2-node`).
    pub fn pkg_paths(&self) -> (PathBuf, PathBuf) {
        let cfg = &self.layout().config;
        if cfg == std::path::Path::new("/etc/k2-node") {
            (PathBuf::from(crate::packages::APPROVED_FILE), PathBuf::from(crate::packages::POLICY_FILE))
        } else {
            (cfg.join("k2-node-packages.toml"), cfg.join("k2-node-package-policy.toml"))
        }
    }

    /// Remember the `sys:` names a server's job asked for (the offer says
    /// whether they're installed).
    pub fn note_pkg_asked(&self, slot: Slot, names: &[String]) {
        let mut c = self.pkg.lock().unwrap();
        let q = c.asked.entry(slot).or_default();
        for n in names {
            if !q.contains(n) {
                q.push_back(n.clone());
            }
        }
        while q.len() > ASKED_KEEP {
            q.pop_front();
        }
    }

    /// `t`'s view of the packages here (protocol 7, `pkg-status`).
    pub fn pkg_status(&self, t: &Tenant) -> Option<PkgStatus> {
        let c = self.pkg.lock().unwrap();
        let manager = c.manager.clone()?;
        let p = c.policy.for_server(t.is_owner(), &server_fp(t));
        let asked: BTreeSet<&String> = c.asked.get(&t.slot).map(|q| q.iter().collect()).unwrap_or_default();
        let installed: Vec<String> = c.installed.iter().filter(|n| c.approved.contains(n) || asked.contains(n)).cloned().collect();
        Some(PkgStatus {
            manager,
            level: p.level,
            profiles: p.profiles,
            max_install_mb: p.max_install_mb,
            approved: c.approved.iter().take(pkgpolicy::MAX_STATUS_NAMES).cloned().collect(),
            installed,
            index_age_secs: c.index_age_secs,
            open_requests: self.pkg_reqs.lock().unwrap().open_for(t.slot) as u32,
        })
    }

    // ── requests ─────────────────────────────────────────────────────

    /// `pkg_request` from `t`'s server.
    pub async fn on_pkg_request(self: &Arc<Self>, t: &Arc<Tenant>, r: PkgRequest) {
        let refuse = |code: &str, note: String| PkgRequestDone {
            server_request: asker_id(&r),
            state: state::FAILED.into(),
            note: Some(format!("{code}: {note}")),
            installed: vec![],
            owner: self.owner_label(),
        };
        if let Err(e) = pkgpolicy::check_request(&r.names, r.why.as_deref()) {
            t.hub.send(Frame::PkgRequestDone(refuse(codes::PACKAGE_NOT_APPROVED, e)));
            return;
        }
        if self.pkg_reqs.lock().unwrap().open_for(t.slot) >= pkgpolicy::MAX_OPEN_PER_SERVER {
            t.hub.send(Frame::PkgRequestDone(refuse(codes::TOO_MANY_PENDING, format!("{} requests from your server already wait on this node; wait for the owner's answer", pkgpolicy::MAX_OPEN_PER_SERVER))));
            return;
        }
        let Some((h, b)) = self.pkg_backend() else {
            let why = if cfg!(target_os = "macos") { "macOS nodes don't install system packages; the Mac's owner runs brew install at the machine" } else { "this node has no package installer (its owner re-runs sudo k2-node setup)" };
            t.hub.send(Frame::PkgRequestDone(refuse(codes::REQUESTS_UNSUPPORTED, why.into())));
            return;
        };
        let names = match pkgpolicy::expand_names(&r.names) {
            Ok(n) => n,
            Err(e) => {
                t.hub.send(Frame::PkgRequestDone(refuse(k2_node_proto::profiles::PROFILE_UNKNOWN, e)));
                return;
            }
        };
        let (b2, h2, n2) = (b.clone(), h.clone(), names.clone());
        let dry = tokio::task::spawn_blocking(move || crate::pkgsys::dry_summary(b2.as_ref(), &h2, &n2)).await.ok();
        let natives: BTreeMap<String, Vec<String>> = names.iter().filter_map(|n| h.natives(n).map(|v| (n.clone(), v))).collect();
        let now = self.now();
        let id = format!("npr_{}", k2_node_proto::crypto::random_id().unwrap_or_default().chars().take(16).collect::<String>());
        let req = OpenReq {
            id,
            slot: t.slot,
            server: server_label(t),
            server_request: asker_id(&r),
            names,
            natives,
            job_id: r.job_id.clone(),
            why: r.why.clone(),
            asked_by: r.asked_by.chars().filter(|c| !c.is_control()).take(80).collect(),
            created_at: now,
            expires_at: now + pkgpolicy::REQUEST_TTL_SECS,
            dry,
            forwarded: false,
            done: None,
        };
        crate::nlog!("package request {} from {}: {}", req.id, req.server, req.names.join(" "));
        {
            let mut l = self.pkg_reqs.lock().unwrap();
            l.open.push(req);
            l.save(self);
        }
        self.pkg_sweep();
    }

    fn owner_label(&self) -> String {
        self.owner().pin.lock().unwrap().as_ref().and_then(|p| p.routes.first().map(|u| crate::admin::servers::label_of(u))).unwrap_or_else(|| "the node's owner".into())
    }

    /// `pkg_request_decide` (a deny) from the owner's server.
    pub fn on_pkg_decide(&self, t: &Tenant, d: PkgRequestDecide) {
        if !t.is_owner() {
            crate::nlog!("package decide from a non-owner server ignored");
            return;
        }
        self.pkg_finish(&d.id, state::DENIED, d.note.map(|n| n.chars().filter(|c| !c.is_control()).take(400).collect()), vec![]);
    }

    /// Root's answer to `packages.allow {request}` (the executor's own
    /// result).
    pub fn on_pkg_admin_result(&self, r: &k2_node_proto::admin::AdminResult) {
        let Some(id) = r.result.get("pkg_request").and_then(|v| v.as_str()) else { return };
        let installed: Vec<String> = r.result.get("installed").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
        if r.state == "done" {
            self.pkg_finish(id, state::INSTALLED, None, installed);
        } else {
            let note = format!("{}: {}", r.code.clone().unwrap_or_default(), r.message.clone().unwrap_or_default());
            if r.code.as_deref() == Some(codes::INDEX_STALE) {
                self.pkg.lock().unwrap().index_stale = true;
            }
            self.pkg_finish(id, state::FAILED, Some(note), vec![]);
        }
        // What is installed changed.
        self.refresh_pkg_cache();
        self.notify_offers();
    }

    fn pkg_finish(&self, id: &str, st: &str, note: Option<String>, installed: Vec<String>) {
        let owner = self.owner_label();
        let mut l = self.pkg_reqs.lock().unwrap();
        let Some(r) = l.open.iter_mut().find(|r| r.id == id && r.done.is_none()) else { return };
        r.done = Some(PkgRequestDone { server_request: r.server_request.clone(), state: st.into(), note, installed, owner });
        crate::nlog!("package request {id}: {st}");
        l.save(self);
        drop(l);
        self.pkg_sweep();
    }

    /// Expire old requests, forward new ones to the owner, deliver
    /// outcomes (every 30 s and after each change).
    pub fn pkg_sweep(&self) {
        let now = self.now();
        let owner = self.owner();
        let owner_label = self.owner_label();
        let tenants: BTreeMap<Slot, Arc<Tenant>> = self.tenants().into_iter().map(|t| (t.slot, t)).collect();
        let mut l = self.pkg_reqs.lock().unwrap();
        let mut changed = false;
        for r in l.open.iter_mut() {
            if r.done.is_none() && now >= r.expires_at {
                r.done = Some(PkgRequestDone { server_request: r.server_request.clone(), state: state::EXPIRED.into(), note: Some("the node's owner didn't answer within 24 h".into()), installed: vec![], owner: owner_label.clone() });
                changed = true;
            }
            if r.done.is_none() && !r.forwarded && owner.speaks_host() {
                let host = self.host.lock().unwrap().line();
                let f = PkgRequestFwd {
                    id: r.id.clone(),
                    server: r.server.clone(),
                    server_request: r.server_request.clone(),
                    names: r.names.clone(),
                    natives: r.natives.clone(),
                    job_id: r.job_id.clone(),
                    why: r.why.clone(),
                    asked_by: r.asked_by.clone(),
                    host,
                    dry: r.dry.clone(),
                    created_at: r.created_at,
                    expires_at: r.expires_at,
                };
                if owner.hub.send(Frame::PkgRequestFwd(f)) {
                    r.forwarded = true;
                    changed = true;
                }
            }
        }
        // Deliver outcomes; a delivered one leaves the ledger. One the
        // asker never picks up goes after 7 days.
        let before = l.open.len();
        l.open.retain(|r| match &r.done {
            Some(d) => {
                let delivered = tenants.get(&r.slot).is_some_and(|t| t.speaks_host() && t.hub.send(Frame::PkgRequestDone(d.clone())));
                !delivered && now - r.created_at < 7 * 24 * 3600
            }
            None => true,
        });
        if changed || l.open.len() != before {
            l.save(self);
        }
    }
}

/// The asker's id, bounded to a safe charset.
fn asker_id(r: &PkgRequest) -> String {
    r.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(64).collect()
}
