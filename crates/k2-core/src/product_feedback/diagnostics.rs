//! Diagnostics: details about K2 on this computer, sent only with the
//! person's consent, exactly as previewed (PRD §6a).

use serde::{Deserialize, Serialize};

/// One section of the bundle, as previewed and as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub text: String,
}
