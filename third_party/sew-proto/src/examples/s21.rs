//! Examples for the wire types S21 adds (prd-sew-21 §18).

use super::{ID_BOB, ID_CHANNEL, ID_DM, ID_MESSAGE, ID_REPLY, ID_SPACE, t};
use crate::api::notify::*;
use crate::changes::NotificationReason;
use crate::http::Patch;
use crate::mgmt::{MailTestRequest, MailTestResponse, MailTls, StatusMail};
use crate::registry::Example;

fn settings(keywords: bool) -> NotifySettings {
    NotifySettings {
        default_level: None,
        effective_default_level: NotifyLevel::Mentions,
        thread_replies: true,
        broad_mentions: true,
        mute_lets_mentions: true,
        digest: DigestAvailability {
            interval: DigestInterval::H1,
            available: false,
            reason: Some(digest_reasons::POLICY_OFF.into()),
        },
        keywords: keywords.then(|| vec!["deploy".into(), "on-call".into()]),
        keyword_count: 2,
        rev: 3,
    }
}

fn channel_prefs() -> ChannelNotifyPrefs {
    ChannelNotifyPrefs {
        channel_id: ID_CHANNEL,
        level: Some(NotifyLevel::All),
        effective_level: NotifyLevel::All,
        muted: true,
        muted_until: Some(t(21_600_000)),
    }
}

fn channel_prefs_min() -> ChannelNotifyPrefs {
    ChannelNotifyPrefs {
        channel_id: ID_DM,
        level: None,
        effective_level: NotifyLevel::All,
        muted: false,
        muted_until: None,
    }
}

fn space_prefs() -> SpaceNotifyPrefs {
    SpaceNotifyPrefs {
        space_id: ID_SPACE,
        level: Some(NotifyLevel::None),
        effective_level: NotifyLevel::None,
    }
}

fn notification() -> Notification {
    Notification {
        reason: NotificationReason::Mention,
        message_id: ID_MESSAGE,
        channel_id: ID_CHANNEL,
        space_id: Some(ID_SPACE),
        thread_root_id: None,
        author_id: ID_BOB,
        message_created_at: t(1_400),
        at: t(1_520),
    }
}

/// READY's section (used by `ready_full`).
pub(super) fn ready_notify_full() -> ReadyNotify {
    ReadyNotify {
        settings: settings(false),
        channel_prefs: vec![channel_prefs(), channel_prefs_min()],
        space_prefs: vec![space_prefs()],
    }
}

/// `status.mail` (used by the `Status` full example).
pub(super) fn status_mail_full() -> StatusMail {
    StatusMail {
        configured: true,
        tls: Some(MailTls::Starttls),
        last_ok_at: Some(t(3_600_000)),
        last_error: Some("451 4.3.0 try again later".into()),
        last_error_at: Some(t(1_800_000)),
        sent_24h: 12,
        failed_24h: 1,
    }
}

impl Example for NotifySettings {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", settings(true)),
            (
                "min",
                NotifySettings {
                    default_level: Some(NotifyLevel::None),
                    effective_default_level: NotifyLevel::None,
                    thread_replies: false,
                    broad_mentions: false,
                    mute_lets_mentions: false,
                    digest: DigestAvailability {
                        interval: DigestInterval::M15,
                        available: true,
                        reason: None,
                    },
                    keywords: None,
                    keyword_count: 0,
                    rev: 1,
                },
            ),
        ]
    }
}

impl Example for UpdateNotifySettingsRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateNotifySettingsRequest {
                    default_level: Patch::Set(NotifyLevel::All),
                    thread_replies: Some(false),
                    broad_mentions: Some(false),
                    mute_lets_mentions: Some(false),
                    digest_interval: Some(DigestInterval::D1),
                    keywords: Some(vec!["deploy".into(), "release train".into()]),
                    if_rev: Some(3),
                },
            ),
            (
                "clear_default",
                UpdateNotifySettingsRequest {
                    default_level: Patch::Clear,
                    ..UpdateNotifySettingsRequest::default()
                },
            ),
            ("min", UpdateNotifySettingsRequest::default()),
        ]
    }
}

impl Example for ChannelNotifyPrefs {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", channel_prefs()), ("min", channel_prefs_min())]
    }
}

impl Example for SetChannelNotifyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetChannelNotifyRequest {
                    level: Patch::Set(NotifyLevel::All),
                    mute: Some(MuteInput {
                        muted: true,
                        until: Some(t(21_600_000)),
                    }),
                },
            ),
            (
                "min",
                SetChannelNotifyRequest {
                    level: Patch::Clear,
                    mute: None,
                },
            ),
            (
                "unmute",
                SetChannelNotifyRequest {
                    level: Patch::Keep,
                    mute: Some(MuteInput {
                        muted: false,
                        until: None,
                    }),
                },
            ),
        ]
    }
}

impl Example for SpaceNotifyPrefs {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", space_prefs()),
            (
                "min",
                SpaceNotifyPrefs {
                    space_id: ID_SPACE,
                    level: None,
                    effective_level: NotifyLevel::Mentions,
                },
            ),
        ]
    }
}

impl Example for SetSpaceNotifyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetSpaceNotifyRequest {
                    level: Patch::Set(NotifyLevel::None),
                },
            ),
            (
                "min",
                SetSpaceNotifyRequest {
                    level: Patch::Clear,
                },
            ),
        ]
    }
}

impl Example for NotifyPrefsList {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                NotifyPrefsList {
                    channels: vec![channel_prefs(), channel_prefs_min()],
                    spaces: vec![space_prefs()],
                },
            ),
            ("min", NotifyPrefsList::default()),
        ]
    }
}

impl Example for Notification {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", notification()),
            (
                "min",
                Notification {
                    reason: NotificationReason::Dm,
                    message_id: ID_REPLY,
                    channel_id: ID_DM,
                    space_id: None,
                    thread_root_id: Some(ID_MESSAGE),
                    author_id: ID_BOB,
                    message_created_at: t(2_000),
                    at: t(2_050),
                },
            ),
        ]
    }
}

impl Example for NotificationsReleased {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                NotificationsReleased {
                    items: vec![notification()],
                    more: 4,
                },
            ),
            (
                "min",
                NotificationsReleased {
                    items: Vec::new(),
                    more: 0,
                },
            ),
        ]
    }
}

impl Example for NotificationPrefsUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                // Every field (fixture coverage); the server sets exactly one.
                "full",
                NotificationPrefsUpdated {
                    settings: Some(settings(false)),
                    channel: Some(channel_prefs()),
                    space: Some(space_prefs()),
                },
            ),
            (
                "settings",
                NotificationPrefsUpdated {
                    settings: Some(settings(false)),
                    ..NotificationPrefsUpdated::default()
                },
            ),
            (
                "channel",
                NotificationPrefsUpdated {
                    channel: Some(channel_prefs()),
                    ..NotificationPrefsUpdated::default()
                },
            ),
            (
                "space",
                NotificationPrefsUpdated {
                    space: Some(space_prefs()),
                    ..NotificationPrefsUpdated::default()
                },
            ),
        ]
    }
}

impl Example for ReadyNotify {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", ready_notify_full()),
            (
                "min",
                ReadyNotify {
                    settings: settings(false),
                    channel_prefs: Vec::new(),
                    space_prefs: Vec::new(),
                },
            ),
        ]
    }
}

impl Example for MailTestRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MailTestRequest {
                to: "ops@example.com".into(),
            },
        )]
    }
}

impl Example for MailTestResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MailTestResponse {
                    ok: false,
                    smtp_code: Some(550),
                    error: Some("550 5.1.1 mailbox unavailable".into()),
                },
            ),
            (
                "min",
                MailTestResponse {
                    ok: true,
                    smtp_code: None,
                    error: None,
                },
            ),
        ]
    }
}
