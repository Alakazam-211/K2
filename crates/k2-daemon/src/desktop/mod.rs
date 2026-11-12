//! Remote desktop, the daemon side (`prd-remote-desktop-tab-v1`).
//!
//! - [`routes`]: `/cli/desktop/*` (the switch, policy, grants, viewers,
//!   kick, audit, open, tickets), D2. The truth is `k2_core::desktop`.
//! - [`deskd`]: the trait the routes call (tests install fakes, so every
//!   route runs headless; [`deskd::NotBuilt`] until boot installs
//!   [`managed::ManagedDeskd`]).
//! - [`deskd_link`]: the link to k2-deskd ("K2 Screen"), D3.
//! - [`launch_agent`]: macOS's `dev.k2.deskd` LaunchAgent, D3.
//! - [`stream`]: `/cli/desktop/stream` on K2 Render's engine, D6.
//! - [`managed`]: [`deskd::Deskd`] and [`stream::StreamDeskd`] over
//!   [`deskd_link::manager`], installed at boot.

pub mod deskd;
pub mod deskd_link;
pub mod launch_agent;
pub mod managed;
pub mod routes;
pub mod stream;
pub mod virtual_desktop;

/// The `/boot-status` feature key: the routes exist (clients show + →
/// Desktop and the off card with it).
pub const FEATURE: &str = k2_core::desktop::FEATURE_KEY;
