//! Channel 0 of the sim stream: control JSON between the server's remote
//! runner and the node's Simulator service.
//!
//! Every request carries an `id`; the node answers each with one
//! [`Reply`]. Helpers live on channels 1…n: the server sends each one the
//! helper's own JSON request lines (policed by the node, [`super::policy`]),
//! and the node sends back the helper's framed messages unchanged.
//!
//! Nothing here is ever logged with its arguments: k2-node and simhost log
//! the op name and how long it took, and nothing else (§3.7).

use serde::{Deserialize, Serialize};

/// Channel 0 is control; helpers start at 1.
pub const CONTROL: u8 = 0;
pub const FIRST_HELPER: u8 = 1;
/// Upload chunks (raw bytes, base64 in JSON).
pub const UPLOAD_CHUNK: usize = 256 * 1024;
/// Largest stdout / stderr a `run` returns.
pub const MAX_RUN_OUTPUT: usize = 4 << 20;
/// Largest file a `read` returns (a screenshot PNG).
pub const MAX_READ: usize = 12 << 20;

/// Disk ops the iOS lane needs on the node (IN-A3). Each path is policed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskOp {
    EnsureDir,
    IsDir,
    IsFile,
    Read,
    RemoveFile,
    RemoveDirAll,
}

impl DiskOp {
    pub fn as_str(self) -> &'static str {
        match self {
            DiskOp::EnsureDir => "ensure_dir",
            DiskOp::IsDir => "is_dir",
            DiskOp::IsFile => "is_file",
            DiskOp::Read => "read",
            DiskOp::RemoveFile => "remove_file",
            DiskOp::RemoveDirAll => "remove_dir_all",
        }
    }
}

/// Server → node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    /// Run one allow-listed program ([`super::policy::check_run`]).
    Run { id: u64, program: String, args: Vec<String>, timeout_ms: u64 },
    Disk { id: u64, op: DiskOp, path: String },
    /// One piece of a `.app` pack ([`super::pack`]). `offset` is where it
    /// goes; `last` unpacks it and answers [`ReplyBody::Path`].
    Upload { id: u64, sha256: String, total: u64, offset: u64, data: String, last: bool },
    /// Start the helper for `udid` on `channel` (the udid must be in this
    /// slot's set). Answers once the helper said `ready`.
    HelperStart { id: u64, channel: u8, udid: String },
    HelperStop { id: u64, channel: u8 },
    /// The server took the last frame on `channel` (one in flight, §3.4).
    Ack { channel: u8 },
    /// The server is done with the stream (the session continues until
    /// `sim_close`).
    Bye,
}

impl Request {
    pub fn id(&self) -> Option<u64> {
        match self {
            Request::Run { id, .. } | Request::Disk { id, .. } | Request::Upload { id, .. } | Request::HelperStart { id, .. } | Request::HelperStop { id, .. } => Some(*id),
            Request::Ack { .. } | Request::Bye => None,
        }
    }

    /// The op name, for the only log line anyone writes.
    pub fn op(&self) -> &'static str {
        match self {
            Request::Run { .. } => "run",
            Request::Disk { op, .. } => op.as_str(),
            Request::Upload { .. } => "upload",
            Request::HelperStart { .. } => "helper_start",
            Request::HelperStop { .. } => "helper_stop",
            Request::Ack { .. } => "ack",
            Request::Bye => "bye",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum ReplyBody {
    Run { status: i32, stdout: String, stderr: String },
    Bool { value: bool },
    /// base64.
    Bytes { data: String },
    Path { path: String },
    Ok,
    /// Policy said no ([`super::codes::NODE_REFUSED`]).
    Refused { code: String, message: String },
    /// It was allowed and failed.
    Failed { code: String, message: String },
}

/// Node → server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    #[serde(flatten)]
    pub body: ReplyBody,
}

/// Node → server on channel 0 when nothing asked: a helper channel ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum Event {
    HelperGone { channel: u8, reason: String },
}

/// Either kind on channel 0, node → server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FromNode {
    Reply(Reply),
    Event(Event),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_round_trip() {
        let r = Request::Run { id: 3, program: "xcrun".into(), args: vec!["simctl".into()], timeout_ms: 1000 };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
        assert_eq!(r.op(), "run");
        let rep = FromNode::Reply(Reply { id: 3, body: ReplyBody::Bool { value: true } });
        let s = serde_json::to_string(&rep).unwrap();
        assert_eq!(serde_json::from_str::<FromNode>(&s).unwrap(), rep);
        let ev = FromNode::Event(Event::HelperGone { channel: 2, reason: "device stopped".into() });
        let s = serde_json::to_string(&ev).unwrap();
        assert_eq!(serde_json::from_str::<FromNode>(&s).unwrap(), ev);
    }
}
