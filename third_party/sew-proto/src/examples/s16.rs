//! Examples for the wire types S16 adds (prd-sew-16 §16).

use super::{ID_CHANNEL, ID_EMOJI, ID_ME, ID_MESSAGE, ID_SPACE, id, t};
use crate::api::media::*;
use crate::gateway::events::*;
use crate::http::{Page, Patch};
use crate::registry::Example;

const EMOJI_URL: &str = "/api/v1/emoji-images/48831765147549709/full?exp=1791158400&sig=c2ln";
const EMOJI_STILL_URL: &str =
    "/api/v1/emoji-images/48831765147549709/still?exp=1791158400&sig=c2ln";

fn custom_emoji_full() -> CustomEmoji {
    CustomEmoji {
        id: ID_EMOJI,
        space_id: ID_SPACE,
        name: "party".into(),
        aliases: vec!["parrot".into(), "pp".into()],
        animated: true,
        width: 128,
        height: 96,
        url: EMOJI_URL.into(),
        still_url: Some(EMOJI_STILL_URL.into()),
        created_by: Some(ID_ME),
        created_at: t(0),
        updated_at: t(60_000),
    }
}

fn custom_emoji_min() -> CustomEmoji {
    CustomEmoji {
        id: id(19),
        space_id: ID_SPACE,
        name: "shipit".into(),
        aliases: vec![],
        animated: false,
        width: 64,
        height: 64,
        url: "/api/v1/emoji-images/48831765147549715/full?exp=1791158400&sig=c2ln".into(),
        still_url: None,
        created_by: None,
        created_at: t(0),
        updated_at: t(0),
    }
}

impl Example for CustomEmoji {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", custom_emoji_full()), ("min", custom_emoji_min())]
    }
}

impl Example for Page<CustomEmoji> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![custom_emoji_full(), custom_emoji_min()],
                has_more: false,
            },
        )]
    }
}

pub(super) fn emoji_ref_full() -> CustomEmojiRef {
    CustomEmojiRef {
        id: ID_EMOJI,
        space_id: ID_SPACE,
        name: "party".into(),
        animated: true,
        url: Some(EMOJI_URL.into()),
        still_url: Some(EMOJI_STILL_URL.into()),
        available: true,
    }
}

pub(super) fn emoji_ref_deleted() -> CustomEmojiRef {
    CustomEmojiRef {
        id: id(19),
        space_id: ID_SPACE,
        name: "shipit".into(),
        animated: false,
        url: None,
        still_url: None,
        available: false,
    }
}

impl Example for CustomEmojiRef {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", emoji_ref_full()), ("min", emoji_ref_deleted())]
    }
}

impl Example for CreateEmojiQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateEmojiQuery {
                    name: "party".into(),
                    aliases: Some("parrot,pp".into()),
                },
            ),
            (
                "min",
                CreateEmojiQuery {
                    name: "shipit".into(),
                    aliases: None,
                },
            ),
        ]
    }
}

impl Example for UpdateEmojiRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateEmojiRequest {
                    name: Some("fiesta".into()),
                    aliases: Patch::Set(vec!["parrot".into()]),
                },
            ),
            ("min", UpdateEmojiRequest::default()),
        ]
    }
}

pub(super) fn link_preview_full() -> LinkPreview {
    LinkPreview {
        url: "http://example.com/post/1".into(),
        host: "example.com".into(),
        kind: LinkPreviewKind::Page,
        title: Some("A post".into()),
        description: Some("What the post is about.".into()),
        site_name: Some("Example".into()),
        redirected_host: Some("www.example.com".into()),
        image: Some(PreviewImage {
            url: "/api/v1/previews/48831765147549716/image?exp=1790514000&sig=c2ln".into(),
            width: 800,
            height: 418,
        }),
    }
}

pub(super) fn link_preview_min() -> LinkPreview {
    LinkPreview {
        url: "https://example.com/cat.png".into(),
        host: "example.com".into(),
        kind: LinkPreviewKind::Image,
        title: None,
        description: None,
        site_name: None,
        redirected_host: None,
        image: None,
    }
}

impl Example for LinkPreview {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", link_preview_full()), ("min", link_preview_min())]
    }
}

impl Example for GifSearchQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GifSearchQuery {
                    q: "cat".into(),
                    limit: Some(24),
                    offset: Some(24),
                },
            ),
            (
                "min",
                GifSearchQuery {
                    q: "cat".into(),
                    limit: None,
                    offset: None,
                },
            ),
        ]
    }
}

impl Example for GifPageQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GifPageQuery {
                    limit: Some(10),
                    offset: Some(0),
                },
            ),
            ("min", GifPageQuery::default()),
        ]
    }
}

fn gif_item() -> GifItem {
    GifItem {
        id: "xT9IgG50Fb7Mi0prBC".into(),
        title: "Cat typing".into(),
        width: 480,
        height: 270,
        preview: GifMedia {
            url:
                "/api/v1/gifs/media/aHR0cHM6Ly9tZWRpYS5naXBoeS5jb20vYS5naWY?exp=1790514000&sig=c2ln"
                    .into(),
            width: 200,
            height: 113,
        },
    }
}

impl Example for GifPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GifPage {
                    provider: "giphy".into(),
                    items: vec![gif_item()],
                    next_offset: Some(1),
                },
            ),
            (
                "min",
                GifPage {
                    provider: "giphy".into(),
                    items: vec![],
                    next_offset: None,
                },
            ),
        ]
    }
}

impl Example for GifAttachRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            GifAttachRequest {
                id: "xT9IgG50Fb7Mi0prBC".into(),
            },
        )]
    }
}

impl Example for GifAttachResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            GifAttachResponse {
                upload_id: id(20),
                filename: "giphy-xT9IgG50Fb7Mi0prBC.gif".into(),
                content_type: "image/gif".into(),
                size: 1_048_576,
                width: 480,
                height: 270,
                expires_at: t(86_400_000),
            },
        )]
    }
}

impl Example for SetSecretRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SetSecretRequest {
                name: "gifs.giphy".into(),
                value: "EXAMPLE-KEY-not-a-real-key".into(),
            },
        )]
    }
}

impl Example for ClearSecretRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ClearSecretRequest {
                name: "gifs.giphy".into(),
            },
        )]
    }
}

fn secret_full() -> SecretStatus {
    SecretStatus {
        name: "gifs.giphy".into(),
        configured: true,
        fingerprint: Some("5e884898".into()),
        set_at: Some(t(0)),
        set_via: Some(SecretSetVia::Mgmt),
    }
}

fn secret_min() -> SecretStatus {
    SecretStatus {
        name: "gifs.giphy".into(),
        configured: false,
        fingerprint: None,
        set_at: None,
        set_via: None,
    }
}

impl Example for SecretStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", secret_full()), ("min", secret_min())]
    }
}

impl Example for SecretsResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SecretsResponse {
                items: vec![secret_full()],
            },
        )]
    }
}

impl Example for EmojiUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            EmojiUpdated {
                space_id: ID_SPACE,
                version: "e4".into(),
                emoji: custom_emoji_full(),
            },
        )]
    }
}

impl Example for EmojiDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            EmojiDeleted {
                space_id: ID_SPACE,
                version: "e5".into(),
                id: ID_EMOJI,
            },
        )]
    }
}

impl Example for MessagePreviewsUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MessagePreviewsUpdated {
                    message_id: ID_MESSAGE,
                    channel_id: ID_CHANNEL,
                    previews: vec![link_preview_full(), link_preview_min()],
                    previews_suppressed: false,
                    seq: 1_025,
                },
            ),
            (
                "min",
                MessagePreviewsUpdated {
                    message_id: ID_MESSAGE,
                    channel_id: ID_CHANNEL,
                    previews: vec![],
                    previews_suppressed: true,
                    seq: 1_026,
                },
            ),
        ]
    }
}
