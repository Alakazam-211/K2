//! `desktop packages install` (protocol 16): not built yet (tests first).

use serde_json::Value as J;

use crate::ctx::{Ctx, Fail, HResult};

pub fn packages_install(_ctx: &Ctx) -> HResult<J> {
    Err(Fail::state("not_built"))
}
