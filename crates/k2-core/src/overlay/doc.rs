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
    /// `pending` | `answered` | `voided` | `expired`
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Who answered or dismissed the card, from the caller's credential
    /// (never the body), like a post's `author`. None while pending, for
    /// an agent's own answer, and on docs from before this field.
    #[serde(default, rename = "answeredBy", skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<PostAuthor>,
    /// Unix seconds the card was answered, dismissed or expired.
    #[serde(default, rename = "answeredAt", skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<i64>,
    /// Unix seconds when a still-pending card expires (status `expired`,
    /// set by the daemon's sweep, never a client timer). Choice cards
    /// default to 7 days; a secret card shows its vault request's 7 days.
    /// None: open until answered or voided (`--no-expiry`, older cards).
    #[serde(default, rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecretBody {
    pub name: String,
    /// `pending` | `set` | `voided` | `expired`
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// K2 vault (prd-k2-vault-v1 §10.1, KV9): what the asking agent
    /// proposes. People can change all of it on the card. Never a value.
    /// `workspace` | `server` | `session`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Proposed sites (exact hosts, normalised origins).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// `fetch` | `file` | `env`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// `api_key` | `password` | …
    #[serde(default, rename = "secretKind", skip_serializing_if = "Option::is_none")]
    pub secret_kind: Option<String>,
    /// After a person saved it: the `k2v://` reference (never the value).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Who answered or dismissed the card, from the caller's credential
    /// (never the body), like a post's `author`. None while pending, for
    /// an agent's own answer, and on docs from before this field.
    #[serde(default, rename = "answeredBy", skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<PostAuthor>,
    /// Unix seconds the card was answered, dismissed or expired.
    #[serde(default, rename = "answeredAt", skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<i64>,
    /// Unix seconds when a still-pending card expires (status `expired`,
    /// set by the daemon's sweep, never a client timer). Choice cards
    /// default to 7 days; a secret card shows its vault request's 7 days.
    /// None: open until answered or voided (`--no-expiry`, older cards).
    #[serde(default, rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// The vault-side proposal on a secret card (KV9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretProposal {
    pub scope: Option<String>,
    pub hosts: Vec<String>,
    pub delivery: Option<String>,
    pub kind: Option<String>,
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

/// A Thread post a Design tab sent for the person (prd-browser-design-mode-v1
/// D-A10): the batched page notes from one Send. Still the person's own
/// message (`via: "compose"`), but set only by `POST /cli/browser/notes`,
/// never by a body field on `/cli/thread/post`, and kept out of the compose
/// history. Wire: `design: {itemId?, url, notes}` on the doc.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesignOrigin {
    /// The Design tab's item id, when a tab sent it (`k2 browser note` has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// The page the notes are about (query and hash stripped).
    pub url: String,
    /// How many notes the post carries.
    pub notes: u32,
}

/// Who posted a person's Thread message, stamped by the daemon from the
/// request's credential (never from the request body). A Thread is shared
/// by the owner, Connect logins and app guests, and `from` alone can't say
/// which of them sent a post (an owner display name can equal a Connect
/// username), so a viewer could not tell its own posts from other people's.
///
/// Wire: `author: {kind, login}` on the doc. Optional: agent posts and docs
/// from before this field have none.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostAuthor {
    /// [`AUTHOR_OWNER`] (the owner daemon token) | [`AUTHOR_USER`] (a
    /// Connect login) | [`AUTHOR_GUEST`] (an app pass). A string, so a doc
    /// with a later kind still parses.
    pub kind: String,
    /// `owner` for the owner token, else the Connect username or the app
    /// guest's pass username.
    pub login: String,
    /// The owner's display name, on a card the owner token answered or
    /// dismissed (`answeredBy`), when one is set: the same name an owner's
    /// post carries in `from`, stamped at answer time. None otherwise (and
    /// for every other kind), so viewers fall back to "the owner".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// [`PostAuthor::kind`] for the owner daemon token.
pub const AUTHOR_OWNER: &str = "owner";
/// [`PostAuthor::kind`] for a Connect login.
pub const AUTHOR_USER: &str = "user";
/// [`PostAuthor::kind`] for an app guest (`k2skn_` pass).
pub const AUTHOR_GUEST: &str = "guest";

impl PostAuthor {
    pub fn owner() -> Self {
        Self { kind: AUTHOR_OWNER.to_string(), login: "owner".to_string(), name: None }
    }

    /// The owner, carrying `display_name` (the resolved owner display
    /// name). The bare default `owner` (no name set) carries none.
    pub fn owner_named(display_name: &str) -> Self {
        let n = display_name.trim();
        let name = (!n.is_empty() && !n.eq_ignore_ascii_case("owner")).then(|| n.to_string());
        Self { name, ..Self::owner() }
    }

    pub fn user(username: &str) -> Self {
        Self { kind: AUTHOR_USER.to_string(), login: username.trim().to_string(), name: None }
    }

    pub fn guest(username: &str) -> Self {
        Self { kind: AUTHOR_GUEST.to_string(), login: username.trim().to_string(), name: None }
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
    /// Set when a Design tab (or `k2 browser note`) sent this post (D-A10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<DesignOrigin>,
    /// Set on a person's post (the compose bar, a Garden widget, an app
    /// guest): who sent it, from the credential. None on agent posts and
    /// on docs written before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PostAuthor>,
    /// Set on a line K2 collected from an outside agent product (a local
    /// Grok Bot): where it came from and how to draw it. None on every
    /// other doc and on docs from before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<CollectedMeta>,
}

/// [`CollectedMeta::origin`]: the product's own local chat store (imported).
pub const ORIGIN_STORE: &str = "store";
/// [`CollectedMeta::origin`]: the product's live roster summary (its
/// latest line at one of the daemon's checks).
pub const ORIGIN_ROSTER: &str = "roster";
/// [`CollectedMeta::origin`]: a message K2 sent to the product's agent.
pub const ORIGIN_K2: &str = "k2";
/// [`CollectedMeta::origin`]: a note K2 added (a gap marker).
pub const ORIGIN_SYSTEM: &str = "system";

/// A Thread line K2 collected from an outside agent product (0.46.0: local
/// Grok Bots, address `gb:<name>`). These conversations belong to no
/// workspace: they never wake an agent, never reach a PTY and never
/// federate. Holds chat text only, never a token or a secret value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CollectedMeta {
    /// The product (`grokbot`).
    pub source: String,
    /// [`ORIGIN_STORE`] | [`ORIGIN_ROSTER`] | [`ORIGIN_K2`] | [`ORIGIN_SYSTEM`].
    pub origin: String,
    /// The product's id for the line (dedupe key); `k2-<nonce>` for a K2
    /// send; `gap:<id of the next line>` for a gap marker.
    pub entry_id: String,
    /// `human` | `agent` | `peer` | `system`.
    pub author: String,
    /// When the line was written (ms since epoch), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
    /// Sent from K2.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub via_k2: bool,
    /// The text is the product's one-line preview (may be cut short).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview_only: bool,
    /// The line has no text of its own: `body` is a label for readers
    /// (`k2 thread gb:<name>`), and the chat view draws `detail` instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub body_is_label: bool,
    /// Product-specific drawing detail (peer chips, a read-only card, what
    /// to open in the product). Never a secret value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl OverlayDoc {
    pub fn text(id: String, from: String, to: String, body: String, via: &str) -> Self {
        Self {
            id,
            kind: "text".to_string(),
            from,
            to: Some(to),
            created_at: now_secs(),
            body: Some(crate::redaction::redact(&body)),
            via: Some(via.to_string()),
            inject: None,
            choice: None,
            secret: None,
            widget: None,
            design: None,
            author: None,
            external: None,
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
                answered_by: None,
                answered_at: None,
                expires_at: None,
            }),
            secret: None,
            widget: None,
            design: None,
            author: None,
            external: None,
        }
    }

    pub fn secret_card(
        id: String,
        from: String,
        to: String,
        name: String,
        prompt: Option<String>,
        proposal: SecretProposal,
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
                scope: proposal.scope,
                hosts: proposal.hosts,
                delivery: proposal.delivery,
                secret_kind: proposal.kind,
                reference: None,
                answered_by: None,
                answered_at: None,
                expires_at: None,
            }),
            widget: None,
            design: None,
            author: None,
            external: None,
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
            body: Some(crate::redaction::redact(&body)),
            via: Some(via.to_string()),
            inject: Some(inject.to_string()),
            choice: None,
            secret: None,
            widget: None,
            design: None,
            author: None,
            external: None,
        }
    }

    pub fn is_pending_choice(&self) -> bool {
        self.kind == "choice"
            && self
                .choice
                .as_ref()
                .is_some_and(|c| c.status == "pending")
    }

    /// A pending card whose `expiresAt` has passed (`now` in unix seconds).
    pub fn is_due(&self, now: i64) -> bool {
        let at = if self.is_pending_choice() {
            self.choice.as_ref().and_then(|c| c.expires_at)
        } else if self.is_pending_secret() {
            self.secret.as_ref().and_then(|s| s.expires_at)
        } else {
            None
        };
        at.is_some_and(|t| t <= now)
    }

    pub fn is_pending_secret(&self) -> bool {
        self.kind == "secret"
            && self
                .secret
                .as_ref()
                .is_some_and(|s| s.status == "pending")
    }
}

pub(crate) fn now_secs() -> i64 {
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
    fn post_author_round_trips_and_old_docs_still_parse() {
        let mut d = OverlayDoc::text("i".into(), "alice".into(), "k2/1".into(), "hi".into(), "compose");
        let plain = serde_json::to_string(&d).expect("json");
        assert!(!plain.contains("author"), "absent author is not written: {plain}");
        d.author = Some(PostAuthor::user("  alice "));
        let s = serde_json::to_string(&d).expect("json");
        assert!(s.contains(r#""author":{"kind":"user","login":"alice"}"#), "{s}");
        // Every field the Companion reads keeps its name and value.
        let v: serde_json::Value = serde_json::from_str(&s).expect("value");
        assert_eq!(v["from"], "alice");
        assert_eq!(v["via"], "compose");
        assert_eq!(v["kind"], "text");
        assert_eq!(serde_json::from_str::<OverlayDoc>(&s).expect("parse"), d);
        let old = r#"{"id":"i","kind":"text","from":"alice","to":"k2/1","created_at":1,"body":"hi","via":"compose"}"#;
        assert_eq!(serde_json::from_str::<OverlayDoc>(old).expect("old doc").author, None);
        let later = r#"{"id":"i","kind":"text","from":"x","created_at":1,"author":{"kind":"robot","login":"x"}}"#;
        assert_eq!(
            serde_json::from_str::<OverlayDoc>(later).expect("later kind").author,
            Some(PostAuthor { kind: "robot".into(), login: "x".into(), name: None })
        );
        assert_eq!(PostAuthor::owner(), PostAuthor { kind: "owner".into(), login: "owner".into(), name: None });
        assert_eq!(PostAuthor::guest("g1"), PostAuthor { kind: "guest".into(), login: "g1".into(), name: None });
        // A post's author never carries a name (its `from` already does).
        assert!(!serde_json::to_string(&PostAuthor::owner()).unwrap().contains("name"));
    }

    /// The owner's display name rides an owner `answeredBy`, so viewers can
    /// say "Rosson" instead of "owner"; the unset default carries none.
    #[test]
    fn owner_answered_by_carries_the_display_name() {
        let a = PostAuthor::owner_named("  Rosson Long ");
        assert_eq!((a.kind.as_str(), a.login.as_str(), a.name.as_deref()), ("owner", "owner", Some("Rosson Long")));
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({"kind": "owner", "login": "owner", "name": "Rosson Long"})
        );
        assert_eq!(PostAuthor::owner_named("owner"), PostAuthor::owner(), "the unset default names nobody");
        assert_eq!(PostAuthor::owner_named("  "), PostAuthor::owner());
        let back: PostAuthor = serde_json::from_str(r#"{"kind":"owner","login":"owner","name":"Rosson"}"#).unwrap();
        assert_eq!(back.name.as_deref(), Some("Rosson"));
        let old: PostAuthor = serde_json::from_str(r#"{"kind":"owner","login":"owner"}"#).unwrap();
        assert_eq!(old, PostAuthor::owner(), "older cards still parse");
    }

    #[test]
    fn card_answered_by_round_trips_and_old_cards_still_parse() {
        let mut d = OverlayDoc::choice("c".into(), "k2".into(), "k2/1".into(), "Ship?".into(), vec!["Go".into()], false);
        let plain = serde_json::to_string(&d).expect("json");
        assert!(!plain.contains("answeredBy") && !plain.contains("answeredAt"), "pending card writes neither: {plain}");
        let c = d.choice.as_mut().expect("choice");
        c.status = "answered".into();
        c.answer = Some("Go".into());
        c.answered_by = Some(PostAuthor::guest("g1"));
        c.answered_at = Some(42);
        let s = serde_json::to_string(&d).expect("json");
        assert!(s.contains(r#""answeredBy":{"kind":"guest","login":"g1"}"#), "{s}");
        assert!(s.contains(r#""answeredAt":42"#), "{s}");
        assert_eq!(serde_json::from_str::<OverlayDoc>(&s).expect("parse"), d);
        let old = r#"{"id":"c","kind":"choice","from":"k2","created_at":1,"choice":{"prompt":"?","options":[{"label":"Go"}],"allow_custom":false,"status":"answered","answer":"Go"}}"#;
        let old = serde_json::from_str::<OverlayDoc>(old).expect("old card");
        let oc = old.choice.expect("choice");
        assert_eq!((oc.answered_by, oc.answered_at), (None, None));
        let old_secret = r#"{"id":"s","kind":"secret","from":"k2","created_at":1,"secret":{"name":"T","status":"set"}}"#;
        let os = serde_json::from_str::<OverlayDoc>(old_secret).expect("old secret").secret.expect("secret");
        assert_eq!((os.answered_by, os.answered_at), (None, None));
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
