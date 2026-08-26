//! Overlay document JSON (redb `docs/{id}` heap). Collections store ids only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChoiceOption {
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChoiceBody {
    pub prompt: String,
    pub options: Vec<ChoiceOption>,
    pub allow_custom: bool,
    /// `pending` | `answered` | `voided`
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretBody {
    pub name: String,
    /// `pending` | `set` | `voided`
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// One overlay document body. Secret **value** never lives here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverlayDoc {
    pub id: String,
    /// `text` | `choice` | `secret` | `chatter`
    pub kind: String,
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `thread` | `compose` | `card` | `msg` | `talk` | `inbox` | `v1`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<ChoiceBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretBody>,
}

impl OverlayDoc {
    pub fn text(id: String, from: String, to: String, body: String, via: &str) -> Self {
        Self {
            id,
            kind: "text".to_string(),
            from,
            to: Some(to),
            created_at: now_secs(),
            body: Some(body),
            via: Some(via.to_string()),
            inject: None,
            choice: None,
            secret: None,
        }
    }

    pub fn choice(
        id: String,
        from: String,
        to: String,
        prompt: String,
        options: Vec<String>,
        allow_custom: bool,
    ) -> Self {
        Self {
            id,
            kind: "choice".to_string(),
            from,
            to: Some(to),
            created_at: now_secs(),
            body: Some(prompt.clone()),
            via: Some("card".to_string()),
            inject: None,
            choice: Some(ChoiceBody {
                prompt,
                options: options
                    .into_iter()
                    .map(|label| ChoiceOption { label })
                    .collect(),
                allow_custom,
                status: "pending".to_string(),
                answer: None,
            }),
            secret: None,
        }
    }

    pub fn secret_card(
        id: String,
        from: String,
        to: String,
        name: String,
        prompt: Option<String>,
    ) -> Self {
        Self {
            id,
            kind: "secret".to_string(),
            from,
            to: Some(to),
            created_at: now_secs(),
            body: prompt.clone(),
            via: Some("card".to_string()),
            inject: None,
            choice: None,
            secret: Some(SecretBody {
                name,
                status: "pending".to_string(),
                prompt,
            }),
        }
    }

    pub fn chatter(
        id: String,
        from: String,
        to: String,
        body: String,
        via: &str,
        inject: &str,
    ) -> Self {
        Self {
            id,
            kind: "chatter".to_string(),
            from,
            to: Some(to),
            created_at: now_secs(),
            body: Some(body),
            via: Some(via.to_string()),
            inject: Some(inject.to_string()),
            choice: None,
            secret: None,
        }
    }

    pub fn is_pending_choice(&self) -> bool {
        self.kind == "choice"
            && self
                .choice
                .as_ref()
                .is_some_and(|c| c.status == "pending")
    }

    pub fn is_pending_secret(&self) -> bool {
        self.kind == "secret"
            && self
                .secret
                .as_ref()
                .is_some_and(|s| s.status == "pending")
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A collection pointer plus the resolved body (GET snapshot item).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverlayItem {
    pub collection: String,
    pub seq: i64,
    pub id: String,
    pub doc: OverlayDoc,
    /// Named conversation this pointer belongs to (`chatterlog` is empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// One collection index write (WS frame source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayLink {
    pub collection: &'static str,
    pub conversation_id: Option<String>,
    pub seq: i64,
    pub id: String,
}
