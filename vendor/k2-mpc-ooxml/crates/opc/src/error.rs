use std::fmt;

/// Why a package can't be held: a pack then leaves the file at tier 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decline {
    /// Stable machine code, e.g. `zip_overlap`, `doctype`, `macros`.
    pub code: &'static str,
    /// What was found, for a person.
    pub message: String,
}

impl Decline {
    /// A refusal with its stable code and a message for a person.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Decline {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Decline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for Decline {}
