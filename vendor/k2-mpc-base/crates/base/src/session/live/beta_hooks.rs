//! Test builds only: what the native engine Beta's rows (`session/native_beta_tests.rs`) read
//! of a session.

use crate::session::error::Result;

impl super::Session {
    /// Whether the session's doc runs on the native engine.
    pub(crate) fn is_native_for_tests(&self) -> Result<bool> {
        Ok(self.lock()?.doc.is_native())
    }
}

/// Whether `bytes` are a text-only disk replica's stored form.
pub(crate) fn is_text_replica_for_tests(bytes: &[u8]) -> bool {
    super::text_replica::is_text_replica(bytes)
}
