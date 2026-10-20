//! Audit events and their canonical JSON (C14, gap G25).
//!
//! Audit rows never hold content: `details` carries ids, names, old and new policy values and
//! permission bits only. S3 chains rows with `hash = SHA-256(prev_hash ‖ canonical_json)`.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::id::Id;
use crate::time::Timestamp;

unit_enum! {
    /// What kind of principal acted.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ActorKind {
        /// A person member.
        Person = "person",
        /// An agent member.
        Agent = "agent",
        /// The mgmt service (K2's daemon or the `sew` CLI).
        Service = "service",
        /// Sew itself (jobs).
        System = "system",
    }
}

unit_enum! {
    /// How an audited action ended.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum AuditResult {
        /// It succeeded.
        Ok = "ok",
        /// A permission or policy check refused it.
        Denied = "denied",
        /// It failed.
        Error = "error",
    }
}

unit_enum! {
    /// An audited action, `<entity>.<verb>` in `lower_snake` (C14). Owning slices add more.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Action {
        /// A session was created.
        SessionCreate = "session.create",
        /// A session was renewed.
        SessionRenew = "session.renew",
        /// A session was revoked.
        SessionRevoke = "session.revoke",
        /// A pass with a valid signature was rejected.
        PassReject = "pass.reject",
        /// A standalone login failed.
        LoginFail = "login.fail",
        /// A principal was provisioned.
        MemberProvision = "member.provision",
        /// A principal's profile was updated.
        MemberUpdate = "member.update",
        /// A principal was deprovisioned.
        MemberDeprovision = "member.deprovision",
        /// The install owner was set.
        OwnerSet = "owner.set",
        /// An assistant was pinned.
        AgentPin = "agent.pin",
        /// An assistant was unpinned.
        AgentUnpin = "agent.unpin",
        /// An act-as call with no domain action of its own.
        ApiCall = "api.call",
        /// A space was created.
        SpaceCreate = "space.create",
        /// A space was updated.
        SpaceUpdate = "space.update",
        /// A category was created.
        CategoryCreate = "category.create",
        /// A category was updated.
        CategoryUpdate = "category.update",
        /// A category was deleted.
        CategoryDelete = "category.delete",
        /// A channel was created.
        ChannelCreate = "channel.create",
        /// A channel was updated.
        ChannelUpdate = "channel.update",
        /// A channel was archived.
        ChannelArchive = "channel.archive",
        /// A channel was unarchived.
        ChannelUnarchive = "channel.unarchive",
        /// A channel's overwrites were re-synced with its category.
        ChannelSync = "channel.sync",
        /// A role was created.
        RoleCreate = "role.create",
        /// A role was updated.
        RoleUpdate = "role.update",
        /// A role was deleted.
        RoleDelete = "role.delete",
        /// A role was assigned.
        RoleAssign = "role.assign",
        /// A role was unassigned.
        RoleUnassign = "role.unassign",
        /// An overwrite was set.
        OverwriteSet = "overwrite.set",
        /// An overwrite was deleted.
        OverwriteDelete = "overwrite.delete",
        /// Org policies were changed.
        PolicySet = "policy.set",
        /// A member was timed out.
        MemberTimeout = "member.timeout",
        /// A timeout was lifted.
        MemberUntimeout = "member.untimeout",
        /// A member was removed from a space.
        SpaceMemberRemove = "space_member.remove",
        /// Slow mode was set on a channel.
        ChannelSlowModeSet = "channel.slow_mode_set",
        /// A moderator deleted a message.
        MessageModeratorDelete = "message.moderator_delete",
        /// A moderator deleted a whole thread (S13 gap G17).
        ThreadDelete = "thread.delete",
        /// A moderator removed another member's reaction (S13 gap G17).
        ReactionModeratorRemove = "reaction.moderator_remove",
        /// A retention policy was set.
        RetentionSet = "retention.set",
        /// A retention preview was requested (S22).
        RetentionPreview = "retention.preview",
        /// A pending retention policy was cancelled or replaced (S22).
        RetentionCancel = "retention.cancel",
        /// A pending retention policy took effect (S22).
        RetentionActivate = "retention.activate",
        /// A retention run was requested by hand (S22).
        RetentionRunRequested = "retention.run_requested",
        /// A retention run finished (S22).
        RetentionRun = "retention.run",
        /// A legal hold was created.
        LegalHoldCreate = "legal_hold.create",
        /// A legal hold was updated.
        LegalHoldUpdate = "legal_hold.update",
        /// A legal hold was released.
        LegalHoldRelease = "legal_hold.release",
        /// An export was started.
        ExportStart = "export.start",
        /// An eDiscovery export was run.
        EdiscoveryExport = "ediscovery.export",
        /// A backup was started.
        BackupStart = "backup.start",
        /// Retired by S23 (an import that fails leaves no install to audit into); never written.
        ImportStart = "import.start",
        /// The master key was rotated.
        KeyRotateMaster = "key.rotate_master",
        /// A month key was shredded.
        KeyShred = "key.shred",
        /// The audit log was read.
        AuditRead = "audit.read",
        /// A compliance read happened.
        ComplianceRead = "compliance.read",
        /// A job exhausted its attempts or failed permanently (S3 gap S3-G8).
        JobDead = "job.dead",
        /// An admin requeued a dead job (S3 gap S3-G8).
        JobRetry = "job.retry",
        /// An admin cancelled a pending job (S3 gap S3-G8).
        JobCancel = "job.cancel",
        /// The audit hash chain failed verification (S3 gap S3-G8).
        AuditVerifyFailed = "audit.verify_failed",
        /// A standalone password was set through a one-time setup link (S8 gap S8-G14).
        PasswordSet = "password.set",
        /// Failed service-token checks on the mgmt socket, coalesced per peer uid per 10 s
        /// (S9 G16).
        MgmtAuthFail = "mgmt.auth_fail",
        /// A non-empty agent deliveries read (S9 G16).
        AgentDeliveriesRead = "agent.deliveries_read",
        /// A one-time password setup link was issued; never the token or URL (S24 G16).
        PasswordSetupLink = "password.setup_link",
        /// A member's install role was set (S10 route, S24 local role route; one
        /// implementation, S9's `provisioning::set_install_role`).
        MemberInstallRoleSet = "member.install_role_set",
        /// A backup bundle was made (S6 G17).
        InstallBackup = "install.backup",
        /// A backup bundle was restored (S6 G17).
        InstallRestore = "install.restore",
        /// A restore drill ran (S6 G17).
        InstallDrill = "install.drill",
        /// The binary was upgraded or rolled back (S6 G17).
        InstallUpgrade = "install.upgrade",
        /// The media engine was installed or reconfigured (S49).
        MediaInstall = "media.install",
        /// Voice was enabled (S49).
        MediaEnable = "media.enable",
        /// Voice was disabled (S49).
        MediaDisable = "media.disable",
        /// The media engine was uninstalled (S49).
        MediaUninstall = "media.uninstall",
        /// Media keys were rotated; kids only, never key bytes (S49).
        MediaKeysRotate = "media.keys_rotate",
        /// The TURN certificate was replaced (S49).
        MediaTurnCert = "media.turn_cert",
        /// The media engine was upgraded or rolled back (S49).
        MediaUpgrade = "media.upgrade",
        /// The recorder and transcriber were installed (S49).
        RecordingInstall = "recording.install",
        /// The recorder or transcriber was upgraded (S49).
        RecordingUpgrade = "recording.upgrade",
        /// The transcriber's model was changed (S49).
        RecordingSetModel = "recording.set_model",
        /// The master key was exported for a box move (S6 G17).
        KeysMasterExported = "keys.master_exported",
        /// The keyring was exported for a box move (S6 G17).
        KeysKeyringExported = "keys.keyring_exported",
        /// A keyring export was imported (S6 G17).
        KeysKeyringImported = "keys.keyring_imported",
        /// A month key was rolled over early (S4 G14, contracts rev 42).
        KeyRollover = "key.rollover",
        /// Keyring entries were re-wrapped under a new master key (S4 G14, rev 42).
        KeyRewrap = "key.rewrap",
        /// The local keyring was rebuilt from its mirror (S4 G14, rev 42).
        KeyRestoreFromMirror = "key.restore_from_mirror",
        /// The keyring mirror went stale (S4 G14, rev 42).
        KeyMirrorStale = "key.mirror_stale",
        /// A key file for a shredded kid was found and refused (S4 G14, rev 42).
        KeyResurrectedRefused = "key.resurrected_refused",
        /// Space ownership moved (S10 gap G25).
        SpaceTransfer = "space.transfer",
        /// Members were added to a space (S10 gap G25).
        SpaceMemberAdd = "space_member.add",
        /// A member was added to a channel (S10 gap G25).
        ChannelMemberAdd = "channel_member.add",
        /// A member was removed from a channel (S10 gap G25).
        ChannelMemberRemove = "channel_member.remove",
        /// A space admin joined a private channel through the policy join (S10 gap G25).
        ChannelAdminJoin = "channel.admin_join",
        /// A space's roles were reordered (S12 gap G9).
        RoleReorder = "role.reorder",
        /// A user group was created (S18).
        UserGroupCreate = "user_group.create",
        /// A user group was changed (S18).
        UserGroupUpdate = "user_group.update",
        /// A user group was deleted (S18).
        UserGroupDelete = "user_group.delete",
        /// Members were added to a user group (S18).
        UserGroupMembersAdd = "user_group.members_add",
        /// Members were removed from a user group (S18).
        UserGroupMembersRemove = "user_group.members_remove",
        /// A custom profile field was created (S18).
        ProfileFieldCreate = "profile_field.create",
        /// A custom profile field was changed (S18).
        ProfileFieldUpdate = "profile_field.update",
        /// A custom profile field was deleted (S18).
        ProfileFieldDelete = "profile_field.delete",
        /// Custom profile fields were reordered (S18).
        ProfileFieldReorder = "profile_field.reorder",
        /// An admin reset another member's avatar or status (S18).
        MemberProfileReset = "member.profile_reset",
        /// K2 pushed an avatar through mgmt (S18).
        MemberAvatarPush = "member.avatar_push",
        /// A moderator removed another member's attachments (S15 G16).
        AttachmentModeratorRemove = "attachment.moderator_remove",
        /// A custom emoji was created (S16 gap G26).
        EmojiCreate = "emoji.create",
        /// A custom emoji was renamed or its aliases changed (S16 gap G26).
        EmojiUpdate = "emoji.update",
        /// A custom emoji was deleted (S16 gap G26).
        EmojiDelete = "emoji.delete",
        /// A space icon was set (S16 gap G26).
        SpaceIconSet = "space.icon_set",
        /// A space icon was cleared (S16 gap G26).
        SpaceIconClear = "space.icon_clear",
        /// A moderator turned off another member's link previews (S16 gap G17).
        MessagePreviewsSuppress = "message.previews_suppress",
        /// An install secret was set; details carry the name and fingerprint only (S16 G11).
        SecretSet = "secret.set",
        /// An install secret was cleared (S16 G11).
        SecretClear = "secret.clear",
        /// A channel bookmark was created (S17 §12).
        BookmarkCreate = "bookmark.create",
        /// A channel bookmark was edited (S17 §12).
        BookmarkUpdate = "bookmark.update",
        /// A channel bookmark was deleted (S17 §12).
        BookmarkDelete = "bookmark.delete",
        /// A channel's bookmarks were reordered (S17 §12).
        BookmarkReorder = "bookmark.reorder",
        /// A moderator closed another member's poll (S17 §11.3).
        PollModeratorClose = "poll.moderator_close",
        /// An agent's scheduled message was sent (S17 §9.3).
        ScheduledMessageSend = "scheduled_message.send",
        /// An admin asked for a search index rebuild over mgmt (S20 §18).
        SearchRebuildRequested = "search.rebuild_requested",
        /// A search index rebuild finished and swapped in (S20 §18; system actor).
        SearchRebuild = "search.rebuild",
        /// S22 compacted the search index (S20 §18).
        SearchCompact = "search.compact",
        /// A member became a guest or a guest became a member (S19 §7.4, §7.5).
        GuestConvert = "guest.convert",
        /// A guest left a space because its last channel there was gone (S19 §7.3).
        GuestSpaceRemove = "guest.space_remove",
        /// `guests.enabled` turned off: every guest session revoked (S19 §7.7).
        GuestSuspendAll = "guest.suspend_all",
        /// `guests.enabled` turned back on (S19 §7.7).
        GuestRestoreAll = "guest.restore_all",
        /// A test mail was sent over mgmt; details `{ok, smtp_code}`, never the address (S21).
        MailTest = "mail.test",
        /// A digest unsubscribe link turned a member's digest off (S21; system actor).
        NotifyDigestUnsubscribe = "notify.digest_unsubscribe",
        /// The compliance flag was granted or revoked (S23 §6.2).
        MemberComplianceSet = "member.compliance_set",
        /// An eDiscovery archive was written (S23 §15).
        EdiscoveryComplete = "ediscovery.complete",
        /// An eDiscovery export failed (S23 §15).
        EdiscoveryFail = "ediscovery.fail",
        /// An eDiscovery export was cancelled (S23 §15).
        EdiscoveryCancel = "ediscovery.cancel",
        /// An eDiscovery archive was downloaded, or a range of it (S23 §15).
        EdiscoveryDownload = "ediscovery.download",
        /// An eDiscovery archive was deleted by a caller (S23 §15).
        EdiscoveryDelete = "ediscovery.delete",
        /// An eDiscovery archive expired (S23 §15).
        EdiscoveryExpire = "ediscovery.expire",
        /// A full export archive was written (S23 §15).
        ExportComplete = "export.complete",
        /// A full export failed (S23 §15).
        ExportFail = "export.fail",
        /// A full export was cancelled (S23 §15).
        ExportCancel = "export.cancel",
        /// A full export archive was downloaded (S23 §15).
        ExportDownload = "export.download",
        /// A full export archive was deleted by a caller (S23 §15).
        ExportDelete = "export.delete",
        /// A full export archive expired (S23 §15).
        ExportExpire = "export.expire",
        /// An import finished (the first row of a moved install's new tail, S23 §15).
        ImportComplete = "import.complete",
        /// A client asked for an audit chain check (S23 §7.2).
        AuditVerifyRequested = "audit.verify_requested",
        /// A member joined voice (S33 v1 §21.1; `details {channel_id, via}`).
        VoiceJoin = "voice.join",
        /// A member left voice (S33 v1 §21.1; `details {channel_id, reason, duration_ms}`).
        VoiceLeave = "voice.leave",
        /// A moderator server-muted or unmuted a member (S33 v1 §21.1).
        VoiceServerMute = "voice.server_mute",
        /// A moderator server-deafened or undeafened a member (S33 v1 §21.1).
        VoiceServerDeafen = "voice.server_deafen",
        /// A moderator moved a member to another voice channel (S33 v1 §21.1).
        VoiceMove = "voice.move",
        /// A moderator disconnected a member from voice (S33 v1 §21.1).
        VoiceDisconnect = "voice.disconnect",
        /// Voice was installed, reconfigured, enabled or disabled from the CLI (S33 v1 §21.1).
        VoiceConfig = "voice.config",
        /// The media token key was rotated from the CLI (S33 v1 §21.1).
        VoiceKeysRotate = "voice.keys_rotate",
        /// The call DS and CA keys were generated (S50 §19.4; `details {ds_kid, ca_kid}`).
        CallE2eeKeysCreated = "call.e2ee_keys_created",
        /// A forced removal from a call's key group completed (S50 §19.4).
        CallE2eeMemberRemoved = "call.e2ee_member_removed",
        /// A call's key group was reset (S50 §19.4).
        CallE2eeGroupReset = "call.e2ee_group_reset",
        /// A call's key group was rebuilt after a restart (S50 §19.4).
        CallE2eeRebuilt = "call.e2ee_rebuilt",
        /// A recorder certificate was issued (S50 §19.4).
        CallE2eeRecorderCertified = "call.e2ee_recorder_certified",
        /// A client reported a roster mismatch (S50 §19.4).
        CallE2eeRosterReport = "call.e2ee_roster_report",
        /// A device was removed and refused for a stuck key slot (S50 §19.4).
        CallE2eeSenderStuck = "call.e2ee_sender_stuck",
        /// A client's bridge reported a misbehaving engine (S50 §19.4).
        CallE2eeEngineAnomaly = "call.e2ee_engine_anomaly",
        /// A device key was enrolled (S50 §10.2; `details {device_id, serial, platform}`).
        DeviceEnroll = "device.enroll",
        /// A device certificate was renewed (S50 §10.3).
        DeviceRenew = "device.renew",
        /// A device was revoked (S50 §10.4; `details {device_id, serials, reason}`).
        DeviceRevoke = "device.revoke",
        /// A DM call started (S34 v2 §25.1; actor = the starter; `details {call_id,
        /// channel_id, rung_count}`).
        CallStart = "call.start",
        /// A DM call ended (S34 v2 §25.1; actor `system`; `details {call_id, channel_id,
        /// duration_ms, joined_count, end_reason}`).
        CallEnd = "call.end",
        /// A recording started (S35 v2 §22.1; `details {recording_id, channel_id,
        /// call_record_id?, auto, audio_keep_days, mixed}`).
        RecordingStart = "recording.start",
        /// A recording stopped (`details {recording_id, duration_ms, stop_reason,
        /// recorded_members}`).
        RecordingStop = "recording.stop",
        /// A consent answer (`details {recording_id, consent, via, surface, notice_version}`).
        RecordingConsent = "recording.consent",
        /// Audio keep days changed (`details {recording_id, old_days, new_days}`).
        RecordingAudioRetention = "recording.audio_retention",
        /// Audio purged (`details {recording_id, segments, bytes, held_segments}`).
        RecordingAudioPurge = "recording.audio_purge",
        /// The recorder rejoined (`details {recording_id, run, reason}`).
        RecordingRecorderRejoin = "recording.recorder_rejoin",
        /// The recorder refused engine misbehaviour (`details {recording_id, kind}`).
        RecordingEngineAnomaly = "recording.engine_anomaly",
        /// The recorder refused a binding table (`details {recording_id, kind}`).
        RecordingBindingAnomaly = "recording.binding_anomaly",
        /// A segment of an ungranted identity was discarded (`details {recording_id, run,
        /// seq}`; never the identity).
        RecordingConsentViolation = "recording.consent_violation",
        /// A transcript became final (`details {recording_id, turns, gaps_ms,
        /// failed_segments, model, backend}`).
        TranscriptFinal = "transcript.final",
        /// A compliance download grant (`details {recording_id, channel_id, download_id, what,
        /// parts, segments, reason_given}`; rev 91).
        RecordingDownload = "recording.download",
        /// A meeting was made (Meet MC1, CC10; `details {meeting_id, channel_id, kind,
        /// starts_at, ends_at, member_attendees, email_attendees}`: counts and ids only, never a
        /// title, name or email).
        MeetingCreate = "meeting.create",
        /// A meeting changed (`details {meeting_id, fields, ics_sequence}`).
        MeetingUpdate = "meeting.update",
        /// A scheduled meeting was cancelled (`details {meeting_id, ics_sequence}`).
        MeetingCancel = "meeting.cancel",
        /// A meeting ended (actor `system` for the end job, the host for End for all (MC3);
        /// `details {meeting_id, end_reason, duration_ms}`).
        MeetingEnd = "meeting.end",
        /// A host locked or unlocked a meeting (Meet MC3; `details {meeting_id, locked}`).
        MeetingLock = "meeting.lock",
        /// A host removed someone from a meeting (Meet MC3; `details {meeting_id, member_id,
        /// kind, in_call, sessions_revoked, devices_revoked}`): ids and counts only, never a
        /// name, an email or an IP.
        MeetingRemove = "meeting.remove",
        /// A meeting link was made (Meet MC2a; at create and in a rotate). No secret.
        MeetingLinkCreate = "meeting_link.create",
        /// A host read a meeting's link (Meet MC2a). No secret.
        MeetingLinkRead = "meeting_link.read",
        /// A host rotated a meeting's link (Meet MC2a). No secret.
        MeetingLinkRotate = "meeting_link.rotate",
        /// A host revoked a meeting's link, or the meeting closed (Meet MC2a). No secret.
        MeetingLinkRevoke = "meeting_link.revoke",
        /// A meeting link was redeemed with its valid secret (Meet MC2a): ids and the outcome,
        /// never a name, a nonce, a secret or an IP.
        MeetingRedeem = "meeting.redeem",
        /// Someone knocked on a meeting's lobby (Meet MC2b; a redeem's new entry, or a
        /// signed-in member with the link): ids only, never a name, a nonce, a secret or an IP.
        LobbyKnock = "lobby.knock",
        /// A host let a lobby entry in (Meet MC2b; `details {meeting_id, entry_id, member_id,
        /// all}`).
        LobbyAdmit = "lobby.admit",
        /// A host turned a lobby entry away (Meet MC2b; `details {meeting_id, entry_id,
        /// member_id, blocked}`).
        LobbyDeny = "lobby.deny",
    }
}

unit_enum! {
    /// Who may read an audit row (prd-sew-23 §7.1). Every [`Action`] has one
    /// ([`Action::visibility`]); a test fails if one is missing.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum AuditVisibility {
        /// Rows of one space: `VIEW_AUDIT_LOG` there, the install owner, compliance, service.
        Space = "space",
        /// Install-level rows: the install owner, compliance, service.
        Install = "install",
        /// Legal holds, retention, eDiscovery, compliance reads, flag changes, audit reads:
        /// compliance-flag holders and the mgmt service only. An owner without the flag cannot
        /// learn that a hold or an investigation exists.
        Sealed = "sealed",
    }
}

impl Action {
    /// The action's read class, independent of any row (prd-sew-23 §7.1). `Space`-class
    /// actions are readable at space level only when the row carries a `space_id`; see
    /// [`Action::row_visibility`].
    pub const fn visibility(self) -> AuditVisibility {
        use Action as A;
        match self {
            // Sealed: holds, retention, eDiscovery, compliance reads, flag changes, audit reads.
            A::LegalHoldCreate
            | A::LegalHoldUpdate
            | A::LegalHoldRelease
            | A::RetentionSet
            | A::RetentionPreview
            | A::RetentionCancel
            | A::RetentionActivate
            | A::RetentionRunRequested
            | A::RetentionRun
            | A::ComplianceRead
            | A::EdiscoveryExport
            | A::EdiscoveryComplete
            | A::EdiscoveryFail
            | A::EdiscoveryCancel
            | A::EdiscoveryDownload
            | A::EdiscoveryDelete
            | A::EdiscoveryExpire
            | A::MemberComplianceSet
            | A::AuditRead
            | A::RecordingDownload => AuditVisibility::Sealed,
            // Install: exports, imports, keys, install ops, policies, members, owner, jobs,
            // audit verification, mgmt, auth.
            A::ExportStart
            | A::ExportComplete
            | A::ExportFail
            | A::ExportCancel
            | A::ExportDownload
            | A::ExportDelete
            | A::ExportExpire
            | A::ImportStart
            | A::ImportComplete
            | A::KeyRotateMaster
            | A::KeyShred
            | A::KeyRollover
            | A::KeyRewrap
            | A::KeyRestoreFromMirror
            | A::KeyMirrorStale
            | A::KeyResurrectedRefused
            | A::KeysMasterExported
            | A::KeysKeyringExported
            | A::KeysKeyringImported
            | A::InstallBackup
            | A::InstallRestore
            | A::InstallDrill
            | A::InstallUpgrade
            | A::MediaInstall
            | A::MediaEnable
            | A::MediaDisable
            | A::MediaUninstall
            | A::MediaKeysRotate
            | A::MediaTurnCert
            | A::MediaUpgrade
            | A::RecordingInstall
            | A::RecordingUpgrade
            | A::RecordingSetModel
            | A::RecordingEngineAnomaly
            | A::RecordingBindingAnomaly
            | A::RecordingConsentViolation
            | A::BackupStart
            | A::PolicySet
            | A::MemberProvision
            | A::MemberUpdate
            | A::MemberDeprovision
            | A::MemberInstallRoleSet
            | A::MemberProfileReset
            | A::MemberAvatarPush
            | A::OwnerSet
            | A::JobDead
            | A::JobRetry
            | A::JobCancel
            | A::AuditVerifyFailed
            | A::AuditVerifyRequested
            | A::MgmtAuthFail
            | A::SessionCreate
            | A::SessionRenew
            | A::SessionRevoke
            | A::PassReject
            | A::LoginFail
            | A::PasswordSet
            | A::PasswordSetupLink
            | A::AgentPin
            | A::AgentUnpin
            | A::AgentDeliveriesRead
            | A::SecretSet
            | A::SecretClear
            | A::ProfileFieldCreate
            | A::ProfileFieldUpdate
            | A::ProfileFieldDelete
            | A::ProfileFieldReorder
            | A::SearchRebuildRequested
            | A::SearchRebuild
            | A::SearchCompact
            | A::GuestConvert
            | A::GuestSuspendAll
            | A::GuestRestoreAll
            | A::MailTest
            | A::NotifyDigestUnsubscribe
            | A::VoiceConfig
            | A::VoiceKeysRotate
            | A::CallE2eeKeysCreated
            | A::CallE2eeMemberRemoved
            | A::CallE2eeGroupReset
            | A::CallE2eeRebuilt
            | A::CallE2eeRecorderCertified
            | A::CallE2eeRosterReport
            | A::CallE2eeSenderStuck
            | A::CallE2eeEngineAnomaly
            | A::DeviceEnroll
            | A::DeviceRenew
            | A::DeviceRevoke => AuditVisibility::Install,
            // Space: everything else (readable at space level when the row names a space).
            A::ApiCall
            | A::GuestSpaceRemove
            | A::SpaceCreate
            | A::SpaceUpdate
            | A::CategoryCreate
            | A::CategoryUpdate
            | A::CategoryDelete
            | A::ChannelCreate
            | A::ChannelUpdate
            | A::ChannelArchive
            | A::ChannelUnarchive
            | A::ChannelSync
            | A::RoleCreate
            | A::RoleUpdate
            | A::RoleDelete
            | A::RoleAssign
            | A::RoleUnassign
            | A::OverwriteSet
            | A::OverwriteDelete
            | A::MemberTimeout
            | A::MemberUntimeout
            | A::SpaceMemberRemove
            | A::ChannelSlowModeSet
            | A::MessageModeratorDelete
            | A::ThreadDelete
            | A::ReactionModeratorRemove
            | A::SpaceTransfer
            | A::SpaceMemberAdd
            | A::ChannelMemberAdd
            | A::ChannelMemberRemove
            | A::ChannelAdminJoin
            | A::RoleReorder
            | A::UserGroupCreate
            | A::UserGroupUpdate
            | A::UserGroupDelete
            | A::UserGroupMembersAdd
            | A::UserGroupMembersRemove
            | A::AttachmentModeratorRemove
            | A::EmojiCreate
            | A::EmojiUpdate
            | A::EmojiDelete
            | A::SpaceIconSet
            | A::SpaceIconClear
            | A::MessagePreviewsSuppress
            | A::BookmarkCreate
            | A::BookmarkUpdate
            | A::BookmarkDelete
            | A::BookmarkReorder
            | A::PollModeratorClose
            | A::ScheduledMessageSend
            | A::VoiceJoin
            | A::VoiceLeave
            | A::VoiceServerMute
            | A::VoiceServerDeafen
            | A::VoiceMove
            | A::VoiceDisconnect
            | A::CallStart
            | A::CallEnd
            | A::RecordingStart
            | A::RecordingStop
            | A::RecordingConsent
            | A::RecordingAudioRetention
            | A::RecordingAudioPurge
            | A::RecordingRecorderRejoin
            | A::TranscriptFinal
            | A::MeetingCreate
            | A::MeetingUpdate
            | A::MeetingCancel
            | A::MeetingEnd
            | A::MeetingLock
            | A::MeetingRemove
            | A::MeetingLinkCreate
            | A::MeetingLinkRead
            | A::MeetingLinkRotate
            | A::MeetingLinkRevoke
            | A::MeetingRedeem
            | A::LobbyKnock
            | A::LobbyAdmit
            | A::LobbyDeny => AuditVisibility::Space,
            // A newer peer's action: only the fullest readers see it.
            A::Unknown => AuditVisibility::Sealed,
        }
    }

    /// The class of a stored row: a `Space`-class action without a `space_id` is `Install`
    /// (prd-sew-23 §7.1: "every action without a `space_id`" is install-level).
    pub const fn row_visibility(self, has_space: bool) -> AuditVisibility {
        match self.visibility() {
            AuditVisibility::Space if !has_space => AuditVisibility::Install,
            v => v,
        }
    }

    /// Every action of `class`.
    pub fn of_class(class: AuditVisibility) -> Vec<Action> {
        Action::ALL
            .iter()
            .copied()
            .filter(|a| a.visibility() == class)
            .collect()
    }
}

/// How an act-as call reached Sew (C9): through K2, for an agent, in a K2 session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActorVia {
    /// The delegating system; `"k2"` in v1.
    pub system: String,
    /// The K2 workspace uuid of the agent.
    pub agent: String,
    /// The K2 session id.
    pub k2_session: String,
}

/// Who performed an audited action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Actor {
    /// The acting member, when a member acted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<Id>,
    /// What kind of principal acted.
    pub kind: ActorKind,
    /// Set on act-as calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<ActorVia>,
    /// For an assistant, the member id of its person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<Id>,
    /// The mgmt caller's `SO_PEERCRED` uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_uid: Option<u32>,
    /// The client IP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
}

/// What an audited action touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditTarget {
    /// The entity kind, `lower_snake` (for example `channel`).
    pub kind: String,
    /// The entity id.
    pub id: Id,
}

/// One row of the append-only audit log (C14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AuditEvent {
    /// The event id.
    pub id: Id,
    /// When it happened.
    pub at: Timestamp,
    /// Who acted.
    pub actor: Actor,
    /// What was done.
    pub action: Action,
    /// What it touched; absent when there is no target (for example a failed login).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AuditTarget>,
    /// The space involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// The channel involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// How it ended.
    pub result: AuditResult,
    /// Ids, names, old and new values, permission bits. Never content.
    #[serde(default)]
    pub details: Map<String, Value>,
}

/// Why canonical JSON could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalError {
    /// A float appeared; canonical JSON holds integers only.
    Float,
    /// Serialization failed.
    Serialize(String),
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CanonicalError::Float => {
                f.write_str("canonical JSON holds integers only; found a float")
            }
            CanonicalError::Serialize(e) => write!(f, "canonical JSON serialization failed: {e}"),
        }
    }
}

impl std::error::Error for CanonicalError {}

impl AuditEvent {
    /// The canonical bytes hashed into the audit chain: object keys sorted by UTF-8 bytes at
    /// every level, no whitespace, serde_json string escaping, integers only.
    pub fn canonical_json(&self) -> Result<Vec<u8>, CanonicalError> {
        let value =
            serde_json::to_value(self).map_err(|e| CanonicalError::Serialize(e.to_string()))?;
        canonical_json(&value)
    }
}

/// Writes any JSON value in the canonical form used by the audit chain.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut out = Vec::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_string(s: &str, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    let quoted = serde_json::to_string(s).map_err(|e| CanonicalError::Serialize(e.to_string()))?;
    out.extend_from_slice(quoted.as_bytes());
    Ok(())
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                out.extend_from_slice(u.to_string().as_bytes());
            } else if let Some(i) = n.as_i64() {
                out.extend_from_slice(i.to_string().as_bytes());
            } else {
                return Err(CanonicalError::Float);
            }
        }
        Value::String(s) => write_string(s, out)?,
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(key, out)?;
                out.push(b':');
                if let Some(v) = map.get(key) {
                    write_canonical(v, out)?;
                }
            }
            out.push(b'}');
        }
    }
    Ok(())
}
