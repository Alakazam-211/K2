//! The engine seam. Private to the kit: no pack sees an engine type.
//!
//! To replace the engine, implement [`EngineDoc`] and switch [`crate::doc::Doc`] to it.
//! Packs, sessions and stored formats above this line don't change (stored state carries
//! an engine/format stamp so it can be re-encoded on upgrade).

mod yrs;

pub(crate) use self::yrs::YrsDoc;

use crate::doc::{ClientId, DocError, Edit, UpdateInfo};
use crate::schema::Schema;

/// The doc's own client ID. It never authors content.
pub(crate) const SESSION_CLIENT: ClientId = 1;

/// Engine identity and stored-state encoding, stamped on each doc in the store
/// ([`crate::store::DocMeta`]) so a later release can re-encode on upgrade.
pub(crate) const ENGINE_NAME: &str = "yrs-0.28";
/// Stored state is Yjs v2.
pub(crate) const ENGINE_FORMAT: u32 = 2;

#[allow(dead_code)] // the trait documents the seam; one implementation today
pub(crate) trait EngineDoc: Sized + Send {
    fn new(schema: &Schema) -> Self;
    fn load(schema: &Schema, state: &[u8]) -> Result<Self, DocError>;
    fn text(&self, root: &str) -> String;
    fn state_vector(&self) -> Vec<u8>;
    fn encode_state(&self) -> Vec<u8>;
    fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError>;
    fn check_update(&self, update: &[u8], allowed: ClientId) -> Result<UpdateInfo, DocError>;
    fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError>;
    /// Apply already-validated edits as `client` through a throwaway replica; return the update.
    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Vec<u8>, DocError>;
}
