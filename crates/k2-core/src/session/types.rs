//! Session — the container type for a daemon-owned event stream.
//!
//! Phase 1 scope: plain-old-data (POD). `broadcast::Sender<Frame>` +
//! replay ring from the PRD's `Session` struct land in Phase 2 when
//! a consumer actually subscribes. See
//! `.k2so/prds/session-stream-and-awareness-bus.md` §"Session persistence
//! model" for the three-layer live / replay-ring / archive split.
//!
//! The `cwd` field binds every session to an explicit worktree path —
//! claw-code's `workspace_root` pattern — so parallel daemons sharing
//! session state can't write to the wrong directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Opaque identifier for a session. UUID v4 under the hood; callers
/// should not assume any internal structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Generate a fresh random SessionId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse from a UUID string. Returns `None` on malformed input
    /// — callers log / 400 / ignore as appropriate. Keeps the uuid
    /// crate out of downstream callers' dep graphs.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s).ok().map(Self)
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Which harness the session is hosting. Set once at session
/// creation; never changes. Per-harness recognizers and stream-json
/// adapters dispatch on this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    /// Claude Code (any variant — TS, claw-code Rust port, etc.).
    ClaudeCode,
    /// OpenAI Codex CLI.
    Codex,
    /// Google Gemini CLI.
    Gemini,
    /// Aider.
    Aider,
    /// Pi (pi-Mono).
    Pi,
    /// Goose.
    Goose,
    /// Harness K2SO hasn't characterized. T0 only, no recognizer.
    Other,
}

/// Container type for a session. Phase 1 POD: id + harness + cwd.
/// Phase 2 adds the broadcast channel + replay ring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub harness: HarnessKind,
    pub cwd: PathBuf,
}

impl Session {
    /// Fresh session with a random `SessionId`.
    pub fn new(harness: HarnessKind, cwd: PathBuf) -> Self {
        Self {
            id: SessionId::new(),
            harness,
            cwd,
        }
    }
}
