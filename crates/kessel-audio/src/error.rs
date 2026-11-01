//! Errors.

use std::fmt;

/// What went wrong, as a stable code. The string forms
/// ([`ErrorCode::as_str`]) are part of the public API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// A device in use went away (unplugged, disabled, or its stream died).
    DeviceLost,
    /// The device is in use by something else, or temporarily unavailable.
    DeviceBusy,
    /// The OS refused access (for example microphone privacy settings).
    NoPermission,
    /// No such device, or no device of that kind at all.
    NoDevice,
    /// The device can't run any configuration we can use.
    UnsupportedConfig,
    /// The configuration passed in is invalid.
    InvalidConfig,
    /// Anything else from the audio backend.
    Backend,
}

impl ErrorCode {
    /// The stable string form: `device_lost`, `device_busy`,
    /// `no_permission`, `no_device`, `unsupported_config`, `invalid_config`
    /// or `backend`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::DeviceLost => "device_lost",
            ErrorCode::DeviceBusy => "device_busy",
            ErrorCode::NoPermission => "no_permission",
            ErrorCode::NoDevice => "no_device",
            ErrorCode::UnsupportedConfig => "unsupported_config",
            ErrorCode::InvalidConfig => "invalid_config",
            ErrorCode::Backend => "backend",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error with its code and a human-readable detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioError {
    /// The stable code.
    pub code: ErrorCode,
    /// Free text for logs; not stable.
    pub detail: String,
}

impl AudioError {
    pub(crate) fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        AudioError {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for AudioError {}
