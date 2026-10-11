//! Signed self-update + rollback (S5). Stub in S1: refuses until built.

use k2_node_proto::admin::{code, AdminRefusal};
use serde_json::Value;

use super::exec::{Checked, Executor};

pub fn apply(_e: &Executor, _c: &Checked, _sig: &str, _bytes: &[u8]) -> Result<Value, AdminRefusal> {
    Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "node.update is not built yet"))
}

pub fn rollback(_e: &Executor, _why: &str) -> Result<Value, AdminRefusal> {
    Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "node.rollback is not built yet"))
}

pub fn resume_pending(_e: &Executor) {}
