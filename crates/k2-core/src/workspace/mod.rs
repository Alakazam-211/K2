//! Workspace — lifecycle, state, identity, and launch helpers.
//!
//! Phase 2.5c relocation home. Before Phase 2.5c, most of these
//! responsibilities lived under `agents/` (e.g. `agents/scheduler.rs`,
//! `agents/checkin.rs`, `agents/wake.rs`), but post-Phase-2.1 the
//! "workspace == agent" 1:1 invariant means these are workspace-scoped
//! concerns, not agent-scoped. The folder name now matches what the
//! code does.
//!
//! See `.k2so/prds/phase-2.5c-core-rename.md` for the relocation
//! catalogue and rationale.

// Phase 2.5e: per-agent CLI channel ops (status / done / reserve /
// release). Relocated from `agents/channel.rs`; renamed to
// `agent_channel` to disambiguate from `workspace::events` (the
// "channel event queue" — different concept).
pub mod agent_channel;
pub mod agent_identity;
pub mod agent_launch;
pub mod checkin;
pub mod display;
pub mod events;
pub mod launch_profile;
// Phase 2.5e: workspace lifecycle DB ops (register / create / open /
// cleanup / remove_db_only). Relocated from `agents/workspaces.rs`;
// renamed to `lifecycle` to disambiguate from the `workspace` module
// itself.
pub mod lifecycle;
// Phase 2.5d: boot-time idempotent migration helpers, extracted from
// the monolithic `agents/workspace.rs`.
pub mod migrations;
pub mod dot_dir_migration;
pub mod onboarding;
// Phase 2.5e: `claude --resume` / `--session-id` arg resolver for the
// workspace's canonical chat session. Relocated from
// `agents/resume_chat.rs` — operates on workspace_sessions, naturally
// workspace-scoped.
pub mod resume_chat;
// Phase 2.5e: review queue — workspace-manager approval path for
// agent worktrees (review_queue / review_approve / review_reject /
// review_request_changes / agent_complete). Relocated from
// `agents/reviews.rs`.
pub mod reviews;
pub mod scheduler;
// Phase 2.5d: SKILL.md regen + scaffolding (write_workspace_skill_file*
// + regenerate_workspace_skill cluster). Extracted from
// `agents/workspace.rs`. Renamed in 0.39.0 from `skill_writer` →
// `skill_regen` to disambiguate from the lower-level per-harness
// fanout writer at `crate::skills::writer`.
pub mod skill_regen;
// Phase 2.5d: workspace-root harness file-discovery cluster
// (symlink/scaffold helpers + preview + ingest + disable). Extracted
// from `agents/workspace.rs`.
pub mod harness;
// Canonical-agents feature: per-harness state detection + the
// deterministic safety net (backup + atomic-write + manifest, copies not
// symlinks). The ONLY destructive surface for the K2 Canonical Agent +
// role skills. See `.k2so/prds/k2-canonical-agents.md` §5–§6.
pub mod canonical;
// Phase 2.5d: workspace teardown (disconnect) — freeze or restore the
// symlinks K2SO scaffolded. Extracted from `agents/workspace.rs`.
pub mod teardown;
// Phase 2.5d: Agent CRUD commands. Extracted from `agents/commands.rs`.
pub mod agent;
// Phase 2.5d: AIFileEditor surface for editing AGENT.md. Extracted from
// `agents/commands.rs`.
pub mod agent_editor;
// Phase 2.5d: workspace_sessions + workspace_relations DB accessors.
// Extracted from `agents/commands.rs`.
pub mod relations;
pub mod session;
pub mod settings;
pub mod terminal_id;
pub mod triage;
pub mod wake_prompts;
pub mod work_item;

