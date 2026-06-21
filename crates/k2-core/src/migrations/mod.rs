//! Historical migration helpers.
//!
//! Each submodule here is a one-shot migration tied to a specific
//! K2SO release. They get scheduled once at daemon boot, write a
//! sentinel file when they finish, and no-op on subsequent boots.
//!
//! Older modules are kept around (rather than deleted post-release)
//! so freshly-upgraded workspaces from a pre-migration version still
//! pick up the fix. Each module's name encodes the release it shipped
//! in (e.g. `unification_0_37_0`).

pub mod unification_0_37_0;
// `legacy_agent_types_v1` — frontmatter rewrite for the pre-0.34 pod
// vocabulary (pod-member → agent-template, pod-leader → manager).
// Moved from `src-tauri/src/lib.rs` to daemon first-boot in 0.39.0
// so K2 Connect / headless daemons pick it up without Tauri.
pub mod legacy_agent_types_v1;
// `auto_pin_existing_agents_0_39_0` — flip pinned=1 for every workspace
// that was in agent mode pre-0.39.0, so users upgrading don't see their
// agents "disappear" from the top of the nav (the auto-promote-to-top
// behavior was retired in 0.39.0). 0.39.1 narrowed the filter to only
// ('agent', 'custom') after the original ship over-pinned managers.
pub mod auto_pin_existing_agents_0_39_0;
// `correct_auto_pin_filter_0_39_1` — corrective migration for users who
// already ran the buggy 0.39.0 version of auto_pin_existing_agents.
// Unpins manager / coordinator / pod workspaces.
pub mod correct_auto_pin_filter_0_39_1;
// `enable_fanout_for_enabled_agents_v1` — **NEUTERED to a no-op**.
// It used to bulk-write the `.k2/.harness-fanout-enabled` marker for
// every existing agent-mode workspace on upgrade. That retroactive
// fleet-wide application of (destructive) fan-out is now forbidden:
// fan-out is applied ONLY by the per-workspace, user-confirmed checkbox.
// There is no auto-apply on new agents either — the former global
// "default for new agents" flag was removed entirely. The module is kept
// (not deleted) so already-run boxes keep a stable sentinel; its `run` now
// applies nothing. See the module docs for the full rationale.
pub mod enable_fanout_for_enabled_agents_v1;
