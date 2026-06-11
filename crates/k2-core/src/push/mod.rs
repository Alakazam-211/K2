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
//! ### Future implementation (v2, paid tier)
//!
//! - `K2soCloud` — talks to Alakazam-Labs-hosted APNs/FCM sender so push
//!   reaches locked iOS/Android screens. Not in 0.33.0. Designed into this
//!   trait now so the v2 impl drops in as a new enum variant + `impl
//!   PushTarget for K2soCloud` without touching callers.
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
        }
    }

    /// Body text for the notification.
    pub fn body(&self) -> &str {
        match self {
            Self::AgentNeedsAttention { summary, .. } => summary,
            Self::HeartbeatFailed { reason, .. } => reason,
        }
    }

    /// Tappable URL if the push surface supports one, else empty.
    pub fn action_url(&self) -> &str {
        match self {
            Self::AgentNeedsAttention { action_url, .. } => action_url,
            Self::HeartbeatFailed { .. } => "",
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
            .body(event.body().to_string())
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
}
