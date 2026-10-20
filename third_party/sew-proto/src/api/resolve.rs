//! Address resolution (C4): `GET /api/v1/resolve?addr=<addr>` with `#` sent as `%23`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Addr;
use crate::error::{Validate, ValidationError};
use crate::id::Id;

/// Query of `GET /api/v1/resolve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveQuery {
    /// The address to resolve.
    pub addr: Addr,
}

impl Validate for ResolveQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Response of `GET /api/v1/resolve`. A hidden address gives the same `404` as a missing one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolveResponse {
    /// The space; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// The channel.
    pub channel_id: Id,
    /// The thread root, when the address names a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
}
