//! The App runtime's live parts (`prd-k2-app-runtime-v1` stage 1): the
//! `/cli/app/*` routes, the process engine, the supervisor (deploy
//! pipeline, watch window, auto-rollback, crash restart), the daemon-local
//! route override with drain, and agent alerts. The pure and SQLite parts
//! are `k2_core::deploy`.

pub mod alerts;
pub mod app_key_routes;
pub mod approvals;
pub mod audit;
pub mod attribution;
pub mod box_prep;
pub mod egress_proxy;
pub mod egress_routes;
pub mod box_service_routes;
pub mod engine_install;
pub mod exception;
pub mod exec;
pub mod gateway;
pub mod grace;
pub mod grants;
pub mod guarded;
pub mod guest_rooms;
pub mod podman_install;
pub mod process_engine;
pub mod process_exception;
pub mod quotas;
pub mod router;
pub mod routes;
pub mod supervisor;
pub mod suspend;

/// The 0.46.1 door's route tests (lane 2: S19, S20, S42, AR10).
#[cfg(test)]
mod door_tests;
