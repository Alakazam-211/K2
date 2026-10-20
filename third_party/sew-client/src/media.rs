//! S16 calls: custom emoji, space icons, GIF search and attach, preview suppress, install
//! secrets, and the signed image fetches (prd-sew-16 §12, §17.12).

use bytes::Bytes;
use http::Method;
use sew_proto::api::media::{
    ClearSecretRequest, CustomEmoji, CustomEmojiPage, GifAttachRequest, GifAttachResponse, GifPage,
    GifPageQuery, GifSearchQuery, SecretStatus, SecretsResponse, SetSecretRequest,
    UpdateEmojiRequest,
};
use sew_proto::api::messages::Message;
use sew_proto::api::spaces::Space;
use sew_proto::http::{EmptyRequest, OkResponse};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use crate::api::decode;
use crate::config::RequestOptions;
use crate::structure::with_query;
use crate::{Client, ClientError, RawResponse};

fn signed_prefix(template: &str) -> &str {
    template.split('{').next().unwrap_or(template)
}

impl Client {
    // ---- custom emoji

    /// `GET /api/v1/spaces/{id}/emoji`: every live custom emoji of the space.
    pub async fn space_emoji(&self, space: Id) -> Result<CustomEmojiPage, ClientError> {
        self.get(&with_id(r::SPACE_EMOJI, space)).await
    }

    /// `GET /api/v1/spaces/{id}/emoji` with `If-None-Match` (the raw response: `200` with an
    /// `ETag`, or `304`).
    pub async fn space_emoji_if_none_match(
        &self,
        space: Id,
        etag: &str,
    ) -> Result<RawResponse, ClientError> {
        self.request(
            "GET",
            &with_id(r::SPACE_EMOJI, space),
            &[("if-none-match", etag)],
            None,
        )
        .await
    }

    /// `POST /api/v1/spaces/{id}/emoji?name=&aliases=` with a raw image body.
    pub async fn create_emoji(
        &self,
        space: Id,
        name: &str,
        aliases: &[&str],
        image: Vec<u8>,
    ) -> Result<CustomEmoji, ClientError> {
        let path = with_query(
            &with_id(r::SPACE_EMOJI, space),
            &[
                ("name", Some(name.to_owned())),
                ("aliases", (!aliases.is_empty()).then(|| aliases.join(","))),
            ],
        );
        let (resp, _) = self
            .execute(
                Method::POST,
                path,
                Some(Bytes::from(image)),
                RequestOptions::default(),
            )
            .await?;
        decode(resp.status, &resp.body)
    }

    /// `GET /api/v1/emoji/{id}`.
    pub async fn emoji(&self, id: Id) -> Result<CustomEmoji, ClientError> {
        self.get(&with_id(r::EMOJI, id)).await
    }

    /// `POST /api/v1/emoji/{id}/edit`.
    pub async fn edit_emoji(
        &self,
        id: Id,
        req: &UpdateEmojiRequest,
    ) -> Result<CustomEmoji, ClientError> {
        self.post(&with_id(r::EMOJI_EDIT, id), req).await
    }

    /// `POST /api/v1/emoji/{id}/delete`.
    pub async fn delete_emoji(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::EMOJI_DELETE, id), &EmptyRequest {})
            .await
    }

    /// `GET` a signed custom emoji image (`/api/v1/emoji-images/…?exp=&sig=`, as
    /// `CustomEmoji.url` carries it); the raw response. The URL is the credential: no Bearer.
    pub async fn fetch_emoji_image(&self, url: &str) -> Result<RawResponse, ClientError> {
        self.fetch_signed(url, signed_prefix(r::EMOJI_IMAGE)).await
    }

    // ---- space icons

    /// `POST /api/v1/spaces/{id}/icon` with a raw image body.
    pub async fn set_space_icon(&self, space: Id, image: Vec<u8>) -> Result<Space, ClientError> {
        let (resp, _) = self
            .execute(
                Method::POST,
                with_id(r::SPACE_ICON, space),
                Some(Bytes::from(image)),
                RequestOptions::default(),
            )
            .await?;
        decode(resp.status, &resp.body)
    }

    /// `POST /api/v1/spaces/{id}/icon/clear`.
    pub async fn clear_space_icon(&self, space: Id) -> Result<Space, ClientError> {
        self.post(&with_id(r::SPACE_ICON_CLEAR, space), &EmptyRequest {})
            .await
    }

    /// `GET` a signed space icon (`Space.icon_url` / `icon_thumb_url`); the raw response.
    pub async fn fetch_space_icon(&self, url: &str) -> Result<RawResponse, ClientError> {
        self.fetch_signed(url, signed_prefix(r::SPACE_ICON_IMAGE))
            .await
    }

    // ---- GIFs

    /// `GET /api/v1/gifs/search`.
    pub async fn gif_search(&self, q: &GifSearchQuery) -> Result<GifPage, ClientError> {
        let path = with_query(
            r::GIFS_SEARCH,
            &[
                ("q", Some(q.q.clone())),
                ("limit", q.limit.map(|v| v.to_string())),
                ("offset", q.offset.map(|v| v.to_string())),
            ],
        );
        self.get(&path).await
    }

    /// `GET /api/v1/gifs/trending`.
    pub async fn gif_trending(&self, q: &GifPageQuery) -> Result<GifPage, ClientError> {
        let path = with_query(
            r::GIFS_TRENDING,
            &[
                ("limit", q.limit.map(|v| v.to_string())),
                ("offset", q.offset.map(|v| v.to_string())),
            ],
        );
        self.get(&path).await
    }

    /// `GET` a signed GIF media URL (`GifItem.preview.url`); the raw response.
    pub async fn fetch_gif_media(&self, url: &str) -> Result<RawResponse, ClientError> {
        self.fetch_signed(url, signed_prefix(r::GIFS_MEDIA)).await
    }

    /// `POST /api/v1/channels/{id}/gifs/attach`: the server fetches the GIF into a pending
    /// upload; post it with `upload_ids`.
    pub async fn gif_attach(
        &self,
        channel: Id,
        req: &GifAttachRequest,
    ) -> Result<GifAttachResponse, ClientError> {
        self.post(&with_id(r::CHANNEL_GIFS_ATTACH, channel), req)
            .await
    }

    // ---- link previews

    /// `POST /api/v1/messages/{id}/previews/suppress`.
    pub async fn suppress_previews(&self, message: Id) -> Result<Message, ClientError> {
        self.post(
            &with_id(r::MESSAGE_PREVIEWS_SUPPRESS, message),
            &EmptyRequest {},
        )
        .await
    }

    /// `GET` a signed preview image (`LinkPreview.image.url`); the raw response.
    pub async fn fetch_preview_image(&self, url: &str) -> Result<RawResponse, ClientError> {
        self.fetch_signed(url, signed_prefix(r::PREVIEW_IMAGE))
            .await
    }

    // ---- install secrets

    /// `GET /api/v1/install/secrets` (install owner or admin; values are never returned).
    pub async fn install_secrets(&self) -> Result<SecretsResponse, ClientError> {
        self.get(r::INSTALL_SECRETS).await
    }

    /// `POST /api/v1/install/secrets/set`.
    pub async fn set_install_secret(
        &self,
        req: &SetSecretRequest,
    ) -> Result<SecretStatus, ClientError> {
        self.post(r::INSTALL_SECRETS_SET, req).await
    }

    /// `POST /api/v1/install/secrets/clear`.
    pub async fn clear_install_secret(
        &self,
        req: &ClearSecretRequest,
    ) -> Result<SecretStatus, ClientError> {
        self.post(r::INSTALL_SECRETS_CLEAR, req).await
    }

    /// A signed `GET` (no Bearer) of a path starting with `prefix`.
    async fn fetch_signed(&self, url: &str, prefix: &str) -> Result<RawResponse, ClientError> {
        if !url.starts_with(prefix) {
            return Err(ClientError::invalid(
                "url",
                format!("a signed URL of this kind starts with {prefix}"),
            ));
        }
        self.request("GET", url, &[("authorization", "")], None)
            .await
    }
}
