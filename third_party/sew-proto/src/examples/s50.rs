//! Examples for the wire types S50 adds (prd-sew-50 §9, §10, §19.2, §20). Blobs are short fake
//! bytes; the binary structures have their own vectors (`tests/fixtures/e2ee/vectors/`).

use super::{ID_CHANNEL, t};
use crate::e2ee::tls::{EXPORTER_LABEL, RELEASE_INFO_LABEL};
use crate::e2ee::*;
use crate::id::Id;
use crate::registry::Example;

const ID_DEVICE: Id = super::id(501);
const ID_DEVICE_2: Id = super::id(502);
const ID_SERIAL: Id = super::id(503);
const ID_SERIAL_2: Id = super::id(504);
const ID_SERIAL_3: Id = super::id(505);
const ID_RECORDING: Id = super::id(506);

fn filled(n: usize, seed: u8) -> B64 {
    B64::new(
        (0..n)
            .map(|i| seed.wrapping_add(i as u8))
            .collect::<Vec<u8>>(),
    )
}

fn name<T: std::str::FromStr>(s: &str) -> T
where
    T::Err: std::fmt::Display,
{
    match s.parse() {
        Ok(v) => v,
        Err(e) => panic!("example name {s:?}: {e}"),
    }
}

pub(super) fn call_id() -> CallId {
    CallId::from_random([
        0x9a, 0x1f, 0x33, 0x04, 0x51, 0x66, 0x70, 0x88, 0x91, 0xa2, 0xb3, 0xc4, 0xd5, 0xe6, 0xf7,
        0x08,
    ])
}

pub(super) fn join_id() -> JoinIdHex {
    name("5b0e00ff102030405060708090a0b0c0")
}

fn join_id_2() -> JoinIdHex {
    name("0123456789abcdef0123456789abcdef")
}

fn recorder_identity() -> LkIdentity {
    JoinIdHex::from_bytes([7; 16]).lk_identity()
}

fn group_id() -> B64 {
    filled(16, 0x40)
}

fn kp_ref() -> B64 {
    filled(32, 0x10)
}

fn kp_ref_2() -> B64 {
    filled(32, 0x90)
}

fn caps_full() -> E2eeCaps {
    E2eeCaps {
        sender_key_index: true,
        kdf: KDF_HKDF.into(),
        key_ring_size: 64,
        sdk: "js".into(),
        sdk_version: "2.22.3".into(),
        browser: Some("chrome".into()),
        lib: Some("sew-e2ee/0.1.0".into()),
        ciphersuites: Some(vec![CIPHERSUITE_V1]),
    }
}

fn caps_min() -> E2eeCaps {
    E2eeCaps {
        sender_key_index: true,
        kdf: KDF_HKDF.into(),
        key_ring_size: 64,
        sdk: "rust".into(),
        sdk_version: "0.7.2".into(),
        browser: None,
        lib: None,
        ciphersuites: None,
    }
}

pub(super) fn e2ee_join_full() -> E2eeJoin {
    E2eeJoin {
        key_package_ref: kp_ref(),
        gateway_session: "gw_5f2b9c0e".into(),
        caps: caps_full(),
    }
}

impl Example for E2eeCaps {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", caps_full()), ("min", caps_min())]
    }
}

impl Example for E2eeJoin {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", e2ee_join_full()),
            (
                "min",
                E2eeJoin {
                    caps: caps_min(),
                    ..e2ee_join_full()
                },
            ),
        ]
    }
}

impl Example for KeyJoinRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyJoinRequest {
                join_id: join_id(),
                gateway_session: "gw_5f2b9c0e".into(),
                key_package_ref: kp_ref_2(),
                caps: caps_full(),
            },
        )]
    }
}

pub(super) fn params() -> CallKeysParams {
    CallKeysParams {
        ciphersuite: CIPHERSUITE_V1,
        ciphersuites: vec![CIPHERSUITE_V1],
        key_ring_size: 64,
        kdf: KDF_HKDF.into(),
        exporter_label: EXPORTER_LABEL.into(),
        release_info_label: RELEASE_INFO_LABEL.into(),
        retire_after_ms: 10_000,
        client_activate_fallback_ms: 2_500,
        key_package_max_lifetime_s: 7_200,
        unknown_call_buffer_ms: 5_000,
        securing_indicator_after_ms: 1_500,
    }
}

impl Example for CallKeysParams {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", params())]
    }
}

pub(super) fn init_full() -> CallKeysInit {
    CallKeysInit {
        call_id: call_id(),
        join_id: join_id(),
        lk_identity: join_id().lk_identity(),
        scope: CallScope::call(ID_CHANNEL),
        group_id: group_id(),
        epoch: 17,
        create: false,
        activate_on_create: false,
        slot_base: 0,
        external_sender: filled(48, 0x20),
        params: params(),
    }
}

fn init_create() -> CallKeysInit {
    CallKeysInit {
        epoch: 0,
        create: true,
        activate_on_create: true,
        ..init_full()
    }
}

impl Example for CallKeysInit {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", init_full()), ("create", init_create())]
    }
}

impl Example for KeyPackageUploadRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                KeyPackageUploadRequest {
                    key_packages: vec![filled(24, 0x01), filled(24, 0x81)],
                },
            ),
            (
                "min",
                KeyPackageUploadRequest {
                    key_packages: vec![filled(24, 0x01)],
                },
            ),
        ]
    }
}

fn ref_item() -> KeyPackageRefItem {
    KeyPackageRefItem {
        key_package_ref: kp_ref(),
        expires_at_ms: 1_790_514_000_123,
    }
}

impl Example for KeyPackageRefItem {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", ref_item())]
    }
}

impl Example for KeyPackageUploadResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyPackageUploadResponse {
                items: vec![
                    ref_item(),
                    KeyPackageRefItem {
                        key_package_ref: kp_ref_2(),
                        expires_at_ms: 1_790_514_000_123,
                    },
                ],
            },
        )]
    }
}

fn leaf_entry(leaf: u32, j: JoinIdHex) -> LeafBindingEntryJson {
    LeafBindingEntryJson {
        leaf: Some(leaf),
        key_package_ref: None,
        lk_identity: j.lk_identity(),
        join_id: j,
    }
}

fn ref_entry(r: B64, j: JoinIdHex) -> LeafBindingEntryJson {
    LeafBindingEntryJson {
        leaf: None,
        key_package_ref: Some(r),
        lk_identity: j.lk_identity(),
        join_id: j,
    }
}

/// A key-type-1 table (leaves).
pub(super) fn table_leaves() -> LeafBindingTable {
    LeafBindingTable {
        call_id: call_id(),
        group_id: group_id(),
        epoch: 18,
        entries: vec![leaf_entry(0, join_id_2()), leaf_entry(4, join_id())],
        sig: filled(64, 0x60),
    }
}

/// A key-type-2 table (one Add, by ref).
fn table_refs() -> LeafBindingTable {
    LeafBindingTable {
        call_id: call_id(),
        group_id: group_id(),
        epoch: 17,
        entries: vec![ref_entry(kp_ref(), join_id())],
        sig: filled(64, 0x61),
    }
}

impl Example for LeafBindingEntryJson {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", leaf_entry(4, join_id())),
            ("full_ref", ref_entry(kp_ref(), join_id())),
        ]
    }
}

impl Example for LeafBindingTable {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", table_leaves()), ("refs", table_refs())]
    }
}

impl Example for BindingsUpdate {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            BindingsUpdate {
                group_id: group_id(),
                epoch: 18,
                slot_base: 0,
                table: table_leaves(),
            },
        )]
    }
}

fn ca_key() -> CallPublicKey {
    CallPublicKey {
        kid: "9a1f33045166708a".into(),
        public_key: filled(32, 0xa0),
    }
}

impl Example for CallPublicKey {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", ca_key())]
    }
}

impl Example for CallPublicKeys {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            CallPublicKeys {
                ca: ca_key(),
                ds: CallPublicKey {
                    kid: "04be71c2a3d4e5f6".into(),
                    public_key: filled(32, 0xc0),
                },
            },
        )]
    }
}

fn state_req() -> KeyStateRequest {
    KeyStateRequest {
        join_id: join_id(),
        gateway_session: "gw_5f2b9c0e".into(),
        group_id: group_id(),
        leaf: 4,
        epoch: 17,
        active_epoch: 17,
        slot_base: 0,
        tree_hash: filled(32, 0x30),
        bindings_hash: filled(32, 0x31),
        group_info: filled(40, 0x32),
        ratchet_tree: Some(filled(64, 0x33)),
    }
}

impl Example for KeyStateRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", state_req()),
            (
                "min",
                KeyStateRequest {
                    ratchet_tree: None,
                    ..state_req()
                },
            ),
        ]
    }
}

impl Example for KeyStateResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                KeyStateResponse {
                    status: KeyStateStatus::Rebuilding,
                    retry_after_ms: Some(1_200),
                    bindings: Some(table_leaves()),
                },
            ),
            (
                "min",
                KeyStateResponse {
                    status: KeyStateStatus::InSync,
                    retry_after_ms: None,
                    bindings: None,
                },
            ),
        ]
    }
}

fn commit_req() -> KeyCommitRequest {
    KeyCommitRequest {
        join_id: join_id_2(),
        group_id: group_id(),
        epoch: 17,
        commit: filled(48, 0x50),
        welcome: Some(filled(40, 0x51)),
        group_info: filled(40, 0x52),
    }
}

impl Example for KeyCommitRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", commit_req()),
            (
                "min",
                KeyCommitRequest {
                    welcome: None,
                    ..commit_req()
                },
            ),
        ]
    }
}

impl Example for KeyCommitAccepted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyCommitAccepted {
                accepted: true,
                epoch: 18,
                slot: 18,
            },
        )]
    }
}

impl Example for KeyReadyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyReadyRequest {
                join_id: join_id(),
                group_id: group_id(),
                epoch: 18,
            },
        )]
    }
}

impl Example for KeyInvalidRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyInvalidRequest {
                join_id: join_id(),
                group_id: group_id(),
                epoch: 17,
                reason: InvalidReason::MissedEpoch,
                key_package: filled(24, 0x05),
                gateway_session: Some("gw_5f2b9c0e".into()),
            },
        )]
    }
}

impl Example for KeyResetPackageRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyResetPackageRequest {
                join_id: join_id(),
                new_group_id: filled(16, 0x41),
                key_package: filled(24, 0x06),
            },
        )]
    }
}

impl Example for KeyReleaseRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyReleaseRequest {
                join_id: join_id(),
                group_id: group_id(),
                epoch: 18,
                slot: 18,
                recording_id: ID_RECORDING,
                leaf: 4,
                sender_identity: join_id().lk_identity().to_string(),
                enc: filled(32, 0x70),
                ct: filled(48, 0x71),
            },
        )]
    }
}

pub(super) fn release_full() -> Release {
    Release {
        call_id: call_id(),
        recording_id: ID_RECORDING,
        group_id: group_id(),
        epoch: 18,
        leaf: 4,
        slot: 18,
        sender_identity: join_id().lk_identity(),
        enc: filled(32, 0x70),
        ct: filled(48, 0x71),
    }
}

impl Example for Release {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", release_full())]
    }
}

impl Example for KeyRekeyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            KeyRekeyRequest {
                join_id: join_id(),
                reason: RekeyReason::ConsentWithdrawn,
            },
        )]
    }
}

fn details_full() -> ReportDetails {
    ReportDetails {
        expected_slot: Some(18),
        observed_slot: Some(17),
        since_ms: Some(2_600),
        failures: Some(41),
        attempts: Some(2),
        missing_index: Some(18),
        reason: Some("missing_key".into()),
        sdk: Some("js".into()),
        sdk_version: Some("2.22.3".into()),
    }
}

impl Example for ReportDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", details_full()), ("min", ReportDetails::default())]
    }
}

impl Example for KeyReportRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                KeyReportRequest {
                    join_id: join_id(),
                    group_id: group_id(),
                    epoch: 18,
                    kind: ReportKind::PeerUndecryptable,
                    leaf: Some(7),
                    details: Some(details_full()),
                },
            ),
            (
                "min",
                KeyReportRequest {
                    join_id: join_id(),
                    group_id: group_id(),
                    epoch: 18,
                    kind: ReportKind::CaChanged,
                    leaf: None,
                    details: None,
                },
            ),
        ]
    }
}

impl Example for CallKeysMessage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full_init", CallKeysMessage::Init(init_create())),
            (
                "full_proposals",
                CallKeysMessage::Proposals {
                    call_id: call_id(),
                    group_id: group_id(),
                    epoch: 17,
                    proposals: vec![filled(40, 0x80), filled(40, 0x88)],
                    committer: 0,
                    reason: TransitionReason::Join,
                    bindings: table_refs(),
                },
            ),
            (
                "full_commit_request",
                CallKeysMessage::CommitRequest {
                    call_id: call_id(),
                    group_id: group_id(),
                    epoch: 17,
                    committer: 2,
                    attempt: 2,
                },
            ),
            (
                "full_commit",
                CallKeysMessage::Commit {
                    call_id: call_id(),
                    group_id: group_id(),
                    epoch_from: 17,
                    commit: filled(48, 0x50),
                    slot: 18,
                    bindings: LeafBindingTable {
                        entries: vec![leaf_entry(4, join_id())],
                        ..table_leaves()
                    },
                },
            ),
            (
                "full_welcome",
                CallKeysMessage::Welcome {
                    call_id: call_id(),
                    group_id: group_id(),
                    join_id: join_id(),
                    epoch: 18,
                    welcome: filled(40, 0x51),
                    ratchet_tree: filled(64, 0x33),
                    slot_base: 0,
                    slot: 18,
                    bindings: table_leaves(),
                },
            ),
            (
                "full_activate",
                CallKeysMessage::Activate {
                    call_id: call_id(),
                    group_id: group_id(),
                    epoch: 18,
                    slot: 18,
                },
            ),
            (
                "full_reset",
                CallKeysMessage::Reset {
                    call_id: call_id(),
                    new_group_id: filled(16, 0x41),
                    reason: ResetReason::RemoveDeadline,
                    slot_base: 18,
                    collect_ms: 250,
                },
            ),
            (
                "full_recorder",
                CallKeysMessage::Recorder {
                    call_id: call_id(),
                    recording_id: ID_RECORDING,
                    recorder_cert: Some(filled(56, 0x90)),
                    hpke_pub: Some(filled(32, 0x91)),
                    lk_identity: Some(recorder_identity()),
                    removed: false,
                },
            ),
            (
                "full_recorder_removed",
                CallKeysMessage::Recorder {
                    call_id: call_id(),
                    recording_id: ID_RECORDING,
                    recorder_cert: None,
                    hpke_pub: None,
                    lk_identity: None,
                    removed: true,
                },
            ),
            (
                "full_revoked",
                CallKeysMessage::Revoked {
                    serials: vec![ID_SERIAL, ID_SERIAL_2],
                },
            ),
            (
                "full_removed",
                CallKeysMessage::Removed {
                    call_id: call_id(),
                    group_id: group_id(),
                    join_id: join_id(),
                    reason: RemoveReason::RemovedByModerator,
                },
            ),
            (
                "full_resync",
                CallKeysMessage::Resync {
                    call_id: call_id(),
                    join_id: join_id(),
                    reason: KeyResyncReason::StaleSlot,
                    expected_slot: Some(18),
                },
            ),
            (
                "resync_min",
                CallKeysMessage::Resync {
                    call_id: call_id(),
                    join_id: join_id(),
                    reason: KeyResyncReason::Lagging,
                    expected_slot: None,
                },
            ),
        ]
    }
}

pub(super) fn ready_call_keys_full() -> ReadyCallKeys {
    ReadyCallKeys {
        external_sender: filled(48, 0x20),
        ds_public_key: filled(32, 0xc0),
        ca: ca_key(),
        crl: vec![ID_SERIAL, ID_SERIAL_2],
        crl_partial: true,
        verification_code: VerificationCode::Shown,
        params: params(),
    }
}

impl Example for ReadyCallKeys {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", ready_call_keys_full()),
            (
                "min",
                ReadyCallKeys {
                    crl: Vec::new(),
                    crl_partial: false,
                    ..ready_call_keys_full()
                },
            ),
        ]
    }
}

pub(super) fn status_e2ee_full() -> StatusVoiceE2ee {
    StatusVoiceE2ee {
        calls: 3,
        leaves: 11,
        lagging: 0,
        rebuilding: 0,
        resets_1h: 0,
        removal_overdue_1h: 0,
        roster_reports_1h: 0,
        ca_kid: Some("9a1f33045166708a".into()),
        ds_kid: Some("04be71c2a3d4e5f6".into()),
        devices: 42,
        crl_size: 1,
    }
}

impl Example for StatusVoiceE2ee {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", status_e2ee_full()),
            (
                "min",
                StatusVoiceE2ee {
                    ca_kid: None,
                    ds_kid: None,
                    ..status_e2ee_full()
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- devices

impl Example for DeviceEnrollRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let mut pubkey = vec![0x04];
        pubkey.extend((0..64).map(|i| i as u8));
        vec![(
            "full",
            DeviceEnrollRequest {
                pubkey: B64::new(pubkey),
                platform: DevicePlatform::Web,
                label: "Chrome on macOS".into(),
                proof: filled(64, 0xd0),
            },
        )]
    }
}

impl Example for DeviceCertificateResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DeviceCertificateResponse {
                device_id: ID_DEVICE,
                serial: ID_SERIAL,
                cert: filled(64, 0xe0),
                not_after: t(2_592_000_000),
            },
        )]
    }
}

fn device_summary() -> DeviceSummary {
    DeviceSummary {
        device_id: ID_DEVICE,
        platform: DevicePlatform::Web,
        label: "Chrome on macOS".into(),
        created_at: t(0),
        serial: ID_SERIAL_3,
        not_after: t(2_592_000_000),
        revoked_at: None,
    }
}

impl Example for DeviceSummary {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DeviceSummary {
                    device_id: ID_DEVICE_2,
                    platform: DevicePlatform::Ios,
                    label: "iPhone".into(),
                    revoked_at: Some(t(86_400_000)),
                    ..device_summary()
                },
            ),
            ("min", device_summary()),
        ]
    }
}

impl Example for DevicePage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DevicePage {
                items: vec![device_summary()],
                has_more: false,
            },
        )]
    }
}

impl Example for DeviceRevokeRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DeviceRevokeRequest {
                    reason: Some(DeviceRevokeReason::Lost),
                },
            ),
            ("min", DeviceRevokeRequest::default()),
        ]
    }
}

impl Example for DeviceRevokeResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DeviceRevokeResponse {
                ok: true,
                serials: vec![ID_SERIAL, ID_SERIAL_3],
            },
        )]
    }
}

impl Example for CrlPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            CrlPage {
                items: vec![ID_SERIAL, ID_SERIAL_2],
                has_more: true,
            },
        )]
    }
}

impl Example for CrlQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CrlQuery {
                    after: Some(ID_SERIAL),
                    limit: Some(1_000),
                },
            ),
            ("min", CrlQuery::default()),
        ]
    }
}
