//! Remote desktop, the daemon side (`prd-remote-desktop-tab-v1` D2).
//!
//! - [`routes`]: `/cli/desktop/*` (the switch, policy, grants, viewers,
//!   kick, audit, open, tickets). The truth is `k2_core::desktop`.
//! - [`deskd`]: the `k2-deskd` link behind a trait ([`deskd::NotBuilt`]
//!   until D3), so every route runs headless.
//!
//! The stream (`/cli/desktop/stream`, D6) comes with K2 Render's engine.

pub mod deskd;
pub mod routes;

/// The `/boot-status` feature key: the routes exist (clients show + →
/// Desktop and the off card with it).
pub const FEATURE: &str = k2_core::desktop::FEATURE_KEY;
