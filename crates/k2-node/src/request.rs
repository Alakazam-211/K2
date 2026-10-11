//! Enroll requests from the machine's own K2 ("Make this computer a
//! compute node", compute polish v1).
//!
//! Root installs the node once, unenrolled. After that the machine's
//! owner (a `k2nodectl` member: their K2 daemon, or `k2-node request`)
//! writes `enroll-request.toml` into the config dir, which the node user
//! can't write. The running node picks it up within seconds, enrolls as
//! itself (its key never leaves its home), and reports the result in
//! `run/enroll-result.json` (0644) plus its status (the SAS to confirm on
//! the controller). Each request id is tried once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::enroll::Credential;
use crate::node::Node;
use crate::paths::Layout;

pub const REQUEST_FILE: &str = "enroll-request.toml";
pub const RESULT_FILE: &str = "enroll-result.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub controller: String,
    #[serde(default)]
    pub enroll: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    pub name: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub requested_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub ok: bool,
    pub message: String,
    pub sas: Option<String>,
    pub name: Option<String>,
    pub at: i64,
}

pub fn request_path(layout: &Layout) -> PathBuf {
    layout.config.join(REQUEST_FILE)
}

pub fn result_path(layout: &Layout) -> PathBuf {
    layout.run().join(RESULT_FILE)
}

fn valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Request {
    /// Shape checks before anything dials out.
    pub fn credential(&self) -> Result<Credential, String> {
        if !valid_id(&self.id) {
            return Err("request id must be 8–64 of A-Z a-z 0-9 - _".into());
        }
        if !crate::enroll::valid_name(&self.name) {
            return Err(format!("node name {:?}: use 1-32 of a-z, 0-9 and -", self.name));
        }
        if !(self.controller.starts_with("https://") || self.controller.starts_with("http://")) {
            return Err("controller must be an http(s) URL".into());
        }
        for k in self.labels.keys() {
            if matches!(k.as_str(), "os" | "arch" | "name" | "vm") {
                return Err(format!("label {k} is set by the node itself"));
            }
        }
        match (&self.enroll, &self.ticket) {
            (Some(e), None) => k2_node_proto::pairing::EnrollString::parse(e)
                .map(Credential::Code)
                .ok_or_else(|| "enroll must look like ABCDE-FGHJK.0123456789abcdef".to_string()),
            (None, Some(t)) => k2_node_proto::ticket::Ticket::decode(t).map(Credential::Ticket),
            _ => Err("pass exactly one of enroll (a code) or ticket (one-click)".into()),
        }
    }
}

/// The request waiting in the config dir, unless it was already tried.
pub fn pending(layout: &Layout) -> Option<Result<Request, (String, String)>> {
    let text = std::fs::read_to_string(request_path(layout)).ok()?;
    let parsed: Result<Request, String> = toml::from_str(&text).map_err(|e| format!("{REQUEST_FILE}: {e}"));
    let last = read_result(&result_path(layout));
    match parsed {
        Ok(r) => {
            if last.as_ref().is_some_and(|o| o.id == r.id) {
                return None;
            }
            Some(Ok(r))
        }
        Err(e) => {
            // A broken file is reported once per content.
            let id = format!("bad-{}", &k2_node_proto::crypto::sha256_hex(text.as_bytes())[..16]);
            if last.as_ref().is_some_and(|o| o.id == id) {
                return None;
            }
            Some(Err((id, e)))
        }
    }
}

pub fn read_result(p: &Path) -> Option<Outcome> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn write_result(layout: &Layout, o: &Outcome) {
    match serde_json::to_vec_pretty(o) {
        Ok(b) => {
            if let Err(e) = crate::util::atomic_write(&result_path(layout), &b, 0o644) {
                crate::nlog!("enroll result: {e}");
            }
        }
        Err(e) => crate::nlog!("enroll result: {e}"),
    }
}

/// Try the waiting request once. True when the node enrolled now.
pub async fn process(node: &Arc<Node>) -> bool {
    let layout = node.layout().clone();
    let Some(req) = pending(&layout) else { return false };
    let now = crate::util::now();
    let req = match req {
        Ok(r) => r,
        Err((id, e)) => {
            write_result(&layout, &Outcome { id, ok: false, message: e.clone(), at: now, ..Default::default() });
            node.set_status_state("unenrolled", None, Some(e));
            return false;
        }
    };
    let fail = |msg: String| {
        crate::nlog!("enroll request {}: {msg}", req.id);
        write_result(&layout, &Outcome { id: req.id.clone(), ok: false, message: msg.clone(), name: Some(req.name.clone()), at: now, ..Default::default() });
        msg
    };
    if let Some(p) = node.pin.lock().unwrap().clone() {
        if !p.revoked {
            let m = fail(format!(
                "already enrolled as {} with {}; remove it on the controller first",
                p.name,
                p.controller_label()
            ));
            node.set_status_state(&node.status.lock().unwrap().state.clone(), None, Some(m));
            return false;
        }
    }
    let cred = match req.credential() {
        Ok(c) => c,
        Err(e) => {
            let m = fail(e);
            node.set_status_state("unenrolled", None, Some(m));
            return false;
        }
    };
    crate::nlog!("enroll request {} from {}: enrolling as {} with {}", req.id, req.requested_by.clone().unwrap_or_default(), req.name, req.controller);
    let r = async {
        let mut ws = crate::session::connect(&req.controller, crate::session::ENROLL_PATH).await?;
        crate::enroll::enroll_on(&mut ws, &node.key, &req.controller, &cred, &req.name, &req.labels, now).await
    }
    .await;
    match r {
        Ok(done) => {
            if let Err(e) = crate::identity::write_pin(&layout.pin(), &done.pin) {
                let m = fail(e);
                node.set_status_state("unenrolled", None, Some(m));
                return false;
            }
            node.reload_pin();
            write_result(
                &layout,
                &Outcome {
                    id: req.id.clone(),
                    ok: true,
                    message: format!("enrolled as {} (pending): confirm code {} on the controller", done.pin.name, done.sas),
                    sas: Some(done.sas.clone()),
                    name: Some(done.pin.name.clone()),
                    at: now,
                },
            );
            node.set_status_state("pending", None, None);
            true
        }
        Err(e) => {
            let m = fail(e);
            node.set_status_state("unenrolled", None, Some(m));
            false
        }
    }
}

/// `k2-node request …`: write the request file (the owner, as a
/// `k2nodectl` member or root). The daemon writes the same file itself.
pub fn write_request(layout: &Layout, r: &Request) -> Result<(), String> {
    r.credential()?;
    let body = toml::to_string(r).map_err(|e| format!("encode request: {e}"))?;
    crate::util::atomic_write(&request_path(layout), body.as_bytes(), 0o660).map_err(|e| {
        format!("{e}\n{} is the machine owner's: run this as root (sudo) or as a member of the k2nodectl group", layout.config.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> Request {
        Request {
            id: "req-0001-abcd".into(),
            controller: "https://rosson.k2.dev".into(),
            enroll: Some("ABCDE-FGHJK.0123456789abcdef".into()),
            ticket: None,
            name: "mini-1".into(),
            labels: BTreeMap::new(),
            requested_by: Some("rosson".into()),
            requested_at: Some(1),
        }
    }

    #[test]
    fn requests_are_checked_before_dialling() {
        assert!(matches!(req().credential().unwrap(), Credential::Code(_)));
        let mut both = req();
        both.ticket = Some("x".into());
        assert!(both.credential().is_err());
        let mut none = req();
        none.enroll = None;
        assert!(none.credential().is_err());
        let mut bad_name = req();
        bad_name.name = "Mini 1".into();
        assert!(bad_name.credential().is_err());
        let mut os_label = req();
        os_label.labels.insert("os".into(), "linux".into());
        assert!(os_label.credential().is_err());
        let mut short_id = req();
        short_id.id = "x".into();
        assert!(short_id.credential().is_err());
    }

    #[test]
    fn a_request_is_tried_once_per_id() {
        let d = crate::util::temp_dir("req");
        let layout = Layout::new(d.join("home"), d.join("etc"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        assert!(pending(&layout).is_none());
        write_request(&layout, &req()).unwrap();
        assert_eq!(pending(&layout).unwrap().unwrap(), req());
        write_result(&layout, &Outcome { id: req().id, ok: false, message: "x".into(), at: 1, ..Default::default() });
        assert!(pending(&layout).is_none(), "same id: not retried");
        let mut again = req();
        again.id = "req-0002-abcd".into();
        write_request(&layout, &again).unwrap();
        assert_eq!(pending(&layout).unwrap().unwrap().id, "req-0002-abcd");
        std::fs::write(request_path(&layout), "nonsense = [").unwrap();
        let (id, e) = pending(&layout).unwrap().unwrap_err();
        assert!(id.starts_with("bad-") && e.contains(REQUEST_FILE), "{id} {e}");
    }
}
