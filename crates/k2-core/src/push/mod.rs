//! Pluggable push-notification adapter.
//!
//! The daemon wants to *fire notifications* when an agent needs attention,
//! a heartbeat fails, or another long-running workflow crosses a user-visible
//! boundary. The daemon does *not* want to know how those notifications
//! reach the user's phone. That decision is pushed out to a `PushTarget`
//! implementation chosen by user settings.
//!
//! ### Shipped implementations (v1, 0.33.0)
//!
//! - [`NoOp`]   — default. Silently drops events. Users who haven't opted in.
//! - [`Webhook`]  — `POST` the event as JSON to a user-supplied URL.
//!   Power users route to Slack incoming-webhooks, Discord, their own ntfy
//!   server, Apple Shortcuts HTTP, etc.
//! - [`NtfySh`] — convenience wrapper for <https://ntfy.sh>. Same shape as
//!   `Webhook` but with the topic / server baked in and the payload format
//!   matched to ntfy's expected fields.
//!
//! ### Companion C4 implementation (the reserved cloud seam, filled)
//!
//! - [`K2Cloud`] (naming: k2, not k2so) — POSTs the daemon-held device
//!   registry ([`crate::push_devices`]) + a content-free payload to the
//!   relay's push gateway (`POST <gateway>/push/dispatch`), which fans
//!   out to APNs/FCM so push reaches locked iOS/Android screens.
//!   Config = gateway URL + shared gateway token
//!   (`K2_PUSH_GATEWAY_URL`/`K2_PUSH_GATEWAY_TOKEN` env, falling back
//!   to the `pushGatewayUrl`/`pushGatewayToken` app settings). When
//!   unset, [`K2Cloud::from_config`] returns `None` and the impl is
//!   never constructed — dispatch is a startup no-op: **DORMANT**.
//!   Payloads are deliberately dumb (§4.5 invariant): generic title +
//!   attribution line + IDs ONLY — question/message content NEVER
//!   rides APNs/FCM; the app pulls over the E2E tunnel on open.
//!
//! ### Design constraints (ratified in the persistent-agents PRD)
//!
//! - **No cloud dependency in v1.** None of the shipped impls phone home.
//! - **Fire-and-forget semantics.** `send` returns on send/attempt
//!   completion; the daemon does not queue for retry on behalf of the
//!   adapter. Individual impls may internally retry with backoff before
//!   returning an error, but the daemon never stores undelivered events.
//! - **Thread-safe.** `PushTarget: Send + Sync` so a daemon-wide instance
//!   can be wrapped in `Arc` and shared across tokio tasks.

use std::fmt;

/// Events the daemon emits to the user's configured push target.
#[derive(Debug, Clone)]
pub enum PushEvent {
    /// An agent hit a decision point and wants human attention.
    AgentNeedsAttention {
        agent: String,
        summary: String,
        /// Deep link back into the Tauri app / mobile companion, e.g.
        /// `k2so://agents/<id>` or the ngrok tunnel's `/agents/<id>`
        /// path.
        action_url: String,
    },
    /// A scheduled heartbeat didn't complete successfully.
    HeartbeatFailed {
        heartbeat: String,
        reason: String,
    },
    /// Companion C4: an agent filed a ticket (`feedback:created`).
    /// CONTENT-FREE by construction (§4.5 invariant): asking agent name
    /// + ticket id ONLY — never the ask's title/body. Optional
    /// `assignees` narrows push to those usernames' devices (empty =
    /// fan out to everyone registered on this server).
    FeedbackCreated {
        agent_name: String,
        feedback_id: String,
        /// Username snapshots from `feedback_assignees` (may be empty).
        assignees: Vec<String>,
    },
    /// New comment on a ticket (thread activity). CONTENT-FREE; same
    /// assignee targeting as [`Self::FeedbackCreated`].
    FeedbackCommented {
        feedback_id: String,
        assignees: Vec<String>,
    },
    /// Companion C4: a non-owner author posted into a project-group
    /// chat (`project-group:message-created`, author != "owner").
    /// CONTENT-FREE: project name + group id ONLY — never the message
    /// text.
    ProjectMessageCreated {
        project_name: String,
        group_id: String,
    },
}

impl PushEvent {
    /// Short one-line title suitable for a mobile notification header.
    pub fn title(&self) -> String {
        match self {
            Self::AgentNeedsAttention { agent, .. } => {
                format!("{agent} needs your attention")
            }
            Self::HeartbeatFailed { heartbeat, .. } => {
                format!("Heartbeat failed: {heartbeat}")
            }
            // C4 mobile pushes use the generic app title; the body
            // carries the (still content-free) attribution line.
            Self::FeedbackCreated { .. }
            | Self::FeedbackCommented { .. }
            | Self::ProjectMessageCreated { .. } => "K2".to_string(),
        }
    }

    /// Body text for the notification.
    pub fn body(&self) -> String {
        match self {
            Self::AgentNeedsAttention { summary, .. } => summary.clone(),
            Self::HeartbeatFailed { reason, .. } => reason.clone(),
            Self::FeedbackCreated { agent_name, .. } => {
                format!("{agent_name} needs you on a ticket")
            }
            Self::FeedbackCommented { .. } => "New reply on a ticket".to_string(),
            Self::ProjectMessageCreated { project_name, .. } => {
                format!("New message in {project_name}")
            }
        }
    }

    /// Tappable URL if the push surface supports one, else empty.
    pub fn action_url(&self) -> &str {
        match self {
            Self::AgentNeedsAttention { action_url, .. } => action_url,
            Self::HeartbeatFailed { .. }
            | Self::FeedbackCreated { .. }
            | Self::FeedbackCommented { .. }
            | Self::ProjectMessageCreated { .. } => "",
        }
    }

    /// When `Some(non-empty)`, only devices for these usernames receive
    /// the push. `None` / empty → all registered devices on this host.
    pub fn target_usernames(&self) -> Option<&[String]> {
        match self {
            Self::FeedbackCreated { assignees, .. } | Self::FeedbackCommented { assignees, .. }
                if !assignees.is_empty() =>
            {
                Some(assignees.as_slice())
            }
            _ => None,
        }
    }

    /// The mobile deep-link `data` object ([`K2Cloud`] payloads): IDs
    /// + a `kind` discriminator ONLY — tap-to-open navigates on these,
    /// content is pulled in-app (feedback PRD §8.6).
    pub fn data(&self) -> serde_json::Value {
        match self {
            Self::FeedbackCreated { feedback_id, .. }
            | Self::FeedbackCommented { feedback_id, .. } => serde_json::json!({
                "kind": "ticket",
                "feedbackId": feedback_id,
            }),
            Self::ProjectMessageCreated { group_id, .. } => serde_json::json!({
                "kind": "project",
                "groupId": group_id,
            }),
            // Legacy desktop-era events carry no mobile deep link.
            Self::AgentNeedsAttention { .. } | Self::HeartbeatFailed { .. } => {
                serde_json::json!({})
            }
        }
    }
}

/// What happens when a push attempt fails. Adapters return `Err` after
/// exhausting their own retry policy; the daemon does not queue events for
/// later retry. This is fire-and-forget by design — the source of truth is
/// the workspace DB + foreground companion WS, not the push channel.
#[derive(Debug)]
pub enum PushError {
    /// Network / HTTP transport error (DNS, TCP, TLS, timeout).
    Transport(String),
    /// Target returned a non-2xx status.
    Remote { status: u16, body: String },
    /// Adapter rejected the event before sending (e.g. missing config).
    BadConfig(String),
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "push transport error: {msg}"),
            Self::Remote { status, body } => {
                write!(f, "push remote error: HTTP {status}: {body}")
            }
            Self::BadConfig(msg) => write!(f, "push adapter misconfigured: {msg}"),
        }
    }
}

impl std::error::Error for PushError {}

/// Implementations of this trait carry the user's push-target configuration
/// and know how to deliver a [`PushEvent`] to it. The daemon holds one
/// instance per configured target in an `Arc<dyn PushTarget>`.
pub trait PushTarget: Send + Sync {
    /// Deliver (or attempt to deliver) a push event. Fire-and-forget — the
    /// daemon does not persist unsent events.
    fn send(&self, event: &PushEvent) -> Result<(), PushError>;

    /// Human-readable label for logs and the Settings UI. `"NoOp"`,
    /// `"Webhook(https://hooks.slack.com/…)"`, etc.
    fn label(&self) -> String;
}

/// Default adapter. Silently drops every event. Chosen when the user hasn't
/// opted into any push configuration, which is the out-of-the-box state.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOp;

impl PushTarget for NoOp {
    fn send(&self, _event: &PushEvent) -> Result<(), PushError> {
        Ok(())
    }

    fn label(&self) -> String {
        "NoOp".to_string()
    }
}

/// HTTP `POST` adapter: sends the event to a user-configured URL as JSON.
/// Payload shape is `{"title": "...", "body": "...", "action_url": "..."}` —
/// simple enough that users can route it into Slack webhooks, Discord,
/// Apple Shortcuts HTTP, their own ntfy server, etc.
#[derive(Debug, Clone)]
pub struct Webhook {
    pub url: String,
}

impl PushTarget for Webhook {
    fn send(&self, event: &PushEvent) -> Result<(), PushError> {
        let payload = serde_json::json!({
            "title": event.title(),
            "body": event.body(),
            "action_url": event.action_url(),
        });
        let response = reqwest::blocking::Client::new()
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "k2so-daemon")
            .body(payload.to_string())
            .send()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(PushError::Remote {
                status: status.as_u16(),
                body,
            });
        }
        Ok(())
    }

    fn label(&self) -> String {
        format!("Webhook({})", self.url)
    }
}

/// Convenience wrapper for <https://ntfy.sh>. Supports user-specified ntfy
/// servers for fully self-hosted delivery. Payload matches the ntfy HTTP
/// protocol: `POST <server>/<topic>` with title / message / click headers
/// (a plain text body carries the message).
#[derive(Debug, Clone)]
pub struct NtfySh {
    /// `https://ntfy.sh` or the user's self-hosted server URL.
    pub server: String,
    /// Topic under `<server>/<topic>`.
    pub topic: String,
}

impl PushTarget for NtfySh {
    fn send(&self, event: &PushEvent) -> Result<(), PushError> {
        let url = format!("{}/{}", self.server.trim_end_matches('/'), self.topic);
        let mut request = reqwest::blocking::Client::new()
            .post(&url)
            .header("User-Agent", "k2so-daemon")
            .header("Title", event.title());
        let action = event.action_url();
        if !action.is_empty() {
            request = request.header("Click", action);
        }
        let response = request
            .body(event.body())
            .send()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(PushError::Remote {
                status: status.as_u16(),
                body,
            });
        }
        Ok(())
    }

    fn label(&self) -> String {
        format!("NtfySh({}/{})", self.server, self.topic)
    }
}

// ── K2Cloud (companion C4) ──────────────────────────────────────────────

/// The one HTTP call [`K2Cloud`] makes, behind a seam so tests inject
/// a recorder instead of a live socket (no network in tests — the
/// gateway itself is exercised in the k2-connect repo). `Ok` carries
/// the gateway's parsed JSON response (per-token outcomes, incl.
/// `dead:true` for `410 Unregistered` tokens the daemon must prune).
pub trait DispatchSender: Send + Sync {
    fn post(
        &self,
        url: &str,
        gateway_token: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, PushError>;
}

/// Production sender: blocking reqwest POST with the shared gateway
/// token as a bearer credential. Mirrors the [`Webhook`] transport
/// idiom (fire-and-forget; no internal retry — the gateway is on the
/// same relay the daemon already depends on).
#[derive(Debug, Default, Clone, Copy)]
pub struct HttpDispatchSender;

impl DispatchSender for HttpDispatchSender {
    fn post(
        &self,
        url: &str,
        gateway_token: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, PushError> {
        let response = reqwest::blocking::Client::new()
            .post(url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {gateway_token}"))
            .header("User-Agent", "k2-daemon")
            .body(body.to_string())
            .send()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(PushError::Remote {
                status: status.as_u16(),
                body: text,
            });
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }
}

/// Companion C4 mobile push adapter — the module's reserved cloud
/// seam, filled (naming: k2, not k2so). `send` reads the daemon-held
/// device registry ([`crate::push_devices`]) and POSTs
/// `{devices:[{platform,token}], payload:{title, body, data}}` to
/// `<gateway>/push/dispatch`; the stateless relay gateway fans out to
/// APNs/FCM. Tokens the response marks `dead:true` are pruned from
/// the registry (aggressive dead-token hygiene, §4.5).
///
/// Constructed ONLY via [`K2Cloud::from_config`] in production: no
/// gateway URL + token configured → no instance → dispatch is a
/// no-op (DORMANT — the shipped-but-inactive state).
pub struct K2Cloud {
    gateway_url: String,
    gateway_token: String,
    sender: std::sync::Arc<dyn DispatchSender>,
}

/// The exact env/settings keys [`K2Cloud::from_config`] reads.
pub const ENV_GATEWAY_URL: &str = "K2_PUSH_GATEWAY_URL";
pub const ENV_GATEWAY_TOKEN: &str = "K2_PUSH_GATEWAY_TOKEN";

impl K2Cloud {
    /// Normalize a gateway base URL so `send` can always append
    /// `/push/dispatch` once.
    ///
    /// Accepts either form that ships in the wild:
    /// - base: `https://push.k2.dev` (docs / `from_parts` tests)
    /// - full dispatch: `https://push.k2.dev/push/dispatch` (some
    ///   app/settings writers store the full path)
    ///
    /// Without this strip, a full-path setting becomes
    /// `…/push/dispatch/push/dispatch` → gateway 404, so feedback +
    /// project-chat pushes silently fail while a hand-crafted POST to
    /// the correct URL still works.
    pub fn normalize_gateway_base(url: &str) -> String {
        let mut u = url.trim().trim_end_matches('/').to_string();
        // Case-insensitive path suffix (env/settings may vary casing).
        let lower = u.to_ascii_lowercase();
        if let Some(stripped) = lower.strip_suffix("/push/dispatch") {
            let keep = stripped.len();
            u.truncate(keep);
            u = u.trim_end_matches('/').to_string();
        }
        u
    }

    /// Build from resolved config parts. `None` unless BOTH the
    /// gateway URL and the gateway token are present and non-blank —
    /// the dormancy gate, kept pure so it's directly testable.
    pub fn from_parts(url: Option<String>, token: Option<String>) -> Option<Self> {
        let url = url.map(|s| Self::normalize_gateway_base(&s))?;
        let token = token.map(|s| s.trim().to_string())?;
        if url.is_empty() || token.is_empty() {
            return None;
        }
        Some(Self {
            gateway_url: url,
            gateway_token: token,
            sender: std::sync::Arc::new(HttpDispatchSender),
        })
    }

    /// Resolve config and construct, or `None` = DORMANT. Env wins
    /// over settings (the `K2_*`-override house pattern):
    /// [`ENV_GATEWAY_URL`]/[`ENV_GATEWAY_TOKEN`], falling back to the
    /// `pushGatewayUrl`/`pushGatewayToken` app settings.
    pub fn from_config() -> Option<Self> {
        let settings = crate::app_settings::load();
        let url = std::env::var(ENV_GATEWAY_URL)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(settings.push_gateway_url);
        let token = std::env::var(ENV_GATEWAY_TOKEN)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(settings.push_gateway_token);
        Self::from_parts(url, token)
    }

    /// Test seam: same adapter, injected transport.
    pub fn with_sender(
        gateway_url: &str,
        gateway_token: &str,
        sender: std::sync::Arc<dyn DispatchSender>,
    ) -> Self {
        Self {
            gateway_url: Self::normalize_gateway_base(gateway_url),
            gateway_token: gateway_token.to_string(),
            sender,
        }
    }

    /// The dispatch body: every registered device (V1 fans out to ALL
    /// — per-user subscriptions later) + the content-free payload.
    fn dispatch_body(
        devices: &[crate::push_devices::PushDevice],
        event: &PushEvent,
    ) -> serde_json::Value {
        serde_json::json!({
            "devices": devices
                .iter()
                .map(|d| serde_json::json!({ "platform": d.platform, "token": d.token }))
                .collect::<Vec<_>>(),
            "payload": {
                "title": event.title(),
                "body": event.body(),
                "data": event.data(),
            },
        })
    }
}

impl PushTarget for K2Cloud {
    fn send(&self, event: &PushEvent) -> Result<(), PushError> {
        let devices = match event.target_usernames() {
            Some(users) => crate::push_devices::list_devices_for_usernames(users)
                .map_err(PushError::BadConfig)?,
            None => crate::push_devices::list_devices().map_err(PushError::BadConfig)?,
        };
        if devices.is_empty() {
            // Nobody registered (or no assignee has a device) — no network call.
            return Ok(());
        }
        let url = format!("{}/push/dispatch", self.gateway_url);
        let body = Self::dispatch_body(&devices, event);
        let response = self.sender.post(&url, &self.gateway_token, &body)?;
        // Dead-token feedback: the gateway reports per-token outcomes;
        // `dead:true` (APNs 410 Unregistered / FCM NotRegistered)
        // tokens are pruned so they never ride another dispatch. A
        // malformed/missing results array is simply no prunes — the
        // push itself already succeeded.
        if let Some(results) = response.get("results").and_then(|r| r.as_array()) {
            for entry in results {
                let dead = entry.get("dead").and_then(|d| d.as_bool()).unwrap_or(false);
                let token = entry.get("token").and_then(|t| t.as_str());
                if let (true, Some(token)) = (dead, token) {
                    if let Err(e) = crate::push_devices::prune_device_by_token(token) {
                        crate::log_debug!("[push] dead-token prune failed: {e}");
                    }
                }
            }
        }
        Ok(())
    }

    fn label(&self) -> String {
        format!("K2Cloud({})", self.gateway_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> PushEvent {
        PushEvent::AgentNeedsAttention {
            agent: "cortana".to_string(),
            summary: "Weekly financial report drafted — ready for review".to_string(),
            action_url: "k2so://agents/cortana".to_string(),
        }
    }

    #[test]
    fn noop_swallows_events() {
        let target = NoOp;
        assert!(target.send(&sample_event()).is_ok());
        assert_eq!(target.label(), "NoOp");
    }

    #[test]
    fn webhook_is_labeled_with_url() {
        let target = Webhook {
            url: "https://example.com/hook".to_string(),
        };
        assert_eq!(target.label(), "Webhook(https://example.com/hook)");
    }

    #[test]
    fn ntfy_is_labeled_with_server_and_topic() {
        let target = NtfySh {
            server: "https://ntfy.sh".to_string(),
            topic: "k2so-rosson".to_string(),
        };
        assert_eq!(target.label(), "NtfySh(https://ntfy.sh/k2so-rosson)");
    }

    #[test]
    fn push_event_renders_title_and_body() {
        let e = sample_event();
        assert!(e.title().contains("cortana"));
        assert!(e.body().contains("financial"));
        assert_eq!(e.action_url(), "k2so://agents/cortana");
    }

    #[test]
    fn push_event_heartbeat_has_no_action_url() {
        let e = PushEvent::HeartbeatFailed {
            heartbeat: "daily-brief".to_string(),
            reason: "wakeup.md missing".to_string(),
        };
        assert_eq!(e.action_url(), "");
        assert!(e.title().contains("daily-brief"));
    }

    #[test]
    fn push_target_object_safe() {
        // Adapters must be usable via `Arc<dyn PushTarget>` so the daemon
        // can hold exactly one and share it across tokio tasks.
        let target: std::sync::Arc<dyn PushTarget> = std::sync::Arc::new(NoOp);
        let _ = target.send(&sample_event());
        assert_eq!(target.label(), "NoOp");
    }

    // ── Companion C4: event shaping + K2Cloud ─────────────────────────

    use std::sync::{Arc, Mutex};

    #[test]
    fn c4_events_render_generic_title_and_deep_link_data() {
        let e = PushEvent::FeedbackCreated {
            agent_name: "cortana".to_string(),
            feedback_id: "fb-123".to_string(),
            assignees: vec![],
        };
        assert_eq!(e.title(), "K2");
        assert_eq!(e.body(), "cortana needs you on a ticket");
        assert_eq!(e.action_url(), "");
        assert_eq!(
            e.data(),
            serde_json::json!({ "kind": "ticket", "feedbackId": "fb-123" })
        );

        let e = PushEvent::ProjectMessageCreated {
            project_name: "Apollo".to_string(),
            group_id: "pg-9".to_string(),
        };
        assert_eq!(e.title(), "K2");
        assert_eq!(e.body(), "New message in Apollo");
        assert_eq!(e.action_url(), "");
        assert_eq!(
            e.data(),
            serde_json::json!({ "kind": "project", "groupId": "pg-9" })
        );

        // Legacy desktop-era events carry no mobile deep link.
        assert_eq!(sample_event().data(), serde_json::json!({}));
    }

    /// Recording sender: captures every call, answers a canned
    /// response. No network anywhere in these tests.
    struct MockSender {
        calls: Mutex<Vec<(String, String, serde_json::Value)>>,
        response: serde_json::Value,
    }

    impl MockSender {
        fn new(response: serde_json::Value) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                response,
            })
        }

        fn calls(&self) -> Vec<(String, String, serde_json::Value)> {
            self.calls.lock().expect("mock lock").clone()
        }
    }

    impl DispatchSender for MockSender {
        fn post(
            &self,
            url: &str,
            gateway_token: &str,
            body: &serde_json::Value,
        ) -> Result<serde_json::Value, PushError> {
            self.calls.lock().expect("mock lock").push((
                url.to_string(),
                gateway_token.to_string(),
                body.clone(),
            ));
            Ok(self.response.clone())
        }
    }

    fn unique_token(label: &str) -> String {
        format!("tok-push-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn normalize_gateway_base_accepts_base_or_full_dispatch_url() {
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev"),
            "https://push.k2.dev"
        );
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev/"),
            "https://push.k2.dev"
        );
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev/push/dispatch"),
            "https://push.k2.dev"
        );
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev/push/dispatch/"),
            "https://push.k2.dev"
        );
        // from_parts with full-path setting still builds a clean base.
        let c = K2Cloud::from_parts(
            Some("https://push.k2.dev/push/dispatch".into()),
            Some("tok".into()),
        )
        .expect("configured");
        assert_eq!(c.label(), "K2Cloud(https://push.k2.dev)");
    }

    #[test]
    fn k2cloud_dispatch_payload_is_content_free() {
        let token = unique_token("shape");
        let device_id = format!("dev-push-shape-{}", uuid::Uuid::new_v4());
        crate::push_devices::upsert_device(&device_id, "apns", &token, "owner")
            .expect("register device");

        let sender = MockSender::new(serde_json::json!({ "results": [] }));
        // Full-path setting form (matches live ~/.k2/settings.json) must
        // still POST exactly once to …/push/dispatch, not …/dispatch/push/dispatch.
        let cloud = K2Cloud::with_sender(
            "https://push.k2.dev/push/dispatch",
            "gw-secret",
            sender.clone(),
        );
        assert_eq!(cloud.label(), "K2Cloud(https://push.k2.dev)");

        cloud
            .send(&PushEvent::FeedbackCreated {
                agent_name: "cortana".to_string(),
                feedback_id: "fb-77".to_string(),
                assignees: vec![],
            })
            .expect("send");

        let calls = sender.calls();
        assert_eq!(calls.len(), 1, "exactly one dispatch POST");
        let (url, gw_token, body) = &calls[0];
        assert_eq!(url, "https://push.k2.dev/push/dispatch");
        assert_eq!(gw_token, "gw-secret");

        // Wire shape: {devices:[{platform,token}], payload:{title,body,data}}
        // and NOTHING else — the §4.5 content-free invariant is
        // structural: no field exists that could carry ask/message text.
        let top: Vec<_> = body.as_object().expect("object").keys().collect();
        assert_eq!(top, vec!["devices", "payload"], "no extra top-level fields");
        let payload = body["payload"].as_object().expect("payload object");
        assert_eq!(
            payload.keys().collect::<Vec<_>>(),
            vec!["body", "data", "title"],
            "payload carries exactly title/body/data"
        );
        assert_eq!(body["payload"]["title"], "K2");
        assert_eq!(body["payload"]["body"], "cortana needs you on a ticket");
        assert_eq!(
            body["payload"]["data"],
            serde_json::json!({ "kind": "feedback", "feedbackId": "fb-77" })
        );
        let devices = body["devices"].as_array().expect("devices array");
        let mine = devices
            .iter()
            .find(|d| d["token"] == token.as_str())
            .expect("registered device rides the dispatch");
        assert_eq!(mine["platform"], "apns");
        assert_eq!(
            mine.as_object().expect("device object").keys().collect::<Vec<_>>(),
            vec!["platform", "token"],
            "devices carry platform+token only — usernames stay home"
        );

        crate::push_devices::remove_device(&device_id).expect("cleanup");
    }

    #[test]
    fn k2cloud_prunes_tokens_the_gateway_marks_dead() {
        let dead_token = unique_token("dead");
        let live_token = unique_token("live");
        let dead_id = format!("dev-push-dead-{}", uuid::Uuid::new_v4());
        let live_id = format!("dev-push-live-{}", uuid::Uuid::new_v4());
        crate::push_devices::upsert_device(&dead_id, "apns", &dead_token, "owner")
            .expect("register dead");
        crate::push_devices::upsert_device(&live_id, "fcm", &live_token, "owner")
            .expect("register live");

        let sender = MockSender::new(serde_json::json!({
            "results": [
                { "token": dead_token, "dead": true },
                { "token": live_token, "dead": false },
            ]
        }));
        let cloud = K2Cloud::with_sender("https://push.k2.dev", "gw", sender);
        cloud
            .send(&PushEvent::ProjectMessageCreated {
                project_name: "Apollo".to_string(),
                group_id: "pg-1".to_string(),
            })
            .expect("send");

        let ids: Vec<_> = crate::push_devices::list_devices()
            .expect("list")
            .into_iter()
            .map(|d| d.device_id)
            .collect();
        assert!(!ids.contains(&dead_id), "dead-marked token pruned");
        assert!(ids.contains(&live_id), "live token survives");
        crate::push_devices::remove_device(&live_id).expect("cleanup");
    }

    #[test]
    fn k2cloud_from_parts_is_dormant_unless_fully_configured() {
        // The dormancy gate: BOTH url and token, non-blank, or nothing.
        assert!(K2Cloud::from_parts(None, None).is_none());
        assert!(K2Cloud::from_parts(Some("https://push.k2.dev".into()), None).is_none());
        assert!(K2Cloud::from_parts(None, Some("gw".into())).is_none());
        assert!(K2Cloud::from_parts(Some("  ".into()), Some("gw".into())).is_none());
        assert!(K2Cloud::from_parts(Some("https://push.k2.dev".into()), Some(" ".into()))
            .is_none());
        let cloud = K2Cloud::from_parts(
            Some("https://push.k2.dev/".into()),
            Some("gw".into()),
        )
        .expect("fully configured constructs");
        assert_eq!(cloud.label(), "K2Cloud(https://push.k2.dev)");
    }
}
