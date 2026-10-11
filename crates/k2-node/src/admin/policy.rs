//! Remote policy (S3). Stub in S1: refuses until built.

use std::path::Path;

use k2_node_proto::admin::{code, AdminRefusal, Class};
use serde_json::Value;

use crate::paths::Layout;

pub fn classify_set(_l: &Layout, _args: &Value) -> Result<Class, AdminRefusal> {
    Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "policy.set is not built yet"))
}

pub fn apply_set(_l: &Layout, _args: &Value) -> Result<Value, AdminRefusal> {
    Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "policy.set is not built yet"))
}

pub fn apply_narrow(_config: &Path, _args: &Value) -> Result<Value, AdminRefusal> {
    Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "policy.narrow is not built yet"))
}
