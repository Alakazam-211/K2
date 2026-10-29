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
    /// 0.46.3: an action card (agents present settings cards). K2 runs the
    /// action when a PERSON taps an option; an agent, a K2 terminal, a
    /// Garden widget or an App guest is refused. None on every other card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<CardAction>,
}

/// What an action card does when a person answers it: the daemon's action
/// `id` (e.g. `zen.widget.keep`) with its `params`. `dedupe` keys "one
/// pending card per thing" (a widget, a setting).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardAction {
    pub id: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dedupe: String,
    /// What the action did, once a person answered: `{message, ...}`.
    /// Clients show `message` under the chosen option ("Kept as yours.
    /// Undo in Settings → Gardens → Local Data."). None while pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Permission cards (prd-agent-settings-cards-v1 rev 3.1 §4.1, §17.2):
    /// the daemon-written card view, `k2_core::permission::card::{PermissionView,
    /// BatchView}` as JSON. Written only by the daemon and projected per
    /// viewer at the serve doors (`cards=2` people only). None on every other
    /// action card (keep-as-mine), so their docs are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<serde_json::Value>,
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
    /// How K2 will send it (prd-secret-card-injection-v1 §1, SC-A10): a
    /// header, a Basic login or a query parameter, and the format. Display
    /// only: the answer is checked against the sealed vault request, never
    /// this. Save echoes it back (`inject`); a client that can't show it
    /// gets `card_inject_unseen` ("Update K2 to answer this card").
    /// None: the card names none (K2's default for the key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<SecretInject>,
    /// The environment variable the agent gets if this is delivered by env
    /// (`hetzner-robot` → `HETZNER_ROBOT`). The card shows it for env.
    #[serde(default, rename = "envName", skip_serializing_if = "Option::is_none")]
    pub env_name: Option<String>,
    /// A secret by this name already exists where the card proposes to keep
    /// it, so Save replaces it (SC-A16; best-effort: the person may pick
    /// another scope, and the answer checks again).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replaces: bool,
}

/// How a secret card says K2 sends the secret: the vault's injection
/// destination and format, as text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecretInject {
    /// `header:<Name>` | `basic` | `query:<param>`
    pub dst: String,
    /// Exactly one `{}` where the value goes (`{}`, `Bearer {}`, `ws123:{}`).
    pub fmt: String,
}

/// The vault-side proposal on a secret card (KV9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretProposal {
    pub scope: Option<String>,
    pub hosts: Vec<String>,
    pub delivery: Option<String>,
    pub kind: Option<String>,
    /// How it is sent (prd-secret-card-injection-v1 §1).
    pub inject: Option<SecretInject>,
    /// The env name an env delivery uses (shown on the card).
    pub env_name: Option<String>,
    /// Save replaces an existing secret of this name (SC-A16).
    pub replaces: bool,
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

    /// Parse a post's `origin` with the placement claim (0.46.1): the
    /// label ([`WidgetOrigin::parse`]) plus `placement` and `hash`, both or
    /// neither. The app sends them on a post to an agent on this computer,
    /// so the daemon can check the placement's approved caps; a post to
    /// another server carries the label only (its Garden's own daemon
    /// checked it). The claim is never stored on the doc.
    pub fn parse_claim(v: &serde_json::Value) -> Result<(Self, Option<WidgetPlacementClaim>), String> {
        let Some(obj) = v.as_object() else {
            return Err("origin must be {widget, garden, placement?, hash?}".into());
        };
        let mut label = obj.clone();
        let placement = label.remove("placement");
        let hash = label.remove("hash");
        let origin = Self::parse(&serde_json::Value::Object(label))?;
        let claim = match (placement, hash) {
            (None, None) => None,
            (Some(p), Some(h)) => {
                let p = p.as_str().map(str::trim).unwrap_or_default().to_string();
                let h = h.as_str().map(str::trim).unwrap_or_default().to_string();
                if p.is_empty() || p.chars().count() > WIDGET_ORIGIN_MAX_CHARS || p.chars().any(char::is_control) {
                    return Err(format!(
                        "origin.placement must be 1-{WIDGET_ORIGIN_MAX_CHARS} characters with no control characters"
                    ));
                }
                if h.is_empty() || h.len() > 128 || !h.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err("origin.hash must be the widget's bundle hash (hex)".into());
                }
                Some(WidgetPlacementClaim { placement: p, hash: h })
            }
            _ => return Err("origin.placement and origin.hash go together".into()),
        };
        Ok((origin, claim))
    }
}

/// The placement a widget's post claims (`origin.placement`, `origin.hash`):
/// which slot of the Garden the widget sits in and the bundle hash its
/// frame runs. Checked by the daemon that holds the Garden
/// (`crate::zen::widget_access::check_placement_cap`); never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetPlacementClaim {
    pub placement: String,
    pub hash: String,
}

/// The Board context chip (thread-context-v1, prd-garden-column-collapse-
/// and-agent-list-v1 §4): one line a Garden widget set
/// (`k2.conversation.setContext`) that rode the person's own compose send,
/// shown to the person as a removable chip before they sent. It is the
/// widget's words, never the person's: it is stored as its own field (the
/// body stays exactly what the person typed) and the agent gets it on K2's
/// own quoted line BEFORE the `[from <user>]` stamp, never inside it.
///
/// Wire in: `context: {text, origin: {widget, garden, placement, hash?}}` on
/// `POST /cli/thread/post` (unbound `via=compose` only). Stored: the text and
/// where it came from; never the hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadContext {
    /// One line, at most [`THREAD_CONTEXT_MAX_CHARS`], plain text.
    pub text: String,
    /// The widget's folder (manifest) name: author-chosen.
    pub widget: String,
    /// The Garden it sits in (a Garden id).
    pub garden: String,
    /// The placement (the widget's id on the page).
    pub placement: String,
}

/// Longest context line, in characters.
pub const THREAD_CONTEXT_MAX_CHARS: usize = 120;

/// Text a context line may never hold (ASCII case-insensitive): the
/// markers of K2's own stamps, so a widget can't forge one.
pub const THREAD_CONTEXT_FORBIDDEN: &[&str] = &["[thread:", "[from", "[context"];

/// Unicode that breaks a line or reorders what the person sees: line and
/// paragraph separators, bidi embeddings, overrides and isolates.
fn context_breaks_line(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Check a context line (the renderer runs the same rule first): trimmed,
/// 1–120 characters, one line, no control or bidi characters, none of
/// [`THREAD_CONTEXT_FORBIDDEN`]. Returns the trimmed text.
pub fn check_context_text(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("context.text is empty".into());
    }
    if t.chars().count() > THREAD_CONTEXT_MAX_CHARS {
        return Err(format!("context.text is one line of at most {THREAD_CONTEXT_MAX_CHARS} characters"));
    }
    if t.chars().any(|c| c.is_control() || context_breaks_line(c)) {
        return Err("context.text is one line of plain text (no line breaks or control characters)".into());
    }
    let lower = t.to_ascii_lowercase();
    if let Some(m) = THREAD_CONTEXT_FORBIDDEN.iter().find(|m| lower.contains(*m)) {
        return Err(format!("context.text may not contain '{m}'"));
    }
    Ok(t.to_string())
}

impl ThreadContext {
    /// Parse a post's `context` (`{text, origin}`): the text checked
    /// ([`check_context_text`]), the origin's label and placement claim as
    /// [`WidgetOrigin::parse_claim`] takes them. `origin.placement` is
    /// required; `origin.hash` rides only to this computer's daemon (a post
    /// to another server carries the label and placement, its Garden's own
    /// daemon checked it first).
    pub fn parse(v: &serde_json::Value) -> Result<(Self, WidgetOrigin, Option<WidgetPlacementClaim>), String> {
        let Some(obj) = v.as_object() else {
            return Err("context must be {text, origin}".into());
        };
        if let Some(k) = obj.keys().find(|k| k.as_str() != "text" && k.as_str() != "origin") {
            return Err(format!("context has no field '{k}' (it is {{text, origin}})"));
        }
        let text = check_context_text(obj.get("text").and_then(|t| t.as_str()).ok_or("context.text is a string")?)?;
        let Some(raw) = obj.get("origin").and_then(|o| o.as_object()) else {
            return Err("context.origin must be {widget, garden, placement, hash?}".into());
        };
        let mut origin = raw.clone();
        let placement = match (origin.get("placement"), origin.contains_key("hash")) {
            (Some(p), true) => p.as_str().map(str::trim).unwrap_or_default().to_string(),
            (Some(p), false) => {
                let p = p.as_str().map(str::trim).unwrap_or_default().to_string();
                origin.remove("placement");
                p
            }
            (None, _) => return Err("context.origin.placement is required".into()),
        };
        if placement.is_empty() || placement.chars().count() > WIDGET_ORIGIN_MAX_CHARS || placement.chars().any(char::is_control) {
            return Err(format!(
                "context.origin.placement must be 1-{WIDGET_ORIGIN_MAX_CHARS} characters with no control characters"
            ));
        }
        let (label, claim) = WidgetOrigin::parse_claim(&serde_json::Value::Object(origin)).map_err(|e| format!("context.{e}"))?;
        let ctx = ThreadContext { text, widget: label.widget.clone(), garden: label.garden.clone(), placement };
        Ok((ctx, label, claim))
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
/// [`PostAuthor::kind`] for a person's post that arrived with a clone: a
/// historical name, never "You" on this server (CL7). `login` is the
/// original login, `name` the label shown. Older viewers treat an unknown
/// kind as someone else's post and show its `from` (the same label).
pub const AUTHOR_IMPORTED: &str = "imported";

impl PostAuthor {
    pub fn owner() -> Self {
        Self { kind: AUTHOR_OWNER.to_string(), login: "owner".to_string(), name: None }
    }

    /// The owner, carrying `display_name` as its stamp (0.46.0: the shared
    /// stamp rule, `person_name::sender_identity` — the linked account's
    /// username). The bare default `owner` (nothing linked) carries none.
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

    /// A historical person from an imported Thread (see [`AUTHOR_IMPORTED`]).
    pub fn imported(login: &str, label: &str) -> Self {
        let label = label.trim();
        Self {
            kind: AUTHOR_IMPORTED.to_string(),
            login: login.trim().to_string(),
            name: (!label.is_empty()).then(|| label.to_string()),
        }
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
    /// thread-context-v1: the Board context line that rode the person's
    /// compose send (shown to them as a chip). Never part of `body`. None
    /// on every other doc and on docs from before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ThreadContext>,
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
    /// Set on a line that arrived with a clone (or `k2 thread import`):
    /// where it came from and the originals of every field the import
    /// rewrote. Read-only history. None on every other doc and on docs
    /// from before this field. Never a secret value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<ImportedMeta>,
    /// Chatter only: the `m_…` id of this live message's `msg_deliveries`
    /// receipt (`k2 msg who <id>`). Just the id: sender, via and actor stay
    /// on the receipt. None on every other doc, on a send with no receipt
    /// and on docs written before this field.
    #[serde(default, rename = "deliveryId", skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    /// Kind `handoff` only (prd-agent-secret-handoff-v1 §10.1, H18): a
    /// secret another agent sent, waiting for a person. Names only, never
    /// a value. Answered through `/cli/vault/incoming/decide` (people
    /// only), never the card answer route, so a passport can't take it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffCardBody>,
    /// File objects riding the message (0.46.1, `thread-attachments-v1`):
    /// workspace-relative pointers with a send-time snapshot, written only
    /// by the daemon after its own checks (`super::attachments`). Never a
    /// copy, never a grant. Empty on every other doc and on older docs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<super::attachments::ThreadAttachment>,
    /// Chatter only (0.46.3): one line for people — the sender's
    /// (`k2 msg --summary`) or derived from the body's first line
    /// (`super::summary`). Redacted like the body. None elsewhere and on
    /// older docs (filled at read time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// `sender` | `derived`.
    #[serde(default, rename = "summarySource", skip_serializing_if = "Option::is_none")]
    pub summary_source: Option<String>,
    /// Kind `changes` only (workbench-v1 B7, 0.46.7): K2's changed-files
    /// card for one agent turn, read from git at the turn's start and end
    /// (never the agent's transcript). `body` carries the plain summary for
    /// renderers that don't draw the card. None on every other doc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<ThreadChanges>,
}

/// One file in a [`ThreadChanges`] card: relative to the agent's worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadChangedFile {
    pub file: String,
    #[serde(default)]
    pub old_file: Option<String>,
    /// `added` | `modified` | `deleted` | `renamed` | `untracked`.
    pub status: String,
    #[serde(default)]
    pub additions: Option<u32>,
    #[serde(default)]
    pub deletions: Option<u32>,
}

/// The catalog's `ThreadChanges` (workbench-v1): what one turn changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadChanges {
    /// At most [`THREAD_CHANGES_MAX_FILES`], by path; `truncated` past it.
    pub files: Vec<ThreadChangedFile>,
    /// Every file the turn changed.
    pub total: u32,
    pub additions: u32,
    pub deletions: u32,
    /// Commits the agent made during the turn.
    pub commits: u32,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    pub truncated: bool,
}

/// The most files one changes card lists.
pub const THREAD_CHANGES_MAX_FILES: usize = 50;

/// The body of a `handoff` card. `body` on the doc carries a plain line
/// for renderers that don't draw the card (H18).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct HandoffCardBody {
    /// `hd_…`.
    pub delivery_id: String,
    /// The name it lands under.
    pub name: String,
    /// The sender: `sales` (this server) or `sales::rosson.k2.dev`.
    pub from: String,
    /// The sender's server, cross server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// `api_key` | `password` | …
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Why it waits (`not_allowed`, `clash`, `person_given`, …).
    #[serde(default)]
    pub reason: String,
    /// The receiving workspace already has a secret of that name.
    #[serde(default)]
    pub clash: bool,
    /// `pending` | `landed` | `declined` | `expired` | `unsent`
    pub status: String,
    /// After it landed: the `k2v://` reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<PostAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// The receiving workspace's id.
    #[serde(default)]
    pub workspace: String,
    /// The sender's `--ttl` (seconds): the copy is kept that long after it
    /// was sent, then destroyed. Absent: no expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<i64>,
}

/// Provenance of an imported Thread line (prd-clone-carries-threads-and-
/// vault-v1 §5.3). Holds the originals of the fields the import rewrote
/// (author, via, from, to), so nothing about the source line is lost.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportedMeta {
    /// The clone (manifest `clone_id`) or export id this line came with.
    pub clone_id: String,
    /// The server it was written on (`rosson.k2.dev`).
    pub source_host: String,
    /// The source workspace's handle (`sales`).
    pub source_handle: String,
    /// The doc id on the source (empty on the divider line).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_doc_id: String,
    /// Unix seconds the line was imported here.
    pub imported_at: i64,
    /// The original `author` (a person's post), before the rewrite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PostAuthor>,
    /// The original `via`, when the import changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// The original `from`, when the import changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The original `to`, when the import changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The original card `answeredBy`, when the import changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<PostAuthor>,
    /// A card that was still open on the source: closed here, with this
    /// note ("carried over: answer on the source").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The "Imported from … on …" line placed above the imported history.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub divider: bool,
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
    /// A settings (action) card: a choice card carrying `choice.action`
    /// (keep-as-mine today; permission cards later). An owner/admin ask: App
    /// guests never see one, neither as a card nor as any fallback text.
    pub fn is_action_card(&self) -> bool {
        self.choice.as_ref().is_some_and(|c| c.action.is_some())
    }

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
            context: None,
            author: None,
            external: None,
            imported: None,
            delivery_id: None,
            handoff: None,
            attachments: Vec::new(),
            summary: None,
            summary_source: None,
            changes: None,
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
                action: None,
            }),
            secret: None,
            widget: None,
            design: None,
            context: None,
            author: None,
            external: None,
            imported: None,
            delivery_id: None,
            handoff: None,
            attachments: Vec::new(),
            summary: None,
            summary_source: None,
            changes: None,
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
                inject: proposal.inject,
                env_name: proposal.env_name,
                replaces: proposal.replaces,
            }),
            widget: None,
            design: None,
            context: None,
            author: None,
            external: None,
            imported: None,
            delivery_id: None,
            handoff: None,
            attachments: Vec::new(),
            summary: None,
            summary_source: None,
            changes: None,
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
            context: None,
            author: None,
            external: None,
            imported: None,
            delivery_id: None,
            handoff: None,
            attachments: Vec::new(),
            summary: None,
            summary_source: None,
            changes: None,
        }
    }

    /// A `handoff` card (H18). `body` is the plain line older renderers show.
    /// K2's changed-files card for one turn (workbench-v1 B7).
    pub fn changes_card(id: String, from: String, to: String, body: String, changes: ThreadChanges) -> Self {
        let mut doc = Self::text(id, from, to, body, "k2");
        doc.kind = "changes".to_string();
        doc.changes = Some(changes);
        doc
    }

    pub fn handoff_card(id: String, from: String, to: String, body: String, card: HandoffCardBody) -> Self {
        Self {
            id,
            kind: "handoff".to_string(),
            from,
            to: Some(to),
            created_at: now_secs(),
            body: Some(crate::redaction::redact(&body)),
            via: Some("card".to_string()),
            inject: None,
            choice: None,
            secret: None,
            widget: None,
            design: None,
            context: None,
            author: None,
            external: None,
            imported: None,
            delivery_id: None,
            handoff: Some(card),
            attachments: Vec::new(),
            summary: None,
            summary_source: None,
            changes: None,
        }
    }

    pub fn is_pending_handoff(&self) -> bool {
        self.kind == "handoff" && self.handoff.as_ref().is_some_and(|h| h.status == "pending")
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

    #[test]
    fn widget_origin_claim_takes_placement_and_hash_together() {
        let (o, c) = WidgetOrigin::parse_claim(&json!({"widget": "board", "garden": "g-1"})).expect("label only");
        assert_eq!(o, WidgetOrigin { widget: "board".into(), garden: "g-1".into() });
        assert_eq!(c, None);
        let (o, c) =
            WidgetOrigin::parse_claim(&json!({"widget": "board", "garden": "g-1", "placement": " b1 ", "hash": "ab12"}))
                .expect("with claim");
        assert_eq!(o, WidgetOrigin { widget: "board".into(), garden: "g-1".into() });
        assert_eq!(c, Some(WidgetPlacementClaim { placement: "b1".into(), hash: "ab12".into() }));
        for bad in [
            json!({"widget": "board", "garden": "g-1", "placement": "b1"}),
            json!({"widget": "board", "garden": "g-1", "hash": "ab12"}),
            json!({"widget": "board", "garden": "g-1", "placement": "", "hash": "ab12"}),
            json!({"widget": "board", "garden": "g-1", "placement": "b1", "hash": "../x"}),
            json!({"widget": "board", "garden": "g-1", "placement": 1, "hash": "ab12"}),
            json!({"widget": "board", "garden": "g-1", "placement": "b1", "hash": "ab12", "x": 1}),
            json!("board"),
        ] {
            assert!(WidgetOrigin::parse_claim(&bad).is_err(), "{bad}");
        }
    }
}
