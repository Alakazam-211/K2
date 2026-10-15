//! The App runtime's live parts (`prd-k2-app-runtime-v1` stage 1): the
//! `/cli/app/*` routes, the process engine, the supervisor (deploy
//! pipeline, watch window, auto-rollback, crash restart), the daemon-local
//! route override with drain, and agent alerts. The pure and SQLite parts
//! are `k2_core::deploy`.

pub mod alerts;
pub mod box_prep;
pub mod egress_proxy;
pub mod egress_routes;
pub mod engine_install;
pub mod guarded;
pub mod process_engine;
pub mod router;
pub mod routes;
pub mod supervisor;
