//! The App runtime's live parts (`prd-k2-app-runtime-v1` stage 1): the
//! `/cli/app/*` routes, the process engine, the supervisor (deploy
//! pipeline, watch window, auto-rollback, crash restart), the daemon-local
//! route override with drain, and agent alerts. The pure and SQLite parts
//! are `k2_core::deploy`.

/// Admission-lite: the box's App memory pool (`check_memory`, §4.3).
pub mod admission;
pub mod alerts;
pub mod app_key_routes;
pub mod approvals;
pub mod audit;
pub mod attribution;
pub mod box_prep;
pub mod egress_proxy;
pub mod egress_routes;
pub mod box_service_routes;
/// The box view and clash naming (agents talk to each other, §4.1–4.2).
pub mod coordination;
pub mod engine_install;
pub mod exception;
pub mod exec;
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
pub mod quotas;
pub mod router;
pub mod routes;
pub mod supervisor;
pub mod surfaces;
pub mod suspend;
pub mod wall_build;

/// The 0.46.1 door's route tests (lane 2: S19, S20, S42, AR10).
#[cfg(test)]
mod door_tests;

/// Room-scope R9/R16 surface tests (0.46.1).
#[cfg(test)]
mod surfaces_tests;

/// Slices D, F, G of prd-agent-app-permissions-v1 (0.46.3).
#[cfg(test)]
mod ownership_tests;
