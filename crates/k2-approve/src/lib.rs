//! Signed approvals (prd-signed-approvals-v1 rev 2).
//!
//! A relying party (whatever will act on a person's yes) writes the payload
//! bytes once: `K2-SIGNED-APPROVAL-1`, a zero byte, then JSON text in the
//! strict form ([`payload`]). The person's device checks the form, shows the
//! words [`describe`] makes, and signs exactly those bytes. The relying party
//! then checks the [`record`] against the keys IT trusts for that person
//! ([`keys`]) and its own copy of the bytes ([`verify`]). The check and view
//! of an approval come from the pinned key, never from the record (SA-5).
//!
//! The fixtures under `fixtures/sa1/` are shared with the phone app (Swift)
//! and k2sign (Python): every reader must reach the same verdict on each.

#[cfg(feature = "verify")]
pub mod attest;
pub mod describe;
pub mod json;
pub mod keys;
pub mod payload;
pub mod record;
pub mod time;
#[cfg(feature = "verify")]
pub mod verify;

/// The fixed prefix: these keys can never be made to sign anything that is
/// also valid somewhere else.
pub const PREFIX: &[u8] = b"K2-SIGNED-APPROVAL-1\0";
/// The whole payload (prefix included) is at most this many bytes.
pub const MAX_PAYLOAD: usize = 16384;
/// An approval record (JSON) is at most this many bytes.
pub const MAX_RECORD: usize = 32768;

/// Why something was refused. The `code` strings are the shared contract:
/// the fixtures name them, and every reader must give the same one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    // The payload's bytes and strict form.
    TooLarge,
    Prefix,
    Utf8,
    Json,
    DuplicateKey,
    ForbiddenChar,
    NotInteger,
    NotObject,
    UnknownField,
    MissingField,
    FieldType,
    FieldFormat,
    BodyValue,
    Times,
    Shape,
    // The record.
    Record,
    // The relying party's checks.
    PayloadMismatch,
    UnknownKey,
    AlgMismatch,
    BadSignature,
    KeyNotListed,
    WrongApprover,
    WrongRp,
    WrongKind,
    WeakCheck,
    WeakView,
    NotAttested,
    IssuedFuture,
    Expired,
    WrongNonce,
    NonceUsed,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::TooLarge => "too_large",
            Code::Prefix => "prefix",
            Code::Utf8 => "utf8",
            Code::Json => "json",
            Code::DuplicateKey => "duplicate_key",
            Code::ForbiddenChar => "forbidden_char",
            Code::NotInteger => "not_integer",
            Code::NotObject => "not_object",
            Code::UnknownField => "unknown_field",
            Code::MissingField => "missing_field",
            Code::FieldType => "field_type",
            Code::FieldFormat => "field_format",
            Code::BodyValue => "body_value",
            Code::Times => "times",
            Code::Shape => "shape",
            Code::Record => "record",
            Code::PayloadMismatch => "payload_mismatch",
            Code::UnknownKey => "unknown_key",
            Code::AlgMismatch => "alg_mismatch",
            Code::BadSignature => "bad_signature",
            Code::KeyNotListed => "key_not_listed",
            Code::WrongApprover => "wrong_approver",
            Code::WrongRp => "wrong_rp",
            Code::WrongKind => "wrong_kind",
            Code::WeakCheck => "weak_check",
            Code::WeakView => "weak_view",
            Code::NotAttested => "not_attested",
            Code::IssuedFuture => "issued_future",
            Code::Expired => "expired",
            Code::WrongNonce => "wrong_nonce",
            Code::NonceUsed => "nonce_used",
        }
    }
}

/// A refusal: a code from the contract and a plain sentence for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    pub detail: String,
}

impl Refusal {
    pub fn new(code: Code, detail: impl Into<String>) -> Self {
        Refusal { code, detail: detail.into() }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for Refusal {}

pub type Result<T> = std::result::Result<T, Refusal>;

pub(crate) fn refuse<T>(code: Code, detail: impl Into<String>) -> Result<T> {
    Err(Refusal::new(code, detail))
}

/// How the person proved it was them. Fixed when the key was enrolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// The device passcode, PIN or password, or a check that may have been
    /// either (Windows Hello, the Mac release key's Touch ID or password).
    Credential,
    /// A face or fingerprint, on a key nothing else unlocks.
    Biometric,
}

impl Check {
    pub fn as_str(self) -> &'static str {
        match self {
            Check::Credential => "credential",
            Check::Biometric => "biometric",
        }
    }
    pub fn parse(s: &str) -> Option<Check> {
        match s {
            "credential" => Some(Check::Credential),
            "biometric" => Some(Check::Biometric),
            _ => None,
        }
    }
}

/// Who could have shown the person something else. Fixed at enrolment.
/// Ordered weakest first: a policy of `host` accepts `host` or `device`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum View {
    /// A web page showed the bytes; its server could have shown other words.
    Page,
    /// A computer where other programs of the same user could ask the key to
    /// sign under different words. Proves presence, not content.
    Host,
    /// Only the K2 app can use the key, and its native screen showed the bytes.
    Device,
}

impl View {
    pub fn as_str(self) -> &'static str {
        match self {
            View::Page => "page",
            View::Host => "host",
            View::Device => "device",
        }
    }
    pub fn parse(s: &str) -> Option<View> {
        match s {
            "page" => Some(View::Page),
            "host" => Some(View::Host),
            "device" => Some(View::Device),
            _ => None,
        }
    }
}

/// The signature algorithm, from the trusted key's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    /// ECDSA P-256 over SHA-256 of the payload bytes; the signature is DER.
    Es256,
    /// RSASSA-PKCS1-v1_5 SHA-256 (Windows Hello keys are RSA-2048). The
    /// padding Hello uses is confirmed in slice S4 before anything relies
    /// on it; until then no relying party should enrol an RSA key.
    Rs256,
}

impl Alg {
    pub fn as_str(self) -> &'static str {
        match self {
            Alg::Es256 => "ES256",
            Alg::Rs256 => "RS256",
        }
    }
    pub fn parse(s: &str) -> Option<Alg> {
        match s {
            "ES256" => Some(Alg::Es256),
            "RS256" => Some(Alg::Rs256),
            _ => None,
        }
    }
}

/// Which App Attest environment attested a key (prd-signed-approvals-v1 SA-7).
/// A key with none was pinned on a person's fingerprint check alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Environment {
    /// Xcode builds: fine for testing relying parties, never for releases.
    Development,
    /// App Store and TestFlight builds.
    Production,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Development => "development",
            Environment::Production => "production",
        }
    }
    pub fn parse(s: &str) -> Option<Environment> {
        match s {
            "development" => Some(Environment::Development),
            "production" => Some(Environment::Production),
            _ => None,
        }
    }
}

