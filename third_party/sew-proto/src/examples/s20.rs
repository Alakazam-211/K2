//! Examples for the wire types S20 adds (prd-sew-20 §20).

use super::{
    ID_ATTACHMENT, ID_BOB, ID_CHANNEL, ID_DM, ID_JOB, ID_ME, ID_MESSAGE, ID_SPACE, message_full,
    message_min, t,
};
use crate::api::channels::ChannelType;
use crate::api::search::*;
use crate::mgmt::{SearchRebuildRequest, SearchRebuildResponse};
use crate::registry::Example;

const CURSOR: &str = "eyJ2IjoxLCJyIjoibWVzc2FnZXMifQ.q1ZxW0cT3ZQ5aHr0a1b2cw";

impl Example for SearchRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SearchRequest {
                    q: "deploy -staging from:@alice in:acme/#deploys after:2026-09-01".into(),
                    sort: Some(SearchSort::Recent),
                    limit: Some(50),
                    cursor: Some(CURSOR.into()),
                    tz: Some("America/New_York".into()),
                    include_unjoined_public: true,
                    include_archived: false,
                },
            ),
            ("min", SearchRequest::new("deploy")),
        ]
    }
}

fn channel_ref() -> SearchChannelRef {
    SearchChannelRef {
        id: ID_CHANNEL,
        space_id: Some(ID_SPACE),
        kind: ChannelType::Text,
        slug: Some("deploys".into()),
        name: Some("deploys".into()),
        archived: false,
        joined: true,
        dm_member_ids: Vec::new(),
    }
}

fn dm_ref() -> SearchChannelRef {
    SearchChannelRef {
        id: ID_DM,
        space_id: None,
        kind: ChannelType::Dm,
        slug: None,
        name: None,
        archived: false,
        joined: true,
        dm_member_ids: vec![ID_ME, ID_BOB],
    }
}

impl Example for SearchMessagesResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SearchMessagesResponse {
                    items: vec![SearchMessageHit {
                        message: message_full(),
                        score: Some(7.412),
                        highlights: Highlights {
                            body: vec![[0, 6]],
                            forwarded_body: vec![[0, 9]],
                            name: Vec::new(),
                        },
                    }],
                    channels: vec![channel_ref(), dm_ref()],
                    sort: SearchSort::Relevance,
                    as_of: t(0),
                    approx_total: 57,
                    has_more: true,
                    cursor: Some(CURSOR.into()),
                },
            ),
            (
                "min",
                SearchMessagesResponse {
                    items: Vec::new(),
                    channels: Vec::new(),
                    sort: SearchSort::Recent,
                    as_of: t(0),
                    approx_total: 0,
                    has_more: false,
                    cursor: None,
                },
            ),
        ]
    }
}

impl Example for SearchFilesResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SearchFilesResponse {
                    items: vec![SearchFileHit {
                        file: SearchFileRef {
                            attachment_id: ID_ATTACHMENT,
                            message_id: ID_MESSAGE,
                            channel_id: ID_CHANNEL,
                            name: "Q3-Report.PDF".into(),
                            ext: Some("pdf".into()),
                            mime: "application/pdf".into(),
                            size: 482_133,
                        },
                        message: message_min(),
                        score: Some(3.25),
                        highlights: Highlights {
                            body: Vec::new(),
                            forwarded_body: Vec::new(),
                            name: vec![[3, 9]],
                        },
                    }],
                    channels: vec![channel_ref()],
                    sort: SearchSort::Relevance,
                    as_of: t(0),
                    approx_total: 1,
                    has_more: false,
                    cursor: Some(CURSOR.into()),
                },
            ),
            (
                "min",
                SearchFilesResponse {
                    items: Vec::new(),
                    channels: Vec::new(),
                    sort: SearchSort::Recent,
                    as_of: t(0),
                    approx_total: 0,
                    has_more: false,
                    cursor: None,
                },
            ),
        ]
    }
}

impl Example for SearchRebuildRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SearchRebuildRequest {
                    reason: Some("stem language changed".into()),
                },
            ),
            ("min", SearchRebuildRequest { reason: None }),
        ]
    }
}

impl Example for SearchRebuildResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", SearchRebuildResponse { job_id: ID_JOB })]
    }
}
