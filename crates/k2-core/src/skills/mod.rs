//! Skills — the unified home for documentation profiles (Phase 2.5b).
//!
//! Before Phase 2.5b the workspace had three folders that all held
//! the same concept (a markdown profile describing a role/persona):
//!
//! - `.k2so/agents/<name>/SKILL.md` — instantiated skill (workspace
//!   customizations a sub-agent referenced).
//! - `.k2so/agent-templates/<role>/AGENT.md` — master template
//!   (seed for new skills).
//! - `.k2so/skills/<name>.md` — bare-md Unit-6 skill layers.
//!
//! Per the Phase 2.1 A19 skill reframe ("a skill is a documentation
//! profile your harness loads when you want to apply that role to
//! specific work"), all three are the same thing wearing different
//! costumes. Phase 2.5b consolidates them under a single home:
//!
//! ```text
//! .k2so/skills/<name>/SKILL.md
//! ```
//!
//! Migration runs at daemon boot via
//! [`consolidation::consolidate_skills_v1`], gated on a marker file
//! so repeat boots are no-ops. Collision rule when the same name
//! exists in multiple sources:
//!
//! 1. **Instance wins** (`.k2so/agents/<name>/`) — the workspace
//!    customized this skill, the customization trumps the template.
//! 2. **Template** (`.k2so/agent-templates/<name>/`) — applies a
//!    `-template01`, `-template02`, … suffix on collision.
//! 3. **Bare layer** (`.k2so/skills/<name>.md`) — normalized to
//!    folder shape first so it can act as a collision target.
//!
//! Side-effects of the migration:
//! - `AGENT.md` files in legacy instance dirs get renamed to
//!   `SKILL.md` so every consolidated entry has the same shape.
//! - Per-skill heartbeats (`.k2so/agents/<name>/heartbeats/<sched>/`)
//!   move to the workspace-level `.k2so/heartbeats/` with the skill
//!   name prefixed (`<skill>-<sched>`).
//! - The two source roots (`agents/` + `agent-templates/`) are sent
//!   to the OS recycle bin via [`crate::safe_delete::trash`] —
//!   recoverable for ~30 days if a user needs the originals back.

pub mod consolidation;
pub mod content;
pub mod crud;
pub mod version;
pub mod writer;

pub use crud::SkillSummary;
