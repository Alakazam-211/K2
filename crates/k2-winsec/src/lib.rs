//! Owner-only file ACLs on Windows.
//!
//! On Unix K2 makes its private directories 0700 and its private files
//! 0600, and refuses a directory another uid owns. Windows has no mode
//! bits; the equivalent is a *protected* DACL (no inherited entries) with
//! one entry: the current user, full control, inherited by everything
//! created inside. That is what [`set_owner_only`] writes. Administrators
//! can still take ownership, as root can read a 0600 file.
//!
//! Empty on Unix: callers keep their mode-bit code there unchanged.

#[cfg(windows)]
mod imp;
#[cfg(windows)]
pub use imp::{is_owner_only, owned_by_current_user, set_owner_only};
