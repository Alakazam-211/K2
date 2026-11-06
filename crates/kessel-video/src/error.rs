//! Errors.

/// Every error kessel-video returns or reports.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VideoError {
    /// The OS permission is denied or restricted (camera, screen recording).
    #[error("no_permission")]
    NoPermission,
    /// Another app holds the device exclusively.
    #[error("device_busy")]
    DeviceBusy,
    /// The device went away (unplugged, display disconnected, window closed).
    #[error("device_lost")]
    DeviceLost,
    /// No such camera or screen source (or the picker was cancelled).
    #[error("not_found")]
    NotFound,
    /// The codec, profile, size, rate or format isn't available here.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The hardware session limit is reached (`max_instances`).
    #[error("no_resources")]
    NoResources,
    /// A bounded queue is full; try again after the next wake.
    #[error("busy")]
    Busy,
    /// The OS failed (`code`: OSStatus, HRESULT or errno; `detail`: human
    /// text). Never carries frame bytes.
    #[error("os {code}: {detail}")]
    Os {
        /// The OS's own code.
        code: i64,
        /// What failed.
        detail: String,
    },
    /// After `shutdown`, or the app is in the background on a phone.
    #[error("shut_down")]
    ShutDown,
    /// A bad argument from the caller: a frame whose planes don't fit its
    /// buffer, an odd or zero coded size, a zero rate. Not in Sew's §A4
    /// draft; added because none of the others fits a caller's mistake.
    #[error("invalid: {0}")]
    Invalid(String),
}

impl VideoError {
    /// The stable short code: `no_permission`, `device_busy`, `device_lost`,
    /// `not_found`, `unsupported`, `no_resources`, `busy`, `os`, `shut_down`
    /// or `invalid`.
    pub fn code(&self) -> &'static str {
        match self {
            VideoError::NoPermission => "no_permission",
            VideoError::DeviceBusy => "device_busy",
            VideoError::DeviceLost => "device_lost",
            VideoError::NotFound => "not_found",
            VideoError::Unsupported(_) => "unsupported",
            VideoError::NoResources => "no_resources",
            VideoError::Busy => "busy",
            VideoError::Os { .. } => "os",
            VideoError::ShutDown => "shut_down",
            VideoError::Invalid(_) => "invalid",
        }
    }

    /// An OS failure.
    pub fn os(code: i64, detail: impl Into<String>) -> VideoError {
        VideoError::Os {
            code,
            detail: detail.into(),
        }
    }
}

pub(crate) fn invalid(detail: impl Into<String>) -> VideoError {
    VideoError::Invalid(detail.into())
}

pub(crate) fn unsupported(detail: impl Into<String>) -> VideoError {
    VideoError::Unsupported(detail.into())
}

/// Decoder failures. All but `Busy` mean: send a PLI and call `reset` (the
/// decoder already drops input until a keyframe).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// A delta frame arrived first or after a reset or error: it was
    /// dropped; the decoder waits for a keyframe.
    #[error("need_keyframe")]
    NeedKeyframe,
    /// The input queue is full; retry after the next wake (or drop deltas
    /// until a keyframe).
    #[error("busy")]
    Busy,
    /// The bitstream is broken.
    #[error("corrupt: {0}")]
    Corrupt(String),
    /// The OS session is gone (GPU reset, backgrounded); `reset` reopens it.
    #[error("session_lost")]
    SessionLost,
    /// Anything else.
    #[error(transparent)]
    Video(#[from] VideoError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_print_their_codes() {
        assert_eq!(VideoError::NoPermission.to_string(), "no_permission");
        assert_eq!(VideoError::Unsupported("VP9".into()).to_string(), "unsupported: VP9");
        assert_eq!(VideoError::os(-12902, "VTDecompressionSessionCreate").to_string(), "os -12902: VTDecompressionSessionCreate");
        assert_eq!(VideoError::os(1, "x").code(), "os");
        assert_eq!(DecodeError::NeedKeyframe.to_string(), "need_keyframe");
        assert_eq!(DecodeError::from(VideoError::Busy).to_string(), "busy");
    }
}
