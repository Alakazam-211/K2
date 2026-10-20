//! Device enrolment, renewal, listing, revocation and the CRL (prd-sew-50 §10; routes K11–K15).
//! Device ids and certificate serials are Sew ids (C3; S50-m12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::b64::B64;
use super::tls::platform;
use crate::error::{Validate, ValidationError, check_known};
use crate::http::Page;
use crate::id::Id;
use crate::time::Timestamp;

/// A device public key: 65-byte SEC1 uncompressed P-256.
pub const DEVICE_PUBKEY_BYTES: usize = 65;
/// A raw r‖s ECDSA-P256 signature.
pub const P256_SIG_BYTES: usize = 64;
/// Longest device label (UTF-8 bytes).
pub const DEVICE_LABEL_MAX_BYTES: usize = 64;

unit_enum! {
    /// The device's platform (`DeviceCertTbs.platform` 1–5).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DevicePlatform {
        /// A browser.
        Web = "web",
        /// A desktop app.
        Desktop = "desktop",
        /// iOS.
        Ios = "ios",
        /// Android.
        Android = "android",
        /// Anything else.
        Other = "other",
    }
}

impl DevicePlatform {
    /// The certificate code (`None` for `Unknown`).
    pub fn code(self) -> Option<u8> {
        platform::code(self.as_str())
    }

    /// The platform of a certificate code.
    pub fn from_code(code: u8) -> Option<DevicePlatform> {
        platform::name(code).and_then(DevicePlatform::from_wire)
    }
}

/// K11 body: enrol a device key for the caller (S50 §10.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceEnrollRequest {
    /// 65-byte SEC1 uncompressed P-256 public key.
    pub pubkey: B64,
    /// The platform.
    pub platform: DevicePlatform,
    /// A human label (≤ 64 bytes UTF-8), for example "Chrome on macOS".
    pub label: String,
    /// Raw r‖s ECDSA-P256-SHA256 over [`super::tls::enroll_proof_input`].
    pub proof: B64,
}

impl Validate for DeviceEnrollRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.pubkey.len() != DEVICE_PUBKEY_BYTES {
            return Err(ValidationError::invalid(
                "pubkey",
                "pubkey must be a 65-byte SEC1 uncompressed P-256 point",
            ));
        }
        check_known("platform", self.platform)?;
        if self.label.len() > DEVICE_LABEL_MAX_BYTES {
            return Err(ValidationError::invalid(
                "label",
                format!("label must be at most {DEVICE_LABEL_MAX_BYTES} bytes"),
            ));
        }
        if self.proof.len() != P256_SIG_BYTES {
            return Err(ValidationError::invalid(
                "proof",
                "proof must be a 64-byte raw r||s signature",
            ));
        }
        Ok(())
    }
}

/// K11 and K12 answer: a fresh device certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceCertificateResponse {
    /// The device.
    pub device_id: Id,
    /// The certificate's serial.
    pub serial: Id,
    /// TLS `SewDeviceCert`.
    pub cert: B64,
    /// When the certificate expires (informational; the certificate is authoritative).
    pub not_after: Timestamp,
}

/// One of the caller's devices (K13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceSummary {
    /// The device.
    pub device_id: Id,
    /// The platform.
    pub platform: DevicePlatform,
    /// The label.
    pub label: String,
    /// When it was enrolled.
    pub created_at: Timestamp,
    /// The newest certificate's serial.
    pub serial: Id,
    /// When the newest certificate expires.
    pub not_after: Timestamp,
    /// When it was revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
}

/// K13 answer: the caller's devices (one page, `has_more: false`).
pub type DevicePage = Page<DeviceSummary>;

coded_enum! {
    /// Why a device was revoked (`call_devices.revoke_reason`). A caller may send `lost`,
    /// `retired`, `compromised` or `other`; the server uses the last two values itself.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DeviceRevokeReason {
        /// The device was lost or stolen.
        Lost = "lost" => 1,
        /// The device is no longer used.
        Retired = "retired" => 2,
        /// The key may be compromised.
        Compromised = "compromised" => 3,
        /// Anything else.
        Other = "other" => 4,
        /// The login it was enrolled under was revoked (automatic).
        LoginRevoked = "login_revoked" => 5,
        /// The member was deprovisioned (automatic).
        Deprovisioned = "deprovisioned" => 6,
    }
}

impl DeviceRevokeReason {
    /// Whether a caller may send it (the automatic reasons are the server's).
    pub const fn caller_may_send(self) -> bool {
        matches!(
            self,
            DeviceRevokeReason::Lost
                | DeviceRevokeReason::Retired
                | DeviceRevokeReason::Compromised
                | DeviceRevokeReason::Other
        )
    }
}

/// K14 body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceRevokeRequest {
    /// Why (default `other`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<DeviceRevokeReason>,
}

impl Validate for DeviceRevokeRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(r) = self.reason {
            check_known("reason", r)?;
            if !r.caller_may_send() {
                return Err(ValidationError::invalid(
                    "reason",
                    "reason must be lost, retired, compromised or other",
                ));
            }
        }
        Ok(())
    }
}

/// K14 answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceRevokeResponse {
    /// Always `true`.
    pub ok: bool,
    /// Every serial of the device, now in the CRL.
    pub serials: Vec<Id>,
}

/// K15 answer: revoked, unexpired serials after `after`, ascending (only when READY says
/// `crl_partial`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CrlPage {
    /// The serials.
    pub items: Vec<Id>,
    /// More remain.
    pub has_more: bool,
}

/// K15 query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CrlQuery {
    /// Serials after this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size (1–5,000; default 1,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The largest K15 page.
pub const CRL_PAGE_MAX: u32 = 5_000;

impl Validate for CrlQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.limit
            && (l == 0 || l > CRL_PAGE_MAX)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{CRL_PAGE_MAX}"),
            ));
        }
        Ok(())
    }
}
