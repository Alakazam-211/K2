//! Every `details.reason` string of the call key routes (prd-sew-50 §9.1, §10, §11, §14.3,
//! §18), plus the values of `details.check` and `details.unsupported`. Clients match on these;
//! renaming one is a contract change.

/// `409`: the ref was already claimed or consumed (§11.1).
pub const E2EE_KEY_PACKAGE_REF_USED: &str = "e2ee_key_package_ref_used";
/// `422`: unknown, expired, another member's or another login's ref; one answer for all (§11.1).
pub const E2EE_KEY_PACKAGE_REF_UNKNOWN: &str = "e2ee_key_package_ref_unknown";
/// `503`: the install-wide ref store or a call's actor inbox is full (§11.1, §18).
pub const E2EE_BUSY: &str = "e2ee_busy";
/// `503`: the call's leaf or queue cap is reached (§13.1, §18).
pub const E2EE_CALL_FULL: &str = "e2ee_call_full";
/// `422`: a KeyPackage failed an upload or join check; `details.check` names it (§11.2).
pub const E2EE_KEY_PACKAGE_INVALID: &str = "e2ee_key_package_invalid";
/// `422`: the client cannot do encrypted calls; `details.unsupported` names why (§11.3 J1).
pub const E2EE_UNSUPPORTED_CLIENT: &str = "e2ee_unsupported_client";
/// `422`: `gateway_session` is not the caller's live session on this login (§8.3).
pub const E2EE_GATEWAY_SESSION_INVALID: &str = "e2ee_gateway_session_invalid";
/// `409`: K2 with a second, different ref for a join id that is already keyed (§9.1).
pub const E2EE_JOIN_ALREADY_KEYED: &str = "e2ee_join_already_keyed";
/// `404`: K2 for a join id with no expected join and no live voice entry of the caller (§9.1).
pub const KEY_JOIN_REQUIRES_LIVE_ADMIT: &str = "key_join_requires_live_admit";
/// `409`: K3 from a client behind the DS's epoch; `details.epoch` (§9.1).
pub const E2EE_EPOCH_BEHIND: &str = "e2ee_epoch_behind";
/// `409`: K3 for another group (§9.1).
pub const E2EE_GROUP_MISMATCH: &str = "e2ee_group_mismatch";
/// `409`: K3 for a join id with no leaf (§9.1).
pub const E2EE_NOT_IN_GROUP: &str = "e2ee_not_in_group";
/// `422`: the creator's group was refused (§12.1).
pub const E2EE_GROUP_INVALID: &str = "e2ee_group_invalid";
/// `409`: another commit won; `details.epoch` (§12.4).
pub const E2EE_COMMIT_SUPERSEDED: &str = "e2ee_commit_superseded";
/// `409`: an epoch the DS already left; `details.epoch` (§12.4, §14.3).
pub const E2EE_STALE_EPOCH: &str = "e2ee_stale_epoch";
/// `409`: the caller was not asked to commit (§12.4).
pub const E2EE_NOT_COMMITTER: &str = "e2ee_not_committer";
/// `409`: nothing is proposed (§12.4).
pub const E2EE_NO_TRANSITION: &str = "e2ee_no_transition";
/// `422`: the commit failed a check; `details.check` names it (§12.4).
pub const E2EE_COMMIT_INVALID: &str = "e2ee_commit_invalid";
/// `422`: K5 for an epoch above the DS's (§9.1).
pub const E2EE_EPOCH_AHEAD: &str = "e2ee_epoch_ahead";
/// `409`: K7 while no new group is collecting (§12.9).
pub const E2EE_NO_RESET: &str = "e2ee_no_reset";
/// `403`: a release from a member whose consent is not granted (§14.3 rule 3).
pub const E2EE_CONSENT_NOT_GRANTED: &str = "e2ee_consent_not_granted";
/// `409`: no live recording, or another one (§14.3 rule 1).
pub const E2EE_RECORDING_MISMATCH: &str = "e2ee_recording_mismatch";
/// `422`: a release for another identity (§14.3 rule 2).
pub const E2EE_SENDER_IDENTITY_MISMATCH: &str = "e2ee_sender_identity_mismatch";
/// `422`: a release whose `leaf` is not the caller's leaf in that epoch (§14.3 rule 2).
pub const E2EE_RELEASE_LEAF_MISMATCH: &str = "e2ee_release_leaf_mismatch";
/// `422`: a release with a wrong slot or `enc`/`ct` size (§14.3 rules 4–5).
pub const E2EE_RELEASE_MALFORMED: &str = "e2ee_release_malformed";
/// `422`: the device or its certificate is revoked (§10.3, §11.2 U5).
pub const DEVICE_REVOKED: &str = "device_revoked";
/// `409`: the key is enrolled for another member (§10.2).
pub const DEVICE_KEY_IN_USE: &str = "device_key_in_use";
/// `403`: key and device routes refuse act-as, as voice does (§9.1).
pub const AGENTS_NOT_IN_VOICE: &str = "agents_not_in_voice";
/// `503`: the DS is rebuilding and cannot answer (`details.engine = syncing`, `Retry-After: 1`).
pub const VOICE_UNAVAILABLE: &str = "voice_unavailable";

/// `details.check` of `e2ee_commit_invalid` (§9.1 K4, §12.4).
pub mod commit_check {
    /// Not an `MLSMessage(PublicMessage)`.
    pub const DECODE: &str = "decode";
    /// Not a plaintext commit of this group and epoch.
    pub const WIRE_FORMAT: &str = "wire_format";
    /// The framing sender is not the caller's leaf.
    pub const SENDER: &str = "sender";
    /// The proposal references differ from the DS's pending set.
    pub const PROPOSAL_SET: &str = "proposal_set";
    /// A proposal by value.
    pub const BY_VALUE: &str = "by_value";
    /// No update path.
    pub const NO_PATH: &str = "no_path";
    /// The committer's credential changed.
    pub const CREDENTIAL_CHANGED: &str = "credential_changed";
    /// The Welcome's joiners differ from the batch's Adds.
    pub const WELCOME_MISMATCH: &str = "welcome_mismatch";
    /// The GroupInfo is wrong or unsigned.
    pub const GROUP_INFO: &str = "group_info";
    /// openmls refused it (`details.mls` names its error kind).
    pub const MLS: &str = "mls";
    /// Every value.
    pub const ALL: &[&str] = &[
        DECODE,
        WIRE_FORMAT,
        SENDER,
        PROPOSAL_SET,
        BY_VALUE,
        NO_PATH,
        CREDENTIAL_CHANGED,
        WELCOME_MISMATCH,
        GROUP_INFO,
        MLS,
    ];
}

/// `details.check` of `e2ee_key_package_invalid` (§11.2 U1–U6, §11.3 J5).
pub mod key_package_check {
    /// Not an `MLSMessage(KeyPackage)`, or over 4 KiB.
    pub const DECODE: &str = "decode";
    /// openmls' signature or init-key check failed.
    pub const SIGNATURE: &str = "signature";
    /// Not ciphersuite 1.
    pub const CIPHERSUITE: &str = "ciphersuite";
    /// Missing a required capability.
    pub const CAPABILITIES: &str = "capabilities";
    /// Lifetime too long or expired.
    pub const LIFETIME: &str = "lifetime";
    /// Not a Basic `SewCallIdentity` v2 member credential.
    pub const CREDENTIAL: &str = "credential";
    /// The scope is empty, malformed or another channel's.
    pub const SCOPE: &str = "scope";
    /// The device certificate failed.
    pub const DEVICE_CERT: &str = "device_cert";
    /// `device_sig` failed.
    pub const DEVICE_SIG: &str = "device_sig";
    /// Every value.
    pub const ALL: &[&str] = &[
        DECODE,
        SIGNATURE,
        CIPHERSUITE,
        CAPABILITIES,
        LIFETIME,
        CREDENTIAL,
        SCOPE,
        DEVICE_CERT,
        DEVICE_SIG,
    ];
}

/// `details.unsupported` of `e2ee_unsupported_client` (§11.3 J1).
pub mod unsupported {
    /// The join carried no `e2ee` object.
    pub const E2EE: &str = "e2ee";
    /// `caps.sender_key_index` is false, or the device is in the stuck-sender refusal memory
    /// (`details.observed = true`).
    pub const SENDER_KEY_INDEX: &str = "sender_key_index";
    /// `caps.kdf` is not `hkdf`.
    pub const KDF: &str = "kdf";
    /// `caps.key_ring_size` is not 64.
    pub const KEY_RING_SIZE: &str = "key_ring_size";
    /// `caps.ciphersuites` lacks 1.
    pub const CIPHERSUITE: &str = "ciphersuite";
    /// `caps.browser` is refused (`refused_browsers`).
    pub const BROWSER: &str = "browser";
    /// Every value.
    pub const ALL: &[&str] = &[
        E2EE,
        SENDER_KEY_INDEX,
        KDF,
        KEY_RING_SIZE,
        CIPHERSUITE,
        BROWSER,
    ];
}

/// Every reason above (for lints and docs).
pub const ALL: &[&str] = &[
    E2EE_KEY_PACKAGE_REF_USED,
    E2EE_KEY_PACKAGE_REF_UNKNOWN,
    E2EE_BUSY,
    E2EE_CALL_FULL,
    E2EE_KEY_PACKAGE_INVALID,
    E2EE_UNSUPPORTED_CLIENT,
    E2EE_GATEWAY_SESSION_INVALID,
    E2EE_JOIN_ALREADY_KEYED,
    KEY_JOIN_REQUIRES_LIVE_ADMIT,
    E2EE_EPOCH_BEHIND,
    E2EE_GROUP_MISMATCH,
    E2EE_NOT_IN_GROUP,
    E2EE_GROUP_INVALID,
    E2EE_COMMIT_SUPERSEDED,
    E2EE_STALE_EPOCH,
    E2EE_NOT_COMMITTER,
    E2EE_NO_TRANSITION,
    E2EE_COMMIT_INVALID,
    E2EE_EPOCH_AHEAD,
    E2EE_NO_RESET,
    E2EE_CONSENT_NOT_GRANTED,
    E2EE_RECORDING_MISMATCH,
    E2EE_SENDER_IDENTITY_MISMATCH,
    E2EE_RELEASE_LEAF_MISMATCH,
    E2EE_RELEASE_MALFORMED,
    DEVICE_REVOKED,
    DEVICE_KEY_IN_USE,
    AGENTS_NOT_IN_VOICE,
    VOICE_UNAVAILABLE,
];

#[cfg(test)]
mod tests {
    #[test]
    fn reasons_are_unique_lower_snake() {
        let mut seen = std::collections::BTreeSet::new();
        for r in super::ALL {
            assert!(seen.insert(*r), "{r} twice");
            assert!(
                r.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{r}"
            );
        }
    }
}
