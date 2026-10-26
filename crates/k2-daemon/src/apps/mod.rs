//! The App runtime's live parts (`prd-k2-app-runtime-v1` stage 1): the
//! `/cli/app/*` routes, the process engine, the supervisor (deploy
//! pipeline, watch window, auto-rollback, crash restart), the daemon-local
//! route override with drain, and agent alerts. The pure and SQLite parts
//! are `k2_core::deploy`.

pub mod access;
// Switch 2's App grants for agents and their one-time cards (0.46.4).
pub mod agent_grants;
// K2 presents those held requests as Thread cards (0.46.4).
pub mod held_cards;
/// Automatic App box prep through the owner's root helper (0.46.3).
pub mod auto_prepare;
/// `k2 app data put`: a workspace's files into an App's data folder.
pub mod data_put;
/// `k2 app data ls|get|rm|mv`: read back and tidy an App's data folder
/// (0.46.4).
pub mod data_files;
/// Box usage, measuring: folder walks, the engine's `disk-usage`,
/// Postgres, mail, memory, the scan cache (0.46.3, disk guard A1).
pub mod disk_scan;
// 0.46.4 (disk guard Slice B): `k2 app resources clean`.
pub mod disk_clean;
/// The disk watch, notices and the daily Seed sweep (0.46.3, A4/A5).
pub mod disk_watch;
// 0.46.4 (disk guard Slice B): CPU and memory trends (migration 0217).
pub mod trends;
/// `k2 app label take|return`: a legacy service's label to its App.
pub mod labels;
/// `k2 app domain add|rm`: any subdomain of the App's workspace (0.46.4).
pub mod domains;
/// `k2 app move-plan`: every step of a move, one owner approval (0.46.4).
pub mod move_plan;
/// Test guests of one App, made by a person (0.46.4).
pub mod test_guests;
/// Legacy `--cmd` services, labelled (0.46.3).
pub mod legacy;
/// Admission-lite: the box's App memory pool (`check_memory`, §4.3).
pub mod admission;
pub mod alerts;
pub mod app_key_routes;
pub mod approvals;
pub mod audit;
pub mod attribution;
pub mod box_prep;
/// Undo box setup: the owner's `k2 app box unprep` (0.46.3 F8).
pub mod box_unprep;
pub mod egress_proxy;
pub mod egress_routes;
pub mod box_service_routes;
/// The box view and clash naming (agents talk to each other, §4.1–4.2).
pub mod coordination;
/// Pre-approved App cores, the Use layer (slice B, §3).
pub mod cores;
pub mod engine_install;
/// The App engine as the supervisor sees it: adopt before any start, the
/// protocol check, the engine-change kick (0.46.4, update continuity S1).
pub mod engine_gate;
/// The boot heal: the engine that matches this K2, through the helper (R4).
pub mod engine_heal;
/// The matching engine staged by the updater and the installer (R1, R3).
pub mod engine_stage;
/// Re-host every App on a K2 update (update continuity S2).
pub mod rehost;
/// Loud failures: ERROR lines and owner notices (R34, R44).
pub mod runtime_notes;
pub mod exception;
pub mod exec;
pub mod failures;
pub mod gateway;
pub mod grace;
pub mod grants;
pub mod origins;
pub mod guarded;
pub mod guest_rooms;
/// Switch-1 default limits (§4.3).
pub mod limits;
pub mod open;
/// The delete guard, hand over / park / unpark, the orphan sweep (Q8).
pub mod ownership;
pub mod podman_install;
pub mod process_engine;
pub mod process_exception;
/// Project chat as an App surface (0.46.3, App only).
pub mod project_chat;
pub mod quotas;
/// Box usage and the disk guard (prd-app-resources-disk-guard-v1, 0.46.3).
pub mod resources;
/// `requires.sew` / `requires.voice` against this box's Sew (0.46.3).
pub mod requires;
/// Per-App Postgres majors: the App's database and `requires.postgres`
/// (prd-app-pg-choice-dualrun-v1 Part A, 0.46.4).
pub mod pg_choice;
pub mod router;
pub mod routes;
/// The per-App seal key in the vault (K-M3, 0.46.3).
pub mod seal;
pub mod supervisor;
pub mod surfaces;
pub mod suspend;
pub mod wall_build;

/// The 0.46.1 door's route tests (lane 2: S19, S20, S42, AR10).
#[cfg(test)]
mod door_tests;

/// Per-agent App permissions (prd-agent-app-permissions-v1, 0.46.3).
#[cfg(test)]
mod access_tests;

/// Room-scope R9/R16 surface tests (0.46.1).
#[cfg(test)]
mod surfaces_tests;

/// Project chat App surface tests (0.46.3).
#[cfg(test)]
mod project_chat_tests;

/// Slices D, F, G of prd-agent-app-permissions-v1 (0.46.3).
#[cfg(test)]
mod ownership_tests;

/// Slice B of prd-agent-app-permissions-v1: the App cores (0.46.3).
#[cfg(test)]
mod cores_tests;

/// Box usage and the disk guard (prd-app-resources-disk-guard-v1, 0.46.3).
#[cfg(test)]
mod disk_guard_tests;

/// `k2 app data ls|get|rm|mv` through the routes (0.46.4).
#[cfg(test)]
mod data_files_tests;
