//! Remote desktop, the daemon side (`prd-remote-desktop-tab-v1`).
//!
//! - [`routes`]: `/cli/desktop/*` (the switch, policy, grants, viewers,
//!   kick, audit, open, tickets), D2. The truth is `k2_core::desktop`.
//! - [`deskd`]: the trait the routes call ([`deskd::NotBuilt`] until it is
//!   wired to [`deskd_link::manager`]), so every route runs headless.
//! - [`deskd_link`]: the link to k2-deskd ("K2 Screen"), D3.
//! - [`launch_agent`]: macOS's `dev.k2.deskd` LaunchAgent, D3.
//!
//! The stream (`/cli/desktop/stream`, D6) comes with K2 Render's engine.

pub mod deskd;
pub mod deskd_link;
pub mod launch_agent;
pub mod routes;

/// The `/boot-status` feature key: the routes exist (clients show + →
/// Desktop and the off card with it).
pub const FEATURE: &str = k2_core::desktop::FEATURE_KEY;
