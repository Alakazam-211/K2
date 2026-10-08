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

/// A Thread post a Garden widget sent for the person (prd-zen-user-widgets-v2
/// UWB12a). The post is still the person's own message (`via: "compose"`);
/// this records which widget sent it, so the Thread can say
/// "You · via <widget>" and the daemon can keep it out of the compose
/// history. Wire: `origin: {widget, garden}` on `POST /cli/thread/post`;
/// stored on the doc as `widget`. Feature key `thread-widget-origin`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WidgetOrigin {
    /// The widget's name as the person sees it (its manifest `name`).
    pub widget: String,
    /// The Garden it sits in (a Garden id).
    pub garden: String,
}

/// Longest `widget` / `garden` text a [`WidgetOrigin`] keeps.
pub const WIDGET_ORIGIN_MAX_CHARS: usize = 80;

impl WidgetOrigin {
    /// Parse and check the `origin` object of a post body: both fields
    /// present, trimmed, 1–80 characters, no control characters.
    pub fn parse(v: &serde_json::Value) -> Result<Self, String> {
        let o: WidgetOrigin = serde_json::from_value(v.clone())
            .map_err(|e| format!("origin must be {{widget, garden}}: {e}"))?;
        let check = |field: &str, s: &str| -> Result<String, String> {
            let t = s.trim();
            if t.is_empty() || t.chars().count() > WIDGET_ORIGIN_MAX_CHARS || t.chars().any(char::is_control) {
                return Err(format!("origin.{field} must be 1-{WIDGET_ORIGIN_MAX_CHARS} characters with no control characters"));
            }
            Ok(t.to_string())
        };
        Ok(WidgetOrigin { widget: check("widget", &o.widget)?, garden: check("garden", &o.garden)? })
    }
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
    /// Set when a Garden widget sent this post for the person (UWB12a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WidgetOrigin>,
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
            widget: None,
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
            widget: None,
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
            widget: None,
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
            widget: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn widget_origin_round_trips_and_old_docs_still_parse() {
        let mut d = OverlayDoc::text("i".into(), "alice".into(), "k2/1".into(), "hi".into(), "compose");
        assert!(!serde_json::to_string(&d).expect("json").contains("widget"), "absent field is not written");
        d.widget = Some(WidgetOrigin { widget: "Agent Arcade".into(), garden: "g-test0001".into() });
        let s = serde_json::to_string(&d).expect("json");
        assert!(s.contains(r#""widget":{"widget":"Agent Arcade","garden":"g-test0001"}"#), "{s}");
        assert_eq!(serde_json::from_str::<OverlayDoc>(&s).expect("parse"), d);
        let old = r#"{"id":"i","kind":"text","from":"alice","to":"k2/1","created_at":1,"body":"hi","via":"compose"}"#;
        assert_eq!(serde_json::from_str::<OverlayDoc>(old).expect("old doc").widget, None);
    }

    #[test]
    fn widget_origin_parse_checks_shape_and_length() {
        let ok = WidgetOrigin::parse(&json!({"widget": "  Agent Arcade ", "garden": "g-test0001"})).expect("ok");
        assert_eq!(ok, WidgetOrigin { widget: "Agent Arcade".into(), garden: "g-test0001".into() });
        for bad in [
            json!({"widget": "A"}),
            json!({"garden": "g"}),
            json!({"widget": "", "garden": "g"}),
            json!({"widget": "A", "garden": "   "}),
            json!({"widget": "A\u{0007}", "garden": "g"}),
            json!({"widget": "x".repeat(81), "garden": "g"}),
            json!({"widget": "A", "garden": "g", "extra": 1}),
            json!({"widget": 3, "garden": "g"}),
            json!("Agent Arcade"),
        ] {
            assert!(WidgetOrigin::parse(&bad).is_err(), "{bad}");
        }
        assert!(WidgetOrigin::parse(&json!({"widget": "x".repeat(80), "garden": "g"})).is_ok());
    }
}
